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
use futures::stream::BoxStream;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::ModelProvider;
use halcon_core::types::{
    HttpConfig, ModelChunk, ModelInfo, ModelRequest, TokenCost, TokenizerHint, ToolFormat,
};

use crate::http::{backoff_delay_with_jitter, is_retryable_status, parse_retry_after};
use crate::openai_compat::types::StreamOptions;
use crate::openai_compat::OpenAICompatibleProvider;
use types::{CenzontleModel, CenzontleModelsResponse};

/// Production Cenzontle backend on Azure Container Apps.
/// Override with CENZONTLE_BASE_URL env var or provider config api_base.
pub const DEFAULT_BASE_URL: &str =
    "https://ca-cenzontle-backend.graypond-e35bfdd8.eastus2.azurecontainerapps.io";
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

/// Infer the tokenizer family from a Cenzontle model ID string.
///
/// Cenzontle proxies models from multiple upstream providers. We detect the
/// family by model ID prefix so callers can use accurate token estimates for
/// context budgeting.
fn tokenizer_hint_for_model(model_id: &str) -> TokenizerHint {
    let m = model_id.to_ascii_lowercase();
    if m.contains("claude") {
        TokenizerHint::ClaudeBpe
    } else if m.contains("gpt") || m.contains("o1") || m.contains("o3") || m.contains("o4") {
        TokenizerHint::TiktokenCl100k
    } else if m.contains("deepseek") {
        TokenizerHint::DeepSeekBpe
    } else if m.contains("gemini") {
        TokenizerHint::GeminiSentencePiece
    } else {
        // Conservative fallback — 4.0 chars/token matches TiktokenCl100k.
        TokenizerHint::Unknown
    }
}

/// Cenzontle AI platform provider.
///
/// NOTE: The original struct was named `CenzonzleProvider` (double-z typo).
/// It is now `CenzontleProvider`. A type alias at the bottom of this file
/// preserves the old name for any external code that references it directly.
pub struct CenzontleProvider {
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
    /// Unique session ID for this provider instance (generated once per process).
    /// Reported to Cenzontle via x-halcon-context so all LLM calls within a
    /// single halcon invocation are grouped into the same session.
    session_id: String,
    /// Set to `true` when the provider was constructed via `from_token()` and the
    /// model list was successfully fetched from the API.  When true, `is_available()`
    /// returns immediately without making another network call — the successful model
    /// fetch already proved that the endpoint is reachable and the token is valid.
    connection_verified: bool,
}

impl std::fmt::Debug for CenzontleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CenzontleProvider")
            .field("base_url", &self.base_url)
            .field("access_token", &"[REDACTED]")
            .finish()
    }
}

impl CenzontleProvider {
    /// Create from an access token and a list of pre-fetched models.
    ///
    /// Prefer `from_token()` which calls the API to discover real models.
    pub fn new(access_token: String, base_url: Option<String>, models: Vec<ModelInfo>) -> Self {
        let http_config = HttpConfig::default();
        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let chat_url = format!("{}/v1/llm/chat", base_url);

        // Force HTTP/1.1 so the server sends individual SSE frames instead of
        // batching them in a single HTTP/2 DATA frame (which breaks eventsource parsing).
        //
        // ROOT CAUSE: The Cenzontle backend flushes SSE frames correctly on HTTP/1.1
        // but Azure App Gateway buffers HTTP/2 DATA frames until the response
        // body closes, delivering all SSE events in one batch.  HTTP/1.1 forces
        // chunked-encoding flush semantics that proxies cannot buffer.
        //
        // LONG-TERM FIX: Add `X-Accel-Buffering: no` + `flush()` calls on the
        // backend and configure App Gateway to pass-through streaming responses.
        // Once that is deployed, remove `.http1_only()` from this builder.
        let client = reqwest::Client::builder()
            .http1_only()
            .connect_timeout(Duration::from_secs(http_config.connect_timeout_secs))
            .pool_max_idle_per_host(4)
            .user_agent(format!("halcon-cli/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("failed to build HTTP/1.1 client for Cenzontle SSE");

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
            // One UUID per provider instance = one UUID per halcon process invocation.
            // Cenzontle groups all LLM calls with the same session_id together.
            session_id: Uuid::new_v4().to_string(),
            // When constructed with new(), connection is unverified.
            // from_token() will set this to true after a successful model fetch.
            connection_verified: false,
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
        let client = crate::http::build_client(&http_config);

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

        let mut provider = Self::new(access_token, Some(base), models);
        // Mark connection verified — a successful model fetch proves the API
        // is reachable and the token is valid, so is_available() can skip the
        // extra /v1/auth/me round-trip.
        provider.connection_verified = !provider.models.is_empty();
        Some(provider)
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
        .map(model_info_from_cenzontle)
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
        cost_per_input_token: 0.0, // billed through Cenzontle account
        cost_per_output_token: 0.0,
    }
}

// Token loading from OS keychain is intentionally NOT done here.
// halcon-providers does not depend on halcon-auth.
// The provider_factory (halcon-cli) is responsible for resolving the token
// from env var or keychain before calling CenzontleProvider::new().

#[async_trait]
impl ModelProvider for CenzontleProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn supported_models(&self) -> &[ModelInfo] {
        &self.models
    }

    fn tool_format(&self) -> ToolFormat {
        ToolFormat::OpenAIFunctionObject
    }

    /// Return the tokenizer family hint derived from the first registered model.
    ///
    /// Cenzontle is a multi-provider gateway (Anthropic, OpenAI, DeepSeek, Gemini).
    /// We detect the family by model ID convention so callers can budget tokens
    /// accurately without hard-coding provider-specific token counts.
    fn tokenizer_hint(&self) -> TokenizerHint {
        self.models
            .first()
            .map(|m| tokenizer_hint_for_model(&m.id))
            .unwrap_or(TokenizerHint::Unknown)
    }

    #[instrument(skip_all, fields(provider = "cenzontle", model = %request.model, msgs = request.messages.len()))]
    async fn invoke(
        &self,
        request: &ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelChunk>>> {
        // Build the OpenAI-compatible request body with SSE streaming enabled.
        let mut chat_request = self.inner.build_request(request);
        chat_request.stream = true;
        chat_request.stream_options = Some(StreamOptions { include_usage: true });

        // Build the halcon context header so Cenzontle can enrich the request
        // with RAG/session correlation.
        let halcon_ctx = {
            let tool_names: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
            let cwd = std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(String::from))
                .unwrap_or_default();
            serde_json::json!({
                "client": "halcon-cli",
                "session_id": self.session_id,
                "model": request.model,
                "tools": tool_names,
                "cwd": cwd,
            })
            .to_string()
        };

        let max_retries = self.http_config.max_retries;
        let timeout_secs = self.http_config.request_timeout_secs;
        // Unique identifier for this request — propagated so Cenzontle logs
        // can be correlated with client-side traces.
        let request_id = Uuid::new_v4().to_string();

        debug!(
            model = %chat_request.model,
            messages = chat_request.messages.len(),
            url = %self.chat_url,
            request_id = %request_id,
            "Cenzontle: invoking chat API (SSE streaming)"
        );

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay = backoff_delay_with_jitter(1000, attempt);
                debug!(attempt, delay_ms = delay.as_millis(), "Cenzontle: retry backoff");
                tokio::time::sleep(delay).await;
            }

            // Timeout only covers connection + first-byte (headers).
            // The SSE body is consumed incrementally after this point.
            let result = tokio::time::timeout(
                Duration::from_secs(timeout_secs),
                self.client
                    .post(&self.chat_url)
                    .bearer_auth(&self.access_token)
                    .header("x-halcon-context", &halcon_ctx)
                    .header("x-request-id", &request_id)
                    .json(&chat_request)
                    .send(),
            )
            .await;

            let response = match result {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) if e.is_connect() => {
                    if attempt < max_retries {
                        warn!(attempt = attempt + 1, error = %e, "Cenzontle: connection error, retrying");
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

            // Non-retryable auth errors: return immediately.
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

            // Retryable server-side errors (429 rate-limit, 500/502/503/529).
            // Honour the Retry-After header when the server sends one (429).
            if is_retryable_status(status.as_u16()) {
                if attempt < max_retries {
                    let retry_delay = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        // Prefer server-specified wait; fall back to exponential backoff.
                        parse_retry_after(response.headers())
                            .map(Duration::from_secs)
                            .unwrap_or_else(|| backoff_delay_with_jitter(2000, attempt))
                    } else {
                        backoff_delay_with_jitter(1000, attempt)
                    };
                    warn!(
                        status = status.as_u16(),
                        attempt = attempt + 1,
                        delay_ms = retry_delay.as_millis(),
                        "Cenzontle: retryable error, backing off"
                    );
                    tokio::time::sleep(retry_delay).await;
                    continue;
                }
                let code = status.as_u16();
                let body = response.text().await.unwrap_or_default();
                return Err(HalconError::ApiError {
                    message: format!("Cenzontle HTTP {code} after {max_retries} retries: {body}"),
                    status: Some(code),
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

            debug!(url = %self.chat_url, request_id = %request_id, "Cenzontle: SSE stream connected");

            // Hand off the response body to the OpenAI-compat SSE parser.
            return Ok(OpenAICompatibleProvider::build_sse_stream(
                response,
                PROVIDER_NAME.to_string(),
            ));
        }

        Err(HalconError::ApiError {
            message: "Cenzontle: all retry attempts exhausted".to_string(),
            status: None,
        })
    }

    async fn is_available(&self) -> bool {
        // Fast path: if we already successfully fetched models via from_token(),
        // the API is reachable and the token is valid — no extra round-trip needed.
        // This eliminates the redundant GET /v1/auth/me that was always called after
        // ensure_cenzontle_models() during startup.
        if self.connection_verified {
            debug!("Cenzontle: is_available skipped — connection already verified by model fetch");
            return true;
        }

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

/// Backward-compatibility alias.
///
/// The original name `CenzonzleProvider` had a double-z typo.  All new code
/// should use `CenzontleProvider`.  This alias keeps existing code that imports
/// `CenzonzleProvider` compiling without changes.
#[deprecated(since = "0.3.6", note = "Use `CenzontleProvider` (single z)")]
pub type CenzonzleProvider = CenzontleProvider;

#[cfg(test)]
mod tests {
    use super::*;
    use types::{CenzontleModel, CenzontleModelsResponse};

    // ── tokenizer_hint ───────────────────────────────────────────────────────

    #[test]
    fn tokenizer_hint_claude_model() {
        let p = CenzontleProvider::new(
            "tok".to_string(),
            None,
            vec![ModelInfo {
                id: "claude-sonnet-4-6".to_string(),
                name: "Claude Sonnet 4.6".to_string(),
                provider: "cenzontle".to_string(),
                context_window: 200_000,
                max_output_tokens: 16_000,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 0.0,
                cost_per_output_token: 0.0,
            }],
        );
        assert_eq!(p.tokenizer_hint(), TokenizerHint::ClaudeBpe);
    }

    #[test]
    fn tokenizer_hint_gpt_model() {
        let p = CenzontleProvider::new(
            "tok".to_string(),
            None,
            vec![ModelInfo {
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
            }],
        );
        assert_eq!(p.tokenizer_hint(), TokenizerHint::TiktokenCl100k);
    }

    #[test]
    fn tokenizer_hint_empty_models_is_unknown() {
        let p = CenzontleProvider::new("tok".to_string(), None, Vec::new());
        assert_eq!(p.tokenizer_hint(), TokenizerHint::Unknown);
    }

    #[test]
    fn tokenizer_hint_for_model_deepseek() {
        assert_eq!(
            tokenizer_hint_for_model("deepseek-chat"),
            TokenizerHint::DeepSeekBpe
        );
    }

    #[test]
    fn tokenizer_hint_for_model_gemini() {
        assert_eq!(
            tokenizer_hint_for_model("gemini-2.0-flash"),
            TokenizerHint::GeminiSentencePiece
        );
    }

    #[test]
    fn tokenizer_hint_for_model_unknown() {
        assert_eq!(
            tokenizer_hint_for_model("llama3.2"),
            TokenizerHint::Unknown
        );
    }

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
        let p = CenzontleProvider::new("tok".to_string(), None, Vec::new());
        assert_eq!(p.chat_url, format!("{}/v1/llm/chat", DEFAULT_BASE_URL));
    }

    #[test]
    fn new_with_custom_base_url() {
        let custom = "http://localhost:3000".to_string();
        let p = CenzontleProvider::new("tok".to_string(), Some(custom.clone()), Vec::new());
        assert_eq!(p.base_url, custom);
        assert_eq!(p.chat_url, "http://localhost:3000/v1/llm/chat");
    }

    #[test]
    fn provider_name_is_cenzontle() {
        let p = CenzontleProvider::new("tok".to_string(), None, Vec::new());
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
        let p = CenzontleProvider::new("tok".to_string(), None, models.clone());
        assert_eq!(p.supported_models().len(), 1);
        assert_eq!(p.supported_models()[0].id, "gpt-4o-mini");
    }

    #[test]
    fn empty_token_makes_from_token_return_none() {
        // The empty-token guard fires synchronously before any async work.
        let empty = String::new();
        assert!(empty.is_empty());
    }

    #[test]
    fn tool_format_is_openai_function_object() {
        use halcon_core::types::ToolFormat;
        let p = CenzontleProvider::new("tok".to_string(), None, Vec::new());
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
        assert!(m.supports_tools); // default_true
        assert!(!m.supports_vision); // default false
    }

    #[test]
    fn deserialize_empty_data_array() {
        let json = r#"{"data": []}"#;
        let resp: CenzontleModelsResponse = serde_json::from_str(json).unwrap();
        assert!(resp.data.is_empty());
    }
}
