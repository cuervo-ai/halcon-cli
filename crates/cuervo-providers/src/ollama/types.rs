//! Ollama API types for chat completion.
//!
//! These types serialize to / deserialize from the Ollama REST API
//! (`/api/chat`). Responses arrive as newline-delimited JSON (NDJSON),
//! one JSON object per line.

use serde::{Deserialize, Serialize};

/// Request body for `POST /api/chat`.
#[derive(Debug, Clone, Serialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub messages: Vec<OllamaMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<OllamaOptions>,
}

/// A single message in an Ollama conversation.
#[derive(Debug, Clone, Serialize)]
pub struct OllamaMessage {
    pub role: String,
    pub content: String,
}

/// Optional model parameters.
#[derive(Debug, Clone, Serialize)]
pub struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<u32>,
}

/// A single NDJSON line from the streaming `/api/chat` response.
#[derive(Debug, Deserialize)]
pub struct OllamaChatResponse {
    /// The partial message content (present on every line).
    #[serde(default)]
    pub message: OllamaResponseMessage,
    /// `true` on the final line of the stream.
    #[serde(default)]
    pub done: bool,
    /// Total prompt tokens (only present on final line).
    #[serde(default)]
    pub prompt_eval_count: Option<u32>,
    /// Total completion tokens (only present on final line).
    #[serde(default)]
    pub eval_count: Option<u32>,
}

/// Message field inside the streaming response.
#[derive(Debug, Default, Deserialize)]
pub struct OllamaResponseMessage {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: String,
}

/// Response from `GET /api/tags` (health check).
#[derive(Debug, Deserialize)]
pub struct OllamaTagsResponse {
    #[serde(default)]
    pub models: Vec<OllamaTagModel>,
}

/// A single model entry from `/api/tags`.
#[derive(Debug, Deserialize)]
pub struct OllamaTagModel {
    pub name: String,
}
