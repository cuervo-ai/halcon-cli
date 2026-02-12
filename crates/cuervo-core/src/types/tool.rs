use serde::{Deserialize, Serialize};

/// Permission level required to execute a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    /// Read-only operations: file_read, glob, grep, git_status.
    ReadOnly,
    /// Read-write operations: file_write, file_edit.
    ReadWrite,
    /// Destructive operations: bash, git_push, file_delete.
    Destructive,
}

impl std::fmt::Display for PermissionLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PermissionLevel::ReadOnly => write!(f, "read-only"),
            PermissionLevel::ReadWrite => write!(f, "read-write"),
            PermissionLevel::Destructive => write!(f, "destructive"),
        }
    }
}

/// Input passed to a tool for execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInput {
    pub tool_use_id: String,
    pub arguments: serde_json::Value,
    pub working_directory: String,
}

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Permission decision for a tool execution request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// User granted permission for this execution.
    Allowed,
    /// User granted permission for all executions of this tool in this session.
    AllowedAlways,
    /// User denied permission.
    Denied,
}
