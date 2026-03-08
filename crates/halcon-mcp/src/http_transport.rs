//! HTTP transport for MCP servers using Streamable-HTTP / SSE.
//!
//! Implements the MCP HTTP transport spec (2025-03-26):
//! - POST to `<base_url>` for JSON-RPC requests
//! - SSE `GET <base_url>` for server-sent event streams (notifications)
//! - `Authorization: Bearer <token>` header for OAuth-protected endpoints
//!
//! The transport is stateless from the caller's perspective: each
//! `send_request()` creates a new POST, and the SSE listener is a
//! long-lived background task. Both share a `reqwest::Client` for connection
//! pooling.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::error::{McpError, McpResult};
use crate::types::{JsonRpcNotification, JsonRpcResponse};

/// Timeout for individual JSON-RPC POST requests.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Timeout for the initial SSE connection.
const SSE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// An HTTP MCP transport that supports OAuth bearer tokens.
///
/// # Usage
///
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// use halcon_mcp::http_transport::HttpTransport;
/// let t = HttpTransport::new("https://api.example.com/mcp/", Some("bearer_token".into()));
/// let result = t.send_request(1, "initialize", serde_json::json!({})).await?;
/// # Ok(())
/// # }
/// ```
pub struct HttpTransport {
    base_url: String,
    bearer_token: Option<String>,
    client: Arc<Client>,
}

impl HttpTransport {
    /// Create a new HTTP transport.
    ///
    /// `bearer_token` is the OAuth access token (if any).  Pass `None` for
    /// public MCP servers that do not require authentication.
    pub fn new(base_url: impl Into<String>, bearer_token: Option<String>) -> Self {
        let client = Arc::new(
            Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("reqwest client build"),
        );
        Self {
            base_url: base_url.into(),
            bearer_token,
            client,
        }
    }

    /// Update the bearer token (called after a token refresh).
    pub fn set_bearer_token(&mut self, token: String) {
        self.bearer_token = Some(token);
    }

    /// Send a JSON-RPC request and return the parsed response.
    pub async fn send_request(
        &self,
        id: i64,
        method: &str,
        params: Value,
    ) -> McpResult<JsonRpcResponse> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let mut req = self.client.post(&self.base_url).json(&body);
        if let Some(ref token) = self.bearer_token {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await.map_err(|e| McpError::Transport(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(McpError::Transport(
                "HTTP 401 Unauthorized — run `halcon mcp auth <name>` to refresh OAuth token".into(),
            ));
        }

        if !resp.status().is_success() {
            return Err(McpError::Transport(format!(
                "HTTP {} from MCP server",
                resp.status()
            )));
        }

        let json: Value = resp.json().await.map_err(|e| McpError::Transport(e.to_string()))?;
        let parsed: JsonRpcResponse =
            serde_json::from_value(json).map_err(McpError::Json)?;
        Ok(parsed)
    }

    /// Send a JSON-RPC notification (fire-and-forget, no response expected).
    pub async fn send_notification(&self, method: &str, params: Value) -> McpResult<()> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let mut req = self.client.post(&self.base_url).json(&body);
        if let Some(ref token) = self.bearer_token {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await.map_err(|e| McpError::Transport(e.to_string()))?;
        if !resp.status().is_success() && resp.status() != reqwest::StatusCode::NO_CONTENT {
            tracing::warn!(
                method,
                status = %resp.status(),
                "MCP notification returned non-success"
            );
        }
        Ok(())
    }

    /// Start an SSE listener for server-sent notifications.
    ///
    /// Returns a channel receiver and a task handle.  The task runs until the
    /// SSE connection closes or the handle is dropped (via `abort()`).
    ///
    /// Notifications are parsed as `JsonRpcNotification` objects.
    pub async fn start_sse_listener(
        &self,
        buffer: usize,
    ) -> McpResult<(mpsc::Receiver<JsonRpcNotification>, JoinHandle<()>)> {
        let url = format!("{}/sse", self.base_url.trim_end_matches('/'));
        let bearer = self.bearer_token.clone();
        let client = self.client.clone();
        let (tx, rx) = mpsc::channel(buffer);

        let handle = tokio::spawn(async move {
            let mut req = client.get(&url)
                .header("Accept", "text/event-stream")
                .header("Cache-Control", "no-cache")
                .timeout(SSE_CONNECT_TIMEOUT);
            if let Some(ref token) = bearer {
                req = req.bearer_auth(token);
            }

            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("SSE connect failed: {e}");
                    return;
                }
            };

            if !resp.status().is_success() {
                tracing::warn!(status = %resp.status(), "SSE endpoint returned error");
                return;
            }

            let mut stream = resp.bytes_stream();
            let mut buf = String::new();

            use futures::StreamExt;
            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::debug!("SSE stream error: {e}");
                        break;
                    }
                };

                if let Ok(text) = std::str::from_utf8(&chunk) {
                    buf.push_str(text);
                }

                // Parse SSE events: "data: <json>\n\n"
                while let Some(pos) = buf.find("\n\n") {
                    let event = buf[..pos].to_string();
                    buf = buf[pos + 2..].to_string();

                    for line in event.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Ok(notif) = serde_json::from_str::<JsonRpcNotification>(data) {
                                if tx.send(notif).await.is_err() {
                                    return; // Receiver dropped — stop listening.
                                }
                            }
                        }
                    }
                }
            }
            tracing::debug!("SSE stream closed");
        });

        Ok((rx, handle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_construction() {
        let t = HttpTransport::new("https://example.com/mcp/", Some("token".into()));
        assert_eq!(t.base_url, "https://example.com/mcp/");
        assert_eq!(t.bearer_token.as_deref(), Some("token"));
    }

    #[test]
    fn transport_no_auth() {
        let t = HttpTransport::new("https://example.com/mcp/", None);
        assert!(t.bearer_token.is_none());
    }

    #[test]
    fn set_bearer_token_updates() {
        let mut t = HttpTransport::new("https://example.com/mcp/", None);
        t.set_bearer_token("new_token".into());
        assert_eq!(t.bearer_token.as_deref(), Some("new_token"));
    }
}
