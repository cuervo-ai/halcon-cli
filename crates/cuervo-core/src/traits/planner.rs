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
