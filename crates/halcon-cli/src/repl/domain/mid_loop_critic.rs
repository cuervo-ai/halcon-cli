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
#[derive(Debug, Clone)]
pub struct CriticCheckpoint {
    pub round: usize,
    pub progress_deficit: f64,
    pub evidence_rate: f64,
    pub evidence_declining: bool,
    pub objective_drift: f64,
    pub action: CriticAction,
    pub rationale: &'static str,
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
        evidence_coverage: f64,
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

        // Decision cascade (priority order)
        let (action, rationale) = if budget_fraction >= 0.90 && plan_progress < 0.60 {
            (CriticAction::ForceSynthesis, "budget >90% with <60% progress")
        } else if progress_deficit > self.policy.progress_deficit_threshold {
            (CriticAction::Replan, "progress deficit exceeds threshold")
        } else if drift_score > self.policy.objective_drift_threshold {
            (CriticAction::ChangeStrategy, "objective drift exceeds threshold")
        } else if evidence_declining && progress_deficit > self.policy.progress_deficit_threshold * 0.5 {
            (CriticAction::ReduceScope, "evidence declining with moderate deficit")
        } else {
            (CriticAction::Continue, "progress within expected range")
        };

        CriticCheckpoint {
            round,
            progress_deficit,
            evidence_rate: self.evidence_rate_ema,
            evidence_declining,
            objective_drift: drift_score,
            action,
            rationale,
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
        assert_eq!(cp.rationale, "budget >90% with <60% progress");
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
        critic.record_snapshot(2, 0.15, 0.20, 0.4, true);  // sharp decline
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
}
