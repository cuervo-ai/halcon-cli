// DECISION: BedrockProvider wraps the Anthropic Messages API format
// but routes through AWS Bedrock's InvokeModelWithResponseStream endpoint.
// We reuse existing AnthropicRequest/Response types from anthropic.rs
// to avoid a parallel type hierarchy — the only difference is the URL
// and the Authorization header (AWS SigV4 vs Bearer token).
//
// Bedrock endpoint pattern:
//   POST https://bedrock-runtime.{region}.amazonaws.com/model/{modelId}/invoke-with-response-stream
//
// Cross-region inference prefixes (us., eu., ap.) are passed verbatim in the model ID.
// See US-bedrock (PASO 2-B).

pub mod auth;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use tracing::{debug, info, warn};

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::ModelProvider;
use halcon_core::types::{
    HttpConfig, ModelChunk, ModelInfo, ModelRequest, TokenCost, TokenUsage, StopReason,
};

use crate::anthropic::types::{ApiRequest, SseEvent};
use crate::http;

use auth::AwsCredentials;

/// AWS Bedrock provider for Claude models.
///
/// Routes Anthropic-format requests through Bedrock's streaming endpoint
/// with SigV4 request signing.
///
/// Activated when `CLAUDE_CODE_USE_BEDROCK=1` environment variable is set.
pub struct BedrockProvider {
    client: reqwest::Client,
    credentials: AwsCredentials,
    base_url: String,
    http_config: HttpConfig,
    models: Vec<ModelInfo>,
}

impl std::fmt::Debug for BedrockProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BedrockProvider")
            .field("base_url", &self.base_url)
            .field("region", &self.credentials.region)
            .field("access_key_id", &"[REDACTED]")
            .finish()
    }
}

impl BedrockProvider {
    /// Build supported models list.
    ///
    /// Cross-region prefix variants (us.*/eu.*/ap.*) are included because
    /// Bedrock cross-region inference profiles use these IDs.
    fn default_models() -> Vec<ModelInfo> {
        let base = vec![
            ("anthropic.claude-sonnet-4-6", "Claude Sonnet 4.6 (Bedrock)"),
            ("anthropic.claude-sonnet-4-5-20250929-v1:0", "Claude Sonnet 4.5 (Bedrock)"),
            ("anthropic.claude-haiku-4-5-20251001-v1:0", "Claude Haiku 4.5 (Bedrock)"),
            ("anthropic.claude-opus-4-6", "Claude Opus 4.6 (Bedrock)"),
            // Cross-region inference profile IDs
            ("us.anthropic.claude-sonnet-4-6", "Claude Sonnet 4.6 US (Bedrock)"),
            ("eu.anthropic.claude-sonnet-4-6", "Claude Sonnet 4.6 EU (Bedrock)"),
            ("ap.anthropic.claude-sonnet-4-6", "Claude Sonnet 4.6 AP (Bedrock)"),
        ];
        base.into_iter().map(|(id, name)| ModelInfo {
            id: id.into(),
            name: name.into(),
            provider: "bedrock".into(),
            context_window: 200_000,
            max_output_tokens: 8192,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            supports_reasoning: false,
            cost_per_input_token: 3.0 / 1_000_000.0,
            cost_per_output_token: 15.0 / 1_000_000.0,
        }).collect()
    }

    /// Construct from AWS environment variables.
    ///
    /// Returns `None` if required env vars are missing.
    pub fn from_env() -> Option<Arc<Self>> {
        let creds = AwsCredentials::from_env()?;
        let base_url = creds.bedrock_base_url();
        let http_config = HttpConfig::default();
        Some(Arc::new(Self {
            client: http::build_client(&http_config),
            credentials: creds,
            base_url,
            http_config,
            models: Self::default_models(),
        }))
    }

    /// Build the Bedrock invoke URL for a given model.
    ///
    /// Format: {base_url}/model/{modelId}/invoke-with-response-stream
    fn invoke_url(&self, model: &str) -> String {
        format!("{}/model/{}/invoke-with-response-stream", self.base_url, model)
    }

    /// Build base HTTP headers (Content-Type only; SigV4 adds Authorization).
    fn build_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "accept",
            HeaderValue::from_static("application/vnd.amazon.eventstream"),
        );
        headers
    }

    /// Build the SSE stream from the Bedrock response.
    ///
    /// Bedrock uses the same SSE format as the direct Anthropic API for
    /// Claude models, so we reuse the same SSE event parsing.
    fn build_sse_stream(response: reqwest::Response) -> BoxStream<'static, Result<ModelChunk>> {
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
                            Ok(sse_event) => crate::anthropic::AnthropicProvider::map_sse_event_pub(&sse_event)
                                .into_iter()
                                .map(Ok)
                                .collect(),
                            Err(e) => {
                                warn!(data = %event.data, error = %e, "bedrock: failed to parse SSE event");
                                vec![]
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "bedrock: SSE stream error");
                    vec![Ok(ModelChunk::Error(format!("Bedrock SSE error: {e}")))]
                }
            };
            futures::stream::iter(chunks)
        }))
    }
}

#[async_trait]
impl ModelProvider for BedrockProvider {
    fn name(&self) -> &str {
        "bedrock"
    }

    fn supported_models(&self) -> &[ModelInfo] {
        &self.models
    }

    async fn invoke(
        &self,
        request: &ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelChunk>>> {
        use crate::anthropic::AnthropicProvider;

        let api_request = AnthropicProvider::build_api_request_pub(request);
        let url = self.invoke_url(&api_request.model);
        let headers = Self::build_headers();

        debug!(
            model = %api_request.model,
            url = %url,
            "bedrock: invoking model"
        );

        let body_bytes: bytes::Bytes = serde_json::to_string(&api_request)
            .map_err(|e| HalconError::ApiError {
                message: format!("bedrock: failed to serialize request: {e}"),
                status: None,
            })?
            .into();

        let timeout = Duration::from_secs(self.http_config.request_timeout_secs);
        let max_retries = self.http_config.max_retries;
        let base_delay = self.http_config.retry_base_delay_ms;

        let mut last_error: Option<HalconError> = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay = http::backoff_delay(base_delay, attempt - 1);
                tokio::time::sleep(delay).await;
            }

            // Build a fresh request each attempt (SigV4 includes a timestamp).
            let mut req = self.client
                .post(&url)
                .headers(headers.clone())
                .body(body_bytes.clone())
                .build()
                .map_err(|e| HalconError::ApiError {
                    message: format!("bedrock: failed to build request: {e}"),
                    status: None,
                })?;

            // Sign the request with SigV4.
            auth::sign_request(&mut req, &self.credentials, "bedrock")
                .map_err(|e| HalconError::ApiError {
                    message: format!("bedrock: SigV4 signing failed: {e}"),
                    status: None,
                })?;

            let response = match tokio::time::timeout(timeout, self.client.execute(req)).await {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => {
                    let err = HalconError::ConnectionError {
                        provider: "bedrock".into(),
                        message: format!("{e}"),
                    };
                    if attempt < max_retries {
                        warn!(attempt, error = %e, "bedrock: request failed, retrying");
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
                Err(_) => {
                    let err = HalconError::RequestTimeout {
                        provider: "bedrock".into(),
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
                info!(model = %api_request.model, attempts = attempt + 1, "bedrock: stream established");
                return Ok(Self::build_sse_stream(response));
            }

            // 401 — credentials invalid or expired
            if status.as_u16() == 401 {
                return Err(HalconError::ApiError {
                    message: "Bedrock: unauthorized. Check AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, and AWS_REGION.".into(),
                    status: Some(401),
                });
            }

            // Retryable: 429 (throttling), 5xx
            let retryable = status.as_u16() == 429 || status.is_server_error();
            let body = response.text().await.unwrap_or_default();
            let err = HalconError::ApiError {
                message: format!("Bedrock HTTP {}: {}", status.as_u16(), body),
                status: Some(status.as_u16()),
            };
            if retryable && attempt < max_retries {
                warn!(attempt, status = %status, "bedrock: retryable error");
                last_error = Some(err);
                continue;
            }
            return Err(err);
        }

        Err(last_error.unwrap_or_else(|| HalconError::ApiError {
            message: "bedrock: exhausted retries".into(),
            status: None,
        }))
    }

    async fn is_available(&self) -> bool {
        // Bedrock is available if credentials are present.
        // We do NOT make a network call here (too slow for startup checks).
        !self.credentials.access_key_id.is_empty()
    }

    fn estimate_cost(&self, request: &ModelRequest) -> TokenCost {
        use crate::anthropic::AnthropicProvider;
        // Reuse Anthropic cost estimation — same pricing through Bedrock.
        AnthropicProvider::estimate_cost_pub(request)
    }

    fn validate_model(&self, model: &str) -> halcon_core::error::Result<()> {
        // Accept any model that starts with "anthropic." or a cross-region prefix.
        // This is permissive because new Bedrock model IDs are added frequently.
        if model.starts_with("anthropic.")
            || model.starts_with("us.anthropic.")
            || model.starts_with("eu.anthropic.")
            || model.starts_with("ap.anthropic.")
        {
            return Ok(());
        }
        Err(HalconError::ModelNotFound {
            provider: "bedrock".into(),
            model: model.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invoke_url_format() {
        let creds = AwsCredentials {
            access_key_id: "AKID".into(),
            secret_access_key: "SECRET".into(),
            session_token: None,
            region: "us-east-1".into(),
        };
        let base_url = "https://bedrock-runtime.us-east-1.amazonaws.com".to_string();
        let http_config = HttpConfig::default();
        let provider = BedrockProvider {
            client: http::build_client(&http_config),
            credentials: creds,
            base_url,
            http_config,
            models: BedrockProvider::default_models(),
        };
        let url = provider.invoke_url("anthropic.claude-sonnet-4-6");
        assert!(url.contains("/model/anthropic.claude-sonnet-4-6/invoke-with-response-stream"));
    }

    #[test]
    fn validate_model_accepts_anthropic_prefix() {
        let p = BedrockProvider {
            client: reqwest::Client::new(),
            credentials: AwsCredentials {
                access_key_id: "A".into(),
                secret_access_key: "S".into(),
                session_token: None,
                region: "us-east-1".into(),
            },
            base_url: String::new(),
            http_config: HttpConfig::default(),
            models: vec![],
        };
        assert!(p.validate_model("anthropic.claude-sonnet-4-6").is_ok());
        assert!(p.validate_model("us.anthropic.claude-sonnet-4-6").is_ok());
        assert!(p.validate_model("eu.anthropic.claude-opus-4-6").is_ok());
        assert!(p.validate_model("openai.gpt-4o").is_err());
    }

    #[test]
    fn provider_name_is_bedrock() {
        let p = BedrockProvider {
            client: reqwest::Client::new(),
            credentials: AwsCredentials {
                access_key_id: "A".into(),
                secret_access_key: "S".into(),
                session_token: None,
                region: "us-east-1".into(),
            },
            base_url: String::new(),
            http_config: HttpConfig::default(),
            models: vec![],
        };
        assert_eq!(p.name(), "bedrock");
    }
}
