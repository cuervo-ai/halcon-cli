//! Embedding provider trait for semantic vector search.
//!
//! Allows pluggable embedding models (local or remote).
//! When no provider is configured, the memory system falls back
//! to BM25 (FTS5) keyword search.

use async_trait::async_trait;

use crate::error::Result;

/// A single embedding vector.
#[derive(Debug, Clone)]
pub struct Embedding {
    /// The raw float vector.
    pub values: Vec<f32>,
    /// Model used to produce this embedding.
    pub model: String,
}

/// Trait for providers that produce text embeddings.
///
/// Implementations may call a remote API (e.g., OpenAI, Voyage)
/// or run a local model (e.g., via ONNX runtime).
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Human-readable name of this embedding provider.
    fn name(&self) -> &str;

    /// Dimensionality of the embedding vectors.
    fn dimensions(&self) -> usize;

    /// Embed a single text string.
    async fn embed(&self, text: &str) -> Result<Embedding>;

    /// Embed multiple texts in one call (batch).
    ///
    /// Default implementation calls `embed()` sequentially.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_debug() {
        let emb = Embedding {
            values: vec![0.1, 0.2, 0.3],
            model: "test-model".to_string(),
        };
        let debug = format!("{emb:?}");
        assert!(debug.contains("test-model"));
        assert!(debug.contains("0.1"));
    }

    #[test]
    fn embedding_clone() {
        let emb = Embedding {
            values: vec![1.0, 2.0],
            model: "m".to_string(),
        };
        let cloned = emb.clone();
        assert_eq!(cloned.values, emb.values);
        assert_eq!(cloned.model, emb.model);
    }
}
