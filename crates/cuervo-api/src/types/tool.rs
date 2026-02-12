use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Permission level for a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    ReadOnly,
    ReadWrite,
    Destructive,
}

/// Information about a registered tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub permission_level: PermissionLevel,
    pub enabled: bool,
    pub requires_confirmation: bool,
    pub execution_count: u64,
    pub last_executed: Option<DateTime<Utc>>,
    pub input_schema: serde_json::Value,
}

/// Request to toggle a tool's enabled state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToggleToolRequest {
    pub enabled: bool,
}

/// A single tool execution record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionRecord {
    pub tool_name: String,
    pub tool_use_id: String,
    pub input_summary: String,
    pub output_summary: String,
    pub is_error: bool,
    pub duration_ms: u64,
    pub executed_at: DateTime<Utc>,
}

/// Query parameters for tool execution history.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolHistoryQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}
