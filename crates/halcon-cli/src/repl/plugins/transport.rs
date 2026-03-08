//! Plugin Transport Runtime — executes plugin tool calls over Stdio, HTTP, or Local bridges.
//!
//! Each plugin has a declared transport in its manifest. The runtime keeps a
//! lightweight handle per transport type and dispatches invocations accordingly.
//! Stdio transports spawn a fresh subprocess per call (stateless); HTTP transports
//! reuse a shared `reqwest::Client`; Local transports return a synthetic success.
//!
//! This module is I/O-capable (async), but all paths are timeout-guarded so a
//! hanging plugin cannot stall the agent loop.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time::timeout;

// ─── Wire Protocol ────────────────────────────────────────────────────────────

/// JSON-RPC 2.0 request (plugin tool invocation).
#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: JsonRpcParams,
}

#[derive(Serialize)]
struct JsonRpcParams {
    tool: String,
    arguments: serde_json::Value,
}

/// JSON-RPC 2.0 response (plugin tool result).
#[derive(Deserialize)]
struct JsonRpcResponse {
    result: Option<JsonRpcResult>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
struct JsonRpcResult {
    content: Option<String>,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    tokens_used: u64,
    #[serde(default)]
    cost_usd: f64,
}

#[derive(Deserialize)]
struct JsonRpcError {
    message: String,
}

// ─── Transport Handle ─────────────────────────────────────────────────────────

/// Wire handle for communicating with a plugin.
#[derive(Clone, Debug)]
pub enum TransportHandle {
    /// Spawn a subprocess per call; communicate via stdio JSON-RPC.
    Stdio { command: String, args: Vec<String> },
    /// Send JSON-RPC POST requests to a remote HTTP service.
    Http { client: Arc<reqwest::Client>, base_url: String },
    /// In-process bridge — returns a synthetic success (test/demo).
    Local,
}

// ─── Invoke Result ────────────────────────────────────────────────────────────

/// Result of a single plugin tool invocation.
#[derive(Debug, Clone)]
pub struct PluginInvokeResult {
    pub content: String,
    pub is_error: bool,
    pub tokens_used: u64,
    pub cost_usd: f64,
    pub latency_ms: u64,
}

// ─── Runtime ─────────────────────────────────────────────────────────────────

/// Shared runtime that manages transport handles for all loaded plugins.
///
/// Created once per session, wrapped in `Arc<>`, shared across `PluginProxyTool`
/// instances. All invoke paths are async and timeout-guarded.
pub struct PluginTransportRuntime {
    handles: HashMap<String, TransportHandle>,
}

impl PluginTransportRuntime {
    /// Create an empty runtime (no plugins registered yet).
    pub fn new() -> Self {
        Self { handles: HashMap::new() }
    }

    /// Register a transport handle for a plugin.
    pub fn register(&mut self, plugin_id: String, handle: TransportHandle) {
        self.handles.insert(plugin_id, handle);
    }

    /// Invoke a plugin tool call.
    ///
    /// Returns `Err` only on infrastructure failure (timeout, transport error).
    /// Plugin-level errors (tool failure, permission denied) are returned as
    /// `Ok(result)` with `is_error: true`.
    pub async fn invoke(
        &self,
        plugin_id: &str,
        tool_name: &str,
        args: serde_json::Value,
        timeout_ms: u64,
    ) -> Result<PluginInvokeResult, String> {
        let handle = match self.handles.get(plugin_id) {
            Some(h) => h.clone(),
            None => {
                return Ok(PluginInvokeResult {
                    content: format!("Plugin '{plugin_id}' not registered in transport runtime"),
                    is_error: true,
                    tokens_used: 0,
                    cost_usd: 0.0,
                    latency_ms: 0,
                });
            }
        };

        let dur = Duration::from_millis(timeout_ms.max(1000));
        let start = std::time::Instant::now();

        let fut = Self::invoke_handle(handle, tool_name, args);
        match timeout(dur, fut).await {
            Ok(result) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                result.map(|mut r| {
                    r.latency_ms = latency_ms;
                    r
                })
            }
            Err(_) => Err(format!(
                "Plugin '{plugin_id}' tool '{tool_name}' timed out after {timeout_ms}ms"
            )),
        }
    }

    async fn invoke_handle(
        handle: TransportHandle,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<PluginInvokeResult, String> {
        match handle {
            TransportHandle::Local => Self::invoke_local(tool_name, args).await,
            TransportHandle::Stdio { command, args: cmd_args } => {
                Self::invoke_stdio(&command, &cmd_args, tool_name, args).await
            }
            TransportHandle::Http { client, base_url } => {
                Self::invoke_http(&client, &base_url, tool_name, args).await
            }
        }
    }

    /// Local/in-process bridge: echoes the tool name and args as a synthetic result.
    async fn invoke_local(
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<PluginInvokeResult, String> {
        let content = format!(
            "[local plugin] tool={tool_name} args={}",
            serde_json::to_string(&args).unwrap_or_default()
        );
        Ok(PluginInvokeResult {
            content,
            is_error: false,
            tokens_used: 0,
            cost_usd: 0.0,
            latency_ms: 0,
        })
    }

    /// Stdio transport: spawn subprocess, write JSON-RPC to stdin, read from stdout.
    async fn invoke_stdio(
        command: &str,
        cmd_args: &[String],
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<PluginInvokeResult, String> {
        use tokio::io::AsyncWriteExt;
        use tokio::io::AsyncBufReadExt;

        let rpc = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "tool/invoke",
            params: JsonRpcParams { tool: tool_name.to_string(), arguments: args },
        };
        let payload = serde_json::to_string(&rpc)
            .map_err(|e| format!("serialize RPC: {e}"))?;

        let mut child = tokio::process::Command::new(command)
            .args(cmd_args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true) // ensure process is killed when the future is dropped (e.g. on timeout)
            .spawn()
            .map_err(|e| format!("spawn plugin '{command}': {e}"))?;

        // Write request to stdin
        if let Some(stdin) = child.stdin.take() {
            let mut stdin = stdin;
            let line = format!("{payload}\n");
            stdin.write_all(line.as_bytes()).await
                .map_err(|e| format!("write to plugin stdin: {e}"))?;
            drop(stdin);
        }

        // Read one line from stdout
        let stdout = child.stdout.take()
            .ok_or_else(|| "no stdout from plugin process".to_string())?;
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).await
            .map_err(|e| format!("read plugin stdout: {e}"))?;

        // Wait for child to exit (non-blocking, best effort)
        let _ = child.wait().await;

        // Parse JSON-RPC response
        let resp: JsonRpcResponse = serde_json::from_str(line.trim())
            .map_err(|e| format!("parse plugin response: {e} (raw: {line:?})"))?;

        Self::extract_result(resp)
    }

    /// HTTP transport: POST JSON-RPC to `{base_url}/invoke`.
    async fn invoke_http(
        client: &reqwest::Client,
        base_url: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<PluginInvokeResult, String> {
        let url = format!("{base_url}/invoke");
        let rpc = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "tool/invoke",
            params: JsonRpcParams { tool: tool_name.to_string(), arguments: args },
        };

        let response = client
            .post(&url)
            .json(&rpc)
            .send()
            .await
            .map_err(|e| format!("HTTP plugin call to {url}: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("HTTP plugin returned status {}", response.status()));
        }

        let resp: JsonRpcResponse = response.json().await
            .map_err(|e| format!("parse HTTP plugin response: {e}"))?;

        Self::extract_result(resp)
    }

    fn extract_result(resp: JsonRpcResponse) -> Result<PluginInvokeResult, String> {
        if let Some(err) = resp.error {
            return Ok(PluginInvokeResult {
                content: format!("Plugin error: {}", err.message),
                is_error: true,
                tokens_used: 0,
                cost_usd: 0.0,
                latency_ms: 0,
            });
        }
        let result = resp.result.ok_or_else(|| "plugin returned neither result nor error".to_string())?;
        Ok(PluginInvokeResult {
            content: result.content.unwrap_or_default(),
            is_error: result.is_error,
            tokens_used: result.tokens_used,
            cost_usd: result.cost_usd,
            latency_ms: 0,
        })
    }
}

impl Default for PluginTransportRuntime {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_runtime_with_local(plugin_id: &str) -> PluginTransportRuntime {
        let mut rt = PluginTransportRuntime::new();
        rt.register(plugin_id.to_string(), TransportHandle::Local);
        rt
    }

    #[tokio::test]
    async fn local_transport_returns_ok() {
        let rt = make_runtime_with_local("my-plugin");
        let result = rt
            .invoke("my-plugin", "my_tool", serde_json::json!({"key": "value"}), 5000)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("my_tool"));
    }

    #[tokio::test]
    async fn local_transport_includes_args_in_content() {
        let rt = make_runtime_with_local("p1");
        let result = rt
            .invoke("p1", "echo", serde_json::json!({"msg": "hello"}), 5000)
            .await
            .unwrap();
        assert!(result.content.contains("echo"));
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn unregistered_plugin_returns_is_error_true() {
        let rt = PluginTransportRuntime::new();
        let result = rt
            .invoke("nonexistent", "tool", serde_json::json!({}), 5000)
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not registered"));
    }

    #[tokio::test]
    async fn timeout_on_hanging_process() {
        // Use `sleep 100` which genuinely blocks for 100s (far beyond our timeout).
        // `cat` exits quickly once stdin closes, so it does NOT test timeout behavior.
        // kill_on_drop(true) ensures the subprocess is reaped when the future is cancelled.
        let mut rt = PluginTransportRuntime::new();
        rt.register(
            "hang-plugin".to_string(),
            TransportHandle::Stdio {
                command: "sleep".to_string(),
                args: vec!["100".to_string()],
            },
        );
        // 3000ms timeout — well above the 1000ms minimum clamp and stable under parallel load.
        let result = rt.invoke("hang-plugin", "tool", serde_json::json!({}), 3000).await;
        assert!(result.is_err(), "Expected timeout error");
        assert!(
            result.unwrap_err().contains("timed out"),
            "Error should mention timed out"
        );
    }

    #[tokio::test]
    async fn register_multiple_plugins() {
        let mut rt = PluginTransportRuntime::new();
        rt.register("plugin-a".to_string(), TransportHandle::Local);
        rt.register("plugin-b".to_string(), TransportHandle::Local);
        let r1 = rt.invoke("plugin-a", "tool_a", serde_json::json!({}), 5000).await.unwrap();
        let r2 = rt.invoke("plugin-b", "tool_b", serde_json::json!({}), 5000).await.unwrap();
        assert!(!r1.is_error);
        assert!(!r2.is_error);
    }

    #[tokio::test]
    async fn local_transport_zero_tokens_and_cost() {
        let rt = make_runtime_with_local("test");
        let result = rt.invoke("test", "tool", serde_json::json!({}), 5000).await.unwrap();
        assert_eq!(result.tokens_used, 0);
        assert!((result.cost_usd - 0.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn stdio_nonexistent_command_returns_err() {
        let mut rt = PluginTransportRuntime::new();
        rt.register(
            "bad-plugin".to_string(),
            TransportHandle::Stdio {
                command: "/nonexistent/binary/path_xyz".to_string(),
                args: vec![],
            },
        );
        let result = rt.invoke("bad-plugin", "tool", serde_json::json!({}), 2000).await;
        assert!(result.is_err());
    }

    #[test]
    fn runtime_default_is_empty() {
        let rt = PluginTransportRuntime::default();
        assert!(rt.handles.is_empty());
    }

    #[tokio::test]
    async fn local_latency_is_set() {
        let rt = make_runtime_with_local("latency-test");
        let result = rt
            .invoke("latency-test", "tool", serde_json::json!({}), 5000)
            .await
            .unwrap();
        // latency_ms is set after the await — should be >= 0 (usually 0 for local)
        assert!(result.latency_ms < 1000, "latency_ms should be low for local transport");
    }

    #[tokio::test]
    async fn invoke_with_complex_args() {
        let rt = make_runtime_with_local("complex");
        let args = serde_json::json!({
            "nested": {"key": [1, 2, 3]},
            "flag": true
        });
        let result = rt.invoke("complex", "process", args, 5000).await.unwrap();
        assert!(!result.is_error);
    }
}
