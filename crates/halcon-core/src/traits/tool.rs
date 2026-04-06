use async_trait::async_trait;

use crate::error::{HalconError, Result};
use crate::types::{PermissionLevel, ToolInput, ToolOutput};

/// Trait for executable tools (file ops, bash, git, search, etc.).
///
/// Tools are invoked by the agent loop when the model requests tool use.
/// Each tool declares its permission level for the human-in-the-loop system.
///
/// # Execution Contract
///
/// Callers invoke `execute()` which is a **provided method** that:
/// 1. Runs `pre_execute_check()` — tool-specific safety gate (e.g., catastrophic pattern matching)
/// 2. Calls `execute_inner()` — the tool's actual logic
///
/// Tool implementors override `execute_inner()` for their logic and optionally
/// `pre_execute_check()` for tool-specific safety checks.
///
/// The `execute()` method itself MUST NOT be overridden — it is the structural
/// enforcement point ensuring pre-execution checks always run.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique name matching the tool_use name from the model.
    fn name(&self) -> &str;

    /// Human-readable description for the model's tool definition.
    fn description(&self) -> &str;

    /// Permission level required to execute this tool.
    fn permission_level(&self) -> PermissionLevel;

    /// Tool-specific implementation logic. Override this for your tool's behavior.
    ///
    /// **Do NOT call this directly** — use `execute()` instead, which runs
    /// pre-execution safety checks before dispatching to this method.
    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput>;

    /// Pre-execution safety check. Override to add tool-specific safety gates.
    ///
    /// Returns `Ok(())` if execution may proceed, or `Err(reason)` to block.
    ///
    /// **Default:** no-op (returns Ok). Override in tools that execute arbitrary
    /// user/model-provided commands (e.g., `BashTool` checks catastrophic patterns).
    ///
    /// This runs BEFORE any permission pipeline — it is the innermost defense layer.
    fn pre_execute_check(&self, _input: &ToolInput) -> std::result::Result<(), String> {
        Ok(())
    }

    /// Execute the tool with pre-execution safety checks.
    ///
    /// **This is a provided method — do NOT override it.**
    ///
    /// Execution contract:
    /// 1. `pre_execute_check()` — tool-specific safety gate
    /// 2. `execute_inner()` — actual tool logic
    ///
    /// This ensures every `tool.execute()` call — regardless of code path —
    /// runs through the tool's safety check. This is the structural hard-veto
    /// enforcement point (defense-in-depth alongside the permission pipeline).
    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        // STRUCTURAL HARD VETO: runs for EVERY tool invocation, cannot be bypassed.
        self.pre_execute_check(&input).map_err(|reason| {
            HalconError::SecurityBlocked(format!(
                "Pre-execution check failed for '{}': {}",
                self.name(),
                reason
            ))
        })?;
        self.execute_inner(input).await
    }

    /// Whether this specific invocation requires user confirmation.
    ///
    /// Default: true for Destructive tools, false otherwise.
    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        self.permission_level() >= PermissionLevel::Destructive
    }

    /// Whether this tool can safely run concurrently with other tools.
    ///
    /// Default: true for ReadOnly tools, false for ReadWrite/Destructive.
    /// Tools that only read state (file reads, searches, web fetches) return true.
    /// Tools that mutate state (bash, file writes) return false.
    fn is_concurrency_safe(&self, _input: &ToolInput) -> bool {
        self.permission_level() == PermissionLevel::ReadOnly
    }

    /// JSON Schema for the tool's input parameters (for model API).
    fn input_schema(&self) -> serde_json::Value;
}
