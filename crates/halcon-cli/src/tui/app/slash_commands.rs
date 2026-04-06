//! Slash command execution and command palette filtering for TuiApp.
use super::*;

impl TuiApp {
    /// Execute a slash command by action name.
    pub(super) fn execute_slash_command(&mut self, cmd: &str) {
        match cmd {
            // --- Agent control commands ---
            "pause" => {
                use crate::tui::state::AgentControl;
                if !self.state.agent_running {
                    self.activity_model
                        .push_warning("[pause] No agent is running", None);
                    return;
                }
                self.state.agent_control = AgentControl::Paused;
                let _ = self.ctrl_tx.send(ControlEvent::Pause);
                self.activity_model.push_info("[control] ⏸ Agent paused — /resume to continue, /step for one step, /cancel to abort");
            }
            "resume" => {
                use crate::tui::state::AgentControl;
                if self.state.agent_control != AgentControl::Paused {
                    self.activity_model
                        .push_warning("[resume] Agent is not paused", None);
                    return;
                }
                self.state.agent_control = AgentControl::Running;
                let _ = self.ctrl_tx.send(ControlEvent::Resume);
                self.activity_model.push_info("[control] ▶ Agent resumed");
            }
            "step" => {
                use crate::tui::state::AgentControl;
                if !self.state.agent_running {
                    self.activity_model
                        .push_warning("[step] No agent is running", None);
                    return;
                }
                self.state.agent_control = AgentControl::StepMode;
                let _ = self.ctrl_tx.send(ControlEvent::Step);
                self.activity_model
                    .push_info("[control] ⏭ Step mode — executing one agent step");
            }
            "cancel" => {
                if !self.state.agent_running {
                    self.activity_model
                        .push_warning("[cancel] No agent is running", None);
                    return;
                }
                let _ = self.ctrl_tx.send(ControlEvent::CancelAgent);
                self.state.agent_running = false;
                self.activity_model.push_info("[control] ✕ Agent cancelled");
            }
            // --- Session info commands ---
            "status" => {
                let provider = self.status.current_provider();
                let model = self.status.current_model();
                let agent_state = if self.state.agent_running {
                    "running"
                } else {
                    "idle"
                };
                let session = self.status.session_id();
                self.activity_model.push_info(&format!(
                    "[status] Provider: {provider} | Model: {model} | Agent: {agent_state} | Session: {session}"
                ));
            }
            "session" => {
                let session = self.status.session_id();
                let provider = self.status.current_provider();
                let model = self.status.current_model();
                self.activity_model
                    .push_info(&format!("[session] ID: {session} | {provider}/{model}"));
            }
            "metrics" => {
                let metrics = self.panel.metrics_summary();
                self.activity_model
                    .push_info(&format!("[metrics] {metrics}"));
            }
            "context" => {
                let ctx = self.panel.context_summary();
                self.activity_model.push_info(&format!("[context] {ctx}"));
            }
            "cost" => {
                let cost = self.status.cost_summary();
                self.activity_model.push_info(&format!("[cost] {cost}"));
            }
            "history" => {
                let count = self.activity_model.len();
                self.activity_model.push_info(&format!(
                    "[history] {count} activity lines in current session"
                ));
            }
            "why" => {
                let reasoning = self.panel.reasoning_summary();
                self.activity_model
                    .push_info(&format!("[reasoning] {reasoning}"));
            }
            "inspect" => {
                let provider = self.status.current_provider();
                let model = self.status.current_model();
                let session = self.status.session_id();
                let cost = self.status.cost_summary();
                let metrics = self.panel.metrics_summary();
                self.activity_model.push_info(&format!(
                    "[inspect] Session: {session} | {provider}/{model} | {cost} | {metrics}"
                ));
            }
            // --- UI commands ---
            "help" => {
                self.state.overlay.open(OverlayKind::Help);
            }
            "model" | "model select" | "models" => {
                // Open the model selector overlay (same as Ctrl+M).
                self.handle_action(input::InputAction::OpenModelSelector);
            }
            "mode" => {
                self.handle_action(input::InputAction::CycleUiMode);
            }
            "plan" => {
                self.state.panel_visible = true;
                self.state.panel_section = crate::tui::state::PanelSection::Plan;
                self.activity_model
                    .push_info("[plan] Side panel switched to Plan view");
            }
            "panel" => {
                self.state.panel_visible = !self.state.panel_visible;
            }
            "search" => {
                self.state.overlay.open(OverlayKind::Search);
            }
            "clear" => {
                self.activity_model.clear();
            }
            "quit" => {
                self.state.should_quit = true;
            }
            // --- Extended commands ---
            "init" => {
                self.state.overlay.open(OverlayKind::InitWizard {
                    step: 0,
                    preview: String::new(),
                    save_path: String::new(),
                    dry_run: false,
                });
                // Spawn real-time background analysis — emits UiEvent::Info progress
                // messages and finally UiEvent::ProjectAnalysisComplete to advance
                // the wizard from Step 0 → Step 1.
                if let Some(ref tx) = self.ui_tx_for_bg {
                    let tx = tx.clone();
                    let cwd =
                        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                    tokio::spawn(async move {
                        super::super::project_analyzer::analyze_and_emit(tx, cwd).await;
                    });
                }
            }
            "tools" => {
                let metrics = self.panel.metrics_summary();
                self.activity_model.push_info(&format!(
                    "[tools] Tool usage this session — {metrics}  |  Use /inspect for full details"
                ));
            }
            "plugins" => {
                self.activity_model.push_info(
                    "[plugins] Plugin system is active — use ~/.halcon/config.toml to configure plugins",
                );
            }
            "dry-run" | "dryrun" => {
                self.state.dry_run_active = !self.state.dry_run_active;
                let state = if self.state.dry_run_active {
                    "ON"
                } else {
                    "OFF"
                };
                self.activity_model.push_info(&format!(
                    "[dry-run] Dry-run mode {state} — destructive tools will be skipped"
                ));
            }
            "reasoning" => {
                let summary = self.panel.reasoning_summary();
                self.activity_model
                    .push_info(&format!("[reasoning] {summary}"));
            }
            "experts" | "expert" => {
                self.handle_action(input::InputAction::CycleUiMode);
                self.activity_model
                    .push_info("[mode] Cycled UI display mode");
            }
            "settings" | "config" => {
                self.handle_action(input::InputAction::OpenSettings);
            }
            "lsp" => {
                self.handle_action(input::InputAction::OpenLspStatus);
            }
            // --- Remote Control ---
            "remote-control" => {
                // Show active remote-control session status from persisted state.
                let rc_path = dirs::data_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                    .join("halcon")
                    .join("active_rc_session");
                match std::fs::read_to_string(&rc_path) {
                    Ok(sid) if !sid.is_empty() => {
                        self.activity_model.push_info(&format!(
                            "[remote-control] Active session: {} — use /remote-control attach or `halcon remote-control status`",
                            sid.trim()
                        ));
                    }
                    _ => {
                        self.activity_model.push_info(
                            "[remote-control] No active session. Start one with `halcon remote-control start`",
                        );
                    }
                }
            }
            "remote-control-attach" => {
                let rc_path = dirs::data_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                    .join("halcon")
                    .join("active_rc_session");
                match std::fs::read_to_string(&rc_path) {
                    Ok(sid) if !sid.is_empty() => {
                        self.activity_model.push_info(&format!(
                            "[remote-control] To attach, run in a terminal:\n  halcon remote-control attach -s {}",
                            sid.trim()
                        ));
                    }
                    _ => {
                        self.activity_model.push_warning(
                            "[remote-control] No active session to attach to",
                            Some("Start one with `halcon remote-control start`"),
                        );
                    }
                }
            }
            other => {
                self.activity_model.push_warning(
                    &format!("[cmd] Unknown command: /{other}"),
                    Some("Type / to browse all available commands"),
                );
            }
        }
    }

    /// Re-filter the command palette items based on current overlay input.
    pub(super) fn refilter_palette(&mut self) {
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
}
