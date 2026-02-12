//! HTTP remote agent bridge: wraps an HTTP API endpoint as a RuntimeAgent.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;

use crate::agent::{
    AgentCapability, AgentDescriptor, AgentHealth, AgentKind, AgentRequest, AgentResponse,
    ProtocolSupport, RuntimeAgent,
};
use crate::error::{Result, RuntimeError};

/// Wraps an HTTP API endpoint as a RuntimeAgent.
///
/// POSTs AgentRequest as JSON, expects AgentResponse as JSON.
pub struct HttpRemoteAgent {
    descriptor: AgentDescriptor,
    endpoint: String,
    auth_header: Option<(String, String)>,
    client: reqwest::Client,
}

impl HttpRemoteAgent {
    pub fn new(
        name: &str,
        endpoint: &str,
        auth_header: Option<(String, String)>,
        capabilities: Vec<AgentCapability>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_default();

        Self {
            descriptor: AgentDescriptor {
                id: Uuid::new_v4(),
                name: name.to_string(),
                agent_kind: AgentKind::HttpEndpoint,
                capabilities,
                protocols: vec![ProtocolSupport::Rest],
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("endpoint".to_string(), serde_json::json!(endpoint));
                    m
                },
                max_concurrency: 10,
            },
            endpoint: endpoint.to_string(),
            auth_header,
            client,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

#[async_trait]
impl RuntimeAgent for HttpRemoteAgent {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, request: AgentRequest) -> Result<AgentResponse> {
        let start = std::time::Instant::now();

        let mut req = self.client.post(&self.endpoint).json(&request);
        if let Some((name, value)) = &self.auth_header {
            req = req.header(name, value);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| RuntimeError::Execution(format!("HTTP request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(RuntimeError::Execution(format!(
                "HTTP error {status}: {body}"
            )));
        }

        let mut response: AgentResponse = resp
            .json()
            .await
            .map_err(|e| RuntimeError::Execution(format!("response parse error: {e}")))?;

        response.usage.latency_ms = start.elapsed().as_millis() as u64;

        Ok(response)
    }

    async fn health(&self) -> AgentHealth {
        // Simple HTTP connectivity check
        match self.client.get(&self.endpoint).send().await {
            Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 405 => {
                AgentHealth::Healthy
            }
            Ok(resp) => AgentHealth::Degraded {
                reason: format!("HTTP status {}", resp.status()),
            },
            Err(e) => AgentHealth::Unavailable {
                reason: format!("connection failed: {e}"),
            },
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor() {
        let agent = HttpRemoteAgent::new(
            "api",
            "https://api.example.com/agent",
            Some(("Authorization".to_string(), "Bearer xyz".to_string())),
            vec![AgentCapability::CodeGeneration],
        );
        assert_eq!(agent.descriptor().name, "api");
        assert_eq!(agent.descriptor().agent_kind, AgentKind::HttpEndpoint);
        assert_eq!(agent.endpoint(), "https://api.example.com/agent");
    }

    #[test]
    fn descriptor_no_auth() {
        let agent = HttpRemoteAgent::new("api", "http://localhost:8080", None, vec![]);
        assert_eq!(agent.descriptor().protocols, vec![ProtocolSupport::Rest]);
    }

    #[test]
    fn descriptor_max_concurrency() {
        let agent = HttpRemoteAgent::new("api", "http://localhost:8080", None, vec![]);
        assert_eq!(agent.descriptor().max_concurrency, 10);
    }

    #[tokio::test]
    async fn invoke_connection_refused() {
        let agent =
            HttpRemoteAgent::new("api", "http://127.0.0.1:19999/agent", None, vec![]);
        let req = AgentRequest::new("hello");
        let result = agent.invoke(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn health_connection_refused() {
        let agent =
            HttpRemoteAgent::new("api", "http://127.0.0.1:19999/health", None, vec![]);
        let health = agent.health().await;
        assert!(matches!(health, AgentHealth::Unavailable { .. }));
    }

    #[tokio::test]
    async fn shutdown() {
        let agent = HttpRemoteAgent::new("api", "http://localhost:8080", None, vec![]);
        assert!(agent.shutdown().await.is_ok());
    }
}
