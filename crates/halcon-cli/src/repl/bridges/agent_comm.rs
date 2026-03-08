//! Inter-agent communication hub for orchestrator coordination.
//!
//! Provides typed message passing between sub-agents and a shared
//! context store (blackboard) for cross-agent data sharing.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

/// A typed message between agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    /// Sender agent ID.
    pub from: Uuid,
    /// Target agent ID (None = broadcast to all).
    pub to: Option<Uuid>,
    /// Message type.
    pub kind: AgentMessageKind,
    /// Arbitrary payload data.
    pub data: serde_json::Value,
    /// When the message was created.
    pub timestamp: DateTime<Utc>,
}

/// Categories of inter-agent messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMessageKind {
    /// Share context/data with another agent or broadcast.
    ContextShare,
    /// Request delegation of a sub-task.
    DelegateRequest,
    /// Response to a delegation request.
    DelegateResponse,
    /// Signal that an agent has completed its task.
    CompletionSignal,
    /// Signal that an agent encountered an error.
    ErrorSignal,
}

/// Thread-safe shared context store (blackboard pattern).
///
/// Allows agents to read/write shared key-value data concurrently.
#[derive(Debug, Clone)]
pub struct SharedContextStore {
    inner: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}

impl SharedContextStore {
    /// Create a new empty shared context store.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set a key-value pair in the shared context.
    pub async fn set(&self, key: String, value: serde_json::Value) {
        self.inner.write().await.insert(key, value);
    }

    /// Get a value by key from the shared context.
    #[allow(dead_code)] // Used in tests; production will use it via delegation.
    pub async fn get(&self, key: &str) -> Option<serde_json::Value> {
        self.inner.read().await.get(key).cloned()
    }

    /// Get all keys in the shared context.
    #[allow(dead_code)] // Used in tests; production will use it via delegation.
    pub async fn keys(&self) -> Vec<String> {
        self.inner.read().await.keys().cloned().collect()
    }

    /// Take a snapshot of the entire shared context.
    pub async fn snapshot(&self) -> HashMap<String, serde_json::Value> {
        self.inner.read().await.clone()
    }
}

impl Default for SharedContextStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Cloneable sender handle for inter-agent communication.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct AgentCommSender {
    senders: Arc<HashMap<Uuid, mpsc::Sender<AgentMessage>>>,
}

impl AgentCommSender {
    /// Send a message to a specific agent.
    #[allow(dead_code)]
    pub async fn send_to(&self, target: Uuid, msg: AgentMessage) -> Result<(), String> {
        match self.senders.get(&target) {
            Some(tx) => tx
                .send(msg)
                .await
                .map_err(|e| format!("send to {target}: {e}")),
            None => Err(format!("unknown target agent: {target}")),
        }
    }

    /// Broadcast a message to all agents.
    #[allow(dead_code)]
    pub async fn broadcast(&self, msg: AgentMessage) -> Result<(), String> {
        for (id, tx) in self.senders.iter() {
            if let Err(e) = tx.send(msg.clone()).await {
                tracing::warn!("broadcast to {id} failed: {e}");
            }
        }
        Ok(())
    }
}

/// Central communication hub managing channels between agents.
#[allow(dead_code)]
pub struct AgentCommHub {
    receivers: HashMap<Uuid, mpsc::Receiver<AgentMessage>>,
    sender: AgentCommSender,
    /// Shared context store accessible to all agents.
    pub shared_context: SharedContextStore,
}

impl AgentCommHub {
    /// Create a new hub with channels for the given task IDs.
    #[allow(dead_code)]
    pub fn new(task_ids: &[Uuid], capacity: usize) -> Self {
        let mut senders = HashMap::new();
        let mut receivers = HashMap::new();

        for &id in task_ids {
            let (tx, rx) = mpsc::channel(capacity);
            senders.insert(id, tx);
            receivers.insert(id, rx);
        }

        Self {
            receivers,
            sender: AgentCommSender {
                senders: Arc::new(senders),
            },
            shared_context: SharedContextStore::new(),
        }
    }

    /// Take the receiver for a specific agent (can only be taken once).
    #[allow(dead_code)]
    pub fn take_receiver(&mut self, id: &Uuid) -> Option<mpsc::Receiver<AgentMessage>> {
        self.receivers.remove(id)
    }

    /// Get a cloneable sender handle.
    #[allow(dead_code)]
    pub fn sender(&self) -> AgentCommSender {
        self.sender.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_context_set_get() {
        let store = SharedContextStore::new();
        store.set("key1".into(), serde_json::json!("value1")).await;
        let val = store.get("key1").await;
        assert_eq!(val, Some(serde_json::json!("value1")));
    }

    #[tokio::test]
    async fn shared_context_keys() {
        let store = SharedContextStore::new();
        store.set("a".into(), serde_json::json!(1)).await;
        store.set("b".into(), serde_json::json!(2)).await;
        let mut keys = store.keys().await;
        keys.sort();
        assert_eq!(keys, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn shared_context_snapshot() {
        let store = SharedContextStore::new();
        store.set("x".into(), serde_json::json!(42)).await;
        let snap = store.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap["x"], serde_json::json!(42));
    }

    #[tokio::test]
    async fn shared_context_concurrent_reads() {
        let store = SharedContextStore::new();
        store.set("data".into(), serde_json::json!("shared")).await;

        let s1 = store.clone();
        let s2 = store.clone();
        let (r1, r2) = tokio::join!(s1.get("data"), s2.get("data"));
        assert_eq!(r1, r2);
    }

    #[test]
    fn agent_comm_hub_creation() {
        let ids = vec![Uuid::new_v4(), Uuid::new_v4()];
        let hub = AgentCommHub::new(&ids, 16);
        assert_eq!(hub.receivers.len(), 2);
    }

    #[test]
    fn agent_comm_hub_take_receiver() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let mut hub = AgentCommHub::new(&[id1, id2], 16);
        assert!(hub.take_receiver(&id1).is_some());
        assert!(hub.take_receiver(&id1).is_none()); // Already taken.
    }

    #[tokio::test]
    async fn agent_comm_sender_send_to() {
        let id1 = Uuid::new_v4();
        let mut hub = AgentCommHub::new(&[id1], 16);
        let sender = hub.sender();
        let mut rx = hub.take_receiver(&id1).unwrap();

        let msg = AgentMessage {
            from: Uuid::new_v4(),
            to: Some(id1),
            kind: AgentMessageKind::ContextShare,
            data: serde_json::json!({"info": "hello"}),
            timestamp: Utc::now(),
        };
        sender.send_to(id1, msg.clone()).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.kind, AgentMessageKind::ContextShare);
    }

    #[tokio::test]
    async fn agent_comm_sender_broadcast() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let mut hub = AgentCommHub::new(&[id1, id2], 16);
        let sender = hub.sender();
        let mut rx1 = hub.take_receiver(&id1).unwrap();
        let mut rx2 = hub.take_receiver(&id2).unwrap();

        let msg = AgentMessage {
            from: Uuid::new_v4(),
            to: None,
            kind: AgentMessageKind::CompletionSignal,
            data: serde_json::json!(null),
            timestamp: Utc::now(),
        };
        sender.broadcast(msg).await.unwrap();

        assert!(rx1.recv().await.is_some());
        assert!(rx2.recv().await.is_some());
    }

    #[tokio::test]
    async fn agent_comm_sender_unknown_target() {
        let hub = AgentCommHub::new(&[], 16);
        let sender = hub.sender();
        let msg = AgentMessage {
            from: Uuid::new_v4(),
            to: Some(Uuid::new_v4()),
            kind: AgentMessageKind::ErrorSignal,
            data: serde_json::json!(null),
            timestamp: Utc::now(),
        };
        let result = sender.send_to(Uuid::new_v4(), msg).await;
        assert!(result.is_err());
    }

    #[test]
    fn agent_message_serde_roundtrip() {
        let msg = AgentMessage {
            from: Uuid::new_v4(),
            to: Some(Uuid::new_v4()),
            kind: AgentMessageKind::DelegateRequest,
            data: serde_json::json!({"task": "do_thing"}),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.kind, msg.kind);
        assert_eq!(parsed.from, msg.from);
    }

    #[test]
    fn agent_message_kind_serde() {
        let json = serde_json::to_string(&AgentMessageKind::DelegateResponse).unwrap();
        assert_eq!(json, r#""delegate_response""#);
        let parsed: AgentMessageKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, AgentMessageKind::DelegateResponse);
    }

    #[test]
    fn shared_context_default() {
        let store = SharedContextStore::default();
        // Just verifying Default works.
        let _clone = store.clone();
    }

    #[test]
    fn agent_context_comm_optional() {
        // Verify that the comm system can be constructed but is optional.
        let hub = AgentCommHub::new(&[], 1);
        let _sender = hub.sender();
        let _store = hub.shared_context.clone();
    }
}
