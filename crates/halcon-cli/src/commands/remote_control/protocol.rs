//! Protocol types for the remote-control system.
//!
//! Shared between CLI client and backend. All types derive Serialize + Deserialize
//! for JSON transport over REST and WebSocket.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// ── Commands (CLI → Backend) ────────────────────────────────────────────────

/// A command sent from the CLI to the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "command")]
pub enum RemoteCommand {
    /// Approve a pending permission request.
    ApprovePermission {
        session_id: String,
        request_id: Uuid,
    },
    /// Reject a pending permission request.
    RejectPermission {
        session_id: String,
        request_id: Uuid,
    },
    /// Submit a new execution plan.
    Replan {
        session_id: String,
        payload: ReplanPayload,
    },
    /// Cancel a running session/task.
    Cancel { session_id: String },
    /// Inject context into the running session.
    InjectContext {
        session_id: String,
        context: String,
    },
    /// Submit a user message (human-in-the-loop chat).
    SubmitMessage {
        session_id: String,
        content: String,
        orchestrate: bool,
    },
}

/// A plan to replace the current execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplanPayload {
    /// Human-readable description of the plan.
    pub description: String,
    /// Ordered steps to execute.
    pub steps: Vec<ReplanStep>,
    /// Optional metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// A single step in a replan payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplanStep {
    /// Step identifier.
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// Tool to execute (optional — some steps are LLM-only).
    pub tool: Option<String>,
    /// Tool arguments.
    #[serde(default)]
    pub args: HashMap<String, serde_json::Value>,
    /// Steps that must complete before this one.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

// ── Events (Backend → CLI) ──────────────────────────────────────────────────

/// An event received from the backend via WebSocket.
///
/// This is a subset of WsServerEvent relevant to remote-control.
/// The full WsServerEvent is also supported via pass-through deserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RemoteControlEvent {
    // Permission lifecycle
    PermissionRequired {
        request_id: Uuid,
        session_id: Uuid,
        tool_name: String,
        risk_level: String,
        args_preview: HashMap<String, String>,
        description: String,
        deadline_secs: u64,
    },
    PermissionResolved {
        request_id: Uuid,
        session_id: Uuid,
        decision: String,
        tool_executed: bool,
    },
    PermissionExpired {
        request_id: Uuid,
        session_id: Uuid,
        deadline_elapsed_ms: u64,
    },

    // Task/execution lifecycle
    TaskProgress(TaskProgressInfo),
    ToolExecuted {
        name: String,
        tool_use_id: String,
        duration_ms: u64,
        success: bool,
    },

    // Chat streaming
    ChatStreamToken {
        session_id: Uuid,
        token: String,
        is_thinking: bool,
        sequence_num: u64,
    },

    // Sub-agent lifecycle
    SubAgentStarted {
        session_id: Uuid,
        sub_agent_id: String,
        task_description: String,
        wave: usize,
        allowed_tools: Vec<String>,
    },
    SubAgentCompleted {
        session_id: Uuid,
        sub_agent_id: String,
        success: bool,
        summary: String,
        tools_used: Vec<String>,
        duration_ms: u64,
    },

    // Execution terminal events
    ConversationCompleted {
        session_id: Uuid,
        stop_reason: String,
        total_duration_ms: u64,
    },
    ExecutionFailed {
        session_id: Uuid,
        error_code: String,
        message: String,
        recoverable: bool,
    },

    // Session lifecycle
    ChatSessionCreated {
        session_id: Uuid,
        model: String,
        provider: String,
    },
    ChatSessionDeleted {
        session_id: Uuid,
    },

    // Connection
    Connected {
        server_version: String,
    },
    Pong,

    // System
    Error {
        code: String,
        message: String,
    },

    // Replan acknowledgement (custom for remote-control)
    ReplanAccepted {
        session_id: Uuid,
        step_count: usize,
    },
    ReplanRejected {
        session_id: Uuid,
        reason: String,
    },

    // Catch-all for unknown events (forward compatibility).
    #[serde(other)]
    Unknown,
}

/// Task progress info (subset of TaskProgressEvent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgressInfo {
    pub execution_id: Uuid,
    pub node_id: String,
    pub status: String,
    pub progress_pct: Option<f32>,
    pub message: Option<String>,
}

// ── Session Info ────────────────────────────────────────────────────────────

/// Session information returned by the status endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSessionInfo {
    pub id: String,
    pub title: Option<String>,
    pub model: String,
    pub provider: String,
    pub status: String,
    pub message_count: usize,
}

// ── Remote-Control Session Status ───────────────────────────────────────────

/// Aggregated status of a remote-controlled session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteControlStatus {
    pub session: RemoteSessionInfo,
    pub pending_permissions: Vec<PendingPermission>,
    pub recent_tools: Vec<RecentToolExecution>,
    pub active_sub_agents: Vec<ActiveSubAgent>,
}

/// A permission request waiting for user decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPermission {
    pub request_id: Uuid,
    pub tool_name: String,
    pub risk_level: String,
    pub description: String,
    pub deadline_secs: u64,
    pub args_preview: HashMap<String, String>,
}

/// A recently executed tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentToolExecution {
    pub name: String,
    pub duration_ms: u64,
    pub success: bool,
}

/// An active sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveSubAgent {
    pub id: String,
    pub description: String,
    pub wave: usize,
}

// ── Protocol Version ────────────────────────────────────────────────────────

/// Current protocol version.
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// WebSocket subscribe message for remote-control channels.
pub fn subscribe_message() -> String {
    serde_json::json!({
        "type": "subscribe",
        "channels": ["chat", "permissions", "tasks", "execution", "sub_agents", "tools"]
    })
    .to_string()
}
