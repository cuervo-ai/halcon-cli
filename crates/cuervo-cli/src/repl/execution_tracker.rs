//! Centralized plan execution tracking with timing, state machine, and timeline export.
//!
//! `ExecutionTracker` consolidates scattered plan step matching, event emission,
//! and index tracking from agent.rs into a single testable unit.

use std::time::Instant;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use cuervo_core::traits::{ExecutionPlan, StepOutcome, TaskStatus, TrackedStep};
use cuervo_core::types::{DomainEvent, EventPayload};
use cuervo_core::EventSender;

/// Matched outcome from `record_tool_results`, returned for DB persistence.
pub(crate) struct MatchedOutcome {
    pub step_index: usize,
    pub outcome: StepOutcome,
}

/// Centralized tracker for plan execution with step timing and state management.
///
/// Owned mutably by the agent loop (same ownership model as `ToolLoopGuard`).
pub(crate) struct ExecutionTracker {
    plan: ExecutionPlan,
    steps: Vec<TrackedStep>,
    plan_start: Instant,
    event_tx: EventSender,
}

impl ExecutionTracker {
    /// Create a new tracker wrapping an execution plan.
    pub fn new(plan: ExecutionPlan, event_tx: EventSender) -> Self {
        let steps = plan
            .steps
            .iter()
            .map(|s| {
                // Derive initial status from pre-set outcomes (e.g., from replanned steps).
                let status = match &s.outcome {
                    Some(StepOutcome::Success { .. }) => TaskStatus::Completed,
                    Some(StepOutcome::Failed { .. }) => TaskStatus::Failed,
                    Some(StepOutcome::Skipped { .. }) => TaskStatus::Skipped,
                    None => TaskStatus::Pending,
                };
                TrackedStep {
                    step: s.clone(),
                    status,
                    started_at: None,
                    finished_at: None,
                    duration_ms: None,
                    tool_use_ids: Vec::new(),
                    round: None,
                    delegation: None,
                }
            })
            .collect();
        Self {
            plan,
            steps,
            plan_start: Instant::now(),
            event_tx,
        }
    }

    /// Record tool execution results, matching them to plan steps.
    ///
    /// For each success/failure, finds the first matching step by `tool_name` with
    /// non-terminal status, transitions it, records timing, and emits events.
    ///
    /// Returns matched outcomes for DB persistence (tracker doesn't do I/O).
    pub fn record_tool_results(
        &mut self,
        successes: &[String],
        failures: &[(String, String)],
        round: usize,
    ) -> Vec<MatchedOutcome> {
        let mut matched = Vec::new();
        let now = Utc::now();

        // Match successes first.
        for tool_name in successes {
            if let Some(idx) = self.find_matching_step(tool_name) {
                let tracked = &mut self.steps[idx];

                // Transition: Pending → Running → Completed (or Running → Completed).
                if tracked.status == TaskStatus::Pending {
                    tracked.status = TaskStatus::Running;
                    tracked.started_at = Some(now);
                }
                tracked.status = TaskStatus::Completed;
                tracked.finished_at = Some(now);
                tracked.duration_ms = tracked
                    .started_at
                    .map(|start| (now - start).num_milliseconds().max(0) as u64);
                tracked.round = Some(round);

                // Update the plan step's outcome.
                let outcome = StepOutcome::Success {
                    summary: format!("{tool_name} OK"),
                };
                tracked.step.outcome = Some(outcome.clone());
                self.plan.steps[idx].outcome = Some(outcome.clone());

                // Emit event.
                let _ = self.event_tx.send(DomainEvent::new(EventPayload::PlanStepCompleted {
                    plan_id: self.plan.plan_id,
                    step_index: idx,
                    outcome: "success".to_string(),
                }));

                matched.push(MatchedOutcome {
                    step_index: idx,
                    outcome: StepOutcome::Success {
                        summary: format!("{tool_name} OK"),
                    },
                });
            }
        }

        // Match failures.
        for (tool_name, error_msg) in failures {
            if let Some(idx) = self.find_matching_step(tool_name) {
                let tracked = &mut self.steps[idx];

                if tracked.status == TaskStatus::Pending {
                    tracked.status = TaskStatus::Running;
                    tracked.started_at = Some(now);
                }
                tracked.status = TaskStatus::Failed;
                tracked.finished_at = Some(now);
                tracked.duration_ms = tracked
                    .started_at
                    .map(|start| (now - start).num_milliseconds().max(0) as u64);
                tracked.round = Some(round);

                let outcome = StepOutcome::Failed {
                    error: error_msg.clone(),
                };
                tracked.step.outcome = Some(outcome.clone());
                self.plan.steps[idx].outcome = Some(outcome.clone());

                let _ = self.event_tx.send(DomainEvent::new(EventPayload::PlanStepCompleted {
                    plan_id: self.plan.plan_id,
                    step_index: idx,
                    outcome: "failed".to_string(),
                }));

                matched.push(MatchedOutcome {
                    step_index: idx,
                    outcome: StepOutcome::Failed {
                        error: error_msg.clone(),
                    },
                });
            }
        }

        matched
    }

    /// Index of the first non-terminal step (replaces `plan_step_index`).
    pub fn current_step(&self) -> usize {
        self.steps
            .iter()
            .position(|s| !s.status.is_terminal())
            .unwrap_or(self.steps.len())
    }

    /// Whether all steps are in a terminal state.
    pub fn is_complete(&self) -> bool {
        self.steps.iter().all(|s| s.status.is_terminal())
    }

    /// Returns a snapshot of the execution plan with current outcomes.
    pub fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    /// Returns `(completed_count, total_count, elapsed_ms)`.
    pub fn progress(&self) -> (usize, usize, u64) {
        let completed = self
            .steps
            .iter()
            .filter(|s| s.status.is_terminal())
            .count();
        let total = self.steps.len();
        let elapsed = self.plan_start.elapsed().as_millis() as u64;
        (completed, total, elapsed)
    }

    /// Access tracked steps for rendering with timing data.
    pub fn tracked_steps(&self) -> &[TrackedStep] {
        &self.steps
    }

    /// Reset with a new plan after replanning. Already-completed step timing is preserved
    /// in the old tracker; the new plan starts fresh.
    pub fn reset_plan(&mut self, new_plan: ExecutionPlan) {
        let new_steps = new_plan
            .steps
            .iter()
            .map(|s| {
                // If the step already has an outcome (carried from replan), mark it terminal.
                let status = match &s.outcome {
                    Some(StepOutcome::Success { .. }) => TaskStatus::Completed,
                    Some(StepOutcome::Failed { .. }) => TaskStatus::Failed,
                    Some(StepOutcome::Skipped { .. }) => TaskStatus::Skipped,
                    None => TaskStatus::Pending,
                };
                TrackedStep {
                    step: s.clone(),
                    status,
                    started_at: None,
                    finished_at: None,
                    duration_ms: None,
                    tool_use_ids: Vec::new(),
                    round: None,
                    delegation: None,
                }
            })
            .collect();
        self.plan = new_plan;
        self.steps = new_steps;
        // plan_start is intentionally NOT reset — total elapsed covers the full session.
    }

    /// Export execution timeline for logging/serialization.
    #[allow(dead_code)]
    pub fn to_timeline(&self) -> ExecutionTimeline {
        ExecutionTimeline {
            plan_id: self.plan.plan_id,
            goal: self.plan.goal.clone(),
            total_elapsed_ms: self.plan_start.elapsed().as_millis() as u64,
            completed_steps: self.steps.iter().filter(|s| s.status == TaskStatus::Completed).count(),
            total_steps: self.steps.len(),
            steps: self
                .steps
                .iter()
                .enumerate()
                .map(|(i, ts)| TimelineEntry {
                    index: i,
                    description: ts.step.description.clone(),
                    tool_name: ts.step.tool_name.clone(),
                    status: ts.status,
                    started_at: ts.started_at.map(|t| t.to_rfc3339()),
                    finished_at: ts.finished_at.map(|t| t.to_rfc3339()),
                    duration_ms: ts.duration_ms,
                    round: ts.round,
                    delegated_to: ts.delegation.as_ref().map(|d| d.agent_type.clone()),
                    sub_agent_task_id: ts
                        .delegation
                        .as_ref()
                        .map(|d| d.task_id.to_string()),
                })
                .collect(),
        }
    }

    /// Serialize timeline to JSON value.
    #[allow(dead_code)]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self.to_timeline()).unwrap_or_default()
    }

    /// Record delegation of a step to a sub-agent.
    ///
    /// Sets the delegation metadata and transitions the step to `Running`.
    pub fn mark_delegated(&mut self, step_index: usize, task_id: Uuid, agent_type: &str) {
        if let Some(tracked) = self.steps.get_mut(step_index) {
            tracked.delegation = Some(cuervo_core::traits::DelegationInfo {
                task_id,
                agent_type: agent_type.to_string(),
                delegated: true,
            });
            if tracked.status == TaskStatus::Pending {
                tracked.status = TaskStatus::Running;
                tracked.started_at = Some(Utc::now());
            }
        }
    }

    /// Record results from orchestrator execution, matching by task_id.
    ///
    /// Returns matched outcomes for DB persistence.
    pub fn record_delegation_results(
        &mut self,
        results: &[cuervo_core::types::SubAgentResult],
        round: usize,
    ) -> Vec<MatchedOutcome> {
        let now = Utc::now();
        let mut matched = Vec::new();

        for result in results {
            // Find the step delegated to this task_id.
            let idx = self.steps.iter().position(|s| {
                s.delegation
                    .as_ref()
                    .map(|d| d.task_id == result.task_id)
                    .unwrap_or(false)
                    && !s.status.is_terminal()
            });

            if let Some(idx) = idx {
                let tracked = &mut self.steps[idx];

                if result.success {
                    tracked.status = TaskStatus::Completed;
                    let outcome = StepOutcome::Success {
                        summary: format!("delegated: {}", result.agent_result.summary),
                    };
                    tracked.step.outcome = Some(outcome.clone());
                    self.plan.steps[idx].outcome = Some(outcome.clone());
                    matched.push(MatchedOutcome {
                        step_index: idx,
                        outcome: StepOutcome::Success {
                            summary: result.agent_result.summary.clone(),
                        },
                    });
                } else {
                    tracked.status = TaskStatus::Failed;
                    let error_msg = result
                        .error
                        .clone()
                        .unwrap_or_else(|| "sub-agent failed".into());
                    let outcome = StepOutcome::Failed {
                        error: error_msg.clone(),
                    };
                    tracked.step.outcome = Some(outcome.clone());
                    self.plan.steps[idx].outcome = Some(outcome.clone());
                    matched.push(MatchedOutcome {
                        step_index: idx,
                        outcome: StepOutcome::Failed { error: error_msg },
                    });
                }

                tracked.finished_at = Some(now);
                tracked.duration_ms = tracked
                    .started_at
                    .map(|start| (now - start).num_milliseconds().max(0) as u64);
                tracked.round = Some(round);

                let _ =
                    self.event_tx
                        .send(DomainEvent::new(EventPayload::PlanStepCompleted {
                            plan_id: self.plan.plan_id,
                            step_index: idx,
                            outcome: if result.success { "success" } else { "failed" }
                                .to_string(),
                        }));
            }
        }

        matched
    }

    /// Get steps that are delegated and still running (for progress display).
    #[allow(dead_code)]
    pub fn delegated_running(&self) -> Vec<(usize, &TrackedStep)> {
        self.steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.delegation.is_some() && s.status == TaskStatus::Running)
            .collect()
    }

    // ── Private helpers ──

    fn find_matching_step(&self, tool_name: &str) -> Option<usize> {
        self.steps.iter().position(|ts| {
            ts.step.tool_name.as_deref() == Some(tool_name) && !ts.status.is_terminal()
        })
    }
}

// ── Timeline types ──

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExecutionTimeline {
    pub plan_id: Uuid,
    pub goal: String,
    pub total_elapsed_ms: u64,
    pub completed_steps: usize,
    pub total_steps: usize,
    pub steps: Vec<TimelineEntry>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TimelineEntry {
    pub index: usize,
    pub description: String,
    pub tool_name: Option<String>,
    pub status: TaskStatus,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub round: Option<usize>,
    /// Agent type if this step was delegated to a sub-agent.
    #[serde(default)]
    pub delegated_to: Option<String>,
    /// Sub-agent task ID if this step was delegated.
    #[serde(default)]
    pub sub_agent_task_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::traits::{ExecutionPlan, PlanStep};

    fn make_plan(steps: Vec<PlanStep>) -> ExecutionPlan {
        ExecutionPlan {
            goal: "Test goal".into(),
            steps,
            requires_confirmation: false,
            plan_id: Uuid::nil(),
            replan_count: 0,
            parent_plan_id: None,
        }
    }

    fn make_step(name: &str, tool: &str) -> PlanStep {
        PlanStep {
            description: name.into(),
            tool_name: Some(tool.into()),
            parallel: false,
            confidence: 0.9,
            expected_args: None,
            outcome: None,
        }
    }

    fn make_tracker(steps: Vec<PlanStep>) -> ExecutionTracker {
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        ExecutionTracker::new(make_plan(steps), tx)
    }

    #[test]
    fn new_tracker_all_pending() {
        let tracker = make_tracker(vec![
            make_step("Read file", "file_read"),
            make_step("Edit file", "file_edit"),
        ]);
        assert_eq!(tracker.current_step(), 0);
        assert!(!tracker.is_complete());
        assert_eq!(tracker.tracked_steps().len(), 2);
        for ts in tracker.tracked_steps() {
            assert_eq!(ts.status, TaskStatus::Pending);
        }
    }

    #[test]
    fn record_success_matches_first_step() {
        let mut tracker = make_tracker(vec![
            make_step("Read file", "file_read"),
            make_step("Edit file", "file_edit"),
        ]);
        let matched = tracker.record_tool_results(&["file_read".into()], &[], 1);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].step_index, 0);
        assert!(matches!(matched[0].outcome, StepOutcome::Success { .. }));
        assert_eq!(tracker.tracked_steps()[0].status, TaskStatus::Completed);
        assert_eq!(tracker.tracked_steps()[1].status, TaskStatus::Pending);
        assert_eq!(tracker.current_step(), 1);
    }

    #[test]
    fn record_failure_marks_step_failed() {
        let mut tracker = make_tracker(vec![make_step("Read file", "file_read")]);
        let matched = tracker.record_tool_results(
            &[],
            &[("file_read".into(), "not found".into())],
            1,
        );
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].step_index, 0);
        assert!(matches!(matched[0].outcome, StepOutcome::Failed { .. }));
        assert_eq!(tracker.tracked_steps()[0].status, TaskStatus::Failed);
    }

    #[test]
    fn no_match_returns_empty() {
        let mut tracker = make_tracker(vec![make_step("Run tests", "bash")]);
        let matched = tracker.record_tool_results(&["file_read".into()], &[], 1);
        assert!(matched.is_empty());
        assert_eq!(tracker.tracked_steps()[0].status, TaskStatus::Pending);
    }

    #[test]
    fn multi_same_tool_matches_sequentially() {
        let mut tracker = make_tracker(vec![
            make_step("Read first", "file_read"),
            make_step("Read second", "file_read"),
        ]);
        // First call matches step 0.
        let m1 = tracker.record_tool_results(&["file_read".into()], &[], 1);
        assert_eq!(m1.len(), 1);
        assert_eq!(m1[0].step_index, 0);
        assert_eq!(tracker.tracked_steps()[0].status, TaskStatus::Completed);
        assert_eq!(tracker.tracked_steps()[1].status, TaskStatus::Pending);
        // Second call matches step 1.
        let m2 = tracker.record_tool_results(&["file_read".into()], &[], 2);
        assert_eq!(m2.len(), 1);
        assert_eq!(m2[0].step_index, 1);
        assert_eq!(tracker.tracked_steps()[1].status, TaskStatus::Completed);
    }

    #[test]
    fn is_complete_when_all_terminal() {
        let mut tracker = make_tracker(vec![
            make_step("Read file", "file_read"),
            make_step("Edit file", "file_edit"),
        ]);
        tracker.record_tool_results(&["file_read".into(), "file_edit".into()], &[], 1);
        assert!(tracker.is_complete());
        assert_eq!(tracker.current_step(), 2); // Past all steps
    }

    #[test]
    fn current_step_skips_completed() {
        let mut tracker = make_tracker(vec![
            make_step("Step 1", "file_read"),
            make_step("Step 2", "bash"),
            make_step("Step 3", "file_edit"),
        ]);
        tracker.record_tool_results(&["file_read".into()], &[], 1);
        assert_eq!(tracker.current_step(), 1);
        tracker.record_tool_results(&["bash".into()], &[], 2);
        assert_eq!(tracker.current_step(), 2);
    }

    #[test]
    fn progress_tracks_completed_and_elapsed() {
        let mut tracker = make_tracker(vec![
            make_step("Step 1", "file_read"),
            make_step("Step 2", "bash"),
        ]);
        let (c, t, _e) = tracker.progress();
        assert_eq!(c, 0);
        assert_eq!(t, 2);
        tracker.record_tool_results(&["file_read".into()], &[], 1);
        let (c, t, e) = tracker.progress();
        assert_eq!(c, 1);
        assert_eq!(t, 2);
        let _ = e; // Elapsed is non-negative (u64)
    }

    #[test]
    fn plan_reflects_outcomes() {
        let mut tracker = make_tracker(vec![
            make_step("Read file", "file_read"),
            make_step("Edit file", "file_edit"),
        ]);
        tracker.record_tool_results(&["file_read".into()], &[], 1);
        let plan = tracker.plan();
        assert!(matches!(plan.steps[0].outcome, Some(StepOutcome::Success { .. })));
        assert!(plan.steps[1].outcome.is_none());
    }

    #[test]
    fn timeline_export() {
        let mut tracker = make_tracker(vec![
            make_step("Read file", "file_read"),
            make_step("Edit file", "file_edit"),
        ]);
        tracker.record_tool_results(&["file_read".into()], &[], 1);
        let tl = tracker.to_timeline();
        assert_eq!(tl.goal, "Test goal");
        assert_eq!(tl.completed_steps, 1);
        assert_eq!(tl.total_steps, 2);
        assert_eq!(tl.steps.len(), 2);
        assert_eq!(tl.steps[0].status, TaskStatus::Completed);
        assert_eq!(tl.steps[0].round, Some(1));
        assert!(tl.steps[0].started_at.is_some());
        assert!(tl.steps[0].finished_at.is_some());
        assert_eq!(tl.steps[1].status, TaskStatus::Pending);
    }

    #[test]
    fn json_export() {
        let mut tracker = make_tracker(vec![make_step("Read file", "file_read")]);
        tracker.record_tool_results(&["file_read".into()], &[], 1);
        let json = tracker.to_json();
        assert_eq!(json["completed_steps"], 1);
        assert_eq!(json["total_steps"], 1);
        assert_eq!(json["steps"][0]["status"], "Completed");
    }

    #[test]
    fn reset_plan_replaces_steps() {
        let mut tracker = make_tracker(vec![
            make_step("Old step", "file_read"),
        ]);
        tracker.record_tool_results(&["file_read".into()], &[], 1);
        assert!(tracker.is_complete());

        let new_plan = make_plan(vec![
            make_step("New step 1", "bash"),
            make_step("New step 2", "file_edit"),
        ]);
        tracker.reset_plan(new_plan);
        assert!(!tracker.is_complete());
        assert_eq!(tracker.current_step(), 0);
        assert_eq!(tracker.tracked_steps().len(), 2);
        assert_eq!(tracker.plan().steps.len(), 2);
    }

    #[test]
    fn reset_plan_preserves_pre_set_outcomes() {
        let mut steps = vec![make_step("Already done", "file_read")];
        steps[0].outcome = Some(StepOutcome::Success {
            summary: "pre-done".into(),
        });
        let plan = make_plan(steps);
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let mut tracker = ExecutionTracker::new(make_plan(vec![make_step("Old", "bash")]), tx);
        tracker.reset_plan(plan);
        // Step with pre-set outcome should be Completed.
        assert_eq!(tracker.tracked_steps()[0].status, TaskStatus::Completed);
        assert!(tracker.is_complete());
    }

    #[test]
    fn event_emission_on_success() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        let plan = make_plan(vec![make_step("Read file", "file_read")]);
        let mut tracker = ExecutionTracker::new(plan, tx);
        tracker.record_tool_results(&["file_read".into()], &[], 1);
        let event = rx.try_recv().unwrap();
        assert!(matches!(
            event.payload,
            EventPayload::PlanStepCompleted {
                step_index: 0,
                ref outcome,
                ..
            } if outcome == "success"
        ));
    }

    #[test]
    fn event_emission_on_failure() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        let plan = make_plan(vec![make_step("Read file", "file_read")]);
        let mut tracker = ExecutionTracker::new(plan, tx);
        tracker.record_tool_results(&[], &[("file_read".into(), "not found".into())], 1);
        let event = rx.try_recv().unwrap();
        assert!(matches!(
            event.payload,
            EventPayload::PlanStepCompleted {
                step_index: 0,
                ref outcome,
                ..
            } if outcome == "failed"
        ));
    }

    #[test]
    fn mixed_success_and_failure() {
        let mut tracker = make_tracker(vec![
            make_step("Read file", "file_read"),
            make_step("Run tests", "bash"),
        ]);
        let matched = tracker.record_tool_results(
            &["file_read".into()],
            &[("bash".into(), "exit 1".into())],
            1,
        );
        assert_eq!(matched.len(), 2);
        assert_eq!(tracker.tracked_steps()[0].status, TaskStatus::Completed);
        assert_eq!(tracker.tracked_steps()[1].status, TaskStatus::Failed);
        assert!(tracker.is_complete());
    }

    #[test]
    fn timing_recorded_on_completion() {
        let mut tracker = make_tracker(vec![make_step("Read file", "file_read")]);
        tracker.record_tool_results(&["file_read".into()], &[], 1);
        let ts = &tracker.tracked_steps()[0];
        assert!(ts.started_at.is_some());
        assert!(ts.finished_at.is_some());
        assert!(ts.duration_ms.is_some());
        assert_eq!(ts.round, Some(1));
    }

    #[test]
    fn empty_plan_is_complete() {
        let tracker = make_tracker(vec![]);
        assert!(tracker.is_complete());
        assert_eq!(tracker.current_step(), 0);
        let (c, t, _) = tracker.progress();
        assert_eq!(c, 0);
        assert_eq!(t, 0);
    }

    #[test]
    fn step_without_tool_name_never_matches() {
        let step = PlanStep {
            description: "Synthesize response".into(),
            tool_name: None,
            parallel: false,
            confidence: 1.0,
            expected_args: None,
            outcome: None,
        };
        let mut tracker = make_tracker(vec![step]);
        let matched = tracker.record_tool_results(&["file_read".into()], &[], 1);
        assert!(matched.is_empty());
        assert_eq!(tracker.tracked_steps()[0].status, TaskStatus::Pending);
    }

    #[test]
    fn already_completed_step_not_rematched() {
        let mut tracker = make_tracker(vec![make_step("Read file", "file_read")]);
        tracker.record_tool_results(&["file_read".into()], &[], 1);
        // Second call should not rematch the already-completed step.
        let matched = tracker.record_tool_results(&["file_read".into()], &[], 2);
        assert!(matched.is_empty());
    }

    #[test]
    fn timeline_serde_roundtrip() {
        let mut tracker = make_tracker(vec![make_step("Read file", "file_read")]);
        tracker.record_tool_results(&["file_read".into()], &[], 1);
        let tl = tracker.to_timeline();
        let json = serde_json::to_string(&tl).unwrap();
        let back: ExecutionTimeline = serde_json::from_str(&json).unwrap();
        assert_eq!(back.completed_steps, 1);
        assert_eq!(back.total_steps, 1);
        assert_eq!(back.steps[0].status, TaskStatus::Completed);
    }

    // ── Delegation tests ──

    #[test]
    fn mark_delegated_transitions_to_running() {
        let mut tracker = make_tracker(vec![
            make_step("Read file", "file_read"),
            make_step("Edit file", "file_edit"),
        ]);
        let task_id = Uuid::new_v4();
        tracker.mark_delegated(0, task_id, "Coder");

        assert_eq!(tracker.tracked_steps()[0].status, TaskStatus::Running);
        assert!(tracker.tracked_steps()[0].started_at.is_some());
        // Second step remains pending.
        assert_eq!(tracker.tracked_steps()[1].status, TaskStatus::Pending);
    }

    #[test]
    fn mark_delegated_sets_info() {
        let mut tracker = make_tracker(vec![make_step("Read file", "file_read")]);
        let task_id = Uuid::new_v4();
        tracker.mark_delegated(0, task_id, "Coder");

        let d = tracker.tracked_steps()[0].delegation.as_ref().unwrap();
        assert_eq!(d.task_id, task_id);
        assert_eq!(d.agent_type, "Coder");
        assert!(d.delegated);
    }

    #[test]
    fn record_delegation_results_success() {
        let mut tracker = make_tracker(vec![
            make_step("Read file", "file_read"),
            make_step("Edit file", "file_edit"),
        ]);
        let task_id = Uuid::new_v4();
        tracker.mark_delegated(0, task_id, "Coder");

        let result = cuervo_core::types::SubAgentResult {
            task_id,
            success: true,
            output_text: "Done".into(),
            agent_result: cuervo_core::types::AgentResult {
                success: true,
                summary: "File read successfully".into(),
                files_modified: vec![],
                tools_used: vec!["file_read".into()],
            },
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.001,
            latency_ms: 200,
            rounds: 1,
            error: None,
        };

        let matched = tracker.record_delegation_results(&[result], 1);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].step_index, 0);
        assert!(matches!(matched[0].outcome, StepOutcome::Success { .. }));
        assert_eq!(tracker.tracked_steps()[0].status, TaskStatus::Completed);
        assert!(tracker.tracked_steps()[0].finished_at.is_some());
        assert!(tracker.tracked_steps()[0].duration_ms.is_some());
        assert_eq!(tracker.tracked_steps()[0].round, Some(1));
    }

    #[test]
    fn record_delegation_results_failure() {
        let mut tracker = make_tracker(vec![make_step("Run tests", "bash")]);
        let task_id = Uuid::new_v4();
        tracker.mark_delegated(0, task_id, "Coder");

        let result = cuervo_core::types::SubAgentResult {
            task_id,
            success: false,
            output_text: "Error".into(),
            agent_result: cuervo_core::types::AgentResult {
                success: false,
                summary: "Tests failed".into(),
                files_modified: vec![],
                tools_used: vec!["bash".into()],
            },
            input_tokens: 50,
            output_tokens: 20,
            cost_usd: 0.0005,
            latency_ms: 100,
            rounds: 1,
            error: Some("exit code 1".into()),
        };

        let matched = tracker.record_delegation_results(&[result], 1);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].step_index, 0);
        assert!(matches!(matched[0].outcome, StepOutcome::Failed { .. }));
        assert_eq!(tracker.tracked_steps()[0].status, TaskStatus::Failed);
    }

    #[test]
    fn record_delegation_results_no_match() {
        let mut tracker = make_tracker(vec![make_step("Read file", "file_read")]);
        // No delegation was marked — task_id won't match.
        let result = cuervo_core::types::SubAgentResult {
            task_id: Uuid::new_v4(),
            success: true,
            output_text: "Done".into(),
            agent_result: cuervo_core::types::AgentResult {
                success: true,
                summary: "ok".into(),
                files_modified: vec![],
                tools_used: vec![],
            },
            input_tokens: 10,
            output_tokens: 5,
            cost_usd: 0.0,
            latency_ms: 50,
            rounds: 1,
            error: None,
        };

        let matched = tracker.record_delegation_results(&[result], 1);
        assert!(matched.is_empty());
        assert_eq!(tracker.tracked_steps()[0].status, TaskStatus::Pending);
    }

    #[test]
    fn mixed_inline_and_delegated() {
        let mut tracker = make_tracker(vec![
            make_step("Read file", "file_read"),
            make_step("Edit file", "file_edit"),
            make_step("Run tests", "bash"),
        ]);

        // Step 0: inline execution.
        tracker.record_tool_results(&["file_read".into()], &[], 1);
        assert_eq!(tracker.tracked_steps()[0].status, TaskStatus::Completed);
        assert!(tracker.tracked_steps()[0].delegation.is_none());

        // Steps 1, 2: delegated.
        let task_id_1 = Uuid::new_v4();
        let task_id_2 = Uuid::new_v4();
        tracker.mark_delegated(1, task_id_1, "Coder");
        tracker.mark_delegated(2, task_id_2, "Coder");

        let results = vec![
            cuervo_core::types::SubAgentResult {
                task_id: task_id_1,
                success: true,
                output_text: "Edited".into(),
                agent_result: cuervo_core::types::AgentResult {
                    success: true,
                    summary: "File edited".into(),
                    files_modified: vec!["main.rs".into()],
                    tools_used: vec!["file_edit".into()],
                },
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: 0.001,
                latency_ms: 300,
                rounds: 1,
                error: None,
            },
            cuervo_core::types::SubAgentResult {
                task_id: task_id_2,
                success: true,
                output_text: "Tests pass".into(),
                agent_result: cuervo_core::types::AgentResult {
                    success: true,
                    summary: "All tests pass".into(),
                    files_modified: vec![],
                    tools_used: vec!["bash".into()],
                },
                input_tokens: 80,
                output_tokens: 30,
                cost_usd: 0.0008,
                latency_ms: 500,
                rounds: 1,
                error: None,
            },
        ];

        let matched = tracker.record_delegation_results(&results, 2);
        assert_eq!(matched.len(), 2);
        assert!(tracker.is_complete());
    }

    #[test]
    fn delegated_running_filter() {
        let mut tracker = make_tracker(vec![
            make_step("Read file", "file_read"),
            make_step("Edit file", "file_edit"),
            make_step("Run tests", "bash"),
        ]);

        tracker.mark_delegated(0, Uuid::new_v4(), "Coder");
        tracker.mark_delegated(1, Uuid::new_v4(), "Coder");

        let running = tracker.delegated_running();
        assert_eq!(running.len(), 2);

        // Complete step 0.
        let task_id_0 = tracker.tracked_steps()[0]
            .delegation
            .as_ref()
            .unwrap()
            .task_id;
        let result = cuervo_core::types::SubAgentResult {
            task_id: task_id_0,
            success: true,
            output_text: "Done".into(),
            agent_result: cuervo_core::types::AgentResult {
                success: true,
                summary: "ok".into(),
                files_modified: vec![],
                tools_used: vec![],
            },
            input_tokens: 10,
            output_tokens: 5,
            cost_usd: 0.0,
            latency_ms: 50,
            rounds: 1,
            error: None,
        };
        tracker.record_delegation_results(&[result], 1);

        let running = tracker.delegated_running();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].0, 1); // Only step 1 still running.
    }

    #[test]
    fn timeline_includes_delegation_info() {
        let mut tracker = make_tracker(vec![
            make_step("Read file", "file_read"),
            make_step("Edit file", "file_edit"),
        ]);
        let task_id = Uuid::new_v4();
        tracker.mark_delegated(0, task_id, "Coder");

        let tl = tracker.to_timeline();
        assert_eq!(tl.steps[0].delegated_to.as_deref(), Some("Coder"));
        assert_eq!(
            tl.steps[0].sub_agent_task_id.as_deref(),
            Some(task_id.to_string().as_str())
        );
        // Non-delegated step has no delegation info.
        assert!(tl.steps[1].delegated_to.is_none());
        assert!(tl.steps[1].sub_agent_task_id.is_none());
    }
}
