use chrono::Utc;

use cuervo_core::error::{CuervoError, Result};

use super::Database;

impl Database {
    /// Record a resilience event.
    pub fn insert_resilience_event(
        &self,
        event: &crate::resilience::ResilienceEvent,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "INSERT INTO resilience_events (provider, event_type, from_state, to_state, score, details, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                event.provider,
                event.event_type,
                event.from_state,
                event.to_state,
                event.score.map(|s| s as i32),
                event.details,
                event.created_at.to_rfc3339(),
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("insert resilience event: {e}")))?;

        Ok(())
    }

    /// Query recent resilience events, optionally filtered by provider and/or type.
    pub fn resilience_events(
        &self,
        provider: Option<&str>,
        event_type: Option<&str>,
        limit: u32,
    ) -> Result<Vec<crate::resilience::ResilienceEvent>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut sql = String::from(
            "SELECT provider, event_type, from_state, to_state, score, details, created_at
             FROM resilience_events WHERE 1=1",
        );
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(p) = provider {
            sql.push_str(" AND provider = ?");
            params.push(Box::new(p.to_string()));
        }
        if let Some(t) = event_type {
            sql.push_str(" AND event_type = ?");
            params.push(Box::new(t.to_string()));
        }
        sql.push_str(" ORDER BY created_at DESC LIMIT ?");
        params.push(Box::new(limit));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let events = stmt
            .query_map(param_refs.as_slice(), |row| {
                let created_str: String = row.get(6)?;
                let created_at = chrono::DateTime::parse_from_rfc3339(&created_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                let score: Option<i32> = row.get(4)?;
                Ok(crate::resilience::ResilienceEvent {
                    provider: row.get(0)?,
                    event_type: row.get(1)?,
                    from_state: row.get(2)?,
                    to_state: row.get(3)?,
                    score: score.map(|s| s as u32),
                    details: row.get(5)?,
                    created_at,
                })
            })
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(events)
    }

    /// Delete resilience events older than the given number of days.
    pub fn prune_resilience_events(&self, max_age_days: u32) -> Result<u64> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let cutoff = Utc::now() - chrono::Duration::days(max_age_days as i64);
        let deleted = conn
            .execute(
                "DELETE FROM resilience_events WHERE created_at < ?1",
                rusqlite::params![cutoff.to_rfc3339()],
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prune resilience events: {e}")))?;

        Ok(deleted as u64)
    }
}
