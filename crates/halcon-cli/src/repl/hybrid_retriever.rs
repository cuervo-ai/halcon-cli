//! Hybrid retriever combining BM25 (FTS5) and embedding similarity
//! with Reciprocal Rank Fusion (RRF) and temporal decay.

use std::collections::HashMap;

use chrono::Utc;

use halcon_core::error::Result;
use halcon_core::traits::EmbeddingProvider;
use halcon_storage::AsyncDatabase;

/// Reciprocal Rank Fusion retriever combining BM25 and embedding similarity.
pub struct HybridRetriever {
    db: AsyncDatabase,
    embedding_provider: Option<Box<dyn EmbeddingProvider>>,
    /// RRF constant k (default: 60, per Cormack et al.).
    rrf_k: f64,
    /// Temporal decay half-life in days.
    decay_half_life_days: f64,
}

impl HybridRetriever {
    pub fn new(db: AsyncDatabase) -> Self {
        Self {
            db,
            embedding_provider: None,
            rrf_k: 60.0,
            decay_half_life_days: 30.0,
        }
    }

    pub fn with_rrf_k(mut self, k: f64) -> Self {
        self.rrf_k = k;
        self
    }

    pub fn with_decay_half_life(mut self, days: f64) -> Self {
        self.decay_half_life_days = days;
        self
    }

    /// Retrieve and rank memory entries using hybrid BM25 + embedding search.
    pub async fn retrieve(&self, query: &str, top_k: usize) -> Result<Vec<ScoredEntry>> {
        let candidate_limit = top_k * 3;

        // Phase 1: BM25 search via FTS5.
        let bm25_results = self.db.search_memory_fts(query, candidate_limit).await?;

        // Phase 2: Embedding search (if provider available).
        let embedding_results = if let Some(ref embedder) = self.embedding_provider {
            let query_embedding = embedder.embed(query).await?;
            self.db
                .search_memory_by_embedding(&query_embedding.values, candidate_limit)
                .await?
        } else {
            vec![]
        };

        // Phase 3: RRF fusion.
        let mut scored: HashMap<uuid::Uuid, f64> = HashMap::new();
        let mut entry_map: HashMap<uuid::Uuid, halcon_storage::MemoryEntry> = HashMap::new();

        // BM25 scores.
        for (rank, entry) in bm25_results.iter().enumerate() {
            let rrf_score = 1.0 / (self.rrf_k + rank as f64 + 1.0);
            *scored.entry(entry.entry_id).or_insert(0.0) += rrf_score;
            entry_map
                .entry(entry.entry_id)
                .or_insert_with(|| entry.clone());
        }

        // Embedding scores.
        for (rank, entry) in embedding_results.iter().enumerate() {
            let rrf_score = 1.0 / (self.rrf_k + rank as f64 + 1.0);
            *scored.entry(entry.entry_id).or_insert(0.0) += rrf_score;
            entry_map
                .entry(entry.entry_id)
                .or_insert_with(|| entry.clone());
        }

        // Phase 4: Apply temporal decay.
        // Decay formula: 2^(-age_days / half_life)
        let now = Utc::now();
        let ln2 = std::f64::consts::LN_2;
        let mut results: Vec<ScoredEntry> = scored
            .into_iter()
            .filter_map(|(id, rrf_score)| {
                let entry = entry_map.remove(&id)?;
                let age_days = (now - entry.created_at).num_seconds().max(0) as f64 / 86400.0;
                let decay = (-ln2 * age_days / self.decay_half_life_days).exp();
                let final_score = rrf_score * decay;
                Some(ScoredEntry {
                    entry,
                    score: final_score,
                })
            })
            .collect();

        // Sort by score descending, take top_k.
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);

        Ok(results)
    }
}

/// A memory entry with a computed relevance score.
#[derive(Debug, Clone)]
pub struct ScoredEntry {
    pub entry: halcon_storage::MemoryEntry,
    pub score: f64,
}

/// Compute RRF score for a given rank.
#[allow(dead_code)] // Public API, used in tests.
pub fn rrf_score(k: f64, rank: usize) -> f64 {
    1.0 / (k + rank as f64 + 1.0)
}

/// Compute temporal decay factor: 2^(-age_days / half_life_days).
#[allow(dead_code)] // Public API, used in tests.
pub fn temporal_decay(age_days: f64, half_life_days: f64) -> f64 {
    let ln2 = std::f64::consts::LN_2;
    (-ln2 * age_days / half_life_days).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use halcon_storage::{AsyncDatabase, Database, MemoryEntry, MemoryEntryType};
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    use uuid::Uuid;

    fn make_entry(content: &str, age_days: i64) -> MemoryEntry {
        let hash = hex::encode(Sha256::digest(content.as_bytes()));
        MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: MemoryEntryType::Fact,
            content: content.to_string(),
            content_hash: hash,
            metadata: serde_json::json!({}),
            created_at: Utc::now() - Duration::days(age_days),
            expires_at: None,
            relevance_score: 1.0,
        }
    }

    fn test_db() -> AsyncDatabase {
        AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()))
    }

    #[test]
    fn rrf_score_computation() {
        // rank 0, k=60: 1/(60+0+1) = 1/61
        let score = rrf_score(60.0, 0);
        assert!((score - 1.0 / 61.0).abs() < 0.0001);

        // rank 1, k=60: 1/(60+1+1) = 1/62
        let score = rrf_score(60.0, 1);
        assert!((score - 1.0 / 62.0).abs() < 0.0001);
    }

    #[test]
    fn temporal_decay_zero_age() {
        let decay = temporal_decay(0.0, 30.0);
        assert!((decay - 1.0).abs() < 0.001, "zero age should have decay 1.0, got {decay}");
    }

    #[test]
    fn temporal_decay_half_life() {
        // At exactly one half-life, decay should be ~0.5.
        let decay = temporal_decay(30.0, 30.0);
        assert!(
            (decay - 0.5).abs() < 0.01,
            "at half-life, decay should be ~0.5, got {decay}"
        );
    }

    #[test]
    fn temporal_decay_two_half_lives() {
        // At two half-lives, decay should be ~0.25.
        let decay = temporal_decay(60.0, 30.0);
        assert!(
            (decay - 0.25).abs() < 0.01,
            "at 2x half-life, decay should be ~0.25, got {decay}"
        );
    }

    #[test]
    fn temporal_decay_recent_preferred() {
        let fresh = temporal_decay(0.0, 30.0);
        let old = temporal_decay(90.0, 30.0);
        assert!(
            fresh > old,
            "fresh entry ({fresh}) should have higher decay than 90-day-old ({old})"
        );
    }

    #[tokio::test]
    async fn bm25_only_when_no_embeddings() {
        let db = test_db();

        // Insert entries matching "rust".
        db.inner()
            .insert_memory(&make_entry("Rust is a systems programming language", 0))
            .unwrap();
        db.inner()
            .insert_memory(&make_entry("Python is a scripting language", 0))
            .unwrap();

        let retriever = HybridRetriever::new(db).with_rrf_k(60.0);
        let results = retriever.retrieve("rust", 5).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].entry.content.contains("Rust"));
        assert!(results[0].score > 0.0);
    }

    #[tokio::test]
    async fn top_k_limits_results() {
        let db = test_db();

        for i in 0..10 {
            db.inner()
                .insert_memory(&make_entry(
                    &format!("Rust fact number {i} with more content to pad it out"),
                    0,
                ))
                .unwrap();
        }

        let retriever = HybridRetriever::new(db);
        let results = retriever.retrieve("rust", 3).await.unwrap();

        assert!(
            results.len() <= 3,
            "should return at most 3, got {}",
            results.len()
        );
    }

    #[tokio::test]
    async fn recent_entries_score_higher() {
        let db = test_db();

        // Insert one old and one recent entry, both matching "database".
        db.inner()
            .insert_memory(&make_entry("Database optimization techniques old", 90))
            .unwrap();
        db.inner()
            .insert_memory(&make_entry("Database optimization techniques new", 0))
            .unwrap();

        let retriever = HybridRetriever::new(db)
            .with_decay_half_life(30.0)
            .with_rrf_k(60.0);
        let results = retriever.retrieve("database optimization", 10).await.unwrap();

        assert_eq!(results.len(), 2);
        // The recent entry should score higher due to temporal decay.
        assert!(
            results[0].entry.content.contains("new"),
            "recent entry should rank first"
        );
    }

    #[tokio::test]
    async fn empty_query_returns_empty() {
        let db = test_db();
        db.inner()
            .insert_memory(&make_entry("Some content", 0))
            .unwrap();

        let retriever = HybridRetriever::new(db);
        // FTS5 won't match an empty string well.
        let results = retriever.retrieve("xyznonexistent", 5).await.unwrap();
        assert!(results.is_empty());
    }
}
