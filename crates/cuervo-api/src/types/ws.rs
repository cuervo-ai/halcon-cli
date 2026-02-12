use serde::{Deserialize, Serialize};
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
}
