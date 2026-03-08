//! `VectorMemorySource` — pipeline-triggered semantic memory retrieval.
//!
//! Implements `ContextSource` so that each round the context pipeline queries the
//! vector index for memories relevant to the user's current message.  Results are
//! surfaced as `ContextChunk`s with priority 25 (below L2 cold-store at 40 and L3
//! BM25 at 30, above no-context filler).
//!
//! The `SharedVectorStore` is shared with `SearchMemoryTool` so both the pipeline
//! and the agent-triggered tool query the same in-memory index.

use async_trait::async_trait;
use halcon_context::{SearchResult, VectorMemoryStore};
use halcon_core::{
    error::Result,
    traits::{ContextChunk, ContextQuery, ContextSource},
};
use std::sync::{Arc, Mutex};

/// Thread-safe shared handle to a `VectorMemoryStore`.
pub type SharedVectorStore = Arc<Mutex<VectorMemoryStore>>;

/// Priority assigned to semantic memory chunks in the context pipeline.
const PRIORITY: u32 = 25;

/// Pipeline-triggered semantic memory retrieval via cosine similarity + MMR.
pub struct VectorMemorySource {
    store: SharedVectorStore,
    top_k: usize,
}

impl VectorMemorySource {
    /// Create a new source backed by the given shared store.
    pub fn new(store: SharedVectorStore, top_k: usize) -> Self {
        Self { store, top_k }
    }

    /// Format a `SearchResult` into a `ContextChunk`.
    fn format_chunk(result: &SearchResult) -> ContextChunk {
        let score_pct = (result.score * 100.0).round() as u32;
        let content = format!(
            "<!-- memory: {} (similarity: {score_pct}%) -->\n{}",
            result.entry.source,
            result.entry.text.trim()
        );
        let estimated_tokens = (content.len() / 4).max(1);
        ContextChunk {
            source: format!("vector_memory:{}", result.entry.source),
            priority: PRIORITY,
            content,
            estimated_tokens,
        }
    }
}

#[async_trait]
impl ContextSource for VectorMemorySource {
    fn name(&self) -> &str {
        "vector_memory"
    }

    fn priority(&self) -> u32 {
        PRIORITY
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let user_message = match &query.user_message {
            Some(m) if !m.trim().is_empty() => m.clone(),
            _ => return Ok(Vec::new()),
        };

        let results = {
            let store = match self.store.lock() {
                Ok(g) => g,
                Err(e) => {
                    tracing::warn!("vector_memory: store lock poisoned: {e}");
                    return Ok(Vec::new());
                }
            };
            if store.is_empty() {
                return Ok(Vec::new());
            }
            store.search(&user_message, self.top_k)
        };

        let chunks: Vec<ContextChunk> = results.iter().map(Self::format_chunk).collect();

        if !chunks.is_empty() {
            tracing::debug!(
                "vector_memory: surfaced {} chunk(s) for query '{}'",
                chunks.len(),
                &user_message[..user_message.len().min(60)]
            );
        }

        Ok(chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_context::VectorMemoryStore;
    use halcon_core::traits::ContextQuery;

    fn make_store() -> SharedVectorStore {
        let mut s = VectorMemoryStore::new();
        s.index_text(
            "## FASE-2 Debugging\nFASE-2 path gate fired on bad file paths. Fix: explore with directory_tree.",
            "project:MEMORY.md§FASE-2",
        );
        s.index_text(
            "## Authentication\nJWT RS256 tokens for API auth.",
            "project:MEMORY.md§Auth",
        );
        Arc::new(Mutex::new(s))
    }

    fn query(msg: &str) -> ContextQuery {
        ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: Some(msg.to_string()),
            token_budget: 4096,
        }
    }

    #[tokio::test]
    async fn gather_returns_relevant_chunks() {
        let src = VectorMemorySource::new(make_store(), 3);
        let chunks = src.gather(&query("file path errors gate")).await.unwrap();
        assert!(!chunks.is_empty());
        assert!(chunks[0].content.contains("FASE-2") || chunks[0].content.contains("path"));
    }

    #[tokio::test]
    async fn gather_empty_query_returns_empty() {
        let src = VectorMemorySource::new(make_store(), 3);
        let chunks = src
            .gather(&ContextQuery {
                working_directory: "/tmp".to_string(),
                user_message: None,
                token_budget: 4096,
            })
            .await
            .unwrap();
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn gather_empty_store_returns_empty() {
        let store = Arc::new(Mutex::new(VectorMemoryStore::new()));
        let src = VectorMemorySource::new(store, 3);
        let chunks = src.gather(&query("rust async tokio")).await.unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn name_is_vector_memory() {
        let store = Arc::new(Mutex::new(VectorMemoryStore::new()));
        let src = VectorMemorySource::new(store, 5);
        assert_eq!(src.name(), "vector_memory");
        assert_eq!(src.priority(), PRIORITY);
    }

    #[tokio::test]
    async fn chunk_contains_source_label() {
        let src = VectorMemorySource::new(make_store(), 3);
        let chunks = src.gather(&query("JWT authentication")).await.unwrap();
        if !chunks.is_empty() {
            assert!(chunks[0].source.starts_with("vector_memory:"));
        }
    }
}
