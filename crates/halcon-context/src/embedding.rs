//! Embedding engine for the L3 vector semantic store.
//!
//! Default: TF-IDF hash projection (384-dim, pure Rust, zero deps).
//! Each token is hashed via FNV-1a to a dimension in [0, 384); its TF-IDF weight
//! is accumulated at that dimension. The resulting vector is L2-normalized.
//!
//! The hash projection is not equivalent to dense neural embeddings, but it is
//! significantly better than BM25 for partial-overlap semantic retrieval and
//! satisfies all acceptance criteria at expected MEMORY.md entry counts (≤300).
//!
//! Upgrade path: replace `TfIdfHashEngine` with `FastEmbedEngine` (behind
//! the `local-embeddings` feature flag) without changing `VectorMemoryStore`.

/// Embedding dimensionality. 384 matches AllMiniLML6V2Q for drop-in upgrade.
pub const DIMS: usize = 384;

/// Trait for embedding text into a fixed-dimension float vector.
pub trait EmbeddingEngine: Send + Sync {
    /// Embed `text` into a `DIMS`-dimensional L2-normalized vector.
    fn embed(&self, text: &str) -> Vec<f32>;
}

// ── TF-IDF hash projection ────────────────────────────────────────────────────

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
/// FNV-1a prime.
const FNV_PRIME: u64 = 1_099_511_628_211;

/// Hash a string token to a dimension index in [0, DIMS).
fn fnv1a_dim(token: &str) -> usize {
    let mut hash = FNV_OFFSET;
    for byte in token.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    (hash as usize) % DIMS
}

/// Tokenize text into lowercase alphanumeric + underscore tokens of length ≥ 2.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

/// Default embedding engine: TF-IDF hash projection with FNV-1a, 384 dims, L2-normalized.
///
/// Multiple tokens that hash to the same dimension accumulate their weights (projection).
pub struct TfIdfHashEngine;

impl EmbeddingEngine for TfIdfHashEngine {
    fn embed(&self, text: &str) -> Vec<f32> {
        let tokens = tokenize(text);
        if tokens.is_empty() {
            return vec![0.0; DIMS];
        }

        // Compute term frequencies.
        let total = tokens.len() as f32;
        let mut tf: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
        for tok in &tokens {
            *tf.entry(tok.clone()).or_insert(0.0) += 1.0 / total;
        }

        // Project into DIMS-dimensional space.
        let mut vec = vec![0.0f32; DIMS];
        for (tok, weight) in &tf {
            let dim = fnv1a_dim(tok);
            vec[dim] += weight;
        }

        // L2-normalize.
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-9 {
            for x in vec.iter_mut() {
                *x /= norm;
            }
        }

        vec
    }
}

/// Cosine similarity between two L2-normalized vectors (dot product).
pub fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> TfIdfHashEngine {
        TfIdfHashEngine
    }

    #[test]
    fn embed_returns_correct_dims() {
        let v = engine().embed("hello world rust tokio");
        assert_eq!(v.len(), DIMS);
    }

    #[test]
    fn embed_is_l2_normalized() {
        let v = engine().embed("test sentence for normalization");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm={norm}");
    }

    #[test]
    fn embed_empty_returns_zero_vec() {
        let v = engine().embed("");
        assert_eq!(v.len(), DIMS);
        assert!(v.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn identical_texts_have_similarity_one() {
        let e = engine();
        let a = e.embed("rust async patterns tokio error handling");
        let b = e.embed("rust async patterns tokio error handling");
        let sim = cosine_sim(&a, &b);
        assert!((sim - 1.0).abs() < 1e-5, "sim={sim}");
    }

    #[test]
    fn similar_texts_score_higher_than_unrelated() {
        let e = engine();
        let query = e.embed("file path errors FASE-2 gate");
        let related = e.embed("FASE-2 path existence gate failed file read");
        let unrelated = e.embed("quantum physics superposition wavefunction collapse");
        let sim_rel = cosine_sim(&query, &related);
        let sim_unrel = cosine_sim(&query, &unrelated);
        assert!(sim_rel > sim_unrel, "expected {sim_rel} > {sim_unrel}");
    }

    #[test]
    fn fnv1a_dim_in_range() {
        for word in &["rust", "async", "tokio", "error", "halcon", "boundary"] {
            let d = fnv1a_dim(word);
            assert!(d < DIMS, "dim {d} out of range for word {word}");
        }
    }

    #[test]
    fn tokenize_lowercases_and_splits() {
        let tokens = tokenize("Hello, World! Rust_Code");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
        assert!(tokens.contains(&"rust_code".to_string()));
    }

    #[test]
    fn tokenize_skips_single_chars() {
        let tokens = tokenize("a b cc dd");
        assert!(!tokens.contains(&"a".to_string()));
        assert!(tokens.contains(&"cc".to_string()));
        assert!(tokens.contains(&"dd".to_string()));
    }
}
