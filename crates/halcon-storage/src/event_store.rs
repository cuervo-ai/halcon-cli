//! Global Event Store — append-only, ordered journal for system-wide event sourcing.
//!
//! Every significant system event (DAG mutations, permission decisions, agent lifecycle,
//! Cenzontle decisions, execution state changes) is persisted here with a monotonic
//! sequence number and W3C trace context.
//!
//! Supports:
//! - Append-only writes (no update/delete of events)
//! - Replay from any sequence number (time-travel debugging)
//! - Snapshots for fast state reconstruction
//! - Session-scoped and global queries
//! - Streaming (poll from last_seq)

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

/// A single event in the global store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Monotonically increasing sequence number (global ordering).
    pub seq: u64,
    /// Unique event ID.
    pub event_id: Uuid,
    /// Session this event belongs to (None = system-level).
    pub session_id: Option<Uuid>,
    /// Event category for filtering.
    pub category: EventCategory,
    /// Event type tag (e.g., "dag_mutation", "permission_resolved").
    pub event_type: String,
    /// JSON-serialized event payload.
    pub payload: String,
    /// W3C trace ID for distributed tracing.
    pub trace_id: Option<String>,
    /// W3C span ID.
    pub span_id: Option<String>,
    /// Timestamp of event creation.
    pub timestamp: DateTime<Utc>,
}

/// Event categories for efficient filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    /// DAG structure changes (insert, remove, update nodes/edges).
    DagMutation,
    /// Execution state transitions (running, completed, failed).
    Execution,
    /// Permission lifecycle (requested, approved, denied, expired).
    Permission,
    /// Agent lifecycle (started, completed, failed).
    Agent,
    /// Planning decisions (plan generated, replan, plan accepted).
    Planning,
    /// Cenzontle synchronization events.
    Cenzontle,
    /// Session lifecycle (created, deleted).
    Session,
    /// Tool execution events.
    Tool,
    /// System-level events (config, health).
    System,
}

impl EventCategory {
    fn as_str(&self) -> &'static str {
        match self {
            Self::DagMutation => "dag_mutation",
            Self::Execution => "execution",
            Self::Permission => "permission",
            Self::Agent => "agent",
            Self::Planning => "planning",
            Self::Cenzontle => "cenzontle",
            Self::Session => "session",
            Self::Tool => "tool",
            Self::System => "system",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "dag_mutation" => Self::DagMutation,
            "execution" => Self::Execution,
            "permission" => Self::Permission,
            "agent" => Self::Agent,
            "planning" => Self::Planning,
            "cenzontle" => Self::Cenzontle,
            "session" => Self::Session,
            "tool" => Self::Tool,
            "system" => Self::System,
            _ => Self::System,
        }
    }
}

/// A snapshot of system state at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSnapshot {
    /// Sequence number this snapshot was taken at.
    pub at_seq: u64,
    /// Session this snapshot belongs to (None = global).
    pub session_id: Option<Uuid>,
    /// JSON-serialized state.
    pub state_json: String,
    /// Snapshot creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Query parameters for replaying events.
#[derive(Debug, Clone, Default)]
pub struct ReplayQuery {
    /// Start from this sequence number (inclusive).
    pub from_seq: u64,
    /// End at this sequence number (inclusive). 0 = no limit.
    pub to_seq: u64,
    /// Filter by session ID.
    pub session_id: Option<Uuid>,
    /// Filter by event categories.
    pub categories: Vec<EventCategory>,
    /// Maximum number of events to return.
    pub limit: usize,
}

/// Statistics about the event store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventStoreStats {
    pub total_events: u64,
    pub max_seq: u64,
    pub snapshot_count: u64,
    pub events_by_category: Vec<(String, u64)>,
    pub oldest_event: Option<DateTime<Utc>>,
    pub newest_event: Option<DateTime<Utc>>,
}

/// The global event store.
/// Thread-safe via internal Mutex<Connection>.
pub struct EventStore {
    conn: Mutex<Connection>,
}

impl EventStore {
    /// Get a locked reference to the connection.
    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    /// Open or create the event store at the given path.
    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;
             PRAGMA synchronous=NORMAL;",
        )?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                session_id TEXT,
                category TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                trace_id TEXT,
                span_id TEXT,
                timestamp TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_events_session
                ON events(session_id) WHERE session_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_events_category
                ON events(category);
            CREATE INDEX IF NOT EXISTS idx_events_timestamp
                ON events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_events_type
                ON events(event_type);

            CREATE TABLE IF NOT EXISTS snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                at_seq INTEGER NOT NULL,
                session_id TEXT,
                state_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_snapshots_session
                ON snapshots(session_id);
            CREATE INDEX IF NOT EXISTS idx_snapshots_seq
                ON snapshots(at_seq DESC);",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Open an in-memory event store (for testing).
    pub fn open_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.conn().execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                session_id TEXT,
                category TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                trace_id TEXT,
                span_id TEXT,
                timestamp TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                at_seq INTEGER NOT NULL,
                session_id TEXT,
                state_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );",
        )?;
        Ok(store)
    }

    /// Append an event to the store. Returns the assigned sequence number.
    pub fn append(
        &self,
        event_id: Uuid,
        session_id: Option<Uuid>,
        category: EventCategory,
        event_type: &str,
        payload: &str,
        trace_id: Option<&str>,
        span_id: Option<&str>,
    ) -> Result<u64, rusqlite::Error> {
        let now = Utc::now().to_rfc3339();
        let sid = session_id.map(|s| s.to_string());

        let conn = self.conn();
        conn.execute(
            "INSERT INTO events (event_id, session_id, category, event_type, payload, trace_id, span_id, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event_id.to_string(),
                sid,
                category.as_str(),
                event_type,
                payload,
                trace_id,
                span_id,
                now,
            ],
        )?;

        Ok(conn.last_insert_rowid() as u64)
    }

    /// Replay events matching the given query.
    pub fn replay(&self, query: &ReplayQuery) -> Result<Vec<StoredEvent>, rusqlite::Error> {
        let mut sql = String::from(
            "SELECT seq, event_id, session_id, category, event_type, payload, trace_id, span_id, timestamp
             FROM events WHERE seq >= ?1",
        );
        let mut param_idx = 2u32;

        if query.to_seq > 0 {
            sql.push_str(&format!(" AND seq <= ?{param_idx}"));
            param_idx += 1;
        }
        if query.session_id.is_some() {
            sql.push_str(&format!(" AND session_id = ?{param_idx}"));
            param_idx += 1;
        }
        if !query.categories.is_empty() {
            let placeholders: Vec<String> = query
                .categories
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", param_idx + i as u32))
                .collect();
            sql.push_str(&format!(" AND category IN ({})", placeholders.join(",")));
        }

        sql.push_str(" ORDER BY seq ASC");

        if query.limit > 0 {
            sql.push_str(&format!(" LIMIT {}", query.limit));
        }

        // Build dynamic params — rusqlite needs &dyn ToSql
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        params_vec.push(Box::new(query.from_seq as i64));
        if query.to_seq > 0 {
            params_vec.push(Box::new(query.to_seq as i64));
        }
        if let Some(ref sid) = query.session_id {
            params_vec.push(Box::new(sid.to_string()));
        }
        for cat in &query.categories {
            params_vec.push(Box::new(cat.as_str().to_string()));
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            let seq: i64 = row.get(0)?;
            let event_id_str: String = row.get(1)?;
            let session_id_str: Option<String> = row.get(2)?;
            let category_str: String = row.get(3)?;
            let event_type: String = row.get(4)?;
            let payload: String = row.get(5)?;
            let trace_id: Option<String> = row.get(6)?;
            let span_id: Option<String> = row.get(7)?;
            let timestamp_str: String = row.get(8)?;

            Ok(StoredEvent {
                seq: seq as u64,
                event_id: Uuid::parse_str(&event_id_str).unwrap_or_default(),
                session_id: session_id_str
                    .and_then(|s| Uuid::parse_str(&s).ok()),
                category: EventCategory::from_str(&category_str),
                event_type,
                payload,
                trace_id,
                span_id,
                timestamp: DateTime::parse_from_rfc3339(&timestamp_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;

        rows.collect()
    }

    /// Get events after a sequence number (for streaming / polling).
    pub fn events_after(&self, after_seq: u64, limit: usize) -> Result<Vec<StoredEvent>, rusqlite::Error> {
        self.replay(&ReplayQuery {
            from_seq: after_seq + 1,
            limit,
            ..Default::default()
        })
    }

    /// Get the current maximum sequence number.
    pub fn max_seq(&self) -> Result<u64, rusqlite::Error> {
        self.conn()
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|v| v as u64)
    }

    /// Save a state snapshot at the current sequence number.
    pub fn save_snapshot(
        &self,
        session_id: Option<Uuid>,
        state_json: &str,
    ) -> Result<u64, rusqlite::Error> {
        let at_seq = self.max_seq()?;
        let now = Utc::now().to_rfc3339();
        let sid = session_id.map(|s| s.to_string());

        self.conn().execute(
            "INSERT INTO snapshots (at_seq, session_id, state_json, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![at_seq as i64, sid, state_json, now],
        )?;

        Ok(at_seq)
    }

    /// Load the most recent snapshot for a session (or global if None).
    pub fn latest_snapshot(
        &self,
        session_id: Option<Uuid>,
    ) -> Result<Option<EventSnapshot>, rusqlite::Error> {
        let (sql, param): (&str, Option<String>) = match session_id {
            Some(sid) => (
                "SELECT at_seq, session_id, state_json, created_at FROM snapshots
                 WHERE session_id = ?1 ORDER BY at_seq DESC LIMIT 1",
                Some(sid.to_string()),
            ),
            None => (
                "SELECT at_seq, session_id, state_json, created_at FROM snapshots
                 WHERE session_id IS NULL ORDER BY at_seq DESC LIMIT 1",
                None,
            ),
        };

        self.conn()
            .query_row(sql, rusqlite::params_from_iter(param.iter()), |row| {
                let at_seq: i64 = row.get(0)?;
                let sid_str: Option<String> = row.get(1)?;
                let state_json: String = row.get(2)?;
                let created_str: String = row.get(3)?;

                Ok(EventSnapshot {
                    at_seq: at_seq as u64,
                    session_id: sid_str.and_then(|s| Uuid::parse_str(&s).ok()),
                    state_json,
                    created_at: DateTime::parse_from_rfc3339(&created_str)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                })
            })
            .optional()
    }

    /// Get store statistics.
    pub fn stats(&self) -> Result<EventStoreStats, rusqlite::Error> {
        let total_events: i64 =
            self.conn()
                .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        let max_seq: i64 = self
            .conn()
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |row| {
                row.get(0)
            })?;
        let snapshot_count: i64 =
            self.conn()
                .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))?;

        let oldest: Option<String> = self
            .conn()
            .query_row(
                "SELECT MIN(timestamp) FROM events",
                [],
                |row| row.get(0),
            )
            .optional()?
            .flatten();

        let newest: Option<String> = self
            .conn()
            .query_row(
                "SELECT MAX(timestamp) FROM events",
                [],
                |row| row.get(0),
            )
            .optional()?
            .flatten();

        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT category, COUNT(*) FROM events GROUP BY category")?;
        let by_category: Vec<(String, u64)> = stmt
            .query_map([], |row| {
                let cat: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((cat, count as u64))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(EventStoreStats {
            total_events: total_events as u64,
            max_seq: max_seq as u64,
            snapshot_count: snapshot_count as u64,
            events_by_category: by_category,
            oldest_event: oldest.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .ok()
            }),
            newest_event: newest.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .ok()
            }),
        })
    }

    /// Prune events older than the given sequence number.
    /// Keeps at least one snapshot before the prune point for reconstruction.
    pub fn prune_before(&self, before_seq: u64) -> Result<u64, rusqlite::Error> {
        let deleted: usize = self.conn().execute(
            "DELETE FROM events WHERE seq < ?1",
            params![before_seq as i64],
        )?;
        Ok(deleted as u64)
    }

    /// Count events in a category for a session.
    pub fn count_by_category(
        &self,
        session_id: Option<Uuid>,
        category: EventCategory,
    ) -> Result<u64, rusqlite::Error> {
        let count: i64 = match session_id {
            Some(sid) => self.conn().query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = ?1 AND category = ?2",
                params![sid.to_string(), category.as_str()],
                |row| row.get(0),
            )?,
            None => self.conn().query_row(
                "SELECT COUNT(*) FROM events WHERE category = ?1",
                params![category.as_str()],
                |row| row.get(0),
            )?,
        };
        Ok(count as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_replay() {
        let store = EventStore::open_in_memory().unwrap();
        let sid = Uuid::new_v4();

        let seq1 = store
            .append(
                Uuid::new_v4(),
                Some(sid),
                EventCategory::Execution,
                "task_started",
                r#"{"task_id":"abc"}"#,
                None,
                None,
            )
            .unwrap();

        let seq2 = store
            .append(
                Uuid::new_v4(),
                Some(sid),
                EventCategory::Tool,
                "tool_executed",
                r#"{"tool":"bash","success":true}"#,
                Some("trace-001"),
                Some("span-001"),
            )
            .unwrap();

        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);

        let all = store
            .replay(&ReplayQuery {
                from_seq: 0,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].event_type, "task_started");
        assert_eq!(all[1].event_type, "tool_executed");
        assert_eq!(all[1].trace_id.as_deref(), Some("trace-001"));
    }

    #[test]
    fn replay_with_session_filter() {
        let store = EventStore::open_in_memory().unwrap();
        let sid1 = Uuid::new_v4();
        let sid2 = Uuid::new_v4();

        store
            .append(Uuid::new_v4(), Some(sid1), EventCategory::Execution, "e1", "{}", None, None)
            .unwrap();
        store
            .append(Uuid::new_v4(), Some(sid2), EventCategory::Execution, "e2", "{}", None, None)
            .unwrap();
        store
            .append(Uuid::new_v4(), Some(sid1), EventCategory::Tool, "e3", "{}", None, None)
            .unwrap();

        let filtered = store
            .replay(&ReplayQuery {
                from_seq: 0,
                session_id: Some(sid1),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn replay_with_category_filter() {
        let store = EventStore::open_in_memory().unwrap();

        store
            .append(Uuid::new_v4(), None, EventCategory::Execution, "e1", "{}", None, None)
            .unwrap();
        store
            .append(Uuid::new_v4(), None, EventCategory::Permission, "e2", "{}", None, None)
            .unwrap();
        store
            .append(Uuid::new_v4(), None, EventCategory::Execution, "e3", "{}", None, None)
            .unwrap();

        let filtered = store
            .replay(&ReplayQuery {
                from_seq: 0,
                categories: vec![EventCategory::Permission],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].event_type, "e2");
    }

    #[test]
    fn snapshot_and_restore() {
        let store = EventStore::open_in_memory().unwrap();
        let sid = Uuid::new_v4();

        store
            .append(Uuid::new_v4(), Some(sid), EventCategory::Execution, "e1", "{}", None, None)
            .unwrap();
        store
            .append(Uuid::new_v4(), Some(sid), EventCategory::Execution, "e2", "{}", None, None)
            .unwrap();

        let snap_seq = store
            .save_snapshot(Some(sid), r#"{"state":"after_2_events"}"#)
            .unwrap();
        assert_eq!(snap_seq, 2);

        let loaded = store.latest_snapshot(Some(sid)).unwrap().unwrap();
        assert_eq!(loaded.at_seq, 2);
        assert!(loaded.state_json.contains("after_2_events"));
    }

    #[test]
    fn events_after_polling() {
        let store = EventStore::open_in_memory().unwrap();

        store
            .append(Uuid::new_v4(), None, EventCategory::System, "e1", "{}", None, None)
            .unwrap();
        store
            .append(Uuid::new_v4(), None, EventCategory::System, "e2", "{}", None, None)
            .unwrap();
        store
            .append(Uuid::new_v4(), None, EventCategory::System, "e3", "{}", None, None)
            .unwrap();

        let after_1 = store.events_after(1, 100).unwrap();
        assert_eq!(after_1.len(), 2);
        assert_eq!(after_1[0].seq, 2);
        assert_eq!(after_1[1].seq, 3);
    }

    #[test]
    fn stats_correct() {
        let store = EventStore::open_in_memory().unwrap();

        store
            .append(Uuid::new_v4(), None, EventCategory::Execution, "e1", "{}", None, None)
            .unwrap();
        store
            .append(Uuid::new_v4(), None, EventCategory::Execution, "e2", "{}", None, None)
            .unwrap();
        store
            .append(Uuid::new_v4(), None, EventCategory::Permission, "e3", "{}", None, None)
            .unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.total_events, 3);
        assert_eq!(stats.max_seq, 3);
        assert_eq!(stats.snapshot_count, 0);
        assert!(stats.events_by_category.iter().any(|(c, n)| c == "execution" && *n == 2));
        assert!(stats.events_by_category.iter().any(|(c, n)| c == "permission" && *n == 1));
    }

    #[test]
    fn prune_removes_old_events() {
        let store = EventStore::open_in_memory().unwrap();

        for i in 0..5 {
            store
                .append(
                    Uuid::new_v4(),
                    None,
                    EventCategory::System,
                    &format!("e{i}"),
                    "{}",
                    None,
                    None,
                )
                .unwrap();
        }

        let deleted = store.prune_before(3).unwrap();
        assert_eq!(deleted, 2);

        let remaining = store
            .replay(&ReplayQuery {
                from_seq: 0,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(remaining.len(), 3);
        assert_eq!(remaining[0].seq, 3);
    }

    #[test]
    fn max_seq_empty() {
        let store = EventStore::open_in_memory().unwrap();
        assert_eq!(store.max_seq().unwrap(), 0);
    }

    #[test]
    fn duplicate_event_id_rejected() {
        let store = EventStore::open_in_memory().unwrap();
        let eid = Uuid::new_v4();

        store
            .append(eid, None, EventCategory::System, "e1", "{}", None, None)
            .unwrap();
        let result = store.append(eid, None, EventCategory::System, "e2", "{}", None, None);
        assert!(result.is_err()); // UNIQUE constraint on event_id
    }

    #[test]
    fn replay_with_limit() {
        let store = EventStore::open_in_memory().unwrap();

        for i in 0..10 {
            store
                .append(
                    Uuid::new_v4(),
                    None,
                    EventCategory::System,
                    &format!("e{i}"),
                    "{}",
                    None,
                    None,
                )
                .unwrap();
        }

        let limited = store
            .replay(&ReplayQuery {
                from_seq: 0,
                limit: 3,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(limited.len(), 3);
    }

    #[test]
    fn count_by_category() {
        let store = EventStore::open_in_memory().unwrap();
        let sid = Uuid::new_v4();

        store.append(Uuid::new_v4(), Some(sid), EventCategory::Tool, "t1", "{}", None, None).unwrap();
        store.append(Uuid::new_v4(), Some(sid), EventCategory::Tool, "t2", "{}", None, None).unwrap();
        store.append(Uuid::new_v4(), Some(sid), EventCategory::Permission, "p1", "{}", None, None).unwrap();

        assert_eq!(store.count_by_category(Some(sid), EventCategory::Tool).unwrap(), 2);
        assert_eq!(store.count_by_category(Some(sid), EventCategory::Permission).unwrap(), 1);
        assert_eq!(store.count_by_category(Some(sid), EventCategory::Agent).unwrap(), 0);
    }
}
