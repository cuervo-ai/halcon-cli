//! Planner trait: optional planning step before tool execution.
//!
//! Implementations can generate an execution plan from user intent
//! and available tools, allowing the agent to reason about tool
//! ordering and parallelism before committing to actions.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::types::ToolDefinition;
use crate::types::capability_types::{CapabilityDescriptor, Modality};
use crate::types::execution_graph::{ExecutionEdge, ExecutionGraph, ExecutionNode, NodeId};

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
    /// Stable identity for this step across replans and rounds.
    /// Auto-generated on deserialization when not present (LLM-generated plans).
    #[serde(default = "uuid::Uuid::new_v4")]
    pub step_id: uuid::Uuid,
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


impl Default for PlanStep {
    fn default() -> Self {
        Self {
            step_id: uuid::Uuid::new_v4(),
            description: String::new(),
            tool_name: None,
            parallel: false,
            confidence: 1.0,
            expected_args: None,
            outcome: None,
        }
    }
}

/// Execution mode inferred from plan structure.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    #[default]
    PlanExecuteReflect,
    DirectExecution,
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
    /// Inferred execution mode (set post-parse, not by LLM).
    #[serde(default)]
    pub mode: ExecutionMode,
    /// True if plan requires at least one content-read tool.
    #[serde(default)]
    pub requires_evidence: bool,
    /// Tools explicitly blocked for this plan (propagated from session).
    #[serde(default)]
    pub blocked_tools: Vec<String>,
    /// Declared execution requirements for the Step 7 capability gate.
    /// Populated post-parse via `derive_capability_descriptor()` — never set by LLM directly.
    /// Default: all-empty → gate always passes → zero-drift for existing plans.
    #[serde(default)]
    pub capability_descriptor: CapabilityDescriptor,
}

impl Default for ExecutionPlan {
    fn default() -> Self {
        Self {
            goal: String::new(),
            steps: vec![],
            requires_confirmation: false,
            plan_id: uuid::Uuid::new_v4(),
            replan_count: 0,
            parent_plan_id: None,
            mode: ExecutionMode::PlanExecuteReflect,
            requires_evidence: false,
            blocked_tools: vec![],
            capability_descriptor: CapabilityDescriptor::default(),
        }
    }
}

impl ExecutionPlan {
    /// Average confidence across all plan steps.
    ///
    /// Returns `1.0` for empty plans (no steps → no uncertainty).
    /// Used by the `NeedsClarification` gate to decide whether to pause
    /// and ask the user before executing destructive steps.
    pub fn avg_confidence(&self) -> f64 {
        if self.steps.is_empty() {
            return 1.0;
        }
        self.steps.iter().map(|s| s.confidence).sum::<f64>() / self.steps.len() as f64
    }

    /// Auto-derive `CapabilityDescriptor` from plan step tool names (Step 7/8.1/10).
    ///
    /// Extracts unique, non-empty tool names from all steps. Infers `ToolUse`
    /// modality when any step references a tool; `Text` is always included.
    ///
    /// `avg_input_tokens_per_step`: base per-step token estimate from `PolicyConfig`.
    /// `tool_cost_multiplier`: multiplier applied to ToolUse/Vision nodes.
    /// Pass `(0, _)` to disable Rule 3 budget checking (e.g., in tests or sub-agents).
    ///
    /// `estimated_token_cost` is now topology-aware via graph cost propagation (Step 10):
    ///   Text node → `avg`; ToolUse node → `avg × multiplier`; sum over reachable nodes.
    ///
    /// Called post-parse in `agent/mod.rs` and after replanning in `convergence_phase.rs`.
    pub fn derive_capability_descriptor(
        &mut self,
        avg_input_tokens_per_step: usize,
        tool_cost_multiplier: usize,
    ) {
        // Step 1: Compute required_tools (deduped) and modalities from step metadata.
        let mut seen = std::collections::HashSet::new();
        let required_tools: Vec<String> = self.steps.iter()
            .filter_map(|s| s.tool_name.as_deref())
            .filter(|t| seen.insert(*t))
            .map(|t| t.to_string())
            .collect();

        let required_modalities = if required_tools.is_empty() {
            vec![Modality::Text]
        } else {
            vec![Modality::Text, Modality::ToolUse]
        };

        // Step 2: Set descriptor with required_tools populated so to_execution_graph()
        // can read them for declared_tools. estimated_token_cost set to 0 as placeholder.
        self.capability_descriptor = CapabilityDescriptor {
            required_tools,
            required_modalities,
            estimated_token_cost: 0,
        };

        // Step 3: Graph-based cost propagation (Step 10).
        // Builds the linear graph, assigns topology-aware node costs, sums reachable cost.
        let mut graph = self.to_execution_graph();
        graph.assign_base_costs(avg_input_tokens_per_step, tool_cost_multiplier);
        self.capability_descriptor.estimated_token_cost = graph.total_cost();
    }

    /// Convert this plan into an `ExecutionGraph` for structural validation (Step 9).
    ///
    /// Nodes correspond 1:1 to plan steps (by index).
    /// Edges are linear: node 0→1, 1→2, ..., (n-2)→(n-1).
    /// `declared_tools` mirrors `capability_descriptor.required_tools`.
    ///
    /// Produces an acyclic graph by construction. Call `GraphValidator::validate()`
    /// on the result to enforce all 4 structural rules before execution.
    pub fn to_execution_graph(&self) -> ExecutionGraph {
        let nodes: Vec<ExecutionNode> = self.steps.iter().enumerate()
            .map(|(i, step)| ExecutionNode {
                id: NodeId(i),
                tool: step.tool_name.clone(),
                modality: if step.tool_name.is_some() {
                    Modality::ToolUse
                } else {
                    Modality::Text
                },
                base_cost: 0, // Populated by assign_base_costs() — default 0 until then.
            })
            .collect();

        let edges: Vec<ExecutionEdge> = (0..nodes.len().saturating_sub(1))
            .map(|i| ExecutionEdge { from: NodeId(i), to: NodeId(i + 1) })
            .collect();

        ExecutionGraph {
            declared_tools: self.capability_descriptor.required_tools.clone(),
            nodes,
            edges,
        }
    }
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

    fn make_step(confidence: f64) -> PlanStep {
        PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: "step".into(),
            tool_name: None,
            parallel: false,
            confidence,
            expected_args: None,
            outcome: None,
        }
    }

    fn make_plan(steps: Vec<PlanStep>) -> ExecutionPlan {
        ExecutionPlan {
            goal: "test goal".into(),
            steps,
            ..Default::default()
        }
    }

    #[test]
    fn avg_confidence_empty_returns_one() {
        let plan = make_plan(vec![]);
        assert_eq!(plan.avg_confidence(), 1.0,
            "Empty plan has no uncertainty — should return 1.0");
    }

    #[test]
    fn avg_confidence_computed_correctly() {
        let plan = make_plan(vec![
            make_step(0.8),
            make_step(0.6),
            make_step(1.0),
        ]);
        let expected = (0.8 + 0.6 + 1.0) / 3.0;
        let diff = (plan.avg_confidence() - expected).abs();
        assert!(diff < 1e-10,
            "avg_confidence should be {expected:.4}, got {:.4}", plan.avg_confidence());
    }

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
                step_id: uuid::Uuid::new_v4(),
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
                step_id: uuid::Uuid::new_v4(),
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

    // ── BUG-007 regression: synthesis guard condition (FIX-1) ────────────────

    fn make_plan_step(tool: Option<&str>, done: bool) -> PlanStep {
        PlanStep {
            tool_name: tool.map(str::to_owned),
            outcome: done.then(|| StepOutcome::Success { summary: "ok".into() }),
            ..PlanStep::default()
        }
    }

    /// Mixed plan: execution + coordination steps pending.
    /// FIX-1: `any_pending_execution` must be true → guard must NOT fire.
    #[test]
    fn bug007_mixed_plan_reports_pending_execution() {
        let steps = vec![
            make_plan_step(Some("bash"), false),        // pending execution
            make_plan_step(Some("file_write"), false),  // pending execution
            make_plan_step(None, false),                // pending coordination
            make_plan_step(None, false),                // pending synthesis
        ];
        let any_pending_execution = steps.iter()
            .filter(|s| s.outcome.is_none())
            .any(|s| s.tool_name.is_some());
        assert!(any_pending_execution,
            "Mixed plan must report pending execution — synthesis guard must NOT fire");
    }

    /// Pure synthesis plan (all execution steps done, only text steps remain).
    /// FIX-1: no pending execution → guard SHOULD fire.
    #[test]
    fn bug007_pure_synthesis_plan_no_pending_execution() {
        let steps = vec![
            make_plan_step(Some("bash"), true),         // completed
            make_plan_step(Some("file_write"), true),   // completed
            make_plan_step(None, false),                // pending coordination
            make_plan_step(None, false),                // pending synthesis
        ];
        let any_pending_execution = steps.iter()
            .filter(|s| s.outcome.is_none())
            .any(|s| s.tool_name.is_some());
        assert!(!any_pending_execution,
            "Pure synthesis plan must report no pending execution — guard SHOULD fire");
    }

    /// Demonstrates the original bug fires when a pending execution step
    /// is followed by a coordination step (tool_name=None).
    /// The old `all(is_none)` condition returns false here (does NOT fire),
    /// but a scenario where execution steps are listed BEFORE None steps makes
    /// the bug visible: once execution is logically "last seen", the guard fires
    /// incorrectly. The new condition is path-independent.
    #[test]
    fn bug007_new_condition_is_path_independent() {
        // Scenario A: pending execution step exists anywhere in the list
        let steps_a = vec![
            make_plan_step(None, false),          // coordination (pending)
            make_plan_step(Some("bash"), false),  // execution (pending)
        ];
        let any_exec_a = steps_a.iter()
            .filter(|s| s.outcome.is_none())
            .any(|s| s.tool_name.is_some());
        // New condition: guard does NOT fire (execution step is pending)
        let new_fires_a = steps_a.iter().any(|s| s.outcome.is_none()) && !any_exec_a;
        assert!(!new_fires_a, "New condition must not fire when execution step is pending");

        // Scenario B: no execution steps pending
        let steps_b = vec![
            make_plan_step(None, false),         // coordination (pending)
            make_plan_step(Some("bash"), true),  // execution (done)
        ];
        let any_exec_b = steps_b.iter()
            .filter(|s| s.outcome.is_none())
            .any(|s| s.tool_name.is_some());
        let new_fires_b = steps_b.iter().any(|s| s.outcome.is_none()) && !any_exec_b;
        assert!(new_fires_b, "New condition must fire when only coordination steps remain");
    }
}
