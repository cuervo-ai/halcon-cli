//! L3 Semantic Store: BM25-based segment retrieval.
//!
//! When segments are evicted from L2 (cold store), they flow into L3 for
//! semantic retrieval. Uses BM25 (Best Match 25) scoring to rank stored
//! segments by relevance to a query string, enabling the pipeline to
//! surface relevant past context even after it has been evicted from
//! the compressed store.
//!
//! Fully synchronous — no async embedding provider required.
//! Budget-aware: retrieval stops when token budget is exhausted.

use std::collections::HashMap;

use cuervo_core::traits::ContextChunk;

use crate::segment::ContextSegment;

/// BM25 tuning parameters.
const K1: f32 = 1.5;
const B: f32 = 0.75;

/// A stored segment with pre-computed term frequencies.
struct SemanticEntry {
    segment: ContextSegment,
    /// Pre-formatted context string (avoids regeneration on retrieval).
    context_string: String,
    /// Term frequency map: term → count.
    term_freqs: HashMap<String, u32>,
    /// Total term count in this document.
    term_count: u32,
}

/// L3 Semantic Store: BM25-based retrieval over evicted segments.
pub struct SemanticStore {
    entries: Vec<SemanticEntry>,
    /// Document frequency: term → number of documents containing it.
    doc_freqs: HashMap<String, u32>,
    /// Sum of all document term counts (for computing avgdl).
    total_terms: u64,
    /// Maximum entries before oldest eviction.
    max_entries: usize,
}

impl SemanticStore {
    /// Create a new semantic store with the given capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            doc_freqs: HashMap::new(),
            total_terms: 0,
            max_entries,
        }
    }

    /// Store a segment in L3. Builds term frequency index.
    pub fn store(&mut self, segment: &ContextSegment) {
        // Evict oldest if at capacity.
        if self.entries.len() >= self.max_entries {
            self.evict_oldest();
        }

        let context_string = segment.to_context_string();
        let text = context_string.to_lowercase();
        let (term_freqs, term_count) = Self::tokenize_and_count(&text);

        // Update document frequencies.
        for term in term_freqs.keys() {
            *self.doc_freqs.entry(term.clone()).or_insert(0) += 1;
        }
        self.total_terms += term_count as u64;

        self.entries.push(SemanticEntry {
            segment: segment.clone(),
            context_string,
            term_freqs,
            term_count,
        });
    }

    /// Retrieve top-K segments matching the query, bounded by token budget.
    ///
    /// Returns segments ordered by BM25 relevance score (highest first).
    pub fn retrieve(&self, query: &str, budget: u32) -> Vec<ContextChunk> {
        if self.entries.is_empty() || budget == 0 {
            return Vec::new();
        }

        let query_lower = query.to_lowercase();
        let query_terms = Self::tokenize(&query_lower);

        if query_terms.is_empty() {
            return Vec::new();
        }

        // Score all entries.
        let mut scored: Vec<(usize, f32)> = self
            .entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                let score = self.bm25_score(entry, &query_terms);
                (idx, score)
            })
            .filter(|(_, score)| *score > 0.0)
            .collect();

        // Sort by score descending.
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Collect within budget.
        let mut result = Vec::new();
        let mut used = 0u32;

        for (idx, _score) in scored {
            let entry = &self.entries[idx];
            let tokens = entry.segment.token_estimate;
            if used + tokens > budget {
                continue; // Skip this one, try smaller ones
            }
            used += tokens;
            result.push(ContextChunk {
                source: format!("L3:semantic:r{}-{}", entry.segment.round_start, entry.segment.round_end),
                priority: 30, // Lower priority than L1 (50) and L2 (40)
                content: entry.context_string.clone(),
                estimated_tokens: tokens as usize,
            });
        }

        result
    }

    /// Evict the oldest entry from the store.
    pub fn evict_oldest(&mut self) -> Option<ContextSegment> {
        if self.entries.is_empty() {
            return None;
        }

        let removed = self.entries.remove(0);

        // Update document frequencies.
        for term in removed.term_freqs.keys() {
            if let Some(df) = self.doc_freqs.get_mut(term) {
                *df = df.saturating_sub(1);
                if *df == 0 {
                    self.doc_freqs.remove(term);
                }
            }
        }
        self.total_terms = self.total_terms.saturating_sub(removed.term_count as u64);

        Some(removed.segment)
    }

    /// Number of stored entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total original tokens across all stored segments.
    pub fn original_tokens(&self) -> u32 {
        self.entries
            .iter()
            .map(|e| e.segment.token_estimate)
            .sum()
    }

    /// Maximum entries capacity.
    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    /// Number of unique terms in the index.
    pub fn vocabulary_size(&self) -> usize {
        self.doc_freqs.len()
    }

    // --- BM25 internals ---

    /// Compute BM25 score for a document against query terms.
    fn bm25_score(&self, entry: &SemanticEntry, query_terms: &[String]) -> f32 {
        let n = self.entries.len() as f32;
        let avgdl = if self.entries.is_empty() {
            1.0
        } else {
            self.total_terms as f32 / n
        };
        let dl = entry.term_count as f32;

        let mut score = 0.0f32;

        for term in query_terms {
            // Term frequency in this document.
            let tf = entry.term_freqs.get(term).copied().unwrap_or(0) as f32;
            if tf == 0.0 {
                continue;
            }

            // Document frequency (how many documents contain this term).
            let df = self.doc_freqs.get(term).copied().unwrap_or(0) as f32;

            // IDF: ln((N - df + 0.5) / (df + 0.5) + 1)
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();

            // BM25 term score.
            let numerator = tf * (K1 + 1.0);
            let denominator = tf + K1 * (1.0 - B + B * dl / avgdl);

            score += idf * numerator / denominator;
        }

        score
    }

    /// Tokenize text into lowercase terms (simple whitespace + punctuation split).
    fn tokenize(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|w| w.len() >= 2) // skip single chars
            .map(|w| w.to_string())
            .collect()
    }

    /// Tokenize and compute term frequencies.
    fn tokenize_and_count(text: &str) -> (HashMap<String, u32>, u32) {
        let mut freqs = HashMap::new();
        let mut count = 0u32;

        for token in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if token.len() < 2 {
                continue;
            }
            *freqs.entry(token.to_string()).or_insert(0) += 1;
            count += 1;
        }

        (freqs, count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::assembler::estimate_tokens;

    fn make_segment(start: u32, end: u32, summary: &str) -> ContextSegment {
        ContextSegment {
            round_start: start,
            round_end: end,
            summary: summary.to_string(),
            decisions: vec![],
            files_modified: vec![],
            tools_used: vec![],
            token_estimate: estimate_tokens(summary) as u32,
            created_at: Utc::now(),
        }
    }

    // --- Basic store/retrieve ---

    #[test]
    fn new_store_is_empty() {
        let store = SemanticStore::new(100);
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        assert_eq!(store.original_tokens(), 0);
    }

    #[test]
    fn store_single_segment() {
        let mut store = SemanticStore::new(100);
        store.store(&make_segment(1, 1, "Implemented async Rust patterns with tokio"));
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
        assert!(store.vocabulary_size() > 0);
    }

    #[test]
    fn store_multiple_segments() {
        let mut store = SemanticStore::new(100);
        store.store(&make_segment(1, 1, "Implemented Rust async patterns"));
        store.store(&make_segment(2, 2, "Added Python test suite"));
        store.store(&make_segment(3, 3, "Configured SQLite database"));
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn retrieve_relevant_segment() {
        let mut store = SemanticStore::new(100);
        store.store(&make_segment(1, 1, "Implemented Rust async patterns with tokio runtime"));
        store.store(&make_segment(2, 2, "Added Python unit tests for the API"));
        store.store(&make_segment(3, 3, "Configured SQLite database with WAL mode"));

        let results = store.retrieve("Rust async tokio", 10_000);
        assert!(!results.is_empty());
        // First result should be the Rust/tokio segment.
        assert!(results[0].content.contains("Rust async"));
    }

    #[test]
    fn retrieve_empty_query_returns_nothing() {
        let mut store = SemanticStore::new(100);
        store.store(&make_segment(1, 1, "Some content about Rust"));
        let results = store.retrieve("", 10_000);
        assert!(results.is_empty());
    }

    #[test]
    fn retrieve_no_match_returns_empty() {
        let mut store = SemanticStore::new(100);
        store.store(&make_segment(1, 1, "Implemented Rust patterns"));
        let results = store.retrieve("quantum physics", 10_000);
        assert!(results.is_empty());
    }

    #[test]
    fn retrieve_from_empty_store() {
        let store = SemanticStore::new(100);
        let results = store.retrieve("anything", 10_000);
        assert!(results.is_empty());
    }

    // --- Budget enforcement ---

    #[test]
    fn retrieve_respects_budget() {
        let mut store = SemanticStore::new(100);
        // Create segments with known token sizes.
        for i in 0..10 {
            store.store(&make_segment(
                i,
                i,
                &format!(
                    "Segment about Rust async patterns number {i} with detailed discussion \
                     of error handling strategies and tokio runtime configuration"
                ),
            ));
        }

        // Very tight budget should return fewer results.
        let tight = store.retrieve("Rust async tokio", 5);
        let loose = store.retrieve("Rust async tokio", 50_000);
        assert!(tight.len() <= loose.len());
    }

    #[test]
    fn retrieve_zero_budget_returns_empty() {
        let mut store = SemanticStore::new(100);
        store.store(&make_segment(1, 1, "Content about Rust"));
        let results = store.retrieve("Rust", 0);
        assert!(results.is_empty());
    }

    // --- Eviction ---

    #[test]
    fn evict_oldest_removes_first() {
        let mut store = SemanticStore::new(100);
        store.store(&make_segment(1, 1, "First entry about Rust"));
        store.store(&make_segment(2, 2, "Second entry about Python"));
        store.store(&make_segment(3, 3, "Third entry about Go"));

        let evicted = store.evict_oldest();
        assert!(evicted.is_some());
        assert_eq!(evicted.unwrap().round_start, 1);
        assert_eq!(store.len(), 2);

        // Doc freqs updated: "first" should be gone.
        assert!(!store.doc_freqs.contains_key("first"));
    }

    #[test]
    fn evict_from_empty_returns_none() {
        let mut store = SemanticStore::new(100);
        assert!(store.evict_oldest().is_none());
    }

    #[test]
    fn auto_evict_at_capacity() {
        let mut store = SemanticStore::new(3);
        store.store(&make_segment(1, 1, "Alpha about Rust"));
        store.store(&make_segment(2, 2, "Beta about Python"));
        store.store(&make_segment(3, 3, "Gamma about Go"));
        // This should trigger auto-eviction.
        store.store(&make_segment(4, 4, "Delta about Java"));

        assert_eq!(store.len(), 3);
        // First entry (Alpha) should be evicted.
        let results = store.retrieve("Alpha Rust", 10_000);
        assert!(results.is_empty());
    }

    // --- BM25 scoring ---

    #[test]
    fn bm25_ranks_more_relevant_higher() {
        let mut store = SemanticStore::new(100);
        store.store(&make_segment(
            1,
            1,
            "Rust async patterns tokio runtime error handling thiserror anyhow",
        ));
        store.store(&make_segment(
            2,
            2,
            "Python Flask web server with JSON REST API endpoints",
        ));
        store.store(&make_segment(
            3,
            3,
            "Rust async tokio spawn blocking performance optimization",
        ));

        let results = store.retrieve("Rust async tokio", 50_000);
        assert!(results.len() >= 2);
        // Both Rust segments should appear, Python should not.
        assert!(results.iter().all(|r| r.content.contains("Rust") || r.content.contains("tokio")));
    }

    #[test]
    fn bm25_idf_penalizes_common_terms() {
        let mut store = SemanticStore::new(100);
        // "the" appears in all documents (low IDF), "quantum" appears in one (high IDF).
        store.store(&make_segment(1, 1, "the quick brown fox jumps over the lazy dog"));
        store.store(&make_segment(2, 2, "the cat sat on the mat by the door"));
        store.store(&make_segment(3, 3, "quantum computing uses the principles of superposition"));

        let results = store.retrieve("quantum principles", 50_000);
        assert!(!results.is_empty());
        assert!(results[0].content.contains("quantum"));
    }

    // --- Vocabulary tracking ---

    #[test]
    fn vocabulary_grows_with_segments() {
        let mut store = SemanticStore::new(100);
        store.store(&make_segment(1, 1, "alpha beta gamma"));
        let v1 = store.vocabulary_size();

        store.store(&make_segment(2, 2, "delta epsilon zeta"));
        let v2 = store.vocabulary_size();
        assert!(v2 > v1);
    }

    #[test]
    fn vocabulary_shrinks_on_eviction() {
        let mut store = SemanticStore::new(100);
        store.store(&make_segment(1, 1, "unique_alpha unique_beta"));
        store.store(&make_segment(2, 2, "common shared terms"));
        let before = store.vocabulary_size();

        store.evict_oldest();
        let after = store.vocabulary_size();
        assert!(after < before);
    }

    // --- Tokenization ---

    #[test]
    fn tokenize_splits_on_punctuation() {
        let tokens = SemanticStore::tokenize("hello, world! foo-bar");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
        assert!(tokens.contains(&"foo".to_string()));
        assert!(tokens.contains(&"bar".to_string()));
    }

    #[test]
    fn tokenize_preserves_underscores() {
        let tokens = SemanticStore::tokenize("snake_case variable_name");
        assert!(tokens.contains(&"snake_case".to_string()));
        assert!(tokens.contains(&"variable_name".to_string()));
    }

    #[test]
    fn tokenize_skips_single_chars() {
        let tokens = SemanticStore::tokenize("a b c dd ee");
        assert!(!tokens.contains(&"a".to_string()));
        assert!(tokens.contains(&"dd".to_string()));
    }

    // --- Original tokens tracking ---

    #[test]
    fn original_tokens_sum() {
        let mut store = SemanticStore::new(100);
        let seg1 = make_segment(1, 1, "hello world");
        let seg2 = make_segment(2, 2, "foo bar baz");
        let t1 = seg1.token_estimate;
        let t2 = seg2.token_estimate;
        store.store(&seg1);
        store.store(&seg2);
        assert_eq!(store.original_tokens(), t1 + t2);
    }

    // --- Edge cases ---

    #[test]
    fn retrieve_with_metadata_keywords() {
        let mut seg = make_segment(1, 1, "Modified authentication module");
        seg.files_modified = vec!["src/auth.rs".to_string()];
        seg.tools_used = vec!["file_write".to_string()];
        seg.decisions = vec!["Use JWT tokens".to_string()];

        let mut store = SemanticStore::new(100);
        store.store(&seg);

        // Should match on metadata too (to_context_string includes metadata).
        let results = store.retrieve("auth JWT", 10_000);
        assert!(!results.is_empty());
    }

    #[test]
    fn case_insensitive_retrieval() {
        let mut store = SemanticStore::new(100);
        store.store(&make_segment(1, 1, "Rust ASYNC Patterns"));
        let results = store.retrieve("rust async patterns", 10_000);
        assert!(!results.is_empty());
    }
}
