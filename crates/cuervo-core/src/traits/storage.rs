use async_trait::async_trait;
use uuid::Uuid;

use crate::error::Result;
use crate::types::{DomainEvent, Session};

/// Trait for session and event persistence.
///
/// Implemented by cuervo-storage (SQLite). The trait lives in core
/// to keep the domain independent of storage details.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Save or update a session.
    async fn save_session(&self, session: &Session) -> Result<()>;

    /// Load a session by ID.
    async fn load_session(&self, id: Uuid) -> Result<Option<Session>>;

    /// List recent sessions (most recent first).
    async fn list_sessions(&self, limit: u32) -> Result<Vec<Session>>;

    /// Delete a session by ID.
    async fn delete_session(&self, id: Uuid) -> Result<()>;
}

/// Trait for the immutable audit trail.
#[async_trait]
pub trait AuditStore: Send + Sync {
    /// Append an event to the audit log (immutable, hash-chained).
    async fn append_event(&self, event: &DomainEvent) -> Result<()>;

    /// Query audit events by time range and optional event type filter.
    async fn query_events(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        event_type: Option<&str>,
        limit: u32,
    ) -> Result<Vec<DomainEvent>>;
}
