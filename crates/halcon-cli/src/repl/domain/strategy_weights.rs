//! Adaptive Strategy Weighting — Intra-session utility weight adjustment (P5.3).
//!
//! Dynamically adjusts weights for the convergence utility function (P3.6) based
//! on the current `ProblemClass` and per-round feedback. Shifts are bounded by
//! `AdaptationBoundsChecker` to prevent runaway self-modification.
//!
//! # Weight Channels
//!
//! | Weight | Default | Maps to |
//! |--------|---------|---------|
//! | drift_weight | 0.10 | `utility_w_drift` |
//! | evidence_weight | 0.25 | `utility_w_evidence` |
//! | utility_weight | 0.15 | `utility_w_coherence` |
//! | cycle_weight | 0.15 | `utility_w_cost` |
//! | sla_weight | 0.20 | `utility_w_pressure` |
//!
//! Remaining 0.15 goes to `utility_w_progress` (not in StrategyWeights).
//!
//! Pure business logic — no I/O.

use std::sync::Arc;

use halcon_core::types::PolicyConfig;

use super::convergence_utility::UtilityWeights;
use super::problem_classifier::ProblemClass;
use super::round_feedback::RoundFeedback;

// ── StrategyWeights ────────────────────────────────────────────────────────

/// Adjustable strategy weights for the convergence utility function.
#[derive(Debug, Clone)]
pub struct StrategyWeights {
    pub drift_weight: f64,
    pub evidence_weight: f64,
    pub utility_weight: f64,
    pub cycle_weight: f64,
    pub sla_weight: f64,
}

impl StrategyWeights {
    /// Initialize from PolicyConfig defaults.
    pub fn from_policy(policy: &PolicyConfig) -> Self {
        Self {
            drift_weight: policy.utility_w_drift,
            evidence_weight: policy.utility_w_evidence,
            utility_weight: policy.utility_w_coherence,
            cycle_weight: policy.utility_w_cost,
            sla_weight: policy.utility_w_pressure,
        }
    }

    /// Class-specific weight presets.
    pub fn for_class(class: ProblemClass) -> Self {
        match class {
            ProblemClass::DeterministicLinear => Self {
                drift_weight: 0.08,
                evidence_weight: 0.30,
                utility_weight: 0.20,
                cycle_weight: 0.12,
                sla_weight: 0.15,
            },
            ProblemClass::HighExploration => Self {
                drift_weight: 0.08,
                evidence_weight: 0.35,
                utility_weight: 0.15,
                cycle_weight: 0.15,
                sla_weight: 0.12,
            },
            ProblemClass::ToolConstrained => Self {
                drift_weight: 0.12,
                evidence_weight: 0.20,
                utility_weight: 0.15,
                cycle_weight: 0.20,
                sla_weight: 0.18,
            },
            ProblemClass::EvidenceSparse => Self {
                drift_weight: 0.08,
                evidence_weight: 0.37,
                utility_weight: 0.15,
                cycle_weight: 0.10,
                sla_weight: 0.15,
            },
            ProblemClass::Oscillatory => Self {
                drift_weight: 0.13,
                evidence_weight: 0.18,
                utility_weight: 0.19,
                cycle_weight: 0.25,
                sla_weight: 0.10,
            },
            ProblemClass::SLAConstrained => Self {
                drift_weight: 0.05,
                evidence_weight: 0.15,
                utility_weight: 0.10,
                cycle_weight: 0.15,
                sla_weight: 0.40,
            },
        }
    }

    /// Bridge to P3.6 UtilityWeights.
    pub fn to_utility_weights(&self, progress_weight: f64) -> UtilityWeights {
        UtilityWeights {
            w_evidence: self.evidence_weight,
            w_coherence: self.utility_weight,
            w_progress: progress_weight,
            w_pressure: self.sla_weight,
            w_cost: self.cycle_weight,
            w_drift: self.drift_weight,
        }
    }

    /// Total absolute shift from a baseline.
    pub fn total_shift_from(&self, baseline: &StrategyWeights) -> f64 {
        (self.drift_weight - baseline.drift_weight).abs()
            + (self.evidence_weight - baseline.evidence_weight).abs()
            + (self.utility_weight - baseline.utility_weight).abs()
            + (self.cycle_weight - baseline.cycle_weight).abs()
            + (self.sla_weight - baseline.sla_weight).abs()
    }

    /// Sum of all 5 weights.
    fn total(&self) -> f64 {
        self.drift_weight + self.evidence_weight + self.utility_weight
            + self.cycle_weight + self.sla_weight
    }

    /// Renormalize weights to sum to `target` (default ~0.85).
    fn renormalize(&mut self, target: f64) {
        let sum = self.total();
        if sum > 0.0 {
            let scale = target / sum;
            self.drift_weight *= scale;
            self.evidence_weight *= scale;
            self.utility_weight *= scale;
            self.cycle_weight *= scale;
            self.sla_weight *= scale;
        }
    }
}

// ── WeightAdjustment ───────────────────────────────────────────────────────

/// Record of a weight adjustment.
#[derive(Debug, Clone)]
pub struct WeightAdjustment {
    pub original: StrategyWeights,
    pub adjusted: StrategyWeights,
    pub total_shift: f64,
    pub rationale: &'static str,
    pub bounded: bool,
}

// ── StrategyWeightManager ──────────────────────────────────────────────────

/// Manages strategy weights with bounded adjustment.
pub struct StrategyWeightManager {
    current: StrategyWeights,
    baseline: StrategyWeights,
    policy: Arc<PolicyConfig>,
}

impl StrategyWeightManager {
    /// Create with policy defaults.
    pub fn new(policy: Arc<PolicyConfig>) -> Self {
        let weights = StrategyWeights::from_policy(&policy);
        Self {
            baseline: weights.clone(),
            current: weights,
            policy,
        }
    }

    /// Create with class-specific preset as baseline.
    pub fn from_class(class: ProblemClass, policy: Arc<PolicyConfig>) -> Self {
        let weights = StrategyWeights::for_class(class);
        Self {
            baseline: weights.clone(),
            current: weights,
            policy,
        }
    }

    /// Current weights.
    pub fn current(&self) -> &StrategyWeights {
        &self.current
    }

    /// Baseline weights (initial or class preset).
    pub fn baseline(&self) -> &StrategyWeights {
        &self.baseline
    }

    /// Set a new baseline (e.g., after problem classification).
    pub fn set_baseline(&mut self, weights: StrategyWeights) {
        self.baseline = weights.clone();
        self.current = weights;
    }

    /// Per-round micro-adjustment based on feedback.
    ///
    /// Returns `Some(WeightAdjustment)` if weights changed, `None` if no adjustment needed.
    pub fn adjust(
        &mut self,
        feedback: &RoundFeedback,
        _class: &ProblemClass,
    ) -> Option<WeightAdjustment> {
        let max_shift = self.policy.max_weight_shift_per_round;
        let original = self.current.clone();
        let mut changed = false;
        let mut rationale = "no adjustment";

        // Rule 1: High cycle severity → boost cycle_weight
        if feedback.cycle_severity > 0.5 {
            self.current.cycle_weight += 0.02_f64.min(max_shift);
            self.current.evidence_weight -= 0.02_f64.min(max_shift);
            changed = true;
            rationale = "cycle severity boost";
        }

        // Rule 2: Evidence coverage declining → boost evidence_weight
        if feedback.evidence_coverage < 0.30 {
            self.current.evidence_weight += 0.02_f64.min(max_shift);
            changed = true;
            rationale = "evidence coverage boost";
        }

        // Rule 3: SLA pressure → boost sla_weight
        // Use utility_score as proxy for SLA pressure (lower utility ≈ higher pressure late)
        if feedback.combined_score < 0.30 as f32 {
            self.current.drift_weight += 0.02_f64.min(max_shift);
            changed = true;
            rationale = "low score drift boost";
        }

        if !changed {
            return None;
        }

        // Renormalize to ~0.85
        self.current.renormalize(0.85);

        // Check total shift from baseline
        let total_shift = self.current.total_shift_from(&self.baseline);
        let bounded = total_shift > self.policy.max_sensitivity_shift;

        // If shift exceeds global bounds, revert to previous
        if bounded {
            self.current = original;
            return None;
        }

        Some(WeightAdjustment {
            original,
            adjusted: self.current.clone(),
            total_shift,
            rationale,
            bounded: false,
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::convergence_controller::ConvergenceAction;
    use super::super::round_feedback::LoopSignal;

    fn default_policy() -> Arc<PolicyConfig> {
        Arc::new(PolicyConfig::default())
    }

    fn make_feedback() -> RoundFeedback {
        RoundFeedback {
            round: 0,
            combined_score: 0.5,
            convergence_action: ConvergenceAction::Continue,
            loop_signal: LoopSignal::Continue,
            trajectory_trend: 0.5,
            oscillation: 0.0,
            replan_advised: false,
            synthesis_advised: false,
            tool_round: true,
            had_errors: false,
            mini_critic_replan: false,
            mini_critic_synthesis: false,
            evidence_coverage: 0.5,
            semantic_cycle_detected: false,
            cycle_severity: 0.0,
            utility_score: 0.5,
            mid_critic_action: None,
            complexity_upgraded: false,
            problem_class: None,
            forecast_rounds_remaining: None,
        }
    }

    #[test]
    fn phase5_weights_from_policy_matches_defaults() {
        let policy = PolicyConfig::default();
        let weights = StrategyWeights::from_policy(&policy);
        assert!((weights.drift_weight - 0.10).abs() < 1e-4);
        assert!((weights.evidence_weight - 0.25).abs() < 1e-4);
        assert!((weights.utility_weight - 0.15).abs() < 1e-4);
        assert!((weights.cycle_weight - 0.15).abs() < 1e-4);
        assert!((weights.sla_weight - 0.20).abs() < 1e-4);
    }

    #[test]
    fn phase5_weights_class_presets_sum_approximately_085() {
        let classes = [
            ProblemClass::DeterministicLinear,
            ProblemClass::HighExploration,
            ProblemClass::ToolConstrained,
            ProblemClass::EvidenceSparse,
            ProblemClass::Oscillatory,
            ProblemClass::SLAConstrained,
        ];
        for class in &classes {
            let weights = StrategyWeights::for_class(*class);
            let total = weights.total();
            assert!(
                (total - 0.85).abs() < 0.02,
                "{:?}: total={total}, expected ~0.85",
                class
            );
        }
    }

    #[test]
    fn phase5_weights_sla_constrained_emphasizes_sla() {
        let weights = StrategyWeights::for_class(ProblemClass::SLAConstrained);
        assert!(weights.sla_weight > weights.evidence_weight);
        assert!(weights.sla_weight > weights.drift_weight);
        assert!(weights.sla_weight > weights.utility_weight);
        assert!(weights.sla_weight > weights.cycle_weight);
    }

    #[test]
    fn phase5_weights_evidence_sparse_emphasizes_evidence() {
        let weights = StrategyWeights::for_class(ProblemClass::EvidenceSparse);
        assert!(weights.evidence_weight > weights.drift_weight);
        assert!(weights.evidence_weight > weights.sla_weight);
        assert!(weights.evidence_weight > weights.cycle_weight);
    }

    #[test]
    fn phase5_weights_oscillatory_emphasizes_cycle() {
        let weights = StrategyWeights::for_class(ProblemClass::Oscillatory);
        assert!(weights.cycle_weight > weights.evidence_weight);
        assert!(weights.cycle_weight > weights.sla_weight);
    }

    #[test]
    fn phase5_weights_to_utility_bridge() {
        let weights = StrategyWeights::from_policy(&PolicyConfig::default());
        let utility = weights.to_utility_weights(0.15);
        assert!((utility.w_evidence - 0.25).abs() < 1e-4);
        assert!((utility.w_coherence - 0.15).abs() < 1e-4);
        assert!((utility.w_progress - 0.15).abs() < 1e-4);
        assert!((utility.w_pressure - 0.20).abs() < 1e-4);
        assert!((utility.w_cost - 0.15).abs() < 1e-4);
        assert!((utility.w_drift - 0.10).abs() < 1e-4);
    }

    #[test]
    fn phase5_weights_total_shift_from_baseline() {
        let a = StrategyWeights::from_policy(&PolicyConfig::default());
        let b = StrategyWeights::for_class(ProblemClass::SLAConstrained);
        let shift = a.total_shift_from(&b);
        assert!(shift > 0.0, "different presets should have non-zero shift");
    }

    #[test]
    fn phase5_weights_manager_no_adjustment_on_normal_feedback() {
        let mut manager = StrategyWeightManager::new(default_policy());
        let feedback = make_feedback();
        let result = manager.adjust(&feedback, &ProblemClass::DeterministicLinear);
        assert!(result.is_none(), "normal feedback should not trigger adjustment");
    }

    #[test]
    fn phase5_weights_manager_adjusts_on_high_cycle_severity() {
        let mut manager = StrategyWeightManager::new(default_policy());
        let mut feedback = make_feedback();
        feedback.cycle_severity = 0.8;
        let result = manager.adjust(&feedback, &ProblemClass::DeterministicLinear);
        assert!(result.is_some(), "high cycle severity should trigger adjustment");
        let adj = result.unwrap();
        assert_eq!(adj.rationale, "cycle severity boost");
        assert!(!adj.bounded);
    }

    #[test]
    fn phase5_weights_manager_adjusts_on_low_evidence() {
        let mut manager = StrategyWeightManager::new(default_policy());
        let mut feedback = make_feedback();
        feedback.evidence_coverage = 0.10;
        let result = manager.adjust(&feedback, &ProblemClass::EvidenceSparse);
        assert!(result.is_some());
    }

    #[test]
    fn phase5_weights_renormalize_preserves_sum() {
        let mut weights = StrategyWeights {
            drift_weight: 0.20,
            evidence_weight: 0.30,
            utility_weight: 0.20,
            cycle_weight: 0.25,
            sla_weight: 0.30,
        };
        weights.renormalize(0.85);
        assert!((weights.total() - 0.85).abs() < 1e-4, "renormalized total should be 0.85");
    }

    #[test]
    fn phase5_weights_manager_from_class() {
        let manager = StrategyWeightManager::from_class(ProblemClass::SLAConstrained, default_policy());
        assert!((manager.current().sla_weight - 0.40).abs() < 1e-4);
        assert!((manager.baseline().sla_weight - 0.40).abs() < 1e-4);
    }
}
