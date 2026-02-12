//! Federation protocol for inter-agent communication.
//!
//! Typed message protocol for delegation, context sharing, and lifecycle coordination.

pub mod router;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::{AgentBudget, AgentDescriptor, AgentHealth, AgentUsage};

/// A federation message between agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationMessage {
    pub id: Uuid,
    pub from: Uuid,
    /// None = broadcast to all agents.
    pub to: Option<Uuid>,
    pub kind: FederationMessageKind,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl FederationMessage {
    pub fn new(from: Uuid, to: Option<Uuid>, kind: FederationMessageKind) -> Self {
        Self {
            id: Uuid::new_v4(),
            from,
            to,
            kind,
            timestamp: chrono::Utc::now(),
        }
    }

    pub fn is_broadcast(&self) -> bool {
        self.to.is_none()
    }
}

/// The specific kind of federation message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FederationMessageKind {
    // Discovery
    Announce(AgentDescriptor),

    // Delegation
    DelegateTask {
        instruction: String,
        budget: Option<AgentBudget>,
        #[serde(default)]
        context: HashMap<String, serde_json::Value>,
    },
    DelegateResult {
        success: bool,
        output: String,
        usage: AgentUsage,
    },

    // Context sharing
    ContextUpdate {
        key: String,
        value: serde_json::Value,
    },
    ContextRequest {
        key: String,
    },
    ContextResponse {
        key: String,
        value: Option<serde_json::Value>,
    },

    // Lifecycle
    Ping,
    Pong {
        health: AgentHealth,
    },
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentKind, ProtocolSupport};

    fn test_descriptor() -> AgentDescriptor {
        AgentDescriptor {
            id: Uuid::from_u128(1),
            name: "test".to_string(),
            agent_kind: AgentKind::Llm,
            capabilities: vec![],
            protocols: vec![ProtocolSupport::Native],
            metadata: HashMap::new(),
            max_concurrency: 1,
        }
    }

    #[test]
    fn federation_message_construction() {
        let from = Uuid::from_u128(1);
        let to = Uuid::from_u128(2);
        let msg = FederationMessage::new(from, Some(to), FederationMessageKind::Ping);
        assert_eq!(msg.from, from);
        assert_eq!(msg.to, Some(to));
        assert!(!msg.is_broadcast());
    }

    #[test]
    fn federation_broadcast() {
        let msg = FederationMessage::new(
            Uuid::from_u128(1),
            None,
            FederationMessageKind::Shutdown,
        );
        assert!(msg.is_broadcast());
    }

    #[test]
    fn announce_serde_roundtrip() {
        let desc = test_descriptor();
        let msg = FederationMessage::new(
            desc.id,
            None,
            FederationMessageKind::Announce(desc.clone()),
        );
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: FederationMessage = serde_json::from_str(&json).unwrap();
        if let FederationMessageKind::Announce(d) = &parsed.kind {
            assert_eq!(d.name, "test");
        } else {
            panic!("expected Announce");
        }
    }

    #[test]
    fn delegate_task_serde() {
        let msg = FederationMessage::new(
            Uuid::from_u128(1),
            Some(Uuid::from_u128(2)),
            FederationMessageKind::DelegateTask {
                instruction: "fix bug".to_string(),
                budget: None,
                context: HashMap::new(),
            },
        );
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: FederationMessage = serde_json::from_str(&json).unwrap();
        if let FederationMessageKind::DelegateTask { instruction, .. } = &parsed.kind {
            assert_eq!(instruction, "fix bug");
        } else {
            panic!("expected DelegateTask");
        }
    }

    #[test]
    fn delegate_result_serde() {
        let msg = FederationMessage::new(
            Uuid::from_u128(2),
            Some(Uuid::from_u128(1)),
            FederationMessageKind::DelegateResult {
                success: true,
                output: "done".to_string(),
                usage: AgentUsage::default(),
            },
        );
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: FederationMessage = serde_json::from_str(&json).unwrap();
        if let FederationMessageKind::DelegateResult { success, output, .. } = &parsed.kind {
            assert!(success);
            assert_eq!(output, "done");
        } else {
            panic!("expected DelegateResult");
        }
    }

    #[test]
    fn context_update_serde() {
        let msg = FederationMessage::new(
            Uuid::from_u128(1),
            None,
            FederationMessageKind::ContextUpdate {
                key: "result".to_string(),
                value: serde_json::json!({"status": "ok"}),
            },
        );
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: FederationMessage = serde_json::from_str(&json).unwrap();
        if let FederationMessageKind::ContextUpdate { key, value } = &parsed.kind {
            assert_eq!(key, "result");
            assert_eq!(value["status"], "ok");
        } else {
            panic!("expected ContextUpdate");
        }
    }

    #[test]
    fn ping_pong_serde() {
        let ping = FederationMessage::new(
            Uuid::from_u128(1),
            Some(Uuid::from_u128(2)),
            FederationMessageKind::Ping,
        );
        let json = serde_json::to_string(&ping).unwrap();
        let parsed: FederationMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed.kind, FederationMessageKind::Ping));

        let pong = FederationMessage::new(
            Uuid::from_u128(2),
            Some(Uuid::from_u128(1)),
            FederationMessageKind::Pong {
                health: AgentHealth::Healthy,
            },
        );
        let json = serde_json::to_string(&pong).unwrap();
        let parsed: FederationMessage = serde_json::from_str(&json).unwrap();
        if let FederationMessageKind::Pong { health } = &parsed.kind {
            assert_eq!(*health, AgentHealth::Healthy);
        } else {
            panic!("expected Pong");
        }
    }
}
