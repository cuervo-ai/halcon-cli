//! TUI application shell — manages the render loop and event dispatch.

pub(crate) use std::collections::{HashMap, VecDeque};
pub(crate) use std::io;
pub(crate) use std::time::{Duration, Instant};

pub(crate) use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyboardEnhancementFlags, MouseButton, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
pub(crate) use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
pub(crate) use crossterm::ExecutableCommand;
pub(crate) use ratatui::backend::CrosstermBackend;
pub(crate) use ratatui::layout::{Constraint, Direction, Layout, Rect};
pub(crate) use ratatui::style::{Modifier, Style};
pub(crate) use ratatui::text::{Line, Span};
pub(crate) use ratatui::widgets::{Block, Borders, Paragraph};
pub(crate) use ratatui::Terminal;
pub(crate) use tokio::sync::mpsc;

pub(crate) use super::constants;
pub(crate) use super::conversational_overlay::ConversationalOverlay;
pub(crate) use super::events::{ControlEvent, SessionInfo, UiEvent};
pub(crate) use super::highlight::HighlightManager;
pub(crate) use super::input;
pub(crate) use super::layout;
pub(crate) use super::overlay::{self, OverlayKind};
pub(crate) use super::permission_context::{PermissionContext, RiskLevel};
pub(crate) use super::state::{AgentControl, AppState, FocusZone, PendingAttachment, UiMode};
pub(crate) use super::clipboard::{paste_safe, PasteOutcome};
pub(crate) use super::transition_engine::TransitionEngine;
pub(crate) use super::activity_types::ActivityLine; // P0.1B: Migrated to activity_types
pub(crate) use super::widgets::activity_indicator::AgentState;
pub(crate) use super::widgets::agent_badge::AgentBadge;
pub(crate) use super::widgets::panel::SidePanel;
pub(crate) use super::widgets::permission_modal::PermissionModal;
pub(crate) use super::widgets::prompt::PromptState;
pub(crate) use super::widgets::status::{StatusPatch, StatusState};
pub(crate) use super::widgets::toast::{Toast, ToastLevel, ToastStack};

/// Maximum number of events stored in the ring buffer for the inspector.
pub(crate) const EVENT_RING_CAPACITY: usize = 200;

/// A timestamped event entry for the inspector ring buffer.
#[derive(Debug, Clone)]
pub struct EventEntry {
    /// Wall-clock offset from app start in milliseconds.
    pub offset_ms: u64,
    /// Summary label of the event.
    pub label: String,
}

/// Expansion animation state for a single line.
///
/// Tracks progress of expand/collapse animation using time-based easing.
/// Phase B1: Smooth height transitions for tool results.
#[derive(Debug, Clone)]
pub struct ExpansionAnimation {
    /// Target state: true = expanding to 1.0, false = collapsing to 0.0.
    pub expanding: bool,
    /// Current progress [0.0, 1.0] where 0.0 = collapsed, 1.0 = fully expanded.
    pub progress: f32,
    /// When this animation started.
    pub started_at: Instant,
    /// Animation duration.
    pub duration: Duration,
}

impl ExpansionAnimation {
    /// Start expanding from current progress.
    pub fn expand_from(progress: f32) -> Self {
        Self {
            expanding: true,
            progress,
            started_at: Instant::now(),
            duration: Duration::from_millis(200), // 200ms expand
        }
    }

    /// Start collapsing from current progress.
    pub fn collapse_from(progress: f32) -> Self {
        Self {
            expanding: false,
            progress,
            started_at: Instant::now(),
            duration: Duration::from_millis(150), // 150ms collapse (snappier)
        }
    }

    /// Get current eased progress [0.0, 1.0].
    ///
    /// Uses EaseInOut for smooth acceleration/deceleration.
    /// Returns target value (0.0 or 1.0) once animation completes.
    pub fn current(&self) -> f32 {
        let elapsed = self.started_at.elapsed();
        if elapsed >= self.duration {
            return if self.expanding { 1.0 } else { 0.0 };
        }

        let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        let t_eased = ease_in_out(t);

        if self.expanding {
            self.progress + (1.0 - self.progress) * t_eased
        } else {
            self.progress * (1.0 - t_eased)
        }
    }

    /// Check if animation is complete.
    pub fn is_complete(&self) -> bool {
        self.started_at.elapsed() >= self.duration
    }
}

/// EaseInOut easing function (smoothstep).
///
/// Slow start, fast middle, slow end.
fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        2.0 * t * t
    } else {
        -1.0 + (4.0 - 2.0 * t) * t
    }
}

/// Calculate shimmer progress [0.0, 1.0] for a loading skeleton.
///
/// Phase B2: Cyclic shimmer animation with 1-second period.
/// Returns normalized position [0.0, 1.0] of the shimmer wave.
pub fn shimmer_progress(elapsed: Duration) -> f32 {
    const SHIMMER_PERIOD_MS: f32 = 1000.0; // 1 second cycle
    let elapsed_ms = elapsed.as_millis() as f32;
    let t = (elapsed_ms % SHIMMER_PERIOD_MS) / SHIMMER_PERIOD_MS;
    t // Returns [0.0, 1.0] repeating
}

/// The TUI application. Owns the terminal, state, and event channels.
pub struct TuiApp {
    state: AppState,
    prompt: PromptState,
    // P0.4B: activity: ActivityState removed — migrated to activity_model
    status: StatusState,
    panel: SidePanel,
    /// Receives UiEvents from the agent loop (via TuiSink).
    /// Unbounded to prevent PermissionAwaiting from being dropped on high-throughput streams.
    ui_rx: mpsc::UnboundedReceiver<UiEvent>,
    /// Sends prompt text to the agent loop.
    prompt_tx: mpsc::UnboundedSender<String>,
    /// Sends control events (pause/step/cancel) to the agent loop.
    ctrl_tx: mpsc::UnboundedSender<ControlEvent>,
    /// Sends permission decisions to the executor's PermissionChecker.
    /// Extended from bool to PermissionDecision to support 8-option advanced modal.
    /// Dedicated channel ensures the decision reaches the executor even while the
    /// agent loop is blocked on tool execution.
    perm_tx: mpsc::UnboundedSender<halcon_core::types::PermissionDecision>,
    /// Conversational permission overlay instance (Phase I-6C, kept for compatibility).
    conversational_overlay: Option<ConversationalOverlay>,
    /// Permission modal (Phase 2.2) — replaces conversational_overlay in new flow.
    permission_modal: Option<PermissionModal>,
    /// Sub-agent permission reply channel (set when PermissionAwaiting arrives with reply_tx).
    /// When Some, permission decisions are sent here instead of perm_tx (main agent channel).
    pending_perm_reply_tx: Option<mpsc::UnboundedSender<halcon_core::types::PermissionDecision>>,
    /// Phase I2 Fix: Submit button area for compact styled button (14 cols, 1 line).
    submit_button_area: Rect,
    /// Ring buffer of recent events for the Expert inspector panel.
    event_log: VecDeque<EventEntry>,
    /// Start time for computing event offsets.
    start_time: Instant,
    /// Toast notification stack (Phase F1).
    toasts: ToastStack,
    /// Search state for activity zone search (B4).
    search_matches: Vec<usize>,
    search_current: usize,
    /// Watchdog: timestamp when agent last started processing (for timeout detection).
    agent_started_at: Option<Instant>,
    /// Watchdog: maximum agent duration in seconds before forcing UI unlock (default: 600 = 10 min).
    max_agent_duration_secs: u64,
    /// Phase 2.3: Perceptual color transition engine.
    transition_engine: TransitionEngine,
    /// Phase 2.3: Highlight pulse manager.
    highlights: HighlightManager,
    /// Phase 3.1: Agent status badge with transitions.
    agent_badge: AgentBadge,

    // Phase A1: SOTA Activity Architecture
    /// Activity data model with O(1) search indexing.
    activity_model: crate::tui::activity_model::ActivityModel,
    /// Activity navigation state (J/K selection, expand/collapse, search).
    activity_navigator: crate::tui::activity_navigator::ActivityNavigator,
    /// Activity interaction controller (keyboard/mouse handlers).
    activity_controller: crate::tui::activity_controller::ActivityController,

    // Phase A2: Virtual Scroll Optimization
    /// Activity renderer with LRU cache and virtual scrolling.
    activity_renderer: crate::tui::activity_renderer::ActivityRenderer,

    // Phase B1: Expand/Collapse Animations
    /// Expansion animations keyed by line index.
    /// Tracks smooth height transitions for expanding/collapsing tool results.
    expansion_animations: HashMap<usize, ExpansionAnimation>,

    // Phase B2: Loading Skeletons
    /// Executing tools keyed by tool name → start time.
    /// Used to calculate shimmer animation progress for loading skeletons.
    executing_tools: HashMap<String, Instant>,

    // Phase B4: Hover Effects (mouse event routing)
    /// Last rendered activity zone area (for mouse event boundary detection).
    /// Updated on each render, used in mouse event handler.
    last_activity_area: Rect,

    /// Last rendered panel area (for scroll calculation).
    /// Updated on each render, used to calculate max scroll offset.
    last_panel_area: Rect,

    // Phase 3 SRCH-004: Database for search history persistence
    /// Optional database for saving/loading search history.
    /// None when running without database (e.g., tests, --no-db mode).
    db: Option<halcon_storage::AsyncDatabase>,

    /// Flag to track if search history has been loaded from database.
    /// Prevents redundant database queries on every search overlay open.
    search_history_loaded: bool,

    // Phase 45: Status Bar Audit + Session Management
    /// Last rendered status bar area (for STOP button click detection).
    last_status_area: Rect,
    /// Computed ctrl button (▶ RUN / ■ STOP) area for mouse click detection.
    ctrl_button_area: Rect,
    /// Computed session ID label area for click-to-copy detection.
    session_id_button_area: Rect,
    /// Sender clone used by background async tasks to push UiEvents back to the app.
    ui_tx_for_bg: Option<tokio::sync::mpsc::UnboundedSender<UiEvent>>,
    /// Cached session list loaded from DB (for SessionList overlay).
    session_list: Vec<SessionInfo>,
    /// Cursor index in the session list overlay.
    session_list_selected: usize,

    // --- Sudo Password Elevation (Phase 50) ---
    /// Sender to deliver the password (or None on cancel) to the executor.
    sudo_pw_tx: Option<tokio::sync::mpsc::UnboundedSender<Option<String>>>,
    /// Current password being typed (masked in the modal).
    sudo_password_buf: String,
    /// "Remember for 5 minutes" toggle state.
    sudo_remember_password: bool,
    /// Whether a cached sudo password is available (within 5-minute TTL).
    sudo_has_cached: bool,
    /// Cached sudo password + expiry (in-process, never written to disk).
    sudo_cache: Option<(String, std::time::Instant)>,

    /// Whether the command palette was opened because the user typed `/` in the prompt
    /// (as opposed to Ctrl+P).  When true, typed characters continue flowing to the
    /// prompt and the palette mirrors the current `/xxx` prefix as its filter; when
    /// false the palette consumes Backspace and Char events itself (normal Ctrl+P mode).
    slash_completing: bool,

    // ─── Frontier update notification (v0.3.10) ──────────────────────────────
    /// Pending update info — Some when `get_pending_update_info()` returned data.
    /// When Some the UpdateAvailable overlay is opened at startup before accepting input.
    pub(crate) pending_update: Option<crate::commands::update::UpdateInfo>,
    /// Set to `true` by the overlay handler when the user chooses to install.
    /// Checked by repl/mod.rs after TUI exits; triggers `run_update_from_info` + re-exec.
    pub(crate) update_install_signal: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// Detect the OS username for the user avatar in the activity feed.
///
/// Priority: $USER → $LOGNAME → home dir basename → "you".
fn detect_username() -> String {
    if let Ok(u) = std::env::var("USER") {
        if !u.is_empty() {
            return u;
        }
    }
    if let Ok(u) = std::env::var("LOGNAME") {
        if !u.is_empty() {
            return u;
        }
    }
    dirs::home_dir()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "you".to_string())
}

impl TuiApp {
    /// Create a new TUI application with the given initial UI mode.
    pub fn new(
        ui_rx: mpsc::UnboundedReceiver<UiEvent>,
        prompt_tx: mpsc::UnboundedSender<String>,
        ctrl_tx: mpsc::UnboundedSender<ControlEvent>,
        perm_tx: mpsc::UnboundedSender<halcon_core::types::PermissionDecision>,
        db: Option<halcon_storage::AsyncDatabase>,
    ) -> Self {
        Self::with_mode(ui_rx, prompt_tx, ctrl_tx, perm_tx, db, UiMode::Standard)
    }

    /// Create a new TUI application with a specific initial UI mode.
    pub fn with_mode(
        ui_rx: mpsc::UnboundedReceiver<UiEvent>,
        prompt_tx: mpsc::UnboundedSender<String>,
        ctrl_tx: mpsc::UnboundedSender<ControlEvent>,
        perm_tx: mpsc::UnboundedSender<halcon_core::types::PermissionDecision>,
        db: Option<halcon_storage::AsyncDatabase>,
        initial_mode: UiMode,
    ) -> Self {
        let panel_visible = matches!(initial_mode, UiMode::Standard | UiMode::Expert);
        let mut state = AppState::new();
        state.ui_mode = initial_mode;
        state.panel_visible = panel_visible;
        state.user_display_name = detect_username();
        Self {
            state,
            prompt: PromptState::new(),
            // P0.4B: activity: ActivityState::new() removed — using activity_model instead
            status: StatusState::new(),
            panel: SidePanel::new(),
            ui_rx,
            prompt_tx,
            ctrl_tx,
            perm_tx,
            conversational_overlay: None,
            permission_modal: None, // Phase 2.2
            pending_perm_reply_tx: None,
            submit_button_area: Rect::default(),
            event_log: VecDeque::with_capacity(EVENT_RING_CAPACITY),
            start_time: Instant::now(),
            toasts: ToastStack::new(),
            search_matches: Vec::new(),
            search_current: 0,
            agent_started_at: None,
            max_agent_duration_secs: 600, // 10 minutes default watchdog timeout
            transition_engine: TransitionEngine::new(),
            highlights: HighlightManager::new(),
            agent_badge: AgentBadge::new(),
            // Phase A1: Initialize SOTA activity modules
            activity_model: crate::tui::activity_model::ActivityModel::new(),
            activity_navigator: crate::tui::activity_navigator::ActivityNavigator::new(),
            activity_controller: crate::tui::activity_controller::ActivityController::new(),
            // Phase A2: Initialize virtual scroll renderer
            activity_renderer: crate::tui::activity_renderer::ActivityRenderer::new(),
            // Phase B1: Initialize expansion animations
            expansion_animations: HashMap::new(),
            // Phase B2: Initialize executing tools tracker
            executing_tools: HashMap::new(),
            // Phase B4: Initialize last activity area (will be updated on first render)
            last_activity_area: Rect::default(),
            // Panel area tracking for scroll calculation
            last_panel_area: Rect::default(),
            // Phase 3 SRCH-004: Database for search history persistence
            db,
            search_history_loaded: false,
            // Phase 45: Status Bar Audit + Session Management
            last_status_area: Rect::default(),
            ctrl_button_area: Rect::default(),
            session_id_button_area: Rect::default(),
            ui_tx_for_bg: None,
            session_list: Vec::new(),
            session_list_selected: 0,
            // Sudo Password Elevation (Phase 50)
            sudo_pw_tx: None,
            sudo_password_buf: String::new(),
            sudo_remember_password: false,
            sudo_has_cached: false,
            sudo_cache: None,
            slash_completing: false,
            pending_update: None,
            update_install_signal: None,
        }
    }

    /// Set a pending update so the TUI opens the UpdateAvailable overlay at startup.
    pub fn set_pending_update(
        &mut self,
        info: crate::commands::update::UpdateInfo,
        signal: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        self.pending_update = Some(info);
        self.update_install_signal = Some(signal);
    }

    /// Wire the sudo password sender so the TUI can deliver passwords to the executor.
    /// Called from repl/mod.rs after TuiApp creation.
    pub fn set_sudo_pw_tx(&mut self, tx: tokio::sync::mpsc::UnboundedSender<Option<String>>) {
        self.sudo_pw_tx = Some(tx);
    }

    /// Set a background sender so async tasks can push UiEvents back into the app.
    /// Called from repl/mod.rs after TuiApp::with_mode().
    pub fn set_ui_tx(&mut self, tx: tokio::sync::mpsc::UnboundedSender<UiEvent>) {
        self.ui_tx_for_bg = Some(tx);
    }

    /// Push an enhanced startup banner with real feature data and artistic Momoto crow.
    pub fn push_banner(
        &mut self,
        version: &str,
        provider: &str,
        provider_connected: bool,
        model: &str,
        session_id: &str,
        session_type: &str,
        routing: Option<&crate::render::banner::RoutingDisplay>,
        features: &crate::render::banner::FeatureStatus,
    ) {
        // Minimalist SOTA banner using momoto design principles
        self.activity_model.push_info("");

        // Welcome header with version (clean, single line)
        let status_icon = if provider_connected { "●" } else { "○" };
        self.activity_model.push_info(&format!(
            "  {} Bienvenido a halcon v{}  —  {} {} {}",
            status_icon,
            version,
            provider,
            if provider_connected { "↗" } else { "⊗" },
            session_id
        ));

        self.activity_model.push_info("");

        // Minimal essential help (adaptive based on UI mode and features)
        let help_line = if features.background_tools_enabled {
            format!("  F1 Ayuda  │  Enter Enviar  │  Shift+↵ nueva línea  │  Ctrl+P comandos  │  {} herramientas activas", features.tool_count)
        } else {
            format!("  F1 Ayuda  │  Enter Enviar  │  Shift+↵ nueva línea  │  Ctrl+P comandos  │  {} herramientas", features.tool_count)
        };
        self.activity_model.push_info(&help_line);
        self.activity_model.push_info("");

        // Eagerly initialize status bar with session info — prevents blank SESSION on first frame.
        // The async ui_tx StatusUpdate arrives later but races with the first render.
        self.status.apply_patch(StatusPatch {
            provider: Some(provider.to_string()),
            model: Some(model.to_string()),
            session_id: Some(session_id.to_string()),
            ..Default::default()
        });
    }

}

// ─── Sub-module declarations ────────────────────────────────────────────────
// Each sub-module contains one logical slice of `impl TuiApp`.
// Child modules have access to private fields of TuiApp because they are
// declared inside the same module that owns the struct definition.

mod run_loop;
mod render;
mod overlay_handler;
mod search;
mod slash_commands;
mod action_handler;
mod ui_event_handler;
mod utils;
#[cfg(test)]
mod tests;

/// Format a short preview string from a tool's input JSON value.
pub(crate) fn format_input_preview(input: &serde_json::Value) -> String {
    match input {
        serde_json::Value::Object(map) => {
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in map.iter().take(3) {
                let val = match v {
                    serde_json::Value::String(s) => truncate_str(s, 40),
                    other => truncate_str(&other.to_string(), 40),
                };
                parts.push(format!("{k}={val}"));
            }
            if map.len() > 3 {
                parts.push(format!("+{} more", map.len() - 3));
            }
            parts.join(", ")
        }
        serde_json::Value::String(s) => truncate_str(s, 60),
        other => truncate_str(&other.to_string(), 60),
    }
}

/// Truncate a string to at most `max_chars` Unicode characters, appending `…` if truncated.
/// Safe for all Unicode text — never panics on multi-byte characters.
pub(crate) fn truncate_str(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

/// Generate a one-line summary label for an event (for the ring buffer).
pub(crate) fn event_summary(ev: &UiEvent) -> String {
    match ev {
        UiEvent::StreamChunk(_) => constants::EVENT_STREAM_CHUNK.into(),
        UiEvent::StreamThinking(_) => "StreamThinking".into(),
        UiEvent::ThinkingProgress { chars } => format!("ThinkingProgress({chars}chars)"),
        UiEvent::ThinkingComplete { char_count, .. } => format!("ThinkingComplete({char_count}chars)"),
        UiEvent::StreamCodeBlock { lang, .. } => format!("CodeBlock({lang})"),
        UiEvent::StreamToolMarker(n) => format!("ToolMarker({n})"),
        UiEvent::StreamDone => constants::EVENT_STREAM_DONE.into(),
        UiEvent::StreamError(e) => format!("StreamError({e})"),
        UiEvent::ToolStart { name, .. } => format!("ToolStart({name})"),
        UiEvent::ToolOutput { name, is_error, .. } => {
            if *is_error { format!("ToolError({name})") } else { format!("ToolDone({name})") }
        }
        UiEvent::ToolDenied(n) => format!("ToolDenied({n})"),
        UiEvent::SpinnerStart(l) => format!("SpinnerStart({l})"),
        UiEvent::SpinnerStop => constants::EVENT_SPINNER_STOP.into(),
        UiEvent::Warning { message, .. } => format!("Warning({message})"),
        UiEvent::Error { message, .. } => format!("Error({message})"),
        UiEvent::Info(m) => format!("Info({m})"),
        UiEvent::StatusUpdate { .. } => constants::EVENT_STATUS_UPDATE.into(),
        UiEvent::RoundStart(n) => format!("RoundStart({n})"),
        UiEvent::RoundEnd(n) => format!("RoundEnd({n})"),
        UiEvent::Redraw => constants::EVENT_REDRAW.into(),
        UiEvent::AgentStartedPrompt => "AgentStarted".into(),
        UiEvent::AgentFinishedPrompt => "AgentFinished".into(),
        UiEvent::PromptQueueStatus(n) => format!("QueueStatus({n})"),
        UiEvent::AgentDone => constants::EVENT_AGENT_DONE.into(),
        UiEvent::Quit => constants::EVENT_QUIT.into(),
        UiEvent::PlanProgress { current_step, .. } => format!("PlanProgress(step={current_step})"),
        UiEvent::SessionInitialized { session_id } => format!("SessionInit({session_id})"),
        UiEvent::RoundStarted { round, .. } => format!("RoundStarted({round})"),
        UiEvent::RoundEnded { round, .. } => format!("RoundEnded({round})"),
        UiEvent::ModelSelected { model, .. } => format!("ModelSelected({model})"),
        UiEvent::ProviderFallback { from, to, .. } => format!("Fallback({from}→{to})"),
        UiEvent::LoopGuardAction { action, .. } => format!("LoopGuard({action})"),
        UiEvent::CompactionComplete { .. } => constants::EVENT_COMPACTION.into(),
        UiEvent::CacheStatus { hit, .. } => format!("Cache({})", if *hit { "hit" } else { "miss" }),
        UiEvent::SpeculativeResult { tool, hit } => format!("Speculative({tool},{})", if *hit { "hit" } else { "miss" }),
        UiEvent::PermissionAwaiting { tool, risk_level, .. } => format!("PermAwait({tool},{risk_level})"),
        UiEvent::ReflectionStarted => constants::EVENT_REFLECTION_START.into(),
        UiEvent::ReflectionComplete { .. } => constants::EVENT_REFLECTION_DONE.into(),
        UiEvent::ConsolidationStatus { .. } => constants::EVENT_CONSOLIDATION.into(),
        UiEvent::ConsolidationComplete { merged, pruned, .. } => format!("ConsolidationDone(m:{merged},p:{pruned})"),
        UiEvent::ToolRetrying { tool, attempt, .. } => format!("ToolRetry({tool},{attempt})"),
        UiEvent::ContextTierUpdate { .. } => constants::EVENT_CONTEXT_UPDATE.into(),
        UiEvent::ReasoningUpdate { strategy, .. } => format!("Reasoning({strategy})"),
        UiEvent::Phase2Metrics { .. } => "Phase2Metrics".into(),
        UiEvent::DryRunActive(a) => format!("DryRun({a})"),
        UiEvent::TokenBudgetUpdate { .. } => constants::EVENT_TOKEN_BUDGET.into(),
        UiEvent::ProviderHealthUpdate { provider, .. } => format!("Health({provider})"),
        UiEvent::CircuitBreakerUpdate { provider, .. } => format!("Breaker({provider})"),
        UiEvent::AgentStateTransition { from, to, .. } => format!("State({from:?}→{to:?})"),
        UiEvent::TaskStatus { ref title, ref status, .. } => format!("TaskStatus({title},{status})"),
        UiEvent::ReasoningStatus { ref task_type, .. } => format!("Reasoning({task_type})"),
        UiEvent::ContextServersList { total_count, .. } => format!("ContextServers({total_count})"),
        // Phase 45: Status Bar Audit + Session Management
        UiEvent::TokenDelta { session_input, session_output, .. } => format!("TokenDelta(↑{session_input}↓{session_output})"),
        UiEvent::SessionList { sessions } => format!("SessionList({})", sessions.len()),
        // FASE 1.2: HICON event summaries
        UiEvent::HiconCorrection { strategy, round, .. } => format!("HICON:Correction({strategy},r{round})"),
        UiEvent::HiconAnomaly { anomaly_type, severity, .. } => format!("HICON:Anomaly({severity}:{anomaly_type})"),
        UiEvent::HiconCoherence { phi, status, .. } => format!("HICON:Coherence(Φ={phi:.2},{status})"),
        UiEvent::HiconBudgetWarning { predicted_overflow_rounds, .. } => format!("HICON:Budget(overflow:{predicted_overflow_rounds}r)"),
        UiEvent::SudoPasswordRequest { tool, .. } => format!("SudoPasswordRequest({tool})"),
        // Dev Ecosystem Phase 5
        UiEvent::IdeConnected { port } => format!("IdeConnected(:{port})"),
        UiEvent::IdeDisconnected => "IdeDisconnected".into(),
        UiEvent::IdeBuffersUpdated { count, .. } => format!("IdeBuffers({count})"),
        // Multi-Agent Orchestration Visibility
        UiEvent::OrchestratorWave { wave_index, total_waves, task_count } => format!("OrchestratorWave({wave_index}/{total_waves},{task_count}tasks)"),
        UiEvent::SubAgentSpawned { step_index, total_steps, .. } => format!("SubAgentSpawned({step_index}/{total_steps})"),
        UiEvent::SubAgentCompleted { step_index, total_steps, success, .. } => format!("SubAgentCompleted({step_index}/{total_steps},{})", if *success { "ok" } else { "fail" }),
        // Multimodal
        UiEvent::MediaAnalysisStarted { count } => format!("MediaAnalysisStarted({count})"),
        UiEvent::MediaAnalysisComplete { filename, tokens } => format!("MediaAnalysisComplete({filename},{tokens})"),
        // Phase 83: Phase-Aware Skeleton/Spinner
        UiEvent::PhaseStarted { phase, .. } => format!("PhaseStarted({phase})"),
        UiEvent::PhaseEnded => "PhaseEnded".into(),
        // Phase 93: Cross-Platform SOTA — media attachment events
        UiEvent::AttachmentAdded { path, modality } => format!("AttachmentAdded({modality}:{path})"),
        UiEvent::AttachmentRemoved { index } => format!("AttachmentRemoved({index})"),
        // Phase 94: Project Onboarding
        UiEvent::OnboardingAvailable { root, project_type } => format!("OnboardingAvailable({project_type}@{root})"),
        UiEvent::ProjectAnalysisComplete { project_type, .. } => format!("ProjectAnalysisComplete({project_type})"),
        UiEvent::ProjectConfigCreated { path } => format!("ProjectConfigCreated({path})"),
        UiEvent::ProjectHealthCalculated { score, issues, .. } => format!("ProjectHealthCalculated(score={score}, issues={})", issues.len()),
        UiEvent::ProjectConfigLoaded { path } => format!("ProjectConfigLoaded({path})"),
        UiEvent::OpenInitWizard { dry_run } => format!("OpenInitWizard(dry_run={dry_run})"),
        // Phase 95: Plugin Auto-Implantation
        UiEvent::PluginSuggestionReady { suggestions, dry_run } => format!("PluginSuggestionReady({} suggestions, dry_run={dry_run})", suggestions.len()),
        UiEvent::PluginBootstrapStarted { count, dry_run } => format!("PluginBootstrapStarted({count}, dry_run={dry_run})"),
        UiEvent::PluginBootstrapComplete { installed, skipped, failed } => format!("PluginBootstrapComplete(✓{installed} ○{skipped} ✗{failed})"),
        UiEvent::PluginStatusChanged { plugin_id, new_status } => format!("PluginStatusChanged({plugin_id}→{new_status})"),
    }
}

/// Cleanup del terminal cuando TuiApp se destruye.
/// Esto asegura que el terminal se restaure correctamente incluso si el TUI

impl Drop for TuiApp {
    fn drop(&mut self) {
        // Desactivar raw mode
        let _ = terminal::disable_raw_mode();

        // Salir de la pantalla alternativa
        let _ = io::stdout().execute(LeaveAlternateScreen);

        // Desactivar captura de mouse
        let _ = io::stdout().execute(DisableMouseCapture);

        // Restaurar mejoras de teclado
        let _ = io::stdout().execute(PopKeyboardEnhancementFlags);

        // Phase 93: Disable bracketed paste on drop (crash safety)
        let _ = io::stdout().execute(DisableBracketedPaste);

        tracing::debug!("Terminal cleanup completed");
    }

}
