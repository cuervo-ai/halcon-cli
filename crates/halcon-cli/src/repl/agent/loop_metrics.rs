//! Structured loop observability (Frontier AAA).
//!
//! Captures structured metrics from the agent loop for diagnostics,
//! post-mortem analysis, and runtime monitoring.
//!
//! Design rationale (papers):
//!   - Rabanser 2026: "Track reliability separately from capability — measure
//!     consistency, robustness, predictability, and safety."
//!   - AIOS (COLM 2025): Model the agent runtime as an operating system with
//!     system-call-level observability.

use std::time::{Duration, Instant};

use super::feedback_arbiter::RecoveryAction;

// ── Recovery event ──────────────────────────────────────────────────────────

/// A single recovery event recorded during the loop.
#[derive(Debug, Clone)]
pub struct RecoveryEvent {
    /// Round number when recovery was triggered.
    pub round: u32,
    /// Type of recovery action taken.
    pub action: RecoveryActionKind,
    /// Duration of the recovery (compaction latency, etc.).
    pub duration: Duration,
}

/// Simplified recovery action kind for metrics (avoids cloning strings from RecoveryAction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryActionKind {
    Compact,
    ReactiveCompact,
    EscalateTokens,
    FallbackProvider,
    StopHookBlocked,
    Replan,
    ReplanWithFeedback,
}

impl From<&RecoveryAction> for RecoveryActionKind {
    fn from(action: &RecoveryAction) -> Self {
        match action {
            RecoveryAction::Compact => Self::Compact,
            RecoveryAction::ReactiveCompact => Self::ReactiveCompact,
            RecoveryAction::EscalateTokens => Self::EscalateTokens,
            RecoveryAction::FallbackProvider => Self::FallbackProvider,
            RecoveryAction::StopHookBlocked => Self::StopHookBlocked,
            RecoveryAction::Replan { .. } => Self::Replan,
            RecoveryAction::ReplanWithFeedback(_) => Self::ReplanWithFeedback,
        }
    }
}

// ── Compaction event ────────────────────────────────────────────────────────

/// A single compaction event with before/after token counts.
#[derive(Debug, Clone)]
pub struct CompactionEvent {
    pub round: u32,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub level: CompactionLevelMetric,
    pub latency: Duration,
}

/// Compaction level for metrics (matches TieredCompactor levels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionLevelMetric {
    Trim,
    Nominal,
    Degraded,
    Emergency,
    Legacy,
}

// ── LoopMetrics ─────────────────────────────────────────────────────────────

/// Structured metrics collected during the agent loop.
///
/// Captures the four reliability dimensions from Rabanser 2026:
/// - Consistency: recovery_events, provider_switches (how often does it self-correct?)
/// - Robustness: compaction_events, tool_errors (does it handle pressure?)
/// - Predictability: rounds_with_progress / rounds_total (is progress monotonic?)
/// - Safety: stagnation_episodes, reflection_injections (does it avoid loops?)
#[derive(Debug, Clone)]
pub struct LoopMetrics {
    /// Total rounds executed (including recovery rounds).
    pub rounds_total: u32,
    /// Rounds that included tool execution.
    pub rounds_with_tools: u32,
    /// Rounds where ProgressTracker reported new unique successful tools.
    pub rounds_with_progress: u32,
    /// Total compaction events.
    pub compaction_events: Vec<CompactionEvent>,
    /// Total recovery events.
    pub recovery_events: Vec<RecoveryEvent>,
    /// Number of times the provider was switched.
    pub provider_switches: u32,
    /// Total tool errors across all rounds.
    pub total_tool_errors: u32,
    /// Number of stagnation episodes (consecutive stall sequences).
    pub stagnation_episodes: u32,
    /// Number of reflection prompts injected.
    pub reflection_injections: u32,
    /// Cumulative time spent streaming model responses.
    pub time_streaming: Duration,
    /// Cumulative time spent executing tools.
    pub time_tools: Duration,
    /// Cumulative time spent in recovery (compaction, replan, etc.).
    pub time_recovery: Duration,
    /// Start time of the loop (for total elapsed calculation).
    loop_start: Instant,
}

impl LoopMetrics {
    pub fn new() -> Self {
        Self {
            rounds_total: 0,
            rounds_with_tools: 0,
            rounds_with_progress: 0,
            compaction_events: Vec::new(),
            recovery_events: Vec::new(),
            provider_switches: 0,
            total_tool_errors: 0,
            stagnation_episodes: 0,
            reflection_injections: 0,
            time_streaming: Duration::ZERO,
            time_tools: Duration::ZERO,
            time_recovery: Duration::ZERO,
            loop_start: Instant::now(),
        }
    }

    /// Record a tool round.
    pub fn record_tool_round(&mut self, had_progress: bool, tool_errors: u32) {
        self.rounds_total += 1;
        self.rounds_with_tools += 1;
        if had_progress {
            self.rounds_with_progress += 1;
        }
        self.total_tool_errors += tool_errors;
    }

    /// Record a non-tool round (model text response, recovery, etc.).
    pub fn record_non_tool_round(&mut self) {
        self.rounds_total += 1;
    }

    /// Record a recovery event.
    pub fn record_recovery(&mut self, round: u32, action: &RecoveryAction, duration: Duration) {
        self.recovery_events.push(RecoveryEvent {
            round,
            action: RecoveryActionKind::from(action),
            duration,
        });
        self.time_recovery += duration;
    }

    /// Record a compaction event.
    pub fn record_compaction(
        &mut self,
        round: u32,
        tokens_before: usize,
        tokens_after: usize,
        level: CompactionLevelMetric,
        latency: Duration,
    ) {
        self.compaction_events.push(CompactionEvent {
            round,
            tokens_before,
            tokens_after,
            level,
            latency,
        });
    }

    /// Record a provider switch.
    pub fn record_provider_switch(&mut self) {
        self.provider_switches += 1;
    }

    /// Record a stagnation episode.
    pub fn record_stagnation(&mut self) {
        self.stagnation_episodes += 1;
    }

    /// Record a reflection injection.
    pub fn record_reflection(&mut self) {
        self.reflection_injections += 1;
    }

    /// Add streaming time.
    pub fn add_streaming_time(&mut self, duration: Duration) {
        self.time_streaming += duration;
    }

    /// Add tool execution time.
    pub fn add_tool_time(&mut self, duration: Duration) {
        self.time_tools += duration;
    }

    /// Progress ratio: fraction of rounds that made forward progress.
    /// Higher is better. Values < 0.3 indicate severe stagnation.
    pub fn progress_ratio(&self) -> f64 {
        if self.rounds_with_tools == 0 {
            return 1.0;
        }
        self.rounds_with_progress as f64 / self.rounds_with_tools as f64
    }

    /// Recovery rate: fraction of rounds that required recovery.
    /// Lower is better. Values > 0.3 indicate instability.
    pub fn recovery_rate(&self) -> f64 {
        if self.rounds_total == 0 {
            return 0.0;
        }
        self.recovery_events.len() as f64 / self.rounds_total as f64
    }

    /// Total compaction reduction: tokens saved across all compaction events.
    pub fn total_tokens_saved(&self) -> usize {
        self.compaction_events
            .iter()
            .map(|e| e.tokens_before.saturating_sub(e.tokens_after))
            .sum()
    }

    /// Total elapsed time since loop start.
    pub fn total_elapsed(&self) -> Duration {
        self.loop_start.elapsed()
    }

    /// Emit structured tracing log with all metrics.
    pub fn emit_summary(&self) {
        tracing::info!(
            rounds_total = self.rounds_total,
            rounds_with_tools = self.rounds_with_tools,
            rounds_with_progress = self.rounds_with_progress,
            progress_ratio = format!("{:.2}", self.progress_ratio()),
            compaction_count = self.compaction_events.len(),
            recovery_count = self.recovery_events.len(),
            recovery_rate = format!("{:.2}", self.recovery_rate()),
            provider_switches = self.provider_switches,
            total_tool_errors = self.total_tool_errors,
            stagnation_episodes = self.stagnation_episodes,
            reflection_injections = self.reflection_injections,
            tokens_saved = self.total_tokens_saved(),
            streaming_ms = self.time_streaming.as_millis() as u64,
            tools_ms = self.time_tools.as_millis() as u64,
            recovery_ms = self.time_recovery.as_millis() as u64,
            total_ms = self.total_elapsed().as_millis() as u64,
            "loop_metrics_summary"
        );
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_ratio_calculation() {
        let mut m = LoopMetrics::new();
        m.record_tool_round(true, 0);
        m.record_tool_round(true, 0);
        m.record_tool_round(false, 1);
        assert!((m.progress_ratio() - 0.666).abs() < 0.01);
    }

    #[test]
    fn recovery_rate_calculation() {
        let mut m = LoopMetrics::new();
        m.record_tool_round(true, 0);
        m.record_non_tool_round();
        m.record_recovery(1, &RecoveryAction::Compact, Duration::from_millis(100));
        // 1 recovery in 2 rounds = 0.5
        assert!((m.recovery_rate() - 0.5).abs() < 0.01);
    }

    #[test]
    fn tokens_saved_accumulation() {
        let mut m = LoopMetrics::new();
        m.record_compaction(
            1,
            100_000,
            60_000,
            CompactionLevelMetric::Nominal,
            Duration::from_millis(500),
        );
        m.record_compaction(
            5,
            90_000,
            50_000,
            CompactionLevelMetric::Degraded,
            Duration::from_millis(200),
        );
        assert_eq!(m.total_tokens_saved(), 80_000);
    }

    #[test]
    fn empty_metrics_defaults() {
        let m = LoopMetrics::new();
        assert_eq!(m.rounds_total, 0);
        assert_eq!(m.progress_ratio(), 1.0); // No rounds = no stagnation
        assert_eq!(m.recovery_rate(), 0.0);
        assert_eq!(m.total_tokens_saved(), 0);
    }
}
