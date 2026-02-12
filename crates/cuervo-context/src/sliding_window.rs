//! L1 Sliding Window: ordered segments of compacted conversation context.
//!
//! Stores summaries of evicted L0 messages as ContextSegments, grouped by round.
//! Supports merging of adjacent small segments to reduce count and
//! retrieval as context chunks for model prompt assembly.

use cuervo_core::traits::ContextChunk;

use crate::segment::ContextSegment;

/// L1: Sliding window of conversation segments.
pub struct SlidingWindow {
    segments: Vec<ContextSegment>,
    token_count: u32,
}

impl SlidingWindow {
    /// Create an empty sliding window.
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
            token_count: 0,
        }
    }

    /// Push a segment into the window.
    pub fn push(&mut self, segment: ContextSegment) {
        self.token_count += segment.token_estimate;
        self.segments.push(segment);
    }

    /// Evict the oldest segment, returning it.
    pub fn evict_oldest(&mut self) -> Option<ContextSegment> {
        if self.segments.is_empty() {
            return None;
        }
        let seg = self.segments.remove(0);
        self.token_count = self.token_count.saturating_sub(seg.token_estimate);
        Some(seg)
    }

    /// Merge adjacent segments that are smaller than the threshold.
    pub fn merge_adjacent(&mut self, max_merged_tokens: u32) {
        let mut i = 0;
        while i + 1 < self.segments.len() {
            let combined =
                self.segments[i].token_estimate + self.segments[i + 1].token_estimate;
            if combined <= max_merged_tokens {
                let next = self.segments.remove(i + 1);
                let merged = ContextSegment::merge(&self.segments[i], &next);
                self.segments[i] = merged;
                // Don't increment i — check if merged segment can merge with next
            } else {
                i += 1;
            }
        }
        self.recalculate_tokens();
    }

    /// Retrieve segments as context chunks, up to the budget.
    pub fn retrieve(&self, budget: u32) -> Vec<ContextChunk> {
        let mut chunks = Vec::new();
        let mut remaining = budget;
        for seg in &self.segments {
            if seg.token_estimate <= remaining {
                chunks.push(ContextChunk {
                    source: format!("l1:rounds_{}-{}", seg.round_start, seg.round_end),
                    priority: 80,
                    content: seg.to_context_string(),
                    estimated_tokens: seg.token_estimate as usize,
                });
                remaining -= seg.token_estimate;
            }
        }
        chunks
    }

    /// Current total token count across all segments.
    pub fn token_count(&self) -> u32 {
        self.token_count
    }

    /// Number of segments.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Whether the window is empty.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Borrow the segments.
    pub fn segments(&self) -> &[ContextSegment] {
        &self.segments
    }

    fn recalculate_tokens(&mut self) {
        self.token_count = self.segments.iter().map(|s| s.token_estimate).sum();
    }
}

impl Default for SlidingWindow {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_segment(start: u32, end: u32, summary: &str, tokens: u32) -> ContextSegment {
        ContextSegment {
            round_start: start,
            round_end: end,
            summary: summary.to_string(),
            decisions: vec![],
            files_modified: vec![],
            tools_used: vec![],
            token_estimate: tokens,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn empty_window() {
        let window = SlidingWindow::new();
        assert!(window.is_empty());
        assert_eq!(window.len(), 0);
        assert_eq!(window.token_count(), 0);
    }

    #[test]
    fn push_increases_count() {
        let mut window = SlidingWindow::new();
        window.push(make_segment(1, 2, "summary", 100));
        assert_eq!(window.len(), 1);
        assert_eq!(window.token_count(), 100);
    }

    #[test]
    fn push_multiple() {
        let mut window = SlidingWindow::new();
        window.push(make_segment(1, 2, "first", 100));
        window.push(make_segment(3, 4, "second", 200));
        assert_eq!(window.len(), 2);
        assert_eq!(window.token_count(), 300);
    }

    #[test]
    fn evict_oldest() {
        let mut window = SlidingWindow::new();
        window.push(make_segment(1, 2, "first", 100));
        window.push(make_segment(3, 4, "second", 200));

        let evicted = window.evict_oldest();
        assert!(evicted.is_some());
        let seg = evicted.unwrap();
        assert_eq!(seg.round_start, 1);
        assert_eq!(seg.summary, "first");
        assert_eq!(window.len(), 1);
        assert_eq!(window.token_count(), 200);
    }

    #[test]
    fn evict_oldest_empty() {
        let mut window = SlidingWindow::new();
        assert!(window.evict_oldest().is_none());
    }

    #[test]
    fn merge_adjacent_small_segments() {
        let mut window = SlidingWindow::new();
        // Use segments with large enough summaries that merged pair exceeds threshold
        let large_a = "a".repeat(800); // 800 chars = 200 tokens
        let large_b = "b".repeat(800); // 800 chars = 200 tokens
        let large_c = "c".repeat(800); // 800 chars = 200 tokens
        window.push(make_segment(1, 2, &large_a, 200));
        window.push(make_segment(3, 4, &large_b, 200));
        window.push(make_segment(5, 6, &large_c, 200));

        // Threshold of 500: a+b merged summary ≈ 400 tokens, then merged+c ≈ 600 > 500
        window.merge_adjacent(500);
        assert_eq!(window.len(), 2);
        assert_eq!(window.segments()[0].round_start, 1);
        assert_eq!(window.segments()[0].round_end, 4);
    }

    #[test]
    fn merge_all_small() {
        let mut window = SlidingWindow::new();
        window.push(make_segment(1, 1, "a", 10));
        window.push(make_segment(2, 2, "b", 10));
        window.push(make_segment(3, 3, "c", 10));

        // Threshold of 1000: all should merge into one
        window.merge_adjacent(1000);
        assert_eq!(window.len(), 1);
        assert_eq!(window.segments()[0].round_start, 1);
        assert_eq!(window.segments()[0].round_end, 3);
    }

    #[test]
    fn merge_none_when_all_large() {
        let mut window = SlidingWindow::new();
        window.push(make_segment(1, 1, "a", 500));
        window.push(make_segment(2, 2, "b", 500));

        window.merge_adjacent(100); // Threshold too small
        assert_eq!(window.len(), 2);
    }

    #[test]
    fn retrieve_within_budget() {
        let mut window = SlidingWindow::new();
        window.push(make_segment(1, 2, "summary one", 100));
        window.push(make_segment(3, 4, "summary two", 200));

        let chunks = window.retrieve(500); // plenty of budget
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].priority, 80);
        assert!(chunks[0].content.contains("[Rounds 1-2]"));
    }

    #[test]
    fn retrieve_respects_budget() {
        let mut window = SlidingWindow::new();
        window.push(make_segment(1, 2, "first", 100));
        window.push(make_segment(3, 4, "second", 200));

        let chunks = window.retrieve(150); // only room for first
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("[Rounds 1-2]"));
    }

    #[test]
    fn retrieve_empty_returns_empty() {
        let window = SlidingWindow::new();
        let chunks = window.retrieve(1000);
        assert!(chunks.is_empty());
    }

    #[test]
    fn segments_borrow() {
        let mut window = SlidingWindow::new();
        window.push(make_segment(1, 1, "test", 50));
        let segs = window.segments();
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn token_count_after_merge() {
        let mut window = SlidingWindow::new();
        window.push(make_segment(1, 1, "a", 50));
        window.push(make_segment(2, 2, "b", 50));
        let before = window.token_count();

        window.merge_adjacent(1000);
        let after = window.token_count();
        // After merge, token count recalculated from actual summary
        // It may differ from before since merged summary is re-estimated
        assert!(after > 0);
        let _ = before; // suppress unused
    }
}
