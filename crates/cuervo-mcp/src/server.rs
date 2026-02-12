//! MCP Server: exposes cuervo tools via the Model Context Protocol.
//!
//! Reads newline-delimited JSON-RPC 2.0 from stdin, dispatches to tool
//! handlers, and writes responses to stdout. Designed to be spawned as
//! a sidecar process by an IDE (e.g., Tauri `externalBin`).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use cuervo_core::traits::Tool;

use crate::error::{McpError, McpResult};
use crate::types::{
    error_codes, CallToolResult, InitializeResult, JsonRpcError, JsonRpcResponse,
    McpToolDefinition, ServerCapabilities, ServerInfo, ToolResultContent, ToolsCapability,
    PROTOCOL_VERSION,
};

/// MCP Server that exposes cuervo tools over stdio JSON-RPC.
pub struct McpServer {
    tools: HashMap<String, Arc<dyn Tool>>,
    working_directory: String,
}

impl McpServer {
    /// Create a new MCP server with the given tools and working directory.
    pub fn new(tools: Vec<Arc<dyn Tool>>, working_directory: String) -> Self {
        let tool_map: HashMap<String, Arc<dyn Tool>> = tools
            .into_iter()
            .map(|t| (t.name().to_string(), t))
            .collect();
        Self {
            tools: tool_map,
            working_directory,
        }
    }

    /// Run the MCP server event loop: read from stdin, write to stdout.
    ///
    /// Blocks until stdin is closed (client disconnects).
    pub async fn run(&self) -> McpResult<()> {
        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();

        tracing::info!("MCP server starting, {} tools registered", self.tools.len());

        loop {
            line.clear();
            let bytes_read = reader
                .read_line(&mut line)
                .await
                .map_err(|e| McpError::Transport(format!("stdin read failed: {e}")))?;

            if bytes_read == 0 {
                tracing::info!("MCP server: stdin closed, shutting down");
                break;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Parse as generic JSON first to handle both requests and notifications.
            let value: serde_json::Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    let resp = Self::error_response(
                        None,
                        error_codes::PARSE_ERROR,
                        &format!("Parse error: {e}"),
                    );
                    write_response(&mut stdout, &resp).await?;
                    continue;
                }
            };

            let method = value
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            let id = value.get("id").and_then(|i| i.as_u64());
            let params = value.get("params").cloned();

            // Notifications (no id) — handle silently, no response.
            if id.is_none() {
                tracing::debug!("MCP notification received: {method}");
                continue;
            }

            let request_id = id.unwrap();
            let response = self.handle_request(request_id, &method, params).await;

            write_response(&mut stdout, &response).await?;
        }

        Ok(())
    }

    /// Dispatch a JSON-RPC request to the appropriate handler.
    async fn handle_request(
        &self,
        id: u64,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> JsonRpcResponse {
        match method {
            "initialize" => self.handle_initialize(id),
            "tools/list" => self.handle_tools_list(id),
            "tools/call" => self.handle_tools_call(id, params).await,
            "ping" => Self::success_response(id, serde_json::json!({})),
            _ => Self::error_response(
                Some(id),
                error_codes::METHOD_NOT_FOUND,
                &format!("Unknown method: {method}"),
            ),
        }
    }

    /// Handle `initialize` — return server info and capabilities.
    fn handle_initialize(&self, id: u64) -> JsonRpcResponse {
        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: false,
                }),
            },
            server_info: ServerInfo {
                name: "cuervo-mcp-server".to_string(),
                version: Some(crate::version().to_string()),
            },
        };
        Self::success_response(id, serde_json::to_value(result).unwrap())
    }

    /// Handle `tools/list` — return all registered tool definitions.
    fn handle_tools_list(&self, id: u64) -> JsonRpcResponse {
        let tools: Vec<McpToolDefinition> = self
            .tools
            .values()
            .map(|t| McpToolDefinition {
                name: t.name().to_string(),
                description: Some(t.description().to_string()),
                input_schema: t.input_schema(),
            })
            .collect();

        Self::success_response(id, serde_json::json!({ "tools": tools }))
    }

    /// Handle `tools/call` — execute a tool and return the result.
    async fn handle_tools_call(
        &self,
        id: u64,
        params: Option<serde_json::Value>,
    ) -> JsonRpcResponse {
        let params = match params {
            Some(p) => p,
            None => {
                return Self::error_response(
                    Some(id),
                    error_codes::INVALID_PARAMS,
                    "Missing params for tools/call",
                );
            }
        };

        let tool_name = match params.get("name").and_then(|n| n.as_str()) {
            Some(n) => n.to_string(),
            None => {
                return Self::error_response(
                    Some(id),
                    error_codes::INVALID_PARAMS,
                    "Missing 'name' in tools/call params",
                );
            }
        };

        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        let tool = match self.tools.get(&tool_name) {
            Some(t) => Arc::clone(t),
            None => {
                return Self::error_response(
                    Some(id),
                    error_codes::INVALID_PARAMS,
                    &format!("Unknown tool: {tool_name}"),
                );
            }
        };

        let input = cuervo_core::types::ToolInput {
            tool_use_id: format!("mcp-{id}"),
            arguments,
            working_directory: self.working_directory.clone(),
        };

        match tool.execute(input).await {
            Ok(output) => {
                let result = CallToolResult {
                    content: vec![ToolResultContent::Text {
                        text: output.content,
                    }],
                    is_error: output.is_error,
                };
                Self::success_response(id, serde_json::to_value(result).unwrap())
            }
            Err(e) => {
                let result = CallToolResult {
                    content: vec![ToolResultContent::Text {
                        text: format!("Tool execution error: {e}"),
                    }],
                    is_error: true,
                };
                Self::success_response(id, serde_json::to_value(result).unwrap())
            }
        }
    }

    /// Build a success JSON-RPC response.
    fn success_response(id: u64, result: serde_json::Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }

    /// Build an error JSON-RPC response.
    fn error_response(id: Option<u64>, code: i64, message: &str) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                data: None,
            }),
        }
    }

    /// Get the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Get tool names.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}

/// Write a JSON-RPC response as newline-delimited JSON to the writer.
async fn write_response<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    response: &JsonRpcResponse,
) -> McpResult<()> {
    let mut json = serde_json::to_string(response)?;
    json.push('\n');
    writer
        .write_all(json.as_bytes())
        .await
        .map_err(|e| McpError::Transport(format!("stdout write failed: {e}")))?;
    writer
        .flush()
        .await
        .map_err(|e| McpError::Transport(format!("stdout flush failed: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::error::Result as CoreResult;
    use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

    /// A simple test tool for MCP server tests.
    struct EchoTool;

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes the input message back"
        }
        fn permission_level(&self) -> PermissionLevel {
            PermissionLevel::ReadOnly
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "Message to echo" }
                },
                "required": ["message"]
            })
        }
        async fn execute(&self, input: ToolInput) -> CoreResult<ToolOutput> {
            let msg = input
                .arguments
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("(no message)");
            Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("echo: {msg}"),
                is_error: false,
                metadata: None,
            })
        }
    }

    /// A tool that always fails.
    struct FailTool;

    #[async_trait::async_trait]
    impl Tool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "Always fails"
        }
        fn permission_level(&self) -> PermissionLevel {
            PermissionLevel::ReadOnly
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            })
        }
        async fn execute(&self, _input: ToolInput) -> CoreResult<ToolOutput> {
            Err(cuervo_core::error::CuervoError::ToolExecutionFailed {
                tool: "fail".into(),
                message: "intentional failure".into(),
            })
        }
    }

    fn make_server() -> McpServer {
        McpServer::new(
            vec![Arc::new(EchoTool), Arc::new(FailTool)],
            "/tmp".to_string(),
        )
    }

    #[test]
    fn server_creation_registers_tools() {
        let server = make_server();
        assert_eq!(server.tool_count(), 2);
        let names = server.tool_names();
        assert!(names.contains(&"echo".to_string()));
        assert!(names.contains(&"fail".to_string()));
    }

    #[test]
    fn server_empty_tools() {
        let server = McpServer::new(vec![], "/tmp".to_string());
        assert_eq!(server.tool_count(), 0);
    }

    #[tokio::test]
    async fn handle_initialize() {
        let server = make_server();
        let resp = server.handle_request(1, "initialize", None).await;
        assert_eq!(resp.id, Some(1));
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], "cuervo-mcp-server");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn handle_tools_list() {
        let server = make_server();
        let resp = server.handle_request(2, "tools/list", None).await;
        assert_eq!(resp.id, Some(2));
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);

        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"fail"));

        // Verify tool definitions have required fields.
        for tool in tools {
            assert!(tool["name"].is_string());
            assert!(tool["inputSchema"].is_object());
        }
    }

    #[tokio::test]
    async fn handle_tools_call_success() {
        let server = make_server();
        let params = serde_json::json!({
            "name": "echo",
            "arguments": { "message": "hello MCP" }
        });
        let resp = server.handle_request(3, "tools/call", Some(params)).await;
        assert_eq!(resp.id, Some(3));
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], false);
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "echo: hello MCP");
    }

    #[tokio::test]
    async fn handle_tools_call_tool_error() {
        let server = make_server();
        let params = serde_json::json!({
            "name": "fail",
            "arguments": {}
        });
        let resp = server.handle_request(4, "tools/call", Some(params)).await;
        assert_eq!(resp.id, Some(4));
        assert!(resp.error.is_none()); // JSON-RPC level is success; tool error is in result.
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("intentional failure"));
    }

    #[tokio::test]
    async fn handle_tools_call_unknown_tool() {
        let server = make_server();
        let params = serde_json::json!({
            "name": "nonexistent",
            "arguments": {}
        });
        let resp = server
            .handle_request(5, "tools/call", Some(params))
            .await;
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, error_codes::INVALID_PARAMS);
        assert!(err.message.contains("nonexistent"));
    }

    #[tokio::test]
    async fn handle_tools_call_missing_params() {
        let server = make_server();
        let resp = server.handle_request(6, "tools/call", None).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, error_codes::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn handle_tools_call_missing_name() {
        let server = make_server();
        let params = serde_json::json!({ "arguments": {} });
        let resp = server.handle_request(7, "tools/call", Some(params)).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, error_codes::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn handle_unknown_method() {
        let server = make_server();
        let resp = server
            .handle_request(8, "unknown/method", None)
            .await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, error_codes::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn handle_ping() {
        let server = make_server();
        let resp = server.handle_request(9, "ping", None).await;
        assert_eq!(resp.id, Some(9));
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn write_response_format() {
        let resp = McpServer::success_response(1, serde_json::json!({"ok": true}));
        let mut buf = Vec::new();
        write_response(&mut buf, &resp).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.ends_with('\n'));
        let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["result"]["ok"], true);
    }

    #[test]
    fn error_response_format() {
        let resp =
            McpServer::error_response(Some(10), error_codes::INTERNAL_ERROR, "test error");
        assert_eq!(resp.id, Some(10));
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, error_codes::INTERNAL_ERROR);
        assert_eq!(err.message, "test error");
    }

    #[test]
    fn error_response_no_id() {
        let resp = McpServer::error_response(None, error_codes::PARSE_ERROR, "bad json");
        assert_eq!(resp.id, None);
    }

    #[tokio::test]
    async fn tools_call_default_arguments() {
        let server = make_server();
        // Call with no arguments field — should default to empty object.
        let params = serde_json::json!({ "name": "echo" });
        let resp = server.handle_request(11, "tools/call", Some(params)).await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["content"][0]["text"], "echo: (no message)");
    }

    #[tokio::test]
    async fn tool_definitions_have_schemas() {
        let server = make_server();
        let resp = server.handle_request(12, "tools/list", None).await;
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        for tool in &tools {
            let schema = &tool["inputSchema"];
            assert_eq!(schema["type"], "object");
            assert!(schema["properties"].is_object());
        }
    }
}
