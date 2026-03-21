//! Cenzontle MCP Bridge: exposes Cenzontle's MCP tools as native Halcón tools.
//!
//! Discovers tools from `GET /v1/mcp/tools` and registers them in the local
//! `ToolRegistry` with a `cenzontle_` prefix to avoid name collisions with
//! local tools (e.g. `llm_chat` → `cenzontle_llm_chat`).
//!
//! Each tool is backed by a `CenzontleMcpTool` that calls
//! `POST /v1/mcp/tools/call` on the Cenzontle backend.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info, warn};

use halcon_core::error::{HalconError, Result as HalconResult};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};
use halcon_providers::agent_types::{McpToolCallRequest, McpToolDef};
use halcon_providers::CenzontleAgentClient;
use halcon_tools::ToolRegistry;

/// Prefix added to Cenzontle MCP tool names to avoid collisions.
const TOOL_PREFIX: &str = "cenzontle_";

// ---------------------------------------------------------------------------
// CenzontleMcpTool — Tool impl backed by CenzontleAgentClient
// ---------------------------------------------------------------------------

/// A `Tool` implementation that routes calls through the Cenzontle MCP API.
struct CenzontleMcpTool {
    client: Arc<CenzontleAgentClient>,
    /// Prefixed tool name (e.g. "cenzontle_llm_chat").
    prefixed_name: String,
    /// Original tool name on the Cenzontle side (e.g. "llm_chat").
    original_name: String,
    definition: McpToolDef,
}

impl CenzontleMcpTool {
    fn infer_permission(&self) -> PermissionLevel {
        // Cenzontle tools run server-side, but some have side effects.
        let name = self.original_name.to_lowercase();
        if name.contains("search") || name.contains("list") || name.contains("get") {
            PermissionLevel::ReadOnly
        } else {
            // Default to ReadWrite for tools that may have side effects
            // (e.g. agent_submit_task, conversation_create).
            PermissionLevel::Destructive
        }
    }
}

#[async_trait]
impl Tool for CenzontleMcpTool {
    fn name(&self) -> &str {
        &self.prefixed_name
    }

    fn description(&self) -> &str {
        self.definition
            .description
            .as_deref()
            .unwrap_or("Cenzontle MCP tool")
    }

    fn permission_level(&self) -> PermissionLevel {
        self.infer_permission()
    }

    async fn execute(&self, input: ToolInput) -> HalconResult<ToolOutput> {
        let req = McpToolCallRequest {
            name: self.original_name.clone(),
            arguments: input.arguments,
        };

        let resp =
            self.client
                .call_mcp_tool(&req)
                .await
                .map_err(|e| HalconError::ToolExecutionFailed {
                    tool: self.prefixed_name.clone(),
                    message: format!("Cenzontle MCP tool call failed: {e}"),
                })?;

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: resp.text(),
            is_error: resp.is_error,
            metadata: None,
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        self.definition.input_schema.clone()
    }
}

// ---------------------------------------------------------------------------
// CenzontleMcpManager — lazy discovery and registration
// ---------------------------------------------------------------------------

/// Manages Cenzontle MCP tool discovery and registration.
///
/// Call `ensure_initialized()` during REPL setup to discover and register
/// Cenzontle's MCP tools into the local ToolRegistry.
pub struct CenzontleMcpManager {
    client: Arc<CenzontleAgentClient>,
    initialized: bool,
    tool_count: usize,
}

impl CenzontleMcpManager {
    /// Create a new manager from an agent client.
    pub fn new(client: Arc<CenzontleAgentClient>) -> Self {
        Self {
            client,
            initialized: false,
            tool_count: 0,
        }
    }

    /// Discover Cenzontle MCP tools and register them in the tool registry.
    ///
    /// Idempotent: does nothing on subsequent calls.
    pub async fn ensure_initialized(&mut self, registry: &mut ToolRegistry) {
        if self.initialized {
            return;
        }
        self.initialized = true;

        let tools = match self.client.list_mcp_tools().await {
            Ok(t) => t,
            Err(e) => {
                warn!(error = %e, "Failed to discover Cenzontle MCP tools — skipping bridge");
                return;
            }
        };

        if tools.is_empty() {
            debug!("Cenzontle MCP: no tools available");
            return;
        }

        let mut registered = 0;
        for tool_def in tools {
            let prefixed = format!("{}{}", TOOL_PREFIX, tool_def.name);

            // Don't override native tools.
            if registry.get(&prefixed).is_some() {
                debug!(name = %prefixed, "Skipping Cenzontle MCP tool — name collision with native tool");
                continue;
            }

            let tool = CenzontleMcpTool {
                client: Arc::clone(&self.client),
                prefixed_name: prefixed.clone(),
                original_name: tool_def.name.clone(),
                definition: tool_def,
            };

            registry.register(Arc::new(tool));
            registered += 1;
        }

        self.tool_count = registered;
        info!(
            count = registered,
            "Cenzontle MCP bridge: registered tools with '{}' prefix",
            TOOL_PREFIX
        );
    }

    /// Number of Cenzontle MCP tools registered.
    pub fn tool_count(&self) -> usize {
        self.tool_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_prefix_constant() {
        assert_eq!(TOOL_PREFIX, "cenzontle_");
    }

    #[test]
    fn infer_permission_search_is_readonly() {
        let tool = CenzontleMcpTool {
            client: Arc::new(CenzontleAgentClient::new("tok".into(), None)),
            prefixed_name: "cenzontle_knowledge_search".into(),
            original_name: "knowledge_search".into(),
            definition: McpToolDef {
                name: "knowledge_search".into(),
                description: Some("Search knowledge base".into()),
                input_schema: serde_json::json!({}),
            },
        };
        assert_eq!(tool.infer_permission(), PermissionLevel::ReadOnly);
    }

    #[test]
    fn infer_permission_submit_is_destructive() {
        let tool = CenzontleMcpTool {
            client: Arc::new(CenzontleAgentClient::new("tok".into(), None)),
            prefixed_name: "cenzontle_agent_submit_task".into(),
            original_name: "agent_submit_task".into(),
            definition: McpToolDef {
                name: "agent_submit_task".into(),
                description: Some("Submit task to agent".into()),
                input_schema: serde_json::json!({}),
            },
        };
        assert_eq!(tool.infer_permission(), PermissionLevel::Destructive);
    }
}
