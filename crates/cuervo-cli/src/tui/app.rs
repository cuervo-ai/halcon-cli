//! TUI application shell — manages the render loop and event dispatch.

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags,
    MouseButton, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use tokio::sync::mpsc;

use super::constants;
use super::conversational_overlay::ConversationalOverlay;
use super::events::{ControlEvent, UiEvent};
use super::input;
use super::layout;
use super::overlay::{self, OverlayKind};
use super::state::{AgentControl, AppState, FocusZone, UiMode};
use super::widgets::activity::ActivityState;
use super::widgets::panel::SidePanel;
use super::widgets::prompt::PromptState;
use super::widgets::status::StatusState;
use super::widgets::toast::{Toast, ToastLevel, ToastStack};

/// Maximum number of events stored in the ring buffer for the inspector.
const EVENT_RING_CAPACITY: usize = 200;

/// A timestamped event entry for the inspector ring buffer.
#[derive(Debug, Clone)]
pub struct EventEntry {
    /// Wall-clock offset from app start in milliseconds.
    pub offset_ms: u64,
    /// Summary label of the event.
    pub label: String,
}

/// The TUI application. Owns the terminal, state, and event channels.
pub struct TuiApp {
    state: AppState,
    prompt: PromptState,
    activity: ActivityState,
    status: StatusState,
    panel: SidePanel,
    /// Receives UiEvents from the agent loop (via TuiSink).
    ui_rx: mpsc::Receiver<UiEvent>,
    /// Sends prompt text to the agent loop.
    prompt_tx: mpsc::UnboundedSender<String>,
    /// Sends control events (pause/step/cancel) to the agent loop.
    ctrl_tx: mpsc::UnboundedSender<ControlEvent>,
    /// Sends permission approval/rejection to the executor's PermissionChecker.
    /// `true` = approve, `false` = reject. Dedicated channel ensures the decision
    /// reaches the executor even while the agent loop is blocked on tool execution.
    perm_tx: mpsc::UnboundedSender<bool>,
    /// Conversational permission overlay instance (Phase I-6C).
    conversational_overlay: Option<ConversationalOverlay>,
    /// Tracked area of the submit button for mouse click detection.
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
}

impl TuiApp {
    /// Create a new TUI application with the given initial UI mode.
    pub fn new(
        ui_rx: mpsc::Receiver<UiEvent>,
        prompt_tx: mpsc::UnboundedSender<String>,
        ctrl_tx: mpsc::UnboundedSender<ControlEvent>,
        perm_tx: mpsc::UnboundedSender<bool>,
    ) -> Self {
        Self::with_mode(ui_rx, prompt_tx, ctrl_tx, perm_tx, UiMode::Standard)
    }

    /// Create a new TUI application with a specific initial UI mode.
    pub fn with_mode(
        ui_rx: mpsc::Receiver<UiEvent>,
        prompt_tx: mpsc::UnboundedSender<String>,
        ctrl_tx: mpsc::UnboundedSender<ControlEvent>,
        perm_tx: mpsc::UnboundedSender<bool>,
        initial_mode: UiMode,
    ) -> Self {
        let panel_visible = matches!(initial_mode, UiMode::Standard | UiMode::Expert);
        let mut state = AppState::new();
        state.ui_mode = initial_mode;
        state.panel_visible = panel_visible;
        Self {
            state,
            prompt: PromptState::new(),
            activity: ActivityState::new(),
            status: StatusState::new(),
            panel: SidePanel::new(),
            ui_rx,
            prompt_tx,
            ctrl_tx,
            perm_tx,
            conversational_overlay: None,
            submit_button_area: Rect::default(),
            event_log: VecDeque::with_capacity(EVENT_RING_CAPACITY),
            start_time: Instant::now(),
            toasts: ToastStack::new(),
            search_matches: Vec::new(),
            search_current: 0,
            agent_started_at: None,
            max_agent_duration_secs: 600, // 10 minutes default watchdog timeout
        }
    }

    /// Push a startup banner into the activity zone.
    pub fn push_banner(
        &mut self,
        version: &str,
        provider: &str,
        provider_connected: bool,
        model: &str,
        session_id: &str,
        session_type: &str,
        routing: Option<&crate::render::banner::RoutingDisplay>,
    ) {
        self.activity.push_info("    ▄▀▀▀▄");
        self.activity.push_info("   █  ●  █▄");
        self.activity.push_info("   █     ██▀");
        self.activity.push_info("    ▀▄▄▄▀▀");
        self.activity.push_info(" ▀▀▀▀▀▀▀▀▀▀▀");
        self.activity.push_info("");
        self.activity.push_info(&format!(
            "  CUERVO v{}  —  AI-powered CLI for software development",
            version
        ));
        self.activity.push_info("  ─────────────────────────────────────────────");

        let status = if provider_connected {
            "connected"
        } else {
            "not configured"
        };
        self.activity.push_info(&format!(
            "  Provider:  {} ({})",
            provider, status
        ));
        self.activity
            .push_info(&format!("  Model:     {}", model));
        self.activity.push_info(&format!(
            "  Session:   {} ({})",
            session_id, session_type
        ));
        if let Some(r) = routing {
            if !r.fallback_chain.is_empty() {
                self.activity.push_info(&format!(
                    "  Routing:   {}: {}",
                    r.mode,
                    r.fallback_chain.join(" → ")
                ));
            }
        }
        self.activity.push_info("");
        self.activity.push_info(
            "  Ctrl+Enter = submit | Enter = newline | Tab = switch zone | Scroll = navigate | Ctrl+C = quit"
        );
        self.activity.push_info("");
    }

    /// Run the TUI render loop. Blocks until quit.
    pub async fn run(&mut self) -> io::Result<()> {
        tracing::debug!("TUI run() started");

        // Enter alternate screen + raw mode + mouse capture.
        let mut stdout = io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        tracing::debug!("Entered alternate screen");

        terminal::enable_raw_mode()?;
        tracing::debug!("Enabled raw mode");

        stdout.execute(EnableMouseCapture)?;
        tracing::debug!("Enabled mouse capture");

        // Enable keyboard enhancement to detect Cmd (SUPER) on macOS.
        let _ = stdout.execute(PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES,
        ));
        tracing::debug!("Enabled keyboard enhancements");

        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        tracing::debug!("Created terminal");

        terminal.clear()?;
        tracing::debug!("Cleared terminal, entering main loop");

        // Spawn a single dedicated thread for crossterm event polling.
        // Phase 44C: Reduced polling interval for snappier keyboard response.
        let (key_tx, mut key_rx) = mpsc::unbounded_channel::<Event>();
        std::thread::spawn(move || {
            loop {
                // 10ms polling for <50ms input latency (was 50ms).
                if event::poll(Duration::from_millis(10)).unwrap_or(false) {
                    if let Ok(ev) = event::read() {
                        if key_tx.send(ev).is_err() {
                            break; // Receiver dropped, TUI is shutting down.
                        }
                    }
                }
            }
        });

        // Spinner tick timer — 100ms interval to animate the braille spinner.
        let mut tick_interval = tokio::time::interval(Duration::from_millis(100));
        tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Phase 44C: Frame rate limiter — minimum 8ms between frames (≈120 FPS cap).
        // Increased from 60 FPS for smoother scrolling and animations.
        let min_frame_interval = Duration::from_millis(8);
        let mut last_render = Instant::now();
        let mut needs_render = true;

        tracing::debug!("TUI entering main event loop");
        let mut loop_iterations = 0;

        loop {
            loop_iterations += 1;
            if loop_iterations % 100 == 1 {
                tracing::trace!(iterations = loop_iterations, "TUI loop iteration");
            }

            // Phase F7: Skip render if within minimum frame interval (debounce burst events).
            let since_last = last_render.elapsed();
            if !needs_render && since_last < min_frame_interval {
                // Process events without rendering.
            } else {
                needs_render = false;
                last_render = Instant::now();
            }

            // Phase 44C: Auto-hide typing indicator after 2 seconds of inactivity.
            if self.state.typing_indicator
                && self.state.last_keystroke.elapsed() > Duration::from_secs(2)
            {
                self.state.typing_indicator = false;
            }

            // Watchdog: force UI unlock if agent is stuck longer than max duration.
            if let Some(started) = self.agent_started_at {
                let elapsed_secs = started.elapsed().as_secs();
                if elapsed_secs > self.max_agent_duration_secs {
                    tracing::warn!(
                        elapsed_secs,
                        max_secs = self.max_agent_duration_secs,
                        agent_running = self.state.agent_running,
                        prompts_queued = self.state.prompts_queued,
                        "WATCHDOG TRIGGERED: Agent timeout exceeded - forcing UI unlock"
                    );

                    // Force unlock all state
                    self.state.agent_running = false;
                    self.state.prompts_queued = 0;
                    self.state.spinner_active = false;
                    self.state.focus = FocusZone::Prompt;
                    self.state.agent_control = crate::tui::state::AgentControl::Running;
                    self.agent_started_at = None;

                    // Alert user
                    self.activity.push_warning(
                        &format!("Agent watchdog triggered after {} seconds - UI unlocked", elapsed_secs),
                        Some("The agent may have hung. Check logs for details.")
                    );
                    self.toasts.push(Toast::new(
                        format!("Agent timeout ({elapsed_secs}s) - UI force-unlocked"),
                        ToastLevel::Warning
                    ));
                }
            }

            // Render frame.
            terminal.draw(|frame| {
                let area = frame.area();

                // Phase F5: Graceful degradation for small terminals.
                if layout::is_too_small(area.width, area.height) {
                    let p = &crate::render::theme::active().palette;
                    let msg = Paragraph::new("Terminal too small.\nMinimum: 40x10")
                        .style(Style::default().fg(p.warning_ratatui()));
                    frame.render_widget(msg, area);
                    return;
                }

                // Mode-aware layout: Minimal/Standard/Expert with optional panels.
                // Effective mode may be downgraded for narrow terminals.
                let effective_mode = layout::effective_mode(area.width, self.state.ui_mode);
                let mode_layout = layout::calculate_mode_layout(
                    area,
                    effective_mode,
                    self.state.panel_visible,
                );

                // Split prompt zone: [textarea | submit button].
                // Adapt button width based on queue state.
                let button_width = if self.state.prompts_queued > 1 {
                    20  // Wider for "Queue (#N)"
                } else {
                    18  // Normal "Send (Ctrl+⏎)"
                };

                let prompt_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Min(1), Constraint::Length(button_width)])
                    .split(mode_layout.prompt);
                let textarea_area = prompt_chunks[0];
                let button_area = prompt_chunks[1];

                self.prompt.render(
                    frame,
                    textarea_area,
                    self.state.focus == FocusZone::Prompt,
                    self.state.typing_indicator,
                );

                // Render submit button with theme colors and queue-aware state.
                let p = &crate::render::theme::active().palette;

                let (btn_text, btn_text_style, btn_border_style, btn_title) = if self.state.prompts_queued > 0 {
                    // Agent is processing or prompts queued
                    let text = if self.state.prompts_queued == 1 {
                        "  ▶ Processing  "
                    } else {
                        "  ⏳ Queued (#N)  "  // Will be replaced below
                    };
                    (
                        text,
                        Style::default()
                            .fg(p.text_ratatui())
                            .bg(p.warning_ratatui()),
                        Style::default().fg(p.warning_ratatui()),
                        "",
                    )
                } else {
                    // Ready for new prompt
                    (
                        "  ► Send (Ctrl+⏎)  ",
                        Style::default()
                            .fg(p.bg_panel_ratatui())
                            .bg(p.success_ratatui())
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(p.success_ratatui()),
                        "",
                    )
                };
                let h = button_area.height.saturating_sub(2); // inner height
                let pad_top = if h > 2 { (h - 2) / 2 } else { 0 };
                let mut btn_lines: Vec<Line<'_>> = Vec::new();
                for _ in 0..pad_top {
                    btn_lines.push(Line::from(""));
                }

                // Replace #N with actual queue count
                let display_text = if btn_text.contains("#N") {
                    btn_text.replace("#N", &format!("#{}", self.state.prompts_queued))
                } else {
                    btn_text.to_string()
                };

                btn_lines.push(Line::from(Span::styled(display_text, btn_text_style)));
                if h > 2 && self.state.prompts_queued == 0 {
                    // Only show ⏎ hint when ready (not when busy)
                    btn_lines.push(Line::from(Span::styled("    ⏎     ", btn_text_style)));
                }
                let button = Paragraph::new(btn_lines)
                    .alignment(ratatui::layout::Alignment::Center)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(btn_title)
                            .border_style(btn_border_style),
                    );
                frame.render_widget(button, button_area);
                self.submit_button_area = button_area;

                // Render side panel if visible.
                if let Some(panel_area) = mode_layout.side_panel {
                    self.panel.render(frame, panel_area, self.state.panel_section);
                }

                // Render inspector panel in Expert mode — event log.
                if let Some(inspector_area) = mode_layout.inspector {
                    let p_theme = &crate::render::theme::active().palette;
                    let c_border_insp = p_theme.border_ratatui();
                    let c_muted_insp = p_theme.muted_ratatui();
                    let c_text_insp = p_theme.text_ratatui();

                    let inner_height = inspector_area.height.saturating_sub(2) as usize;
                    let total = self.event_log.len();
                    let skip = total.saturating_sub(inner_height);

                    let lines: Vec<Line<'_>> = self.event_log.iter()
                        .skip(skip)
                        .map(|entry| {
                            let ts = format!("{:>6}ms ", entry.offset_ms);
                            Line::from(vec![
                                Span::styled(ts, Style::default().fg(c_muted_insp)),
                                Span::styled(entry.label.clone(), Style::default().fg(c_text_insp)),
                            ])
                        })
                        .collect();

                    let inspector = Paragraph::new(lines)
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(format!(" Inspector ({total}) "))
                                .border_style(Style::default().fg(c_border_insp)),
                        );
                    frame.render_widget(inspector, inspector_area);
                }

                self.activity.render(frame, mode_layout.activity, &self.state);
                self.status.agent_control = self.state.agent_control;
                self.status.dry_run_active = self.state.dry_run_active;
                self.status.token_budget = self.state.token_budget;
                self.status.ui_mode = self.state.ui_mode;
                self.status.reasoning_strategy = self.panel.reasoning.strategy.clone();
                // Compute cache hit rate from panel metrics.
                let cache_total = self.panel.metrics.cache_hits + self.panel.metrics.cache_misses;
                self.status.cache_hit_rate = if cache_total > 0 {
                    Some((self.panel.metrics.cache_hits as f64 / cache_total as f64) * 100.0)
                } else {
                    None
                };
                self.status.render(frame, mode_layout.status);

                // Render footer with context-aware keybinding hints.
                // Use effective_mode (degraded for terminal width) not ui_mode.
                self.render_footer(frame, mode_layout.footer, effective_mode);

                // Render active overlay on top of everything.
                match &self.state.overlay.active {
                    Some(OverlayKind::Help) => {
                        overlay::render_help(frame, area);
                    }
                    Some(OverlayKind::CommandPalette) => {
                        overlay::render_command_palette(
                            frame,
                            area,
                            &self.state.overlay.input,
                            &self.state.overlay.filtered_items,
                            self.state.overlay.selected,
                        );
                    }
                    Some(OverlayKind::Search) => {
                        let match_count = self.search_matches.len();
                        let current = if match_count > 0 { self.search_current + 1 } else { 0 };
                        overlay::render_search(frame, area, &self.state.overlay.input, match_count, current);
                    }
                    Some(OverlayKind::PermissionPrompt { .. }) => {
                        // Phase I-6C: Render conversational permission overlay.
                        if let Some(ref conv_overlay) = self.conversational_overlay {
                            conv_overlay.render(area, frame.buffer_mut());
                        } else {
                            // Fallback to simple prompt (shouldn't happen).
                            overlay::render_permission_prompt(frame, area, "(unknown)");
                        }
                    }
                    None => {}
                }

                // Phase F1: Render toast notifications on top.
                self.toasts.render(frame, area);
            })?;

            // Phase F1: GC expired toasts each frame.
            self.toasts.gc();

            // Event loop: crossterm events + agent UiEvents.
            tokio::select! {
                Some(ev) = key_rx.recv() => {
                    match ev {
                        Event::Key(key) => {
                            // If overlay is active, route keys to overlay first.
                            if self.state.overlay.is_active() {
                                self.handle_overlay_key(key);
                            } else {
                                let action = input::dispatch_key(key, self.state.agent_running);
                                self.handle_action(action);
                            }
                        }
                        Event::Mouse(mouse) => {
                            match mouse.kind {
                                MouseEventKind::Down(MouseButton::Left) => {
                                    let r = self.submit_button_area;
                                    // Permitir click solo si no hay prompts en cola (consistente con la visualización del botón)
                                    if self.state.prompts_queued == 0
                                        && mouse.column >= r.x
                                        && mouse.column < r.x + r.width
                                        && mouse.row >= r.y
                                        && mouse.row < r.y + r.height
                                    {
                                        self.handle_action(input::InputAction::SubmitPrompt);
                                    }
                                }
                                MouseEventKind::ScrollUp => {
                                    self.activity.scroll_up(3);
                                }
                                MouseEventKind::ScrollDown => {
                                    self.activity.scroll_down(3);
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                Some(ev) = self.ui_rx.recv() => {
                    self.handle_ui_event(ev);
                }
                _ = tick_interval.tick() => {
                    // Advance spinner animation frame.
                    self.state.tick_spinner();
                }
            }

            if self.state.should_quit {
                tracing::debug!(iterations = loop_iterations, "TUI loop exiting: should_quit = true");
                break;
            }
        }

        tracing::debug!(iterations = loop_iterations, "TUI loop completed normally");

        // Restore terminal.
        let mut stdout = io::stdout();
        let _ = stdout.execute(PopKeyboardEnhancementFlags);
        stdout.execute(DisableMouseCapture)?;
        terminal::disable_raw_mode()?;
        stdout.execute(LeaveAlternateScreen)?;
        Ok(())
    }

    /// Render the footer bar with context-aware keybinding hints.
    ///
    /// `eff_mode` is the terminal-width-degraded mode (not the user's raw `ui_mode`).
    fn render_footer(&self, frame: &mut ratatui::Frame, area: Rect, eff_mode: UiMode) {
        use super::theme_bridge;
        use super::state::AgentControl;

        let hint_style = theme_bridge::footer_hint_style();
        let key_style = theme_bridge::footer_key_style();

        let mut spans = Vec::new();

        // Context-aware hints based on current state.
        if self.state.overlay.is_active() {
            // Overlay mode: show overlay-specific hints.
            spans.push(Span::styled(" Esc", key_style));
            spans.push(Span::styled(" close  ", hint_style));
            if matches!(self.state.overlay.active, Some(OverlayKind::PermissionPrompt { .. })) {
                spans.push(Span::styled("Y", key_style));
                spans.push(Span::styled(" approve  ", hint_style));
                spans.push(Span::styled("N", key_style));
                spans.push(Span::styled(" reject  ", hint_style));
            } else if matches!(self.state.overlay.active, Some(OverlayKind::CommandPalette)) {
                spans.push(Span::styled("↑↓", key_style));
                spans.push(Span::styled(" navigate  ", hint_style));
                spans.push(Span::styled("Enter", key_style));
                spans.push(Span::styled(" select  ", hint_style));
            } else if matches!(self.state.overlay.active, Some(OverlayKind::Search)) {
                spans.push(Span::styled("↑↓", key_style));
                spans.push(Span::styled(" prev/next  ", hint_style));
                spans.push(Span::styled("Enter", key_style));
                spans.push(Span::styled(" next  ", hint_style));
            }
        } else if self.state.agent_running {
            // Agent running mode: show pause/step/cancel hints.
            match self.state.agent_control {
                AgentControl::Paused => {
                    spans.push(Span::styled(" Space", key_style));
                    spans.push(Span::styled(" resume  ", hint_style));
                    spans.push(Span::styled("N", key_style));
                    spans.push(Span::styled(" step  ", hint_style));
                }
                AgentControl::WaitingApproval => {
                    spans.push(Span::styled(" Y", key_style));
                    spans.push(Span::styled(" approve  ", hint_style));
                    spans.push(Span::styled("Shift+N", key_style));
                    spans.push(Span::styled(" reject  ", hint_style));
                }
                _ => {
                    spans.push(Span::styled(" Space", key_style));
                    spans.push(Span::styled(" pause  ", hint_style));
                    spans.push(Span::styled("Esc", key_style));
                    spans.push(Span::styled(" cancel  ", hint_style));
                }
            }
        } else {
            // Idle mode: show prompt and navigation hints.
            spans.push(Span::styled(" Ctrl+Enter", key_style));
            spans.push(Span::styled(" send  ", hint_style));
            spans.push(Span::styled("Ctrl+P", key_style));
            spans.push(Span::styled(" commands  ", hint_style));
            spans.push(Span::styled("Ctrl+F", key_style));
            spans.push(Span::styled(" search  ", hint_style));
            spans.push(Span::styled("Tab", key_style));
            spans.push(Span::styled(" focus  ", hint_style));
        }

        // Always show mode (effective, not raw) and panel toggle.
        // Show degradation indicator if effective mode differs from user-selected mode.
        let mode_label = if eff_mode != self.state.ui_mode {
            format!(" F3:{} (→{})  ", self.state.ui_mode.label(), eff_mode.label())
        } else {
            format!(" F3:{}  ", eff_mode.label())
        };
        spans.push(Span::styled("F1", key_style));
        spans.push(Span::styled(" help  ", hint_style));
        spans.push(Span::styled("F2", key_style));
        spans.push(Span::styled(" panel  ", hint_style));
        spans.push(Span::styled(mode_label, hint_style));

        // Quit hint at end.
        spans.push(Span::styled("Ctrl+C", key_style));
        spans.push(Span::styled(" quit", hint_style));

        // Footer ellipsis: truncate spans if they exceed the available width.
        let total_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if total_width > area.width as usize {
            let mut accumulated = 0usize;
            let max = area.width as usize;
            let mut truncated = Vec::new();
            for span in &spans {
                let len = span.content.chars().count();
                if accumulated + len > max.saturating_sub(1) {
                    // Truncate this span and add ellipsis.
                    let remaining = max.saturating_sub(accumulated + 1);
                    if remaining > 0 {
                        let content: String = span.content.chars().take(remaining).collect();
                        truncated.push(Span::styled(content, span.style));
                    }
                    truncated.push(Span::styled("…", hint_style));
                    break;
                }
                truncated.push(span.clone());
                accumulated += len;
            }
            let footer_line = Line::from(truncated);
            let footer = Paragraph::new(footer_line);
            frame.render_widget(footer, area);
        } else {
            let footer_line = Line::from(spans);
            let footer = Paragraph::new(footer_line);
            frame.render_widget(footer, area);
        }
    }

    /// Handle key events when an overlay is active.
    fn handle_overlay_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        // Phase I-6C: Route permission prompt input through conversational overlay.
        if matches!(self.state.overlay.active, Some(OverlayKind::PermissionPrompt { .. })) {
            // Special case: Esc always closes.
            if matches!(key.code, KeyCode::Esc) {
                self.conversational_overlay = None;
                self.state.agent_control = AgentControl::Running;
                self.state.overlay.close();
                return;
            }

            // Route all other input through the conversational overlay.
            if let Some(ref mut conv_overlay) = self.conversational_overlay {
                let input_char = match key.code {
                    KeyCode::Enter => '\n',
                    KeyCode::Backspace => '\x7f',
                    KeyCode::Char(c) => c,
                    _ => return, // Ignore other keys
                };

                if let Some(msg) = conv_overlay.handle_input(input_char) {
                    // Terminal state reached — convert to boolean and send.
                    use crate::repl::conversation_protocol::PermissionMessage;
                    let approved = matches!(msg, PermissionMessage::Approve);
                    let _ = self.perm_tx.send(approved);

                    let status_msg = if approved {
                        "[control] Action approved"
                    } else {
                        "[control] Action rejected"
                    };
                    if approved {
                        self.activity.push_info(status_msg);
                    } else {
                        self.activity.push_warning(status_msg, None);
                    }

                    self.conversational_overlay = None;
                    self.state.agent_control = AgentControl::Running;
                    self.state.overlay.close();
                }
            }
            return;
        }

        // Non-permission overlays: use original logic.
        match key.code {
            KeyCode::Esc => {
                if matches!(self.state.overlay.active, Some(OverlayKind::Search)) {
                    self.search_matches.clear();
                    self.search_current = 0;
                }
                self.state.overlay.close();
            }
            KeyCode::Enter => {
                match &self.state.overlay.active {
                    Some(OverlayKind::CommandPalette) => {
                        let action = self.state.overlay.filtered_items
                            .get(self.state.overlay.selected)
                            .map(|item| item.action.clone());
                        self.state.overlay.close();
                        if let Some(cmd) = action {
                            self.execute_slash_command(&cmd);
                        }
                    }
                    Some(OverlayKind::Search) => {
                        // Enter = jump to next match.
                        self.search_next();
                    }
                    _ => {
                        self.state.overlay.close();
                    }
                }
            }
            KeyCode::Up => {
                if matches!(self.state.overlay.active, Some(OverlayKind::Search)) {
                    self.search_prev();
                } else {
                    self.state.overlay.select_prev();
                }
            }
            KeyCode::Down => {
                if matches!(self.state.overlay.active, Some(OverlayKind::Search)) {
                    self.search_next();
                } else {
                    let max = self.state.overlay.filtered_items.len();
                    self.state.overlay.select_next(max);
                }
            }
            KeyCode::Backspace => {
                self.state.overlay.backspace();
                self.refilter_palette();
                self.rerun_search();
            }
            KeyCode::Char(c) => {
                // All character input for other overlays.
                self.state.overlay.type_char(c);
                self.refilter_palette();
                self.rerun_search();
            }
            _ => {}
        }
    }

    /// Re-run search against activity lines (incremental search on keystroke).
    fn rerun_search(&mut self) {
        if matches!(self.state.overlay.active, Some(OverlayKind::Search)) {
            let query = self.state.overlay.input.clone();
            self.search_matches = self.activity.search(&query);
            self.search_current = 0;
            // Jump to first match if any.
            if let Some(&line_idx) = self.search_matches.first() {
                self.activity.scroll_to_line(line_idx);
            }
        }
    }

    /// Navigate to the next search match.
    fn search_next(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_current = (self.search_current + 1) % self.search_matches.len();
        let line_idx = self.search_matches[self.search_current];
        self.activity.scroll_to_line(line_idx);
    }

    /// Navigate to the previous search match.
    fn search_prev(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        if self.search_current == 0 {
            self.search_current = self.search_matches.len() - 1;
        } else {
            self.search_current -= 1;
        }
        let line_idx = self.search_matches[self.search_current];
        self.activity.scroll_to_line(line_idx);
    }

    /// Execute a slash command by action name.
    fn execute_slash_command(&mut self, cmd: &str) {
        match cmd {
            "help" => {
                self.state.overlay.open(OverlayKind::Help);
            }
            "model" => {
                // Show current model/provider info.
                let provider = if self.status.current_provider().is_empty() {
                    "(none)"
                } else {
                    self.status.current_provider()
                };
                let model = if self.status.current_model().is_empty() {
                    "(none)"
                } else {
                    self.status.current_model()
                };
                self.activity.push_info(&format!(
                    "[model] Current: {provider}/{model}  —  Use config file to change provider/model"
                ));
            }
            "mode" => {
                self.handle_action(input::InputAction::CycleUiMode);
            }
            "plan" => {
                // Toggle plan section in side panel, auto-show panel if hidden.
                self.state.panel_visible = true;
                self.state.panel_section = crate::tui::state::PanelSection::Plan;
                self.activity.push_info("[plan] Side panel switched to Plan view");
            }
            "panel" => {
                self.state.panel_visible = !self.state.panel_visible;
            }
            "clear" => {
                self.activity.clear();
            }
            "quit" => {
                self.state.should_quit = true;
            }
            "search" => {
                self.state.overlay.open(OverlayKind::Search);
            }
            other => {
                self.activity.push_warning(
                    &format!("[cmd] Unknown command: /{other}"),
                    Some("Type Ctrl+P to see available commands"),
                );
            }
        }
    }

    /// Re-filter the command palette items based on current overlay input.
    fn refilter_palette(&mut self) {
        if matches!(self.state.overlay.active, Some(OverlayKind::CommandPalette)) {
            let all = overlay::default_commands();
            self.state.overlay.filtered_items =
                overlay::filter_commands(&all, &self.state.overlay.input);
            // Clamp selection to valid range.
            let max = self.state.overlay.filtered_items.len();
            if self.state.overlay.selected >= max {
                self.state.overlay.selected = max.saturating_sub(1);
            }
        }
    }

    fn handle_action(&mut self, action: input::InputAction) {
        match action {
            input::InputAction::SubmitPrompt => {
                let text = self.prompt.take_text();
                if text.trim().is_empty() {
                    return;
                }
                // Phase E7: Intercept slash commands before sending to agent.
                let trimmed = text.trim();
                if trimmed == "/" {
                    // Bare "/" opens the command palette instead of sending to agent.
                    self.state.overlay.open(OverlayKind::CommandPalette);
                    self.state.overlay.filtered_items = overlay::default_commands();
                    return;
                }
                if trimmed.starts_with('/') {
                    let cmd = trimmed.trim_start_matches('/').split_whitespace().next().unwrap_or("");
                    self.activity.push_user_prompt(&text);
                    self.execute_slash_command(cmd);
                    return;
                }
                // Phase 44B: Allow queueing prompts even when agent is running.
                self.activity.push_user_prompt(&text);

                // Queue the prompt (unbounded channel never blocks).
                if let Err(e) = self.prompt_tx.send(text) {
                    self.activity.push_error(&format!("Failed to queue prompt: {e}"), None);
                    return;
                }

                // Optimistically increment queue count (will be corrected by events).
                self.state.prompts_queued += 1;

                // If agent already running, show toast that prompt was queued.
                if self.state.agent_running {
                    self.toasts.push(Toast::new(
                        format!("Prompt #{} queued", self.state.prompts_queued),
                        ToastLevel::Info
                    ));
                } else {
                    // First prompt, start agent.
                    self.state.agent_running = true;
                    self.state.focus = FocusZone::Activity;
                }

                // Logging de estado para debugging
                tracing::debug!(
                    agent_running = self.state.agent_running,
                    prompts_queued = self.state.prompts_queued,
                    agent_control = ?self.state.agent_control,
                    focus = ?self.state.focus,
                    "Prompt submitted to queue"
                );
            }
            input::InputAction::ClearPrompt => {
                self.prompt.clear();
            }
            input::InputAction::HistoryBack => {
                self.prompt.history_back();
            }
            input::InputAction::HistoryForward => {
                self.prompt.history_forward();
            }
            input::InputAction::CancelAgent => {
                // Signal cancellation (handled externally via Ctrl+C signal).
                self.state.agent_running = false;
                self.state.spinner_active = false;
                self.activity.push_warning("Agent cancelled by user", None);
            }
            input::InputAction::Quit => {
                self.state.should_quit = true;
            }
            input::InputAction::CycleFocus => {
                self.state.cycle_focus();
            }
            input::InputAction::ScrollUp => {
                self.activity.scroll_up(3);
            }
            input::InputAction::ScrollDown => {
                self.activity.scroll_down(3);
            }
            input::InputAction::ScrollToBottom => {
                self.activity.scroll_to_bottom();
            }
            input::InputAction::TogglePanel => {
                self.state.panel_visible = !self.state.panel_visible;
            }
            input::InputAction::CyclePanelSection => {
                self.state.panel_section = self.state.panel_section.next();
            }
            input::InputAction::CycleUiMode => {
                self.state.ui_mode = self.state.ui_mode.next();
                // Auto-show/hide panel based on mode.
                match self.state.ui_mode {
                    crate::tui::state::UiMode::Minimal => {
                        self.state.panel_visible = false;
                    }
                    crate::tui::state::UiMode::Standard
                    | crate::tui::state::UiMode::Expert => {
                        self.state.panel_visible = true;
                    }
                }
            }
            input::InputAction::PauseAgent => {
                use crate::tui::state::AgentControl;
                if self.state.agent_control == AgentControl::Paused {
                    self.state.agent_control = AgentControl::Running;
                    let _ = self.ctrl_tx.send(ControlEvent::Resume);
                    self.activity.push_info("[control] Resumed");
                } else {
                    self.state.agent_control = AgentControl::Paused;
                    let _ = self.ctrl_tx.send(ControlEvent::Pause);
                    self.activity.push_info("[control] Paused — Space to resume, N to step");
                }
            }
            input::InputAction::StepAgent => {
                use crate::tui::state::AgentControl;
                self.state.agent_control = AgentControl::StepMode;
                let _ = self.ctrl_tx.send(ControlEvent::Step);
                self.activity.push_info("[control] Step mode — executing one step");
            }
            input::InputAction::ApproveAction => {
                let _ = self.perm_tx.send(true);
                self.activity.push_info("[control] Action approved");
            }
            input::InputAction::RejectAction => {
                let _ = self.perm_tx.send(false);
                self.activity.push_warning("[control] Action rejected", None);
            }
            input::InputAction::OpenHelp => {
                self.state.overlay.open(OverlayKind::Help);
            }
            input::InputAction::OpenCommandPalette => {
                self.state.overlay.open(OverlayKind::CommandPalette);
                self.state.overlay.filtered_items = overlay::default_commands();
            }
            input::InputAction::OpenSearch => {
                self.state.overlay.open(OverlayKind::Search);
            }
            input::InputAction::DismissToasts => {
                self.toasts.dismiss_all();
            }
            input::InputAction::ForwardToWidget(key) => {
                match self.state.focus {
                    FocusZone::Prompt => {
                        self.prompt.handle_key(key);
                        // Phase 44C: Track typing activity for indicator.
                        self.state.typing_indicator = true;
                        self.state.last_keystroke = std::time::Instant::now();
                    }
                    FocusZone::Activity => {
                        // Arrow keys scroll the activity zone.
                        use crossterm::event::KeyCode;
                        match key.code {
                            KeyCode::Up => self.activity.scroll_up(1),
                            KeyCode::Down => self.activity.scroll_down(1),
                            _ => {} // Other keys ignored in activity zone.
                        }
                    }
                }
            }
        }
    }

    /// Push an event summary into the ring buffer for inspector display.
    fn log_event(&mut self, label: String) {
        let offset_ms = self.start_time.elapsed().as_millis() as u64;
        if self.event_log.len() >= EVENT_RING_CAPACITY {
            self.event_log.pop_front();
        }
        self.event_log.push_back(EventEntry { offset_ms, label });
    }

    /// Get the event log entries (for inspector rendering).
    #[allow(dead_code)]
    pub fn event_log(&self) -> &VecDeque<EventEntry> {
        &self.event_log
    }

    fn handle_ui_event(&mut self, ev: UiEvent) {
        // Log every event to the ring buffer for inspector.
        self.log_event(event_summary(&ev));

        match ev {
            UiEvent::StreamChunk(text) => {
                self.activity.push_assistant_text(&text);
            }
            UiEvent::StreamCodeBlock { lang, code } => {
                self.activity.push_code_block(&lang, &code);
            }
            UiEvent::StreamToolMarker(name) => {
                self.activity.push_info(&format!("[tool: {name}]"));
            }
            UiEvent::StreamDone => {
                // Streaming complete for this round — no UI action needed.
                // The round transition is handled by RoundEnded.
                tracing::trace!("StreamDone received");
            }
            UiEvent::StreamError(msg) => {
                self.activity.push_error(&msg, None);
                self.toasts.push(Toast::new("Stream error", ToastLevel::Error));
            }
            UiEvent::ToolStart { name, input } => {
                // Build a short input preview from the JSON value.
                let input_preview = format_input_preview(&input);
                self.activity.push_tool_start(&name, &input_preview);
                self.panel.metrics.tool_count += 1;
            }
            UiEvent::ToolOutput { name, content, is_error, duration_ms } => {
                self.activity.complete_tool(&name, content, is_error, duration_ms);
            }
            UiEvent::ToolDenied(name) => {
                self.activity.push_warning(&format!("Tool denied: {name}"), None);
                self.toasts.push(Toast::new(format!("Denied: {name}"), ToastLevel::Warning));
            }
            UiEvent::SpinnerStart(label) => {
                self.state.spinner_active = true;
                self.state.spinner_label = label;
            }
            UiEvent::SpinnerStop => {
                self.state.spinner_active = false;
            }
            UiEvent::Warning { message, hint } => {
                self.activity.push_warning(&message, hint.as_deref());
            }
            UiEvent::Error { message, hint } => {
                self.activity.push_error(&message, hint.as_deref());
                self.toasts.push(Toast::new(
                    if message.len() > 40 { format!("{}...", &message[..37]) } else { message.clone() },
                    ToastLevel::Error,
                ));
            }
            UiEvent::Info(msg) => {
                self.activity.push_info(&msg);
            }
            UiEvent::StatusUpdate {
                provider, model, round, tokens, cost,
                session_id, elapsed_ms, tool_count, input_tokens, output_tokens,
            } => {
                self.status.update(
                    provider, model, round, tokens, cost,
                    session_id, elapsed_ms, tool_count, input_tokens, output_tokens,
                );
            }
            UiEvent::RoundStart(n) => {
                self.activity.push_round_separator(n);
            }
            UiEvent::RoundEnd(_n) => {
                // Legacy round end — superseded by RoundEnded with metrics.
                tracing::trace!(round = _n, "RoundEnd (legacy) received");
            }
            UiEvent::Redraw => {
                // Force redraw — the next frame will pick up any pending changes.
                tracing::trace!("Redraw requested");
            }
            // Phase 44B: Continuous interaction events
            UiEvent::AgentStartedPrompt => {
                // Agent dequeued a prompt and started processing.
                // Decrement queue count (will be corrected by PromptQueueStatus).
                self.state.prompts_queued = self.state.prompts_queued.saturating_sub(1);
                self.state.agent_running = true;

                // Start watchdog timer to prevent permanent UI freeze
                self.agent_started_at = Some(Instant::now());

                tracing::debug!(
                    agent_running = self.state.agent_running,
                    prompts_queued = self.state.prompts_queued,
                    watchdog_started = true,
                    "Agent dequeued and started processing prompt"
                );
            }
            UiEvent::AgentFinishedPrompt => {
                // Agent finished processing one prompt.
                // Decrementar inmediatamente si la cola está vacía para evitar desincronización.
                // PromptQueueStatus proporcionará la cuenta autoritativa después.
                if self.state.prompts_queued > 0 {
                    self.state.prompts_queued -= 1;
                }
                tracing::debug!(
                    prompts_queued = self.state.prompts_queued,
                    "Agent finished processing prompt"
                );
            }
            UiEvent::PromptQueueStatus(count) => {
                // Authoritative queue count from the agent loop.
                self.state.prompts_queued = count;
                tracing::debug!(queued = count, "Prompt queue status updated");
            }
            UiEvent::AgentDone => {
                // Capture state BEFORE changes for debugging
                let before_agent_running = self.state.agent_running;
                let before_prompts_queued = self.state.prompts_queued;
                let watchdog_elapsed = self.agent_started_at.map(|t| t.elapsed().as_secs());

                tracing::debug!(
                    before_agent_running,
                    before_prompts_queued,
                    watchdog_elapsed_secs = ?watchdog_elapsed,
                    "AgentDone event received - transitioning to idle state"
                );

                // Apply state transitions
                self.state.agent_running = false;
                self.state.spinner_active = false;
                self.state.focus = FocusZone::Prompt;
                self.state.agent_control = crate::tui::state::AgentControl::Running;

                // Clear watchdog timer
                self.agent_started_at = None;

                // Validation: warn if prompts still queued (expected if user queued during processing)
                if self.state.prompts_queued > 0 {
                    tracing::info!(
                        prompts_queued = self.state.prompts_queued,
                        "AgentDone: prompts still queued - agent will process next prompt"
                    );
                } else {
                    // Only show completion toast if queue is empty
                    self.toasts.push(Toast::new("Agent completed", ToastLevel::Success));
                }

                // Log final state AFTER changes
                tracing::debug!(
                    after_agent_running = self.state.agent_running,
                    after_prompts_queued = self.state.prompts_queued,
                    agent_control = ?self.state.agent_control,
                    focus = ?self.state.focus,
                    watchdog_cleared = true,
                    "AgentDone: state transition complete - UI ready for input"
                );
            }
            UiEvent::Quit => {
                self.state.should_quit = true;
            }
            UiEvent::PlanProgress { goal, steps, current_step, .. } => {
                self.activity.set_plan_overview(&goal, steps.clone(), current_step);
                self.panel.update_plan(steps.clone(), current_step);
                // Update status bar plan step indicator.
                if current_step < steps.len() {
                    let desc = &steps[current_step].description;
                    let truncated = if desc.len() > 30 {
                        format!("{}...", &desc[..27])
                    } else {
                        desc.clone()
                    };
                    self.status.plan_step = Some(format!(
                        "Step {}/{}: {truncated}",
                        current_step + 1,
                        steps.len()
                    ));
                } else {
                    self.status.plan_step = Some("Plan complete".into());
                }
            }

            // --- Phase 42B: Cockpit feedback event handlers ---
            UiEvent::RoundStarted { round, provider, model } => {
                self.activity.push_round_separator(round);
                self.status.update(
                    Some(provider), Some(model), Some(round),
                    None, None, None, None, None, None, None,
                );
            }
            UiEvent::RoundEnded { round, input_tokens, output_tokens, cost, duration_ms } => {
                self.status.update(
                    None, None, None, None, Some(cost),
                    None, Some(duration_ms), None,
                    Some(input_tokens), Some(output_tokens),
                );
                self.panel.update_metrics(round, input_tokens, output_tokens, cost, duration_ms);
            }
            UiEvent::ModelSelected { model, provider, reason } => {
                self.activity.push_info(&format!("[model] {provider}/{model} — {reason}"));
                self.toasts.push(Toast::new(
                    format!("Model: {provider}/{model}"),
                    ToastLevel::Info,
                ));
            }
            UiEvent::ProviderFallback { from, to, reason } => {
                self.activity.push_warning(&format!("Fallback: {from} → {to} — {reason}"), None);
                self.toasts.push(Toast::new(format!("{from} → {to}"), ToastLevel::Warning));
            }
            UiEvent::LoopGuardAction { action, reason } => {
                self.activity.push_warning(&format!("[guard] {action}: {reason}"), None);
            }
            UiEvent::CompactionComplete { old_msgs, new_msgs, tokens_saved } => {
                self.activity.push_info(&format!(
                    "[compaction] {old_msgs} → {new_msgs} messages ({tokens_saved} tokens saved)"
                ));
            }
            UiEvent::CacheStatus { hit, source } => {
                let label = if hit { "hit" } else { "miss" };
                self.activity.push_info(&format!("[cache {label}] {source}"));
                self.panel.record_cache(hit);
            }
            UiEvent::SpeculativeResult { tool, hit } => {
                let label = if hit { "hit" } else { "miss" };
                self.activity.push_info(&format!("[speculative {label}] {tool}"));
            }
            UiEvent::PermissionAwaiting { tool, args, risk_level } => {
                self.activity.push_info(&format!("[permission] awaiting approval for {tool}"));
                self.state.agent_control = crate::tui::state::AgentControl::WaitingApproval;

                // Phase I-6C: Create conversational overlay instance.
                let risk = match risk_level.as_str() {
                    "High" => crate::repl::adaptive_prompt::RiskLevel::High,
                    "Medium" => crate::repl::adaptive_prompt::RiskLevel::Medium,
                    _ => crate::repl::adaptive_prompt::RiskLevel::Low,
                };
                self.conversational_overlay = Some(ConversationalOverlay::new(&tool, args.clone(), risk));

                self.state.overlay.open(OverlayKind::PermissionPrompt { tool: tool.clone() });
                self.toasts.push(Toast::new(
                    format!("Approval needed: {tool}"),
                    ToastLevel::Warning,
                ));
            }
            // Phase 43C: Feedback completeness events.
            UiEvent::ReflectionStarted => {
                self.activity.push_info("[reflecting] analyzing round outcome...");
            }
            UiEvent::ReflectionComplete { analysis, score } => {
                let preview = if analysis.len() > 80 { &analysis[..80] } else { &analysis };
                self.activity.push_info(&format!("[reflection] {preview} (score: {score:.2})"));
            }
            UiEvent::ConsolidationStatus { action } => {
                self.activity.push_info(&format!("[memory] {action}"));
            }
            UiEvent::ConsolidationComplete { merged, pruned, duration_ms } => {
                let duration_s = duration_ms as f64 / 1000.0;
                self.activity.push_info(&format!(
                    "[memory] consolidation complete: merged={merged}, pruned={pruned}, {duration_s:.2}s"
                ));
                tracing::debug!(
                    merged,
                    pruned,
                    duration_ms,
                    "Memory consolidation completed successfully"
                );
            }
            UiEvent::ToolRetrying { tool, attempt, max_attempts, delay_ms } => {
                self.activity.push_warning(
                    &format!("[retry] {tool} attempt {attempt}/{max_attempts} in {delay_ms}ms"),
                    None,
                );
                self.toasts.push(Toast::new(
                    format!("Retrying {tool} ({attempt}/{max_attempts})"),
                    ToastLevel::Warning,
                ));
            }

            // Phase 43D: Live panel data
            UiEvent::ContextTierUpdate {
                l0_tokens, l0_capacity, l1_tokens, l1_entries,
                l2_entries, l3_entries, l4_entries, total_tokens,
            } => {
                self.panel.update_context(
                    l0_tokens, l0_capacity, l1_tokens, l1_entries,
                    l2_entries, l3_entries, l4_entries, total_tokens,
                );
            }
            UiEvent::ReasoningUpdate { strategy, task_type, complexity } => {
                self.panel.update_reasoning(strategy, task_type, complexity);
            }

            // Phase 44A: Observability events
            UiEvent::DryRunActive(active) => {
                self.state.dry_run_active = active;
                if active {
                    self.activity.push_warning(
                        constants::DRY_RUN_WARNING,
                        Some(constants::DRY_RUN_HINT),
                    );
                    self.toasts.push(Toast::new(constants::DRY_RUN_TOAST, ToastLevel::Warning));
                }
            }
            UiEvent::TokenBudgetUpdate { used, limit, rate_per_minute } => {
                self.state.token_budget.used = used;
                self.state.token_budget.limit = limit;
                self.state.token_budget.rate_per_minute = rate_per_minute;
            }
            UiEvent::ProviderHealthUpdate { provider, status } => {
                let label = match &status {
                    crate::tui::events::ProviderHealthStatus::Healthy => "healthy".to_string(),
                    crate::tui::events::ProviderHealthStatus::Degraded { failure_rate, .. } => {
                        format!("degraded (fail:{:.0}%)", failure_rate * 100.0)
                    }
                    crate::tui::events::ProviderHealthStatus::Unhealthy { reason } => {
                        format!("unhealthy: {reason}")
                    }
                };
                self.activity.push_info(&format!("[health] {provider}: {label}"));
                // Update status bar health indicator for the active provider.
                if provider == self.status.current_provider() {
                    self.status.provider_health = status;
                }
            }

            // Phase B4: Circuit breaker state
            UiEvent::CircuitBreakerUpdate { provider, state, failure_count } => {
                let label = match &state {
                    crate::tui::events::CircuitBreakerState::Closed => "closed",
                    crate::tui::events::CircuitBreakerState::Open => "OPEN",
                    crate::tui::events::CircuitBreakerState::HalfOpen => "half-open",
                };
                self.activity.push_info(&format!(
                    "[breaker] {provider}: {label} (failures: {failure_count})"
                ));
                self.panel.update_breaker(provider.clone(), state.clone(), failure_count);
                if matches!(state, crate::tui::events::CircuitBreakerState::Open) {
                    self.toasts.push(Toast::new(
                        format!("Breaker OPEN: {provider}"),
                        ToastLevel::Error,
                    ));
                }
            }

            // Phase B5: Agent state transition
            UiEvent::AgentStateTransition { from, to, reason } => {
                // FSM transition validation.
                if !from.can_transition_to(&to) {
                    self.activity.push_warning(
                        &format!("[state] INVALID: {:?} → {:?}: {reason}", from, to),
                        Some("This transition is not expected by the FSM"),
                    );
                    tracing::warn!(
                        from = ?from, to = ?to, reason = %reason,
                        "Invalid agent state transition"
                    );
                } else {
                    self.activity.push_info(&format!(
                        "[state] {:?} → {:?}: {reason}", from, to
                    ));
                }
                // Persist in AppState.
                self.state.agent_state = to.clone();
                // Toast for failure transitions.
                if matches!(to, crate::tui::events::AgentState::Failed) {
                    self.toasts.push(Toast::new(
                        format!("Agent failed: {reason}"),
                        ToastLevel::Error,
                    ));
                }
            }

            // Sprint 1 B2: Task status (parity with ClassicSink)
            UiEvent::TaskStatus { title, status, duration_ms, artifact_count } => {
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
                self.activity.push_info(&format!("[task] {title} — {status}{suffix}"));
            }

            // Sprint 1 B3: Reasoning status (parity with ClassicSink)
            UiEvent::ReasoningStatus { task_type, complexity, strategy, score, success } => {
                let outcome = if success { "Success" } else { "Below threshold" };
                self.activity.push_info(&format!("[reasoning] {task_type} ({complexity}) → {strategy}"));
                self.activity.push_info(&format!("[evaluation] Score: {score:.2} — {outcome}"));
            }
        }
    }
}

/// Format a short preview string from a tool's input JSON value.
fn format_input_preview(input: &serde_json::Value) -> String {
    match input {
        serde_json::Value::Object(map) => {
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in map.iter().take(3) {
                let val = match v {
                    serde_json::Value::String(s) => {
                        if s.len() > 40 {
                            format!("{}...", &s[..37])
                        } else {
                            s.clone()
                        }
                    }
                    other => {
                        let s = other.to_string();
                        if s.len() > 40 {
                            format!("{}...", &s[..37])
                        } else {
                            s
                        }
                    }
                };
                parts.push(format!("{k}={val}"));
            }
            if map.len() > 3 {
                parts.push(format!("+{} more", map.len() - 3));
            }
            parts.join(", ")
        }
        serde_json::Value::String(s) => {
            if s.len() > 60 {
                format!("{}...", &s[..57])
            } else {
                s.clone()
            }
        }
        other => {
            let s = other.to_string();
            if s.len() > 60 {
                format!("{}...", &s[..57])
            } else {
                s
            }
        }
    }
}

/// Generate a one-line summary label for an event (for the ring buffer).
fn event_summary(ev: &UiEvent) -> String {
    match ev {
        UiEvent::StreamChunk(_) => constants::EVENT_STREAM_CHUNK.into(),
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
        UiEvent::DryRunActive(a) => format!("DryRun({a})"),
        UiEvent::TokenBudgetUpdate { .. } => constants::EVENT_TOKEN_BUDGET.into(),
        UiEvent::ProviderHealthUpdate { provider, .. } => format!("Health({provider})"),
        UiEvent::CircuitBreakerUpdate { provider, .. } => format!("Breaker({provider})"),
        UiEvent::AgentStateTransition { from, to, .. } => format!("State({from:?}→{to:?})"),
        UiEvent::TaskStatus { ref title, ref status, .. } => format!("TaskStatus({title},{status})"),
        UiEvent::ReasoningStatus { ref task_type, .. } => format!("Reasoning({task_type})"),
    }
}

/// Cleanup del terminal cuando TuiApp se destruye.
/// Esto asegura que el terminal se restaure correctamente incluso si el TUI
/// se cierra abruptamente (panic, Ctrl+C, etc.).
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

        tracing::debug!("Terminal cleanup completed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper struct to keep channel receivers alive during tests
    struct TestAppContext {
        app: TuiApp,
        #[allow(dead_code)]
        prompt_rx: mpsc::UnboundedReceiver<String>,
        #[allow(dead_code)]
        ctrl_rx: mpsc::UnboundedReceiver<ControlEvent>,
        #[allow(dead_code)]
        perm_rx: mpsc::UnboundedReceiver<bool>,
    }

    impl std::ops::Deref for TestAppContext {
        type Target = TuiApp;
        fn deref(&self) -> &Self::Target {
            &self.app
        }
    }

    impl std::ops::DerefMut for TestAppContext {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.app
        }
    }

    fn test_app() -> TestAppContext {
        let (_tx, rx) = mpsc::channel(1024);
        let (prompt_tx, prompt_rx) = mpsc::unbounded_channel();
        let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
        let (perm_tx, perm_rx) = mpsc::unbounded_channel();
        TestAppContext {
            app: TuiApp::new(rx, prompt_tx, ctrl_tx, perm_tx),
            prompt_rx,
            ctrl_rx,
            perm_rx,
        }
    }

    #[test]
    fn app_initial_state() {
        let app = test_app();
        assert!(!app.state.agent_running);
        assert!(!app.state.should_quit);
        assert_eq!(app.state.focus, FocusZone::Prompt);
    }

    #[test]
    fn app_with_expert_mode() {
        let (_tx, rx) = mpsc::channel(1024);
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        let (ctrl_tx, _ctrl_rx) = mpsc::unbounded_channel();
        let (perm_tx, _perm_rx) = mpsc::unbounded_channel();
        let app = TuiApp::with_mode(rx, prompt_tx, ctrl_tx, perm_tx, UiMode::Expert);
        assert_eq!(app.state.ui_mode, UiMode::Expert);
        assert!(app.state.panel_visible);
    }

    #[test]
    fn app_with_minimal_mode() {
        let (_tx, rx) = mpsc::channel(1024);
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        let (ctrl_tx, _ctrl_rx) = mpsc::unbounded_channel();
        let (perm_tx, _perm_rx) = mpsc::unbounded_channel();
        let app = TuiApp::with_mode(rx, prompt_tx, ctrl_tx, perm_tx, UiMode::Minimal);
        assert_eq!(app.state.ui_mode, UiMode::Minimal);
        assert!(!app.state.panel_visible);
    }

    #[test]
    fn handle_quit_action() {
        let mut app = test_app();
        app.handle_action(input::InputAction::Quit);
        assert!(app.state.should_quit);
    }

    #[test]
    fn handle_cycle_focus() {
        let mut app = test_app();
        assert_eq!(app.state.focus, FocusZone::Prompt);
        app.handle_action(input::InputAction::CycleFocus);
        assert_eq!(app.state.focus, FocusZone::Activity);
        app.handle_action(input::InputAction::CycleFocus);
        assert_eq!(app.state.focus, FocusZone::Prompt);
    }

    #[test]
    fn handle_agent_done_event() {
        let mut app = test_app();
        app.state.agent_running = true;
        app.state.focus = FocusZone::Activity;
        app.handle_ui_event(UiEvent::AgentDone);
        assert!(!app.state.agent_running);
        assert_eq!(app.state.focus, FocusZone::Prompt);
    }

    #[test]
    fn handle_spinner_events() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::SpinnerStart("Thinking...".into()));
        assert!(app.state.spinner_active);
        assert_eq!(app.state.spinner_label, "Thinking...");
        app.handle_ui_event(UiEvent::SpinnerStop);
        assert!(!app.state.spinner_active);
    }

    #[test]
    fn cancel_agent_action() {
        let mut app = test_app();
        app.state.agent_running = true;
        app.handle_action(input::InputAction::CancelAgent);
        assert!(!app.state.agent_running);
    }

    #[test]
    fn empty_submit_rejected() {
        let mut app = test_app();
        app.handle_action(input::InputAction::SubmitPrompt);
        // Should not start agent on empty prompt.
        assert!(!app.state.agent_running);
    }

    #[test]
    fn submit_button_area_default_is_zero() {
        let app = test_app();
        assert_eq!(app.submit_button_area, Rect::default());
    }

    #[test]
    fn handle_info_event() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::Info("round separator".into()));
        assert!(app.activity.line_count() > 0);
    }

    #[test]
    fn push_banner_adds_lines() {
        let mut app = test_app();
        app.push_banner("0.1.0", "deepseek", true, "deepseek-chat", "abc12345", "new", None);
        // Banner should populate the activity zone with multiple lines.
        assert!(app.activity.line_count() > 5);
    }

    #[test]
    fn push_banner_with_routing_shows_chain() {
        use crate::render::banner::RoutingDisplay;
        let mut app = test_app();
        let routing = RoutingDisplay {
            mode: "failover".into(),
            strategy: "balanced".into(),
            fallback_chain: vec![
                "anthropic".into(),
                "deepseek".into(),
                "ollama".into(),
            ],
        };
        let before = app.activity.line_count();
        app.push_banner(
            "0.1.0", "anthropic", true, "claude-sonnet",
            "abc12345", "new", Some(&routing),
        );
        let after = app.activity.line_count();
        // Should have at least one more line than without routing.
        assert!(after > before + 5);
    }

    #[test]
    fn tool_start_event_creates_tool_exec() {
        let mut app = test_app();
        let input = serde_json::json!({"path": "src/main.rs"});
        app.handle_ui_event(UiEvent::ToolStart {
            name: "file_read".into(),
            input,
        });
        assert_eq!(app.activity.line_count(), 1);
        assert!(app.activity.has_loading_tools());
    }

    #[test]
    fn tool_output_event_completes_tool() {
        let mut app = test_app();
        let input = serde_json::json!({"command": "ls"});
        app.handle_ui_event(UiEvent::ToolStart {
            name: "bash".into(),
            input,
        });
        assert!(app.activity.has_loading_tools());
        app.handle_ui_event(UiEvent::ToolOutput {
            name: "bash".into(),
            content: "file1\nfile2".into(),
            is_error: false,
            duration_ms: 42,
        });
        assert!(!app.activity.has_loading_tools());
    }

    #[test]
    fn format_input_preview_object() {
        let val = serde_json::json!({"path": "src/main.rs", "line": 10});
        let preview = super::format_input_preview(&val);
        assert!(preview.contains("path=src/main.rs"));
        assert!(preview.contains("line=10"));
    }

    #[test]
    fn format_input_preview_string() {
        let val = serde_json::Value::String("hello world".into());
        let preview = super::format_input_preview(&val);
        assert_eq!(preview, "hello world");
    }

    #[test]
    fn format_input_preview_truncates_long_values() {
        let long_val = "a".repeat(100);
        let val = serde_json::json!({"data": long_val});
        let preview = super::format_input_preview(&val);
        assert!(preview.contains("..."));
        assert!(preview.len() < 100);
    }

    #[test]
    fn arrow_keys_scroll_in_activity_zone() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut app = test_app();
        app.state.focus = FocusZone::Activity;
        // Add enough content to have scroll range.
        for i in 0..50 {
            app.activity.push_info(&format!("line {i}"));
        }
        app.activity.last_max_scroll = 40; // Simulate render having computed this.
        let up_key = KeyEvent {
            code: KeyCode::Up,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        app.handle_action(input::InputAction::ForwardToWidget(up_key));
        assert!(!app.activity.auto_scroll);
    }

    #[test]
    fn cycle_ui_mode_updates_state_and_panel() {
        use crate::tui::state::UiMode;
        let mut app = test_app();
        assert_eq!(app.state.ui_mode, UiMode::Standard);
        assert!(app.state.panel_visible); // Standard starts with panel

        // Standard → Expert: panel stays shown
        app.handle_action(input::InputAction::CycleUiMode);
        assert_eq!(app.state.ui_mode, UiMode::Expert);
        assert!(app.state.panel_visible);

        // Expert → Minimal: panel hidden
        app.handle_action(input::InputAction::CycleUiMode);
        assert_eq!(app.state.ui_mode, UiMode::Minimal);
        assert!(!app.state.panel_visible);

        // Minimal → Standard: panel shown
        app.handle_action(input::InputAction::CycleUiMode);
        assert_eq!(app.state.ui_mode, UiMode::Standard);
        assert!(app.state.panel_visible);
    }

    #[test]
    fn pause_agent_sends_control_event() {
        use crate::tui::state::AgentControl;
        let (_tx, rx) = mpsc::channel(1024);
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (perm_tx, _perm_rx) = mpsc::unbounded_channel();
        let mut app = TuiApp::new(rx, prompt_tx, ctrl_tx, perm_tx);
        app.state.agent_running = true;
        app.handle_action(input::InputAction::PauseAgent);
        assert_eq!(app.state.agent_control, AgentControl::Paused);
        assert_eq!(ctrl_rx.try_recv().unwrap(), ControlEvent::Pause);
    }

    #[test]
    fn pause_resumes_on_second_press() {
        use crate::tui::state::AgentControl;
        let (_tx, rx) = mpsc::channel(1024);
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (perm_tx, _perm_rx) = mpsc::unbounded_channel();
        let mut app = TuiApp::new(rx, prompt_tx, ctrl_tx, perm_tx);
        app.state.agent_running = true;
        app.handle_action(input::InputAction::PauseAgent);
        assert_eq!(app.state.agent_control, AgentControl::Paused);
        let _ = ctrl_rx.try_recv(); // consume Pause
        app.handle_action(input::InputAction::PauseAgent);
        assert_eq!(app.state.agent_control, AgentControl::Running);
        assert_eq!(ctrl_rx.try_recv().unwrap(), ControlEvent::Resume);
    }

    #[test]
    fn step_agent_sends_step_event() {
        use crate::tui::state::AgentControl;
        let (_tx, rx) = mpsc::channel(1024);
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (perm_tx, _perm_rx) = mpsc::unbounded_channel();
        let mut app = TuiApp::new(rx, prompt_tx, ctrl_tx, perm_tx);
        app.state.agent_running = true;
        app.handle_action(input::InputAction::StepAgent);
        assert_eq!(app.state.agent_control, AgentControl::StepMode);
        assert_eq!(ctrl_rx.try_recv().unwrap(), ControlEvent::Step);
    }

    #[test]
    fn approve_sends_on_perm_channel() {
        let (_tx, rx) = mpsc::channel(1024);
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        let (ctrl_tx, _ctrl_rx) = mpsc::unbounded_channel();
        let (perm_tx, mut perm_rx) = mpsc::unbounded_channel();
        let mut app = TuiApp::new(rx, prompt_tx, ctrl_tx, perm_tx);
        app.state.agent_running = true;
        app.handle_action(input::InputAction::ApproveAction);
        assert_eq!(perm_rx.try_recv().unwrap(), true);
    }

    #[test]
    fn reject_sends_on_perm_channel() {
        let (_tx, rx) = mpsc::channel(1024);
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        let (ctrl_tx, _ctrl_rx) = mpsc::unbounded_channel();
        let (perm_tx, mut perm_rx) = mpsc::unbounded_channel();
        let mut app = TuiApp::new(rx, prompt_tx, ctrl_tx, perm_tx);
        app.state.agent_running = true;
        app.handle_action(input::InputAction::RejectAction);
        assert_eq!(perm_rx.try_recv().unwrap(), false);
    }

    #[test]
    fn plan_progress_event_updates_activity_and_status() {
        use crate::tui::events::{PlanStepDisplayStatus, PlanStepStatus};
        let mut app = test_app();
        app.handle_ui_event(UiEvent::PlanProgress {
            goal: "Fix bug".into(),
            steps: vec![
                PlanStepStatus {
                    description: "Read file".into(),
                    tool_name: Some("file_read".into()),
                    status: PlanStepDisplayStatus::Succeeded,
                    duration_ms: Some(120),
                },
                PlanStepStatus {
                    description: "Edit file".into(),
                    tool_name: Some("file_edit".into()),
                    status: PlanStepDisplayStatus::InProgress,
                    duration_ms: None,
                },
            ],
            current_step: 1,
            elapsed_ms: 500,
        });
        // Should have a PlanOverview in activity.
        assert!(app.activity.line_count() > 0);
        // Status bar should show plan step.
        assert!(app.status.plan_step.is_some());
        let step_text = app.status.plan_step.as_ref().unwrap();
        assert!(step_text.contains("Step 2/2"));
        assert!(step_text.contains("Edit file"));
    }

    // --- Phase B6: Event ring buffer tests ---

    #[test]
    fn event_log_starts_empty() {
        let app = test_app();
        assert!(app.event_log.is_empty());
    }

    #[test]
    fn event_log_records_events() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::Info("test".into()));
        app.handle_ui_event(UiEvent::SpinnerStart("thinking".into()));
        assert_eq!(app.event_log.len(), 2);
        assert!(app.event_log[0].label.contains("Info"));
        assert!(app.event_log[1].label.contains("SpinnerStart"));
    }

    #[test]
    fn event_log_offsets_increase() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::Info("first".into()));
        app.handle_ui_event(UiEvent::Info("second".into()));
        assert!(app.event_log[1].offset_ms >= app.event_log[0].offset_ms);
    }

    #[test]
    fn event_log_respects_capacity() {
        let mut app = test_app();
        for i in 0..(EVENT_RING_CAPACITY + 50) {
            app.handle_ui_event(UiEvent::Info(format!("event {i}")));
        }
        assert_eq!(app.event_log.len(), EVENT_RING_CAPACITY);
        // Oldest should have been evicted, newest should be last.
        assert!(app.event_log.back().unwrap().label.contains("event 249"));
    }

    #[test]
    fn event_summary_covers_all_variants() {
        // Just verify event_summary doesn't panic for a few key variants.
        let summaries = vec![
            event_summary(&UiEvent::StreamChunk("test".into())),
            event_summary(&UiEvent::ToolStart {
                name: "bash".into(),
                input: serde_json::json!({}),
            }),
            event_summary(&UiEvent::AgentDone),
            event_summary(&UiEvent::Quit),
        ];
        assert!(summaries.iter().all(|s| !s.is_empty()));
    }

    // --- Phase E7: Slash command interception tests ---

    #[test]
    fn slash_command_not_sent_to_agent() {
        let mut app = test_app();
        // Type "/help" into the prompt textarea.
        for c in "/help".chars() {
            app.prompt.handle_key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char(c), crossterm::event::KeyModifiers::NONE));
        }
        app.handle_action(input::InputAction::SubmitPrompt);
        // Should NOT start agent for slash commands.
        assert!(!app.state.agent_running);
        // Help overlay should be open.
        assert!(app.state.overlay.is_active());
    }

    #[test]
    fn slash_clear_clears_activity() {
        let mut app = test_app();
        app.activity.push_info("some data");
        assert!(app.activity.line_count() > 0);
        for c in "/clear".chars() {
            app.prompt.handle_key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char(c), crossterm::event::KeyModifiers::NONE));
        }
        app.handle_action(input::InputAction::SubmitPrompt);
        assert!(!app.state.agent_running);
        assert_eq!(app.activity.line_count(), 0);
    }

    #[test]
    fn slash_quit_sets_should_quit() {
        let mut app = test_app();
        for c in "/quit".chars() {
            app.prompt.handle_key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char(c), crossterm::event::KeyModifiers::NONE));
        }
        app.handle_action(input::InputAction::SubmitPrompt);
        assert!(app.state.should_quit);
    }

    #[test]
    fn normal_text_sent_to_agent() {
        let mut app = test_app();
        for c in "hello world".chars() {
            app.prompt.handle_key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char(c), crossterm::event::KeyModifiers::NONE));
        }
        app.handle_action(input::InputAction::SubmitPrompt);
        assert!(app.state.agent_running);
    }

    // --- Phase E: Agent integration event handler tests ---

    #[test]
    fn dry_run_active_event_handled() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::DryRunActive(true));
        // Should be logged in the event ring buffer.
        assert!(app.event_log.back().is_some());
        assert!(app.event_log.back().unwrap().label.contains("DryRun"));
    }

    #[test]
    fn token_budget_update_event_handled() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::TokenBudgetUpdate {
            used: 500,
            limit: 1000,
            rate_per_minute: 120.5,
        });
        assert!(app.event_log.back().is_some());
    }

    #[test]
    fn agent_state_transition_event_handled() {
        use crate::tui::events::AgentState;
        let mut app = test_app();
        app.handle_ui_event(UiEvent::AgentStateTransition {
            from: AgentState::Idle,
            to: AgentState::Executing,
            reason: "started".into(),
        });
        assert!(app.event_log.back().unwrap().label.contains("State"));
    }

    // --- Sprint 1 B2+B3: Data parity tests ---

    #[test]
    fn task_status_event_visible_in_activity() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::TaskStatus {
            title: "Read config".into(),
            status: "Completed".into(),
            duration_ms: Some(1200),
            artifact_count: 2,
        });
        assert!(app.activity.line_count() > 0);
        assert!(app.event_log.back().unwrap().label.contains("TaskStatus"));
    }

    #[test]
    fn reasoning_status_event_visible_in_activity() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::ReasoningStatus {
            task_type: "CodeModification".into(),
            complexity: "Complex".into(),
            strategy: "PlanExecuteReflect".into(),
            score: 0.85,
            success: true,
        });
        // Should add 2 lines: [reasoning] + [evaluation]
        assert!(app.activity.line_count() >= 2);
    }

    // --- Sprint 1 B4: Search tests ---

    #[test]
    fn search_finds_matching_lines() {
        let mut app = test_app();
        app.activity.push_info("hello world");
        app.activity.push_info("goodbye world");
        app.activity.push_info("hello again");
        let matches = app.activity.search("hello");
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn search_case_insensitive() {
        let mut app = test_app();
        app.activity.push_info("Hello World");
        let matches = app.activity.search("hello");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let mut app = test_app();
        app.activity.push_info("data");
        let matches = app.activity.search("");
        assert!(matches.is_empty());
    }

    #[test]
    fn search_no_match_returns_empty() {
        let mut app = test_app();
        app.activity.push_info("hello");
        let matches = app.activity.search("zzzzz");
        assert!(matches.is_empty());
    }

    #[test]
    fn search_next_wraps_around() {
        let mut app = test_app();
        app.activity.push_info("match1");
        app.activity.push_info("other");
        app.activity.push_info("match2");
        app.search_matches = app.activity.search("match");
        assert_eq!(app.search_matches.len(), 2);
        app.search_current = 0;
        app.search_next();
        assert_eq!(app.search_current, 1);
        app.search_next();
        assert_eq!(app.search_current, 0); // wrapped
    }

    #[test]
    fn search_prev_wraps_around() {
        let mut app = test_app();
        app.activity.push_info("match1");
        app.activity.push_info("match2");
        app.search_matches = app.activity.search("match");
        app.search_current = 0;
        app.search_prev();
        assert_eq!(app.search_current, 1); // wrapped to last
    }

    #[test]
    fn search_enter_navigates_forward() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app();
        app.activity.push_info("alpha");
        app.activity.push_info("beta");
        app.activity.push_info("alpha");
        app.state.overlay.open(OverlayKind::Search);
        app.state.overlay.input = "alpha".into();
        app.rerun_search();
        assert_eq!(app.search_matches.len(), 2);
        assert_eq!(app.search_current, 0);
        // Press Enter to go to next match.
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.search_current, 1);
        // Press Enter again to wrap back to first.
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.search_current, 0);
    }

    #[test]
    fn search_shift_enter_navigates_backward() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app();
        app.activity.push_info("test1");
        app.activity.push_info("test2");
        app.state.overlay.open(OverlayKind::Search);
        app.state.overlay.input = "test".into();
        app.rerun_search();
        assert_eq!(app.search_matches.len(), 2);
        assert_eq!(app.search_current, 0);
        // Press Shift+Enter to go to previous (wraps to last).
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(app.search_current, 1);
    }

    #[test]
    fn search_empty_query_no_matches() {
        let mut app = test_app();
        app.activity.push_info("content");
        app.state.overlay.open(OverlayKind::Search);
        app.state.overlay.input = "".into();
        app.rerun_search();
        assert_eq!(app.search_matches.len(), 0);
    }

    // --- Sprint 1 B1: Permission channel tests ---

    #[test]
    fn permission_overlay_y_sends_approve_on_perm_channel() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (_tx, rx) = mpsc::channel(1024);
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        let (ctrl_tx, _ctrl_rx) = mpsc::unbounded_channel();
        let (perm_tx, mut perm_rx) = mpsc::unbounded_channel();
        let mut app = TuiApp::new(rx, prompt_tx, ctrl_tx, perm_tx);
        // Phase I-6C: Trigger PermissionAwaiting event to create conversational overlay.
        app.handle_ui_event(UiEvent::PermissionAwaiting {
            tool: "bash".into(),
            args: serde_json::json!({"command": "echo test"}),
            risk_level: "Low".into(),
        });
        // Type 'y' then Enter to approve.
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(perm_rx.try_recv().unwrap(), true);
        assert!(!app.state.overlay.is_active()); // overlay closed
    }

    #[test]
    fn permission_overlay_n_sends_reject_on_perm_channel() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (_tx, rx) = mpsc::channel(1024);
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        let (ctrl_tx, _ctrl_rx) = mpsc::unbounded_channel();
        let (perm_tx, mut perm_rx) = mpsc::unbounded_channel();
        let mut app = TuiApp::new(rx, prompt_tx, ctrl_tx, perm_tx);
        // Phase I-6C: Trigger PermissionAwaiting event to create conversational overlay.
        app.handle_ui_event(UiEvent::PermissionAwaiting {
            tool: "bash".into(),
            args: serde_json::json!({"command": "rm -rf /tmp/*.txt"}),
            risk_level: "High".into(),
        });
        // Type 'n' then Enter to reject.
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(perm_rx.try_recv().unwrap(), false);
        assert!(!app.state.overlay.is_active());
    }

    #[test]
    fn permission_overlay_enter_sends_approve_on_perm_channel() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (_tx, rx) = mpsc::channel(1024);
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        let (ctrl_tx, _ctrl_rx) = mpsc::unbounded_channel();
        let (perm_tx, mut perm_rx) = mpsc::unbounded_channel();
        let mut app = TuiApp::new(rx, prompt_tx, ctrl_tx, perm_tx);
        // Phase I-6C: Trigger PermissionAwaiting event to create conversational overlay.
        app.handle_ui_event(UiEvent::PermissionAwaiting {
            tool: "file_write".into(),
            args: serde_json::json!({"path": "/tmp/test.txt", "content": "Hello"}),
            risk_level: "Medium".into(),
        });
        // Type 'yes' then Enter to approve.
        for c in "yes".chars() {
            app.handle_overlay_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(perm_rx.try_recv().unwrap(), true);
    }

    // --- Sprint 2: UX + consistency tests ---

    #[test]
    fn agent_done_resets_agent_control() {
        use crate::tui::state::AgentControl;
        let mut app = test_app();
        app.state.agent_running = true;
        app.state.agent_control = AgentControl::Paused;
        app.handle_ui_event(UiEvent::AgentDone);
        assert_eq!(app.state.agent_control, AgentControl::Running);
        assert!(!app.state.agent_running);
    }

    #[test]
    fn agent_done_emits_toast() {
        let mut app = test_app();
        app.state.agent_running = true;
        app.handle_ui_event(UiEvent::AgentDone);
        assert!(!app.toasts.is_empty());
    }

    #[test]
    fn error_event_emits_toast() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::Error {
            message: "Connection failed".into(),
            hint: None,
        });
        assert!(!app.toasts.is_empty());
    }

    #[test]
    fn stream_error_emits_toast() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::StreamError("timeout".into()));
        assert!(!app.toasts.is_empty());
    }

    #[test]
    fn tool_denied_emits_toast() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::ToolDenied("bash".into()));
        assert!(!app.toasts.is_empty());
    }

    #[test]
    fn permission_awaiting_emits_toast() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::PermissionAwaiting {
            tool: "bash".into(),
            args: serde_json::json!({"command": "echo test"}),
            risk_level: "Low".into(),
        });
        assert!(!app.toasts.is_empty());
    }

    #[test]
    fn model_selected_emits_toast() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::ModelSelected {
            model: "gpt-4o".into(),
            provider: "openai".into(),
            reason: "complex task".into(),
        });
        assert!(!app.toasts.is_empty());
    }

    #[test]
    fn dry_run_emits_toast() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::DryRunActive(true));
        assert!(!app.toasts.is_empty());
    }

    #[test]
    fn agent_state_transition_persists_in_app_state() {
        use crate::tui::events::AgentState;
        let mut app = test_app();
        assert_eq!(app.state.agent_state, AgentState::Idle);
        app.handle_ui_event(UiEvent::AgentStateTransition {
            from: AgentState::Idle,
            to: AgentState::Planning,
            reason: "new task".into(),
        });
        assert_eq!(app.state.agent_state, AgentState::Planning);
    }

    #[test]
    fn agent_state_failed_transition_emits_toast() {
        use crate::tui::events::AgentState;
        let mut app = test_app();
        app.handle_ui_event(UiEvent::AgentStateTransition {
            from: AgentState::Executing,
            to: AgentState::Failed,
            reason: "provider error".into(),
        });
        assert!(!app.toasts.is_empty());
    }

    #[test]
    fn invalid_fsm_transition_logged_as_warning() {
        use crate::tui::events::AgentState;
        let mut app = test_app();
        // Idle → Complete is not a valid transition.
        app.handle_ui_event(UiEvent::AgentStateTransition {
            from: AgentState::Idle,
            to: AgentState::Complete,
            reason: "invalid".into(),
        });
        // State should still be persisted.
        assert_eq!(app.state.agent_state, AgentState::Complete);
        // Activity should contain a warning.
        assert!(app.activity.line_count() > 0);
    }

    #[test]
    fn slash_model_shows_info() {
        let mut app = test_app();
        app.execute_slash_command("model");
        assert!(app.activity.line_count() > 0);
    }

    #[test]
    fn slash_plan_switches_panel_to_plan() {
        let mut app = test_app();
        app.state.panel_visible = false;
        app.execute_slash_command("plan");
        assert!(app.state.panel_visible);
        assert_eq!(app.state.panel_section, crate::tui::state::PanelSection::Plan);
    }

    #[test]
    fn unknown_slash_command_shows_warning() {
        let mut app = test_app();
        app.execute_slash_command("nonexistent");
        assert!(app.activity.line_count() > 0);
    }

    // --- Sprint 3: Hardening tests ---

    #[test]
    fn burst_events_bounded_memory() {
        // Simulate 1000 rapid events — verify memory remains bounded.
        let mut app = test_app();
        for i in 0..1000 {
            app.handle_ui_event(UiEvent::Info(format!("burst event {i}")));
        }
        // Event log should be bounded at EVENT_RING_CAPACITY.
        assert!(app.event_log.len() <= EVENT_RING_CAPACITY);
        // Activity lines should all be present (no memory cap on activity).
        assert_eq!(app.activity.line_count(), 1000);
    }

    #[test]
    fn burst_events_event_log_oldest_evicted() {
        let mut app = test_app();
        for i in 0..500 {
            app.handle_ui_event(UiEvent::Info(format!("event {i}")));
        }
        assert_eq!(app.event_log.len(), EVENT_RING_CAPACITY);
        // Newest should be the last event.
        assert!(app.event_log.back().unwrap().label.contains("event 499"));
    }

    #[test]
    fn dismiss_toasts_action() {
        let mut app = test_app();
        app.handle_ui_event(UiEvent::Error {
            message: "test error".into(),
            hint: None,
        });
        assert!(!app.toasts.is_empty());
        app.handle_action(input::InputAction::DismissToasts);
        assert!(app.toasts.is_empty());
    }

    #[test]
    fn bare_slash_opens_command_palette() {
        let mut app = test_app();
        for c in "/".chars() {
            app.prompt.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        app.handle_action(input::InputAction::SubmitPrompt);
        // Should open command palette, not send to agent.
        assert!(!app.state.agent_running);
        assert!(matches!(
            app.state.overlay.active,
            Some(OverlayKind::CommandPalette)
        ));
    }

    #[test]
    fn event_summary_exhaustive() {
        // Verify event_summary covers all UiEvent variants without panicking.
        use crate::tui::events::*;
        let events: Vec<UiEvent> = vec![
            UiEvent::StreamChunk("test".into()),
            UiEvent::StreamCodeBlock { lang: "rs".into(), code: "fn(){}".into() },
            UiEvent::StreamToolMarker("bash".into()),
            UiEvent::StreamDone,
            UiEvent::StreamError("err".into()),
            UiEvent::ToolStart { name: "bash".into(), input: serde_json::json!({}) },
            UiEvent::ToolOutput { name: "bash".into(), content: "ok".into(), is_error: false, duration_ms: 10 },
            UiEvent::ToolDenied("bash".into()),
            UiEvent::SpinnerStart("think".into()),
            UiEvent::SpinnerStop,
            UiEvent::Warning { message: "w".into(), hint: None },
            UiEvent::Error { message: "e".into(), hint: None },
            UiEvent::Info("info".into()),
            UiEvent::StatusUpdate { provider: None, model: None, round: None, tokens: None, cost: None, session_id: None, elapsed_ms: None, tool_count: None, input_tokens: None, output_tokens: None },
            UiEvent::RoundStart(1),
            UiEvent::RoundEnd(1),
            UiEvent::Redraw,
            UiEvent::AgentDone,
            UiEvent::Quit,
            UiEvent::PlanProgress { goal: "g".into(), steps: vec![], current_step: 0, elapsed_ms: 0 },
            UiEvent::RoundStarted { round: 1, provider: "p".into(), model: "m".into() },
            UiEvent::RoundEnded { round: 1, input_tokens: 0, output_tokens: 0, cost: 0.0, duration_ms: 0 },
            UiEvent::ModelSelected { model: "m".into(), provider: "p".into(), reason: "r".into() },
            UiEvent::ProviderFallback { from: "a".into(), to: "b".into(), reason: "r".into() },
            UiEvent::LoopGuardAction { action: "a".into(), reason: "r".into() },
            UiEvent::CompactionComplete { old_msgs: 10, new_msgs: 5, tokens_saved: 100 },
            UiEvent::CacheStatus { hit: true, source: "s".into() },
            UiEvent::SpeculativeResult { tool: "t".into(), hit: false },
            UiEvent::PermissionAwaiting { tool: "bash".into(), args: serde_json::json!({}), risk_level: "Low".into() },
            UiEvent::ReflectionStarted,
            UiEvent::ReflectionComplete { analysis: "a".into(), score: 0.5 },
            UiEvent::ConsolidationStatus { action: "a".into() },
            UiEvent::ToolRetrying { tool: "t".into(), attempt: 1, max_attempts: 3, delay_ms: 100 },
            UiEvent::ContextTierUpdate { l0_tokens: 0, l0_capacity: 0, l1_tokens: 0, l1_entries: 0, l2_entries: 0, l3_entries: 0, l4_entries: 0, total_tokens: 0 },
            UiEvent::ReasoningUpdate { strategy: "s".into(), task_type: "t".into(), complexity: "c".into() },
            UiEvent::DryRunActive(false),
            UiEvent::TokenBudgetUpdate { used: 0, limit: 0, rate_per_minute: 0.0 },
            UiEvent::ProviderHealthUpdate { provider: "p".into(), status: ProviderHealthStatus::Healthy },
            UiEvent::CircuitBreakerUpdate { provider: "p".into(), state: CircuitBreakerState::Closed, failure_count: 0 },
            UiEvent::AgentStateTransition { from: AgentState::Idle, to: AgentState::Planning, reason: "r".into() },
            UiEvent::TaskStatus { title: "t".into(), status: "s".into(), duration_ms: None, artifact_count: 0 },
            UiEvent::ReasoningStatus { task_type: "t".into(), complexity: "c".into(), strategy: "s".into(), score: 0.0, success: true },
        ];
        for ev in &events {
            let summary = event_summary(ev);
            assert!(!summary.is_empty(), "empty summary for {:?}", ev);
        }
        // All 42 UiEvent variants covered.
        assert_eq!(events.len(), 42);
    }
}
