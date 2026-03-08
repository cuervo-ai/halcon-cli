pub mod error;
pub mod security;
pub mod traits;
pub mod types;

/// Event bus type alias for domain events.
pub type EventSender = tokio::sync::broadcast::Sender<types::DomainEvent>;
pub type EventReceiver = tokio::sync::broadcast::Receiver<types::DomainEvent>;

/// Create a new event bus with the given capacity.
pub fn event_bus(capacity: usize) -> (EventSender, EventReceiver) {
    tokio::sync::broadcast::channel(capacity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    #[test]
    fn session_creation() {
        let session = Session::new(
            "claude-sonnet-4-5-20250929".to_string(),
            "anthropic".to_string(),
            "/tmp".to_string(),
        );
        assert!(session.messages.is_empty());
        assert_eq!(session.total_usage.total(), 0);
        assert_eq!(session.provider, "anthropic");
    }

    #[test]
    fn session_add_message_and_usage() {
        let mut session = Session::new(
            "test-model".to_string(),
            "test".to_string(),
            "/tmp".to_string(),
        );
        session.add_message(ChatMessage {
            role: Role::User,
            content: MessageContent::Text("hello".to_string()),
        });
        assert_eq!(session.messages.len(), 1);

        session.accumulate_usage(&TokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            ..Default::default()
        });
        assert_eq!(session.total_usage.total(), 30);
    }

    #[test]
    fn permission_level_ordering() {
        assert!(PermissionLevel::ReadOnly < PermissionLevel::ReadWrite);
        assert!(PermissionLevel::ReadWrite < PermissionLevel::Destructive);
    }

    #[test]
    fn domain_event_creation() {
        let event = DomainEvent::new(EventPayload::SessionStarted {
            session_id: uuid::Uuid::new_v4(),
        });
        assert!(!event.id.is_nil());
    }

    #[test]
    fn config_defaults() {
        let config = AppConfig::default();
        assert_eq!(config.general.default_provider, "anthropic");
        assert!(config.security.pii_detection);
        assert!(config.tools.confirm_destructive);
    }

    #[test]
    fn event_bus_send_receive() {
        let (tx, mut rx) = event_bus(16);
        let event = DomainEvent::new(EventPayload::SessionStarted {
            session_id: uuid::Uuid::new_v4(),
        });
        tx.send(event.clone()).unwrap();
        let received = rx.try_recv().unwrap();
        assert_eq!(received.id, event.id);
    }

    #[test]
    fn session_metrics_default_zero() {
        let session = Session::new("m".into(), "p".into(), "/tmp".into());
        assert_eq!(session.tool_invocations, 0);
        assert_eq!(session.agent_rounds, 0);
        assert_eq!(session.total_latency_ms, 0);
    }

    #[test]
    fn session_metrics_serde_backward_compat() {
        // Simulate a JSON session from an older version (no metric fields).
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "title": null,
            "model": "test",
            "provider": "test",
            "working_directory": "/tmp",
            "messages": [],
            "total_usage": {"input_tokens": 0, "output_tokens": 0},
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;
        let session: Session = serde_json::from_str(json).expect("should deserialize without new fields");
        assert_eq!(session.tool_invocations, 0);
        assert_eq!(session.agent_rounds, 0);
        assert_eq!(session.total_latency_ms, 0);
    }
}
