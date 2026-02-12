use async_trait::async_trait;

use crate::error::Result;
use crate::types::{PermissionLevel, ToolInput, ToolOutput};

/// Trait for executable tools (file ops, bash, git, search, etc.).
///
/// Tools are invoked by the agent loop when the model requests tool use.
/// Each tool declares its permission level for the human-in-the-loop system.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique name matching the tool_use name from the model.
    fn name(&self) -> &str;

    /// Human-readable description for the model's tool definition.
    fn description(&self) -> &str;

    /// Permission level required to execute this tool.
    fn permission_level(&self) -> PermissionLevel;

    /// Execute the tool with the given input.
    async fn execute(&self, input: ToolInput) -> Result<ToolOutput>;

    /// Whether this specific invocation requires user confirmation.
    ///
    /// Default: true for Destructive tools, false otherwise.
    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        self.permission_level() >= PermissionLevel::Destructive
    }

    /// JSON Schema for the tool's input parameters (for model API).
    fn input_schema(&self) -> serde_json::Value;
}
