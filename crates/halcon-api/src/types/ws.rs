use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use super::agent::{AgentInfo, HealthStatus, UsageInfo};
use super::observability::{LogEntry, MetricPoint};
use super::protocol::ProtocolMessageInfo;
use super::task::TaskProgressEvent;

/// Message sent from client to server over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum WsClientMessage {
    /// Subscribe to event channels.
    Subscribe { channels: Vec<WsChannel> },
    /// Unsubscribe from event channels.
    Unsubscribe { channels: Vec<WsChannel> },
    /// Keepalive ping.
    Ping,
}

/// Event sent from server to client over WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum WsServerEvent {
    // Agent lifecycle
    AgentRegistered {
        agent: AgentInfo,
    },
    AgentDeregistered {
        id: Uuid,
    },
    AgentHealthChanged {
        id: Uuid,
        health: HealthStatus,
    },
    AgentInvoked {
        id: Uuid,
        request_id: Uuid,
    },
    AgentCompleted {
        id: Uuid,
        request_id: Uuid,
        success: bool,
        usage: UsageInfo,
    },

    // Task lifecycle
    TaskSubmitted {
        execution_id: Uuid,
        node_count: usize,
    },
    TaskProgress(TaskProgressEvent),
    TaskCompleted {
        execution_id: Uuid,
        success: bool,
        usage: UsageInfo,
    },

    // Tool events
    ToolExecuted {
        name: String,
        tool_use_id: String,
        duration_ms: u64,
        success: bool,
    },

    // Observability
    Log(LogEntry),
    Metric(MetricPoint),
    Protocol(ProtocolMessageInfo),

    // System
    ConfigChanged {
        section: String,
    },
    SystemHealthChanged {
        health: String,
    },
    Error {
        code: String,
        message: String,
    },
    Pong,
    Connected {
        server_version: String,
    },
    // Chat streaming events
    ChatStreamToken {
        session_id: Uuid,
        token: String,
        is_thinking: bool,
        sequence_num: u64,
    },
    ThinkingProgress {
        session_id: Uuid,
        chars_so_far: usize,
        elapsed_secs: f32,
    },

    // Permission events
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
    /// B1: A permission request expired before the user responded.
    /// The tool was automatically denied (fail-closed).
    /// Clients MUST dismiss the pending permission modal when this event arrives.
    PermissionExpired {
        request_id: Uuid,
        session_id: Uuid,
        /// Milliseconds past the original deadline when the timeout fired.
        deadline_elapsed_ms: u64,
    },

    // Sub-agent lifecycle events
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

    // Execution lifecycle events
    ExecutionFailed {
        session_id: Uuid,
        error_code: String,
        message: String,
        recoverable: bool,
    },
    ConversationCompleted {
        session_id: Uuid,
        assistant_message_id: Uuid,
        stop_reason: String,
        usage: super::chat::ChatTokenUsage,
        total_duration_ms: u64,
    },

    // Chat session lifecycle
    ChatSessionCreated {
        session_id: Uuid,
        model: String,
        provider: String,
    },
    ChatSessionDeleted {
        session_id: Uuid,
    },

    // Remote-control events
    /// A replan was accepted and will be executed.
    RemoteControlReplanAccepted {
        session_id: Uuid,
        step_count: usize,
    },
    /// A replan was rejected (invalid DAG or authorization failure).
    RemoteControlReplanRejected {
        session_id: Uuid,
        reason: String,
    },

    // Media attachment processing
    /// Fired when the server begins processing inline attachments.
    MediaAnalysisStarted {
        session_id: Uuid,
        /// Total number of attachments being processed.
        file_count: usize,
    },
    /// Progress update for a single attachment.
    MediaAnalysisProgress {
        session_id: Uuid,
        /// 0-based index of the attachment being processed.
        index: usize,
        total: usize,
        filename: String,
        modality: String,
    },
    /// Fired when all attachment processing is complete and the turn is about to start.
    MediaAnalysisCompleted {
        session_id: Uuid,
        /// Number of attachments successfully processed.
        processed: usize,
        /// True if the analysis description was injected into the system prompt.
        context_injected: bool,
    },
}

/// Available WebSocket subscription channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsChannel {
    Agents,
    Tasks,
    Tools,
    Logs,
    Metrics,
    Protocols,
    System,
    All,
    Chat,
    Permissions,
    SubAgents,
    Execution,
}
