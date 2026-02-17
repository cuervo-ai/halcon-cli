//! Activity search history persistence functions.
//! Phase 3 SRCH-004: Search history storage for TUI activity zone.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::Database;

/// Search history entry from activity_search_history table.
#[derive(Debug, Clone)]
pub struct ActivitySearchEntry {
    pub id: i64,
    pub query: String,
    pub search_mode: String,
    pub match_count: i32,
    pub searched_at: DateTime<Utc>,
    pub session_id: Option<String>,
}

impl Database {
    /// Save a search query to history.
    ///
    /// ## Arguments
    /// - `query`: The search string
    /// - `search_mode`: "exact", "fuzzy", or "regex"
    /// - `match_count`: Number of matches found
    /// - `session_id`: Optional current session ID
    ///
    /// ## Behavior
    /// - Inserts new entry with current timestamp
    /// - Cleanup trigger auto-deletes old entries (keeps last 1000)
    /// - Returns the ID of the inserted entry
    pub fn save_search_history(
        &self,
        query: &str,
        search_mode: &str,
        match_count: i32,
        session_id: Option<&str>,
    ) -> rusqlite::Result<i64> {
        self.with_connection(|conn| {
            save_search_history_inner(conn, query, search_mode, match_count, session_id)
        })
    }

    /// Load recent search history (descending by time).
    ///
    /// ## Arguments
    /// - `limit`: Maximum number of entries to return (default: 50)
    ///
    /// ## Returns
    /// Vec of ActivitySearchEntry, most recent first.
    pub fn load_search_history(&self, limit: usize) -> rusqlite::Result<Vec<ActivitySearchEntry>> {
        self.with_connection(|conn| load_search_history_inner(conn, limit))
    }

    /// Load search history for a specific session.
    ///
    /// ## Arguments
    /// - `session_id`: Session ID to filter by
    /// - `limit`: Maximum number of entries
    ///
    /// ## Returns
    /// Vec of ActivitySearchEntry for the session, most recent first.
    pub fn load_search_history_for_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<ActivitySearchEntry>> {
        self.with_connection(|conn| load_search_history_for_session_inner(conn, session_id, limit))
    }

    /// Get distinct recent queries (deduped by query text).
    ///
    /// ## Arguments
    /// - `limit`: Maximum number of unique queries
    ///
    /// ## Returns
    /// Vec of query strings, most recently searched first.
    /// Useful for autocomplete / suggestions.
    pub fn get_recent_queries(&self, limit: usize) -> rusqlite::Result<Vec<String>> {
        self.with_connection(|conn| get_recent_queries_inner(conn, limit))
    }

    /// Clear all search history.
    ///
    /// ## Use Case
    /// Privacy / cleanup command (e.g., `/clear history`).
    pub fn clear_search_history(&self) -> rusqlite::Result<()> {
        self.with_connection(|conn| {
            conn.execute("DELETE FROM activity_search_history", [])?;
            Ok(())
        })
    }
}

fn save_search_history_inner(
    conn: &Connection,
    query: &str,
    search_mode: &str,
    match_count: i32,
    session_id: Option<&str>,
) -> rusqlite::Result<i64> {
    let now = Utc::now().to_rfc3339();

    conn.execute(
        r#"
        INSERT INTO activity_search_history (query, search_mode, match_count, searched_at, session_id)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        params![query, search_mode, match_count, now, session_id],
    )?;

    Ok(conn.last_insert_rowid())
}

fn load_search_history_inner(
    conn: &Connection,
    limit: usize,
) -> rusqlite::Result<Vec<ActivitySearchEntry>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, query, search_mode, match_count, searched_at, session_id
        FROM activity_search_history
        ORDER BY searched_at DESC
        LIMIT ?1
        "#,
    )?;

    let rows = stmt.query_map(params![limit as i64], |row| {
        let searched_at_str: String = row.get(4)?;
        let searched_at = DateTime::parse_from_rfc3339(&searched_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        Ok(ActivitySearchEntry {
            id: row.get(0)?,
            query: row.get(1)?,
            search_mode: row.get(2)?,
            match_count: row.get(3)?,
            searched_at,
            session_id: row.get(5)?,
        })
    })?;

    rows.collect()
}

fn load_search_history_for_session_inner(
    conn: &Connection,
    session_id: &str,
    limit: usize,
) -> rusqlite::Result<Vec<ActivitySearchEntry>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, query, search_mode, match_count, searched_at, session_id
        FROM activity_search_history
        WHERE session_id = ?1
        ORDER BY searched_at DESC
        LIMIT ?2
        "#,
    )?;

    let rows = stmt.query_map(params![session_id, limit as i64], |row| {
        let searched_at_str: String = row.get(4)?;
        let searched_at = DateTime::parse_from_rfc3339(&searched_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        Ok(ActivitySearchEntry {
            id: row.get(0)?,
            query: row.get(1)?,
            search_mode: row.get(2)?,
            match_count: row.get(3)?,
            searched_at,
            session_id: row.get(5)?,
        })
    })?;

    rows.collect()
}

fn get_recent_queries_inner(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT query
        FROM activity_search_history
        GROUP BY query
        ORDER BY MAX(searched_at) DESC
        LIMIT ?1
        "#,
    )?;

    let rows = stmt.query_map(params![limit as i64], |row| row.get(0))?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_and_load_search_history() {
        let db = Database::open_in_memory().unwrap();

        // Save 3 search queries
        db.save_search_history("hello", "exact", 5, Some("session-1"))
            .unwrap();
        db.save_search_history("world", "fuzzy", 3, Some("session-1"))
            .unwrap();
        db.save_search_history("test.*", "regex", 10, Some("session-2"))
            .unwrap();

        // Load recent history (limit 10)
        let history = db.load_search_history(10).unwrap();
        assert_eq!(history.len(), 3);

        // Most recent first (test.* is last inserted)
        assert_eq!(history[0].query, "test.*");
        assert_eq!(history[0].search_mode, "regex");
        assert_eq!(history[0].match_count, 10);

        assert_eq!(history[1].query, "world");
        assert_eq!(history[2].query, "hello");
    }

    #[test]
    fn test_load_search_history_with_limit() {
        let db = Database::open_in_memory().unwrap();

        for i in 0..5 {
            db.save_search_history(&format!("query{}", i), "exact", i, None)
                .unwrap();
        }

        let history = db.load_search_history(2).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].query, "query4"); // Most recent
        assert_eq!(history[1].query, "query3");
    }

    #[test]
    fn test_load_search_history_for_session() {
        let db = Database::open_in_memory().unwrap();

        db.save_search_history("q1", "exact", 1, Some("session-A"))
            .unwrap();
        db.save_search_history("q2", "fuzzy", 2, Some("session-B"))
            .unwrap();
        db.save_search_history("q3", "regex", 3, Some("session-A"))
            .unwrap();

        // Load only session-A
        let history = db.load_search_history_for_session("session-A", 10).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].query, "q3"); // Most recent
        assert_eq!(history[1].query, "q1");
    }

    #[test]
    fn test_get_recent_queries_deduped() {
        let db = Database::open_in_memory().unwrap();

        // Insert duplicate queries
        db.save_search_history("hello", "exact", 1, None).unwrap();
        db.save_search_history("world", "exact", 2, None).unwrap();
        db.save_search_history("hello", "fuzzy", 3, None).unwrap(); // Duplicate query

        let queries = db.get_recent_queries(10).unwrap();
        assert_eq!(queries.len(), 2); // GROUP BY deduplicates by query text

        // Most recent usage of each query determines order
        // "hello" was last searched after "world", so it comes first
        assert_eq!(queries[0], "hello");
        assert_eq!(queries[1], "world");
    }

    #[test]
    fn test_clear_search_history() {
        let db = Database::open_in_memory().unwrap();

        db.save_search_history("query1", "exact", 1, None).unwrap();
        db.save_search_history("query2", "fuzzy", 2, None).unwrap();

        let history_before = db.load_search_history(10).unwrap();
        assert_eq!(history_before.len(), 2);

        db.clear_search_history().unwrap();

        let history_after = db.load_search_history(10).unwrap();
        assert_eq!(history_after.len(), 0);
    }

    #[test]
    fn test_cleanup_trigger_limits_to_1000() {
        let db = Database::open_in_memory().unwrap();

        // Insert 1005 entries (trigger should delete oldest 5)
        for i in 0..1005 {
            db.save_search_history(&format!("query{}", i), "exact", 1, None)
                .unwrap();
        }

        let history = db.load_search_history(2000).unwrap();
        assert_eq!(
            history.len(),
            1000,
            "Cleanup trigger should limit to 1000 entries"
        );

        // Verify oldest entries were deleted (query0-query4)
        let all_queries: Vec<String> = history.iter().map(|e| e.query.clone()).collect();
        assert!(!all_queries.contains(&"query0".to_string()));
        assert!(!all_queries.contains(&"query4".to_string()));

        // Verify newest entries remain (query1000-query1004)
        assert!(all_queries.contains(&"query1004".to_string()));
        assert!(all_queries.contains(&"query1000".to_string()));
    }
}
