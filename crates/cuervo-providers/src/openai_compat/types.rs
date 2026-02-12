//! OpenAI Chat Completions API types.
//!
//! Shared by OpenAI and DeepSeek providers (same wire format).

use serde::{Deserialize, Serialize};

// --- Request types ---

#[derive(Debug, Serialize)]
pub struct OpenAIChatRequest {
    pub model: String,
    pub messages: Vec<OpenAIChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Used by OpenAI reasoning models (o1, o3-mini) instead of max_tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OpenAITool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

#[derive(Debug, Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAIChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<OpenAIMessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Message content: either a plain string or structured parts.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum OpenAIMessageContent {
    Text(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: OpenAIFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIFunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Serialize)]
pub struct OpenAITool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIFunctionDef,
}

#[derive(Debug, Serialize)]
pub struct OpenAIFunctionDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

// --- SSE response types ---

#[derive(Debug, Deserialize)]
pub struct OpenAISseChunk {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub choices: Vec<OpenAIChoice>,
    #[serde(default)]
    pub usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIChoice {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub delta: Option<OpenAIDelta>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIDelta {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<OpenAIToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIToolCallDelta {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<OpenAIFunctionDelta>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIFunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

// --- Error response ---

#[derive(Debug, Deserialize)]
pub struct OpenAIErrorResponse {
    pub error: OpenAIErrorDetail,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_basic_request() {
        let req = OpenAIChatRequest {
            model: "gpt-4o".into(),
            messages: vec![OpenAIChatMessage {
                role: "user".into(),
                content: Some(OpenAIMessageContent::Text("hello".into())),
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(1024),
            max_completion_tokens: None,
            temperature: Some(0.7),
            stream: true,
            tools: vec![],
            stream_options: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("gpt-4o"));
        assert!(json.contains("\"stream\":true"));
        assert!(!json.contains("tools")); // empty vec skipped
        assert!(!json.contains("stream_options")); // None skipped
        assert!(!json.contains("max_completion_tokens")); // None skipped
    }

    #[test]
    fn serialize_request_with_tools() {
        let req = OpenAIChatRequest {
            model: "gpt-4o".into(),
            messages: vec![OpenAIChatMessage {
                role: "user".into(),
                content: Some(OpenAIMessageContent::Text("read a file".into())),
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(1024),
            max_completion_tokens: None,
            temperature: None,
            stream: true,
            tools: vec![OpenAITool {
                tool_type: "function".into(),
                function: OpenAIFunctionDef {
                    name: "file_read".into(),
                    description: "Read a file".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"]
                    }),
                },
            }],
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"tools\""));
        assert!(json.contains("file_read"));
        assert!(json.contains("include_usage"));
    }

    #[test]
    fn serialize_tool_result_message() {
        let msg = OpenAIChatMessage {
            role: "tool".into(),
            content: Some(OpenAIMessageContent::Text("file contents here".into())),
            tool_calls: None,
            tool_call_id: Some("call_abc123".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"tool\""));
        assert!(json.contains("call_abc123"));
    }

    #[test]
    fn deserialize_sse_text_chunk() {
        let json = r#"{"id":"chatcmpl-abc","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let chunk: OpenAISseChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices.len(), 1);
        let delta = chunk.choices[0].delta.as_ref().unwrap();
        assert_eq!(delta.content.as_deref(), Some("Hello"));
        assert!(chunk.choices[0].finish_reason.is_none());
    }

    #[test]
    fn deserialize_sse_tool_call_chunk() {
        let json = r#"{"id":"chatcmpl-abc","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_xyz","function":{"name":"bash","arguments":""}}]},"finish_reason":null}]}"#;
        let chunk: OpenAISseChunk = serde_json::from_str(json).unwrap();
        let delta = chunk.choices[0].delta.as_ref().unwrap();
        let tc = &delta.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.id.as_deref(), Some("call_xyz"));
        assert_eq!(tc.function.as_ref().unwrap().name.as_deref(), Some("bash"));
    }

    #[test]
    fn deserialize_sse_finish_reasons() {
        for (reason, expected) in [
            ("stop", "stop"),
            ("length", "length"),
            ("tool_calls", "tool_calls"),
        ] {
            let json = format!(
                r#"{{"id":"chatcmpl-abc","choices":[{{"index":0,"delta":{{}},"finish_reason":"{reason}"}}]}}"#,
            );
            let chunk: OpenAISseChunk = serde_json::from_str(&json).unwrap();
            assert_eq!(
                chunk.choices[0].finish_reason.as_deref(),
                Some(expected)
            );
        }
    }

    #[test]
    fn deserialize_sse_usage() {
        let json = r#"{"id":"chatcmpl-abc","choices":[],"usage":{"prompt_tokens":25,"completion_tokens":100}}"#;
        let chunk: OpenAISseChunk = serde_json::from_str(json).unwrap();
        let usage = chunk.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 25);
        assert_eq!(usage.completion_tokens, 100);
    }

    #[test]
    fn deserialize_error_response() {
        let json = r#"{"error":{"message":"Invalid API key","type":"invalid_request_error"}}"#;
        let resp: OpenAIErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.error.message, "Invalid API key");
        assert_eq!(
            resp.error.error_type.as_deref(),
            Some("invalid_request_error")
        );
    }
}
