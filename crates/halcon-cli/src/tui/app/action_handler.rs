//! InputAction dispatch handler for TuiApp.
use super::*;

impl TuiApp {
    pub(super) fn handle_action(&mut self, action: input::InputAction) {
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
                    let cmd = trimmed
                        .trim_start_matches('/')
                        .split_whitespace()
                        .next()
                        .unwrap_or("");
                    self.activity_model.push_user_prompt(&text);
                    // Always scroll to bottom on submit so prompt is immediately visible.
                    self.activity_navigator.scroll_to_bottom();
                    self.execute_slash_command(cmd);
                    return;
                }
                // Phase 44B: Allow queueing prompts even when agent is running.
                self.activity_model.push_user_prompt(&text);
                // Always scroll to bottom on submit so prompt is immediately visible.
                self.activity_navigator.scroll_to_bottom();

                // Queue the prompt (unbounded channel never blocks).
                if let Err(e) = self.prompt_tx.send(text) {
                    self.activity_model
                        .push_error(&format!("Failed to queue prompt: {e}"), None);
                    return;
                }

                // Optimistically increment queue count (will be corrected by events).
                self.state.prompts_queued += 1;

                // If agent already running, show toast that prompt was queued.
                if self.state.agent_running {
                    self.toasts.push(Toast::new(
                        format!("Prompt #{} queued", self.state.prompts_queued),
                        ToastLevel::Info,
                    ));
                } else {
                    // First prompt, start agent.
                    // CRITICAL: Keep focus on Prompt so user can type next message while agent processes.
                    // Focus NEVER auto-switches to Activity — user must press Tab to navigate activity.
                    self.state.agent_running = true;
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
                self.state.prompts_queued = 0;
                self.prompt
                    .set_input_state(crate::tui::input_state::InputState::Idle);
                self.activity_model
                    .push_warning("Agent cancelled by user", None);
            }
            input::InputAction::Quit => {
                self.state.should_quit = true;
            }
            input::InputAction::CycleFocus => {
                self.state.cycle_focus();
            }
            input::InputAction::ScrollUp => {
                self.activity_navigator.scroll_up(3);
                // Also scroll panel if visible (panel content may overflow)
                if self.state.panel_visible {
                    self.panel.scroll_up(3);
                }
            }
            input::InputAction::ScrollDown => {
                self.activity_navigator.scroll_down(3);
                // Also scroll panel if visible (panel content may overflow)
                if self.state.panel_visible {
                    // Calculate max_lines from panel content (approximation)
                    let max_lines = self.calculate_panel_content_lines();
                    // Account for borders: inner height is area height - 2
                    let viewport_height = self.last_panel_area.height.saturating_sub(2);
                    self.panel.scroll_down(3, max_lines, viewport_height);
                }
            }
            input::InputAction::ScrollToBottom => {
                self.activity_navigator.scroll_to_bottom();
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
                    crate::tui::state::UiMode::Standard | crate::tui::state::UiMode::Expert => {
                        self.state.panel_visible = true;
                    }
                }
            }
            input::InputAction::PauseAgent => {
                use crate::tui::state::AgentControl;
                if self.state.agent_control == AgentControl::Paused {
                    self.state.agent_control = AgentControl::Running;
                    let _ = self.ctrl_tx.send(ControlEvent::Resume);
                    self.activity_model.push_info("[control] Resumed");
                } else {
                    self.state.agent_control = AgentControl::Paused;
                    let _ = self.ctrl_tx.send(ControlEvent::Pause);
                    self.activity_model
                        .push_info("[control] Paused — Space to resume, N to step");
                }
            }
            input::InputAction::StepAgent => {
                use crate::tui::state::AgentControl;
                self.state.agent_control = AgentControl::StepMode;
                let _ = self.ctrl_tx.send(ControlEvent::Step);
                self.activity_model
                    .push_info("[control] Step mode — executing one step");
            }
            input::InputAction::ApproveAction => {
                self.send_perm_decision(halcon_core::types::PermissionDecision::Allowed);
                self.activity_model.push_info("[control] Action approved");
            }
            input::InputAction::RejectAction => {
                self.send_perm_decision(halcon_core::types::PermissionDecision::Denied);
                self.activity_model
                    .push_warning("[control] Action rejected", None);
            }
            input::InputAction::ApproveAlways => {
                self.send_perm_decision(halcon_core::types::PermissionDecision::AllowedAlways);
                self.activity_model
                    .push_info("[control] Approved always (global)");
            }
            input::InputAction::ApproveDirectory => {
                self.send_perm_decision(
                    halcon_core::types::PermissionDecision::AllowedForDirectory,
                );
                self.activity_model
                    .push_info("[control] Approved for this directory");
            }
            input::InputAction::ApproveSession => {
                self.send_perm_decision(halcon_core::types::PermissionDecision::AllowedThisSession);
                self.activity_model
                    .push_info("[control] Approved for this session");
            }
            input::InputAction::ApprovePattern => {
                self.send_perm_decision(halcon_core::types::PermissionDecision::AllowedForPattern);
                self.activity_model
                    .push_info("[control] Approved for this pattern");
            }
            input::InputAction::DenyDirectory => {
                self.send_perm_decision(halcon_core::types::PermissionDecision::DeniedForDirectory);
                self.activity_model
                    .push_warning("[control] Denied for this directory", None);
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
                // Phase 3 SRCH-004: Search history is pre-loaded at TUI startup (see run() method)
            }
            input::InputAction::DismissToasts => {
                self.toasts.dismiss_all();
            }
            input::InputAction::ToggleConversationFilter => {
                self.activity_model.toggle_conversation_filter();
            }
            input::InputAction::ToggleSubAgentDetail => {
                self.state.show_sub_agent_detail = !self.state.show_sub_agent_detail;
            }

            // Phase A3: SOTA Activity Navigation handlers (only when Activity focused)
            input::InputAction::SelectNextLine => {
                if self.state.focus == FocusZone::Activity {
                    self.activity_navigator.select_next(&self.activity_model);
                }
            }
            input::InputAction::SelectPrevLine => {
                if self.state.focus == FocusZone::Activity {
                    self.activity_navigator.select_prev(&self.activity_model);
                }
            }
            input::InputAction::ToggleExpand => {
                if self.state.focus == FocusZone::Activity {
                    if let Some(idx) = self.activity_navigator.selected() {
                        self.activity_navigator.toggle_expand(idx);
                    }
                }
            }
            input::InputAction::CopySelected => {
                if let Some(idx) = self.activity_navigator.selected() {
                    if let Some(line) = self.activity_model.get(idx) {
                        let text = line.text_content();
                        match super::super::clipboard::copy_to_clipboard(&text) {
                            Ok(()) => {
                                self.toasts.push(Toast::new(
                                    format!("Copied line {} to clipboard", idx + 1),
                                    ToastLevel::Success,
                                ));
                            }
                            Err(e) => {
                                self.toasts.push(Toast::new(
                                    format!("Copy failed: {e}"),
                                    ToastLevel::Error,
                                ));
                            }
                        }
                    }
                }
            }
            input::InputAction::InspectSelected => {
                if let Some(idx) = self.activity_navigator.selected() {
                    let provider = self.status.current_provider().to_string();
                    let model = self.status.current_model().to_string();
                    let session = self.status.session_id().to_string();
                    let cost = self.status.cost_summary();
                    let metrics = self.panel.metrics_summary();
                    self.activity_model.push_info(&format!(
                        "[inspect:line-{idx}] {provider}/{model}  session:{session}  {cost}  {metrics}"
                    ));
                }
            }
            input::InputAction::ExpandAllTools => {
                self.activity_navigator
                    .expand_all_tools(&self.activity_model);
            }
            input::InputAction::CollapseAllTools => {
                self.activity_navigator.collapse_all_tools();
            }
            input::InputAction::JumpToPlan => {
                // Switch panel to Plan view and scroll activity to the plan overview.
                self.state.panel_visible = true;
                self.state.panel_section = crate::tui::state::PanelSection::Plan;
                if let Some(plan_line) = self.activity_model.find_plan_overview_idx() {
                    let viewport_h = self.last_panel_area.height.max(20) as usize;
                    self.activity_navigator
                        .scroll_to_line(plan_line, viewport_h);
                    self.activity_model
                        .push_info("[plan] Jumped to plan overview");
                } else {
                    self.activity_model
                        .push_info("[plan] Plan panel opened (no plan overview yet)");
                }
            }
            input::InputAction::SearchNext => {
                if self.activity_navigator.is_searching() {
                    self.activity_navigator.search_next();
                }
            }
            input::InputAction::SearchPrev => {
                if self.activity_navigator.is_searching() {
                    self.activity_navigator.search_prev();
                }
            }
            input::InputAction::ClearSelection => {
                self.activity_navigator.clear_selection();
            }

            input::InputAction::InsertNewline => {
                self.prompt.insert_newline();
            }
            input::InputAction::PasteFromClipboard => {
                match super::super::clipboard::paste_from_clipboard() {
                    Ok(text) => {
                        self.prompt.insert_str(&text);
                    }
                    Err(e) => {
                        self.toasts.push(Toast::new(
                            format!("Paste failed: {e}"),
                            ToastLevel::Warning,
                        ));
                    }
                }
            }
            input::InputAction::OpenContextServers => {
                self.state.overlay.open(OverlayKind::ContextServers);
                let _ = self.ctrl_tx.send(ControlEvent::RequestContextServers);
            }

            // Phase 45E: Open session browser overlay.
            input::InputAction::OpenSessionList => {
                // Trigger async DB load; result comes back as UiEvent::SessionList.
                if let Some(ref db) = self.db {
                    let db = db.clone();
                    if let Some(ref tx) = self.ui_tx_for_bg {
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            if let Ok(sessions) = db.list_sessions(20).await {
                                let infos: Vec<SessionInfo> = sessions
                                    .into_iter()
                                    .map(|s| SessionInfo {
                                        id: s.id.to_string(),
                                        title: s.title,
                                        provider: s.provider,
                                        model: s.model,
                                        created_at: s.created_at.to_rfc3339(),
                                        updated_at: s.updated_at.to_rfc3339(),
                                        input_tokens: s.total_usage.input_tokens,
                                        output_tokens: s.total_usage.output_tokens,
                                        agent_rounds: s.agent_rounds as usize,
                                        estimated_cost: s.estimated_cost_usd,
                                    })
                                    .collect();
                                let _ = tx.send(UiEvent::SessionList { sessions: infos });
                            }
                        });
                    } else {
                        // No background sender — show empty overlay immediately.
                        self.session_list.clear();
                        self.session_list_selected = 0;
                        self.state.overlay.open(OverlayKind::SessionList);
                    }
                } else {
                    self.session_list.clear();
                    self.session_list_selected = 0;
                    self.state.overlay.open(OverlayKind::SessionList);
                }
            }

            // Copy: Activity focused → copy selected line; Prompt focused → copy all text
            input::InputAction::CopyToClipboard => {
                let text = if self.state.focus == FocusZone::Activity {
                    self.activity_navigator
                        .selected()
                        .and_then(|idx| self.activity_model.get(idx))
                        .map(|line| line.text_content())
                } else {
                    let t = self.prompt.text();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t)
                    }
                };
                if let Some(text) = text {
                    match super::super::clipboard::copy_to_clipboard(&text) {
                        Ok(()) => {
                            self.toasts
                                .push(Toast::new("Copied to clipboard", ToastLevel::Success));
                        }
                        Err(e) => {
                            self.toasts
                                .push(Toast::new(format!("Copy failed: {e}"), ToastLevel::Error));
                        }
                    }
                }
            }

            // Cut: only works on prompt — copies text and clears prompt
            input::InputAction::CutToClipboard => {
                if self.state.focus == FocusZone::Prompt {
                    let text = self.prompt.text();
                    if !text.is_empty() {
                        match super::super::clipboard::copy_to_clipboard(&text) {
                            Ok(()) => {
                                self.prompt.clear();
                                self.toasts
                                    .push(Toast::new("Cut to clipboard", ToastLevel::Success));
                            }
                            Err(e) => {
                                self.toasts.push(Toast::new(
                                    format!("Cut failed: {e}"),
                                    ToastLevel::Error,
                                ));
                            }
                        }
                    }
                }
            }

            // Select all: Prompt focused → select all text (via clear + re-insert pattern)
            input::InputAction::SelectAll => {
                if self.state.focus == FocusZone::Prompt {
                    self.prompt.select_all();
                }
            }

            input::InputAction::OpenSettings => {
                self.state.overlay.open(OverlayKind::Settings);
            }

            input::InputAction::OpenLspStatus => {
                self.state.overlay.open(OverlayKind::LspStatus);
            }

            input::InputAction::OpenModelSelector => {
                let current_model = self.status.current_model().to_string();
                let current_provider = self.status.current_provider().to_string();

                // Build the models list: start from known_models, ensure current is present.
                let mut models = self.known_models.clone();
                let current_key = format!("{}/{}", current_provider, current_model);
                let already_present = models
                    .iter()
                    .any(|(p, m, _)| format!("{p}/{m}") == current_key);
                if !already_present && !current_model.is_empty() {
                    models.insert(
                        0,
                        (
                            current_provider.clone(),
                            current_model.clone(),
                            format!("{}/{}", current_provider, current_model),
                        ),
                    );
                }

                // Pre-select the currently active model.
                let selected = models
                    .iter()
                    .position(|(_, m, _)| m == &current_model)
                    .unwrap_or(0);

                let error_context = self.model_error_context.clone();

                self.state.overlay.open(OverlayKind::ModelSelector {
                    models,
                    selected,
                    current_model,
                    error_context,
                });
            }

            // Phase 93: Open file picker / attachment instructions.
            input::InputAction::OpenFilePicker => {
                self.toasts.push(Toast::new(
                    "Paste a file path or drag a file into this terminal to attach media",
                    ToastLevel::Info,
                ));
            }

            // Phase 93: Remove the last pending media attachment chip.
            input::InputAction::RemoveLastAttachment => {
                if !self.state.pending_attachments.is_empty() {
                    self.state.pending_attachments.pop();
                }
            }

            input::InputAction::ForwardToWidget(key) => {
                use crossterm::event::{KeyCode, KeyModifiers};

                // Enter (no modifiers) when Prompt is focused → SUBMIT the message.
                // When Activity is focused, Enter falls through to activity_controller (toggle expand).
                if key.code == KeyCode::Enter
                    && key.modifiers.is_empty()
                    && self.state.focus == FocusZone::Prompt
                {
                    tracing::debug!("Enter in Prompt zone → submitting prompt");
                    self.handle_action(input::InputAction::SubmitPrompt);
                    return;
                }

                // Ctrl+Enter → submit (backward compat; also matched in dispatch_key but guarded
                // by the SHIFT arm, so it arrives here when entered from overlay path).
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Enter {
                    tracing::debug!("Ctrl+Enter in ForwardToWidget → submitting prompt");
                    self.handle_action(input::InputAction::SubmitPrompt);
                    return;
                }

                // ── Esc: toggle pause/resume when agent is running ──────────────
                // Works regardless of focus zone. If no agent running, Esc falls
                // through to normal routing (clear textarea or activity selection).
                if key.code == KeyCode::Esc && key.modifiers.is_empty() && self.state.agent_running
                {
                    use crate::tui::state::AgentControl;
                    if self.state.agent_control == AgentControl::Paused {
                        self.state.agent_control = AgentControl::Running;
                        let _ = self.ctrl_tx.send(ControlEvent::Resume);
                        self.activity_model.push_info("[control] ▶ Agent resumed");
                    } else {
                        self.state.agent_control = AgentControl::Paused;
                        let _ = self.ctrl_tx.send(ControlEvent::Pause);
                        self.activity_model.push_info(
                            "[control] ⏸ Paused — Esc resume  /step one step  /cancel abort",
                        );
                    }
                    return;
                }

                // ── Up/Down: history navigation in Prompt zone ───────────────────
                // If the cursor is on the first line → Up recalls previous prompt.
                // If the cursor is on the last line  → Down advances to next prompt.
                // Otherwise the key moves the cursor within the multi-line textarea.
                if key.code == KeyCode::Up
                    && key.modifiers.is_empty()
                    && self.state.focus == FocusZone::Prompt
                    && self.prompt.is_on_first_line()
                {
                    self.prompt.history_back();
                    return;
                }
                if key.code == KeyCode::Down
                    && key.modifiers.is_empty()
                    && self.state.focus == FocusZone::Prompt
                    && self.prompt.is_on_last_line()
                {
                    self.prompt.history_forward();
                    return;
                }

                // CRITICAL FIX: Determine if this is a navigation key or a typing key.
                // Navigation keys respect focus for scrolling.
                // ALL other keys ALWAYS go to the prompt (user can ALWAYS type).
                let is_navigation_key = matches!(key.code, KeyCode::Up | KeyCode::Down);

                // Phase A3: Activity-focused navigation keys (J/K vim-style + actions)
                let is_activity_action = matches!(
                    key.code,
                    KeyCode::Char('j')
                        | KeyCode::Char('k')
                        | KeyCode::Char('y')
                        | KeyCode::Char('i')
                        | KeyCode::Char('x')
                        | KeyCode::Char('z')
                        | KeyCode::Char('p')
                        | KeyCode::Char('n')
                        | KeyCode::Char('/')
                        | KeyCode::Enter
                        | KeyCode::Esc
                ) && key.modifiers.is_empty(); // Only when no modifiers (Ctrl+J still goes to prompt)

                if (is_navigation_key || is_activity_action)
                    && self.state.focus == FocusZone::Activity
                {
                    // Phase A3: Route to activity controller when Activity focused
                    if is_activity_action {
                        let ctrl_action = self.activity_controller.handle_key(
                            key,
                            &mut self.activity_navigator,
                            &self.activity_model,
                        );
                        // Execute the returned action
                        match ctrl_action {
                            crate::tui::activity_controller::ControlAction::None => {}
                            crate::tui::activity_controller::ControlAction::ToggleExpand(idx) => {
                                // Phase B1: Start smooth expand/collapse animation
                                let was_expanded = self.activity_navigator.is_expanded(idx);
                                self.activity_navigator.toggle_expand(idx);
                                let now_expanded = self.activity_navigator.is_expanded(idx);

                                // Get current animation progress (or start from 0.0/1.0)
                                let current_progress = self
                                    .expansion_animations
                                    .get(&idx)
                                    .map(|anim| anim.current())
                                    .unwrap_or(if was_expanded { 1.0 } else { 0.0 });

                                // Start animation in opposite direction
                                let anim = if now_expanded {
                                    ExpansionAnimation::expand_from(current_progress)
                                } else {
                                    ExpansionAnimation::collapse_from(current_progress)
                                };
                                self.expansion_animations.insert(idx, anim);
                            }
                            crate::tui::activity_controller::ControlAction::CopyOutput(idx) => {
                                if let Some(line) = self.activity_model.get(idx) {
                                    // Phase A3: Clipboard copy implementation
                                    let text = line.text_content();
                                    match super::super::clipboard::copy_to_clipboard(&text) {
                                        Ok(()) => {
                                            self.toasts.push(Toast::new(
                                                "Copied to clipboard",
                                                ToastLevel::Success,
                                            ));
                                        }
                                        Err(e) => {
                                            self.toasts.push(Toast::new(
                                                format!("Copy failed: {}", e),
                                                ToastLevel::Error,
                                            ));
                                        }
                                    }
                                }
                            }
                            crate::tui::activity_controller::ControlAction::JumpToPlanStep(
                                step_idx,
                            ) => {
                                // Switch side panel to Plan view and scroll activity to the plan overview.
                                self.state.panel_visible = true;
                                self.state.panel_section = crate::tui::state::PanelSection::Plan;
                                if let Some(plan_line) =
                                    self.activity_model.find_plan_overview_idx()
                                {
                                    let viewport_h = self.last_panel_area.height.max(20) as usize;
                                    self.activity_navigator
                                        .scroll_to_line(plan_line, viewport_h);
                                    self.activity_model.push_info(&format!(
                                        "[plan] Jumped to step {} — plan overview above",
                                        step_idx + 1
                                    ));
                                } else {
                                    self.activity_model.push_info(&format!(
                                        "[plan] Step {} — plan panel opened (no plan overview yet)",
                                        step_idx + 1
                                    ));
                                }
                            }
                            crate::tui::activity_controller::ControlAction::OpenInspector(
                                target,
                            ) => {
                                // Show inspection data inline in the activity feed.
                                let provider = self.status.current_provider().to_string();
                                let model = self.status.current_model().to_string();
                                let session = self.status.session_id().to_string();
                                let cost = self.status.cost_summary();
                                let metrics = self.panel.metrics_summary();
                                self.activity_model.push_info(&format!(
                                    "[inspect:{:?}] {provider}/{model}  session:{session}  {cost}  {metrics}",
                                    target
                                ));
                            }
                            crate::tui::activity_controller::ControlAction::FilterByTool(tool) => {
                                // Open Search overlay pre-filled with the tool name.
                                self.state.overlay.open(OverlayKind::Search);
                                for ch in tool.chars() {
                                    self.state.overlay.type_char(ch);
                                }
                                self.activity_model.push_info(&format!(
                                    "[filter] Showing tool: {tool} — use n/N to navigate matches"
                                ));
                            }
                            crate::tui::activity_controller::ControlAction::SlashCommand(cmd) => {
                                // Route directly to execute_slash_command for unified handling.
                                self.execute_slash_command(&cmd);
                            }
                        }
                    } else {
                        // Arrow keys in Activity zone → scroll via navigator
                        match key.code {
                            KeyCode::Up => self.activity_navigator.scroll_up(1),
                            KeyCode::Down => self.activity_navigator.scroll_down(1),
                            _ => unreachable!(),
                        }
                    }
                } else {
                    // ALL other keys (chars, backspace, enter, etc.) → ALWAYS to prompt
                    // This ensures input is NEVER blocked, regardless of focus or agent state.
                    self.prompt.handle_key(key);

                    // ── Slash autocomplete mirror ──────────────────────────────────────
                    // After every keystroke, inspect the current prompt text.  If it
                    // looks like the start of a slash command (e.g. "/", "/pa", "/pause")
                    // open (or keep open) the CommandPalette and sync its filter to the
                    // portion after the `/`.  Once the user clears the slash or adds a
                    // space (indicating they want the command + arguments), we close the
                    // palette and reset the flag.
                    {
                        let text = self.prompt.text();
                        let trimmed = text.trim_start();
                        // Only activate for single-line "/xxx" prefix with no spaces after the command.
                        let is_slash_prefix = trimmed.starts_with('/')
                            && !trimmed[1..].contains(' ')
                            && !trimmed.contains('\n');
                        if is_slash_prefix {
                            let query = trimmed.trim_start_matches('/').to_string();
                            if !self.slash_completing {
                                self.slash_completing = true;
                                self.state.overlay.open(OverlayKind::CommandPalette);
                                self.state.overlay.selected = 0;
                            }
                            self.state.overlay.input = query.clone();
                            let all = overlay::default_commands();
                            self.state.overlay.filtered_items =
                                overlay::filter_commands(&all, &query);
                            let max = self.state.overlay.filtered_items.len();
                            if self.state.overlay.selected >= max {
                                self.state.overlay.selected = max.saturating_sub(1);
                            }
                        } else if self.slash_completing {
                            // User cleared the `/` prefix or added a space → dismiss palette.
                            self.slash_completing = false;
                            self.state.overlay.close();
                        }
                    }

                    // Track typing activity for indicator (only for actual typing, not navigation).
                    if !is_navigation_key && !is_activity_action {
                        self.state.typing_indicator = true;
                        self.state.last_keystroke = std::time::Instant::now();
                    }
                }
            }
        }
    }
}
