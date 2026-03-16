//! Cenzontle API response types for model listing and auth profile.

use serde::Deserialize;

/// A single model returned by `GET /v1/llm/models`.
#[derive(Debug, Deserialize)]
pub struct CenzontleModel {
    pub id: String,
    pub name: Option<String>,
    pub tier: Option<String>,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default = "default_true")]
    pub supports_streaming: bool,
    #[serde(default = "default_true")]
    pub supports_tools: bool,
    #[serde(default)]
    pub supports_vision: bool,
}

fn default_true() -> bool {
    true
}

/// Response from `GET /v1/llm/models`.
#[derive(Debug, Deserialize)]
pub struct CenzontleModelsResponse {
    pub data: Vec<CenzontleModel>,
}

/// Non-streaming response from `POST /v1/llm/chat` (stream=false).
///
/// Cenzontle buffers the full LLM response before returning, so non-streaming
/// mode is preferred over SSE to avoid issues with single-chunk HTTP/2 delivery.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CenzonzleChatResponse {
    pub content: String,
    pub model: Option<String>,
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
    #[serde(default)]
    pub total_tokens: Option<u32>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

/// Auth profile from `GET /v1/auth/me`.
#[derive(Debug, Deserialize)]
pub struct CenzontleAuthMe {
    pub sub: String,
    pub email: Option<String>,
    pub tenant_slug: Option<String>,
    pub role: Option<String>,
}
