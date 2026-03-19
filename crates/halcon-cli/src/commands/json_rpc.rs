/**
 * JSON-RPC stdio mode for the Halcon VS Code extension (Feature 6).
 *
 * Protocol (newline-delimited JSON):
 *   stdin  ← { id?, method, params? }
 *   stdout → { event, data? }        (streaming events during agent loop)
 *          → { event: "pong", id? }  (ping reply)
 *          → { event: "done" }       (agent turn finished)
 *          → { event: "error", data: "msg" }
 *
 * Supported methods:
 *   ping   — health check, replies immediately with pong
 *   chat   — { message: String, context?: {...} } — runs the agent loop
 *   cancel — acknowledges with done (graceful; does not kill in-flight work)
 */

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use halcon_core::types::ContentBlock;
use serde_json::Value;

use crate::render::sink::RenderSink;

// ── JSON-RPC sink ─────────────────────────────────────────────────────────────

/// A `RenderSink` that serialises every agent event as newline-delimited JSON
/// to stdout. All ANSI colour output is suppressed; only JSON goes to stdout.
pub struct JsonRpcSink {
    stdout: Arc<Mutex<std::io::Stdout>>,
}

impl JsonRpcSink {
    pub fn new() -> Self {
        Self {
            stdout: Arc::new(Mutex::new(std::io::stdout())),
        }
    }

    fn emit(&self, event: &str, data: Value) {
        let msg = serde_json::json!({ "event": event, "data": data });
        if let Ok(mut out) = self.stdout.lock() {
            let _ = writeln!(out, "{msg}");
            let _ = out.flush();
        }
    }
}

impl RenderSink for JsonRpcSink {
    fn stream_text(&self, text: &str) {
        self.emit("token", serde_json::json!({ "text": text }));
    }

    fn stream_thinking(&self, text: &str) {
        // Surface thinking tokens with a distinct event so the webview can dim them.
        self.emit("thinking", serde_json::json!({ "text": text }));
    }

    fn stream_code_block(&self, _lang: &str, _code: &str) {
        // Already included in the token stream — no separate event needed.
    }

    fn stream_tool_marker(&self, name: &str) {
        self.emit("tool_call", serde_json::json!({ "name": name }));
    }

    fn stream_done(&self) {
        // Individual stream completion — not session done.
    }

    fn stream_error(&self, msg: &str) {
        self.emit("error", Value::String(msg.to_string()));
    }

    fn tool_start(&self, name: &str, _input: &serde_json::Value) {
        self.emit("tool_call", serde_json::json!({ "name": name }));
    }

    fn tool_output(&self, block: &ContentBlock, _duration_ms: u64) {
        match block {
            ContentBlock::ToolResult { content, is_error, .. } => {
                self.emit(
                    "tool_result",
                    serde_json::json!({
                        "success": !is_error,
                        "output": content,
                    }),
                );
            }
            ContentBlock::Text { text } => {
                self.emit(
                    "tool_result",
                    serde_json::json!({ "success": true, "output": text }),
                );
            }
            _ => {}
        }
    }

    fn tool_denied(&self, name: &str) {
        self.emit(
            "tool_result",
            serde_json::json!({
                "success": false,
                "output": format!("Tool '{name}' denied by permission system"),
            }),
        );
    }

    fn spinner_start(&self, _label: &str) {}
    fn spinner_stop(&self) {}

    fn warning(&self, message: &str, _hint: Option<&str>) {
        self.emit("warning", Value::String(message.to_string()));
    }

    fn error(&self, message: &str, _hint: Option<&str>) {
        self.emit("error", Value::String(message.to_string()));
    }

    fn info(&self, _message: &str) {
        // Suppress informational lines (round separators, compaction notices).
    }

    fn is_silent(&self) -> bool { false }

    fn stream_reset(&self) {}

    fn stream_full_text(&self) -> String { String::new() }
}

// ── Top-level run function ─────────────────────────────────────────────────────

/// Run Halcon in JSON-RPC mode.
///
/// Sets up the full agent runtime (providers, DB, tool registry, Repl), emits
/// an initial pong to signal readiness, then processes newline-delimited JSON
/// requests from stdin until EOF.
pub async fn run(
    config: &halcon_core::types::AppConfig,
    provider: &str,
    model: &str,
    max_turns: Option<u32>,
    explicit_model: bool,
) -> Result<()> {
    use std::sync::Arc;

    use crate::config_loader::default_db_path;
    use crate::repl::Repl;
    use halcon_storage::Database;

    let mut config = config.clone();
    // Honour --max-turns override (passed by VS Code extension).
    if let Some(turns) = max_turns {
        config.agent.limits.max_rounds = turns as usize;
    }

    // Proactively refresh Cenzontle SSO token if near-expiry (< 5 min remaining).
    // Must run before build_registry() so the refreshed token is read from the credential store.
    let _ = super::sso::refresh_if_needed().await;

    // Build provider registry.
    let mut registry = super::provider_factory::build_registry(&config);
    super::provider_factory::ensure_local_fallback(&mut registry).await;
    super::provider_factory::ensure_cenzontle_models(&mut registry).await;

    let (provider_str, model_str) = super::provider_factory::precheck_providers_explicit(
        &registry, provider, model, explicit_model,
    )
    .await?;

    // Open database (non-fatal).
    let db_path = config.storage.database_path.clone().unwrap_or_else(default_db_path);
    let db = match Database::open(&db_path) {
        Ok(db) => Some(Arc::new(db)),
        Err(e) => {
            tracing::warn!("json-rpc: could not open database: {e}");
            None
        }
    };

    let (event_tx, event_rx) = halcon_core::event_bus(256);
    drop(event_rx); // Not used in JSON-RPC mode.

    let tool_registry = halcon_tools::full_registry(&config.tools, None, db.clone(), None);

    let mut repl = Repl::new(
        &config,
        provider_str,
        model_str,
        db,
        None, // no session resume in JSON-RPC mode
        registry,
        tool_registry,
        event_tx,
        true,  // no_banner — suppress all decorative output
        explicit_model,
    )?;

    // Auto-approve tools (no TTY in IDE sidecar).
    repl.set_non_interactive_mode();

    let sink = Arc::new(JsonRpcSink::new());

    // Signal readiness: the extension waits for a pong before sending requests.
    emit_line(&serde_json::json!({ "event": "pong" }));

    // Process requests from stdin until EOF.
    let stdin = std::io::stdin();
    for line_result in stdin.lock().lines() {
        let line = match line_result {
            Ok(l) => l.trim().to_string(),
            Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }

        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // Non-JSON — skip silently.
        };

        let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let id = msg.get("id").cloned();

        match method {
            "ping" => {
                let pong = match id {
                    Some(id_val) => serde_json::json!({ "event": "pong", "id": id_val }),
                    None => serde_json::json!({ "event": "pong" }),
                };
                emit_line(&pong);
            }

            "cancel" => {
                // We acknowledge the cancel. In-flight work finishes naturally;
                // subsequent chat requests will start fresh.
                emit_line(&serde_json::json!({ "event": "done" }));
            }

            "chat" => {
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                let message = params
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if message.is_empty() {
                    emit_line(&serde_json::json!({
                        "event": "error",
                        "data": "chat.params.message is required"
                    }));
                    continue;
                }

                // Append VS Code context (active file, diagnostics, git state) if provided.
                let full_message = build_message_with_context(&message, params.get("context"));

                match repl.run_json_rpc_turn(&full_message, sink.as_ref()).await {
                    Ok(_) => {
                        emit_line(&serde_json::json!({ "event": "done" }));
                    }
                    Err(e) => {
                        emit_line(&serde_json::json!({
                            "event": "error",
                            "data": e.to_string()
                        }));
                        emit_line(&serde_json::json!({ "event": "done" }));
                    }
                }
            }

            _ => {
                emit_line(&serde_json::json!({
                    "event": "error",
                    "data": format!("unknown method: {method}")
                }));
            }
        }
    }

    Ok(())
}

/// Append a `<vscode_context>` block to the user message when the extension
/// has sent IDE context (active file, diagnostics, git branch, etc.).
fn build_message_with_context(message: &str, context: Option<&Value>) -> String {
    match context {
        Some(ctx) if !ctx.is_null() => {
            let ctx_pretty = serde_json::to_string_pretty(ctx).unwrap_or_default();
            if ctx_pretty.is_empty() || ctx_pretty == "null" {
                return message.to_string();
            }
            format!("{message}\n\n<vscode_context>\n{ctx_pretty}\n</vscode_context>")
        }
        _ => message.to_string(),
    }
}

/// Write a JSON value as a single newline-terminated record to stdout.
fn emit_line(val: &Value) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{val}");
    let _ = out.flush();
}
