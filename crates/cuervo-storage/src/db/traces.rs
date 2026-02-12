use chrono::Utc;
use uuid::Uuid;

use cuervo_core::error::{CuervoError, Result};

use crate::trace::{TraceExport, TraceStep, TraceStepType};

use super::Database;

impl Database {
    /// Append a trace step to the trace log (append-only).
    pub fn append_trace_step(&self, step: &TraceStep) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "INSERT INTO trace_steps (session_id, step_index, step_type, data_json, duration_ms, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                step.session_id.to_string(),
                step.step_index,
                step.step_type.as_str(),
                step.data_json,
                step.duration_ms,
                step.timestamp.to_rfc3339(),
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("append trace step: {e}")))?;

        Ok(())
    }

    /// Load all trace steps for a session, ordered by step_index.
    pub fn load_trace_steps(&self, session_id: Uuid) -> Result<Vec<TraceStep>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT session_id, step_index, step_type, data_json, duration_ms, timestamp
                 FROM trace_steps WHERE session_id = ?1 ORDER BY step_index ASC",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare: {e}")))?;

        let steps = stmt
            .query_map(rusqlite::params![session_id.to_string()], |row| {
                Ok(Self::row_to_trace_step(row))
            })
            .map_err(|e| CuervoError::DatabaseError(format!("load trace steps: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CuervoError::DatabaseError(format!("collect: {e}")))?;

        steps.into_iter().collect()
    }

    /// Export trace steps for a session as a deterministic JSON structure.
    pub fn export_trace(&self, session_id: Uuid) -> Result<TraceExport> {
        let steps = self.load_trace_steps(session_id)?;
        Ok(TraceExport {
            session_id,
            exported_at: Utc::now(),
            step_count: steps.len() as u32,
            steps,
        })
    }

    /// Return the maximum step_index for a session, or None if no steps exist.
    pub fn max_step_index(&self, session_id: Uuid) -> Result<Option<u32>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let idx: Option<u32> = conn
            .query_row(
                "SELECT MAX(step_index) FROM trace_steps WHERE session_id = ?1",
                rusqlite::params![session_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|e| CuervoError::DatabaseError(format!("max_step_index: {e}")))?;

        Ok(idx)
    }

    fn row_to_trace_step(row: &rusqlite::Row) -> Result<TraceStep> {
        let session_id_str: String = row
            .get(0)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let step_index: u32 = row
            .get(1)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let step_type_str: String = row
            .get(2)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let data_json: String = row
            .get(3)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let duration_ms: u64 = row
            .get(4)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let timestamp_str: String = row
            .get(5)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let session_id = Uuid::parse_str(&session_id_str)
            .map_err(|e| CuervoError::DatabaseError(format!("parse uuid: {e}")))?;
        let step_type = TraceStepType::parse(&step_type_str).ok_or_else(|| {
            CuervoError::DatabaseError(format!("unknown step type: {step_type_str}"))
        })?;
        let timestamp = chrono::DateTime::parse_from_rfc3339(&timestamp_str)
            .map_err(|e| CuervoError::DatabaseError(format!("parse date: {e}")))?
            .with_timezone(&Utc);

        Ok(TraceStep {
            session_id,
            step_index,
            step_type,
            data_json,
            duration_ms,
            timestamp,
        })
    }
}
