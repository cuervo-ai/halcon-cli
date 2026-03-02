//! Convergence utility function for optimal synthesis timing.
//!
//! Computes a scalar utility U(R) that captures the diminishing returns of additional
//! agent loop rounds. Synthesis is triggered when utility drops below threshold or
//! marginal utility approaches zero.
//!
//! Formula:
//! ```text
//! U(R) = w_e×E + w_c×C + w_pg×P − w_p×T² − w_r×K − w_d×D
//! ```
//!
//! Pure business logic — no I/O, no state mutation.

use crate::repl::domain::round_feedback::RoundFeedback;

/// Inputs to the utility function, gathered from LoopState sub-structs.
#[derive(Debug, Clone)]
pub struct UtilityInputs {
    /// Evidence coverage [0,1].
    pub evidence_coverage: f64,
    /// Coherence score [0,1].
    pub coherence_score: f64,
    /// Plan progress fraction [0,1].
    pub plan_progress: f64,
    /// Time/token pressure [0,1] — max(sla_fraction, token_fraction).
    pub time_pressure: f64,
    /// Cumulative retry cost [0,1] — cumulative_tokens / budget.
    pub retry_cost: f64,
    /// Drift penalty [0,1].
    pub drift_penalty: f64,
    /// Evidence rate — bytes extracted this round / expected bytes per round.
    pub evidence_rate: f64,
}

/// Weights for utility function terms.
#[derive(Debug, Clone)]
pub struct UtilityWeights {
    pub w_evidence: f64,
    pub w_coherence: f64,
    pub w_progress: f64,
    pub w_pressure: f64,
    pub w_cost: f64,
    pub w_drift: f64,
}

impl UtilityWeights {
    /// Build weights from PolicyConfig fields.
    pub fn from_policy(policy: &halcon_core::types::PolicyConfig) -> Self {
        Self {
            w_evidence: policy.utility_w_evidence,
            w_coherence: policy.utility_w_coherence,
            w_progress: policy.utility_w_progress,
            w_pressure: policy.utility_w_pressure,
            w_cost: policy.utility_w_cost,
            w_drift: policy.utility_w_drift,
        }
    }
}

impl Default for UtilityWeights {
    fn default() -> Self {
        Self {
            w_evidence: 0.25,
            w_coherence: 0.15,
            w_progress: 0.15,
            w_pressure: 0.20,
            w_cost: 0.15,
            w_drift: 0.10,
        }
    }
}

/// Computed utility result with synthesis decision.
#[derive(Debug, Clone)]
pub struct UtilityResult {
    /// Current utility score [roughly -1.0, 1.0] — higher is better.
    pub utility: f64,
    /// Marginal utility of one more round: dU/dR ≈ (1−E)×(1−T)×evidence_rate.
    pub marginal_utility: f64,
    /// Whether synthesis should be triggered based on utility analysis.
    pub should_synthesize: bool,
    /// Human-readable reason if synthesis is recommended.
    pub reason: Option<&'static str>,
}

/// Compute convergence utility from inputs.
///
/// Returns a `UtilityResult` with the utility score, marginal utility, and
/// a synthesis recommendation.
pub fn compute_utility(
    inputs: &UtilityInputs,
    weights: &UtilityWeights,
    synthesis_threshold: f64,
    marginal_threshold: f64,
) -> UtilityResult {
    let e = inputs.evidence_coverage.clamp(0.0, 1.0);
    let c = inputs.coherence_score.clamp(0.0, 1.0);
    let p = inputs.plan_progress.clamp(0.0, 1.0);
    let t = inputs.time_pressure.clamp(0.0, 1.0);
    let k = inputs.retry_cost.clamp(0.0, 1.0);
    let d = inputs.drift_penalty.clamp(0.0, 1.0);

    // U(R) = w_e×E + w_c×C + w_pg×P − w_p×T² − w_r×K − w_d×D
    let utility = weights.w_evidence * e
        + weights.w_coherence * c
        + weights.w_progress * p
        - weights.w_pressure * t * t  // quadratic pressure
        - weights.w_cost * k
        - weights.w_drift * d;

    // Marginal utility: dU/dR ≈ (1−E) × (1−T) × evidence_rate
    let marginal_utility = (1.0 - e) * (1.0 - t) * inputs.evidence_rate.max(0.0);

    // Synthesis decision
    let (should_synthesize, reason) = if t > 0.95 {
        (true, Some("time pressure critical (>0.95)"))
    } else if utility < synthesis_threshold {
        (true, Some("utility below synthesis threshold"))
    } else if marginal_utility < marginal_threshold {
        (true, Some("marginal utility below threshold"))
    } else {
        (false, None)
    };

    UtilityResult {
        utility,
        marginal_utility,
        should_synthesize,
        reason,
    }
}

/// Convenience: compute utility using PolicyConfig defaults directly.
pub fn compute_utility_from_policy(
    inputs: &UtilityInputs,
    policy: &halcon_core::types::PolicyConfig,
) -> UtilityResult {
    let weights = UtilityWeights::from_policy(policy);
    compute_utility(
        inputs,
        &weights,
        policy.utility_synthesis_threshold,
        policy.utility_marginal_threshold,
    )
}

/// Extract utility score from the last RoundFeedback, defaulting to 0.5.
pub fn last_utility_score(feedback: &RoundFeedback) -> f64 {
    feedback.utility_score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_inputs() -> UtilityInputs {
        UtilityInputs {
            evidence_coverage: 0.50,
            coherence_score: 0.70,
            plan_progress: 0.40,
            time_pressure: 0.30,
            retry_cost: 0.10,
            drift_penalty: 0.10,
            evidence_rate: 0.20,
        }
    }

    #[test]
    fn phase3_utility_default_weights_balanced() {
        let w = UtilityWeights::default();
        let total = w.w_evidence + w.w_coherence + w.w_progress + w.w_pressure + w.w_cost + w.w_drift;
        assert!((total - 1.0).abs() < f64::EPSILON, "weights must sum to 1.0, got {total}");
    }

    #[test]
    fn phase3_utility_perfect_score() {
        let inputs = UtilityInputs {
            evidence_coverage: 1.0,
            coherence_score: 1.0,
            plan_progress: 1.0,
            time_pressure: 0.0,
            retry_cost: 0.0,
            drift_penalty: 0.0,
            evidence_rate: 0.0,
        };
        let r = compute_utility(&inputs, &UtilityWeights::default(), 0.35, 0.05);
        // U = 0.25 + 0.15 + 0.15 = 0.55
        assert!((r.utility - 0.55).abs() < 1e-10, "got {}", r.utility);
        // Marginal utility = (1-1.0)*(1-0)*0 = 0 < 0.05 → synthesis recommended
        // This is correct: all evidence collected, nothing left to do → synthesize
        assert!(r.should_synthesize);
        assert_eq!(r.reason, Some("marginal utility below threshold"));
    }

    #[test]
    fn phase3_utility_worst_case() {
        let inputs = UtilityInputs {
            evidence_coverage: 0.0,
            coherence_score: 0.0,
            plan_progress: 0.0,
            time_pressure: 1.0,
            retry_cost: 1.0,
            drift_penalty: 1.0,
            evidence_rate: 0.0,
        };
        let r = compute_utility(&inputs, &UtilityWeights::default(), 0.35, 0.05);
        // U = 0 - 0.20 - 0.15 - 0.10 = -0.45
        assert!((r.utility - (-0.45)).abs() < 1e-10, "got {}", r.utility);
        assert!(r.should_synthesize);
    }

    #[test]
    fn phase3_utility_quadratic_pressure_low() {
        let inputs = UtilityInputs {
            time_pressure: 0.3,
            ..default_inputs()
        };
        let r = compute_utility(&inputs, &UtilityWeights::default(), 0.35, 0.05);
        // T²=0.09, pressure penalty = 0.20 * 0.09 = 0.018
        let _expected_pressure = 0.20 * 0.09;
        // Positive terms: 0.25*0.5 + 0.15*0.7 + 0.15*0.4 = 0.125+0.105+0.06 = 0.29
        // Negative terms: 0.018 + 0.15*0.1 + 0.10*0.1 = 0.018+0.015+0.01 = 0.043
        let expected = 0.29 - 0.043;
        assert!((r.utility - expected).abs() < 1e-10, "got {}", r.utility);
        // utility=0.247 < threshold 0.35, so synthesis is correctly triggered
        assert!(r.should_synthesize, "utility {} < 0.35 triggers synthesis", r.utility);
    }

    #[test]
    fn phase3_utility_quadratic_pressure_high() {
        let inputs = UtilityInputs {
            time_pressure: 0.9,
            ..default_inputs()
        };
        let r = compute_utility(&inputs, &UtilityWeights::default(), 0.35, 0.05);
        // T²=0.81, pressure penalty = 0.20 * 0.81 = 0.162
        let positive = 0.25 * 0.5 + 0.15 * 0.7 + 0.15 * 0.4;
        let negative = 0.20 * 0.81 + 0.15 * 0.1 + 0.10 * 0.1;
        let expected = positive - negative;
        assert!((r.utility - expected).abs() < 1e-10, "got {}", r.utility);
    }

    #[test]
    fn phase3_utility_time_pressure_critical() {
        let inputs = UtilityInputs {
            time_pressure: 0.96,
            ..default_inputs()
        };
        let r = compute_utility(&inputs, &UtilityWeights::default(), 0.35, 0.05);
        assert!(r.should_synthesize);
        assert_eq!(r.reason, Some("time pressure critical (>0.95)"));
    }

    #[test]
    fn phase3_utility_marginal_zero_when_full_evidence() {
        let inputs = UtilityInputs {
            evidence_coverage: 1.0,
            evidence_rate: 0.50,
            ..default_inputs()
        };
        let r = compute_utility(&inputs, &UtilityWeights::default(), 0.35, 0.05);
        // (1-1.0) * (1-T) * rate = 0
        assert!(r.marginal_utility.abs() < 1e-10);
        assert!(r.should_synthesize, "marginal=0 < 0.05 threshold");
        assert_eq!(r.reason, Some("marginal utility below threshold"));
    }

    #[test]
    fn phase3_utility_marginal_positive_mid_session() {
        let inputs = UtilityInputs {
            evidence_coverage: 0.60,
            coherence_score: 0.90,
            plan_progress: 0.70,
            time_pressure: 0.10,
            retry_cost: 0.0,
            drift_penalty: 0.0,
            evidence_rate: 0.30,
        };
        let r = compute_utility(&inputs, &UtilityWeights::default(), 0.35, 0.05);
        // marginal = (1-0.6) * (1-0.1) * 0.3 = 0.4 * 0.9 * 0.3 = 0.108
        assert!((r.marginal_utility - 0.108).abs() < 1e-10);
        // U = 0.25*0.6 + 0.15*0.9 + 0.15*0.7 - 0.20*0.01 = 0.15+0.135+0.105-0.002 = 0.388
        assert!(r.utility > 0.35, "utility {} should be > 0.35", r.utility);
        assert!(!r.should_synthesize);
    }

    #[test]
    fn phase3_utility_below_synthesis_threshold() {
        let inputs = UtilityInputs {
            evidence_coverage: 0.10,
            coherence_score: 0.20,
            plan_progress: 0.10,
            time_pressure: 0.60,
            retry_cost: 0.50,
            drift_penalty: 0.40,
            evidence_rate: 0.10,
        };
        let r = compute_utility(&inputs, &UtilityWeights::default(), 0.35, 0.05);
        assert!(r.utility < 0.35, "utility {} should be < 0.35", r.utility);
        assert!(r.should_synthesize);
        assert_eq!(r.reason, Some("utility below synthesis threshold"));
    }

    #[test]
    fn phase3_utility_clamps_inputs() {
        let inputs = UtilityInputs {
            evidence_coverage: 1.5,
            coherence_score: -0.3,
            plan_progress: 2.0,
            time_pressure: 1.5,
            retry_cost: -1.0,
            drift_penalty: 3.0,
            evidence_rate: -0.5,
        };
        let r = compute_utility(&inputs, &UtilityWeights::default(), 0.35, 0.05);
        // Clamped: E=1, C=0, P=1, T=1, K=0, D=1
        // U = 0.25*1 + 0.15*0 + 0.15*1 - 0.20*1 - 0.15*0 - 0.10*1 = 0.25+0.15-0.20-0.10 = 0.10
        assert!((r.utility - 0.10).abs() < 1e-10, "got {}", r.utility);
    }

    #[test]
    fn phase3_utility_from_policy_defaults() {
        let policy = halcon_core::types::PolicyConfig::default();
        let inputs = default_inputs();
        let r = compute_utility_from_policy(&inputs, &policy);
        // Should match default weights computation
        let r2 = compute_utility(&inputs, &UtilityWeights::default(), 0.35, 0.05);
        assert!((r.utility - r2.utility).abs() < 1e-10);
    }

    #[test]
    fn phase3_utility_custom_weights() {
        let weights = UtilityWeights {
            w_evidence: 0.50,
            w_coherence: 0.10,
            w_progress: 0.10,
            w_pressure: 0.10,
            w_cost: 0.10,
            w_drift: 0.10,
        };
        let inputs = UtilityInputs {
            evidence_coverage: 0.80,
            coherence_score: 0.60,
            plan_progress: 0.50,
            time_pressure: 0.20,
            retry_cost: 0.10,
            drift_penalty: 0.10,
            evidence_rate: 0.15,
        };
        let r = compute_utility(&inputs, &weights, 0.35, 0.05);
        let expected = 0.50 * 0.80 + 0.10 * 0.60 + 0.10 * 0.50
            - 0.10 * 0.04 - 0.10 * 0.10 - 0.10 * 0.10;
        assert!((r.utility - expected).abs() < 1e-10, "got {}", r.utility);
    }

    #[test]
    fn phase3_utility_continue_when_healthy() {
        let inputs = UtilityInputs {
            evidence_coverage: 0.70,
            coherence_score: 0.90,
            plan_progress: 0.60,
            time_pressure: 0.10,
            retry_cost: 0.0,
            drift_penalty: 0.0,
            evidence_rate: 0.25,
        };
        let r = compute_utility(&inputs, &UtilityWeights::default(), 0.35, 0.05);
        // U = 0.25*0.7 + 0.15*0.9 + 0.15*0.6 - 0.20*0.01 = 0.175+0.135+0.09-0.002 = 0.398
        // marginal = (1-0.7)*(1-0.1)*0.25 = 0.3*0.9*0.25 = 0.0675 > 0.05
        assert!(r.utility > 0.35, "utility {} should be > 0.35", r.utility);
        assert!(r.marginal_utility > 0.05, "marginal {} should be > 0.05", r.marginal_utility);
        assert!(!r.should_synthesize, "healthy session should continue");
        assert!(r.reason.is_none());
    }

    #[test]
    fn phase3_utility_default_backward_compat() {
        // Default utility = 0.5 (from RoundFeedback default) > threshold 0.35
        // → delays synthesis, matching old behavior
        let policy = halcon_core::types::PolicyConfig::default();
        assert!(0.5 > policy.utility_synthesis_threshold);
    }
}
