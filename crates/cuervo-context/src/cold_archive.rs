//! L4 Cold Archive: disk-backed persistent segment storage.
//!
//! Final tier in the L0-L4 memory hierarchy. Receives segments evicted from
//! L3 (semantic store) and persists them for cross-session retrieval.
//! Uses zstd compression for disk efficiency.
//!
//! Design: in-memory index + on-demand disk I/O.
//! - `store()` adds to in-memory buffer (sync, fast)
//! - `flush()` writes buffer to disk (call from async context)
//! - `load()` restores from disk on session start
//! - `retrieve()` returns segments within token budget

use std::path::{Path, PathBuf};

use crate::assembler::estimate_tokens;
use crate::compression::{compress, decompress};
use crate::segment::ContextSegment;

use cuervo_core::traits::ContextChunk;

/// A single archived entry: compressed segment + metadata index.
#[derive(Debug)]
struct ArchiveEntry {
    /// Compressed segment bytes (zstd).
    compressed: Vec<u8>,
    /// Original token estimate (for budget decisions without decompression).
    original_tokens: u32,
    /// Round range covered (for ordering/display).
    round_start: u32,
    round_end: u32,
    /// Key terms for lightweight keyword filtering (top 8 terms by frequency).
    key_terms: Vec<String>,
}

/// L4 Cold Archive: persistent segment storage.
pub struct ColdArchive {
    /// In-memory entries (compressed).
    entries: Vec<ArchiveEntry>,
    /// Pending entries not yet flushed to disk.
    pending_flush: Vec<usize>,
    /// Maximum entries before oldest eviction.
    max_entries: usize,
    /// Archive file path (None = memory-only mode for testing).
    archive_path: Option<PathBuf>,
    /// Total compressed bytes across all entries.
    total_compressed_bytes: usize,
    /// Total original tokens across all entries.
    total_original_tokens: u32,
}

impl ColdArchive {
    /// Create a new cold archive (memory-only, for testing).
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            pending_flush: Vec::new(),
            max_entries,
            archive_path: None,
            total_compressed_bytes: 0,
            total_original_tokens: 0,
        }
    }

    /// Create a new cold archive backed by a file path.
    pub fn with_path(max_entries: usize, path: PathBuf) -> Self {
        Self {
            entries: Vec::new(),
            pending_flush: Vec::new(),
            max_entries,
            archive_path: Some(path),
            total_compressed_bytes: 0,
            total_original_tokens: 0,
        }
    }

    /// Store a segment in L4. Compresses and indexes for retrieval.
    pub fn store(&mut self, segment: &ContextSegment) {
        // Evict oldest if at capacity.
        if self.entries.len() >= self.max_entries {
            self.evict_oldest();
        }

        let text = segment.to_context_string();
        let key_terms = Self::extract_key_terms(&text, 8);

        let compressed_bytes = match compress(&text) {
            Some(block) => block.data,
            None => text.as_bytes().to_vec(), // fallback to raw (too small to compress)
        };

        let entry = ArchiveEntry {
            compressed: compressed_bytes.clone(),
            original_tokens: segment.token_estimate,
            round_start: segment.round_start,
            round_end: segment.round_end,
            key_terms,
        };

        self.total_compressed_bytes += compressed_bytes.len();
        self.total_original_tokens += segment.token_estimate;

        let idx = self.entries.len();
        self.entries.push(entry);
        self.pending_flush.push(idx);
    }

    /// Retrieve archived segments within token budget, optionally filtered by query.
    ///
    /// If query is provided, only segments with matching key terms are returned.
    /// Returns most recent entries first (within budget).
    pub fn retrieve(&self, query: Option<&str>, budget: u32) -> Vec<ContextChunk> {
        if self.entries.is_empty() || budget == 0 {
            return Vec::new();
        }

        let query_terms: Vec<String> = query
            .map(Self::tokenize)
            .unwrap_or_default();

        let mut candidates: Vec<(usize, f32)> = self
            .entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                let score = if query_terms.is_empty() {
                    // No query: recency-based (higher index = more recent)
                    idx as f32
                } else {
                    // Query-based: keyword overlap score
                    let matches = entry
                        .key_terms
                        .iter()
                        .filter(|t| query_terms.iter().any(|qt| t.contains(qt.as_str())))
                        .count();
                    matches as f32 + (idx as f32 * 0.001) // recency tiebreaker
                };
                (idx, score)
            })
            .filter(|(_, score)| *score > 0.0)
            .collect();

        // Sort by score descending (most relevant/recent first).
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Collect within budget, decompressing on demand.
        let mut result = Vec::new();
        let mut used = 0u32;

        for (idx, _score) in candidates {
            let entry = &self.entries[idx];
            if used + entry.original_tokens > budget {
                continue;
            }

            // Decompress on demand.
            let text = Self::decompress_entry(entry);
            let actual_tokens = estimate_tokens(&text) as u32;
            if used + actual_tokens > budget {
                continue;
            }

            used += actual_tokens;
            result.push(ContextChunk {
                source: format!(
                    "L4:archive:r{}-{}",
                    entry.round_start, entry.round_end
                ),
                priority: 20, // Lowest priority (below L3=30)
                content: text,
                estimated_tokens: actual_tokens as usize,
            });
        }

        result
    }

    /// Evict the oldest entry.
    pub fn evict_oldest(&mut self) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let removed = self.entries.remove(0);
        self.total_compressed_bytes = self
            .total_compressed_bytes
            .saturating_sub(removed.compressed.len());
        self.total_original_tokens = self
            .total_original_tokens
            .saturating_sub(removed.original_tokens);
        // Shift pending flush indices.
        self.pending_flush.retain_mut(|idx| {
            if *idx == 0 {
                false
            } else {
                *idx -= 1;
                true
            }
        });
        true
    }

    /// Number of archived entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the archive is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total original tokens across all archived segments.
    pub fn original_tokens(&self) -> u32 {
        self.total_original_tokens
    }

    /// Total compressed bytes on disk/memory.
    pub fn compressed_bytes(&self) -> usize {
        self.total_compressed_bytes
    }

    /// Compression ratio (compressed / original estimate).
    pub fn compression_ratio(&self) -> f64 {
        if self.total_original_tokens == 0 {
            return 0.0;
        }
        self.total_compressed_bytes as f64 / (self.total_original_tokens as f64 * 4.0)
    }

    /// Maximum entries capacity.
    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    /// Number of entries pending flush to disk.
    pub fn pending_count(&self) -> usize {
        self.pending_flush.len()
    }

    /// Serialize the archive to bytes for disk persistence.
    /// Returns compressed archive data.
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::new();
        // Header: entry count (u32 LE).
        data.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());

        for entry in &self.entries {
            // round_start (u32 LE)
            data.extend_from_slice(&entry.round_start.to_le_bytes());
            // round_end (u32 LE)
            data.extend_from_slice(&entry.round_end.to_le_bytes());
            // original_tokens (u32 LE)
            data.extend_from_slice(&entry.original_tokens.to_le_bytes());
            // key_terms count (u16 LE)
            data.extend_from_slice(&(entry.key_terms.len() as u16).to_le_bytes());
            for term in &entry.key_terms {
                let bytes = term.as_bytes();
                data.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
                data.extend_from_slice(bytes);
            }
            // compressed data length (u32 LE) + data
            data.extend_from_slice(&(entry.compressed.len() as u32).to_le_bytes());
            data.extend_from_slice(&entry.compressed);
        }

        self.pending_flush.len(); // acknowledge
        data
    }

    /// Deserialize archive from bytes (loaded from disk).
    pub fn deserialize(data: &[u8], max_entries: usize) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }

        let entry_count = u32::from_le_bytes(data[0..4].try_into().ok()?) as usize;
        let mut offset = 4usize;
        let mut entries = Vec::with_capacity(entry_count.min(max_entries));
        let mut total_compressed_bytes = 0usize;
        let mut total_original_tokens = 0u32;

        for _ in 0..entry_count {
            if offset + 12 > data.len() {
                break;
            }
            let round_start = u32::from_le_bytes(data[offset..offset + 4].try_into().ok()?);
            offset += 4;
            let round_end = u32::from_le_bytes(data[offset..offset + 4].try_into().ok()?);
            offset += 4;
            let original_tokens = u32::from_le_bytes(data[offset..offset + 4].try_into().ok()?);
            offset += 4;

            if offset + 2 > data.len() {
                break;
            }
            let term_count = u16::from_le_bytes(data[offset..offset + 2].try_into().ok()?) as usize;
            offset += 2;

            let mut key_terms = Vec::with_capacity(term_count);
            for _ in 0..term_count {
                if offset + 2 > data.len() {
                    return None;
                }
                let term_len =
                    u16::from_le_bytes(data[offset..offset + 2].try_into().ok()?) as usize;
                offset += 2;
                if offset + term_len > data.len() {
                    return None;
                }
                let term = String::from_utf8_lossy(&data[offset..offset + term_len]).to_string();
                offset += term_len;
                key_terms.push(term);
            }

            if offset + 4 > data.len() {
                break;
            }
            let compressed_len =
                u32::from_le_bytes(data[offset..offset + 4].try_into().ok()?) as usize;
            offset += 4;
            if offset + compressed_len > data.len() {
                break;
            }
            let compressed = data[offset..offset + compressed_len].to_vec();
            offset += compressed_len;

            total_compressed_bytes += compressed.len();
            total_original_tokens += original_tokens;

            entries.push(ArchiveEntry {
                compressed,
                original_tokens,
                round_start,
                round_end,
                key_terms,
            });
        }

        Some(Self {
            entries,
            pending_flush: Vec::new(),
            max_entries,
            archive_path: None,
            total_compressed_bytes,
            total_original_tokens,
        })
    }

    /// Flush pending entries to disk (call from async context via spawn_blocking).
    /// Returns number of bytes written, or None if no archive path configured.
    pub fn flush_to_disk(&mut self) -> Option<usize> {
        let path = self.archive_path.as_ref()?;
        let data = self.serialize();
        std::fs::write(path, &data).ok()?;
        let bytes = data.len();
        self.pending_flush.clear();
        Some(bytes)
    }

    /// Load archive from disk file.
    pub fn load_from_disk(path: &Path, max_entries: usize) -> Option<Self> {
        let data = std::fs::read(path).ok()?;
        let mut archive = Self::deserialize(&data, max_entries)?;
        archive.archive_path = Some(path.to_path_buf());
        Some(archive)
    }

    // --- Internal helpers ---

    fn decompress_entry(entry: &ArchiveEntry) -> String {
        use crate::compression::CompressedBlock;
        let block = CompressedBlock {
            data: entry.compressed.clone(),
            original_size: (entry.original_tokens as usize) * 4,
        };
        decompress(&block).unwrap_or_else(|| {
            // Fallback: try as raw UTF-8.
            String::from_utf8_lossy(&entry.compressed).to_string()
        })
    }

    fn tokenize(text: &str) -> Vec<String> {
        text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|w| w.len() >= 2)
            .map(|w| w.to_string())
            .collect()
    }

    fn extract_key_terms(text: &str, max_terms: usize) -> Vec<String> {
        let mut freq: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for token in Self::tokenize(text) {
            *freq.entry(token).or_insert(0) += 1;
        }
        let mut terms: Vec<(String, u32)> = freq.into_iter().collect();
        // Sort by frequency descending, then alphabetically for determinism.
        terms.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        terms.into_iter().take(max_terms).map(|(t, _)| t).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    // --- Basic store/retrieve ---

    #[test]
    fn new_archive_is_empty() {
        let archive = ColdArchive::new(100);
        assert!(archive.is_empty());
        assert_eq!(archive.len(), 0);
        assert_eq!(archive.original_tokens(), 0);
        assert_eq!(archive.compressed_bytes(), 0);
    }

    #[test]
    fn store_single_segment() {
        let mut archive = ColdArchive::new(100);
        archive.store(&make_segment(1, 5, "Implemented Rust async patterns with tokio runtime"));
        assert_eq!(archive.len(), 1);
        assert!(!archive.is_empty());
        assert!(archive.original_tokens() > 0);
        assert!(archive.compressed_bytes() > 0);
    }

    #[test]
    fn store_multiple_segments() {
        let mut archive = ColdArchive::new(100);
        archive.store(&make_segment(1, 3, "First segment about Rust"));
        archive.store(&make_segment(4, 6, "Second segment about Python"));
        archive.store(&make_segment(7, 9, "Third segment about Go"));
        assert_eq!(archive.len(), 3);
    }

    #[test]
    fn retrieve_without_query() {
        let mut archive = ColdArchive::new(100);
        archive.store(&make_segment(1, 3, "Implemented Rust async with tokio"));
        archive.store(&make_segment(4, 6, "Added Python tests for API"));

        let chunks = archive.retrieve(None, 50_000);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn retrieve_with_query() {
        let mut archive = ColdArchive::new(100);
        archive.store(&make_segment(1, 3, "Implemented Rust async patterns with tokio runtime"));
        archive.store(&make_segment(4, 6, "Added Python Flask API endpoints"));
        archive.store(&make_segment(7, 9, "Configured SQLite database with WAL"));

        let chunks = archive.retrieve(Some("Rust tokio async"), 50_000);
        assert!(!chunks.is_empty());
        // Should prefer the Rust segment
        assert!(chunks[0].content.contains("Rust"));
    }

    #[test]
    fn retrieve_zero_budget() {
        let mut archive = ColdArchive::new(100);
        archive.store(&make_segment(1, 3, "Content about Rust"));
        let chunks = archive.retrieve(None, 0);
        assert!(chunks.is_empty());
    }

    #[test]
    fn retrieve_from_empty() {
        let archive = ColdArchive::new(100);
        let chunks = archive.retrieve(Some("anything"), 50_000);
        assert!(chunks.is_empty());
    }

    // --- Eviction ---

    #[test]
    fn auto_evict_at_capacity() {
        let mut archive = ColdArchive::new(3);
        archive.store(&make_segment(1, 1, "Alpha about Rust"));
        archive.store(&make_segment(2, 2, "Beta about Python"));
        archive.store(&make_segment(3, 3, "Gamma about Go"));
        archive.store(&make_segment(4, 4, "Delta about Java"));

        assert_eq!(archive.len(), 3);
        // First entry should be evicted.
        let chunks = archive.retrieve(Some("Alpha"), 50_000);
        assert!(chunks.is_empty() || !chunks.iter().any(|c| c.content.contains("Alpha")));
    }

    #[test]
    fn evict_empty_returns_false() {
        let mut archive = ColdArchive::new(100);
        assert!(!archive.evict_oldest());
    }

    #[test]
    fn evict_updates_metrics() {
        let mut archive = ColdArchive::new(100);
        archive.store(&make_segment(1, 3, "Some content about Rust patterns"));
        let tokens_before = archive.original_tokens();
        let bytes_before = archive.compressed_bytes();

        archive.evict_oldest();
        assert_eq!(archive.len(), 0);
        assert!(archive.original_tokens() < tokens_before);
        assert!(archive.compressed_bytes() < bytes_before);
    }

    // --- Serialization ---

    #[test]
    fn serialize_deserialize_roundtrip() {
        let mut archive = ColdArchive::new(100);
        archive.store(&make_segment(1, 3, "Rust async patterns with tokio"));
        archive.store(&make_segment(4, 6, "Python Flask web server"));
        archive.store(&make_segment(7, 9, "SQLite WAL mode configuration"));

        let data = archive.serialize();
        let restored = ColdArchive::deserialize(&data, 100).unwrap();

        assert_eq!(restored.len(), 3);
        assert_eq!(restored.original_tokens(), archive.original_tokens());

        // Retrieve should work on restored archive.
        let chunks = restored.retrieve(Some("Rust async"), 50_000);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn deserialize_empty() {
        let data = 0u32.to_le_bytes();
        let archive = ColdArchive::deserialize(&data, 100).unwrap();
        assert!(archive.is_empty());
    }

    #[test]
    fn deserialize_truncated_returns_none() {
        let result = ColdArchive::deserialize(&[0, 1], 100);
        assert!(result.is_none());
    }

    // --- Disk persistence ---

    #[test]
    fn flush_and_load() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("archive.l4");

        let mut archive = ColdArchive::with_path(100, path.clone());
        archive.store(&make_segment(1, 5, "Rust async patterns with tokio runtime"));
        archive.store(&make_segment(6, 10, "SQLite database WAL mode setup"));

        let bytes = archive.flush_to_disk().unwrap();
        assert!(bytes > 0);
        assert_eq!(archive.pending_count(), 0);

        // Load from disk.
        let loaded = ColdArchive::load_from_disk(&path, 100).unwrap();
        assert_eq!(loaded.len(), 2);

        let chunks = loaded.retrieve(Some("Rust async"), 50_000);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn flush_without_path_returns_none() {
        let mut archive = ColdArchive::new(100);
        archive.store(&make_segment(1, 3, "Content"));
        assert!(archive.flush_to_disk().is_none());
    }

    // --- Compression ---

    #[test]
    fn compression_ratio_meaningful() {
        let mut archive = ColdArchive::new(100);
        let repetitive = "Discussing Rust async patterns and error handling. ".repeat(50);
        archive.store(&make_segment(1, 1, &repetitive));
        assert!(archive.compression_ratio() < 1.0);
    }

    #[test]
    fn compression_ratio_empty() {
        let archive = ColdArchive::new(100);
        assert_eq!(archive.compression_ratio(), 0.0);
    }

    // --- Key terms ---

    #[test]
    fn key_terms_extracted() {
        let terms = ColdArchive::extract_key_terms(
            "Rust async patterns with tokio runtime for error handling",
            4,
        );
        assert!(terms.len() <= 4);
        assert!(terms.iter().any(|t| t == "rust" || t == "async" || t == "tokio"));
    }

    // --- Pending flush tracking ---

    #[test]
    fn pending_count_tracks_unflushed() {
        let mut archive = ColdArchive::new(100);
        assert_eq!(archive.pending_count(), 0);
        archive.store(&make_segment(1, 1, "First"));
        assert_eq!(archive.pending_count(), 1);
        archive.store(&make_segment(2, 2, "Second"));
        assert_eq!(archive.pending_count(), 2);
    }

    // --- Budget enforcement ---

    #[test]
    fn retrieve_respects_budget() {
        let mut archive = ColdArchive::new(100);
        for i in 0..10 {
            archive.store(&make_segment(
                i,
                i,
                &format!(
                    "Segment about Rust async patterns number {i} with detailed \
                     discussion of error handling and tokio runtime configuration"
                ),
            ));
        }

        let tight = archive.retrieve(None, 20);
        let loose = archive.retrieve(None, 50_000);
        assert!(tight.len() <= loose.len());
    }

    // --- Max entries ---

    #[test]
    fn max_entries_accessor() {
        let archive = ColdArchive::new(42);
        assert_eq!(archive.max_entries(), 42);
    }
}
