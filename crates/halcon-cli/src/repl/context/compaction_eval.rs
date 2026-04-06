//! Compaction evaluation hooks (Fase 2).
//!
//! Structured metrics and probes for measuring compaction quality.
//! These hooks emit data that can be consumed by benchmarks, tests,
//! and observability pipelines to evaluate the runtime's behavior.
//!
//! Metrics tracked:
//!   - Continuation quality: does the agent repeat completed work?
//!   - Duplicate action rate: tool calls duplicated after compaction
//!   - Context inflation rate: token growth per round
//!   - Recovery handle usage: how often are persisted results recovered?
//!   - File re-read utility: did re-reads prevent repeated tool calls?
//!   - Compaction frequency: how often does compaction trigger?

use std::collections::HashSet;
use std::time::Instant;

// ── CompactionEvalMetrics ───────────────────────────────────────────────────

/// Structured evaluation metrics for the compaction subsystem.
///
/// Accumulated across the session lifetime. Emitted via tracing on
/// session end and available for programmatic access by benchmarks.
#[derive(Debug, Clone)]
pub struct CompactionEvalMetrics {
    /// Total compaction events this session.
    pub compaction_count: u32,

    /// Total tokens freed across all compaction events.
    pub total_tokens_freed: usize,

    /// Total tokens injected by file re-reads.
    pub total_reread_tokens: usize,

    /// Number of file re-reads performed.
    pub total_rereads: u32,

    /// Number of tool results persisted to disk.
    pub total_persisted: u32,

    /// Number of persisted results recovered by the agent.
    pub total_recoveries: u32,

    /// Number of tool results evicted (replaced with summaries).
    pub total_evicted: u32,

    /// Tokens freed by eviction.
    pub total_eviction_tokens_freed: usize,

    /// Number of tool results truncated inline.
    pub total_truncated: u32,

    /// Tool calls that were duplicates of prior calls (post-compaction repetition).
    pub duplicate_tool_calls_post_compaction: u32,

    /// Rounds that occurred after a compaction event.
    pub rounds_post_compaction: u32,

    /// Rounds post-compaction where progress was made.
    pub rounds_post_compaction_with_progress: u32,

    /// Token estimates at compaction trigger points.
    pub context_sizes_at_compaction: Vec<usize>,

    /// Session start time (for rate calculations).
    start: Instant,
}

impl CompactionEvalMetrics {
    pub fn new() -> Self {
        Self {
            compaction_count: 0,
            total_tokens_freed: 0,
            total_reread_tokens: 0,
            total_rereads: 0,
            total_persisted: 0,
            total_recoveries: 0,
            total_evicted: 0,
            total_eviction_tokens_freed: 0,
            total_truncated: 0,
            duplicate_tool_calls_post_compaction: 0,
            rounds_post_compaction: 0,
            rounds_post_compaction_with_progress: 0,
            context_sizes_at_compaction: Vec::new(),
            start: Instant::now(),
        }
    }

    // ── Recording methods ───────────────────────────────────────────────

    pub fn record_compaction(&mut self, tokens_before: usize, tokens_after: usize) {
        self.compaction_count += 1;
        self.total_tokens_freed += tokens_before.saturating_sub(tokens_after);
        self.context_sizes_at_compaction.push(tokens_before);
    }

    pub fn record_reread(&mut self, tokens_injected: usize, files: u32) {
        self.total_reread_tokens += tokens_injected;
        self.total_rereads += files;
    }

    pub fn record_persistence(&mut self, count: u32) {
        self.total_persisted += count;
    }

    pub fn record_recovery(&mut self) {
        self.total_recoveries += 1;
    }

    pub fn record_eviction(&mut self, count: u32, tokens_freed: usize) {
        self.total_evicted += count;
        self.total_eviction_tokens_freed += tokens_freed;
    }

    pub fn record_truncation(&mut self, count: u32) {
        self.total_truncated += count;
    }

    pub fn record_post_compaction_round(&mut self, had_progress: bool) {
        self.rounds_post_compaction += 1;
        if had_progress {
            self.rounds_post_compaction_with_progress += 1;
        }
    }

    pub fn record_duplicate_tool_call(&mut self) {
        self.duplicate_tool_calls_post_compaction += 1;
    }

    // ── Derived metrics ─────────────────────────────────────────────────

    /// Post-compaction completion rate: fraction of post-compaction rounds
    /// that made forward progress. Higher is better.
    pub fn post_compaction_completion_rate(&self) -> f64 {
        if self.rounds_post_compaction == 0 {
            return 1.0; // No compaction → no regression
        }
        self.rounds_post_compaction_with_progress as f64 / self.rounds_post_compaction as f64
    }

    /// Context inflation reduction: total tokens freed by all mechanisms
    /// (eviction + truncation + compaction) per compaction event.
    pub fn avg_tokens_freed_per_compaction(&self) -> f64 {
        if self.compaction_count == 0 {
            return 0.0;
        }
        (self.total_tokens_freed + self.total_eviction_tokens_freed) as f64
            / self.compaction_count as f64
    }

    /// Recovery utilization: fraction of persisted results that were actually recovered.
    pub fn recovery_utilization(&self) -> f64 {
        if self.total_persisted == 0 {
            return 0.0;
        }
        self.total_recoveries as f64 / self.total_persisted as f64
    }

    /// Duplicate action rate post-compaction: lower is better.
    pub fn duplicate_action_rate(&self) -> f64 {
        if self.rounds_post_compaction == 0 {
            return 0.0;
        }
        self.duplicate_tool_calls_post_compaction as f64 / self.rounds_post_compaction as f64
    }

    // ── Emission ────────────────────────────────────────────────────────

    /// Emit all metrics via structured tracing.
    pub fn emit_summary(&self) {
        tracing::info!(
            compaction_count = self.compaction_count,
            total_tokens_freed = self.total_tokens_freed,
            total_reread_tokens = self.total_reread_tokens,
            total_rereads = self.total_rereads,
            total_persisted = self.total_persisted,
            total_recoveries = self.total_recoveries,
            recovery_utilization = format!("{:.2}", self.recovery_utilization()),
            total_evicted = self.total_evicted,
            eviction_tokens_freed = self.total_eviction_tokens_freed,
            total_truncated = self.total_truncated,
            post_compaction_completion_rate =
                format!("{:.2}", self.post_compaction_completion_rate()),
            duplicate_action_rate = format!("{:.2}", self.duplicate_action_rate()),
            avg_freed_per_compaction = format!("{:.0}", self.avg_tokens_freed_per_compaction()),
            rounds_post_compaction = self.rounds_post_compaction,
            session_duration_secs = self.start.elapsed().as_secs(),
            "compaction_eval_summary"
        );
    }
}

// ── DuplicateToolDetector ───────────────────────────────────────────────────

/// Detects tool calls that duplicate prior calls (post-compaction repetition).
///
/// Uses (tool_name, args_hash) pairs to detect when the agent re-executes
/// a tool call it already performed before compaction removed the context.
pub struct DuplicateToolDetector {
    /// Set of (tool_name, args_hash) pairs seen before compaction.
    pre_compaction_calls: HashSet<(String, u64)>,
    /// Whether compaction has occurred (only track post-compaction duplicates).
    compaction_occurred: bool,
}

impl DuplicateToolDetector {
    pub fn new() -> Self {
        Self {
            pre_compaction_calls: HashSet::new(),
            compaction_occurred: false,
        }
    }

    /// Record a tool call (call this every round).
    pub fn record_call(&mut self, tool_name: &str, args_hash: u64) {
        if !self.compaction_occurred {
            self.pre_compaction_calls
                .insert((tool_name.to_string(), args_hash));
        }
    }

    /// Signal that compaction occurred.
    pub fn mark_compaction(&mut self) {
        self.compaction_occurred = true;
    }

    /// Check if a tool call is a duplicate of a pre-compaction call.
    pub fn is_post_compaction_duplicate(&self, tool_name: &str, args_hash: u64) -> bool {
        self.compaction_occurred
            && self
                .pre_compaction_calls
                .contains(&(tool_name.to_string(), args_hash))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_defaults() {
        let m = CompactionEvalMetrics::new();
        assert_eq!(m.compaction_count, 0);
        assert_eq!(m.post_compaction_completion_rate(), 1.0);
        assert_eq!(m.duplicate_action_rate(), 0.0);
        assert_eq!(m.recovery_utilization(), 0.0);
    }

    #[test]
    fn post_compaction_completion_rate() {
        let mut m = CompactionEvalMetrics::new();
        m.record_post_compaction_round(true);
        m.record_post_compaction_round(true);
        m.record_post_compaction_round(false);
        assert!((m.post_compaction_completion_rate() - 0.666).abs() < 0.01);
    }

    #[test]
    fn recovery_utilization_tracking() {
        let mut m = CompactionEvalMetrics::new();
        m.record_persistence(10);
        m.record_recovery();
        m.record_recovery();
        assert!((m.recovery_utilization() - 0.2).abs() < 0.01);
    }

    #[test]
    fn avg_tokens_freed() {
        let mut m = CompactionEvalMetrics::new();
        m.record_compaction(100_000, 60_000);
        m.record_compaction(90_000, 50_000);
        m.record_eviction(5, 10_000);
        assert!((m.avg_tokens_freed_per_compaction() - 45_000.0).abs() < 1.0);
    }

    #[test]
    fn duplicate_detector_pre_compaction() {
        let mut d = DuplicateToolDetector::new();
        d.record_call("bash", 12345);
        // No compaction yet → not a duplicate
        assert!(!d.is_post_compaction_duplicate("bash", 12345));
    }

    #[test]
    fn duplicate_detector_post_compaction() {
        let mut d = DuplicateToolDetector::new();
        d.record_call("bash", 12345);
        d.mark_compaction();
        // Same call after compaction → duplicate
        assert!(d.is_post_compaction_duplicate("bash", 12345));
        // Different call → not duplicate
        assert!(!d.is_post_compaction_duplicate("bash", 99999));
    }

    #[test]
    fn duplicate_action_rate_calculation() {
        let mut m = CompactionEvalMetrics::new();
        m.record_post_compaction_round(true);
        m.record_post_compaction_round(true);
        m.record_duplicate_tool_call();
        assert!((m.duplicate_action_rate() - 0.5).abs() < 0.01);
    }
}
