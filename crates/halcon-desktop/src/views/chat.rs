//! Chat view — interactive conversation interface.
//!
//! Layout (all panels fill the full available height):
//!   ┌────────────────┬──────────────────────────────────────────┐
//!   │  Session list  │  Chat messages (ScrollArea, stick bottom) │
//!   │  (SidePanel)   ├──────────────────────────────────────────┤
//!   │                │  Sub-agent panel (optional, collapsible)  │
//!   │                ├──────────────────────────────────────────┤
//!   │                │  Input row (pinned bottom)                │
//!   └────────────────┴──────────────────────────────────────────┘

use egui::ScrollArea;
use egui_commonmark::CommonMarkViewer;
use tokio::sync::mpsc;

use crate::state::{AppState, ChatDisplayRole};
use crate::theme::HalconTheme;
use crate::widgets::permission_modal::{self, PermissionOutcome};
use crate::workers::UiCommand;
use halcon_api::types::chat::MediaAttachmentInline;

/// Entry point: render the full chat view inside the CentralPanel.
pub fn render(ui: &mut egui::Ui, state: &mut AppState, cmd_tx: &mpsc::Sender<UiCommand>) {
    // Overlay: permission modal (renders on top of all panels).
    if let Some(ref modal) = state.chat.permission_modal.clone() {
        let session_id = state.chat.active_session.unwrap_or_default();
        let outcome = permission_modal::show(ui.ctx(), modal);
        match outcome {
            PermissionOutcome::Approved => {
                state.chat.permission_modal = None;
                if let Err(e) = cmd_tx.try_send(UiCommand::ResolvePermission {
                    session_id,
                    request_id: modal.request_id,
                    approve: true,
                }) {
                    tracing::error!("Failed to send permission approval: {e}");
                }
            }
            PermissionOutcome::Denied => {
                state.chat.permission_modal = None;
                if let Err(e) = cmd_tx.try_send(UiCommand::ResolvePermission {
                    session_id,
                    request_id: modal.request_id,
                    approve: false,
                }) {
                    tracing::error!("Failed to send permission denial: {e}");
                }
            }
            PermissionOutcome::Pending => {}
        }
    }

    // Session list — left panel, resizable by the user.
    egui::SidePanel::left("chat_sessions_panel")
        .default_width(200.0)
        .min_width(150.0)
        .max_width(300.0)
        .resizable(true)
        .show_inside(ui, |ui| {
            render_session_list(ui, state, cmd_tx);
        });

    // Main chat area — fills all remaining horizontal space.
    egui::CentralPanel::default().show_inside(ui, |ui| {
        if state.chat.active_session.is_some() {
            render_chat_area(ui, state, cmd_tx);
        } else {
            render_no_session(ui, state);
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Session list (left panel)
// ─────────────────────────────────────────────────────────────────────────────

fn render_session_list(ui: &mut egui::Ui, state: &mut AppState, cmd_tx: &mpsc::Sender<UiCommand>) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("Sessions")
            .size(13.0)
            .strong()
            .color(HalconTheme::ACCENT),
    );
    ui.add_space(4.0);

    if ui.button("+ New Session").clicked() {
        state.chat.show_new_session_dialog = !state.chat.show_new_session_dialog;
    }

    if state.chat.show_new_session_dialog {
        render_new_session_dialog(ui, state, cmd_tx);
    }

    ui.add_space(6.0);

    let mut session_to_delete: Option<uuid::Uuid> = None;
    let mut rename_submitted: Option<(uuid::Uuid, String)> = None;
    let mut rename_cancelled = false;

    ScrollArea::vertical()
        .id_salt("chat_session_list")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            let sessions = state.chat.sessions.clone();
            for session in &sessions {
                let is_active = state.chat.active_session == Some(session.id);

                // Inline rename mode.
                if state.chat.rename_session_id == Some(session.id) {
                    ui.horizontal(|ui| {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut state.chat.rename_buffer)
                                .desired_width(120.0)
                                .hint_text("Session title"),
                        );
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            rename_submitted =
                                Some((session.id, state.chat.rename_buffer.trim().to_string()));
                        }
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            rename_cancelled = true;
                        }
                        if ui.small_button("✓").clicked() {
                            rename_submitted =
                                Some((session.id, state.chat.rename_buffer.trim().to_string()));
                        }
                        if ui.small_button("✗").clicked() {
                            rename_cancelled = true;
                        }
                    });
                    continue;
                }

                let label = session.title.as_deref().unwrap_or("Untitled").to_string();
                let label_display = if label.len() > 14 {
                    format!("{}…", &label[..13])
                } else {
                    label.clone()
                };

                ui.horizontal(|ui| {
                    let response = ui.selectable_label(is_active, &label_display);
                    if response.clicked() && !is_active {
                        state.chat.active_session = Some(session.id);
                        state.chat.messages.clear();
                        state.chat.messages_loading = true;
                        state.chat.messages_visible_count = crate::state::CHAT_PAGE_SIZE;
                        state.chat.streaming_token.clear();
                        state.chat.streaming_token_count = 0;
                        state.chat.is_streaming = false;
                        state.chat.error = None;
                        state.chat.sub_agents.clear();
                        let _ = cmd_tx.try_send(UiCommand::LoadChatMessages {
                            session_id: session.id,
                        });
                    }
                    if response.double_clicked() {
                        state.chat.rename_session_id = Some(session.id);
                        state.chat.rename_buffer = label.clone();
                    }
                    response.on_hover_text(format!(
                        "Model: {}\nProvider: {}\nDouble-click to rename",
                        session.model, session.provider
                    ));

                    let rename_btn = ui.add(
                        egui::Button::new(
                            egui::RichText::new("✎")
                                .size(10.0)
                                .color(HalconTheme::TEXT_MUTED),
                        )
                        .small()
                        .frame(false),
                    );
                    if rename_btn.clicked() {
                        state.chat.rename_session_id = Some(session.id);
                        state.chat.rename_buffer = label.clone();
                    }
                    rename_btn.on_hover_text("Rename session");

                    let del = ui.add(
                        egui::Button::new(
                            egui::RichText::new("×")
                                .size(11.0)
                                .color(HalconTheme::TEXT_MUTED),
                        )
                        .small()
                        .frame(false),
                    );
                    if del.clicked() {
                        session_to_delete = Some(session.id);
                    }
                    del.on_hover_text("Delete session");
                });
            }
        });

    if let Some((id, title)) = rename_submitted {
        if !title.is_empty() {
            let _ = cmd_tx.try_send(UiCommand::RenameChatSession {
                session_id: id,
                title,
            });
        }
        state.chat.rename_session_id = None;
        state.chat.rename_buffer.clear();
    }
    if rename_cancelled {
        state.chat.rename_session_id = None;
        state.chat.rename_buffer.clear();
    }

    if let Some(id) = session_to_delete {
        let _ = cmd_tx.try_send(UiCommand::DeleteChatSession { session_id: id });
        state.chat.sessions.retain(|s| s.id != id);
        if state.chat.active_session == Some(id) {
            state.chat.active_session = None;
            state.chat.messages.clear();
            state.chat.streaming_token.clear();
            state.chat.is_streaming = false;
            state.chat.error = None;
            state.chat.sub_agents.clear();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Chat area (right side)
// ─────────────────────────────────────────────────────────────────────────────

fn render_chat_area(ui: &mut egui::Ui, state: &mut AppState, cmd_tx: &mpsc::Sender<UiCommand>) {
    let session_id = state.chat.active_session.unwrap();

    // Error banner (top, thin) — shows Retry when the server flagged the error
    // as recoverable and the user has not yet exhausted their 3-attempt budget.
    if let Some(ref err) = state.chat.error.clone() {
        egui::TopBottomPanel::top("chat_error_banner")
            .min_height(0.0)
            .show_inside(ui, |ui| {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.colored_label(HalconTheme::ERROR, format!("⚠  {}", err));
                    if state.chat.error_recoverable && !state.chat.is_streaming {
                        ui.add_space(8.0);
                        if ui
                            .small_button(egui::RichText::new("Retry").color(HalconTheme::WARNING))
                            .clicked()
                        {
                            if let Some((content, orchestrate)) = state.chat.last_message.clone() {
                                state.chat.error = None;
                                state.chat.retry_count += 1;
                                state.chat.error_recoverable = false;
                                state.chat.is_streaming = true;
                                state.chat.streaming_token.clear();
                                state.chat.streaming_token_count = 0;
                                state.chat.turn_started_at = Some(std::time::Instant::now());
                                let _ = cmd_tx.try_send(UiCommand::SendChatMessage {
                                    session_id,
                                    content,
                                    orchestrate,
                                    attachments: vec![],
                                });
                            }
                        }
                        if ui
                            .small_button(
                                egui::RichText::new("Dismiss").color(HalconTheme::TEXT_MUTED),
                            )
                            .clicked()
                        {
                            state.chat.error = None;
                            state.chat.error_recoverable = false;
                        }
                    }
                });
                ui.add_space(2.0);
            });
    }

    // Input row — pinned at the very bottom.
    egui::TopBottomPanel::bottom("chat_input_panel")
        .min_height(48.0)
        .show_inside(ui, |ui| {
            render_input_area(ui, state, cmd_tx, session_id);
        });

    // Sub-agent panel — above the input when active.
    if !state.chat.sub_agents.is_empty() {
        egui::TopBottomPanel::bottom("chat_sub_agents_panel")
            .max_height(120.0)
            .show_inside(ui, |ui| {
                render_sub_agent_panel(ui, state);
            });
    }

    // Activity panel — collapsible WS event feed.
    egui::TopBottomPanel::bottom("chat_activity_panel")
        .max_height(140.0)
        .min_height(0.0)
        .show_inside(ui, |ui| {
            render_activity_panel(ui, state);
        });

    // Messages — fills all remaining space.
    egui::CentralPanel::default().show_inside(ui, |ui| {
        // Loading placeholder while a session switch is in flight.
        if state.chat.messages_loading {
            ui.centered_and_justified(|ui| {
                ui.colored_label(HalconTheme::TEXT_MUTED, "Loading messages…");
            });
            return;
        }

        ScrollArea::vertical()
            .id_salt("chat_messages_scroll")
            .stick_to_bottom(true)
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // Client-side pagination: show the last `messages_visible_count` messages.
                let total = state.chat.messages.len();
                let start = total.saturating_sub(state.chat.messages_visible_count);
                if start > 0 {
                    if ui
                        .button(
                            egui::RichText::new(format!("⟨ Load earlier ({start} more)"))
                                .size(11.0)
                                .color(HalconTheme::TEXT_SECONDARY),
                        )
                        .clicked()
                    {
                        state.chat.messages_visible_count += crate::state::CHAT_PAGE_SIZE;
                    }
                    ui.separator();
                }

                // Split the borrow: collect the visible window first, then borrow cache.
                let messages: Vec<_> = state.chat.messages.iter().skip(start).cloned().collect();
                for msg in &messages {
                    render_message(ui, msg, &mut state.chat.md_cache);
                }

                // E2/E4: Media analysis progress indicator — shown while the server
                // is processing inline attachments before the turn starts.
                if let Some((index, total, ref filename)) = state.chat.media_analysis_progress {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.colored_label(HalconTheme::ACCENT, egui::RichText::new("◈").size(11.0));
                        let fname = filename.chars().take(30).collect::<String>();
                        ui.colored_label(
                            HalconTheme::TEXT_MUTED,
                            egui::RichText::new(format!(
                                "Analyzing {}/{}: {fname}…",
                                index + 1,
                                total
                            ))
                            .size(11.0),
                        );
                    });
                }

                // Live streaming response.
                if state.chat.is_streaming {
                    render_streaming(ui, state);
                } else if let Some(duration_ms) = state.chat.last_turn_duration_ms {
                    // D4: Show server-measured turn duration after the last completed turn.
                    // Gives users visibility into model latency without opening dev tools.
                    let dur_label = if duration_ms >= 60_000 {
                        format!("{:.1}m", duration_ms as f64 / 60_000.0)
                    } else if duration_ms >= 1_000 {
                        format!("{:.1}s", duration_ms as f64 / 1_000.0)
                    } else {
                        format!("{}ms", duration_ms)
                    };
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            HalconTheme::TEXT_MUTED,
                            egui::RichText::new(format!("⏱ {dur_label}")).size(10.0),
                        );
                        if state.chat.gaps_detected > 0 {
                            ui.colored_label(
                                HalconTheme::WARNING,
                                egui::RichText::new(format!(
                                    "⚠ {} gap(s)",
                                    state.chat.gaps_detected
                                ))
                                .size(10.0),
                            );
                        }
                    });
                }
            });
    });
}

/// Render the live streaming block (thinking bubble → tokens with cursor).
fn render_streaming(ui: &mut egui::Ui, state: &AppState) {
    let elapsed_secs = state
        .chat
        .turn_started_at
        .map(|t| t.elapsed().as_secs_f32())
        .unwrap_or(0.0);

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.colored_label(
            HalconTheme::ACCENT,
            egui::RichText::new("AI").strong().size(11.0),
        );
        ui.separator();
        ui.colored_label(HalconTheme::TEXT_MUTED, egui::RichText::new("…").size(10.0));
    });

    if !state.chat.streaming_token.is_empty() {
        ui.label(&state.chat.streaming_token);
        // Blinking block cursor.
        ui.label(
            egui::RichText::new("\u{2587}")
                .color(HalconTheme::ACCENT)
                .size(14.0),
        );
    } else {
        crate::widgets::thinking_bubble::show(ui, elapsed_secs, state.chat.streaming_token_count);
    }
    ui.add_space(4.0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Input area
// ─────────────────────────────────────────────────────────────────────────────

fn render_input_area(
    ui: &mut egui::Ui,
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<UiCommand>,
    session_id: uuid::Uuid,
) {
    let is_busy = state.chat.is_streaming;
    ui.add_space(6.0);

    // Option row: orchestrate toggle + activity panel toggle + attach button.
    ui.horizontal(|ui| {
        ui.checkbox(&mut state.chat.orchestrate, "")
            .on_hover_text("Orchestrate: enable sub-agents for complex tasks");
        ui.colored_label(
            if state.chat.orchestrate {
                HalconTheme::ACCENT
            } else {
                HalconTheme::TEXT_MUTED
            },
            egui::RichText::new("Orchestrate").size(10.0),
        );
        ui.add_space(8.0);
        ui.checkbox(&mut state.chat.show_activity_panel, "")
            .on_hover_text("Show live activity feed");
        ui.colored_label(
            HalconTheme::TEXT_MUTED,
            egui::RichText::new("Activity").size(10.0),
        );

        ui.add_space(8.0);
        // Attach button — opens a native file dialog.
        let attach_btn = ui.add_enabled(
            !is_busy && !state.chat.is_uploading_attachment,
            egui::Button::new(
                egui::RichText::new("+ Attach")
                    .size(10.0)
                    .color(HalconTheme::TEXT_SECONDARY),
            )
            .small()
            .frame(true),
        );
        if attach_btn.clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["jpg", "jpeg", "png", "gif", "webp"])
                .add_filter("Audio", &["mp3", "wav", "ogg", "m4a", "flac"])
                .add_filter("Video", &["mp4", "webm", "mov"])
                .add_filter(
                    "Text / Code",
                    &[
                        "txt", "md", "rs", "py", "js", "ts", "go", "java", "cpp", "c", "rb", "sh",
                        "json", "yaml", "toml", "csv",
                    ],
                )
                .add_filter("All files", &["*"])
                .pick_file()
            {
                state.chat.is_uploading_attachment = true;
                let _ = cmd_tx.try_send(UiCommand::AttachFile { path });
            }
        }
        attach_btn.on_hover_text("Attach a file (image, audio, video, or text)");

        if state.chat.is_uploading_attachment {
            ui.add_space(4.0);
            ui.colored_label(
                HalconTheme::TEXT_MUTED,
                egui::RichText::new("Reading…").size(10.0),
            );
        }
    });

    // Attachment chips row — shown when pending attachments exist.
    let mut remove_idx: Option<usize> = None;
    if !state.chat.pending_attachments.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for (i, att) in state.chat.pending_attachments.iter().enumerate() {
                let label = format!("{} {} ({})", att.icon(), att.name, att.size_label());
                ui.group(|ui| {
                    ui.colored_label(
                        HalconTheme::TEXT_SECONDARY,
                        egui::RichText::new(&label).size(10.0),
                    );
                    let rm = ui.add(
                        egui::Button::new(
                            egui::RichText::new("×")
                                .size(10.0)
                                .color(HalconTheme::TEXT_MUTED),
                        )
                        .small()
                        .frame(false),
                    );
                    if rm.clicked() {
                        remove_idx = Some(i);
                    }
                    rm.on_hover_text("Remove attachment");
                });
            }
        });
    }
    if let Some(idx) = remove_idx {
        state.chat.pending_attachments.remove(idx);
    }

    // Handle drag-and-drop onto this panel.
    let drop_target = ui.interact(
        ui.available_rect_before_wrap(),
        egui::Id::new("chat_drop_target"),
        egui::Sense::hover(),
    );
    if drop_target.hovered() {
        ui.ctx().input(|i| {
            for file in &i.raw.dropped_files {
                if let Some(ref path) = file.path {
                    state.chat.is_uploading_attachment = true;
                    let _ = cmd_tx.try_send(UiCommand::AttachFile { path: path.clone() });
                }
            }
        });
    }

    ui.horizontal(|ui| {
        let text_edit = egui::TextEdit::singleline(&mut state.chat.input)
            .hint_text(if is_busy {
                "Agent is running…"
            } else {
                "Type a message…"
            })
            .interactive(!is_busy)
            .desired_width(ui.available_width() - 120.0);

        let response = ui.add(text_edit);

        let send_triggered =
            response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !is_busy;
        let send_clicked = ui
            .add_enabled(!is_busy, egui::Button::new("Send"))
            .clicked();

        if (send_triggered || send_clicked) && !state.chat.input.trim().is_empty() {
            let content = state.chat.input.trim().to_string();
            let orchestrate = state.chat.orchestrate;

            // Collect pending attachments and clear them.
            let attachments: Vec<MediaAttachmentInline> = state
                .chat
                .pending_attachments
                .drain(..)
                .map(|a| MediaAttachmentInline {
                    filename: a.name,
                    content_type: a.content_type,
                    data_base64: a.data_base64,
                })
                .collect();

            state.chat.input.clear();
            state
                .chat
                .messages
                .push_back(crate::state::ChatDisplayMessage {
                    id: uuid::Uuid::new_v4(),
                    role: ChatDisplayRole::User,
                    content: content.clone(),
                    timestamp: chrono::Utc::now(),
                });
            state.chat.is_streaming = true;
            state.chat.streaming_token.clear();
            state.chat.streaming_token_count = 0;
            state.chat.turn_started_at = Some(std::time::Instant::now());
            state.chat.sub_agents.clear();
            // Cache for potential retry if the turn fails with recoverable=true.
            state.chat.last_message = Some((content.clone(), orchestrate));
            state.chat.retry_count = 0;
            if let Err(e) = cmd_tx.try_send(UiCommand::SendChatMessage {
                session_id,
                content,
                orchestrate,
                attachments,
            }) {
                tracing::error!("Failed to send chat message: {e}");
                state.chat.is_streaming = false;
            }
        }

        if is_busy && ui.button("Cancel").clicked() {
            if let Err(e) = cmd_tx.try_send(UiCommand::CancelChatExecution { session_id }) {
                tracing::warn!("Failed to send cancellation: {e}");
            }
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Live activity panel (collapsible WS event feed)
// ─────────────────────────────────────────────────────────────────────────────

fn render_activity_panel(ui: &mut egui::Ui, state: &AppState) {
    if !state.chat.show_activity_panel {
        return;
    }
    ui.separator();
    crate::widgets::activity_panel::show(ui, &state.events, Some("Activity"));
}

// ─────────────────────────────────────────────────────────────────────────────
// Sub-agent activity panel
// ─────────────────────────────────────────────────────────────────────────────

fn render_sub_agent_panel(ui: &mut egui::Ui, state: &AppState) {
    let running = state
        .chat
        .sub_agents
        .iter()
        .filter(|a| a.success.is_none())
        .count();
    let header = if running > 0 {
        format!("Sub-agents — {} running", running)
    } else {
        format!("Sub-agents ({})", state.chat.sub_agents.len())
    };

    egui::CollapsingHeader::new(
        egui::RichText::new(&header)
            .size(11.0)
            .color(HalconTheme::TEXT_MUTED),
    )
    .default_open(true)
    .id_salt("chat_sub_agents_header")
    .show(ui, |ui| {
        ScrollArea::vertical()
            .id_salt("sub_agents_scroll")
            .max_height(80.0)
            .show(ui, |ui| {
                for agent in &state.chat.sub_agents {
                    let (icon, color) = match agent.success {
                        None => ("⟳", HalconTheme::WARNING),
                        Some(true) => ("✓", HalconTheme::SUCCESS),
                        Some(false) => ("✗", HalconTheme::ERROR),
                    };
                    let id_short = if agent.sub_agent_id.len() >= 8 {
                        &agent.sub_agent_id[..8]
                    } else {
                        &agent.sub_agent_id
                    };
                    let desc = agent.description.chars().take(55).collect::<String>();
                    let ms = agent
                        .duration_ms
                        .map(|d| format!(" ({d}ms)"))
                        .unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.colored_label(color, icon);
                        ui.label(
                            egui::RichText::new(format!("W{} [{id_short}] {desc}{ms}", agent.wave))
                                .size(10.0)
                                .color(HalconTheme::TEXT_MUTED),
                        );
                    });
                    if let Some(ref summary) = agent.summary {
                        let s = summary.chars().take(90).collect::<String>();
                        ui.label(
                            egui::RichText::new(format!("  → {s}"))
                                .size(10.0)
                                .color(HalconTheme::TEXT_MUTED),
                        );
                    }
                    // C3/D: Show tools actually used by this sub-agent.
                    if !agent.tools_used.is_empty() {
                        let tools = agent.tools_used.join(", ");
                        let t = tools.chars().take(80).collect::<String>();
                        ui.label(
                            egui::RichText::new(format!("  ⚙ {t}"))
                                .size(10.0)
                                .color(HalconTheme::TEXT_MUTED),
                        );
                    }
                }
            });
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Single message with markdown rendering
// ─────────────────────────────────────────────────────────────────────────────

fn render_message(
    ui: &mut egui::Ui,
    msg: &crate::state::ChatDisplayMessage,
    cache: &mut egui_commonmark::CommonMarkCache,
) {
    let (role_label, color) = match msg.role {
        ChatDisplayRole::User => ("You", HalconTheme::TEXT_PRIMARY),
        ChatDisplayRole::Assistant => ("AI", HalconTheme::ACCENT),
        ChatDisplayRole::System => ("System", HalconTheme::WARNING),
    };

    ui.push_id(msg.id, |ui| {
        // Header row: role + timestamp + copy button.
        ui.horizontal(|ui| {
            ui.colored_label(color, egui::RichText::new(role_label).strong().size(11.0));
            ui.separator();
            ui.colored_label(
                HalconTheme::TEXT_MUTED,
                egui::RichText::new(msg.timestamp.format("%H:%M:%S").to_string()).size(10.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let copy_btn = ui.add(
                    egui::Button::new(
                        egui::RichText::new("⎘")
                            .size(10.0)
                            .color(HalconTheme::TEXT_MUTED),
                    )
                    .small()
                    .frame(false),
                );
                if copy_btn.clicked() {
                    ui.output_mut(|o| o.copied_text = msg.content.clone());
                }
                copy_btn.on_hover_text("Copy to clipboard");
            });
        });

        // Content: render with CommonMark for assistant/system; plain text for user.
        match msg.role {
            ChatDisplayRole::User => {
                // User input is plain text — no markdown processing.
                ui.label(&msg.content);
            }
            ChatDisplayRole::Assistant | ChatDisplayRole::System => {
                // AI/system responses rendered as Markdown.
                CommonMarkViewer::new().show(ui, cache, &msg.content);
            }
        }

        ui.add_space(4.0);

        // Thin separator between messages.
        ui.separator();
        ui.add_space(2.0);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// No session selected placeholder
// ─────────────────────────────────────────────────────────────────────────────

fn render_no_session(ui: &mut egui::Ui, state: &mut AppState) {
    ui.vertical_centered(|ui| {
        ui.add_space(80.0);
        ui.label(
            egui::RichText::new("No session selected")
                .size(18.0)
                .color(HalconTheme::TEXT_MUTED),
        );
        ui.add_space(8.0);
        ui.colored_label(
            HalconTheme::TEXT_MUTED,
            "Select a session from the left panel\nor create a new one.",
        );
        ui.add_space(16.0);
        if ui.button("Create New Session").clicked() {
            state.chat.show_new_session_dialog = true;
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// New session dialog
// ─────────────────────────────────────────────────────────────────────────────

fn render_new_session_dialog(
    ui: &mut egui::Ui,
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<UiCommand>,
) {
    // D1: Static fallback provider list — includes claude_code for local execution.
    // Used only when no server runtime config is available.
    let static_providers: &[(&str, &str)] = &[
        ("deepseek", "deepseek-chat"),
        ("anthropic", "claude-sonnet-4-6"),
        ("claude_code", "claude-sonnet-4-6"),
        ("openai", "gpt-4o"),
        ("ollama", "llama3.2"),
        ("gemini", "gemini-2.0-flash"),
    ];

    // D3: Known model suggestions per provider — shown as a ComboBox so users
    // don't have to type model IDs from memory.  Free-text input is still shown
    // below as an override for models not in this list.
    let known_models: std::collections::HashMap<&str, &[&str]> = [
        (
            "anthropic",
            &[
                "claude-sonnet-4-6",
                "claude-opus-4-6",
                "claude-haiku-4-5-20251001",
            ] as &[&str],
        ),
        (
            "claude_code",
            &[
                "claude-sonnet-4-6",
                "claude-opus-4-6",
                "claude-haiku-4-5-20251001",
            ] as &[&str],
        ),
        ("openai", &["gpt-4o", "gpt-4o-mini", "o3-mini"] as &[&str]),
        (
            "deepseek",
            &["deepseek-chat", "deepseek-coder-v2", "deepseek-reasoner"] as &[&str],
        ),
        (
            "ollama",
            &["llama3.2", "qwen2.5-coder", "mistral"] as &[&str],
        ),
        ("gemini", &["gemini-2.0-flash", "gemini-2.5-pro"] as &[&str]),
    ]
    .iter()
    .cloned()
    .collect();

    // Build the provider list: prefer server-reported enabled providers over static list.
    let dynamic_providers: Vec<(String, String)> = state
        .runtime_config
        .as_ref()
        .map(|cfg| {
            let mut sorted: Vec<_> = cfg
                .providers
                .iter()
                .filter(|(_, p)| p.enabled)
                .map(|(name, p)| {
                    let model = p
                        .default_model
                        .clone()
                        .unwrap_or_else(|| "default".to_string());
                    (name.clone(), model)
                })
                .collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            sorted
        })
        .unwrap_or_default();

    // D2: If dialog just opened and provider/model are still at defaults, sync to
    // the server's reported default provider/model so the right selection is
    // pre-filled without the user having to pick it manually.
    if state.chat.new_session_provider == "deepseek" {
        if let Some(ref cfg) = state.runtime_config {
            if !cfg.general.default_provider.is_empty() {
                state.chat.new_session_provider = cfg.general.default_provider.clone();
                state.chat.new_session_model = cfg.general.default_model.clone();
            }
        }
    }

    let cur_provider = state.chat.new_session_provider.clone();
    let suggested_models: &[&str] = known_models
        .get(cur_provider.as_str())
        .copied()
        .unwrap_or(&[]);

    ui.group(|ui| {
        ui.set_min_width(200.0);
        ui.label(egui::RichText::new("New Session").strong().size(12.0));
        ui.add_space(4.0);

        ui.label(egui::RichText::new("Provider").size(11.0));
        egui::ComboBox::from_id_salt("chat_provider_select")
            .selected_text(&state.chat.new_session_provider)
            .width(190.0)
            .show_ui(ui, |ui| {
                if dynamic_providers.is_empty() {
                    for (provider, default_model) in static_providers {
                        let selected = state.chat.new_session_provider == *provider;
                        if ui.selectable_label(selected, *provider).clicked() {
                            state.chat.new_session_provider = provider.to_string();
                            state.chat.new_session_model = default_model.to_string();
                        }
                    }
                } else {
                    for (provider, default_model) in &dynamic_providers {
                        let selected = &state.chat.new_session_provider == provider;
                        if ui.selectable_label(selected, provider.as_str()).clicked() {
                            state.chat.new_session_provider = provider.clone();
                            state.chat.new_session_model = default_model.clone();
                        }
                    }
                }
            });

        ui.add_space(4.0);
        ui.label(egui::RichText::new("Model").size(11.0));

        // D3: Show a ComboBox with known models when available, plus free-text fallback.
        if !suggested_models.is_empty() {
            egui::ComboBox::from_id_salt("chat_model_select")
                .selected_text(&state.chat.new_session_model)
                .width(190.0)
                .show_ui(ui, |ui| {
                    for &model in suggested_models {
                        let selected = state.chat.new_session_model == model;
                        if ui.selectable_label(selected, model).clicked() {
                            state.chat.new_session_model = model.to_string();
                        }
                    }
                });
            // Allow overriding the selected model with free text.
            ui.add(
                egui::TextEdit::singleline(&mut state.chat.new_session_model)
                    .desired_width(190.0)
                    .hint_text("or type a custom model ID"),
            );
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut state.chat.new_session_model)
                    .desired_width(190.0)
                    .hint_text("e.g. deepseek-chat"),
            );
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let can_create = !state.chat.new_session_model.trim().is_empty()
                && !state.chat.new_session_provider.trim().is_empty();
            if ui
                .add_enabled(can_create, egui::Button::new("Create"))
                .clicked()
            {
                let _ = cmd_tx.try_send(UiCommand::CreateChatSession {
                    model: state.chat.new_session_model.trim().to_string(),
                    provider: state.chat.new_session_provider.trim().to_string(),
                    title: None,
                });
                state.chat.show_new_session_dialog = false;
            }
            if ui.button("Cancel").clicked() {
                state.chat.show_new_session_dialog = false;
            }
        });
    });
    ui.add_space(4.0);
}
