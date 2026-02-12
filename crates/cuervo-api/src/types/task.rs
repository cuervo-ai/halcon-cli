use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use super::agent::{AgentKind, BudgetSpec, UsageInfo};

/// Status of a task execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Specification for selecting which agent handles a task node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSelectorSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_capability: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_kind: Option<AgentKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_name: Option<String>,
}

/// A single node in a task DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNodeSpec {
    pub task_id: Uuid,
    pub instruction: String,
    pub agent_selector: AgentSelectorSpec,
    #[serde(default)]
    pub depends_on: Vec<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetSpec>,
    #[serde(default)]
    pub context_keys: Vec<String>,
}

/// Request to submit a task DAG for execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTaskRequest {
    pub nodes: Vec<TaskNodeSpec>,
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
}

/// Response after submitting a task DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTaskResponse {
    pub execution_id: Uuid,
    pub node_count: usize,
    pub wave_count: usize,
}

/// Result for a single task node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNodeResult {
    pub task_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub status: TaskStatus,
    pub output: Option<String>,
    pub usage: Option<UsageInfo>,
    pub error: Option<String>,
}

/// Full task execution status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecution {
    pub id: Uuid,
    pub status: TaskStatus,
    pub wave_count: usize,
    pub node_results: Vec<TaskNodeResult>,
    pub submitted_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub total_usage: UsageInfo,
}

/// Progress event streamed over WebSocket during task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum TaskProgressEvent {
    WaveStarted {
        execution_id: Uuid,
        wave: usize,
        node_ids: Vec<Uuid>,
    },
    NodeStarted {
        execution_id: Uuid,
        node_id: Uuid,
        agent_id: Uuid,
    },
    NodeCompleted {
        execution_id: Uuid,
        node_id: Uuid,
        success: bool,
        usage: UsageInfo,
    },
    NodeFailed {
        execution_id: Uuid,
        node_id: Uuid,
        error: String,
    },
    ExecutionCompleted {
        execution_id: Uuid,
        success: bool,
        total_usage: UsageInfo,
    },
}

/// Query parameters for listing task executions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListTasksQuery {
    pub status: Option<TaskStatus>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}
