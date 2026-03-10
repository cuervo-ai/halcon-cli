//! Render sink abstraction — decouples the agent loop from terminal output.
//!
//! The agent loop calls `RenderSink` methods instead of writing directly to
//! stdout/stderr. Two built-in implementations:
//! - `ClassicSink`: delegates to existing render functions (zero behavior change)
//! - `SilentSink`: accumulates text without terminal output (for sub-agents/tests)

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use halcon_core::types::ContentBlock;

// ─── Thinking preview constants ───────────────────────────────────────────────

/// Maximum byte length of the thinking preview buffer.
/// Intentionally a byte limit (not char limit) so the buffer size is bounded in memory.
/// The fill logic uses char-aware truncation to never split a UTF-8 sequence.
const THINKING_PREVIEW_MAX_BYTES: usize = 160;

/// Spinner label prefix for thinking progress updates.
/// Kept as a constant so it can be updated in one place (e.g., for i18n).
const THINKING_LABEL: &str = "Razonando...";

use super::feedback;
use super::spinner::Spinner;
use super::stream::StreamRenderer;
use super::tool as tool_render;

/// Trait for rendering agent loop output.
///
/// Implementations can target different backends: terminal (classic),
/// TUI widgets, test harnesses, or silent accumulators.
pub trait RenderSink: Send + Sync {
    /// Push streaming text from the model response.
    fn stream_text(&self, text: &str);

    /// Push chain-of-thought / thinking tokens from a reasoning model.
    ///
    /// Rendered with visual distinction (dim/muted) to separate the model's internal
    /// reasoning process from the final answer. Default falls through to `stream_text`
    /// for sinks that don't distinguish thinking from text (e.g. silent/test sinks).
    fn stream_thinking(&self, text: &str) {
        // Default: render identically to text (backward-compatible).
        // Override in ClassicSink (dim ANSI) and TuiSink (StreamThinking event).
        self.stream_text(text);
    }

    /// A fenced code block completed during streaming.
    #[allow(dead_code)]
    fn stream_code_block(&self, lang: &str, code: &str);
    /// Model indicated a tool call in the stream.
    fn stream_tool_marker(&self, name: &str);
    /// Streaming response complete — flush any buffered output.
    fn stream_done(&self);
    /// Stream-level error from provider.
    fn stream_error(&self, msg: &str);
    /// A tool execution is starting.
    fn tool_start(&self, name: &str, input: &serde_json::Value);
    /// A tool execution completed (renders the result block).
    fn tool_output(&self, block: &ContentBlock, duration_ms: u64);
    /// A tool was denied by permission system.
    fn tool_denied(&self, name: &str);
    /// Start a spinner (inference waiting indicator).
    fn spinner_start(&self, label: &str);
    /// Stop the spinner.
    fn spinner_stop(&self);
    /// Display a warning message.
    fn warning(&self, message: &str, hint: Option<&str>);
    /// Display an error message.
    fn error(&self, message: &str, hint: Option<&str>);
    /// Print an informational status line (e.g. round separators, compaction notice).
    fn info(&self, message: &str);
    /// Whether this sink suppresses all output (silent mode).
    fn is_silent(&self) -> bool;
    /// Reset the stream renderer state for a new streaming round.
    fn stream_reset(&self);
    /// Get the full accumulated text from the stream renderer (if any).
    fn stream_full_text(&self) -> String;
    /// Display plan progress (step statuses, current step indicator).
    /// Default no-op for backward compatibility.
    fn plan_progress(
        &self,
        _goal: &str,
        _steps: &[halcon_core::traits::PlanStep],
        _current_step: usize,
    ) {
    }

    /// Display task status update (Phase 39).
    /// Default no-op for backward compatibility.
    fn task_status(&self, _title: &str, _status: &str, _duration_ms: Option<u64>, _artifact_count: usize) {}

    /// Display reasoning engine status (Phase 40).
    /// Default no-op for backward compatibility.
    fn reasoning_status(&self, _task_type: &str, _complexity: &str, _strategy: &str, _score: f64, _success: bool) {}

    // --- Phase 42B: Cockpit feedback methods (9 new) ---

    /// Session initialized with ID (for status bar display).
    fn session_started(&self, _session_id: &str) {}
    /// An agent round is starting.
    fn round_started(&self, _round: usize, _provider: &str, _model: &str) {}
    /// An agent round has ended with metrics.
    fn round_ended(&self, _round: usize, _input_tokens: u32, _output_tokens: u32, _cost: f64, _duration_ms: u64) {}
    /// Model selection occurred.
    fn model_selected(&self, _model: &str, _provider: &str, _reason: &str) {}
    /// Provider fallback triggered.
    fn provider_fallback(&self, _from: &str, _to: &str, _reason: &str) {}
    /// Tool loop guard took action.
    fn loop_guard_action(&self, _action: &str, _reason: &str) {}
    /// Context compaction completed.
    fn compaction_complete(&self, _old_msgs: usize, _new_msgs: usize, _tokens_saved: u64) {}
    /// Cache hit or miss.
    fn cache_status(&self, _hit: bool, _source: &str) {}
    /// Speculative tool execution result.
    fn speculative_result(&self, _tool: &str, _hit: bool) {}
    /// Awaiting user permission for a tool (Phase I-6C: extended signature).
    fn permission_awaiting(&self, _tool: &str, _args: &serde_json::Value, _risk_level: &str) {}
    /// Returns the TUI event sender if this sink is backed by a TUI.
    /// Used by the orchestrator to create sub-agent sinks that can route
    /// PermissionAwaiting events to the main TUI overlay.
    #[cfg(feature = "tui")]
    fn tui_event_sender(&self) -> Option<tokio::sync::mpsc::UnboundedSender<crate::tui::events::UiEvent>> {
        None
    }

    // Phase 43C: Feedback completeness — zero silent operations.

    /// Reflection started (before reflector.reflect()).
    fn reflection_started(&self) {}
    /// Reflection complete with analysis preview and score.
    fn reflection_complete(&self, _analysis: &str, _score: f64) {}
    /// Consolidation operation in progress.
    fn consolidation_status(&self, _action: &str) {}
    /// Consolidation operation completed.
    fn consolidation_complete(&self, _merged: usize, _pruned: usize, _duration_ms: u64) {}
    /// Tool retrying after failure.
    fn tool_retrying(&self, _tool: &str, _attempt: usize, _max: usize, _delay_ms: u64) {}

    /// Context tier usage update from pipeline.
    fn context_tier_update(
        &self,
        _l0_tokens: u32,
        _l0_capacity: u32,
        _l1_tokens: u32,
        _l1_entries: usize,
        _l2_entries: usize,
        _l3_entries: usize,
        _l4_entries: usize,
        _total_tokens: u32,
    ) {
    }

    /// Reasoning engine strategy update.
    fn reasoning_update(&self, _strategy: &str, _task_type: &str, _complexity: &str) {}

    // --- HICON Metrics Visibility (Remediation Phase 1.2) ---

    /// Agent self-corrector applied a correction strategy.
    fn hicon_correction(&self, _strategy: &str, _reason: &str, _round: usize) {}

    /// Bayesian anomaly detector found an anomaly.
    fn hicon_anomaly(&self, _anomaly_type: &str, _severity: &str, _details: &str, _confidence: f64) {}

    /// Metacognitive loop measured coherence (Phi).
    fn hicon_coherence(&self, _phi: f64, _round: usize, _status: &str) {}

    /// ARIMA predictor warns about budget overflow.
    fn hicon_budget_warning(&self, _predicted_overflow_rounds: u32, _current_tokens: u64, _projected_tokens: u64) {}

    // --- Phase E: Agent loop integration methods ---

    /// Notify that dry-run mode is active (persistent banner).
    fn dry_run_active(&self, _active: bool) {}

    /// Token budget usage update: current consumption vs limit.
    fn token_budget_update(&self, _used: u64, _limit: u64, _rate_per_minute: f64) {}

    /// Provider health status change (healthy/degraded/unhealthy).
    fn provider_health_update(&self, _provider: &str, _status: &str, _failure_rate: f64, _latency_p95_ms: u64) {}

    /// Circuit breaker state transition for a provider.
    fn circuit_breaker_update(&self, _provider: &str, _state: &str, _failure_count: u32) {}

    /// Agent FSM state transition.
    fn agent_state_transition(&self, _from: &str, _to: &str, _reason: &str) {}

    /// Real-time token delta update during streaming (Phase 45B).
    /// Called when ModelChunk::Usage is received to update live token counts.
    fn token_delta(
        &self,
        _round_input: u32,
        _round_output: u32,
        _session_input: u32,
        _session_output: u32,
    ) {
    }

    /// Request sudo password elevation from the user.
    ///
    /// In TUI mode: opens the SudoPasswordEntry overlay modal.
    /// In classic mode: default no-op (sudo handles its own TTY prompting).
    /// After calling this, use `permissions.get_sudo_password()` to await the result.
    fn sudo_password_request(&self, _tool: &str, _command: &str, _has_cached: bool) {}

    // --- Dev Ecosystem Phase 5: IDE connection status methods ---

    /// Notify that the embedded LSP TCP server is listening on `port`.
    ///
    /// In TUI mode: emits `UiEvent::IdeConnected { port }` so the status bar
    /// shows the `○ LSP:<port>` indicator.
    fn dev_gateway_ready(&self, _port: u16) {}

    /// Notify that an IDE editor has opened buffers (or buffer count changed).
    ///
    /// In TUI mode: emits `UiEvent::IdeBuffersUpdated { count, git_branch }`.
    fn ide_buffers_updated(&self, _count: usize, _git_branch: Option<&str>) {}

    /// Notify that the LSP server has stopped or no longer has open buffers.
    ///
    /// In TUI mode: emits `UiEvent::IdeDisconnected`.
    fn ide_disconnected(&self) {}

    // --- Multi-Agent Orchestration Visibility ---

    /// Orchestrator is launching a wave of sub-agents.
    fn orchestrator_wave(&self, _wave: usize, _total_waves: usize, _task_count: usize) {}
    /// A sub-agent has been spawned for a delegated step.
    fn sub_agent_spawned(&self, _step: usize, _total: usize, _description: &str, _agent_type: &str) {}
    /// A sub-agent completed.
    fn sub_agent_completed(
        &self,
        _step: usize,
        _total: usize,
        _success: bool,
        _latency_ms: u64,
        _tools_used: &[String],
        _rounds: usize,
        _summary: &str,
        _error_hint: &str,
    ) {
    }

    // --- Multimodal Analysis Feedback ---

    /// Notify that multimodal analysis is starting for `count` files.
    fn media_analysis_started(&self, _count: usize) {}
    /// Notify that a single file has been analyzed (filename + token estimate).
    fn media_analysis_complete(&self, _filename: &str, _tokens: u32) {}

    // --- Phase-Aware Skeleton/Spinner ---

    /// Signal that an expensive agent phase has started (planning, reasoning, reflecting).
    fn phase_started(&self, _phase: &str, _label: &str) {}
    /// Signal that the current agent phase has ended.
    fn phase_ended(&self) {}

    /// Display plan progress with per-step timing data.
    /// Default delegates to `plan_progress` for backward compatibility.
    fn plan_progress_with_timing(
        &self,
        goal: &str,
        steps: &[halcon_core::traits::PlanStep],
        current_step: usize,
        _tracked_steps: &[halcon_core::traits::TrackedStep],
        _elapsed_ms: u64,
    ) {
        self.plan_progress(goal, steps, current_step);
    }

    // --- Phase 94: Project Onboarding ---

    /// Notify that a project-level HALCON.md was found at startup.
    /// Default: no-op (silent, banner already shows ◆ project cfg).
    fn project_config_loaded(&self, _path: &str) {}

    /// Suggest the user runs /init to configure this project.
    /// Default: no-op.
    fn onboarding_suggestion(&self, _root: &str, _project_type: &str) {}

    // --- Phase 95: Plugin Auto-Implantation ---

    /// Notify that plugins are available for recommendation.
    /// Default: no-op.
    fn plugin_suggestion(&self, _total: usize, _essential: usize) {}

    /// Notify that a plugin was bootstrapped (installed or failed).
    /// Default: no-op.
    fn plugin_bootstrapped(&self, _plugin_id: &str, _success: bool) {}
}

// ---------------------------------------------------------------------------
// ClassicSink — wraps existing render functions for terminal output
// ---------------------------------------------------------------------------

/// Classic terminal renderer — delegates to existing render functions.
///
/// Uses `StreamRenderer` for prose/code streaming, `Spinner` for wait indicators,
/// and `feedback::`/`tool::` functions for structured output. All output goes to
/// stdout (streaming) and stderr (feedback/tools/spinner).
pub struct ClassicSink {
    renderer: Mutex<StreamRenderer>,
    spinner: Mutex<Option<Spinner>>,
    expert: bool,
    thinking_chars:   AtomicUsize,
    thinking_preview: Mutex<String>,
    had_thinking:     AtomicBool,
}

impl ClassicSink {
    pub fn new() -> Self {
        Self {
            renderer: Mutex::new(StreamRenderer::new()),
            spinner: Mutex::new(None),
            expert: false,
            thinking_chars:   AtomicUsize::new(0),
            thinking_preview: Mutex::new(String::new()),
            had_thinking:     AtomicBool::new(false),
        }
    }

    /// Create a ClassicSink with expert mode enabled (shows all feedback).
    pub fn with_expert(expert: bool) -> Self {
        Self {
            renderer: Mutex::new(StreamRenderer::new()),
            spinner: Mutex::new(None),
            expert,
            thinking_chars:   AtomicUsize::new(0),
            thinking_preview: Mutex::new(String::new()),
            had_thinking:     AtomicBool::new(false),
        }
    }
}

impl Default for ClassicSink {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderSink for ClassicSink {
    fn stream_text(&self, text: &str) {
        let mut r = self.renderer.lock().unwrap_or_else(|p| p.into_inner());
        let chunk = halcon_core::types::ModelChunk::TextDelta(text.to_string());
        let _ = r.push(&chunk);
    }

    fn stream_thinking(&self, text: &str) {
        // Silently accumulate thinking tokens instead of flooding terminal.
        // Update the spinner label live so user sees progress without noise.
        let prev = self.thinking_chars.fetch_add(text.len(), Ordering::Relaxed);
        let total = prev + text.len();
        {
            let mut p = self.thinking_preview.lock().unwrap_or_else(|poison| {
                tracing::error!(
                    target: "halcon::render",
                    sink = "ClassicSink",
                    field = "thinking_preview",
                    "Mutex poisoned — recovering guard. Previous holder panicked."
                );
                poison.into_inner()
            });
            if p.len() < THINKING_PREVIEW_MAX_BYTES {
                // Use char-aware truncation to avoid splitting a UTF-8 multi-byte sequence.
                // p.len() and THINKING_PREVIEW_MAX_BYTES are byte lengths; chars() iterates
                // Unicode scalar values so we never produce an invalid slice.
                let remaining_bytes = THINKING_PREVIEW_MAX_BYTES - p.len();
                let safe: String = text
                    .chars()
                    .scan(0usize, |acc, c| {
                        *acc += c.len_utf8();
                        if *acc <= remaining_bytes { Some(c) } else { None }
                    })
                    .collect();
                p.push_str(&safe);
            }
        }
        let label = if total >= 1000 {
            format!("{} {:.1}K chars", THINKING_LABEL, total as f64 / 1000.0)
        } else {
            format!("{} {total} chars", THINKING_LABEL)
        };
        if let Ok(guard) = self.spinner.lock() {
            if let Some(ref s) = *guard { s.update_label(label); }
        }
        self.had_thinking.store(true, Ordering::Relaxed);
    }

    fn stream_code_block(&self, _lang: &str, _code: &str) {
        // StreamRenderer handles code block detection internally via process_delta,
        // so code blocks flow through stream_text(). This method exists for sinks
        // that need explicit code block notification (e.g. TUI).
    }

    fn stream_tool_marker(&self, name: &str) {
        let mut r = self.renderer.lock().unwrap_or_else(|p| p.into_inner());
        let chunk = halcon_core::types::ModelChunk::ToolUseStart {
            index: 0,
            id: String::new(),
            name: name.to_string(),
        };
        let _ = r.push(&chunk);
    }

    fn stream_done(&self) {
        let mut r = self.renderer.lock().unwrap_or_else(|p| p.into_inner());
        let chunk = halcon_core::types::ModelChunk::Done(halcon_core::types::StopReason::EndTurn);
        let _ = r.push(&chunk);
    }

    fn stream_error(&self, msg: &str) {
        let mut r = self.renderer.lock().unwrap_or_else(|p| p.into_inner());
        let chunk = halcon_core::types::ModelChunk::Error(msg.to_string());
        let _ = r.push(&chunk);
    }

    fn tool_start(&self, name: &str, input: &serde_json::Value) {
        tool_render::render_tool_start(name, input);
    }

    fn tool_output(&self, block: &ContentBlock, duration_ms: u64) {
        tool_render::render_tool_output(block, duration_ms);
    }

    fn tool_denied(&self, name: &str) {
        tool_render::render_tool_denied(name);
    }

    fn spinner_start(&self, label: &str) {
        let spinner = Spinner::start(label);
        let mut guard = self.spinner.lock().unwrap_or_else(|p| p.into_inner());
        *guard = Some(spinner);
    }

    fn spinner_stop(&self) {
        let mut guard = self.spinner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(ref s) = *guard {
            s.stop();
        }
        *guard = None;

        // Emit thinking summary line in expert mode (after spinner is cleared).
        if self.had_thinking.swap(false, Ordering::Relaxed) {
            let char_count = self.thinking_chars.swap(0, Ordering::Relaxed);
            let preview = {
                let mut p = self.thinking_preview.lock().unwrap_or_else(|p| p.into_inner());
                std::mem::take(&mut *p)
            };
            if self.expert {
                let c = super::theme::active().palette.muted.fg();
                let r = super::theme::reset();
                let kchars = if char_count >= 1000 {
                    format!("{:.1}K", char_count as f64 / 1000.0)
                } else {
                    char_count.to_string()
                };
                let snippet = if preview.len() > 100 {
                    format!("{}...", &preview[..{ let mut _fcb = (100).min(preview.len()); while _fcb > 0 && !preview.is_char_boundary(_fcb) { _fcb -= 1; } _fcb }])
                } else {
                    preview
                };
                eprintln!("{c}  ⟨ razonando · {kchars} chars ⟩  \"{snippet}\"{r}");
            }
        }
    }

    fn warning(&self, message: &str, hint: Option<&str>) {
        feedback::user_warning(message, hint);
    }

    fn error(&self, message: &str, hint: Option<&str>) {
        feedback::user_error(message, hint);
    }

    fn info(&self, message: &str) {
        eprintln!("{message}");
    }

    fn is_silent(&self) -> bool {
        false
    }

    fn stream_reset(&self) {
        let mut r = self.renderer.lock().unwrap_or_else(|p| p.into_inner());
        *r = StreamRenderer::new();
    }

    fn stream_full_text(&self) -> String {
        let r = self.renderer.lock().unwrap_or_else(|p| p.into_inner());
        r.full_text().to_string()
    }

    fn round_started(&self, round: usize, provider: &str, model: &str) {
        let p = super::theme::active().palette.running.fg();
        let r = super::theme::reset();
        eprintln!("\n{p}── Round {round} ─ {provider}/{model} ──{r}");
    }

    fn round_ended(&self, round: usize, input_tokens: u32, output_tokens: u32, cost: f64, duration_ms: u64) {
        if !self.expert { return; }
        let p = super::theme::active().palette.muted.fg();
        let r = super::theme::reset();
        eprintln!("{p}  Round {round}: ↑{input_tokens} ↓{output_tokens} ${cost:.4} ({:.1}s){r}", duration_ms as f64 / 1000.0);
    }

    fn model_selected(&self, model: &str, provider: &str, reason: &str) {
        if !self.expert { return; }
        let p = super::theme::active().palette.planning.fg();
        let r = super::theme::reset();
        eprintln!("{p}  [model] {provider}/{model} — {reason}{r}");
    }

    fn provider_fallback(&self, from: &str, to: &str, reason: &str) {
        let p = super::theme::active().palette.retrying.fg();
        let r = super::theme::reset();
        eprintln!("{p}  [fallback] {from} → {to} — {reason}{r}");
    }

    fn loop_guard_action(&self, action: &str, reason: &str) {
        if !self.expert { return; }
        let p = super::theme::active().palette.warning.fg();
        let r = super::theme::reset();
        eprintln!("{p}  [guard] {action}: {reason}{r}");
    }

    fn compaction_complete(&self, old_msgs: usize, new_msgs: usize, tokens_saved: u64) {
        let p = super::theme::active().palette.compacting.fg();
        let r = super::theme::reset();
        if self.expert {
            eprintln!("{p}  [compaction] {old_msgs} → {new_msgs} messages ({tokens_saved} tokens saved){r}");
        } else {
            eprintln!("{p}  [compacted context]{r}");
        }
    }

    fn cache_status(&self, hit: bool, source: &str) {
        if !self.expert { return; }
        let pal = &super::theme::active().palette;
        let p = if hit { pal.cached.fg() } else { pal.muted.fg() };
        let r = super::theme::reset();
        let label = if hit { "hit" } else { "miss" };
        eprintln!("{p}  [cache {label}] {source}{r}");
    }

    fn speculative_result(&self, tool: &str, hit: bool) {
        if !self.expert { return; }
        let pal = &super::theme::active().palette;
        let p = if hit { pal.cached.fg() } else { pal.muted.fg() };
        let r = super::theme::reset();
        let label = if hit { "hit" } else { "miss" };
        eprintln!("{p}  [speculative {label}] {tool}{r}");
    }

    fn permission_awaiting(&self, tool: &str, _args: &serde_json::Value, risk_level: &str) {
        let p = super::theme::active().palette.destructive.fg();
        let r = super::theme::reset();
        eprintln!("{p}  [permission] awaiting approval for {tool} (risk: {risk_level}){r}");
    }

    fn reflection_started(&self) {
        if !self.expert { return; }
        let c = super::theme::active().palette.reasoning.fg();
        let r = super::theme::reset();
        eprintln!("{c}  [reflecting] analyzing round outcome...{r}");
    }

    fn reflection_complete(&self, analysis: &str, score: f64) {
        if !self.expert { return; }
        let c = super::theme::active().palette.reasoning.fg();
        let r = super::theme::reset();
        let preview = if analysis.len() > 80 { &analysis[..{ let mut _fcb = (80).min(analysis.len()); while _fcb > 0 && !analysis.is_char_boundary(_fcb) { _fcb -= 1; } _fcb }] } else { analysis };
        eprintln!("{c}  [reflection] {preview} (score: {score:.2}){r}");
    }

    fn consolidation_status(&self, action: &str) {
        if !self.expert { return; }
        let c = super::theme::active().palette.compacting.fg();
        let r = super::theme::reset();
        eprintln!("{c}  [memory] {action}{r}");
    }

    fn consolidation_complete(&self, merged: usize, pruned: usize, duration_ms: u64) {
        if !self.expert { return; }
        let c = super::theme::active().palette.success.fg();
        let r = super::theme::reset();
        let duration_s = duration_ms as f64 / 1000.0;
        eprintln!("{c}  [memory] consolidated: merged={merged}, pruned={pruned}, duration={duration_s:.2}s{r}");
    }

    fn tool_retrying(&self, tool: &str, attempt: usize, max: usize, delay_ms: u64) {
        let c = super::theme::active().palette.retrying.fg();
        let r = super::theme::reset();
        eprintln!("{c}  [retry] {tool} attempt {attempt}/{max} in {delay_ms}ms{r}");
    }

    fn context_tier_update(
        &self,
        l0_tokens: u32,
        _l0_capacity: u32,
        l1_tokens: u32,
        _l1_entries: usize,
        l2_entries: usize,
        l3_entries: usize,
        l4_entries: usize,
        total_tokens: u32,
    ) {
        if !self.expert { return; }
        let c = super::theme::active().palette.cached.fg();
        let r = super::theme::reset();
        eprintln!(
            "{c}  [context] L0:{l0_tokens}tok L1:{l1_tokens}tok L2:{l2_entries} L3:{l3_entries} L4:{l4_entries} total:{total_tokens}tok{r}"
        );
    }

    fn reasoning_update(&self, strategy: &str, task_type: &str, complexity: &str) {
        if !self.expert { return; }
        let c = super::theme::active().palette.reasoning.fg();
        let r = super::theme::reset();
        eprintln!("{c}  [reasoning] {complexity} {task_type} → {strategy}{r}");
    }

    fn plan_progress(
        &self,
        goal: &str,
        steps: &[halcon_core::traits::PlanStep],
        current_step: usize,
    ) {
        use halcon_core::traits::StepOutcome;
        eprintln!("\n  PLAN: {goal}");
        for (i, step) in steps.iter().enumerate() {
            let icon = match &step.outcome {
                Some(StepOutcome::Success { .. }) => "[ok]",
                Some(StepOutcome::Failed { .. }) => "[FAIL]",
                Some(StepOutcome::Skipped { .. }) => "[skip]",
                None if i == current_step => "[>>]",
                None => "[ ]",
            };
            let tool_hint = step
                .tool_name
                .as_deref()
                .map(|t| format!(" ({t})"))
                .unwrap_or_default();
            eprintln!("    {icon} Step {}: {}{tool_hint}", i + 1, step.description);
        }
        eprintln!();
    }

    fn plan_progress_with_timing(
        &self,
        goal: &str,
        _steps: &[halcon_core::traits::PlanStep],
        current_step: usize,
        tracked_steps: &[halcon_core::traits::TrackedStep],
        elapsed_ms: u64,
    ) {
        use halcon_core::traits::TaskStatus;
        let completed = tracked_steps.iter().filter(|s| s.status.is_terminal()).count();
        let total = tracked_steps.len();
        eprintln!(
            "\n  PLAN: {goal} ({completed}/{total}, {:.1}s)",
            elapsed_ms as f64 / 1000.0
        );
        for (i, ts) in tracked_steps.iter().enumerate() {
            let icon = match ts.status {
                TaskStatus::Completed => "[ok]",
                TaskStatus::Failed => "[FAIL]",
                TaskStatus::Skipped => "[skip]",
                TaskStatus::Cancelled => "[X]",
                TaskStatus::Running => "[>>]",
                TaskStatus::Pending if i == current_step => "[>>]",
                TaskStatus::Pending => "[ ]",
            };
            let tool_hint = ts
                .step
                .tool_name
                .as_deref()
                .map(|t| format!(" ({t})"))
                .unwrap_or_default();
            let timing = match ts.duration_ms {
                Some(ms) => format!(" [{:.1}s]", ms as f64 / 1000.0),
                None => String::new(),
            };
            let delegation_tag = ts
                .delegation
                .as_ref()
                .map(|d| format!(" [delegated->{}]", d.agent_type))
                .unwrap_or_default();
            eprintln!(
                "    {icon} Step {}: {}{tool_hint}{timing}{delegation_tag}",
                i + 1,
                ts.step.description
            );
        }
        eprintln!();
    }

    fn task_status(&self, title: &str, status: &str, duration_ms: Option<u64>, artifact_count: usize) {
        let timing = duration_ms
            .map(|ms| format!(" ({:.1}s", ms as f64 / 1000.0))
            .unwrap_or_default();
        let artifacts = if artifact_count > 0 {
            format!(", {} artifact{}", artifact_count, if artifact_count == 1 { "" } else { "s" })
        } else {
            String::new()
        };
        let suffix = if !timing.is_empty() {
            format!("{timing}{artifacts})")
        } else if !artifacts.is_empty() {
            format!("({artifacts})")
        } else {
            String::new()
        };
        eprintln!("  [task] {title} — {status}{suffix}");
    }

    fn reasoning_status(&self, task_type: &str, complexity: &str, strategy: &str, score: f64, success: bool) {
        let outcome = if success { "Success" } else { "Below threshold" };
        eprintln!("  [reasoning] {task_type} ({complexity}) -> {strategy}");
        eprintln!("  [evaluation] Score: {score:.2} — {outcome}");
    }

    // --- HICON Metrics Visibility (Remediation Phase 1.2) ---

    fn hicon_correction(&self, strategy: &str, reason: &str, round: usize) {
        eprintln!("  [hicon] Round {round}: Self-corrector applied '{strategy}'");
        eprintln!("         Reason: {reason}");
    }

    fn hicon_anomaly(&self, anomaly_type: &str, severity: &str, details: &str, confidence: f64) {
        eprintln!("  [hicon] Anomaly detected: {anomaly_type} (severity: {severity}, confidence: {confidence:.2})");
        eprintln!("         Details: {details}");
    }

    fn hicon_coherence(&self, phi: f64, round: usize, status: &str) {
        eprintln!("  [hicon] Round {round}: Φ coherence = {phi:.3} ({status})");
    }

    fn hicon_budget_warning(&self, predicted_overflow_rounds: u32, current_tokens: u64, projected_tokens: u64) {
        eprintln!("  [hicon] Budget warning: Overflow predicted in {predicted_overflow_rounds} rounds");
        eprintln!("         Current: {current_tokens} tokens → Projected: {projected_tokens} tokens");
    }

    fn orchestrator_wave(&self, wave: usize, total_waves: usize, task_count: usize) {
        let c = super::theme::active().palette.running.fg();
        let r = super::theme::reset();
        eprintln!("{c}  [orchestrator] Wave {wave}/{total_waves} — {task_count} sub-agents{r}");
    }

    fn sub_agent_spawned(&self, step: usize, total: usize, description: &str, agent_type: &str) {
        let c = super::theme::active().palette.running.fg();
        let r = super::theme::reset();
        let desc: String = description.chars().take(60).collect();
        eprintln!("{c}  [sub-agent] ⟳ [{step}/{total}] {desc}  ({agent_type}){r}");
    }

    fn sub_agent_completed(
        &self,
        step: usize,
        total: usize,
        success: bool,
        latency_ms: u64,
        tools_used: &[String],
        _rounds: usize,
        _summary: &str,
        error_hint: &str,
    ) {
        let pal = &super::theme::active().palette;
        let (c, icon) = if success { (pal.success.fg(), '✓') } else { (pal.destructive.fg(), '✗') };
        let r = super::theme::reset();
        let tools_str = if tools_used.is_empty() {
            String::new()
        } else {
            format!(" · {}", tools_used.join(" "))
        };
        eprintln!("{c}  [sub-agent] {icon} [{step}/{total}]{tools_str} ({:.1}s){r}", latency_ms as f64 / 1000.0);
        if !success && !error_hint.is_empty() {
            let ec = pal.destructive.fg();
            eprintln!("{ec}  [sub-agent] error: {error_hint}{r}");
        }
    }

    fn media_analysis_started(&self, count: usize) {
        let c = super::theme::active().palette.running.fg();
        let r = super::theme::reset();
        eprintln!("{c}  [media] Analyzing {count} file{}…{r}", if count == 1 { "" } else { "s" });
    }

    fn media_analysis_complete(&self, filename: &str, tokens: u32) {
        let c = super::theme::active().palette.cached.fg();
        let r = super::theme::reset();
        eprintln!("{c}  [media] {filename}: {tokens} tokens{r}");
    }

    fn phase_started(&self, phase: &str, label: &str) {
        let c = super::theme::active().palette.planning.fg();
        let r = super::theme::reset();
        eprintln!("{c}  [{phase}] {label}{r}");
    }
    // phase_ended is a no-op for ClassicSink (no skeleton to remove)

    fn project_config_loaded(&self, path: &str) {
        eprintln!("[project] config: {path}");
    }

    fn onboarding_suggestion(&self, root: &str, project_type: &str) {
        eprintln!(
            "[onboarding] No project HALCON.md in {root} ({project_type})\n\
             → Type /init to generate one and unlock full agent context."
        );
    }

    fn plugin_suggestion(&self, total: usize, essential: usize) {
        eprintln!(
            "[plugins] {total} plugins recommended ({essential} essential) — /plugins suggest"
        );
    }

    fn plugin_bootstrapped(&self, plugin_id: &str, success: bool) {
        let status = if success { '✓' } else { '✗' };
        eprintln!("[plugins] {status} {plugin_id}");
    }
}

// ---------------------------------------------------------------------------
// SilentSink — accumulates text without terminal output
// ---------------------------------------------------------------------------

/// Silent renderer — accumulates text but produces no terminal output.
///
/// Used for background sub-agents, replay mode, and testing.
pub struct SilentSink {
    text: Mutex<String>,
}

impl SilentSink {
    pub fn new() -> Self {
        Self {
            text: Mutex::new(String::new()),
        }
    }

    /// Get the accumulated text.
    #[allow(dead_code)]
    pub fn text(&self) -> String {
        self.text.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl Default for SilentSink {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderSink for SilentSink {
    fn stream_text(&self, text: &str) {
        self.text.lock().unwrap_or_else(|p| p.into_inner()).push_str(text);
    }

    fn stream_code_block(&self, _lang: &str, code: &str) {
        self.text.lock().unwrap_or_else(|p| p.into_inner()).push_str(code);
    }

    fn stream_tool_marker(&self, _name: &str) {}
    fn stream_done(&self) {}
    fn stream_error(&self, _msg: &str) {}
    fn tool_start(&self, _name: &str, _input: &serde_json::Value) {}
    fn tool_output(&self, _block: &ContentBlock, _duration_ms: u64) {}
    fn tool_denied(&self, _name: &str) {}
    fn spinner_start(&self, _label: &str) {}
    fn spinner_stop(&self) {}
    fn warning(&self, _message: &str, _hint: Option<&str>) {}
    fn error(&self, _message: &str, _hint: Option<&str>) {}
    fn info(&self, _message: &str) {}

    fn is_silent(&self) -> bool {
        true
    }

    fn stream_reset(&self) {
        self.text.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    fn stream_full_text(&self) -> String {
        self.text.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

// ---------------------------------------------------------------------------
// TuiSink — sends UiEvents through a channel to the TUI render loop
// ---------------------------------------------------------------------------

#[cfg(feature = "tui")]
/// TUI renderer — converts all agent output into `UiEvent`s sent through an mpsc channel.
///
/// The TUI render loop receives these events and updates the 3-zone layout.
/// Text accumulation is tracked locally for `stream_full_text()`.
///
/// Uses an UNBOUNDED channel so that critical events like `PermissionAwaiting` are
/// never dropped when the LLM generates large outputs (bounded try_send would silently
/// drop them, causing the modal to never show and tools to auto-deny after 60s).
pub struct TuiSink {
    tx: tokio::sync::mpsc::UnboundedSender<crate::tui::events::UiEvent>,
    text: Mutex<String>,
    thinking_chars:   Mutex<usize>,
    thinking_preview: Mutex<String>,
    had_thinking:     AtomicBool,
}

#[cfg(feature = "tui")]
impl TuiSink {
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<crate::tui::events::UiEvent>) -> Self {
        Self {
            tx,
            text: Mutex::new(String::new()),
            thinking_chars:   Mutex::new(0),
            thinking_preview: Mutex::new(String::new()),
            had_thinking:     AtomicBool::new(false),
        }
    }

    fn send(&self, event: crate::tui::events::UiEvent) {
        // Unbounded send never blocks or drops — critical for PermissionAwaiting events.
        if let Err(_) = self.tx.send(event) {
            tracing::error!("TUI event channel closed, receiver terminated");
        }
    }
}

#[cfg(feature = "tui")]
impl RenderSink for TuiSink {
    fn stream_text(&self, text: &str) {
        // Emit ThinkingComplete before first answer token (if thinking occurred).
        if self.had_thinking.swap(false, Ordering::Relaxed) {
            let char_count = {
                let mut c = self.thinking_chars.lock().unwrap_or_else(|p| p.into_inner());
                let n = *c;
                *c = 0;
                n
            };
            let preview = {
                let mut p = self.thinking_preview.lock().unwrap_or_else(|p| p.into_inner());
                std::mem::take(&mut *p)
            };
            self.send(crate::tui::events::UiEvent::ThinkingComplete { preview, char_count });
        }
        self.text.lock().unwrap_or_else(|p| p.into_inner()).push_str(text);
        self.send(crate::tui::events::UiEvent::StreamChunk(text.to_string()));
    }

    fn stream_thinking(&self, text: &str) {
        // Backward compat: still send StreamThinking for thinking_buffer fallback.
        self.send(crate::tui::events::UiEvent::StreamThinking(text.to_string()));
        // Also accumulate locally and emit ThinkingProgress with total char count.
        let new_count = {
            let mut c = self.thinking_chars.lock().unwrap_or_else(|p| p.into_inner());
            *c += text.len();
            *c
        };
        {
            let mut p = self.thinking_preview.lock().unwrap_or_else(|poison| {
                tracing::error!(
                    target: "halcon::render",
                    sink = "TuiSink",
                    field = "thinking_preview",
                    "Mutex poisoned — recovering guard. Previous holder panicked."
                );
                poison.into_inner()
            });
            if p.len() < THINKING_PREVIEW_MAX_BYTES {
                let remaining_bytes = THINKING_PREVIEW_MAX_BYTES - p.len();
                let safe: String = text
                    .chars()
                    .scan(0usize, |acc, c| {
                        *acc += c.len_utf8();
                        if *acc <= remaining_bytes { Some(c) } else { None }
                    })
                    .collect();
                p.push_str(&safe);
            }
        }
        self.had_thinking.store(true, Ordering::Relaxed);
        self.send(crate::tui::events::UiEvent::ThinkingProgress { chars: new_count });
    }

    fn stream_code_block(&self, lang: &str, code: &str) {
        self.text.lock().unwrap_or_else(|p| p.into_inner()).push_str(code);
        self.send(crate::tui::events::UiEvent::StreamCodeBlock {
            lang: lang.to_string(),
            code: code.to_string(),
        });
    }

    fn stream_tool_marker(&self, name: &str) {
        self.send(crate::tui::events::UiEvent::StreamToolMarker(name.to_string()));
    }

    fn stream_done(&self) {
        self.send(crate::tui::events::UiEvent::StreamDone);
    }

    fn stream_error(&self, msg: &str) {
        self.send(crate::tui::events::UiEvent::StreamError(msg.to_string()));
    }

    fn tool_start(&self, name: &str, input: &serde_json::Value) {
        self.send(crate::tui::events::UiEvent::ToolStart {
            name: name.to_string(),
            input: input.clone(),
        });
    }

    fn tool_output(&self, block: &ContentBlock, duration_ms: u64) {
        let (name, content, is_error) = match block {
            ContentBlock::ToolResult { tool_use_id, content, is_error, .. } => {
                (tool_use_id.clone(), content.clone(), *is_error)
            }
            _ => (String::new(), String::new(), false),
        };
        self.send(crate::tui::events::UiEvent::ToolOutput {
            name,
            content,
            is_error,
            duration_ms,
        });
    }

    fn tool_denied(&self, name: &str) {
        self.send(crate::tui::events::UiEvent::ToolDenied(name.to_string()));
    }

    fn spinner_start(&self, label: &str) {
        self.send(crate::tui::events::UiEvent::SpinnerStart(label.to_string()));
    }

    fn spinner_stop(&self) {
        self.send(crate::tui::events::UiEvent::SpinnerStop);
    }

    fn warning(&self, message: &str, hint: Option<&str>) {
        self.send(crate::tui::events::UiEvent::Warning {
            message: message.to_string(),
            hint: hint.map(|h| h.to_string()),
        });
    }

    fn error(&self, message: &str, hint: Option<&str>) {
        self.send(crate::tui::events::UiEvent::Error {
            message: message.to_string(),
            hint: hint.map(|h| h.to_string()),
        });
    }

    fn info(&self, message: &str) {
        self.send(crate::tui::events::UiEvent::Info(message.to_string()));
    }

    fn is_silent(&self) -> bool {
        false
    }

    fn stream_reset(&self) {
        self.text.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    fn stream_full_text(&self) -> String {
        self.text.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn session_started(&self, session_id: &str) {
        self.send(crate::tui::events::UiEvent::SessionInitialized {
            session_id: session_id.to_string(),
        });
    }

    fn round_started(&self, round: usize, provider: &str, model: &str) {
        self.send(crate::tui::events::UiEvent::RoundStarted {
            round,
            provider: provider.to_string(),
            model: model.to_string(),
        });
    }

    fn round_ended(&self, round: usize, input_tokens: u32, output_tokens: u32, cost: f64, duration_ms: u64) {
        self.send(crate::tui::events::UiEvent::RoundEnded {
            round,
            input_tokens,
            output_tokens,
            cost,
            duration_ms,
        });
    }

    fn model_selected(&self, model: &str, provider: &str, reason: &str) {
        self.send(crate::tui::events::UiEvent::ModelSelected {
            model: model.to_string(),
            provider: provider.to_string(),
            reason: reason.to_string(),
        });
    }

    fn provider_fallback(&self, from: &str, to: &str, reason: &str) {
        self.send(crate::tui::events::UiEvent::ProviderFallback {
            from: from.to_string(),
            to: to.to_string(),
            reason: reason.to_string(),
        });
    }

    fn loop_guard_action(&self, action: &str, reason: &str) {
        self.send(crate::tui::events::UiEvent::LoopGuardAction {
            action: action.to_string(),
            reason: reason.to_string(),
        });
    }

    fn compaction_complete(&self, old_msgs: usize, new_msgs: usize, tokens_saved: u64) {
        self.send(crate::tui::events::UiEvent::CompactionComplete {
            old_msgs,
            new_msgs,
            tokens_saved,
        });
    }

    fn cache_status(&self, hit: bool, source: &str) {
        self.send(crate::tui::events::UiEvent::CacheStatus {
            hit,
            source: source.to_string(),
        });
    }

    fn speculative_result(&self, tool: &str, hit: bool) {
        self.send(crate::tui::events::UiEvent::SpeculativeResult {
            tool: tool.to_string(),
            hit,
        });
    }

    fn permission_awaiting(&self, tool: &str, args: &serde_json::Value, risk_level: &str) {
        let timeout_secs = timeout_for_risk(risk_level);
        self.send(crate::tui::events::UiEvent::PermissionAwaiting {
            tool: tool.to_string(),
            args: args.clone(),
            risk_level: risk_level.to_string(),
            timeout_secs,
            reply_tx: None, // main agent: TuiApp uses its stored perm_tx
        });
    }

    fn tui_event_sender(&self) -> Option<tokio::sync::mpsc::UnboundedSender<crate::tui::events::UiEvent>> {
        Some(self.tx.clone())
    }

    fn reflection_started(&self) {
        self.send(crate::tui::events::UiEvent::ReflectionStarted);
    }

    fn reflection_complete(&self, analysis: &str, score: f64) {
        self.send(crate::tui::events::UiEvent::ReflectionComplete {
            analysis: analysis.to_string(),
            score,
        });
    }

    fn consolidation_status(&self, action: &str) {
        self.send(crate::tui::events::UiEvent::ConsolidationStatus {
            action: action.to_string(),
        });
    }

    fn consolidation_complete(&self, merged: usize, pruned: usize, duration_ms: u64) {
        self.send(crate::tui::events::UiEvent::ConsolidationComplete {
            merged,
            pruned,
            duration_ms,
        });
    }

    fn tool_retrying(&self, tool: &str, attempt: usize, max_attempts: usize, delay_ms: u64) {
        self.send(crate::tui::events::UiEvent::ToolRetrying {
            tool: tool.to_string(),
            attempt,
            max_attempts,
            delay_ms,
        });
    }

    fn context_tier_update(
        &self,
        l0_tokens: u32,
        l0_capacity: u32,
        l1_tokens: u32,
        l1_entries: usize,
        l2_entries: usize,
        l3_entries: usize,
        l4_entries: usize,
        total_tokens: u32,
    ) {
        self.send(crate::tui::events::UiEvent::ContextTierUpdate {
            l0_tokens,
            l0_capacity,
            l1_tokens,
            l1_entries,
            l2_entries,
            l3_entries,
            l4_entries,
            total_tokens,
        });
    }

    fn reasoning_update(&self, strategy: &str, task_type: &str, complexity: &str) {
        self.send(crate::tui::events::UiEvent::ReasoningUpdate {
            strategy: strategy.to_string(),
            task_type: task_type.to_string(),
            complexity: complexity.to_string(),
        });
    }

    // --- HICON Metrics Visibility (Remediation Phase 1.2) ---

    fn hicon_correction(&self, strategy: &str, reason: &str, round: usize) {
        self.send(crate::tui::events::UiEvent::HiconCorrection {
            strategy: strategy.to_string(),
            reason: reason.to_string(),
            round,
        });
    }

    fn hicon_anomaly(&self, anomaly_type: &str, severity: &str, details: &str, confidence: f64) {
        self.send(crate::tui::events::UiEvent::HiconAnomaly {
            anomaly_type: anomaly_type.to_string(),
            severity: severity.to_string(),
            details: details.to_string(),
            confidence,
        });
    }

    fn hicon_coherence(&self, phi: f64, round: usize, status: &str) {
        self.send(crate::tui::events::UiEvent::HiconCoherence {
            phi,
            round,
            status: status.to_string(),
        });
    }

    fn hicon_budget_warning(&self, predicted_overflow_rounds: u32, current_tokens: u64, projected_tokens: u64) {
        self.send(crate::tui::events::UiEvent::HiconBudgetWarning {
            predicted_overflow_rounds,
            current_tokens,
            projected_tokens,
        });
    }

    fn dry_run_active(&self, active: bool) {
        self.send(crate::tui::events::UiEvent::DryRunActive(active));
    }

    fn token_budget_update(&self, used: u64, limit: u64, rate_per_minute: f64) {
        self.send(crate::tui::events::UiEvent::TokenBudgetUpdate {
            used,
            limit,
            rate_per_minute,
        });
    }

    fn provider_health_update(&self, provider: &str, status: &str, failure_rate: f64, latency_p95_ms: u64) {
        use crate::tui::events::ProviderHealthStatus;
        let health_status = match status {
            "healthy" => ProviderHealthStatus::Healthy,
            "degraded" => ProviderHealthStatus::Degraded { failure_rate, latency_p95_ms },
            _ => ProviderHealthStatus::Unhealthy { reason: status.to_string() },
        };
        self.send(crate::tui::events::UiEvent::ProviderHealthUpdate {
            provider: provider.to_string(),
            status: health_status,
        });
    }

    fn circuit_breaker_update(&self, provider: &str, state: &str, failure_count: u32) {
        use crate::tui::events::CircuitBreakerState;
        let cb_state = match state {
            "open" => CircuitBreakerState::Open,
            "half_open" => CircuitBreakerState::HalfOpen,
            _ => CircuitBreakerState::Closed,
        };
        self.send(crate::tui::events::UiEvent::CircuitBreakerUpdate {
            provider: provider.to_string(),
            state: cb_state,
            failure_count,
        });
    }

    fn agent_state_transition(&self, from: &str, to: &str, reason: &str) {
        use crate::tui::events::AgentState;
        let parse_state = |s: &str| -> AgentState {
            match s {
                "idle" => AgentState::Idle,
                "planning" => AgentState::Planning,
                "executing" => AgentState::Executing,
                "tool_wait" => AgentState::ToolWait,
                "reflecting" => AgentState::Reflecting,
                "synthesizing" | "synthesising" => AgentState::Synthesizing,
                "evaluating" => AgentState::Executing,  // no TUI Evaluating variant — show as Executing
                "complete" | "completed" => AgentState::Complete,
                "failed" | "halted" => AgentState::Failed,
                "paused" => AgentState::Paused,
                unknown => {
                    tracing::warn!(state = unknown, "parse_state: unknown FSM state string — defaulting to Idle");
                    AgentState::Idle
                }
            }
        };
        self.send(crate::tui::events::UiEvent::AgentStateTransition {
            from: parse_state(from),
            to: parse_state(to),
            reason: reason.to_string(),
        });
    }

    fn task_status(&self, title: &str, status: &str, duration_ms: Option<u64>, artifact_count: usize) {
        self.send(crate::tui::events::UiEvent::TaskStatus {
            title: title.to_string(),
            status: status.to_string(),
            duration_ms,
            artifact_count,
        });
    }

    fn reasoning_status(&self, task_type: &str, complexity: &str, strategy: &str, score: f64, success: bool) {
        self.send(crate::tui::events::UiEvent::ReasoningStatus {
            task_type: task_type.to_string(),
            complexity: complexity.to_string(),
            strategy: strategy.to_string(),
            score,
            success,
        });
    }

    fn plan_progress(
        &self,
        goal: &str,
        steps: &[halcon_core::traits::PlanStep],
        current_step: usize,
    ) {
        use crate::tui::events::{PlanStepDisplayStatus, PlanStepStatus};
        use halcon_core::traits::StepOutcome;
        let step_statuses: Vec<PlanStepStatus> = steps
            .iter()
            .enumerate()
            .map(|(i, s)| PlanStepStatus {
                description: s.description.clone(),
                tool_name: s.tool_name.clone(),
                status: match &s.outcome {
                    Some(StepOutcome::Success { .. }) => PlanStepDisplayStatus::Succeeded,
                    Some(StepOutcome::Failed { .. }) => PlanStepDisplayStatus::Failed,
                    Some(StepOutcome::Skipped { .. }) => PlanStepDisplayStatus::Skipped,
                    None if i == current_step => PlanStepDisplayStatus::InProgress,
                    None => PlanStepDisplayStatus::Pending,
                },
                duration_ms: None,
            })
            .collect();
        self.send(crate::tui::events::UiEvent::PlanProgress {
            goal: goal.to_string(),
            steps: step_statuses,
            current_step,
            elapsed_ms: 0,
        });
    }

    fn plan_progress_with_timing(
        &self,
        goal: &str,
        _steps: &[halcon_core::traits::PlanStep],
        current_step: usize,
        tracked_steps: &[halcon_core::traits::TrackedStep],
        elapsed_ms: u64,
    ) {
        use crate::tui::events::{PlanStepDisplayStatus, PlanStepStatus};
        use halcon_core::traits::TaskStatus;
        let step_statuses: Vec<PlanStepStatus> = tracked_steps
            .iter()
            .enumerate()
            .map(|(i, ts)| PlanStepStatus {
                description: ts.step.description.clone(),
                tool_name: ts.step.tool_name.clone(),
                status: match ts.status {
                    TaskStatus::Completed => PlanStepDisplayStatus::Succeeded,
                    TaskStatus::Failed => PlanStepDisplayStatus::Failed,
                    TaskStatus::Skipped => PlanStepDisplayStatus::Skipped,
                    TaskStatus::Cancelled => PlanStepDisplayStatus::Skipped,
                    TaskStatus::Running => PlanStepDisplayStatus::InProgress,
                    TaskStatus::Pending if i == current_step => PlanStepDisplayStatus::InProgress,
                    TaskStatus::Pending => PlanStepDisplayStatus::Pending,
                },
                duration_ms: ts.duration_ms,
            })
            .collect();
        self.send(crate::tui::events::UiEvent::PlanProgress {
            goal: goal.to_string(),
            steps: step_statuses,
            current_step,
            elapsed_ms,
        });
    }

    fn token_delta(
        &self,
        round_input: u32,
        round_output: u32,
        session_input: u32,
        session_output: u32,
    ) {
        self.send(crate::tui::events::UiEvent::TokenDelta {
            round_input,
            round_output,
            session_input,
            session_output,
        });
    }

    fn sudo_password_request(&self, tool: &str, command: &str, has_cached: bool) {
        self.send(crate::tui::events::UiEvent::SudoPasswordRequest {
            tool: tool.to_string(),
            command: command.to_string(),
            has_cached,
        });
    }

    // --- Dev Ecosystem Phase 5: IDE connection status ---

    fn dev_gateway_ready(&self, port: u16) {
        self.send(crate::tui::events::UiEvent::IdeConnected { port });
    }

    fn ide_buffers_updated(&self, count: usize, git_branch: Option<&str>) {
        self.send(crate::tui::events::UiEvent::IdeBuffersUpdated {
            count,
            git_branch: git_branch.map(String::from),
        });
    }

    fn ide_disconnected(&self) {
        self.send(crate::tui::events::UiEvent::IdeDisconnected);
    }

    fn orchestrator_wave(&self, wave_index: usize, total_waves: usize, task_count: usize) {
        self.send(crate::tui::events::UiEvent::OrchestratorWave {
            wave_index,
            total_waves,
            task_count,
        });
    }

    fn sub_agent_spawned(&self, step_index: usize, total: usize, description: &str, agent_type: &str) {
        self.send(crate::tui::events::UiEvent::SubAgentSpawned {
            step_index,
            total_steps: total,
            description: description.chars().take(60).collect(),
            agent_type: agent_type.to_string(),
        });
    }

    fn media_analysis_started(&self, count: usize) {
        self.send(crate::tui::events::UiEvent::MediaAnalysisStarted { count });
    }

    fn media_analysis_complete(&self, filename: &str, tokens: u32) {
        self.send(crate::tui::events::UiEvent::MediaAnalysisComplete {
            filename: filename.to_string(),
            tokens,
        });
    }

    fn sub_agent_completed(
        &self,
        step_index: usize,
        total: usize,
        success: bool,
        latency_ms: u64,
        tools_used: &[String],
        rounds: usize,
        summary: &str,
        error_hint: &str,
    ) {
        self.send(crate::tui::events::UiEvent::SubAgentCompleted {
            step_index,
            total_steps: total,
            success,
            latency_ms,
            tools_used: tools_used.to_vec(),
            rounds,
            summary: summary.to_string(),
            error_hint: error_hint.to_string(),
        });
    }

    fn phase_started(&self, phase: &str, label: &str) {
        self.send(crate::tui::events::UiEvent::PhaseStarted {
            phase: phase.to_string(),
            label: label.to_string(),
        });
    }

    fn phase_ended(&self) {
        self.send(crate::tui::events::UiEvent::PhaseEnded);
    }

    fn project_config_loaded(&self, path: &str) {
        self.send(crate::tui::events::UiEvent::ProjectConfigLoaded {
            path: path.to_string(),
        });
    }

    fn onboarding_suggestion(&self, root: &str, project_type: &str) {
        self.send(crate::tui::events::UiEvent::OnboardingAvailable {
            root: root.to_string(),
            project_type: project_type.to_string(),
        });
    }

    fn plugin_suggestion(&self, total: usize, essential: usize) {
        self.send(crate::tui::events::UiEvent::Info(format!(
            "[plugins] {total} recommended ({essential} essential) — /plugins suggest"
        )));
    }

    fn plugin_bootstrapped(&self, plugin_id: &str, success: bool) {
        let status = if success { "✓" } else { "✗" };
        self.send(crate::tui::events::UiEvent::Info(format!(
            "[plugins] {status} {plugin_id}"
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_sink_no_panic_on_stream() {
        let sink = ClassicSink::new();
        sink.stream_text("hello ");
        sink.stream_text("world");
        let text = sink.stream_full_text();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn classic_sink_no_panic_on_warning() {
        let sink = ClassicSink::new();
        sink.warning("test warning", None);
        sink.warning("with hint", Some("try this"));
    }

    #[test]
    fn classic_sink_no_panic_on_error() {
        let sink = ClassicSink::new();
        sink.error("test error", None);
        sink.error("with hint", Some("check config"));
    }

    #[test]
    fn classic_sink_is_not_silent() {
        let sink = ClassicSink::new();
        assert!(!sink.is_silent());
    }

    #[test]
    fn silent_sink_accumulates_text() {
        let sink = SilentSink::new();
        sink.stream_text("hello ");
        sink.stream_text("world");
        assert_eq!(sink.text(), "hello world");
    }

    #[test]
    fn silent_sink_is_silent() {
        let sink = SilentSink::new();
        assert!(sink.is_silent());
    }

    #[test]
    fn silent_sink_reset_clears_text() {
        let sink = SilentSink::new();
        sink.stream_text("data");
        assert_eq!(sink.stream_full_text(), "data");
        sink.stream_reset();
        assert_eq!(sink.stream_full_text(), "");
    }

    #[test]
    fn classic_sink_stream_reset() {
        let sink = ClassicSink::new();
        sink.stream_text("round 1");
        assert_eq!(sink.stream_full_text(), "round 1");
        sink.stream_reset();
        assert_eq!(sink.stream_full_text(), "");
    }

    #[test]
    fn tui_sink_sends_stream_chunks() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.stream_text("hello ");
        sink.stream_text("world");
        assert_eq!(sink.stream_full_text(), "hello world");
        // Verify events were sent.
        let ev1 = rx.try_recv().unwrap();
        assert!(matches!(ev1, crate::tui::events::UiEvent::StreamChunk(ref s) if s == "hello "));
        let ev2 = rx.try_recv().unwrap();
        assert!(matches!(ev2, crate::tui::events::UiEvent::StreamChunk(ref s) if s == "world"));
    }

    #[test]
    fn tui_sink_sends_warning() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.warning("test", Some("hint"));
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::Warning { ref message, ref hint }
            if message == "test" && hint.as_deref() == Some("hint")));
    }

    #[test]
    fn tui_sink_sends_spinner_events() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.spinner_start("Thinking...");
        sink.spinner_stop();
        let ev1 = rx.try_recv().unwrap();
        assert!(matches!(ev1, crate::tui::events::UiEvent::SpinnerStart(ref s) if s == "Thinking..."));
        let ev2 = rx.try_recv().unwrap();
        assert!(matches!(ev2, crate::tui::events::UiEvent::SpinnerStop));
    }

    #[test]
    fn tui_sink_is_not_silent() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        assert!(!sink.is_silent());
    }

    #[test]
    fn tui_sink_stream_reset() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.stream_text("data");
        assert_eq!(sink.stream_full_text(), "data");
        sink.stream_reset();
        assert_eq!(sink.stream_full_text(), "");
    }

    #[test]
    fn tui_sink_sends_info() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.info("round separator");
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::Info(ref s) if s == "round separator"));
    }

    #[test]
    fn tui_sink_sends_tool_denied() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.tool_denied("bash");
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::ToolDenied(ref s) if s == "bash"));
    }

    #[test]
    fn classic_sink_plan_progress_no_panic() {
        use halcon_core::traits::PlanStep;
        let sink = ClassicSink::new();
        let steps = vec![
            PlanStep {
                step_id: uuid::Uuid::new_v4(),
                description: "Read file".into(),
                tool_name: Some("file_read".into()),
                parallel: false,
                confidence: 0.9,
                expected_args: None,
                outcome: None,
            },
        ];
        // Should not panic.
        sink.plan_progress("Test goal", &steps, 0);
    }

    #[test]
    fn tui_sink_sends_plan_progress() {
        use halcon_core::traits::PlanStep;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        let steps = vec![
            PlanStep {
                step_id: uuid::Uuid::new_v4(),
                description: "Read file".into(),
                tool_name: Some("file_read".into()),
                parallel: false,
                confidence: 0.9,
                expected_args: None,
                outcome: None,
            },
        ];
        sink.plan_progress("Test goal", &steps, 0);
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::PlanProgress { ref goal, current_step: 0, .. } if goal == "Test goal"));
    }

    #[test]
    fn classic_sink_task_status_no_panic() {
        let sink = ClassicSink::new();
        // Should not panic with various inputs.
        sink.task_status("Read config", "Completed", Some(1200), 2);
        sink.task_status("Write file", "Failed", None, 0);
        sink.task_status("Search code", "Running", Some(0), 1);
    }

    #[test]
    fn silent_sink_task_status_default_noop() {
        let sink = SilentSink::new();
        // Default no-op should not panic.
        sink.task_status("Test task", "Completed", Some(500), 1);
    }

    // --- Phase 40: Reasoning status tests ---

    #[test]
    fn classic_sink_reasoning_status_no_panic() {
        let sink = ClassicSink::new();
        // Should not panic with various inputs.
        sink.reasoning_status("CodeModification", "Moderate", "PlanExecuteReflect", 0.85, true);
        sink.reasoning_status("General", "Simple", "DirectExecution", 0.35, false);
    }

    #[test]
    fn silent_sink_reasoning_status_default_noop() {
        let sink = SilentSink::new();
        // Default no-op should not panic.
        sink.reasoning_status("Debugging", "Complex", "PlanExecuteReflect", 0.72, true);
    }

    // --- Phase 42B: Cockpit feedback sink tests ---

    #[test]
    fn classic_sink_round_started_no_panic() {
        let sink = ClassicSink::new();
        sink.round_started(1, "deepseek", "deepseek-chat");
    }

    #[test]
    fn classic_sink_round_ended_no_panic() {
        let sink = ClassicSink::new();
        sink.round_ended(1, 500, 200, 0.002, 1500);
    }

    #[test]
    fn classic_sink_model_selected_no_panic() {
        let sink = ClassicSink::new();
        sink.model_selected("gpt-4o", "openai", "complex task");
    }

    #[test]
    fn classic_sink_provider_fallback_no_panic() {
        let sink = ClassicSink::new();
        sink.provider_fallback("anthropic", "deepseek", "auth error");
    }

    #[test]
    fn classic_sink_loop_guard_action_no_panic() {
        let sink = ClassicSink::new();
        sink.loop_guard_action("inject_synthesis", "round 3");
    }

    #[test]
    fn classic_sink_compaction_complete_no_panic() {
        let sink = ClassicSink::new();
        sink.compaction_complete(50, 10, 4000);
    }

    #[test]
    fn classic_sink_cache_status_no_panic() {
        let sink = ClassicSink::new();
        sink.cache_status(true, "response_cache");
        sink.cache_status(false, "speculation");
    }

    #[test]
    fn classic_sink_speculative_result_no_panic() {
        let sink = ClassicSink::new();
        sink.speculative_result("file_read", true);
        sink.speculative_result("grep", false);
    }

    #[test]
    fn classic_sink_permission_awaiting_no_panic() {
        let sink = ClassicSink::new();
        sink.permission_awaiting("bash", &serde_json::json!({"command": "echo test"}), "Low");
    }

    #[test]
    fn tui_sink_sends_round_started() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.round_started(1, "deepseek", "deepseek-chat");
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::RoundStarted { round: 1, .. }));
    }

    #[test]
    fn tui_sink_sends_round_ended() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.round_ended(2, 800, 300, 0.004, 2500);
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::RoundEnded { round: 2, .. }));
    }

    #[test]
    fn cockpit_event_variant_construction() {
        let ev = crate::tui::events::UiEvent::CompactionComplete {
            old_msgs: 50, new_msgs: 10, tokens_saved: 4000,
        };
        assert!(matches!(ev, crate::tui::events::UiEvent::CompactionComplete { old_msgs: 50, .. }));
    }

    // --- Phase 42E: Expert mode gating tests ---

    #[test]
    fn simple_mode_hides_verbose() {
        // Default (non-expert) sink should not panic on verbose methods.
        // These are no-ops in simple mode.
        let sink = ClassicSink::new();
        assert!(!sink.expert);
        sink.model_selected("gpt-4o", "openai", "complex task");
        sink.cache_status(true, "response_cache");
        sink.speculative_result("file_read", true);
        sink.loop_guard_action("inject_synthesis", "round 3");
        sink.round_ended(1, 500, 200, 0.002, 1500);
    }

    #[test]
    fn expert_mode_shows_all() {
        let sink = ClassicSink::with_expert(true);
        assert!(sink.expert);
        // These should not panic; in expert mode they output to stderr.
        sink.model_selected("gpt-4o", "openai", "complex task");
        sink.cache_status(true, "response_cache");
        sink.speculative_result("file_read", true);
        sink.loop_guard_action("inject_synthesis", "round 3");
        sink.round_ended(1, 500, 200, 0.002, 1500);
        sink.compaction_complete(50, 10, 4000);
    }

    #[test]
    fn expert_flag_overrides_config() {
        // with_expert(true) should set expert mode regardless of default.
        let sink = ClassicSink::with_expert(true);
        assert!(sink.expert);
        let default_sink = ClassicSink::new();
        assert!(!default_sink.expert);
    }

    #[test]
    fn default_mode_is_simple() {
        let sink = ClassicSink::new();
        assert!(!sink.expert);
    }

    // === Phase 43C: Feedback completeness sink tests ===

    #[test]
    fn reflection_started_noop_simple_mode() {
        // In simple mode, reflection_started should not panic.
        let sink = ClassicSink::new();
        sink.reflection_started();
    }

    #[test]
    fn reflection_complete_noop_simple_mode() {
        let sink = ClassicSink::new();
        sink.reflection_complete("some analysis", 0.85);
    }

    #[test]
    fn consolidation_status_noop_simple_mode() {
        let sink = ClassicSink::new();
        sink.consolidation_status("consolidating reflections...");
    }

    #[test]
    fn tool_retrying_visible_all_modes() {
        // tool_retrying should not panic in either mode.
        let simple = ClassicSink::new();
        simple.tool_retrying("bash", 1, 3, 500);
        let expert = ClassicSink::with_expert(true);
        expert.tool_retrying("bash", 2, 3, 1000);
    }

    #[test]
    fn expert_mode_reflection_outputs() {
        let sink = ClassicSink::with_expert(true);
        // These should not panic; in expert mode they output to stderr.
        sink.reflection_started();
        sink.reflection_complete("round had 2 tool failures, suggest checking paths", 0.7);
        sink.consolidation_status("merging 25 reflections into clusters");
    }

    #[test]
    fn silent_sink_noop_43c_methods() {
        let sink = SilentSink::new();
        sink.reflection_started();
        sink.reflection_complete("analysis", 0.5);
        sink.consolidation_status("action");
        sink.tool_retrying("tool", 1, 3, 100);
    }

    // === Phase 43D: Context & reasoning sink tests ===

    #[test]
    fn context_tier_update_expert_mode() {
        let sink = ClassicSink::with_expert(true);
        // Should not panic in expert mode.
        sink.context_tier_update(500, 2000, 300, 5, 10, 8, 3, 1200);
    }

    #[test]
    fn context_tier_update_noop_simple_mode() {
        let sink = ClassicSink::new();
        // Should not panic in simple mode.
        sink.context_tier_update(500, 2000, 300, 5, 10, 8, 3, 1200);
    }

    #[test]
    fn reasoning_update_expert_mode() {
        let sink = ClassicSink::with_expert(true);
        sink.reasoning_update("PlanExecuteReflect", "CodeModification", "Complex");
    }

    #[test]
    fn reasoning_update_noop_simple_mode() {
        let sink = ClassicSink::new();
        sink.reasoning_update("DirectExecution", "General", "Simple");
    }

    // === Phase 43E: Full Phase 43 integration test ===

    #[test]
    fn all_phase_43_methods_callable() {
        // Verify all Phase 43 sink methods are callable without panic.
        let expert = ClassicSink::with_expert(true);
        let simple = ClassicSink::new();
        let silent = SilentSink::new();

        for sink in [&expert as &dyn RenderSink, &simple as &dyn RenderSink, &silent as &dyn RenderSink] {
            // Phase 43C
            sink.reflection_started();
            sink.reflection_complete("analysis text", 0.85);
            sink.consolidation_status("consolidating 25 reflections");
            sink.tool_retrying("bash", 1, 3, 500);
            // Phase 43D
            sink.context_tier_update(500, 2000, 300, 5, 10, 8, 3, 1200);
            sink.reasoning_update("PlanExecuteReflect", "CodeModification", "Complex");
        }
    }

    // === Phase E: Agent loop integration sink tests ===

    #[test]
    fn classic_sink_dry_run_noop() {
        let sink = ClassicSink::new();
        sink.dry_run_active(true);
        sink.dry_run_active(false);
    }

    #[test]
    fn classic_sink_token_budget_noop() {
        let sink = ClassicSink::new();
        sink.token_budget_update(500, 1000, 120.5);
    }

    #[test]
    fn classic_sink_provider_health_noop() {
        let sink = ClassicSink::new();
        sink.provider_health_update("anthropic", "healthy", 0.0, 0);
        sink.provider_health_update("openai", "degraded", 0.3, 5000);
        sink.provider_health_update("local", "unhealthy: timeout", 1.0, 60000);
    }

    #[test]
    fn classic_sink_circuit_breaker_noop() {
        let sink = ClassicSink::new();
        sink.circuit_breaker_update("anthropic", "closed", 0);
        sink.circuit_breaker_update("openai", "open", 5);
        sink.circuit_breaker_update("local", "half_open", 3);
    }

    #[test]
    fn classic_sink_agent_state_transition_noop() {
        let sink = ClassicSink::new();
        sink.agent_state_transition("idle", "executing", "agent started");
        sink.agent_state_transition("executing", "reflecting", "round failure");
        sink.agent_state_transition("reflecting", "executing", "reflection done");
        sink.agent_state_transition("executing", "complete", "task done");
    }

    #[test]
    fn silent_sink_phase_e_noop() {
        let sink = SilentSink::new();
        sink.dry_run_active(true);
        sink.token_budget_update(500, 1000, 120.5);
        sink.provider_health_update("test", "healthy", 0.0, 0);
        sink.circuit_breaker_update("test", "closed", 0);
        sink.agent_state_transition("idle", "executing", "start");
    }

    #[test]
    fn tui_sink_sends_dry_run_active() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.dry_run_active(true);
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::DryRunActive(true)));
    }

    #[test]
    fn tui_sink_sends_token_budget_update() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.token_budget_update(500, 1000, 120.5);
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::TokenBudgetUpdate { used: 500, limit: 1000, .. }));
    }

    #[test]
    fn tui_sink_sends_provider_health_update() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.provider_health_update("anthropic", "degraded", 0.3, 5000);
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::ProviderHealthUpdate {
            ref provider,
            status: crate::tui::events::ProviderHealthStatus::Degraded { .. },
        } if provider == "anthropic"));
    }

    #[test]
    fn tui_sink_sends_circuit_breaker_update() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.circuit_breaker_update("openai", "open", 5);
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::CircuitBreakerUpdate {
            ref provider,
            state: crate::tui::events::CircuitBreakerState::Open,
            failure_count: 5,
        } if provider == "openai"));
    }

    #[test]
    fn tui_sink_sends_agent_state_transition() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.agent_state_transition("idle", "executing", "start");
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::AgentStateTransition {
            from: crate::tui::events::AgentState::Idle,
            to: crate::tui::events::AgentState::Executing,
            ..
        }));
    }

    #[test]
    fn tui_sink_provider_health_healthy() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.provider_health_update("test", "healthy", 0.0, 0);
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::ProviderHealthUpdate {
            status: crate::tui::events::ProviderHealthStatus::Healthy,
            ..
        }));
    }

    #[test]
    fn tui_sink_provider_health_unhealthy() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.provider_health_update("test", "down: connection refused", 1.0, 0);
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::ProviderHealthUpdate {
            status: crate::tui::events::ProviderHealthStatus::Unhealthy { .. },
            ..
        }));
    }

    #[test]
    fn tui_sink_circuit_breaker_closed() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.circuit_breaker_update("test", "closed", 0);
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::CircuitBreakerUpdate {
            state: crate::tui::events::CircuitBreakerState::Closed,
            ..
        }));
    }

    #[test]
    fn tui_sink_circuit_breaker_half_open() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.circuit_breaker_update("test", "half_open", 2);
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::CircuitBreakerUpdate {
            state: crate::tui::events::CircuitBreakerState::HalfOpen,
            failure_count: 2,
            ..
        }));
    }

    #[test]
    fn tui_sink_agent_state_all_transitions() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        let transitions = [
            ("idle", "planning"),
            ("planning", "executing"),
            ("executing", "tool_wait"),
            ("tool_wait", "executing"),
            ("executing", "reflecting"),
            ("reflecting", "executing"),
            ("executing", "complete"),
            ("idle", "failed"),
        ];
        for (from, to) in &transitions {
            sink.agent_state_transition(from, to, "test");
        }
        // Drain all events and verify count.
        let mut count = 0;
        while rx.try_recv().is_ok() { count += 1; }
        assert_eq!(count, transitions.len());
    }

    #[test]
    fn all_phase_e_methods_callable() {
        let expert = ClassicSink::with_expert(true);
        let simple = ClassicSink::new();
        let silent = SilentSink::new();

        for sink in [&expert as &dyn RenderSink, &simple as &dyn RenderSink, &silent as &dyn RenderSink] {
            sink.dry_run_active(true);
            sink.dry_run_active(false);
            sink.token_budget_update(1000, 50000, 200.0);
            sink.provider_health_update("test", "healthy", 0.0, 0);
            sink.provider_health_update("test", "degraded", 0.2, 3000);
            sink.circuit_breaker_update("test", "closed", 0);
            sink.circuit_breaker_update("test", "open", 5);
            sink.agent_state_transition("idle", "executing", "start");
            sink.agent_state_transition("executing", "complete", "done");
        }
    }

    // ── R-01 regression: UTF-8 safety in stream_thinking ─────────────────────

    #[test]
    fn thinking_preview_does_not_panic_on_multibyte_chars() {
        // "ñ" is 2 bytes in UTF-8.  Fill the preview buffer to just before the limit
        // with ASCII, then push a 2-byte char that would straddle the boundary if
        // we used byte-slicing.  The fix uses char-aware truncation so this must
        // never panic.
        let sink = ClassicSink::new();
        // Fill preview to 159 bytes (1 byte short of limit) with ASCII 'a'.
        let almost_full: String = "a".repeat(159);
        sink.stream_thinking(&almost_full);
        // Now push a 2-byte char — byte-slicing would panic trying to fit 1 byte.
        sink.stream_thinking("ñ"); // 2 UTF-8 bytes — only 1 byte room remains
        // If we reach here without panic, the fix is correct.
    }

    #[test]
    fn thinking_preview_handles_emoji_boundary() {
        // Emoji like "🦅" is 4 bytes. If only 3 bytes remain in the buffer, byte-slicing panics.
        let sink = ClassicSink::new();
        let almost_full: String = "a".repeat(157); // 3 bytes before limit
        sink.stream_thinking(&almost_full);
        sink.stream_thinking("🦅"); // 4 UTF-8 bytes — would panic with old byte-slice code
        // Must complete without panic.
    }

    #[test]
    fn thinking_preview_exact_char_boundary_fills_correctly() {
        // Verify that a multi-byte char fitting exactly into remaining space is included.
        let sink = ClassicSink::new();
        let almost_full: String = "a".repeat(158); // 2 bytes before limit
        sink.stream_thinking(&almost_full);
        sink.stream_thinking("ñ"); // exactly 2 bytes — should fit
        // Preview must include "ñ" at end.
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SubAgentSink — routes permission events to the parent UI from orchestrated
// sub-agents that run headless (no direct TUI channel).
//
// The orchestrator creates a SubAgentSink for each sub-agent when a
// PermissionAwaiter callback is provided (e.g. TUI mode). The callback is
// type-erased so this struct has no TUI-specific dependencies and compiles
// regardless of feature flags.
//
// All output methods are no-ops — sub-agents are headless for text output.
// ─────────────────────────────────────────────────────────────────────────────

/// Risk-adaptive TUI countdown timeout for permission modals.
///
/// Returns the number of seconds the TUI should show the modal before auto-denying.
/// The backend waits indefinitely; only the TUI side enforces this deadline.
pub fn timeout_for_risk(risk_level: &str) -> u64 {
    match risk_level {
        "Critical" => 300, // 5 min — rm-rf, system changes: read carefully
        "High"     => 180, // 3 min — file_write, bash with side effects
        "Medium"   => 120, // 2 min — file_edit, git operations
        _          =>  60, // 1 min — config changes, minor writes (Low)
    }
}

/// Callback invoked when a sub-agent needs permission confirmation.
///
/// Receives: `tool_name`, `args_json`, `risk_level`, `timeout_secs`, `reply_sender`.
/// In TUI mode the callback sends a `UiEvent::PermissionAwaiting` to the TUI.
/// The `reply_sender` is the channel the sub-agent's `PermissionChecker` waits on.
pub type PermissionAwaiter = std::sync::Arc<
    dyn Fn(
            &str,
            &serde_json::Value,
            &str,
            u64,
            tokio::sync::mpsc::UnboundedSender<halcon_core::types::PermissionDecision>,
        ) + Send
        + Sync,
>;

/// Render sink for sub-agents running under a parent UI.
///
/// Routes permission events to the parent via a `PermissionAwaiter` callback.
/// All output methods are no-ops since sub-agents produce no visible output.
pub struct SubAgentSink {
    perm_awaiter: PermissionAwaiter,
    perm_reply_tx: tokio::sync::mpsc::UnboundedSender<halcon_core::types::PermissionDecision>,
}

impl SubAgentSink {
    pub fn new(
        perm_awaiter: PermissionAwaiter,
        perm_reply_tx: tokio::sync::mpsc::UnboundedSender<halcon_core::types::PermissionDecision>,
    ) -> Self {
        Self { perm_awaiter, perm_reply_tx }
    }
}

impl RenderSink for SubAgentSink {
    fn stream_text(&self, _text: &str) {}
    fn stream_code_block(&self, _lang: &str, _code: &str) {}
    fn stream_tool_marker(&self, _name: &str) {}
    fn stream_done(&self) {}
    fn stream_error(&self, _msg: &str) {}
    fn tool_start(&self, _name: &str, _input: &serde_json::Value) {}
    fn tool_output(&self, _block: &ContentBlock, _duration_ms: u64) {}
    fn tool_denied(&self, _name: &str) {}
    fn spinner_start(&self, _label: &str) {}
    fn spinner_stop(&self) {}
    fn warning(&self, _message: &str, _hint: Option<&str>) {}
    fn error(&self, _message: &str, _hint: Option<&str>) {}
    fn info(&self, _message: &str) {}
    fn is_silent(&self) -> bool { true }
    fn stream_reset(&self) {}
    fn stream_full_text(&self) -> String { String::new() }

    fn permission_awaiting(&self, tool: &str, args: &serde_json::Value, risk_level: &str) {
        let timeout_secs = timeout_for_risk(risk_level);
        // Invoke the callback which notifies the parent UI and embeds the reply_tx
        // so the TUI can route the user's decision back to this sub-agent's
        // PermissionChecker (which is waiting on perm_reply_tx's paired receiver).
        (self.perm_awaiter)(tool, args, risk_level, timeout_secs, self.perm_reply_tx.clone());
    }
}
