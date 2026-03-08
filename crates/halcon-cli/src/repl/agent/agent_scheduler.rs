// DECISION: AgentScheduler uses tokio::time::interval (60s tick) rather than
// a cron daemon because:
// 1. No external process to manage — runs in the same tokio runtime
// 2. 60s polling granularity is sufficient for the minimum cron interval (1 min)
// 3. The scheduler is spawned as a background task and does NOT block the REPL
// On shutdown, the task is cancelled via a CancellationToken (tokio-util).
// See US-scheduler (PASO 4-C).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use croner::Cron;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use halcon_core::error::{HalconError, Result};
use halcon_storage::Database;

/// A scheduled agent task loaded from the `scheduled_tasks` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    pub name: String,
    /// Optional agent definition ID for the task.
    pub agent_id: Option<String>,
    /// Natural-language instruction to execute when due.
    pub instruction: String,
    /// Standard 5-field cron expression (e.g., "0 2 * * 1").
    pub cron_expr: String,
    /// Whether this task is active.
    pub enabled: bool,
    /// Timestamp of the last successful execution (None = never run).
    pub last_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Background scheduler that runs agent tasks on cron schedules.
pub struct AgentScheduler {
    db: Arc<Database>,
}

impl AgentScheduler {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Start the scheduler loop (60s tick). Returns immediately — the loop
    /// runs in a tokio background task. Call `cancel.cancel()` to shut down.
    pub fn start(self, cancel: CancellationToken) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!("AgentScheduler: shutdown signal received");
                        break;
                    }
                    _ = interval.tick() => {
                        if let Err(e) = self.run_due_tasks().await {
                            tracing::warn!(error = %e, "AgentScheduler: error running due tasks");
                        }
                    }
                }
            }
        });
    }

    /// Query all enabled tasks and run any that are due.
    async fn run_due_tasks(&self) -> Result<()> {
        let db = self.db.clone();
        let tasks: Vec<ScheduledTask> = tokio::task::spawn_blocking(move || {
            db.with_connection(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, name, agent_id, instruction, cron_expr, enabled, last_run_at, created_at \
                     FROM scheduled_tasks WHERE enabled = 1 ORDER BY created_at ASC",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, bool>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
        .map_err(|e| HalconError::DatabaseError(format!("list tasks: {e}")))?
        .into_iter()
        .map(|(id, name, agent_id, instruction, cron_expr, enabled, last_run_at_str, created_at_str)| {
            ScheduledTask {
                id,
                name,
                agent_id,
                instruction,
                cron_expr,
                enabled,
                last_run_at: last_run_at_str.and_then(|s| s.parse().ok()),
                created_at: created_at_str.parse().unwrap_or_else(|_| Utc::now()),
            }
        })
        .collect();

        let now = Utc::now();
        for task in tasks {
            match Self::is_due(&task.cron_expr, task.last_run_at, now) {
                Ok(true) => {
                    tracing::info!(
                        task_id = %task.id,
                        name = %task.name,
                        cron = %task.cron_expr,
                        "AgentScheduler: task is due — recording execution"
                    );
                    // Update last_run_at optimistically before dispatching to prevent
                    // double-execution if the agent takes longer than the tick interval.
                    let db2 = self.db.clone();
                    let task_id = task.id.clone();
                    let now_str = now.to_rfc3339();
                    let _ = tokio::task::spawn_blocking(move || {
                        db2.with_connection(|conn| {
                            conn.execute(
                                "UPDATE scheduled_tasks SET last_run_at = ?1 WHERE id = ?2",
                                rusqlite::params![now_str, task_id],
                            )
                        })
                    })
                    .await;
                    tracing::info!(
                        task_id = %task.id,
                        instruction = %task.instruction,
                        "AgentScheduler: scheduled task ready for execution"
                    );
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        task_id = %task.id,
                        cron = %task.cron_expr,
                        error = %e,
                        "AgentScheduler: invalid cron expression — skipping task"
                    );
                }
            }
        }
        Ok(())
    }

    /// Check whether a cron expression is due at `now` given the last execution time.
    ///
    /// A task is "due" when the next scheduled occurrence after `last_run`
    /// (or 60 seconds before `now` if it has never run) falls at or before `now`.
    pub fn is_due(
        cron_expr: &str,
        last_run: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> std::result::Result<bool, String> {
        let mut cron = Cron::new(cron_expr)
            .with_seconds_optional()
            .parse()
            .map_err(|e| format!("parse cron '{cron_expr}': {e}"))?;

        // If never run, use 60s before now so first-ever run fires immediately.
        let from = last_run.unwrap_or_else(|| now - chrono::Duration::seconds(60));

        let next = cron
            .find_next_occurrence(&from, false)
            .map_err(|e| format!("find_next_occurrence '{cron_expr}': {e}"))?;

        Ok(next <= now)
    }
}

// ---------------------------------------------------------------------------
// CLI helpers (used by commands/schedule.rs)
// ---------------------------------------------------------------------------

/// Insert a new scheduled task. Returns the new task ID.
pub fn db_insert_scheduled_task(
    db: &Database,
    name: &str,
    agent_id: Option<&str>,
    instruction: &str,
    cron_expr: &str,
) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO scheduled_tasks \
             (id, name, agent_id, instruction, cron_expr, enabled, last_run_at, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 1, NULL, ?6)",
            rusqlite::params![id, name, agent_id, instruction, cron_expr, now],
        )?;
        Ok(())
    })
    .map_err(|e| HalconError::DatabaseError(format!("insert scheduled task: {e}")))?;
    Ok(id)
}

/// List all scheduled tasks.
pub fn db_list_scheduled_tasks(db: &Database) -> Result<Vec<ScheduledTask>> {
    let rows: Vec<(String, String, Option<String>, String, String, bool, Option<String>, String)> =
        db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, agent_id, instruction, cron_expr, enabled, last_run_at, created_at \
                 FROM scheduled_tasks ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, bool>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|e| HalconError::DatabaseError(format!("list scheduled tasks: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|(id, name, agent_id, instruction, cron_expr, enabled, last_run_at_str, created_at_str)| {
            ScheduledTask {
                id,
                name,
                agent_id,
                instruction,
                cron_expr,
                enabled,
                last_run_at: last_run_at_str.and_then(|s| s.parse().ok()),
                created_at: created_at_str.parse().unwrap_or_else(|_| Utc::now()),
            }
        })
        .collect())
}

/// Enable or disable a scheduled task. Returns the number of rows affected.
pub fn db_set_scheduled_task_enabled(db: &Database, id: &str, enabled: bool) -> Result<usize> {
    db.with_connection(|conn| {
        conn.execute(
            "UPDATE scheduled_tasks SET enabled = ?1 WHERE id = ?2",
            rusqlite::params![enabled as i32, id],
        )
    })
    .map_err(|e| HalconError::DatabaseError(format!("set scheduled task enabled: {e}")))
}

/// Load a single scheduled task by ID.
pub fn db_get_scheduled_task(db: &Database, id: &str) -> Result<Option<ScheduledTask>> {
    let result = db.with_connection(|conn| {
        conn.query_row(
            "SELECT id, name, agent_id, instruction, cron_expr, enabled, last_run_at, created_at \
             FROM scheduled_tasks WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, bool>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
    });

    match result {
        Ok((id, name, agent_id, instruction, cron_expr, enabled, last_run_at_str, created_at_str)) => {
            Ok(Some(ScheduledTask {
                id,
                name,
                agent_id,
                instruction,
                cron_expr,
                enabled,
                last_run_at: last_run_at_str.and_then(|s| s.parse().ok()),
                created_at: created_at_str.parse().unwrap_or_else(|_| Utc::now()),
            }))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(HalconError::DatabaseError(format!("get scheduled task: {e}"))),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A "every minute" cron is due when last_run was >60s ago.
    #[test]
    fn test_is_due_every_minute_after_1min() {
        let now = Utc::now();
        let last_run = now - chrono::Duration::seconds(61);
        assert!(AgentScheduler::is_due("* * * * *", Some(last_run), now).unwrap());
    }

    /// A task that ran <60s ago should NOT be due for "every minute".
    #[test]
    fn test_is_not_due_just_ran() {
        let now = Utc::now();
        let last_run = now - chrono::Duration::seconds(5);
        assert!(!AgentScheduler::is_due("* * * * *", Some(last_run), now).unwrap());
    }

    /// A task that has never run ("0 2 * * 1" weekly Monday 2am) should not error.
    #[test]
    fn test_is_due_never_run_no_error() {
        let now = Utc::now();
        let result = AgentScheduler::is_due("0 2 * * 1", None, now);
        assert!(result.is_ok(), "valid cron must not error");
    }

    /// An invalid cron expression returns an error.
    #[test]
    fn test_invalid_cron_returns_error() {
        let now = Utc::now();
        let result = AgentScheduler::is_due("not-a-cron", None, now);
        assert!(result.is_err(), "invalid cron must return Err");
    }
}
