//! `process_list` tool: list running processes.
//!
//! Uses the system `ps` command to list processes with PID, CPU%, MEM%, and name.
//! Optionally filters by name or user.

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

#[allow(unused_imports)]
use tracing::instrument;

pub struct ProcessListTool;

impl ProcessListTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ProcessListTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ProcessListTool {
    fn name(&self) -> &str {
        "process_list"
    }

    fn description(&self) -> &str {
        "List running processes on the system. \
         Returns PID, CPU%, memory%, user, and process name/command. \
         Optionally filter by process name substring or user. \
         Useful for checking if a server is running, finding zombie processes, \
         or verifying that a background task started correctly."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    #[tracing::instrument(skip(self), fields(tool = "process_list"))]
    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let filter_name = input
            .arguments
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        let filter_user = input
            .arguments
            .get("user")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        let limit = input
            .arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .min(200) as usize;

        // Use ps with consistent output format across macOS/Linux.
        // `comm` shows only the executable name (not full args) — no arg injection risk.
        let output = tokio::process::Command::new("ps")
            .args(["-eo", "pid,pcpu,pmem,user,comm"])
            .output()
            .await
            .map_err(|e| HalconError::ToolExecutionFailed {
                tool: "process_list".to_string(),
                message: format!("Failed to run ps: {e}"),
            })?;

        if !output.status.success() {
            return Err(HalconError::ToolExecutionFailed {
                tool: "process_list".to_string(),
                message: format!(
                    "ps exited with status {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Cap raw output to 1 MB to bound memory use regardless of OS behaviour.
        const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
        let raw = if output.stdout.len() > MAX_OUTPUT_BYTES {
            &output.stdout[..MAX_OUTPUT_BYTES]
        } else {
            &output.stdout[..]
        };
        let stdout = String::from_utf8_lossy(raw);
        let lines: Vec<&str> = stdout.lines().collect();

        // Keep header line always
        let header = lines.first().copied().unwrap_or("");
        let data_lines: Vec<&str> = lines
            .iter()
            .skip(1) // skip header
            .filter(|line| {
                let lower = line.to_lowercase();
                (filter_name.is_empty() || lower.contains(&filter_name))
                    && (filter_user.is_empty() || lower.contains(&filter_user))
            })
            .take(limit)
            .copied()
            .collect();

        let total = data_lines.len();
        let mut content = String::new();
        content.push_str(header);
        content.push('\n');
        for line in &data_lines {
            content.push_str(line);
            content.push('\n');
        }

        if total == 0 {
            let filter_desc = if !filter_name.is_empty() {
                format!("name filter '{filter_name}'")
            } else if !filter_user.is_empty() {
                format!("user filter '{filter_user}'")
            } else {
                "no filters".to_string()
            };
            content = format!("No processes found matching {filter_desc}.");
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "process_count": total,
                "filter_name": filter_name,
                "filter_user": filter_user,
                "limit": limit,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Filter processes by name substring (case-insensitive). E.g. 'rust', 'node', 'halcon'."
                },
                "user": {
                    "type": "string",
                    "description": "Filter processes by username (case-insensitive)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of processes to return (1-200, default 50).",
                    "minimum": 1,
                    "maximum": 200
                }
            },
            "required": []
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_schema() {
        let tool = ProcessListTool::new();
        assert_eq!(tool.name(), "process_list");
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn lists_processes() {
        let tool = ProcessListTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "limit": 10 }),
            working_directory: "/tmp".into(),
        };
        let out = tool.execute(input).await.unwrap();
        assert!(!out.is_error);
        // Header line should contain PID or pid
        assert!(
            out.content.to_lowercase().contains("pid"),
            "missing header: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn filter_by_nonexistent_name() {
        let tool = ProcessListTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "name": "ZZZZZ_no_such_process_XYZZY" }),
            working_directory: "/tmp".into(),
        };
        let out = tool.execute(input).await.unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("No processes found")
                || out.metadata.as_ref().unwrap()["process_count"]
                    .as_u64()
                    .unwrap_or(1)
                    == 0,
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn limit_is_respected() {
        let tool = ProcessListTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "limit": 3 }),
            working_directory: "/tmp".into(),
        };
        let out = tool.execute(input).await.unwrap();
        assert!(!out.is_error);
        let count = out.metadata.as_ref().unwrap()["process_count"]
            .as_u64()
            .unwrap_or(999);
        assert!(count <= 3, "expected at most 3 processes, got {count}");
    }
}
