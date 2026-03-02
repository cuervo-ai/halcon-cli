//! Signal Conflict Resolution — Deterministic arbitration of contradictory signals (P4.4).
//!
//! When multiple decision systems produce conflicting recommendations, the
//! arbitrator resolves them using a formal priority hierarchy. This makes
//! conflict resolution explicit, testable, and traceable.
//!
//! # Signal Sources (by authority)
//!
//! | Priority | Source | Nature |
//! |----------|--------|--------|
//! | 1 (highest) | EBS (Evidence Gate) | Hard constraint — blocks synthesis |
//! | 2 | TerminationOracle | Authoritative loop control |
//! | 3 | MidLoopStrategy (P3.1) | Structural replan guidance |
//! | 4 | MidLoopCritic (P3.4) | Progress-aware intervention |
//! | 5 | AdaptivePolicy | Runtime parameter shift |
//! | 6 (lowest) | ConvergenceUtility (P3.6) | Advisory synthesis timing |
//!
//! # Conflict Patterns
//!
//! - **Replan vs Synthesis**: Oracle says synthesize, strategy says replan → Oracle wins
//! - **Continue vs ForceSynthesis**: Utility says continue, critic says synthesize → Critic wins (higher authority)
//! - **EBS vs Everything**: EBS gate blocks synthesis regardless of all other signals
//!
//! Pure business logic — no I/O.

use super::mid_loop_critic::CriticAction;
use super::mid_loop_strategy::StrategyMutation;
use super::termination_oracle::TerminationDecision;

// ── ConflictType ─────────────────────────────────────────────────────────────

/// Identifies a detected conflict between signals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictType {
    /// Oracle says continue but critic wants intervention.
    OracleVsCritic,
    /// Oracle says replan but strategy wants different mutation.
    OracleVsStrategy,
    /// Utility says productive but critic says declining.
    UtilityVsCritic,
    /// Strategy wants replan but EBS would block subsequent synthesis.
    StrategyVsEbs,
    /// Multiple synthesis sources fire simultaneously.
    MultipleSynthesisSources,
    /// No conflict detected.
    None,
}

// ── SignalBundle ──────────────────────────────────────────────────────────────

/// Aggregated signals from all decision systems for one round.
#[derive(Debug, Clone)]
pub struct SignalBundle {
    /// TerminationOracle decision.
    pub oracle: TerminationDecision,
    /// MidLoopStrategy mutation recommendation (if replan was triggered).
    pub strategy: Option<StrategyMutation>,
    /// MidLoopCritic action recommendation (if checkpoint fired).
    pub critic: Option<CriticAction>,
    /// Whether EBS evidence gate would fire (hard constraint).
    pub ebs_gate_active: bool,
    /// Utility score from P3.6.
    pub utility_score: f64,
    /// Utility synthesis threshold.
    pub utility_threshold: f64,
}

// ── ArbitrationResult ────────────────────────────────────────────────────────

/// Result of signal arbitration.
#[derive(Debug, Clone)]
pub struct ArbitrationResult {
    /// The resolved action to take.
    pub action: ResolvedAction,
    /// Any conflict that was detected and resolved.
    pub conflict: ConflictType,
    /// Explanation of the resolution.
    pub rationale: &'static str,
}

/// The final resolved action after arbitration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedAction {
    /// Continue to next round.
    Continue,
    /// Suppress tools next round.
    ForceNoTools,
    /// Synthesize output.
    Synthesize,
    /// Replan with optional strategy mutation.
    Replan { strategy: Option<StrategyMutation> },
    /// Hard stop.
    Halt,
    /// EBS intercept — block synthesis, emit limitation notice.
    EbsIntercept,
}

// ── SignalArbitrator ──────────────────────────────────────────────────────────

/// Stateless arbitrator that resolves signal conflicts deterministically.
pub struct SignalArbitrator;

impl SignalArbitrator {
    /// Resolve all signals into a single action.
    ///
    /// Priority: EBS > Oracle > Critic > Strategy > Utility > Continue
    pub fn arbitrate(bundle: &SignalBundle) -> ArbitrationResult {
        // Priority 1: EBS hard constraint
        // If EBS gate is active AND the oracle wants synthesis, block it.
        if bundle.ebs_gate_active {
            if is_synthesis_decision(&bundle.oracle) {
                return ArbitrationResult {
                    action: ResolvedAction::EbsIntercept,
                    conflict: ConflictType::StrategyVsEbs,
                    rationale: "EBS gate blocks synthesis — insufficient evidence",
                };
            }
            // EBS active but oracle isn't synthesizing → EBS doesn't interfere.
        }

        // Priority 2: Oracle authoritative decision
        match &bundle.oracle {
            TerminationDecision::Halt => {
                return ArbitrationResult {
                    action: ResolvedAction::Halt,
                    conflict: detect_conflict(bundle),
                    rationale: "Oracle halt — highest authority",
                };
            }
            TerminationDecision::InjectSynthesis { .. } => {
                // Check for conflict with critic wanting replan
                let conflict = if matches!(bundle.critic, Some(CriticAction::Replan)) {
                    ConflictType::OracleVsCritic
                } else {
                    ConflictType::None
                };
                return ArbitrationResult {
                    action: ResolvedAction::Synthesize,
                    conflict,
                    rationale: "Oracle synthesis — authoritative",
                };
            }
            TerminationDecision::Replan { .. } => {
                // Merge with strategy mutation if available
                let conflict = if matches!(bundle.strategy, Some(StrategyMutation::ForceSynthesis)) {
                    ConflictType::OracleVsStrategy
                } else {
                    ConflictType::None
                };
                return ArbitrationResult {
                    action: ResolvedAction::Replan {
                        strategy: bundle.strategy.clone(),
                    },
                    conflict,
                    rationale: "Oracle replan — with strategy mutation",
                };
            }
            TerminationDecision::ForceNoTools => {
                return ArbitrationResult {
                    action: ResolvedAction::ForceNoTools,
                    conflict: ConflictType::None,
                    rationale: "Oracle force-no-tools",
                };
            }
            TerminationDecision::Continue => {
                // Fall through to lower-priority signals.
            }
        }

        // Priority 3: Critic checkpoint action
        if let Some(critic_action) = &bundle.critic {
            match critic_action {
                CriticAction::ForceSynthesis => {
                    let conflict = if bundle.utility_score > bundle.utility_threshold {
                        ConflictType::UtilityVsCritic
                    } else {
                        ConflictType::None
                    };
                    return ArbitrationResult {
                        action: ResolvedAction::Synthesize,
                        conflict,
                        rationale: "Critic force-synthesis overrides utility",
                    };
                }
                CriticAction::Replan => {
                    return ArbitrationResult {
                        action: ResolvedAction::Replan { strategy: bundle.strategy.clone() },
                        conflict: ConflictType::None,
                        rationale: "Critic replan",
                    };
                }
                CriticAction::ChangeStrategy | CriticAction::ReduceScope => {
                    // These are advisory — don't override oracle Continue
                }
                CriticAction::Continue => {}
            }
        }

        // Priority 4: Utility-based synthesis
        if bundle.utility_score < bundle.utility_threshold {
            return ArbitrationResult {
                action: ResolvedAction::Synthesize,
                conflict: ConflictType::None,
                rationale: "Utility below threshold — synthesize",
            };
        }

        // Default: Continue
        ArbitrationResult {
            action: ResolvedAction::Continue,
            conflict: ConflictType::None,
            rationale: "No authority fired — continue",
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn is_synthesis_decision(decision: &TerminationDecision) -> bool {
    matches!(decision, TerminationDecision::InjectSynthesis { .. })
}

fn detect_conflict(bundle: &SignalBundle) -> ConflictType {
    if matches!(bundle.critic, Some(CriticAction::ForceSynthesis))
        && matches!(bundle.oracle, TerminationDecision::Halt)
    {
        return ConflictType::OracleVsCritic;
    }
    if matches!(bundle.strategy, Some(StrategyMutation::ForceSynthesis))
        && matches!(bundle.oracle, TerminationDecision::Halt)
    {
        return ConflictType::OracleVsStrategy;
    }
    ConflictType::None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::termination_oracle::{SynthesisReason, ReplanReason};

    fn base_bundle() -> SignalBundle {
        SignalBundle {
            oracle: TerminationDecision::Continue,
            strategy: None,
            critic: None,
            ebs_gate_active: false,
            utility_score: 0.5,
            utility_threshold: 0.35,
        }
    }

    // ── EBS Priority ─────────────────────────────────────────────────────

    #[test]
    fn phase4_arb_ebs_blocks_synthesis() {
        let mut bundle = base_bundle();
        bundle.oracle = TerminationDecision::InjectSynthesis {
            reason: SynthesisReason::ConvergenceControllerSynthesizeAction,
        };
        bundle.ebs_gate_active = true;
        let result = SignalArbitrator::arbitrate(&bundle);
        assert_eq!(result.action, ResolvedAction::EbsIntercept);
        assert_eq!(result.conflict, ConflictType::StrategyVsEbs);
    }

    #[test]
    fn phase4_arb_ebs_does_not_block_continue() {
        let mut bundle = base_bundle();
        bundle.ebs_gate_active = true;
        // Oracle is Continue, EBS active but irrelevant
        let result = SignalArbitrator::arbitrate(&bundle);
        assert_eq!(result.action, ResolvedAction::Continue);
    }

    // ── Oracle Priority ──────────────────────────────────────────────────

    #[test]
    fn phase4_arb_oracle_halt_wins() {
        let mut bundle = base_bundle();
        bundle.oracle = TerminationDecision::Halt;
        bundle.critic = Some(CriticAction::ForceSynthesis);
        let result = SignalArbitrator::arbitrate(&bundle);
        assert_eq!(result.action, ResolvedAction::Halt);
    }

    #[test]
    fn phase4_arb_oracle_synthesis_beats_critic_replan() {
        let mut bundle = base_bundle();
        bundle.oracle = TerminationDecision::InjectSynthesis {
            reason: SynthesisReason::LoopGuardInjectSynthesis,
        };
        bundle.critic = Some(CriticAction::Replan);
        let result = SignalArbitrator::arbitrate(&bundle);
        assert_eq!(result.action, ResolvedAction::Synthesize);
        assert_eq!(result.conflict, ConflictType::OracleVsCritic);
    }

    #[test]
    fn phase4_arb_oracle_replan_merges_strategy() {
        let mut bundle = base_bundle();
        bundle.oracle = TerminationDecision::Replan {
            reason: ReplanReason::ConvergenceControllerReplanAction,
        };
        bundle.strategy = Some(StrategyMutation::ReplanWithDecomposition { failing_step_idx: 2 });
        let result = SignalArbitrator::arbitrate(&bundle);
        assert!(matches!(result.action, ResolvedAction::Replan { strategy: Some(StrategyMutation::ReplanWithDecomposition { .. }) }));
    }

    #[test]
    fn phase4_arb_oracle_replan_conflicts_with_force_synthesis_strategy() {
        let mut bundle = base_bundle();
        bundle.oracle = TerminationDecision::Replan {
            reason: ReplanReason::RoundScorerLowTrajectory,
        };
        bundle.strategy = Some(StrategyMutation::ForceSynthesis);
        let result = SignalArbitrator::arbitrate(&bundle);
        // Oracle replan wins over strategy's ForceSynthesis
        assert!(matches!(result.action, ResolvedAction::Replan { .. }));
        assert_eq!(result.conflict, ConflictType::OracleVsStrategy);
    }

    #[test]
    fn phase4_arb_oracle_force_no_tools() {
        let mut bundle = base_bundle();
        bundle.oracle = TerminationDecision::ForceNoTools;
        let result = SignalArbitrator::arbitrate(&bundle);
        assert_eq!(result.action, ResolvedAction::ForceNoTools);
    }

    // ── Critic Priority (oracle=Continue) ────────────────────────────────

    #[test]
    fn phase4_arb_critic_force_synthesis_overrides_utility() {
        let mut bundle = base_bundle();
        bundle.critic = Some(CriticAction::ForceSynthesis);
        bundle.utility_score = 0.8; // utility says productive
        bundle.utility_threshold = 0.35;
        let result = SignalArbitrator::arbitrate(&bundle);
        assert_eq!(result.action, ResolvedAction::Synthesize);
        assert_eq!(result.conflict, ConflictType::UtilityVsCritic);
    }

    #[test]
    fn phase4_arb_critic_replan() {
        let mut bundle = base_bundle();
        bundle.critic = Some(CriticAction::Replan);
        let result = SignalArbitrator::arbitrate(&bundle);
        assert!(matches!(result.action, ResolvedAction::Replan { .. }));
    }

    #[test]
    fn phase4_arb_critic_change_strategy_is_advisory() {
        let mut bundle = base_bundle();
        bundle.critic = Some(CriticAction::ChangeStrategy);
        // ChangeStrategy is advisory, should not override Continue
        let result = SignalArbitrator::arbitrate(&bundle);
        assert_eq!(result.action, ResolvedAction::Continue);
    }

    // ── Utility Priority ─────────────────────────────────────────────────

    #[test]
    fn phase4_arb_utility_below_threshold_synthesizes() {
        let mut bundle = base_bundle();
        bundle.utility_score = 0.20;
        bundle.utility_threshold = 0.35;
        let result = SignalArbitrator::arbitrate(&bundle);
        assert_eq!(result.action, ResolvedAction::Synthesize);
    }

    #[test]
    fn phase4_arb_utility_above_threshold_continues() {
        let mut bundle = base_bundle();
        bundle.utility_score = 0.60;
        bundle.utility_threshold = 0.35;
        let result = SignalArbitrator::arbitrate(&bundle);
        assert_eq!(result.action, ResolvedAction::Continue);
    }

    // ── Default ──────────────────────────────────────────────────────────

    #[test]
    fn phase4_arb_no_signals_continues() {
        let bundle = base_bundle();
        let result = SignalArbitrator::arbitrate(&bundle);
        assert_eq!(result.action, ResolvedAction::Continue);
        assert_eq!(result.conflict, ConflictType::None);
    }

    // ── Conflict detection ───────────────────────────────────────────────

    #[test]
    fn phase4_arb_conflict_types_distinct() {
        let types = [
            ConflictType::OracleVsCritic,
            ConflictType::OracleVsStrategy,
            ConflictType::UtilityVsCritic,
            ConflictType::StrategyVsEbs,
            ConflictType::MultipleSynthesisSources,
            ConflictType::None,
        ];
        for (i, a) in types.iter().enumerate() {
            for (j, b) in types.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "conflict types must be distinct");
                }
            }
        }
    }
}
