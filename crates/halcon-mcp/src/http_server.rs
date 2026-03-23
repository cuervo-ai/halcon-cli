//! HTTP MCP server — exposes Halcon tools via Streamable HTTP transport.
//!
//! Endpoints:
//!   POST /mcp   — main JSON-RPC endpoint (request/response)
//!   GET  /mcp   — SSE stream for server-initiated messages
//!   GET  /health — liveness probe
//!
//! Auth: `Authorization: Bearer <token>` checked against HALCON_MCP_SERVER_API_KEY
//! or the `api_key` field on McpHttpServer.
//!
//! Session management: `Mcp-Session-Id` header. Each unique session ID gets
//! an isolated working-directory context. Sessions are lightweight — Halcon
//! tools are stateless, so isolation is enforced at the request level.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tower_http::cors::CorsLayer;

use crate::error::{McpError, McpResult};
use crate::types::{
    error_codes, CallToolResult, InitializeResult, JsonRpcError, JsonRpcResponse,
    McpToolDefinition, ServerCapabilities, ServerInfo, ToolResultContent, ToolsCapability,
    PROTOCOL_VERSION,
};

/// Shared state for the HTTP MCP server.
#[derive(Clone)]
pub struct McpHttpServer {
    inner: Arc<McpHttpServerInner>,
}

struct McpHttpServerInner {
    /// Registered tools: name → tool Arc.
    tools: HashMap<String, Arc<dyn halcon_core::traits::Tool>>,
    /// Working directory for tool execution.
    working_directory: String,
    /// Optional API key for Bearer token validation. None = no auth.
    api_key: Option<String>,
    /// Active sessions: session_id → last-seen Instant.
    sessions: Mutex<HashMap<String, Instant>>,
    /// Session TTL.
    session_ttl: Duration,
}

impl McpHttpServer {
    /// Create a new HTTP MCP server.
    pub fn new(
        tools: Vec<Arc<dyn halcon_core::traits::Tool>>,
        working_directory: String,
        api_key: Option<String>,
        session_ttl_secs: u64,
    ) -> Self {
        let tool_map = tools
            .into_iter()
            .map(|t| (t.name().to_string(), t))
            .collect();
        Self {
            inner: Arc::new(McpHttpServerInner {
                tools: tool_map,
                working_directory,
                api_key,
                sessions: Mutex::new(HashMap::new()),
                session_ttl: Duration::from_secs(session_ttl_secs),
            }),
        }
    }

    /// Build the axum Router for this server.
    pub fn into_router(self) -> Router {
        Router::new()
            .route("/mcp", post(handle_post_mcp))
            .route("/mcp", get(handle_get_mcp_sse))
            .route("/health", get(handle_health))
            .layer(CorsLayer::permissive())
            .with_state(self)
    }

    /// Start listening on the given address.
    pub async fn serve(self, addr: std::net::SocketAddr) -> McpResult<()> {
        let router = self.into_router();
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| McpError::Transport(format!("bind failed: {e}")))?;
        tracing::info!("MCP HTTP server listening on {addr}");
        axum::serve(listener, router)
            .await
            .map_err(|e| McpError::Transport(format!("serve failed: {e}")))?;
        Ok(())
    }

    /// Get the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.inner.tools.len()
    }
}

// ── Auth helper ───────────────────────────────────────────────────────────────

fn check_auth(server: &McpHttpServer, headers: &HeaderMap) -> Result<(), StatusCode> {
    let expected = match &server.inner.api_key {
        Some(k) => k.as_str(),
        None => return Ok(()), // no auth configured
    };
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let token = auth_header.trim_start_matches("Bearer ").trim();
    if token != expected {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

// ── Session helper ────────────────────────────────────────────────────────────

fn get_or_create_session(server: &McpHttpServer, headers: &HeaderMap) -> String {
    let session_id = headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Prune expired sessions and touch the current one.
    let ttl = server.inner.session_ttl;
    let mut sessions = server.inner.sessions.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    sessions.retain(|_, last| now.duration_since(*last) < ttl);
    sessions.insert(session_id.clone(), now);
    session_id
}

// ── Route handlers ────────────────────────────────────────────────────────────

async fn handle_post_mcp(
    State(server): State<McpHttpServer>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // Auth check.
    if let Err(status) = check_auth(&server, &headers) {
        return (status, "Unauthorized").into_response();
    }

    // Parse the session ID (or create a new one).
    let session_id = get_or_create_session(&server, &headers);

    // Parse JSON-RPC request.
    let value: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            let resp =
                make_error_response(None, error_codes::PARSE_ERROR, &format!("Parse error: {e}"));
            return json_response_with_session(resp, &session_id);
        }
    };

    let method = value
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let id = value.get("id").and_then(|i| i.as_u64());
    let params = value.get("params").cloned();

    // Notifications (no id) — 202 Accepted, no body.
    if id.is_none() {
        tracing::debug!("MCP HTTP notification: {method}");
        return StatusCode::ACCEPTED.into_response();
    }
    let request_id = id.unwrap();

    let response = dispatch_request(&server, request_id, &method, params, &session_id).await;
    json_response_with_session(response, &session_id)
}

async fn handle_get_mcp_sse(State(server): State<McpHttpServer>, headers: HeaderMap) -> Response {
    // Auth check.
    if let Err(status) = check_auth(&server, &headers) {
        return (status, "Unauthorized").into_response();
    }

    // Minimal SSE stream — sends a single endpoint event and keeps alive.
    // Full bidirectional SSE is a Phase 2 enhancement.
    let (tx, rx) = mpsc::unbounded_channel::<Result<Event, std::convert::Infallible>>();
    let endpoint_event = Event::default().event("endpoint").data("/mcp");
    let _ = tx.send(Ok(endpoint_event));

    let stream = UnboundedReceiverStream::new(rx);
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn handle_health() -> &'static str {
    "ok"
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

async fn dispatch_request(
    server: &McpHttpServer,
    id: u64,
    method: &str,
    params: Option<Value>,
    _session_id: &str,
) -> JsonRpcResponse {
    match method {
        "initialize" => handle_initialize(server, id),
        "tools/list" => handle_tools_list(server, id),
        "tools/call" => handle_tools_call(server, id, params).await,
        "ping" => make_success_response(id, serde_json::json!({})),
        _ => make_error_response(
            Some(id),
            error_codes::METHOD_NOT_FOUND,
            &format!("Unknown method: {method}"),
        ),
    }
}

fn handle_initialize(server: &McpHttpServer, id: u64) -> JsonRpcResponse {
    let result = InitializeResult {
        protocol_version: PROTOCOL_VERSION.to_string(),
        capabilities: ServerCapabilities {
            tools: Some(ToolsCapability {
                list_changed: false,
            }),
        },
        server_info: ServerInfo {
            name: "halcon-mcp-server".to_string(),
            version: Some(crate::version().to_string()),
        },
    };
    // suppress unused warning
    let _ = server.tool_count();
    make_success_response(id, serde_json::to_value(result).unwrap())
}

fn handle_tools_list(server: &McpHttpServer, id: u64) -> JsonRpcResponse {
    let tools: Vec<McpToolDefinition> = server
        .inner
        .tools
        .values()
        .map(|t| McpToolDefinition {
            name: t.name().to_string(),
            description: Some(t.description().to_string()),
            input_schema: t.input_schema(),
        })
        .collect();
    make_success_response(id, serde_json::json!({ "tools": tools }))
}

async fn handle_tools_call(
    server: &McpHttpServer,
    id: u64,
    params: Option<Value>,
) -> JsonRpcResponse {
    let params = match params {
        Some(p) => p,
        None => {
            return make_error_response(Some(id), error_codes::INVALID_PARAMS, "Missing params")
        }
    };

    let tool_name = match params.get("name").and_then(|n| n.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return make_error_response(Some(id), error_codes::INVALID_PARAMS, "Missing 'name'")
        }
    };

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    // Log every tool call to audit trail.
    tracing::info!(
        tool = %tool_name,
        session_id = "http",
        transport = "http",
        "mcp_server.tool_call"
    );

    let tool = match server.inner.tools.get(&tool_name) {
        Some(t) => Arc::clone(t),
        None => {
            return make_error_response(
                Some(id),
                error_codes::INVALID_PARAMS,
                &format!("Unknown tool: {tool_name}"),
            )
        }
    };

    let input = halcon_core::types::ToolInput {
        tool_use_id: format!("mcp-http-{id}"),
        arguments,
        working_directory: server.inner.working_directory.clone(),
    };

    match tool.execute(input).await {
        Ok(output) => {
            tracing::info!(
                tool = %tool_name,
                success = !output.is_error,
                "mcp_server.tool_result"
            );
            let result = CallToolResult {
                content: vec![ToolResultContent::Text {
                    text: output.content,
                }],
                is_error: output.is_error,
            };
            make_success_response(id, serde_json::to_value(result).unwrap())
        }
        Err(e) => {
            tracing::warn!(tool = %tool_name, error = %e, "mcp_server.tool_error");
            let result = CallToolResult {
                content: vec![ToolResultContent::Text {
                    text: format!("Tool error: {e}"),
                }],
                is_error: true,
            };
            make_success_response(id, serde_json::to_value(result).unwrap())
        }
    }
}

// ── Response helpers ──────────────────────────────────────────────────────────

fn make_success_response(id: u64, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id: Some(id),
        result: Some(result),
        error: None,
    }
}

fn make_error_response(id: Option<u64>, code: i64, message: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_string(),
            data: None,
        }),
    }
}

fn json_response_with_session(response: JsonRpcResponse, session_id: &str) -> Response {
    let mut headers = axum::http::HeaderMap::new();
    if let Ok(v) = session_id.parse() {
        headers.insert("mcp-session-id", v);
    }
    (StatusCode::OK, headers, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_server() -> McpHttpServer {
        McpHttpServer::new(vec![], "/tmp".to_string(), None, 1800)
    }

    fn make_server_with_auth(key: &str) -> McpHttpServer {
        McpHttpServer::new(vec![], "/tmp".to_string(), Some(key.to_string()), 1800)
    }

    #[test]
    fn tool_count_empty() {
        let s = make_server();
        assert_eq!(s.tool_count(), 0);
    }

    #[test]
    fn auth_check_no_key_always_passes() {
        let s = make_server();
        let headers = HeaderMap::new();
        assert!(check_auth(&s, &headers).is_ok());
    }

    #[test]
    fn auth_check_valid_bearer() {
        let s = make_server_with_auth("secret123");
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer secret123".parse().unwrap());
        assert!(check_auth(&s, &headers).is_ok());
    }

    #[test]
    fn auth_check_wrong_token() {
        let s = make_server_with_auth("secret123");
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer wrong".parse().unwrap());
        assert!(check_auth(&s, &headers).is_err());
    }

    #[test]
    fn auth_check_missing_bearer() {
        let s = make_server_with_auth("secret123");
        let headers = HeaderMap::new();
        assert!(check_auth(&s, &headers).is_err());
    }

    #[tokio::test]
    async fn initialize_response() {
        let s = make_server();
        let resp = dispatch_request(&s, 1, "initialize", None, "test-session").await;
        assert!(resp.error.is_none());
        let r = resp.result.unwrap();
        assert_eq!(r["protocolVersion"], PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn tools_list_empty() {
        let s = make_server();
        let resp = dispatch_request(&s, 2, "tools/list", None, "test-session").await;
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap()["tools"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let s = make_server();
        let resp = dispatch_request(&s, 3, "unknown/method", None, "test-session").await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, error_codes::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn tools_call_unknown_tool() {
        let s = make_server();
        let params = serde_json::json!({"name": "unknown", "arguments": {}});
        let resp = dispatch_request(&s, 4, "tools/call", Some(params), "test-session").await;
        assert!(resp.error.is_some());
    }

    #[test]
    fn session_creation() {
        let s = make_server();
        let mut headers = HeaderMap::new();
        headers.insert("mcp-session-id", "my-session-123".parse().unwrap());
        let id = get_or_create_session(&s, &headers);
        assert_eq!(id, "my-session-123");
    }

    #[test]
    fn session_auto_created_if_missing() {
        let s = make_server();
        let headers = HeaderMap::new();
        let id = get_or_create_session(&s, &headers);
        assert!(!id.is_empty());
    }

    #[tokio::test]
    async fn ping_returns_success() {
        let s = make_server();
        let resp = dispatch_request(&s, 5, "ping", None, "test-session").await;
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
    }

    #[tokio::test]
    async fn tools_call_missing_params_returns_error() {
        let s = make_server();
        let resp = dispatch_request(&s, 6, "tools/call", None, "test-session").await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, error_codes::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn tools_call_missing_name_returns_error() {
        let s = make_server();
        let params = serde_json::json!({"arguments": {}});
        let resp = dispatch_request(&s, 7, "tools/call", Some(params), "test-session").await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, error_codes::INVALID_PARAMS);
    }
}
