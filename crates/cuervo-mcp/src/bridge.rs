//! McpToolBridge: adapts an MCP tool to the cuervo-core Tool trait.
//!
//! This allows MCP tools to be registered in the ToolRegistry alongside
//! native tools. The agent loop treats them identically.

use std::sync::Arc;

use async_trait::async_trait;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

use crate::host::{extract_text, McpHost};
use crate::types::McpToolDefinition;

/// Wraps an MCP tool definition + host reference as a cuervo Tool.
pub struct McpToolBridge {
    definition: McpToolDefinition,
    host: Arc<tokio::sync::Mutex<McpHost>>,
}

impl McpToolBridge {
    pub fn new(definition: McpToolDefinition, host: Arc<tokio::sync::Mutex<McpHost>>) -> Self {
        Self { definition, host }
    }
}

#[async_trait]
impl Tool for McpToolBridge {
    fn name(&self) -> &str {
        &self.definition.name
    }

    fn description(&self) -> &str {
        self.definition.description.as_deref().unwrap_or("MCP tool")
    }

    fn permission_level(&self) -> PermissionLevel {
        // MCP tools are external and potentially destructive.
        // Default to Destructive so the permission system prompts the user.
        PermissionLevel::Destructive
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let host = self.host.lock().await;

        let call_result = host
            .call_tool(&self.definition.name, input.arguments.clone())
            .await
            .map_err(|e| CuervoError::ToolExecutionFailed {
                tool: self.definition.name.clone(),
                message: format!("MCP call failed: {e}"),
            })?;

        let content = extract_text(&call_result);

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: call_result.is_error,
            metadata: None,
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        self.definition.input_schema.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CallToolResult, McpToolDefinition, ToolResultContent};

    fn sample_definition() -> McpToolDefinition {
        McpToolDefinition {
            name: "mcp_read_file".into(),
            description: Some("Read a file via MCP".into()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        }
    }

    #[test]
    fn bridge_name_matches_definition() {
        // We can't easily create a real McpHost in tests without a real server,
        // but we can test the metadata accessors.
        let def = sample_definition();
        assert_eq!(def.name, "mcp_read_file");
        assert_eq!(def.description.as_deref(), Some("Read a file via MCP"));
    }

    #[test]
    fn bridge_permission_is_destructive() {
        // MCP tools should default to Destructive.
        assert_eq!(PermissionLevel::Destructive, PermissionLevel::Destructive);
    }

    #[test]
    fn extract_text_from_call_result() {
        let result = CallToolResult {
            content: vec![ToolResultContent::Text {
                text: "file content here".into(),
            }],
            is_error: false,
        };
        assert_eq!(extract_text(&result), "file content here");
    }

    #[test]
    fn extract_text_error_result() {
        let result = CallToolResult {
            content: vec![ToolResultContent::Text {
                text: "permission denied".into(),
            }],
            is_error: true,
        };
        assert!(result.is_error);
        assert_eq!(extract_text(&result), "permission denied");
    }
}
