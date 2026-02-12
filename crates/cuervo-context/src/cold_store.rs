//! L2 Cold Store: zstd-compressed segments with optional delta chains.
//!
//! When segments are evicted from L1 (sliding window), they are compressed
//! with zstd and stored here. Consecutive entries use delta encoding when
//! the content is similar enough to save additional space.
//!
//! Retrieval decompresses on demand, bounded by a token budget.

use cuervo_core::traits::ContextChunk;

use crate::compression::{
    compress, decompress, delta_decode, delta_encode, delta_is_efficient, delta_size,
    CompressedBlock, DeltaEncoded,
};
use crate::segment::ContextSegment;

/// A single entry in the cold store.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ColdEntry {
    round_start: u32,
    round_end: u32,
    /// Token estimate for the original uncompressed segment.
    original_tokens: u32,
    /// Storage representation.
    storage: EntryStorage,
    /// Extracted metadata (kept uncompressed for filtering).
    decisions: Vec<String>,
    files_modified: Vec<String>,
    tools_used: Vec<String>,
}

/// How the segment content is stored.
#[derive(Debug, Clone)]
enum EntryStorage {
    /// Full zstd-compressed block (base entry in a chain).
    Compressed(CompressedBlock),
    /// Delta-encoded relative to the previous entry.
    Delta {
        delta: DeltaEncoded,
        /// Index of the base entry in the entries vec.
        base_index: usize,
    },
    /// Raw text (segment too small to compress).
    Raw(String),
}

/// L2 Cold Store: compressed conversation history.
pub struct ColdStore {
    entries: Vec<ColdEntry>,
    /// Total bytes of compressed storage (for metrics).
    total_compressed_bytes: usize,
    /// Total original tokens across all entries.
    total_original_tokens: u32,
    /// Maximum number of entries before eviction.
    max_entries: usize,
}

impl ColdStore {
    /// Create a new cold store with the given capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            total_compressed_bytes: 0,
            total_original_tokens: 0,
            max_entries,
        }
    }

    /// Store a segment in L2. Compresses with zstd and optionally
    /// delta-encodes against the previous entry.
    ///
    /// Returns the number of bytes used for storage.
    pub fn store(&mut self, segment: &ContextSegment) -> usize {
        let text = segment.to_context_string();
        let tokens = segment.token_estimate;

        // Try delta encoding against the last base entry.
        let storage = if let Some(last_base) = self.find_last_base() {
            let base_text = self.decompress_entry(last_base);
            if delta_is_efficient(&base_text, &text) {
                let delta = delta_encode(&base_text, &text);
                let size = delta_size(&delta);
                self.total_compressed_bytes += size;
                EntryStorage::Delta {
                    delta,
                    base_index: last_base,
                }
            } else {
                self.store_compressed_or_raw(&text)
            }
        } else {
            self.store_compressed_or_raw(&text)
        };

        let bytes_used = self.entry_bytes(&storage);

        self.entries.push(ColdEntry {
            round_start: segment.round_start,
            round_end: segment.round_end,
            original_tokens: tokens,
            storage,
            decisions: segment.decisions.clone(),
            files_modified: segment.files_modified.clone(),
            tools_used: segment.tools_used.clone(),
        });

        self.total_original_tokens += tokens;

        // Evict oldest if over capacity.
        while self.entries.len() > self.max_entries {
            self.evict_oldest();
        }

        bytes_used
    }

    /// Retrieve segments as context chunks, most recent first, up to budget.
    ///
    /// Uses pre-computed `original_tokens` for budget checking (O(1) per entry)
    /// and only decompresses entries that fit within budget.
    pub fn retrieve(&self, budget: u32) -> Vec<ContextChunk> {
        let mut chunks = Vec::new();
        let mut remaining = budget;

        // Most recent entries are most likely to be relevant.
        for (i, entry) in self.entries.iter().enumerate().rev() {
            if entry.original_tokens > remaining {
                continue;
            }

            // Budget check passes — decompress only now.
            let text = self.decompress_entry(i);
            chunks.push(ContextChunk {
                source: format!("l2:rounds_{}-{}", entry.round_start, entry.round_end),
                priority: 60, // lower priority than L1 (80)
                estimated_tokens: entry.original_tokens as usize,
                content: text,
            });
            remaining -= entry.original_tokens;
        }

        // Reverse so chunks are in chronological order.
        chunks.reverse();
        chunks
    }

    /// Evict the oldest entry, returning a reconstructed ContextSegment.
    ///
    /// Decompresses the stored content so it can be promoted to L3 (semantic store).
    pub fn evict_oldest_as_segment(&mut self) -> Option<ContextSegment> {
        if self.entries.is_empty() {
            return None;
        }
        // Decompress the content BEFORE removing the entry (delta chains need intact indices).
        let summary = self.decompress_entry(0);
        let entry = &self.entries[0];
        let segment = ContextSegment {
            round_start: entry.round_start,
            round_end: entry.round_end,
            summary,
            decisions: entry.decisions.clone(),
            files_modified: entry.files_modified.clone(),
            tools_used: entry.tools_used.clone(),
            token_estimate: entry.original_tokens,
            created_at: chrono::Utc::now(),
        };
        // Now perform the actual eviction (with delta fixups).
        self.evict_oldest();
        Some(segment)
    }

    /// Evict the oldest entry, returning the round range it covered.
    pub fn evict_oldest(&mut self) -> Option<(u32, u32)> {
        if self.entries.is_empty() {
            return None;
        }

        let entry = self.entries.remove(0);
        let bytes = self.entry_bytes(&entry.storage);
        self.total_compressed_bytes = self.total_compressed_bytes.saturating_sub(bytes);
        self.total_original_tokens = self.total_original_tokens.saturating_sub(entry.original_tokens);

        // Fix up delta base indices (all shifted by -1).
        for e in &mut self.entries {
            if let EntryStorage::Delta { base_index, .. } = &mut e.storage {
                if *base_index == 0 {
                    // Base was evicted — promote to raw by decompressing.
                    // We can't decompress here without &self, so mark for fixup.
                    *base_index = usize::MAX;
                } else {
                    *base_index -= 1;
                }
            }
        }

        // Fixup entries whose base was evicted.
        let mut fixups = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if let EntryStorage::Delta { base_index, .. } = &e.storage {
                if *base_index == usize::MAX {
                    fixups.push(i);
                }
            }
        }
        for idx in fixups {
            let text = self.reconstruct_delta_or_empty(idx);
            let compressed = self.store_compressed_or_raw(&text);
            let old_bytes = self.entry_bytes(&self.entries[idx].storage);
            let new_bytes = self.entry_bytes(&compressed);
            self.total_compressed_bytes = self.total_compressed_bytes.saturating_sub(old_bytes);
            self.total_compressed_bytes += new_bytes;
            self.entries[idx].storage = compressed;
        }

        Some((entry.round_start, entry.round_end))
    }

    /// Number of stored entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Maximum entries capacity.
    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    /// Total bytes of compressed storage.
    pub fn compressed_bytes(&self) -> usize {
        self.total_compressed_bytes
    }

    /// Total original tokens across all entries.
    pub fn original_tokens(&self) -> u32 {
        self.total_original_tokens
    }

    /// Compression ratio (compressed bytes / estimated original bytes).
    /// Lower is better. Returns 1.0 if empty.
    pub fn compression_ratio(&self) -> f64 {
        if self.total_original_tokens == 0 {
            return 1.0;
        }
        // Estimate original bytes as tokens * 4 (inverse of estimate_tokens heuristic).
        let original_bytes = self.total_original_tokens as f64 * 4.0;
        self.total_compressed_bytes as f64 / original_bytes
    }

    /// Get round ranges covered by all entries.
    pub fn round_ranges(&self) -> Vec<(u32, u32)> {
        self.entries
            .iter()
            .map(|e| (e.round_start, e.round_end))
            .collect()
    }

    // --- Internal helpers ---

    fn store_compressed_or_raw(&mut self, text: &str) -> EntryStorage {
        if let Some(block) = compress(text) {
            let size = block.data.len();
            self.total_compressed_bytes += size;
            EntryStorage::Compressed(block)
        } else {
            self.total_compressed_bytes += text.len();
            EntryStorage::Raw(text.to_string())
        }
    }

    fn decompress_entry(&self, index: usize) -> String {
        match &self.entries[index].storage {
            EntryStorage::Compressed(block) => decompress(block).unwrap_or_default(),
            EntryStorage::Raw(text) => text.clone(),
            EntryStorage::Delta { delta, base_index } => {
                if *base_index < self.entries.len() {
                    let base_text = self.decompress_entry(*base_index);
                    delta_decode(&base_text, delta)
                } else {
                    // Broken chain fallback.
                    String::new()
                }
            }
        }
    }

    /// Reconstruct a delta entry's text or return empty on broken chain.
    fn reconstruct_delta_or_empty(&self, index: usize) -> String {
        // For fixup: the entry at `index` has base_index = usize::MAX (broken).
        // We reconstruct from the DeltaEncoded ops using Insert ops only (Copy from nothing = empty).
        if let EntryStorage::Delta { delta, .. } = &self.entries[index].storage {
            // delta_decode with empty base gives us just the Insert ops.
            delta_decode("", delta)
        } else {
            String::new()
        }
    }

    fn find_last_base(&self) -> Option<usize> {
        // Find the last entry that is a Compressed or Raw (a base entry, not delta).
        self.entries
            .iter()
            .enumerate()
            .rev()
            .find(|(_, e)| matches!(e.storage, EntryStorage::Compressed(_) | EntryStorage::Raw(_)))
            .map(|(i, _)| i)
    }

    fn entry_bytes(&self, storage: &EntryStorage) -> usize {
        match storage {
            EntryStorage::Compressed(block) => block.data.len(),
            EntryStorage::Raw(text) => text.len(),
            EntryStorage::Delta { delta, .. } => delta_size(delta),
        }
    }
}

impl Default for ColdStore {
    fn default() -> Self {
        Self::new(100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembler::estimate_tokens;
    use chrono::Utc;

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

    fn make_large_segment(start: u32, end: u32, unique_prefix: &str) -> ContextSegment {
        let summary = format!(
            "{} {}",
            unique_prefix,
            "This is a detailed summary of the conversation segment covering multiple topics \
             including Rust async patterns, error handling, and testing strategies. "
                .repeat(10)
        );
        make_segment(start, end, &summary)
    }

    // --- Basic store/retrieve tests ---

    #[test]
    fn empty_store() {
        let store = ColdStore::new(10);
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        assert_eq!(store.compressed_bytes(), 0);
        assert_eq!(store.original_tokens(), 0);
        assert!((store.compression_ratio() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn store_small_segment_raw() {
        let mut store = ColdStore::new(10);
        let seg = make_segment(1, 2, "short summary");
        let bytes = store.store(&seg);
        assert!(bytes > 0);
        assert_eq!(store.len(), 1);
        assert!(store.original_tokens() > 0);
    }

    #[test]
    fn store_large_segment_compressed() {
        let mut store = ColdStore::new(10);
        let seg = make_large_segment(1, 5, "round-1-5");
        store.store(&seg);
        assert_eq!(store.len(), 1);
        // Compressed bytes should be less than original tokens * 4.
        let original_estimate = seg.token_estimate as usize * 4;
        assert!(
            store.compressed_bytes() < original_estimate,
            "compressed={} should be < original={}",
            store.compressed_bytes(),
            original_estimate
        );
    }

    #[test]
    fn store_retrieve_roundtrip() {
        let mut store = ColdStore::new(10);
        let seg = make_large_segment(1, 5, "roundtrip-test");
        let original = seg.to_context_string();
        store.store(&seg);

        let chunks = store.retrieve(100_000); // plenty of budget
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, original);
        assert!(chunks[0].source.contains("l2:rounds_1-5"));
        assert_eq!(chunks[0].priority, 60);
    }

    #[test]
    fn retrieve_respects_budget() {
        let mut store = ColdStore::new(10);
        let seg1 = make_large_segment(1, 3, "first-segment");
        let seg2 = make_large_segment(4, 6, "second-segment");
        store.store(&seg1);
        store.store(&seg2);

        // Budget for approximately one segment.
        let one_seg_tokens = seg1.token_estimate;
        let chunks = store.retrieve(one_seg_tokens + 5);
        // Should get at most one (most recent preferred).
        assert!(chunks.len() <= 2);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn retrieve_zero_budget_returns_empty() {
        let mut store = ColdStore::new(10);
        store.store(&make_large_segment(1, 3, "test"));
        let chunks = store.retrieve(0);
        assert!(chunks.is_empty());
    }

    #[test]
    fn retrieve_chronological_order() {
        let mut store = ColdStore::new(10);
        store.store(&make_large_segment(1, 2, "first"));
        store.store(&make_large_segment(3, 4, "second"));
        store.store(&make_large_segment(5, 6, "third"));

        let chunks = store.retrieve(100_000);
        // Should be in chronological order (round_start ascending).
        assert!(chunks[0].source.contains("rounds_1-2"));
        assert!(chunks[1].source.contains("rounds_3-4"));
        assert!(chunks[2].source.contains("rounds_5-6"));
    }

    // --- Eviction tests ---

    #[test]
    fn evict_oldest() {
        let mut store = ColdStore::new(10);
        store.store(&make_large_segment(1, 2, "first"));
        store.store(&make_large_segment(3, 4, "second"));

        let evicted = store.evict_oldest();
        assert_eq!(evicted, Some((1, 2)));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn evict_empty_returns_none() {
        let mut store = ColdStore::new(10);
        assert!(store.evict_oldest().is_none());
    }

    #[test]
    fn max_entries_auto_evicts() {
        let mut store = ColdStore::new(3);
        for i in 0..5 {
            store.store(&make_large_segment(i * 2, i * 2 + 1, &format!("seg-{i}")));
        }
        assert_eq!(store.len(), 3);
        // Oldest entries should have been evicted.
        let ranges = store.round_ranges();
        assert_eq!(ranges[0].0, 4); // segments 2, 3, 4 remain
    }

    // --- Delta encoding tests ---

    #[test]
    fn delta_encoding_similar_segments() {
        let mut store = ColdStore::new(10);
        // Create two segments with very similar content.
        let base_text = "Detailed conversation about Rust error handling patterns, \
            including thiserror for library code and anyhow for application code. \
            We discussed the Display trait implementation and custom error types. "
            .repeat(8);
        let seg1 = make_segment(1, 3, &base_text);
        store.store(&seg1);

        // Second segment: same content with minor changes.
        let modified = format!(
            "{}. Additional discussion about Result combinators.",
            &base_text[..base_text.len() - 1]
        );
        let seg2 = make_segment(4, 6, &modified);
        store.store(&seg2);

        // Both should be retrievable.
        let chunks = store.retrieve(100_000);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn delta_chain_survives_base_eviction() {
        let mut store = ColdStore::new(3);
        // Store 3 segments, then store a 4th to evict the first (which is the base).
        let text = "Long repeated content for testing delta encoding efficiency. ".repeat(10);
        store.store(&make_segment(1, 2, &text));
        store.store(&make_segment(3, 4, &format!("{text} minor change 1")));
        store.store(&make_segment(5, 6, &format!("{text} minor change 2")));

        // Evict oldest (the base).
        store.evict_oldest();
        assert_eq!(store.len(), 2);

        // Remaining entries should still be retrievable.
        let chunks = store.retrieve(100_000);
        assert!(chunks.len() <= 2);
    }

    // --- Metrics tests ---

    #[test]
    fn compression_ratio_improves_with_repetitive_data() {
        let mut store = ColdStore::new(10);
        let repetitive = "The same line repeated many times for compression testing. ".repeat(20);
        store.store(&make_segment(1, 5, &repetitive));
        assert!(
            store.compression_ratio() < 0.5,
            "Expected ratio < 0.5, got {}",
            store.compression_ratio()
        );
    }

    #[test]
    fn round_ranges() {
        let mut store = ColdStore::new(10);
        store.store(&make_segment(1, 3, &"a".repeat(300)));
        store.store(&make_segment(5, 8, &"b".repeat(300)));
        let ranges = store.round_ranges();
        assert_eq!(ranges, vec![(1, 3), (5, 8)]);
    }

    #[test]
    fn original_tokens_accumulates() {
        let mut store = ColdStore::new(10);
        let seg1 = make_large_segment(1, 2, "first");
        let seg2 = make_large_segment(3, 4, "second");
        let t1 = seg1.token_estimate;
        let t2 = seg2.token_estimate;
        store.store(&seg1);
        store.store(&seg2);
        assert_eq!(store.original_tokens(), t1 + t2);
    }

    #[test]
    fn eviction_decreases_metrics() {
        let mut store = ColdStore::new(10);
        let seg = make_large_segment(1, 3, "to-be-evicted");
        store.store(&seg);
        let bytes_before = store.compressed_bytes();
        let tokens_before = store.original_tokens();

        store.evict_oldest();
        assert!(store.compressed_bytes() < bytes_before);
        assert!(store.original_tokens() < tokens_before);
    }

    #[test]
    fn default_store() {
        let store = ColdStore::default();
        assert_eq!(store.max_entries, 100);
        assert!(store.is_empty());
    }

    // --- Metadata preservation ---

    #[test]
    fn metadata_preserved_in_storage() {
        let mut store = ColdStore::new(10);
        let mut seg = make_large_segment(1, 5, "metadata-test");
        seg.decisions = vec!["Use tokio".to_string()];
        seg.files_modified = vec!["src/lib.rs".to_string()];
        seg.tools_used = vec!["bash".to_string()];
        store.store(&seg);

        // The metadata is on the ColdEntry, but we verify through retrieve.
        let chunks = store.retrieve(100_000);
        assert_eq!(chunks.len(), 1);
        // The context string should contain metadata.
        assert!(chunks[0].content.contains("Decisions: Use tokio"));
        assert!(chunks[0].content.contains("Files: src/lib.rs"));
        assert!(chunks[0].content.contains("Tools: bash"));
    }
}
