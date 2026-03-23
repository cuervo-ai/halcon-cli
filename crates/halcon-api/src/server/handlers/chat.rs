//! HTTP handlers for chat session management (ARQ-001 Iteration 3).
//!
//! Endpoints:
//!   POST   /api/v1/chat/sessions              — create session
//!   GET    /api/v1/chat/sessions              — list sessions
//!   GET    /api/v1/chat/sessions/:id          — get session
//!   DELETE /api/v1/chat/sessions/:id          — delete session
//!   POST   /api/v1/chat/sessions/:id/messages — submit message + launch execution
//!   DELETE /api/v1/chat/sessions/:id/active   — cancel active execution
//!   POST   /api/v1/chat/sessions/:id/permissions/:req_id — resolve permission

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use halcon_core::traits::{ChatExecutionEvent, ChatExecutionInput, ChatHistoryMessage};
use uuid::Uuid;

use crate::server::state::AppState;
use crate::types::chat::{
    ChatMessageEntry, ChatSession, ChatSessionStatus, CreateSessionRequest, CreateSessionResponse,
    ListMessagesResponse, ListSessionsResponse, PermissionDecisionStr, ResolvePermissionRequest,
    ResolvePermissionResponse, SubmitMessageRequest, SubmitMessageResponse,
    UpdateSessionTitleRequest, UpdateSessionTitleResponse,
};
use crate::types::ws::WsServerEvent;
use halcon_core::traits::chat_executor::MediaAttachmentInline as CoreAttachment;

/// POST /api/v1/chat/sessions — create a new chat session.
pub async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, StatusCode> {
    let now = Utc::now();
    let session = ChatSession {
        id: Uuid::new_v4(),
        title: req.title,
        model: req.model.clone(),
        provider: req.provider.clone(),
        status: ChatSessionStatus::Idle,
        message_count: 0,
        created_at: now,
        updated_at: now,
    };

    let (handle, _cancel_rx) = crate::types::chat::ChatSessionHandle::new(session.clone());
    state.active_chat_sessions.insert(session.id, handle);

    state.broadcast(WsServerEvent::ChatSessionCreated {
        session_id: session.id,
        model: req.model,
        provider: req.provider,
    });

    state.persist_sessions().await;

    Ok(Json(CreateSessionResponse { session }))
}

/// GET /api/v1/chat/sessions — list all active sessions.
pub async fn list_sessions(State(state): State<AppState>) -> Json<ListSessionsResponse> {
    let sessions: Vec<ChatSession> = state
        .active_chat_sessions
        .iter()
        .map(|entry| entry.value().session.clone())
        .collect();
    let total = sessions.len();
    Json(ListSessionsResponse { sessions, total })
}

/// GET /api/v1/chat/sessions/:id — get a specific session.
pub async fn get_session(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<ChatSession>, StatusCode> {
    state
        .active_chat_sessions
        .get(&id)
        .map(|entry| Json(entry.value().session.clone()))
        .ok_or(StatusCode::NOT_FOUND)
}

/// DELETE /api/v1/chat/sessions/:id — delete a session.
///
/// A6 — Cancels any running executor before removing the handle so that
/// orphaned sub-agents and background tasks are signalled to stop immediately.
/// Without this, a deleted session's executor would keep consuming API tokens
/// and CPU until it naturally completed or timed out.
pub async fn delete_session(Path(id): Path<Uuid>, State(state): State<AppState>) -> StatusCode {
    if let Some((_, handle)) = state.active_chat_sessions.remove(&id) {
        // Signal the running executor (and any sub-agents) to stop.
        handle.cancel();
        // Remove the permission sender — any pending permission decisions are now moot.
        state.perm_senders.remove(&id);
        state.broadcast(WsServerEvent::ChatSessionDeleted { session_id: id });
        state.persist_sessions().await;
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

/// GET /api/v1/chat/sessions/:id/messages — list conversation history for a session.
pub async fn list_messages(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<ListMessagesResponse>, StatusCode> {
    let history_arc = state
        .active_chat_sessions
        .get(&id)
        .map(|e| std::sync::Arc::clone(&e.history))
        .ok_or(StatusCode::NOT_FOUND)?;

    let messages: Vec<ChatMessageEntry> = {
        let h = history_arc.lock().await;
        h.iter()
            .map(|(role, content)| ChatMessageEntry {
                role: role.clone(),
                content: content.clone(),
            })
            .collect()
    };
    let total = messages.len();
    Ok(Json(ListMessagesResponse {
        session_id: id,
        messages,
        total,
    }))
}

/// POST /api/v1/chat/sessions/:id/messages — submit a user message and start execution.
///
/// If a ChatExecutor is registered in AppState, launches agent execution in a background
/// task. Events are translated to WsServerEvent and broadcast to all WebSocket clients.
/// If no executor is registered, returns 501 Not Implemented.
pub async fn submit_message(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    Json(req): Json<SubmitMessageRequest>,
) -> Result<Json<SubmitMessageResponse>, StatusCode> {
    // Verify session exists and extract model, provider, and shared history Arc.
    let (model, provider, history_arc) = state
        .active_chat_sessions
        .get(&id)
        .map(|e| {
            (
                e.session.model.clone(),
                e.session.provider.clone(),
                std::sync::Arc::clone(&e.history),
            )
        })
        .ok_or(StatusCode::NOT_FOUND)?;
    // DashMap ref dropped here — safe to .await below.

    // Require executor — return 501 if not registered.
    let executor = match state.chat_executor.clone() {
        Some(e) => e,
        None => {
            tracing::warn!(session_id = %id, "no ChatExecutor registered — use `halcon serve` to enable chat");
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
    };

    // F2: Guard against re-entrant execution — reject if the session is already Executing.
    // Without this guard, a second POST to /messages while an executor is running would
    // launch a second agent loop, causing interleaved events and double-billing.
    {
        let already_executing = state
            .active_chat_sessions
            .get(&id)
            .map(|e| e.session.status == ChatSessionStatus::Executing)
            .unwrap_or(false);
        if already_executing {
            tracing::warn!(session_id = %id, "rejecting duplicate submit — session already executing");
            return Err(StatusCode::CONFLICT);
        }
    }

    // Update session status to Executing.
    if let Some(mut entry) = state.active_chat_sessions.get_mut(&id) {
        entry.session.status = ChatSessionStatus::Executing;
        entry.session.updated_at = Utc::now();
    }

    let user_message_id = Uuid::new_v4();
    let content = req.content.clone();
    let orchestrate = req.orchestrate.unwrap_or(false);
    let expert = req.expert.unwrap_or(false);
    let attachments = req.attachments.clone();

    // Load conversation history for multi-turn context.
    let history: Vec<ChatHistoryMessage> = {
        let h = history_arc.lock().await;
        h.iter()
            .map(|(role, content)| ChatHistoryMessage {
                role: role.clone(),
                content: content.clone(),
            })
            .collect()
    };

    // Convert API attachments to core types and broadcast progress events.
    let media_attachments: Vec<CoreAttachment> = if attachments.is_empty() {
        Vec::new()
    } else {
        let file_count = attachments.len();
        state.broadcast(WsServerEvent::MediaAnalysisStarted {
            session_id: id,
            file_count,
        });

        let core_atts: Vec<CoreAttachment> = attachments
            .iter()
            .enumerate()
            .map(|(i, att)| {
                let modality = if att.content_type.starts_with("image/") {
                    "image"
                } else if att.content_type.starts_with("audio/") {
                    "audio"
                } else if att.content_type.starts_with("video/") {
                    "video"
                } else {
                    "text"
                };
                state.broadcast(WsServerEvent::MediaAnalysisProgress {
                    session_id: id,
                    index: i,
                    total: file_count,
                    filename: att.filename.clone(),
                    modality: modality.to_string(),
                });
                CoreAttachment {
                    filename: att.filename.clone(),
                    content_type: att.content_type.clone(),
                    data_base64: att.data_base64.clone(),
                }
            })
            .collect();

        state.broadcast(WsServerEvent::MediaAnalysisCompleted {
            session_id: id,
            processed: core_atts.len(),
            context_injected: true,
        });
        core_atts
    };

    // Channels: events from executor → broadcast; cancellation; permission decisions.
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<ChatExecutionEvent>();
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let (perm_tx, perm_rx) = tokio::sync::mpsc::unbounded_channel::<(Uuid, bool)>();

    // Store cancel_tx in session handle so cancel_active can signal it.
    if let Some(mut entry) = state.active_chat_sessions.get_mut(&id) {
        entry.cancellation_tx = cancel_tx;
    }

    // Store perm_tx so resolve_permission can send decisions.
    state.perm_senders.insert(id, perm_tx);

    // Build execution input.
    let user_message = content.clone();
    let input = ChatExecutionInput {
        session_id: id,
        user_message,
        model,
        provider,
        working_directory: std::env::current_dir()
            .unwrap_or_else(|_| "/tmp".into())
            .to_string_lossy()
            .to_string(),
        orchestrate,
        expert,
        system_prompt: None,
        history,
        media_attachments,
    };

    // Spawn event-translation task: ChatExecutionEvent → WsServerEvent broadcast.
    // Also accumulates assistant tokens so the completed turn can be appended to history.
    let state_clone = state.clone();
    let history_arc_task = std::sync::Arc::clone(&history_arc);
    let user_content_for_history = content.clone();
    tokio::spawn(async move {
        let mut accumulated_text = String::new();
        // B3: Track whether a terminal event (Completed/Failed) was received so we can
        // broadcast ExecutionFailed if the channel closes prematurely (executor panic/abort).
        let mut received_terminal_event = false;
        while let Some(event) = event_rx.recv().await {
            // Accumulate non-thinking output tokens for history.
            if let ChatExecutionEvent::Token {
                ref text,
                is_thinking,
                ..
            } = event
            {
                if !is_thinking {
                    accumulated_text.push_str(text);
                }
            }
            let turn_completed = matches!(&event, ChatExecutionEvent::Completed { .. });
            if turn_completed || matches!(&event, ChatExecutionEvent::Failed { .. }) {
                received_terminal_event = true;
            }
            let ws_event = translate_event(id, event);
            if let Some(ev) = ws_event {
                state_clone.broadcast(ev);
            }
            if turn_completed {
                // Persist this turn: user message + assistant response.
                let new_count = {
                    let mut h = history_arc_task.lock().await;
                    h.push(("user".to_string(), user_content_for_history.clone()));
                    h.push((
                        "assistant".to_string(),
                        std::mem::take(&mut accumulated_text),
                    ));
                    h.len()
                };
                if let Some(mut entry) = state_clone.active_chat_sessions.get_mut(&id) {
                    entry.session.message_count = new_count;
                }
                state_clone.persist_sessions().await;
            }
        }
        // B3: If the event channel closed without a Completed/Failed event the executor
        // panicked or was killed.  Broadcast a synthetic failure so the client's spinner
        // does not hang indefinitely.
        if !received_terminal_event {
            tracing::warn!(
                session_id = %id,
                "executor event channel closed without terminal event — executor may have panicked"
            );
            state_clone.broadcast(WsServerEvent::ExecutionFailed {
                session_id: id,
                error_code: "internal_error".to_string(),
                message: "Agent terminated unexpectedly. Please retry.".to_string(),
                recoverable: true,
            });
        }
        // Execution complete — update session status back to Idle.
        if let Some(mut entry) = state_clone.active_chat_sessions.get_mut(&id) {
            entry.session.status = ChatSessionStatus::Idle;
            entry.session.updated_at = Utc::now();
        }
        state_clone.perm_senders.remove(&id);
    });

    // Spawn executor task.
    tokio::spawn(async move {
        executor.execute(input, event_tx, cancel_rx, perm_rx).await;
    });

    tracing::info!(session_id = %id, "chat execution launched");

    Ok(Json(SubmitMessageResponse {
        session_id: id,
        user_message_id,
        status: ChatSessionStatus::Executing,
    }))
}

/// DELETE /api/v1/chat/sessions/:id/active — cancel the active execution.
pub async fn cancel_active(Path(id): Path<Uuid>, State(state): State<AppState>) -> StatusCode {
    if let Some(entry) = state.active_chat_sessions.get(&id) {
        entry.value().cancel();
        state.broadcast(WsServerEvent::ExecutionFailed {
            session_id: id,
            error_code: "cancelled".to_string(),
            message: "Execution cancelled by user".to_string(),
            recoverable: true,
        });
        StatusCode::ACCEPTED
    } else {
        StatusCode::NOT_FOUND
    }
}

/// POST /api/v1/chat/sessions/:id/permissions/:req_id — resolve a permission request.
pub async fn resolve_permission(
    Path((session_id, request_id)): Path<(Uuid, Uuid)>,
    State(state): State<AppState>,
    Json(req): Json<ResolvePermissionRequest>,
) -> Result<Json<ResolvePermissionResponse>, StatusCode> {
    if !state.active_chat_sessions.contains_key(&session_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    let approved = matches!(req.decision, PermissionDecisionStr::Approve);
    let decision_str = if approved { "approve" } else { "deny" }.to_string();

    // Forward decision to executor via perm_senders.
    if let Some(tx) = state.perm_senders.get(&session_id) {
        if let Err(e) = tx.send((request_id, approved)) {
            tracing::error!(session_id = %session_id, request_id = %request_id, "Permission decision channel closed: {e}");
        }
    }

    state.broadcast(WsServerEvent::PermissionResolved {
        request_id,
        session_id,
        decision: decision_str,
        tool_executed: approved,
    });

    Ok(Json(ResolvePermissionResponse {
        request_id,
        decision: req.decision,
        tool_executed: approved,
    }))
}

/// PATCH /api/v1/chat/sessions/:id — update session title.
pub async fn update_session(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    Json(req): Json<UpdateSessionTitleRequest>,
) -> Result<Json<UpdateSessionTitleResponse>, StatusCode> {
    if let Some(mut entry) = state.active_chat_sessions.get_mut(&id) {
        entry.session.title = Some(req.title.clone());
        entry.session.updated_at = Utc::now();
        drop(entry);
        state.persist_sessions().await;
        Ok(Json(UpdateSessionTitleResponse {
            session_id: id,
            title: req.title,
        }))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// Translate a ChatExecutionEvent to a WsServerEvent.
fn translate_event(session_id: Uuid, event: ChatExecutionEvent) -> Option<WsServerEvent> {
    match event {
        ChatExecutionEvent::Token {
            text,
            is_thinking,
            sequence_num,
        } => Some(WsServerEvent::ChatStreamToken {
            session_id,
            token: text,
            is_thinking,
            sequence_num,
        }),
        ChatExecutionEvent::ThinkingProgress {
            chars_so_far,
            elapsed_secs,
        } => Some(WsServerEvent::ThinkingProgress {
            session_id,
            chars_so_far,
            elapsed_secs,
        }),
        // F1: Emit ToolExecuted only on ToolCompleted (with real duration + outcome).
        // Previously, ToolStarted emitted a synthetic event with duration_ms=0 and
        // ToolCompleted was silently discarded, giving the UI no actual timing info.
        ChatExecutionEvent::ToolStarted { .. } => None,
        ChatExecutionEvent::ToolCompleted {
            name,
            duration_ms,
            success,
        } => Some(WsServerEvent::ToolExecuted {
            name,
            tool_use_id: Uuid::new_v4().to_string(),
            duration_ms,
            success,
        }),
        ChatExecutionEvent::PermissionRequired {
            request_id,
            tool_name,
            risk_level,
            description,
            deadline_secs,
            args_preview,
        } => Some(WsServerEvent::PermissionRequired {
            request_id,
            session_id,
            tool_name,
            risk_level,
            args_preview,
            description,
            deadline_secs,
        }),
        ChatExecutionEvent::SubAgentStarted {
            id,
            description,
            wave,
            allowed_tools,
        } => Some(WsServerEvent::SubAgentStarted {
            session_id,
            sub_agent_id: id,
            task_description: description,
            wave,
            allowed_tools,
        }),
        ChatExecutionEvent::SubAgentCompleted {
            id,
            success,
            summary,
            tools_used,
            duration_ms,
        } => Some(WsServerEvent::SubAgentCompleted {
            session_id,
            sub_agent_id: id,
            success,
            summary,
            tools_used,
            duration_ms,
        }),
        ChatExecutionEvent::Completed {
            assistant_message_id,
            stop_reason,
            input_tokens,
            output_tokens,
            total_duration_ms,
        } => Some(WsServerEvent::ConversationCompleted {
            session_id,
            assistant_message_id,
            stop_reason,
            usage: crate::types::chat::ChatTokenUsage {
                input: input_tokens,
                output: output_tokens,
                thinking: 0,
                total: input_tokens + output_tokens,
            },
            total_duration_ms,
        }),
        ChatExecutionEvent::Failed {
            error_code,
            message,
            recoverable,
        } => Some(WsServerEvent::ExecutionFailed {
            session_id,
            error_code,
            message,
            recoverable,
        }),
        // B1: Translate permission timeout to PermissionExpired so clients
        // can dismiss their pending modals deterministically.
        ChatExecutionEvent::PermissionExpired { request_id } => {
            Some(WsServerEvent::PermissionExpired {
                request_id,
                session_id,
                deadline_elapsed_ms: 0,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_token_event() {
        let event = ChatExecutionEvent::Token {
            text: "hello".to_string(),
            is_thinking: false,
            sequence_num: 0,
        };
        let sid = Uuid::new_v4();
        let ws = translate_event(sid, event).unwrap();
        assert!(matches!(ws, WsServerEvent::ChatStreamToken { .. }));
    }

    #[test]
    fn test_translate_completed_event() {
        let event = ChatExecutionEvent::Completed {
            assistant_message_id: Uuid::new_v4(),
            stop_reason: "end_turn".to_string(),
            input_tokens: 100,
            output_tokens: 200,
            total_duration_ms: 5000,
        };
        let sid = Uuid::new_v4();
        let ws = translate_event(sid, event).unwrap();
        assert!(matches!(ws, WsServerEvent::ConversationCompleted { .. }));
    }

    #[test]
    fn test_translate_failed_event() {
        let event = ChatExecutionEvent::Failed {
            error_code: "err".to_string(),
            message: "test".to_string(),
            recoverable: false,
        };
        let sid = Uuid::new_v4();
        let ws = translate_event(sid, event).unwrap();
        assert!(matches!(ws, WsServerEvent::ExecutionFailed { .. }));
    }

    #[test]
    fn test_translate_permission_required() {
        let event = ChatExecutionEvent::PermissionRequired {
            request_id: Uuid::new_v4(),
            tool_name: "bash".to_string(),
            risk_level: "Destructive".to_string(),
            description: "run bash".to_string(),
            deadline_secs: 60,
            args_preview: std::collections::HashMap::new(),
        };
        let sid = Uuid::new_v4();
        let ws = translate_event(sid, event).unwrap();
        assert!(matches!(ws, WsServerEvent::PermissionRequired { .. }));
    }

    /// F1: ToolStarted should produce no WS event (avoids premature 0-duration events).
    #[test]
    fn test_tool_started_yields_no_event() {
        let event = ChatExecutionEvent::ToolStarted {
            name: "file_read".to_string(),
            risk_level: "Safe".to_string(),
        };
        let ws = translate_event(Uuid::new_v4(), event);
        assert!(ws.is_none(), "ToolStarted must produce no WS event");
    }

    /// F1: ToolCompleted should produce a ToolExecuted WS event with real duration.
    #[test]
    fn test_tool_completed_yields_executed_event_with_duration() {
        let event = ChatExecutionEvent::ToolCompleted {
            name: "bash".to_string(),
            duration_ms: 1234,
            success: false,
        };
        let sid = Uuid::new_v4();
        let ws = translate_event(sid, event).unwrap();
        match ws {
            WsServerEvent::ToolExecuted {
                name,
                duration_ms,
                success,
                ..
            } => {
                assert_eq!(name, "bash");
                assert_eq!(duration_ms, 1234);
                assert!(!success);
            }
            other => panic!("expected ToolExecuted, got {:?}", other),
        }
    }

    /// F1: Thinking token (is_thinking=true) is forwarded so clients can drive the bubble.
    #[test]
    fn test_thinking_token_forwarded() {
        let event = ChatExecutionEvent::Token {
            text: "…reasoning…".to_string(),
            is_thinking: true,
            sequence_num: 7,
        };
        let sid = Uuid::new_v4();
        let ws = translate_event(sid, event).unwrap();
        match ws {
            WsServerEvent::ChatStreamToken {
                is_thinking,
                sequence_num,
                ..
            } => {
                assert!(is_thinking);
                assert_eq!(sequence_num, 7);
            }
            other => panic!("expected ChatStreamToken, got {:?}", other),
        }
    }

    /// B1: PermissionExpired should translate to a PermissionExpired WS event.
    #[test]
    fn test_permission_expired_translates() {
        let req_id = Uuid::new_v4();
        let event = ChatExecutionEvent::PermissionExpired { request_id: req_id };
        let sid = Uuid::new_v4();
        let ws = translate_event(sid, event).unwrap();
        match ws {
            WsServerEvent::PermissionExpired {
                request_id,
                session_id,
                ..
            } => {
                assert_eq!(request_id, req_id);
                assert_eq!(session_id, sid);
            }
            other => panic!("expected PermissionExpired, got {:?}", other),
        }
    }

    /// B1: SubAgentCompleted carries tools_used through the translation pipeline.
    #[test]
    fn test_sub_agent_completed_carries_tools_used() {
        let event = ChatExecutionEvent::SubAgentCompleted {
            id: "agent-abc".to_string(),
            success: true,
            summary: "wrote file".to_string(),
            tools_used: vec!["file_write".to_string(), "bash".to_string()],
            duration_ms: 42_000,
        };
        let sid = Uuid::new_v4();
        let ws = translate_event(sid, event).unwrap();
        match ws {
            WsServerEvent::SubAgentCompleted {
                tools_used,
                duration_ms,
                ..
            } => {
                assert_eq!(tools_used, vec!["file_write", "bash"]);
                assert_eq!(duration_ms, 42_000);
            }
            other => panic!("expected SubAgentCompleted, got {:?}", other),
        }
    }
}
