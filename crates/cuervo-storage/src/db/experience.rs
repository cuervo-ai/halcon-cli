//! Reasoning experience persistence — cross-session strategy learning.

use cuervo_core::error::{CuervoError, Result};

use super::Database;

/// A row from the reasoning_experience table.
#[derive(Debug, Clone)]
pub struct ExperienceRow {
    pub task_type: String,
    pub strategy: String,
    pub avg_score: f64,
    pub uses: u32,
    pub last_score: f64,
    pub last_task_hash: Option<String>,
    pub updated_at: String,
}

impl Database {
    /// Save or update an experience record (INSERT OR REPLACE).
    pub fn save_experience(
        &self,
        task_type: &str,
        strategy: &str,
        avg_score: f64,
        uses: u32,
        last_score: f64,
        last_task_hash: Option<&str>,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "INSERT OR REPLACE INTO reasoning_experience
             (task_type, strategy, avg_score, uses, last_score, last_task_hash, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                task_type,
                strategy,
                avg_score,
                uses,
                last_score,
                last_task_hash,
                chrono::Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("save_experience: {e}")))?;

        Ok(())
    }

    /// Load experience records for a specific task type.
    pub fn load_experience_by_type(&self, task_type: &str) -> Result<Vec<ExperienceRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT task_type, strategy, avg_score, uses, last_score, last_task_hash, updated_at
                 FROM reasoning_experience WHERE task_type = ?1",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![task_type], |row| {
                Ok(ExperienceRow {
                    task_type: row.get(0)?,
                    strategy: row.get(1)?,
                    avg_score: row.get(2)?,
                    uses: row.get(3)?,
                    last_score: row.get(4)?,
                    last_task_hash: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })
            .map_err(|e| CuervoError::DatabaseError(format!("query: {e}")))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CuervoError::DatabaseError(format!("row: {e}")))?);
        }
        Ok(result)
    }

    /// Load all experience records.
    pub fn load_all_experience(&self) -> Result<Vec<ExperienceRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT task_type, strategy, avg_score, uses, last_score, last_task_hash, updated_at
                 FROM reasoning_experience ORDER BY task_type, strategy",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(ExperienceRow {
                    task_type: row.get(0)?,
                    strategy: row.get(1)?,
                    avg_score: row.get(2)?,
                    uses: row.get(3)?,
                    last_score: row.get(4)?,
                    last_task_hash: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })
            .map_err(|e| CuervoError::DatabaseError(format!("query: {e}")))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CuervoError::DatabaseError(format!("row: {e}")))?);
        }
        Ok(result)
    }

    /// Delete all experience records (for reset).
    pub fn delete_all_experience(&self) -> Result<u64> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let count = conn
            .execute("DELETE FROM reasoning_experience", [])
            .map_err(|e| CuervoError::DatabaseError(format!("delete: {e}")))?;

        Ok(count as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    #[test]
    fn save_and_load_roundtrip() {
        let db = test_db();
        db.save_experience("CodeModification", "DirectExecution", 0.75, 5, 0.8, Some("hash1"))
            .unwrap();

        let records = db.load_experience_by_type("CodeModification").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].strategy, "DirectExecution");
        assert!((records[0].avg_score - 0.75).abs() < f64::EPSILON);
        assert_eq!(records[0].uses, 5);
        assert!((records[0].last_score - 0.8).abs() < f64::EPSILON);
        assert_eq!(records[0].last_task_hash.as_deref(), Some("hash1"));
    }

    #[test]
    fn unique_constraint_updates_in_place() {
        let db = test_db();
        db.save_experience("General", "DirectExecution", 0.5, 1, 0.5, None)
            .unwrap();
        db.save_experience("General", "DirectExecution", 0.9, 10, 0.95, Some("hash2"))
            .unwrap();

        let records = db.load_experience_by_type("General").unwrap();
        assert_eq!(records.len(), 1);
        assert!((records[0].avg_score - 0.9).abs() < f64::EPSILON);
        assert_eq!(records[0].uses, 10);
    }

    #[test]
    fn load_by_type_filters_correctly() {
        let db = test_db();
        db.save_experience("General", "DirectExecution", 0.5, 1, 0.5, None)
            .unwrap();
        db.save_experience("CodeModification", "PlanExecuteReflect", 0.8, 3, 0.8, None)
            .unwrap();

        let general = db.load_experience_by_type("General").unwrap();
        assert_eq!(general.len(), 1);
        assert_eq!(general[0].task_type, "General");

        let code_mod = db.load_experience_by_type("CodeModification").unwrap();
        assert_eq!(code_mod.len(), 1);
        assert_eq!(code_mod[0].task_type, "CodeModification");
    }

    #[test]
    fn load_all_returns_all() {
        let db = test_db();
        db.save_experience("General", "DirectExecution", 0.5, 1, 0.5, None)
            .unwrap();
        db.save_experience("CodeModification", "PlanExecuteReflect", 0.8, 3, 0.8, None)
            .unwrap();
        db.save_experience("Debugging", "DirectExecution", 0.6, 2, 0.6, None)
            .unwrap();

        let all = db.load_all_experience().unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn delete_clears_table() {
        let db = test_db();
        db.save_experience("General", "DirectExecution", 0.5, 1, 0.5, None)
            .unwrap();
        db.save_experience("CodeModification", "PlanExecuteReflect", 0.8, 3, 0.8, None)
            .unwrap();

        let deleted = db.delete_all_experience().unwrap();
        assert_eq!(deleted, 2);

        let all = db.load_all_experience().unwrap();
        assert!(all.is_empty());
    }

    #[test]
    fn empty_results_for_unknown_type() {
        let db = test_db();
        let records = db.load_experience_by_type("NonExistent").unwrap();
        assert!(records.is_empty());
    }
}
