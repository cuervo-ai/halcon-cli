//! Post-execution evaluation for adaptive learning.
//!
//! Evaluates agent loop outcomes using multiple factors:
//! - Stop condition quality (EndTurn=best, ProviderError=worst)
//! - Efficiency (round utilization)
//! - Completion (did it produce output?)

use super::super::agent_types::StopCondition;

/// Result of agent loop execution (simplified view for evaluation).
#[derive(Debug, Clone)]
pub struct AgentLoopOutcome {
    pub stop_condition: StopCondition,
    pub rounds_used: usize,
    pub max_rounds: usize,
    pub has_output: bool,
}

/// Evaluates stop condition quality.
struct StopConditionEvaluator;

impl StopConditionEvaluator {
    const WEIGHT: f64 = 0.5;

    fn evaluate(condition: &StopCondition) -> f64 {
        match condition {
            StopCondition::EndTurn => 1.0,             // Perfect: model chose to stop
            StopCondition::ForcedSynthesis => 0.7,    // Good: loop guard intervened
            StopCondition::Interrupted => 0.5,         // Neutral: user cancelled
            StopCondition::MaxRounds => 0.4,           // Suboptimal: hit round limit
            StopCondition::TokenBudget => 0.3,         // Suboptimal: hit token budget
            StopCondition::DurationBudget => 0.3,      // Suboptimal: hit time budget
            StopCondition::ProviderError => 0.0,       // Failure: provider error
            StopCondition::EnvironmentError => 0.0,    // Failure: MCP/env unavailable
            StopCondition::CostBudget => 0.3,          // Suboptimal: hit cost limit
            StopCondition::SupervisorDenied => 0.3,    // Governance gate: valid work, blocked write
        }
    }
}

/// Evaluates round efficiency.
struct EfficiencyEvaluator;

impl EfficiencyEvaluator {
    const WEIGHT: f64 = 0.2;

    fn evaluate(outcome: &AgentLoopOutcome) -> f64 {
        if outcome.max_rounds == 0 {
            return 0.0;
        }
        // Lower rounds = better (more efficient)
        1.0 - (outcome.rounds_used as f64 / outcome.max_rounds as f64)
    }
}

/// Evaluates task completion.
struct CompletionEvaluator;

impl CompletionEvaluator {
    const WEIGHT: f64 = 0.3;

    fn evaluate(outcome: &AgentLoopOutcome) -> f64 {
        if outcome.has_output {
            1.0
        } else {
            0.0
        }
    }
}

/// Composite evaluator combining multiple factors.
pub struct CompositeEvaluator;

impl CompositeEvaluator {
    /// Evaluate agent loop outcome and return score [0.0, 1.0].
    ///
    /// Formula: stop * 0.5 + efficiency * 0.2 + completion * 0.3
    pub fn evaluate(outcome: &AgentLoopOutcome) -> f64 {
        let stop_score = StopConditionEvaluator::evaluate(&outcome.stop_condition);
        let efficiency_score = EfficiencyEvaluator::evaluate(outcome);
        let completion_score = CompletionEvaluator::evaluate(outcome);

        stop_score * StopConditionEvaluator::WEIGHT
            + efficiency_score * EfficiencyEvaluator::WEIGHT
            + completion_score * CompletionEvaluator::WEIGHT
    }

    /// Check if outcome is considered successful.
    pub fn is_success(outcome: &AgentLoopOutcome, threshold: f64) -> bool {
        Self::evaluate(outcome) >= threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_condition_end_turn_perfect() {
        let score = StopConditionEvaluator::evaluate(&StopCondition::EndTurn);
        assert_eq!(score, 1.0);
    }

    #[test]
    fn stop_condition_forced_synthesis_good() {
        let score = StopConditionEvaluator::evaluate(&StopCondition::ForcedSynthesis);
        assert_eq!(score, 0.7);
    }

    #[test]
    fn stop_condition_max_rounds_suboptimal() {
        let score = StopConditionEvaluator::evaluate(&StopCondition::MaxRounds);
        assert_eq!(score, 0.4);
    }

    #[test]
    fn stop_condition_provider_error_failure() {
        let score = StopConditionEvaluator::evaluate(&StopCondition::ProviderError);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn stop_condition_interrupted_neutral() {
        let score = StopConditionEvaluator::evaluate(&StopCondition::Interrupted);
        assert_eq!(score, 0.5);
    }

    #[test]
    fn stop_condition_token_budget_suboptimal() {
        let score = StopConditionEvaluator::evaluate(&StopCondition::TokenBudget);
        assert_eq!(score, 0.3);
    }

    #[test]
    fn stop_condition_duration_budget_suboptimal() {
        let score = StopConditionEvaluator::evaluate(&StopCondition::DurationBudget);
        assert_eq!(score, 0.3);
    }

    #[test]
    fn efficiency_low_rounds_high_score() {
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::EndTurn,
            rounds_used: 2,
            max_rounds: 10,
            has_output: true,
        };
        let score = EfficiencyEvaluator::evaluate(&outcome);
        assert_eq!(score, 0.8); // 1.0 - (2/10) = 0.8
    }

    #[test]
    fn efficiency_max_rounds_low_score() {
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::MaxRounds,
            rounds_used: 10,
            max_rounds: 10,
            has_output: true,
        };
        let score = EfficiencyEvaluator::evaluate(&outcome);
        assert_eq!(score, 0.0); // 1.0 - (10/10) = 0.0
    }

    #[test]
    fn efficiency_zero_max_rounds() {
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::EndTurn,
            rounds_used: 0,
            max_rounds: 0,
            has_output: false,
        };
        let score = EfficiencyEvaluator::evaluate(&outcome);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn completion_has_output() {
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::EndTurn,
            rounds_used: 3,
            max_rounds: 10,
            has_output: true,
        };
        let score = CompletionEvaluator::evaluate(&outcome);
        assert_eq!(score, 1.0);
    }

    #[test]
    fn completion_no_output() {
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::ProviderError,
            rounds_used: 1,
            max_rounds: 10,
            has_output: false,
        };
        let score = CompletionEvaluator::evaluate(&outcome);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn composite_perfect_outcome() {
        // EndTurn (1.0) * 0.5 + low rounds (0.9) * 0.2 + has output (1.0) * 0.3
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::EndTurn,
            rounds_used: 1,
            max_rounds: 10,
            has_output: true,
        };
        let score = CompositeEvaluator::evaluate(&outcome);
        // 1.0*0.5 + 0.9*0.2 + 1.0*0.3 = 0.5 + 0.18 + 0.3 = 0.98
        assert!((score - 0.98).abs() < 0.01);
    }

    #[test]
    fn composite_provider_error_no_output() {
        // ProviderError (0.0) * 0.5 + any efficiency * 0.2 + no output (0.0) * 0.3
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::ProviderError,
            rounds_used: 1,
            max_rounds: 10,
            has_output: false,
        };
        let score = CompositeEvaluator::evaluate(&outcome);
        // 0.0*0.5 + 0.9*0.2 + 0.0*0.3 = 0.18
        assert!((score - 0.18).abs() < 0.01);
    }

    #[test]
    fn composite_max_rounds_with_output() {
        // MaxRounds (0.4) * 0.5 + low efficiency (0.0) * 0.2 + has output (1.0) * 0.3
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::MaxRounds,
            rounds_used: 10,
            max_rounds: 10,
            has_output: true,
        };
        let score = CompositeEvaluator::evaluate(&outcome);
        // 0.4*0.5 + 0.0*0.2 + 1.0*0.3 = 0.2 + 0.0 + 0.3 = 0.5
        assert_eq!(score, 0.5);
    }

    #[test]
    fn is_success_above_threshold() {
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::EndTurn,
            rounds_used: 3,
            max_rounds: 10,
            has_output: true,
        };
        assert!(CompositeEvaluator::is_success(&outcome, 0.6));
    }

    #[test]
    fn is_success_below_threshold() {
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::ProviderError,
            rounds_used: 1,
            max_rounds: 10,
            has_output: false,
        };
        assert!(!CompositeEvaluator::is_success(&outcome, 0.6));
    }

    #[test]
    fn is_success_at_threshold() {
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::MaxRounds,
            rounds_used: 10,
            max_rounds: 10,
            has_output: true,
        };
        // Score = 0.5 (from test above)
        assert!(!CompositeEvaluator::is_success(&outcome, 0.6));
        assert!(CompositeEvaluator::is_success(&outcome, 0.5));
    }

    // --- Coverage for stop conditions added in Phase 77 / 77b ---

    #[test]
    fn stop_condition_environment_error_scores_zero() {
        // EnvironmentError (MCP persistently dead) must score 0.0 — UCB1 must penalise
        // strategies that dispatch tools into a dead environment.
        let score = StopConditionEvaluator::evaluate(&StopCondition::EnvironmentError);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn stop_condition_cost_budget_scores_point_three() {
        // CostBudget must score 0.3 — suboptimal (agent ran out of money before converging)
        // but not a hard failure, matching TokenBudget / DurationBudget treatment.
        let score = StopConditionEvaluator::evaluate(&StopCondition::CostBudget);
        assert_eq!(score, 0.3);
    }

    #[test]
    fn composite_environment_error_never_success() {
        // EnvironmentError (0.0) * 0.5 + efficiency * 0.2 + no output (0.0) * 0.3
        // = 0.0*0.5 + 0.7*0.2 + 0.0*0.3 = 0.14 — well below any success threshold.
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::EnvironmentError,
            rounds_used: 3,
            max_rounds: 10,
            has_output: false,
        };
        let score = CompositeEvaluator::evaluate(&outcome);
        assert!((score - 0.14).abs() < 0.01);
        assert!(!CompositeEvaluator::is_success(&outcome, 0.6));
    }

    #[test]
    fn composite_cost_budget_with_output_below_success_threshold() {
        // CostBudget (0.3) * 0.5 + (1 - 8/10) * 0.2 + 1.0 * 0.3
        // = 0.15 + 0.04 + 0.30 = 0.49 — below the default 0.6 threshold.
        let outcome = AgentLoopOutcome {
            stop_condition: StopCondition::CostBudget,
            rounds_used: 8,
            max_rounds: 10,
            has_output: true,
        };
        let score = CompositeEvaluator::evaluate(&outcome);
        assert!((score - 0.49).abs() < 0.01);
        assert!(!CompositeEvaluator::is_success(&outcome, 0.6));
    }
}
