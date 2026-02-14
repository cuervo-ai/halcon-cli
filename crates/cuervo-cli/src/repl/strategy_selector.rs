//! Strategy selector — chooses reasoning strategy based on task analysis and experience.
//!
//! Uses UCB1 (Upper Confidence Bound) to balance exploitation of known-good
//! strategies with exploration of underused ones.

use serde::{Deserialize, Serialize};

use cuervo_core::types::{
    ReasoningStrategyKind, StrategyPlan, TaskAnalysis, TaskComplexity, TaskType,
};

/// Record of past experience with a (task_type, strategy) pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExperienceRecord {
    pub task_type: TaskType,
    pub strategy: ReasoningStrategyKind,
    pub avg_score: f64,
    pub uses: u32,
    pub last_score: f64,
}

/// Selects reasoning strategies using UCB1 multi-armed bandit.
pub(crate) struct StrategySelector {
    experience: Vec<ExperienceRecord>,
    exploration_factor: f64,
}

impl StrategySelector {
    pub fn new(exploration_factor: f64) -> Self {
        Self {
            experience: Vec::new(),
            exploration_factor,
        }
    }

    /// Bulk-load experience records (e.g., from DB at startup).
    pub fn load_experience(&mut self, records: Vec<ExperienceRecord>) {
        self.experience = records;
    }

    /// Select a reasoning strategy for the given task analysis.
    pub fn select(&self, analysis: &TaskAnalysis) -> ReasoningStrategyKind {
        // Filter experience for this task type.
        let relevant: Vec<&ExperienceRecord> = self
            .experience
            .iter()
            .filter(|r| r.task_type == analysis.task_type)
            .collect();

        if relevant.is_empty() {
            // No experience: use default mapping.
            return default_strategy(analysis.complexity);
        }

        // UCB1 selection.
        let total_uses: u32 = relevant.iter().map(|r| r.uses).sum();
        if total_uses == 0 {
            return default_strategy(analysis.complexity);
        }

        let strategies = [
            ReasoningStrategyKind::DirectExecution,
            ReasoningStrategyKind::PlanExecuteReflect,
        ];

        let mut best_strategy = default_strategy(analysis.complexity);
        let mut best_ucb = f64::NEG_INFINITY;

        for &strategy in &strategies {
            let record = relevant.iter().find(|r| r.strategy == strategy);
            let ucb = match record {
                Some(r) if r.uses > 0 => {
                    // UCB1: avg_score + c * sqrt(ln(total) / uses)
                    let exploitation = r.avg_score;
                    let exploration = self.exploration_factor
                        * ((total_uses as f64).ln() / r.uses as f64).sqrt();
                    exploitation + exploration
                }
                _ => {
                    // Never tried: infinite UCB (explore first).
                    f64::INFINITY
                }
            };

            if ucb > best_ucb {
                best_ucb = ucb;
                best_strategy = strategy;
            }
        }

        best_strategy
    }

    /// Configure a strategy plan based on selected kind and analysis.
    pub fn configure(
        kind: ReasoningStrategyKind,
        analysis: &TaskAnalysis,
    ) -> StrategyPlan {
        let base_rounds = match kind {
            ReasoningStrategyKind::DirectExecution => 5,
            ReasoningStrategyKind::PlanExecuteReflect => 10,
        };

        let max_rounds = match analysis.complexity {
            TaskComplexity::Simple => (base_rounds / 2).max(2),
            TaskComplexity::Moderate => base_rounds,
            TaskComplexity::Complex => (base_rounds as f64 * 1.5) as usize,
        };

        StrategyPlan {
            kind,
            use_planner: matches!(kind, ReasoningStrategyKind::PlanExecuteReflect),
            max_rounds,
            enable_reflection: matches!(kind, ReasoningStrategyKind::PlanExecuteReflect),
            system_prompt_addendum: None,
        }
    }

    /// Record the outcome of a strategy execution (updates running average).
    pub fn record_outcome(
        &mut self,
        kind: ReasoningStrategyKind,
        task_type: TaskType,
        score: f64,
    ) {
        if let Some(record) = self
            .experience
            .iter_mut()
            .find(|r| r.task_type == task_type && r.strategy == kind)
        {
            // Update running average.
            let total = record.avg_score * record.uses as f64 + score;
            record.uses += 1;
            record.avg_score = total / record.uses as f64;
            record.last_score = score;
        } else {
            // New experience entry.
            self.experience.push(ExperienceRecord {
                task_type,
                strategy: kind,
                avg_score: score,
                uses: 1,
                last_score: score,
            });
        }
    }

    /// Get current experience records (for persistence).
    pub fn experience(&self) -> &[ExperienceRecord] {
        &self.experience
    }
}

/// Default strategy mapping when no experience is available.
fn default_strategy(complexity: TaskComplexity) -> ReasoningStrategyKind {
    match complexity {
        TaskComplexity::Simple => ReasoningStrategyKind::DirectExecution,
        TaskComplexity::Moderate | TaskComplexity::Complex => {
            ReasoningStrategyKind::PlanExecuteReflect
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mapping_simple() {
        let selector = StrategySelector::new(1.4);
        let analysis = TaskAnalysis {
            complexity: TaskComplexity::Simple,
            task_type: TaskType::General,
            estimated_steps: 1,
            keywords: vec![],
            task_hash: "abc".into(),
        };
        assert_eq!(
            selector.select(&analysis),
            ReasoningStrategyKind::DirectExecution
        );
    }

    #[test]
    fn default_mapping_moderate() {
        let selector = StrategySelector::new(1.4);
        let analysis = TaskAnalysis {
            complexity: TaskComplexity::Moderate,
            task_type: TaskType::CodeModification,
            estimated_steps: 3,
            keywords: vec![],
            task_hash: "abc".into(),
        };
        assert_eq!(
            selector.select(&analysis),
            ReasoningStrategyKind::PlanExecuteReflect
        );
    }

    #[test]
    fn default_mapping_complex() {
        let selector = StrategySelector::new(1.4);
        let analysis = TaskAnalysis {
            complexity: TaskComplexity::Complex,
            task_type: TaskType::CodeGeneration,
            estimated_steps: 5,
            keywords: vec![],
            task_hash: "abc".into(),
        };
        assert_eq!(
            selector.select(&analysis),
            ReasoningStrategyKind::PlanExecuteReflect
        );
    }

    #[test]
    fn ucb1_selects_underexplored() {
        let mut selector = StrategySelector::new(1.4);
        // Load experience: DirectExecution used 10 times with avg 0.5
        selector.load_experience(vec![ExperienceRecord {
            task_type: TaskType::General,
            strategy: ReasoningStrategyKind::DirectExecution,
            avg_score: 0.5,
            uses: 10,
            last_score: 0.5,
        }]);

        let analysis = TaskAnalysis {
            complexity: TaskComplexity::Simple,
            task_type: TaskType::General,
            estimated_steps: 1,
            keywords: vec![],
            task_hash: "abc".into(),
        };

        // PlanExecuteReflect has never been tried → infinite UCB → should be selected.
        assert_eq!(
            selector.select(&analysis),
            ReasoningStrategyKind::PlanExecuteReflect
        );
    }

    #[test]
    fn ucb1_converges_to_best() {
        let mut selector = StrategySelector::new(0.1); // Low exploration
        selector.load_experience(vec![
            ExperienceRecord {
                task_type: TaskType::CodeModification,
                strategy: ReasoningStrategyKind::DirectExecution,
                avg_score: 0.3,
                uses: 20,
                last_score: 0.3,
            },
            ExperienceRecord {
                task_type: TaskType::CodeModification,
                strategy: ReasoningStrategyKind::PlanExecuteReflect,
                avg_score: 0.9,
                uses: 20,
                last_score: 0.9,
            },
        ]);

        let analysis = TaskAnalysis {
            complexity: TaskComplexity::Moderate,
            task_type: TaskType::CodeModification,
            estimated_steps: 3,
            keywords: vec![],
            task_hash: "abc".into(),
        };

        // With low exploration factor and enough data, should pick the higher scorer.
        assert_eq!(
            selector.select(&analysis),
            ReasoningStrategyKind::PlanExecuteReflect
        );
    }

    #[test]
    fn configure_direct_execution() {
        let analysis = TaskAnalysis {
            complexity: TaskComplexity::Simple,
            task_type: TaskType::General,
            estimated_steps: 1,
            keywords: vec![],
            task_hash: "abc".into(),
        };
        let plan = StrategySelector::configure(
            ReasoningStrategyKind::DirectExecution,
            &analysis,
        );
        assert!(!plan.use_planner);
        assert!(!plan.enable_reflection);
        assert!(plan.max_rounds >= 2);
    }

    #[test]
    fn configure_plan_execute_reflect() {
        let analysis = TaskAnalysis {
            complexity: TaskComplexity::Moderate,
            task_type: TaskType::CodeModification,
            estimated_steps: 3,
            keywords: vec![],
            task_hash: "abc".into(),
        };
        let plan = StrategySelector::configure(
            ReasoningStrategyKind::PlanExecuteReflect,
            &analysis,
        );
        assert!(plan.use_planner);
        assert!(plan.enable_reflection);
        assert_eq!(plan.max_rounds, 10);
    }

    #[test]
    fn max_rounds_adjusted_by_complexity() {
        let simple = TaskAnalysis {
            complexity: TaskComplexity::Simple,
            task_type: TaskType::General,
            estimated_steps: 1,
            keywords: vec![],
            task_hash: "abc".into(),
        };
        let complex = TaskAnalysis {
            complexity: TaskComplexity::Complex,
            task_type: TaskType::General,
            estimated_steps: 5,
            keywords: vec![],
            task_hash: "abc".into(),
        };

        let plan_s = StrategySelector::configure(
            ReasoningStrategyKind::PlanExecuteReflect,
            &simple,
        );
        let plan_c = StrategySelector::configure(
            ReasoningStrategyKind::PlanExecuteReflect,
            &complex,
        );

        assert!(plan_s.max_rounds < plan_c.max_rounds);
    }

    #[test]
    fn record_outcome_updates_average() {
        let mut selector = StrategySelector::new(1.4);
        selector.record_outcome(
            ReasoningStrategyKind::DirectExecution,
            TaskType::General,
            0.8,
        );
        assert_eq!(selector.experience().len(), 1);
        assert!((selector.experience()[0].avg_score - 0.8).abs() < f64::EPSILON);
        assert_eq!(selector.experience()[0].uses, 1);

        selector.record_outcome(
            ReasoningStrategyKind::DirectExecution,
            TaskType::General,
            0.6,
        );
        assert_eq!(selector.experience()[0].uses, 2);
        assert!((selector.experience()[0].avg_score - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn load_experience_populates_state() {
        let mut selector = StrategySelector::new(1.4);
        assert!(selector.experience().is_empty());

        selector.load_experience(vec![
            ExperienceRecord {
                task_type: TaskType::General,
                strategy: ReasoningStrategyKind::DirectExecution,
                avg_score: 0.7,
                uses: 5,
                last_score: 0.8,
            },
        ]);

        assert_eq!(selector.experience().len(), 1);
        assert_eq!(selector.experience()[0].uses, 5);
    }
}
