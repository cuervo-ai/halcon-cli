use std::time::Duration;

/// Configuration for the Cuervo client.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Base URL of the control plane API (e.g., "http://127.0.0.1:9849").
    pub base_url: String,
    /// Authentication token.
    pub auth_token: String,
    /// Request timeout.
    pub timeout: Duration,
    /// Maximum retry attempts for transient failures.
    pub max_retries: u32,
    /// Interval between reconnection attempts for WebSocket.
    pub reconnect_interval: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            base_url: format!(
                "http://{}:{}",
                cuervo_api::DEFAULT_BIND,
                cuervo_api::DEFAULT_PORT
            ),
            auth_token: String::new(),
            timeout: Duration::from_secs(30),
            max_retries: 3,
            reconnect_interval: Duration::from_secs(5),
        }
    }
}

impl ClientConfig {
    pub fn new(base_url: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            auth_token: auth_token.into(),
            ..Default::default()
        }
    }

    /// WebSocket URL derived from the base HTTP URL.
    pub fn ws_url(&self) -> String {
        let base = self
            .base_url
            .replace("http://", "ws://")
            .replace("https://", "wss://");
        format!("{base}/ws/events?token={}", self.auth_token)
    }

    /// Full API URL for a given path.
    pub fn api_url(&self, path: &str) -> String {
        format!(
            "{}/api/{}/{}",
            self.base_url.trim_end_matches('/'),
            cuervo_api::API_VERSION,
            path.trim_start_matches('/')
        )
    }
}
