//! Overlay keyboard event handler for TuiApp.
use super::*;

impl TuiApp {
    /// Handle key events when an overlay is active.
    pub(super) fn handle_overlay_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        // Phase I-6C: Route permission prompt input through conversational overlay.
        if matches!(self.state.overlay.active, Some(OverlayKind::PermissionPrompt { .. })) {
            // Special case: Esc always closes and sends Denied to unblock authorize().
            if matches!(key.code, KeyCode::Esc) {
                // Fix #6 (Bug #6): Esc was closing the modal visually but NOT sending
                // a decision to `perm_tx`, leaving `permissions.authorize()` blocked
                // on the 60s timeout. Always send Denied when user cancels with Esc.
                self.send_perm_decision(halcon_core::types::PermissionDecision::Denied);
                self.conversational_overlay = None;
                self.permission_modal = None; // Phase 2.2
                self.state.agent_control = AgentControl::Running;
                self.state.overlay.close();
                self.state.overlay.show_advanced_permissions = false; // Phase 6: Reset flag

                // Phase 2.1: Restore input state after canceling permission
                use crate::tui::input_state::InputState;
                self.prompt.set_input_state(InputState::Idle);

                self.activity_model.push_warning("[permission] Denied (canceled)", None);
                tracing::debug!("Permission canceled (Esc) — Denied sent to unblock authorize()");
                return;
            }

            // Phase 6: F1 toggles advanced permission options (progressive disclosure).
            if matches!(key.code, KeyCode::F(1)) {
                self.state.overlay.show_advanced_permissions = !self.state.overlay.show_advanced_permissions;
                tracing::debug!(
                    show_advanced = self.state.overlay.show_advanced_permissions,
                    "Toggled advanced permission options"
                );
                return;
            }

            // ========================================================================
            // CRITICAL INTEGRATION POINT: 8-Option Permission Modal Key Routing
            // ========================================================================
            //
            // Phase 5/6/7: Direct key-to-option mapping for permission modal.
            //
            // This is the CORRECT implementation that makes all 8 permission options
            // functional. Keys map directly to PermissionOptions without going through
            // a conversational overlay.
            //
            // KEY BINDINGS:
            // - Y/y → Yes (approve once)
            // - N/n → No (reject once)
            // - A/a → AlwaysThisTool (global approval) - only when advanced shown
            // - D/d → ThisDirectory (directory-scoped) - only when advanced shown
            // - S/s → ThisSession (session-scoped) - only when advanced shown
            // - P/p → ThisPattern (pattern-matched) - only when advanced shown
            // - X/x → NeverThisDirectory (directory denial) - only when advanced shown
            // - Esc → Cancel (handled above at line 743)
            // - F1 → Toggle advanced options (handled above at line 763)
            //
            // PROGRESSIVE DISCLOSURE (Phase 6):
            // Advanced options (A/D/S/P/X) only work when show_advanced_permissions=true
            // (toggled with F1 key). This prevents accidental over-permissioning.
            //
            // DO NOT route through conversational_overlay! That was the old Phase I-6C
            // implementation that only supported yes/no text input.
            //
            // FIX HISTORY: Previously routed ALL input to conversational overlay
            // (CRITICAL BUG #2). Fixed: 2026-02-15, now uses direct key mapping.
            // ========================================================================

            use crate::tui::permission_context::PermissionOption;

            let permission_option = match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => Some(PermissionOption::Yes),
                KeyCode::Char('n') | KeyCode::Char('N') => Some(PermissionOption::No),
                KeyCode::Char('a') | KeyCode::Char('A') if self.state.overlay.show_advanced_permissions => {
                    Some(PermissionOption::AlwaysThisTool)
                }
                KeyCode::Char('d') | KeyCode::Char('D') if self.state.overlay.show_advanced_permissions => {
                    Some(PermissionOption::ThisDirectory)
                }
                KeyCode::Char('s') | KeyCode::Char('S') if self.state.overlay.show_advanced_permissions => {
                    Some(PermissionOption::ThisSession)
                }
                KeyCode::Char('p') | KeyCode::Char('P') if self.state.overlay.show_advanced_permissions => {
                    Some(PermissionOption::ThisPattern)
                }
                KeyCode::Char('x') | KeyCode::Char('X') if self.state.overlay.show_advanced_permissions => {
                    Some(PermissionOption::NeverThisDirectory)
                }
                _ => None, // Ignore unrecognized keys
            };

            if let Some(option) = permission_option {
                // Get risk level from modal to check if option is available
                let is_option_available = if let Some(ref modal) = self.permission_modal {
                    let available_options = modal.risk_level().available_options();
                    available_options.contains(&option)
                } else {
                    true // Fallback: allow if modal not present
                };

                if !is_option_available {
                    // Option not available at this risk level (e.g., AlwaysThisTool for Critical)
                    self.activity_model.push_warning(
                        &format!("[permission] Option '{}' not available at this risk level", option.label()),
                        None,
                    );
                    return;
                }

                // Convert PermissionOption to PermissionDecision
                let decision = option.to_decision();
                self.send_perm_decision(decision);

                let is_approved = !matches!(
                    decision,
                    halcon_core::types::PermissionDecision::Denied
                        | halcon_core::types::PermissionDecision::DeniedForDirectory
                );
                let status_msg = format!("[control] {} - {}", option.label(), if is_approved { "Approved" } else { "Denied" });
                if is_approved {
                    self.activity_model.push_info(&status_msg);
                } else {
                    self.activity_model.push_warning(&status_msg, None);
                }

                // Close modal and restore state
                self.conversational_overlay = None;
                self.permission_modal = None;
                self.state.agent_control = AgentControl::Running;
                self.state.overlay.close();
                self.state.overlay.show_advanced_permissions = false;

                use crate::tui::input_state::InputState;
                self.prompt.set_input_state(InputState::Idle);

                self.highlights.stop("permission_prompt");
                self.agent_badge.set_state(AgentState::Running);
                self.agent_badge.set_detail(Some("Continuing...".to_string()));

                tracing::debug!(
                    decision = ?decision,
                    option = ?option,
                    input_state = ?self.prompt.input_state(),
                    "Permission resolved via 8-option modal"
                );
            }
            return;
        }

        // Phase 50: Sudo password entry overlay — masked input with remember toggle.
        if matches!(self.state.overlay.active, Some(OverlayKind::SudoPasswordEntry { .. })) {
            use crossterm::event::{KeyCode, KeyModifiers};
            match key.code {
                KeyCode::Esc => {
                    // User cancelled — send None to unblock the executor.
                    let _ = self.sudo_pw_tx.as_ref().map(|tx| tx.send(None));
                    self.sudo_password_buf.clear();
                    self.state.overlay.close();
                    self.activity_model.push_warning("[sudo] Password entry cancelled", None);
                    tracing::debug!("Sudo password entry cancelled by user");
                }
                KeyCode::Enter => {
                    // Submit the password (empty = user just hit Enter, still valid for cached-cred cases).
                    let pw = self.sudo_password_buf.clone();

                    // If "Remember" toggle is on, cache with 5-minute TTL.
                    if self.sudo_remember_password && !pw.is_empty() {
                        self.sudo_cache = Some((pw.clone(), std::time::Instant::now()));
                        self.sudo_has_cached = true;
                        tracing::debug!("Sudo password cached for 5 minutes");
                    }

                    let _ = self.sudo_pw_tx.as_ref().map(|tx| tx.send(Some(pw)));
                    self.sudo_password_buf.clear();
                    self.state.overlay.close();
                    self.activity_model.push_info("[sudo] Password submitted — elevating privileges");
                    tracing::debug!("Sudo password submitted");
                }
                KeyCode::Tab => {
                    // Toggle "Remember for 5 minutes".
                    self.sudo_remember_password = !self.sudo_remember_password;
                }
                KeyCode::Char('c') | KeyCode::Char('C')
                    if key.modifiers == KeyModifiers::NONE
                        && self.sudo_has_cached =>
                {
                    // Use cached password immediately.
                    if let Some((ref pw, _)) = self.sudo_cache {
                        let _ = self.sudo_pw_tx.as_ref().map(|tx| tx.send(Some(pw.clone())));
                    }
                    self.sudo_password_buf.clear();
                    self.state.overlay.close();
                    self.activity_model.push_info("[sudo] Using cached password");
                    tracing::debug!("Using cached sudo password");
                }
                KeyCode::Backspace => {
                    // Remove last character from masked password buffer.
                    self.sudo_password_buf.pop();
                }
                KeyCode::Char(c) if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT => {
                    // Append printable character to password buffer (never echoed).
                    self.sudo_password_buf.push(c);
                }
                _ => {}
            }
            return;
        }

        // Phase 45E: Session list overlay gets its own key routing.
        if matches!(self.state.overlay.active, Some(OverlayKind::SessionList)) {
            match key.code {
                KeyCode::Esc => {
                    self.state.overlay.close();
                }
                KeyCode::Up => {
                    if self.session_list_selected > 0 {
                        self.session_list_selected -= 1;
                    }
                }
                KeyCode::Down => {
                    if self.session_list_selected + 1 < self.session_list.len() {
                        self.session_list_selected += 1;
                    }
                }
                KeyCode::Enter => {
                    if let Some(session) = self.session_list.get(self.session_list_selected) {
                        let id = session.id.clone();
                        let short_id = if id.len() >= 8 { id[..8].to_string() } else { id.clone() };
                        let _ = self.ctrl_tx.send(ControlEvent::ResumeSession(id));
                        self.state.overlay.close();
                        self.activity_model.push_info(&format!(
                            "⟳ Loading session {}…", short_id
                        ));
                    }
                }
                _ => {}
            }
            return;
        }

        // Phase 94: Init wizard overlay key routing.
        if matches!(self.state.overlay.active, Some(OverlayKind::InitWizard { .. })) {
            match key.code {
                KeyCode::Esc => {
                    self.state.overlay.close();
                }
                KeyCode::Enter => {
                    if let Some(OverlayKind::InitWizard { ref mut step, ref preview, ref save_path, dry_run }) =
                        self.state.overlay.active
                    {
                        match *step {
                            1 => *step = 2,
                            2 => *step = 3,
                            3 => {
                                if dry_run {
                                    // Dry-run: just advance to done without writing.
                                    *step = 4;
                                    self.activity_model.push_info("[onboarding] dry-run: archivo no escrito");
                                } else {
                                    // Write the file.
                                    let path_str = save_path.clone();
                                    let content = preview.clone();
                                    let path = std::path::Path::new(&path_str);
                                    let write_ok = if let Some(parent) = path.parent() {
                                        std::fs::create_dir_all(parent).is_ok()
                                    } else {
                                        true
                                    };
                                    if write_ok {
                                        match std::fs::write(path, content.as_bytes()) {
                                            Ok(_) => {
                                                self.activity_model.push_info(
                                                    &format!("[onboarding] ✓ Guardado: {path_str}")
                                                );
                                                *step = 4;
                                            }
                                            Err(e) => {
                                                self.toasts.push(Toast::new(
                                                    format!("Error al guardar: {e}"),
                                                    ToastLevel::Warning,
                                                ));
                                            }
                                        }
                                    } else {
                                        self.toasts.push(Toast::new(
                                            format!("Error al crear directorio para: {path_str}"),
                                            ToastLevel::Warning,
                                        ));
                                    }
                                }
                            }
                            _ => {
                                // Step 0 (analyzing) or step 4 (done) — close on Enter.
                                self.state.overlay.close();
                            }
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        // Update-available overlay key routing.
        if matches!(self.state.overlay.active, Some(OverlayKind::UpdateAvailable { .. })) {
            match key.code {
                KeyCode::Enter => {
                    // Signal the caller to run the update after TUI exits.
                    if let Some(ref sig) = self.update_install_signal {
                        sig.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    self.state.overlay.close();
                    self.state.should_quit = true;
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                    self.state.overlay.close();
                    self.toasts.push(Toast::new(
                        "Actualización pospuesta — usa `halcon update` cuando quieras".to_string(),
                        ToastLevel::Info,
                    ));
                }
                _ => {}
            }
            return;
        }

        // Phase 95: PluginSuggest overlay key routing.
        if matches!(self.state.overlay.active, Some(OverlayKind::PluginSuggest { .. })) {
            match key.code {
                KeyCode::Up => {
                    if let Some(OverlayKind::PluginSuggest { ref mut selected, .. }) =
                        self.state.overlay.active
                    {
                        if *selected > 0 {
                            *selected -= 1;
                        }
                    }
                }
                KeyCode::Down => {
                    if let Some(OverlayKind::PluginSuggest { ref mut selected, ref suggestions, .. }) =
                        self.state.overlay.active
                    {
                        if *selected + 1 < suggestions.len() {
                            *selected += 1;
                        }
                    }
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.activity_model.push_info(
                        "[plugins] Type /plugins auto to install Essential + Recommended"
                    );
                    self.state.overlay.close();
                }
                KeyCode::Esc => {
                    self.state.overlay.close();
                }
                _ => {}
            }
            return;
        }

        // Non-permission overlays: use original logic.
        match key.code {
            KeyCode::Esc => {
                if matches!(self.state.overlay.active, Some(OverlayKind::Search)) {
                    self.search_matches.clear();
                    self.search_current = 0;
                    // Phase 3 SRCH-004: Reset history navigation state when closing search
                    self.activity_navigator.reset_history_nav();
                }
                // Always reset slash_completing when the overlay is dismissed.
                self.slash_completing = false;
                self.state.overlay.close();
            }
            KeyCode::Enter => {
                match &self.state.overlay.active {
                    Some(OverlayKind::CommandPalette) => {
                        let action = self.state.overlay.filtered_items
                            .get(self.state.overlay.selected)
                            .map(|item| item.action.clone());
                        // If we got here via slash-typing, clear the /xxx prefix from the prompt.
                        let was_slash = self.slash_completing;
                        self.slash_completing = false;
                        self.state.overlay.close();
                        if was_slash {
                            self.prompt.clear();
                        }
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
                    // Phase 3 SRCH-004: Ctrl+Up = navigate search history (older queries)
                    if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                        if let Some(query) = self.activity_navigator.history_up() {
                            self.state.overlay.input = query.clone();
                            self.rerun_search();
                        }
                    } else {
                        // Plain Up = navigate to previous match
                        self.search_prev();
                    }
                } else {
                    self.state.overlay.select_prev();
                }
            }
            KeyCode::Down => {
                if matches!(self.state.overlay.active, Some(OverlayKind::Search)) {
                    // Phase 3 SRCH-004: Ctrl+Down = navigate search history (newer queries)
                    if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                        if let Some(query) = self.activity_navigator.history_down() {
                            self.state.overlay.input = query.clone();
                            self.rerun_search();
                        }
                    } else {
                        // Plain Down = navigate to next match
                        self.search_next();
                    }
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

    /// Route a permission decision to the correct executor.
    ///
    /// If a sub-agent registered a reply channel via `PermissionAwaiting.reply_tx`,
    /// the decision goes there. Otherwise it goes to the main agent's `perm_tx`.
    /// The sub-agent slot is consumed (take) so subsequent requests start fresh.
    pub(super) fn send_perm_decision(&mut self, decision: halcon_core::types::PermissionDecision) {
        // Clear countdown so the run_loop tick handler doesn't fire a redundant auto-deny.
        self.state.overlay.permission_deadline = None;
        if let Some(tx) = self.pending_perm_reply_tx.take() {
            let _ = tx.send(decision);
        } else {
            let _ = self.perm_tx.send(decision);
        }
    }
}
