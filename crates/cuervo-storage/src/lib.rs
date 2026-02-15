//! Persistence layer for Cuervo CLI.
//!
//! Uses SQLite (via rusqlite with bundled feature) for:
//! - Session storage (conversations, messages)
//! - Audit trail (immutable hash-chained events)
//! - Response cache
//! - Migration management
//!
//! The database is stored at `~/.cuervo/cuervo.db` by default.

pub mod async_db;
pub mod cache;
pub mod memory;
pub mod metrics;
pub mod migrations;
pub mod resilience;
pub mod db;
pub mod trace;

pub use async_db::AsyncDatabase;
pub use cache::{CacheEntry, CacheStats};
pub use db::{AgentTaskRow, Database, PlanStepRow, SessionCheckpoint};
pub use db::reasoning::ReasoningExperience;
pub use memory::{MemoryEntry, MemoryEntryType, MemoryEpisode, MemoryStats};
pub use metrics::{InvocationMetric, ModelStats, ProviderWindowedMetrics, SystemMetrics, ToolExecutionMetric, ToolStats};
pub use resilience::ResilienceEvent;
pub use trace::{TraceExport, TraceStep, TraceStepType};
