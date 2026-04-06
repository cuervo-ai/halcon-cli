//! Mid-loop critic checkpoints — progress-aware round evaluation.
//!
//! Enhances (not replaces) existing `mini_critic_check()` with structured progress
//! tracking, evidence rate monitoring, and objective drift detection.
//!
//! Pure business logic — no I/O, no LLM calls.

use std::sync::Arc;

use halcon_core::types::PolicyConfig;

/// Action recommendation from mid-loop critic evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriticAction {
    /// Normal progress — no intervention needed.
    Continue,
    /// Change execution strategy (investigation ↔ execution).
    ChangeStrategy,
    /// Trigger replanning with fresh approach.
    Replan,
    /// Reduce scope — drop lower-priority plan steps.
    ReduceScope,
    /// Skip remaining work and synthesize immediately.
    ForceSynthesis,
}

/// Per-round snapshot for trend analysis.
#[derive(Debug, Clone)]
struct RoundSnapshot {
    round: usize,
    plan_progress: f64,
    evidence_coverage: f64,
    combined_score: f32,
    tool_round: bool,
}

/// Checkpoint result from periodic evaluation.
///
/// Designed for constructive, actionable output:
/// - `completed_fraction` and `evidence_rate` surface what has been achieved.
/// - `rationale` names the specific problem detected.
/// - `suggestion` provides a concrete next step so the developer sees *what to fix*, not
///   just a CRITICAL label.
/// - `rounds_remaining` allows callers to calibrate urgency.
#[derive(Debug, Clone)]
pub struct CriticCheckpoint {
    pub round: usize,
    pub rounds_remaining: usize,
    /// Progress fraction actually achieved (0.0–1.0).
    pub completed_fraction: f64,
    /// Progress fraction expected at this point in the budget (0.0–1.0).
    pub expected_fraction: f64,
    pub progress_deficit: f64,
    pub evidence_rate: f64,
    pub evidence_declining: bool,
    pub objective_drift: f64,
    pub action: CriticAction,
    /// Short description of what went wrong.
    pub rationale: &'static str,
    /// Constructive suggestion for the next step (shown in UI alongside completed work).
    pub suggestion: &'static str,
}

/// Mid-loop critic — evaluates progress at regular checkpoint intervals.
#[derive(Debug)]
pub struct MidLoopCritic {
    snapshots: Vec<RoundSnapshot>,
    checkpoint_interval: usize,
    expected_progress_per_round: f64,
    evidence_rate_ema: f64,
    policy: Arc<PolicyConfig>,
}

impl MidLoopCritic {
    /// Create a new critic with the given policy and max_rounds hint.
    pub fn new(policy: Arc<PolicyConfig>, max_rounds: usize) -> Self {
        let expected = if max_rounds > 0 {
            1.0 / max_rounds as f64
        } else {
            0.10
        };
        Self {
            snapshots: Vec::new(),
            checkpoint_interval: policy.mid_critic_interval,
            expected_progress_per_round: expected,
            evidence_rate_ema: 0.0,
            policy,
        }
    }

    /// Record a round snapshot for trend analysis.
    pub fn record_snapshot(
        &mut self,
        round: usize,
        plan_progress: f64,
        evidence_coverage: f64,
        combined_score: f32,
        tool_round: bool,
    ) {
        // Update evidence rate EMA (α=0.3)
        let alpha = 0.3;
        self.evidence_rate_ema = alpha * evidence_coverage + (1.0 - alpha) * self.evidence_rate_ema;

        self.snapshots.push(RoundSnapshot {
            round,
            plan_progress,
            evidence_coverage,
            combined_score,
            tool_round,
        });
    }

    /// Check if this round is a checkpoint round.
    pub fn is_checkpoint(&self, round: usize) -> bool {
        round > 0 && self.checkpoint_interval > 0 && round % self.checkpoint_interval == 0
    }

    /// Evaluate progress and return a checkpoint with action recommendation.
    ///
    /// Call only on checkpoint rounds (`is_checkpoint() == true`).
    pub fn evaluate(
        &self,
        round: usize,
        max_rounds: usize,
        plan_progress: f64,
        _evidence_coverage: f64,
        drift_score: f64,
    ) -> CriticCheckpoint {
        let budget_fraction = if max_rounds > 0 {
            round as f64 / max_rounds as f64
        } else {
            0.0
        };

        // Expected progress = budget_fraction (linear expectation)
        let progress_deficit = budget_fraction - plan_progress;

        // Evidence rate: compare last 2 snapshots
        let evidence_declining = self.detect_evidence_decline();

        // Decision cascade (priority order).
        // Each arm returns (action, problem_rationale, constructive_suggestion).
        let (action, rationale, suggestion) = if budget_fraction >= 0.90 && plan_progress < 0.60 {
            (
                CriticAction::ForceSynthesis,
                "budget >90% consumed with <60% progress",
                "Summarize completed work and mark remaining steps as deferred",
            )
        } else if progress_deficit > self.policy.progress_deficit_threshold {
            (
                CriticAction::Replan,
                "progress deficit exceeds threshold",
                "Re-plan with reduced scope: drop optional steps and focus on core deliverables",
            )
        } else if drift_score > self.policy.objective_drift_threshold {
            (
                CriticAction::ChangeStrategy,
                "objective drift detected — execution diverging from original goal",
                "Switch strategy: re-read the original goal and adjust the current approach",
            )
        } else if evidence_declining
            && progress_deficit > self.policy.progress_deficit_threshold * 0.5
        {
            (
                CriticAction::ReduceScope,
                "evidence collection declining with moderate progress deficit",
                "Drop low-priority steps and consolidate evidence from completed rounds",
            )
        } else {
            (
                CriticAction::Continue,
                "progress within expected range",
                "Continue current approach",
            )
        };

        let rounds_remaining = max_rounds.saturating_sub(round);

        CriticCheckpoint {
            round,
            rounds_remaining,
            completed_fraction: plan_progress,
            expected_fraction: budget_fraction,
            progress_deficit,
            evidence_rate: self.evidence_rate_ema,
            evidence_declining,
            objective_drift: drift_score,
            action,
            rationale,
            suggestion,
        }
    }

    /// Detect whether evidence collection rate is declining.
    fn detect_evidence_decline(&self) -> bool {
        if self.snapshots.len() < 2 {
            return false;
        }
        let n = self.snapshots.len();
        let recent = &self.snapshots[n - 1];
        let prior = &self.snapshots[n - 2];

        if prior.evidence_coverage < 1e-10 {
            return false;
        }

        let rate_ratio = (recent.evidence_coverage - prior.evidence_coverage)
            / prior.evidence_coverage.max(0.01);

        rate_ratio < -self.policy.evidence_rate_decline_ratio
    }

    /// Get the last recorded evidence rate EMA.
    pub fn evidence_rate(&self) -> f64 {
        self.evidence_rate_ema
    }
}

impl CriticCheckpoint {
    /// Format a user-facing summary of the checkpoint.
    ///
    /// Designed to be constructive: leads with what was achieved, then the issue, then
    /// the suggestion.  Avoids flooding the output with CRITICAL labels — one concise line
    /// per concern.
    ///
    /// Example output:
    ///   [critic] Round 6/20 — 40% complete (expected 60%)  ⚠ progress deficit
    ///   Suggestion: Re-plan with reduced scope: drop optional steps…
    pub fn format_summary(&self) -> String {
        let max_round = self.round + self.rounds_remaining;
        let pct_done = (self.completed_fraction * 100.0) as u32;
        let pct_exp = (self.expected_fraction * 100.0) as u32;

        let status = match self.action {
            CriticAction::Continue => "✓ on track",
            CriticAction::ChangeStrategy => "⚠ strategy shift needed",
            CriticAction::ReduceScope => "⚠ scope reduction advised",
            CriticAction::Replan => "⚠ replan triggered",
            CriticAction::ForceSynthesis => "⚠ synthesizing early",
        };

        let mut out = format!(
            "[critic] Round {}/{} — {}% complete (expected {}%)  {status}\n  Issue: {}\n  Next:  {}",
            self.round, max_round,
            pct_done, pct_exp,
            self.rationale,
            self.suggestion,
        );

        if self.evidence_declining {
            out.push_str("\n  Note:  evidence collection rate is declining");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_policy() -> Arc<PolicyConfig> {
        Arc::new(PolicyConfig::default())
    }

    #[test]
    fn phase3_mid_critic_checkpoint_interval() {
        let critic = MidLoopCritic::new(test_policy(), 10);
        assert!(!critic.is_checkpoint(0));
        assert!(!critic.is_checkpoint(1));
        assert!(!critic.is_checkpoint(2));
        assert!(critic.is_checkpoint(3));
        assert!(!critic.is_checkpoint(4));
        assert!(!critic.is_checkpoint(5));
        assert!(critic.is_checkpoint(6));
        assert!(critic.is_checkpoint(9));
    }

    #[test]
    fn phase3_mid_critic_continue_normal_progress() {
        let critic = MidLoopCritic::new(test_policy(), 10);
        let cp = critic.evaluate(3, 10, 0.30, 0.40, 0.10);
        assert_eq!(cp.action, CriticAction::Continue);
    }

    #[test]
    fn phase3_mid_critic_force_synthesis_budget_exhausted() {
        let critic = MidLoopCritic::new(test_policy(), 10);
        let cp = critic.evaluate(9, 10, 0.40, 0.30, 0.10);
        assert_eq!(cp.action, CriticAction::ForceSynthesis);
        assert_eq!(cp.rationale, "budget >90% consumed with <60% progress");
    }

    #[test]
    fn phase3_mid_critic_replan_on_deficit() {
        let critic = MidLoopCritic::new(test_policy(), 10);
        // Round 5/10 = 50% budget, but only 10% progress → deficit = 0.40 > 0.25
        let cp = critic.evaluate(5, 10, 0.10, 0.40, 0.10);
        assert_eq!(cp.action, CriticAction::Replan);
    }

    #[test]
    fn phase3_mid_critic_change_strategy_on_drift() {
        let critic = MidLoopCritic::new(test_policy(), 10);
        // Round 3/10, progress on track (0.30), but high drift
        let cp = critic.evaluate(3, 10, 0.30, 0.40, 0.50);
        assert_eq!(cp.action, CriticAction::ChangeStrategy);
    }

    #[test]
    fn phase3_mid_critic_reduce_scope_declining_evidence() {
        let mut critic = MidLoopCritic::new(test_policy(), 10);
        // Record declining evidence
        critic.record_snapshot(1, 0.10, 0.50, 0.5, true);
        critic.record_snapshot(2, 0.15, 0.20, 0.4, true); // sharp decline
                                                          // Moderate deficit (0.30 - 0.15 = 0.15 > 0.125)
        let cp = critic.evaluate(3, 10, 0.15, 0.20, 0.10);
        assert_eq!(cp.action, CriticAction::ReduceScope);
    }

    #[test]
    fn phase3_mid_critic_evidence_rate_ema() {
        let mut critic = MidLoopCritic::new(test_policy(), 10);
        critic.record_snapshot(0, 0.0, 0.0, 0.5, true);
        assert!(critic.evidence_rate().abs() < 1e-10);

        critic.record_snapshot(1, 0.1, 0.50, 0.6, true);
        // EMA = 0.3*0.50 + 0.7*0 = 0.15
        assert!((critic.evidence_rate() - 0.15).abs() < 1e-10);

        critic.record_snapshot(2, 0.2, 0.60, 0.7, true);
        // EMA = 0.3*0.60 + 0.7*0.15 = 0.18 + 0.105 = 0.285
        assert!((critic.evidence_rate() - 0.285).abs() < 1e-10);
    }

    #[test]
    fn phase3_mid_critic_no_decline_when_insufficient_data() {
        let critic = MidLoopCritic::new(test_policy(), 10);
        let cp = critic.evaluate(3, 10, 0.30, 0.40, 0.10);
        assert!(!cp.evidence_declining);
    }

    #[test]
    fn phase3_mid_critic_expected_progress_scales_with_rounds() {
        let critic_10 = MidLoopCritic::new(test_policy(), 10);
        assert!((critic_10.expected_progress_per_round - 0.10).abs() < 1e-10);

        let critic_20 = MidLoopCritic::new(test_policy(), 20);
        assert!((critic_20.expected_progress_per_round - 0.05).abs() < 1e-10);
    }

    #[test]
    fn phase3_mid_critic_force_synthesis_overrides_replan() {
        let critic = MidLoopCritic::new(test_policy(), 10);
        // Budget 95%, progress 20% → both deficit AND budget trigger
        // ForceSynthesis has higher priority
        let cp = critic.evaluate(9, 10, 0.20, 0.10, 0.50);
        assert_eq!(cp.action, CriticAction::ForceSynthesis);
    }

    #[test]
    fn phase3_mid_critic_deficit_calculation() {
        let critic = MidLoopCritic::new(test_policy(), 10);
        let cp = critic.evaluate(5, 10, 0.30, 0.40, 0.10);
        // budget_fraction = 0.5, plan_progress = 0.3 → deficit = 0.2
        assert!((cp.progress_deficit - 0.20).abs() < 1e-10);
    }

    #[test]
    fn phase3_mid_critic_zero_max_rounds() {
        let critic = MidLoopCritic::new(test_policy(), 0);
        assert!((critic.expected_progress_per_round - 0.10).abs() < 1e-10);
        let cp = critic.evaluate(3, 0, 0.30, 0.40, 0.10);
        // budget_fraction = 0 when max_rounds=0 → deficit = -0.30
        assert_eq!(cp.action, CriticAction::Continue);
    }

    // ── New fields & format_summary tests ────────────────────────────────────

    #[test]
    fn checkpoint_populates_completed_and_expected_fraction() {
        let critic = MidLoopCritic::new(test_policy(), 20);
        // Round 10/20 = 50% budget, 35% done
        let cp = critic.evaluate(10, 20, 0.35, 0.40, 0.05);
        assert!(
            (cp.completed_fraction - 0.35).abs() < 1e-6,
            "completed_fraction should be plan_progress"
        );
        assert!(
            (cp.expected_fraction - 0.50).abs() < 1e-6,
            "expected_fraction should be budget_fraction"
        );
        assert_eq!(cp.rounds_remaining, 10);
    }

    #[test]
    fn checkpoint_provides_constructive_suggestion_on_replan() {
        let critic = MidLoopCritic::new(test_policy(), 10);
        let cp = critic.evaluate(5, 10, 0.10, 0.40, 0.10);
        assert_eq!(cp.action, CriticAction::Replan);
        // Suggestion must be non-empty and not a CRITICAL label
        assert!(!cp.suggestion.is_empty());
        assert!(!cp.suggestion.contains("CRITICAL"));
    }

    #[test]
    fn format_summary_continue_is_terse() {
        let critic = MidLoopCritic::new(test_policy(), 20);
        let cp = critic.evaluate(4, 20, 0.20, 0.40, 0.05);
        let summary = cp.format_summary();
        assert!(
            summary.contains("on track"),
            "continue path should say on track"
        );
        assert!(summary.contains("20%"), "should show completed %");
    }

    #[test]
    fn format_summary_replan_shows_issue_and_next() {
        let critic = MidLoopCritic::new(test_policy(), 10);
        let cp = critic.evaluate(5, 10, 0.10, 0.40, 0.05);
        let summary = cp.format_summary();
        assert!(summary.contains("Issue:"), "must label the problem");
        assert!(summary.contains("Next:"), "must provide next step");
        assert!(!summary.contains("CRITICAL"), "must not use CRITICAL label");
    }

    #[test]
    fn format_summary_mentions_declining_evidence_when_present() {
        let mut critic = MidLoopCritic::new(test_policy(), 10);
        critic.record_snapshot(1, 0.10, 0.50, 0.5, true);
        critic.record_snapshot(2, 0.15, 0.20, 0.4, true);
        let cp = critic.evaluate(3, 10, 0.15, 0.20, 0.10);
        let summary = cp.format_summary();
        assert!(
            summary.contains("declining"),
            "should mention evidence decline"
        );
    }
}
