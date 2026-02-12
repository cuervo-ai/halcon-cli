//! HTTP transport for remote agent endpoints.
//!
//! POST-based request/response communication with configurable
//! authentication, timeout, and retry.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use super::{AgentTransport, TransportMessage, TransportMessageKind};
use crate::error::{Result, RuntimeError};

/// HTTP transport that POSTs messages to a remote endpoint.
pub struct HttpTransport {
    endpoint: String,
    auth_header: Option<(String, String)>,
    client: reqwest::Client,
    timeout: Duration,
    connected: AtomicBool,
}

impl HttpTransport {
    /// Create a new HTTP transport targeting the given endpoint.
    pub fn new(
        endpoint: impl Into<String>,
        auth_header: Option<(String, String)>,
        timeout: Duration,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_default();

        Self {
            endpoint: endpoint.into(),
            auth_header,
            client,
            timeout,
            connected: AtomicBool::new(true),
        }
    }

    /// The configured endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The configured timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

#[async_trait]
impl AgentTransport for HttpTransport {
    async fn send(&self, message: TransportMessage) -> Result<()> {
        if !self.is_connected() {
            return Err(RuntimeError::Transport("HTTP transport closed".to_string()));
        }

        let mut req = self.client.post(&self.endpoint).json(&message);
        if let Some((name, value)) = &self.auth_header {
            req = req.header(name, value);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| RuntimeError::Transport(format!("HTTP send error: {e}")))?;

        if !resp.status().is_success() {
            return Err(RuntimeError::Transport(format!(
                "HTTP error: status {}",
                resp.status()
            )));
        }
        Ok(())
    }

    async fn receive(&self) -> Result<TransportMessage> {
        // HTTP transport is request/response — receive is not independently callable.
        // The response is expected to come back from the send() call.
        // For a true async receive, you'd need SSE or WebSocket.
        // For now, return a synthetic response indicating the limitation.
        Err(RuntimeError::Transport(
            "HTTP transport does not support independent receive; use send() which returns response"
                .to_string(),
        ))
    }

    async fn close(&self) -> Result<()> {
        self.connected.store(false, Ordering::Release);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}

/// Send a request and receive the response in a single roundtrip.
pub async fn http_roundtrip(
    transport: &HttpTransport,
    message: TransportMessage,
) -> Result<TransportMessage> {
    if !transport.is_connected() {
        return Err(RuntimeError::Transport("HTTP transport closed".to_string()));
    }

    let mut req = transport.client.post(&transport.endpoint).json(&message);
    if let Some((name, value)) = &transport.auth_header {
        req = req.header(name, value);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| RuntimeError::Transport(format!("HTTP roundtrip error: {e}")))?;

    if !resp.status().is_success() {
        return Err(RuntimeError::Transport(format!(
            "HTTP error: status {}",
            resp.status()
        )));
    }

    let response_msg: TransportMessage = resp
        .json()
        .await
        .map_err(|e| RuntimeError::Transport(format!("HTTP response parse error: {e}")))?;

    Ok(response_msg)
}

/// Create a simple response message for an incoming request.
pub fn make_response(request_id: uuid::Uuid, payload: serde_json::Value) -> TransportMessage {
    TransportMessage {
        id: request_id,
        kind: TransportMessageKind::Response,
        payload,
        timestamp: chrono::Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_transport_construction() {
        let t = HttpTransport::new(
            "https://example.com/agent",
            Some(("Authorization".to_string(), "Bearer xyz".to_string())),
            Duration::from_secs(30),
        );
        assert_eq!(t.endpoint(), "https://example.com/agent");
        assert_eq!(t.timeout(), Duration::from_secs(30));
        assert!(t.is_connected());
    }

    #[test]
    fn http_transport_no_auth() {
        let t = HttpTransport::new("http://localhost:8080", None, Duration::from_secs(10));
        assert_eq!(t.endpoint(), "http://localhost:8080");
        assert!(t.is_connected());
    }

    #[tokio::test]
    async fn http_transport_close() {
        let t = HttpTransport::new("http://localhost:9999", None, Duration::from_secs(5));
        assert!(t.is_connected());
        t.close().await.unwrap();
        assert!(!t.is_connected());
    }

    #[tokio::test]
    async fn http_send_after_close() {
        let t = HttpTransport::new("http://localhost:9999", None, Duration::from_secs(5));
        t.close().await.unwrap();
        let msg = TransportMessage::new(TransportMessageKind::Request, serde_json::json!("x"));
        let result = t.send(msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn http_receive_not_supported() {
        let t = HttpTransport::new("http://localhost:9999", None, Duration::from_secs(5));
        let result = t.receive().await;
        assert!(result.is_err());
    }

    #[test]
    fn make_response_helper() {
        let req_id = uuid::Uuid::new_v4();
        let resp = make_response(req_id, serde_json::json!({"status": "ok"}));
        assert_eq!(resp.id, req_id);
        assert_eq!(resp.kind, TransportMessageKind::Response);
    }
}
