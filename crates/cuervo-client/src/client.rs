use crate::config::ClientConfig;
use crate::error::ClientError;
use crate::stream::EventStream;
use cuervo_api::error::ApiError;
use cuervo_api::types::agent::*;
use cuervo_api::types::config::{RuntimeConfigResponse, UpdateConfigRequest};
use cuervo_api::types::observability::MetricsSnapshot;
use cuervo_api::types::system::*;
use cuervo_api::types::task::*;
use cuervo_api::types::tool::*;
use reqwest::Client as HttpClient;
use uuid::Uuid;

/// Typed async client for the Cuervo control plane API.
pub struct CuervoClient {
    http: HttpClient,
    config: ClientConfig,
}

impl CuervoClient {
    /// Create a new client with the given configuration.
    pub fn new(config: ClientConfig) -> Result<Self, ClientError> {
        let http = HttpClient::builder()
            .timeout(config.timeout)
            .build()?;
        Ok(Self { http, config })
    }

    /// Get the client configuration.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    // ── Agents ──────────────────────────────────────────

    /// List all registered agents.
    pub async fn list_agents(&self) -> Result<Vec<AgentInfo>, ClientError> {
        self.get("agents").await
    }

    /// Get a specific agent by ID.
    pub async fn get_agent(&self, id: Uuid) -> Result<AgentInfo, ClientError> {
        self.get(&format!("agents/{id}")).await
    }

    /// Stop (deregister) an agent.
    pub async fn stop_agent(&self, id: Uuid) -> Result<serde_json::Value, ClientError> {
        self.delete(&format!("agents/{id}")).await
    }

    /// Invoke an agent with the given request.
    pub async fn invoke_agent(
        &self,
        id: Uuid,
        request: InvokeAgentRequest,
    ) -> Result<InvokeAgentResponse, ClientError> {
        self.post(&format!("agents/{id}/invoke"), &request).await
    }

    /// Get agent health details.
    pub async fn agent_health(&self, id: Uuid) -> Result<AgentHealthDetail, ClientError> {
        self.get(&format!("agents/{id}/health")).await
    }

    // ── Tasks ───────────────────────────────────────────

    /// List all task executions.
    pub async fn list_tasks(&self) -> Result<Vec<TaskExecution>, ClientError> {
        self.get("tasks").await
    }

    /// Submit a task DAG for execution.
    pub async fn submit_task(
        &self,
        request: SubmitTaskRequest,
    ) -> Result<SubmitTaskResponse, ClientError> {
        self.post("tasks", &request).await
    }

    /// Get a task execution by ID.
    pub async fn get_task(&self, id: Uuid) -> Result<TaskExecution, ClientError> {
        self.get(&format!("tasks/{id}")).await
    }

    /// Cancel a running task.
    pub async fn cancel_task(&self, id: Uuid) -> Result<serde_json::Value, ClientError> {
        self.delete(&format!("tasks/{id}")).await
    }

    // ── Tools ───────────────────────────────────────────

    /// List all registered tools.
    pub async fn list_tools(&self) -> Result<Vec<ToolInfo>, ClientError> {
        self.get("tools").await
    }

    /// Toggle a tool's enabled state.
    pub async fn toggle_tool(
        &self,
        name: &str,
        enabled: bool,
    ) -> Result<serde_json::Value, ClientError> {
        self.post(
            &format!("tools/{name}/toggle"),
            &ToggleToolRequest { enabled },
        )
        .await
    }

    /// Get a tool's execution history.
    pub async fn tool_history(
        &self,
        name: &str,
    ) -> Result<Vec<ToolExecutionRecord>, ClientError> {
        self.get(&format!("tools/{name}/history")).await
    }

    // ── Observability ───────────────────────────────────

    /// Get current metrics snapshot.
    pub async fn metrics(&self) -> Result<MetricsSnapshot, ClientError> {
        self.get("metrics").await
    }

    // ── System ──────────────────────────────────────────

    /// Get system status.
    pub async fn system_status(&self) -> Result<SystemStatus, ClientError> {
        self.get("system/status").await
    }

    /// Request system shutdown.
    pub async fn shutdown(
        &self,
        graceful: bool,
        reason: Option<String>,
    ) -> Result<ShutdownResponse, ClientError> {
        self.post(
            "system/shutdown",
            &ShutdownRequest { graceful, reason },
        )
        .await
    }

    // ── Config ───────────────────────────────────────────

    /// Get the full runtime configuration.
    pub async fn get_config(&self) -> Result<RuntimeConfigResponse, ClientError> {
        self.get("system/config").await
    }

    /// Apply a partial configuration update. Returns the updated full config.
    pub async fn update_config(
        &self,
        update: UpdateConfigRequest,
    ) -> Result<RuntimeConfigResponse, ClientError> {
        self.put("system/config", &update).await
    }

    /// Check if the server is reachable.
    pub async fn health_check(&self) -> Result<bool, ClientError> {
        let url = format!(
            "{}/health",
            self.config.base_url.trim_end_matches('/')
        );
        let resp = self.http.get(&url).send().await?;
        Ok(resp.status().is_success())
    }

    // ── Streaming ───────────────────────────────────────

    /// Open a WebSocket event stream.
    pub async fn event_stream(&self) -> Result<EventStream, ClientError> {
        EventStream::connect(&self.config).await
    }

    // ── HTTP helpers ────────────────────────────────────

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let url = self.config.api_url(path);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.config.auth_token)
            .send()
            .await?;
        self.handle_response(resp).await
    }

    async fn post<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        let url = self.config.api_url(path);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.config.auth_token)
            .json(body)
            .send()
            .await?;
        self.handle_response(resp).await
    }

    async fn put<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        let url = self.config.api_url(path);
        let resp = self
            .http
            .put(&url)
            .bearer_auth(&self.config.auth_token)
            .json(body)
            .send()
            .await?;
        self.handle_response(resp).await
    }

    async fn delete<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, ClientError> {
        let url = self.config.api_url(path);
        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&self.config.auth_token)
            .send()
            .await?;
        self.handle_response(resp).await
    }

    async fn handle_response<T: serde::de::DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T, ClientError> {
        if resp.status().is_success() {
            let body = resp.text().await?;
            Ok(serde_json::from_str(&body)?)
        } else {
            let body = resp.text().await?;
            match serde_json::from_str::<ApiError>(&body) {
                Ok(api_err) => Err(ClientError::Api(api_err)),
                Err(_) => Err(ClientError::ConnectionFailed(body)),
            }
        }
    }
}
