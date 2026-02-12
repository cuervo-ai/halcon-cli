//! `background_output` tool: check output of a background job.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

use super::ProcessRegistry;

/// Maximum output size before truncation.
const MAX_OUTPUT_CHARS: usize = 100_000;

/// Check output of a background job by job_id.
pub struct BackgroundOutputTool {
    registry: Arc<ProcessRegistry>,
}

impl BackgroundOutputTool {
    pub fn new(registry: Arc<ProcessRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for BackgroundOutputTool {
    fn name(&self) -> &str {
        "background_output"
    }

    fn description(&self) -> &str {
        "Check the output and status of a background job by its job ID."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let job_id = input.arguments["job_id"]
            .as_str()
            .ok_or_else(|| CuervoError::InvalidInput("background_output requires 'job_id' string".into()))?;

        let Some((stdout, stderr, finished, exit_code, elapsed_secs)) =
            self.registry.get_output(job_id)
        else {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("background_output error: unknown job '{job_id}'"),
                is_error: true,
                metadata: None,
            });
        };

        let status = if finished {
            match exit_code {
                Some(0) => "completed (success)".to_string(),
                Some(code) => format!("completed (exit code {code})"),
                None => "completed (unknown exit code)".to_string(),
            }
        } else {
            format!("running ({elapsed_secs}s elapsed)")
        };

        let mut content = format!("Job {job_id}: {status}\n");

        if !stdout.is_empty() {
            content.push_str("\n--- stdout ---\n");
            if stdout.len() > MAX_OUTPUT_CHARS {
                let truncated: String = stdout.chars().take(MAX_OUTPUT_CHARS).collect();
                content.push_str(&truncated);
                content.push_str("\n... (truncated)");
            } else {
                content.push_str(&stdout);
            }
        }

        if !stderr.is_empty() {
            content.push_str("\n--- stderr ---\n");
            if stderr.len() > MAX_OUTPUT_CHARS {
                let truncated: String = stderr.chars().take(MAX_OUTPUT_CHARS).collect();
                content.push_str(&truncated);
                content.push_str("\n... (truncated)");
            } else {
                content.push_str(&stderr);
            }
        }

        let metadata = json!({
            "job_id": job_id,
            "finished": finished,
            "exit_code": exit_code,
            "elapsed_secs": elapsed_secs,
        });

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(metadata),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": {
                    "type": "string",
                    "description": "The job ID returned by background_start."
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
    async fn unknown_job() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let tool = BackgroundOutputTool::new(reg);
        let output = tool.execute(make_input("bg-999")).await.unwrap();
        assert!(output.is_error);
        assert!(output.content.contains("unknown job"));
    }

    #[tokio::test]
    async fn running_job() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let p = BackgroundProcess {
            job_id: "bg-0".into(),
            command: "sleep 60".into(),
            child: None,
            started_at: std::time::Instant::now(),
            stdout_buf: "partial output\n".into(),
            stderr_buf: String::new(),
            exit_code: None,
            finished: false,
        };
        reg.register(p).unwrap();

        let tool = BackgroundOutputTool::new(reg);
        let output = tool.execute(make_input("bg-0")).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("running"));
        assert!(output.content.contains("partial output"));
    }

    #[tokio::test]
    async fn completed_job() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let p = BackgroundProcess {
            job_id: "bg-0".into(),
            command: "echo done".into(),
            child: None,
            started_at: std::time::Instant::now(),
            stdout_buf: "done\n".into(),
            stderr_buf: String::new(),
            exit_code: Some(0),
            finished: true,
        };
        reg.register(p).unwrap();

        let tool = BackgroundOutputTool::new(reg);
        let output = tool.execute(make_input("bg-0")).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("completed (success)"));
        assert!(output.content.contains("done"));

        let meta = output.metadata.unwrap();
        assert_eq!(meta["finished"], true);
        assert_eq!(meta["exit_code"], 0);
    }

    #[tokio::test]
    async fn empty_output() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let p = BackgroundProcess {
            job_id: "bg-0".into(),
            command: "true".into(),
            child: None,
            started_at: std::time::Instant::now(),
            stdout_buf: String::new(),
            stderr_buf: String::new(),
            exit_code: Some(0),
            finished: true,
        };
        reg.register(p).unwrap();

        let tool = BackgroundOutputTool::new(reg);
        let output = tool.execute(make_input("bg-0")).await.unwrap();
        assert!(!output.is_error);
        assert!(!output.content.contains("stdout"));
    }

    #[test]
    fn schema_is_valid() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let tool = BackgroundOutputTool::new(reg);
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "job_id"));
    }
}
