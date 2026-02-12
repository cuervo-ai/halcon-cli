//! Pluggable transport abstraction for agent communication.

pub mod channel;
pub mod http;
pub mod stdio;

use std::fmt;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;

/// Trait for bidirectional agent communication channels.
#[async_trait]
pub trait AgentTransport: Send + Sync {
    /// Send a message to the remote agent.
    async fn send(&self, message: TransportMessage) -> Result<()>;

    /// Receive the next message from the remote agent.
    async fn receive(&self) -> Result<TransportMessage>;

    /// Close the transport connection.
    async fn close(&self) -> Result<()>;

    /// Whether the transport is currently connected.
    fn is_connected(&self) -> bool;
}

/// A message sent over a transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportMessage {
    pub id: Uuid,
    pub kind: TransportMessageKind,
    pub payload: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

impl TransportMessage {
    pub fn new(kind: TransportMessageKind, payload: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            payload,
            timestamp: Utc::now(),
        }
    }
}

/// The kind of transport message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMessageKind {
    Request,
    Response,
    Heartbeat,
    Shutdown,
    Error,
}

impl fmt::Display for TransportMessageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportMessageKind::Request => write!(f, "request"),
            TransportMessageKind::Response => write!(f, "response"),
            TransportMessageKind::Heartbeat => write!(f, "heartbeat"),
            TransportMessageKind::Shutdown => write!(f, "shutdown"),
            TransportMessageKind::Error => write!(f, "error"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_message_construction() {
        let msg = TransportMessage::new(TransportMessageKind::Request, serde_json::json!({"key": "value"}));
        assert_eq!(msg.kind, TransportMessageKind::Request);
        assert!(!msg.id.is_nil());
    }

    #[test]
    fn transport_message_serde_roundtrip() {
        let msg = TransportMessage::new(TransportMessageKind::Response, serde_json::json!("ok"));
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: TransportMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, msg.id);
        assert_eq!(parsed.kind, TransportMessageKind::Response);
    }

    #[test]
    fn transport_message_kind_display() {
        assert_eq!(TransportMessageKind::Request.to_string(), "request");
        assert_eq!(TransportMessageKind::Heartbeat.to_string(), "heartbeat");
        assert_eq!(TransportMessageKind::Shutdown.to_string(), "shutdown");
    }

    #[test]
    fn transport_message_kind_serde() {
        let kinds = vec![
            TransportMessageKind::Request,
            TransportMessageKind::Response,
            TransportMessageKind::Heartbeat,
            TransportMessageKind::Shutdown,
            TransportMessageKind::Error,
        ];
        for kind in kinds {
            let json = serde_json::to_string(&kind).unwrap();
            let parsed: TransportMessageKind = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn transport_message_with_complex_payload() {
        let payload = serde_json::json!({
            "instruction": "write code",
            "context": {"file": "main.rs"},
            "nested": [1, 2, 3]
        });
        let msg = TransportMessage::new(TransportMessageKind::Request, payload);
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: TransportMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.payload["instruction"], "write code");
    }
}
