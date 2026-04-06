//! Main TUI render and event loop.
use super::*;

impl TuiApp {
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

        // Phase 93: Enable bracketed paste for proper right-click/middle-click support.
        // Without this, terminal emulators simulate paste as individual keypresses and
        // Event::Paste is never fired. Enabled before the event poll loop starts.
        let _ = stdout.execute(EnableBracketedPaste);
        tracing::debug!("Enabled bracketed paste");

        // Enable keyboard enhancement to detect Cmd (SUPER) on macOS.
        // REPORT_EVENT_TYPES: needed for Super/Cmd key detection via Kitty protocol.
        let _ = stdout.execute(PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES,
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

        // Phase 3 SRCH-004: Load search history from database on startup
        if let Some(ref db) = self.db {
            tracing::debug!("Loading search history from database");
            match db.get_recent_queries(50).await {
                Ok(queries) => {
                    tracing::debug!("Loaded {} search queries from database", queries.len());
                    self.activity_navigator.load_history(queries);
                    self.search_history_loaded = true;
                }
                Err(e) => {
                    tracing::warn!("Failed to load search history: {}", e);
                }
            }
        }

        // Frontier update: open overlay before the first frame if an update is waiting.
        if let Some(ref info) = self.pending_update.take() {
            self.state.overlay.open(OverlayKind::UpdateAvailable {
                current: info.current.clone(),
                remote: info.remote.clone(),
                notes: info.notes.clone(),
                published_at: info.published_at.clone(),
                size_bytes: info.size_bytes,
            });
        }

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
                    self.prompt
                        .set_input_state(crate::tui::input_state::InputState::Idle);

                    // Alert user
                    self.activity_model.push_warning(
                        &format!(
                            "Agent watchdog triggered after {} seconds - UI unlocked",
                            elapsed_secs
                        ),
                        Some("The agent may have hung. Check logs for details."),
                    );
                    self.toasts.push(Toast::new(
                        format!("Agent timeout ({elapsed_secs}s) - UI force-unlocked"),
                        ToastLevel::Warning,
                    ));
                }
            }

            // Sync spinner state to status bar (ensures spinner is always visible
            // even when the activity panel is scrolled up or full).
            self.status.spinner_active = self.state.spinner_active;
            self.status.spinner_frame = self.state.spinner_frame;

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

                // Phase I2: Calculate dynamic layout based on prompt content lines
                let mode_layout = layout::calculate_mode_layout_dynamic(
                    area,
                    effective_mode,
                    self.state.panel_visible,
                    self.state.prompt_content_lines.max(1), // At least 1 line
                );

                // Phase 93: Reserve rows above prompt for media attachment chips.
                let prompt_area = if !self.state.pending_attachments.is_empty() {
                    let attach_count = self.state.pending_attachments.len();
                    // 1 row per ≤4 attachments, max 2 rows.
                    let chip_rows = ((attach_count + 3) / 4).min(2) as u16;
                    let area = mode_layout.prompt;
                    if area.height > chip_rows + 1 {
                        // Split: [chip rows] + [prompt area]
                        use ratatui::layout::{Constraint, Direction, Layout};
                        use ratatui::style::{Modifier, Style as RStyle};
                        use ratatui::text::{Line, Span};
                        use ratatui::widgets::Paragraph;
                        let split = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([Constraint::Length(chip_rows), Constraint::Min(1)])
                            .split(area);
                        let chips_area = split[0];
                        let p = &crate::render::theme::active().palette;
                        let chip_style = RStyle::default()
                            .fg(p.bg_panel_ratatui())
                            .bg(p.running_ratatui())
                            .add_modifier(Modifier::BOLD);
                        let separator_style = RStyle::default().fg(p.border_ratatui());
                        let mut spans: Vec<Span> = Vec::new();
                        for att in &self.state.pending_attachments {
                            let icon = match att.modality {
                                "image" => "◫",
                                "audio" => "♫",
                                _ => "▶",
                            };
                            let chip_text = format!(" {} {} × ", icon, att.display_name);
                            spans.push(Span::styled(chip_text, chip_style));
                            spans.push(Span::styled(" ", separator_style));
                        }
                        let chip_line = Line::from(spans);
                        frame.render_widget(Paragraph::new(chip_line), chips_area);
                        split[1]
                    } else {
                        area
                    }
                } else {
                    mode_layout.prompt
                };

                // Phase I2 Fix: Render compact prompt with styled Momoto button
                let (content_lines, button_area) = self.prompt.render_compact(
                    frame,
                    prompt_area,
                    self.state.focus == FocusZone::Prompt,
                    self.state.typing_indicator,
                );

                // Update state for next frame's dynamic height calculation
                self.state.prompt_content_lines = content_lines;

                // Phase I2 Fix: Render styled Momoto send button if area available
                if let Some(btn_area) = button_area {
                    use ratatui::style::{Modifier, Style};
                    use ratatui::text::{Line, Span};
                    use ratatui::widgets::Paragraph;

                    let p = &crate::render::theme::active().palette;
                    let input_state = self.prompt.input_state();

                    // Button text and colors based on InputState
                    let (btn_text, btn_bg, btn_fg) = match input_state {
                        super::super::input_state::InputState::Idle => {
                            if self.prompt.text().trim().is_empty() {
                                ("  Type...  ", p.muted_ratatui(), p.text_label_ratatui())
                            } else {
                                ("  ► Send  ", p.success_ratatui(), p.bg_panel_ratatui())
                            }
                        }
                        super::super::input_state::InputState::Sending => {
                            ("  ↑ Sending", p.planning_ratatui(), p.bg_panel_ratatui())
                        }
                        super::super::input_state::InputState::LockedByPermission => {
                            ("  🔒 Locked", p.destructive_ratatui(), p.bg_panel_ratatui())
                        }
                    };

                    let button = Paragraph::new(Line::from(vec![Span::styled(
                        btn_text,
                        Style::default()
                            .bg(btn_bg)
                            .fg(btn_fg)
                            .add_modifier(Modifier::BOLD),
                    )]));

                    frame.render_widget(button, btn_area);
                    self.submit_button_area = btn_area; // For mouse click detection (optional)
                }

                // Render side panel if visible.
                if let Some(panel_area) = mode_layout.side_panel {
                    self.last_panel_area = panel_area;
                    self.panel
                        .render(frame, panel_area, self.state.panel_section);
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

                    let lines: Vec<Line<'_>> = self
                        .event_log
                        .iter()
                        .skip(skip)
                        .map(|entry| {
                            let ts = format!("{:>6}ms ", entry.offset_ms);
                            Line::from(vec![
                                Span::styled(ts, Style::default().fg(c_muted_insp)),
                                Span::styled(entry.label.clone(), Style::default().fg(c_text_insp)),
                            ])
                        })
                        .collect();

                    let inspector = Paragraph::new(lines).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(format!(" Inspector ({total}) "))
                            .border_style(Style::default().fg(c_border_insp)),
                    );
                    frame.render_widget(inspector, inspector_area);
                }

                // Phase B1: Clean up completed expansion animations
                self.expansion_animations
                    .retain(|_, anim| !anim.is_complete());

                // Phase B4: Save activity area for mouse event routing
                self.last_activity_area = mode_layout.activity;

                // Phase A2: Use new virtual scroll renderer (Phase B1: with expansion animations, B2: with shimmer, B3: with highlights)
                let (max_scroll, viewport_height) = self.activity_renderer.render(
                    frame,
                    mode_layout.activity,
                    &self.activity_model,
                    &self.activity_navigator,
                    &self.state,
                    &self.expansion_animations, // Phase B1: pass animations
                    &self.executing_tools,      // Phase B2: pass executing tools for shimmer
                    &self.highlights,           // Phase B3: pass highlights for search
                );

                // Phase 1 Remediation: Sync max_scroll and viewport_height to Navigator
                // This prevents stale clamping and enables proper selection centering
                self.activity_navigator.last_max_scroll = max_scroll;
                self.activity_navigator.viewport_height = Some(viewport_height);

                self.status.agent_control = self.state.agent_control;
                self.status.dry_run_active = self.state.dry_run_active;
                self.status.token_budget = self.state.token_budget;
                self.status.ui_mode = self.state.ui_mode;
                self.status.reasoning_strategy = self.panel.reasoning.strategy.clone();
                // Phase A3: Update contextual hints when Activity focused
                self.status.activity_hints = if self.state.focus == FocusZone::Activity {
                    self.activity_controller
                        .contextual_actions(&self.activity_navigator, &self.activity_model)
                } else {
                    Vec::new()
                };
                // Phase 3 SRCH-003: Update search state
                self.status.search_active = self.activity_navigator.is_searching();
                self.status.search_mode = self.activity_navigator.search_mode_label().to_string();
                self.status.search_current = self.activity_navigator.current_match_position();
                self.status.search_total = self.activity_navigator.match_count();
                // Compute cache hit rate from panel metrics.
                let cache_total = self.panel.metrics.cache_hits + self.panel.metrics.cache_misses;
                self.status.cache_hit_rate = if cache_total > 0 {
                    Some((self.panel.metrics.cache_hits as f64 / cache_total as f64) * 100.0)
                } else {
                    None
                };
                self.status.render(frame, mode_layout.status);
                // Track status area for mouse click detection (Phase 45C/D).
                self.last_status_area = mode_layout.status;
                // ctrl button is at col+2 (inside border), row+1 (inside border), ~6 chars wide.
                self.ctrl_button_area = Rect {
                    x: mode_layout.status.x + 2,
                    y: mode_layout.status.y + 1,
                    width: 6,
                    height: 1,
                };
                // session ID button: ctrl label (6) + " │ ◆ Halcon │ " (14) ≈ 20 chars offset from left inside border.
                let sid_len = self.status.session_id_display_len() as u16;
                self.session_id_button_area = Rect {
                    x: mode_layout.status.x + 20,
                    y: mode_layout.status.y + 1,
                    width: sid_len,
                    height: 1,
                };
                // Model button: after session ID + separator " │ " (3 chars).
                // Covers "provider/model ↕" — underlined clickable area.
                let model_x = mode_layout.status.x + 20 + sid_len + 3;
                let model_w = (
                    unicode_width::UnicodeWidthStr::width(self.status.provider.as_str())
                    + 1  // "/"
                    + unicode_width::UnicodeWidthStr::width(self.status.model.as_str())
                    + 2
                    // " v"
                ) as u16;
                self.model_button_area = Rect {
                    x: model_x,
                    y: mode_layout.status.y + 1,
                    width: model_w,
                    height: 1,
                };

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
                        let current = if match_count > 0 {
                            self.search_current + 1
                        } else {
                            0
                        };
                        overlay::render_search(
                            frame,
                            area,
                            &self.state.overlay.input,
                            match_count,
                            current,
                        );
                    }
                    Some(OverlayKind::PermissionPrompt { .. }) => {
                        // Phase 2.2: Render permission modal with momoto colors + countdown bar.
                        if let Some(ref modal) = self.permission_modal {
                            let remaining_secs = self.state.overlay.permission_deadline.map(|d| {
                                d.saturating_duration_since(std::time::Instant::now())
                                    .as_secs()
                            });
                            let total_secs = self.state.overlay.permission_total_secs;
                            modal.render(
                                frame,
                                area,
                                self.state.overlay.show_advanced_permissions,
                                remaining_secs,
                                total_secs,
                            );
                        } else if let Some(ref conv_overlay) = self.conversational_overlay {
                            // Fallback to conversational overlay (legacy).
                            conv_overlay.render(area, frame.buffer_mut());
                        } else {
                            // Fallback to simple prompt (shouldn't happen).
                            overlay::render_permission_prompt(frame, area, "(unknown)");
                        }
                    }
                    Some(OverlayKind::ContextServers) => {
                        overlay::render_context_servers(
                            frame,
                            area,
                            &self.state.context_servers,
                            self.state.context_servers_total,
                            self.state.context_servers_enabled,
                        );
                    }
                    Some(OverlayKind::SessionList) => {
                        overlay::render_session_list(
                            frame,
                            area,
                            &self.session_list,
                            self.session_list_selected,
                        );
                    }
                    Some(OverlayKind::SudoPasswordEntry { tool, command }) => {
                        use crate::tui::widgets::sudo_modal::{SudoModal, SudoModalContext};
                        let ctx = SudoModalContext::new(
                            tool.clone(),
                            command.clone(),
                            self.sudo_has_cached,
                        );
                        let modal = SudoModal::new(ctx);
                        modal.render(
                            frame,
                            area,
                            &self.sudo_password_buf,
                            self.sudo_remember_password,
                            self.sudo_has_cached,
                        );
                    }
                    Some(OverlayKind::InitWizard {
                        step,
                        preview,
                        save_path,
                        dry_run,
                    }) => {
                        overlay::render_init_wizard(
                            frame,
                            area,
                            *step,
                            preview,
                            save_path,
                            *dry_run,
                            self.state.spinner_frame,
                        );
                    }
                    Some(OverlayKind::PluginSuggest {
                        suggestions,
                        selected,
                        dry_run,
                    }) => {
                        overlay::render_plugin_suggest(
                            frame,
                            area,
                            suggestions,
                            *selected,
                            *dry_run,
                        );
                    }
                    Some(OverlayKind::UpdateAvailable {
                        current,
                        remote,
                        notes,
                        published_at,
                        size_bytes,
                    }) => {
                        overlay::render_update_available(
                            frame,
                            area,
                            current,
                            remote,
                            notes,
                            published_at,
                            *size_bytes,
                        );
                    }
                    Some(OverlayKind::ModelSelector {
                        models,
                        selected,
                        current_model,
                        error_context,
                    }) => {
                        overlay::render_model_selector(
                            frame,
                            area,
                            models,
                            *selected,
                            current_model,
                            error_context.as_deref(),
                        );
                    }
                    Some(OverlayKind::Settings) => {
                        let sections = overlay::build_settings_entries(&self.app_config);
                        overlay::render_settings(
                            frame,
                            area,
                            &sections,
                            self.settings_selected,
                            self.settings_editing,
                            &self.settings_edit_buffer,
                        );
                    }
                    Some(OverlayKind::LspStatus) => {
                        overlay::render_lsp_status(frame, area, &self.lsp_info);
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
                            use crossterm::event::KeyCode;

                            // NOTE: Ctrl+S is now handled via dispatch_key → InputAction::OpenContextServers.
                            // All keybindings are unified in input::dispatch_key for consistency.

                            // CRITICAL FIX: Input ALWAYS available, overlays only intercept specific keys.
                            // This ensures the user can ALWAYS type, even during permission prompts.

                            if self.state.overlay.is_active() {
                                // Tab completes the selected slash-command item into the prompt.
                                if self.slash_completing && key.code == KeyCode::Tab {
                                    let action = self.state.overlay.filtered_items
                                        .get(self.state.overlay.selected)
                                        .map(|item| item.action.clone());
                                    self.slash_completing = false;
                                    self.state.overlay.close();
                                    if let Some(cmd) = action {
                                        // Find the full label (e.g. "/pause") for the selected command.
                                        let label = overlay::default_commands()
                                            .into_iter()
                                            .find(|i| i.action == cmd)
                                            .map(|i| i.label)
                                            .unwrap_or_else(|| format!("/{cmd}"));
                                        self.prompt.clear();
                                        self.prompt.insert_str(&label);
                                    }
                                } else {
                                    // Determine if this is an overlay control key or a typing key.
                                    // In slash_completing mode Backspace and all Char keys go to the
                                    // prompt (which mirrors its content back to the palette filter),
                                    // so only Esc/Enter/Up/Down are intercepted by the overlay.
                                    let is_overlay_control = if self.slash_completing {
                                        matches!(
                                            key.code,
                                            KeyCode::Esc
                                                | KeyCode::Enter
                                                | KeyCode::Up
                                                | KeyCode::Down
                                        )
                                    } else {
                                        matches!(
                                            key.code,
                                            KeyCode::Esc
                                                | KeyCode::Enter
                                                | KeyCode::Up
                                                | KeyCode::Down
                                                | KeyCode::Backspace
                                                | KeyCode::Char('y')
                                                | KeyCode::Char('n')
                                                | KeyCode::Char('Y')
                                                | KeyCode::Char('N')
                                        )
                                    };

                                    if is_overlay_control {
                                        // Overlay-specific control keys → route to overlay
                                        self.handle_overlay_key(key);
                                    } else {
                                        // ALL other keys (chars, numbers, symbols) → ALWAYS to prompt
                                        // This allows typing prompts even during permission modals (for queuing)
                                        let action = input::dispatch_key(key);
                                        self.handle_action(action);
                                    }
                                }
                            } else {
                                // No overlay active → normal routing
                                let action = input::dispatch_key(key);
                                self.handle_action(action);
                            }
                        }
                        Event::Mouse(mouse) => {
                            // Phase 45C: Check STOP button (status bar ctrl area).
                            {
                                let r = self.ctrl_button_area;
                                if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                                    && r.width > 0
                                    && mouse.column >= r.x
                                    && mouse.column < r.x + r.width
                                    && mouse.row >= r.y
                                    && mouse.row < r.y + r.height
                                    && self.state.agent_running
                                {
                                    let _ = self.ctrl_tx.send(ControlEvent::CancelAgent);
                                    self.state.agent_running = false;
                                    self.status.agent_running = false;
                                    use crate::tui::input_state::InputState;
                                    self.prompt.set_input_state(InputState::Idle);
                                    self.activity_model.push_info("[control] ■ Agent stopped by user");
                                }
                            }

                            // Phase 45D: Check session ID area (click to copy full UUID).
                            {
                                let r = self.session_id_button_area;
                                if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                                    && r.width > 0
                                    && mouse.column >= r.x
                                    && mouse.column < r.x + r.width
                                    && mouse.row >= r.y
                                    && mouse.row < r.y + r.height
                                {
                                    let full_id = self.status.full_session_id.clone();
                                    if !full_id.is_empty() {
                                        match crate::tui::clipboard::copy_to_clipboard(&full_id) {
                                            Ok(_) => self.toasts.push(Toast::new("Session ID copied", ToastLevel::Success)),
                                            Err(e) => self.toasts.push(Toast::new(format!("Copy failed: {e}"), ToastLevel::Warning)),
                                        }
                                    }
                                }
                            }

                            // Model selector button: click provider/model in status bar.
                            {
                                let r = self.model_button_area;
                                if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                                    && r.width > 0
                                    && mouse.column >= r.x
                                    && mouse.column < r.x + r.width
                                    && mouse.row >= r.y
                                    && mouse.row < r.y + r.height
                                {
                                    self.handle_action(input::InputAction::OpenModelSelector);
                                }
                            }

                            // Phase I2: Check submit button first
                            let r = self.submit_button_area;
                            if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                                && r.width > 0
                                && mouse.column >= r.x
                                && mouse.column < r.x + r.width
                                && mouse.row >= r.y
                                && mouse.row < r.y + r.height
                            {
                                tracing::debug!("Submit button clicked at ({}, {})", mouse.column, mouse.row);
                                self.handle_action(input::InputAction::SubmitPrompt);
                            } else {
                                // Phase B4: Route ALL mouse events to activity controller
                                // This enables: hover (MouseMove), click selection, scroll, expand/collapse
                                let viewport_height = self.last_activity_area.height.saturating_sub(2) as usize;
                                let ctrl_action = self.activity_controller.handle_mouse(
                                    mouse,
                                    self.last_activity_area,
                                    &mut self.activity_navigator,
                                    &self.activity_model,
                                    viewport_height,
                                );

                                // Execute returned action (e.g., ToggleExpand)
                                match ctrl_action {
                                    crate::tui::activity_controller::ControlAction::None => {}
                                    crate::tui::activity_controller::ControlAction::ToggleExpand(idx) => {
                                        // Phase B1: Start smooth expand/collapse animation
                                        let was_expanded = self.activity_navigator.is_expanded(idx);
                                        self.activity_navigator.toggle_expand(idx);
                                        let now_expanded = self.activity_navigator.is_expanded(idx);

                                        let current_progress = self
                                            .expansion_animations
                                            .get(&idx)
                                            .map(|anim| anim.current())
                                            .unwrap_or(if was_expanded { 1.0 } else { 0.0 });

                                        let anim = if now_expanded {
                                            ExpansionAnimation::expand_from(current_progress)
                                        } else {
                                            ExpansionAnimation::collapse_from(current_progress)
                                        };
                                        self.expansion_animations.insert(idx, anim);
                                    }
                                    _ => {
                                        // Other actions (CopyOutput, JumpToPlanStep, OpenInspector) - future
                                        tracing::debug!("Unhandled control action: {:?}", ctrl_action);
                                    }
                                }
                            }
                        }
                        // Phase 93: Bracketed paste — fires when EnableBracketedPaste is active.
                        // Handles right-click paste (macOS), middle-click paste (Linux),
                        // and terminal emulator paste gestures (iTerm2, WezTerm, Ghostty, etc.).
                        Event::Paste(raw_text) => {
                            let outcome = paste_safe(&raw_text);
                            let (text, warning) = match outcome {
                                PasteOutcome::Ok(t) => (t, None),
                                PasteOutcome::Large { text, original_len } => (
                                    text,
                                    Some(format!(
                                        "Large paste: {}K chars — consider using a file attachment",
                                        original_len / 1000
                                    )),
                                ),
                                PasteOutcome::Truncated { text, original_len } => (
                                    text,
                                    Some(format!(
                                        "Paste truncated: {} chars → {}K limit",
                                        original_len,
                                        super::super::clipboard::PASTE_LIMIT_CHARS / 1000,
                                    )),
                                ),
                            };
                            if let Some(msg) = warning {
                                self.toasts.push(Toast::new(msg, ToastLevel::Warning));
                            }
                            // Detect if pasted text is a single-line media file path.
                            if let Some(attachment) = self.try_detect_media_path(&text) {
                                self.state.pending_attachments.push(attachment);
                            } else {
                                self.prompt.insert_str(&text);
                            }
                            needs_render = true;
                        }
                        _ => {}
                    }
                }
                Some(first_ev) = self.ui_rx.recv() => {
                    // FIX: Batch process UI events to prevent channel saturation.
                    // Drain up to 10 events per select! iteration for higher throughput.
                    self.handle_ui_event(first_ev);

                    // Try to drain additional available events (non-blocking).
                    for _ in 0..9 {
                        match self.ui_rx.try_recv() {
                            Ok(ev) => self.handle_ui_event(ev),
                            Err(_) => break,  // No more events immediately available
                        }
                    }
                }
                _ = tick_interval.tick() => {
                    // Advance spinner animation frame.
                    self.state.tick_spinner();

                    // Phase B1: Force re-render if expansion animations are active
                    // This ensures smooth 60 FPS animation playback (100ms tick = 10 FPS baseline,
                    // but active animations trigger render on every tick)
                    if !self.expansion_animations.is_empty() {
                        needs_render = true;
                    }

                    // Phase 2.3: Prune completed transitions.
                    self.transition_engine.prune_completed();

                    // Phase 3.1: Tick agent badge and panel for transitions.
                    self.agent_badge.tick();
                    self.panel.tick();

                    // Phase 2.3: Force render if active transitions/highlights.
                    if self.transition_engine.has_active() || self.highlights.has_active() {
                        needs_render = true;
                    }

                    // Permission deadline guard (defensive fallback only).
                    // In normal operation, permission_deadline is always None —
                    // the agent waits indefinitely for user input.
                    // This block only fires if an external caller explicitly sets a deadline.
                    if matches!(self.state.overlay.active, Some(OverlayKind::PermissionPrompt { .. })) {
                        if let Some(deadline) = self.state.overlay.permission_deadline {
                            if std::time::Instant::now() >= deadline {
                                self.state.overlay.permission_deadline = None;
                                self.send_perm_decision(halcon_core::types::PermissionDecision::Denied);
                                self.permission_modal = None;
                                self.state.overlay.close();
                                self.state.overlay.show_advanced_permissions = false;
                                self.activity_model.push_info("[permission] deadline expired — auto-denied");
                                use crate::tui::input_state::InputState;
                                self.prompt.set_input_state(InputState::Idle);
                                self.highlights.stop("permission_prompt");
                                self.state.agent_control = AgentControl::Running;
                                needs_render = true;
                            } else {
                                // Force re-render on every tick so countdown bar updates live.
                                needs_render = true;
                            }
                        }
                    }
                }
            }

            if self.state.should_quit {
                tracing::debug!(
                    iterations = loop_iterations,
                    "TUI loop exiting: should_quit = true"
                );
                break;
            }
        }

        tracing::debug!(iterations = loop_iterations, "TUI loop completed normally");

        // Restore terminal.
        let mut stdout = io::stdout();
        let _ = stdout.execute(PopKeyboardEnhancementFlags);
        // Phase 93: Disable bracketed paste — non-fatal if terminal doesn't support it.
        let _ = stdout.execute(DisableBracketedPaste);
        stdout.execute(DisableMouseCapture)?;
        terminal::disable_raw_mode()?;
        stdout.execute(LeaveAlternateScreen)?;
        Ok(())
    }
}
