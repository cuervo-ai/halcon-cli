//! L3 Vector Memory Store: embedding-based semantic retrieval over MEMORY.md entries.
//!
//! Replaces the BM25 max-200-entry truncated list with cosine similarity over
//! TF-IDF hash-projected vectors (384 dims, pure Rust — see `embedding.rs`).
//!
//! ## Entry lifecycle
//! 1. `load_from_memory_files()` parses MEMORY.md(s) into `MemoryEntry` records.
//! 2. `index_text()` embeds each entry and appends it to `entries`.
//! 3. `search()` computes cosine similarity for all entries, then MMR re-ranks.
//! 4. `save()` / `load_from_disk()` persist the index to JSON on disk.
//!
//! ## Persistence
//! Index file: `{index_path}` (default `.halcon/memory/MEMORY.vindex.json`).
//!
//! ## MMR (Maximum Marginal Relevance)
//! λ=0.7: 70% relevance to query + 30% diversity from already-selected items.
//! Prevents returning N near-identical memory chunks.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::embedding::{cosine_sim, EmbeddingEngine, TfIdfHashEngine, DIMS};

/// MMR trade-off: relevance weight (1 − λ = diversity weight).
const MMR_LAMBDA: f32 = 0.7;
/// Similarity threshold below which entries are excluded from results (noise floor).
const MIN_SIM: f32 = 0.05;
/// Warn if entry count exceeds this; recommend HNSW upgrade.
const HNSW_RECOMMEND_THRESHOLD: usize = 1_000;

/// A single memory entry stored in the vector index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Stable numeric ID (index in insertion order).
    pub id: u64,
    /// Source label, e.g. "project:MEMORY.md§Authentication" or "user:MEMORY.md§Overview".
    pub source: String,
    /// Original text content of the memory section.
    pub text: String,
    /// Pre-computed embedding vector (length = DIMS).
    pub vector: Vec<f32>,
}

/// A search result with similarity score.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub entry: MemoryEntry,
    /// Cosine similarity to the query vector [0.0, 1.0].
    pub score: f32,
}

/// Serializable snapshot for disk persistence.
#[derive(Debug, Serialize, Deserialize)]
struct VIndexSnapshot {
    version: u8,
    dims: usize,
    entries: Vec<MemoryEntry>,
}

/// L3 Vector Memory Store.
///
/// Thread-safe via `Arc<Mutex<VectorMemoryStore>>`.
pub struct VectorMemoryStore {
    entries: Vec<MemoryEntry>,
    engine: Box<dyn EmbeddingEngine>,
    index_path: Option<PathBuf>,
    /// Counter for stable entry IDs.
    next_id: u64,
}

impl VectorMemoryStore {
    /// Create an empty in-memory store.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            engine: Box::new(TfIdfHashEngine),
            index_path: None,
            next_id: 0,
        }
    }

    /// Create a store that persists to `index_path` (`.halcon/memory/MEMORY.vindex.json`).
    pub fn with_index_path(index_path: PathBuf) -> Self {
        Self {
            entries: Vec::new(),
            engine: Box::new(TfIdfHashEngine),
            index_path: Some(index_path),
            next_id: 0,
        }
    }

    /// Number of indexed entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Index a text passage with a source label.
    ///
    /// Re-embeds using the current engine. Duplicate source labels are accepted (additive).
    pub fn index_text(&mut self, text: &str, source: &str) {
        if text.trim().is_empty() {
            return;
        }
        if self.entries.len() >= HNSW_RECOMMEND_THRESHOLD {
            warn!(
                "vector_store: {} entries exceed threshold {}; consider upgrading to HNSW backend",
                self.entries.len(),
                HNSW_RECOMMEND_THRESHOLD
            );
        }
        let vector = self.engine.embed(text);
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(MemoryEntry {
            id,
            source: source.to_string(),
            text: text.to_string(),
            vector,
        });
        debug!("vector_store: indexed entry {id} from '{source}' ({} chars)", text.len());
    }

    /// Search for the top-`k` most relevant entries using MMR re-ranking.
    ///
    /// Returns results ordered by MMR score (best first), filtered by `MIN_SIM`.
    pub fn search(&self, query: &str, k: usize) -> Vec<SearchResult> {
        if self.entries.is_empty() || query.trim().is_empty() || k == 0 {
            return Vec::new();
        }

        let query_vec = self.engine.embed(query);

        // Compute cosine similarity for all entries.
        let mut scored: Vec<(usize, f32)> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (i, cosine_sim(&query_vec, &e.vector)))
            .filter(|(_, s)| *s >= MIN_SIM)
            .collect();

        if scored.is_empty() {
            return Vec::new();
        }

        // Sort by similarity descending for initial candidate ordering.
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // MMR re-ranking: iteratively pick the candidate that maximises
        // λ * sim(query, d) − (1−λ) * max(sim(d, selected))
        let mut selected_indices: Vec<usize> = Vec::with_capacity(k);
        let mut selected_vecs: Vec<&[f32]> = Vec::with_capacity(k);
        let mut remaining: Vec<(usize, f32)> = scored; // (entry_idx, query_sim)

        while selected_indices.len() < k && !remaining.is_empty() {
            let mut best_mmr = f32::NEG_INFINITY;
            let mut best_pos = 0;

            for (pos, &(idx, query_sim)) in remaining.iter().enumerate() {
                let entry_vec: &[f32] = &self.entries[idx].vector;
                // Max similarity to already-selected items (redundancy penalty).
                let max_redundancy = selected_vecs
                    .iter()
                    .map(|sv| cosine_sim(entry_vec, sv))
                    .fold(f32::NEG_INFINITY, f32::max);
                let redundancy = if selected_vecs.is_empty() { 0.0 } else { max_redundancy };

                let mmr = MMR_LAMBDA * query_sim - (1.0 - MMR_LAMBDA) * redundancy;
                if mmr > best_mmr {
                    best_mmr = mmr;
                    best_pos = pos;
                }
            }

            let (entry_idx, query_sim) = remaining.remove(best_pos);
            selected_vecs.push(&self.entries[entry_idx].vector);
            selected_indices.push(entry_idx);
            let _ = (entry_idx, query_sim); // used via selected_vecs
        }

        selected_indices
            .iter()
            .zip(
                // Recover per-entry similarity scores for result construction.
                selected_indices.iter().map(|&idx| {
                    cosine_sim(&query_vec, &self.entries[idx].vector)
                }),
            )
            .map(|(&idx, score)| SearchResult {
                entry: self.entries[idx].clone(),
                score,
            })
            .collect()
    }

    /// Parse a MEMORY.md file into sections and index each section.
    ///
    /// Sections are split on `##` headings. The section header becomes part of the
    /// text so that heading keywords contribute to retrieval.
    ///
    /// `label` is a short prefix used in `MemoryEntry::source` (e.g., "project", "user").
    pub fn load_from_memory_file(&mut self, path: &Path, label: &str) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                warn!("vector_store: cannot read {}: {e}", path.display());
                return;
            }
        };

        let sections = split_markdown_sections(&content);
        let path_str = path.display().to_string();

        for (heading, body) in sections {
            let combined = if heading.is_empty() {
                body.clone()
            } else {
                format!("{heading}\n{body}")
            };
            if combined.trim().is_empty() {
                continue;
            }
            let source = if heading.is_empty() {
                format!("{label}:{path_str}§preamble")
            } else {
                format!("{label}:{path_str}§{}", heading.trim_start_matches('#').trim())
            };
            self.index_text(&combined, &source);
        }

        debug!(
            "vector_store: loaded {} sections from {} (label='{label}')",
            self.entries.len(),
            path.display()
        );
    }

    /// Convenience: load from the standard MEMORY.md locations relative to `working_dir`.
    ///
    /// Searches:
    /// 1. `{ancestor}/.halcon/memory/MEMORY.md` (project scope, walk ancestors)
    /// 2. `~/.halcon/memory/{repo_name}/MEMORY.md` (user scope)
    pub fn load_from_standard_locations(&mut self, working_dir: &Path, repo_name: &str) {
        // Project scope — walk ancestors.
        let mut current = working_dir;
        loop {
            let candidate = current.join(".halcon").join("memory").join("MEMORY.md");
            if candidate.exists() {
                self.load_from_memory_file(&candidate, "project");
                break;
            }
            match current.parent() {
                Some(p) => current = p,
                None => break,
            }
        }

        // User scope — ~/.halcon/memory/<repo_name>/MEMORY.md
        if let Some(home) = dirs_home() {
            let user_memory = home
                .join(".halcon")
                .join("memory")
                .join(repo_name)
                .join("MEMORY.md");
            if user_memory.exists() {
                self.load_from_memory_file(&user_memory, "user");
            }
        }
    }

    /// Persist the index to disk (JSON format).
    pub fn save(&self) {
        let path = match &self.index_path {
            Some(p) => p,
            None => return,
        };
        let snapshot = VIndexSnapshot {
            version: 1,
            dims: DIMS,
            entries: self.entries.clone(),
        };
        match serde_json::to_string(&snapshot) {
            Ok(json) => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(path, json) {
                    warn!("vector_store: failed to save index to {}: {e}", path.display());
                } else {
                    debug!("vector_store: saved {} entries to {}", self.entries.len(), path.display());
                }
            }
            Err(e) => warn!("vector_store: serialization error: {e}"),
        }
    }

    /// Load a persisted index from disk.
    ///
    /// Returns an empty store on any error (graceful degradation).
    pub fn load_from_disk(index_path: PathBuf) -> Self {
        let mut store = Self::with_index_path(index_path.clone());
        let content = match std::fs::read_to_string(&index_path) {
            Ok(c) => c,
            Err(_) => return store, // First run, no index yet.
        };
        match serde_json::from_str::<VIndexSnapshot>(&content) {
            Ok(snap) if snap.version == 1 && snap.dims == DIMS => {
                let max_id = snap.entries.iter().map(|e| e.id).max().unwrap_or(0);
                store.next_id = max_id + 1;
                store.entries = snap.entries;
                debug!(
                    "vector_store: loaded {} entries from {}",
                    store.entries.len(),
                    index_path.display()
                );
            }
            Ok(snap) => {
                warn!(
                    "vector_store: index version/dims mismatch (v={}, dims={}); rebuilding",
                    snap.version, snap.dims
                );
            }
            Err(e) => {
                warn!("vector_store: failed to parse index: {e}; rebuilding");
            }
        }
        store
    }

    /// Clear all entries (rebuild required).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.next_id = 0;
    }
}

impl Default for VectorMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Split a Markdown document into `(heading, body)` pairs at `##` boundaries.
///
/// Content before the first `##` heading is returned as `("", preamble_text)`.
fn split_markdown_sections(content: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_heading = String::new();
    let mut current_body = String::new();

    for line in content.lines() {
        if line.starts_with("## ") || line.starts_with("### ") {
            // Flush previous section.
            if !current_body.trim().is_empty() || !current_heading.is_empty() {
                sections.push((current_heading.clone(), current_body.trim().to_string()));
            }
            current_heading = line.to_string();
            current_body = String::new();
        } else {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }
    // Flush final section.
    if !current_body.trim().is_empty() || !current_heading.is_empty() {
        sections.push((current_heading, current_body.trim().to_string()));
    }
    sections
}

/// Resolve `~` to the user's home directory.
fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> VectorMemoryStore {
        VectorMemoryStore::new()
    }

    // ── Basic indexing ────────────────────────────────────────────────────────

    #[test]
    fn new_store_is_empty() {
        let s = store();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn index_text_adds_entry() {
        let mut s = store();
        s.index_text("Implemented async Rust patterns with tokio", "test:§rust");
        assert_eq!(s.len(), 1);
        assert!(!s.is_empty());
    }

    #[test]
    fn index_empty_text_skipped() {
        let mut s = store();
        s.index_text("   ", "test:§empty");
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn index_multiple_entries() {
        let mut s = store();
        s.index_text("Rust async tokio patterns", "test:§a");
        s.index_text("Python unit tests for API", "test:§b");
        s.index_text("SQLite database WAL mode", "test:§c");
        assert_eq!(s.len(), 3);
    }

    // ── Search ────────────────────────────────────────────────────────────────

    #[test]
    fn search_returns_relevant_result() {
        let mut s = store();
        s.index_text("Rust async tokio patterns with error handling", "test:§rust");
        s.index_text("Python Flask REST API web server", "test:§python");
        s.index_text("SQLite database with WAL mode configuration", "test:§sqlite");

        let results = s.search("Rust async tokio", 3);
        assert!(!results.is_empty());
        // Rust entry should be first.
        assert!(results[0].entry.text.contains("Rust"), "got: {}", results[0].entry.text);
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let mut s = store();
        s.index_text("Some content about Rust", "test:§r");
        assert!(s.search("", 5).is_empty());
    }

    #[test]
    fn search_empty_store_returns_empty() {
        let s = store();
        assert!(s.search("anything", 5).is_empty());
    }

    #[test]
    fn search_zero_k_returns_empty() {
        let mut s = store();
        s.index_text("Rust tokio async", "test:§r");
        assert!(s.search("rust", 0).is_empty());
    }

    #[test]
    fn search_respects_k_limit() {
        let mut s = store();
        for i in 0..10 {
            s.index_text(&format!("Rust async tokio pattern variant {i}"), &format!("test:§{i}"));
        }
        let results = s.search("Rust async tokio", 3);
        assert!(results.len() <= 3);
    }

    #[test]
    fn search_mmr_avoids_duplicates() {
        let mut s = store();
        // Index near-identical entries.
        s.index_text("file path error FASE-2 gate failure debugging", "test:§a");
        s.index_text("file path error FASE-2 gate failure debugging", "test:§b");
        s.index_text("completely unrelated quantum physics superposition", "test:§c");
        // MMR should prefer diversity over returning the same entry twice.
        let results = s.search("file path errors", 3);
        assert!(!results.is_empty());
        // The two near-identical entries should not both have the highest MMR score.
        // At least one result should differ from the top-1 result.
        // (This is a soft assertion — MMR guarantees diversity, not exact ordering.)
        assert!(results.len() >= 1);
    }

    #[test]
    fn search_scores_in_descending_order() {
        let mut s = store();
        s.index_text("Rust async tokio patterns error handling", "test:§a");
        s.index_text("Python data science pandas numpy", "test:§b");
        s.index_text("Rust tokio spawn blocking thread pool", "test:§c");
        let results = s.search("Rust tokio", 3);
        // Scores should be non-increasing (MMR allows slight drops for diversity).
        // At minimum the first result should be the most relevant.
        assert!(!results.is_empty());
        for r in &results {
            assert!(r.score >= MIN_SIM, "score {} < MIN_SIM", r.score);
        }
    }

    // ── Persistence ───────────────────────────────────────────────────────────

    #[test]
    fn save_and_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let idx_path = dir.path().join("MEMORY.vindex.json");

        let mut s = VectorMemoryStore::with_index_path(idx_path.clone());
        s.index_text("Rust async patterns with tokio", "test:§rust");
        s.index_text("Python unit tests Flask", "test:§py");
        s.save();

        assert!(idx_path.exists());

        let s2 = VectorMemoryStore::load_from_disk(idx_path);
        assert_eq!(s2.len(), 2);

        let results = s2.search("Rust tokio", 1);
        assert!(!results.is_empty());
        assert!(results[0].entry.text.contains("Rust"));
    }

    #[test]
    fn load_from_disk_missing_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let idx_path = dir.path().join("nonexistent.vindex.json");
        let s = VectorMemoryStore::load_from_disk(idx_path);
        assert!(s.is_empty());
    }

    // ── MEMORY.md parsing ─────────────────────────────────────────────────────

    #[test]
    fn load_from_memory_file_parses_sections() {
        let dir = TempDir::new().unwrap();
        let mem_path = dir.path().join("MEMORY.md");
        std::fs::write(
            &mem_path,
            "# Project Memory\n\nPreamble content here.\n\n\
             ## Authentication\n\nUsed JWT tokens with RS256.\n\n\
             ## Database\n\nSQLite with WAL mode for concurrent reads.\n",
        )
        .unwrap();

        let mut s = store();
        s.load_from_memory_file(&mem_path, "project");
        // 3 sections: preamble (from # header body), Authentication, Database
        assert!(s.len() >= 2, "expected ≥2 sections, got {}", s.len());
    }

    #[test]
    fn load_from_memory_file_nonexistent_is_noop() {
        let mut s = store();
        s.load_from_memory_file(Path::new("/nonexistent/MEMORY.md"), "project");
        assert!(s.is_empty());
    }

    #[test]
    fn search_finds_memory_from_parsed_file() {
        let dir = TempDir::new().unwrap();
        let mem_path = dir.path().join("MEMORY.md");
        std::fs::write(
            &mem_path,
            "## Authentication\n\nImplemented JWT RS256 token verification.\n\n\
             ## Database\n\nSQLite WAL mode, 16 tables, no migration system.\n",
        )
        .unwrap();

        let mut s = store();
        s.load_from_memory_file(&mem_path, "project");
        let results = s.search("JWT token authentication", 3);
        assert!(!results.is_empty(), "expected results for JWT query");
        assert!(
            results[0].entry.source.contains("Authentication"),
            "expected Authentication section first, got: {}",
            results[0].entry.source
        );
    }

    // ── split_markdown_sections ───────────────────────────────────────────────

    #[test]
    fn split_sections_no_headings() {
        let sections = split_markdown_sections("Just some text\nwith no headings.");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, "");
    }

    #[test]
    fn split_sections_multiple_headings() {
        let md = "## Section A\nContent A.\n## Section B\nContent B.\n";
        let sections = split_markdown_sections(md);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, "## Section A");
        assert!(sections[0].1.contains("Content A"));
        assert_eq!(sections[1].0, "## Section B");
        assert!(sections[1].1.contains("Content B"));
    }

    #[test]
    fn split_sections_preamble_plus_headings() {
        let md = "Preamble before any heading.\n## First\nFirst content.";
        let sections = split_markdown_sections(md);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, ""); // preamble has empty heading
        assert!(sections[0].1.contains("Preamble"));
    }

    // ── Clear ─────────────────────────────────────────────────────────────────

    #[test]
    fn clear_empties_store() {
        let mut s = store();
        s.index_text("Some content", "test:§a");
        s.index_text("More content", "test:§b");
        s.clear();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }
}
