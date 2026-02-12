use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Protocol type for message classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolType {
    Federation,
    Mcp,
    A2A,
    Internal,
}

/// Direction of a protocol message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    Inbound,
    Outbound,
    Internal,
}

/// Information about a protocol message for debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolMessageInfo {
    pub id: Uuid,
    pub protocol: ProtocolType,
    pub direction: MessageDirection,
    pub from_agent: Option<Uuid>,
    pub to_agent: Option<Uuid>,
    pub message_type: String,
    pub timestamp: DateTime<Utc>,
    pub payload_size_bytes: usize,
    pub payload: serde_json::Value,
    pub latency_ms: Option<u64>,
}

/// Query parameters for listing protocol messages.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListMessagesQuery {
    pub protocol: Option<ProtocolType>,
    pub direction: Option<MessageDirection>,
    pub agent_id: Option<Uuid>,
    pub message_type: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}
