//! Task backlog — manages a DAG of structured tasks with dependency tracking.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use cuervo_core::traits::ExecutionPlan;
use cuervo_core::types::{
    RetryPolicy, StructuredTask, StructuredTaskStatus, TaskArtifact, TaskProvenance,
};

/// Manages a collection of structured tasks with dependency resolution.
pub(crate) struct TaskBacklog {
    tasks: HashMap<Uuid, StructuredTask>,
    /// Reverse dependency index: task_id → set of tasks that depend on it.
    dependents: HashMap<Uuid, HashSet<Uuid>>,
    /// Insertion order for deterministic iteration.
    insertion_order: Vec<Uuid>,
}

impl TaskBacklog {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            dependents: HashMap::new(),
            insertion_order: Vec::new(),
        }
    }

    /// Add a task. Validates no duplicate ID, builds dependency graph,
    /// and auto-sets status to Ready (if no deps) or Blocked (if deps exist).
    pub fn add_task(&mut self, mut task: StructuredTask) -> Result<(), String> {
        if self.tasks.contains_key(&task.task_id) {
            return Err(format!("duplicate task ID: {}", task.task_id));
        }

        // Auto-set Ready/Blocked based on dependencies.
        let has_unsatisfied_deps = task.depends_on.iter().any(|dep_id| {
            self.tasks
                .get(dep_id)
                .map_or(true, |t| !t.status.is_terminal() || t.status == StructuredTaskStatus::Failed)
        });

        if task.status == StructuredTaskStatus::Pending {
            if has_unsatisfied_deps && !task.depends_on.is_empty() {
                task.status = StructuredTaskStatus::Blocked;
            } else {
                task.status = StructuredTaskStatus::Ready;
            }
        }

        // Build reverse dependency index.
        for dep_id in &task.depends_on {
            self.dependents
                .entry(*dep_id)
                .or_default()
                .insert(task.task_id);
        }

        let id = task.task_id;
        self.tasks.insert(id, task);
        self.insertion_order.push(id);
        Ok(())
    }

    /// Bulk-lift PlanSteps into StructuredTasks with sequential dependencies.
    /// Returns the list of (step_index, task_id) pairs.
    pub fn add_from_plan(
        &mut self,
        plan: &ExecutionPlan,
        retry_policy: &RetryPolicy,
    ) -> Vec<(usize, Uuid)> {
        let mut result = Vec::new();
        let mut prev_id: Option<Uuid> = None;

        for (i, step) in plan.steps.iter().enumerate() {
            let mut task = StructuredTask::from_plan_step(step, plan.plan_id, i, retry_policy);

            // Sequential dependency: each step depends on the previous (unless parallel).
            if !step.parallel {
                if let Some(prev) = prev_id {
                    task.depends_on.push(prev);
                }
            }

            let task_id = task.task_id;
            // Ignore errors (IDs are always fresh UUIDs).
            let _ = self.add_task(task);
            result.push((i, task_id));
            prev_id = Some(task_id);
        }

        result
    }

    /// Return the highest-priority Ready task.
    pub fn next_ready(&self) -> Option<&StructuredTask> {
        self.insertion_order
            .iter()
            .filter_map(|id| self.tasks.get(id))
            .filter(|t| t.status == StructuredTaskStatus::Ready)
            .max_by_key(|t| t.priority)
    }

    /// Return up to `max_concurrent` Ready tasks sorted by priority (descending).
    pub fn ready_wave(&self, max_concurrent: usize) -> Vec<&StructuredTask> {
        let mut ready: Vec<_> = self
            .insertion_order
            .iter()
            .filter_map(|id| self.tasks.get(id))
            .filter(|t| t.status == StructuredTaskStatus::Ready)
            .collect();
        ready.sort_by(|a, b| b.priority.cmp(&a.priority));
        ready.truncate(max_concurrent);
        ready
    }

    /// Validate and apply a status transition.
    pub fn transition(
        &mut self,
        task_id: Uuid,
        target: StructuredTaskStatus,
    ) -> Result<(), String> {
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| format!("task not found: {task_id}"))?;
        let new_status = task.status.transition_to(target)?;
        task.status = new_status;

        // If completed, recalculate dependents' readiness.
        if new_status == StructuredTaskStatus::Completed {
            self.recalculate_dependents(task_id);
        }

        Ok(())
    }

    /// Complete a task with provenance and artifacts.
    pub fn complete_task(
        &mut self,
        task_id: Uuid,
        provenance: Option<TaskProvenance>,
        artifacts: Vec<TaskArtifact>,
    ) -> Result<(), String> {
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| format!("task not found: {task_id}"))?;
        let new_status = task
            .status
            .transition_to(StructuredTaskStatus::Completed)?;
        task.status = new_status;
        task.finished_at = Some(chrono::Utc::now());
        if let Some(started) = task.started_at {
            task.duration_ms = Some(
                (chrono::Utc::now() - started).num_milliseconds().max(0) as u64,
            );
        }
        task.provenance = provenance;
        task.artifacts = artifacts;

        self.recalculate_dependents(task_id);
        Ok(())
    }

    /// Fail a task. If retries remain, transition to Retrying; otherwise Failed + cascade.
    /// Returns the list of task IDs affected by cascade (empty if retrying).
    pub fn fail_task(&mut self, task_id: Uuid, error: String) -> Result<Vec<Uuid>, String> {
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| format!("task not found: {task_id}"))?;

        task.error = Some(error);
        task.finished_at = Some(chrono::Utc::now());
        if let Some(started) = task.started_at {
            task.duration_ms = Some(
                (chrono::Utc::now() - started).num_milliseconds().max(0) as u64,
            );
        }

        // Always transition to Failed first (Running→Failed is valid).
        let failed_status = task.status.transition_to(StructuredTaskStatus::Failed)?;
        task.status = failed_status;

        if task.retry_count < task.retry_policy.max_retries {
            task.retry_count += 1;
            // Then Failed→Retrying (valid per FSM).
            let retry_status = task.status.transition_to(StructuredTaskStatus::Retrying)?;
            task.status = retry_status;
            Ok(Vec::new())
        } else {
            Ok(self.cascade_failure(task_id))
        }
    }

    /// Cascade failure: Blocked/Pending dependents → Skipped. Returns affected IDs.
    pub fn cascade_failure(&mut self, failed_id: Uuid) -> Vec<Uuid> {
        let mut affected = Vec::new();
        let dependent_ids: Vec<Uuid> = self
            .dependents
            .get(&failed_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();

        for dep_id in dependent_ids {
            if let Some(task) = self.tasks.get_mut(&dep_id) {
                if matches!(
                    task.status,
                    StructuredTaskStatus::Pending
                        | StructuredTaskStatus::Blocked
                        | StructuredTaskStatus::Ready
                ) {
                    task.status = StructuredTaskStatus::Skipped;
                    task.error = Some(format!("dependency {} failed", failed_id));
                    affected.push(dep_id);
                    // Recursively cascade.
                    let sub = self.cascade_failure(dep_id);
                    affected.extend(sub);
                }
            }
        }

        affected
    }

    /// Whether all tasks are in terminal states.
    pub fn is_complete(&self) -> bool {
        self.tasks.values().all(|t| t.status.is_terminal())
    }

    /// Progress: (completed_count, failed_count, total_count).
    pub fn progress(&self) -> (usize, usize, usize) {
        let total = self.tasks.len();
        let completed = self
            .tasks
            .values()
            .filter(|t| t.status == StructuredTaskStatus::Completed)
            .count();
        let failed = self
            .tasks
            .values()
            .filter(|t| t.status == StructuredTaskStatus::Failed)
            .count();
        (completed, failed, total)
    }

    /// Get a task by ID.
    pub fn get(&self, id: Uuid) -> Option<&StructuredTask> {
        self.tasks.get(&id)
    }

    /// Get a mutable task by ID.
    pub fn get_mut(&mut self, id: Uuid) -> Option<&mut StructuredTask> {
        self.tasks.get_mut(&id)
    }

    /// Iterate all tasks in insertion order.
    pub fn tasks(&self) -> impl Iterator<Item = &StructuredTask> {
        self.insertion_order
            .iter()
            .filter_map(move |id| self.tasks.get(id))
    }

    /// Total number of tasks.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Recalculate readiness for tasks that depend on the given task.
    fn recalculate_dependents(&mut self, completed_id: Uuid) {
        let dependent_ids: Vec<Uuid> = self
            .dependents
            .get(&completed_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();

        for dep_id in dependent_ids {
            if let Some(task) = self.tasks.get(&dep_id) {
                if task.status != StructuredTaskStatus::Blocked {
                    continue;
                }
                // Check if ALL dependencies are now satisfied (Completed).
                let all_satisfied = task.depends_on.iter().all(|d| {
                    self.tasks
                        .get(d)
                        .map_or(false, |t| t.status == StructuredTaskStatus::Completed)
                });
                if all_satisfied {
                    if let Some(task) = self.tasks.get_mut(&dep_id) {
                        // Blocked → Ready is a valid transition.
                        task.status = StructuredTaskStatus::Ready;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(title: &str, priority: u32) -> StructuredTask {
        StructuredTask {
            title: title.into(),
            description: title.into(),
            priority,
            ..Default::default()
        }
    }

    fn make_task_with_deps(title: &str, priority: u32, deps: Vec<Uuid>) -> StructuredTask {
        StructuredTask {
            title: title.into(),
            description: title.into(),
            priority,
            depends_on: deps,
            ..Default::default()
        }
    }

    #[test]
    fn add_task_and_get() {
        let mut backlog = TaskBacklog::new();
        let task = make_task("Task 1", 10);
        let id = task.task_id;
        backlog.add_task(task).unwrap();

        let got = backlog.get(id).unwrap();
        assert_eq!(got.title, "Task 1");
        assert_eq!(got.status, StructuredTaskStatus::Ready); // auto-promoted
    }

    #[test]
    fn duplicate_id_rejected() {
        let mut backlog = TaskBacklog::new();
        let task = make_task("Task 1", 10);
        let id = task.task_id;
        backlog.add_task(task).unwrap();

        let mut dup = make_task("Dup", 5);
        dup.task_id = id;
        assert!(backlog.add_task(dup).is_err());
    }

    #[test]
    fn insertion_order_preserved() {
        let mut backlog = TaskBacklog::new();
        let t1 = make_task("First", 1);
        let t2 = make_task("Second", 2);
        let t3 = make_task("Third", 3);
        let id1 = t1.task_id;
        let id2 = t2.task_id;
        let id3 = t3.task_id;
        backlog.add_task(t1).unwrap();
        backlog.add_task(t2).unwrap();
        backlog.add_task(t3).unwrap();

        let ids: Vec<Uuid> = backlog.tasks().map(|t| t.task_id).collect();
        assert_eq!(ids, vec![id1, id2, id3]);
    }

    #[test]
    fn add_from_plan_creates_tasks() {
        let mut backlog = TaskBacklog::new();
        let plan = ExecutionPlan {
            goal: "Test goal".into(),
            steps: vec![
                cuervo_core::traits::PlanStep {
                    description: "Step 1".into(),
                    tool_name: Some("file_read".into()),
                    parallel: false,
                    confidence: 0.9,
                    expected_args: None,
                    outcome: None,
                },
                cuervo_core::traits::PlanStep {
                    description: "Step 2".into(),
                    tool_name: Some("bash".into()),
                    parallel: false,
                    confidence: 0.8,
                    expected_args: None,
                    outcome: None,
                },
            ],
            requires_confirmation: false,
            plan_id: Uuid::new_v4(),
            replan_count: 0,
            parent_plan_id: None,
        };
        let policy = RetryPolicy::default();
        let result = backlog.add_from_plan(&plan, &policy);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, 0); // step_index 0
        assert_eq!(result[1].0, 1); // step_index 1
        assert_eq!(backlog.len(), 2);

        // First task should be Ready (no deps), second should be Blocked (depends on first).
        let first = backlog.get(result[0].1).unwrap();
        assert_eq!(first.status, StructuredTaskStatus::Ready);
        assert_eq!(first.plan_id, Some(plan.plan_id));
        assert_eq!(first.step_index, Some(0));

        let second = backlog.get(result[1].1).unwrap();
        assert_eq!(second.status, StructuredTaskStatus::Blocked);
        assert_eq!(second.depends_on, vec![result[0].1]);
    }

    #[test]
    fn next_ready_highest_priority() {
        let mut backlog = TaskBacklog::new();
        backlog.add_task(make_task("Low", 1)).unwrap();
        backlog.add_task(make_task("High", 10)).unwrap();
        backlog.add_task(make_task("Mid", 5)).unwrap();

        let next = backlog.next_ready().unwrap();
        assert_eq!(next.title, "High");
    }

    #[test]
    fn ready_wave_respects_max() {
        let mut backlog = TaskBacklog::new();
        for i in 0..5 {
            backlog.add_task(make_task(&format!("T{i}"), i as u32)).unwrap();
        }

        let wave = backlog.ready_wave(3);
        assert_eq!(wave.len(), 3);
        // Highest priority first.
        assert_eq!(wave[0].priority, 4);
        assert_eq!(wave[1].priority, 3);
        assert_eq!(wave[2].priority, 2);
    }

    #[test]
    fn transition_validates_fsm() {
        let mut backlog = TaskBacklog::new();
        let task = make_task("T", 5);
        let id = task.task_id;
        backlog.add_task(task).unwrap();

        // Ready → Running (valid)
        backlog.transition(id, StructuredTaskStatus::Running).unwrap();
        assert_eq!(backlog.get(id).unwrap().status, StructuredTaskStatus::Running);

        // Running → Ready (invalid)
        assert!(backlog.transition(id, StructuredTaskStatus::Ready).is_err());
    }

    #[test]
    fn complete_task_attaches_provenance() {
        let mut backlog = TaskBacklog::new();
        let task = make_task("T", 5);
        let id = task.task_id;
        backlog.add_task(task).unwrap();
        backlog.transition(id, StructuredTaskStatus::Running).unwrap();

        let prov = TaskProvenance {
            model: Some("gpt-4o".into()),
            ..Default::default()
        };
        backlog
            .complete_task(id, Some(prov), Vec::new())
            .unwrap();

        let t = backlog.get(id).unwrap();
        assert_eq!(t.status, StructuredTaskStatus::Completed);
        assert_eq!(t.provenance.as_ref().unwrap().model.as_deref(), Some("gpt-4o"));
        assert!(t.finished_at.is_some());
    }

    #[test]
    fn fail_task_within_retry_budget() {
        let mut backlog = TaskBacklog::new();
        let mut task = make_task("T", 5);
        task.retry_policy.max_retries = 2;
        let id = task.task_id;
        backlog.add_task(task).unwrap();
        backlog.transition(id, StructuredTaskStatus::Running).unwrap();

        let affected = backlog.fail_task(id, "timeout".into()).unwrap();
        assert!(affected.is_empty()); // No cascade when retrying.
        assert_eq!(backlog.get(id).unwrap().status, StructuredTaskStatus::Retrying);
        assert_eq!(backlog.get(id).unwrap().retry_count, 1);
    }

    #[test]
    fn fail_task_exhausted_retries() {
        let mut backlog = TaskBacklog::new();
        let mut task = make_task("T", 5);
        task.retry_policy.max_retries = 0; // No retries allowed.
        let id = task.task_id;
        backlog.add_task(task).unwrap();
        backlog.transition(id, StructuredTaskStatus::Running).unwrap();

        let affected = backlog.fail_task(id, "fatal".into()).unwrap();
        assert_eq!(backlog.get(id).unwrap().status, StructuredTaskStatus::Failed);
        // No dependents to cascade.
        assert!(affected.is_empty());
    }

    #[test]
    fn cascade_failure_propagates() {
        let mut backlog = TaskBacklog::new();
        let t1 = make_task("T1", 5);
        let id1 = t1.task_id;
        backlog.add_task(t1).unwrap();

        let t2 = make_task_with_deps("T2", 5, vec![id1]);
        let id2 = t2.task_id;
        backlog.add_task(t2).unwrap();

        let t3 = make_task_with_deps("T3", 5, vec![id2]);
        let id3 = t3.task_id;
        backlog.add_task(t3).unwrap();

        // Transition T1 to Running → Failed.
        backlog.transition(id1, StructuredTaskStatus::Running).unwrap();
        let mut task1 = backlog.get_mut(id1).unwrap();
        task1.retry_policy.max_retries = 0;

        let affected = backlog.fail_task(id1, "error".into()).unwrap();

        // T2 and T3 should be Skipped.
        assert!(affected.contains(&id2));
        assert!(affected.contains(&id3));
        assert_eq!(backlog.get(id2).unwrap().status, StructuredTaskStatus::Skipped);
        assert_eq!(backlog.get(id3).unwrap().status, StructuredTaskStatus::Skipped);
    }

    #[test]
    fn is_complete_and_progress() {
        let mut backlog = TaskBacklog::new();
        let t1 = make_task("T1", 5);
        let id1 = t1.task_id;
        backlog.add_task(t1).unwrap();

        assert!(!backlog.is_complete());
        assert_eq!(backlog.progress(), (0, 0, 1));

        backlog.transition(id1, StructuredTaskStatus::Running).unwrap();
        backlog
            .complete_task(id1, None, Vec::new())
            .unwrap();

        assert!(backlog.is_complete());
        assert_eq!(backlog.progress(), (1, 0, 1));
    }

    #[test]
    fn dependency_completion_unblocks() {
        let mut backlog = TaskBacklog::new();
        let t1 = make_task("T1", 5);
        let id1 = t1.task_id;
        backlog.add_task(t1).unwrap();

        let t2 = make_task_with_deps("T2", 5, vec![id1]);
        let id2 = t2.task_id;
        backlog.add_task(t2).unwrap();

        assert_eq!(backlog.get(id2).unwrap().status, StructuredTaskStatus::Blocked);

        // Complete T1 → T2 should become Ready.
        backlog.transition(id1, StructuredTaskStatus::Running).unwrap();
        backlog.complete_task(id1, None, Vec::new()).unwrap();

        assert_eq!(backlog.get(id2).unwrap().status, StructuredTaskStatus::Ready);
    }
}
