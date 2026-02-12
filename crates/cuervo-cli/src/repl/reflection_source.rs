//! ContextSource that injects query-relevant self-reflections into the system prompt.
//!
//! Priority 85: above memory (80), below planning (90) and instructions (100).
//!
//! Uses FTS5 BM25 search scoped to Reflection entries, with temporal decay
//! and relevance-score weighting. Falls back to recency-ordered retrieval
//! when no query is available.

use async_trait::async_trait;
use chrono::Utc;

use cuervo_core::error::Result;
use cuervo_core::traits::{ContextChunk, ContextQuery, ContextSource};
use cuervo_storage::AsyncDatabase;

/// Temporal decay half-life in days for reflection scoring.
const DECAY_HALF_LIFE_DAYS: f64 = 14.0;

/// Injects query-relevant self-reflections from memory into context.
///
/// When a user message is available, uses FTS5 BM25 search scoped to
/// Reflection entries, then applies temporal decay and relevance weighting.
/// Falls back to recency-ordered retrieval when no query is available.
pub struct ReflectionSource {
    db: AsyncDatabase,
    max_reflections: usize,
}

impl ReflectionSource {
    pub fn new(db: AsyncDatabase, max_reflections: usize) -> Self {
        Self {
            db,
            max_reflections,
        }
    }
}

/// Score a reflection entry: combines stored relevance with temporal decay.
///
/// Formula: relevance_score * 2^(-age_days / half_life)
fn score_reflection(relevance: f64, age_days: f64) -> f64 {
    let decay = (-std::f64::consts::LN_2 * age_days / DECAY_HALF_LIFE_DAYS).exp();
    relevance * decay
}

#[async_trait]
impl ContextSource for ReflectionSource {
    fn name(&self) -> &str {
        "reflections"
    }

    fn priority(&self) -> u32 {
        85
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let candidate_limit = self.max_reflections * 3;
        let now = Utc::now();

        // Phase 1: Try query-relevant FTS search if user message available.
        let mut entries = if let Some(ref msg) = query.user_message {
            let trimmed = msg.trim();
            if !trimmed.is_empty() {
                self.db
                    .search_memory_fts_by_type(
                        trimmed,
                        cuervo_storage::MemoryEntryType::Reflection,
                        candidate_limit,
                    )
                    .await?
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Phase 2: Fall back to recency-ordered if FTS found nothing.
        if entries.is_empty() {
            entries = self
                .db
                .list_memories(
                    Some(cuervo_storage::MemoryEntryType::Reflection),
                    candidate_limit as u32,
                )
                .await?;
        }

        if entries.is_empty() {
            return Ok(vec![]);
        }

        // Phase 3: Score by relevance * temporal decay, sort, take top-K.
        let mut scored: Vec<(f64, &cuervo_storage::MemoryEntry)> = entries
            .iter()
            .map(|e| {
                let age_days =
                    (now - e.created_at).num_seconds().max(0) as f64 / 86400.0;
                let score = score_reflection(e.relevance_score, age_days);
                (score, e)
            })
            .collect();

        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(self.max_reflections);

        // Phase 4: Build context chunk.
        let mut content = String::from("## Previous Self-Reflections\n\n");
        content.push_str("Use these reflections to avoid repeating past mistakes:\n\n");
        for (score, entry) in &scored {
            content.push_str(&format!("- (relevance: {score:.2}) {}\n", entry.content));
        }

        let tokens = cuervo_context::estimate_tokens(&content);
        Ok(vec![ContextChunk {
            source: "reflection".into(),
            priority: self.priority(),
            content,
            estimated_tokens: tokens,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn test_async_db() -> AsyncDatabase {
        AsyncDatabase::new(Arc::new(cuervo_storage::Database::open_in_memory().unwrap()))
    }

    #[test]
    fn reflection_source_priority() {
        let source = ReflectionSource::new(test_async_db(), 3);
        assert_eq!(source.priority(), 85);
    }

    #[test]
    fn reflection_source_name() {
        let source = ReflectionSource::new(test_async_db(), 3);
        assert_eq!(source.name(), "reflections");
    }

    #[tokio::test]
    async fn empty_when_no_reflections() {
        let source = ReflectionSource::new(test_async_db(), 3);
        let query = ContextQuery {
            working_directory: "/tmp".into(),
            user_message: Some("test".into()),
            token_budget: 10000,
        };
        let chunks = source.gather(&query).await.unwrap();
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn returns_reflections_from_memory() {
        use sha2::Digest;
        let db = test_async_db();

        let entry = cuervo_storage::MemoryEntry {
            entry_id: uuid::Uuid::new_v4(),
            session_id: None,
            entry_type: cuervo_storage::MemoryEntryType::Reflection,
            content: "Check file exists before reading".into(),
            content_hash: hex::encode(sha2::Sha256::digest(
                b"Check file exists before reading",
            )),
            metadata: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        db.insert_memory(&entry).await.unwrap();

        let source = ReflectionSource::new(db, 5);
        let query = ContextQuery {
            working_directory: "/tmp".into(),
            user_message: Some("test".into()),
            token_budget: 10000,
        };
        let chunks = source.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("Check file exists before reading"));
        assert!(chunks[0].content.contains("Self-Reflections"));
        assert_eq!(chunks[0].priority, 85);
    }

    #[tokio::test]
    async fn query_relevant_retrieval() {
        use sha2::Digest;
        let db = test_async_db();

        // Insert two reflections — one about error handling, one about file ops.
        let r1 = cuervo_storage::MemoryEntry {
            entry_id: uuid::Uuid::new_v4(),
            session_id: None,
            entry_type: cuervo_storage::MemoryEntryType::Reflection,
            content: "Error handling should use thiserror in libraries".into(),
            content_hash: hex::encode(sha2::Sha256::digest(
                b"Error handling should use thiserror in libraries",
            )),
            metadata: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        let r2 = cuervo_storage::MemoryEntry {
            entry_id: uuid::Uuid::new_v4(),
            session_id: None,
            entry_type: cuervo_storage::MemoryEntryType::Reflection,
            content: "Always check file permissions before writing".into(),
            content_hash: hex::encode(sha2::Sha256::digest(
                b"Always check file permissions before writing",
            )),
            metadata: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        db.insert_memory(&r1).await.unwrap();
        db.insert_memory(&r2).await.unwrap();

        let source = ReflectionSource::new(db, 5);

        // Query about error handling → should surface the error reflection.
        let query = ContextQuery {
            working_directory: "/tmp".into(),
            user_message: Some("error handling thiserror".into()),
            token_budget: 10000,
        };
        let chunks = source.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        // BM25 should rank error handling reflection first.
        let lines: Vec<&str> = chunks[0].content.lines().collect();
        let first_reflection = lines.iter().find(|l| l.starts_with("- ")).unwrap();
        assert!(
            first_reflection.contains("thiserror"),
            "first reflection should be about error handling, got: {first_reflection}"
        );
    }

    #[tokio::test]
    async fn fallback_to_recency_on_no_query() {
        use sha2::Digest;
        let db = test_async_db();

        let entry = cuervo_storage::MemoryEntry {
            entry_id: uuid::Uuid::new_v4(),
            session_id: None,
            entry_type: cuervo_storage::MemoryEntryType::Reflection,
            content: "Always run tests before committing".into(),
            content_hash: hex::encode(sha2::Sha256::digest(
                b"Always run tests before committing",
            )),
            metadata: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        db.insert_memory(&entry).await.unwrap();

        let source = ReflectionSource::new(db, 5);
        // No user_message → falls back to recency.
        let query = ContextQuery {
            working_directory: "/tmp".into(),
            user_message: None,
            token_budget: 10000,
        };
        let chunks = source.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("Always run tests"));
    }

    #[tokio::test]
    async fn higher_relevance_ranked_first() {
        use sha2::Digest;
        let db = test_async_db();

        // Both mention "testing" but r2 has higher relevance.
        let r1 = cuervo_storage::MemoryEntry {
            entry_id: uuid::Uuid::new_v4(),
            session_id: None,
            entry_type: cuervo_storage::MemoryEntryType::Reflection,
            content: "Testing with mocks requires careful setup".into(),
            content_hash: hex::encode(sha2::Sha256::digest(
                b"Testing with mocks requires careful setup",
            )),
            metadata: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            expires_at: None,
            relevance_score: 0.5,
        };
        let r2 = cuervo_storage::MemoryEntry {
            entry_id: uuid::Uuid::new_v4(),
            session_id: None,
            entry_type: cuervo_storage::MemoryEntryType::Reflection,
            content: "Testing integration points is more valuable than unit testing".into(),
            content_hash: hex::encode(sha2::Sha256::digest(
                b"Testing integration points is more valuable than unit testing",
            )),
            metadata: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            expires_at: None,
            relevance_score: 1.8,
        };
        db.insert_memory(&r1).await.unwrap();
        db.insert_memory(&r2).await.unwrap();

        let source = ReflectionSource::new(db, 5);
        let query = ContextQuery {
            working_directory: "/tmp".into(),
            user_message: None,
            token_budget: 10000,
        };
        let chunks = source.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        // Higher relevance should be ranked first.
        let lines: Vec<&str> = chunks[0]
            .content
            .lines()
            .filter(|l| l.starts_with("- "))
            .collect();
        assert!(lines[0].contains("integration points"), "high-relevance should be first");
    }

    #[test]
    fn score_reflection_zero_age() {
        let score = score_reflection(1.0, 0.0);
        assert!((score - 1.0).abs() < 0.001);
    }

    #[test]
    fn score_reflection_half_life() {
        let score = score_reflection(1.0, DECAY_HALF_LIFE_DAYS);
        assert!(
            (score - 0.5).abs() < 0.01,
            "at half-life, score should be ~0.5, got {score}"
        );
    }

    #[test]
    fn score_reflection_relevance_multiplier() {
        let base = score_reflection(1.0, 7.0);
        let boosted = score_reflection(1.5, 7.0);
        assert!(boosted > base, "higher relevance should produce higher score");
        assert!((boosted / base - 1.5).abs() < 0.01, "should scale linearly with relevance");
    }
}
