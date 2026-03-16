//! Embedding engine for the L3 vector semantic store.
//!
//! ## Engine hierarchy (SOTA 2026)
//!
//! 1. `OllamaEmbeddingEngine` — neural multilingual embeddings via local Ollama server.
//!    Supports nomic-embed-text, mxbai-embed-large, paraphrase-multilingual-minilm, etc.
//!    Activated automatically when `http://localhost:11434` is reachable.
//!    Language-agnostic: "crear un juego" ≈ "create a game" in the shared embedding space.
//!
//! 2. `TfIdfHashEngine` — TF-IDF hash projection (384-dim, pure Rust, zero deps).
//!    Fallback when Ollama is unavailable. English-biased but fast and dependency-free.
//!
//! Use `EmbeddingEngineFactory::best_available()` to obtain the best engine at runtime.
//!
//! ## SOTA references (2026)
//!
//! - Wang et al. (2024) "Improving Text Embeddings with Large Language Models" (E5-mistral)
//! - Muennighoff et al. (2023) "MTEB: Massive Text Embedding Benchmark"
//! - Reimers & Gurevych (2020) "Making Monolingual Sentence Embeddings Multilingual via
//!   Knowledge Distillation" — the paraphrase-multilingual-minilm family
//! - Zhang et al. (2022) "E5: Text Embeddings by Weakly-Supervised Contrastive Pre-training"
//!
//! Neural multilingual embeddings outperform token-hash projections by 15–30% mAP on
//! cross-lingual intent benchmarks (MTEB 2024, BEIR multilingual subset).

use std::time::Duration;

/// Default embedding dimensionality for TfIdfHashEngine.
/// 384 matches AllMiniLML6V2Q for drop-in upgrade.
/// Neural engines (Ollama) may return different dimensions depending on model.
pub const DIMS: usize = 384;

/// Default Ollama endpoint for local inference.
pub const OLLAMA_DEFAULT_ENDPOINT: &str = "http://localhost:11434";

/// Default multilingual model for Ollama.
/// nomic-embed-text: 768-dim, trained on 300M multilingual pairs, strong MTEB performance.
/// Alternative: "mxbai-embed-large" (1024-dim), "all-minilm:l6-v2" (384-dim, drop-in).
pub const OLLAMA_DEFAULT_MODEL: &str = "nomic-embed-text";

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

// ── OllamaEmbeddingEngine ─────────────────────────────────────────────────────

/// Neural multilingual embedding engine backed by a local Ollama server.
///
/// Calls `POST {endpoint}/api/embeddings` and returns an L2-normalized embedding
/// vector of the model's native dimensionality. The same endpoint supports any
/// model installed in Ollama — change `model` to switch between:
///
/// | Model                           | Dims | Languages | Notes                    |
/// |----------------------------------|------|-----------|--------------------------|
/// | nomic-embed-text                 |  768 | 100+      | Best multilingual balance |
/// | mxbai-embed-large                | 1024 | EN-heavy  | Highest EN MTEB score     |
/// | paraphrase-multilingual-minilm   |  384 | 50+       | Drop-in for DIMS=384      |
/// | all-minilm:l6-v2                 |  384 | EN        | Fastest, EN only          |
///
/// When the server is unreachable, `embed()` returns an empty `Vec<f32>`.
/// Use `EmbeddingEngineFactory::best_available()` to fall back gracefully.
pub struct OllamaEmbeddingEngine {
    client: reqwest::blocking::Client,
    endpoint: String,
    model: String,
}

impl OllamaEmbeddingEngine {
    /// Construct with explicit endpoint, model, and HTTP timeout.
    pub fn new(endpoint: &str, model: &str, timeout_ms: u64) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self {
            client,
            endpoint: endpoint.trim_end_matches('/').to_string(),
            model: model.to_string(),
        }
    }

    /// Probe availability: embed a short string and return dimensionality on success.
    ///
    /// Returns `None` when the server is unreachable or returns an empty vector.
    pub fn probe(&self) -> Option<usize> {
        let v = self.embed("probe");
        if !v.is_empty() { Some(v.len()) } else { None }
    }
}

impl EmbeddingEngine for OllamaEmbeddingEngine {
    fn embed(&self, text: &str) -> Vec<f32> {
        #[derive(serde::Serialize)]
        struct EmbedRequest<'a> {
            model: &'a str,
            prompt: &'a str,
        }

        #[derive(serde::Deserialize)]
        struct EmbedResponse {
            embedding: Vec<f32>,
        }

        let url = format!("{}/api/embeddings", self.endpoint);
        let req = EmbedRequest { model: &self.model, prompt: text };

        let resp = match self.client.post(&url).json(&req).send() {
            Ok(r) => r,
            Err(_) => return vec![],
        };

        let body: EmbedResponse = match resp.json() {
            Ok(b) => b,
            Err(_) => return vec![],
        };

        if body.embedding.is_empty() {
            return vec![];
        }

        // L2-normalize so cosine_sim = dot product (consistent with TfIdfHashEngine).
        let mut v = body.embedding;
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-9 {
            v.iter_mut().for_each(|x| *x /= norm);
        }
        v
    }
}

// ── EmbeddingEngineFactory ────────────────────────────────────────────────────

/// Factory for obtaining the best available embedding engine at runtime.
///
/// ## Selection policy
///
/// 1. Probe Ollama at `endpoint` with a 300 ms timeout.
/// 2. Success → return `OllamaEmbeddingEngine` with a 5 s inference timeout.
/// 3. Failure → return `TfIdfHashEngine` (pure Rust, zero deps).
///
/// The 300 ms probe is fast enough for startup: on a loopback connection refused
/// returns in < 1 ms; the full 300 ms budget is only spent on hosts with Ollama
/// installed but a model not yet loaded (rare in practice).
///
/// ## Why this matters for multilingual classification
///
/// TfIdfHashEngine uses FNV-1a hash projection over ASCII-lowercased tokens.
/// Spanish tokens like "crear", "juego", "implementar" project to completely
/// different dims than their English equivalents — the classifier must learn
/// separate prototypes per language. OllamaEmbeddingEngine uses a shared
/// multilingual space where semantically equivalent queries cluster together
/// regardless of language (MTEB multilingual BEIR avg: +18 nDCG@10 over BM25).
pub struct EmbeddingEngineFactory;

impl EmbeddingEngineFactory {
    /// Return the best available engine for the given endpoint and model.
    ///
    /// The Ollama probe is isolated in a `std::thread::spawn` to prevent panics
    /// when called from an async tokio context. `reqwest::blocking::Client` cannot
    /// be dropped inside a tokio runtime without `spawn_blocking`, but since we
    /// need to return a `Box<dyn EmbeddingEngine>` synchronously this thread-channel
    /// pattern (identical to `AnthropicLlmLayer`) is the correct solution.
    pub fn best_available(endpoint: &str, model: &str) -> Box<dyn EmbeddingEngine> {
        const PROBE_TIMEOUT_MS: u64 = 300;
        const INFERENCE_TIMEOUT_MS: u64 = 5_000;
        let endpoint_s = endpoint.to_string();
        let model_s = model.to_string();
        // Both the probe AND the final engine construction must happen inside a
        // std::thread::spawn because reqwest::blocking::Client cannot be created
        // or dropped inside a Tokio async runtime.
        let (tx, rx) = std::sync::mpsc::channel::<Option<Box<dyn EmbeddingEngine>>>();
        std::thread::spawn(move || {
            let probe = OllamaEmbeddingEngine::new(&endpoint_s, &model_s, PROBE_TIMEOUT_MS);
            if probe.probe().is_some() {
                // Probe succeeded — create the full-timeout engine in this thread.
                let engine: Box<dyn EmbeddingEngine> =
                    Box::new(OllamaEmbeddingEngine::new(&endpoint_s, &model_s, INFERENCE_TIMEOUT_MS));
                let _ = tx.send(Some(engine));
            } else {
                let _ = tx.send(None);
            }
            // All reqwest::blocking resources dropped here — safe, not in async context.
        });
        match rx
            .recv_timeout(std::time::Duration::from_millis(PROBE_TIMEOUT_MS + 100))
            .ok()
            .flatten()
        {
            Some(engine) => {
                tracing::info!(
                    target: "halcon::embedding",
                    endpoint = endpoint,
                    model = model,
                    "OllamaEmbeddingEngine available — multilingual mode active"
                );
                engine
            }
            None => {
                tracing::debug!(
                    target: "halcon::embedding",
                    endpoint = endpoint,
                    "Ollama unavailable — falling back to TfIdfHashEngine"
                );
                Box::new(TfIdfHashEngine)
            }
        }
    }

    /// Probe the default local Ollama instance with the default model.
    ///
    /// Equivalent to `best_available(OLLAMA_DEFAULT_ENDPOINT, OLLAMA_DEFAULT_MODEL)`.
    pub fn default_local() -> Box<dyn EmbeddingEngine> {
        Self::best_available(OLLAMA_DEFAULT_ENDPOINT, OLLAMA_DEFAULT_MODEL)
    }

    /// Select the best engine, honouring environment variable overrides.
    ///
    /// Resolution order:
    /// 1. `HALCON_EMBEDDING_ENDPOINT` + `HALCON_EMBEDDING_MODEL` — explicit Halcon overrides
    /// 2. `OLLAMA_HOST` — Ollama CLI convention (e.g. `http://gpu-server:11434`)
    /// 3. `OLLAMA_DEFAULT_ENDPOINT` / `OLLAMA_DEFAULT_MODEL` — compile-time defaults
    ///
    /// This is the **canonical factory method** for all subsystems.
    /// Prefer `from_env()` over `default_local()` to support remote and air-gapped deployments.
    pub fn from_env() -> Box<dyn EmbeddingEngine> {
        let endpoint = std::env::var("HALCON_EMBEDDING_ENDPOINT")
            .or_else(|_| std::env::var("OLLAMA_HOST"))
            .unwrap_or_else(|_| OLLAMA_DEFAULT_ENDPOINT.to_string());
        let model = std::env::var("HALCON_EMBEDDING_MODEL")
            .unwrap_or_else(|_| OLLAMA_DEFAULT_MODEL.to_string());
        Self::best_available(&endpoint, &model)
    }

    /// Select the best engine with explicit config, still honouring env var overrides.
    ///
    /// Priority (highest first):
    /// 1. `HALCON_EMBEDDING_ENDPOINT` / `HALCON_EMBEDDING_MODEL` env vars
    /// 2. `OLLAMA_HOST` env var
    /// 3. `endpoint` / `model` parameters (from PolicyConfig or caller)
    ///
    /// Use this when the caller has policy-level defaults (e.g., halcon-cli agent loop
    /// reads `policy.embedding_endpoint` and passes it here, letting env vars still win).
    pub fn with_config(endpoint: &str, model: &str) -> Box<dyn EmbeddingEngine> {
        let resolved_endpoint = std::env::var("HALCON_EMBEDDING_ENDPOINT")
            .or_else(|_| std::env::var("OLLAMA_HOST"))
            .unwrap_or_else(|_| endpoint.to_string());
        let resolved_model = std::env::var("HALCON_EMBEDDING_MODEL")
            .unwrap_or_else(|_| model.to_string());
        Self::best_available(&resolved_endpoint, &resolved_model)
    }
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
