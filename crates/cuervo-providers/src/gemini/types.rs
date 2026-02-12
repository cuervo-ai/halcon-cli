//! Gemini API types.
//!
//! Different from OpenAI: URL-embedded model, query-param auth,
//! `contents`/`parts` format, `functionDeclarations`.

use serde::{Deserialize, Serialize};

// --- Request types ---

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiRequest {
    pub contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<GeminiToolDeclaration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub parts: Vec<GeminiPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GeminiPart {
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionCall {
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionResponse {
    pub name: String,
    pub response: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct GeminiToolDeclaration {
    #[serde(rename = "functionDeclarations")]
    pub function_declarations: Vec<GeminiFunctionDecl>,
}

#[derive(Debug, Serialize)]
pub struct GeminiFunctionDecl {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

// --- Streaming response types (SSE) ---

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiStreamChunk {
    #[serde(default)]
    pub candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    pub usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiCandidate {
    pub content: Option<GeminiContent>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiUsageMetadata {
    #[serde(default)]
    pub prompt_token_count: u32,
    #[serde(default)]
    pub candidates_token_count: u32,
}

// --- Error response ---

#[derive(Debug, Deserialize)]
pub struct GeminiErrorResponse {
    pub error: GeminiErrorDetail,
}

#[derive(Debug, Deserialize)]
pub struct GeminiErrorDetail {
    pub message: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub code: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_basic_request() {
        let req = GeminiRequest {
            contents: vec![GeminiContent {
                role: Some("user".into()),
                parts: vec![GeminiPart::Text {
                    text: "hello".into(),
                }],
            }],
            system_instruction: None,
            tools: vec![],
            generation_config: Some(GeminiGenerationConfig {
                temperature: Some(0.7),
                max_output_tokens: Some(1024),
            }),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("hello"));
        assert!(json.contains("generationConfig"));
        assert!(!json.contains("systemInstruction")); // None skipped
        assert!(!json.contains("tools")); // empty vec skipped
    }

    #[test]
    fn serialize_request_with_system() {
        let req = GeminiRequest {
            contents: vec![GeminiContent {
                role: Some("user".into()),
                parts: vec![GeminiPart::Text {
                    text: "hello".into(),
                }],
            }],
            system_instruction: Some(GeminiContent {
                role: None,
                parts: vec![GeminiPart::Text {
                    text: "You are helpful.".into(),
                }],
            }),
            tools: vec![],
            generation_config: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("systemInstruction"));
        assert!(json.contains("You are helpful."));
    }

    #[test]
    fn serialize_request_with_tools() {
        let req = GeminiRequest {
            contents: vec![GeminiContent {
                role: Some("user".into()),
                parts: vec![GeminiPart::Text {
                    text: "read file".into(),
                }],
            }],
            system_instruction: None,
            tools: vec![GeminiToolDeclaration {
                function_declarations: vec![GeminiFunctionDecl {
                    name: "file_read".into(),
                    description: "Read a file".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": { "path": { "type": "string" } }
                    }),
                }],
            }],
            generation_config: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("functionDeclarations"));
        assert!(json.contains("file_read"));
    }

    #[test]
    fn serialize_function_call_part() {
        let part = GeminiPart::FunctionCall {
            function_call: GeminiFunctionCall {
                name: "bash".into(),
                args: serde_json::json!({"command": "ls"}),
            },
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("functionCall"));
        assert!(json.contains("bash"));
    }

    #[test]
    fn serialize_function_response_part() {
        let part = GeminiPart::FunctionResponse {
            function_response: GeminiFunctionResponse {
                name: "bash".into(),
                response: serde_json::json!({"output": "file1.txt"}),
            },
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("functionResponse"));
        assert!(json.contains("file1.txt"));
    }

    #[test]
    fn deserialize_stream_text_chunk() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello"}]},"finishReason":null}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.candidates.len(), 1);
        let content = chunk.candidates[0].content.as_ref().unwrap();
        match &content.parts[0] {
            GeminiPart::Text { text } => assert_eq!(text, "Hello"),
            _ => panic!("Expected text part"),
        }
    }

    #[test]
    fn deserialize_stream_function_call() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        let content = chunk.candidates[0].content.as_ref().unwrap();
        match &content.parts[0] {
            GeminiPart::FunctionCall { function_call } => {
                assert_eq!(function_call.name, "bash");
            }
            _ => panic!("Expected function call part"),
        }
    }

    #[test]
    fn deserialize_stream_usage() {
        let json = r#"{"candidates":[],"usageMetadata":{"promptTokenCount":25,"candidatesTokenCount":100}}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        let usage = chunk.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, 25);
        assert_eq!(usage.candidates_token_count, 100);
    }

    #[test]
    fn deserialize_finish_reasons() {
        for reason in ["STOP", "MAX_TOKENS", "SAFETY"] {
            let json = format!(
                r#"{{"candidates":[{{"content":{{"role":"model","parts":[{{"text":"done"}}]}},"finishReason":"{reason}"}}]}}"#,
            );
            let chunk: GeminiStreamChunk = serde_json::from_str(&json).unwrap();
            assert_eq!(
                chunk.candidates[0].finish_reason.as_deref(),
                Some(reason)
            );
        }
    }

    #[test]
    fn deserialize_error_response() {
        let json = r#"{"error":{"message":"API key not valid","status":"INVALID_ARGUMENT","code":400}}"#;
        let resp: GeminiErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.error.message, "API key not valid");
        assert_eq!(resp.error.code, Some(400));
    }
}
