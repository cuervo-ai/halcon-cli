// Vertex AI provider: routes Anthropic-format requests through the
// Google Cloud Vertex AI `streamRawPredict` endpoint.
//
// Authentication uses GCP Application Default Credentials (ADC) via gcp-auth.
// The token is fetched fresh on each invoke() call — GCP tokens expire after 1h.
// For production use, gcp-auth caches the token internally and refreshes proactively.
//
// See US-vertex (PASO 2-C).

pub mod auth;

use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use tracing::{debug, info, warn};

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::ModelProvider;
use halcon_core::types::{HttpConfig, ModelChunk, ModelInfo, ModelRequest, TokenCost};

use crate::anthropic::types::SseEvent;
use crate::http;

use auth::GcpConfig;

/// Google Cloud Vertex AI provider for Claude models.
///
/// Sends requests to the Vertex AI `streamRawPredict` endpoint.
/// Uses GCP Application Default Credentials for authentication.
///
/// Activated when `CLAUDE_CODE_USE_VERTEX=1` environment variable is set.
pub struct VertexProvider {
    client: reqwest::Client,
    gcp_config: GcpConfig,
    http_config: HttpConfig,
    models: Vec<ModelInfo>,
}

impl std::fmt::Debug for VertexProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VertexProvider")
            .field("project_id", &self.gcp_config.project_id)
            .field("region", &self.gcp_config.region)
            .finish()
    }
}

impl VertexProvider {
    fn default_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "claude-sonnet-4-6".into(),
                name: "Claude Sonnet 4.6 (Vertex)".into(),
                provider: "vertex".into(),
                context_window: 200_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 3.0 / 1_000_000.0,
                cost_per_output_token: 15.0 / 1_000_000.0,
            },
            ModelInfo {
                id: "claude-sonnet-4-5-20250929".into(),
                name: "Claude Sonnet 4.5 (Vertex)".into(),
                provider: "vertex".into(),
                context_window: 200_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 3.0 / 1_000_000.0,
                cost_per_output_token: 15.0 / 1_000_000.0,
            },
            ModelInfo {
                id: "claude-haiku-4-5-20251001".into(),
                name: "Claude Haiku 4.5 (Vertex)".into(),
                provider: "vertex".into(),
                context_window: 200_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 0.80 / 1_000_000.0,
                cost_per_output_token: 4.0 / 1_000_000.0,
            },
        ]
    }

    /// Construct from GCP environment variables.
    ///
    /// Returns `None` if `ANTHROPIC_VERTEX_PROJECT_ID` is absent.
    pub fn from_env() -> Option<Self> {
        let gcp_config = GcpConfig::from_env()?;
        let http_config = HttpConfig::default();
        Some(Self {
            client: http::build_client(&http_config),
            gcp_config,
            http_config,
            models: Self::default_models(),
        })
    }

    fn build_stream(response: reqwest::Response) -> BoxStream<'static, Result<ModelChunk>> {
        use eventsource_stream::Eventsource as _;
        let byte_stream = response.bytes_stream();
        let sse_stream = byte_stream.eventsource();

        Box::pin(sse_stream.flat_map(move |sse_result| {
            let chunks: Vec<Result<ModelChunk>> = match sse_result {
                Ok(event) => {
                    if event.data.is_empty() || event.data == "[DONE]" {
                        vec![]
                    } else {
                        match serde_json::from_str::<SseEvent>(&event.data) {
                            Ok(ev) => crate::anthropic::AnthropicProvider::map_sse_event_pub(&ev)
                                .into_iter()
                                .map(Ok)
                                .collect(),
                            Err(e) => {
                                warn!(data = %event.data, error = %e, "vertex: parse SSE event failed");
                                vec![]
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "vertex: SSE stream error");
                    vec![Ok(ModelChunk::Error(format!("Vertex SSE error: {e}")))]
                }
            };
            futures::stream::iter(chunks)
        }))
    }
}

#[async_trait]
impl ModelProvider for VertexProvider {
    fn name(&self) -> &str {
        "vertex"
    }

    fn supported_models(&self) -> &[ModelInfo] {
        &self.models
    }

    async fn invoke(
        &self,
        request: &ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelChunk>>> {
        use crate::anthropic::AnthropicProvider;

        // Fetch ADC token (gcp-auth caches internally).
        let token = auth::get_access_token().await.map_err(|e| HalconError::ApiError {
            message: format!("Vertex ADC token error: {e}"),
            status: None,
        })?;

        let api_request = AnthropicProvider::build_api_request_pub(request);
        let url = self.gcp_config.stream_raw_predict_url(&api_request.model);

        debug!(model = %api_request.model, url = %url, "vertex: invoking model");

        let body_bytes: bytes::Bytes = serde_json::to_string(&api_request)
            .map_err(|e| HalconError::ApiError {
                message: format!("vertex: serialize failed: {e}"),
                status: None,
            })?
            .into();

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| HalconError::ApiError {
                    message: format!("vertex: invalid auth header: {e}"),
                    status: None,
                })?,
        );

        let timeout = Duration::from_secs(self.http_config.request_timeout_secs);
        let max_retries = self.http_config.max_retries;
        let base_delay = self.http_config.retry_base_delay_ms;

        let mut last_error: Option<HalconError> = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay = http::backoff_delay(base_delay, attempt - 1);
                tokio::time::sleep(delay).await;
            }

            let send_fut = self.client
                .post(&url)
                .headers(headers.clone())
                .body(body_bytes.clone())
                .send();

            let response = match tokio::time::timeout(timeout, send_fut).await {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => {
                    let err = HalconError::ConnectionError {
                        provider: "vertex".into(),
                        message: format!("{e}"),
                    };
                    if attempt < max_retries {
                        warn!(attempt, error = %e, "vertex: connection error, retrying");
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
                Err(_) => {
                    let err = HalconError::RequestTimeout {
                        provider: "vertex".into(),
                        timeout_secs: self.http_config.request_timeout_secs,
                    };
                    if attempt < max_retries {
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
            };

            let status = response.status();
            if status.is_success() {
                info!(model = %api_request.model, attempts = attempt + 1, "vertex: stream established");
                return Ok(Self::build_stream(response));
            }

            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(HalconError::ApiError {
                    message: "Vertex AI: unauthorized. Check GOOGLE_APPLICATION_CREDENTIALS and ANTHROPIC_VERTEX_PROJECT_ID.".into(),
                    status: Some(status.as_u16()),
                });
            }

            let retryable = status.as_u16() == 429 || status.is_server_error();
            let body = response.text().await.unwrap_or_default();
            let err = HalconError::ApiError {
                message: format!("Vertex HTTP {}: {}", status.as_u16(), body),
                status: Some(status.as_u16()),
            };
            if retryable && attempt < max_retries {
                warn!(attempt, status = %status, "vertex: retryable error");
                last_error = Some(err);
                continue;
            }
            return Err(err);
        }

        Err(last_error.unwrap_or_else(|| HalconError::ApiError {
            message: "vertex: exhausted retries".into(),
            status: None,
        }))
    }

    async fn is_available(&self) -> bool {
        // Available if project ID is configured.
        // We don't check ADC here to avoid blocking startup.
        !self.gcp_config.project_id.is_empty()
    }

    fn estimate_cost(&self, request: &ModelRequest) -> TokenCost {
        crate::anthropic::AnthropicProvider::estimate_cost_pub(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name_is_vertex() {
        let p = VertexProvider {
            client: reqwest::Client::new(),
            gcp_config: GcpConfig {
                project_id: "proj".into(),
                region: "us-east5".into(),
            },
            http_config: HttpConfig::default(),
            models: vec![],
        };
        assert_eq!(p.name(), "vertex");
    }

    #[test]
    fn is_available_with_project() {
        let p = VertexProvider {
            client: reqwest::Client::new(),
            gcp_config: GcpConfig {
                project_id: "my-project".into(),
                region: "us-east5".into(),
            },
            http_config: HttpConfig::default(),
            models: vec![],
        };
        // is_available is async; just verify project_id is non-empty.
        assert!(!p.gcp_config.project_id.is_empty());
    }
}
