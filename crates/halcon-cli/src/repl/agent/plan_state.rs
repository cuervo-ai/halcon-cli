//! PlanState — Task analysis and execution tracking (optional, task-level).
//!
//! Phase 2.2: This struct groups fields related to plan-based execution.
//! All fields are Option<> since planning is optional (not all queries have plans).

use halcon_core::traits::ExecutionPlan;

/// Task planning and execution tracking state.
///
/// Maximum 15 fields (Phase 2 constraint). All fields Option<> because
/// planning is optional — conversational queries may not generate plans.
/// Focused on plan lifecycle and progress tracking.
#[derive(Debug, Default)]
pub(super) struct PlanState {
    /// Currently active execution plan (from LlmPlanner or PlaybookPlanner).
    pub active_plan: Option<ExecutionPlan>,

    /// Plan progress tracker (per-step status, completion, failures).
    pub execution_tracker: Option<super::super::execution_tracker::ExecutionTracker>,

    /// Orchestrator's delegation decision (when multi-agent orchestration active).
    pub orchestration_decision: Option<super::super::decision_layer::OrchestrationDecision>,

    /// Boundary decision from the decision engine (when boundary engine active).
    pub boundary_decision: Option<super::super::decision_engine::BoundaryDecision>,

    /// SLA budget tracker (when SLA enforcement enabled).
    pub sla_budget: Option<super::super::sla_manager::SlaBudget>,

    /// Whether the plan was silently truncated by SLA budget or depth clamping.
    ///
    /// Set to true when plan steps were removed. Used by EvidenceThreshold to
    /// avoid premature firing on truncated plans (Fix 4).
    pub plan_was_sla_truncated: bool,

    /// Original step count before truncation (0 if no truncation occurred).
    pub original_plan_step_count: usize,
}

impl PlanState {
    /// Check if a plan is currently active.
    pub(super) fn has_plan(&self) -> bool {
        self.active_plan.is_some()
    }

    /// Mark the plan as truncated (called when SLA/depth clamping applies).
    pub(super) fn mark_truncated(&mut self, original_count: usize) {
        self.plan_was_sla_truncated = true;
        self.original_plan_step_count = original_count;
    }

    /// Check if the current plan was truncated.
    pub(super) fn is_truncated(&self) -> bool {
        self.plan_was_sla_truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_state_default_has_no_plan() {
        let state = PlanState::default();
        assert!(!state.has_plan());
        assert!(state.active_plan.is_none());
        assert!(state.execution_tracker.is_none());
        assert!(!state.is_truncated());
    }

    #[test]
    fn plan_state_mark_truncated() {
        let mut state = PlanState::default();
        assert!(!state.is_truncated());
        assert_eq!(state.original_plan_step_count, 0);

        state.mark_truncated(10);

        assert!(state.is_truncated());
        assert_eq!(state.original_plan_step_count, 10);
    }

    #[test]
    fn plan_state_has_plan_when_active() {
        let mut state = PlanState::default();
        assert!(!state.has_plan());

        // Simulate plan assignment
        state.active_plan = Some(ExecutionPlan {
            steps: vec![],
            goal: "test".to_string(),
            ..Default::default()
        });

        assert!(state.has_plan());
    }
}
