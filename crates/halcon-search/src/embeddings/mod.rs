//! Semantic embeddings engine for hybrid search.
//!
//! Uses fastembed-rs for efficient local text embeddings, enabling semantic
//! similarity search alongside traditional BM25 keyword matching.
//!
//! ## Lazy initialization
//!
//! The ONNX model (~87 MB on disk, ~300 MB in memory with runtime overhead) is
//! **not** loaded when `EmbeddingEngine::new()` is called.  It is initialized on
//! the first actual embedding request via `embed_batch()` / `embed_text()`.
//!
//! This prevents the process from being OOM-killed at startup (e.g. on MacBook
//! Air in low-power mode) while still delivering full semantic search once the
//! app is running.

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use ndarray::Array1;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, OnceLock};

/// Configuration for the embedding engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Model to use for embeddings.
    /// Default: "BAAI/bge-small-en-v1.5" (384 dims, fast, good quality)
    pub model: String,

    /// Maximum text length before truncation (tokens).
    pub max_length: usize,

    /// Batch size for embedding generation.
    pub batch_size: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            // AllMiniLML6V2 is a fast, lightweight model (384 dims)
            model: "AllMiniLML6V2".to_string(),
            max_length: 512,
            batch_size: 32,
        }
    }
}

/// Embedding engine for generating semantic vectors.
///
/// The underlying ONNX model is **lazily loaded** on the first embedding
/// request so that constructing an `EmbeddingEngine` is instant and never
/// causes an OOM kill at process startup.
pub struct EmbeddingEngine {
    /// Lazily initialized model.  `OnceLock` guarantees at-most-once init.
    model: Arc<OnceLock<Mutex<TextEmbedding>>>,
    config: EmbeddingConfig,
}

impl EmbeddingEngine {
    /// Create an embedding engine.
    ///
    /// **No model is loaded here.**  The ONNX model is loaded on the first
    /// call to `embed_batch()` or `embed_text()`.  Construction is instant.
    pub fn new() -> Result<Self> {
        Self::with_config(EmbeddingConfig::default())
    }

    /// Create with custom configuration (model still loaded lazily).
    pub fn with_config(config: EmbeddingConfig) -> Result<Self> {
        Ok(Self {
            model: Arc::new(OnceLock::new()),
            config,
        })
    }

    /// Ensure the ONNX model is loaded, initializing on the first call.
    ///
    /// Uses `tokio::task::spawn_blocking` so the heavy I/O + ONNX init
    /// never blocks the async runtime.  Concurrent callers may each start
    /// an init race; only the first `OnceLock::set()` wins — the rest
    /// discard their loaded model (no data loss, just a brief double-load).
    async fn ensure_model(&self) -> Result<()> {
        // Fast path: already initialized.
        if self.model.get().is_some() {
            return Ok(());
        }

        tracing::info!("Loading embedding model (first use) — this may take a moment");

        let model_arc = self.model.clone();

        // Load in the blocking thread pool so we don't starve the async runtime.
        let model = tokio::task::spawn_blocking(move || -> Result<TextEmbedding> {
            TextEmbedding::try_new(
                InitOptions::new(EmbeddingModel::AllMiniLML6V2)
                    .with_show_download_progress(false),
            )
            .context("Failed to initialize embedding model (AllMiniLML6V2)")
        })
        .await
        .context("Embedding model init task panicked")??;

        // `set` fails silently if another concurrent caller already set it — that's fine.
        let _ = model_arc.set(Mutex::new(model));

        tracing::info!("Embedding model loaded successfully");
        Ok(())
    }

    /// Generate embedding for a single text.
    ///
    /// Returns a normalized 384-dimensional vector.
    /// Triggers lazy model load on first call.
    pub async fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.embed_batch(&[text.to_string()]).await?;
        embeddings
            .into_iter()
            .next()
            .context("Empty embedding result")
    }

    /// Generate embeddings for a batch of texts.
    ///
    /// More efficient than calling `embed_text()` in a loop.
    /// Triggers lazy model load on first call.
    pub async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        // Lazy-init: blocks only the first time.
        self.ensure_model().await?;

        let texts_owned: Vec<String> = texts.to_vec();
        let model_arc = self.model.clone();

        // Run embedding generation in blocking thread pool (CPU-bound).
        tokio::task::spawn_blocking(move || -> Result<Vec<Vec<f32>>> {
            // SAFETY: ensure_model() succeeded above, so get() is Some.
            let mutex = model_arc.get().expect("model initialized");
            let model = mutex
                .lock()
                .map_err(|e| anyhow::anyhow!("Embedding mutex poisoned: {}", e))?;
            model
                .embed(texts_owned, None)
                .context("Failed to generate embeddings")
        })
        .await
        .context("Embedding task panicked")?
    }

    /// Compute cosine similarity between two embeddings.
    ///
    /// Returns a value in [-1.0, 1.0] where 1.0 is identical.
    /// Does **not** require the model to be loaded.
    pub fn cosine_similarity(&self, a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return 0.0;
        }

        let a_arr = Array1::from_vec(a.to_vec());
        let b_arr = Array1::from_vec(b.to_vec());

        let dot = a_arr.dot(&b_arr);
        let norm_a = a_arr.dot(&a_arr).sqrt();
        let norm_b = b_arr.dot(&b_arr).sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }

        dot / (norm_a * norm_b)
    }

    /// Get the embedding dimension for this model (384 for AllMiniLML6V2).
    ///
    /// Does **not** require the model to be loaded.
    pub fn dimension(&self) -> usize {
        384
    }

    /// Normalize an embedding vector to unit length.
    ///
    /// Does **not** require the model to be loaded.
    pub fn normalize(&self, embedding: &mut [f32]) {
        let arr = Array1::from_vec(embedding.to_vec());
        let norm = arr.dot(&arr).sqrt();

        if norm > 0.0 {
            for val in embedding.iter_mut() {
                *val /= norm;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires libonnxruntime.dylib — install ONNX Runtime to run"]
    async fn test_embed_single_text() {
        let engine = EmbeddingEngine::new().expect("Failed to init engine");

        let text = "This is a test document about search engines.";
        let embedding = engine.embed_text(text).await.expect("Embedding failed");

        // bge-small-en-v1.5 produces 384-dim vectors
        assert_eq!(embedding.len(), 384);

        // Embeddings should be non-zero
        let sum: f32 = embedding.iter().sum();
        assert!(sum.abs() > 0.0, "Embedding is all zeros");
    }

    #[tokio::test]
    #[ignore = "requires libonnxruntime.dylib — install ONNX Runtime to run"]
    async fn test_embed_batch() {
        let engine = EmbeddingEngine::new().expect("Failed to init engine");

        let texts = vec![
            "First document".to_string(),
            "Second document".to_string(),
            "Third document".to_string(),
        ];

        let embeddings = engine.embed_batch(&texts).await.expect("Batch embedding failed");

        assert_eq!(embeddings.len(), 3);
        for emb in &embeddings {
            assert_eq!(emb.len(), 384);
        }
    }

    #[tokio::test]
    #[ignore = "requires libonnxruntime.dylib — install ONNX Runtime to run"]
    async fn test_cosine_similarity() {
        let engine = EmbeddingEngine::new().expect("Failed to init engine");

        let text1 = "search engine optimization";
        let text2 = "search engine ranking";
        let text3 = "banana recipe cooking";

        let emb1 = engine.embed_text(text1).await.unwrap();
        let emb2 = engine.embed_text(text2).await.unwrap();
        let emb3 = engine.embed_text(text3).await.unwrap();

        let sim_12 = engine.cosine_similarity(&emb1, &emb2);
        let sim_13 = engine.cosine_similarity(&emb1, &emb3);

        // Similar texts should have higher similarity
        assert!(
            sim_12 > sim_13,
            "Expected similarity(search, search) > similarity(search, banana), got {} vs {}",
            sim_12,
            sim_13
        );

        // Similarity should be in valid range
        assert!(sim_12 >= -1.0 && sim_12 <= 1.0);
        assert!(sim_13 >= -1.0 && sim_13 <= 1.0);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let engine = EmbeddingEngine::new().expect("Failed to init engine");

        let vec = vec![1.0, 2.0, 3.0];
        let sim = engine.cosine_similarity(&vec, &vec);

        // Identical vectors should have similarity ~1.0
        assert!((sim - 1.0).abs() < 0.001, "Expected ~1.0, got {}", sim);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let engine = EmbeddingEngine::new().expect("Failed to init engine");

        let vec1 = vec![1.0, 0.0, 0.0];
        let vec2 = vec![0.0, 1.0, 0.0];
        let sim = engine.cosine_similarity(&vec1, &vec2);

        // Orthogonal vectors should have similarity ~0.0
        assert!(sim.abs() < 0.001, "Expected ~0.0, got {}", sim);
    }

    #[test]
    fn test_normalize() {
        let engine = EmbeddingEngine::new().expect("Failed to init engine");

        let mut vec = vec![3.0, 4.0];
        engine.normalize(&mut vec);

        // Normalized vector should have unit length
        let arr = Array1::from_vec(vec.clone());
        let length = arr.dot(&arr).sqrt();
        assert!((length - 1.0).abs() < 0.001, "Expected length 1.0, got {}", length);
    }

    #[tokio::test]
    async fn test_empty_batch() {
        let engine = EmbeddingEngine::new().expect("Failed to init engine");

        let embeddings = engine.embed_batch(&[]).await.expect("Empty batch failed");
        assert_eq!(embeddings.len(), 0);
    }

    #[test]
    fn test_dimension() {
        let engine = EmbeddingEngine::new().expect("Failed to init engine");
        assert_eq!(engine.dimension(), 384);
    }

    /// Verify that constructing an EmbeddingEngine is instant (lazy init).
    /// The model is NOT loaded here — this must not OOM.
    #[test]
    fn test_construction_is_instant_no_model_load() {
        // This should complete in microseconds regardless of available RAM.
        let start = std::time::Instant::now();
        let engine = EmbeddingEngine::new().expect("Construction failed");
        let elapsed = start.elapsed();

        // No model loaded yet.
        assert!(engine.model.get().is_none(), "Model should not be loaded at construction");
        // Should be sub-millisecond.
        assert!(
            elapsed.as_millis() < 100,
            "Construction took too long ({:?}) — model may be loading eagerly",
            elapsed
        );
    }

    #[tokio::test]
    #[ignore = "requires libonnxruntime.dylib — install ONNX Runtime to run"]
    async fn test_semantic_similarity_examples() {
        let engine = EmbeddingEngine::new().expect("Failed to init engine");

        let query = "database indexing performance";
        let doc1 = "optimizing database index structures for faster queries";
        let doc2 = "cooking pasta with tomato sauce";

        let query_emb = engine.embed_text(query).await.unwrap();
        let doc1_emb = engine.embed_text(doc1).await.unwrap();
        let doc2_emb = engine.embed_text(doc2).await.unwrap();

        let sim1 = engine.cosine_similarity(&query_emb, &doc1_emb);
        let sim2 = engine.cosine_similarity(&query_emb, &doc2_emb);

        // Semantically related should score higher
        assert!(
            sim1 > sim2,
            "Expected database docs to be more similar, got {} vs {}",
            sim1,
            sim2
        );
    }
}
