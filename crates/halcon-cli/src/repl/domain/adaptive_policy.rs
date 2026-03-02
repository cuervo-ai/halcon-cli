//! Within-session adaptive policy — Sprint 3 of SOTA 2026 L6 architecture.
//!
//! `AdaptivePolicy` is the key enabler of **L6 (Intelligent Autonomous)** maturity.
//! It observes per-round performance feedback and adjusts its own decision thresholds
//! mid-session — a form of metacognitive self-regulation.
//!
//! # What makes this L6
//! Parameters set at session start (from `StrategyContext.replan_sensitivity`) are
//! static in the current system. `AdaptivePolicy` makes them dynamic: the system
//! escalates `replan_sensitivity` when it detects a declining trajectory, so it
//! demands higher-quality rounds earlier when it's struggling. This is the system
//! literally changing its own decision thresholds based on what it observes.
//!
//! # Integration
//! ```text
//! Per round:
//!   let adj = adaptive_policy.observe(&round_feedback);
//!   if adj.replan_sensitivity_delta > 0.0 {
//!       round_scorer.set_replan_sensitivity(adaptive_policy.current_sensitivity());
//!   }
//! On replan:
//!   adaptive_policy.reset_after_replan();
//! ```

use super::round_feedback::RoundFeedback;

// ── AdaptationRationale ───────────────────────────────────────────────────────

/// Why an adjustment was made this round.
///
/// Used for tracing/observability — never drives control flow.
#[derive(Debug, Clone, PartialEq)]
pub enum AdaptationRationale {
    /// No adjustment — trajectory is healthy.
    #[allow(dead_code)]
    NoChange,
    /// Escalating because trajectory has been below threshold for N consecutive rounds.
    DecliningTrajectory { consecutive_rounds: usize },
    /// Boosting synthesis urgency because oscillation is erratic.
    OscillationDetected { penalty: f32 },
    /// Issuing model downgrade advisory because trend has been below threshold for N rounds.
    ModelUnderperforming { trend: f32 },
}

impl Default for AdaptationRationale {
    fn default() -> Self {
        Self::NoChange
    }
}

// ── PolicyAdjustment ─────────────────────────────────────────────────────────

/// Adjustment to apply to the agent loop's decision thresholds this round.
///
/// Returned by `AdaptivePolicy::observe()`. The caller (infrastructure `agent/mod.rs`)
/// applies the delta to the appropriate component. The policy itself does not reach
/// into infrastructure — it only returns adjustments as data.
#[derive(Debug, Clone, Default)]
pub struct PolicyAdjustment {
    /// How much to add to `RoundScorer`'s `replan_sensitivity` this round.
    ///
    /// Positive = sensitivity escalation (triggers replanning sooner).
    /// Never negative — sensitivity never decreases mid-session (use `reset_after_replan` for that).
    pub replan_sensitivity_delta: f32,
    /// Urgency boost for synthesis (0.0 = no change, 1.0 = maximum urgency).
    ///
    /// Currently advisory only — future sprints may wire this into synthesis threshold.
    pub synthesis_urgency_boost: f32,
    /// Advisory: selected model tier may be underperforming for this session.
    ///
    /// Infrastructure decides whether to act (e.g., switch to a smaller/faster model).
    /// The domain layer only signals the pattern — it never makes the switch itself.
    pub model_downgrade_advisory: bool,
    /// Why this adjustment was made (for tracing / observability).
    pub rationale: AdaptationRationale,
}

// ── AdaptivePolicy ────────────────────────────────────────────────────────────

/// Within-session parameter self-adjustment engine.
///
/// Observes `RoundFeedback` per round and returns `PolicyAdjustment` to apply.
/// Maintains internal state (consecutive counters, current sensitivity) across rounds.
///
/// # Thresholds
/// All thresholds are named constants, not magic numbers, and are exhaustively tested.
pub struct AdaptivePolicy {
    /// Number of consecutive rounds with `trajectory_trend < LOW_TRAJECTORY_THRESHOLD`.
    consecutive_low_rounds: usize,
    /// Number of consecutive rounds with `oscillation > HIGH_OSCILLATION_THRESHOLD`.
    consecutive_oscillation_rounds: usize,
    /// Sensitivity set at session start (from `StrategyContext.replan_sensitivity`).
    base_sensitivity: f32,
    /// Current effective sensitivity = base + accumulated escalation.
    current_sensitivity: f32,
}

impl AdaptivePolicy {
    /// Trajectory trend below this is considered "struggling".
    const LOW_TRAJECTORY_THRESHOLD: f32 = 0.30;
    /// Oscillation penalty above this is considered "erratic".
    const HIGH_OSCILLATION_THRESHOLD: f32 = 0.15;
    /// Sensitivity escalation step per low round (after 2 consecutive).
    const SENSITIVITY_STEP: f32 = 0.10;
    /// Maximum total escalation above base (cap to prevent over-sensitization).
    const MAX_SENSITIVITY_ESCALATION: f32 = 0.40;
    /// Trend below this triggers model downgrade advisory.
    const MODEL_DOWNGRADE_TREND: f32 = 0.20;
    /// Consecutive low rounds required before model downgrade advisory is issued.
    const MODEL_DOWNGRADE_ROUNDS: usize = 3;
    /// Consecutive low rounds required before sensitivity escalation begins.
    const MIN_LOW_ROUNDS_FOR_ESCALATION: usize = 2;

    /// Create a new policy with the given base sensitivity.
    ///
    /// `base_sensitivity` should come from `StrategyContext.replan_sensitivity`.
    /// Use `0.0` if no strategy context is available (permissive default).
    pub fn new(base_sensitivity: f32) -> Self {
        let base = base_sensitivity.clamp(0.0, 1.0);
        Self {
            consecutive_low_rounds: 0,
            consecutive_oscillation_rounds: 0,
            base_sensitivity: base,
            current_sensitivity: base,
        }
    }

    /// Observe a round's feedback and return the adjustment to apply.
    ///
    /// Updates internal state (consecutive counters, current_sensitivity).
    /// The returned `PolicyAdjustment` should be applied by the caller
    /// immediately after this call.
    pub fn observe(&mut self, feedback: &RoundFeedback) -> PolicyAdjustment {
        let mut adj = PolicyAdjustment::default();

        // ── Trajectory decline detection ───────────────────────────────────────
        if feedback.trajectory_trend < Self::LOW_TRAJECTORY_THRESHOLD {
            self.consecutive_low_rounds += 1;
        } else {
            // Recovery: any healthy round resets the counter.
            self.consecutive_low_rounds = 0;
        }

        // ── Oscillation detection ──────────────────────────────────────────────
        if feedback.oscillation > Self::HIGH_OSCILLATION_THRESHOLD {
            self.consecutive_oscillation_rounds += 1;
            // Boost synthesis urgency whenever oscillation is detected.
            adj.synthesis_urgency_boost = (feedback.oscillation / Self::HIGH_OSCILLATION_THRESHOLD)
                .min(1.0)
                .max(0.0);
            adj.rationale = AdaptationRationale::OscillationDetected {
                penalty: feedback.oscillation,
            };
        } else {
            self.consecutive_oscillation_rounds = 0;
        }

        // ── Sensitivity escalation (requires ≥ MIN_LOW_ROUNDS_FOR_ESCALATION) ─
        if self.consecutive_low_rounds >= Self::MIN_LOW_ROUNDS_FOR_ESCALATION {
            let max_sensitivity = (self.base_sensitivity + Self::MAX_SENSITIVITY_ESCALATION).min(1.0);
            let new_sensitivity =
                (self.current_sensitivity + Self::SENSITIVITY_STEP).min(max_sensitivity);
            let delta = new_sensitivity - self.current_sensitivity;
            if delta > 0.0 {
                adj.replan_sensitivity_delta = delta;
                self.current_sensitivity = new_sensitivity;
                // Only override rationale if oscillation didn't already set it.
                if adj.rationale == AdaptationRationale::default() {
                    adj.rationale = AdaptationRationale::DecliningTrajectory {
                        consecutive_rounds: self.consecutive_low_rounds,
                    };
                }
            }
        }

        // ── Model downgrade advisory ───────────────────────────────────────────
        if self.consecutive_low_rounds >= Self::MODEL_DOWNGRADE_ROUNDS
            && feedback.trajectory_trend < Self::MODEL_DOWNGRADE_TREND
        {
            adj.model_downgrade_advisory = true;
            // Override rationale to reflect the more severe condition.
            adj.rationale = AdaptationRationale::ModelUnderperforming {
                trend: feedback.trajectory_trend,
            };
        }

        adj
    }

    /// Reset escalation state after a successful replan.
    ///
    /// A new plan starts fresh — prevents over-sensitization from carrying over
    /// state accumulated under a stale plan. The base sensitivity is restored
    /// as the new current sensitivity.
    pub fn reset_after_replan(&mut self) {
        self.consecutive_low_rounds = 0;
        self.consecutive_oscillation_rounds = 0;
        self.current_sensitivity = self.base_sensitivity;
    }

    /// Current effective sensitivity (base + accumulated escalation).
    ///
    /// This is the value to pass to `RoundScorer::set_replan_sensitivity()`.
    pub fn current_sensitivity(&self) -> f32 {
        self.current_sensitivity
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::domain::convergence_controller::ConvergenceAction;
    use crate::repl::domain::round_feedback::{LoopSignal, RoundFeedback};

    fn make_feedback(trajectory_trend: f32, oscillation: f32) -> RoundFeedback {
        RoundFeedback {
            round: 0,
            combined_score: 0.5,
            convergence_action: ConvergenceAction::Continue,
            loop_signal: LoopSignal::Continue,
            trajectory_trend,
            oscillation,
            replan_advised: false,
            synthesis_advised: false,
            tool_round: true,
            had_errors: false,
            mini_critic_replan: false,
            mini_critic_synthesis: false,
            evidence_coverage: 1.0,
            semantic_cycle_detected: false,
            cycle_severity: 0.0,
            utility_score: 0.5,
            mid_critic_action: None,
            complexity_upgraded: false,
            problem_class: None,
            forecast_rounds_remaining: None,
        }
    }

    fn healthy_feedback() -> RoundFeedback {
        make_feedback(0.60, 0.0)
    }

    fn low_feedback() -> RoundFeedback {
        make_feedback(0.20, 0.0)
    }

    fn oscillating_feedback() -> RoundFeedback {
        make_feedback(0.50, 0.25)
    }

    fn very_low_feedback() -> RoundFeedback {
        make_feedback(0.15, 0.0)
    }

    #[test]
    fn new_with_zero_base_starts_at_zero() {
        let policy = AdaptivePolicy::new(0.0);
        assert!((policy.current_sensitivity() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn new_with_nonzero_base_starts_at_base() {
        let policy = AdaptivePolicy::new(0.5);
        assert!((policy.current_sensitivity() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn no_adjustment_when_trajectory_healthy() {
        let mut policy = AdaptivePolicy::new(0.0);
        let adj = policy.observe(&healthy_feedback());
        assert!((adj.replan_sensitivity_delta - 0.0).abs() < f32::EPSILON);
        assert!(!adj.model_downgrade_advisory);
        assert_eq!(adj.rationale, AdaptationRationale::NoChange);
    }

    #[test]
    fn single_low_round_no_adjustment() {
        // Requires ≥ 2 consecutive low rounds before escalation starts.
        let mut policy = AdaptivePolicy::new(0.0);
        let adj = policy.observe(&low_feedback());
        assert!((adj.replan_sensitivity_delta - 0.0).abs() < f32::EPSILON,
            "single low round should not trigger escalation");
    }

    #[test]
    fn two_consecutive_low_rounds_escalates_sensitivity() {
        let mut policy = AdaptivePolicy::new(0.0);
        policy.observe(&low_feedback()); // round 1
        let adj = policy.observe(&low_feedback()); // round 2
        assert!(adj.replan_sensitivity_delta > 0.0,
            "two consecutive low rounds should trigger escalation");
        assert!((adj.replan_sensitivity_delta - AdaptivePolicy::SENSITIVITY_STEP).abs() < f32::EPSILON);
    }

    #[test]
    fn three_consecutive_low_rounds_escalates_more() {
        let mut policy = AdaptivePolicy::new(0.0);
        policy.observe(&low_feedback()); // round 1
        policy.observe(&low_feedback()); // round 2 — first escalation
        let adj = policy.observe(&low_feedback()); // round 3 — second escalation
        assert!(adj.replan_sensitivity_delta > 0.0);
        let expected = AdaptivePolicy::SENSITIVITY_STEP;
        assert!((adj.replan_sensitivity_delta - expected).abs() < f32::EPSILON,
            "each additional low round escalates by SENSITIVITY_STEP");
    }

    #[test]
    fn escalation_capped_at_max() {
        let mut policy = AdaptivePolicy::new(0.0);
        // Drive many consecutive low rounds to hit the cap.
        for _ in 0..20 {
            policy.observe(&low_feedback());
        }
        let max = AdaptivePolicy::MAX_SENSITIVITY_ESCALATION;
        assert!(policy.current_sensitivity() <= max + f32::EPSILON,
            "sensitivity must never exceed base + MAX_SENSITIVITY_ESCALATION");
    }

    #[test]
    fn recovery_round_resets_low_counter() {
        let mut policy = AdaptivePolicy::new(0.0);
        policy.observe(&low_feedback()); // low
        policy.observe(&low_feedback()); // low → escalation
        let sensitivity_after_escalation = policy.current_sensitivity();
        policy.observe(&healthy_feedback()); // recovery — resets counter
        policy.observe(&low_feedback()); // first low again — no escalation
        let adj = policy.observe(&low_feedback()); // second low — escalation resumes
        assert!(adj.replan_sensitivity_delta > 0.0);
        // Sensitivity should have increased beyond the earlier escalation.
        assert!(policy.current_sensitivity() > sensitivity_after_escalation);
    }

    #[test]
    fn oscillation_detected_triggers_synthesis_urgency() {
        let mut policy = AdaptivePolicy::new(0.0);
        let adj = policy.observe(&oscillating_feedback());
        assert!(adj.synthesis_urgency_boost > 0.0,
            "oscillation above threshold should trigger synthesis urgency boost");
        assert!(matches!(adj.rationale, AdaptationRationale::OscillationDetected { .. }));
    }

    #[test]
    fn model_downgrade_advisory_after_three_low_rounds() {
        let mut policy = AdaptivePolicy::new(0.0);
        policy.observe(&very_low_feedback()); // round 1
        policy.observe(&very_low_feedback()); // round 2
        let adj = policy.observe(&very_low_feedback()); // round 3 — MODEL_DOWNGRADE_ROUNDS
        assert!(adj.model_downgrade_advisory,
            "3 consecutive very low rounds should trigger model downgrade advisory");
        assert!(matches!(adj.rationale, AdaptationRationale::ModelUnderperforming { .. }));
    }

    #[test]
    fn model_downgrade_advisory_not_issued_when_trend_above_threshold() {
        let mut policy = AdaptivePolicy::new(0.0);
        // Trend just above MODEL_DOWNGRADE_TREND (0.20) — advisory should NOT fire.
        let fb = make_feedback(0.25, 0.0); // above MODEL_DOWNGRADE_TREND but below LOW_TRAJECTORY_THRESHOLD
        // Need 3 rounds for the consecutive counter, but trend is too high for advisory
        // Note: 0.25 < LOW_TRAJECTORY_THRESHOLD (0.30) so counter DOES increment
        // but 0.25 > MODEL_DOWNGRADE_TREND (0.20) so advisory DOES NOT fire
        policy.observe(&fb);
        policy.observe(&fb);
        let adj = policy.observe(&fb);
        assert!(!adj.model_downgrade_advisory,
            "trend above MODEL_DOWNGRADE_TREND should not trigger advisory");
    }

    #[test]
    fn reset_after_replan_clears_state() {
        let mut policy = AdaptivePolicy::new(0.2);
        // Drive escalation.
        policy.observe(&low_feedback());
        policy.observe(&low_feedback());
        assert!(policy.current_sensitivity() > 0.2, "should have escalated");
        // Reset.
        policy.reset_after_replan();
        assert!((policy.current_sensitivity() - 0.2).abs() < f32::EPSILON,
            "reset should restore base sensitivity");
    }

    #[test]
    fn current_sensitivity_returns_base_initially() {
        let policy = AdaptivePolicy::new(0.3);
        assert!((policy.current_sensitivity() - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn current_sensitivity_reflects_escalation() {
        let mut policy = AdaptivePolicy::new(0.0);
        policy.observe(&low_feedback());
        policy.observe(&low_feedback()); // triggers first escalation
        assert!(policy.current_sensitivity() > 0.0,
            "current_sensitivity should reflect accumulated escalation");
    }

    #[test]
    fn no_downgrade_advisory_when_rounds_insufficient() {
        let mut policy = AdaptivePolicy::new(0.0);
        // Only 2 rounds (need MODEL_DOWNGRADE_ROUNDS = 3)
        policy.observe(&very_low_feedback());
        let adj = policy.observe(&very_low_feedback());
        assert!(!adj.model_downgrade_advisory,
            "fewer than MODEL_DOWNGRADE_ROUNDS rounds should not issue advisory");
    }

    #[test]
    fn rationale_correctly_set_for_each_adjustment_type() {
        // No change
        let mut policy = AdaptivePolicy::new(0.0);
        let adj = policy.observe(&healthy_feedback());
        assert_eq!(adj.rationale, AdaptationRationale::NoChange);

        // Oscillation
        let mut policy = AdaptivePolicy::new(0.0);
        let adj = policy.observe(&oscillating_feedback());
        assert!(matches!(adj.rationale, AdaptationRationale::OscillationDetected { .. }));

        // Declining trajectory (after 2+ consecutive low rounds)
        let mut policy = AdaptivePolicy::new(0.0);
        policy.observe(&low_feedback());
        let adj = policy.observe(&low_feedback());
        assert!(matches!(adj.rationale, AdaptationRationale::DecliningTrajectory { .. }));

        // Model underperforming (after 3 very low rounds)
        let mut policy = AdaptivePolicy::new(0.0);
        policy.observe(&very_low_feedback());
        policy.observe(&very_low_feedback());
        let adj = policy.observe(&very_low_feedback());
        assert!(matches!(adj.rationale, AdaptationRationale::ModelUnderperforming { .. }));
    }
}
