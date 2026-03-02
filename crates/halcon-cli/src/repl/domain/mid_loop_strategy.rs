//! Mid-loop structural strategy mutation — decides HOW to replan.
//!
//! The existing `TerminationOracle` decides WHETHER to replan (Sprint 2 L6).
//! P3.1 decides HOW to replan. It sits between the oracle's `Replan` decision
//! and the actual replan execution in `convergence_phase.rs`.
//!
//! Pure business logic — no I/O.

use crate::repl::agent::loop_state::ExecutionIntentPhase;

/// What kind of structural mutation to apply to the replan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrategyMutation {
    /// Skip replan — ambiguous signals, not worth a replan attempt.
    ContinueCurrentPlan,
    /// Fresh approach, same step count.
    ReplanSameGranularity,
    /// Break a coarse failing step into sub-steps.
    ReplanWithDecomposition { failing_step_idx: usize },
    /// Simplify remaining plan to a single broad step.
    CollapsePlan,
    /// Stop writing, start reading.
    SwitchInvestigationMode,
    /// Stop reading, start acting.
    SwitchExecutionMode,
    /// Skip replan entirely, synthesize now.
    ForceSynthesis,
}

/// Rationale for the selected mutation.
#[derive(Debug, Clone)]
pub struct MutationRationale {
    pub mutation: StrategyMutation,
    pub primary_signal: &'static str,
    pub confidence: f32,
}

/// Signals gathered from LoopState sub-structs for strategy selection.
#[derive(Debug, Clone)]
pub struct StrategySignals {
    pub evidence_coverage: f64,
    pub drift_score: f32,
    pub replan_attempts: u32,
    pub max_replan_attempts: u32,
    pub consecutive_errors: u32,
    pub tool_failure_clustering: f32,
    pub sla_fraction_consumed: f64,
    pub execution_intent: ExecutionIntentPhase,
    pub plan_completion_fraction: f32,
    pub cycle_detected: bool,
    pub round: usize,
    pub max_rounds: usize,
}

/// Policy thresholds for strategy selection.
#[derive(Debug, Clone)]
pub struct StrategyThresholds {
    pub force_synthesis_sla: f64,
    pub min_evidence_for_synthesis: f64,
    pub collapse_min_progress: f32,
    pub drift_threshold: f32,
    pub failure_cluster_threshold: f32,
}

impl StrategyThresholds {
    /// Build from PolicyConfig.
    pub fn from_policy(policy: &halcon_core::types::PolicyConfig) -> Self {
        Self {
            force_synthesis_sla: policy.strategy_force_synthesis_sla,
            min_evidence_for_synthesis: policy.strategy_min_evidence_for_synthesis,
            collapse_min_progress: policy.strategy_collapse_min_progress,
            drift_threshold: policy.strategy_drift_threshold,
            failure_cluster_threshold: policy.strategy_failure_cluster_threshold,
        }
    }
}

impl Default for StrategyThresholds {
    fn default() -> Self {
        Self {
            force_synthesis_sla: 0.85,
            min_evidence_for_synthesis: 0.30,
            collapse_min_progress: 0.50,
            drift_threshold: 0.50,
            failure_cluster_threshold: 0.50,
        }
    }
}

/// Select the best strategy mutation based on current signals.
///
/// Priority cascade (highest to lowest):
/// 1. ForceSynthesis — budget almost exhausted + some evidence
/// 2. CollapsePlan — near replan limit + good progress
/// 3. SwitchInvestigationMode — executing but errors + low evidence
/// 4. SwitchExecutionMode — investigating with good evidence + high drift
/// 5. ReplanWithDecomposition — clustered failures early in plan
/// 6. ReplanSameGranularity — default when no cycle detected
/// 7. ContinueCurrentPlan — fallback
pub fn select_mutation(
    signals: &StrategySignals,
    thresholds: &StrategyThresholds,
) -> MutationRationale {
    // 1. ForceSynthesis: budget almost gone + minimum evidence
    if signals.sla_fraction_consumed > thresholds.force_synthesis_sla
        && signals.evidence_coverage > thresholds.min_evidence_for_synthesis
    {
        return MutationRationale {
            mutation: StrategyMutation::ForceSynthesis,
            primary_signal: "SLA budget near exhaustion with sufficient evidence",
            confidence: 0.90,
        };
    }

    // 2. CollapsePlan: near replan limit + good progress
    if signals.replan_attempts >= signals.max_replan_attempts.saturating_sub(1)
        && signals.plan_completion_fraction > thresholds.collapse_min_progress
    {
        return MutationRationale {
            mutation: StrategyMutation::CollapsePlan,
            primary_signal: "replan attempts near limit with good progress",
            confidence: 0.80,
        };
    }

    // 3. SwitchInvestigationMode: executing but errors + low evidence
    if matches!(signals.execution_intent, ExecutionIntentPhase::Execution)
        && signals.consecutive_errors >= 3
        && signals.evidence_coverage < 0.20
    {
        return MutationRationale {
            mutation: StrategyMutation::SwitchInvestigationMode,
            primary_signal: "execution failing with insufficient evidence",
            confidence: 0.75,
        };
    }

    // 4. SwitchExecutionMode: investigating with good evidence + high drift
    if matches!(signals.execution_intent, ExecutionIntentPhase::Investigation)
        && signals.evidence_coverage > 0.60
        && signals.drift_score > thresholds.drift_threshold
    {
        return MutationRationale {
            mutation: StrategyMutation::SwitchExecutionMode,
            primary_signal: "investigation complete, high drift suggests action needed",
            confidence: 0.70,
        };
    }

    // 5. ReplanWithDecomposition: clustered failures early in plan
    if signals.tool_failure_clustering > thresholds.failure_cluster_threshold
        && signals.plan_completion_fraction < 0.30
    {
        return MutationRationale {
            mutation: StrategyMutation::ReplanWithDecomposition { failing_step_idx: 0 },
            primary_signal: "clustered tool failures in early plan steps",
            confidence: 0.65,
        };
    }

    // 6. ReplanSameGranularity: default when no cycle
    if !signals.cycle_detected {
        return MutationRationale {
            mutation: StrategyMutation::ReplanSameGranularity,
            primary_signal: "standard replan (no cycle detected)",
            confidence: 0.50,
        };
    }

    // 7. ContinueCurrentPlan: fallback (cycle detected but no clear mutation)
    MutationRationale {
        mutation: StrategyMutation::ContinueCurrentPlan,
        primary_signal: "cycle detected but no clear alternative",
        confidence: 0.30,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_signals() -> StrategySignals {
        StrategySignals {
            evidence_coverage: 0.50,
            drift_score: 0.20,
            replan_attempts: 0,
            max_replan_attempts: 2,
            consecutive_errors: 0,
            tool_failure_clustering: 0.0,
            sla_fraction_consumed: 0.40,
            execution_intent: ExecutionIntentPhase::Execution,
            plan_completion_fraction: 0.30,
            cycle_detected: false,
            round: 3,
            max_rounds: 10,
        }
    }

    #[test]
    fn phase3_strategy_force_synthesis_budget_exhausted() {
        let signals = StrategySignals {
            sla_fraction_consumed: 0.90,
            evidence_coverage: 0.40,
            ..base_signals()
        };
        let r = select_mutation(&signals, &StrategyThresholds::default());
        assert_eq!(r.mutation, StrategyMutation::ForceSynthesis);
        assert!(r.confidence > 0.85);
    }

    #[test]
    fn phase3_strategy_no_force_synthesis_low_evidence() {
        let signals = StrategySignals {
            sla_fraction_consumed: 0.90,
            evidence_coverage: 0.10, // below 0.30
            ..base_signals()
        };
        let r = select_mutation(&signals, &StrategyThresholds::default());
        assert_ne!(r.mutation, StrategyMutation::ForceSynthesis);
    }

    #[test]
    fn phase3_strategy_collapse_plan_near_limit() {
        let signals = StrategySignals {
            replan_attempts: 1, // max-1 = 1
            plan_completion_fraction: 0.60,
            ..base_signals()
        };
        let r = select_mutation(&signals, &StrategyThresholds::default());
        assert_eq!(r.mutation, StrategyMutation::CollapsePlan);
    }

    #[test]
    fn phase3_strategy_switch_investigation() {
        let signals = StrategySignals {
            execution_intent: ExecutionIntentPhase::Execution,
            consecutive_errors: 4,
            evidence_coverage: 0.10,
            ..base_signals()
        };
        let r = select_mutation(&signals, &StrategyThresholds::default());
        assert_eq!(r.mutation, StrategyMutation::SwitchInvestigationMode);
    }

    #[test]
    fn phase3_strategy_switch_execution() {
        let signals = StrategySignals {
            execution_intent: ExecutionIntentPhase::Investigation,
            evidence_coverage: 0.70,
            drift_score: 0.60,
            ..base_signals()
        };
        let r = select_mutation(&signals, &StrategyThresholds::default());
        assert_eq!(r.mutation, StrategyMutation::SwitchExecutionMode);
    }

    #[test]
    fn phase3_strategy_decomposition_clustered_failures() {
        let signals = StrategySignals {
            tool_failure_clustering: 0.60,
            plan_completion_fraction: 0.10,
            ..base_signals()
        };
        let r = select_mutation(&signals, &StrategyThresholds::default());
        assert!(matches!(r.mutation, StrategyMutation::ReplanWithDecomposition { .. }));
    }

    #[test]
    fn phase3_strategy_same_granularity_default() {
        let signals = base_signals();
        let r = select_mutation(&signals, &StrategyThresholds::default());
        assert_eq!(r.mutation, StrategyMutation::ReplanSameGranularity);
    }

    #[test]
    fn phase3_strategy_continue_on_cycle() {
        let signals = StrategySignals {
            cycle_detected: true,
            ..base_signals()
        };
        let r = select_mutation(&signals, &StrategyThresholds::default());
        assert_eq!(r.mutation, StrategyMutation::ContinueCurrentPlan);
    }

    #[test]
    fn phase3_strategy_priority_synthesis_over_collapse() {
        // Both conditions met: SLA > 0.85 AND replan near limit
        let signals = StrategySignals {
            sla_fraction_consumed: 0.90,
            evidence_coverage: 0.40,
            replan_attempts: 1,
            plan_completion_fraction: 0.60,
            ..base_signals()
        };
        let r = select_mutation(&signals, &StrategyThresholds::default());
        // ForceSynthesis has higher priority
        assert_eq!(r.mutation, StrategyMutation::ForceSynthesis);
    }

    #[test]
    fn phase3_strategy_priority_collapse_over_investigation() {
        let signals = StrategySignals {
            replan_attempts: 1,
            plan_completion_fraction: 0.60,
            consecutive_errors: 4,
            evidence_coverage: 0.10,
            ..base_signals()
        };
        let r = select_mutation(&signals, &StrategyThresholds::default());
        // CollapsePlan has higher priority than SwitchInvestigation
        assert_eq!(r.mutation, StrategyMutation::CollapsePlan);
    }

    #[test]
    fn phase3_strategy_custom_thresholds() {
        let thresholds = StrategyThresholds {
            force_synthesis_sla: 0.50, // more aggressive
            min_evidence_for_synthesis: 0.10,
            ..StrategyThresholds::default()
        };
        let signals = StrategySignals {
            sla_fraction_consumed: 0.55,
            evidence_coverage: 0.15,
            ..base_signals()
        };
        let r = select_mutation(&signals, &thresholds);
        assert_eq!(r.mutation, StrategyMutation::ForceSynthesis);
    }

    #[test]
    fn phase3_strategy_from_policy() {
        let policy = halcon_core::types::PolicyConfig::default();
        let t = StrategyThresholds::from_policy(&policy);
        assert!((t.force_synthesis_sla - 0.85).abs() < f64::EPSILON);
        assert!((t.min_evidence_for_synthesis - 0.30).abs() < f64::EPSILON);
    }

    #[test]
    fn phase3_strategy_decomposition_not_late_plan() {
        // Clustered failures but past 30% completion → should NOT decompose
        let signals = StrategySignals {
            tool_failure_clustering: 0.60,
            plan_completion_fraction: 0.50, // >0.30
            ..base_signals()
        };
        let r = select_mutation(&signals, &StrategyThresholds::default());
        assert!(!matches!(r.mutation, StrategyMutation::ReplanWithDecomposition { .. }));
    }

    #[test]
    fn phase3_strategy_confidence_ordering() {
        // ForceSynthesis should have highest confidence
        let fs = MutationRationale {
            mutation: StrategyMutation::ForceSynthesis,
            primary_signal: "",
            confidence: 0.90,
        };
        let cp = MutationRationale {
            mutation: StrategyMutation::ContinueCurrentPlan,
            primary_signal: "",
            confidence: 0.30,
        };
        assert!(fs.confidence > cp.confidence);
    }
}
