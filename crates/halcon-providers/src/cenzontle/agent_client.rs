//! HTTP client for Cenzontle's agent orchestration, MCP, and RAG APIs.
//!
//! Complements `CenzontleProvider` (which handles LLM chat via `/v1/llm/chat`)
//! by providing access to Cenzontle's higher-level capabilities:
//!
//! - Agent session management and task execution (with SSE streaming)
//! - MCP tool discovery and invocation
//! - RAG knowledge search
//!
//! # Construction
//!
//! ```ignore
//! // Share auth with existing CenzontleProvider:
//! let client = CenzontleAgentClient::new(access_token, base_url);
//!
//! // Or extract from provider:
//! let client = CenzontleAgentClient::from_provider(&provider);
//! ```

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::BoxStream;
use futures::StreamExt;
use tracing::{debug, info, warn};
use uuid::Uuid;

use halcon_core::error::{HalconError, Result};

use super::agent_types::*;
use super::DEFAULT_BASE_URL;
use crate::http::{backoff_delay_with_jitter, is_retryable_status, parse_retry_after};

const CLIENT_NAME: &str = "halcon-cli";

/// Circuit breaker for agent API calls (separate from LLM circuit breaker).
#[derive(Debug, Default)]
struct AgentCircuitBreaker {
    consecutive_failures: AtomicU32,
    open_until_unix_ms: AtomicU64,
}

const CB_THRESHOLD: u32 = 5;
const CB_OPEN_MS: u64 = 60_000;

impl AgentCircuitBreaker {
    fn is_open(&self) -> bool {
        let until = self.open_until_unix_ms.load(Ordering::Relaxed);
        if until == 0 {
            return false;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now_ms < until
    }

    fn record_failure(&self) {
        let n = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if n >= CB_THRESHOLD {
            let until = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
                + CB_OPEN_MS;
            self.open_until_unix_ms.store(until, Ordering::Relaxed);
            warn!(
                failures = n,
                "Cenzontle agent API: circuit breaker opened for 60s"
            );
        }
    }

    fn record_success(&self) {
        let prev = self.consecutive_failures.swap(0, Ordering::Relaxed);
        if prev > 0 {
            self.open_until_unix_ms.store(0, Ordering::Relaxed);
            info!("Cenzontle agent API: circuit breaker reset");
        }
    }
}

/// Client for Cenzontle's agent orchestration, MCP, and RAG APIs.
pub struct CenzontleAgentClient {
    client: reqwest::Client,
    access_token: String,
    base_url: String,
    session_id: String,
    circuit_breaker: Arc<AgentCircuitBreaker>,
}

impl std::fmt::Debug for CenzontleAgentClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CenzontleAgentClient")
            .field("base_url", &self.base_url)
            .field("access_token", &"[REDACTED]")
            .finish()
    }
}

impl CenzontleAgentClient {
    /// Create a new agent client with explicit token and base URL.
    pub fn new(access_token: String, base_url: Option<String>) -> Self {
        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        // Force HTTP/1.1 for SSE streaming (same reason as CenzontleProvider).
        let client = reqwest::Client::builder()
            .http1_only()
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(4)
            .user_agent(format!("halcon-cli/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("failed to build HTTP client for Cenzontle agent API");

        Self {
            client,
            access_token,
            base_url,
            session_id: Uuid::new_v4().to_string(),
            circuit_breaker: Arc::new(AgentCircuitBreaker::default()),
        }
    }

    /// Create a client that shares auth credentials with a `CenzontleProvider`.
    pub fn from_provider(provider: &super::CenzontleProvider) -> Self {
        Self::new(
            provider.access_token().to_string(),
            Some(provider.base_url().to_string()),
        )
    }

    /// Base URL of the Cenzontle instance.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    // ── Agent Sessions ──────────────────────────────────────────────────────

    /// Create a new agent session.
    pub async fn create_session(&self, req: &CreateSessionRequest) -> Result<AgentSession> {
        let url = format!("{}/v1/agents/sessions", self.base_url);
        self.post_json(&url, req).await
    }

    /// Get the current state of an agent session.
    pub async fn get_session(&self, session_id: &str) -> Result<AgentSession> {
        let url = format!("{}/v1/agents/sessions/{}", self.base_url, session_id);
        self.get_json(&url).await
    }

    // ── Task Execution ──────────────────────────────────────────────────────

    /// Submit a task and stream execution events via SSE.
    ///
    /// Returns a stream of `TaskEvent` items. The stream ends when the task
    /// completes or errors.
    pub async fn submit_task(
        &self,
        session_id: &str,
        req: &SubmitTaskRequest,
    ) -> Result<BoxStream<'static, Result<TaskEvent>>> {
        self.check_circuit_breaker()?;

        let url = format!(
            "{}/v1/agents/sessions/{}/tasks",
            self.base_url, session_id
        );

        let halcon_ctx = serde_json::json!({
            "client": CLIENT_NAME,
            "session_id": self.session_id,
        })
        .to_string();

        let request_id = Uuid::new_v4().to_string();

        debug!(
            url = %url,
            request_id = %request_id,
            "Cenzontle: submitting agent task (SSE streaming)"
        );

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.access_token)
            .header("x-halcon-context", &halcon_ctx)
            .header("x-request-id", &request_id)
            .header("accept", "text/event-stream")
            .json(req)
            .send()
            .await
            .map_err(|e| HalconError::ConnectionError {
                provider: "cenzontle-agent".to_string(),
                message: format!("Cannot reach Cenzontle agent API: {e}"),
            })?;

        let status = response.status();
        if !status.is_success() {
            self.circuit_breaker.record_failure();
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(HalconError::ApiError {
                message: format!("Cenzontle agent task HTTP {code}: {body}"),
                status: Some(code),
            });
        }

        self.circuit_breaker.record_success();

        // Detect response type: JSON (synchronous result) vs SSE (streaming).
        // The Cenzontle task endpoint returns JSON when the task completes
        // synchronously (e.g., no agents available, instant result) and SSE
        // when agents execute asynchronously with streaming output.
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if content_type.contains("application/json") {
            // Synchronous JSON response — convert to synthetic TaskEvent stream.
            let body: serde_json::Value = response.json().await.map_err(|e| {
                HalconError::ApiError {
                    message: format!("Failed to parse task JSON response: {e}"),
                    status: None,
                }
            })?;

            let mut events = Vec::new();

            // Extract combinedOutput or error from the JSON response.
            let all_succeeded = body.get("allSucceeded").and_then(|v| v.as_bool()).unwrap_or(false);
            let combined_output = body
                .get("combinedOutput")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if all_succeeded {
                events.push(Ok(TaskEvent::Completed {
                    output: combined_output,
                    tokens_used: None,
                }));
            } else {
                // Check for agent-level errors.
                let error_msg = body
                    .get("agentResults")
                    .and_then(|a| a.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|r| r.get("error"))
                    .and_then(|e| e.as_str())
                    .unwrap_or("unknown");

                if !combined_output.is_empty() {
                    events.push(Ok(TaskEvent::Error {
                        message: combined_output,
                        code: Some(error_msg.to_string()),
                    }));
                } else {
                    events.push(Ok(TaskEvent::Error {
                        message: format!("Task failed: {}", error_msg),
                        code: Some(error_msg.to_string()),
                    }));
                }
            }

            return Ok(Box::pin(futures::stream::iter(events)));
        }

        // SSE streaming response — parse events incrementally.
        let cb = Arc::clone(&self.circuit_breaker);

        struct SseState {
            byte_stream: futures::stream::BoxStream<'static, std::result::Result<bytes::Bytes, reqwest::Error>>,
            buffer: String,
            pending_events: std::collections::VecDeque<Result<TaskEvent>>,
            done: bool,
            cb: Arc<AgentCircuitBreaker>,
        }

        /// Max SSE buffer size (4 MB) to prevent OOM on malformed streams.
        const MAX_SSE_BUFFER: usize = 4 * 1024 * 1024;

        let byte_stream: futures::stream::BoxStream<'static, _> =
            Box::pin(response.bytes_stream());
        let initial = SseState {
            byte_stream,
            buffer: String::new(),
            pending_events: std::collections::VecDeque::new(),
            done: false,
            cb,
        };

        let stream = futures::stream::unfold(initial, |mut state| async move {
            // Drain pending parsed events first (FIFO order).
            if let Some(event) = state.pending_events.pop_front() {
                return Some((event, state));
            }
            if state.done {
                return None;
            }

            loop {
                match state.byte_stream.next().await {
                    None => return None,
                    Some(Err(e)) => {
                        state.cb.record_failure();
                        state.done = true;
                        return Some((
                            Err(HalconError::ApiError {
                                message: format!("SSE stream error: {e}"),
                                status: None,
                            }),
                            state,
                        ));
                    }
                    Some(Ok(chunk)) => {
                        // Normalize \r\n to \n for cross-platform SSE parsing.
                        let text = String::from_utf8_lossy(&chunk).replace("\r\n", "\n");
                        state.buffer.push_str(&text);

                        // Guard against unbounded buffer growth.
                        if state.buffer.len() > MAX_SSE_BUFFER {
                            state.done = true;
                            return Some((
                                Err(HalconError::ApiError {
                                    message: "SSE buffer exceeded 4 MB — aborting stream".to_string(),
                                    status: None,
                                }),
                                state,
                            ));
                        }

                        // Process complete SSE events (double-newline delimited).
                        while let Some(pos) = state.buffer.find("\n\n") {
                            let event_text = state.buffer[..pos].to_string();
                            state.buffer = state.buffer[pos + 2..].to_string();

                            for line in event_text.lines() {
                                let line = line.trim();
                                if let Some(data) = line.strip_prefix("data: ") {
                                    if data == "[DONE]" {
                                        state.done = true;
                                        if let Some(ev) = state.pending_events.pop_front() {
                                            return Some((ev, state));
                                        }
                                        return None;
                                    }
                                    match serde_json::from_str::<TaskEvent>(data) {
                                        Ok(event) => {
                                            state.pending_events.push_back(Ok(event));
                                        }
                                        Err(_e) => {
                                            // Skip unparseable SSE events (forward compatibility).
                                        }
                                    }
                                }
                            }
                        }

                        // Return first pending event if any (FIFO).
                        if let Some(event) = state.pending_events.pop_front() {
                            return Some((event, state));
                        }
                    }
                }
            }
        });

        Ok(Box::pin(stream))
    }

    /// Submit a task and collect all events into a `TaskResult`.
    ///
    /// Convenience method that consumes the SSE stream and accumulates results.
    pub async fn submit_task_blocking(
        &self,
        session_id: &str,
        req: &SubmitTaskRequest,
    ) -> Result<TaskResult> {
        let mut stream = self.submit_task(session_id, req).await?;
        let mut result = TaskResult::default();

        while let Some(event) = stream.next().await {
            match event? {
                TaskEvent::Content { content } => result.output.push_str(&content),
                TaskEvent::Thinking { content } => result.thinking.push_str(&content),
                TaskEvent::ToolCall { name, .. } => result.tool_calls.push(name),
                TaskEvent::Completed {
                    output,
                    tokens_used,
                } => {
                    if result.output.is_empty() {
                        result.output = output;
                    }
                    result.tokens_used = tokens_used.unwrap_or(0);
                    result.success = true;
                }
                TaskEvent::Error { message, .. } => {
                    result.error = Some(message);
                    result.success = false;
                }
                _ => {}
            }
        }

        Ok(result)
    }

    // ── Agent Listing ───────────────────────────────────────────────────────

    /// List all registered agents.
    ///
    /// The Cenzontle `/v1/agents` endpoint returns a bare JSON array `[...]`,
    /// not a wrapper object.
    pub async fn list_agents(&self) -> Result<Vec<AgentInfo>> {
        let url = format!("{}/v1/agents", self.base_url);
        self.get_json(&url).await
    }

    // ── MCP Tools ───────────────────────────────────────────────────────────

    /// List available MCP tools.
    pub async fn list_mcp_tools(&self) -> Result<Vec<McpToolDef>> {
        let url = format!("{}/v1/mcp/tools", self.base_url);
        let resp: McpToolListResponse = self.get_json(&url).await?;
        Ok(resp.tools)
    }

    /// Call an MCP tool.
    pub async fn call_mcp_tool(&self, req: &McpToolCallRequest) -> Result<McpToolCallResponse> {
        let url = format!("{}/v1/mcp/tools/call", self.base_url);
        self.post_json(&url, req).await
    }

    // ── Knowledge Search (RAG) ──────────────────────────────────────────────

    /// Search the knowledge base via RAG.
    ///
    /// Routes through the MCP `knowledge_search` tool since Cenzontle's RAG
    /// is exposed via the MCP tool layer, not a dedicated REST endpoint.
    pub async fn knowledge_search(
        &self,
        req: &KnowledgeSearchRequest,
    ) -> Result<KnowledgeSearchResponse> {
        let mcp_args = serde_json::json!({
            "query": req.query,
            "botId": req.bot_id,
            "topK": req.top_k.unwrap_or(5),
        });

        let mcp_req = McpToolCallRequest {
            name: "knowledge_search".to_string(),
            arguments: mcp_args,
        };

        let resp = self.call_mcp_tool(&mcp_req).await?;

        if resp.is_error {
            return Err(HalconError::ApiError {
                message: format!("Knowledge search failed: {}", resp.text()),
                status: None,
            });
        }

        // Parse the MCP tool response content as KnowledgeSearchResponse.
        // If it doesn't parse as structured data, wrap as a single chunk.
        let text = resp.text();
        match serde_json::from_str::<KnowledgeSearchResponse>(&text) {
            Ok(parsed) => Ok(parsed),
            Err(_) => {
                // MCP tool returned plain text — wrap as a single result chunk.
                Ok(KnowledgeSearchResponse {
                    chunks: vec![KnowledgeChunk {
                        content: text,
                        score: 1.0,
                        source: None,
                        metadata: serde_json::Value::Null,
                    }],
                })
            }
        }
    }

    // ── Internal Helpers ────────────────────────────────────────────────────

    fn check_circuit_breaker(&self) -> Result<()> {
        if self.circuit_breaker.is_open() {
            return Err(HalconError::ApiError {
                message: "Cenzontle agent API: circuit breaker open — backend is degraded"
                    .to_string(),
                status: None,
            });
        }
        Ok(())
    }

    /// GET request with JSON response, retry, and circuit breaker.
    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        self.check_circuit_breaker()?;

        let max_retries = 2u32;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay = backoff_delay_with_jitter(1000, attempt);
                tokio::time::sleep(delay).await;
            }

            let result = self
                .client
                .get(url)
                .bearer_auth(&self.access_token)
                .header("x-halcon-context", &self.halcon_context())
                .timeout(Duration::from_secs(15))
                .send()
                .await;

            let response = match result {
                Ok(r) => r,
                Err(e) if e.is_connect() && attempt < max_retries => {
                    self.circuit_breaker.record_failure();
                    warn!(attempt = attempt + 1, error = %e, "Cenzontle agent API: retry");
                    continue;
                }
                Err(e) => {
                    self.circuit_breaker.record_failure();
                    return Err(HalconError::ConnectionError {
                        provider: "cenzontle-agent".to_string(),
                        message: format!("Cannot reach Cenzontle: {e}"),
                    });
                }
            };

            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(HalconError::ApiError {
                    message: "Cenzontle: token expired. Run `halcon login cenzontle`.".to_string(),
                    status: Some(401),
                });
            }

            if is_retryable_status(status.as_u16()) && attempt < max_retries {
                self.circuit_breaker.record_failure();
                let delay = parse_retry_after(response.headers())
                    .map(Duration::from_secs)
                    .unwrap_or_else(|| backoff_delay_with_jitter(1000, attempt));
                tokio::time::sleep(delay).await;
                continue;
            }

            if !status.is_success() {
                let code = status.as_u16();
                let body = response.text().await.unwrap_or_default();
                return Err(HalconError::ApiError {
                    message: format!("Cenzontle agent API HTTP {code}: {body}"),
                    status: Some(code),
                });
            }

            self.circuit_breaker.record_success();
            let body: T = response.json().await.map_err(|e| HalconError::ApiError {
                message: format!("Failed to parse Cenzontle response: {e}"),
                status: None,
            })?;
            return Ok(body);
        }

        Err(HalconError::ApiError {
            message: "Cenzontle agent API: all retries exhausted".to_string(),
            status: None,
        })
    }

    /// POST request with JSON body/response, retry, and circuit breaker.
    async fn post_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T> {
        self.check_circuit_breaker()?;

        let max_retries = 2u32;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay = backoff_delay_with_jitter(1000, attempt);
                tokio::time::sleep(delay).await;
            }

            let result = self
                .client
                .post(url)
                .bearer_auth(&self.access_token)
                .header("x-halcon-context", &self.halcon_context())
                .json(body)
                .timeout(Duration::from_secs(30))
                .send()
                .await;

            let response = match result {
                Ok(r) => r,
                Err(e) if e.is_connect() && attempt < max_retries => {
                    self.circuit_breaker.record_failure();
                    warn!(attempt = attempt + 1, error = %e, "Cenzontle agent API: retry");
                    continue;
                }
                Err(e) => {
                    self.circuit_breaker.record_failure();
                    return Err(HalconError::ConnectionError {
                        provider: "cenzontle-agent".to_string(),
                        message: format!("Cannot reach Cenzontle: {e}"),
                    });
                }
            };

            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(HalconError::ApiError {
                    message: "Cenzontle: token expired. Run `halcon login cenzontle`.".to_string(),
                    status: Some(401),
                });
            }

            if is_retryable_status(status.as_u16()) && attempt < max_retries {
                self.circuit_breaker.record_failure();
                let delay = parse_retry_after(response.headers())
                    .map(Duration::from_secs)
                    .unwrap_or_else(|| backoff_delay_with_jitter(1000, attempt));
                tokio::time::sleep(delay).await;
                continue;
            }

            if !status.is_success() {
                let code = status.as_u16();
                let body_text = response.text().await.unwrap_or_default();
                return Err(HalconError::ApiError {
                    message: format!("Cenzontle agent API HTTP {code}: {body_text}"),
                    status: Some(code),
                });
            }

            self.circuit_breaker.record_success();
            let body: T = response.json().await.map_err(|e| HalconError::ApiError {
                message: format!("Failed to parse Cenzontle response: {e}"),
                status: None,
            })?;
            return Ok(body);
        }

        Err(HalconError::ApiError {
            message: "Cenzontle agent API: all retries exhausted".to_string(),
            status: None,
        })
    }

    fn halcon_context(&self) -> String {
        serde_json::json!({
            "client": CLIENT_NAME,
            "session_id": self.session_id,
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_construction() {
        let client = CenzontleAgentClient::new("test-token".into(), None);
        assert_eq!(client.base_url(), DEFAULT_BASE_URL);
    }

    #[test]
    fn client_custom_base_url() {
        let client =
            CenzontleAgentClient::new("tok".into(), Some("http://localhost:3001".into()));
        assert_eq!(client.base_url(), "http://localhost:3001");
    }

    #[test]
    fn client_debug_redacts_token() {
        let client = CenzontleAgentClient::new("secret-token".into(), None);
        let debug = format!("{:?}", client);
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("secret-token"));
    }

    #[test]
    fn circuit_breaker_initially_closed() {
        let cb = AgentCircuitBreaker::default();
        assert!(!cb.is_open());
    }

    #[test]
    fn circuit_breaker_opens_after_threshold() {
        let cb = AgentCircuitBreaker::default();
        for _ in 0..CB_THRESHOLD {
            cb.record_failure();
        }
        assert!(cb.is_open());
    }

    #[test]
    fn circuit_breaker_resets_on_success() {
        let cb = AgentCircuitBreaker::default();
        for _ in 0..3 {
            cb.record_failure();
        }
        cb.record_success();
        assert!(!cb.is_open());
        assert_eq!(cb.consecutive_failures.load(Ordering::Relaxed), 0);
    }
}
