//! HTTP + WebSocket client for the remote-control system.
//!
//! Connects to a running `halcon serve` instance. All operations are idempotent.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use super::protocol::{RemoteControlEvent, RemoteSessionInfo, ReplanPayload};

/// Client for the Halcon remote-control API.
pub struct RemoteControlClient {
    base_url: String,
    ws_url: String,
    token: String,
    http: reqwest::Client,
}

impl RemoteControlClient {
    /// Create a new client targeting the given server.
    pub fn new(server_url: &str, token: &str) -> Result<Self> {
        let base_url = server_url.trim_end_matches('/').to_string();

        // Derive WebSocket URL from HTTP URL.
        let ws_url = if base_url.starts_with("https://") {
            base_url.replace("https://", "wss://")
        } else {
            base_url.replace("http://", "ws://")
        };

        Ok(Self {
            base_url,
            ws_url,
            token: token.to_string(),
            http: reqwest::Client::new(),
        })
    }

    // ── REST API Methods ────────────────────────────────────────────────────

    /// Create a new chat session.
    pub async fn create_session(&self, model: &str, provider: &str) -> Result<RemoteSessionInfo> {
        let url = format!("{}/api/v1/chat/sessions", self.base_url);
        let body = serde_json::json!({
            "model": model,
            "provider": provider,
            "title": format!("remote-control-{}", chrono::Utc::now().format("%H%M%S")),
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .context("Failed to connect to halcon serve")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Create session failed ({status}): {text}");
        }

        let data: serde_json::Value = resp.json().await?;
        let session = &data["session"];

        Ok(RemoteSessionInfo {
            id: session["id"].as_str().unwrap_or_default().to_string(),
            title: session["title"].as_str().map(|s| s.to_string()),
            model: session["model"].as_str().unwrap_or_default().to_string(),
            provider: session["provider"].as_str().unwrap_or_default().to_string(),
            status: session["status"].as_str().unwrap_or("idle").to_string(),
            message_count: session["message_count"].as_u64().unwrap_or(0) as usize,
        })
    }

    /// List all active sessions.
    pub async fn list_sessions(&self) -> Result<Vec<RemoteSessionInfo>> {
        let url = format!("{}/api/v1/chat/sessions", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("Failed to connect to halcon serve")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("List sessions failed ({status}): {text}");
        }

        let data: serde_json::Value = resp.json().await?;
        let sessions = data["sessions"].as_array().cloned().unwrap_or_default();

        Ok(sessions
            .iter()
            .map(|s| RemoteSessionInfo {
                id: s["id"].as_str().unwrap_or_default().to_string(),
                title: s["title"].as_str().map(|t| t.to_string()),
                model: s["model"].as_str().unwrap_or_default().to_string(),
                provider: s["provider"].as_str().unwrap_or_default().to_string(),
                status: s["status"].as_str().unwrap_or("idle").to_string(),
                message_count: s["message_count"].as_u64().unwrap_or(0) as usize,
            })
            .collect())
    }

    /// Submit a user message to a session and start execution.
    pub async fn submit_message(
        &self,
        session_id: &str,
        content: &str,
        orchestrate: bool,
    ) -> Result<()> {
        let url = format!(
            "{}/api/v1/chat/sessions/{}/messages",
            self.base_url, session_id
        );
        let body = serde_json::json!({
            "content": content,
            "orchestrate": orchestrate,
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .context("Failed to submit message")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Submit message failed ({status}): {text}");
        }

        Ok(())
    }

    /// Resolve a permission request (approve or deny).
    pub async fn resolve_permission(
        &self,
        session_id: &str,
        request_id: uuid::Uuid,
        approve: bool,
    ) -> Result<PermissionResponse> {
        let url = format!(
            "{}/api/v1/chat/sessions/{}/permissions/{}",
            self.base_url, session_id, request_id
        );
        let decision = if approve { "approve" } else { "deny" };
        let body = serde_json::json!({ "decision": decision });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .context("Failed to resolve permission")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Resolve permission failed ({status}): {text}");
        }

        let data: serde_json::Value = resp.json().await?;
        Ok(PermissionResponse {
            request_id,
            decision: decision.to_string(),
            tool_executed: data["tool_executed"].as_bool().unwrap_or(false),
        })
    }

    /// Submit a replan payload.
    pub async fn submit_replan(&self, session_id: &str, payload: &ReplanPayload) -> Result<()> {
        // Replan is submitted as a special user message with metadata.
        // The backend interprets messages starting with `@replan` as plan replacements.
        let url = format!(
            "{}/api/v1/chat/sessions/{}/messages",
            self.base_url, session_id
        );
        let content = format!(
            "@replan {}\n\n{}",
            payload.description,
            serde_json::to_string_pretty(payload)?
        );
        let body = serde_json::json!({
            "content": content,
            "orchestrate": true,
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .context("Failed to submit replan")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Replan failed ({status}): {text}");
        }

        Ok(())
    }

    /// Cancel an active session.
    pub async fn cancel_session(&self, session_id: &str) -> Result<()> {
        let url = format!(
            "{}/api/v1/chat/sessions/{}/active",
            self.base_url, session_id
        );

        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("Failed to cancel session")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Cancel failed ({status}): {text}");
        }

        Ok(())
    }

    // ── WebSocket Methods ───────────────────────────────────────────────────

    /// Connect to the WebSocket event stream and return a receiver for events.
    ///
    /// The connection subscribes to all remote-control-relevant channels.
    pub async fn connect_ws(
        &self,
    ) -> Result<(
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
        futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    )> {
        let ws_url = format!("{}/ws/events", self.ws_url);

        // Build request with auth header.
        let request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(&ws_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .header("Sec-WebSocket-Version", "13")
            .header("Host", extract_host(&ws_url))
            .body(())
            .context("Failed to build WebSocket request")?;

        let (ws_stream, _response) = tokio_tungstenite::connect_async(request)
            .await
            .context("Failed to connect WebSocket to halcon serve")?;

        let (mut sink, stream) = ws_stream.split();

        // Subscribe to relevant channels.
        let sub_msg = super::protocol::subscribe_message();
        sink.send(Message::Text(sub_msg))
            .await
            .context("Failed to subscribe to channels")?;

        Ok((sink, stream))
    }

    /// Parse a WebSocket text message into a RemoteControlEvent.
    pub fn parse_event(text: &str) -> Option<RemoteControlEvent> {
        serde_json::from_str(text).ok()
    }

    /// Server base URL (for display).
    pub fn server_url(&self) -> &str {
        &self.base_url
    }

    /// Auth token (for display/debug).
    pub fn token(&self) -> &str {
        &self.token
    }
}

/// Response from resolving a permission.
#[derive(Debug)]
pub struct PermissionResponse {
    pub request_id: uuid::Uuid,
    pub decision: String,
    pub tool_executed: bool,
}

/// Extract host:port from a URL.
fn extract_host(url: &str) -> String {
    url.split("//")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .unwrap_or("localhost")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_host() {
        assert_eq!(
            extract_host("ws://127.0.0.1:9849/ws/events"),
            "127.0.0.1:9849"
        );
        assert_eq!(extract_host("wss://example.com/ws"), "example.com");
    }

    #[test]
    fn test_parse_permission_required() {
        let json = r#"{
            "type": "permission_required",
            "request_id": "00000000-0000-0000-0000-000000000001",
            "session_id": "00000000-0000-0000-0000-000000000002",
            "tool_name": "bash",
            "risk_level": "Destructive",
            "args_preview": {"command": "rm -rf /tmp/test"},
            "description": "Execute bash command",
            "deadline_secs": 60
        }"#;
        let event = RemoteControlClient::parse_event(json);
        assert!(matches!(
            event,
            Some(RemoteControlEvent::PermissionRequired { .. })
        ));
    }

    #[test]
    fn test_parse_unknown_event_graceful() {
        let json = r#"{"type": "some_future_event", "data": 42}"#;
        let event = RemoteControlClient::parse_event(json);
        assert!(matches!(event, Some(RemoteControlEvent::Unknown)));
    }

    #[test]
    fn test_client_construction() {
        let client = RemoteControlClient::new("http://127.0.0.1:9849", "test-token").unwrap();
        assert_eq!(client.server_url(), "http://127.0.0.1:9849");
        assert_eq!(client.token(), "test-token");
    }
}
