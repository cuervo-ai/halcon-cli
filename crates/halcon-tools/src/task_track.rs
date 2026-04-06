//! `task_track` tool: in-memory task list for agent self-management.
//!
//! Actions: add, update, list.
//! Constraint: only one task can be `in_progress` at a time.
//! ReadOnly permission — this manages internal agent state, not user files.

use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

/// A tracked agent task.
#[derive(Debug, Clone)]
struct AgentTask {
    content: String,
    status: TaskStatus,
    active_form: Option<String>,
}

/// Task status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

impl TaskStatus {
    fn as_str(&self) -> &str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::InProgress => "in_progress",
            TaskStatus::Completed => "completed",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(TaskStatus::Pending),
            "in_progress" => Some(TaskStatus::InProgress),
            "completed" => Some(TaskStatus::Completed),
            _ => None,
        }
    }

    fn badge(&self) -> &str {
        match self {
            TaskStatus::Pending => "[ ]",
            TaskStatus::InProgress => "[>]",
            TaskStatus::Completed => "[x]",
        }
    }
}

/// In-memory task tracker. Session-scoped, not persisted.
pub struct TaskTrackTool {
    tasks: Mutex<Vec<AgentTask>>,
}

impl Default for TaskTrackTool {
    fn default() -> Self {
        Self {
            tasks: Mutex::new(Vec::new()),
        }
    }
}

impl TaskTrackTool {
    pub fn new() -> Self {
        Self::default()
    }

    fn format_task_list(tasks: &[AgentTask]) -> String {
        if tasks.is_empty() {
            return "No tasks tracked.".to_string();
        }

        let mut out = String::new();
        for (i, task) in tasks.iter().enumerate() {
            out.push_str(&format!("{} {} {}\n", task.status.badge(), i, task.content));
            if let Some(ref af) = task.active_form {
                if task.status == TaskStatus::InProgress {
                    out.push_str(&format!("    → {af}\n"));
                }
            }
        }
        out
    }

    fn task_counts(tasks: &[AgentTask]) -> serde_json::Value {
        let pending = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Pending)
            .count();
        let in_progress = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::InProgress)
            .count();
        let completed = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count();
        json!({
            "task_count": tasks.len(),
            "pending": pending,
            "in_progress": in_progress,
            "completed": completed,
        })
    }
}

#[async_trait]
impl Tool for TaskTrackTool {
    fn name(&self) -> &str {
        "task_track"
    }

    fn description(&self) -> &str {
        "Track agent tasks: add, update status, or list all tasks. Only one task can be in_progress at a time."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let action = input.arguments["action"].as_str().ok_or_else(|| {
            HalconError::InvalidInput("task_track requires 'action' string".into())
        })?;

        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());

        match action {
            "add" => {
                let content = input.arguments["content"].as_str().ok_or_else(|| {
                    HalconError::InvalidInput("task_track 'add' requires 'content' string".into())
                })?;
                let active_form = input.arguments["active_form"]
                    .as_str()
                    .map(|s| s.to_string());

                tasks.push(AgentTask {
                    content: content.to_string(),
                    status: TaskStatus::Pending,
                    active_form,
                });

                let idx = tasks.len() - 1;
                let content_out = format!(
                    "Added task #{idx}: {content}\n\n{}",
                    Self::format_task_list(&tasks)
                );
                Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: content_out,
                    is_error: false,
                    metadata: Some(Self::task_counts(&tasks)),
                })
            }

            "update" => {
                let task_index = input.arguments["task_index"].as_u64().ok_or_else(|| {
                    HalconError::InvalidInput(
                        "task_track 'update' requires 'task_index' integer".into(),
                    )
                })? as usize;

                let status_str = input.arguments["status"].as_str().ok_or_else(|| {
                    HalconError::InvalidInput("task_track 'update' requires 'status' string".into())
                })?;

                let new_status = TaskStatus::from_str(status_str).ok_or_else(|| {
                    HalconError::InvalidInput(format!(
                        "invalid status '{status_str}': expected pending|in_progress|completed"
                    ))
                })?;

                if task_index >= tasks.len() {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: format!(
                            "task_track error: index {task_index} out of range (0..{})",
                            tasks.len()
                        ),
                        is_error: true,
                        metadata: None,
                    });
                }

                // Enforce only-one-in_progress constraint.
                if new_status == TaskStatus::InProgress {
                    let existing = tasks
                        .iter()
                        .enumerate()
                        .find(|(i, t)| *i != task_index && t.status == TaskStatus::InProgress);
                    if let Some((existing_idx, _)) = existing {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!(
                                "task_track error: task #{existing_idx} is already in_progress. \
                                 Complete or reset it before starting another."
                            ),
                            is_error: true,
                            metadata: None,
                        });
                    }
                }

                // Optionally update content.
                if let Some(new_content) = input.arguments["content"].as_str() {
                    tasks[task_index].content = new_content.to_string();
                }

                tasks[task_index].status = new_status;

                let content_out = format!(
                    "Updated task #{task_index} → {}\n\n{}",
                    new_status.as_str(),
                    Self::format_task_list(&tasks)
                );
                Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: content_out,
                    is_error: false,
                    metadata: Some(Self::task_counts(&tasks)),
                })
            }

            "list" => {
                let content_out = Self::format_task_list(&tasks);
                Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: content_out,
                    is_error: false,
                    metadata: Some(Self::task_counts(&tasks)),
                })
            }

            _ => Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!(
                    "task_track error: unknown action '{action}'. Expected: add|update|list"
                ),
                is_error: true,
                metadata: None,
            }),
        }
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "The action: 'add', 'update', or 'list'.",
                    "enum": ["add", "update", "list"]
                },
                "content": {
                    "type": "string",
                    "description": "Task description (required for 'add', optional for 'update')."
                },
                "task_index": {
                    "type": "integer",
                    "description": "Task index to update (required for 'update')."
                },
                "status": {
                    "type": "string",
                    "description": "New status (required for 'update'): pending|in_progress|completed.",
                    "enum": ["pending", "in_progress", "completed"]
                },
                "active_form": {
                    "type": "string",
                    "description": "Present-continuous form for spinner display (optional for 'add')."
                }
            },
            "required": ["action"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test".to_string(),
            arguments: args,
            working_directory: "/tmp".to_string(),
        }
    }

    #[tokio::test]
    async fn add_task() {
        let tool = TaskTrackTool::new();
        let out = tool
            .execute(make_input(
                json!({"action": "add", "content": "Fix the bug"}),
            ))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("Added task #0"));
        assert!(out.content.contains("Fix the bug"));
        let meta = out.metadata.unwrap();
        assert_eq!(meta["task_count"], 1);
        assert_eq!(meta["pending"], 1);
    }

    #[tokio::test]
    async fn add_with_active_form() {
        let tool = TaskTrackTool::new();
        let out = tool
            .execute(make_input(json!({
                "action": "add",
                "content": "Run tests",
                "active_form": "Running tests"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        // active_form not shown for pending tasks.
        assert!(!out.content.contains("Running tests"));
    }

    #[tokio::test]
    async fn update_status() {
        let tool = TaskTrackTool::new();
        tool.execute(make_input(json!({"action": "add", "content": "Task A"})))
            .await
            .unwrap();

        let out = tool
            .execute(make_input(json!({
                "action": "update",
                "task_index": 0,
                "status": "in_progress"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("in_progress"));
        let meta = out.metadata.unwrap();
        assert_eq!(meta["in_progress"], 1);
        assert_eq!(meta["pending"], 0);
    }

    #[tokio::test]
    async fn list_empty() {
        let tool = TaskTrackTool::new();
        let out = tool
            .execute(make_input(json!({"action": "list"})))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("No tasks"));
        let meta = out.metadata.unwrap();
        assert_eq!(meta["task_count"], 0);
    }

    #[tokio::test]
    async fn list_populated() {
        let tool = TaskTrackTool::new();
        tool.execute(make_input(json!({"action": "add", "content": "Task A"})))
            .await
            .unwrap();
        tool.execute(make_input(json!({"action": "add", "content": "Task B"})))
            .await
            .unwrap();

        let out = tool
            .execute(make_input(json!({"action": "list"})))
            .await
            .unwrap();
        assert!(out.content.contains("Task A"));
        assert!(out.content.contains("Task B"));
        assert!(out.content.contains("[ ]")); // pending badge
    }

    #[tokio::test]
    async fn only_one_in_progress() {
        let tool = TaskTrackTool::new();
        tool.execute(make_input(json!({"action": "add", "content": "Task A"})))
            .await
            .unwrap();
        tool.execute(make_input(json!({"action": "add", "content": "Task B"})))
            .await
            .unwrap();

        // Start Task A.
        tool.execute(make_input(json!({
            "action": "update", "task_index": 0, "status": "in_progress"
        })))
        .await
        .unwrap();

        // Try to start Task B — should fail.
        let out = tool
            .execute(make_input(json!({
                "action": "update", "task_index": 1, "status": "in_progress"
            })))
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("already in_progress"));
    }

    #[tokio::test]
    async fn update_nonexistent_index() {
        let tool = TaskTrackTool::new();
        let out = tool
            .execute(make_input(json!({
                "action": "update", "task_index": 5, "status": "completed"
            })))
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("out of range"));
    }

    #[tokio::test]
    async fn complete_all() {
        let tool = TaskTrackTool::new();
        tool.execute(make_input(json!({"action": "add", "content": "Task A"})))
            .await
            .unwrap();
        tool.execute(make_input(json!({"action": "add", "content": "Task B"})))
            .await
            .unwrap();

        tool.execute(make_input(json!({
            "action": "update", "task_index": 0, "status": "completed"
        })))
        .await
        .unwrap();
        tool.execute(make_input(json!({
            "action": "update", "task_index": 1, "status": "completed"
        })))
        .await
        .unwrap();

        let out = tool
            .execute(make_input(json!({"action": "list"})))
            .await
            .unwrap();
        let meta = out.metadata.unwrap();
        assert_eq!(meta["completed"], 2);
        assert_eq!(meta["pending"], 0);
        assert!(out.content.contains("[x]"));
    }

    #[tokio::test]
    async fn status_transitions() {
        let tool = TaskTrackTool::new();
        tool.execute(make_input(json!({"action": "add", "content": "T"})))
            .await
            .unwrap();

        // pending → in_progress
        tool.execute(make_input(json!({
            "action": "update", "task_index": 0, "status": "in_progress"
        })))
        .await
        .unwrap();

        // in_progress → completed
        let out = tool
            .execute(make_input(json!({
                "action": "update", "task_index": 0, "status": "completed"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("completed"));
    }

    #[tokio::test]
    async fn active_form_shown_when_in_progress() {
        let tool = TaskTrackTool::new();
        tool.execute(make_input(json!({
            "action": "add",
            "content": "Run tests",
            "active_form": "Running tests"
        })))
        .await
        .unwrap();

        tool.execute(make_input(json!({
            "action": "update", "task_index": 0, "status": "in_progress"
        })))
        .await
        .unwrap();

        let out = tool
            .execute(make_input(json!({"action": "list"})))
            .await
            .unwrap();
        assert!(out.content.contains("Running tests"));
        assert!(out.content.contains("[>]")); // in_progress badge
    }

    #[tokio::test]
    async fn unknown_action() {
        let tool = TaskTrackTool::new();
        let out = tool
            .execute(make_input(json!({"action": "delete"})))
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("unknown action"));
    }

    #[test]
    fn schema_is_valid() {
        let tool = TaskTrackTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["action"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "action"));
    }

    #[test]
    fn permission_is_readonly() {
        let tool = TaskTrackTool::new();
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
        let dummy = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(!tool.requires_confirmation(&dummy));
    }
}
