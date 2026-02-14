//! Structured task persistence — CRUD for the structured_tasks table.

use cuervo_core::error::{CuervoError, Result};

use super::Database;

/// Row returned from structured_tasks table queries.
#[derive(Debug, Clone)]
pub struct StructuredTaskRow {
    pub task_id: String,
    pub session_id: Option<String>,
    pub plan_id: Option<String>,
    pub step_index: Option<i64>,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: i64,
    pub depends_on_json: String,
    pub inputs_json: String,
    pub outputs_json: String,
    pub artifacts_json: String,
    pub provenance_json: Option<String>,
    pub retry_policy_json: String,
    pub retry_count: i64,
    pub tags_json: String,
    pub tool_name: Option<String>,
    pub expected_args_json: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
}

impl Database {
    /// Save (INSERT OR REPLACE) a structured task.
    #[allow(clippy::too_many_arguments)]
    pub fn save_structured_task(
        &self,
        task_id: &str,
        session_id: Option<&str>,
        plan_id: Option<&str>,
        step_index: Option<i64>,
        title: &str,
        description: &str,
        status: &str,
        priority: i64,
        depends_on_json: &str,
        inputs_json: &str,
        outputs_json: &str,
        artifacts_json: &str,
        provenance_json: Option<&str>,
        retry_policy_json: &str,
        retry_count: i64,
        tags_json: &str,
        tool_name: Option<&str>,
        expected_args_json: Option<&str>,
        error: Option<&str>,
        created_at: &str,
        started_at: Option<&str>,
        finished_at: Option<&str>,
        duration_ms: Option<i64>,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "INSERT OR REPLACE INTO structured_tasks (
                task_id, session_id, plan_id, step_index, title, description,
                status, priority, depends_on_json, inputs_json, outputs_json,
                artifacts_json, provenance_json, retry_policy_json, retry_count,
                tags_json, tool_name, expected_args_json, error,
                created_at, started_at, finished_at, duration_ms
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23
            )",
            rusqlite::params![
                task_id, session_id, plan_id, step_index, title, description,
                status, priority, depends_on_json, inputs_json, outputs_json,
                artifacts_json, provenance_json, retry_policy_json, retry_count,
                tags_json, tool_name, expected_args_json, error,
                created_at, started_at, finished_at, duration_ms,
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("save structured_task: {e}")))?;

        Ok(())
    }

    /// Update a structured task's status and related fields.
    #[allow(clippy::too_many_arguments)]
    pub fn update_structured_task_status(
        &self,
        task_id: &str,
        status: &str,
        provenance_json: Option<&str>,
        artifacts_json: Option<&str>,
        error: Option<&str>,
        started_at: Option<&str>,
        finished_at: Option<&str>,
        duration_ms: Option<i64>,
        retry_count: Option<i64>,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE structured_tasks SET
                status = ?2,
                provenance_json = COALESCE(?3, provenance_json),
                artifacts_json = COALESCE(?4, artifacts_json),
                error = COALESCE(?5, error),
                started_at = COALESCE(?6, started_at),
                finished_at = COALESCE(?7, finished_at),
                duration_ms = COALESCE(?8, duration_ms),
                retry_count = COALESCE(?9, retry_count)
            WHERE task_id = ?1",
            rusqlite::params![
                task_id, status, provenance_json, artifacts_json, error,
                started_at, finished_at, duration_ms, retry_count,
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("update structured_task status: {e}")))?;

        Ok(())
    }

    /// Load all structured tasks for a session.
    pub fn load_structured_tasks_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<StructuredTaskRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT task_id, session_id, plan_id, step_index, title, description,
                        status, priority, depends_on_json, inputs_json, outputs_json,
                        artifacts_json, provenance_json, retry_policy_json, retry_count,
                        tags_json, tool_name, expected_args_json, error,
                        created_at, started_at, finished_at, duration_ms
                 FROM structured_tasks WHERE session_id = ?1 ORDER BY id",
            )
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map([session_id], |row| {
                Ok(StructuredTaskRow {
                    task_id: row.get(0)?,
                    session_id: row.get(1)?,
                    plan_id: row.get(2)?,
                    step_index: row.get(3)?,
                    title: row.get(4)?,
                    description: row.get(5)?,
                    status: row.get(6)?,
                    priority: row.get(7)?,
                    depends_on_json: row.get(8)?,
                    inputs_json: row.get(9)?,
                    outputs_json: row.get(10)?,
                    artifacts_json: row.get(11)?,
                    provenance_json: row.get(12)?,
                    retry_policy_json: row.get(13)?,
                    retry_count: row.get(14)?,
                    tags_json: row.get(15)?,
                    tool_name: row.get(16)?,
                    expected_args_json: row.get(17)?,
                    error: row.get(18)?,
                    created_at: row.get(19)?,
                    started_at: row.get(20)?,
                    finished_at: row.get(21)?,
                    duration_ms: row.get(22)?,
                })
            })
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CuervoError::DatabaseError(e.to_string()))?);
        }
        Ok(result)
    }

    /// Load all structured tasks for a plan.
    pub fn load_structured_tasks_by_plan(
        &self,
        plan_id: &str,
    ) -> Result<Vec<StructuredTaskRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT task_id, session_id, plan_id, step_index, title, description,
                        status, priority, depends_on_json, inputs_json, outputs_json,
                        artifacts_json, provenance_json, retry_policy_json, retry_count,
                        tags_json, tool_name, expected_args_json, error,
                        created_at, started_at, finished_at, duration_ms
                 FROM structured_tasks WHERE plan_id = ?1 ORDER BY step_index",
            )
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map([plan_id], |row| {
                Ok(StructuredTaskRow {
                    task_id: row.get(0)?,
                    session_id: row.get(1)?,
                    plan_id: row.get(2)?,
                    step_index: row.get(3)?,
                    title: row.get(4)?,
                    description: row.get(5)?,
                    status: row.get(6)?,
                    priority: row.get(7)?,
                    depends_on_json: row.get(8)?,
                    inputs_json: row.get(9)?,
                    outputs_json: row.get(10)?,
                    artifacts_json: row.get(11)?,
                    provenance_json: row.get(12)?,
                    retry_policy_json: row.get(13)?,
                    retry_count: row.get(14)?,
                    tags_json: row.get(15)?,
                    tool_name: row.get(16)?,
                    expected_args_json: row.get(17)?,
                    error: row.get(18)?,
                    created_at: row.get(19)?,
                    started_at: row.get(20)?,
                    finished_at: row.get(21)?,
                    duration_ms: row.get(22)?,
                })
            })
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CuervoError::DatabaseError(e.to_string()))?);
        }
        Ok(result)
    }

    /// Load a single structured task by task_id.
    pub fn load_structured_task(&self, task_id: &str) -> Result<Option<StructuredTaskRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let result = conn
            .query_row(
                "SELECT task_id, session_id, plan_id, step_index, title, description,
                        status, priority, depends_on_json, inputs_json, outputs_json,
                        artifacts_json, provenance_json, retry_policy_json, retry_count,
                        tags_json, tool_name, expected_args_json, error,
                        created_at, started_at, finished_at, duration_ms
                 FROM structured_tasks WHERE task_id = ?1",
                [task_id],
                |row| {
                    Ok(StructuredTaskRow {
                        task_id: row.get(0)?,
                        session_id: row.get(1)?,
                        plan_id: row.get(2)?,
                        step_index: row.get(3)?,
                        title: row.get(4)?,
                        description: row.get(5)?,
                        status: row.get(6)?,
                        priority: row.get(7)?,
                        depends_on_json: row.get(8)?,
                        inputs_json: row.get(9)?,
                        outputs_json: row.get(10)?,
                        artifacts_json: row.get(11)?,
                        provenance_json: row.get(12)?,
                        retry_policy_json: row.get(13)?,
                        retry_count: row.get(14)?,
                        tags_json: row.get(15)?,
                        tool_name: row.get(16)?,
                        expected_args_json: row.get(17)?,
                        error: row.get(18)?,
                        created_at: row.get(19)?,
                        started_at: row.get(20)?,
                        finished_at: row.get(21)?,
                        duration_ms: row.get(22)?,
                    })
                },
            );

        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CuervoError::DatabaseError(format!(
                "load structured_task: {e}"
            ))),
        }
    }

    /// Load all non-terminal structured tasks (for resume).
    pub fn load_incomplete_structured_tasks(&self) -> Result<Vec<StructuredTaskRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT task_id, session_id, plan_id, step_index, title, description,
                        status, priority, depends_on_json, inputs_json, outputs_json,
                        artifacts_json, provenance_json, retry_policy_json, retry_count,
                        tags_json, tool_name, expected_args_json, error,
                        created_at, started_at, finished_at, duration_ms
                 FROM structured_tasks
                 WHERE status NOT IN ('Completed', 'Failed', 'Skipped', 'Cancelled')
                 ORDER BY id",
            )
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(StructuredTaskRow {
                    task_id: row.get(0)?,
                    session_id: row.get(1)?,
                    plan_id: row.get(2)?,
                    step_index: row.get(3)?,
                    title: row.get(4)?,
                    description: row.get(5)?,
                    status: row.get(6)?,
                    priority: row.get(7)?,
                    depends_on_json: row.get(8)?,
                    inputs_json: row.get(9)?,
                    outputs_json: row.get(10)?,
                    artifacts_json: row.get(11)?,
                    provenance_json: row.get(12)?,
                    retry_policy_json: row.get(13)?,
                    retry_count: row.get(14)?,
                    tags_json: row.get(15)?,
                    tool_name: row.get(16)?,
                    expected_args_json: row.get(17)?,
                    error: row.get(18)?,
                    created_at: row.get(19)?,
                    started_at: row.get(20)?,
                    finished_at: row.get(21)?,
                    duration_ms: row.get(22)?,
                })
            })
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CuervoError::DatabaseError(e.to_string()))?);
        }
        Ok(result)
    }

    /// Delete all structured tasks for a session.
    pub fn delete_structured_tasks_by_session(&self, session_id: &str) -> Result<u64> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let count = conn
            .execute(
                "DELETE FROM structured_tasks WHERE session_id = ?1",
                [session_id],
            )
            .map_err(|e| CuervoError::DatabaseError(format!("delete structured_tasks: {e}")))?;

        Ok(count as u64)
    }
}

#[cfg(test)]
mod tests {
    use crate::Database;

    #[test]
    fn save_and_load_roundtrip() {
        let db = Database::open_in_memory().unwrap();

        db.save_structured_task(
            "task-1", Some("sess-1"), Some("plan-1"), Some(0),
            "Read file", "Read the config file",
            "Running", 10,
            "[]", "{}", "{}",
            "[]", None,
            r#"{"max_retries":2,"base_delay_ms":500,"max_delay_ms":30000,"backoff_multiplier":2.0,"idempotent":true}"#,
            0, "[]",
            Some("file_read"), None, None,
            "2026-01-01T00:00:00Z", Some("2026-01-01T00:00:01Z"), None, None,
        )
        .unwrap();

        let row = db.load_structured_task("task-1").unwrap().unwrap();
        assert_eq!(row.title, "Read file");
        assert_eq!(row.status, "Running");
        assert_eq!(row.priority, 10);
        assert_eq!(row.session_id.as_deref(), Some("sess-1"));
        assert_eq!(row.plan_id.as_deref(), Some("plan-1"));
        assert_eq!(row.tool_name.as_deref(), Some("file_read"));
    }

    #[test]
    fn update_status_persists() {
        let db = Database::open_in_memory().unwrap();

        db.save_structured_task(
            "task-2", Some("sess-1"), None, None,
            "Bash cmd", "Run a command",
            "Running", 5,
            "[]", "{}", "{}",
            "[]", None, r#"{"max_retries":0,"base_delay_ms":500,"max_delay_ms":30000,"backoff_multiplier":2.0,"idempotent":true}"#,
            0, "[]",
            Some("bash"), None, None,
            "2026-01-01T00:00:00Z", None, None, None,
        )
        .unwrap();

        db.update_structured_task_status(
            "task-2", "Completed",
            Some(r#"{"model":"gpt-4o","provider":"openai","tools_used":["bash"],"input_tokens":100,"output_tokens":50,"cost_usd":0.01,"context_hash":null,"parent_task_id":null,"delegated_to":null,"session_id":null,"round":1}"#),
            None, None,
            None, Some("2026-01-01T00:00:05Z"), Some(5000), None,
        )
        .unwrap();

        let row = db.load_structured_task("task-2").unwrap().unwrap();
        assert_eq!(row.status, "Completed");
        assert!(row.provenance_json.is_some());
        assert_eq!(row.finished_at.as_deref(), Some("2026-01-01T00:00:05Z"));
        assert_eq!(row.duration_ms, Some(5000));
    }

    #[test]
    fn load_by_session_filters() {
        let db = Database::open_in_memory().unwrap();

        for (id, session) in [("t1", "s1"), ("t2", "s1"), ("t3", "s2")] {
            db.save_structured_task(
                id, Some(session), None, None,
                "T", "Desc", "Pending", 0,
                "[]", "{}", "{}", "[]", None,
                r#"{"max_retries":0,"base_delay_ms":500,"max_delay_ms":30000,"backoff_multiplier":2.0,"idempotent":true}"#,
                0, "[]", None, None, None,
                "2026-01-01T00:00:00Z", None, None, None,
            )
            .unwrap();
        }

        let s1_tasks = db.load_structured_tasks_by_session("s1").unwrap();
        assert_eq!(s1_tasks.len(), 2);

        let s2_tasks = db.load_structured_tasks_by_session("s2").unwrap();
        assert_eq!(s2_tasks.len(), 1);
    }

    #[test]
    fn load_by_plan_filters() {
        let db = Database::open_in_memory().unwrap();

        for (id, plan, idx) in [("t1", "p1", 0), ("t2", "p1", 1), ("t3", "p2", 0)] {
            db.save_structured_task(
                id, None, Some(plan), Some(idx),
                "T", "Desc", "Ready", 0,
                "[]", "{}", "{}", "[]", None,
                r#"{"max_retries":0,"base_delay_ms":500,"max_delay_ms":30000,"backoff_multiplier":2.0,"idempotent":true}"#,
                0, "[]", None, None, None,
                "2026-01-01T00:00:00Z", None, None, None,
            )
            .unwrap();
        }

        let p1_tasks = db.load_structured_tasks_by_plan("p1").unwrap();
        assert_eq!(p1_tasks.len(), 2);
        // Ordered by step_index.
        assert_eq!(p1_tasks[0].step_index, Some(0));
        assert_eq!(p1_tasks[1].step_index, Some(1));
    }

    #[test]
    fn load_incomplete_returns_non_terminal() {
        let db = Database::open_in_memory().unwrap();

        for (id, status) in [("t1", "Pending"), ("t2", "Running"), ("t3", "Completed"), ("t4", "Failed")] {
            db.save_structured_task(
                id, None, None, None,
                "T", "Desc", status, 0,
                "[]", "{}", "{}", "[]", None,
                r#"{"max_retries":0,"base_delay_ms":500,"max_delay_ms":30000,"backoff_multiplier":2.0,"idempotent":true}"#,
                0, "[]", None, None, None,
                "2026-01-01T00:00:00Z", None, None, None,
            )
            .unwrap();
        }

        let incomplete = db.load_incomplete_structured_tasks().unwrap();
        assert_eq!(incomplete.len(), 2);
        let statuses: Vec<&str> = incomplete.iter().map(|r| r.status.as_str()).collect();
        assert!(statuses.contains(&"Pending"));
        assert!(statuses.contains(&"Running"));
    }

    #[test]
    fn delete_by_session() {
        let db = Database::open_in_memory().unwrap();

        for (id, session) in [("t1", "s1"), ("t2", "s1"), ("t3", "s2")] {
            db.save_structured_task(
                id, Some(session), None, None,
                "T", "Desc", "Pending", 0,
                "[]", "{}", "{}", "[]", None,
                r#"{"max_retries":0,"base_delay_ms":500,"max_delay_ms":30000,"backoff_multiplier":2.0,"idempotent":true}"#,
                0, "[]", None, None, None,
                "2026-01-01T00:00:00Z", None, None, None,
            )
            .unwrap();
        }

        let deleted = db.delete_structured_tasks_by_session("s1").unwrap();
        assert_eq!(deleted, 2);

        let remaining = db.load_structured_tasks_by_session("s1").unwrap();
        assert!(remaining.is_empty());

        let s2 = db.load_structured_tasks_by_session("s2").unwrap();
        assert_eq!(s2.len(), 1);
    }

    #[test]
    fn load_unknown_returns_none() {
        let db = Database::open_in_memory().unwrap();
        let result = db.load_structured_task("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn empty_session_returns_empty() {
        let db = Database::open_in_memory().unwrap();
        let result = db.load_structured_tasks_by_session("no-such-session").unwrap();
        assert!(result.is_empty());
    }
}
