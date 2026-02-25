//! Chat session command handlers — one async function per `UiCommand` variant.
//!
//! Each function takes the shared [`HalconClient`], the backend message sender,
//! and the repaint callback, plus the command-specific parameters extracted in
//! [`super::connection::run_connection_worker`].

use halcon_api::types::chat::{
    CreateSessionRequest, MediaAttachmentInline, PermissionDecisionStr,
    ResolvePermissionRequest, SubmitMessageRequest,
};
use halcon_client::HalconClient;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{BackendMessage, RepaintFn};

pub async fn create_session(
    c: &HalconClient,
    msg_tx: &mpsc::Sender<BackendMessage>,
    repaint: &RepaintFn,
    model: String,
    provider: String,
    title: Option<String>,
) {
    let req = CreateSessionRequest {
        model,
        provider,
        title,
        system_prompt: None,
        working_directory: None,
    };
    match c.create_chat_session(req).await {
        Ok(resp) => {
            let _ = msg_tx.try_send(BackendMessage::ChatSessionCreated(resp.session));
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to create chat session");
        }
    }
    (repaint)();
}

pub async fn load_sessions(
    c: &HalconClient,
    msg_tx: &mpsc::Sender<BackendMessage>,
    repaint: &RepaintFn,
) {
    match c.list_chat_sessions().await {
        Ok(resp) => {
            let _ = msg_tx.try_send(BackendMessage::ChatSessionsLoaded(resp.sessions));
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to list chat sessions");
        }
    }
    (repaint)();
}

pub async fn load_messages(
    c: &HalconClient,
    msg_tx: &mpsc::Sender<BackendMessage>,
    repaint: &RepaintFn,
    session_id: Uuid,
) {
    match c.list_chat_messages(session_id).await {
        Ok(resp) => {
            let _ = msg_tx.try_send(BackendMessage::ChatMessagesLoaded {
                session_id,
                messages: resp.messages,
            });
        }
        Err(e) => {
            tracing::warn!(error = %e, session_id = %session_id, "failed to load chat messages");
        }
    }
    (repaint)();
}

pub async fn send_message(
    c: &HalconClient,
    msg_tx: &mpsc::Sender<BackendMessage>,
    repaint: &RepaintFn,
    session_id: Uuid,
    content: String,
    orchestrate: bool,
    attachments: Vec<MediaAttachmentInline>,
) {
    let req = SubmitMessageRequest {
        content,
        orchestrate: Some(orchestrate),
        expert: None,
        attachments,
    };
    match c.submit_chat_message(session_id, req).await {
        Ok(_resp) => {
            // Streaming tokens arrive via WebSocket events.
            tracing::debug!(session_id = %session_id, "chat message submitted");
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to send chat message");
            let _ = msg_tx.try_send(BackendMessage::ChatTurnFailed {
                session_id,
                error: e.to_string(),
                recoverable: false,
            });
        }
    }
    (repaint)();
}

pub async fn cancel_execution(
    c: &HalconClient,
    repaint: &RepaintFn,
    session_id: Uuid,
) {
    if let Err(e) = c.cancel_chat_execution(session_id).await {
        tracing::warn!(error = %e, session_id = %session_id, "failed to cancel chat");
    }
    (repaint)();
}

pub async fn resolve_permission(
    c: &HalconClient,
    repaint: &RepaintFn,
    session_id: Uuid,
    request_id: Uuid,
    approve: bool,
) {
    let req = ResolvePermissionRequest {
        decision: if approve {
            PermissionDecisionStr::Approve
        } else {
            PermissionDecisionStr::Deny
        },
    };
    if let Err(e) = c.resolve_permission(session_id, request_id, req).await {
        tracing::warn!(error = %e, "failed to resolve permission");
    }
    (repaint)();
}

pub async fn delete_session(
    c: &HalconClient,
    repaint: &RepaintFn,
    session_id: Uuid,
) {
    if let Err(e) = c.delete_chat_session(session_id).await {
        tracing::warn!(error = %e, "failed to delete chat session");
    }
    (repaint)();
}

pub async fn rename_session(
    c: &HalconClient,
    msg_tx: &mpsc::Sender<BackendMessage>,
    repaint: &RepaintFn,
    session_id: Uuid,
    title: String,
) {
    match c.update_chat_session_title(session_id, title).await {
        Ok(resp) => {
            let _ = msg_tx.try_send(BackendMessage::ChatSessionRenamed {
                session_id,
                title: resp.title,
            });
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to rename chat session");
        }
    }
    (repaint)();
}
