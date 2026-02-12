use chrono::Utc;
use uuid::Uuid;

use cuervo_core::error::{CuervoError, Result};

use crate::memory::{MemoryEntry, MemoryEntryType, MemoryStats};

use super::{Database, OptionalExt};

impl Database {
    /// Insert a memory entry. Returns Ok(false) if duplicate content_hash exists.
    pub fn insert_memory(&self, entry: &MemoryEntry) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        // Check for duplicate content hash
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM memory_entries WHERE content_hash = ?1",
                rusqlite::params![entry.content_hash],
                |row| row.get(0),
            )
            .map_err(|e| CuervoError::DatabaseError(format!("check hash: {e}")))?;

        if exists {
            return Ok(false);
        }

        let metadata_json = serde_json::to_string(&entry.metadata)
            .map_err(|e| CuervoError::DatabaseError(format!("serialize metadata: {e}")))?;

        conn.execute(
            "INSERT INTO memory_entries (entry_id, session_id, entry_type, content, content_hash, metadata_json, created_at, expires_at, relevance_score)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                entry.entry_id.to_string(),
                entry.session_id.map(|id| id.to_string()),
                entry.entry_type.as_str(),
                entry.content,
                entry.content_hash,
                metadata_json,
                entry.created_at.to_rfc3339(),
                entry.expires_at.map(|dt| dt.to_rfc3339()),
                entry.relevance_score,
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("insert memory: {e}")))?;

        Ok(true)
    }

    /// Load a single memory entry by entry_id.
    pub fn load_memory(&self, entry_id: Uuid) -> Result<Option<MemoryEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT entry_id, session_id, entry_type, content, content_hash, metadata_json, created_at, expires_at, relevance_score
                 FROM memory_entries WHERE entry_id = ?1",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare: {e}")))?;

        let entry = stmt
            .query_row(rusqlite::params![entry_id.to_string()], |row| {
                Ok(Self::row_to_memory_entry(row))
            })
            .optional()
            .map_err(|e| CuervoError::DatabaseError(format!("load memory: {e}")))?;

        match entry {
            Some(Ok(e)) => Ok(Some(e)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    /// Full-text search over memory entries using BM25 ranking.
    ///
    /// Sanitizes the query to prevent FTS5 syntax errors from special characters
    /// like `.`, `?`, `-`, `()` in user input.
    pub fn search_memory_fts(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let sanitized = sanitize_fts5_query(query);
        if sanitized.is_empty() {
            return Ok(vec![]);
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT m.entry_id, m.session_id, m.entry_type, m.content, m.content_hash,
                        m.metadata_json, m.created_at, m.expires_at, m.relevance_score
                 FROM memory_entries m
                 JOIN memory_fts f ON m.id = f.rowid
                 WHERE memory_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare fts: {e}")))?;

        let entries = stmt
            .query_map(rusqlite::params![sanitized, limit as u32], |row| {
                Ok(Self::row_to_memory_entry(row))
            })
            .map_err(|e| CuervoError::DatabaseError(format!("search fts: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CuervoError::DatabaseError(format!("collect: {e}")))?;

        entries.into_iter().collect()
    }

    /// List memory entries, optionally filtered by type.
    pub fn list_memories(
        &self,
        entry_type: Option<MemoryEntryType>,
        limit: u32,
    ) -> Result<Vec<MemoryEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match entry_type {
            Some(ref et) => (
                "SELECT entry_id, session_id, entry_type, content, content_hash, metadata_json, created_at, expires_at, relevance_score
                 FROM memory_entries WHERE entry_type = ?1 ORDER BY created_at DESC LIMIT ?2",
                vec![Box::new(et.as_str().to_string()), Box::new(limit)],
            ),
            None => (
                "SELECT entry_id, session_id, entry_type, content, content_hash, metadata_json, created_at, expires_at, relevance_score
                 FROM memory_entries ORDER BY created_at DESC LIMIT ?1",
                vec![Box::new(limit)],
            ),
        };

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| CuervoError::DatabaseError(format!("prepare: {e}")))?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let entries = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok(Self::row_to_memory_entry(row))
            })
            .map_err(|e| CuervoError::DatabaseError(format!("list memories: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CuervoError::DatabaseError(format!("collect: {e}")))?;

        entries.into_iter().collect()
    }

    /// Delete a memory entry by entry_id.
    pub fn delete_memory(&self, entry_id: Uuid) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let rows = conn
            .execute(
                "DELETE FROM memory_entries WHERE entry_id = ?1",
                rusqlite::params![entry_id.to_string()],
            )
            .map_err(|e| CuervoError::DatabaseError(format!("delete memory: {e}")))?;

        Ok(rows > 0)
    }

    /// Prune memory entries: remove expired and enforce max_entries limit.
    /// Returns the number of entries removed.
    pub fn prune_memories(
        &self,
        max_entries: u32,
        expire_before: Option<chrono::DateTime<Utc>>,
    ) -> Result<u32> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut removed: u32 = 0;

        // 1. Remove expired entries
        if let Some(cutoff) = expire_before {
            let expired = conn
                .execute(
                    "DELETE FROM memory_entries WHERE expires_at IS NOT NULL AND expires_at < ?1",
                    rusqlite::params![cutoff.to_rfc3339()],
                )
                .map_err(|e| CuervoError::DatabaseError(format!("prune expired: {e}")))?;
            removed += expired as u32;
        }

        // 2. Enforce max_entries by removing oldest low-relevance entries
        if max_entries > 0 {
            let total: u32 = conn
                .query_row("SELECT COUNT(*) FROM memory_entries", [], |row| row.get(0))
                .map_err(|e| CuervoError::DatabaseError(format!("count: {e}")))?;

            if total > max_entries {
                let excess = total - max_entries;
                let pruned = conn
                    .execute(
                        "DELETE FROM memory_entries WHERE id IN (
                            SELECT id FROM memory_entries
                            ORDER BY relevance_score ASC, created_at ASC
                            LIMIT ?1
                        )",
                        rusqlite::params![excess],
                    )
                    .map_err(|e| CuervoError::DatabaseError(format!("prune excess: {e}")))?;
                removed += pruned as u32;
            }
        }

        Ok(removed)
    }

    /// Get aggregate statistics about the memory store (2 queries, down from 4).
    pub fn memory_stats(&self) -> Result<MemoryStats> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        // Query 1: scalar aggregates (replaces 3 separate queries).
        let (total_entries, oldest_str, newest_str): (u32, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT COUNT(*), MIN(created_at), MAX(created_at) FROM memory_entries",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| CuervoError::DatabaseError(format!("memory stats: {e}")))?;

        // Query 2: group by type (unchanged).
        let mut by_type_stmt = conn
            .prepare("SELECT entry_type, COUNT(*) FROM memory_entries GROUP BY entry_type ORDER BY COUNT(*) DESC")
            .map_err(|e| CuervoError::DatabaseError(format!("prepare: {e}")))?;

        let by_type: Vec<(String, u32)> = by_type_stmt
            .query_map([], |row| {
                let t: String = row.get(0)?;
                let c: u32 = row.get(1)?;
                Ok((t, c))
            })
            .map_err(|e| CuervoError::DatabaseError(format!("by_type: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CuervoError::DatabaseError(format!("collect: {e}")))?;

        let oldest_entry = oldest_str
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let newest_entry = newest_str
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        Ok(MemoryStats {
            total_entries,
            by_type,
            oldest_entry,
            newest_entry,
        })
    }

    /// Update the relevance score of a memory entry.
    ///
    /// Used by the confidence feedback loop: boost reflections that led to
    /// successful outcomes, decay those followed by repeated failures.
    /// Score is clamped to [0.0, 2.0].
    pub fn update_memory_relevance(&self, entry_id: Uuid, relevance_score: f64) -> Result<bool> {
        let clamped = relevance_score.clamp(0.0, 2.0);
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let rows = conn
            .execute(
                "UPDATE memory_entries SET relevance_score = ?1 WHERE entry_id = ?2",
                rusqlite::params![clamped, entry_id.to_string()],
            )
            .map_err(|e| CuervoError::DatabaseError(format!("update relevance: {e}")))?;

        Ok(rows > 0)
    }

    /// Full-text search over memory entries filtered by type.
    ///
    /// Combines FTS5 BM25 ranking with entry_type filtering for scoped retrieval
    /// (e.g., search only Reflection entries).
    pub fn search_memory_fts_by_type(
        &self,
        query: &str,
        entry_type: MemoryEntryType,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let sanitized = sanitize_fts5_query(query);
        if sanitized.is_empty() {
            return Ok(vec![]);
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT m.entry_id, m.session_id, m.entry_type, m.content, m.content_hash,
                        m.metadata_json, m.created_at, m.expires_at, m.relevance_score
                 FROM memory_entries m
                 JOIN memory_fts f ON m.id = f.rowid
                 WHERE memory_fts MATCH ?1 AND m.entry_type = ?2
                 ORDER BY rank
                 LIMIT ?3",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare fts_by_type: {e}")))?;

        let entries = stmt
            .query_map(
                rusqlite::params![sanitized, entry_type.as_str(), limit as u32],
                |row| Ok(Self::row_to_memory_entry(row)),
            )
            .map_err(|e| CuervoError::DatabaseError(format!("search fts_by_type: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CuervoError::DatabaseError(format!("collect: {e}")))?;

        entries.into_iter().collect()
    }

    /// Check if a memory entry with the given content hash already exists.
    pub fn memory_exists_by_hash(&self, content_hash: &str) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM memory_entries WHERE content_hash = ?1",
                rusqlite::params![content_hash],
                |row| row.get(0),
            )
            .map_err(|e| CuervoError::DatabaseError(format!("check hash: {e}")))?;

        Ok(exists)
    }

    /// Update the embedding vector for a memory entry.
    pub fn update_entry_embedding(
        &self,
        entry_uuid: &str,
        embedding: &[f32],
        model: &str,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();

        conn.execute(
            "UPDATE memory_entries SET embedding = ?1, embedding_model = ?2 WHERE entry_id = ?3",
            rusqlite::params![bytes, model, entry_uuid],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("update embedding: {e}")))?;

        Ok(())
    }

    /// Search memory by embedding cosine similarity.
    /// Loads all entries with embeddings and computes similarity in Rust.
    pub fn search_memory_by_embedding(
        &self,
        query_vec: &[f32],
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT entry_id, session_id, entry_type, content, content_hash,
                        metadata_json, created_at, expires_at, relevance_score, embedding
                 FROM memory_entries
                 WHERE embedding IS NOT NULL",
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prepare embedding search: {e}")))?;

        let entries_with_embeddings: Vec<(MemoryEntry, Vec<f32>)> = stmt
            .query_map([], |row| {
                let entry = Self::row_to_memory_entry(row);
                let embedding_blob: Vec<u8> = row.get(9)?;
                Ok((entry, embedding_blob))
            })
            .map_err(|e| CuervoError::DatabaseError(format!("embedding search: {e}")))?
            .filter_map(|r| r.ok())
            .filter_map(|(entry_result, blob)| {
                let entry = entry_result.ok()?;
                let floats = blob_to_f32_vec(&blob);
                Some((entry, floats))
            })
            .collect();

        // Compute cosine similarity and sort.
        let mut scored: Vec<(MemoryEntry, f32)> = entries_with_embeddings
            .into_iter()
            .map(|(entry, emb)| {
                let sim = cosine_similarity(query_vec, &emb);
                (entry, sim)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored.into_iter().map(|(entry, _)| entry).collect())
    }

    pub(crate) fn row_to_memory_entry(row: &rusqlite::Row) -> Result<MemoryEntry> {
        let entry_id_str: String = row
            .get(0)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let session_id_str: Option<String> = row
            .get(1)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let entry_type_str: String = row
            .get(2)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let content: String = row
            .get(3)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let content_hash: String = row
            .get(4)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let metadata_json: String = row
            .get(5)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let created_at_str: String = row
            .get(6)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let expires_at_str: Option<String> = row
            .get(7)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;
        let relevance_score: f64 = row
            .get(8)
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let entry_id = Uuid::parse_str(&entry_id_str)
            .map_err(|e| CuervoError::DatabaseError(format!("parse uuid: {e}")))?;
        let session_id = session_id_str
            .map(|s| Uuid::parse_str(&s))
            .transpose()
            .map_err(|e| CuervoError::DatabaseError(format!("parse session uuid: {e}")))?;
        let entry_type = MemoryEntryType::parse(&entry_type_str).ok_or_else(|| {
            CuervoError::DatabaseError(format!("unknown memory type: {entry_type_str}"))
        })?;
        let metadata: serde_json::Value = serde_json::from_str(&metadata_json)
            .map_err(|e| CuervoError::DatabaseError(format!("parse metadata: {e}")))?;
        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
            .map_err(|e| CuervoError::DatabaseError(format!("parse date: {e}")))?
            .with_timezone(&Utc);
        let expires_at = expires_at_str
            .map(|s| chrono::DateTime::parse_from_rfc3339(&s))
            .transpose()
            .map_err(|e| CuervoError::DatabaseError(format!("parse expires: {e}")))?
            .map(|dt| dt.with_timezone(&Utc));

        Ok(MemoryEntry {
            entry_id,
            session_id,
            entry_type,
            content,
            content_hash,
            metadata,
            created_at,
            expires_at,
            relevance_score,
        })
    }
}

// --- FTS5 query sanitization ---

/// FTS5 reserved keywords that must not appear as bare tokens.
const FTS5_RESERVED: &[&str] = &["AND", "OR", "NOT", "NEAR"];

/// Sanitize a user query for safe use in FTS5 MATCH clauses.
///
/// Strips FTS5 special characters (`*`, `"`, `(`, `)`, `:`, `^`, `{`, `}`),
/// quotes each surviving token, and removes FTS5 reserved keywords.
/// Returns an empty string if no valid tokens remain.
pub fn sanitize_fts5_query(query: &str) -> String {
    // Strip characters that have special meaning in FTS5.
    let cleaned: String = query
        .chars()
        .map(|c| match c {
            '*' | '"' | '(' | ')' | ':' | '^' | '{' | '}' | '+' => ' ',
            // Hyphens, dots, question marks, exclamation marks → space
            '.' | '?' | '!' | '-' => ' ',
            _ => c,
        })
        .collect();

    let tokens: Vec<String> = cleaned
        .split_whitespace()
        .filter(|t| !FTS5_RESERVED.contains(&t.to_uppercase().as_str()))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();

    tokens.join(" ")
}

// --- Embedding helpers ---

/// Cosine similarity between two f32 vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

/// Convert a BLOB of little-endian f32 bytes to a Vec<f32>.
pub fn blob_to_f32_vec(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_memory_relevance_success() {
        let db = Database::open_in_memory().unwrap();
        let entry = MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: MemoryEntryType::Reflection,
            content: "Always check file permissions".to_string(),
            content_hash: "hash_rel_1".to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        db.insert_memory(&entry).unwrap();

        // Boost relevance.
        assert!(db.update_memory_relevance(entry.entry_id, 1.5).unwrap());

        // Verify updated.
        let loaded = db.load_memory(entry.entry_id).unwrap().unwrap();
        assert!((loaded.relevance_score - 1.5).abs() < 0.001);
    }

    #[test]
    fn update_memory_relevance_clamps() {
        let db = Database::open_in_memory().unwrap();
        let entry = MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: MemoryEntryType::Reflection,
            content: "Clamping test".to_string(),
            content_hash: "hash_clamp".to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        db.insert_memory(&entry).unwrap();

        // Above max → clamped to 2.0.
        db.update_memory_relevance(entry.entry_id, 5.0).unwrap();
        let loaded = db.load_memory(entry.entry_id).unwrap().unwrap();
        assert!((loaded.relevance_score - 2.0).abs() < 0.001);

        // Below min → clamped to 0.0.
        db.update_memory_relevance(entry.entry_id, -1.0).unwrap();
        let loaded = db.load_memory(entry.entry_id).unwrap().unwrap();
        assert!(loaded.relevance_score.abs() < 0.001);
    }

    #[test]
    fn update_memory_relevance_nonexistent() {
        let db = Database::open_in_memory().unwrap();
        // Non-existent entry → returns false.
        assert!(!db.update_memory_relevance(Uuid::new_v4(), 1.0).unwrap());
    }

    #[test]
    fn search_memory_fts_by_type_filters() {
        let db = Database::open_in_memory().unwrap();

        // Insert a Reflection and a Fact both matching "error handling".
        let reflection = MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: MemoryEntryType::Reflection,
            content: "Error handling should use thiserror in libraries".to_string(),
            content_hash: "hash_fts_type_1".to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        let fact = MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: MemoryEntryType::Fact,
            content: "Error handling patterns in Rust are important".to_string(),
            content_hash: "hash_fts_type_2".to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        db.insert_memory(&reflection).unwrap();
        db.insert_memory(&fact).unwrap();

        // Search only Reflection type.
        let results = db
            .search_memory_fts_by_type("error handling", MemoryEntryType::Reflection, 10)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("thiserror"));

        // Search only Fact type.
        let results = db
            .search_memory_fts_by_type("error handling", MemoryEntryType::Fact, 10)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("patterns"));
    }

    #[test]
    fn search_memory_fts_by_type_empty_query() {
        let db = Database::open_in_memory().unwrap();
        let results = db
            .search_memory_fts_by_type("", MemoryEntryType::Reflection, 10)
            .unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn sanitize_fts5_strips_special_chars() {
        let result = sanitize_fts5_query("hello? world. foo-bar");
        assert_eq!(result, "\"hello\" \"world\" \"foo\" \"bar\"");
    }

    #[test]
    fn sanitize_fts5_empty_input() {
        assert_eq!(sanitize_fts5_query(""), "");
        assert_eq!(sanitize_fts5_query("   "), "");
    }

    #[test]
    fn sanitize_fts5_all_special_chars() {
        assert_eq!(sanitize_fts5_query("?.!-*\"(){}:^+"), "");
    }

    #[test]
    fn sanitize_fts5_removes_reserved_keywords() {
        let result = sanitize_fts5_query("foo AND bar OR baz NOT qux NEAR quux");
        assert_eq!(result, "\"foo\" \"bar\" \"baz\" \"qux\" \"quux\"");
    }

    #[test]
    fn sanitize_fts5_preserves_normal_tokens() {
        let result = sanitize_fts5_query("rust programming language");
        assert_eq!(result, "\"rust\" \"programming\" \"language\"");
    }

    #[test]
    fn search_memory_fts_with_special_chars() {
        // This is an integration test that verifies the full path.
        // It requires a DB with FTS5 enabled, so we use Database::open_in_memory().
        let db = Database::open_in_memory().unwrap();

        // Insert a memory entry that we can search for.
        let entry = crate::memory::MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: crate::memory::MemoryEntryType::Fact,
            content: "How to write a one-liner in Rust?".to_string(),
            content_hash: "hash_test_special".to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        db.insert_memory(&entry).unwrap();

        // These queries would previously break FTS5 — now they should work.
        let results = db.search_memory_fts("one-liner?", 10).unwrap();
        assert!(!results.is_empty(), "search with '?' and '-' should find results");

        let results = db.search_memory_fts("Rust.", 10).unwrap();
        assert!(!results.is_empty(), "search with '.' should find results");
    }
}
