//! Task evaluator — scores agent loop execution quality.
//!
//! Multiple evaluators (stop condition, efficiency, completion) each produce
//! a weighted factor. CompositeEvaluator aggregates them into a final score.

use cuervo_core::types::{EvaluationFactor, EvaluationResult, TaskAnalysis};

use super::agent_types::{AgentLoopResult, StopCondition};

/// Evaluates a single dimension of agent loop quality.
pub(crate) trait TaskEvaluator: Send + Sync {
    fn evaluate(&self, result: &AgentLoopResult, analysis: &TaskAnalysis) -> EvaluationFactor;
}

/// Scores based on how the agent loop stopped.
pub(crate) struct StopConditionEvaluator;

impl TaskEvaluator for StopConditionEvaluator {
    fn evaluate(&self, result: &AgentLoopResult, _analysis: &TaskAnalysis) -> EvaluationFactor {
        let score = match result.stop_condition {
            StopCondition::EndTurn => 1.0,
            StopCondition::ForcedSynthesis => 0.7,
            StopCondition::MaxRounds => 0.4,
            StopCondition::TokenBudget => 0.3,
            StopCondition::DurationBudget => 0.3,
            StopCondition::Interrupted => 0.2,
            StopCondition::ProviderError => 0.0,
        };
        EvaluationFactor {
            name: "stop_condition".into(),
            score,
            weight: 0.5,
        }
    }
}

/// Scores round efficiency (fewer rounds = better).
pub(crate) struct EfficiencyEvaluator;

impl TaskEvaluator for EfficiencyEvaluator {
    fn evaluate(&self, result: &AgentLoopResult, analysis: &TaskAnalysis) -> EvaluationFactor {
        let max_expected = (analysis.estimated_steps as usize * 2).max(1);
        let ratio = result.rounds as f64 / max_expected as f64;
        let score = (1.0 - ratio).max(0.0).min(1.0);
        EvaluationFactor {
            name: "efficiency".into(),
            score,
            weight: 0.2,
        }
    }
}

/// Scores output quality based on text presence and length.
pub(crate) struct CompletionEvaluator;

impl TaskEvaluator for CompletionEvaluator {
    fn evaluate(&self, result: &AgentLoopResult, _analysis: &TaskAnalysis) -> EvaluationFactor {
        let text_len = result.full_text.len();
        let score = if text_len == 0 {
            0.0
        } else if text_len > 500 {
            1.0
        } else if text_len > 100 {
            0.8
        } else {
            0.5
        };
        EvaluationFactor {
            name: "completion".into(),
            score,
            weight: 0.3,
        }
    }
}

/// Aggregates multiple evaluators into a composite score.
pub(crate) struct CompositeEvaluator {
    evaluators: Vec<Box<dyn TaskEvaluator>>,
}

impl CompositeEvaluator {
    pub fn new() -> Self {
        Self {
            evaluators: Vec::new(),
        }
    }

    /// Create with the standard evaluator chain.
    pub fn default_chain() -> Self {
        let mut eval = Self::new();
        eval.add(Box::new(StopConditionEvaluator));
        eval.add(Box::new(EfficiencyEvaluator));
        eval.add(Box::new(CompletionEvaluator));
        eval
    }

    pub fn add(&mut self, evaluator: Box<dyn TaskEvaluator>) {
        self.evaluators.push(evaluator);
    }

    /// Evaluate the agent loop result against the task analysis.
    pub fn evaluate(
        &self,
        result: &AgentLoopResult,
        analysis: &TaskAnalysis,
        success_threshold: f64,
    ) -> EvaluationResult {
        if self.evaluators.is_empty() {
            return EvaluationResult {
                score: 0.0,
                success: false,
                factors: vec![],
                suggestion: Some("No evaluators configured".into()),
            };
        }

        let factors: Vec<EvaluationFactor> = self
            .evaluators
            .iter()
            .map(|e| e.evaluate(result, analysis))
            .collect();

        let total_weight: f64 = factors.iter().map(|f| f.weight).sum();
        let score = if total_weight > 0.0 {
            factors.iter().map(|f| f.score * f.weight).sum::<f64>() / total_weight
        } else {
            0.0
        };

        let success = score >= success_threshold;
        let suggestion = if !success {
            // Find the weakest factor as a hint.
            let weakest = factors
                .iter()
                .min_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
            weakest.map(|f| format!("Weakest area: {} (score: {:.2})", f.name, f.score))
        } else {
            None
        };

        EvaluationResult {
            score,
            success,
            factors,
            suggestion,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::{TaskComplexity, TaskType};

    fn make_analysis() -> TaskAnalysis {
        TaskAnalysis {
            complexity: TaskComplexity::Moderate,
            task_type: TaskType::CodeModification,
            estimated_steps: 3,
            keywords: vec!["fix".into()],
            task_hash: "test".into(),
        }
    }

    fn make_result(
        stop: StopCondition,
        rounds: usize,
        text: &str,
    ) -> AgentLoopResult {
        AgentLoopResult {
            full_text: text.to_string(),
            rounds,
            stop_condition: stop,
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.001,
            latency_ms: 1000,
            execution_fingerprint: "fp".into(),
            timeline_json: None,
            ctrl_rx: None,
        }
    }

    // --- StopConditionEvaluator ---

    #[test]
    fn stop_condition_end_turn_scores_one() {
        let eval = StopConditionEvaluator;
        let factor = eval.evaluate(&make_result(StopCondition::EndTurn, 1, "ok"), &make_analysis());
        assert!((factor.score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn stop_condition_forced_synthesis() {
        let eval = StopConditionEvaluator;
        let factor = eval.evaluate(&make_result(StopCondition::ForcedSynthesis, 1, "ok"), &make_analysis());
        assert!((factor.score - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn stop_condition_max_rounds() {
        let eval = StopConditionEvaluator;
        let factor = eval.evaluate(&make_result(StopCondition::MaxRounds, 1, "ok"), &make_analysis());
        assert!((factor.score - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn stop_condition_token_budget() {
        let eval = StopConditionEvaluator;
        let factor = eval.evaluate(&make_result(StopCondition::TokenBudget, 1, "ok"), &make_analysis());
        assert!((factor.score - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn stop_condition_interrupted() {
        let eval = StopConditionEvaluator;
        let factor = eval.evaluate(&make_result(StopCondition::Interrupted, 1, "ok"), &make_analysis());
        assert!((factor.score - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn stop_condition_provider_error() {
        let eval = StopConditionEvaluator;
        let factor = eval.evaluate(&make_result(StopCondition::ProviderError, 1, "ok"), &make_analysis());
        assert!(factor.score.abs() < f64::EPSILON);
    }

    #[test]
    fn stop_condition_duration_budget() {
        let eval = StopConditionEvaluator;
        let factor = eval.evaluate(&make_result(StopCondition::DurationBudget, 1, "ok"), &make_analysis());
        assert!((factor.score - 0.3).abs() < f64::EPSILON);
    }

    // --- EfficiencyEvaluator ---

    #[test]
    fn efficiency_low_rounds_high_score() {
        let eval = EfficiencyEvaluator;
        let analysis = make_analysis(); // estimated_steps=3, max_expected=6
        let factor = eval.evaluate(&make_result(StopCondition::EndTurn, 1, "ok"), &analysis);
        // 1 - (1/6) ≈ 0.833
        assert!(factor.score > 0.8);
    }

    #[test]
    fn efficiency_exceeded_rounds_zero() {
        let eval = EfficiencyEvaluator;
        let analysis = make_analysis(); // estimated_steps=3, max_expected=6
        let factor = eval.evaluate(&make_result(StopCondition::EndTurn, 10, "ok"), &analysis);
        // 1 - (10/6) → capped at 0.0
        assert!(factor.score.abs() < f64::EPSILON);
    }

    // --- CompletionEvaluator ---

    #[test]
    fn completion_empty_text() {
        let eval = CompletionEvaluator;
        let factor = eval.evaluate(&make_result(StopCondition::EndTurn, 1, ""), &make_analysis());
        assert!(factor.score.abs() < f64::EPSILON);
    }

    #[test]
    fn completion_short_text() {
        let eval = CompletionEvaluator;
        let factor = eval.evaluate(&make_result(StopCondition::EndTurn, 1, "yes"), &make_analysis());
        assert!((factor.score - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn completion_long_text() {
        let eval = CompletionEvaluator;
        let text = "a".repeat(600);
        let factor = eval.evaluate(&make_result(StopCondition::EndTurn, 1, &text), &make_analysis());
        assert!((factor.score - 1.0).abs() < f64::EPSILON);
    }

    // --- CompositeEvaluator ---

    #[test]
    fn composite_weighted_average() {
        let composite = CompositeEvaluator::default_chain();
        let result = make_result(StopCondition::EndTurn, 1, &"a".repeat(600));
        let eval = composite.evaluate(&result, &make_analysis(), 0.6);

        // stop_condition=1.0*0.5 + efficiency≈0.833*0.2 + completion=1.0*0.3 = 0.967
        assert!(eval.score > 0.9);
        assert!(eval.success);
        assert!(eval.suggestion.is_none());
    }

    #[test]
    fn composite_threshold_check() {
        let composite = CompositeEvaluator::default_chain();
        let result = make_result(StopCondition::ProviderError, 10, "");
        let eval = composite.evaluate(&result, &make_analysis(), 0.6);

        assert!(eval.score < 0.6);
        assert!(!eval.success);
        assert!(eval.suggestion.is_some());
    }

    #[test]
    fn composite_empty_evaluators() {
        let composite = CompositeEvaluator::new();
        let result = make_result(StopCondition::EndTurn, 1, "hello");
        let eval = composite.evaluate(&result, &make_analysis(), 0.6);
        assert!(eval.score.abs() < f64::EPSILON);
        assert!(!eval.success);
    }

    #[test]
    fn composite_single_evaluator() {
        let mut composite = CompositeEvaluator::new();
        composite.add(Box::new(StopConditionEvaluator));
        let result = make_result(StopCondition::EndTurn, 1, "hello");
        let eval = composite.evaluate(&result, &make_analysis(), 0.6);
        assert!((eval.score - 1.0).abs() < f64::EPSILON);
        assert!(eval.success);
    }

    #[test]
    fn suggestion_populated_below_threshold() {
        let composite = CompositeEvaluator::default_chain();
        let result = make_result(StopCondition::ProviderError, 10, "");
        let eval = composite.evaluate(&result, &make_analysis(), 0.6);
        assert!(eval.suggestion.is_some());
        let suggestion = eval.suggestion.unwrap();
        assert!(suggestion.contains("Weakest area"));
    }
}
