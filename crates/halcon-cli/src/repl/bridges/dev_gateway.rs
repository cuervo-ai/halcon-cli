//! Phase 5C: Dev Gateway
//!
//! Thin async gateway layer that coordinates all dev-ecosystem subsystems and
//! exposes a single entry point for the agent loop to call when answering
//! developer queries.
//!
//! Responsibilities:
//! 1. Provide the LSP-compatible TCP/stdio server scaffold that routes raw
//!    JSON-RPC bytes to `IdeProtocolHandler::handle_raw()`.
//! 2. Aggregate dev-context from open buffers, git state, and CI results into
//!    a single context block suitable for system-prompt injection.
//! 3. Expose a `DevContext` snapshot for the reward pipeline and mod.rs.

use std::sync::Arc;

use tokio::sync::Mutex;

use super::super::ci_result_ingestor::{CiEvent, CiRunRecord};
use super::super::ide_protocol_handler::IdeProtocolHandler;
use super::super::unsaved_buffer_tracker::UnsavedBufferTracker;

// ── DevContext snapshot ───────────────────────────────────────────────────────

/// A point-in-time snapshot of the full dev ecosystem state.
///
/// Built by `DevGateway::build_context()` and injected into the agent's
/// system prompt as a structured Markdown block.
#[derive(Debug, Clone, Default)]
pub struct DevContext {
    /// Current git branch / commit / status summary.
    pub git_summary: Option<String>,
    /// Number of open IDE buffers.
    pub open_buffers: usize,
    /// Context block from unsaved buffers (truncated).
    pub buffer_block: String,
    /// Phase 6: AST symbol index rendered from open buffers (budget-aware).
    pub symbol_block: String,
    /// Most recent terminal CI run record, if available.
    pub latest_ci: Option<CiRunRecord>,
    /// Combined environment reward in [0, 1] (for UCB1 blending).
    pub env_reward: f64,
}

impl DevContext {
    /// Render as a Markdown block for system-prompt injection.
    pub fn as_markdown(&self) -> String {
        if self.git_summary.is_none() && self.open_buffers == 0 && self.latest_ci.is_none() {
            return String::new();
        }

        let mut out = String::from("## Dev Ecosystem Context\n");

        if let Some(ref git) = self.git_summary {
            out.push_str(&format!("\n### Git\n{git}\n"));
        }

        if self.open_buffers > 0 {
            out.push_str(&format!(
                "\n### Open Buffers ({} files)\n{}\n",
                self.open_buffers, self.buffer_block
            ));
        }

        if !self.symbol_block.is_empty() {
            out.push_str(&format!("\n### Symbols\n```\n{}\n```\n", self.symbol_block));
        }

        if let Some(ref ci) = self.latest_ci {
            out.push_str(&format!(
                "\n### CI ({} — {} — reward {:.2})\n",
                ci.workflow_name,
                format!("{:?}", ci.status),
                ci.reward
            ));
            if let Some(ref tr) = ci.test_results {
                out.push_str(&format!("{}\n", tr.summary()));
            }
        }

        out
    }
}

// ── Gateway ───────────────────────────────────────────────────────────────────

/// Coordinator for all dev-ecosystem subsystems.
///
/// Holds references to the protocol handler, buffer tracker, and latest CI
/// state. Can be cloned cheaply — all inner types are reference-counted.
#[derive(Clone)]
pub struct DevGateway {
    pub handler: Arc<IdeProtocolHandler>,
    pub buffers: Arc<UnsavedBufferTracker>,
    latest_ci: Arc<Mutex<Option<CiRunRecord>>>,
}

impl DevGateway {
    /// Create a gateway with a fresh buffer tracker.
    pub fn new() -> Self {
        let buffers = Arc::new(UnsavedBufferTracker::new());
        let handler = Arc::new(IdeProtocolHandler::new(buffers.clone()));
        Self {
            handler,
            buffers,
            latest_ci: Arc::new(Mutex::new(None)),
        }
    }

    /// Feed CI events into the gateway so `build_context()` can include them.
    ///
    /// Call this from a background task that reads the CI broadcast channel:
    /// ```text
    /// while let Ok(event) = rx.recv().await {
    ///     gw.ingest_ci_event(event).await;
    /// }
    /// ```
    pub async fn ingest_ci_event(&self, event: CiEvent) {
        if let CiEvent::RunCompleted(record) = event {
            *self.latest_ci.lock().await = Some(record);
        }
    }

    /// Dispatch a raw LSP JSON-RPC message to the protocol handler.
    ///
    /// Returns the serialized response bytes, or an empty vec for notifications.
    pub async fn handle_lsp_message(&self, raw: &[u8]) -> Vec<u8> {
        match self.handler.handle_raw(raw).await {
            Ok(super::super::ide_protocol_handler::DispatchResult::Response(resp)) => {
                serde_json::to_vec(&resp).unwrap_or_default()
            }
            Ok(super::super::ide_protocol_handler::DispatchResult::Error(err)) => {
                serde_json::to_vec(&err).unwrap_or_default()
            }
            Ok(super::super::ide_protocol_handler::DispatchResult::Notification) => vec![],
            Err(e) => {
                tracing::warn!(error = %e, "LSP message parse error");
                vec![]
            }
        }
    }

    /// Build a complete `DevContext` snapshot from all subsystems.
    ///
    /// Collects git state via blocking I/O in `spawn_blocking`, reads buffer
    /// content, and copies the latest CI record. Safe to call from async code.
    pub async fn build_context(&self) -> DevContext {
        // Git context (blocking I/O on background thread).
        let git_summary = tokio::task::spawn_blocking(|| {
            super::super::git_context::collect(std::path::Path::new(".")).map(|gc| gc.summary())
        })
        .await
        .unwrap_or(None);

        // Buffer context.
        let open_buffers = self.buffers.len().await;
        let buffer_block = if open_buffers > 0 {
            self.buffers.context_block(1024).await
        } else {
            String::new()
        };

        // Phase 6: AST symbol extraction from open buffers (budget-aware).
        // Extract symbols from each tracked buffer and render a compact index.
        let symbol_block = if open_buffers > 0 {
            let uris = self.buffers.tracked_uris().await;
            let mut sym_out = String::new();
            // Budget: 256 chars per file, max 8 files to stay token-efficient.
            for uri in uris.iter().take(8) {
                if let Some(content) = self.buffers.content(uri).await {
                    let index =
                        super::super::ast_symbol_extractor::extract_from_buffer(uri, &content);
                    if !index.is_empty() {
                        sym_out.push_str(&index.render(256));
                    }
                }
            }
            sym_out
        } else {
            String::new()
        };

        // CI context.
        let latest_ci = self.latest_ci.lock().await.clone();
        let env_reward = latest_ci.as_ref().map(|r| r.reward).unwrap_or(0.5); // neutral when no CI data

        DevContext {
            git_summary,
            open_buffers,
            buffer_block,
            symbol_block,
            latest_ci,
            env_reward,
        }
    }

    /// Run a minimal TCP JSON-RPC server that accepts one connection at a time.
    ///
    /// This is a thin scaffold — production deployments should use a full LSP
    /// server crate (e.g. `tower-lsp`). Suitable for integration testing with
    /// IDE extensions via `--lsp-port`.
    ///
    /// Runs until the provided `stop` signal fires.
    pub async fn serve_tcp(
        self: Arc<Self>,
        addr: std::net::SocketAddr,
        stop: Arc<tokio::sync::Notify>,
    ) -> Result<(), String> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| format!("bind failed: {e}"))?;

        tracing::info!(%addr, "DevGateway LSP TCP server listening");

        loop {
            tokio::select! {
                _ = stop.notified() => {
                    tracing::info!("DevGateway LSP server shutting down");
                    break;
                }
                accept = listener.accept() => {
                    let (stream, peer) = accept.map_err(|e| format!("accept error: {e}"))?;
                    tracing::debug!(%peer, "DevGateway: LSP client connected");
                    let gw = self.clone();
                    tokio::spawn(async move {
                        let (reader, mut writer) = stream.into_split();
                        let mut buf_reader = BufReader::new(reader);

                        // Handle the full LSP Content-Length framed protocol for this
                        // connection.  Each message is:
                        //   Content-Length: <N>\r\n
                        //   \r\n
                        //   <N bytes of JSON-RPC body>
                        'connection: loop {
                            // ── 1. Read headers until blank line ─────────────────────
                            let mut content_length: Option<usize> = None;
                            loop {
                                let mut header_line = String::new();
                                match buf_reader.read_line(&mut header_line).await {
                                    Ok(0) => break 'connection, // peer closed connection
                                    Ok(_) => {}
                                    Err(_) => break 'connection, // I/O error
                                }
                                let trimmed = header_line.trim_end_matches(['\r', '\n']);
                                if trimmed.is_empty() {
                                    break; // blank line = end of headers
                                }
                                if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                                    content_length = rest.trim().parse().ok();
                                }
                                // Other headers (Content-Type etc.) are silently ignored.
                            }

                            // ── 2. Validate Content-Length ────────────────────────────
                            let body_len = match content_length {
                                Some(l) if l > 0 => l,
                                _ => {
                                    tracing::warn!(%peer, "LSP TCP: message missing/zero Content-Length");
                                    break 'connection;
                                }
                            };

                            // ── 3. Read exact body bytes ──────────────────────────────
                            let mut body = vec![0u8; body_len];
                            if let Err(e) = buf_reader.read_exact(&mut body).await {
                                tracing::warn!(%peer, error = %e, "LSP TCP: error reading body");
                                break 'connection;
                            }

                            // ── 4. Dispatch to protocol handler ───────────────────────
                            let response = gw.handle_lsp_message(&body).await;

                            // ── 5. Write Content-Length framed response ───────────────
                            // Empty response = LSP notification (no reply expected).
                            if !response.is_empty() {
                                let header = format!("Content-Length: {}\r\n\r\n", response.len());
                                if writer.write_all(header.as_bytes()).await.is_err() {
                                    break 'connection;
                                }
                                if writer.write_all(&response).await.is_err() {
                                    break 'connection;
                                }
                            }
                        }
                        tracing::debug!(%peer, "DevGateway: LSP client disconnected");
                    });
                }
            }
        }

        Ok(())
    }
}

impl Default for DevGateway {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::ci_result_ingestor::CiRunStatus;

    fn make_ci_record(status: CiRunStatus, reward: f64) -> CiRunRecord {
        CiRunRecord {
            run_id: "r1".to_string(),
            head_sha: "abc123".to_string(),
            workflow_name: "CI".to_string(),
            branch: "main".to_string(),
            status,
            test_results: None,
            reward,
        }
    }

    #[tokio::test]
    async fn gateway_builds_context_with_no_state() {
        let gw = DevGateway::new();
        let ctx = gw.build_context().await;
        // Without git repo / open buffers / CI, env_reward defaults to 0.5.
        assert_eq!(ctx.open_buffers, 0);
        assert_eq!(ctx.env_reward, 0.5);
        assert!(ctx.latest_ci.is_none());
    }

    #[tokio::test]
    async fn ingest_ci_event_stores_latest_run() {
        let gw = DevGateway::new();
        let record = make_ci_record(CiRunStatus::Success, 1.0);
        gw.ingest_ci_event(CiEvent::RunCompleted(record.clone()))
            .await;
        let ctx = gw.build_context().await;
        let ci = ctx.latest_ci.unwrap();
        assert_eq!(ci.run_id, "r1");
        assert!((ctx.env_reward - 1.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn ingest_non_completed_event_does_not_overwrite_ci() {
        let gw = DevGateway::new();
        let record = make_ci_record(CiRunStatus::Success, 1.0);
        gw.ingest_ci_event(CiEvent::RunCompleted(record)).await;

        // PollError should not clear the stored run.
        gw.ingest_ci_event(CiEvent::PollError {
            provider: "github".to_string(),
            message: "oops".to_string(),
        })
        .await;

        let ci = gw.latest_ci.lock().await.clone();
        assert!(ci.is_some());
    }

    #[tokio::test]
    async fn handle_lsp_message_did_open_returns_empty() {
        let gw = DevGateway::new();
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "file:///gw.rs",
                    "version": 1,
                    "languageId": "rust",
                    "text": "fn gw() {}"
                }
            }
        });
        let bytes = serde_json::to_vec(&msg).unwrap();
        let resp = gw.handle_lsp_message(&bytes).await;
        // Notifications produce empty response.
        assert!(resp.is_empty());
        assert_eq!(gw.buffers.len().await, 1);
    }

    #[tokio::test]
    async fn handle_lsp_message_context_returns_json() {
        let gw = DevGateway::new();
        // Open a buffer first.
        let open = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "file:///ctx.rs",
                    "version": 1,
                    "languageId": "rust",
                    "text": "fn ctx() {}"
                }
            }
        });
        gw.handle_lsp_message(&serde_json::to_vec(&open).unwrap())
            .await;

        let ctx_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "$/halcon/context",
            "params": {}
        });
        let resp_bytes = gw
            .handle_lsp_message(&serde_json::to_vec(&ctx_req).unwrap())
            .await;
        assert!(!resp_bytes.is_empty());

        let resp: serde_json::Value = serde_json::from_slice(&resp_bytes).unwrap();
        let ctx_str = resp["result"]["context"].as_str().unwrap_or("");
        assert!(ctx_str.contains("file:///ctx.rs"));
    }

    #[test]
    fn dev_context_as_markdown_empty_when_no_state() {
        let ctx = DevContext::default();
        assert!(ctx.as_markdown().is_empty());
    }

    #[test]
    fn dev_context_as_markdown_includes_git() {
        let ctx = DevContext {
            git_summary: Some("branch: main, clean".to_string()),
            ..Default::default()
        };
        let md = ctx.as_markdown();
        assert!(md.contains("### Git"));
        assert!(md.contains("branch: main"));
    }

    #[test]
    fn dev_context_env_reward_from_ci() {
        let ctx = DevContext {
            latest_ci: Some(CiRunRecord {
                run_id: "x".to_string(),
                head_sha: "s".to_string(),
                workflow_name: "CI".to_string(),
                branch: "main".to_string(),
                status: CiRunStatus::Failure,
                test_results: None,
                reward: 0.0,
            }),
            env_reward: 0.0,
            ..Default::default()
        };
        assert!((ctx.env_reward - 0.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn gateway_clone_shares_buffers() {
        let gw = DevGateway::new();
        let clone = gw.clone();
        let open = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "file:///shared.rs",
                    "version": 1,
                    "languageId": "rust",
                    "text": "shared"
                }
            }
        });
        gw.handle_lsp_message(&serde_json::to_vec(&open).unwrap())
            .await;
        // Clone sees the same buffers.
        assert!(clone.buffers.content("file:///shared.rs").await.is_some());
    }
}
