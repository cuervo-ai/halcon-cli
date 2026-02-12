//! In-process channel transport using `tokio::mpsc`.
//!
//! Zero-copy message passing for local agents (LlmAgent, LocalToolAgent).

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};

use super::{AgentTransport, TransportMessage};
use crate::error::{Result, RuntimeError};

/// In-process transport using tokio mpsc channels.
pub struct ChannelTransport {
    tx: mpsc::Sender<TransportMessage>,
    rx: Mutex<mpsc::Receiver<TransportMessage>>,
    connected: AtomicBool,
}

impl ChannelTransport {
    /// Create a paired channel transport (returns both ends).
    pub fn pair(buffer: usize) -> (Self, Self) {
        let (tx_a, rx_a) = mpsc::channel(buffer);
        let (tx_b, rx_b) = mpsc::channel(buffer);

        let a = Self {
            tx: tx_b,
            rx: Mutex::new(rx_a),
            connected: AtomicBool::new(true),
        };
        let b = Self {
            tx: tx_a,
            rx: Mutex::new(rx_b),
            connected: AtomicBool::new(true),
        };
        (a, b)
    }
}

#[async_trait]
impl AgentTransport for ChannelTransport {
    async fn send(&self, message: TransportMessage) -> Result<()> {
        if !self.is_connected() {
            return Err(RuntimeError::Transport("channel closed".to_string()));
        }
        self.tx
            .send(message)
            .await
            .map_err(|_| RuntimeError::Transport("channel send failed".to_string()))
    }

    async fn receive(&self) -> Result<TransportMessage> {
        let mut rx = self.rx.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| RuntimeError::Transport("channel closed".to_string()))
    }

    async fn close(&self) -> Result<()> {
        self.connected.store(false, Ordering::Release);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportMessageKind;

    #[tokio::test]
    async fn channel_send_receive() {
        let (a, b) = ChannelTransport::pair(16);
        let msg = TransportMessage::new(TransportMessageKind::Request, serde_json::json!("hello"));
        let msg_id = msg.id;
        a.send(msg).await.unwrap();
        let received = b.receive().await.unwrap();
        assert_eq!(received.id, msg_id);
        assert_eq!(received.payload, "hello");
    }

    #[tokio::test]
    async fn channel_bidirectional() {
        let (a, b) = ChannelTransport::pair(16);

        let msg1 = TransportMessage::new(TransportMessageKind::Request, serde_json::json!("ping"));
        a.send(msg1).await.unwrap();

        let received = b.receive().await.unwrap();
        assert_eq!(received.payload, "ping");

        let msg2 = TransportMessage::new(TransportMessageKind::Response, serde_json::json!("pong"));
        b.send(msg2).await.unwrap();

        let received = a.receive().await.unwrap();
        assert_eq!(received.payload, "pong");
    }

    #[tokio::test]
    async fn channel_close() {
        let (a, _b) = ChannelTransport::pair(16);
        assert!(a.is_connected());
        a.close().await.unwrap();
        assert!(!a.is_connected());
    }

    #[tokio::test]
    async fn channel_send_after_close() {
        let (a, _b) = ChannelTransport::pair(16);
        a.close().await.unwrap();
        let msg = TransportMessage::new(TransportMessageKind::Request, serde_json::json!("x"));
        let result = a.send(msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn channel_multiple_messages() {
        let (a, b) = ChannelTransport::pair(16);
        for i in 0..5 {
            let msg = TransportMessage::new(
                TransportMessageKind::Request,
                serde_json::json!(i),
            );
            a.send(msg).await.unwrap();
        }
        for i in 0..5 {
            let received = b.receive().await.unwrap();
            assert_eq!(received.payload, serde_json::json!(i));
        }
    }

    #[test]
    fn channel_is_connected_initial() {
        let (a, b) = ChannelTransport::pair(16);
        assert!(a.is_connected());
        assert!(b.is_connected());
    }

    #[tokio::test]
    async fn channel_receive_after_sender_drop() {
        let (a, b) = ChannelTransport::pair(16);
        let msg = TransportMessage::new(TransportMessageKind::Request, serde_json::json!("last"));
        a.send(msg).await.unwrap();
        drop(a);
        // Can still receive the buffered message
        let received = b.receive().await.unwrap();
        assert_eq!(received.payload, "last");
        // Next receive should fail
        let result = b.receive().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn channel_transport_trait_object() {
        let (a, b) = ChannelTransport::pair(16);
        let transport_a: Box<dyn AgentTransport> = Box::new(a);
        let transport_b: Box<dyn AgentTransport> = Box::new(b);

        let msg = TransportMessage::new(TransportMessageKind::Heartbeat, serde_json::json!(null));
        transport_a.send(msg).await.unwrap();
        let received = transport_b.receive().await.unwrap();
        assert_eq!(received.kind, TransportMessageKind::Heartbeat);
    }
}
