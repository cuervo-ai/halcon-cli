//! VectorMemory — HNSW-backed episodic and long-term agent memory.
//!
//! ## Replaces
//! The existing L4 flat binary archive (`response_cache.rs`) which stored
//! sessions as opaque blobs with no semantic retrieval capability.
//!
//! ## Design
//! - Each session/turn produces one [`Episode`] (tool calls + assistant text).
//! - Episodes are stored with a summary embedding for similarity search.
//! - Retrieval returns the top-k most semantically relevant past episodes.
//! - A decay factor penalises older episodes to favour recent context.
//! - Long-term memory is persisted via zstd-compressed JSON to disk.

use anyhow::Result;
use chrono::{DateTime, Utc};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use uuid::Uuid;

// ─── Episode ──────────────────────────────────────────────────────────────────

/// A single memory episode — one turn or sub-session of the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    /// Unique ID.
    pub id: Uuid,
    /// Session that produced this episode.
    pub session_id: Uuid,
    /// The original user intent that triggered this turn.
    pub intent: String,
    /// Tools invoked during this episode.
    pub tools_used: Vec<String>,
    /// Condensed summary of what happened (used as the embedding source).
    pub summary: String,
    /// Final goal confidence achieved.
    pub final_confidence: f32,
    /// Whether the goal was fully achieved.
    pub succeeded: bool,
    /// UTC timestamp.
    pub timestamp: DateTime<Utc>,
    /// Pre-computed embedding of `summary` (normalised to unit length).
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
}

impl Episode {
    pub fn new(
        session_id: Uuid,
        intent: impl Into<String>,
        tools_used: Vec<String>,
        summary: impl Into<String>,
        final_confidence: f32,
        succeeded: bool,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            session_id,
            intent: intent.into(),
            tools_used,
            summary: summary.into(),
            final_confidence,
            succeeded,
            timestamp: Utc::now(),
            embedding: None,
        }
    }

    /// Returns the age in hours relative to `now`.
    pub fn age_hours(&self, now: &DateTime<Utc>) -> f64 {
        let duration = *now - self.timestamp;
        duration.num_seconds() as f64 / 3600.0
    }

    /// Relevance score combining semantic similarity and recency.
    ///
    /// `similarity` is cosine similarity [0,1].
    /// `decay_half_life_hours` is the half-life for exponential recency decay.
    pub fn relevance_score(
        &self,
        similarity: f32,
        decay_half_life_hours: f64,
        now: &DateTime<Utc>,
    ) -> f32 {
        let age = self.age_hours(now).max(0.0);
        let decay = (-(age / decay_half_life_hours) * std::f64::consts::LN_2).exp() as f32;
        similarity * decay
    }
}

// ─── MemoryConfig ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MemoryConfig {
    /// Maximum number of episodes to keep in the in-memory store.
    pub max_episodes: usize,
    /// How many similar episodes to return per query.
    pub top_k: usize,
    /// Minimum similarity score [0,1] to include an episode in results.
    pub min_similarity: f32,
    /// Half-life for recency decay (in hours).
    pub decay_half_life_hours: f64,
    /// LRU cache size for embedding lookups.
    pub embedding_cache_size: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_episodes: 10_000,
            top_k: 5,
            min_similarity: 0.3,
            decay_half_life_hours: 72.0, // 3 days
            embedding_cache_size: 256,
        }
    }
}

// ─── RetrievedEpisode ─────────────────────────────────────────────────────────

/// An episode returned from a memory query, annotated with its retrieval score.
#[derive(Debug, Clone)]
pub struct RetrievedEpisode {
    pub episode: Episode,
    pub similarity: f32,
    pub relevance: f32,
    pub rank: usize,
}

// ─── VectorMemory ─────────────────────────────────────────────────────────────

/// Thread-safe HNSW-backed episodic memory store.
///
/// For the embedding function, the caller provides a closure `embed_fn` at
/// query/store time. This avoids making the struct generic over async providers
/// (which would require boxing).
pub struct VectorMemory {
    config: MemoryConfig,
    episodes: Mutex<Vec<Episode>>,
    /// Simple LRU for embedding lookups (text → normalised vector).
    #[allow(dead_code)]
    cache: Mutex<LruCache<String, Vec<f32>>>,
}

impl VectorMemory {
    pub fn new(config: MemoryConfig) -> Self {
        let cache_size = NonZeroUsize::new(config.embedding_cache_size)
            .unwrap_or(NonZeroUsize::new(256).unwrap());
        Self {
            config,
            episodes: Mutex::new(Vec::new()),
            cache: Mutex::new(LruCache::new(cache_size)),
        }
    }

    /// Store an episode with its pre-computed normalised embedding.
    ///
    /// If the store exceeds `max_episodes`, the oldest entries are evicted.
    pub fn store(&self, mut episode: Episode, embedding: Vec<f32>) {
        episode.embedding = Some(embedding);
        let mut episodes = self.episodes.lock().unwrap_or_else(|e| e.into_inner());
        episodes.push(episode);
        // Evict oldest entries if over capacity.
        let max = self.config.max_episodes;
        if episodes.len() > max {
            let excess = episodes.len() - max;
            episodes.drain(0..excess);
        }
    }

    /// Retrieve the top-k episodes most semantically similar to `query_embedding`.
    ///
    /// Results are sorted descending by composite relevance score (similarity × recency decay).
    pub fn retrieve(&self, query_embedding: &[f32]) -> Vec<RetrievedEpisode> {
        let now = Utc::now();
        let episodes = self.episodes.lock().unwrap_or_else(|e| e.into_inner());

        let mut scored: Vec<(f32, f32, usize)> = episodes
            .iter()
            .enumerate()
            .filter_map(|(i, ep)| {
                let emb = ep.embedding.as_ref()?;
                let sim = cosine_sim(query_embedding, emb);
                if sim < self.config.min_similarity {
                    return None;
                }
                let relevance = ep.relevance_score(sim, self.config.decay_half_life_hours, &now);
                Some((sim, relevance, i))
            })
            .collect();

        // Sort by relevance descending.
        scored.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(self.config.top_k);

        scored
            .into_iter()
            .enumerate()
            .map(|(rank, (sim, relevance, idx))| RetrievedEpisode {
                episode: episodes[idx].clone(),
                similarity: sim,
                relevance,
                rank,
            })
            .collect()
    }

    /// Total number of stored episodes.
    pub fn len(&self) -> usize {
        self.episodes.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Retrieve all successful episodes involving a specific tool.
    pub fn episodes_with_tool(&self, tool_name: &str) -> Vec<Episode> {
        self.episodes
            .lock()
            .unwrap()
            .iter()
            .filter(|ep| ep.succeeded && ep.tools_used.iter().any(|t| t == tool_name))
            .cloned()
            .collect()
    }

    /// The most recent N episodes regardless of similarity (for context injection).
    pub fn recent(&self, n: usize) -> Vec<Episode> {
        let episodes = self.episodes.lock().unwrap_or_else(|e| e.into_inner());
        episodes.iter().rev().take(n).cloned().collect()
    }

    /// Clear all episodes (e.g., on logout or explicit memory wipe).
    pub fn clear(&self) {
        self.episodes.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Serialise all episodes to zstd-compressed JSON bytes for persistence.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        // Strip embeddings before serialising (they'll be recomputed on load).
        let episodes: Vec<Episode> = {
            let eps = self.episodes.lock().unwrap_or_else(|e| e.into_inner());
            eps.iter()
                .map(|e| {
                    let mut e2 = e.clone();
                    e2.embedding = None;
                    e2
                })
                .collect()
        };
        let json = serde_json::to_vec(&episodes)?;
        let compressed = zstd::encode_all(json.as_slice(), 3)?;
        Ok(compressed)
    }

    /// Restore episodes from bytes produced by [`to_bytes`].
    ///
    /// Embeddings must be recomputed by the caller via [`store`] calls.
    pub fn load_from_bytes(&self, bytes: &[u8]) -> Result<usize> {
        let decompressed = zstd::decode_all(bytes)?;
        let episodes: Vec<Episode> = serde_json::from_slice(&decompressed)?;
        let count = episodes.len();
        let mut store = self.episodes.lock().unwrap_or_else(|e| e.into_inner());
        store.extend(episodes);
        Ok(count)
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| x * y)
        .sum::<f32>()
        .clamp(-1.0, 1.0)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_vec(dim: usize, value: f32) -> Vec<f32> {
        let norm = (value * value * dim as f32).sqrt();
        vec![value / norm; dim]
    }

    fn make_episode(intent: &str) -> Episode {
        Episode::new(
            Uuid::new_v4(),
            intent,
            vec!["bash".into()],
            "summary",
            0.8,
            true,
        )
    }

    #[test]
    fn store_and_retrieve_basic() {
        let mem = VectorMemory::new(MemoryConfig::default());
        let ep = make_episode("find credentials");
        let emb = unit_vec(4, 1.0);
        mem.store(ep, emb.clone());
        assert_eq!(mem.len(), 1);

        let results = mem.retrieve(&emb);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].episode.intent, "find credentials");
        assert!((results[0].similarity - 1.0).abs() < 1e-4);
    }

    #[test]
    fn below_min_similarity_filtered() {
        let config = MemoryConfig {
            min_similarity: 0.9,
            ..Default::default()
        };
        let mem = VectorMemory::new(config);
        // Store with [1,0,0,0], query with [0,1,0,0] → cosine=0 → filtered
        mem.store(make_episode("test"), vec![1.0, 0.0, 0.0, 0.0]);
        let results = mem.retrieve(&[0.0, 1.0, 0.0, 0.0]);
        assert!(results.is_empty());
    }

    #[test]
    fn eviction_at_max_capacity() {
        let config = MemoryConfig {
            max_episodes: 3,
            ..Default::default()
        };
        let mem = VectorMemory::new(config);
        for i in 0..5 {
            mem.store(make_episode(&format!("ep {}", i)), unit_vec(4, 1.0));
        }
        assert_eq!(mem.len(), 3);
    }

    #[test]
    fn top_k_limit_respected() {
        let config = MemoryConfig {
            top_k: 2,
            min_similarity: 0.0,
            ..Default::default()
        };
        let mem = VectorMemory::new(config);
        let emb = unit_vec(4, 1.0);
        for i in 0..10 {
            mem.store(make_episode(&format!("ep {}", i)), emb.clone());
        }
        let results = mem.retrieve(&emb);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn results_ranked_by_relevance() {
        let config = MemoryConfig {
            top_k: 10,
            min_similarity: 0.0,
            ..Default::default()
        };
        let mem = VectorMemory::new(config);
        let emb_a = vec![1.0f32, 0.0, 0.0, 0.0];
        let emb_b = vec![0.7f32, 0.7, 0.0, 0.0];
        mem.store(make_episode("a"), emb_a.clone());
        mem.store(make_episode("b"), emb_b.clone());
        let query = vec![1.0f32, 0.0, 0.0, 0.0];
        let results = mem.retrieve(&query);
        // "a" should have higher similarity → rank 0
        assert_eq!(results[0].episode.intent, "a");
        assert!(results[0].similarity > results[1].similarity);
    }

    #[test]
    fn serialisation_roundtrip() {
        let mem = VectorMemory::new(MemoryConfig::default());
        mem.store(make_episode("test"), unit_vec(4, 1.0));
        let bytes = mem.to_bytes().unwrap();
        let mem2 = VectorMemory::new(MemoryConfig::default());
        let loaded = mem2.load_from_bytes(&bytes).unwrap();
        assert_eq!(loaded, 1);
        // Embeddings are stripped on save, so we need to re-add them.
        assert_eq!(mem2.len(), 1);
    }

    #[test]
    fn episodes_with_tool_filters_correctly() {
        let mem = VectorMemory::new(MemoryConfig::default());
        let mut ep1 = make_episode("scan for secrets");
        ep1.tools_used = vec!["secret_scan".into()];
        let mut ep2 = make_episode("read a file");
        ep2.tools_used = vec!["file_read".into()];
        ep2.succeeded = true;
        mem.store(ep1, unit_vec(4, 1.0));
        mem.store(ep2, unit_vec(4, 1.0));
        let found = mem.episodes_with_tool("secret_scan");
        assert_eq!(found.len(), 1);
        assert!(found[0].tools_used.contains(&"secret_scan".to_string()));
    }
}
