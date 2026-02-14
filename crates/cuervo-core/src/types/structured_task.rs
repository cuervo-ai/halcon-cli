//! Structured task model — formal, reproducible, scientifically-traceable task framework.
//!
//! Extends the existing `PlanStep → TrackedStep → ExecutionTracker` pipeline with
//! richer semantics: 9-state FSM, provenance tracking, artifact management, retry
//! policies, and cross-session resume capability.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::traits::PlanStep;

/// Extended FSM with 9 states for structured task lifecycle.
///
/// Transition table:
/// | From      | Valid targets                          |
/// |-----------|---------------------------------------|
/// | Pending   | Ready, Blocked, Cancelled             |
/// | Ready     | Running, Blocked, Skipped, Cancelled  |
/// | Blocked   | Ready, Cancelled, Skipped             |
/// | Running   | Completed, Failed, Cancelled          |
/// | Retrying  | Running, Failed, Cancelled            |
/// | Completed | *(terminal)*                          |
/// | Failed    | Retrying, *(terminal)*                |
/// | Skipped   | *(terminal)*                          |
/// | Cancelled | *(terminal)*                          |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StructuredTaskStatus {
    Pending,
    Ready,
    Blocked,
    Running,
    Retrying,
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

impl StructuredTaskStatus {
    /// Returns `true` if this status is terminal (no further transitions allowed).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Skipped | Self::Cancelled
        )
    }

    /// Returns `true` if the task is eligible for scheduling/execution.
    pub fn is_actionable(&self) -> bool {
        matches!(self, Self::Ready | Self::Retrying)
    }

    /// Attempt a state transition. Returns `Err` if the transition is invalid.
    pub fn transition_to(self, target: StructuredTaskStatus) -> Result<StructuredTaskStatus, String> {
        match (self, target) {
            // Pending → Ready | Blocked | Cancelled
            (Self::Pending, Self::Ready)
            | (Self::Pending, Self::Blocked)
            | (Self::Pending, Self::Cancelled) => Ok(target),

            // Ready → Running | Blocked | Skipped | Cancelled
            (Self::Ready, Self::Running)
            | (Self::Ready, Self::Blocked)
            | (Self::Ready, Self::Skipped)
            | (Self::Ready, Self::Cancelled) => Ok(target),

            // Blocked → Ready | Cancelled | Skipped
            (Self::Blocked, Self::Ready)
            | (Self::Blocked, Self::Cancelled)
            | (Self::Blocked, Self::Skipped) => Ok(target),

            // Running → Completed | Failed | Cancelled
            (Self::Running, Self::Completed)
            | (Self::Running, Self::Failed)
            | (Self::Running, Self::Cancelled) => Ok(target),

            // Retrying → Running | Failed | Cancelled
            (Self::Retrying, Self::Running)
            | (Self::Retrying, Self::Failed)
            | (Self::Retrying, Self::Cancelled) => Ok(target),

            // Failed → Retrying (if retries remain)
            (Self::Failed, Self::Retrying) => Ok(target),

            // Terminal → anything is invalid
            (from, to) if from.is_terminal() => {
                Err(format!("cannot transition from terminal state {from:?} to {to:?}"))
            }
            (from, to) => Err(format!("invalid transition from {from:?} to {to:?}")),
        }
    }
}

impl std::fmt::Display for StructuredTaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "Pending"),
            Self::Ready => write!(f, "Ready"),
            Self::Blocked => write!(f, "Blocked"),
            Self::Running => write!(f, "Running"),
            Self::Retrying => write!(f, "Retrying"),
            Self::Completed => write!(f, "Completed"),
            Self::Failed => write!(f, "Failed"),
            Self::Skipped => write!(f, "Skipped"),
            Self::Cancelled => write!(f, "Cancelled"),
        }
    }
}

/// Configurable retry policy for structured tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retries. 0 = no retries.
    pub max_retries: u32,
    /// Base delay in milliseconds for exponential backoff.
    pub base_delay_ms: u64,
    /// Maximum delay cap in milliseconds.
    pub max_delay_ms: u64,
    /// Backoff multiplier (delay *= multiplier on each retry).
    pub backoff_multiplier: f64,
    /// Whether the operation is safe to retry (idempotent).
    pub idempotent: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay_ms: 500,
            max_delay_ms: 30_000,
            backoff_multiplier: 2.0,
            idempotent: true,
        }
    }
}

/// A versioned output artifact produced by a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskArtifact {
    pub artifact_id: Uuid,
    pub name: String,
    pub artifact_type: ArtifactType,
    /// SHA-256 content hash for deduplication and integrity.
    pub content_hash: String,
    pub size_bytes: u64,
    pub path: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// Classification of artifact content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactType {
    File,
    ToolOutput,
    ModelResponse,
    Summary,
    Custom(String),
}

/// Execution lineage and provenance tracking for reproducibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProvenance {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub tools_used: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub context_hash: Option<String>,
    pub parent_task_id: Option<Uuid>,
    pub delegated_to: Option<String>,
    pub session_id: Option<Uuid>,
    pub round: Option<usize>,
}

impl Default for TaskProvenance {
    fn default() -> Self {
        Self {
            model: None,
            provider: None,
            tools_used: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            context_hash: None,
            parent_task_id: None,
            delegated_to: None,
            session_id: None,
            round: None,
        }
    }
}

/// A structured task with full lifecycle management.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredTask {
    pub task_id: Uuid,
    pub title: String,
    pub description: String,
    pub status: StructuredTaskStatus,
    /// Higher = more important (used for scheduling priority).
    pub priority: u32,
    /// Explicit DAG edges: task IDs this task depends on.
    pub depends_on: Vec<Uuid>,
    /// Expected inputs (JSON schema or values).
    pub inputs: serde_json::Value,
    /// Produced outputs (JSON values).
    pub outputs: serde_json::Value,
    /// Versioned output artifacts.
    pub artifacts: Vec<TaskArtifact>,
    /// Execution lineage.
    pub provenance: Option<TaskProvenance>,
    /// Retry configuration.
    pub retry_policy: RetryPolicy,
    /// Current retry attempt count.
    pub retry_count: u32,
    /// Classification tags.
    pub tags: Vec<String>,
    /// Maps to PlanStep.tool_name.
    pub tool_name: Option<String>,
    /// Expected tool arguments (hint from plan).
    pub expected_args: Option<serde_json::Value>,
    /// Error message (set on failure).
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    /// Session in which this task was created.
    pub session_id: Option<Uuid>,
    /// Plan that generated this task.
    pub plan_id: Option<Uuid>,
    /// Index within the originating plan.
    pub step_index: Option<usize>,
}

impl StructuredTask {
    /// Lift a `PlanStep` into a `StructuredTask` with enrichment.
    pub fn from_plan_step(
        step: &PlanStep,
        plan_id: Uuid,
        index: usize,
        retry: &RetryPolicy,
    ) -> Self {
        Self {
            task_id: Uuid::new_v4(),
            title: step.description.clone(),
            description: step.description.clone(),
            status: StructuredTaskStatus::Pending,
            priority: ((1.0 - step.confidence) * 100.0) as u32, // lower confidence = higher priority attention
            depends_on: Vec::new(),
            inputs: serde_json::Value::Object(serde_json::Map::new()),
            outputs: serde_json::Value::Object(serde_json::Map::new()),
            artifacts: Vec::new(),
            provenance: None,
            retry_policy: retry.clone(),
            retry_count: 0,
            tags: Vec::new(),
            tool_name: step.tool_name.clone(),
            expected_args: step.expected_args.clone(),
            error: None,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            duration_ms: None,
            session_id: None,
            plan_id: Some(plan_id),
            step_index: Some(index),
        }
    }
}

impl Default for StructuredTask {
    fn default() -> Self {
        Self {
            task_id: Uuid::new_v4(),
            title: String::new(),
            description: String::new(),
            status: StructuredTaskStatus::Pending,
            priority: 0,
            depends_on: Vec::new(),
            inputs: serde_json::Value::Object(serde_json::Map::new()),
            outputs: serde_json::Value::Object(serde_json::Map::new()),
            artifacts: Vec::new(),
            provenance: None,
            retry_policy: RetryPolicy::default(),
            retry_count: 0,
            tags: Vec::new(),
            tool_name: None,
            expected_args: None,
            error: None,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            duration_ms: None,
            session_id: None,
            plan_id: None,
            step_index: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- FSM transition tests ---

    #[test]
    fn pending_to_ready() {
        assert_eq!(
            StructuredTaskStatus::Pending
                .transition_to(StructuredTaskStatus::Ready)
                .unwrap(),
            StructuredTaskStatus::Ready
        );
    }

    #[test]
    fn pending_to_blocked() {
        assert_eq!(
            StructuredTaskStatus::Pending
                .transition_to(StructuredTaskStatus::Blocked)
                .unwrap(),
            StructuredTaskStatus::Blocked
        );
    }

    #[test]
    fn pending_to_cancelled() {
        assert_eq!(
            StructuredTaskStatus::Pending
                .transition_to(StructuredTaskStatus::Cancelled)
                .unwrap(),
            StructuredTaskStatus::Cancelled
        );
    }

    #[test]
    fn ready_to_running() {
        assert_eq!(
            StructuredTaskStatus::Ready
                .transition_to(StructuredTaskStatus::Running)
                .unwrap(),
            StructuredTaskStatus::Running
        );
    }

    #[test]
    fn ready_to_blocked() {
        assert_eq!(
            StructuredTaskStatus::Ready
                .transition_to(StructuredTaskStatus::Blocked)
                .unwrap(),
            StructuredTaskStatus::Blocked
        );
    }

    #[test]
    fn ready_to_skipped() {
        assert_eq!(
            StructuredTaskStatus::Ready
                .transition_to(StructuredTaskStatus::Skipped)
                .unwrap(),
            StructuredTaskStatus::Skipped
        );
    }

    #[test]
    fn ready_to_cancelled() {
        assert_eq!(
            StructuredTaskStatus::Ready
                .transition_to(StructuredTaskStatus::Cancelled)
                .unwrap(),
            StructuredTaskStatus::Cancelled
        );
    }

    #[test]
    fn blocked_to_ready() {
        assert_eq!(
            StructuredTaskStatus::Blocked
                .transition_to(StructuredTaskStatus::Ready)
                .unwrap(),
            StructuredTaskStatus::Ready
        );
    }

    #[test]
    fn blocked_to_cancelled() {
        assert_eq!(
            StructuredTaskStatus::Blocked
                .transition_to(StructuredTaskStatus::Cancelled)
                .unwrap(),
            StructuredTaskStatus::Cancelled
        );
    }

    #[test]
    fn blocked_to_skipped() {
        assert_eq!(
            StructuredTaskStatus::Blocked
                .transition_to(StructuredTaskStatus::Skipped)
                .unwrap(),
            StructuredTaskStatus::Skipped
        );
    }

    #[test]
    fn running_to_completed() {
        assert_eq!(
            StructuredTaskStatus::Running
                .transition_to(StructuredTaskStatus::Completed)
                .unwrap(),
            StructuredTaskStatus::Completed
        );
    }

    #[test]
    fn running_to_failed() {
        assert_eq!(
            StructuredTaskStatus::Running
                .transition_to(StructuredTaskStatus::Failed)
                .unwrap(),
            StructuredTaskStatus::Failed
        );
    }

    #[test]
    fn running_to_cancelled() {
        assert_eq!(
            StructuredTaskStatus::Running
                .transition_to(StructuredTaskStatus::Cancelled)
                .unwrap(),
            StructuredTaskStatus::Cancelled
        );
    }

    #[test]
    fn retrying_to_running() {
        assert_eq!(
            StructuredTaskStatus::Retrying
                .transition_to(StructuredTaskStatus::Running)
                .unwrap(),
            StructuredTaskStatus::Running
        );
    }

    #[test]
    fn retrying_to_failed() {
        assert_eq!(
            StructuredTaskStatus::Retrying
                .transition_to(StructuredTaskStatus::Failed)
                .unwrap(),
            StructuredTaskStatus::Failed
        );
    }

    #[test]
    fn retrying_to_cancelled() {
        assert_eq!(
            StructuredTaskStatus::Retrying
                .transition_to(StructuredTaskStatus::Cancelled)
                .unwrap(),
            StructuredTaskStatus::Cancelled
        );
    }

    #[test]
    fn failed_to_retrying() {
        assert_eq!(
            StructuredTaskStatus::Failed
                .transition_to(StructuredTaskStatus::Retrying)
                .unwrap(),
            StructuredTaskStatus::Retrying
        );
    }

    // --- Invalid transitions ---

    #[test]
    fn terminal_states_reject_transitions() {
        for terminal in [
            StructuredTaskStatus::Completed,
            StructuredTaskStatus::Skipped,
            StructuredTaskStatus::Cancelled,
        ] {
            assert!(terminal.transition_to(StructuredTaskStatus::Running).is_err());
            assert!(terminal.transition_to(StructuredTaskStatus::Ready).is_err());
            assert!(terminal.transition_to(StructuredTaskStatus::Pending).is_err());
        }
    }

    #[test]
    fn failed_rejects_non_retrying_transitions() {
        assert!(StructuredTaskStatus::Failed
            .transition_to(StructuredTaskStatus::Running)
            .is_err());
        assert!(StructuredTaskStatus::Failed
            .transition_to(StructuredTaskStatus::Ready)
            .is_err());
        assert!(StructuredTaskStatus::Failed
            .transition_to(StructuredTaskStatus::Completed)
            .is_err());
    }

    #[test]
    fn running_to_ready_invalid() {
        assert!(StructuredTaskStatus::Running
            .transition_to(StructuredTaskStatus::Ready)
            .is_err());
    }

    #[test]
    fn pending_to_completed_invalid() {
        assert!(StructuredTaskStatus::Pending
            .transition_to(StructuredTaskStatus::Completed)
            .is_err());
    }

    // --- is_terminal / is_actionable ---

    #[test]
    fn is_terminal_checks() {
        assert!(!StructuredTaskStatus::Pending.is_terminal());
        assert!(!StructuredTaskStatus::Ready.is_terminal());
        assert!(!StructuredTaskStatus::Blocked.is_terminal());
        assert!(!StructuredTaskStatus::Running.is_terminal());
        assert!(!StructuredTaskStatus::Retrying.is_terminal());
        assert!(StructuredTaskStatus::Completed.is_terminal());
        assert!(StructuredTaskStatus::Failed.is_terminal());
        assert!(StructuredTaskStatus::Skipped.is_terminal());
        assert!(StructuredTaskStatus::Cancelled.is_terminal());
    }

    #[test]
    fn is_actionable_checks() {
        assert!(StructuredTaskStatus::Ready.is_actionable());
        assert!(StructuredTaskStatus::Retrying.is_actionable());
        assert!(!StructuredTaskStatus::Pending.is_actionable());
        assert!(!StructuredTaskStatus::Blocked.is_actionable());
        assert!(!StructuredTaskStatus::Running.is_actionable());
        assert!(!StructuredTaskStatus::Completed.is_actionable());
    }

    // --- Serde roundtrips ---

    #[test]
    fn status_serde_roundtrip() {
        for status in [
            StructuredTaskStatus::Pending,
            StructuredTaskStatus::Ready,
            StructuredTaskStatus::Blocked,
            StructuredTaskStatus::Running,
            StructuredTaskStatus::Retrying,
            StructuredTaskStatus::Completed,
            StructuredTaskStatus::Failed,
            StructuredTaskStatus::Skipped,
            StructuredTaskStatus::Cancelled,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: StructuredTaskStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back);
        }
    }

    #[test]
    fn retry_policy_serde_roundtrip() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay_ms: 1000,
            max_delay_ms: 60_000,
            backoff_multiplier: 3.0,
            idempotent: false,
        };
        let json = serde_json::to_string(&policy).unwrap();
        let back: RetryPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_retries, 3);
        assert!(!back.idempotent);
        assert!((back.backoff_multiplier - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn task_artifact_serde_roundtrip() {
        let artifact = TaskArtifact {
            artifact_id: Uuid::new_v4(),
            name: "output.txt".into(),
            artifact_type: ArtifactType::File,
            content_hash: "abc123".into(),
            size_bytes: 1024,
            path: Some("/tmp/output.txt".into()),
            metadata: serde_json::json!({"key": "value"}),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&artifact).unwrap();
        let back: TaskArtifact = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "output.txt");
        assert_eq!(back.artifact_type, ArtifactType::File);
        assert_eq!(back.size_bytes, 1024);
    }

    #[test]
    fn task_provenance_serde_roundtrip() {
        let prov = TaskProvenance {
            model: Some("gpt-4o".into()),
            provider: Some("openai".into()),
            tools_used: vec!["file_read".into(), "bash".into()],
            input_tokens: 500,
            output_tokens: 200,
            cost_usd: 0.015,
            context_hash: Some("hash123".into()),
            parent_task_id: None,
            delegated_to: Some("Coder".into()),
            session_id: Some(Uuid::new_v4()),
            round: Some(3),
        };
        let json = serde_json::to_string(&prov).unwrap();
        let back: TaskProvenance = serde_json::from_str(&json).unwrap();
        assert_eq!(back.model.as_deref(), Some("gpt-4o"));
        assert_eq!(back.tools_used.len(), 2);
        assert_eq!(back.input_tokens, 500);
    }

    #[test]
    fn structured_task_serde_roundtrip() {
        let task = StructuredTask {
            title: "Read config file".into(),
            description: "Read the configuration file and parse it".into(),
            status: StructuredTaskStatus::Ready,
            priority: 10,
            tags: vec!["io".into(), "config".into()],
            tool_name: Some("file_read".into()),
            plan_id: Some(Uuid::new_v4()),
            step_index: Some(0),
            ..Default::default()
        };
        let json = serde_json::to_string(&task).unwrap();
        let back: StructuredTask = serde_json::from_str(&json).unwrap();
        assert_eq!(back.title, "Read config file");
        assert_eq!(back.status, StructuredTaskStatus::Ready);
        assert_eq!(back.priority, 10);
        assert_eq!(back.tags.len(), 2);
        assert_eq!(back.tool_name.as_deref(), Some("file_read"));
    }

    // --- Default impls ---

    #[test]
    fn retry_policy_defaults() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 2);
        assert_eq!(p.base_delay_ms, 500);
        assert_eq!(p.max_delay_ms, 30_000);
        assert!((p.backoff_multiplier - 2.0).abs() < f64::EPSILON);
        assert!(p.idempotent);
    }

    #[test]
    fn structured_task_defaults() {
        let t = StructuredTask::default();
        assert_eq!(t.status, StructuredTaskStatus::Pending);
        assert_eq!(t.priority, 0);
        assert!(t.depends_on.is_empty());
        assert!(t.artifacts.is_empty());
        assert!(t.provenance.is_none());
        assert_eq!(t.retry_count, 0);
    }

    #[test]
    fn provenance_defaults() {
        let p = TaskProvenance::default();
        assert!(p.model.is_none());
        assert!(p.provider.is_none());
        assert!(p.tools_used.is_empty());
        assert_eq!(p.input_tokens, 0);
        assert_eq!(p.cost_usd, 0.0);
    }

    // --- from_plan_step ---

    #[test]
    fn from_plan_step_preserves_fields() {
        let step = PlanStep {
            description: "Edit the main file".into(),
            tool_name: Some("file_edit".into()),
            parallel: false,
            confidence: 0.85,
            expected_args: Some(serde_json::json!({"path": "main.rs"})),
            outcome: None,
        };
        let plan_id = Uuid::new_v4();
        let policy = RetryPolicy {
            max_retries: 3,
            ..Default::default()
        };

        let task = StructuredTask::from_plan_step(&step, plan_id, 2, &policy);

        assert_eq!(task.title, "Edit the main file");
        assert_eq!(task.description, "Edit the main file");
        assert_eq!(task.status, StructuredTaskStatus::Pending);
        assert_eq!(task.tool_name.as_deref(), Some("file_edit"));
        assert_eq!(task.plan_id, Some(plan_id));
        assert_eq!(task.step_index, Some(2));
        assert_eq!(task.retry_policy.max_retries, 3);
        assert!(task.expected_args.is_some());
        // Priority inversely related to confidence
        assert_eq!(task.priority, 15); // (1.0 - 0.85) * 100 = 15
    }

    #[test]
    fn artifact_type_custom_serde() {
        let t = ArtifactType::Custom("diagram".into());
        let json = serde_json::to_string(&t).unwrap();
        let back: ArtifactType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ArtifactType::Custom("diagram".into()));
    }

    #[test]
    fn status_display() {
        assert_eq!(format!("{}", StructuredTaskStatus::Pending), "Pending");
        assert_eq!(format!("{}", StructuredTaskStatus::Running), "Running");
        assert_eq!(format!("{}", StructuredTaskStatus::Completed), "Completed");
        assert_eq!(format!("{}", StructuredTaskStatus::Retrying), "Retrying");
    }
}
