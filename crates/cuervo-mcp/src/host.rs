//! MCP Host: manages connection lifecycle, tool discovery, and tool calls.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{McpError, McpResult};
use crate::transport::StdioTransport;
use crate::types::{
    CallToolResult, InitializeResult, JsonRpcRequest, McpToolDefinition, ToolResultContent,
    CLIENT_NAME, PROTOCOL_VERSION,
};

/// An MCP Host connected to a single MCP server.
pub struct McpHost {
    name: String,
    transport: StdioTransport,
    next_id: AtomicU64,
    server_info: Option<InitializeResult>,
    tools: Vec<McpToolDefinition>,
}

impl McpHost {
    /// Create a new host by spawning the server process.
    pub fn new(
        name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> McpResult<Self> {
        let transport = StdioTransport::spawn(command, args, env)?;
        Ok(Self {
            name: name.into(),
            transport,
            next_id: AtomicU64::new(1),
            server_info: None,
            tools: Vec::new(),
        })
    }

    fn next_request_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Server name from config.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Discovered tools.
    pub fn tools(&self) -> &[McpToolDefinition] {
        &self.tools
    }

    /// Initialize the MCP connection (required before any other calls).
    pub async fn initialize(&mut self) -> McpResult<&InitializeResult> {
        let id = self.next_request_id();
        let params = serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": CLIENT_NAME,
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let request = JsonRpcRequest::new(id, "initialize", Some(params));
        self.transport.send(&request).await?;
        let response = self.transport.receive().await?;

        if let Some(err) = response.error {
            return Err(McpError::ServerError {
                code: err.code,
                message: err.message,
            });
        }

        let result: InitializeResult =
            serde_json::from_value(response.result.ok_or_else(|| {
                McpError::Protocol("Missing result in initialize response".into())
            })?)?;

        self.server_info = Some(result);

        // Send initialized notification (no response expected, but we send it).
        let notif = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: self.next_request_id(),
            method: "notifications/initialized".into(),
            params: None,
        };
        // Best-effort: don't fail if notification send fails.
        let _ = self.transport.send(&notif).await;

        self.server_info
            .as_ref()
            .ok_or(McpError::NotInitialized)
    }

    /// Discover available tools from the MCP server.
    pub async fn list_tools(&mut self) -> McpResult<&[McpToolDefinition]> {
        if self.server_info.is_none() {
            return Err(McpError::NotInitialized);
        }

        let id = self.next_request_id();
        let request = JsonRpcRequest::new(id, "tools/list", None);
        self.transport.send(&request).await?;
        let response = self.transport.receive().await?;

        if let Some(err) = response.error {
            return Err(McpError::ServerError {
                code: err.code,
                message: err.message,
            });
        }

        let result = response
            .result
            .ok_or_else(|| McpError::Protocol("Missing result in tools/list response".into()))?;

        // MCP tools/list returns { "tools": [...] }
        let tools_array = result
            .get("tools")
            .ok_or_else(|| McpError::Protocol("Missing 'tools' array in response".into()))?;

        self.tools = serde_json::from_value(tools_array.clone())?;
        Ok(&self.tools)
    }

    /// Call a tool on the MCP server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> McpResult<CallToolResult> {
        if self.server_info.is_none() {
            return Err(McpError::NotInitialized);
        }

        let id = self.next_request_id();
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments
        });

        let request = JsonRpcRequest::new(id, "tools/call", Some(params));
        self.transport.send(&request).await?;
        let response = self.transport.receive().await?;

        if let Some(err) = response.error {
            return Err(McpError::ServerError {
                code: err.code,
                message: err.message,
            });
        }

        let result = response
            .result
            .ok_or_else(|| McpError::Protocol("Missing result in tools/call response".into()))?;

        let call_result: CallToolResult = serde_json::from_value(result)?;
        Ok(call_result)
    }

    /// Shut down the MCP server gracefully.
    pub async fn shutdown(&self) -> McpResult<()> {
        self.transport.close().await
    }
}

/// Extract text content from a CallToolResult.
pub fn extract_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            ToolResultContent::Text { text } => text.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_single() {
        let result = CallToolResult {
            content: vec![ToolResultContent::Text {
                text: "hello".into(),
            }],
            is_error: false,
        };
        assert_eq!(extract_text(&result), "hello");
    }

    #[test]
    fn extract_text_multiple() {
        let result = CallToolResult {
            content: vec![
                ToolResultContent::Text {
                    text: "line1".into(),
                },
                ToolResultContent::Text {
                    text: "line2".into(),
                },
            ],
            is_error: false,
        };
        assert_eq!(extract_text(&result), "line1\nline2");
    }

    #[test]
    fn extract_text_empty() {
        let result = CallToolResult {
            content: vec![],
            is_error: false,
        };
        assert_eq!(extract_text(&result), "");
    }

    #[tokio::test]
    async fn spawn_bad_command_returns_error() {
        let result = McpHost::new("bad", "no_such_bin_xyz", &[], &HashMap::new());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_tools_before_init_fails() {
        // Use cat so the spawn succeeds, then try list_tools without initialize.
        let mut host = McpHost::new("test", "cat", &[], &HashMap::new()).unwrap();
        let result = host.list_tools().await;
        assert!(matches!(result, Err(McpError::NotInitialized)));
        host.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn call_tool_before_init_fails() {
        let host = McpHost::new("test", "cat", &[], &HashMap::new()).unwrap();
        let result = host.call_tool("test", serde_json::json!({})).await;
        assert!(matches!(result, Err(McpError::NotInitialized)));
        host.shutdown().await.unwrap();
    }
}
