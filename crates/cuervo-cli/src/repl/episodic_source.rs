//! ContextSource that uses hybrid retrieval (BM25 + embedding + RRF).
//!
//! Priority 80: same as existing MemorySource (replaces it when episodic enabled).

use async_trait::async_trait;

use cuervo_context::estimate_tokens;
use cuervo_core::error::Result;
use cuervo_core::traits::{ContextChunk, ContextQuery, ContextSource};

use super::hybrid_retriever::HybridRetriever;

/// ContextSource that uses hybrid retrieval with RRF and temporal decay.
///
/// Replaces the basic MemorySource when `config.memory.episodic` is enabled.
pub struct EpisodicSource {
    retriever: HybridRetriever,
    top_k: usize,
    token_budget: usize,
}

impl EpisodicSource {
    pub fn new(retriever: HybridRetriever, top_k: usize, token_budget: usize) -> Self {
        Self {
            retriever,
            top_k,
            token_budget,
        }
    }
}

#[async_trait]
impl ContextSource for EpisodicSource {
    fn name(&self) -> &str {
        "episodic_memory"
    }

    fn priority(&self) -> u32 {
        80
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let search_query = match &query.user_message {
            Some(msg) if !msg.trim().is_empty() => msg.clone(),
            _ => return Ok(vec![]),
        };

        let results = self.retriever.retrieve(&search_query, self.top_k).await?;
        if results.is_empty() {
            return Ok(vec![]);
        }

        // Build context chunk respecting token budget.
        let mut content = String::from("## Relevant Memories\n\n");
        let mut total_tokens = estimate_tokens(&content);
        let budget = self.token_budget.min(query.token_budget);

        for scored in &results {
            let label = scored.entry.entry_type.as_str();
            let entry_text = format!("- [{}] (score: {:.3}) {}\n", label, scored.score, scored.entry.content);
            let entry_tokens = estimate_tokens(&entry_text);
            if total_tokens + entry_tokens > budget {
                break;
            }
            content.push_str(&entry_text);
            total_tokens += entry_tokens;
        }

        Ok(vec![ContextChunk {
            source: "episodic_memory".into(),
            priority: self.priority(),
            content,
            estimated_tokens: total_tokens,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_storage::{AsyncDatabase, Database};
    use std::sync::Arc;

    fn test_db() -> AsyncDatabase {
        AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()))
    }

    #[test]
    fn episodic_source_priority() {
        let retriever = HybridRetriever::new(test_db());
        let source = EpisodicSource::new(retriever, 5, 2000);
        assert_eq!(source.priority(), 80);
    }

    #[test]
    fn episodic_source_name() {
        let retriever = HybridRetriever::new(test_db());
        let source = EpisodicSource::new(retriever, 5, 2000);
        assert_eq!(source.name(), "episodic_memory");
    }

    #[tokio::test]
    async fn episodic_source_empty_query() {
        let retriever = HybridRetriever::new(test_db());
        let source = EpisodicSource::new(retriever, 5, 2000);

        let query = ContextQuery {
            working_directory: "/tmp".into(),
            user_message: None,
            token_budget: 10000,
        };
        let chunks = source.gather(&query).await.unwrap();
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn episodic_source_respects_budget() {
        use chrono::Utc;
        use sha2::{Digest, Sha256};

        let db = test_db();
        // Insert entries matching "rust".
        for i in 0..10 {
            let content = format!("Rust implementation detail number {i} with some extra padding content for tokens");
            let hash = hex::encode(Sha256::digest(content.as_bytes()));
            let entry = cuervo_storage::MemoryEntry {
                entry_id: uuid::Uuid::new_v4(),
                session_id: None,
                entry_type: cuervo_storage::MemoryEntryType::Fact,
                content,
                content_hash: hash,
                metadata: serde_json::json!({}),
                created_at: Utc::now(),
                expires_at: None,
                relevance_score: 1.0,
            };
            db.inner().insert_memory(&entry).unwrap();
        }

        let retriever = HybridRetriever::new(db);
        // Very small budget — should limit output.
        let source = EpisodicSource::new(retriever, 10, 50);

        let query = ContextQuery {
            working_directory: "/tmp".into(),
            user_message: Some("rust".into()),
            token_budget: 50,
        };
        let chunks = source.gather(&query).await.unwrap();
        // Should have at most 1 chunk with limited content.
        assert!(chunks.len() <= 1);
        if !chunks.is_empty() {
            assert!(chunks[0].estimated_tokens <= 50);
        }
    }
}
