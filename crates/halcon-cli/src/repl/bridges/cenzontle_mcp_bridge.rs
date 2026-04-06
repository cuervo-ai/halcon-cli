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
/// Known read-only Cenzontle MCP tool names (safe to execute without confirmation).
const KNOWN_READONLY_TOOLS: &[&str] = &[
    "knowledge_search",
    "agent_list",
    "llm_chat", // LLM chat is read-only (no side effects on server)
];

struct CenzontleMcpTool {
    client: Arc<CenzontleAgentClient>,
    /// Prefixed tool name (e.g. "cenzontle_llm_chat").
    prefixed_name: String,
    /// Original tool name on the Cenzontle side (e.g. "llm_chat").
    original_name: String,
    definition: McpToolDef,
    /// Cached permission level (computed once at construction).
    permission: PermissionLevel,
}

/// Infer permission from tool name. Defaults to Destructive for safety —
/// only explicitly allow-listed tools get ReadOnly.
fn infer_permission_for_tool(name: &str) -> PermissionLevel {
    if KNOWN_READONLY_TOOLS.contains(&name) {
        PermissionLevel::ReadOnly
    } else {
        // Default to Destructive — server-side tools like agent_submit_task
        // and conversation_create have side effects.
        PermissionLevel::Destructive
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
        self.permission
    }

    async fn execute_inner(&self, input: ToolInput) -> HalconResult<ToolOutput> {
        let req = McpToolCallRequest {
            name: self.original_name.clone(),
            arguments: input.arguments,
        };

        let resp = self.client.call_mcp_tool(&req).await.map_err(|e| {
            HalconError::ToolExecutionFailed {
                tool: self.prefixed_name.clone(),
                message: format!("Cenzontle MCP tool call failed: {e}"),
            }
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

        // Timeout tool discovery to avoid blocking REPL startup.
        let tools = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.client.list_mcp_tools(),
        )
        .await
        {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => {
                warn!(error = %e, "Failed to discover Cenzontle MCP tools — skipping bridge");
                return;
            }
            Err(_) => {
                warn!("Cenzontle MCP tool discovery timed out (10s) — skipping bridge");
                return;
            }
        };

        if tools.is_empty() {
            debug!("Cenzontle MCP: no tools available");
            return;
        }

        let mut registered = 0;
        let mut seen_names = std::collections::HashSet::new();
        for tool_def in tools {
            // Deduplicate: if the backend returns the same tool name twice, skip.
            if !seen_names.insert(tool_def.name.clone()) {
                debug!(name = %tool_def.name, "Skipping duplicate Cenzontle MCP tool");
                continue;
            }

            let prefixed = format!("{}{}", TOOL_PREFIX, tool_def.name);

            // Don't override native tools.
            if registry.get(&prefixed).is_some() {
                debug!(name = %prefixed, "Skipping Cenzontle MCP tool — name collision with native tool");
                continue;
            }

            let permission = infer_permission_for_tool(&tool_def.name);
            let tool = CenzontleMcpTool {
                client: Arc::clone(&self.client),
                prefixed_name: prefixed.clone(),
                original_name: tool_def.name.clone(),
                definition: tool_def,
                permission,
            };

            registry.register(Arc::new(tool));
            registered += 1;
        }

        self.tool_count = registered;
        info!(
            count = registered,
            "Cenzontle MCP bridge: registered tools with '{}' prefix", TOOL_PREFIX
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
        assert_eq!(
            infer_permission_for_tool("knowledge_search"),
            PermissionLevel::ReadOnly
        );
        assert_eq!(
            infer_permission_for_tool("agent_list"),
            PermissionLevel::ReadOnly
        );
        assert_eq!(
            infer_permission_for_tool("llm_chat"),
            PermissionLevel::ReadOnly
        );
    }

    #[test]
    fn infer_permission_submit_is_destructive() {
        assert_eq!(
            infer_permission_for_tool("agent_submit_task"),
            PermissionLevel::Destructive
        );
        assert_eq!(
            infer_permission_for_tool("conversation_create"),
            PermissionLevel::Destructive
        );
        // Unknown tools default to destructive for safety.
        assert_eq!(
            infer_permission_for_tool("some_new_dangerous_tool"),
            PermissionLevel::Destructive
        );
    }
}
