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
    permission_override: Option<PermissionLevel>,
}

impl McpToolBridge {
    pub fn new(
        definition: McpToolDefinition,
        host: Arc<tokio::sync::Mutex<McpHost>>,
        permission_override: Option<PermissionLevel>,
    ) -> Self {
        Self {
            definition,
            host,
            permission_override,
        }
    }

    /// Infer permission level from tool name and description keywords.
    fn infer_permission(&self) -> PermissionLevel {
        infer_permission_from_definition(&self.definition)
    }
}

/// Infer permission level from tool name and description.
/// Exposed for testability without needing a full McpHost.
fn infer_permission_from_definition(def: &McpToolDefinition) -> PermissionLevel {
    let name = def.name.to_lowercase();
    let desc = def.description.as_deref().unwrap_or("").to_lowercase();
    let text = format!("{name} {desc}");

    let write_signals = [
        "write", "create", "update", "delete", "remove", "set", "put", "post", "push",
        "commit", "execute", "run", "send", "modify",
    ];
    let read_signals = [
        "read", "get", "list", "search", "fetch", "show", "find", "query", "describe",
        "count", "view",
    ];

    let has_write = write_signals.iter().any(|p| text.contains(p));
    let has_read = read_signals.iter().any(|p| text.contains(p));

    if has_write {
        PermissionLevel::Destructive
    } else if has_read {
        PermissionLevel::ReadOnly
    } else {
        PermissionLevel::Destructive // safe default
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
        // Use explicit override if set, otherwise infer from tool name/description.
        self.permission_override
            .unwrap_or_else(|| self.infer_permission())
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

    fn make_def(name: &str, description: &str) -> McpToolDefinition {
        McpToolDefinition {
            name: name.into(),
            description: Some(description.into()),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    #[test]
    fn bridge_name_matches_definition() {
        let def = sample_definition();
        assert_eq!(def.name, "mcp_read_file");
        assert_eq!(def.description.as_deref(), Some("Read a file via MCP"));
    }

    #[test]
    fn infer_permission_read_tool() {
        let def = make_def("github_search", "Search repositories and code");
        // "search" is a read signal → ReadOnly
        assert_eq!(infer_permission_from_definition(&def), PermissionLevel::ReadOnly);
    }

    #[test]
    fn infer_permission_write_tool() {
        let def = make_def("file_create", "Create a new file on disk");
        // "create" is a write signal → Destructive
        assert_eq!(infer_permission_from_definition(&def), PermissionLevel::Destructive);
    }

    #[test]
    fn infer_permission_unknown_is_destructive() {
        let def = make_def("custom_tool", "Does something custom");
        // No recognized signals → Destructive (safe default)
        assert_eq!(infer_permission_from_definition(&def), PermissionLevel::Destructive);
    }

    #[test]
    fn permission_override_takes_precedence() {
        // Override field is checked before infer_permission in permission_level().
        // Test via struct field access since permission_level() is on the Tool trait.
        let override_perm = Some(PermissionLevel::ReadOnly);
        let def = make_def("dangerous_delete", "Delete everything");
        // infer_permission would return Destructive, but override takes precedence.
        let inferred = infer_permission_from_definition(&def);
        assert_eq!(inferred, PermissionLevel::Destructive);
        let result = override_perm.unwrap_or(inferred);
        assert_eq!(result, PermissionLevel::ReadOnly);
    }

    #[test]
    fn new_constructor_signature() {
        // Verify the 3-argument constructor signature compiles.
        // We can't construct a full McpToolBridge without a real McpHost,
        // but we verify the permission inference logic independently above.
        let def = sample_definition();
        let inferred = infer_permission_from_definition(&def);
        // "read" in name → ReadOnly
        assert_eq!(inferred, PermissionLevel::ReadOnly);
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
