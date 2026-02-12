use serde::{Deserialize, Serialize};

// --- Request types ---

#[derive(Debug, Serialize)]
pub struct ApiRequest {
    pub model: String,
    pub messages: Vec<ApiMessage>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ApiToolDefinition>,
}

#[derive(Debug, Serialize)]
pub struct ApiMessage {
    pub role: String,
    pub content: ApiMessageContent,
}

/// Message content: either a plain string or structured blocks.
///
/// Uses `#[serde(untagged)]` so a simple string serializes as `"text"`
/// and blocks serialize as an array of objects.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ApiMessageContent {
    Text(String),
    Blocks(Vec<ApiContentBlock>),
}

/// A content block within an API message.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ApiContentBlock {
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
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

/// Tool definition sent to the API.
#[derive(Debug, Clone, Serialize)]
pub struct ApiToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

// --- SSE event types ---

/// Wrapper for all SSE event data payloads.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum SseEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: MessageStartData },

    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: ContentBlockData,
    },

    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: DeltaData },

    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },

    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaData,
        usage: Option<DeltaUsage>,
    },

    #[serde(rename = "message_stop")]
    MessageStop,

    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "error")]
    Error { error: ApiError },
}

#[derive(Debug, Deserialize)]
pub struct MessageStartData {
    pub id: String,
    pub model: String,
    pub usage: Option<StartUsage>,
}

#[derive(Debug, Deserialize)]
pub struct StartUsage {
    pub input_tokens: u32,
}

#[derive(Debug, Deserialize)]
pub struct ContentBlockData {
    #[serde(rename = "type")]
    pub block_type: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum DeltaData {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
pub struct MessageDeltaData {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeltaUsage {
    pub output_tokens: u32,
}

#[derive(Debug, Deserialize)]
pub struct ApiError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

// --- Non-streaming error response ---

#[derive(Debug, Deserialize)]
pub struct ApiErrorResponse {
    #[serde(rename = "type")]
    pub response_type: Option<String>,
    pub error: ApiError,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_message_start() {
        let json = r#"{"type":"message_start","message":{"id":"msg_01","model":"claude-sonnet-4-5-20250929","usage":{"input_tokens":25}}}"#;
        let event: SseEvent = serde_json::from_str(json).unwrap();
        match event {
            SseEvent::MessageStart { message } => {
                assert_eq!(message.id, "msg_01");
                assert_eq!(message.usage.unwrap().input_tokens, 25);
            }
            _ => panic!("Expected MessageStart"),
        }
    }

    #[test]
    fn deserialize_content_block_delta() {
        let json = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let event: SseEvent = serde_json::from_str(json).unwrap();
        match event {
            SseEvent::ContentBlockDelta { delta, .. } => match delta {
                DeltaData::TextDelta { text } => assert_eq!(text, "Hello"),
                _ => panic!("Expected TextDelta"),
            },
            _ => panic!("Expected ContentBlockDelta"),
        }
    }

    #[test]
    fn deserialize_message_delta() {
        let json = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}"#;
        let event: SseEvent = serde_json::from_str(json).unwrap();
        match event {
            SseEvent::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason.as_deref(), Some("end_turn"));
                assert_eq!(usage.unwrap().output_tokens, 15);
            }
            _ => panic!("Expected MessageDelta"),
        }
    }

    #[test]
    fn deserialize_message_stop() {
        let json = r#"{"type":"message_stop"}"#;
        let event: SseEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, SseEvent::MessageStop));
    }

    #[test]
    fn deserialize_ping() {
        let json = r#"{"type":"ping"}"#;
        let event: SseEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, SseEvent::Ping));
    }

    #[test]
    fn deserialize_error_event() {
        let json =
            r#"{"type":"error","error":{"type":"overloaded_error","message":"API is overloaded"}}"#;
        let event: SseEvent = serde_json::from_str(json).unwrap();
        match event {
            SseEvent::Error { error } => {
                assert_eq!(error.error_type, "overloaded_error");
            }
            _ => panic!("Expected Error"),
        }
    }

    #[test]
    fn serialize_api_request_basic() {
        let req = ApiRequest {
            model: "claude-sonnet-4-5-20250929".into(),
            messages: vec![ApiMessage {
                role: "user".into(),
                content: ApiMessageContent::Text("hello".into()),
            }],
            max_tokens: 1024,
            temperature: Some(0.0),
            system: None,
            stream: true,
            tools: vec![],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("claude-sonnet"));
        assert!(json.contains("\"stream\":true"));
        assert!(!json.contains("system"));
        assert!(!json.contains("tools")); // skip_serializing_if empty
    }

    #[test]
    fn serialize_api_request_with_tools() {
        let req = ApiRequest {
            model: "claude-sonnet-4-5-20250929".into(),
            messages: vec![ApiMessage {
                role: "user".into(),
                content: ApiMessageContent::Text("read a file".into()),
            }],
            max_tokens: 1024,
            temperature: None,
            system: None,
            stream: true,
            tools: vec![ApiToolDefinition {
                name: "file_read".into(),
                description: "Read a file".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                }),
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"tools\""));
        assert!(json.contains("file_read"));
        assert!(json.contains("input_schema"));
    }

    #[test]
    fn serialize_tool_result_message() {
        let msg = ApiMessage {
            role: "user".into(),
            content: ApiMessageContent::Blocks(vec![ApiContentBlock::ToolResult {
                tool_use_id: "toolu_123".into(),
                content: "file contents here".into(),
                is_error: false,
            }]),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("tool_result"));
        assert!(json.contains("toolu_123"));
        assert!(!json.contains("is_error")); // false is skipped
    }

    #[test]
    fn serialize_tool_result_with_error() {
        let msg = ApiMessage {
            role: "user".into(),
            content: ApiMessageContent::Blocks(vec![ApiContentBlock::ToolResult {
                tool_use_id: "toolu_456".into(),
                content: "permission denied".into(),
                is_error: true,
            }]),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"is_error\":true"));
    }

    #[test]
    fn deserialize_api_error_response() {
        let json = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid api key"}}"#;
        let resp: ApiErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.error.error_type, "authentication_error");
    }

    #[test]
    fn deserialize_tool_use_content_block_start() {
        let json = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"file_read"}}"#;
        let event: SseEvent = serde_json::from_str(json).unwrap();
        match event {
            SseEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                assert_eq!(index, 1);
                assert_eq!(content_block.block_type, "tool_use");
                assert_eq!(content_block.id.as_deref(), Some("toolu_abc"));
                assert_eq!(content_block.name.as_deref(), Some("file_read"));
            }
            _ => panic!("Expected ContentBlockStart"),
        }
    }

    #[test]
    fn deserialize_input_json_delta() {
        let json = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#;
        let event: SseEvent = serde_json::from_str(json).unwrap();
        match event {
            SseEvent::ContentBlockDelta { index, delta } => {
                assert_eq!(index, 1);
                match delta {
                    DeltaData::InputJsonDelta { partial_json } => {
                        assert_eq!(partial_json, "{\"path\":");
                    }
                    _ => panic!("Expected InputJsonDelta"),
                }
            }
            _ => panic!("Expected ContentBlockDelta"),
        }
    }
}
