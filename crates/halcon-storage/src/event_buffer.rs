//! Persistent event buffer for bridge relay.
//!
//! Guarantees zero event loss by persisting all outgoing events to SQLite
//! before transmission. Events are marked as 'acked' when confirmed by backend.
//!
//! ## Schema
//!
//! ```sql
//! CREATE TABLE event_buffer (
//!     id INTEGER PRIMARY KEY AUTOINCREMENT,
//!     seq INTEGER NOT NULL,
//!     payload TEXT NOT NULL,
//!     status TEXT NOT NULL CHECK(status IN ('pending', 'sent', 'acked')),
//!     created_at INTEGER NOT NULL,
//!     sent_at INTEGER,
//!     acked_at INTEGER
//! );
//! CREATE INDEX idx_event_buffer_status ON event_buffer(status);
//! CREATE INDEX idx_event_buffer_seq ON event_buffer(seq);
//! ```

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// Event status in the buffer lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventStatus {
    /// Event persisted, not yet sent.
    Pending,
    /// Event sent over WebSocket, awaiting ACK.
    Sent,
    /// Event confirmed by backend ACK.
    Acked,
}

impl EventStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sent => "sent",
            Self::Acked => "acked",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "sent" => Some(Self::Sent),
            "acked" => Some(Self::Acked),
            _ => None,
        }
    }
}

/// Buffered event with metadata.
#[derive(Debug, Clone)]
pub struct BufferedEvent {
    pub id: i64,
    pub seq: u64,
    pub payload: String,
    pub status: EventStatus,
    pub created_at: u64,
    pub sent_at: Option<u64>,
    pub acked_at: Option<u64>,
}

/// Persistent event buffer backed by SQLite.
///
/// Thread-safe via rusqlite's Connection model (not Send, use within single thread
/// or wrap in Arc<Mutex<>>).
pub struct PersistentEventBuffer {
    conn: Connection,
    /// In-memory cache for fast access (LRU with max 1000 entries).
    /// Maps seq → payload for quick retransmit.
    memory_cache: std::collections::VecDeque<(u64, String)>,
    cache_capacity: usize,
}

impl PersistentEventBuffer {
    /// Open or create event buffer at the given path.
    ///
    /// Creates schema if not exists.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path.as_ref())
            .with_context(|| format!("Failed to open event buffer at {:?}", path.as_ref()))?;

        // Create schema
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS event_buffer (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                seq INTEGER NOT NULL,
                payload TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('pending', 'sent', 'acked')),
                created_at INTEGER NOT NULL,
                sent_at INTEGER,
                acked_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_event_buffer_status ON event_buffer(status);
            CREATE INDEX IF NOT EXISTS idx_event_buffer_seq ON event_buffer(seq);
            "#,
        )
        .context("Failed to create event_buffer schema")?;

        info!("Opened persistent event buffer at {:?}", path.as_ref());

        Ok(Self {
            conn,
            memory_cache: std::collections::VecDeque::with_capacity(1000),
            cache_capacity: 1000,
        })
    }

    /// Push a new event to the buffer.
    ///
    /// Returns the assigned event ID.
    pub fn push(&mut self, seq: u64, payload: String) -> Result<i64> {
        let now = current_timestamp();

        let _rows = self.conn.execute(
            "INSERT INTO event_buffer (seq, payload, status, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![seq, &payload, EventStatus::Pending.as_str(), now],
        )
        .context("Failed to insert event into buffer")?;

        let id = self.conn.last_insert_rowid();

        // Update memory cache
        if self.memory_cache.len() >= self.cache_capacity {
            self.memory_cache.pop_front();
        }
        self.memory_cache.push_back((seq, payload));

        debug!(seq, id, "Event buffered");
        Ok(id)
    }

    /// Mark an event as sent.
    pub fn mark_sent(&mut self, seq: u64) -> Result<()> {
        let now = current_timestamp();
        let rows = self.conn.execute(
            "UPDATE event_buffer SET status = ?1, sent_at = ?2 WHERE seq = ?3 AND status = 'pending'",
            params![EventStatus::Sent.as_str(), now, seq],
        )?;

        if rows > 0 {
            debug!(seq, "Event marked as sent");
        }
        Ok(())
    }

    /// Mark all events up to and including `seq` as acked.
    ///
    /// This is the backend ACK confirmation.
    pub fn mark_acked(&mut self, seq: u64) -> Result<usize> {
        let now = current_timestamp();
        let rows = self.conn.execute(
            "UPDATE event_buffer SET status = ?1, acked_at = ?2 WHERE seq <= ?3 AND status != 'acked'",
            params![EventStatus::Acked.as_str(), now, seq],
        )?;

        if rows > 0 {
            info!(seq, rows, "Events marked as acked");
        }
        Ok(rows)
    }

    /// Mark an event as failed (removed from active tracking).
    pub fn mark_failed(&mut self, seq: u64) -> Result<()> {
        let rows = self.conn.execute(
            "DELETE FROM event_buffer WHERE seq = ?1",
            params![seq],
        )?;

        if rows > 0 {
            warn!(seq, "Event marked as failed and removed from buffer");
        }
        Ok(())
    }

    /// Retrieve all sent events (awaiting ACK).
    pub fn get_sent(&self) -> Result<Vec<BufferedEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, seq, payload, status, created_at, sent_at, acked_at \
             FROM event_buffer WHERE status = 'sent' ORDER BY seq ASC",
        )?;

        let events = stmt
            .query_map([], |row| {
                Ok(BufferedEvent {
                    id: row.get(0)?,
                    seq: row.get(1)?,
                    payload: row.get(2)?,
                    status: EventStatus::from_str(&row.get::<_, String>(3)?)
                        .unwrap_or(EventStatus::Sent),
                    created_at: row.get(4)?,
                    sent_at: row.get(5)?,
                    acked_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(events)
    }

    /// Retrieve all pending events (not yet sent).
    pub fn get_pending(&self) -> Result<Vec<BufferedEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, seq, payload, status, created_at, sent_at, acked_at \
             FROM event_buffer WHERE status = 'pending' ORDER BY seq ASC",
        )?;

        let events = stmt
            .query_map([], |row| {
                Ok(BufferedEvent {
                    id: row.get(0)?,
                    seq: row.get(1)?,
                    payload: row.get(2)?,
                    status: EventStatus::from_str(&row.get::<_, String>(3)?)
                        .unwrap_or(EventStatus::Pending),
                    created_at: row.get(4)?,
                    sent_at: row.get(5)?,
                    acked_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(events)
    }

    /// Get the highest sequence number in the buffer (last event).
    pub fn last_seq(&self) -> Result<Option<u64>> {
        self.conn
            .query_row("SELECT MAX(seq) FROM event_buffer", [], |row| row.get(0))
            .optional()
            .context("Failed to query last seq")
    }

    /// Prune acked events older than `max_age_secs`.
    ///
    /// Returns count of deleted rows.
    pub fn prune_acked(&mut self, max_age_secs: u64) -> Result<usize> {
        let cutoff = current_timestamp().saturating_sub(max_age_secs);
        let rows = self.conn.execute(
            "DELETE FROM event_buffer WHERE status = 'acked' AND acked_at <= ?1",
            params![cutoff],
        )?;

        if rows > 0 {
            info!(rows, cutoff, "Pruned acked events");
        }
        Ok(rows)
    }

    /// Get buffer statistics.
    pub fn stats(&self) -> Result<BufferStats> {
        let pending: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM event_buffer WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )?;

        let sent: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM event_buffer WHERE status = 'sent'",
            [],
            |row| row.get(0),
        )?;

        let acked: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM event_buffer WHERE status = 'acked'",
            [],
            |row| row.get(0),
        )?;

        let total = pending + sent + acked;

        Ok(BufferStats {
            pending: pending as usize,
            sent: sent as usize,
            acked: acked as usize,
            total: total as usize,
        })
    }

    /// Recover: get all unsent events (pending + sent without ACK).
    ///
    /// Called on reconnect to retransmit.
    pub fn recover_unsent(&self) -> Result<Vec<BufferedEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, seq, payload, status, created_at, sent_at, acked_at \
             FROM event_buffer WHERE status IN ('pending', 'sent') ORDER BY seq ASC",
        )?;

        let events = stmt
            .query_map([], |row| {
                Ok(BufferedEvent {
                    id: row.get(0)?,
                    seq: row.get(1)?,
                    payload: row.get(2)?,
                    status: EventStatus::from_str(&row.get::<_, String>(3)?)
                        .unwrap_or(EventStatus::Pending),
                    created_at: row.get(4)?,
                    sent_at: row.get(5)?,
                    acked_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(events)
    }

    /// Get mutable connection reference (for testing).
    #[cfg(test)]
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

/// Buffer statistics snapshot.
#[derive(Debug, Clone, Copy)]
pub struct BufferStats {
    pub pending: usize,
    pub sent: usize,
    pub acked: usize,
    pub total: usize,
}

/// Get current UNIX timestamp in seconds.
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_create_and_push() {
        let tmpfile = NamedTempFile::new().unwrap();
        let mut buffer = PersistentEventBuffer::open(tmpfile.path()).unwrap();

        let id = buffer.push(1, r#"{"t":"test"}"#.to_string()).unwrap();
        assert!(id > 0);

        let pending = buffer.get_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].seq, 1);
        assert_eq!(pending[0].payload, r#"{"t":"test"}"#);
    }

    #[test]
    fn test_mark_sent_and_acked() {
        let tmpfile = NamedTempFile::new().unwrap();
        let mut buffer = PersistentEventBuffer::open(tmpfile.path()).unwrap();

        buffer.push(1, r#"{"t":"event1"}"#.to_string()).unwrap();
        buffer.push(2, r#"{"t":"event2"}"#.to_string()).unwrap();

        buffer.mark_sent(1).unwrap();

        let pending = buffer.get_pending().unwrap();
        assert_eq!(pending.len(), 1); // Only event 2

        let sent = buffer.get_sent().unwrap();
        assert_eq!(sent.len(), 1); // Event 1

        buffer.mark_acked(1).unwrap();

        let sent_after = buffer.get_sent().unwrap();
        assert_eq!(sent_after.len(), 0); // Event 1 now acked
    }

    #[test]
    fn test_recover_unsent() {
        let tmpfile = NamedTempFile::new().unwrap();
        let mut buffer = PersistentEventBuffer::open(tmpfile.path()).unwrap();

        buffer.push(1, r#"{"t":"event1"}"#.to_string()).unwrap();
        buffer.push(2, r#"{"t":"event2"}"#.to_string()).unwrap();
        buffer.mark_sent(1).unwrap();
        buffer.mark_acked(1).unwrap();

        let unsent = buffer.recover_unsent().unwrap();
        assert_eq!(unsent.len(), 1); // Only event 2 (pending)
        assert_eq!(unsent[0].seq, 2);
    }

    #[test]
    fn test_stats() {
        let tmpfile = NamedTempFile::new().unwrap();
        let mut buffer = PersistentEventBuffer::open(tmpfile.path()).unwrap();

        buffer.push(1, r#"{"t":"event1"}"#.to_string()).unwrap();
        buffer.push(2, r#"{"t":"event2"}"#.to_string()).unwrap();
        buffer.push(3, r#"{"t":"event3"}"#.to_string()).unwrap();

        buffer.mark_sent(1).unwrap();
        // mark_acked(2) marks ALL events with seq <= 2 as acked (including event 1)
        buffer.mark_acked(2).unwrap();

        let stats = buffer.stats().unwrap();
        assert_eq!(stats.pending, 1); // Event 3
        assert_eq!(stats.sent, 0);    // Event 1 was marked acked
        assert_eq!(stats.acked, 2);   // Events 1 and 2
        assert_eq!(stats.total, 3);
    }

    #[test]
    fn test_prune_acked() {
        let tmpfile = NamedTempFile::new().unwrap();
        let mut buffer = PersistentEventBuffer::open(tmpfile.path()).unwrap();

        buffer.push(1, r#"{"t":"old"}"#.to_string()).unwrap();
        buffer.mark_acked(1).unwrap();

        // Prune acked older than 0 seconds (all events up to now)
        let pruned = buffer.prune_acked(0).unwrap();
        assert_eq!(pruned, 1);

        let stats = buffer.stats().unwrap();
        assert_eq!(stats.total, 0);
    }
}
