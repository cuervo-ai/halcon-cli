use serde::{Deserialize, Serialize};

/// Information about a model available through a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    #[serde(default)]
    pub supports_reasoning: bool,
    pub cost_per_input_token: f64,
    pub cost_per_output_token: f64,
}

/// A request to a model provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub stream: bool,
}

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: MessageContent,
}

/// Message content: either plain text or structured blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContent::Text(s) => Some(s),
            _ => None,
        }
    }
}

/// Supported image media types (detected via magic bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ImageMediaType {
    Jpeg,
    Png,
    Webp,
    Gif,
}

impl ImageMediaType {
    /// Detect image type from the first bytes of file data.
    pub fn from_magic(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(Self::Jpeg);
        }
        if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
            return Some(Self::Png);
        }
        if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
            return Some(Self::Webp);
        }
        if bytes.starts_with(b"GIF8") {
            return Some(Self::Gif);
        }
        None
    }

    /// Return the canonical MIME type string.
    pub fn as_mime_str(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
            Self::Gif => "image/gif",
        }
    }
}

/// Source of image data for multimodal requests.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ImageSource {
    /// Base64-encoded image data with known media type.
    Base64 {
        media_type: ImageMediaType,
        data: String,
    },
    /// A URL pointing to an image (not supported by all providers).
    Url { url: String },
    /// A local filesystem path (must be resolved before API use).
    LocalPath { path: String },
}

/// A content block within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// An image block (for vision-capable models).
    #[serde(rename = "image")]
    Image { source: ImageSource },
    /// The result of audio transcription.
    #[serde(rename = "audio_transcript")]
    AudioTranscript {
        text: String,
        duration_secs: Option<f32>,
        confidence: Option<f32>,
    },
}

/// Conversation role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

/// Tool definition for model API calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// A chunk from a streaming model response.
#[derive(Debug, Clone)]
pub enum ModelChunk {
    /// A text delta (visible answer tokens).
    TextDelta(String),
    /// Chain-of-thought / thinking tokens from reasoning models (deepseek-reasoner, o1, o3-mini).
    ///
    /// Distinct from `TextDelta` — represents the model's internal reasoning process.
    /// Rendered with visual distinction (dim/italic) but NOT accumulated into episodic memory.
    ThinkingDelta(String),
    /// A tool use content block has started (emitted on content_block_start).
    ToolUseStart {
        index: u32,
        id: String,
        name: String,
    },
    /// A partial JSON delta for tool input (emitted on input_json_delta).
    ToolUseDelta { index: u32, partial_json: String },
    /// A fully assembled tool use (produced by the accumulator, not directly by the provider).
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Usage information (may arrive at end of stream).
    Usage(TokenUsage),
    /// Stream completed with a stop reason.
    Done(StopReason),
    /// An error occurred during streaming.
    Error(String),
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    StopSequence,
}

/// Token usage statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
    pub cache_creation_tokens: Option<u32>,
    /// Tokens consumed by chain-of-thought reasoning (deepseek-reasoner, o1, o3-mini).
    /// Billed as output tokens; tracked separately for cost transparency.
    #[serde(default)]
    pub reasoning_tokens: Option<u32>,
}

impl TokenUsage {
    pub fn total(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }
}

/// Estimated cost for a model request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenCost {
    pub estimated_input_tokens: u32,
    pub estimated_cost_usd: f64,
}

/// Look up the context window size (in tokens) for a model by name.
///
/// Returns the well-known context window for common models, or a default
/// for unknown models. This is the single source of truth for context window
/// sizes — avoids the 200K hardcoding scattered across the codebase.
///
/// The returned value represents the TOTAL context window (input + output).
/// Pipeline budget should derive from this (e.g., 80% for input headroom).
pub fn model_context_window(model: &str) -> u32 {
    let m = model.to_lowercase();

    // Anthropic Claude family
    if m.contains("claude-opus") || m.contains("claude-sonnet") || m.contains("claude-4") {
        return 200_000;
    }
    if m.contains("claude-haiku") {
        return 200_000;
    }
    if m.contains("claude-3") {
        return 200_000;
    }

    // OpenAI
    if m.contains("gpt-4o") || m.contains("gpt-4-turbo") {
        return 128_000;
    }
    if m.contains("gpt-4") && !m.contains("turbo") {
        return 8_192;
    }
    if m.contains("o1") || m.contains("o3") || m.contains("o4") {
        return 200_000;
    }

    // DeepSeek
    if m.contains("deepseek") {
        return 64_000;
    }

    // Google Gemini
    if m.contains("gemini-2") || m.contains("gemini-1.5-pro") {
        return 1_000_000;
    }
    if m.contains("gemini") {
        return 128_000;
    }

    // Ollama / local models — conservative default
    if m.contains("llama") || m.contains("mistral") || m.contains("qwen") || m.contains("phi") {
        return 32_000;
    }

    // Default: conservative value for unknown models
    128_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_delta_variant_exists() {
        // Verify ThinkingDelta is a distinct ModelChunk variant (not aliased to TextDelta).
        let chunk = ModelChunk::ThinkingDelta("test reasoning".into());
        match chunk {
            ModelChunk::ThinkingDelta(t) => assert_eq!(t, "test reasoning"),
            _ => panic!("Expected ThinkingDelta"),
        }
    }

    #[test]
    fn token_usage_reasoning_tokens_defaults_to_none() {
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        };
        assert!(usage.reasoning_tokens.is_none());
    }

    #[test]
    fn token_usage_reasoning_tokens_can_be_set() {
        let usage = TokenUsage {
            input_tokens: 50,
            output_tokens: 1500,
            reasoning_tokens: Some(1200),
            ..Default::default()
        };
        assert_eq!(usage.reasoning_tokens, Some(1200));
        // reasoning_tokens are billed as output_tokens — total() reflects the full bill.
        assert_eq!(usage.total(), 1550);
    }

    #[test]
    fn token_usage_serde_roundtrip_with_reasoning() {
        let original = TokenUsage {
            input_tokens: 100,
            output_tokens: 800,
            reasoning_tokens: Some(600),
            cache_read_tokens: None,
            cache_creation_tokens: None,
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: TokenUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.reasoning_tokens, Some(600));
    }

    #[test]
    fn token_usage_serde_missing_reasoning_tokens_defaults_none() {
        // Old serialised form without reasoning_tokens must still deserialise cleanly.
        let json = r#"{"input_tokens":10,"output_tokens":5}"#;
        let usage: TokenUsage = serde_json::from_str(json).unwrap();
        assert!(usage.reasoning_tokens.is_none());
    }
}
