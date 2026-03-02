//! Structured Observability — Per-round and session-level metrics (P4.3).
//!
//! Collects quantitative metrics from the agent loop for structured logging,
//! dashboards, and post-session analysis. Unlike `RoundFeedback` (which carries
//! decision signals for the oracle), `SystemMetrics` captures pure observational
//! data with no decision semantics.
//!
//! # Metric Categories
//!
//! | Category | Metrics | Source |
//! |----------|---------|--------|
//! | Throughput | tokens_in, tokens_out, tool_calls | TokenAccounting, post_batch |
//! | Quality | utility_score, combined_score, evidence_coverage | ConvergenceUtility, RoundScorer |
//! | Health | invariant_violations, cycle_count, drift_score | SystemInvariantChecker, SemanticCycle |
//! | Budget | sla_fraction, token_fraction, replan_attempts | SlaBudget, TokenAccounting |
//! | Timing | round_duration_ms | wall clock |
//!
//! Pure business logic — no I/O.

use std::time::Duration;

// ── RoundMetrics ─────────────────────────────────────────────────────────────

/// Quantitative metrics for a single round.
#[derive(Debug, Clone)]
pub struct RoundMetrics {
    /// Round number (0-based).
    pub round: usize,
    /// Input tokens consumed this round.
    pub tokens_in: u64,
    /// Output tokens produced this round.
    pub tokens_out: u64,
    /// Number of tool calls executed this round.
    pub tool_calls: usize,
    /// Number of tool errors this round.
    pub tool_errors: usize,
    /// Combined score from RoundScorer [0.0, 1.0].
    pub combined_score: f32,
    /// Utility score from ConvergenceUtility [0.0, 1.5].
    pub utility_score: f64,
    /// Evidence coverage from EvidenceGraph [0.0, 1.0].
    pub evidence_coverage: f64,
    /// Cumulative drift score.
    pub drift_score: f32,
    /// SLA budget fraction consumed [0.0, 1.0+].
    pub sla_fraction: f64,
    /// Token budget fraction consumed [0.0, 1.0+].
    pub token_fraction: f64,
    /// Replan attempts so far.
    pub replan_attempts: u32,
    /// Invariant violations detected this round.
    pub invariant_violations: usize,
    /// Semantic cycle count.
    pub cycle_count: usize,
    /// Wall-clock duration for this round.
    pub round_duration: Duration,
    /// Oracle decision for this round (as string label).
    pub oracle_decision: String,
}

// ── SessionMetrics ───────────────────────────────────────────────────────────

/// Aggregate metrics across the full session.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// Total rounds executed.
    pub total_rounds: usize,
    /// Total input tokens.
    pub total_tokens_in: u64,
    /// Total output tokens.
    pub total_tokens_out: u64,
    /// Total cost in USD.
    pub total_cost: f64,
    /// Total tool calls across all rounds.
    pub total_tool_calls: usize,
    /// Total tool errors across all rounds.
    pub total_tool_errors: usize,
    /// Final utility score.
    pub final_utility: f64,
    /// Final evidence coverage.
    pub final_evidence_coverage: f64,
    /// Peak drift score observed.
    pub peak_drift: f32,
    /// Total invariant violations.
    pub total_invariant_violations: usize,
    /// Total semantic cycles detected.
    pub total_cycles: usize,
    /// Total replans triggered.
    pub total_replans: u32,
    /// Total session wall-clock duration.
    pub total_duration: Duration,
    /// Whether any critical invariant violation occurred.
    pub had_critical_violation: bool,
}

// ── MetricsCollector ─────────────────────────────────────────────────────────

/// Stateful metrics collector that accumulates per-round metrics.
pub struct MetricsCollector {
    rounds: Vec<RoundMetrics>,
}

impl MetricsCollector {
    /// Create a new empty collector.
    pub fn new() -> Self {
        Self {
            rounds: Vec::new(),
        }
    }

    /// Record a round's metrics.
    pub fn record_round(&mut self, metrics: RoundMetrics) {
        self.rounds.push(metrics);
    }

    /// All recorded round metrics.
    pub fn rounds(&self) -> &[RoundMetrics] {
        &self.rounds
    }

    /// Number of rounds recorded.
    pub fn round_count(&self) -> usize {
        self.rounds.len()
    }

    /// Compute session summary from all recorded rounds.
    pub fn summarize(&self, total_cost: f64, had_critical: bool) -> SessionSummary {
        let total_rounds = self.rounds.len();
        let total_tokens_in: u64 = self.rounds.iter().map(|r| r.tokens_in).sum();
        let total_tokens_out: u64 = self.rounds.iter().map(|r| r.tokens_out).sum();
        let total_tool_calls: usize = self.rounds.iter().map(|r| r.tool_calls).sum();
        let total_tool_errors: usize = self.rounds.iter().map(|r| r.tool_errors).sum();
        let total_invariant_violations: usize = self.rounds.iter().map(|r| r.invariant_violations).sum();
        let total_cycles: usize = self.rounds.iter().map(|r| r.cycle_count).sum();
        let total_replans = self.rounds.last().map(|r| r.replan_attempts).unwrap_or(0);
        let total_duration: Duration = self.rounds.iter().map(|r| r.round_duration).sum();
        let peak_drift = self.rounds.iter().map(|r| r.drift_score).fold(0.0f32, f32::max);
        let final_utility = self.rounds.last().map(|r| r.utility_score).unwrap_or(0.5);
        let final_evidence_coverage = self.rounds.last().map(|r| r.evidence_coverage).unwrap_or(0.0);

        SessionSummary {
            total_rounds,
            total_tokens_in,
            total_tokens_out,
            total_cost,
            total_tool_calls,
            total_tool_errors,
            final_utility,
            final_evidence_coverage,
            peak_drift,
            total_invariant_violations,
            total_cycles,
            total_replans,
            total_duration,
            had_critical_violation: had_critical,
        }
    }

    /// Average combined score across all rounds.
    pub fn avg_combined_score(&self) -> f32 {
        if self.rounds.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.rounds.iter().map(|r| r.combined_score).sum();
        sum / self.rounds.len() as f32
    }

    /// Average tokens per round (input + output).
    pub fn avg_tokens_per_round(&self) -> f64 {
        if self.rounds.is_empty() {
            return 0.0;
        }
        let total: u64 = self.rounds.iter().map(|r| r.tokens_in + r.tokens_out).sum();
        total as f64 / self.rounds.len() as f64
    }

    /// Tool error rate across all rounds.
    pub fn tool_error_rate(&self) -> f64 {
        let calls: usize = self.rounds.iter().map(|r| r.tool_calls).sum();
        let errors: usize = self.rounds.iter().map(|r| r.tool_errors).sum();
        if calls == 0 {
            return 0.0;
        }
        errors as f64 / calls as f64
    }

    /// Utility score trend: difference between last and first utility.
    pub fn utility_trend(&self) -> f64 {
        if self.rounds.len() < 2 {
            return 0.0;
        }
        self.rounds.last().unwrap().utility_score - self.rounds.first().unwrap().utility_score
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_round(round: usize) -> RoundMetrics {
        RoundMetrics {
            round,
            tokens_in: 1000,
            tokens_out: 500,
            tool_calls: 3,
            tool_errors: 0,
            combined_score: 0.7,
            utility_score: 0.6,
            evidence_coverage: 0.5,
            drift_score: 0.2,
            sla_fraction: 0.3,
            token_fraction: 0.25,
            replan_attempts: 0,
            invariant_violations: 0,
            cycle_count: 0,
            round_duration: Duration::from_millis(500),
            oracle_decision: "Continue".into(),
        }
    }

    #[test]
    fn phase4_metrics_empty_collector() {
        let collector = MetricsCollector::new();
        assert_eq!(collector.round_count(), 0);
        assert_eq!(collector.avg_combined_score(), 0.0);
        assert_eq!(collector.avg_tokens_per_round(), 0.0);
        assert_eq!(collector.tool_error_rate(), 0.0);
        assert_eq!(collector.utility_trend(), 0.0);
    }

    #[test]
    fn phase4_metrics_record_and_count() {
        let mut collector = MetricsCollector::new();
        collector.record_round(make_round(0));
        collector.record_round(make_round(1));
        assert_eq!(collector.round_count(), 2);
        assert_eq!(collector.rounds()[0].round, 0);
        assert_eq!(collector.rounds()[1].round, 1);
    }

    #[test]
    fn phase4_metrics_avg_combined_score() {
        let mut collector = MetricsCollector::new();
        let mut r0 = make_round(0);
        r0.combined_score = 0.4;
        let mut r1 = make_round(1);
        r1.combined_score = 0.8;
        collector.record_round(r0);
        collector.record_round(r1);
        let avg = collector.avg_combined_score();
        assert!((avg - 0.6).abs() < 1e-4, "expected 0.6, got {avg}");
    }

    #[test]
    fn phase4_metrics_avg_tokens_per_round() {
        let mut collector = MetricsCollector::new();
        let mut r0 = make_round(0);
        r0.tokens_in = 2000;
        r0.tokens_out = 1000;
        let mut r1 = make_round(1);
        r1.tokens_in = 1000;
        r1.tokens_out = 500;
        collector.record_round(r0);
        collector.record_round(r1);
        // Total: (2000+1000) + (1000+500) = 4500, avg = 2250
        let avg = collector.avg_tokens_per_round();
        assert!((avg - 2250.0).abs() < 1e-4, "expected 2250, got {avg}");
    }

    #[test]
    fn phase4_metrics_tool_error_rate() {
        let mut collector = MetricsCollector::new();
        let mut r0 = make_round(0);
        r0.tool_calls = 4;
        r0.tool_errors = 1;
        let mut r1 = make_round(1);
        r1.tool_calls = 6;
        r1.tool_errors = 2;
        collector.record_round(r0);
        collector.record_round(r1);
        // 3 errors / 10 calls = 0.30
        let rate = collector.tool_error_rate();
        assert!((rate - 0.30).abs() < 1e-4, "expected 0.30, got {rate}");
    }

    #[test]
    fn phase4_metrics_tool_error_rate_no_calls() {
        let mut collector = MetricsCollector::new();
        let mut r0 = make_round(0);
        r0.tool_calls = 0;
        r0.tool_errors = 0;
        collector.record_round(r0);
        assert_eq!(collector.tool_error_rate(), 0.0);
    }

    #[test]
    fn phase4_metrics_utility_trend() {
        let mut collector = MetricsCollector::new();
        let mut r0 = make_round(0);
        r0.utility_score = 0.3;
        let mut r1 = make_round(1);
        r1.utility_score = 0.7;
        collector.record_round(r0);
        collector.record_round(r1);
        let trend = collector.utility_trend();
        assert!((trend - 0.4).abs() < 1e-4, "expected 0.4, got {trend}");
    }

    #[test]
    fn phase4_metrics_utility_trend_single_round() {
        let mut collector = MetricsCollector::new();
        collector.record_round(make_round(0));
        assert_eq!(collector.utility_trend(), 0.0);
    }

    #[test]
    fn phase4_metrics_session_summary() {
        let mut collector = MetricsCollector::new();
        let mut r0 = make_round(0);
        r0.tokens_in = 1000;
        r0.tokens_out = 500;
        r0.tool_calls = 3;
        r0.tool_errors = 1;
        r0.invariant_violations = 0;
        r0.cycle_count = 0;
        r0.replan_attempts = 0;
        r0.drift_score = 0.3;

        let mut r1 = make_round(1);
        r1.tokens_in = 2000;
        r1.tokens_out = 800;
        r1.tool_calls = 5;
        r1.tool_errors = 0;
        r1.invariant_violations = 1;
        r1.cycle_count = 1;
        r1.replan_attempts = 1;
        r1.drift_score = 0.5;
        r1.utility_score = 0.8;
        r1.evidence_coverage = 0.75;

        collector.record_round(r0);
        collector.record_round(r1);

        let summary = collector.summarize(0.005, false);
        assert_eq!(summary.total_rounds, 2);
        assert_eq!(summary.total_tokens_in, 3000);
        assert_eq!(summary.total_tokens_out, 1300);
        assert_eq!(summary.total_tool_calls, 8);
        assert_eq!(summary.total_tool_errors, 1);
        assert_eq!(summary.total_invariant_violations, 1);
        assert_eq!(summary.total_cycles, 1);
        assert_eq!(summary.total_replans, 1);
        assert!((summary.peak_drift - 0.5).abs() < 1e-4);
        assert!((summary.final_utility - 0.8).abs() < 1e-4);
        assert!((summary.final_evidence_coverage - 0.75).abs() < 1e-4);
        assert!(!summary.had_critical_violation);
    }

    #[test]
    fn phase4_metrics_session_summary_empty() {
        let collector = MetricsCollector::new();
        let summary = collector.summarize(0.0, false);
        assert_eq!(summary.total_rounds, 0);
        assert_eq!(summary.total_tokens_in, 0);
        assert_eq!(summary.final_utility, 0.5);
    }
}
