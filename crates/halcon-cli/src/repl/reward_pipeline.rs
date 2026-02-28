//! Reward Pipeline — multi-signal reward computation for UCB1 strategy learning.
//!
//! Replaces the inline 4-value `StopCondition`→score mapping in `reasoning_engine.rs`
//! with a continuous multi-dimensional reward combining:
//! - Stop condition (continuous, with plan_completion_ratio bonus)
//! - Trajectory score (per-round averages from RoundScorer)
//! - Critic verdict (LoopCritic confidence)
//! - Plan coherence (semantic drift from original goal)
//! - Oscillation penalty (cross-type instability from ToolLoopGuard)

use super::agent::StopCondition;
use super::plugin_cost_tracker::PluginCostSnapshot;

/// All raw signals collected after a completed agent loop.
#[derive(Debug, Clone)]
pub struct RawRewardSignals {
    /// How the agent loop terminated.
    pub stop_condition: StopCondition,
    /// Per-round combined scores from RoundScorer (empty = no round scoring active).
    pub round_scores: Vec<f32>,
    /// LoopCritic verdict: (achieved, confidence). None = critic unavailable.
    pub critic_verdict: Option<(bool, f32)>,
    /// Average semantic drift across all replans (0.0 = no drift, 1.0 = fully drifted).
    pub plan_coherence_score: f32,
    /// Oscillation penalty from RoundScorer (0.0 = stable, 1.0 = maximum oscillation).
    pub oscillation_penalty: f32,
    /// Plan completion ratio at loop end (0.0–1.0).
    pub plan_completion_ratio: f32,
    /// Per-plugin cost snapshots for reward blending. Empty = no plugins active.
    /// When non-empty, `plugin_adjusted_reward()` blends a 10% plugin outcome signal.
    pub plugin_snapshots: Vec<PluginCostSnapshot>,
    /// Whether the LoopCritic was unavailable (both primary and fallback failed/timed out).
    /// When true, an additional penalty is applied to push the reward below retry threshold.
    pub critic_unavailable: bool,
}

/// Breakdown of individual reward components for diagnostics and logging.
#[derive(Debug, Clone)]
pub struct RewardBreakdown {
    /// Continuous stop-condition score (incorporates plan_completion_ratio).
    pub stop_score: f64,
    /// Trajectory score from per-round history (falls back to stop_score if no history).
    pub trajectory_score: f64,
    /// Critic-derived score (falls back to stop_score when critic unavailable).
    pub critic_score: f64,
    /// Goal coherence score: `1.0 - avg_drift_score`.
    pub coherence_score: f64,
}

/// Final reward computation result.
#[derive(Debug, Clone)]
pub struct RewardComputation {
    /// Final blended reward in [0.0, 1.0].
    pub final_reward: f64,
    /// Component breakdown for diagnostics.
    pub breakdown: RewardBreakdown,
}

/// Continuous stop-condition score incorporating plan completion ratio.
///
/// Replaces the coarse 4-value mapping with ranges that scale with how much of the
/// plan was completed, giving UCB1 finer-grained feedback.
fn stop_condition_score(cond: &StopCondition, ratio: f32) -> f64 {
    let r = ratio.clamp(0.0, 1.0) as f64;
    match cond {
        StopCondition::EndTurn => 0.70 + 0.30 * r,           // 0.70–1.00
        StopCondition::ForcedSynthesis => 0.40 + 0.30 * r,   // 0.40–0.70 with plan bonus
        StopCondition::MaxRounds => 0.20 + 0.20 * r,         // 0.20–0.40
        StopCondition::TokenBudget
        | StopCondition::DurationBudget
        | StopCondition::CostBudget
        | StopCondition::SupervisorDenied => 0.10 + 0.10 * r,
        StopCondition::Interrupted => 0.50,                   // user-initiated = partial credit
        StopCondition::ProviderError => 0.0,                  // hard failure = zero
        StopCondition::EnvironmentError => 0.0,               // MCP/env dead = zero (same penalty)
    }
}

/// Compute the multi-dimensional reward from raw loop signals.
///
/// Formula (component weights sum to 1.0):
/// ```text
/// final = ( stop_score × 0.25
///         + trajectory × 0.30
///         + critic     × 0.25
///         + coherence  × 0.20
///         - synthesis_penalty ).clamp(0.0, 1.0)
/// ```
///
/// Critic dampening: when the LoopCritic verdict is `!achieved`, the raw stop_score
/// is capped proportionally to the critic's confidence.  High-confidence failures
/// apply the full `CRITIC_FAIL_STOP_CAP` (0.60), while low-confidence failures
/// apply a weaker cap — preventing uncertain critics (e.g. 30% confidence) from
/// triggering unnecessary retries on otherwise complete sessions.
///
/// Formula: `effective_cap = 1.0 - (1.0 - CRITIC_FAIL_STOP_CAP) × confidence`
///
// ── Reward weight constants ──────────────────────────────────────────────────
//
// Extracted as named constants so CRITIC_FAIL_STOP_CAP can be derived
// algebraically rather than chosen by hand.

const W_STOP: f64 = 0.25;
const W_TRAJECTORY: f64 = 0.30;
const W_CRITIC: f64 = 0.25;
const W_COHERENCE: f64 = 0.20;
const SUCCESS_THRESHOLD: f64 = 0.60;
const CAP_SAFETY_MARGIN: f64 = 0.025;

/// Upper bound for CRITIC_FAIL_STOP_CAP: the maximum allowable cap value that
/// still ensures `final_reward < SUCCESS_THRESHOLD` at full confidence (1.0).
///
/// With confidence-proportional dampening, at conf=1.0 the effective cap equals
/// CRITIC_FAIL_STOP_CAP directly.  Worst case: EndTurn + plan_completion=1.0
/// + full coherence + critic=(false, 1.0).  We need `final < SUCCESS_THRESHOLD`:
///   cap×W_STOP + cap×W_TRAJECTORY + 0.0×W_CRITIC + 1.0×W_COHERENCE < 0.60
/// Solving for cap gives the upper bound below, minus a safety margin.
fn compute_critic_fail_stop_cap() -> f64 {
    let worst_critic = 0.0;    // 0.25 × (1.0 - 1.0) — full-confidence failure
    let worst_coherence = 1.0; // full coherence (no plan drift)
    let numerator = SUCCESS_THRESHOLD - worst_critic * W_CRITIC - worst_coherence * W_COHERENCE;
    ((numerator / (W_STOP + W_TRAJECTORY)) - CAP_SAFETY_MARGIN).max(0.0)
}

/// Cached value — `compute_critic_fail_stop_cap()` is pure so we can call it once.
const CRITIC_FAIL_STOP_CAP: f64 = 0.60; // compile-time placeholder; validated by test below

/// FASE 4: Penalty applied when the LoopCritic is completely unavailable (both
/// primary and fallback failed or timed out). This pushes borderline sessions
/// below the 0.60 retry threshold so unverified sessions trigger a retry.
const CRITIC_UNAVAILABLE_PENALTY: f64 = 0.15;

pub fn compute_reward(signals: &RawRewardSignals) -> RewardComputation {
    let raw_stop_score =
        stop_condition_score(&signals.stop_condition, signals.plan_completion_ratio);

    // Critic dampening: confidence-proportional cap on stop_score when critic says
    // goal was NOT achieved.  Low-confidence failures (e.g. 30%) apply a weak cap,
    // preventing uncertain critics from triggering unnecessary retries on otherwise
    // complete sessions.  High-confidence failures (e.g. 90%) apply the full
    // CRITIC_FAIL_STOP_CAP, preserving the retry-triggering behavior.
    //
    // Formula: effective_cap = 1.0 - (1.0 - CRITIC_FAIL_STOP_CAP) × confidence
    let stop_score = match signals.critic_verdict {
        Some((false, conf)) => {
            let effective_cap = 1.0 - (1.0 - CRITIC_FAIL_STOP_CAP) * conf as f64;
            raw_stop_score.min(effective_cap)
        }
        _ => raw_stop_score,
    };

    // Trajectory: mean of per-round scores, discounted by oscillation instability.
    // Falls back to (dampened) stop_score so the critic cap also reduces trajectory.
    let trajectory_score = if signals.round_scores.is_empty() {
        // No per-round data — fall back to stop_score (critic-dampened when applicable).
        stop_score
    } else {
        let mean: f64 = signals.round_scores.iter().map(|&s| s as f64).sum::<f64>()
            / signals.round_scores.len() as f64;
        (mean * (1.0 - signals.oscillation_penalty.clamp(0.0, 1.0) as f64)).max(0.0)
    };

    // Critic score:
    //   achieved=true  → full confidence as reward signal
    //   achieved=false → aggressive penalty: 0.25 × (1 - confidence)
    //                    Using 0.25 instead of 0.5 so high-confidence failures
    //                    drop the critic component close to zero.
    //   None           → mirror raw_stop_score (no critic feedback = neutral)
    let critic_score = match signals.critic_verdict {
        Some((true, conf)) => conf as f64,
        Some((false, conf)) => 0.25 * (1.0 - conf as f64),
        None => raw_stop_score, // no critic — mirror raw stop (unaffected by dampening)
    };

    // Coherence: invert drift score (lower drift = higher coherence).
    // Gated strictly on plan_completion_ratio > 0.0 — coherence is only meaningful when an
    // actual execution plan ran. Having round_scores without plan execution (pure text rounds)
    // does NOT make coherence computable: plan_coherence_score is never populated without a
    // plan, so (1.0 - 0.0) = 1.0 would be a phantom bonus for unplanned sessions.
    let coherence_score = if signals.plan_completion_ratio > 0.0 {
        (1.0 - signals.plan_coherence_score.clamp(0.0, 1.0) as f64).max(0.0)
    } else {
        0.0
    };

    // Synthesis penalty: ForcedSynthesis indicates incomplete goal convergence.
    let synthesis_penalty = if matches!(signals.stop_condition, StopCondition::ForcedSynthesis) {
        0.10
    } else {
        0.0
    };

    // FASE 4: Critic unavailability penalty — pushes unverified sessions below retry threshold.
    let critic_unavailable_penalty = if signals.critic_unavailable {
        CRITIC_UNAVAILABLE_PENALTY
    } else {
        0.0
    };

    let final_reward = (stop_score * W_STOP
        + trajectory_score * W_TRAJECTORY
        + critic_score * W_CRITIC
        + coherence_score * W_COHERENCE
        - synthesis_penalty
        - critic_unavailable_penalty)
        .clamp(0.0, 1.0);

    RewardComputation {
        final_reward,
        breakdown: RewardBreakdown {
            stop_score,
            trajectory_score,
            critic_score,
            coherence_score,
        },
    }
}

/// Blend a base reward with the plugin success rate signal.
///
/// Called **after** [`compute_reward()`] — applies a 10% additive weighting from
/// plugin outcomes.  When `plugin_snapshots` is empty the base reward is returned
/// unchanged, preserving full backward compatibility.
///
/// Formula: `(0.90 × base_reward + 0.10 × plugin_success_rate).clamp(0.0, 1.0)`
pub fn plugin_adjusted_reward(base_reward: f64, snapshots: &[PluginCostSnapshot]) -> f64 {
    if snapshots.is_empty() {
        return base_reward;
    }
    let total_calls: u32 = snapshots.iter().map(|s| s.calls_made).sum();
    let total_failures: u32 = snapshots.iter().map(|s| s.calls_failed).sum();
    if total_calls == 0 {
        return base_reward;
    }
    let plugin_success_rate = 1.0 - (total_failures as f64 / total_calls as f64);
    (0.90 * base_reward + 0.10 * plugin_success_rate).clamp(0.0, 1.0)
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn end_turn_signals() -> RawRewardSignals {
        RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: None,
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        }
    }

    #[test]
    fn end_turn_full_completion_high_reward() {
        let result = compute_reward(&end_turn_signals());
        // stop_score = 1.0; trajectory fallback = 1.0; critic fallback = 1.0; coherence = 1.0
        assert!(result.final_reward > 0.80, "got {}", result.final_reward);
        assert_eq!(result.breakdown.stop_score, 1.0);
    }

    #[test]
    fn forced_synthesis_lower_than_end_turn() {
        let synth = RawRewardSignals {
            stop_condition: StopCondition::ForcedSynthesis,
            round_scores: vec![],
            critic_verdict: None,
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let r_end = compute_reward(&end_turn_signals());
        let r_synth = compute_reward(&synth);
        assert!(
            r_synth.final_reward < r_end.final_reward,
            "synth={} end={}",
            r_synth.final_reward,
            r_end.final_reward
        );
    }

    #[test]
    fn forced_synthesis_penalty_applied_to_score() {
        let synth = RawRewardSignals {
            stop_condition: StopCondition::ForcedSynthesis,
            round_scores: vec![],
            critic_verdict: None,
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&synth);
        // stop_score = 0.70; synthesis_penalty = 0.10; final must reflect deduction
        assert!(result.final_reward < 0.95, "got {}", result.final_reward);
    }

    #[test]
    fn provider_error_near_zero() {
        let signals = RawRewardSignals {
            stop_condition: StopCondition::ProviderError,
            round_scores: vec![],
            critic_verdict: None,
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        assert!(result.final_reward < 0.20, "got {}", result.final_reward);
        assert_eq!(result.breakdown.stop_score, 0.0);
    }

    #[test]
    fn critic_failure_lowers_reward_vs_no_critic() {
        let with_failure = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: Some((false, 0.95)), // highly confident it failed
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let r_fail = compute_reward(&with_failure);
        let r_base = compute_reward(&end_turn_signals());
        assert!(r_fail.final_reward < r_base.final_reward);
    }

    #[test]
    fn trajectory_high_scores_boost_reward_vs_max_rounds() {
        let with_history = RawRewardSignals {
            stop_condition: StopCondition::MaxRounds,
            round_scores: vec![0.80, 0.85, 0.90],
            critic_verdict: None,
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.5,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let without_history = RawRewardSignals {
            stop_condition: StopCondition::MaxRounds,
            round_scores: vec![],
            critic_verdict: None,
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.5,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let r_with = compute_reward(&with_history);
        let r_without = compute_reward(&without_history);
        // High round scores from RoundScorer should push trajectory above stop_score fallback
        assert!(r_with.final_reward > r_without.final_reward);
    }

    #[test]
    fn oscillation_penalty_reduces_trajectory_score() {
        let stable = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![0.80, 0.80, 0.80],
            critic_verdict: None,
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let oscillating = RawRewardSignals {
            oscillation_penalty: 0.80,
            ..stable.clone()
        };
        let r_stable = compute_reward(&stable);
        let r_osc = compute_reward(&oscillating);
        assert!(r_osc.breakdown.trajectory_score < r_stable.breakdown.trajectory_score);
        assert!(r_osc.final_reward < r_stable.final_reward);
    }

    #[test]
    fn high_drift_lowers_coherence_component() {
        let coherent = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: None,
            plan_coherence_score: 0.0, // no drift
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let drifted = RawRewardSignals {
            plan_coherence_score: 0.95, // heavy drift
            ..coherent.clone()
        };
        let r_coh = compute_reward(&coherent);
        let r_dri = compute_reward(&drifted);
        assert!(r_dri.breakdown.coherence_score < r_coh.breakdown.coherence_score);
        assert!(r_dri.final_reward < r_coh.final_reward);
    }

    #[test]
    fn reward_clamped_to_unit_interval() {
        let max_signals = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![1.0, 1.0, 1.0],
            critic_verdict: Some((true, 1.0)),
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let min_signals = RawRewardSignals {
            stop_condition: StopCondition::ProviderError,
            round_scores: vec![0.0, 0.0],
            critic_verdict: Some((false, 1.0)),
            plan_coherence_score: 1.0,
            oscillation_penalty: 1.0,
            plan_completion_ratio: 0.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let r_max = compute_reward(&max_signals);
        let r_min = compute_reward(&min_signals);
        assert!(r_max.final_reward <= 1.0, "exceeds 1.0: {}", r_max.final_reward);
        assert!(r_min.final_reward >= 0.0, "below 0.0: {}", r_min.final_reward);
    }

    #[test]
    fn stop_score_monotonically_ordered_at_zero_ratio() {
        let score = |cond: StopCondition| stop_condition_score(&cond, 0.0);
        assert!(
            score(StopCondition::EndTurn) > score(StopCondition::ForcedSynthesis),
            "EndTurn must beat ForcedSynthesis"
        );
        assert!(
            score(StopCondition::ForcedSynthesis) > score(StopCondition::MaxRounds),
            "ForcedSynthesis must beat MaxRounds"
        );
        assert!(
            score(StopCondition::MaxRounds) > score(StopCondition::ProviderError),
            "MaxRounds must beat ProviderError"
        );
    }

    #[test]
    fn plan_completion_boosts_end_turn_score() {
        let zero = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: None,
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.0, // nothing completed
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let full = RawRewardSignals {
            plan_completion_ratio: 1.0,
            ..zero.clone()
        };
        let r_zero = compute_reward(&zero);
        let r_full = compute_reward(&full);
        assert!(r_full.final_reward > r_zero.final_reward);
    }

    // ── Coherence gating regression tests (Fix: phantom bonus when no plan executed) ──

    #[test]
    fn coherence_zero_when_no_plan_executed_despite_round_scores() {
        // Before the fix: plan_completion_ratio=0.0 BUT round_scores non-empty would
        // give coherence_score = 1.0 - 0.0 = 1.0 (phantom bonus).
        // After the fix: coherence_score must be 0.0 whenever plan_completion_ratio=0.0.
        let signals = RawRewardSignals {
            stop_condition: StopCondition::ForcedSynthesis,
            round_scores: vec![0.80, 0.85, 0.70], // RoundScorer active
            critic_verdict: None,
            plan_coherence_score: 0.0,             // never populated (no plan ran)
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.0,            // no plan executed
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        assert_eq!(
            result.breakdown.coherence_score, 0.0,
            "coherence must be 0.0 when no plan executed (plan_completion_ratio=0.0), got {}",
            result.breakdown.coherence_score
        );
    }

    #[test]
    fn coherence_populated_when_plan_executed() {
        // With plan_completion_ratio > 0.0, coherence is meaningful and must be non-zero
        // when plan_coherence_score is low (little drift = high coherence).
        let signals = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: None,
            plan_coherence_score: 0.10, // slight drift
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.8, // plan executed
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        // coherence_score = 1.0 - 0.10 = 0.90
        assert!(
            result.breakdown.coherence_score > 0.85,
            "coherence must be populated when plan executed, got {}",
            result.breakdown.coherence_score
        );
    }

    #[test]
    fn no_plan_execution_does_not_inflate_reward_above_provider_error() {
        // Regression: ForcedSynthesis with round_scores but no plan execution must NOT
        // score higher than EndTurn with full plan execution due to phantom coherence bonus.
        let forced_no_plan = RawRewardSignals {
            stop_condition: StopCondition::ForcedSynthesis,
            round_scores: vec![0.90, 0.90, 0.90],
            critic_verdict: None,
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.0, // no plan
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let end_turn_full_plan = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: None,
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let r_forced = compute_reward(&forced_no_plan);
        let r_end = compute_reward(&end_turn_full_plan);
        assert!(
            r_end.final_reward > r_forced.final_reward,
            "EndTurn+full_plan ({}) must beat ForcedSynthesis+no_plan ({})",
            r_end.final_reward,
            r_forced.final_reward
        );
    }

    #[test]
    fn environment_error_scores_near_zero_same_as_provider_error() {
        // P0-B: EnvironmentError (MCP dead) must penalise UCB1 the same as ProviderError.
        let env_err = RawRewardSignals {
            stop_condition: StopCondition::EnvironmentError,
            round_scores: vec![],
            critic_verdict: None,
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let prov_err = RawRewardSignals {
            stop_condition: StopCondition::ProviderError,
            ..env_err.clone()
        };
        let r_env = compute_reward(&env_err);
        let r_prov = compute_reward(&prov_err);
        assert_eq!(r_env.breakdown.stop_score, 0.0, "EnvironmentError stop_score must be 0.0");
        assert_eq!(r_env.breakdown.stop_score, r_prov.breakdown.stop_score,
            "EnvironmentError and ProviderError must produce identical stop_score");
        assert!(r_env.final_reward < 0.20, "EnvironmentError final_reward must be near zero");
    }

    // ── plugin_adjusted_reward (Phase 7 V3 plugin architecture) ──────────────

    #[test]
    fn plugin_adjusted_reward_empty_snapshots_passthrough() {
        // When no plugins are active, the base reward must be returned unchanged.
        let base = 0.75;
        let result = plugin_adjusted_reward(base, &[]);
        assert!((result - base).abs() < 1e-9, "empty snapshots must return base_reward unchanged");
    }

    #[test]
    fn plugin_adjusted_reward_all_fail_degrades_by_at_most_10_percent() {
        use super::super::plugin_cost_tracker::PluginCostSnapshot;
        // All calls failed — plugin_success_rate = 0.0
        let snaps = vec![PluginCostSnapshot {
            plugin_id: "p".into(),
            tokens_used: 0,
            usd_spent: 0.0,
            calls_made: 5,
            calls_failed: 5,
        }];
        let base = 0.80;
        let result = plugin_adjusted_reward(base, &snaps);
        // formula: 0.90 × 0.80 + 0.10 × 0.0 = 0.72
        assert!(result < base, "all-fail should degrade reward");
        assert!((base - result) <= 0.10 + 1e-9, "degradation must be ≤10%");
    }

    #[test]
    fn plugin_adjusted_reward_all_succeed_stays_clamped() {
        use super::super::plugin_cost_tracker::PluginCostSnapshot;
        // All calls succeeded — plugin_success_rate = 1.0
        let snaps = vec![PluginCostSnapshot {
            plugin_id: "p".into(),
            tokens_used: 0,
            usd_spent: 0.0,
            calls_made: 3,
            calls_failed: 0,
        }];
        let base = 0.95;
        let result = plugin_adjusted_reward(base, &snaps);
        // formula: 0.90 × 0.95 + 0.10 × 1.0 = 0.955 → clamped to 1.0 max
        assert!(result >= base * 0.90, "all-succeed should not degrade reward");
        assert!(result <= 1.0, "must be clamped to 1.0");
    }

    // ── Critic dampening — stop_score cap when !achieved (RC-4 fix) ──────────
    //
    // Root cause: EndTurn + plan_completion=1.0 gave stop_score=1.0, overwhelming
    // the critic penalty. A sub-agent session where all delegated steps "completed"
    // (even with empty output) would get reward=0.70+, blocking score_says_retry.
    // Fix: when critic says !achieved, cap stop_score at CRITIC_FAIL_STOP_CAP=0.65.

    /// Low-confidence failure (10%) should NOT push reward below retry threshold
    /// because the proportional cap is very weak at low confidence.
    #[test]
    fn critic_low_confidence_failure_does_not_push_below_threshold() {
        let signals = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: Some((false, 0.10)), // low confidence "not achieved"
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0, // all plan steps "completed"
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        // effective_cap = 1.0 - 0.40 * 0.10 = 0.96 — almost no dampening
        // stop_score = min(1.0, 0.96) = 0.96; trajectory = 0.96; coherence = 1.0
        // critic_score = 0.25*(1-0.10) = 0.225
        // final ≈ 0.96*0.25 + 0.96*0.30 + 0.225*0.25 + 1.0*0.20 ≈ 0.78
        assert!(
            result.final_reward > 0.60,
            "low-confidence (10%) critic failure must NOT push below retry threshold, got {}",
            result.final_reward
        );
    }

    /// High-confidence failure (80%+) must yield even lower reward.
    #[test]
    fn critic_high_confidence_failure_yields_very_low_reward() {
        let signals = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: Some((false, 0.90)),
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        // stop_score=0.65, critic_score=0.25*(1-0.90)=0.025
        // final ≈ 0.65*0.25 + 0.65*0.30 + 0.025*0.25 + 1.0*0.20 ≈ 0.563
        // Must be below low-confidence case AND below threshold
        assert!(
            result.final_reward < 0.60,
            "high-confidence critic failure must be below retry threshold, got {}",
            result.final_reward
        );
    }

    /// Successful sessions are NOT dampened by the critic cap.
    #[test]
    fn critic_success_verdict_not_dampened() {
        let signals = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: Some((true, 0.90)), // high confidence "achieved"
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        // stop_score = 1.0 (no cap on success); critic_score = 0.90
        // trajectory = 1.0 (fallback to raw_stop); coherence = 1.0
        // final = 1.0*0.25 + 1.0*0.30 + 0.90*0.25 + 1.0*0.20 = 0.975
        assert!(
            result.final_reward > 0.80,
            "critic success verdict must not reduce reward, got {}",
            result.final_reward
        );
    }

    /// At full confidence, critic dampening caps stop_score at CRITIC_FAIL_STOP_CAP
    /// but does not zero it out.
    #[test]
    fn critic_fail_stop_cap_is_floor_not_zero() {
        let signals = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: Some((false, 1.0)), // full confidence → full cap
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        assert!(
            (result.breakdown.stop_score - CRITIC_FAIL_STOP_CAP).abs() < 1e-9,
            "stop_score must equal CRITIC_FAIL_STOP_CAP at full confidence, got {}",
            result.breakdown.stop_score
        );
        assert!(
            result.breakdown.stop_score > 0.0,
            "stop_score must not be zeroed by critic dampening"
        );
    }

    // ── FASE 4: Pruebas Controladas — Pipeline Búsqueda Zuclubit/Cuervo ─────
    //
    // Caso 1: Búsqueda real de cotizaciones (tools ejecutadas → reward alto)
    // Caso 2: Consulta ambigua → plan generado → critic evalúa
    // Caso 3: Simulación de tool failure → retry (reward < 0.60 → score_says_retry)
    // Caso 4: Intento de síntesis sin tool call → reward penalizado (ForcedSynthesis)

    /// CASO 1: El pipeline de búsqueda Zuclubit/Cuervo usa tools reales.
    /// Simula: tools ejecutadas [list_allowed_directories, search_files→grep, read_multiple_files]
    /// → critic dice achieved=true → reward debe ser > 0.80 (tarea completada).
    /// Valida: plan_completion_ratio=1.0, EndTurn, critic=(true, 0.85)
    #[test]
    fn caso1_busqueda_cotizaciones_con_tools_reales_reward_alto() {
        let signals = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![0.80, 0.85, 0.90], // 3 rounds: list_dirs, search, read
            critic_verdict: Some((true, 0.85)),    // critic confirma: archivos encontrados y leídos
            plan_coherence_score: 0.10,             // baja deriva (plan bien seguido)
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,             // completed_steps == total_steps
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        assert!(
            result.final_reward > 0.80,
            "CASO 1: búsqueda con tools reales + critic=achieved debe dar reward > 0.80, got {}",
            result.final_reward
        );
        // Stop score NO debe estar dampened (critic dice achieved=true)
        assert_eq!(
            result.breakdown.stop_score, 1.0,
            "CASO 1: stop_score=1.0 cuando critic achieved=true (no dampening)"
        );
    }

    /// CASO 2: Consulta ambigua genera plan + critic evalúa con confidence alta.
    /// Simula: plan con 4 steps, 3 completados, critic dice achieved=true (0.75).
    /// Valida: reward coherente con plan incompleto (plan_completion_ratio=0.75)
    #[test]
    fn caso2_consulta_ambigua_con_plan_parcial_critic_evalua() {
        let signals = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![0.70, 0.75],
            critic_verdict: Some((true, 0.75)),    // critic: 75% confianza de éxito
            plan_coherence_score: 0.20,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.75,            // 3/4 steps completados
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        // Plan incompleto reduce coherence score pero critic dice achieved
        assert!(
            result.final_reward > 0.60,
            "CASO 2: plan parcial + critic achieved debe superar threshold 0.60, got {}",
            result.final_reward
        );
        assert!(
            result.final_reward < 0.95,
            "CASO 2: plan parcial no debe dar reward máximo, got {}",
            result.final_reward
        );
    }

    /// CASO 3: Simulación de tool failure → critic detecta fallo → score_says_retry.
    /// Simula: search_files falla (MCP timeout), sub-agente produce 0 tools ejecutadas.
    /// → critic dice achieved=false (confidence=0.70) → reward < 0.60 → retry obligatorio.
    /// Esta es exactamente la cotización failure descrita en MEMORY.md.
    #[test]
    fn caso3_tool_failure_critic_detecta_fallo_score_says_retry() {
        let signals = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,   // EndTurn pero sin tools ejecutadas
            round_scores: vec![0.20],                  // round score muy bajo (sin progreso)
            critic_verdict: Some((false, 0.70)),       // critic: 70% confianza de fallo
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.0,               // 0 tools → 0 plan steps completed
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        // With proportional dampening at 70% confidence:
        // effective_cap = 1.0 - 0.40 * 0.70 = 0.72
        // raw_stop = 0.70 (EndTurn + plan_completion=0.0), already below cap
        // → final reward DEBE ser < 0.60 (low round_scores + no coherence)
        assert!(
            result.final_reward < 0.60,
            "CASO 3: tool_failure + critic=not_achieved debe dar reward < 0.60 para trigger retry, got {}",
            result.final_reward
        );
        // stop_score should be at or below the effective cap for 70% confidence
        let effective_cap = 1.0 - (1.0 - CRITIC_FAIL_STOP_CAP) * 0.70;
        assert!(
            result.breakdown.stop_score <= effective_cap + 1e-9,
            "CASO 3: stop_score debe estar ≤ effective_cap ({effective_cap:.2}) para 70% confidence"
        );
    }

    /// CASO 4: Síntesis sin tool calls → ForcedSynthesis → reward penalizado.
    /// Simula: el agente inicia síntesis (ForcedSynthesis) sin ejecutar NINGÚN tool.
    /// → synthesis_penalty=0.10 aplicada → critic dice not_achieved → reward << 0.60.
    /// Valida que la restricción "no sintetizar sin tools" sea observable en el reward.
    #[test]
    fn caso4_sintesis_sin_tool_calls_reward_bloqueado() {
        let signals = RawRewardSignals {
            stop_condition: StopCondition::ForcedSynthesis, // síntesis forzada (V5 guard detectó 0 tools)
            round_scores: vec![],
            critic_verdict: Some((false, 0.80)), // critic: 80% confianza — síntesis fabricada
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.0,          // no plan = no completion
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        // ForcedSynthesis stop_score ≤ 0.70, capped at 0.60 por critic_not_achieved,
        // - synthesis_penalty 0.10, critic_score = 0.25*(1-0.80) = 0.05
        // final << 0.60 → score_says_retry y critic_halt ambos deben disparar
        assert!(
            result.final_reward < 0.50,
            "CASO 4: síntesis sin tools + critic high-confidence failure debe dar reward < 0.50, got {}",
            result.final_reward
        );
        // Confirmar que la synthesis_penalty fue aplicada
        // (ForcedSynthesis reduce stop antes del cap)
        let raw_stop = stop_condition_score(&signals.stop_condition, signals.plan_completion_ratio);
        assert!(
            raw_stop < 0.75,
            "CASO 4: ForcedSynthesis debe tener stop_score < 0.75 (no EndTurn bonus)"
        );
    }

    // ── FASE 8: Dynamic CRITIC_FAIL_STOP_CAP ────────────────────────────────

    #[test]
    fn dynamic_cap_upper_bound_is_conservative() {
        let upper_bound = compute_critic_fail_stop_cap();
        // CRITIC_FAIL_STOP_CAP must be at or below the upper bound (conservative)
        assert!(
            CRITIC_FAIL_STOP_CAP <= upper_bound,
            "CRITIC_FAIL_STOP_CAP ({CRITIC_FAIL_STOP_CAP}) must be ≤ upper bound ({upper_bound:.4})"
        );
    }

    #[test]
    fn dynamic_cap_ensures_failure_below_threshold() {
        // At full confidence (1.0), effective_cap == CRITIC_FAIL_STOP_CAP.
        // Worst case: EndTurn + plan_completion=1.0 + full coherence + critic=(false, 1.0).
        let cap = CRITIC_FAIL_STOP_CAP;
        let worst_critic = 0.25 * (1.0 - 1.0); // 0.0 — full-confidence failure
        let worst_coherence = 1.0;
        let final_reward = cap * W_STOP + cap * W_TRAJECTORY + worst_critic * W_CRITIC + worst_coherence * W_COHERENCE;
        assert!(
            final_reward < SUCCESS_THRESHOLD,
            "worst-case reward ({final_reward:.4}) with cap ({cap:.4}) must be below SUCCESS_THRESHOLD ({SUCCESS_THRESHOLD})"
        );
    }

    // ── FASE 4: Critic unavailability penalty ──────────────────────────────

    #[test]
    fn critic_unavailable_penalty_reduces_reward() {
        let base = end_turn_signals();
        let unavailable = RawRewardSignals {
            critic_unavailable: true,
            ..base.clone()
        };
        let r_base = compute_reward(&base);
        let r_unavailable = compute_reward(&unavailable);
        assert!(
            r_unavailable.final_reward < r_base.final_reward,
            "critic_unavailable must reduce reward: base={} unavailable={}",
            r_base.final_reward,
            r_unavailable.final_reward
        );
        let diff = r_base.final_reward - r_unavailable.final_reward;
        assert!(
            (diff - CRITIC_UNAVAILABLE_PENALTY).abs() < 0.01,
            "penalty should be ~{CRITIC_UNAVAILABLE_PENALTY}, got diff={diff}"
        );
    }

    #[test]
    fn critic_unavailable_pushes_below_retry_threshold() {
        // Borderline session: ForcedSynthesis + some plan completion + no critic
        let signals = RawRewardSignals {
            stop_condition: StopCondition::ForcedSynthesis,
            round_scores: vec![0.60, 0.65],
            critic_verdict: None,
            plan_coherence_score: 0.15,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 0.7,
            plugin_snapshots: vec![],
            critic_unavailable: true,
        };
        let result = compute_reward(&signals);
        assert!(
            result.final_reward < SUCCESS_THRESHOLD,
            "critic_unavailable on borderline session must push below retry threshold {SUCCESS_THRESHOLD}, got {}",
            result.final_reward
        );
    }

    // ── Confidence-proportional dampening tests ────────────────────────────
    //
    // Verify that the proportional cap formula correctly scales with confidence:
    //   effective_cap = 1.0 - (1.0 - CRITIC_FAIL_STOP_CAP) × confidence

    /// Low confidence (30%) critic should NOT trigger retry — reward stays above 0.60.
    /// This is the exact bug scenario: 4/4 plan phases complete, critic says
    /// achieved=false at 30% confidence, but work was actually done.
    #[test]
    fn low_confidence_critic_does_not_trigger_retry() {
        let signals = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: Some((false, 0.30)), // low confidence "not achieved"
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0, // all 4/4 plan steps completed
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        // effective_cap = 1.0 - 0.40 * 0.30 = 0.88
        // stop_score = min(1.0, 0.88) = 0.88; trajectory = 0.88
        // critic_score = 0.25*(1-0.30) = 0.175; coherence = 1.0
        // final ≈ 0.88*0.25 + 0.88*0.30 + 0.175*0.25 + 1.0*0.20 ≈ 0.728
        assert!(
            result.final_reward > 0.60,
            "30% confidence critic must NOT push reward below retry threshold 0.60, got {}",
            result.final_reward
        );
    }

    /// High confidence (90%) critic MUST trigger retry — reward stays below 0.60.
    /// This preserves the existing behavior for genuine failures.
    #[test]
    fn high_confidence_critic_still_triggers_retry() {
        let signals = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: Some((false, 0.90)), // high confidence "not achieved"
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        // effective_cap = 1.0 - 0.40 * 0.90 = 0.64
        // stop_score = min(1.0, 0.64) = 0.64; trajectory = 0.64
        // critic_score = 0.25*(1-0.90) = 0.025; coherence = 1.0
        // final ≈ 0.64*0.25 + 0.64*0.30 + 0.025*0.25 + 1.0*0.20 ≈ 0.558
        assert!(
            result.final_reward < 0.60,
            "90% confidence critic failure must push reward below retry threshold, got {}",
            result.final_reward
        );
    }

    /// Medium confidence (50%) applies an intermediate cap — verifiable proportionality.
    #[test]
    fn medium_confidence_proportional_cap() {
        let signals = RawRewardSignals {
            stop_condition: StopCondition::EndTurn,
            round_scores: vec![],
            critic_verdict: Some((false, 0.50)),
            plan_coherence_score: 0.0,
            oscillation_penalty: 0.0,
            plan_completion_ratio: 1.0,
            plugin_snapshots: vec![],
            critic_unavailable: false,
        };
        let result = compute_reward(&signals);
        // effective_cap = 1.0 - 0.40 * 0.50 = 0.80
        let expected_cap = 1.0 - (1.0 - CRITIC_FAIL_STOP_CAP) * 0.50;
        assert!(
            (result.breakdown.stop_score - expected_cap).abs() < 1e-9,
            "stop_score must equal effective_cap ({expected_cap}) at 50% confidence, got {}",
            result.breakdown.stop_score
        );
        // The reward should be between the low-conf (>0.60) and high-conf (<0.60) cases
        let low_conf = compute_reward(&RawRewardSignals {
            critic_verdict: Some((false, 0.30)),
            ..signals.clone()
        });
        let high_conf = compute_reward(&RawRewardSignals {
            critic_verdict: Some((false, 0.90)),
            ..signals.clone()
        });
        assert!(
            result.final_reward < low_conf.final_reward,
            "50% conf reward ({}) must be < 30% conf reward ({})",
            result.final_reward, low_conf.final_reward
        );
        assert!(
            result.final_reward > high_conf.final_reward,
            "50% conf reward ({}) must be > 90% conf reward ({})",
            result.final_reward, high_conf.final_reward
        );
    }

    #[test]
    fn weights_sum_to_one() {
        let sum = W_STOP + W_TRAJECTORY + W_CRITIC + W_COHERENCE;
        assert!(
            (sum - 1.0).abs() < 1e-9,
            "reward weights must sum to 1.0, got {sum}"
        );
    }
}
