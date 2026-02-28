//! Claude Code subprocess provider — V2.
//!
//! Wraps the `claude` CLI as a **persistent subprocess** communicating over
//! stdin/stdout via the NDJSON stream-json protocol.
//!
//! ## Architecture
//!
//! ```text
//! ClaudeCodeProvider
//!   └── Arc<Mutex<ManagedProcess>>
//!         └── Box<dyn CliTransport>
//!               ├── ProcessTransport  (production: real subprocess)
//!               └── MockTransport     (tests: in-process scripted responses)
//! ```
//!
//! ## Key improvements over Goose's `claude_code.rs`
//!
//! | Feature                      | Goose             | Halcon V2                    |
//! |------------------------------|-------------------|------------------------------|
//! | Subprocess handle            | `OnceCell` (immutable) | `Arc<Mutex<ManagedProcess>>` |
//! | Auto-restart on crash        | ❌                | ✅ exponential backoff        |
//! | Drain timeout                | 30 s hardcoded    | Configurable                 |
//! | `--system-prompt` flag       | ✅                | ✅ with change detection      |
//! | `--include-partial-messages` | ✅                | ✅                            |
//! | `--verbose`                  | ✅                | ✅                            |
//! | Model switching              | `control_request` | ✅ `send_set_model`           |
//! | Last-user-only strategy      | ✅                | ✅ (fixes original halcon)    |
//! | Protocol state machine       | Implicit          | ✅ `ProtocolState` enum       |
//! | Mockable transport           | ❌                | ✅ `CliTransport` trait       |
//! | `Approve` mode               | Hard error        | Non-fatal `ConfigError`       |
//! | Token counting               | Approximate       | Marked approximate; prefers CLI value |
//! | Observability                | None              | `ProviderMetrics`             |

pub mod managed;
pub mod metrics;
pub mod process;
pub mod protocol;
pub mod transport;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::ModelProvider;
use halcon_core::types::{ContentBlock, MessageContent, ModelChunk, ModelInfo, ModelRequest, TokenCost};

use managed::{ManagedProcess, RespawnFactory};
use metrics::ProviderMetrics;
use process::{ProcessTransport, SpawnConfig, SpawnMode};
use protocol::request_to_ndjson;
use transport::CliTransport;

// ─────────────────────────────────────────────────────────────────────────────
// Permission mode
// ─────────────────────────────────────────────────────────────────────────────

/// Permission mode for the Claude Code subprocess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClaudeCodeMode {
    /// `--dangerously-skip-permissions` — fully autonomous.
    Auto,
    /// `--permission-mode acceptEdits` — approve edits automatically.
    SmartApprove,
    /// No extra flags — interactive / chat mode (requires TUI approval).
    #[default]
    Chat,
    /// Not supported by stream-json; returns a non-fatal `ConfigError`.
    Approve,
}

impl ClaudeCodeMode {
    fn to_spawn_mode(self) -> SpawnMode {
        match self {
            Self::Auto => SpawnMode::Auto,
            Self::SmartApprove => SpawnMode::SmartApprove,
            Self::Chat | Self::Approve => SpawnMode::Chat,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for `ClaudeCodeProvider`.
#[derive(Debug, Clone)]
pub struct ClaudeCodeConfig {
    /// Path or name of the `claude` binary (default: `"claude"`).
    pub command: String,
    /// Permission mode.
    pub mode: ClaudeCodeMode,
    /// Drain timeout in seconds (default: 30).
    pub drain_timeout_secs: u64,
    /// Automatically restart the subprocess on failure (default: `true`).
    pub auto_restart: bool,
    /// Pass `--strict-mcp-config` when `mcp_config` is set.
    pub mcp_strict: bool,
    /// Optional path to an MCP config file.
    pub mcp_config: Option<PathBuf>,
    /// Request timeout in seconds (default: 120).
    pub request_timeout_secs: u64,
}

impl Default for ClaudeCodeConfig {
    fn default() -> Self {
        Self {
            command: "claude".into(),
            mode: ClaudeCodeMode::Chat,
            drain_timeout_secs: 30,
            auto_restart: true,
            mcp_strict: false,
            mcp_config: None,
            request_timeout_secs: 120,
        }
    }
}

impl ClaudeCodeConfig {
    /// Parse from a `ProviderConfig.extra` map.
    ///
    /// Keys: `command`, `mode`, `drain_timeout_secs`, `auto_restart`,
    ///       `mcp_strict`, `mcp_config_path`, `request_timeout_secs`.
    pub fn from_provider_extra(extra: &HashMap<String, serde_json::Value>) -> Self {
        let command = extra
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("claude")
            .to_string();

        let mode = extra
            .get("mode")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "auto" => ClaudeCodeMode::Auto,
                "smart_approve" => ClaudeCodeMode::SmartApprove,
                "approve" => ClaudeCodeMode::Approve,
                _ => ClaudeCodeMode::Chat,
            })
            .unwrap_or_default();

        let drain_timeout_secs = extra
            .get("drain_timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        let auto_restart = extra
            .get("auto_restart")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let mcp_strict = extra
            .get("mcp_strict")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mcp_config = extra
            .get("mcp_config_path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from);

        let request_timeout_secs = extra
            .get("request_timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(120);

        Self {
            command,
            mode,
            drain_timeout_secs,
            auto_restart,
            mcp_strict,
            mcp_config,
            request_timeout_secs,
        }
    }

    fn to_spawn_config(&self, model: Option<&str>, system_prompt: Option<&str>) -> SpawnConfig {
        SpawnConfig {
            command: self.command.clone(),
            mode: self.mode.to_spawn_mode(),
            mcp_config: self.mcp_config.clone(),
            mcp_strict: self.mcp_strict,
            model: model.map(|m| m.to_string()),
            system_prompt: system_prompt.map(|s| s.to_string()),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider
// ─────────────────────────────────────────────────────────────────────────────

/// `ModelProvider` that delegates to a persistent `claude` CLI subprocess.
///
/// A single subprocess is reused across all requests in a session.
/// If the subprocess crashes, it is respawned automatically (up to 3 times).
pub struct ClaudeCodeProvider {
    config: ClaudeCodeConfig,
    managed: Arc<Mutex<ManagedProcess>>,
    metrics: Arc<Mutex<ProviderMetrics>>,
    models: Vec<ModelInfo>,
    /// Shared spawn config updated before the first spawn so `--model` is passed correctly.
    spawn_config: Arc<tokio::sync::RwLock<SpawnConfig>>,
}

impl std::fmt::Debug for ClaudeCodeProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeCodeProvider")
            .field("command", &self.config.command)
            .field("mode", &self.config.mode)
            .field("drain_timeout_secs", &self.config.drain_timeout_secs)
            .finish()
    }
}

impl ClaudeCodeProvider {
    /// Create a new provider (production constructor — uses real subprocess).
    ///
    /// The subprocess is spawned lazily on the first `invoke()` call.
    pub fn new(config: ClaudeCodeConfig) -> Self {
        let drain_timeout = Duration::from_secs(config.drain_timeout_secs);
        let spawn_config = Arc::new(tokio::sync::RwLock::new(
            config.to_spawn_config(None, None),
        ));

        let factory = Self::make_factory(Arc::clone(&spawn_config));
        let managed = ManagedProcess::new(factory, "default", drain_timeout);
        let models = Self::default_models(&config.command);

        Self {
            config,
            managed: Arc::new(Mutex::new(managed)),
            metrics: Arc::new(Mutex::new(ProviderMetrics::new())),
            models,
            spawn_config,
        }
    }

    /// Test constructor: injects a pre-built transport (no subprocess).
    ///
    /// Sets `initial_model = "claude-opus-4-6"` so test requests with that model
    /// do not trigger `send_set_model` (no `control_response` needed in the mock).
    /// Respawn is disabled — the factory always errors.
    pub fn for_test(transport: impl CliTransport + 'static, config: ClaudeCodeConfig) -> Self {
        let drain_timeout = Duration::from_secs(config.drain_timeout_secs);
        let managed =
            ManagedProcess::with_transport(Box::new(transport), drain_timeout, "claude-opus-4-6");
        let models = Self::default_models(&config.command);
        let spawn_config = Arc::new(tokio::sync::RwLock::new(
            config.to_spawn_config(Some("claude-opus-4-6"), None),
        ));

        Self {
            config,
            managed: Arc::new(Mutex::new(managed)),
            metrics: Arc::new(Mutex::new(ProviderMetrics::new())),
            models,
            spawn_config,
        }
    }

    // ── Factory ───────────────────────────────────────────────────────────────

    fn make_factory(
        spawn_config: Arc<tokio::sync::RwLock<SpawnConfig>>,
    ) -> RespawnFactory {
        Arc::new(move || {
            let cfg_arc = spawn_config.clone();
            Box::pin(async move {
                let cfg = cfg_arc.read().await;
                let transport = ProcessTransport::spawn(&cfg).await?;
                Ok(Box::new(transport) as Box<dyn CliTransport>)
            })
        })
    }

    // ── Models ────────────────────────────────────────────────────────────────

    fn default_models(command: &str) -> Vec<ModelInfo> {
        vec![
            // Sonnet first — used as the default fallback when the global model
            // ("deepseek-chat") is not available for this provider.
            ModelInfo {
                id: "claude-sonnet-4-6".into(),
                name: "Claude Sonnet 4.6 (via claude-code)".into(),
                provider: "claude_code".into(),
                context_window: 200_000,
                max_output_tokens: 8_192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 0.0,
                cost_per_output_token: 0.0,
            },
            ModelInfo {
                id: "claude-opus-4-6".into(),
                name: "Claude Opus 4.6 (via claude-code)".into(),
                provider: "claude_code".into(),
                context_window: 200_000,
                max_output_tokens: 32_000,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                // Billed via Claude subscription, not tracked here.
                cost_per_input_token: 0.0,
                cost_per_output_token: 0.0,
            },
            ModelInfo {
                // Allow the command name (e.g. "claude") as a model alias.
                id: command.to_string(),
                name: format!("Claude Code ({})", command),
                provider: "claude_code".into(),
                context_window: 200_000,
                max_output_tokens: 8_192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.0,
                cost_per_output_token: 0.0,
            },
        ]
    }

    // ── Metrics accessor ──────────────────────────────────────────────────────

    /// Snapshot the current metrics.
    pub async fn metrics(&self) -> ProviderMetrics {
        self.metrics.lock().await.clone()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ModelProvider impl
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl ModelProvider for ClaudeCodeProvider {
    fn name(&self) -> &str {
        "claude_code"
    }

    fn supported_models(&self) -> &[ModelInfo] {
        &self.models
    }

    async fn invoke(
        &self,
        request: &ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelChunk>>> {
        // ── Approve mode guard ────────────────────────────────────────────────
        if self.config.mode == ClaudeCodeMode::Approve {
            return Err(HalconError::ConfigError(
                "claude-code: 'Approve' mode is not supported by the stream-json protocol. \
                 Use 'Auto' or 'SmartApprove' in your config."
                    .into(),
            ));
        }

        let config = self.config.clone();
        let managed_arc = Arc::clone(&self.managed);
        let metrics_arc = Arc::clone(&self.metrics);
        let spawn_config_arc = Arc::clone(&self.spawn_config);
        let request = request.clone();
        let request_timeout = Duration::from_secs(config.request_timeout_secs);

        // Run I/O in a task to hold the async Mutex properly.
        let result = tokio::spawn(async move {
            let mut mgd = managed_arc.lock().await;
            let t0 = Instant::now();
            let model = &request.model;

            // ── Pre-spawn: bake model into SpawnConfig ────────────────────────
            // When the subprocess hasn't been started yet, bake the requested
            // model into SpawnConfig so it's passed as `--model <name>` at spawn
            // time. This avoids a `send_set_model` control_request on first use.
            //
            // NOTE: We intentionally do NOT pass `request.system` as the subprocess
            // system-prompt. The halcon agent's system prompt is large (thousands of
            // tokens of tool definitions) and causes the subprocess to try using
            // those tools on every request, leading to 30+ second timeouts.
            // The claude subprocess uses its own ~/.claude/CLAUDE.md system prompt.
            if mgd.spawn_count == 0 && !model.is_empty() && model != "default"
                && !model.contains('/') // skip if value is a command path, not a model ID
            {
                let mut sc = spawn_config_arc.write().await;
                sc.model = Some(model.to_string());
                // system_prompt intentionally NOT set from request.system
            }

            // ── Ensure subprocess is alive ─────────────────────────────────────
            // No system-prompt re-spawn: we never inject request.system into the
            // subprocess, so the spawned system prompt is always the CLI's own default.
            let did_spawn = mgd.ensure_healthy().await?;
            if did_spawn {
                // After a fresh spawn, set current_model to the requested model so
                // `send_set_model` is NOT triggered (the subprocess was already
                // started with `--model <model>` in SpawnConfig).
                mgd.set_current_model(model);
                let mut m = metrics_arc.lock().await;
                m.record_spawn(mgd.spawn_count > 1);
            }

            // ── Drain any pending response from a previous cancelled request ───
            let ((), drain_timed_out) = mgd.drain_pending().await;
            if mgd.spawn_count > 0 {
                let mut m = metrics_arc.lock().await;
                m.record_drain(drain_timed_out);
            }

            // ── Model switching (no re-spawn needed) ──────────────────────────
            if !model.is_empty() && model != "default" && *model != mgd.current_model() {
                debug!(model = %model, "claude-code: switching model");
                mgd.send_set_model(model).await?;
                let mut m = metrics_arc.lock().await;
                m.record_model_switch();
            }

            // ── Build NDJSON: last-user-only strategy ──────────────────────────
            let session_id = Uuid::new_v4().to_string();
            let ndjson = request_to_ndjson(&request, &session_id);

            info!(
                session_id = %session_id,
                model = %request.model,
                "claude-code: dispatching request"
            );

            // ── Execute request ────────────────────────────────────────────────
            let chunks = mgd.execute_request(&ndjson, request_timeout).await?;

            // ── Record metrics ─────────────────────────────────────────────────
            let ttft = t0.elapsed();
            let (input_tokens, output_tokens, has_error) = chunks.iter().fold(
                (0u32, 0u32, false),
                |(inp, out, err), c| match c {
                    ModelChunk::Usage(u) => (inp + u.input_tokens, out + u.output_tokens, err),
                    ModelChunk::Error(_) => (inp, out, true),
                    _ => (inp, out, err),
                },
            );
            {
                let mut m = metrics_arc.lock().await;
                m.record_request(input_tokens, output_tokens, ttft, has_error);
            }

            Ok(chunks)
        })
        .await
        .map_err(|e| HalconError::Internal(format!("claude-code task join: {e}")))?;

        let chunks = result?;
        Ok(Box::pin(futures::stream::iter(
            chunks.into_iter().map(Ok),
        )))
    }

    async fn is_available(&self) -> bool {
        tokio::process::Command::new(&self.config.command)
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn estimate_cost(&self, request: &ModelRequest) -> TokenCost {
        // Cost is billed through the Claude subscription (Pro/Max).
        // Token count here is a rough approximation (chars/4); actual usage
        // is returned in ModelChunk::Usage from the CLI.
        let approx_input_tokens: u32 = request
            .messages
            .iter()
            .map(|m| estimate_content_chars(&m.content))
            .sum::<usize>()
            .saturating_add(request.system.as_deref().map(|s| s.len()).unwrap_or(0))
            .div_ceil(4) as u32;

        TokenCost {
            estimated_input_tokens: approx_input_tokens,
            // Always 0.0 — billed by Anthropic, not tracked in halcon.
            estimated_cost_usd: 0.0,
        }
    }

    fn tool_format(&self) -> halcon_core::types::ToolFormat {
        halcon_core::types::ToolFormat::AnthropicInputSchema
    }

    fn tokenizer_hint(&self) -> halcon_core::types::TokenizerHint {
        halcon_core::types::TokenizerHint::ClaudeBpe
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

fn estimate_content_chars(content: &MessageContent) -> usize {
    match content {
        MessageContent::Text(t) => t.len(),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .map(|b| match b {
                ContentBlock::Text { text } => text.len(),
                ContentBlock::ToolResult { content, .. } => content.len(),
                _ => 0,
            })
            .sum(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude_code::transport::{mock_success_response, MockTransport};
    use futures::StreamExt;
    use halcon_core::types::{ChatMessage, Role};

    fn user_request(text: &str) -> ModelRequest {
        ModelRequest {
            model: "claude-opus-4-6".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(text.into()),
            }],
            tools: vec![],
            max_tokens: Some(128),
            temperature: None,
            system: None,
            stream: true,
        }
    }

    fn make_provider(mock: MockTransport) -> ClaudeCodeProvider {
        ClaudeCodeProvider::for_test(mock, ClaudeCodeConfig::default())
    }

    // ── Contract tests (no subprocess) ───────────────────────────────────────

    #[test]
    fn provider_name_is_claude_code() {
        let p = make_provider(MockTransport::new());
        assert_eq!(p.name(), "claude_code");
    }

    #[test]
    fn supported_models_not_empty() {
        let p = make_provider(MockTransport::new());
        assert!(!p.supported_models().is_empty());
    }

    #[test]
    fn all_models_have_correct_provider_field() {
        let p = make_provider(MockTransport::new());
        for m in p.supported_models() {
            assert_eq!(m.provider, "claude_code");
        }
    }

    #[test]
    fn estimate_cost_is_zero_usd() {
        let p = make_provider(MockTransport::new());
        let req = user_request("hello");
        assert_eq!(p.estimate_cost(&req).estimated_cost_usd, 0.0);
    }

    #[test]
    fn estimate_cost_non_zero_tokens_for_non_empty_msg() {
        let p = make_provider(MockTransport::new());
        let req = user_request("hello world test");
        assert!(p.estimate_cost(&req).estimated_input_tokens > 0);
    }

    // ── Approve mode guard ────────────────────────────────────────────────────

    #[tokio::test]
    async fn approve_mode_returns_config_error() {
        let mut config = ClaudeCodeConfig::default();
        config.mode = ClaudeCodeMode::Approve;
        let p = ClaudeCodeProvider::for_test(MockTransport::new(), config);
        let result = p.invoke(&user_request("hi")).await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(matches!(err, HalconError::ConfigError(_)), "expected ConfigError, got {err:?}");
    }

    // ── Full invoke via mock ──────────────────────────────────────────────────

    #[tokio::test]
    async fn invoke_returns_text_delta_and_done() {
        let mut mock = MockTransport::new();
        mock.queue_response(mock_success_response("Hello from provider!"));

        let p = make_provider(mock);
        let mut stream = p.invoke(&user_request("hi")).await.unwrap();

        let mut got_text = false;
        let mut got_done = false;

        while let Some(Ok(chunk)) = stream.next().await {
            match chunk {
                ModelChunk::TextDelta(t) if t.contains("Hello from provider!") => {
                    got_text = true;
                }
                ModelChunk::Done(_) => got_done = true,
                _ => {}
            }
        }

        assert!(got_text, "missing TextDelta");
        assert!(got_done, "missing Done");
    }

    #[tokio::test]
    async fn invoke_multiple_requests_sequential() {
        let mut mock = MockTransport::new();
        mock.queue_response(mock_success_response("response one"));
        mock.queue_response(mock_success_response("response two"));

        let p = make_provider(mock);

        let mut s1 = p.invoke(&user_request("first")).await.unwrap();
        let chunks1: Vec<_> = futures::StreamExt::collect::<Vec<_>>(&mut s1).await;
        let text1: String = chunks1
            .iter()
            .filter_map(|r| {
                if let Ok(ModelChunk::TextDelta(t)) = r { Some(t.as_str()) } else { None }
            })
            .collect();
        assert!(text1.contains("response one"), "got: {text1}");

        let mut s2 = p.invoke(&user_request("second")).await.unwrap();
        let chunks2: Vec<_> = futures::StreamExt::collect::<Vec<_>>(&mut s2).await;
        let text2: String = chunks2
            .iter()
            .filter_map(|r| {
                if let Ok(ModelChunk::TextDelta(t)) = r { Some(t.as_str()) } else { None }
            })
            .collect();
        assert!(text2.contains("response two"), "got: {text2}");
    }

    // ── Metrics tracking ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn metrics_tracks_requests() {
        let mut mock = MockTransport::new();
        mock.queue_response(mock_success_response("data"));
        let p = make_provider(mock);

        p.invoke(&user_request("test")).await.unwrap().for_each(|_| async {}).await;
        let m = p.metrics().await;
        assert_eq!(m.total_requests, 1);
    }

    // ── Config parsing ────────────────────────────────────────────────────────

    #[test]
    fn config_defaults_from_empty_extra() {
        let extra: HashMap<String, serde_json::Value> = HashMap::new();
        let cfg = ClaudeCodeConfig::from_provider_extra(&extra);
        assert_eq!(cfg.command, "claude");
        assert_eq!(cfg.mode, ClaudeCodeMode::Chat);
        assert_eq!(cfg.drain_timeout_secs, 30);
        assert!(cfg.auto_restart);
        assert!(!cfg.mcp_strict);
        assert!(cfg.mcp_config.is_none());
        assert_eq!(cfg.request_timeout_secs, 120);
    }

    #[test]
    fn config_mode_auto() {
        let mut extra = HashMap::new();
        extra.insert("mode".into(), serde_json::json!("auto"));
        let cfg = ClaudeCodeConfig::from_provider_extra(&extra);
        assert_eq!(cfg.mode, ClaudeCodeMode::Auto);
    }

    #[test]
    fn config_mode_smart_approve() {
        let mut extra = HashMap::new();
        extra.insert("mode".into(), serde_json::json!("smart_approve"));
        let cfg = ClaudeCodeConfig::from_provider_extra(&extra);
        assert_eq!(cfg.mode, ClaudeCodeMode::SmartApprove);
    }

    #[test]
    fn config_mode_approve_parsed() {
        let mut extra = HashMap::new();
        extra.insert("mode".into(), serde_json::json!("approve"));
        let cfg = ClaudeCodeConfig::from_provider_extra(&extra);
        assert_eq!(cfg.mode, ClaudeCodeMode::Approve);
    }

    #[test]
    fn config_unknown_mode_defaults_to_chat() {
        let mut extra = HashMap::new();
        extra.insert("mode".into(), serde_json::json!("unknown_mode"));
        let cfg = ClaudeCodeConfig::from_provider_extra(&extra);
        assert_eq!(cfg.mode, ClaudeCodeMode::Chat);
    }

    #[test]
    fn config_request_timeout_parsed() {
        let mut extra = HashMap::new();
        extra.insert("request_timeout_secs".into(), serde_json::json!(200u64));
        let cfg = ClaudeCodeConfig::from_provider_extra(&extra);
        assert_eq!(cfg.request_timeout_secs, 200);
    }

    #[test]
    fn debug_impl_does_not_panic() {
        let p = make_provider(MockTransport::new());
        let _ = format!("{p:?}");
    }
}
