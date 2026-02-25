use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::state::{ActiveView, AppState, ConnectionState};
use crate::theme::HalconTheme;
use crate::views;
use crate::workers::{BackendMessage, UiCommand};

/// Maximum number of streaming tokens processed per egui frame.
/// At 60fps: 10 × 60 = 600 tokens/s — well above deepseek-chat's ~200 tokens/s.
/// Prevents a burst of accumulated tokens from making a single frame take >16ms.
const MAX_TOKENS_PER_FRAME: usize = 10;

/// The main desktop application.
pub struct HalconApp {
    state: AppState,
    config: AppConfig,
    cmd_tx: mpsc::Sender<UiCommand>,
    msg_rx: mpsc::Receiver<BackendMessage>,
    theme_applied: bool,
    /// Keep the tokio runtime alive for the lifetime of the app.
    _runtime: tokio::runtime::Runtime,
}

impl HalconApp {
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        cmd_tx: mpsc::Sender<UiCommand>,
        msg_rx: mpsc::Receiver<BackendMessage>,
        runtime: tokio::runtime::Runtime,
    ) -> Self {
        let config = AppConfig::load();
        let mut state = AppState::default();

        // Allow overriding the initial view via env var (for testing/automation).
        if let Ok(view) = std::env::var("HALCON_VIEW") {
            state.active_view = match view.as_str() {
                "dashboard" => ActiveView::Dashboard,
                "agents" => ActiveView::Agents,
                "tasks" => ActiveView::Tasks,
                "tools" => ActiveView::Tools,
                "logs" => ActiveView::Logs,
                "metrics" => ActiveView::Metrics,
                "protocols" => ActiveView::Protocols,
                "files" => ActiveView::Files,
                "settings" => ActiveView::Settings,
                _ => ActiveView::Dashboard,
            };
        }

        // Auto-connect if HALCON_API_TOKEN env var is set.
        if config.has_auto_connect() {
            state.connection = ConnectionState::Connecting;
            state.show_connect_dialog = false;
            let _ = cmd_tx.try_send(UiCommand::Connect {
                url: config.server_url.clone(),
                token: config.auth_token.clone(),
            });
        }

        Self {
            state,
            config,
            cmd_tx,
            msg_rx,
            theme_applied: false,
            _runtime: runtime,
        }
    }

    /// Drain pending backend messages and update state.
    ///
    /// Rate-limits streaming tokens to MAX_TOKENS_PER_FRAME to prevent a burst
    /// of accumulated tokens from stalling the frame. During streaming or when a
    /// permission modal is active, schedules a repaint every ~33ms so the countdown
    /// timer animates smoothly even when no new tokens arrive.
    fn process_backend_messages(&mut self, ctx: &egui::Context) {
        let mut tokens_this_frame: usize = 0;

        loop {
            match self.msg_rx.try_recv() {
                Err(_) => break,
                Ok(msg) => match msg {
                BackendMessage::Connected => {
                    self.state.connection = ConnectionState::Connected;
                    self.state.show_connect_dialog = false;
                    // Persist the URL + token so they're available on next launch.
                    if let Err(e) = self.config.save() {
                        tracing::warn!(error = %e, "failed to save desktop config");
                    }
                    // Trigger initial data load.
                    let _ = self.cmd_tx.try_send(UiCommand::RefreshAgents);
                    let _ = self.cmd_tx.try_send(UiCommand::RefreshTasks);
                    let _ = self.cmd_tx.try_send(UiCommand::RefreshTools);
                    let _ = self.cmd_tx.try_send(UiCommand::RefreshMetrics);
                    let _ = self.cmd_tx.try_send(UiCommand::RefreshStatus);
                    let _ = self.cmd_tx.try_send(UiCommand::RefreshConfig);
                    // Load existing chat sessions into the sidebar.
                    let _ = self.cmd_tx.try_send(UiCommand::LoadChatSessions);
                }
                BackendMessage::Disconnected(reason) => {
                    self.state.connection = ConnectionState::Error(reason);
                    self.state.runtime_config = None;
                    self.state.config_dirty = false;
                    self.state.config_error = None;
                    // A1/A5 — Clear streaming state so the UI never gets stuck in
                    // "Agent is running…" after a network drop.  last_sequence_num is
                    // reset so the first token of the next turn is never silently dropped
                    // as a "duplicate" of a stale sequence from the previous connection.
                    self.state.chat.is_streaming = false;
                    self.state.chat.streaming_token.clear();
                    self.state.chat.streaming_token_count = 0;
                    self.state.chat.turn_started_at = None;
                    self.state.chat.last_sequence_num = None;
                    // Dismiss any pending permission modal — it cannot be resolved
                    // without a live connection to the server.
                    self.state.chat.permission_modal = None;
                    // Auto-reconnect after 5 seconds if we have connection config.
                    if self.config.has_auto_connect() {
                        self.state.reconnect_after =
                            Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                    }
                }
                BackendMessage::ConnectionError(err) => {
                    self.state.connection = ConnectionState::Error(err);
                    // Retry after 5 seconds if we have auto-connect config.
                    if self.config.has_auto_connect() {
                        self.state.reconnect_after =
                            Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                    }
                }
                BackendMessage::AgentsUpdated(agents) => {
                    self.state.agents = agents;
                }
                BackendMessage::TasksUpdated(tasks) => {
                    self.state.tasks = tasks;
                }
                BackendMessage::ToolsUpdated(tools) => {
                    self.state.tools = tools;
                }
                BackendMessage::MetricsUpdated(m) => {
                    // Feed trend charts before storing the snapshot so the charts
                    // always contain the same data visible in the numeric section.
                    self.state.charts.events_per_sec.push(m.events_per_second);
                    self.state.charts.active_tasks.push(m.active_tasks as f64);
                    self.state.metrics = Some(m);
                }
                BackendMessage::SystemStatusUpdated(s) => {
                    self.state.system_status = Some(s);
                }
                BackendMessage::ConfigLoaded(cfg) => {
                    self.state.runtime_config = Some(cfg);
                    self.state.config_dirty = false;
                    self.state.config_error = None;
                }
                BackendMessage::ConfigUpdated(cfg) => {
                    self.state.runtime_config = Some(cfg);
                    self.state.config_dirty = false;
                    self.state.config_error = None;
                }
                BackendMessage::ConfigError(err) => {
                    self.state.config_error = Some(err);
                }
                BackendMessage::ChatSessionCreated(session) => {
                    self.state.chat.sessions.push(session.clone());
                    self.state.chat.active_session = Some(session.id);
                    self.state.active_view = crate::state::ActiveView::Chat;
                }
                BackendMessage::ChatSessionsLoaded(sessions) => {
                    self.state.chat.sessions = sessions;
                }
                BackendMessage::ChatMessagesLoaded { session_id, messages } => {
                    // Only apply if this is still the active session (user may have switched).
                    if self.state.chat.active_session == Some(session_id) {
                        self.state.chat.messages.clear();
                        self.state.chat.messages_loading = false;
                        self.state.chat.messages_visible_count = crate::state::CHAT_PAGE_SIZE;
                        for msg in messages {
                            let role = match msg.role.as_str() {
                                "assistant" => crate::state::ChatDisplayRole::Assistant,
                                "system" => crate::state::ChatDisplayRole::System,
                                _ => crate::state::ChatDisplayRole::User,
                            };
                            self.state.chat.messages.push_back(crate::state::ChatDisplayMessage {
                                id: uuid::Uuid::new_v4(),
                                role,
                                content: msg.content,
                                timestamp: chrono::Utc::now(),
                            });
                        }
                    }
                }
                BackendMessage::ChatMessageReceived { session_id, token, is_thinking, sequence_num } => {
                    // A2 — Route by session_id: silently drop tokens for any session
                    // that is not the one currently displayed.  This prevents messages
                    // from a background or deleted session from leaking into the active view.
                    if self.state.chat.active_session != Some(session_id) {
                        continue;
                    }
                    // Drop duplicate tokens that can re-arrive after a WS reconnect.
                    if let Some(last) = self.state.chat.last_sequence_num {
                        if sequence_num <= last {
                            continue;
                        }
                        // C1 — Sequence gap detection: if gap > 1, tokens were dropped.
                        // Likely cause: WS broadcast overflow (broadcast::channel(4096) is
                        // bounded) or a mid-turn reconnect where some tokens were not replayed.
                        let expected = last + 1;
                        if sequence_num > expected {
                            let gap = sequence_num - expected;
                            self.state.chat.gaps_detected += 1;
                            tracing::warn!(
                                %session_id,
                                sequence_num,
                                expected,
                                gap,
                                total_gaps = self.state.chat.gaps_detected,
                                "streaming gap detected — {} token(s) missing in sequence",
                                gap
                            );
                        }
                    }
                    self.state.chat.last_sequence_num = Some(sequence_num);
                    // Only accumulate visible output tokens — thinking tokens drive the
                    // ThinkingBubble display (bubble shows while streaming_token is empty).
                    if !is_thinking {
                        self.state.chat.streaming_token.push_str(&token);
                        self.state.chat.streaming_token_count += 1;
                    }
                    tokens_this_frame += 1;
                    if tokens_this_frame >= MAX_TOKENS_PER_FRAME {
                        // Leave remaining tokens for the next frame to avoid >16ms stalls.
                        ctx.request_repaint();
                        return;
                    }
                }
                BackendMessage::ChatTurnCompleted { session_id, assistant_text, total_duration_ms, .. } => {
                    // A3 — Route by session_id: ignore completions from inactive sessions.
                    // This prevents a slow background session from appending its final
                    // response to whatever session the user switched to in the meantime.
                    if self.state.chat.active_session != Some(session_id) {
                        tracing::debug!(%session_id, "ignoring stale ChatTurnCompleted for inactive session");
                        continue;
                    }
                    // Finalize the streamed response into a completed message.
                    let streamed = std::mem::take(&mut self.state.chat.streaming_token);
                    let final_text = if !streamed.is_empty() { streamed } else { assistant_text };
                    self.state.chat.messages.push_back(crate::state::ChatDisplayMessage {
                        id: uuid::Uuid::new_v4(),
                        role: crate::state::ChatDisplayRole::Assistant,
                        content: final_text,
                        timestamp: chrono::Utc::now(),
                    });
                    self.state.chat.is_streaming = false;
                    self.state.chat.streaming_token_count = 0;
                    self.state.chat.turn_started_at = None;
                    self.state.chat.error = None;
                    self.state.chat.last_sequence_num = None;
                    // C4: Store server-measured turn duration for display in the UI.
                    self.state.chat.last_turn_duration_ms = Some(total_duration_ms);
                    // E2: Clear media analysis progress — analysis is complete.
                    self.state.chat.media_analysis_progress = None;
                    // Successful turn resets the retry counter and gap counter.
                    self.state.chat.retry_count = 0;
                    self.state.chat.error_recoverable = false;
                    self.state.chat.gaps_detected = 0;
                    tracing::info!(
                        %session_id,
                        duration_ms = total_duration_ms,
                        tokens = self.state.chat.streaming_token_count,
                        "turn completed"
                    );
                }
                BackendMessage::ChatTurnFailed { session_id, error, recoverable } => {
                    // A4 — Route by session_id: ignore failures from inactive sessions.
                    if self.state.chat.active_session != Some(session_id) {
                        tracing::debug!(%session_id, "ignoring stale ChatTurnFailed for inactive session");
                        continue;
                    }
                    self.state.chat.is_streaming = false;
                    self.state.chat.error = Some(error);
                    self.state.chat.streaming_token.clear();
                    self.state.chat.streaming_token_count = 0;
                    self.state.chat.turn_started_at = None;
                    self.state.chat.last_sequence_num = None;
                    // E2: Clear media analysis progress on failure.
                    self.state.chat.media_analysis_progress = None;
                    // Show "Retry" only when the server flagged the error as recoverable
                    // AND the user hasn't already exhausted the consecutive retry budget.
                    const MAX_RETRIES: u32 = 3;
                    self.state.chat.error_recoverable =
                        recoverable && self.state.chat.retry_count < MAX_RETRIES;
                }
                BackendMessage::ChatPermissionRequired {
                    session_id,
                    request_id,
                    tool_name,
                    risk_level,
                    description,
                    deadline_secs,
                } => {
                    // A-bonus: only show the permission modal for the active session.
                    // Modals for background sessions would be orphaned (no way to route
                    // the approval back) and confusing to the user.
                    if self.state.chat.active_session == Some(session_id) {
                        self.state.chat.permission_modal =
                            Some(crate::state::ChatPermissionModal {
                                request_id,
                                tool_name,
                                risk_level,
                                description,
                                deadline_secs,
                                created_at: std::time::Instant::now(),
                            });
                    } else {
                        tracing::debug!(
                            %session_id, %request_id,
                            "dropping PermissionRequired for non-active session"
                        );
                    }
                }
                BackendMessage::SubAgentStarted { session_id, sub_agent_id, description, wave, allowed_tools } => {
                    if self.state.chat.active_session == Some(session_id) {
                        // Remove any stale entry with the same ID before inserting.
                        self.state.chat.sub_agents.retain(|a| a.sub_agent_id != sub_agent_id);
                        self.state.chat.sub_agents.push(crate::state::SubAgentEntry {
                            sub_agent_id,
                            description,
                            wave,
                            allowed_tools,
                            success: None,
                            summary: None,
                            duration_ms: None,
                            tools_used: Vec::new(),
                        });
                        // Cap at 20 most-recent sub-agent entries.
                        if self.state.chat.sub_agents.len() > 20 {
                            self.state.chat.sub_agents.remove(0);
                        }
                    }
                }
                BackendMessage::SubAgentCompleted { session_id, sub_agent_id, success, summary, duration_ms, tools_used } => {
                    if self.state.chat.active_session == Some(session_id) {
                        if let Some(entry) = self.state.chat.sub_agents.iter_mut().find(|a| a.sub_agent_id == sub_agent_id) {
                            entry.success = Some(success);
                            entry.summary = Some(summary);
                            entry.duration_ms = Some(duration_ms);
                            // C3: Record which tools were actually used by this sub-agent.
                            entry.tools_used = tools_used;
                        } else {
                            // A-bonus: log orphaned completions so backend ID mismatches
                            // are visible in diagnostics rather than silently discarded.
                            tracing::warn!(
                                %session_id, %sub_agent_id,
                                "SubAgentCompleted arrived with no matching SubAgentStarted entry — possible ID mismatch"
                            );
                        }
                    }
                }
                BackendMessage::ChatPermissionExpired { session_id, request_id } => {
                    // B1: Dismiss the permission modal if it matches the expired request.
                    // Without this, the modal would persist until the user manually
                    // dismisses it even though the tool has already been denied.
                    if self.state.chat.active_session == Some(session_id) {
                        if let Some(ref modal) = self.state.chat.permission_modal {
                            if modal.request_id == request_id {
                                self.state.chat.permission_modal = None;
                                tracing::info!(
                                    %session_id, %request_id,
                                    "permission modal dismissed — request expired"
                                );
                            }
                        }
                    }
                }
                BackendMessage::ChatSessionRenamed { session_id, title } => {
                    if let Some(session) = self.state.chat.sessions.iter_mut().find(|s| s.id == session_id) {
                        session.title = Some(title);
                    }
                }
                BackendMessage::Event(event) => {
                    // Also extract log entries from events.
                    if let halcon_api::types::ws::WsServerEvent::Log(ref entry) = event {
                        self.state.push_log(entry.clone());
                    }
                    self.state.push_event(event);
                }

                // ── File explorer ─────────────────────────────────────────────
                BackendMessage::DirectoryLoaded { path, entries } => {
                    self.state.files.dir_cache.insert(path, entries);
                    self.state.files.loading = false;
                    self.state.files.error = None;
                }
                BackendMessage::FileLoaded { content, .. } => {
                    self.state.files.content = Some(content);
                    self.state.files.loading = false;
                    self.state.files.error = None;
                }
                BackendMessage::FileError { error, path } => {
                    tracing::warn!(path = %path.display(), error = %error, "file IO error");
                    self.state.files.error = Some(error);
                    self.state.files.loading = false;
                }

                // ── Agent / task operations ───────────────────────────────────
                BackendMessage::OperationError(err) => {
                    self.state.ops.error = Some(err);
                }

                // ── Multimodal attachments ─────────────────────────────────────
                BackendMessage::AttachmentReady(att) => {
                    self.state.chat.pending_attachments.push(att);
                    self.state.chat.is_uploading_attachment = false;
                }
                BackendMessage::AttachmentError { path, error } => {
                    tracing::warn!(path = %path.display(), error = %error, "attachment failed");
                    self.state.chat.is_uploading_attachment = false;
                    self.state.chat.error = Some(format!("Cannot attach: {error}"));
                }
                BackendMessage::MediaAnalysisProgress { session_id, index, total, filename } => {
                    if self.state.chat.active_session == Some(session_id) {
                        tracing::debug!(index, total, filename = %filename, "media analysis in progress");
                        // E2: Track progress so the UI can show an inline indicator.
                        self.state.chat.media_analysis_progress = Some((index, total, filename));
                    }
                }
            } // end match msg
            } // end Ok(msg)
        } // end loop

        // Auto-reconnect: trigger UiCommand::Connect when the scheduled instant passes.
        if let Some(reconnect_at) = self.state.reconnect_after {
            if std::time::Instant::now() >= reconnect_at {
                self.state.reconnect_after = None;
                self.state.connection = ConnectionState::Connecting;
                let _ = self.cmd_tx.try_send(UiCommand::Connect {
                    url: self.config.server_url.clone(),
                    token: self.config.auth_token.clone(),
                });
            } else {
                // Keep repainting until the reconnect fires.
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
        }

        // Keep repainting at ~30fps during streaming or when permission countdown is active
        // so the UI updates even when no new messages arrive from the backend.
        if self.state.chat.is_streaming || self.state.chat.permission_modal.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }
    }

    fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Halcon")
                    .size(16.0)
                    .strong()
                    .color(HalconTheme::ACCENT),
            );
            ui.label(
                egui::RichText::new("Control Plane")
                    .size(11.0)
                    .color(HalconTheme::TEXT_MUTED),
            );
            ui.add_space(12.0);

            // Chat is the primary view — shown first. Ctrl+1.
            if ui
                .selectable_label(self.state.active_view == ActiveView::Chat, "Chat")
                .clicked()
            {
                self.state.active_view = ActiveView::Chat;
            }

            ui.separator();

            // Control-plane views. Ctrl+2..5.
            let mgmt_views = [
                (ActiveView::Dashboard, "Dashboard"),
                (ActiveView::Agents, "Agents"),
                (ActiveView::Tasks, "Tasks"),
                (ActiveView::Tools, "Tools"),
            ];
            for (view, label) in &mgmt_views {
                if ui
                    .selectable_label(self.state.active_view == *view, *label)
                    .clicked()
                {
                    self.state.active_view = *view;
                }
            }

            ui.separator();

            // Observability. Ctrl+6..8.
            let obs_views = [
                (ActiveView::Logs, "Logs"),
                (ActiveView::Metrics, "Metrics"),
                (ActiveView::Protocols, "Protocols"),
            ];
            for (view, label) in &obs_views {
                if ui
                    .selectable_label(self.state.active_view == *view, *label)
                    .clicked()
                {
                    self.state.active_view = *view;
                }
            }

            ui.separator();

            // Utilities. Ctrl+9..0.
            let util_views = [
                (ActiveView::Files, "Files"),
                (ActiveView::Settings, "Settings"),
            ];
            for (view, label) in &util_views {
                if ui
                    .selectable_label(self.state.active_view == *view, *label)
                    .clicked()
                {
                    self.state.active_view = *view;
                }
            }
        });
    }

    fn render_status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Connection indicator.
            let (color, label) = match &self.state.connection {
                ConnectionState::Connected => (HalconTheme::SUCCESS, "Connected"),
                ConnectionState::Connecting => (HalconTheme::WARNING, "Connecting..."),
                ConnectionState::Disconnected => (HalconTheme::TEXT_MUTED, "Disconnected"),
                ConnectionState::Error(_) => (HalconTheme::ERROR, "Error"),
            };
            ui.colored_label(color, format!("\u{25CF} {label}"));
            ui.separator();

            ui.label(format!("{} agents", self.state.agents.len()));
            ui.separator();
            ui.label(format!("{} tools", self.state.tools.len()));
            ui.separator();

            if let Some(ref status) = self.state.system_status {
                ui.label(format!("uptime {}s", status.uptime_seconds));
                ui.separator();
                ui.label(format!("mem {}B", status.platform.memory_usage_bytes));
            }
        });
    }

    fn render_connect_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("Connect to Runtime")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.label("Server URL:");
                    ui.text_edit_singleline(&mut self.config.server_url);
                    ui.add_space(4.0);
                    ui.label("Auth Token:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.config.auth_token).password(true),
                    );
                    ui.add_space(8.0);

                    if let ConnectionState::Error(ref err) = self.state.connection {
                        ui.colored_label(HalconTheme::ERROR, err);
                        ui.add_space(4.0);
                    }

                    ui.horizontal(|ui| {
                        if ui.button("Connect").clicked() {
                            self.state.connection = ConnectionState::Connecting;
                            let _ = self.cmd_tx.try_send(UiCommand::Connect {
                                url: self.config.server_url.clone(),
                                token: self.config.auth_token.clone(),
                            });
                        }
                        if ui.button("Skip").clicked() {
                            self.state.show_connect_dialog = false;
                        }
                    });
                });
            });
    }
}

impl eframe::App for HalconApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.theme_applied {
            HalconTheme::apply(ctx);
            self.theme_applied = true;
        }

        self.process_backend_messages(ctx);

        // Keyboard shortcuts — view switching (Ctrl+1..9) + Ctrl+N (new chat session).
        // Ctrl+1 = Chat (primary), Ctrl+2..9 = other views in sidebar order.
        let (ctrl_n_pressed, view_switch) = ctx.input(|i| {
            let ctrl = i.modifiers.ctrl || i.modifiers.mac_cmd;
            let n = ctrl && i.key_pressed(egui::Key::N);
            let switch = if ctrl {
                use egui::Key;
                let mappings = [
                    (Key::Num1, ActiveView::Chat),
                    (Key::Num2, ActiveView::Dashboard),
                    (Key::Num3, ActiveView::Agents),
                    (Key::Num4, ActiveView::Tasks),
                    (Key::Num5, ActiveView::Tools),
                    (Key::Num6, ActiveView::Logs),
                    (Key::Num7, ActiveView::Metrics),
                    (Key::Num8, ActiveView::Protocols),
                    (Key::Num9, ActiveView::Files),
                    (Key::Num0, ActiveView::Settings),
                ];
                mappings.iter().find_map(|(k, v)| {
                    if i.key_pressed(*k) { Some(*v) } else { None }
                })
            } else {
                None
            };
            (n, switch)
        });

        if let Some(view) = view_switch {
            self.state.active_view = view;
        }

        // Ctrl+N: jump to Chat and open the new-session dialog.
        if ctrl_n_pressed {
            self.state.active_view = ActiveView::Chat;
            self.state.chat.show_new_session_dialog = true;
        }

        // Connect dialog.
        if self.state.show_connect_dialog {
            self.render_connect_dialog(ctx);
        }

        // Top bar.
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Halcon Control Plane")
                        .strong()
                        .color(HalconTheme::ACCENT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (color, label) = match &self.state.connection {
                        ConnectionState::Connected => (HalconTheme::SUCCESS, "Connected"),
                        ConnectionState::Connecting => (HalconTheme::WARNING, "Connecting"),
                        ConnectionState::Disconnected => (HalconTheme::TEXT_MUTED, "Disconnected"),
                        ConnectionState::Error(_) => (HalconTheme::ERROR, "Error"),
                    };
                    ui.colored_label(color, format!("\u{25CF} {label}"));
                });
            });
        });

        // Status bar.
        egui::TopBottomPanel::bottom("status_bar")
            .max_height(24.0)
            .show(ctx, |ui| {
                self.render_status_bar(ui);
            });

        // Sidebar.
        egui::SidePanel::left("sidebar")
            .resizable(false)
            .default_width(130.0)
            .show(ctx, |ui| {
                self.render_sidebar(ui);
            });

        // Central panel — active view.
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.state.active_view {
                ActiveView::Dashboard => views::dashboard::render(ui, &self.state),
                ActiveView::Agents => {
                    views::agents::render(ui, &mut self.state, &self.cmd_tx);
                }
                ActiveView::Tasks => {
                    views::tasks::render(ui, &mut self.state, &self.cmd_tx);
                }
                ActiveView::Tools => {
                    views::tools::render(ui, &self.state, &self.cmd_tx);
                }
                ActiveView::Logs => views::logs::render(ui, &mut self.state),
                ActiveView::Metrics => views::metrics::render(ui, &self.state),
                ActiveView::Protocols => views::protocols::render(ui, &self.state),
                ActiveView::Files => {
                    views::files::render(ui, &mut self.state, &self.cmd_tx);
                }
                ActiveView::Chat => {
                    views::chat::render(ui, &mut self.state, &self.cmd_tx);
                }
                ActiveView::Settings => {
                    views::settings::render(
                        ui,
                        &mut self.state,
                        &mut self.config,
                        &self.cmd_tx,
                    );
                }
            }
        });
    }
}
