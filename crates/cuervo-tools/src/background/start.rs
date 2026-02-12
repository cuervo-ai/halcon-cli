//! `background_start` tool: spawn a long-running command in the background.

use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

use super::{BackgroundProcess, ProcessRegistry};

/// Maximum timeout for background jobs (1 hour).
const MAX_TIMEOUT_SECS: u64 = 3600;
/// Default timeout (5 minutes).
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Start a background command. Destructive — requires confirmation.
pub struct BackgroundStartTool {
    registry: Arc<ProcessRegistry>,
}

impl BackgroundStartTool {
    pub fn new(registry: Arc<ProcessRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for BackgroundStartTool {
    fn name(&self) -> &str {
        "background_start"
    }

    fn description(&self) -> &str {
        "Start a long-running command in the background. Returns a job ID for monitoring."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Destructive
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        true
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let command = input.arguments["command"]
            .as_str()
            .ok_or_else(|| CuervoError::InvalidInput("background_start requires 'command' string".into()))?;

        let timeout_secs = input.arguments["timeout_secs"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);

        let working_dir = &input.working_directory;

        // Spawn the child process via sh -c (same approach as bash tool).
        let child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|e| CuervoError::ToolExecutionFailed {
                tool: "background_start".into(),
                message: format!("failed to spawn process: {e}"),
            })?;

        let job_id = self.registry.next_id();

        let process = BackgroundProcess {
            job_id: job_id.clone(),
            command: command.to_string(),
            child: Some(child),
            started_at: std::time::Instant::now(),
            stdout_buf: String::new(),
            stderr_buf: String::new(),
            exit_code: None,
            finished: false,
        };

        self.registry.register(process).map_err(|e| {
            CuervoError::ToolExecutionFailed {
                tool: "background_start".into(),
                message: format!("background_start error: {e}"),
            }
        })?;

        // Spawn a tokio task to collect output and enforce timeout.
        let reg = self.registry.clone();
        let jid = job_id.clone();
        let timeout = std::time::Duration::from_secs(timeout_secs);

        tokio::spawn(async move {
            // Take the child out of the registry for async waiting.
            let Some(child) = reg.take_child(&jid) else {
                return;
            };

            let result = tokio::time::timeout(timeout, child.wait_with_output()).await;

            match result {
                Ok(Ok(output)) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let exit_code = output.status.code();
                    reg.update_output(&jid, &stdout, &stderr, true, exit_code);
                }
                Ok(Err(e)) => {
                    reg.update_output(&jid, "", &format!("process error: {e}"), true, None);
                }
                Err(_) => {
                    // Timeout — kill via registry (child was consumed by wait_with_output future).
                    reg.kill(&jid);
                    reg.update_output(
                        &jid,
                        "",
                        &format!("process timed out after {timeout_secs}s"),
                        true,
                        None,
                    );
                }
            }
        });

        let content = format!("Started background job: {job_id}\nCommand: {command}\nTimeout: {timeout_secs}s");

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "job_id": job_id,
                "command": command,
                "timeout_secs": timeout_secs,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to run in the background."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds (1-3600, default 300)."
                }
            },
            "required": ["command"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(command: &str) -> ToolInput {
        ToolInput {
            tool_use_id: "test".to_string(),
            arguments: json!({"command": command}),
            working_directory: "/tmp".to_string(),
        }
    }

    #[tokio::test]
    async fn start_simple_command() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let tool = BackgroundStartTool::new(reg.clone());
        let output = tool.execute(make_input("echo hello")).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("bg-0"));
        let meta = output.metadata.unwrap();
        assert_eq!(meta["job_id"], "bg-0");
    }

    #[tokio::test]
    async fn start_respects_max_concurrent() {
        let reg = Arc::new(ProcessRegistry::new(1));
        let tool = BackgroundStartTool::new(reg.clone());

        // First should succeed.
        let out1 = tool.execute(make_input("sleep 60")).await.unwrap();
        assert!(!out1.is_error);

        // Second should fail (max=1, first is still running).
        let result = tool.execute(make_input("sleep 60")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn start_with_timeout() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let tool = BackgroundStartTool::new(reg.clone());
        let input = ToolInput {
            tool_use_id: "test".to_string(),
            arguments: json!({"command": "echo hi", "timeout_secs": 10}),
            working_directory: "/tmp".to_string(),
        };
        let output = tool.execute(input).await.unwrap();
        assert!(!output.is_error);
        let meta = output.metadata.unwrap();
        assert_eq!(meta["timeout_secs"], 10);
    }

    #[tokio::test]
    async fn job_id_increments() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let tool = BackgroundStartTool::new(reg.clone());

        let out1 = tool.execute(make_input("echo 1")).await.unwrap();
        let out2 = tool.execute(make_input("echo 2")).await.unwrap();

        let id1 = out1.metadata.unwrap()["job_id"].as_str().unwrap().to_string();
        let id2 = out2.metadata.unwrap()["job_id"].as_str().unwrap().to_string();
        assert_ne!(id1, id2);
    }

    #[test]
    fn requires_confirmation_always() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let tool = BackgroundStartTool::new(reg);
        let dummy = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(tool.requires_confirmation(&dummy));
    }

    #[test]
    fn schema_is_valid() {
        let reg = Arc::new(ProcessRegistry::new(5));
        let tool = BackgroundStartTool::new(reg);
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["command"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "command"));
    }
}
