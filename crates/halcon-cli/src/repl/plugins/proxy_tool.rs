//! Plugin Proxy Tool — wraps a plugin capability as a first-class `Tool` impl.
//!
//! The executor registers these in the session `ToolRegistry` at plugin load time.
//! The model sees them as ordinary tools; internally they forward invocations
//! through the `PluginTransportRuntime`.

use std::sync::Arc;

use async_trait::async_trait;
use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

use super::manifest::ToolCapabilityDescriptor;
use super::transport::PluginTransportRuntime;

// ─── Proxy Tool ───────────────────────────────────────────────────────────────

/// A `Tool` implementation that proxies calls to an external plugin.
///
/// One `PluginProxyTool` is created per capability exposed by a plugin.
/// All proxy tools for the same plugin share an `Arc<PluginTransportRuntime>`.
pub struct PluginProxyTool {
    /// Prefixed tool name: `plugin_<id_underscored>_<capability>`.
    tool_name: String,
    /// The owning plugin's ID (for transport dispatch).
    plugin_id: String,
    /// Capability metadata (description, permission level, budget).
    descriptor: ToolCapabilityDescriptor,
    /// Shared transport handle (Stdio / HTTP / Local).
    runtime: Arc<PluginTransportRuntime>,
    /// Per-call timeout sourced from plugin's sandbox contract.
    timeout_ms: u64,
}

impl PluginProxyTool {
    /// Create a new proxy tool.
    pub fn new(
        tool_name: String,
        plugin_id: String,
        descriptor: ToolCapabilityDescriptor,
        runtime: Arc<PluginTransportRuntime>,
        timeout_ms: u64,
    ) -> Self {
        Self {
            tool_name,
            plugin_id,
            descriptor,
            runtime,
            timeout_ms,
        }
    }
}

#[async_trait]
impl Tool for PluginProxyTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.descriptor.description
    }

    fn permission_level(&self) -> PermissionLevel {
        self.descriptor.permission_level
    }

    fn input_schema(&self) -> serde_json::Value {
        // Generic schema: accept any JSON object.
        // Phase 9: individual plugins will declare typed schemas.
        serde_json::json!({
            "type": "object",
            "description": self.descriptor.description,
            "additionalProperties": true
        })
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let result = self
            .runtime
            .invoke(
                &self.plugin_id,
                &self.tool_name,
                input.arguments.clone(),
                self.timeout_ms,
            )
            .await
            .map_err(|e| HalconError::ToolExecutionFailed {
                tool: self.tool_name.clone(),
                message: e.to_string(),
            })?;

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: result.content,
            is_error: result.is_error,
            metadata: Some(serde_json::json!({
                "plugin_id": self.plugin_id,
                "tokens_used": result.tokens_used,
                "cost_usd": result.cost_usd,
                "latency_ms": result.latency_ms,
            })),
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::plugins::manifest::{RiskTier, ToolCapabilityDescriptor};
    use crate::repl::plugins::transport::{PluginTransportRuntime, TransportHandle};

    fn make_runtime(plugin_id: &str) -> Arc<PluginTransportRuntime> {
        let mut rt = PluginTransportRuntime::new();
        rt.register(plugin_id.to_string(), TransportHandle::Local);
        Arc::new(rt)
    }

    fn make_descriptor(name: &str) -> ToolCapabilityDescriptor {
        ToolCapabilityDescriptor {
            name: name.to_string(),
            description: format!("Does {name}"),
            risk_tier: RiskTier::Low,
            idempotent: true,
            permission_level: PermissionLevel::ReadOnly,
            budget_tokens_per_call: 100,
        }
    }

    fn make_tool(plugin_id: &str, tool_name: &str) -> PluginProxyTool {
        let rt = make_runtime(plugin_id);
        let desc = make_descriptor(tool_name);
        PluginProxyTool::new(tool_name.to_string(), plugin_id.to_string(), desc, rt, 5000)
    }

    fn make_input(tool_name: &str) -> ToolInput {
        ToolInput {
            tool_use_id: format!("use_{tool_name}"),
            arguments: serde_json::json!({"key": "value"}),
            working_directory: "/tmp".to_string(),
        }
    }

    #[test]
    fn name_returns_tool_name() {
        let tool = make_tool("my-plugin", "plugin_my_plugin_run");
        assert_eq!(tool.name(), "plugin_my_plugin_run");
    }

    #[test]
    fn description_matches_descriptor() {
        let tool = make_tool("p1", "plugin_p1_echo");
        assert!(tool.description().contains("echo"));
    }

    #[test]
    fn permission_level_matches_descriptor() {
        let tool = make_tool("p1", "plugin_p1_run");
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
    }

    #[test]
    fn input_schema_is_valid_json_object() {
        let tool = make_tool("p1", "plugin_p1_run");
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
    }

    #[tokio::test]
    async fn execute_local_returns_ok() {
        let tool = make_tool("my-plugin", "plugin_my_plugin_task");
        let input = make_input("plugin_my_plugin_task");
        let output = tool.execute(input.clone()).await.unwrap();
        assert_eq!(output.tool_use_id, input.tool_use_id);
        assert!(!output.is_error);
    }

    #[tokio::test]
    async fn execute_propagates_tool_use_id() {
        let tool = make_tool("p-test", "plugin_p_test_process");
        let input = ToolInput {
            tool_use_id: "unique-id-xyz".to_string(),
            arguments: serde_json::json!({}),
            working_directory: "/tmp".to_string(),
        };
        let output = tool.execute(input).await.unwrap();
        assert_eq!(output.tool_use_id, "unique-id-xyz");
    }
}
