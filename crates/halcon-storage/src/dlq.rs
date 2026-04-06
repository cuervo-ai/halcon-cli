//! Dead Letter Queue (DLQ) for failed task delegation.
//!
//! Stores tasks that failed to execute or timed out, with exponential backoff
//! retry policy. Provides manual replay capability and observability into failures.
//!
//! ## Schema
//!
//! ```sql
//! CREATE TABLE failed_tasks (
//!     id INTEGER PRIMARY KEY AUTOINCREMENT,
//!     task_id TEXT NOT NULL UNIQUE,
//!     payload TEXT NOT NULL,
//!     error TEXT NOT NULL,
//!     retry_count INTEGER NOT NULL DEFAULT 0,
//!     max_retries INTEGER NOT NULL DEFAULT 3,
//!     last_attempt_at INTEGER NOT NULL,
//!     next_retry_at INTEGER,
//!     created_at INTEGER NOT NULL,
//!     status TEXT NOT NULL CHECK(status IN ('pending', 'exhausted', 'manual'))
//! );
//! CREATE INDEX idx_dlq_status ON failed_tasks(status);
//! CREATE INDEX idx_dlq_next_retry ON failed_tasks(next_retry_at);
//! ```

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// Status of a failed task in the DLQ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DlqStatus {
    /// Waiting for next retry.
    Pending,
    /// Max retries exhausted, manual intervention required.
    Exhausted,
    /// Moved to manual queue (requires explicit replay).
    Manual,
}

impl DlqStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Exhausted => "exhausted",
            Self::Manual => "manual",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "exhausted" => Some(Self::Exhausted),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }
}

/// Failed task entry in the DLQ.
#[derive(Debug, Clone)]
pub struct FailedTask {
    pub id: i64,
    pub task_id: String,
    pub payload: String,
    pub error: String,
    pub retry_count: u32,
    pub max_retries: u32,
    pub last_attempt_at: u64,
    pub next_retry_at: Option<u64>,
    pub created_at: u64,
    pub status: DlqStatus,
}

/// Dead Letter Queue for failed tasks.
pub struct DeadLetterQueue {
    conn: Connection,
    /// Base backoff in seconds (default: 60s).
    base_backoff_secs: u64,
    /// Max backoff cap in seconds (default: 3600s = 1 hour).
    max_backoff_secs: u64,
}

impl DeadLetterQueue {
    /// Open or create DLQ at the given path.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path.as_ref())
            .with_context(|| format!("Failed to open DLQ at {:?}", path.as_ref()))?;

        // Create schema
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS failed_tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL UNIQUE,
                payload TEXT NOT NULL,
                error TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                max_retries INTEGER NOT NULL DEFAULT 3,
                last_attempt_at INTEGER NOT NULL,
                next_retry_at INTEGER,
                created_at INTEGER NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('pending', 'exhausted', 'manual'))
            );
            CREATE INDEX IF NOT EXISTS idx_dlq_status ON failed_tasks(status);
            CREATE INDEX IF NOT EXISTS idx_dlq_next_retry ON failed_tasks(next_retry_at);
            "#,
        )
        .context("Failed to create failed_tasks schema")?;

        info!("Opened DLQ at {:?}", path.as_ref());

        Ok(Self {
            conn,
            base_backoff_secs: 60,
            max_backoff_secs: 3600,
        })
    }

    /// Add a failed task to the DLQ.
    ///
    /// If task already exists, increments retry_count and updates next_retry_at.
    pub fn add_failure(
        &mut self,
        task_id: &str,
        payload: String,
        error: String,
        max_retries: u32,
    ) -> Result<i64> {
        let now = current_timestamp();

        // Check if task already exists
        let existing: Option<(i64, u32)> = self
            .conn
            .query_row(
                "SELECT id, retry_count FROM failed_tasks WHERE task_id = ?1",
                params![task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some((id, retry_count)) = existing {
            // Update existing: increment retry_count
            let new_retry_count = retry_count + 1;
            let status = if new_retry_count >= max_retries {
                DlqStatus::Exhausted
            } else {
                DlqStatus::Pending
            };

            let next_retry = if status == DlqStatus::Pending {
                Some(now + self.calculate_backoff(new_retry_count))
            } else {
                None
            };

            self.conn.execute(
                "UPDATE failed_tasks SET error = ?1, retry_count = ?2, last_attempt_at = ?3, \
                 next_retry_at = ?4, status = ?5 WHERE id = ?6",
                params![error, new_retry_count, now, next_retry, status.as_str(), id],
            )?;

            if status == DlqStatus::Exhausted {
                warn!(
                    task_id,
                    retry_count = new_retry_count,
                    "Task exhausted max retries"
                );
            } else {
                info!(
                    task_id,
                    retry_count = new_retry_count,
                    next_retry_at = next_retry,
                    "Task retry scheduled"
                );
            }

            Ok(id)
        } else {
            // Insert new failure
            let next_retry = Some(now + self.base_backoff_secs);

            self.conn.execute(
                "INSERT INTO failed_tasks (task_id, payload, error, retry_count, max_retries, \
                 last_attempt_at, next_retry_at, created_at, status) \
                 VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?8)",
                params![
                    task_id,
                    payload,
                    error,
                    max_retries,
                    now,
                    next_retry,
                    now,
                    DlqStatus::Pending.as_str()
                ],
            )?;

            let id = self.conn.last_insert_rowid();
            info!(task_id, id, "Task added to DLQ");
            Ok(id)
        }
    }

    /// Get tasks ready for retry (next_retry_at <= now).
    pub fn get_ready_for_retry(&self) -> Result<Vec<FailedTask>> {
        let now = current_timestamp();

        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, payload, error, retry_count, max_retries, last_attempt_at, \
             next_retry_at, created_at, status FROM failed_tasks \
             WHERE status = 'pending' AND next_retry_at IS NOT NULL AND next_retry_at <= ?1 \
             ORDER BY next_retry_at ASC",
        )?;

        let tasks = stmt
            .query_map(params![now], |row| {
                Ok(FailedTask {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    payload: row.get(2)?,
                    error: row.get(3)?,
                    retry_count: row.get(4)?,
                    max_retries: row.get(5)?,
                    last_attempt_at: row.get(6)?,
                    next_retry_at: row.get(7)?,
                    created_at: row.get(8)?,
                    status: DlqStatus::from_str(&row.get::<_, String>(9)?)
                        .unwrap_or(DlqStatus::Pending),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(tasks)
    }

    /// Mark a task as successfully retried (remove from DLQ).
    pub fn mark_success(&mut self, task_id: &str) -> Result<()> {
        let rows = self.conn.execute(
            "DELETE FROM failed_tasks WHERE task_id = ?1",
            params![task_id],
        )?;

        if rows > 0 {
            info!(task_id, "Task retry succeeded, removed from DLQ");
        }
        Ok(())
    }

    /// Move a task to manual intervention queue.
    pub fn mark_manual(&mut self, task_id: &str) -> Result<()> {
        let rows = self.conn.execute(
            "UPDATE failed_tasks SET status = ?1, next_retry_at = NULL WHERE task_id = ?2",
            params![DlqStatus::Manual.as_str(), task_id],
        )?;

        if rows > 0 {
            info!(task_id, "Task moved to manual queue");
        }
        Ok(())
    }

    /// Get all exhausted tasks (max retries exceeded).
    pub fn get_exhausted(&self) -> Result<Vec<FailedTask>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, payload, error, retry_count, max_retries, last_attempt_at, \
             next_retry_at, created_at, status FROM failed_tasks \
             WHERE status = 'exhausted' ORDER BY created_at DESC",
        )?;

        let tasks = stmt
            .query_map([], |row| {
                Ok(FailedTask {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    payload: row.get(2)?,
                    error: row.get(3)?,
                    retry_count: row.get(4)?,
                    max_retries: row.get(5)?,
                    last_attempt_at: row.get(6)?,
                    next_retry_at: row.get(7)?,
                    created_at: row.get(8)?,
                    status: DlqStatus::from_str(&row.get::<_, String>(9)?)
                        .unwrap_or(DlqStatus::Exhausted),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(tasks)
    }

    /// Get all manual tasks (waiting for operator replay).
    pub fn get_manual(&self) -> Result<Vec<FailedTask>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, payload, error, retry_count, max_retries, last_attempt_at, \
             next_retry_at, created_at, status FROM failed_tasks \
             WHERE status = 'manual' ORDER BY created_at DESC",
        )?;

        let tasks = stmt
            .query_map([], |row| {
                Ok(FailedTask {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    payload: row.get(2)?,
                    error: row.get(3)?,
                    retry_count: row.get(4)?,
                    max_retries: row.get(5)?,
                    last_attempt_at: row.get(6)?,
                    next_retry_at: row.get(7)?,
                    created_at: row.get(8)?,
                    status: DlqStatus::from_str(&row.get::<_, String>(9)?)
                        .unwrap_or(DlqStatus::Manual),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(tasks)
    }

    /// Calculate exponential backoff in seconds.
    ///
    /// Formula: base * 2^retry_count, capped at max_backoff_secs.
    fn calculate_backoff(&self, retry_count: u32) -> u64 {
        let backoff = self.base_backoff_secs * 2_u64.saturating_pow(retry_count);
        backoff.min(self.max_backoff_secs)
    }

    /// Get DLQ statistics.
    pub fn stats(&self) -> Result<DlqStats> {
        let pending: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM failed_tasks WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )?;

        let exhausted: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM failed_tasks WHERE status = 'exhausted'",
            [],
            |row| row.get(0),
        )?;

        let manual: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM failed_tasks WHERE status = 'manual'",
            [],
            |row| row.get(0),
        )?;

        let total = pending + exhausted + manual;

        Ok(DlqStats {
            pending: pending as usize,
            exhausted: exhausted as usize,
            manual: manual as usize,
            total: total as usize,
        })
    }

    /// Prune old exhausted tasks (older than max_age_secs).
    pub fn prune_exhausted(&mut self, max_age_secs: u64) -> Result<usize> {
        let cutoff = current_timestamp().saturating_sub(max_age_secs);
        let rows = self.conn.execute(
            "DELETE FROM failed_tasks WHERE status = 'exhausted' AND created_at < ?1",
            params![cutoff],
        )?;

        if rows > 0 {
            info!(rows, cutoff, "Pruned exhausted tasks");
        }
        Ok(rows)
    }
}

/// DLQ statistics snapshot.
#[derive(Debug, Clone, Copy)]
pub struct DlqStats {
    pub pending: usize,
    pub exhausted: usize,
    pub manual: usize,
    pub total: usize,
}

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
    fn test_add_and_retrieve_failure() {
        let tmpfile = NamedTempFile::new().unwrap();
        let mut dlq = DeadLetterQueue::open(tmpfile.path()).unwrap();

        let id = dlq
            .add_failure(
                "task-1",
                r#"{"test":true}"#.to_string(),
                "timeout".to_string(),
                3,
            )
            .unwrap();
        assert!(id > 0);

        let pending = dlq.get_ready_for_retry().unwrap();
        // Not ready immediately (60s backoff)
        assert_eq!(pending.len(), 0);

        let stats = dlq.stats().unwrap();
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.exhausted, 0);
    }

    #[test]
    fn test_retry_exhaustion() {
        let tmpfile = NamedTempFile::new().unwrap();
        let mut dlq = DeadLetterQueue::open(tmpfile.path()).unwrap();

        // Add failure with max_retries = 2
        dlq.add_failure("task-1", "{}".to_string(), "error1".to_string(), 2)
            .unwrap();

        // Simulate 2 more failures (total 3 attempts)
        dlq.add_failure("task-1", "{}".to_string(), "error2".to_string(), 2)
            .unwrap();
        dlq.add_failure("task-1", "{}".to_string(), "error3".to_string(), 2)
            .unwrap();

        let exhausted = dlq.get_exhausted().unwrap();
        assert_eq!(exhausted.len(), 1);
        assert_eq!(exhausted[0].retry_count, 2);
    }

    #[test]
    fn test_mark_success() {
        let tmpfile = NamedTempFile::new().unwrap();
        let mut dlq = DeadLetterQueue::open(tmpfile.path()).unwrap();

        dlq.add_failure("task-1", "{}".to_string(), "error".to_string(), 3)
            .unwrap();

        dlq.mark_success("task-1").unwrap();

        let stats = dlq.stats().unwrap();
        assert_eq!(stats.total, 0);
    }

    #[test]
    fn test_manual_queue() {
        let tmpfile = NamedTempFile::new().unwrap();
        let mut dlq = DeadLetterQueue::open(tmpfile.path()).unwrap();

        dlq.add_failure("task-1", "{}".to_string(), "error".to_string(), 3)
            .unwrap();

        dlq.mark_manual("task-1").unwrap();

        let manual = dlq.get_manual().unwrap();
        assert_eq!(manual.len(), 1);
        assert_eq!(manual[0].status, DlqStatus::Manual);
    }

    #[test]
    fn test_backoff_calculation() {
        let tmpfile = NamedTempFile::new().unwrap();
        let dlq = DeadLetterQueue::open(tmpfile.path()).unwrap();

        assert_eq!(dlq.calculate_backoff(0), 60);
        assert_eq!(dlq.calculate_backoff(1), 120);
        assert_eq!(dlq.calculate_backoff(2), 240);
        assert_eq!(dlq.calculate_backoff(3), 480);
        // Capped at max
        assert_eq!(dlq.calculate_backoff(10), 3600);
    }
}
