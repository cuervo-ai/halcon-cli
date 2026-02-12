//! Session checkpoint CRUD for resume-from-checkpoint replay.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use cuervo_core::error::{CuervoError, Result};

use super::{Database, OptionalExt};

/// A session checkpoint captured at a round boundary.
#[derive(Debug, Clone)]
pub struct SessionCheckpoint {
    pub session_id: Uuid,
    pub round: u32,
    pub step_index: u32,
    pub messages_json: String,
    pub usage_json: String,
    pub fingerprint: String,
    pub created_at: DateTime<Utc>,
    /// Agent state at this checkpoint (Phase 14.1).
    pub agent_state: Option<String>,
}

impl Database {
    /// Save a session checkpoint (INSERT OR REPLACE on session_id+round).
    pub fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> Result<()> {
        let conn = self.conn.lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "INSERT OR REPLACE INTO session_checkpoints (session_id, round, step_index, messages_json, usage_json, fingerprint, created_at, agent_state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                checkpoint.session_id.to_string(),
                checkpoint.round,
                checkpoint.step_index,
                checkpoint.messages_json,
                checkpoint.usage_json,
                checkpoint.fingerprint,
                checkpoint.created_at.to_rfc3339(),
                checkpoint.agent_state,
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("save checkpoint: {e}")))?;

        Ok(())
    }

    /// Load a specific checkpoint by session_id and round.
    pub fn load_checkpoint(&self, session_id: Uuid, round: u32) -> Result<Option<SessionCheckpoint>> {
        let conn = self.conn.lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let result = conn.query_row(
            "SELECT session_id, round, step_index, messages_json, usage_json, fingerprint, created_at
             FROM session_checkpoints WHERE session_id = ?1 AND round = ?2",
            rusqlite::params![session_id.to_string(), round],
            |row| Ok(Self::row_to_checkpoint(row)),
        )
        .optional()
        .map_err(|e| CuervoError::DatabaseError(format!("load checkpoint: {e}")))?;

        match result {
            Some(Ok(cp)) => Ok(Some(cp)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    /// Load the latest checkpoint for a session (highest round number).
    pub fn load_latest_checkpoint(&self, session_id: Uuid) -> Result<Option<SessionCheckpoint>> {
        let conn = self.conn.lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let result = conn.query_row(
            "SELECT session_id, round, step_index, messages_json, usage_json, fingerprint, created_at
             FROM session_checkpoints WHERE session_id = ?1 ORDER BY round DESC LIMIT 1",
            rusqlite::params![session_id.to_string()],
            |row| Ok(Self::row_to_checkpoint(row)),
        )
        .optional()
        .map_err(|e| CuervoError::DatabaseError(format!("load latest checkpoint: {e}")))?;

        match result {
            Some(Ok(cp)) => Ok(Some(cp)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    /// List checkpoint (round, created_at) pairs for a session, ordered by round ascending.
    pub fn list_checkpoints(&self, session_id: Uuid) -> Result<Vec<(u32, String)>> {
        let conn = self.conn.lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn.prepare(
            "SELECT round, created_at FROM session_checkpoints WHERE session_id = ?1 ORDER BY round ASC",
        )
        .map_err(|e| CuervoError::DatabaseError(format!("prepare: {e}")))?;

        let rows = stmt.query_map(rusqlite::params![session_id.to_string()], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| CuervoError::DatabaseError(format!("list checkpoints: {e}")))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| CuervoError::DatabaseError(format!("collect: {e}")))?;

        Ok(rows)
    }

    fn row_to_checkpoint(row: &rusqlite::Row) -> Result<SessionCheckpoint> {
        let session_id_str: String = row.get(0)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let round: u32 = row.get(1)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let step_index: u32 = row.get(2)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let messages_json: String = row.get(3)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let usage_json: String = row.get(4)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let fingerprint: String = row.get(5)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let created_at_str: String = row.get(6)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let session_id = Uuid::parse_str(&session_id_str)
            .map_err(|e| CuervoError::DatabaseError(format!("parse uuid: {e}")))?;
        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
            .map_err(|e| CuervoError::DatabaseError(format!("parse date: {e}")))?
            .with_timezone(&Utc);

        let agent_state: Option<String> = row.get(7).unwrap_or(None);

        Ok(SessionCheckpoint {
            session_id,
            round,
            step_index,
            messages_json,
            usage_json,
            fingerprint,
            created_at,
            agent_state,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    #[test]
    fn save_and_load_checkpoint() {
        let db = test_db();
        let session_id = Uuid::new_v4();

        let cp = SessionCheckpoint {
            session_id,
            round: 0,
            step_index: 5,
            messages_json: r#"[{"role":"user","content":"hello"}]"#.to_string(),
            usage_json: r#"{"input_tokens":10,"output_tokens":5}"#.to_string(),
            fingerprint: "abc123".to_string(),
            created_at: Utc::now(),
            agent_state: None,
        };
        db.save_checkpoint(&cp).unwrap();

        let loaded = db.load_checkpoint(session_id, 0).unwrap().unwrap();
        assert_eq!(loaded.session_id, session_id);
        assert_eq!(loaded.round, 0);
        assert_eq!(loaded.step_index, 5);
        assert_eq!(loaded.fingerprint, "abc123");
        assert!(loaded.messages_json.contains("hello"));
    }

    #[test]
    fn load_latest_checkpoint() {
        let db = test_db();
        let session_id = Uuid::new_v4();

        for round in 0..3 {
            let cp = SessionCheckpoint {
                session_id,
                round,
                step_index: round * 10,
                messages_json: "[]".to_string(),
                usage_json: "{}".to_string(),
                fingerprint: format!("fp_{round}"),
                created_at: Utc::now(),
                agent_state: None,
            };
            db.save_checkpoint(&cp).unwrap();
        }

        let latest = db.load_latest_checkpoint(session_id).unwrap().unwrap();
        assert_eq!(latest.round, 2);
        assert_eq!(latest.fingerprint, "fp_2");
    }

    #[test]
    fn load_checkpoint_not_found() {
        let db = test_db();
        let result = db.load_checkpoint(Uuid::new_v4(), 0).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn checkpoint_overwrites_same_round() {
        let db = test_db();
        let session_id = Uuid::new_v4();

        let cp1 = SessionCheckpoint {
            session_id,
            round: 0,
            step_index: 5,
            messages_json: "[]".to_string(),
            usage_json: "{}".to_string(),
            fingerprint: "first".to_string(),
            created_at: Utc::now(),
            agent_state: None,
        };
        db.save_checkpoint(&cp1).unwrap();

        let cp2 = SessionCheckpoint {
            session_id,
            round: 0,
            step_index: 10,
            messages_json: "[1]".to_string(),
            usage_json: r#"{"x":1}"#.to_string(),
            fingerprint: "second".to_string(),
            created_at: Utc::now(),
            agent_state: Some("executing".to_string()),
        };
        db.save_checkpoint(&cp2).unwrap();

        let loaded = db.load_checkpoint(session_id, 0).unwrap().unwrap();
        assert_eq!(loaded.fingerprint, "second");
        assert_eq!(loaded.step_index, 10);
    }

    #[test]
    fn list_checkpoints_ordered() {
        let db = test_db();
        let session_id = Uuid::new_v4();

        for round in [2u32, 0, 1] {
            let cp = SessionCheckpoint {
                session_id,
                round,
                step_index: round,
                messages_json: "[]".to_string(),
                usage_json: "{}".to_string(),
                fingerprint: format!("fp_{round}"),
                created_at: Utc::now(),
                agent_state: None,
            };
            db.save_checkpoint(&cp).unwrap();
        }

        let list = db.list_checkpoints(session_id).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].0, 0);
        assert_eq!(list[1].0, 1);
        assert_eq!(list[2].0, 2);
    }

    #[test]
    fn load_latest_checkpoint_none() {
        let db = test_db();
        let result = db.load_latest_checkpoint(Uuid::new_v4()).unwrap();
        assert!(result.is_none());
    }
}
