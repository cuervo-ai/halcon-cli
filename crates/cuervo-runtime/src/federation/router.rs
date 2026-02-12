//! Message routing for federation protocol.

use std::collections::HashMap;

use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use uuid::Uuid;

use super::FederationMessage;
use crate::error::{Result, RuntimeError};

/// Mailbox for a single agent — sender + receiver pair.
pub struct AgentMailbox {
    pub tx: mpsc::Sender<FederationMessage>,
    rx: Mutex<mpsc::Receiver<FederationMessage>>,
}

impl AgentMailbox {
    /// Receive the next message for this agent.
    pub async fn receive(&self) -> Option<FederationMessage> {
        let mut rx = self.rx.lock().await;
        rx.recv().await
    }
}

/// Routes federation messages between agents (unicast and broadcast).
pub struct MessageRouter {
    mailboxes: RwLock<HashMap<Uuid, mpsc::Sender<FederationMessage>>>,
    broadcast_tx: broadcast::Sender<FederationMessage>,
}

impl MessageRouter {
    pub fn new(broadcast_capacity: usize) -> Self {
        let (broadcast_tx, _) = broadcast::channel(broadcast_capacity);
        Self {
            mailboxes: RwLock::new(HashMap::new()),
            broadcast_tx,
        }
    }

    /// Create a mailbox for an agent. Returns the mailbox for receiving messages.
    pub async fn create_mailbox(&self, agent_id: Uuid) -> AgentMailbox {
        let (tx, rx) = mpsc::channel(64);
        {
            let mut mailboxes = self.mailboxes.write().await;
            mailboxes.insert(agent_id, tx.clone());
        }
        AgentMailbox {
            tx,
            rx: Mutex::new(rx),
        }
    }

    /// Remove a mailbox for an agent.
    pub async fn remove_mailbox(&self, agent_id: &Uuid) {
        let mut mailboxes = self.mailboxes.write().await;
        mailboxes.remove(agent_id);
    }

    /// Send a message. If `to` is Some, unicast; if None, broadcast.
    pub async fn send(&self, msg: FederationMessage) -> Result<()> {
        if let Some(to) = &msg.to {
            // Unicast
            let mailboxes = self.mailboxes.read().await;
            let tx = mailboxes.get(to).ok_or_else(|| {
                RuntimeError::Federation(format!("no mailbox for agent {to}"))
            })?;
            tx.send(msg)
                .await
                .map_err(|_| RuntimeError::Federation("mailbox send failed".to_string()))
        } else {
            // Broadcast
            let _ = self.broadcast_tx.send(msg);
            Ok(())
        }
    }

    /// Subscribe to broadcast messages.
    pub fn subscribe_broadcast(&self) -> broadcast::Receiver<FederationMessage> {
        self.broadcast_tx.subscribe()
    }

    /// Number of active mailboxes.
    pub async fn mailbox_count(&self) -> usize {
        let mailboxes = self.mailboxes.read().await;
        mailboxes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation::FederationMessageKind;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[tokio::test]
    async fn unicast_send_receive() {
        let router = MessageRouter::new(16);
        let agent_id = id(1);
        let mailbox = router.create_mailbox(agent_id).await;

        let msg = FederationMessage::new(id(2), Some(agent_id), FederationMessageKind::Ping);
        router.send(msg).await.unwrap();

        let received = mailbox.receive().await.unwrap();
        assert!(matches!(received.kind, FederationMessageKind::Ping));
        assert_eq!(received.from, id(2));
    }

    #[tokio::test]
    async fn broadcast_reaches_subscribers() {
        let router = MessageRouter::new(16);
        let mut rx = router.subscribe_broadcast();

        let msg = FederationMessage::new(id(1), None, FederationMessageKind::Shutdown);
        router.send(msg).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert!(matches!(received.kind, FederationMessageKind::Shutdown));
    }

    #[tokio::test]
    async fn unicast_to_nonexistent_fails() {
        let router = MessageRouter::new(16);
        let msg = FederationMessage::new(id(1), Some(id(999)), FederationMessageKind::Ping);
        let result = router.send(msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_and_remove_mailbox() {
        let router = MessageRouter::new(16);
        let agent_id = id(1);
        router.create_mailbox(agent_id).await;
        assert_eq!(router.mailbox_count().await, 1);

        router.remove_mailbox(&agent_id).await;
        assert_eq!(router.mailbox_count().await, 0);
    }

    #[tokio::test]
    async fn multiple_unicast_messages() {
        let router = MessageRouter::new(16);
        let agent_id = id(1);
        let mailbox = router.create_mailbox(agent_id).await;

        for i in 0..5u128 {
            let msg = FederationMessage::new(
                id(100 + i),
                Some(agent_id),
                FederationMessageKind::Ping,
            );
            router.send(msg).await.unwrap();
        }

        for i in 0..5u128 {
            let received = mailbox.receive().await.unwrap();
            assert_eq!(received.from, id(100 + i));
        }
    }

    #[tokio::test]
    async fn delegate_task_roundtrip() {
        let router = MessageRouter::new(16);
        let worker = id(2);
        let mailbox = router.create_mailbox(worker).await;

        let msg = FederationMessage::new(
            id(1),
            Some(worker),
            FederationMessageKind::DelegateTask {
                instruction: "run tests".to_string(),
                budget: None,
                context: std::collections::HashMap::new(),
            },
        );
        router.send(msg).await.unwrap();

        let received = mailbox.receive().await.unwrap();
        if let FederationMessageKind::DelegateTask { instruction, .. } = &received.kind {
            assert_eq!(instruction, "run tests");
        } else {
            panic!("expected DelegateTask");
        }
    }

    #[tokio::test]
    async fn context_update_broadcast() {
        let router = MessageRouter::new(16);
        let mut rx1 = router.subscribe_broadcast();
        let mut rx2 = router.subscribe_broadcast();

        let msg = FederationMessage::new(
            id(1),
            None,
            FederationMessageKind::ContextUpdate {
                key: "status".to_string(),
                value: serde_json::json!("done"),
            },
        );
        router.send(msg).await.unwrap();

        let r1 = rx1.recv().await.unwrap();
        let r2 = rx2.recv().await.unwrap();
        assert!(matches!(r1.kind, FederationMessageKind::ContextUpdate { .. }));
        assert!(matches!(r2.kind, FederationMessageKind::ContextUpdate { .. }));
    }

    #[tokio::test]
    async fn ping_pong_via_router() {
        let router = MessageRouter::new(16);
        let a = id(1);
        let b = id(2);
        let mb_a = router.create_mailbox(a).await;
        let mb_b = router.create_mailbox(b).await;

        // a sends ping to b
        let ping = FederationMessage::new(a, Some(b), FederationMessageKind::Ping);
        router.send(ping).await.unwrap();
        let received = mb_b.receive().await.unwrap();
        assert!(matches!(received.kind, FederationMessageKind::Ping));

        // b sends pong to a
        let pong = FederationMessage::new(
            b,
            Some(a),
            FederationMessageKind::Pong {
                health: crate::agent::AgentHealth::Healthy,
            },
        );
        router.send(pong).await.unwrap();
        let received = mb_a.receive().await.unwrap();
        assert!(matches!(received.kind, FederationMessageKind::Pong { .. }));
    }

    #[tokio::test]
    async fn concurrent_sends() {
        let router = std::sync::Arc::new(MessageRouter::new(64));
        let agent_id = id(1);
        let mailbox = router.create_mailbox(agent_id).await;

        let mut handles = vec![];
        for i in 0..10u128 {
            let r = router.clone();
            handles.push(tokio::spawn(async move {
                let msg = FederationMessage::new(
                    id(100 + i),
                    Some(agent_id),
                    FederationMessageKind::Ping,
                );
                r.send(msg).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        for _ in 0..10 {
            let _ = mailbox.receive().await.unwrap();
        }
    }
}
