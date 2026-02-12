use uuid::Uuid;

use cuervo_core::error::{CuervoError, Result};

use super::Database;

impl Database {
    /// Persist a TBAC policy decision to the audit trail.
    pub fn save_policy_decision(
        &self,
        session_id: &Uuid,
        context_id: &Uuid,
        tool_name: &str,
        decision: &str,
        reason: Option<&str>,
        arguments_hash: Option<&str>,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "INSERT INTO policy_decisions (session_id, context_id, tool_name, decision, reason, arguments_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                session_id.to_string(),
                context_id.to_string(),
                tool_name,
                decision,
                reason,
                arguments_hash,
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("save policy decision: {e}")))?;

        Ok(())
    }
}
