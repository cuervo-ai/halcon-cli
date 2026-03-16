//! Unified loop termination authority — Sprint 2 of SOTA 2026 L6 architecture.
//!
//! `TerminationOracle` consolidates 4 independent loop control systems
//! (`ConvergenceController`, `ToolLoopGuard`, `RoundScorer` synthesis/replan signals)
//! into an explicit, testable precedence order.
//!
//! # Deployment mode
//! Initially deployed in **shadow mode** (advisory only). The oracle's decision is
//! computed and logged at DEBUG level alongside existing control flow. No behavior
//! change until the shadow mode flag is removed in a future sprint.
//!
//! # Precedence order (documented and exhaustively tested)
//! 1. **Halt** — ConvergenceController::Halt OR LoopSignal::Break
//! 2. **InjectSynthesis** — ConvergenceController::Synthesize OR LoopSignal::InjectSynthesis OR replan_advised=synthesis_advised=true with Continue
//! 3. **Replan** — ConvergenceController::Replan OR LoopSignal::ReplanRequired OR replan_advised
//! 4. **ForceNoTools** — LoopSignal::ForceNoTools
//! 5. **Continue** — default

use super::convergence_controller::ConvergenceAction;
use super::round_feedback::{LoopSignal, RoundFeedback};

// ── Reason types ─────────────────────────────────────────────────────────────

/// Identifies which authority triggered a synthesis decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthesisReason {
    /// `ConvergenceController` returned `ConvergenceAction::Synthesize`.
    ConvergenceControllerSynthesizeAction,
    /// `ToolLoopGuard` returned `LoopAction::InjectSynthesis` (mapped to `LoopSignal::InjectSynthesis`).
    LoopGuardInjectSynthesis,
    /// `RoundScorer.should_inject_synthesis()` fired (consecutive regression rounds).
    RoundScorerConsecutiveRegression,
    /// `MidLoopCritic` fired `CriticAction::ForceSynthesis` (budget >90% with <60% progress).
    MidLoopCriticForceSynthesis,
}

/// Identifies which authority triggered a replan decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplanReason {
    /// `ConvergenceController` returned `ConvergenceAction::Replan`.
    ConvergenceControllerReplanAction,
    /// `ToolLoopGuard` returned `LoopAction::ReplanRequired` (mapped to `LoopSignal::ReplanRequired`).
    LoopGuardStagnationDetected,
    /// `RoundScorer.should_trigger_replan()` fired (persistent low trajectory).
    RoundScorerLowTrajectory,
    /// `MidLoopCritic` fired `CriticAction::Replan`, `ReduceScope`, or `ChangeStrategy`.
    MidLoopCriticReplan,
}

// ── TerminationDecision ───────────────────────────────────────────────────────

/// Unified termination decision produced by `TerminationOracle::adjudicate`.
///
/// Single output from 4 input authorities with explicit precedence ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationDecision {
    /// Continue to next round — no termination authority fired.
    Continue,
    /// Suppress tools next round.
    ForceNoTools,
    /// Force the model to synthesize and end this phase of work.
    InjectSynthesis { reason: SynthesisReason },
    /// Current approach is failing — trigger replanning.
    Replan { reason: ReplanReason },
    /// Hard stop — no further rounds.
    Halt,
}

// ── TerminationOracle ─────────────────────────────────────────────────────────

/// Stateless oracle that adjudicates 4 loop control signals into one binding decision.
///
/// All logic is pure: same inputs always produce same output.
/// Stateless means it can be called in advisory/shadow mode without side effects.
pub struct TerminationOracle;

impl TerminationOracle {
    /// Adjudicate 4 independent signals into one binding `TerminationDecision`.
    ///
    /// # Precedence
    /// 1. **Halt** — `ConvergenceAction::Halt` OR `LoopSignal::Break` (hard stop)
    /// 2. **InjectSynthesis** — `ConvergenceAction::Synthesize` (highest semantic authority)
    /// 3c. **Mid-loop critic** — `CriticAction::ForceSynthesis/Replan/ReduceScope/ChangeStrategy`
    ///    OR `LoopSignal::InjectSynthesis` (loop guard escalation)
    ///    OR `feedback.synthesis_advised` (RoundScorer consecutive regressions)
    /// 3. **Replan** — `ConvergenceAction::Replan`
    ///    OR `LoopSignal::ReplanRequired`
    ///    OR `feedback.replan_advised` (RoundScorer low trajectory)
    /// 4. **ForceNoTools** — `LoopSignal::ForceNoTools`
    /// 5. **Continue** — default when no authority fires
    pub fn adjudicate(feedback: &RoundFeedback) -> TerminationDecision {
        // ── Precedence 1: Halt ────────────────────────────────────────────────
        if feedback.convergence_action == ConvergenceAction::Halt
            || feedback.loop_signal == LoopSignal::Break
        {
            return TerminationDecision::Halt;
        }

        // ── GovernanceRescue gate (ARCH-SYNC-1 fix) ───────────────────────────
        // When SynthesisGate::GovernanceRescue would block synthesis (reflection_score < 0.15
        // AND rounds_executed < 3), ALL InjectSynthesis paths below are skipped.
        // Halt (Precedence 1) is unaffected — hard stops always fire.
        // This ensures TerminationOracle cannot bypass the quality gate.
        if feedback.governance_rescue_active {
            tracing::warn!(
                round = feedback.round,
                reflection_score = feedback.combined_score,
                "GovernanceRescue: Synthesize blocked — reflection below threshold"
            );
        }

        // ── Precedence 2: InjectSynthesis ─────────────────────────────────────
        if !feedback.governance_rescue_active {
            if feedback.convergence_action == ConvergenceAction::Synthesize {
                return TerminationDecision::InjectSynthesis {
                    reason: SynthesisReason::ConvergenceControllerSynthesizeAction,
                };
            }
            if feedback.loop_signal == LoopSignal::InjectSynthesis {
                return TerminationDecision::InjectSynthesis {
                    reason: SynthesisReason::LoopGuardInjectSynthesis,
                };
            }
            if feedback.synthesis_advised {
                // Phase 3.6: Utility-based synthesis delay.
                // RoundScorer's synthesis_advised is the weakest synthesis signal. When the
                // convergence utility score is above the synthesis threshold, delay synthesis
                // to let the model continue productive work. Fall-through to Phase 6 legacy
                // evidence check as a safety net.
                const UTILITY_SYNTHESIS_THRESHOLD: f64 = 0.35;
                const EVIDENCE_COVERAGE_THRESHOLD: f64 = 0.30;
                const LATE_ROUND_CUTOFF: usize = 8;

                // Primary gate: utility score (P3.6)
                if feedback.utility_score > UTILITY_SYNTHESIS_THRESHOLD {
                    tracing::debug!(
                        round = feedback.round,
                        utility_score = %feedback.utility_score,
                        "Oracle: delaying synthesis — utility above threshold, work still productive"
                    );
                    // Fall through to lower-precedence checks (Replan/ForceNoTools/Continue).
                }
                // Legacy fallback: evidence coverage (Phase 6)
                else if feedback.evidence_coverage < EVIDENCE_COVERAGE_THRESHOLD
                    && feedback.round < LATE_ROUND_CUTOFF
                {
                    tracing::debug!(
                        round = feedback.round,
                        evidence_coverage = %feedback.evidence_coverage,
                        "Oracle: delaying synthesis — low evidence coverage, allowing more collection"
                    );
                    // Fall through to lower-precedence checks (Replan/ForceNoTools/Continue).
                } else {
                    return TerminationDecision::InjectSynthesis {
                        reason: SynthesisReason::RoundScorerConsecutiveRegression,
                    };
                }
            }
        }

        // ── Precedence 3: Replan ──────────────────────────────────────────────
        if feedback.convergence_action == ConvergenceAction::Replan {
            return TerminationDecision::Replan {
                reason: ReplanReason::ConvergenceControllerReplanAction,
            };
        }
        if feedback.loop_signal == LoopSignal::ReplanRequired {
            return TerminationDecision::Replan {
                reason: ReplanReason::LoopGuardStagnationDetected,
            };
        }
        if feedback.replan_advised {
            return TerminationDecision::Replan {
                reason: ReplanReason::RoundScorerLowTrajectory,
            };
        }

        // ── Precedence 3b: Mini-Critic signals ──────────────────────────────
        // Phase 5 Governance: mini-critic is now INPUT to oracle, not post-override.
        // Its signals have lower precedence than all other replan/synthesis sources
        // but higher precedence than ForceNoTools/Continue.
        // Oracle Halt (precedence 1) always wins over mini-critic.
        if feedback.mini_critic_synthesis && !feedback.governance_rescue_active {
            return TerminationDecision::InjectSynthesis {
                reason: SynthesisReason::ConvergenceControllerSynthesizeAction,
            };
        }
        if feedback.mini_critic_replan {
            return TerminationDecision::Replan {
                reason: ReplanReason::LoopGuardStagnationDetected,
            };
        }

        // ── Precedence 3c: Mid-Loop Critic ───────────────────────────────────
        // Lower precedence than ConvergenceController/LoopGuard/RoundScorer but higher than
        // ForceNoTools.  ForceSynthesis is blocked by GovernanceRescue like all synthesis paths.
        use super::mid_loop_critic::CriticAction;
        match feedback.mid_critic_action {
            Some(CriticAction::ForceSynthesis) if !feedback.governance_rescue_active => {
                return TerminationDecision::InjectSynthesis {
                    reason: SynthesisReason::MidLoopCriticForceSynthesis,
                };
            }
            Some(CriticAction::Replan | CriticAction::ReduceScope | CriticAction::ChangeStrategy) => {
                return TerminationDecision::Replan {
                    reason: ReplanReason::MidLoopCriticReplan,
                };
            }
            _ => {}
        }

        // ── Precedence 4: ForceNoTools ────────────────────────────────────────
        if feedback.loop_signal == LoopSignal::ForceNoTools {
            return TerminationDecision::ForceNoTools;
        }

        // ── Precedence 5: Continue (default) ──────────────────────────────────
        TerminationDecision::Continue
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::domain::round_feedback::RoundFeedback;

    fn base_feedback() -> RoundFeedback {
        RoundFeedback {
            round: 1,
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
            evidence_coverage: 1.0,
            semantic_cycle_detected: false,
            cycle_severity: 0.0,
            utility_score: 0.5,
            mid_critic_action: None,
            complexity_upgraded: false,
            problem_class: None,
            forecast_rounds_remaining: None,
            utility_should_synthesize: false,
            synthesis_request_count: 0,
            fsm_error_count: 0,
            budget_iteration_count: 0,
            budget_stagnation_count: 0,
            budget_token_growth: 0,
            budget_exhausted: false,
            executive_signal_count: 0,
            executive_force_reason: None,
            capability_violation: None,
            security_signals_detected: false,
            tool_call_count: 0,
            tool_failure_count: 0,
            governance_rescue_active: false,
        }
    }

    // ── Precedence 1: Halt ────────────────────────────────────────────────────

    #[test]
    fn halt_beats_all_other_signals() {
        let mut fb = base_feedback();
        fb.convergence_action = ConvergenceAction::Halt;
        fb.loop_signal = LoopSignal::InjectSynthesis;
        fb.replan_advised = true;
        fb.synthesis_advised = true;
        assert_eq!(TerminationOracle::adjudicate(&fb), TerminationDecision::Halt);
    }

    #[test]
    fn break_signal_produces_halt() {
        let mut fb = base_feedback();
        fb.loop_signal = LoopSignal::Break;
        assert_eq!(TerminationOracle::adjudicate(&fb), TerminationDecision::Halt);
    }

    #[test]
    fn convergence_halt_produces_halt() {
        let mut fb = base_feedback();
        fb.convergence_action = ConvergenceAction::Halt;
        assert_eq!(TerminationOracle::adjudicate(&fb), TerminationDecision::Halt);
    }

    #[test]
    fn break_beats_synthesize() {
        let mut fb = base_feedback();
        fb.loop_signal = LoopSignal::Break;
        fb.convergence_action = ConvergenceAction::Synthesize;
        // Break → Halt; Synthesize is lower precedence
        assert_eq!(TerminationOracle::adjudicate(&fb), TerminationDecision::Halt);
    }

    // ── Precedence 2: InjectSynthesis ─────────────────────────────────────────

    #[test]
    fn convergence_synthesize_produces_inject_synthesis() {
        let mut fb = base_feedback();
        fb.convergence_action = ConvergenceAction::Synthesize;
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::InjectSynthesis {
                reason: SynthesisReason::ConvergenceControllerSynthesizeAction,
            }
        );
    }

    #[test]
    fn loop_guard_inject_synthesis_produces_inject_synthesis() {
        let mut fb = base_feedback();
        fb.loop_signal = LoopSignal::InjectSynthesis;
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::InjectSynthesis {
                reason: SynthesisReason::LoopGuardInjectSynthesis,
            }
        );
    }

    #[test]
    fn synthesis_advised_produces_inject_synthesis() {
        let mut fb = base_feedback();
        fb.synthesis_advised = true;
        fb.utility_score = 0.20; // below threshold → synthesis proceeds
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::InjectSynthesis {
                reason: SynthesisReason::RoundScorerConsecutiveRegression,
            }
        );
    }

    #[test]
    fn synthesize_beats_replan() {
        let mut fb = base_feedback();
        fb.convergence_action = ConvergenceAction::Synthesize;
        fb.replan_advised = true;
        // Synthesize (P2) beats Replan (P3)
        assert!(matches!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::InjectSynthesis { .. }
        ));
    }

    // When both synthesis_advised and loop_signal::InjectSynthesis fire,
    // LoopGuard wins because it's checked first.
    #[test]
    fn both_synthesis_signals_loop_guard_wins() {
        let mut fb = base_feedback();
        fb.loop_signal = LoopSignal::InjectSynthesis;
        fb.synthesis_advised = true;
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::InjectSynthesis {
                reason: SynthesisReason::LoopGuardInjectSynthesis,
            }
        );
    }

    // ── Precedence 3: Replan ──────────────────────────────────────────────────

    #[test]
    fn convergence_replan_produces_replan() {
        let mut fb = base_feedback();
        fb.convergence_action = ConvergenceAction::Replan;
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::Replan {
                reason: ReplanReason::ConvergenceControllerReplanAction,
            }
        );
    }

    #[test]
    fn loop_guard_replan_required_produces_replan() {
        let mut fb = base_feedback();
        fb.loop_signal = LoopSignal::ReplanRequired;
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::Replan {
                reason: ReplanReason::LoopGuardStagnationDetected,
            }
        );
    }

    #[test]
    fn replan_advised_produces_replan() {
        let mut fb = base_feedback();
        fb.replan_advised = true;
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::Replan {
                reason: ReplanReason::RoundScorerLowTrajectory,
            }
        );
    }

    #[test]
    fn replan_beats_force_no_tools() {
        let mut fb = base_feedback();
        fb.replan_advised = true;
        fb.loop_signal = LoopSignal::ForceNoTools;
        // Replan (P3) beats ForceNoTools (P4)
        assert!(matches!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::Replan { .. }
        ));
    }

    // When both replan_advised and loop_signal::ReplanRequired fire,
    // LoopGuard wins because it's checked first.
    #[test]
    fn both_replan_signals_loop_guard_wins() {
        let mut fb = base_feedback();
        fb.loop_signal = LoopSignal::ReplanRequired;
        fb.replan_advised = true;
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::Replan {
                reason: ReplanReason::LoopGuardStagnationDetected,
            }
        );
    }

    // ── Precedence 4: ForceNoTools ────────────────────────────────────────────

    #[test]
    fn force_no_tools_beats_continue() {
        let mut fb = base_feedback();
        fb.loop_signal = LoopSignal::ForceNoTools;
        assert_eq!(TerminationOracle::adjudicate(&fb), TerminationDecision::ForceNoTools);
    }

    // ── Precedence 5: Continue (default) ─────────────────────────────────────

    #[test]
    fn no_authority_fires_produces_continue() {
        let fb = base_feedback();
        assert_eq!(TerminationOracle::adjudicate(&fb), TerminationDecision::Continue);
    }

    // ── All reason variants correctly assigned ────────────────────────────────

    #[test]
    fn all_synthesis_reason_variants_reachable() {
        let reasons = [
            SynthesisReason::ConvergenceControllerSynthesizeAction,
            SynthesisReason::LoopGuardInjectSynthesis,
            SynthesisReason::RoundScorerConsecutiveRegression,
            SynthesisReason::MidLoopCriticForceSynthesis,
        ];
        assert_eq!(reasons.len(), 4);
    }

    #[test]
    fn all_replan_reason_variants_reachable() {
        let reasons = [
            ReplanReason::ConvergenceControllerReplanAction,
            ReplanReason::LoopGuardStagnationDetected,
            ReplanReason::RoundScorerLowTrajectory,
            ReplanReason::MidLoopCriticReplan,
        ];
        assert_eq!(reasons.len(), 4);
    }

    // ── Precedence 3c: Mid-Loop Critic ────────────────────────────────────────

    #[test]
    fn mid_critic_force_synthesis_produces_inject_synthesis() {
        use super::super::mid_loop_critic::CriticAction;
        let mut fb = base_feedback();
        fb.mid_critic_action = Some(CriticAction::ForceSynthesis);
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::InjectSynthesis {
                reason: SynthesisReason::MidLoopCriticForceSynthesis,
            }
        );
    }

    #[test]
    fn mid_critic_force_synthesis_blocked_by_governance_rescue() {
        use super::super::mid_loop_critic::CriticAction;
        let mut fb = base_feedback();
        fb.mid_critic_action = Some(CriticAction::ForceSynthesis);
        fb.governance_rescue_active = true;
        // Governance rescue active → ForceSynthesis skipped, falls through to Continue
        assert_eq!(TerminationOracle::adjudicate(&fb), TerminationDecision::Continue);
    }

    #[test]
    fn mid_critic_replan_produces_replan() {
        use super::super::mid_loop_critic::CriticAction;
        let mut fb = base_feedback();
        fb.mid_critic_action = Some(CriticAction::Replan);
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::Replan {
                reason: ReplanReason::MidLoopCriticReplan,
            }
        );
    }

    #[test]
    fn mid_critic_reduce_scope_produces_replan() {
        use super::super::mid_loop_critic::CriticAction;
        let mut fb = base_feedback();
        fb.mid_critic_action = Some(CriticAction::ReduceScope);
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::Replan {
                reason: ReplanReason::MidLoopCriticReplan,
            }
        );
    }

    #[test]
    fn mid_critic_change_strategy_produces_replan() {
        use super::super::mid_loop_critic::CriticAction;
        let mut fb = base_feedback();
        fb.mid_critic_action = Some(CriticAction::ChangeStrategy);
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::Replan {
                reason: ReplanReason::MidLoopCriticReplan,
            }
        );
    }

    #[test]
    fn mid_critic_continue_does_not_interrupt() {
        use super::super::mid_loop_critic::CriticAction;
        let mut fb = base_feedback();
        fb.mid_critic_action = Some(CriticAction::Continue);
        assert_eq!(TerminationOracle::adjudicate(&fb), TerminationDecision::Continue);
    }

    #[test]
    fn halt_beats_mid_critic_force_synthesis() {
        use super::super::mid_loop_critic::CriticAction;
        let mut fb = base_feedback();
        fb.mid_critic_action = Some(CriticAction::ForceSynthesis);
        fb.convergence_action = ConvergenceAction::Halt;
        assert_eq!(TerminationOracle::adjudicate(&fb), TerminationDecision::Halt);
    }

    #[test]
    fn convergence_synthesize_beats_mid_critic_replan() {
        use super::super::mid_loop_critic::CriticAction;
        let mut fb = base_feedback();
        fb.mid_critic_action = Some(CriticAction::Replan);
        fb.convergence_action = ConvergenceAction::Synthesize;
        // InjectSynthesis (P2) beats MidCriticReplan (P3c)
        assert!(matches!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::InjectSynthesis { .. }
        ));
    }

    // ── Phase 6: Evidence-coverage-based synthesis delay ─────────────────────

    #[test]
    fn low_evidence_coverage_delays_synthesis_advised() {
        let mut fb = base_feedback();
        fb.synthesis_advised = true;
        fb.evidence_coverage = 0.10; // well below 0.30 threshold
        fb.round = 3; // early round
        // Low coverage + early round → delay synthesis, fall through to Continue
        assert_eq!(TerminationOracle::adjudicate(&fb), TerminationDecision::Continue);
    }

    #[test]
    fn high_evidence_coverage_does_not_delay_synthesis_advised() {
        let mut fb = base_feedback();
        fb.synthesis_advised = true;
        fb.evidence_coverage = 0.80; // well above threshold
        fb.utility_score = 0.20; // below utility threshold → synthesis proceeds
        fb.round = 3;
        // Low utility + high coverage → synthesis proceeds normally
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::InjectSynthesis {
                reason: SynthesisReason::RoundScorerConsecutiveRegression,
            }
        );
    }

    #[test]
    fn late_round_overrides_low_evidence_coverage_delay() {
        let mut fb = base_feedback();
        fb.synthesis_advised = true;
        fb.evidence_coverage = 0.10; // low coverage
        fb.utility_score = 0.20; // below utility threshold
        fb.round = 10; // late round (>= 8)
        // Low utility + late round → don't delay further, synthesize
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::InjectSynthesis {
                reason: SynthesisReason::RoundScorerConsecutiveRegression,
            }
        );
    }

    #[test]
    fn convergence_synthesize_not_delayed_by_low_coverage() {
        let mut fb = base_feedback();
        fb.convergence_action = ConvergenceAction::Synthesize;
        fb.evidence_coverage = 0.05; // very low coverage
        fb.round = 2;
        // ConvergenceController::Synthesize is a strong signal — never delayed
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::InjectSynthesis {
                reason: SynthesisReason::ConvergenceControllerSynthesizeAction,
            }
        );
    }

    #[test]
    fn loop_guard_inject_not_delayed_by_low_coverage() {
        let mut fb = base_feedback();
        fb.loop_signal = LoopSignal::InjectSynthesis;
        fb.evidence_coverage = 0.05; // very low coverage
        fb.round = 2;
        // LoopGuard::InjectSynthesis is a strong signal — never delayed
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::InjectSynthesis {
                reason: SynthesisReason::LoopGuardInjectSynthesis,
            }
        );
    }

    #[test]
    fn low_coverage_delay_falls_through_to_replan_if_advised() {
        let mut fb = base_feedback();
        fb.synthesis_advised = true;
        fb.replan_advised = true;
        fb.evidence_coverage = 0.10; // low coverage delays synthesis_advised
        fb.round = 3;
        // synthesis_advised delayed → falls through to replan_advised (P3)
        assert_eq!(
            TerminationOracle::adjudicate(&fb),
            TerminationDecision::Replan {
                reason: ReplanReason::RoundScorerLowTrajectory,
            }
        );
    }

    // ── ARCH-SYNC-1: GovernanceRescue cannot be bypassed by oracle ───────────

    #[test]
    fn synthesis_gate_governance_rescue_blocks_convergence_synthesize() {
        let mut fb = base_feedback();
        // ConvergenceController says Synthesize — strongest synthesis signal
        fb.convergence_action = ConvergenceAction::Synthesize;
        // GovernanceRescue conditions: reflection_score < 0.15 AND rounds < 3
        fb.governance_rescue_active = true;
        // Oracle MUST downgrade to Continue (not InjectSynthesis)
        assert_eq!(
            TerminationDecision::Continue,
            TerminationOracle::adjudicate(&fb),
            "GovernanceRescue must block ConvergenceController::Synthesize"
        );
    }

    #[test]
    fn synthesis_gate_governance_rescue_blocks_loop_guard_inject_synthesis() {
        let mut fb = base_feedback();
        fb.loop_signal = LoopSignal::InjectSynthesis;
        fb.governance_rescue_active = true;
        assert_eq!(
            TerminationDecision::Continue,
            TerminationOracle::adjudicate(&fb),
            "GovernanceRescue must block LoopGuard::InjectSynthesis"
        );
    }

    #[test]
    fn synthesis_gate_governance_rescue_blocks_mini_critic_synthesis() {
        let mut fb = base_feedback();
        fb.mini_critic_synthesis = true;
        fb.governance_rescue_active = true;
        assert_eq!(
            TerminationDecision::Continue,
            TerminationOracle::adjudicate(&fb),
            "GovernanceRescue must block mini_critic_synthesis"
        );
    }

    #[test]
    fn synthesis_gate_governance_rescue_does_not_block_halt() {
        let mut fb = base_feedback();
        // Halt always fires — GovernanceRescue only blocks synthesis
        fb.convergence_action = ConvergenceAction::Halt;
        fb.governance_rescue_active = true;
        assert_eq!(
            TerminationDecision::Halt,
            TerminationOracle::adjudicate(&fb),
            "GovernanceRescue must NOT block Halt"
        );
    }

    #[test]
    fn governance_rescue_inactive_allows_synthesize() {
        let mut fb = base_feedback();
        fb.convergence_action = ConvergenceAction::Synthesize;
        // governance_rescue_active = false (default) → synthesis proceeds normally
        assert_eq!(
            TerminationDecision::InjectSynthesis {
                reason: SynthesisReason::ConvergenceControllerSynthesizeAction,
            },
            TerminationOracle::adjudicate(&fb),
            "With governance_rescue_active=false, Synthesize must be honored"
        );
    }
}
