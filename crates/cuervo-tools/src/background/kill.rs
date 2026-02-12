//! `background_kill` tool: terminate a background job.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

use super::ProcessRegistry;

/// Terminate a background job by job_id.
pub struct BackgroundKillTool {
    registry: Arc<ProcessRegistry>,
}

impl BackgroundKillTool {
    pub fn new(registry: Arc<ProcessRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for BackgroundKillTool {
    fn name(&self) -> &str {
        "background_kill"
    }

    fn description(&self) -> &str {
        "Terminate a background job by its job ID."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Destructive
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        // Agent-initiated kills don't need user confirmation.
        false
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let job_id = input.arguments["job_id"]
            .as_str()
            .ok_or_else(|| CuervoError::InvalidInput("background_kill requires 'job_id' string".into()))?;

        let Some((was_running, exit_code)) = self.registry.kill(job_id) else {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("background_kill error: unknown job '{job_id}'"),
                is_error: true,
                metadata: None,
            });
        };

        let content = if was_running {
            format!("Killed job {job_id}")
        } else {
            format!("Job {job_id} was already finished (exit code: {})",
                exit_code.map(|c| c.to_string()).unwrap_or("unknown".into()))
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "job_id": job_id,
                "was_running": was_running,
                "exit_code": exit_code,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": {
                    "type": "string",
                    "description": "The job ID to terminate."
                }
            },
            "required": ["job_id"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::BackgroundProcess;

    fn make_input(job_id: &str) -> ToolInput {
        ToolInput {
            tool_use_id: "test".to_string(),
            arguments: json!({"job_id": job_id}),
            working_directory: "/tmp".to_string(),
        }
    }

    #[tokio::test]
    async fn kill_unknown_job() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let tool = BackgroundKillTool::new(reg);
        let output = tool.execute(make_input("bg-999")).await.unwrap();
        assert!(output.is_error);
        assert!(output.content.contains("unknown job"));
    }

    #[tokio::test]
    async fn kill_running_job() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let p = BackgroundProcess {
            job_id: "bg-0".into(),
            command: "sleep 60".into(),
            child: None,
            started_at: std::time::Instant::now(),
            stdout_buf: String::new(),
            stderr_buf: String::new(),
            exit_code: None,
            finished: false,
        };
        reg.register(p).unwrap();

        let tool = BackgroundKillTool::new(reg);
        let output = tool.execute(make_input("bg-0")).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("Killed"));

        let meta = output.metadata.unwrap();
        assert_eq!(meta["was_running"], true);
    }

    #[tokio::test]
    async fn kill_already_finished() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let p = BackgroundProcess {
            job_id: "bg-0".into(),
            command: "echo done".into(),
            child: None,
            started_at: std::time::Instant::now(),
            stdout_buf: String::new(),
            stderr_buf: String::new(),
            exit_code: Some(0),
            finished: true,
        };
        reg.register(p).unwrap();

        let tool = BackgroundKillTool::new(reg);
        let output = tool.execute(make_input("bg-0")).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("already finished"));

        let meta = output.metadata.unwrap();
        assert_eq!(meta["was_running"], false);
    }

    #[test]
    fn does_not_require_confirmation() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let tool = BackgroundKillTool::new(reg);
        let dummy = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(!tool.requires_confirmation(&dummy));
    }

    #[test]
    fn schema_is_valid() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let tool = BackgroundKillTool::new(reg);
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "job_id"));
    }
}
