//! Compression and delta encoding for context tiers.
//!
//! Provides zstd compression for cold-tier context segments (L2+)
//! and delta encoding to reduce redundancy between adjacent segments.

use serde::{Deserialize, Serialize};

/// Default zstd compression level (3 = good balance of speed/ratio).
const ZSTD_LEVEL: i32 = 3;

/// Minimum byte size to bother compressing (below this, overhead > savings).
const MIN_COMPRESS_SIZE: usize = 256;

/// A compressed block of context data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedBlock {
    /// Original uncompressed size in bytes.
    pub original_size: usize,
    /// Compressed data (zstd).
    pub data: Vec<u8>,
}

impl CompressedBlock {
    /// Compression ratio (compressed / original). Lower is better.
    pub fn ratio(&self) -> f64 {
        if self.original_size == 0 {
            return 1.0;
        }
        self.data.len() as f64 / self.original_size as f64
    }
}

/// Compress a string with zstd. Returns None if input is too small to benefit.
pub fn compress(input: &str) -> Option<CompressedBlock> {
    if input.len() < MIN_COMPRESS_SIZE {
        return None;
    }
    let data = zstd::encode_all(input.as_bytes(), ZSTD_LEVEL).ok()?;
    // Only keep if compression actually helps (at least 10% savings).
    if data.len() >= input.len() * 9 / 10 {
        return None;
    }
    Some(CompressedBlock {
        original_size: input.len(),
        data,
    })
}

/// Decompress a zstd-compressed block back to a string.
pub fn decompress(block: &CompressedBlock) -> Option<String> {
    let bytes = zstd::decode_all(block.data.as_slice()).ok()?;
    String::from_utf8(bytes).ok()
}

/// A delta-encoded representation of text relative to a base.
///
/// Uses a simple operation-based diff: retain N chars, insert "...", skip N chars.
/// Optimized for similar adjacent segments where most content is shared.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaEncoded {
    /// Operations to reconstruct the target from the base.
    pub ops: Vec<DeltaOp>,
    /// Token estimate for the reconstructed text.
    pub token_estimate: u32,
}

/// A single delta operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeltaOp {
    /// Copy `len` bytes from the base at offset `base_offset`.
    Copy { base_offset: usize, len: usize },
    /// Insert new content not in the base.
    Insert(String),
}

/// Compute a delta encoding of `target` relative to `base`.
///
/// Uses a simple longest-common-subsequence approach at line granularity
/// for efficiency. Falls back to full insertion if texts are very different.
pub fn delta_encode(base: &str, target: &str) -> DeltaEncoded {
    let base_lines: Vec<&str> = base.lines().collect();
    let target_lines: Vec<&str> = target.lines().collect();

    let mut ops = Vec::new();
    let mut ti = 0;

    while ti < target_lines.len() {
        // Try to find target_lines[ti] in base_lines.
        if let Some(bi) = base_lines.iter().position(|&bl| bl == target_lines[ti]) {
            // Found a match — try to extend the copy run.
            let mut run_len = 1;
            while ti + run_len < target_lines.len()
                && bi + run_len < base_lines.len()
                && target_lines[ti + run_len] == base_lines[bi + run_len]
            {
                run_len += 1;
            }
            // Calculate byte offset and length in the base string.
            let base_offset: usize = base_lines[..bi].iter().map(|l| l.len() + 1).sum();
            let copy_len: usize = base_lines[bi..bi + run_len]
                .iter()
                .map(|l| l.len() + 1)
                .sum();
            ops.push(DeltaOp::Copy {
                base_offset,
                len: copy_len,
            });
            ti += run_len;
        } else {
            // No match — insert this line.
            ops.push(DeltaOp::Insert(format!("{}\n", target_lines[ti])));
            ti += 1;
        }
    }

    let token_estimate = crate::assembler::estimate_tokens(target) as u32;
    DeltaEncoded {
        ops,
        token_estimate,
    }
}

/// Reconstruct text from a base and delta encoding.
pub fn delta_decode(base: &str, delta: &DeltaEncoded) -> String {
    let base_bytes = base.as_bytes();
    let mut result = String::new();

    for op in &delta.ops {
        match op {
            DeltaOp::Copy { base_offset, len } => {
                let end = (*base_offset + *len).min(base_bytes.len());
                if let Ok(s) = std::str::from_utf8(&base_bytes[*base_offset..end]) {
                    result.push_str(s);
                }
            }
            DeltaOp::Insert(text) => {
                result.push_str(text);
            }
        }
    }

    result
}

/// Size of the delta encoding in bytes (for budget accounting).
pub fn delta_size(delta: &DeltaEncoded) -> usize {
    delta
        .ops
        .iter()
        .map(|op| match op {
            DeltaOp::Copy { .. } => 16, // 2 x usize
            DeltaOp::Insert(s) => s.len(),
        })
        .sum()
}

/// Check if delta encoding is more efficient than storing the full text.
pub fn delta_is_efficient(base: &str, target: &str) -> bool {
    let delta = delta_encode(base, target);
    delta_size(&delta) < target.len() * 3 / 4 // at least 25% savings
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- zstd compression tests ---

    #[test]
    fn compress_small_returns_none() {
        assert!(compress("short").is_none());
    }

    #[test]
    fn compress_decompress_roundtrip() {
        let text = "This is a longer text that should be compressible. ".repeat(20);
        let block = compress(&text).expect("should compress");
        assert!(block.data.len() < text.len());
        assert!(block.ratio() < 1.0);
        let decompressed = decompress(&block).expect("should decompress");
        assert_eq!(decompressed, text);
    }

    #[test]
    fn compress_incompressible_returns_none() {
        // Random-like data that doesn't compress well.
        let text: String = (0..300).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        // This may or may not compress; if it does, ratio should still be checked.
        if let Some(block) = compress(&text) {
            assert!(block.ratio() < 0.9);
        }
    }

    #[test]
    fn compressed_block_ratio() {
        let block = CompressedBlock {
            original_size: 1000,
            data: vec![0; 500],
        };
        assert!((block.ratio() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn compressed_block_ratio_zero_original() {
        let block = CompressedBlock {
            original_size: 0,
            data: vec![],
        };
        assert!((block.ratio() - 1.0).abs() < f64::EPSILON);
    }

    // --- Delta encoding tests ---

    #[test]
    fn delta_identical_texts() {
        let text = "line 1\nline 2\nline 3\n";
        let delta = delta_encode(text, text);
        let decoded = delta_decode(text, &delta);
        assert_eq!(decoded, text);
        // All ops should be Copy.
        for op in &delta.ops {
            assert!(matches!(op, DeltaOp::Copy { .. }));
        }
    }

    #[test]
    fn delta_completely_different() {
        let base = "aaa\nbbb\nccc\n";
        let target = "xxx\nyyy\nzzz\n";
        let delta = delta_encode(base, target);
        let decoded = delta_decode(base, &delta);
        assert_eq!(decoded, target);
        // All ops should be Insert.
        for op in &delta.ops {
            assert!(matches!(op, DeltaOp::Insert(_)));
        }
    }

    #[test]
    fn delta_partial_overlap() {
        let base = "line 1\nline 2\nline 3\nline 4\n";
        let target = "line 1\nnew line\nline 3\nline 4\n";
        let delta = delta_encode(base, target);
        let decoded = delta_decode(base, &delta);
        assert_eq!(decoded, target);
        // Should have a mix of Copy and Insert ops.
        let has_copy = delta.ops.iter().any(|op| matches!(op, DeltaOp::Copy { .. }));
        let has_insert = delta.ops.iter().any(|op| matches!(op, DeltaOp::Insert(_)));
        assert!(has_copy, "expected at least one Copy op");
        assert!(has_insert, "expected at least one Insert op");
    }

    #[test]
    fn delta_empty_base() {
        let delta = delta_encode("", "hello\nworld\n");
        let decoded = delta_decode("", &delta);
        assert_eq!(decoded, "hello\nworld\n");
    }

    #[test]
    fn delta_empty_target() {
        let delta = delta_encode("hello\nworld\n", "");
        let decoded = delta_decode("hello\nworld\n", &delta);
        assert_eq!(decoded, "");
    }

    #[test]
    fn delta_size_all_insert() {
        let delta = DeltaEncoded {
            ops: vec![DeltaOp::Insert("hello world\n".to_string())],
            token_estimate: 3,
        };
        assert_eq!(delta_size(&delta), 12); // "hello world\n".len()
    }

    #[test]
    fn delta_size_all_copy() {
        let delta = DeltaEncoded {
            ops: vec![DeltaOp::Copy {
                base_offset: 0,
                len: 100,
            }],
            token_estimate: 25,
        };
        assert_eq!(delta_size(&delta), 16); // 2 x usize
    }

    #[test]
    fn delta_is_efficient_similar() {
        // Longer text to ensure Copy ops amortize their overhead.
        let base_lines: Vec<String> = (0..20).map(|i| format!("context line {} with some detail about the conversation", i)).collect();
        let mut target_lines = base_lines.clone();
        target_lines[10] = "modified line with new content".to_string();
        let base = base_lines.join("\n") + "\n";
        let target = target_lines.join("\n") + "\n";
        assert!(delta_is_efficient(&base, &target));
    }

    #[test]
    fn delta_is_efficient_very_different() {
        let base_lines: Vec<String> = (0..10).map(|i| format!("aaa_{i}")).collect();
        let target_lines: Vec<String> = (0..10).map(|i| format!("zzz_{i}")).collect();
        let base = base_lines.join("\n") + "\n";
        let target = target_lines.join("\n") + "\n";
        assert!(!delta_is_efficient(&base, &target));
    }

    #[test]
    fn delta_token_estimate() {
        let target = "hello world this is a test";
        let delta = delta_encode("", target);
        assert!(delta.token_estimate > 0);
    }

    #[test]
    fn compress_large_context_segment() {
        // Simulate a real context segment (repetitive conversation data).
        let segment = "[Rounds 1-5] User asked about Rust async patterns. \
            We discussed tokio, futures, and async-await. \
            Decisions: Use tokio multi-thread runtime. \
            Files: src/main.rs, src/lib.rs, Cargo.toml. \
            Tools: file_read, bash, grep.\n"
            .repeat(10);
        let block = compress(&segment).expect("should compress");
        assert!(block.ratio() < 0.5, "expected >50% compression for repetitive context");
        let decompressed = decompress(&block).unwrap();
        assert_eq!(decompressed, segment);
    }
}
