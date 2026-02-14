//! Reasoning engine — metacognitive wrapper around the agent loop.
//!
//! Operates BEFORE and AFTER `run_agent_loop()`:
//! - **pre_loop**: analyzes query, selects strategy, configures plan
//! - **post_loop**: evaluates result, records experience, decides retry
//!
//! The agent loop itself is NOT modified. ReasoningEngine is opt-in
//! (enabled by default via `reasoning.enabled = true`).

use chrono::{DateTime, Utc};
use serde::Serialize;

use cuervo_core::types::{
    EvaluationResult, ReasoningConfig, ReasoningStrategyKind, StrategyPlan, TaskAnalysis,
    TaskType,
};

use super::agent_types::AgentLoopResult;
use super::strategy_selector::{ExperienceRecord, StrategySelector};
use super::task_analyzer::TaskAnalyzer;
use super::task_evaluator::CompositeEvaluator;

/// Session-level reasoning record for diagnostics/export.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReasoningRecord {
    pub query: String,
    pub analysis: TaskAnalysis,
    pub strategy: ReasoningStrategyKind,
    pub evaluation: Option<EvaluationResult>,
    pub timestamp: DateTime<Utc>,
}

/// Metacognitive engine wrapping the agent loop.
#[allow(dead_code)]
pub(crate) struct ReasoningEngine {
    selector: StrategySelector,
    evaluator: CompositeEvaluator,
    config: ReasoningConfig,
    history: Vec<ReasoningRecord>,
    retries_used: u32,
}

#[allow(dead_code)]
impl ReasoningEngine {
    /// Create a new engine with default evaluator chain.
    pub fn new(config: &ReasoningConfig) -> Self {
        Self {
            selector: StrategySelector::new(config.exploration_factor),
            evaluator: CompositeEvaluator::default_chain(),
            config: config.clone(),
            history: Vec::new(),
            retries_used: 0,
        }
    }

    /// Load experience records from DB at startup.
    pub fn load_experience(&mut self, records: Vec<ExperienceRecord>) {
        self.selector.load_experience(records);
    }

    /// Pre-loop phase: analyze query, select strategy, configure plan.
    pub fn pre_loop(&mut self, query: &str) -> StrategyPlan {
        let analysis = TaskAnalyzer::analyze(query);
        let strategy = self.selector.select(&analysis);
        let plan = StrategySelector::configure(strategy, &analysis);

        self.history.push(ReasoningRecord {
            query: query.to_string(),
            analysis,
            strategy,
            evaluation: None,
            timestamp: Utc::now(),
        });

        plan
    }

    /// Post-loop phase: evaluate result, update experience.
    pub fn post_loop(&mut self, result: &AgentLoopResult) -> EvaluationResult {
        let last = self.history.last().expect("pre_loop must be called first");
        let analysis = &last.analysis;
        let strategy = last.strategy;

        let eval = self.evaluator.evaluate(
            result,
            analysis,
            self.config.success_threshold,
        );

        // Update selector with outcome.
        self.selector.record_outcome(strategy, analysis.task_type, eval.score);

        // Update the last record with evaluation.
        if let Some(record) = self.history.last_mut() {
            record.evaluation = Some(eval.clone());
        }

        eval
    }

    /// Whether a retry should be attempted.
    pub fn should_retry(&self, evaluation: &EvaluationResult) -> bool {
        !evaluation.success && self.retries_used < self.config.max_retries
    }

    /// Increment the retry counter (call before retrying).
    pub fn increment_retry(&mut self) {
        self.retries_used += 1;
    }

    /// Reset retry counter (call at the start of each user message).
    pub fn reset_retries(&mut self) {
        self.retries_used = 0;
    }

    /// Get the session reasoning history.
    pub fn history(&self) -> &[ReasoningRecord] {
        &self.history
    }

    /// Export the session reasoning log as JSON.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "records": self.history.len(),
            "history": self.history.iter().map(|r| {
                serde_json::json!({
                    "query": &r.query[..r.query.len().min(100)],
                    "complexity": format!("{:?}", r.analysis.complexity),
                    "task_type": format!("{:?}", r.analysis.task_type),
                    "strategy": format!("{:?}", r.strategy),
                    "score": r.evaluation.as_ref().map(|e| e.score),
                    "success": r.evaluation.as_ref().map(|e| e.success),
                })
            }).collect::<Vec<_>>(),
        })
    }

    /// Get current experience for persistence.
    pub fn experience(&self) -> &[ExperienceRecord] {
        self.selector.experience()
    }

    /// Get the config.
    pub fn config(&self) -> &ReasoningConfig {
        &self.config
    }
}

/// Convert ExperienceRow from DB to the in-memory ExperienceRecord.
pub(crate) fn rows_to_records(
    rows: Vec<cuervo_storage::db::experience::ExperienceRow>,
) -> Vec<ExperienceRecord> {
    rows.into_iter()
        .filter_map(|row| {
            let task_type = match row.task_type.as_str() {
                "CodeGeneration" => TaskType::CodeGeneration,
                "CodeModification" => TaskType::CodeModification,
                "Debugging" => TaskType::Debugging,
                "Research" => TaskType::Research,
                "FileManagement" => TaskType::FileManagement,
                "GitOperation" => TaskType::GitOperation,
                "Explanation" => TaskType::Explanation,
                "Configuration" => TaskType::Configuration,
                "General" => TaskType::General,
                _ => return None,
            };
            let strategy = match row.strategy.as_str() {
                "DirectExecution" => ReasoningStrategyKind::DirectExecution,
                "PlanExecuteReflect" => ReasoningStrategyKind::PlanExecuteReflect,
                _ => return None,
            };
            Some(ExperienceRecord {
                task_type,
                strategy,
                avg_score: row.avg_score,
                uses: row.uses,
                last_score: row.last_score,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::TaskComplexity;

    fn test_config() -> ReasoningConfig {
        ReasoningConfig {
            enabled: true,
            ..Default::default()
        }
    }

    fn make_result(
        stop: super::super::agent_types::StopCondition,
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

    #[test]
    fn new_creates_with_default_evaluators() {
        let engine = ReasoningEngine::new(&test_config());
        assert!(engine.history().is_empty());
        assert!(engine.experience().is_empty());
    }

    #[test]
    fn pre_loop_simple_query() {
        let mut engine = ReasoningEngine::new(&test_config());
        let plan = engine.pre_loop("hello");
        assert_eq!(plan.kind, ReasoningStrategyKind::DirectExecution);
        assert!(!plan.use_planner);
        assert_eq!(engine.history().len(), 1);
    }

    #[test]
    fn pre_loop_complex_query() {
        let mut engine = ReasoningEngine::new(&test_config());
        let plan = engine.pre_loop("refactor the entire authentication module to use JWT tokens");
        assert_eq!(plan.kind, ReasoningStrategyKind::PlanExecuteReflect);
        assert!(plan.use_planner);
    }

    #[test]
    fn post_loop_high_score_success() {
        use super::super::agent_types::StopCondition;
        let mut engine = ReasoningEngine::new(&test_config());
        engine.pre_loop("fix a small bug");
        let result = make_result(StopCondition::EndTurn, 1, &"a".repeat(600));
        let eval = engine.post_loop(&result);
        assert!(eval.success);
        assert!(eval.score > 0.6);
    }

    #[test]
    fn post_loop_low_score_failure() {
        use super::super::agent_types::StopCondition;
        let mut engine = ReasoningEngine::new(&test_config());
        engine.pre_loop("fix a small bug");
        let result = make_result(StopCondition::ProviderError, 10, "");
        let eval = engine.post_loop(&result);
        assert!(!eval.success);
        assert!(eval.score < 0.6);
        assert!(eval.suggestion.is_some());
    }

    #[test]
    fn should_retry_below_threshold_with_retries_left() {
        let engine = ReasoningEngine::new(&test_config());
        let eval = EvaluationResult {
            score: 0.3,
            success: false,
            factors: vec![],
            suggestion: Some("retry".into()),
        };
        assert!(engine.should_retry(&eval));
    }

    #[test]
    fn should_retry_above_threshold() {
        let engine = ReasoningEngine::new(&test_config());
        let eval = EvaluationResult {
            score: 0.9,
            success: true,
            factors: vec![],
            suggestion: None,
        };
        assert!(!engine.should_retry(&eval));
    }

    #[test]
    fn should_retry_exhausted() {
        let mut engine = ReasoningEngine::new(&test_config());
        engine.retries_used = 1; // max_retries = 1
        let eval = EvaluationResult {
            score: 0.3,
            success: false,
            factors: vec![],
            suggestion: Some("retry".into()),
        };
        assert!(!engine.should_retry(&eval));
    }

    #[test]
    fn history_accumulates() {
        use super::super::agent_types::StopCondition;
        let mut engine = ReasoningEngine::new(&test_config());
        engine.pre_loop("task one");
        engine.post_loop(&make_result(StopCondition::EndTurn, 1, "ok"));
        engine.reset_retries();
        engine.pre_loop("task two");
        engine.post_loop(&make_result(StopCondition::EndTurn, 1, "ok"));
        assert_eq!(engine.history().len(), 2);
    }

    #[test]
    fn to_json_valid() {
        use super::super::agent_types::StopCondition;
        let mut engine = ReasoningEngine::new(&test_config());
        engine.pre_loop("test query");
        engine.post_loop(&make_result(StopCondition::EndTurn, 1, "output text"));
        let json = engine.to_json();
        assert_eq!(json["records"], 1);
        assert!(json["history"].is_array());
    }

    #[test]
    fn full_cycle_experience_updated() {
        use super::super::agent_types::StopCondition;
        let mut engine = ReasoningEngine::new(&test_config());

        // Run 3 cycles to build experience.
        for i in 0..3 {
            engine.pre_loop(&format!("task {i}"));
            engine.post_loop(&make_result(StopCondition::EndTurn, 1, &"a".repeat(600)));
            engine.reset_retries();
        }

        // Experience should be populated.
        assert!(!engine.experience().is_empty());
    }

    #[test]
    fn ucb1_convergence_after_cycles() {
        use super::super::agent_types::StopCondition;
        let mut engine = ReasoningEngine::new(&ReasoningConfig {
            enabled: true,
            exploration_factor: 0.1, // Low exploration to converge fast
            ..Default::default()
        });

        // Run 10 cycles with DirectExecution always scoring high.
        for _ in 0..10 {
            engine.selector.record_outcome(
                ReasoningStrategyKind::DirectExecution,
                TaskType::General,
                0.9,
            );
            engine.selector.record_outcome(
                ReasoningStrategyKind::PlanExecuteReflect,
                TaskType::General,
                0.3,
            );
        }

        let analysis = TaskAnalysis {
            complexity: TaskComplexity::Simple,
            task_type: TaskType::General,
            estimated_steps: 1,
            keywords: vec![],
            task_hash: "test".into(),
        };
        assert_eq!(
            engine.selector.select(&analysis),
            ReasoningStrategyKind::DirectExecution
        );
    }

    #[test]
    fn config_enabled_by_default() {
        let config = ReasoningConfig::default();
        assert!(config.enabled);
    }

    #[test]
    fn load_experience_works() {
        let mut engine = ReasoningEngine::new(&test_config());
        engine.load_experience(vec![ExperienceRecord {
            task_type: TaskType::General,
            strategy: ReasoningStrategyKind::DirectExecution,
            avg_score: 0.8,
            uses: 5,
            last_score: 0.9,
        }]);
        assert_eq!(engine.experience().len(), 1);
    }

    #[test]
    fn rows_to_records_conversion() {
        let rows = vec![
            cuervo_storage::db::experience::ExperienceRow {
                task_type: "CodeModification".into(),
                strategy: "PlanExecuteReflect".into(),
                avg_score: 0.85,
                uses: 10,
                last_score: 0.9,
                last_task_hash: Some("hash".into()),
                updated_at: "2026-02-12T00:00:00Z".into(),
            },
            cuervo_storage::db::experience::ExperienceRow {
                task_type: "Unknown".into(),
                strategy: "DirectExecution".into(),
                avg_score: 0.5,
                uses: 1,
                last_score: 0.5,
                last_task_hash: None,
                updated_at: "2026-02-12T00:00:00Z".into(),
            },
        ];

        let records = rows_to_records(rows);
        // "Unknown" task type should be filtered out.
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].task_type, TaskType::CodeModification);
        assert_eq!(records[0].strategy, ReasoningStrategyKind::PlanExecuteReflect);
    }
}
