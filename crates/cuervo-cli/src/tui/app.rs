//! TUI application shell — manages the render loop and event dispatch.

use std::io;
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags,
    MouseButton, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use tokio::sync::mpsc;

use super::events::UiEvent;
use super::input;
use super::layout;
use super::state::{AppState, FocusZone};
use super::widgets::activity::ActivityState;
use super::widgets::prompt::PromptState;
use super::widgets::status::StatusState;

/// The TUI application. Owns the terminal, state, and event channels.
pub struct TuiApp {
    state: AppState,
    prompt: PromptState,
    activity: ActivityState,
    status: StatusState,
    /// Receives UiEvents from the agent loop (via TuiSink).
    ui_rx: mpsc::UnboundedReceiver<UiEvent>,
    /// Sends prompt text to the agent loop.
    prompt_tx: mpsc::UnboundedSender<String>,
    /// Tracked area of the submit button for mouse click detection.
    submit_button_area: Rect,
}

impl TuiApp {
    /// Create a new TUI application.
    pub fn new(
        ui_rx: mpsc::UnboundedReceiver<UiEvent>,
        prompt_tx: mpsc::UnboundedSender<String>,
    ) -> Self {
        Self {
            state: AppState::new(),
            prompt: PromptState::new(),
            activity: ActivityState::new(),
            status: StatusState::new(),
            ui_rx,
            prompt_tx,
            submit_button_area: Rect::default(),
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
            "  Click [► Send] = submit | Enter = newline | Tab = switch zone | Scroll = navigate | Ctrl+C = quit"
        );
        self.activity.push_info("");
    }

    /// Run the TUI render loop. Blocks until quit.
    pub async fn run(&mut self) -> io::Result<()> {
        // Enter alternate screen + raw mode + mouse capture.
        let mut stdout = io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        terminal::enable_raw_mode()?;
        stdout.execute(EnableMouseCapture)?;

        // Enable keyboard enhancement to detect Cmd (SUPER) on macOS.
        let _ = stdout.execute(PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES,
        ));

        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        // Spawn a single dedicated thread for crossterm event polling.
        // This avoids spawn_blocking thread accumulation.
        let (key_tx, mut key_rx) = mpsc::unbounded_channel::<Event>();
        std::thread::spawn(move || {
            loop {
                if event::poll(Duration::from_millis(50)).unwrap_or(false) {
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

        loop {
            // Render frame.
            terminal.draw(|frame| {
                let area = frame.area();
                let zones = layout::calculate_zones(area);

                // Split prompt zone: [textarea | submit button].
                let prompt_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Min(1), Constraint::Length(14)])
                    .split(zones.prompt);
                let textarea_area = prompt_chunks[0];
                let button_area = prompt_chunks[1];

                self.prompt.render(frame, textarea_area, self.state.focus == FocusZone::Prompt);

                // Render submit button with highlighted background when ready.
                let (btn_text_style, btn_border_style, btn_title) = if self.state.agent_running {
                    (
                        Style::default().fg(Color::DarkGray),
                        Style::default().fg(Color::DarkGray),
                        " ··· ",
                    )
                } else {
                    (
                        Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD),
                        Style::default().fg(Color::Green),
                        "",
                    )
                };
                let h = button_area.height.saturating_sub(2); // inner height
                let pad_top = if h > 2 { (h - 2) / 2 } else { 0 };
                let mut btn_lines: Vec<Line<'_>> = Vec::new();
                for _ in 0..pad_top {
                    btn_lines.push(Line::from(""));
                }
                btn_lines.push(Line::from(Span::styled("  ► Send  ", btn_text_style)));
                if h > 2 {
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

                self.activity.render(frame, zones.activity, &self.state);
                self.status.render(frame, zones.status);
            })?;

            // Event loop: crossterm events + agent UiEvents.
            tokio::select! {
                Some(ev) = key_rx.recv() => {
                    match ev {
                        Event::Key(key) => {
                            let action = input::dispatch_key(key, self.state.agent_running);
                            self.handle_action(action);
                        }
                        Event::Mouse(mouse) => {
                            match mouse.kind {
                                MouseEventKind::Down(MouseButton::Left) => {
                                    let r = self.submit_button_area;
                                    if !self.state.agent_running
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
                break;
            }
        }

        // Restore terminal.
        let mut stdout = io::stdout();
        let _ = stdout.execute(PopKeyboardEnhancementFlags);
        stdout.execute(DisableMouseCapture)?;
        terminal::disable_raw_mode()?;
        stdout.execute(LeaveAlternateScreen)?;
        Ok(())
    }

    fn handle_action(&mut self, action: input::InputAction) {
        match action {
            input::InputAction::SubmitPrompt => {
                let text = self.prompt.take_text();
                if text.trim().is_empty() {
                    return;
                }
                self.activity.push_user_prompt(&text);
                self.state.agent_running = true;
                self.state.focus = FocusZone::Activity;
                let _ = self.prompt_tx.send(text);
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
            input::InputAction::ForwardToWidget(key) => {
                match self.state.focus {
                    FocusZone::Prompt => self.prompt.handle_key(key),
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

    fn handle_ui_event(&mut self, ev: UiEvent) {
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
                // Streaming complete for this round.
            }
            UiEvent::StreamError(msg) => {
                self.activity.push_error(&msg, None);
            }
            UiEvent::ToolStart { name, input } => {
                // Build a short input preview from the JSON value.
                let input_preview = format_input_preview(&input);
                self.activity.push_tool_start(&name, &input_preview);
            }
            UiEvent::ToolOutput { name, content, is_error, duration_ms } => {
                self.activity.complete_tool(&name, content, is_error, duration_ms);
            }
            UiEvent::ToolDenied(name) => {
                self.activity.push_warning(&format!("Tool denied: {name}"), None);
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
            UiEvent::RoundEnd(_) => {}
            UiEvent::Redraw => {}
            UiEvent::AgentDone => {
                self.state.agent_running = false;
                self.state.spinner_active = false;
                self.state.focus = FocusZone::Prompt;
            }
            UiEvent::Quit => {
                self.state.should_quit = true;
            }
            UiEvent::PlanProgress { goal, steps, current_step } => {
                self.activity.set_plan_overview(&goal, steps.clone(), current_step);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> TuiApp {
        let (_tx, rx) = mpsc::unbounded_channel();
        let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel();
        TuiApp::new(rx, prompt_tx)
    }

    #[test]
    fn app_initial_state() {
        let app = test_app();
        assert!(!app.state.agent_running);
        assert!(!app.state.should_quit);
        assert_eq!(app.state.focus, FocusZone::Prompt);
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
                },
                PlanStepStatus {
                    description: "Edit file".into(),
                    tool_name: Some("file_edit".into()),
                    status: PlanStepDisplayStatus::InProgress,
                },
            ],
            current_step: 1,
        });
        // Should have a PlanOverview in activity.
        assert!(app.activity.line_count() > 0);
        // Status bar should show plan step.
        assert!(app.status.plan_step.is_some());
        let step_text = app.status.plan_step.as_ref().unwrap();
        assert!(step_text.contains("Step 2/2"));
        assert!(step_text.contains("Edit file"));
    }
}
