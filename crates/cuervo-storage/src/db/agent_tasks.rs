use chrono::Utc;

use cuervo_core::error::{CuervoError, Result};

use super::Database;

/// Row struct for agent task records.
#[derive(Debug, Clone)]
pub struct AgentTaskRow {
    pub task_id: String,
    pub orchestrator_id: String,
    pub session_id: String,
    pub agent_type: String,
    pub instruction: String,
    pub status: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub latency_ms: u64,
    pub rounds: u32,
    pub error_message: Option<String>,
    pub output_text: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

impl Database {
    /// Save a new agent task record.
    #[allow(clippy::too_many_arguments)]
    pub fn save_agent_task(
        &self,
        task_id: &str,
        orchestrator_id: &str,
        session_id: &str,
        agent_type: &str,
        instruction: &str,
        status: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
        latency_ms: u64,
        rounds: u32,
        error_message: Option<&str>,
        output_text: Option<&str>,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "INSERT OR REPLACE INTO agent_tasks
             (task_id, orchestrator_id, session_id, agent_type, instruction, status,
              input_tokens, output_tokens, cost_usd, latency_ms, rounds,
              error_message, output_text, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![
                task_id,
                orchestrator_id,
                session_id,
                agent_type,
                instruction,
                status,
                input_tokens as i64,
                output_tokens as i64,
                cost_usd,
                latency_ms as i64,
                rounds as i64,
                error_message,
                output_text,
                Utc::now().to_rfc3339(),
                Option::<String>::None,
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("save agent task: {e}")))?;

        Ok(())
    }

    /// Load all agent tasks for a given orchestrator run.
    pub fn load_agent_tasks(&self, orchestrator_id: &str) -> Result<Vec<AgentTaskRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT task_id, orchestrator_id, session_id, agent_type, instruction,
                        status, input_tokens, output_tokens, cost_usd, latency_ms,
                        rounds, error_message, output_text, created_at, completed_at
                 FROM agent_tasks WHERE orchestrator_id = ?1 ORDER BY id ASC",
            )
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![orchestrator_id], |row| {
                Ok(AgentTaskRow {
                    task_id: row.get(0)?,
                    orchestrator_id: row.get(1)?,
                    session_id: row.get(2)?,
                    agent_type: row.get(3)?,
                    instruction: row.get(4)?,
                    status: row.get(5)?,
                    input_tokens: row.get::<_, i64>(6)? as u64,
                    output_tokens: row.get::<_, i64>(7)? as u64,
                    cost_usd: row.get(8)?,
                    latency_ms: row.get::<_, i64>(9)? as u64,
                    rounds: row.get::<_, i64>(10)? as u32,
                    error_message: row.get(11)?,
                    output_text: row.get(12)?,
                    created_at: row.get(13)?,
                    completed_at: row.get(14)?,
                })
            })
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(rows)
    }

    /// Update an agent task's status and metrics after completion.
    #[allow(clippy::too_many_arguments)]
    pub fn update_agent_task_status(
        &self,
        task_id: &str,
        status: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
        latency_ms: u64,
        rounds: u32,
        error_message: Option<&str>,
        output_text: Option<&str>,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE agent_tasks SET status = ?1, input_tokens = ?2, output_tokens = ?3,
             cost_usd = ?4, latency_ms = ?5, rounds = ?6, error_message = ?7,
             output_text = ?8, completed_at = ?9
             WHERE task_id = ?10",
            rusqlite::params![
                status,
                input_tokens as i64,
                output_tokens as i64,
                cost_usd,
                latency_ms as i64,
                rounds as i64,
                error_message,
                output_text,
                Utc::now().to_rfc3339(),
                task_id,
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("update agent task: {e}")))?;

        Ok(())
    }

    /// Count recent orchestrator runs (for doctor command).
    pub fn count_recent_orchestrator_runs(&self, days: u32) -> Result<u64> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        let count: u64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT orchestrator_id) FROM agent_tasks WHERE created_at >= ?1",
                rusqlite::params![cutoff.to_rfc3339()],
                |row| row.get(0),
            )
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        Ok(count)
    }
}
