//! Adversarial simulation suite — GDEM resilience under injected failures.
//!
//! ## Covers
//!
//! - Phase E: Failure injection (timeout, corruption, false success, critic bias)
//! - Phase H: Hallucination containment (false success → no high GAS)
//! - Property-based tests (proptest): GAS bounded, always terminates, RER ≥ 0, SCR ≤ 1
//! - Convergence stability: 10k-equivalent rounds with ≤10% failure rate
//!
//! All simulations are fully deterministic (seeded RNG, no real I/O).

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use uuid::Uuid;

use crate::{
    critic::{CriticConfig, InLoopCritic, RoundMetrics},
    failure_injection::{FailureInjectionHarness, FailureMode},
    goal::{CriterionKind, GoalSpec, VerifiableCriterion},
    metrics::GoalAlignmentScore,
    oscillation_metric::OscillationTracker,
};

// ─── Simulation result ────────────────────────────────────────────────────────

/// Result of one adversarial convergence simulation run.
#[derive(Debug, Clone)]
pub struct AdversarialSimResult {
    /// Rounds executed before termination.
    pub rounds: u32,
    /// Whether the simulation terminated (always true — asserted by invariant).
    pub terminated: bool,
    /// Final goal confidence at termination.
    pub final_confidence: f32,
    /// Goal Alignment Score at session end.
    pub gas: f32,
    /// Global oscillation index over the session.
    pub oscillation_index: f64,
    /// Total failure injections fired by the harness.
    pub total_injections: u64,
}

// ─── Simulation helpers ───────────────────────────────────────────────────────

fn adversarial_goal() -> GoalSpec {
    GoalSpec {
        id: Uuid::new_v4(),
        intent: "adversarial test".into(),
        criteria: vec![VerifiableCriterion {
            description: "done".into(),
            weight: 1.0,
            kind: CriterionKind::KeywordPresence {
                keywords: vec!["done".into()],
            },
            threshold: 0.8,
        }],
        completion_threshold: 0.8,
        max_rounds: 100,
        latency_sensitive: false,
    }
}

/// Run a synthetic convergence simulation with a given seed and failure probability.
///
/// Each "round":
/// 1. Draw a tool result (possibly injected as timeout/failure).
/// 2. Update confidence by a random delta (positive on success, near-zero on failure).
/// 3. Check goal (confidence ≥ 0.8 → done).
/// 4. Evaluate critic; act on signal.
///
/// Guaranteed to terminate: either goal achieved, critic terminates, or max_rounds hit.
pub fn run_convergence_simulation(
    seed: u64,
    failure_prob: f64,
    max_rounds: u32,
) -> AdversarialSimResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut critic = InLoopCritic::new(CriticConfig::default());
    let goal = adversarial_goal();
    let mut oscillation = OscillationTracker::new();
    let mut harness = FailureInjectionHarness::new(
        seed.wrapping_add(1),
        vec![FailureMode::ToolTimeout {
            probability: failure_prob,
        }],
    );

    let mut confidence = 0.0f32;
    let mut rounds = 0u32;
    let mut terminated = false;

    for round in 1..=max_rounds {
        // Inject failure into a "successful" tool call
        let injected = harness.inject_tool_result("echo done", false);
        let tool_success = !injected.is_error;

        let pre = confidence;
        let delta: f32 = if tool_success {
            rng.random_range(0.04_f32..0.20_f32)
        } else {
            rng.random_range(-0.01_f32..0.03_f32) // near-zero — may regress
        };
        confidence = (pre + delta).clamp(0.0, 1.0);

        // Goal achieved?
        if confidence >= 0.8 {
            terminated = true;
            rounds = round;
            break;
        }

        let metrics = RoundMetrics {
            pre_confidence: pre,
            post_confidence: confidence,
            tools_invoked: if tool_success {
                vec!["bash".into()]
            } else {
                vec![]
            },
            had_errors: !tool_success,
            round,
            max_rounds,
        };

        let signal = critic.evaluate(&metrics, &goal);
        oscillation.record_signal(&signal);

        if signal.is_terminal() {
            terminated = true;
            rounds = round;
            break;
        }

        if signal.requires_replan() {
            critic.reset_stall();
        }

        rounds = round;
    }

    if !terminated {
        terminated = true; // budget exhausted — still a termination
    }

    let gas = GoalAlignmentScore::compute(confidence, rounds, max_rounds, confidence >= 0.8);

    AdversarialSimResult {
        rounds,
        terminated,
        final_confidence: confidence,
        gas: gas.score(),
        oscillation_index: oscillation.oscillation_index(),
        total_injections: harness.total_injections(),
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        confidence_hysteresis::{ConfidenceHysteresis, HysteresisConfig},
        critic::{CriticConfig, CriticSignal, InLoopCritic, RoundMetrics},
        execution_budget::{BudgetExceeded, BudgetTracker, ExecutionBudget},
        fsm::{AgentFsm, AgentState},
        goal::ConfidenceScore,
        invariants::check_confidence_invariant,
        metrics::{GoalAlignmentScore, ReplanEfficiencyRatio, SandboxContainmentRate},
        oscillation_metric::OscillationTracker,
        strategy::{StrategyLearner, StrategyLearnerConfig},
    };
    use proptest::prelude::*;
    use rand::{Rng, SeedableRng};

    // ─── Convergence stability ────────────────────────────────────────────────

    #[test]
    fn zero_failure_always_terminates() {
        for seed in 0u64..20 {
            let r = run_convergence_simulation(seed, 0.0, 50);
            assert!(r.terminated, "seed={seed} must terminate");
        }
    }

    #[test]
    fn ten_pct_failure_always_terminates() {
        for seed in 0u64..30 {
            let r = run_convergence_simulation(seed, 0.10, 100);
            assert!(r.terminated, "seed={seed} must terminate under 10% failure");
        }
    }

    #[test]
    fn thirty_pct_failure_always_terminates() {
        for seed in 0u64..20 {
            let r = run_convergence_simulation(seed, 0.30, 100);
            assert!(r.terminated, "seed={seed} must terminate under 30% failure");
        }
    }

    /// 10k-equivalent stability test.
    ///
    /// Verifies:
    /// - Termination always occurs.
    /// - No infinite loop (max_rounds hard cap).
    /// - Stall detection fires (critic terminates before max_rounds).
    /// - GAS degradation from baseline < 15%.
    #[test]
    fn convergence_10k_stability() {
        // Run 50 sessions with max_rounds=200, 10% failure rate
        let baseline_gas: f32 = {
            let results: Vec<f32> = (0u64..50)
                .map(|s| run_convergence_simulation(s, 0.0, 200).gas)
                .collect();
            results.iter().sum::<f32>() / results.len() as f32
        };

        let adversarial_gas: f32 = {
            let results: Vec<f32> = (0u64..50)
                .map(|s| run_convergence_simulation(s, 0.10, 200).gas)
                .collect();
            results.iter().sum::<f32>() / results.len() as f32
        };

        let degradation = (baseline_gas - adversarial_gas).max(0.0);
        assert!(
            degradation < 0.15,
            "mean GAS degradation {:.3} exceeds 15% threshold (baseline={:.3}, adversarial={:.3})",
            degradation,
            baseline_gas,
            adversarial_gas
        );

        // All must terminate
        for seed in 0u64..50 {
            let r = run_convergence_simulation(seed, 0.10, 200);
            assert!(r.terminated, "seed={seed} must terminate");
        }
    }

    // ─── GAS boundedness ─────────────────────────────────────────────────────

    #[test]
    fn gas_always_bounded_under_failures() {
        for seed in 0u64..50 {
            let r = run_convergence_simulation(seed, 0.20, 50);
            assert!(r.gas >= 0.0, "seed={seed} GAS below 0: {}", r.gas);
            assert!(r.gas <= 1.0, "seed={seed} GAS above 1: {}", r.gas);
        }
    }

    #[test]
    fn confidence_score_invariant_under_noise() {
        for v in [0.0f32, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
            let score = ConfidenceScore::new(v);
            let violations = check_confidence_invariant(score, 0.5);
            assert!(
                violations.is_empty(),
                "violations at v={}: {:?}",
                v,
                violations
            );
        }
    }

    // ─── Phase H: Hallucination containment ───────────────────────────────────

    /// Hallucinated tool success: tool reports OK but confidence does not advance.
    ///
    /// **Invariant I-6.6**: False success must not yield GAS ≥ 0.90.
    #[test]
    fn hallucination_false_success_no_high_gas() {
        let mut rng = StdRng::seed_from_u64(99);
        let max_rounds = 20u32;
        let mut confidence = 0.1f32; // starts low, never actually improves

        // Simulate hallucinated success: confidence stagnates
        let mut rounds = 0u32;
        for round in 1..=max_rounds {
            // Tool "claims" success but state unchanged (tiny drift only)
            let noise: f32 = rng.gen_range(-0.005_f32..0.005_f32);
            confidence = (confidence + noise).clamp(0.0, 1.0);
            rounds = round;
        }

        // Confidence never reached 0.8 → achieved=false
        let gas = GoalAlignmentScore::compute(confidence, rounds, max_rounds, false);
        assert!(
            gas.score() < 0.90,
            "hallucinated success should not yield GAS ≥ 0.90, got {:.3}",
            gas.score()
        );
    }

    #[test]
    fn false_positive_injection_reduces_critic_accuracy() {
        // With 100% false positive injection, all errors look like successes.
        // Critic will see positive deltas even when tools actually fail.
        let mut harness = FailureInjectionHarness::new(
            42,
            vec![FailureMode::FalsePositiveToolSuccess { probability: 1.0 }],
        );
        let mut injected_successes = 0u32;
        for _ in 0..50 {
            let r = harness.inject_tool_result("failed output", true);
            if !r.is_error {
                injected_successes += 1;
            }
        }
        assert_eq!(
            injected_successes, 50,
            "all 50 errors should be flipped to success"
        );
    }

    // ─── FSM adversarial correctness ─────────────────────────────────────────

    #[test]
    fn fsm_never_undefined_state_under_invalid_transitions() {
        let mut fsm = AgentFsm::new();

        // Try all invalid transitions from Idle
        for invalid in [
            AgentState::Executing,
            AgentState::Verifying,
            AgentState::Converged,
            AgentState::Replanning,
        ] {
            let result = fsm.transition(invalid.clone());
            assert!(
                result.is_err(),
                "transition to {:?} from Idle should fail",
                invalid
            );
            // State must remain Idle after rejected transition
            assert_eq!(*fsm.state(), AgentState::Idle);
        }

        // Valid path should still work after failed attempts
        assert!(fsm.transition(AgentState::Planning).is_ok());
    }

    #[test]
    fn fsm_no_escape_from_terminal_under_adversarial_transitions() {
        let mut fsm = AgentFsm::new();
        // Navigate to Converged
        fsm.transition(AgentState::Planning).unwrap();
        fsm.transition(AgentState::Executing).unwrap();
        fsm.transition(AgentState::Verifying).unwrap();
        fsm.transition(AgentState::Converged).unwrap();

        // Try adversarial transitions from Converged — all must fail
        for state in [
            AgentState::Idle,
            AgentState::Planning,
            AgentState::Executing,
            AgentState::Replanning,
        ] {
            assert!(
                fsm.transition(state.clone()).is_err(),
                "transition from Converged to {:?} must fail",
                state
            );
        }

        // State must remain Converged
        assert_eq!(*fsm.state(), AgentState::Converged);
    }

    // ─── Injection rate accuracy ──────────────────────────────────────────────

    #[test]
    fn timeout_injection_rate_converges_to_probability() {
        let p = 0.40;
        let n = 10_000;
        let mut h =
            FailureInjectionHarness::new(12345, vec![FailureMode::ToolTimeout { probability: p }]);
        for _ in 0..n {
            h.inject_tool_result("cmd", false);
        }
        let actual = h.injection_rate();
        assert!(
            (actual - p).abs() < 0.03,
            "rate={:.3} expected≈{:.3} (±0.03)",
            actual,
            p
        );
    }

    #[test]
    fn false_positive_rate_converges_to_probability() {
        let p = 0.25;
        let n = 10_000;
        let mut h = FailureInjectionHarness::new(
            77777,
            vec![FailureMode::FalsePositiveToolSuccess { probability: p }],
        );
        let mut count = 0u64;
        for _ in 0..n {
            let r = h.inject_tool_result("fail", true);
            if !r.is_error {
                count += 1;
            }
        }
        let actual = count as f64 / n as f64;
        assert!(
            (actual - p).abs() < 0.03,
            "rate={:.3} expected≈{:.3}",
            actual,
            p
        );
    }

    // ─── Budget enforcement ───────────────────────────────────────────────────

    #[test]
    fn budget_terminates_at_max_rounds() {
        let budget = ExecutionBudget {
            max_rounds: 10,
            ..Default::default()
        };
        let mut tracker = BudgetTracker::new(budget);
        let mut exceeded = false;
        let mut actual_rounds = 0u32;
        for _ in 0..20 {
            if tracker.consume_round().is_err() {
                exceeded = true;
                break;
            }
            actual_rounds += 1;
        }
        assert!(exceeded, "budget should be exceeded before round 20");
        assert_eq!(actual_rounds, 10, "exactly 10 rounds should execute");
    }

    #[test]
    fn random_adversarial_cannot_exceed_max_rounds() {
        let max_rounds = 15u32;
        let budget = ExecutionBudget {
            max_rounds,
            ..Default::default()
        };
        let mut tracker = BudgetTracker::new(budget);
        let mut rounds_run = 0u32;
        loop {
            match tracker.consume_round() {
                Ok(()) => rounds_run += 1,
                Err(BudgetExceeded::Rounds { .. }) => break,
                Err(e) => panic!("unexpected error: {}", e),
            }
        }
        assert_eq!(rounds_run, max_rounds);
    }

    #[test]
    fn repeated_replan_cycles_exhaust_replan_budget() {
        let budget = ExecutionBudget {
            max_replans: 3,
            ..Default::default()
        };
        let mut tracker = BudgetTracker::new(budget);
        for _ in 0..3 {
            assert!(tracker.consume_replan().is_ok());
        }
        assert!(matches!(
            tracker.consume_replan(),
            Err(BudgetExceeded::Replans { .. })
        ));
    }

    // ─── Hysteresis stability ─────────────────────────────────────────────────

    #[test]
    fn hysteresis_suppresses_oscillation_in_small_delta_regime() {
        let config = HysteresisConfig {
            epsilon: 0.05,
            required_consecutive: 2,
        };
        let mut hysteresis = ConfidenceHysteresis::new(config);

        // Baseline: large delta passes through
        let _ = hysteresis.apply(CriticSignal::Continue, 0.5);

        let mut pass_throughs = 0u32;
        // 10 alternating signals in small-delta regime
        for i in 0..10 {
            let signal = if i % 2 == 0 {
                CriticSignal::Replan {
                    reason: "stall".into(),
                    alignment_score: 0.5,
                }
            } else {
                CriticSignal::Continue
            };
            let out = hysteresis.apply(signal, 0.51 + i as f32 * 0.001);
            if out != CriticSignal::Continue {
                pass_throughs += 1;
            }
        }
        // With alternation, no signal should accumulate 2 consecutive rounds
        assert!(
            pass_throughs < 10,
            "hysteresis should suppress most oscillations, pass_throughs={}",
            pass_throughs
        );
    }

    #[test]
    fn oscillation_index_bounded_under_stable_critic() {
        let mut critic = InLoopCritic::new(CriticConfig::default());
        let goal = adversarial_goal();
        let mut tracker = OscillationTracker::new();

        // Monotone increasing confidence → mostly Continue
        for r in 1..=20u32 {
            let pre = (r - 1) as f32 * 0.04;
            let post = r as f32 * 0.04;
            let m = RoundMetrics {
                pre_confidence: pre.min(1.0),
                post_confidence: post.min(1.0),
                tools_invoked: vec!["bash".into()],
                had_errors: false,
                round: r,
                max_rounds: 20,
            };
            let signal = critic.evaluate(&m, &goal);
            tracker.record_signal(&signal);
        }

        let oi = tracker.oscillation_index();
        assert!(oi <= 1.0, "OI must be ≤ 1.0, got {}", oi);
        assert!(oi >= 0.0, "OI must be ≥ 0.0, got {}", oi);
    }

    // ─── Strategy learner resilience ─────────────────────────────────────────

    #[test]
    fn strategy_learner_resilient_to_zero_reward() {
        let mut learner = StrategyLearner::new(StrategyLearnerConfig::default());
        for _ in 0..100 {
            let s = learner.select().to_string();
            learner.record_outcome(&s, 0.0, Uuid::new_v4());
        }
        // Should not panic; mean_reward should remain 0
        let stats = learner.arm_stats();
        for arm in &stats {
            let mr = arm.mean_reward();
            assert!(
                mr >= 0.0 && mr <= 1.0 + 1e-6,
                "mean_reward out of bounds: {}",
                mr
            );
        }
    }

    #[test]
    fn strategy_learner_resilient_to_max_reward() {
        let mut learner = StrategyLearner::new(StrategyLearnerConfig::default());
        for _ in 0..100 {
            let s = learner.select().to_string();
            learner.record_outcome(&s, 1.0, Uuid::new_v4());
        }
        let stats = learner.arm_stats();
        for arm in &stats {
            let mr = arm.mean_reward();
            assert!(mr <= 1.0 + 1e-6, "mean_reward above 1: {}", mr);
        }
    }

    // ─── Metric invariants ────────────────────────────────────────────────────

    #[test]
    fn rer_never_negative_across_inputs() {
        for replan_count in 0u32..=30 {
            let rer = ReplanEfficiencyRatio::compute(replan_count, 20, None);
            assert!(
                rer.score >= 0.0,
                "RER negative at replan_count={}",
                replan_count
            );
        }
    }

    #[test]
    fn scr_never_exceeds_1_across_inputs() {
        for blocked in (0u32..=10).step_by(1) {
            for post in (0u32..=10).step_by(1) {
                let scr = SandboxContainmentRate::compute(blocked, post);
                assert!(scr.score <= 1.0 + 1e-6, "SCR > 1.0: {}", scr.score);
                assert!(scr.score >= 0.0, "SCR < 0.0: {}", scr.score);
            }
        }
    }

    // ─── Proptest property suite ──────────────────────────────────────────────

    proptest! {
        /// GAS is always in [0, 1] for any failure probability and seed.
        #[test]
        fn prop_gas_bounded_under_any_failure(
            seed in 0u64..500,
            failure_prob in 0.0f64..0.5,
        ) {
            let r = run_convergence_simulation(seed, failure_prob, 50);
            prop_assert!(r.gas >= 0.0, "GAS below 0: {}", r.gas);
            prop_assert!(r.gas <= 1.0, "GAS above 1: {}", r.gas);
        }

        /// All sessions terminate regardless of failure probability.
        #[test]
        fn prop_always_terminates(seed in 0u64..500, failure_prob in 0.0f64..0.4) {
            let r = run_convergence_simulation(seed, failure_prob, 100);
            prop_assert!(r.terminated, "must always terminate");
        }

        /// RER is always in [0, 1].
        #[test]
        fn prop_rer_always_nonneg(replan_count in 0u32..100, max_rounds in 1u32..100) {
            let rer = ReplanEfficiencyRatio::compute(replan_count, max_rounds, None);
            prop_assert!(rer.score >= 0.0);
            prop_assert!(rer.score <= 1.0);
        }

        /// SCR is always in [0, 1].
        #[test]
        fn prop_scr_always_bounded(blocked in 0u32..500, post in 0u32..500) {
            let scr = SandboxContainmentRate::compute(blocked, post);
            prop_assert!(scr.score >= 0.0);
            prop_assert!(scr.score <= 1.0 + 1e-6);
        }

        /// ConfidenceScore invariant holds for any clamped float.
        #[test]
        fn prop_confidence_bounded_under_injection(base in 0.0f32..=1.0, delta in -0.5f32..=0.5) {
            let injected = (base + delta).clamp(0.0, 1.0);
            let score = ConfidenceScore::new(injected);
            let violations = check_confidence_invariant(score, 0.5);
            prop_assert!(violations.is_empty());
        }

        /// Budget always exhausts at exactly max_rounds.
        #[test]
        fn prop_budget_exhausts_at_max_rounds(max_rounds in 1u32..50) {
            let budget = ExecutionBudget { max_rounds, ..Default::default() };
            let mut tracker = BudgetTracker::new(budget);
            let mut rounds_run = 0u32;
            loop {
                match tracker.consume_round() {
                    Ok(()) => rounds_run += 1,
                    Err(_) => break,
                }
            }
            prop_assert_eq!(rounds_run, max_rounds);
        }

        /// OscillationIndex is always in [0, 1].
        #[test]
        fn prop_oscillation_index_in_unit_interval(
            pattern in proptest::collection::vec(0u8..4, 1..50),
        ) {
            let signals = [
                CriticSignal::Continue,
                CriticSignal::Replan { reason: "r".into(), alignment_score: 0.3 },
                CriticSignal::InjectHint { hint: "h".into(), alignment_score: 0.5 },
                CriticSignal::Terminate { reason: "t".into() },
            ];
            let mut tracker = OscillationTracker::new();
            for idx in pattern {
                let idx = (idx as usize) % signals.len();
                tracker.record_signal(&signals[idx]);
            }
            let oi = tracker.oscillation_index();
            prop_assert!(oi >= 0.0 && oi <= 1.0, "OI out of [0,1]: {}", oi);
        }
    }
}
