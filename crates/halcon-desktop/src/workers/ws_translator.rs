//! WebSocket event → BackendMessage translation.
//!
//! Extracted from `workers/connection.rs` to keep the connection worker focused
//! on I/O and command dispatch.  Translation logic lives here, in isolation,
//! making it straightforward to unit-test independently of the async transport.

use halcon_api::types::ws::WsServerEvent;

use super::BackendMessage;

/// Translate a [`WsServerEvent`] into the most specific [`BackendMessage`] variant.
///
/// High-priority chat events get typed variants so `app.rs` can handle them
/// directly (streaming tokens, permission modals, turn lifecycle).
/// All other events fall through to [`BackendMessage::Event`].
pub fn translate_ws_event(event: WsServerEvent) -> BackendMessage {
    match event {
        WsServerEvent::ChatStreamToken {
            session_id,
            token,
            is_thinking,
            sequence_num,
        } => BackendMessage::ChatMessageReceived {
            session_id,
            token,
            is_thinking,
            sequence_num,
        },

        WsServerEvent::ConversationCompleted {
            session_id,
            stop_reason,
            total_duration_ms,
            ..
        } => BackendMessage::ChatTurnCompleted {
            session_id,
            assistant_text: String::new(), // tokens already streamed
            stop_reason,
            total_duration_ms,
        },

        WsServerEvent::ExecutionFailed {
            session_id,
            error_code,
            message,
            recoverable,
        } => BackendMessage::ChatTurnFailed {
            session_id,
            error: format!("{}: {}", error_code, message),
            recoverable,
        },

        WsServerEvent::PermissionRequired {
            request_id,
            session_id,
            tool_name,
            risk_level,
            description,
            deadline_secs,
            ..
        } => BackendMessage::ChatPermissionRequired {
            session_id,
            request_id,
            tool_name,
            risk_level,
            description,
            deadline_secs,
        },

        WsServerEvent::ChatSessionCreated { session_id, model, provider } => {
            use halcon_api::types::chat::{ChatSession, ChatSessionStatus};
            BackendMessage::ChatSessionCreated(ChatSession {
                id: session_id,
                title: None,
                model,
                provider,
                status: ChatSessionStatus::Idle,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                message_count: 0,
            })
        }

        WsServerEvent::SubAgentStarted {
            session_id,
            sub_agent_id,
            task_description,
            wave,
            allowed_tools,
        } => BackendMessage::SubAgentStarted {
            session_id,
            sub_agent_id,
            description: task_description,
            wave,
            allowed_tools,
        },

        WsServerEvent::SubAgentCompleted {
            session_id,
            sub_agent_id,
            success,
            summary,
            duration_ms,
            tools_used,
            ..
        } => BackendMessage::SubAgentCompleted {
            session_id,
            sub_agent_id,
            success,
            summary,
            duration_ms,
            tools_used,
        },

        // Media analysis progress — surfaced as a typed message so the UI can
        // display an inline progress indicator while attachments are analyzed.
        WsServerEvent::MediaAnalysisProgress {
            session_id,
            index,
            total,
            filename,
            ..
        } => BackendMessage::MediaAnalysisProgress {
            session_id,
            index,
            total,
            filename,
        },

        // B1: Permission timeout — surface as a typed message so the UI can
        // dismiss the pending modal deterministically without relying on silence.
        WsServerEvent::PermissionExpired { session_id, request_id, .. } => {
            BackendMessage::ChatPermissionExpired { session_id, request_id }
        }

        // Everything else goes to the generic event buffer.
        other => BackendMessage::Event(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use halcon_api::types::ws::WsServerEvent;

    /// C4: ConversationCompleted carries total_duration_ms into ChatTurnCompleted.
    #[test]
    fn conversation_completed_carries_duration() {
        let sid = Uuid::new_v4();
        let event = WsServerEvent::ConversationCompleted {
            session_id: sid,
            assistant_message_id: Uuid::new_v4(),
            stop_reason: "end_turn".to_string(),
            usage: halcon_api::types::chat::ChatTokenUsage {
                input: 10, output: 20, thinking: 0, total: 30,
            },
            total_duration_ms: 3_750,
        };
        let msg = translate_ws_event(event);
        match msg {
            BackendMessage::ChatTurnCompleted { session_id, total_duration_ms, .. } => {
                assert_eq!(session_id, sid);
                assert_eq!(total_duration_ms, 3_750);
            }
            other => panic!("expected ChatTurnCompleted, got {:?}", other),
        }
    }

    /// C3: SubAgentCompleted carries tools_used through the translation pipeline.
    #[test]
    fn sub_agent_completed_carries_tools_used() {
        let sid = Uuid::new_v4();
        let event = WsServerEvent::SubAgentCompleted {
            session_id: sid,
            sub_agent_id: "agent-abc".to_string(),
            success: true,
            summary: "done".to_string(),
            tools_used: vec!["file_read".to_string()],
            duration_ms: 1_000,
        };
        let msg = translate_ws_event(event);
        match msg {
            BackendMessage::SubAgentCompleted { tools_used, .. } => {
                assert_eq!(tools_used, vec!["file_read"]);
            }
            other => panic!("expected SubAgentCompleted, got {:?}", other),
        }
    }

    /// B1: PermissionExpired translates to ChatPermissionExpired with correct IDs.
    #[test]
    fn permission_expired_translates() {
        let sid = Uuid::new_v4();
        let rid = Uuid::new_v4();
        let event = WsServerEvent::PermissionExpired {
            session_id: sid,
            request_id: rid,
            deadline_elapsed_ms: 100,
        };
        let msg = translate_ws_event(event);
        match msg {
            BackendMessage::ChatPermissionExpired { session_id, request_id } => {
                assert_eq!(session_id, sid);
                assert_eq!(request_id, rid);
            }
            other => panic!("expected ChatPermissionExpired, got {:?}", other),
        }
    }

    /// F: Unknown events fall through to the generic Event buffer.
    #[test]
    fn unknown_event_goes_to_event_buffer() {
        let event = WsServerEvent::Pong;
        let msg = translate_ws_event(event);
        assert!(matches!(msg, BackendMessage::Event(WsServerEvent::Pong)));
    }

    /// E1/E2: MediaAnalysisProgress is translated to a typed MediaAnalysisProgress message.
    #[test]
    fn media_analysis_progress_translated() {
        let sid = Uuid::new_v4();
        let event = WsServerEvent::MediaAnalysisProgress {
            session_id: sid,
            index: 1,
            total: 3,
            filename: "photo.png".to_string(),
            modality: "image".to_string(),
        };
        let msg = translate_ws_event(event);
        match msg {
            BackendMessage::MediaAnalysisProgress { session_id, index, total, filename } => {
                assert_eq!(session_id, sid);
                assert_eq!(index, 1);
                assert_eq!(total, 3);
                assert_eq!(filename, "photo.png");
            }
            other => panic!("expected MediaAnalysisProgress, got {:?}", other),
        }
    }
}
