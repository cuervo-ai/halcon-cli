//! Task bridge — connects the structured task framework to the existing
//! ExecutionTracker + DelegationRouter + Orchestrator pipeline.
//!
//! The bridge lifts PlanSteps into StructuredTasks, syncs execution outcomes,
//! and optionally persists tasks to SQLite for cross-session resume.

use uuid::Uuid;

use cuervo_core::traits::ExecutionPlan;
use cuervo_core::types::{
    ArtifactType, RetryPolicy, StructuredTaskStatus, TaskArtifact, TaskFrameworkConfig,
};

use super::artifact_store::ArtifactStore;
use super::execution_tracker::ExecutionTracker;
use super::provenance_tracker::ProvenanceTracker;
use super::task_backlog::TaskBacklog;
use super::task_scheduler::TaskScheduler;

/// Bridge between the agent loop and the structured task framework.
pub(crate) struct TaskBridge {
    backlog: TaskBacklog,
    scheduler: TaskScheduler,
    pub(crate) artifacts: ArtifactStore,
    pub(crate) provenance: ProvenanceTracker,
    config: TaskFrameworkConfig,
}

impl TaskBridge {
    pub fn new(config: &TaskFrameworkConfig) -> Self {
        Self {
            backlog: TaskBacklog::new(),
            scheduler: TaskScheduler::new(10),
            artifacts: ArtifactStore::new(),
            provenance: ProvenanceTracker::new(),
            config: config.clone(),
        }
    }

    /// Lift PlanSteps into StructuredTasks. Returns (step_index, task_id) pairs.
    pub fn ingest_plan(&mut self, plan: &ExecutionPlan) -> Vec<(usize, Uuid)> {
        let retry_policy = RetryPolicy {
            max_retries: self.config.default_max_retries,
            base_delay_ms: self.config.default_retry_base_ms,
            ..Default::default()
        };
        self.backlog.add_from_plan(plan, &retry_policy)
    }

    /// Sync ExecutionTracker outcomes into structured task provenance.
    pub fn sync_from_tracker(
        &mut self,
        tracker: &ExecutionTracker,
        model: &str,
        provider: &str,
        session_id: Option<Uuid>,
    ) {
        for tracked in tracker.tracked_steps() {
            // Find corresponding structured task by matching step description + tool_name.
            let matching_task_id = self.backlog.tasks().find(|t| {
                t.tool_name.as_deref() == tracked.step.tool_name.as_deref()
                    && t.title == tracked.step.description
            }).map(|t| t.task_id);

            let Some(task_id) = matching_task_id else {
                continue;
            };

            match tracked.status {
                cuervo_core::traits::TaskStatus::Completed => {
                    // Begin provenance if not already tracking.
                    self.provenance.begin(task_id, session_id);
                    self.provenance.record_model(task_id, model, provider);
                    if let Some(tool) = &tracked.step.tool_name {
                        self.provenance.record_tool(task_id, tool);
                    }
                    if let Some(round) = tracked.round {
                        let prov = self.provenance.finalize(task_id, Some(round));
                        // Transition: Ready→Running→Completed.
                        let task = self.backlog.get_mut(task_id);
                        if let Some(task) = task {
                            if task.status == StructuredTaskStatus::Ready {
                                task.status = StructuredTaskStatus::Running;
                                task.started_at = tracked.started_at;
                            }
                            if task.status == StructuredTaskStatus::Running {
                                task.status = StructuredTaskStatus::Completed;
                                task.finished_at = tracked.finished_at;
                                task.duration_ms = tracked.duration_ms;
                                task.provenance = prov;
                            }
                        }
                    }
                }
                cuervo_core::traits::TaskStatus::Failed => {
                    let error_msg = tracked.step.outcome.as_ref().map(|o| match o {
                        cuervo_core::traits::StepOutcome::Failed { error } => error.clone(),
                        _ => "unknown failure".to_string(),
                    }).unwrap_or_else(|| "unknown failure".to_string());

                    let task = self.backlog.get_mut(task_id);
                    if let Some(task) = task {
                        if task.status == StructuredTaskStatus::Ready {
                            task.status = StructuredTaskStatus::Running;
                            task.started_at = tracked.started_at;
                        }
                    }
                    let _ = self.backlog.fail_task(task_id, error_msg);
                }
                _ => {}
            }
        }
    }

    /// Record an artifact for a task.
    pub fn record_artifact(
        &mut self,
        task_id: Uuid,
        name: String,
        artifact_type: ArtifactType,
        content: &[u8],
        path: Option<String>,
    ) -> TaskArtifact {
        self.artifacts
            .store(task_id, name, artifact_type, content, path)
    }

    /// Access the backlog.
    pub fn backlog(&self) -> &TaskBacklog {
        &self.backlog
    }

    /// Access the backlog mutably.
    #[allow(dead_code)]
    pub fn backlog_mut(&mut self) -> &mut TaskBacklog {
        &mut self.backlog
    }

    /// Access the scheduler.
    #[allow(dead_code)]
    pub fn scheduler(&self) -> &TaskScheduler {
        &self.scheduler
    }

    /// Whether the framework is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Export all tasks as JSON for diagnostics.
    pub fn to_json(&self) -> serde_json::Value {
        let tasks: Vec<serde_json::Value> = self
            .backlog
            .tasks()
            .map(|t| serde_json::to_value(t).unwrap_or(serde_json::Value::Null))
            .collect();

        let (completed, failed, total) = self.backlog.progress();

        serde_json::json!({
            "total_tasks": total,
            "completed": completed,
            "failed": failed,
            "is_complete": self.backlog.is_complete(),
            "total_artifact_size": self.artifacts.total_size(),
            "tasks": tasks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::traits::{ExecutionPlan, PlanStep};

    fn test_config() -> TaskFrameworkConfig {
        TaskFrameworkConfig {
            enabled: true,
            persist_tasks: false,
            default_max_retries: 2,
            default_retry_base_ms: 500,
            resume_on_startup: false,
        }
    }

    fn test_plan() -> ExecutionPlan {
        ExecutionPlan {
            goal: "Fix the bug".into(),
            steps: vec![
                PlanStep {
                    description: "Read file".into(),
                    tool_name: Some("file_read".into()),
                    parallel: false,
                    confidence: 0.9,
                    expected_args: None,
                    outcome: None,
                },
                PlanStep {
                    description: "Edit file".into(),
                    tool_name: Some("file_edit".into()),
                    parallel: false,
                    confidence: 0.85,
                    expected_args: None,
                    outcome: None,
                },
            ],
            requires_confirmation: false,
            plan_id: Uuid::new_v4(),
            replan_count: 0,
            parent_plan_id: None,
        }
    }

    #[test]
    fn ingest_plan_creates_tasks() {
        let mut bridge = TaskBridge::new(&test_config());
        let plan = test_plan();
        let result = bridge.ingest_plan(&plan);

        assert_eq!(result.len(), 2);
        assert_eq!(bridge.backlog().len(), 2);

        let first = bridge.backlog().get(result[0].1).unwrap();
        assert_eq!(first.title, "Read file");
        assert_eq!(first.retry_policy.max_retries, 2);
    }

    #[test]
    fn record_artifact_produces_hash() {
        let mut bridge = TaskBridge::new(&test_config());
        let task_id = Uuid::new_v4();

        let artifact = bridge.record_artifact(
            task_id,
            "output.txt".into(),
            ArtifactType::ToolOutput,
            b"Hello, world!",
            Some("/tmp/output.txt".into()),
        );

        assert!(!artifact.content_hash.is_empty());
        assert_eq!(artifact.size_bytes, 13);
        assert!(bridge.artifacts.contains(&artifact.content_hash));
    }

    #[test]
    fn to_json_format() {
        let mut bridge = TaskBridge::new(&test_config());
        let plan = test_plan();
        bridge.ingest_plan(&plan);

        let json = bridge.to_json();
        assert_eq!(json["total_tasks"], 2);
        assert_eq!(json["completed"], 0);
        assert_eq!(json["failed"], 0);
        assert_eq!(json["is_complete"], false);
    }

    #[test]
    fn config_defaults() {
        let config = TaskFrameworkConfig::default();
        assert!(config.enabled);
        assert!(config.persist_tasks);
        assert_eq!(config.default_max_retries, 2);
        assert_eq!(config.default_retry_base_ms, 500);
        assert!(!config.resume_on_startup);
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = TaskFrameworkConfig {
            enabled: true,
            persist_tasks: false,
            default_max_retries: 5,
            default_retry_base_ms: 1000,
            resume_on_startup: true,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: TaskFrameworkConfig = serde_json::from_str(&json).unwrap();
        assert!(back.enabled);
        assert!(!back.persist_tasks);
        assert_eq!(back.default_max_retries, 5);
        assert!(back.resume_on_startup);
    }

    #[test]
    fn disabled_bridge_still_functional() {
        let config = TaskFrameworkConfig { enabled: false, ..Default::default() };
        let mut bridge = TaskBridge::new(&config);
        assert!(!bridge.is_enabled());

        // Can still ingest plans.
        let plan = test_plan();
        let result = bridge.ingest_plan(&plan);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn config_absent_defaults_correctly() {
        // Simulates loading TOML without [task_framework] section.
        let config = cuervo_core::types::AppConfig::default();
        assert!(config.task_framework.enabled);
        assert!(config.task_framework.persist_tasks);
        assert_eq!(config.task_framework.default_max_retries, 2);
    }
}
