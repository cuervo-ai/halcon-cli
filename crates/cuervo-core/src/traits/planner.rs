//! Planner trait: optional planning step before tool execution.
//!
//! Implementations can generate an execution plan from user intent
//! and available tools, allowing the agent to reason about tool
//! ordering and parallelism before committing to actions.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::types::ToolDefinition;

/// Outcome of executing a plan step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepOutcome {
    Success { summary: String },
    Failed { error: String },
    Skipped { reason: String },
}

/// A step in an execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Human-readable description of what this step does.
    pub description: String,
    /// Tool name to invoke (if this step uses a tool).
    pub tool_name: Option<String>,
    /// Whether this step can run in parallel with the previous step.
    pub parallel: bool,
    /// Estimated importance (0.0 - 1.0).
    pub confidence: f64,
    /// Expected arguments for the tool (optional hint, not enforced).
    #[serde(default)]
    pub expected_args: Option<serde_json::Value>,
    /// Outcome after execution: None until executed.
    #[serde(default)]
    pub outcome: Option<StepOutcome>,
}

/// An execution plan generated before tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    /// High-level goal summary.
    pub goal: String,
    /// Ordered steps to achieve the goal.
    pub steps: Vec<PlanStep>,
    /// Whether the plan requires user confirmation before proceeding.
    pub requires_confirmation: bool,
    /// Unique plan ID for persistence.
    #[serde(default = "uuid::Uuid::new_v4")]
    pub plan_id: uuid::Uuid,
    /// Number of replans that produced this plan (0 = initial).
    #[serde(default)]
    pub replan_count: u32,
    /// Original plan ID if this is a replan.
    #[serde(default)]
    pub parent_plan_id: Option<uuid::Uuid>,
}

/// Formal status of a tracked plan step (FSM).
///
/// Valid transitions:
/// - `Pending` → `Running` | `Skipped` | `Cancelled`
/// - `Running` → `Completed` | `Failed` | `Cancelled`
/// - Terminal states (`Completed`, `Failed`, `Skipped`, `Cancelled`) → error
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

impl TaskStatus {
    /// Returns `true` if this status is terminal (no further transitions allowed).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Skipped | Self::Cancelled
        )
    }

    /// Attempt a state transition. Returns `Err` if the transition is invalid.
    pub fn transition_to(self, target: TaskStatus) -> std::result::Result<TaskStatus, String> {
        match (self, target) {
            // Pending → Running | Skipped | Cancelled
            (Self::Pending, Self::Running)
            | (Self::Pending, Self::Skipped)
            | (Self::Pending, Self::Cancelled) => Ok(target),
            // Running → Completed | Failed | Cancelled
            (Self::Running, Self::Completed)
            | (Self::Running, Self::Failed)
            | (Self::Running, Self::Cancelled) => Ok(target),
            // Terminal → anything is invalid
            (from, to) if from.is_terminal() => {
                Err(format!("cannot transition from terminal state {from:?} to {to:?}"))
            }
            (from, to) => {
                Err(format!("invalid transition from {from:?} to {to:?}"))
            }
        }
    }
}

/// Delegation metadata for steps executed by sub-agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationInfo {
    /// Sub-agent task ID (maps to SubAgentTask.task_id).
    pub task_id: uuid::Uuid,
    /// Agent type that handled this step.
    pub agent_type: String,
    /// Whether the step was delegated (vs. executed inline by main agent).
    pub delegated: bool,
}

/// A plan step enriched with execution tracking metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedStep {
    /// The original plan step.
    pub step: PlanStep,
    /// Current execution status.
    pub status: TaskStatus,
    /// When execution started (UTC).
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When execution finished (UTC).
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Duration in milliseconds (derived from started_at/finished_at).
    pub duration_ms: Option<u64>,
    /// Tool use IDs associated with this step's execution.
    pub tool_use_ids: Vec<String>,
    /// The agent loop round in which this step was resolved.
    pub round: Option<usize>,
    /// Delegation metadata (set when step is executed by a sub-agent).
    #[serde(default)]
    pub delegation: Option<DelegationInfo>,
}

/// Trait for generating execution plans.
///
/// Implementations may use heuristics, templates, or LLM calls
/// to produce a plan from the user's intent and available tools.
#[async_trait]
pub trait Planner: Send + Sync {
    /// Generate a plan for the given user message and available tools.
    async fn plan(
        &self,
        user_message: &str,
        available_tools: &[ToolDefinition],
    ) -> Result<Option<ExecutionPlan>>;

    /// Replan after a step failure, given the current plan and failure context.
    async fn replan(
        &self,
        current_plan: &ExecutionPlan,
        failed_step_index: usize,
        error: &str,
        available_tools: &[ToolDefinition],
    ) -> Result<Option<ExecutionPlan>> {
        // Default: no replanning. Implementations can override.
        let _ = (current_plan, failed_step_index, error, available_tools);
        Ok(None)
    }

    /// Name of this planner implementation.
    fn name(&self) -> &str;

    /// Maximum replans allowed before giving up.
    fn max_replans(&self) -> u32 {
        3
    }

    /// Returns true if the configured model is supported by the backing provider.
    /// Default returns true; LLM-based planners override to validate.
    fn supports_model(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_pending_to_running() {
        assert_eq!(
            TaskStatus::Pending.transition_to(TaskStatus::Running).unwrap(),
            TaskStatus::Running
        );
    }

    #[test]
    fn task_status_pending_to_skipped() {
        assert_eq!(
            TaskStatus::Pending.transition_to(TaskStatus::Skipped).unwrap(),
            TaskStatus::Skipped
        );
    }

    #[test]
    fn task_status_pending_to_cancelled() {
        assert_eq!(
            TaskStatus::Pending.transition_to(TaskStatus::Cancelled).unwrap(),
            TaskStatus::Cancelled
        );
    }

    #[test]
    fn task_status_running_to_completed() {
        assert_eq!(
            TaskStatus::Running.transition_to(TaskStatus::Completed).unwrap(),
            TaskStatus::Completed
        );
    }

    #[test]
    fn task_status_running_to_failed() {
        assert_eq!(
            TaskStatus::Running.transition_to(TaskStatus::Failed).unwrap(),
            TaskStatus::Failed
        );
    }

    #[test]
    fn task_status_running_to_cancelled() {
        assert_eq!(
            TaskStatus::Running.transition_to(TaskStatus::Cancelled).unwrap(),
            TaskStatus::Cancelled
        );
    }

    #[test]
    fn task_status_terminal_rejects_transition() {
        for terminal in [TaskStatus::Completed, TaskStatus::Failed, TaskStatus::Skipped, TaskStatus::Cancelled] {
            assert!(terminal.transition_to(TaskStatus::Running).is_err());
            assert!(terminal.transition_to(TaskStatus::Pending).is_err());
        }
    }

    #[test]
    fn task_status_invalid_pending_to_completed() {
        // Can't skip Running
        assert!(TaskStatus::Pending.transition_to(TaskStatus::Completed).is_err());
    }

    #[test]
    fn task_status_invalid_pending_to_failed() {
        assert!(TaskStatus::Pending.transition_to(TaskStatus::Failed).is_err());
    }

    #[test]
    fn task_status_is_terminal() {
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Skipped.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
    }

    #[test]
    fn task_status_serde_roundtrip() {
        for status in [
            TaskStatus::Pending,
            TaskStatus::Running,
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Skipped,
            TaskStatus::Cancelled,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: TaskStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back);
        }
    }

    #[test]
    fn tracked_step_serde_roundtrip() {
        let ts = TrackedStep {
            step: PlanStep {
                description: "Read file".into(),
                tool_name: Some("file_read".into()),
                parallel: false,
                confidence: 0.9,
                expected_args: None,
                outcome: None,
            },
            status: TaskStatus::Completed,
            started_at: Some(chrono::Utc::now()),
            finished_at: Some(chrono::Utc::now()),
            duration_ms: Some(42),
            tool_use_ids: vec!["id1".into()],
            round: Some(1),
            delegation: None,
        };
        let json = serde_json::to_string(&ts).unwrap();
        let back: TrackedStep = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, TaskStatus::Completed);
        assert_eq!(back.duration_ms, Some(42));
        assert_eq!(back.tool_use_ids, vec!["id1"]);
        assert!(back.delegation.is_none());
    }

    #[test]
    fn delegation_info_serde_roundtrip() {
        let info = DelegationInfo {
            task_id: uuid::Uuid::new_v4(),
            agent_type: "Coder".into(),
            delegated: true,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: DelegationInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.task_id, info.task_id);
        assert_eq!(back.agent_type, "Coder");
        assert!(back.delegated);
    }

    #[test]
    fn tracked_step_with_delegation() {
        let task_id = uuid::Uuid::new_v4();
        let ts = TrackedStep {
            step: PlanStep {
                description: "Edit file".into(),
                tool_name: Some("file_edit".into()),
                parallel: false,
                confidence: 0.8,
                expected_args: None,
                outcome: None,
            },
            status: TaskStatus::Running,
            started_at: Some(chrono::Utc::now()),
            finished_at: None,
            duration_ms: None,
            tool_use_ids: vec![],
            round: None,
            delegation: Some(DelegationInfo {
                task_id,
                agent_type: "Coder".into(),
                delegated: true,
            }),
        };
        let json = serde_json::to_string(&ts).unwrap();
        let back: TrackedStep = serde_json::from_str(&json).unwrap();
        let d = back.delegation.unwrap();
        assert_eq!(d.task_id, task_id);
        assert_eq!(d.agent_type, "Coder");
        assert!(d.delegated);
    }
}
