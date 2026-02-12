use chrono::Utc;

use cuervo_core::error::{CuervoError, Result};

use crate::memory::{MemoryEntry, MemoryEpisode};

use super::{Database, OptionalExt};

impl Database {
    /// Save a memory episode.
    pub fn save_episode(&self, episode: &MemoryEpisode) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let metadata_json = serde_json::to_string(&episode.metadata)
            .map_err(|e| CuervoError::DatabaseError(format!("serialize metadata: {e}")))?;

        conn.execute(
            "INSERT OR REPLACE INTO memory_episodes (episode_id, session_id, title, summary, started_at, ended_at, metadata_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                episode.episode_id,
                episode.session_id,
                episode.title,
                episode.summary,
                episode.started_at.to_rfc3339(),
                episode.ended_at.map(|dt| dt.to_rfc3339()),
                metadata_json,
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("save episode: {e}")))?;

        Ok(())
    }

    /// Load a memory episode by episode_id.
    pub fn load_episode(&self, episode_id: &str) -> Result<Option<MemoryEpisode>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT episode_id, session_id, title, summary, started_at, ended_at, metadata_json
                 FROM memory_episodes WHERE episode_id = ?1",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare: {e}")))?;

        let episode = stmt
            .query_row(rusqlite::params![episode_id], |row| {
                Ok(Self::row_to_episode(row))
            })
            .optional()
            .map_err(|e| CuervoError::DatabaseError(format!("load episode: {e}")))?;

        match episode {
            Some(Ok(e)) => Ok(Some(e)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    /// Link a memory entry to an episode by internal IDs.
    pub fn link_entry_to_episode(
        &self,
        entry_uuid: &str,
        episode_id: &str,
        position: u32,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "INSERT OR IGNORE INTO memory_entry_episodes (entry_id, episode_id, position)
             SELECT me.id, ep.id, ?3
             FROM memory_entries me, memory_episodes ep
             WHERE me.entry_id = ?1 AND ep.episode_id = ?2",
            rusqlite::params![entry_uuid, episode_id, position],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("link entry to episode: {e}")))?;

        Ok(())
    }

    /// Load all memory entries belonging to an episode, ordered by position.
    pub fn load_episode_entries(&self, episode_id: &str) -> Result<Vec<MemoryEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT m.entry_id, m.session_id, m.entry_type, m.content, m.content_hash,
                        m.metadata_json, m.created_at, m.expires_at, m.relevance_score
                 FROM memory_entries m
                 JOIN memory_entry_episodes mee ON m.id = mee.entry_id
                 JOIN memory_episodes ep ON ep.id = mee.episode_id
                 WHERE ep.episode_id = ?1
                 ORDER BY mee.position ASC",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare: {e}")))?;

        let entries = stmt
            .query_map(rusqlite::params![episode_id], |row| {
                Ok(Self::row_to_memory_entry(row))
            })
            .map_err(|e| CuervoError::DatabaseError(format!("load episode entries: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CuervoError::DatabaseError(format!("collect: {e}")))?;

        entries.into_iter().collect()
    }

    /// Count total episodes.
    pub fn count_episodes(&self) -> Result<u64> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let count: u64 = conn
            .query_row("SELECT COUNT(*) FROM memory_episodes", [], |row| row.get(0))
            .map_err(|e| CuervoError::DatabaseError(format!("count episodes: {e}")))?;

        Ok(count)
    }

    fn row_to_episode(row: &rusqlite::Row) -> Result<MemoryEpisode> {
        let episode_id: String = row
            .get(0)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let session_id: Option<String> = row
            .get(1)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let title: String = row
            .get(2)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let summary: Option<String> = row
            .get(3)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let started_at_str: String = row
            .get(4)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let ended_at_str: Option<String> = row
            .get(5)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let metadata_json: String = row
            .get(6)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let started_at = chrono::DateTime::parse_from_rfc3339(&started_at_str)
            .map_err(|e| CuervoError::DatabaseError(format!("parse date: {e}")))?
            .with_timezone(&Utc);
        let ended_at = ended_at_str
            .map(|s| chrono::DateTime::parse_from_rfc3339(&s))
            .transpose()
            .map_err(|e| CuervoError::DatabaseError(format!("parse ended_at: {e}")))?
            .map(|dt| dt.with_timezone(&Utc));
        let metadata: serde_json::Value = serde_json::from_str(&metadata_json)
            .map_err(|e| CuervoError::DatabaseError(format!("parse metadata: {e}")))?;

        Ok(MemoryEpisode {
            episode_id,
            session_id,
            title,
            summary,
            started_at,
            ended_at,
            metadata,
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
    fn count_episodes_empty() {
        let db = test_db();
        assert_eq!(db.count_episodes().unwrap(), 0);
    }

    #[test]
    fn count_episodes_after_insert() {
        let db = test_db();
        let episode = MemoryEpisode {
            episode_id: "ep-1".to_string(),
            session_id: None,
            title: "Test Episode".to_string(),
            summary: None,
            started_at: Utc::now(),
            ended_at: None,
            metadata: serde_json::json!({}),
        };
        db.save_episode(&episode).unwrap();
        assert_eq!(db.count_episodes().unwrap(), 1);

        let episode2 = MemoryEpisode {
            episode_id: "ep-2".to_string(),
            ..episode
        };
        db.save_episode(&episode2).unwrap();
        assert_eq!(db.count_episodes().unwrap(), 2);
    }

    #[test]
    fn count_episodes_upsert_no_double_count() {
        let db = test_db();
        let episode = MemoryEpisode {
            episode_id: "ep-1".to_string(),
            session_id: None,
            title: "Original".to_string(),
            summary: None,
            started_at: Utc::now(),
            ended_at: None,
            metadata: serde_json::json!({}),
        };
        db.save_episode(&episode).unwrap();
        // Same ID again (INSERT OR REPLACE).
        let episode2 = MemoryEpisode {
            title: "Updated".to_string(),
            ..episode
        };
        db.save_episode(&episode2).unwrap();
        assert_eq!(db.count_episodes().unwrap(), 1);
    }
}
