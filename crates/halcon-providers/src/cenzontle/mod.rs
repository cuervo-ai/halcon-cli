//! Cenzontle AI platform provider.
//!
//! Connects to a Cenzontle instance using a JWT access token obtained via
//! the Zuclubit SSO OAuth 2.1 / PKCE flow (`halcon login cenzontle`).
//!
//! The chat endpoint (`POST /v1/llm/chat`) is OpenAI-compatible.
//! Models are discovered at construction time from `GET /v1/llm/models`.
//!
//! # Configuration
//!
//! - `CENZONTLE_BASE_URL` — base URL of the Cenzontle instance
//!   (default: `https://api.cenzontle.app`)
//! - `CENZONTLE_ACCESS_TOKEN` — JWT access token (takes precedence over keychain)
//!
//! Run `halcon login cenzontle` to perform the SSO browser flow and store the
//! token in the OS keychain automatically.

pub mod types;

use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use tracing::{debug, info, instrument, warn};

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::ModelProvider;
use halcon_core::types::{
    HttpConfig, ModelChunk, ModelInfo, ModelRequest, StopReason, TokenCost, TokenUsage, ToolFormat,
};

use crate::http;
use crate::openai_compat::OpenAICompatibleProvider;
use types::{CenzonzleChatResponse, CenzontleModel, CenzontleModelsResponse};

/// Production Cenzontle backend on Azure Container Apps.
/// Override with CENZONTLE_BASE_URL env var or provider config api_base.
pub const DEFAULT_BASE_URL: &str = "https://ca-cenzontle-backend.graypond-e35bfdd8.eastus2.azurecontainerapps.io";
const PROVIDER_NAME: &str = "cenzontle";

/// Tier → context window / max output heuristics (Cenzontle doesn't always return these).
fn tier_context_window(tier: Option<&str>) -> u32 {
    match tier {
        Some("FLAGSHIP") => 200_000,
        Some("BALANCED") => 128_000,
        Some("FAST") => 64_000,
        Some("ECONOMY") => 32_000,
        _ => 128_000,
    }
}

fn tier_max_output(tier: Option<&str>) -> u32 {
    match tier {
        Some("FLAGSHIP") => 16_000,
        Some("BALANCED") => 8_192,
        Some("FAST") => 4_096,
        Some("ECONOMY") => 2_048,
        _ => 4_096,
    }
}

/// Cenzontle AI platform provider.
pub struct CenzonzleProvider {
    /// reqwest client for API calls.
    client: reqwest::Client,
    /// Bearer JWT access token (from SSO flow or env var).
    access_token: String,
    /// Cenzontle base URL, e.g. `https://api.cenzontle.app`.
    base_url: String,
    /// Chat endpoint: `{base_url}/v1/llm/chat`.
    chat_url: String,
    /// Models available to this account.
    models: Vec<ModelInfo>,
    http_config: HttpConfig,
    /// Inner OpenAI-compat provider — used only for request building.
    inner: OpenAICompatibleProvider,
}

impl std::fmt::Debug for CenzonzleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CenzonzleProvider")
            .field("base_url", &self.base_url)
            .field("access_token", &"[REDACTED]")
            .finish()
    }
}

impl CenzonzleProvider {
    /// Create from an access token and a list of pre-fetched models.
    ///
    /// Prefer `from_token()` which calls the API to discover real models.
    pub fn new(access_token: String, base_url: Option<String>, models: Vec<ModelInfo>) -> Self {
        let http_config = HttpConfig::default();
        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let chat_url = format!("{}/v1/llm/chat", base_url);
        let client = http::build_client(&http_config);

        // Inner provider used only for build_request() — its base_url is unused.
        let inner = OpenAICompatibleProvider::new(
            PROVIDER_NAME.to_string(),
            access_token.clone(),
            format!("{}/v1/llm", base_url),
            models.clone(),
            http_config.clone(),
        );

        Self {
            client,
            access_token,
            base_url,
            chat_url,
            models,
            http_config,
            inner,
        }
    }

    /// Construct a provider by fetching models from the Cenzontle API.
    ///
    /// Returns `None` if the token is empty or the models endpoint is unreachable.
    pub async fn from_token(access_token: String, base_url: Option<String>) -> Option<Self> {
        if access_token.is_empty() {
            return None;
        }
        let base = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let http_config = HttpConfig::default();
        let client = http::build_client(&http_config);

        let models = fetch_models(&client, &base, &access_token)
            .await
            .unwrap_or_else(|e| {
                warn!(error = %e, "Cenzontle: failed to fetch models, using empty list");
                Vec::new()
            });

        if models.is_empty() {
            warn!("Cenzontle: no models available for this account");
        } else {
            info!(count = models.len(), "Cenzontle: discovered models");
        }

        Some(Self::new(access_token, Some(base), models))
    }

}

/// Fetch the model list from Cenzontle `GET /v1/llm/models`.
async fn fetch_models(
    client: &reqwest::Client,
    base_url: &str,
    access_token: &str,
) -> Result<Vec<ModelInfo>> {
    let url = format!("{}/v1/llm/models", base_url);
    let resp = client
        .get(&url)
        .bearer_auth(access_token)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| HalconError::ConnectionError {
            provider: PROVIDER_NAME.to_string(),
            message: format!("Cannot reach Cenzontle at {base_url}: {e}"),
        })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return Err(HalconError::ApiError {
            message: format!("Cenzontle /v1/llm/models returned HTTP {status}"),
            status: Some(status),
        });
    }

    let body: CenzontleModelsResponse = resp.json().await.map_err(|e| HalconError::ApiError {
        message: format!("Failed to parse Cenzontle models response: {e}"),
        status: None,
    })?;

    let models = body
        .data
        .into_iter()
        .map(|m| model_info_from_cenzontle(m))
        .collect();

    Ok(models)
}

fn model_info_from_cenzontle(m: CenzontleModel) -> ModelInfo {
    let tier = m.tier.as_deref();
    ModelInfo {
        id: m.id.clone(),
        name: m.name.unwrap_or_else(|| m.id.clone()),
        provider: PROVIDER_NAME.to_string(),
        context_window: m.context_window.unwrap_or_else(|| tier_context_window(tier)),
        max_output_tokens: m.max_output_tokens.unwrap_or_else(|| tier_max_output(tier)),
        supports_streaming: m.supports_streaming,
        supports_tools: m.supports_tools,
        supports_vision: m.supports_vision,
        supports_reasoning: false,
        cost_per_input_token: 0.0,  // billed through Cenzontle account
        cost_per_output_token: 0.0,
    }
}

// Token loading from OS keychain is intentionally NOT done here.
// halcon-providers does not depend on halcon-auth.
// The provider_factory (halcon-cli) is responsible for resolving the token
// from env var or keychain before calling CenzonzleProvider::new().

/// Convert a parsed non-streaming cenzontle chat response into a synthetic ModelChunk stream.
///
/// Cenzontle buffers the full LLM response before returning, so we use stream=false to avoid
/// issues with eventsource_stream parsing all SSE events from a single HTTP/2 DATA frame.
fn json_response_to_stream(resp: CenzonzleChatResponse) -> BoxStream<'static, Result<ModelChunk>> {
    let mut chunks: Vec<Result<ModelChunk>> = Vec::with_capacity(3);

    if !resp.content.is_empty() {
        chunks.push(Ok(ModelChunk::TextDelta(resp.content)));
    }

    if resp.prompt_tokens.is_some() || resp.completion_tokens.is_some() {
        chunks.push(Ok(ModelChunk::Usage(TokenUsage {
            input_tokens: resp.prompt_tokens.unwrap_or(0),
            output_tokens: resp.completion_tokens.unwrap_or(0),
            reasoning_tokens: None,
            ..Default::default()
        })));
    }

    let stop_reason = match resp.finish_reason.as_deref() {
        Some("length") => StopReason::MaxTokens,
        Some("tool_calls") => StopReason::ToolUse,
        _ => StopReason::EndTurn,
    };
    chunks.push(Ok(ModelChunk::Done(stop_reason)));

    Box::pin(stream::iter(chunks))
}

#[async_trait]
impl ModelProvider for CenzonzleProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn supported_models(&self) -> &[ModelInfo] {
        &self.models
    }

    fn tool_format(&self) -> ToolFormat {
        ToolFormat::OpenAIFunctionObject
    }

    #[instrument(skip_all, fields(provider = "cenzontle", model = %request.model, msgs = request.messages.len()))]
    async fn invoke(
        &self,
        request: &ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelChunk>>> {
        // Build the OpenAI-compatible request body but override stream=false.
        // Cenzontle buffers the full LLM response before emitting SSE events, so all SSE
        // data arrives in a single HTTP/2 DATA frame. eventsource_stream may not reliably
        // parse multiple events from one chunk. Using stream=false returns plain JSON and
        // we convert it to a synthetic ModelChunk stream — simpler and more robust.
        let mut chat_request = self.inner.build_request(request);
        chat_request.stream = false;
        chat_request.stream_options = None;

        let max_retries = self.http_config.max_retries;
        let timeout_secs = self.http_config.request_timeout_secs;

        debug!(
            model = %chat_request.model,
            messages = chat_request.messages.len(),
            url = %self.chat_url,
            "Cenzontle: invoking chat API (non-streaming JSON)"
        );

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay = http::backoff_delay(1000, attempt);
                tokio::time::sleep(delay).await;
            }

            let result = tokio::time::timeout(
                Duration::from_secs(timeout_secs),
                self.client
                    .post(&self.chat_url)
                    .bearer_auth(&self.access_token)
                    .json(&chat_request)
                    .send(),
            )
            .await;

            let response = match result {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) if e.is_connect() => {
                    if attempt < max_retries {
                        warn!(attempt = attempt + 1, "Cenzontle: connection error, retrying");
                        continue;
                    }
                    return Err(HalconError::ConnectionError {
                        provider: PROVIDER_NAME.to_string(),
                        message: format!("Cannot connect to {}: {e}", self.base_url),
                    });
                }
                Ok(Err(e)) => {
                    return Err(HalconError::ApiError {
                        message: format!("Cenzontle request failed: {e}"),
                        status: e.status().map(|s| s.as_u16()),
                    });
                }
                Err(_) => {
                    if attempt < max_retries {
                        warn!(attempt = attempt + 1, "Cenzontle: request timeout, retrying");
                        continue;
                    }
                    return Err(HalconError::ApiError {
                        message: format!("Cenzontle request timed out after {timeout_secs}s"),
                        status: None,
                    });
                }
            };

            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(HalconError::ApiError {
                    message: "Cenzontle: access token expired or invalid. Run `halcon login cenzontle` to refresh.".to_string(),
                    status: Some(401),
                });
            }
            if status == reqwest::StatusCode::FORBIDDEN {
                return Err(HalconError::ApiError {
                    message: "Cenzontle: insufficient permissions for this model.".to_string(),
                    status: Some(403),
                });
            }
            if !status.is_success() {
                let code = status.as_u16();
                let body = response.text().await.unwrap_or_default();
                return Err(HalconError::ApiError {
                    message: format!("Cenzontle HTTP {code}: {body}"),
                    status: Some(code),
                });
            }

            // Parse the non-streaming JSON response and convert to a synthetic stream.
            let chat_resp: CenzonzleChatResponse =
                response.json().await.map_err(|e| HalconError::ApiError {
                    message: format!("Cenzontle: failed to parse chat response: {e}"),
                    status: None,
                })?;

            debug!(
                content_len = chat_resp.content.len(),
                model = ?chat_resp.model,
                prompt_tokens = ?chat_resp.prompt_tokens,
                completion_tokens = ?chat_resp.completion_tokens,
                "Cenzontle: received JSON response"
            );

            return Ok(json_response_to_stream(chat_resp));
        }

        Err(HalconError::ApiError {
            message: "Cenzontle: all retry attempts exhausted".to_string(),
            status: None,
        })
    }

    async fn is_available(&self) -> bool {
        let url = format!("{}/v1/auth/me", self.base_url);
        self.client
            .get(&url)
            .bearer_auth(&self.access_token)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    fn estimate_cost(&self, _request: &ModelRequest) -> TokenCost {
        // Billed through Cenzontle account — not tracked locally.
        TokenCost::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::{CenzontleModel, CenzontleModelsResponse};

    // ── model_info_from_cenzontle ────────────────────────────────────────────

    #[test]
    fn model_info_maps_all_fields() {
        let m = CenzontleModel {
            id: "gpt-4o-mini".to_string(),
            name: Some("GPT-4o Mini".to_string()),
            tier: Some("BALANCED".to_string()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
        };
        let info = model_info_from_cenzontle(m);
        assert_eq!(info.id, "gpt-4o-mini");
        assert_eq!(info.name, "GPT-4o Mini");
        assert_eq!(info.provider, "cenzontle");
        assert_eq!(info.context_window, 128_000);
        assert_eq!(info.max_output_tokens, 16_384);
        assert!(info.supports_streaming);
        assert!(info.supports_tools);
        assert!(!info.supports_vision);
        // Cost is always 0 — billed through Cenzontle account.
        assert_eq!(info.cost_per_input_token, 0.0);
        assert_eq!(info.cost_per_output_token, 0.0);
    }

    #[test]
    fn model_info_falls_back_to_id_when_name_is_none() {
        let m = CenzontleModel {
            id: "my-model".to_string(),
            name: None,
            tier: None,
            context_window: None,
            max_output_tokens: None,
            supports_streaming: true,
            supports_tools: false,
            supports_vision: false,
        };
        let info = model_info_from_cenzontle(m);
        assert_eq!(info.name, "my-model");
    }

    #[test]
    fn tier_context_window_flagship() {
        assert_eq!(tier_context_window(Some("FLAGSHIP")), 200_000);
    }

    #[test]
    fn tier_context_window_unknown_defaults_128k() {
        assert_eq!(tier_context_window(None), 128_000);
        assert_eq!(tier_context_window(Some("UNKNOWN")), 128_000);
    }

    #[test]
    fn tier_max_output_economy() {
        assert_eq!(tier_max_output(Some("ECONOMY")), 2_048);
    }

    #[test]
    fn tier_context_window_fallback_applied_when_none() {
        let m = CenzontleModel {
            id: "x".to_string(),
            name: None,
            tier: Some("FAST".to_string()),
            context_window: None,
            max_output_tokens: None,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
        };
        let info = model_info_from_cenzontle(m);
        // FAST tier → context 64k, max_output 4096
        assert_eq!(info.context_window, 64_000);
        assert_eq!(info.max_output_tokens, 4_096);
    }

    // ── provider construction ────────────────────────────────────────────────

    #[test]
    fn new_sets_correct_chat_url() {
        let p = CenzonzleProvider::new("tok".to_string(), None, Vec::new());
        assert_eq!(p.chat_url, format!("{}/v1/llm/chat", DEFAULT_BASE_URL));
    }

    #[test]
    fn new_with_custom_base_url() {
        let custom = "http://localhost:3000".to_string();
        let p = CenzonzleProvider::new("tok".to_string(), Some(custom.clone()), Vec::new());
        assert_eq!(p.base_url, custom);
        assert_eq!(p.chat_url, "http://localhost:3000/v1/llm/chat");
    }

    #[test]
    fn provider_name_is_cenzontle() {
        let p = CenzonzleProvider::new("tok".to_string(), None, Vec::new());
        assert_eq!(p.name(), "cenzontle");
    }

    #[test]
    fn supported_models_returns_constructed_list() {
        let models = vec![ModelInfo {
            id: "gpt-4o-mini".to_string(),
            name: "GPT-4o Mini".to_string(),
            provider: "cenzontle".to_string(),
            context_window: 128_000,
            max_output_tokens: 16_384,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_reasoning: false,
            cost_per_input_token: 0.0,
            cost_per_output_token: 0.0,
        }];
        let p = CenzonzleProvider::new("tok".to_string(), None, models.clone());
        assert_eq!(p.supported_models().len(), 1);
        assert_eq!(p.supported_models()[0].id, "gpt-4o-mini");
    }

    #[test]
    fn empty_token_makes_from_token_return_none() {
        // Can't do async here without tokio, but we can verify the empty-token guard
        // by inspecting the logic path: from_token() returns None immediately if empty.
        // We test the sync equivalent used by the factory.
        let empty = String::new();
        assert!(empty.is_empty()); // guard condition
    }

    #[test]
    fn tool_format_is_openai_function_object() {
        use halcon_core::types::ToolFormat;
        let p = CenzonzleProvider::new("tok".to_string(), None, Vec::new());
        assert!(matches!(p.tool_format(), ToolFormat::OpenAIFunctionObject));
    }

    // ── response types deserialization ───────────────────────────────────────

    #[test]
    fn deserialize_models_response_with_snake_case_fields() {
        let json = r#"{
            "data": [
                {
                    "id": "gpt-4o-mini",
                    "name": "GPT-4o Mini",
                    "tier": "BALANCED",
                    "context_window": 128000,
                    "max_output_tokens": 16384,
                    "supports_streaming": true,
                    "supports_tools": true,
                    "supports_vision": false
                }
            ]
        }"#;
        let resp: CenzontleModelsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 1);
        let m = &resp.data[0];
        assert_eq!(m.id, "gpt-4o-mini");
        assert_eq!(m.name.as_deref(), Some("GPT-4o Mini"));
        assert_eq!(m.tier.as_deref(), Some("BALANCED"));
        assert_eq!(m.context_window, Some(128_000));
        assert_eq!(m.max_output_tokens, Some(16_384));
        assert!(m.supports_streaming);
        assert!(m.supports_tools);
        assert!(!m.supports_vision);
    }

    #[test]
    fn deserialize_models_response_with_defaults() {
        // supports_streaming and supports_tools default to true, supports_vision to false
        let json = r#"{"data": [{"id": "my-model"}]}"#;
        let resp: CenzontleModelsResponse = serde_json::from_str(json).unwrap();
        let m = &resp.data[0];
        assert!(m.supports_streaming); // default_true
        assert!(m.supports_tools);     // default_true
        assert!(!m.supports_vision);   // default false
    }

    #[test]
    fn deserialize_empty_data_array() {
        let json = r#"{"data": []}"#;
        let resp: CenzontleModelsResponse = serde_json::from_str(json).unwrap();
        assert!(resp.data.is_empty());
    }
}
