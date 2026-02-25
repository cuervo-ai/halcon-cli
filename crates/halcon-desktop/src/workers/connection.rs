//! Main connection worker: manages the `HalconClient` lifecycle, dispatches
//! `UiCommand` variants to focused handler modules, and spawns the WebSocket
//! event loop task on successful connect.

use halcon_client::{ClientConfig, HalconClient};
use halcon_api::types::agent::InvokeAgentRequest;
use halcon_api::types::task::{AgentSelectorSpec, SubmitTaskRequest, TaskNodeSpec};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{BackendMessage, UiCommand};
use super::{chat_handlers, file_handlers, media_handlers};
use super::ws_loop::run_ws_event_loop;

/// Background worker that manages the client connection and processes commands.
///
/// When a successful connection is established a second task (`run_ws_event_loop`)
/// is spawned to receive WebSocket events and forward them as `BackendMessage`.
/// The WS task is aborted on `UiCommand::Disconnect` or when a new `Connect` is issued.
pub async fn run_connection_worker(
    mut cmd_rx: mpsc::Receiver<UiCommand>,
    msg_tx: mpsc::Sender<BackendMessage>,
    repaint: Arc<dyn Fn() + Send + Sync>,
) {
    let mut client: Option<HalconClient> = None;
    // Hold the WS task handle so we can abort it on disconnect/reconnect.
    let mut ws_handle: Option<tokio::task::JoinHandle<()>> = None;

    tracing::info!("connection worker started");

    while let Some(cmd) = cmd_rx.recv().await {
        // Detect WS task completion between commands (server restart / graceful close).
        // Clear the stale client so refresh commands fail gracefully instead of spamming errors.
        if let Some(ref handle) = ws_handle {
            if handle.is_finished() {
                ws_handle = None;
                client = None;
                tracing::debug!("WS task finished — cleared stale client");
            }
        }

        match cmd {
            // ── Connection lifecycle ──────────────────────────────────────────
            UiCommand::Connect { url, token } => {
                tracing::info!(url = %url, "connecting");

                // Abort any existing WS listener before reconnecting.
                if let Some(handle) = ws_handle.take() {
                    handle.abort();
                }

                let config = ClientConfig::new(&url, &token);
                match HalconClient::new(config) {
                    Ok(c) => match c.health_check().await {
                        Ok(true) => {
                            tracing::info!("connected");
                            let _ = msg_tx.try_send(BackendMessage::Connected);

                            // Spawn the WebSocket event listener.
                            match c.event_stream().await {
                                Ok(stream) => {
                                    let tx2 = msg_tx.clone();
                                    let rp2 = repaint.clone();
                                    ws_handle = Some(tokio::spawn(run_ws_event_loop(stream, tx2, rp2)));
                                    tracing::info!("WebSocket event stream started");
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "WebSocket event stream failed to start");
                                }
                            }

                            client = Some(c);
                            (repaint)();
                        }
                        Ok(false) => {
                            let _ = msg_tx.try_send(BackendMessage::ConnectionError(
                                "health check failed".into(),
                            ));
                            (repaint)();
                        }
                        Err(e) => {
                            let _ = msg_tx.try_send(BackendMessage::ConnectionError(format!(
                                "connection failed: {e}"
                            )));
                            (repaint)();
                        }
                    },
                    Err(e) => {
                        let _ = msg_tx.try_send(BackendMessage::ConnectionError(e.to_string()));
                        (repaint)();
                    }
                }
            }

            UiCommand::Disconnect => {
                if let Some(handle) = ws_handle.take() {
                    handle.abort();
                }
                client = None;
                let _ = msg_tx.try_send(BackendMessage::Disconnected("user disconnected".into()));
                (repaint)();
            }

            // ── Polling / refresh ─────────────────────────────────────────────
            UiCommand::RefreshAgents => {
                if let Some(ref c) = client {
                    match c.list_agents().await {
                        Ok(agents) => { let _ = msg_tx.try_send(BackendMessage::AgentsUpdated(agents)); }
                        Err(e) => { tracing::warn!(error = %e, "failed to refresh agents"); }
                    }
                    (repaint)();
                }
            }
            UiCommand::RefreshTasks => {
                if let Some(ref c) = client {
                    match c.list_tasks().await {
                        Ok(tasks) => { let _ = msg_tx.try_send(BackendMessage::TasksUpdated(tasks)); }
                        Err(e) => { tracing::warn!(error = %e, "failed to refresh tasks"); }
                    }
                    (repaint)();
                }
            }
            UiCommand::RefreshTools => {
                if let Some(ref c) = client {
                    match c.list_tools().await {
                        Ok(tools) => { let _ = msg_tx.try_send(BackendMessage::ToolsUpdated(tools)); }
                        Err(e) => { tracing::warn!(error = %e, "failed to refresh tools"); }
                    }
                    (repaint)();
                }
            }
            UiCommand::RefreshMetrics => {
                if let Some(ref c) = client {
                    match c.metrics().await {
                        Ok(m) => { let _ = msg_tx.try_send(BackendMessage::MetricsUpdated(m)); }
                        Err(e) => { tracing::warn!(error = %e, "failed to refresh metrics"); }
                    }
                    (repaint)();
                }
            }
            UiCommand::RefreshStatus => {
                if let Some(ref c) = client {
                    match c.system_status().await {
                        Ok(s) => { let _ = msg_tx.try_send(BackendMessage::SystemStatusUpdated(s)); }
                        Err(e) => { tracing::warn!(error = %e, "failed to refresh status"); }
                    }
                    (repaint)();
                }
            }
            UiCommand::RefreshConfig => {
                if let Some(ref c) = client {
                    match c.get_config().await {
                        Ok(cfg) => { let _ = msg_tx.try_send(BackendMessage::ConfigLoaded(cfg)); }
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to refresh config");
                            let _ = msg_tx.try_send(BackendMessage::ConfigError(e.to_string()));
                        }
                    }
                    (repaint)();
                }
            }
            UiCommand::UpdateConfig(update) => {
                if let Some(ref c) = client {
                    match c.update_config(*update).await {
                        Ok(cfg) => { let _ = msg_tx.try_send(BackendMessage::ConfigUpdated(cfg)); }
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to update config");
                            let _ = msg_tx.try_send(BackendMessage::ConfigError(e.to_string()));
                        }
                    }
                    (repaint)();
                }
            }

            // ── Agent / tool controls ─────────────────────────────────────────
            UiCommand::StopAgent(id) => {
                if let Some(ref c) = client {
                    if let Err(e) = c.stop_agent(id).await {
                        tracing::warn!(error = %e, agent = %id, "failed to stop agent");
                    }
                    (repaint)();
                }
            }
            UiCommand::CancelTask(id) => {
                if let Some(ref c) = client {
                    if let Err(e) = c.cancel_task(id).await {
                        tracing::warn!(error = %e, task = %id, "failed to cancel task");
                    }
                    (repaint)();
                }
            }
            UiCommand::ToggleTool { name, enabled } => {
                if let Some(ref c) = client {
                    if let Err(e) = c.toggle_tool(&name, enabled).await {
                        tracing::warn!(error = %e, tool = %name, "failed to toggle tool");
                    }
                    (repaint)();
                }
            }
            UiCommand::Shutdown { graceful } => {
                if let Some(ref c) = client {
                    if let Err(e) = c.shutdown(graceful, None).await {
                        tracing::warn!(error = %e, "failed to shutdown");
                    }
                    (repaint)();
                }
            }

            // ── Chat commands — delegated to chat_handlers ───────────────────
            UiCommand::CreateChatSession { model, provider, title } => {
                if let Some(ref c) = client {
                    chat_handlers::create_session(c, &msg_tx, &repaint, model, provider, title).await;
                }
            }
            UiCommand::LoadChatSessions => {
                if let Some(ref c) = client {
                    chat_handlers::load_sessions(c, &msg_tx, &repaint).await;
                }
            }
            UiCommand::LoadChatMessages { session_id } => {
                if let Some(ref c) = client {
                    chat_handlers::load_messages(c, &msg_tx, &repaint, session_id).await;
                }
            }
            UiCommand::SendChatMessage { session_id, content, orchestrate, attachments } => {
                if let Some(ref c) = client {
                    chat_handlers::send_message(c, &msg_tx, &repaint, session_id, content, orchestrate, attachments).await;
                }
            }
            UiCommand::CancelChatExecution { session_id } => {
                if let Some(ref c) = client {
                    chat_handlers::cancel_execution(c, &repaint, session_id).await;
                }
            }
            UiCommand::ResolvePermission { session_id, request_id, approve } => {
                if let Some(ref c) = client {
                    chat_handlers::resolve_permission(c, &repaint, session_id, request_id, approve).await;
                }
            }
            UiCommand::DeleteChatSession { session_id } => {
                if let Some(ref c) = client {
                    chat_handlers::delete_session(c, &repaint, session_id).await;
                }
            }
            UiCommand::RenameChatSession { session_id, title } => {
                if let Some(ref c) = client {
                    chat_handlers::rename_session(c, &msg_tx, &repaint, session_id, title).await;
                }
            }

            // ── Multimodal attachments ────────────────────────────────────────
            UiCommand::AttachFile { path } => {
                media_handlers::attach_file(path, &msg_tx, &repaint).await;
            }
            // RemoveAttachment and ClearAttachments are handled directly in app.rs
            // (they mutate ChatState without a backend round-trip).
            UiCommand::RemoveAttachment { .. } | UiCommand::ClearAttachments => {
                // No-op here; app.rs intercepts these before they reach the worker.
                (repaint)();
            }

            // ── File explorer — delegated to file_handlers ───────────────────
            UiCommand::LoadDirectory { path } => {
                file_handlers::load_directory(path, &msg_tx, &repaint).await;
            }
            UiCommand::LoadFile { path } => {
                file_handlers::load_file(path, &msg_tx, &repaint).await;
            }

            // ── Agent / task fire-and-forget ──────────────────────────────────
            UiCommand::InvokeAgent { agent_id, instruction } => {
                if let Some(ref c) = client {
                    let req = InvokeAgentRequest {
                        instruction,
                        context: HashMap::new(),
                        budget: None,
                        timeout_ms: None,
                    };
                    if let Err(e) = c.invoke_agent(agent_id, req).await {
                        tracing::warn!(agent = %agent_id, error = %e, "invoke_agent failed");
                        let _ = msg_tx.try_send(BackendMessage::OperationError(e.to_string()));
                    }
                    (repaint)();
                }
            }
            UiCommand::SubmitTask { instruction, agent_id } => {
                if let Some(ref c) = client {
                    let node = TaskNodeSpec {
                        task_id: Uuid::new_v4(),
                        instruction,
                        agent_selector: AgentSelectorSpec {
                            by_id: agent_id,
                            by_capability: None,
                            by_kind: None,
                            by_name: None,
                        },
                        depends_on: Vec::new(),
                        budget: None,
                        context_keys: Vec::new(),
                    };
                    let req = SubmitTaskRequest {
                        nodes: vec![node],
                        context: HashMap::new(),
                    };
                    if let Err(e) = c.submit_task(req).await {
                        tracing::warn!(error = %e, "submit_task failed");
                        let _ = msg_tx.try_send(BackendMessage::OperationError(e.to_string()));
                    }
                    (repaint)();
                }
            }
        }
    }
}
