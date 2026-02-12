use chrono::Utc;

use cuervo_core::error::{CuervoError, Result};

use super::{Database, OptionalExt};

impl Database {
    /// Insert or replace a cache entry.
    pub fn insert_cache_entry(&self, entry: &crate::cache::CacheEntry) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "INSERT OR REPLACE INTO response_cache (cache_key, model, response_text, tool_calls_json, stop_reason, usage_json, created_at, expires_at, hit_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                entry.cache_key,
                entry.model,
                entry.response_text,
                entry.tool_calls_json,
                entry.stop_reason,
                entry.usage_json,
                entry.created_at.to_rfc3339(),
                entry.expires_at.map(|dt| dt.to_rfc3339()),
                entry.hit_count,
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("insert cache: {e}")))?;

        Ok(())
    }

    /// Look up a cache entry by key. Increments hit_count on hit.
    /// Returns None if not found or expired (TTL baked into WHERE clause).
    /// Expired entries are cleaned up by `prune_cache()`, not on lookup.
    pub fn lookup_cache(&self, cache_key: &str) -> Result<Option<crate::cache::CacheEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let now = Utc::now().to_rfc3339();

        // Single SELECT with TTL filter — expired entries never returned (no app-level check needed).
        let mut stmt = conn
            .prepare(
                "SELECT cache_key, model, response_text, tool_calls_json, stop_reason, usage_json, created_at, expires_at, hit_count
                 FROM response_cache WHERE cache_key = ?1 AND (expires_at IS NULL OR expires_at >= ?2)",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare cache: {e}")))?;

        let entry = stmt
            .query_row(rusqlite::params![cache_key, now], |row| {
                Ok(Self::row_to_cache_entry(row))
            })
            .optional()
            .map_err(|e| CuervoError::DatabaseError(format!("lookup cache: {e}")))?;

        let entry = match entry {
            Some(Ok(e)) => e,
            Some(Err(e)) => return Err(e),
            None => return Ok(None),
        };

        // Increment hit_count (same as before).
        let _ = conn.execute(
            "UPDATE response_cache SET hit_count = hit_count + 1 WHERE cache_key = ?1",
            rusqlite::params![cache_key],
        );

        Ok(Some(entry))
    }

    /// Delete all cache entries.
    pub fn clear_cache(&self) -> Result<u32> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let removed = conn
            .execute("DELETE FROM response_cache", [])
            .map_err(|e| CuervoError::DatabaseError(format!("clear cache: {e}")))?;

        Ok(removed as u32)
    }

    /// Prune expired cache entries and enforce max_entries limit.
    pub fn prune_cache(&self, max_entries: u32) -> Result<u32> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut removed: u32 = 0;

        // Remove expired entries
        let expired = conn
            .execute(
                "DELETE FROM response_cache WHERE expires_at IS NOT NULL AND expires_at < ?1",
                rusqlite::params![Utc::now().to_rfc3339()],
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prune expired cache: {e}")))?;
        removed += expired as u32;

        // Enforce max_entries
        if max_entries > 0 {
            let total: u32 = conn
                .query_row("SELECT COUNT(*) FROM response_cache", [], |row| row.get(0))
                .map_err(|e| CuervoError::DatabaseError(format!("count cache: {e}")))?;

            if total > max_entries {
                let excess = total - max_entries;
                let pruned = conn
                    .execute(
                        "DELETE FROM response_cache WHERE id IN (
                            SELECT id FROM response_cache
                            ORDER BY hit_count ASC, created_at ASC
                            LIMIT ?1
                        )",
                        rusqlite::params![excess],
                    )
                    .map_err(|e| CuervoError::DatabaseError(format!("prune excess cache: {e}")))?;
                removed += pruned as u32;
            }
        }

        Ok(removed)
    }

    /// Get cache statistics (1 query, down from 4).
    pub fn cache_stats(&self) -> Result<crate::cache::CacheStats> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        // Single query replaces 4 individual queries.
        let (total_entries, total_hits, oldest_str, newest_str): (u32, u64, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(hit_count), 0), MIN(created_at), MAX(created_at) FROM response_cache",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|e| CuervoError::DatabaseError(format!("cache stats: {e}")))?;

        let oldest_entry = oldest_str
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let newest_entry = newest_str
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        Ok(crate::cache::CacheStats {
            total_entries,
            total_hits,
            oldest_entry,
            newest_entry,
        })
    }

    /// Retrieve top cache entries by hit_count (for L1 warming), excluding expired.
    pub fn top_cache_entries(&self, limit: usize) -> Result<Vec<crate::cache::CacheEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT cache_key, model, response_text, tool_calls_json, stop_reason, usage_json, created_at, expires_at, hit_count
                 FROM response_cache
                 WHERE expires_at IS NULL OR expires_at > datetime('now')
                 ORDER BY hit_count DESC
                 LIMIT ?1",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare: {e}")))?;

        let entries = stmt
            .query_map(rusqlite::params![limit as u32], |row| {
                Ok(Self::row_to_cache_entry(row))
            })
            .map_err(|e| CuervoError::DatabaseError(format!("top cache: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CuervoError::DatabaseError(format!("collect: {e}")))?;

        entries.into_iter().collect()
    }

    fn row_to_cache_entry(row: &rusqlite::Row) -> Result<crate::cache::CacheEntry> {
        let cache_key: String = row
            .get(0)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let model: String = row
            .get(1)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let response_text: String = row
            .get(2)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let tool_calls_json: Option<String> = row
            .get(3)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let stop_reason: String = row
            .get(4)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let usage_json: String = row
            .get(5)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let created_at_str: String = row
            .get(6)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let expires_at_str: Option<String> = row
            .get(7)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let hit_count: u32 = row
            .get(8)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
            .map_err(|e| CuervoError::DatabaseError(format!("parse date: {e}")))?
            .with_timezone(&Utc);
        let expires_at = expires_at_str
            .map(|s| chrono::DateTime::parse_from_rfc3339(&s))
            .transpose()
            .map_err(|e| CuervoError::DatabaseError(format!("parse expires: {e}")))?
            .map(|dt| dt.with_timezone(&Utc));

        Ok(crate::cache::CacheEntry {
            cache_key,
            model,
            response_text,
            tool_calls_json,
            stop_reason,
            usage_json,
            created_at,
            expires_at,
            hit_count,
        })
    }
}
