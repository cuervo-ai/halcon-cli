use chrono::Utc;
use uuid::Uuid;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::types::{Session, TokenUsage};

use super::Database;

impl Database {
    pub fn save_session(&self, session: &Session) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let messages_json = serde_json::to_string(&session.messages)
            .map_err(|e| CuervoError::DatabaseError(format!("serialize messages: {e}")))?;

        conn.execute(
            "INSERT OR REPLACE INTO sessions (id, title, model, provider, working_directory, messages_json, total_input_tokens, total_output_tokens, created_at, updated_at, tool_invocations, agent_rounds, total_latency_ms, estimated_cost_usd, execution_fingerprint, replay_source_session)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            rusqlite::params![
                session.id.to_string(),
                session.title,
                session.model,
                session.provider,
                session.working_directory,
                messages_json,
                session.total_usage.input_tokens,
                session.total_usage.output_tokens,
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
                session.tool_invocations,
                session.agent_rounds,
                session.total_latency_ms as i64,
                session.estimated_cost_usd,
                session.execution_fingerprint,
                session.replay_source_session,
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("save session: {e}")))?;

        Ok(())
    }

    pub fn load_session(&self, id: Uuid) -> Result<Option<Session>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, model, provider, working_directory, messages_json, total_input_tokens, total_output_tokens, created_at, updated_at, tool_invocations, agent_rounds, total_latency_ms, estimated_cost_usd, execution_fingerprint, replay_source_session
                 FROM sessions WHERE id = ?1",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare: {e}")))?;

        let session = stmt
            .query_row(rusqlite::params![id.to_string()], |row| {
                Ok(Self::row_to_session(row))
            })
            .optional()
            .map_err(|e| CuervoError::DatabaseError(format!("load session: {e}")))?;

        match session {
            Some(Ok(s)) => Ok(Some(s)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    pub fn list_sessions(&self, limit: u32) -> Result<Vec<Session>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, model, provider, working_directory, messages_json, total_input_tokens, total_output_tokens, created_at, updated_at, tool_invocations, agent_rounds, total_latency_ms, estimated_cost_usd, execution_fingerprint, replay_source_session
                 FROM sessions ORDER BY updated_at DESC LIMIT ?1",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare: {e}")))?;

        let sessions = stmt
            .query_map(rusqlite::params![limit], |row| {
                Ok(Self::row_to_session(row))
            })
            .map_err(|e| CuervoError::DatabaseError(format!("list sessions: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CuervoError::DatabaseError(format!("collect: {e}")))?;

        sessions.into_iter().collect()
    }

    pub fn delete_session(&self, id: Uuid) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        conn.execute(
            "DELETE FROM sessions WHERE id = ?1",
            rusqlite::params![id.to_string()],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("delete session: {e}")))?;
        Ok(())
    }

    fn row_to_session(row: &rusqlite::Row) -> Result<Session> {
        let id_str: String = row
            .get(0)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let title: Option<String> = row
            .get(1)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let model: String = row
            .get(2)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let provider: String = row
            .get(3)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let working_directory: String = row
            .get(4)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let messages_json: String = row
            .get(5)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let input_tokens: u32 = row
            .get(6)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let output_tokens: u32 = row
            .get(7)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let created_at_str: String = row
            .get(8)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let updated_at_str: String = row
            .get(9)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let id = Uuid::parse_str(&id_str)
            .map_err(|e| CuervoError::DatabaseError(format!("parse uuid: {e}")))?;
        let messages = serde_json::from_str(&messages_json)
            .map_err(|e| CuervoError::DatabaseError(format!("parse messages: {e}")))?;
        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
            .map_err(|e| CuervoError::DatabaseError(format!("parse date: {e}")))?
            .with_timezone(&Utc);
        let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at_str)
            .map_err(|e| CuervoError::DatabaseError(format!("parse date: {e}")))?
            .with_timezone(&Utc);

        // Read new columns with backward compat defaults for pre-migration rows.
        let tool_invocations: u32 = row.get(10).unwrap_or(0);
        let agent_rounds: u32 = row.get(11).unwrap_or(0);
        let total_latency_ms_i64: i64 = row.get(12).unwrap_or(0);
        let estimated_cost_usd: f64 = row.get(13).unwrap_or(0.0);
        let execution_fingerprint: Option<String> = row.get(14).unwrap_or(None);
        let replay_source_session: Option<String> = row.get(15).unwrap_or(None);

        Ok(Session {
            id,
            title,
            model,
            provider,
            working_directory,
            messages,
            total_usage: TokenUsage {
                input_tokens,
                output_tokens,
                ..Default::default()
            },
            created_at,
            updated_at,
            tool_invocations,
            agent_rounds,
            total_latency_ms: total_latency_ms_i64 as u64,
            estimated_cost_usd,
            execution_fingerprint,
            replay_source_session,
        })
    }
}

use super::OptionalExt;
