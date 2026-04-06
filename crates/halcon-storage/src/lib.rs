//! Persistence layer for Halcon CLI.
//!
//! Uses SQLite (via rusqlite with bundled feature) for:
//! - Session storage (conversations, messages)
//! - Audit trail (immutable hash-chained events)
//! - Response cache
//! - Migration management
//!
//! The database is stored at `~/.halcon/halcon.db` by default.

pub mod ack_monitor;
pub mod async_db;
pub mod cache;
pub mod db;
pub mod dlq;
pub mod event_buffer;
pub mod event_store;
pub mod mailbox;
pub mod media;
pub mod memory;
pub mod metrics;
pub mod migrations;
pub mod resilience;
pub mod trace;

pub use ack_monitor::{AckMonitor, AckMonitorConfig};
pub use async_db::AsyncDatabase;
pub use cache::{CacheEntry, CacheStats};
pub use db::reasoning::ReasoningExperience;
pub use db::{AgentTaskRow, Database, PlanStepRow, SessionCheckpoint};
pub use dlq::{DeadLetterQueue, DlqStats, DlqStatus, FailedTask};
pub use event_buffer::{BufferStats, BufferedEvent, EventStatus, PersistentEventBuffer};
pub use event_store::{
    EventCategory, EventSnapshot, EventStore, EventStoreStats, ReplayQuery, StoredEvent,
};
pub use mailbox::{Mailbox, MailboxMessage};
pub use memory::{MemoryEntry, MemoryEntryType, MemoryEpisode, MemoryStats};
pub use metrics::{
    InvocationMetric, ModelStats, ProviderWindowedMetrics, SystemMetrics, ToolExecutionMetric,
    ToolStats,
};
pub use resilience::ResilienceEvent;
pub use trace::{TraceExport, TraceStep, TraceStepType};
