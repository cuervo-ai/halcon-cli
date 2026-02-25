//! Domain types for the AgentBridge protocol.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Token usage for a single conversation turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatTokenUsage {
    pub input: u64,
    pub output: u64,
    pub thinking: u64,
}

impl ChatTokenUsage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.thinking
    }
}

/// Role of a message in the conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnRole {
    User,
    Assistant,
    System,
}

/// A message in the conversation history passed to the bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnMessage {
    pub role: TurnRole,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

/// Context for a single agent execution turn.
#[derive(Debug, Clone)]
pub struct TurnContext {
    pub session_id: Uuid,
    pub user_message: String,
    pub model: String,
    pub provider: String,
    pub history: Vec<TurnMessage>,
    pub working_directory: String,
    pub orchestrate: bool,
    pub expert: bool,
    pub system_prompt: Option<String>,
    /// Inline media attachments forwarded from the API request.
    pub media_attachments: Vec<halcon_core::traits::MediaAttachmentInline>,
}

/// Result of executing a single agent turn.
#[derive(Debug, Clone)]
pub struct TurnResult {
    pub assistant_text: String,
    pub stop_reason: String,
    pub usage: ChatTokenUsage,
    pub duration_ms: u64,
    pub tools_executed: Vec<String>,
    pub rounds: u32,
    pub strategy_used: String,
}

/// Events emitted by the agent pipeline during execution.
#[derive(Debug, Clone)]
pub enum AgentStreamEvent {
    /// A text token from the model output.
    OutputToken { token: String, sequence_num: u64 },
    /// A thinking/reasoning token.
    ThinkingToken { token: String },
    /// Periodic thinking progress update (throttled to 500ms).
    ThinkingProgressUpdate { chars_so_far: usize, elapsed_secs: f32 },
    /// A tool invocation has started.
    ToolStarted {
        name: String,
        risk_level: String,
        input: serde_json::Value,
    },
    /// A tool invocation completed.
    ToolCompleted {
        name: String,
        duration_ms: u64,
        success: bool,
    },
    /// A permission is required before executing a tool.
    PermissionRequested {
        request_id: Uuid,
        tool_name: String,
        risk_level: String,
        args_preview: HashMap<String, String>,
        description: String,
        deadline_secs: u64,
    },
    /// A permission request was resolved.
    PermissionResolved {
        request_id: Uuid,
        decision: String,
        tool_executed: bool,
    },
    /// A permission request timed out — the tool was denied (fail-closed).
    /// B1: Allows the UI to dismiss the pending modal deterministically.
    PermissionExpired {
        request_id: Uuid,
        /// How long (ms) past the deadline the timeout fired.
        deadline_elapsed_ms: u64,
    },
    /// A sub-agent was started by the orchestrator.
    SubAgentStarted {
        sub_agent_id: String,
        task_description: String,
        wave: usize,
        allowed_tools: Vec<String>,
    },
    /// A sub-agent completed.
    SubAgentCompleted {
        sub_agent_id: String,
        success: bool,
        summary: String,
        tools_used: Vec<String>,
        duration_ms: u64,
    },
    /// The turn completed successfully.
    TurnCompleted {
        assistant_message_id: Uuid,
        stop_reason: String,
        usage: ChatTokenUsage,
        total_duration_ms: u64,
    },
    /// The turn failed irrecoverably.
    TurnFailed {
        error_code: String,
        message: String,
        recoverable: bool,
    },
}

/// A pending permission request presented to the PermissionHandler.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub request_id: Uuid,
    pub session_id: Uuid,
    pub tool_name: String,
    pub risk_level: String,
    pub args_preview: HashMap<String, String>,
    pub description: String,
    pub deadline_secs: u64,
}

/// Outcome of a permission decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecisionKind {
    Approved,
    Rejected,
    TimedOut,
}

/// Errors from the AgentBridge execution layer.
#[derive(Debug, thiserror::Error)]
pub enum AgentBridgeError {
    #[error("provider not found: {0}")]
    ProviderNotFound(String),
    #[error("agent execution failed: {0}")]
    ExecutionFailed(String),
    #[error("cancelled by user")]
    CancelledByUser,
    #[error("permission channel closed")]
    PermissionChannelClosed,
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}
