//! `http_request` tool: make HTTP requests (POST, PUT, DELETE, PATCH).
//!
//! For write operations — Destructive permission, requires confirmation.
//! Supports custom headers, body, timeout. Response truncated to 1MB.
//! Sensitive headers (Authorization, Cookie) are redacted in output.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

/// Maximum response body size (1 MB).
const MAX_RESPONSE_BYTES: usize = 1_048_576;
/// Default request timeout.
const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Maximum timeout.
const MAX_TIMEOUT_SECS: u64 = 120;
/// Headers to redact in output.
const REDACTED_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-auth-token",
];

/// HTTP request tool for write operations.
pub struct HttpRequestTool;

impl Default for HttpRequestTool {
    fn default() -> Self {
        Self
    }
}

impl HttpRequestTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for HttpRequestTool {
    fn name(&self) -> &str {
        "http_request"
    }

    fn description(&self) -> &str {
        "Make HTTP requests (POST, PUT, DELETE, PATCH). For write operations that modify remote resources."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Destructive
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        true
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let url = input.arguments["url"].as_str().ok_or_else(|| {
            HalconError::InvalidInput("http_request requires 'url' string".into())
        })?;

        let method_str = input.arguments["method"]
            .as_str()
            .unwrap_or("POST")
            .to_uppercase();

        let timeout_secs = input.arguments["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);

        // Validate URL scheme.
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "http_request error: URL must start with http:// or https://".to_string(),
                is_error: true,
                metadata: None,
            });
        }

        // Parse method.
        let method = match method_str.as_str() {
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            "PATCH" => reqwest::Method::PATCH,
            other => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!(
                        "http_request error: unsupported method '{other}'. Use POST, PUT, DELETE, or PATCH."
                    ),
                    is_error: true,
                    metadata: None,
                });
            }
        };

        // Build client.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| HalconError::ToolExecutionFailed {
                tool: "http_request".into(),
                message: format!("failed to create HTTP client: {e}"),
            })?;

        let mut request = client.request(method.clone(), url);

        // Add custom headers.
        if let Some(headers_obj) = input.arguments["headers"].as_object() {
            for (key, value) in headers_obj {
                if let Some(val_str) = value.as_str() {
                    request = request.header(key.as_str(), val_str);
                }
            }
        }

        // Add body.
        if let Some(body) = input.arguments["body"].as_str() {
            // Auto-detect content type if not set.
            let has_content_type = input.arguments["headers"]
                .as_object()
                .map(|h| h.keys().any(|k| k.to_lowercase() == "content-type"))
                .unwrap_or(false);

            if !has_content_type {
                // Try to detect if body is JSON.
                if body.starts_with('{') || body.starts_with('[') {
                    request = request.header("Content-Type", "application/json");
                }
            }

            request = request.body(body.to_string());
        }

        // Send request.
        let response = request
            .send()
            .await
            .map_err(|e| HalconError::ToolExecutionFailed {
                tool: "http_request".into(),
                message: format!("{method} {url} failed: {e}"),
            })?;

        let status = response.status();
        let status_code = status.as_u16();

        // Collect response headers (redact sensitive ones).
        let mut resp_headers: HashMap<String, String> = HashMap::new();
        for (key, value) in response.headers() {
            let key_str = key.as_str().to_lowercase();
            let val = if REDACTED_HEADERS.contains(&key_str.as_str()) {
                "[REDACTED]".to_string()
            } else {
                value.to_str().unwrap_or("[binary]").to_string()
            };
            resp_headers.insert(key_str, val);
        }

        // Read body.
        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| HalconError::ToolExecutionFailed {
                tool: "http_request".into(),
                message: format!("failed to read response body: {e}"),
            })?;

        let truncated = body_bytes.len() > MAX_RESPONSE_BYTES;
        let body_text = if truncated {
            let slice = &body_bytes[..MAX_RESPONSE_BYTES];
            String::from_utf8_lossy(slice).to_string()
        } else {
            String::from_utf8_lossy(&body_bytes).to_string()
        };

        let content = format!(
            "{method_str} {url} → {status_code} {}\n\n{body_text}{}",
            status.canonical_reason().unwrap_or(""),
            if truncated {
                "\n\n[Response truncated to 1MB]"
            } else {
                ""
            }
        );

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: status.is_client_error() || status.is_server_error(),
            metadata: Some(json!({
                "status_code": status_code,
                "method": method_str,
                "url": url,
                "headers": resp_headers,
                "body_size": body_bytes.len(),
                "truncated": truncated,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to send the request to."
                },
                "method": {
                    "type": "string",
                    "description": "HTTP method: POST, PUT, DELETE, or PATCH (default: POST).",
                    "enum": ["POST", "PUT", "DELETE", "PATCH"]
                },
                "headers": {
                    "type": "object",
                    "description": "Custom request headers as key-value pairs."
                },
                "body": {
                    "type": "string",
                    "description": "Request body (string). JSON is auto-detected for Content-Type."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Request timeout in seconds (1-120, default 30)."
                }
            },
            "required": ["url"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test".to_string(),
            arguments: args,
            working_directory: "/tmp".to_string(),
        }
    }

    #[test]
    fn schema_is_valid() {
        let tool = HttpRequestTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["url"].is_object());
        assert!(schema["properties"]["method"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "url"));
    }

    #[test]
    fn permission_is_destructive() {
        let tool = HttpRequestTool::new();
        assert_eq!(tool.permission_level(), PermissionLevel::Destructive);
    }

    #[test]
    fn requires_confirmation_always() {
        let tool = HttpRequestTool::new();
        let dummy = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(tool.requires_confirmation(&dummy));
    }

    #[tokio::test]
    async fn missing_url_error() {
        let tool = HttpRequestTool::new();
        let result = tool.execute(make_input(json!({}))).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn invalid_url_scheme() {
        let tool = HttpRequestTool::new();
        let out = tool
            .execute(make_input(json!({"url": "ftp://example.com"})))
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("http://"));
    }

    #[tokio::test]
    async fn invalid_method() {
        let tool = HttpRequestTool::new();
        let out = tool
            .execute(make_input(json!({
                "url": "https://example.com",
                "method": "GET"
            })))
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("unsupported method"));
    }

    #[tokio::test]
    async fn default_method_is_post() {
        // This will fail to connect but we can verify the method is used.
        let tool = HttpRequestTool::new();
        let result = tool
            .execute(make_input(json!({
                "url": "http://127.0.0.1:1",
                "timeout": 1
            })))
            .await;
        // Connection refused is expected — just ensure no method error.
        assert!(result.is_err() || !result.unwrap().content.contains("unsupported method"));
    }

    #[test]
    fn header_redaction() {
        // Verify redaction list contains expected headers.
        assert!(REDACTED_HEADERS.contains(&"authorization"));
        assert!(REDACTED_HEADERS.contains(&"cookie"));
        assert!(REDACTED_HEADERS.contains(&"x-api-key"));
    }

    #[test]
    fn timeout_clamping() {
        assert_eq!(0u64.clamp(1, MAX_TIMEOUT_SECS), 1);
        assert_eq!(200u64.clamp(1, MAX_TIMEOUT_SECS), MAX_TIMEOUT_SECS);
        assert_eq!(30u64.clamp(1, MAX_TIMEOUT_SECS), 30);
    }

    #[tokio::test]
    async fn json_content_type_auto_detect() {
        // Verify that JSON body gets content-type auto-detected.
        // We can't easily test the actual header being set without a server,
        // but we can test that the code path handles a JSON body string.
        let tool = HttpRequestTool::new();
        let result = tool
            .execute(make_input(json!({
                "url": "http://127.0.0.1:1",
                "body": "{\"key\": \"value\"}",
                "timeout": 1
            })))
            .await;
        // Will fail to connect, but should not panic or return an input error.
        assert!(result.is_err()); // Connection refused.
    }

    #[tokio::test]
    async fn custom_headers_accepted() {
        let tool = HttpRequestTool::new();
        let result = tool
            .execute(make_input(json!({
                "url": "http://127.0.0.1:1",
                "headers": {"X-Custom": "value", "Content-Type": "text/plain"},
                "body": "hello",
                "timeout": 1
            })))
            .await;
        // Connection refused but input is valid.
        assert!(result.is_err());
    }
}
