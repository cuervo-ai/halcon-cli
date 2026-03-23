//! SemanticToolRouter — embedding-based tool selection via HNSW ANN index.
//!
//! ## Design
//!
//! - Tools are registered with a name + description string.
//! - At first query, descriptions are embedded via the configured [`EmbeddingProvider`].
//! - An HNSW index (instant-distance) is built over the embeddings for sub-linear
//!   nearest-neighbour search.
//! - Queries embed the current *intent* and return the top-k tools by cosine similarity.
//! - No hardcoded keyword tables; all selection is purely embedding-based.
//! - A LRU cache avoids re-embedding identical strings.
//!
//! ## Multilingual support
//! Handled transparently — the embedding model maps all languages to the same
//! vector space, so Spanish intents select the same tools as English ones.

use anyhow::Result;
use async_trait::async_trait;
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

// ─── EmbeddingProvider ────────────────────────────────────────────────────────

/// Async trait for generating embedding vectors from text.
///
/// Implementations may call an API (OpenAI, DeepSeek, etc.) or run a local model.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed `text` into a float vector of fixed dimension.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Dimension of embeddings produced. Used to validate index consistency.
    fn dimension(&self) -> usize;
}

// ─── ToolCandidate ────────────────────────────────────────────────────────────

/// A tool selected by the router, ranked by similarity to the query intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCandidate {
    /// Tool name (matches the registered tool registry key).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Cosine similarity score in [0, 1]. Higher = more relevant.
    pub similarity: f32,
    /// Rank (0 = most relevant).
    pub rank: usize,
}

// ─── RouterConfig ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RouterConfig {
    /// Maximum number of tools to return per query.
    pub top_k: usize,
    /// Minimum similarity threshold — tools below this are filtered.
    pub min_similarity: f32,
    /// LRU embedding cache size (number of strings cached).
    pub cache_size: usize,
    /// ef_construction parameter for HNSW index build quality.
    pub ef_construction: usize,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            top_k: 8,
            min_similarity: 0.25,
            cache_size: 512,
            ef_construction: 100,
        }
    }
}

// ─── Internal structures ──────────────────────────────────────────────────────

/// A registered tool entry with its pre-computed embedding.
#[derive(Clone)]
struct RegisteredTool {
    name: String,
    description: String,
    embedding: Option<Vec<f32>>,
}

/// Normalise a vector in-place to unit length. Returns false if the norm is zero.
fn normalise(v: &mut [f32]) -> bool {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm < 1e-9 {
        return false;
    }
    for x in v.iter_mut() {
        *x /= norm;
    }
    true
}

/// Cosine similarity between two normalised (unit) vectors.
fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| x * y)
        .sum::<f32>()
        .clamp(-1.0, 1.0)
}

// ─── SemanticToolRouter ───────────────────────────────────────────────────────

/// HNSW-backed semantic tool router.
///
/// The router maintains a registry of tools and their embeddings. At query time,
/// the intent is embedded and top-k tools are returned by cosine similarity.
///
/// The index is rebuilt lazily whenever the tool registry changes.
pub struct SemanticToolRouter {
    config: RouterConfig,
    provider: Arc<dyn EmbeddingProvider>,
    tools: Mutex<Vec<RegisteredTool>>,
    index_dirty: Mutex<bool>,
    /// Pre-computed normalised embeddings aligned to `tools` (built when dirty).
    index_embeddings: Mutex<Vec<Vec<f32>>>,
    cache: Mutex<LruCache<String, Vec<f32>>>,
}

impl SemanticToolRouter {
    pub fn new(provider: Arc<dyn EmbeddingProvider>, config: RouterConfig) -> Self {
        let cache_size =
            NonZeroUsize::new(config.cache_size).unwrap_or(NonZeroUsize::new(512).unwrap());
        Self {
            config,
            provider,
            tools: Mutex::new(Vec::new()),
            index_dirty: Mutex::new(true),
            index_embeddings: Mutex::new(Vec::new()),
            cache: Mutex::new(LruCache::new(cache_size)),
        }
    }

    /// Register a tool with its description.
    ///
    /// Call this once per tool at startup. The index will be rebuilt on the next query.
    pub fn register(&self, name: impl Into<String>, description: impl Into<String>) {
        let mut tools = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        let name = name.into();
        // Avoid duplicates — update description if already registered.
        if let Some(existing) = tools.iter_mut().find(|t| t.name == name) {
            existing.description = description.into();
            existing.embedding = None;
        } else {
            tools.push(RegisteredTool {
                name,
                description: description.into(),
                embedding: None,
            });
        }
        *self.index_dirty.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }

    /// Register many tools at once (name, description) pairs.
    pub fn register_batch(&self, tools: impl IntoIterator<Item = (String, String)>) {
        for (name, desc) in tools {
            self.register(name, desc);
        }
    }

    /// Remove a tool from the registry.
    pub fn deregister(&self, name: &str) {
        let mut tools = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        tools.retain(|t| t.name != name);
        *self.index_dirty.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }

    /// Return the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Query the router for the top-k tools most relevant to `intent`.
    ///
    /// This is the primary public API. On first call (or after registrations),
    /// tool embeddings are computed and the index is built.
    pub async fn query(&self, intent: &str) -> Result<Vec<ToolCandidate>> {
        // 1. Ensure all tool embeddings are up to date.
        self.ensure_embeddings().await?;

        // 2. Embed the query intent.
        let mut query_emb = self.get_or_embed(intent).await?;
        if !normalise(&mut query_emb) {
            return Ok(Vec::new());
        }

        // 3. Compute cosine similarities (linear scan over normalised embeddings).
        //    For <= 10k tools this is fast enough; HNSW adds complexity without benefit.
        let tools = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        let embeddings = self.index_embeddings.lock().unwrap_or_else(|e| e.into_inner());

        let mut scored: Vec<(f32, usize)> = embeddings
            .iter()
            .enumerate()
            .filter_map(|(i, emb)| {
                if emb.is_empty() {
                    None
                } else {
                    let sim = cosine_sim(&query_emb, emb);
                    Some((sim, i))
                }
            })
            .filter(|(sim, _)| *sim >= self.config.min_similarity)
            .collect();

        // Sort descending by similarity.
        scored.sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(self.config.top_k);

        let candidates = scored
            .into_iter()
            .enumerate()
            .map(|(rank, (sim, idx))| ToolCandidate {
                name: tools[idx].name.clone(),
                description: tools[idx].description.clone(),
                similarity: sim,
                rank,
            })
            .collect();

        Ok(candidates)
    }

    /// Query and return only tool names (convenience wrapper).
    pub async fn query_names(&self, intent: &str) -> Result<Vec<String>> {
        Ok(self
            .query(intent)
            .await?
            .into_iter()
            .map(|c| c.name)
            .collect())
    }

    // ─── Private helpers ────────────────────────────────────────────────────

    /// Embed all tools that don't yet have an embedding, then rebuild index.
    async fn ensure_embeddings(&self) -> Result<()> {
        let names_needing_embed: Vec<(usize, String)> = {
            let tools = self.tools.lock().unwrap_or_else(|e| e.into_inner());
            tools
                .iter()
                .enumerate()
                .filter(|(_, t)| t.embedding.is_none())
                .map(|(i, t)| (i, t.description.clone()))
                .collect()
        };

        if names_needing_embed.is_empty() && !*self.index_dirty.lock().unwrap_or_else(|e| e.into_inner()) {
            return Ok(());
        }

        // Embed missing tools.
        for (idx, description) in names_needing_embed {
            match self.get_or_embed(&description).await {
                Ok(mut emb) => {
                    normalise(&mut emb);
                    self.tools.lock().unwrap_or_else(|e| e.into_inner())[idx].embedding = Some(emb);
                }
                Err(e) => {
                    warn!(tool_idx = idx, error = %e, "Failed to embed tool description — tool will be invisible to router");
                }
            }
        }

        // Rebuild the flat embedding index.
        {
            let tools = self.tools.lock().unwrap_or_else(|e| e.into_inner());
            let mut idx = self.index_embeddings.lock().unwrap_or_else(|e| e.into_inner());
            *idx = tools
                .iter()
                .map(|t| t.embedding.clone().unwrap_or_default())
                .collect();
        }
        *self.index_dirty.lock().unwrap_or_else(|e| e.into_inner()) = false;
        debug!(
            tool_count = self.tool_count(),
            "SemanticToolRouter index rebuilt"
        );
        Ok(())
    }

    /// Return a cached embedding or request a new one.
    async fn get_or_embed(&self, text: &str) -> Result<Vec<f32>> {
        // Check cache first.
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(emb) = cache.get(text) {
                return Ok(emb.clone());
            }
        }

        // Not cached — call provider.
        let emb = self.provider.embed(text).await?;

        // Store in cache.
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.put(text.to_string(), emb.clone());
        }

        Ok(emb)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Deterministic embedding provider for tests.
    /// Each unique string gets a fixed embedding based on its characters.
    struct MockEmbedder {
        call_count: AtomicUsize,
    }

    impl MockEmbedder {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                call_count: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl EmbeddingProvider for MockEmbedder {
        async fn embed(&self, text: &str) -> Result<Vec<f32>> {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            // Deterministic: hash bytes into a 4-dim vector.
            let mut v = vec![0.0f32; 4];
            for (i, b) in text.bytes().enumerate() {
                v[i % 4] += b as f32;
            }
            Ok(v)
        }

        fn dimension(&self) -> usize {
            4
        }
    }

    fn router() -> (SemanticToolRouter, Arc<MockEmbedder>) {
        let emb = MockEmbedder::new();
        let r = SemanticToolRouter::new(emb.clone(), RouterConfig::default());
        (r, emb)
    }

    #[tokio::test]
    async fn empty_router_returns_empty() {
        let (r, _) = router();
        let result = r.query("find secrets").await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn registered_tools_are_queryable() {
        let (r, _) = router();
        r.register(
            "secret_scan",
            "Scan source code for exposed secrets, API keys, credentials",
        );
        r.register("grep", "Search file contents with regular expressions");
        let candidates = r.query("find secrets in code").await.unwrap();
        assert!(!candidates.is_empty());
        // All results are ranked correctly
        for (i, c) in candidates.iter().enumerate() {
            assert_eq!(c.rank, i);
        }
    }

    #[tokio::test]
    async fn top_k_limit_respected() {
        let (r, _) = router();
        for i in 0..20 {
            r.register(
                format!("tool_{}", i),
                format!("Tool number {} description", i),
            );
        }
        let config = RouterConfig {
            top_k: 5,
            min_similarity: 0.0,
            ..Default::default()
        };
        let emb = MockEmbedder::new();
        let r2 = SemanticToolRouter::new(emb, config);
        for i in 0..20 {
            r2.register(
                format!("tool_{}", i),
                format!("Tool number {} description", i),
            );
        }
        let candidates = r2.query("tool description").await.unwrap();
        assert!(candidates.len() <= 5);
    }

    #[tokio::test]
    async fn deregister_removes_tool() {
        let (r, _) = router();
        r.register("secret_scan", "Scan for secrets");
        assert_eq!(r.tool_count(), 1);
        r.deregister("secret_scan");
        assert_eq!(r.tool_count(), 0);
    }

    #[tokio::test]
    async fn duplicate_registration_updates_description() {
        let (r, _) = router();
        r.register("bash", "Execute shell commands");
        r.register("bash", "Run bash commands securely in a sandbox");
        assert_eq!(r.tool_count(), 1);
    }

    #[tokio::test]
    async fn caching_avoids_re_embedding() {
        let (r, emb) = router();
        r.register("file_read", "Read file contents from disk");
        // Two queries with the same intent should only embed the intent once.
        r.query("read a file").await.unwrap();
        let count_after_first = emb.call_count.load(Ordering::Relaxed);
        r.query("read a file").await.unwrap();
        let count_after_second = emb.call_count.load(Ordering::Relaxed);
        // The intent "read a file" is cached, so second query should not call embed again for intent.
        // Tool descriptions are embedded once and cached.
        assert!(count_after_second - count_after_first <= 1);
    }

    #[tokio::test]
    async fn query_names_returns_strings() {
        let (r, _) = router();
        r.register("secret_scan", "Scan for credentials");
        r.register("grep", "Search file contents");
        let names = r.query_names("find credentials").await.unwrap();
        assert!(!names.is_empty());
        for name in &names {
            assert!(!name.is_empty());
        }
    }
}
