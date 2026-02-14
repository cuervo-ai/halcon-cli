use serde::{Deserialize, Serialize};

/// Complexity level of a task as determined by query analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskComplexity {
    Simple,
    Moderate,
    Complex,
}

/// Type of task inferred from query keywords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskType {
    CodeGeneration,
    CodeModification,
    Debugging,
    Research,
    FileManagement,
    GitOperation,
    Explanation,
    Configuration,
    General,
}

/// Result of analyzing a user query before agent loop execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAnalysis {
    pub complexity: TaskComplexity,
    pub task_type: TaskType,
    /// Heuristic estimate of steps needed.
    pub estimated_steps: u32,
    /// Extracted action keywords from the query.
    pub keywords: Vec<String>,
    /// SHA-256 of normalized query (for experience lookup).
    pub task_hash: String,
}

/// Result of evaluating an agent loop execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    /// Overall score from 0.0 to 1.0.
    pub score: f64,
    /// Whether the score meets the success threshold.
    pub success: bool,
    /// Individual evaluation factors that contributed to the score.
    pub factors: Vec<EvaluationFactor>,
    /// Improvement hint for retry (populated when score < threshold).
    pub suggestion: Option<String>,
}

/// A single evaluation dimension with its score and weight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationFactor {
    pub name: String,
    pub score: f64,
    pub weight: f64,
}

/// Kind of reasoning strategy to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReasoningStrategyKind {
    /// Simple tasks: just run agent loop directly.
    DirectExecution,
    /// Moderate+: plan, execute, then evaluate.
    PlanExecuteReflect,
}

/// Configuration for the agent loop derived from strategy selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyPlan {
    pub kind: ReasoningStrategyKind,
    pub use_planner: bool,
    pub max_rounds: usize,
    pub enable_reflection: bool,
    pub system_prompt_addendum: Option<String>,
}

/// Configuration for the adaptive reasoning engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningConfig {
    /// Enable the reasoning engine. Default: true.
    #[serde(default = "default_enabled_true")]
    pub enabled: bool,
    /// Minimum score for an execution to be considered successful.
    #[serde(default = "default_success_threshold")]
    pub success_threshold: f64,
    /// Maximum retries with a different strategy if score < threshold.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Whether to persist experience records for cross-session learning.
    #[serde(default = "default_learning_enabled")]
    pub learning_enabled: bool,
    /// UCB1 exploration constant (higher = more exploration).
    #[serde(default = "default_exploration_factor")]
    pub exploration_factor: f64,
}

fn default_enabled_true() -> bool {
    true
}

fn default_success_threshold() -> f64 {
    0.6
}

fn default_max_retries() -> u32 {
    1
}

fn default_learning_enabled() -> bool {
    true
}

fn default_exploration_factor() -> f64 {
    1.4
}

impl Default for ReasoningConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            success_threshold: default_success_threshold(),
            max_retries: default_max_retries(),
            learning_enabled: default_learning_enabled(),
            exploration_factor: default_exploration_factor(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_complexity_serde_roundtrip() {
        for variant in [TaskComplexity::Simple, TaskComplexity::Moderate, TaskComplexity::Complex] {
            let json = serde_json::to_string(&variant).unwrap();
            let roundtrip: TaskComplexity = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, roundtrip);
        }
    }

    #[test]
    fn task_type_serde_roundtrip() {
        let variants = [
            TaskType::CodeGeneration,
            TaskType::CodeModification,
            TaskType::Debugging,
            TaskType::Research,
            TaskType::FileManagement,
            TaskType::GitOperation,
            TaskType::Explanation,
            TaskType::Configuration,
            TaskType::General,
        ];
        for variant in variants {
            let json = serde_json::to_string(&variant).unwrap();
            let roundtrip: TaskType = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, roundtrip);
        }
    }

    #[test]
    fn task_analysis_serde_roundtrip() {
        let analysis = TaskAnalysis {
            complexity: TaskComplexity::Moderate,
            task_type: TaskType::CodeModification,
            estimated_steps: 3,
            keywords: vec!["fix".into(), "edit".into()],
            task_hash: "abc123".into(),
        };
        let json = serde_json::to_string(&analysis).unwrap();
        let roundtrip: TaskAnalysis = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.complexity, TaskComplexity::Moderate);
        assert_eq!(roundtrip.task_type, TaskType::CodeModification);
        assert_eq!(roundtrip.estimated_steps, 3);
        assert_eq!(roundtrip.keywords, vec!["fix", "edit"]);
        assert_eq!(roundtrip.task_hash, "abc123");
    }

    #[test]
    fn evaluation_result_serde_roundtrip() {
        let result = EvaluationResult {
            score: 0.85,
            success: true,
            factors: vec![EvaluationFactor {
                name: "completion".into(),
                score: 0.9,
                weight: 0.3,
            }],
            suggestion: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let roundtrip: EvaluationResult = serde_json::from_str(&json).unwrap();
        assert!((roundtrip.score - 0.85).abs() < f64::EPSILON);
        assert!(roundtrip.success);
        assert_eq!(roundtrip.factors.len(), 1);
        assert!(roundtrip.suggestion.is_none());
    }

    #[test]
    fn evaluation_result_score_boundaries() {
        for score in [0.0, 0.5, 1.0] {
            let result = EvaluationResult {
                score,
                success: score >= 0.6,
                factors: vec![],
                suggestion: None,
            };
            let json = serde_json::to_string(&result).unwrap();
            let roundtrip: EvaluationResult = serde_json::from_str(&json).unwrap();
            assert!((roundtrip.score - score).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn strategy_plan_serde_roundtrip() {
        let plan = StrategyPlan {
            kind: ReasoningStrategyKind::PlanExecuteReflect,
            use_planner: true,
            max_rounds: 10,
            enable_reflection: true,
            system_prompt_addendum: Some("Be thorough".into()),
        };
        let json = serde_json::to_string(&plan).unwrap();
        let roundtrip: StrategyPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.kind, ReasoningStrategyKind::PlanExecuteReflect);
        assert!(roundtrip.use_planner);
        assert_eq!(roundtrip.max_rounds, 10);
        assert!(roundtrip.enable_reflection);
        assert_eq!(roundtrip.system_prompt_addendum.as_deref(), Some("Be thorough"));
    }

    #[test]
    fn reasoning_strategy_kind_serde_roundtrip() {
        for kind in [ReasoningStrategyKind::DirectExecution, ReasoningStrategyKind::PlanExecuteReflect] {
            let json = serde_json::to_string(&kind).unwrap();
            let roundtrip: ReasoningStrategyKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, roundtrip);
        }
    }

    #[test]
    fn reasoning_config_defaults() {
        let config = ReasoningConfig::default();
        assert!(config.enabled);
        assert!((config.success_threshold - 0.6).abs() < f64::EPSILON);
        assert_eq!(config.max_retries, 1);
        assert!(config.learning_enabled);
        assert!((config.exploration_factor - 1.4).abs() < f64::EPSILON);
    }

    #[test]
    fn reasoning_config_serde_roundtrip() {
        let config = ReasoningConfig {
            enabled: true,
            success_threshold: 0.8,
            max_retries: 3,
            learning_enabled: false,
            exploration_factor: 2.0,
        };
        let json = serde_json::to_string(&config).unwrap();
        let roundtrip: ReasoningConfig = serde_json::from_str(&json).unwrap();
        assert!(roundtrip.enabled);
        assert!((roundtrip.success_threshold - 0.8).abs() < f64::EPSILON);
        assert_eq!(roundtrip.max_retries, 3);
        assert!(!roundtrip.learning_enabled);
        assert!((roundtrip.exploration_factor - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn reasoning_config_absent_in_app_config_uses_defaults() {
        // Simulates deserializing an AppConfig without [reasoning] section.
        // With #[serde(default = "default_enabled_true")], missing `enabled` defaults to true.
        let json = r#"{}"#;
        let config: ReasoningConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert!((config.success_threshold - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn evaluation_factor_weight_and_score() {
        let factor = EvaluationFactor {
            name: "stop_condition".into(),
            score: 1.0,
            weight: 0.5,
        };
        let json = serde_json::to_string(&factor).unwrap();
        let roundtrip: EvaluationFactor = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.name, "stop_condition");
        assert!((roundtrip.score - 1.0).abs() < f64::EPSILON);
        assert!((roundtrip.weight - 0.5).abs() < f64::EPSILON);
    }
}
