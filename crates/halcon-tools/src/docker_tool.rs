//! DockerTool — inspect and manage Docker containers, images, and compose services.
//!
//! Provides read operations (ps, images, inspect, logs, stats) and limited write
//! operations (start/stop containers) with appropriate confirmation requirements.
//! Destructive operations (rm, rmi, prune) require explicit user confirmation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};

/// Maximum output bytes from docker commands.
const MAX_OUTPUT: usize = 16_384;

pub struct DockerTool {
    timeout_secs: u64,
}

impl DockerTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    async fn run_docker(&self, args: &[&str], working_dir: &Path) -> Result<String, String> {
        let timeout = Duration::from_secs(self.timeout_secs);

        let result = tokio::time::timeout(
            timeout,
            tokio::process::Command::new("docker")
                .args(args)
                .current_dir(working_dir)
                .env("NO_COLOR", "1")
                .output(),
        )
        .await;

        match result {
            Err(_) => Err(format!(
                "docker command timed out after {}s",
                self.timeout_secs
            )),
            Ok(Err(e)) => Err(format!(
                "Failed to run docker: {} (is Docker installed and running?)",
                e
            )),
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);

                if !out.status.success() {
                    let msg = if stderr.is_empty() {
                        stdout.as_ref()
                    } else {
                        stderr.as_ref()
                    };
                    Err(truncate(msg, 2000))
                } else {
                    Ok(truncate(&stdout, MAX_OUTPUT))
                }
            }
        }
    }

    async fn run_compose(&self, args: &[&str], working_dir: &Path) -> Result<String, String> {
        let timeout = Duration::from_secs(self.timeout_secs);

        // Try `docker compose` (v2) first, then `docker-compose` (v1)
        let result = tokio::time::timeout(
            timeout,
            tokio::process::Command::new("docker")
                .arg("compose")
                .args(args)
                .current_dir(working_dir)
                .env("NO_COLOR", "1")
                .output(),
        )
        .await;

        match result {
            Ok(Ok(out)) if out.status.success() => {
                Ok(truncate(&String::from_utf8_lossy(&out.stdout), MAX_OUTPUT))
            }
            _ => {
                // Fallback to docker-compose v1
                let r2 = tokio::time::timeout(
                    Duration::from_secs(self.timeout_secs),
                    tokio::process::Command::new("docker-compose")
                        .args(args)
                        .current_dir(working_dir)
                        .env("NO_COLOR", "1")
                        .output(),
                )
                .await;
                match r2 {
                    Err(_) => Err("docker compose timed out".to_string()),
                    Ok(Err(e)) => Err(format!("docker-compose not found: {}", e)),
                    Ok(Ok(o)) => {
                        if o.status.success() {
                            Ok(truncate(&String::from_utf8_lossy(&o.stdout), MAX_OUTPUT))
                        } else {
                            Err(truncate(&String::from_utf8_lossy(&o.stderr), 2000))
                        }
                    }
                }
            }
        }
    }
}

impl Default for DockerTool {
    fn default() -> Self {
        Self::new(60)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!(
            "{}\n... [output truncated, {} chars total]",
            &s[..max],
            s.len()
        )
    }
}

#[async_trait]
impl Tool for DockerTool {
    fn name(&self) -> &str {
        "docker"
    }

    fn description(&self) -> &str {
        "Inspect and manage Docker containers, images, networks, and compose services. \
         Read operations: ps (list containers), images (list images), inspect, logs, stats, \
         compose ps/logs/config. Write operations: start/stop/restart containers. \
         Destructive operations (rm/rmi/prune) require confirmation. \
         Useful for debugging, monitoring, and managing containerized environments."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "ps", "images", "inspect", "logs", "stats",
                        "start", "stop", "restart", "rm", "rmi", "prune",
                        "network_ls", "volume_ls",
                        "compose_ps", "compose_logs", "compose_up", "compose_down", "compose_config",
                        "info", "version"
                    ],
                    "description": "Docker action to perform."
                },
                "target": {
                    "type": "string",
                    "description": "Container name/ID, image name/ID, or service name (for compose operations)."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Additional flags (e.g. ['--tail', '100'] for logs, ['--all'] for ps)."
                },
                "working_directory": {
                    "type": "string",
                    "description": "Directory for compose operations (must contain docker-compose.yml). Defaults to tool working directory."
                }
            },
            "required": ["action"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadWrite
    }

    fn requires_confirmation(&self, input: &ToolInput) -> bool {
        let action = input.arguments["action"].as_str().unwrap_or("");
        matches!(action, "rm" | "rmi" | "prune" | "compose_down")
    }

    async fn execute_inner(
        &self,
        input: ToolInput,
    ) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let args = &input.arguments;
        let working_dir = args["working_directory"]
            .as_str()
            .map(|p| {
                let p = Path::new(p);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    PathBuf::from(&input.working_directory).join(p)
                }
            })
            .unwrap_or_else(|| PathBuf::from(&input.working_directory));

        let action = args["action"].as_str().unwrap_or("ps");
        let target = args["target"].as_str().unwrap_or("");
        let extra_args: Vec<&str> = args["args"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        tracing::info!(tool = "docker", action, target, "executing docker action");

        let result = match action {
            "ps" => {
                let mut cmd_args = vec!["ps"];
                cmd_args.extend(extra_args.iter().copied());
                self.run_docker(&cmd_args, &working_dir).await
            }
            "images" => {
                let mut cmd_args = vec!["images"];
                cmd_args.extend(extra_args.iter().copied());
                self.run_docker(&cmd_args, &working_dir).await
            }
            "inspect" => {
                if target.is_empty() {
                    Err("'target' is required for inspect".to_string())
                } else {
                    self.run_docker(&["inspect", target], &working_dir).await
                }
            }
            "logs" => {
                if target.is_empty() {
                    Err("'target' (container name) is required for logs".to_string())
                } else {
                    let mut cmd_args = vec!["logs"];
                    cmd_args.extend(extra_args.iter().copied());
                    cmd_args.push(target);
                    self.run_docker(&cmd_args, &working_dir).await
                }
            }
            "stats" => {
                // Run with --no-stream to get a single snapshot
                let mut cmd_args = vec!["stats", "--no-stream"];
                if !target.is_empty() {
                    cmd_args.push(target);
                }
                self.run_docker(&cmd_args, &working_dir).await
            }
            "start" => {
                if target.is_empty() {
                    Err("'target' (container name) is required for start".to_string())
                } else {
                    self.run_docker(&["start", target], &working_dir).await
                }
            }
            "stop" => {
                if target.is_empty() {
                    Err("'target' (container name) is required for stop".to_string())
                } else {
                    self.run_docker(&["stop", target], &working_dir).await
                }
            }
            "restart" => {
                if target.is_empty() {
                    Err("'target' (container name) is required for restart".to_string())
                } else {
                    self.run_docker(&["restart", target], &working_dir).await
                }
            }
            "rm" => {
                if target.is_empty() {
                    Err("'target' (container name) is required for rm".to_string())
                } else {
                    self.run_docker(&["rm", target], &working_dir).await
                }
            }
            "rmi" => {
                if target.is_empty() {
                    Err("'target' (image name) is required for rmi".to_string())
                } else {
                    self.run_docker(&["rmi", target], &working_dir).await
                }
            }
            "prune" => {
                self.run_docker(&["system", "prune", "--force"], &working_dir)
                    .await
            }
            "network_ls" => self.run_docker(&["network", "ls"], &working_dir).await,
            "volume_ls" => self.run_docker(&["volume", "ls"], &working_dir).await,
            "info" => self.run_docker(&["info"], &working_dir).await,
            "version" => self.run_docker(&["version"], &working_dir).await,
            "compose_ps" => {
                let mut compose_args = vec!["ps"];
                compose_args.extend(extra_args.iter().copied());
                self.run_compose(&compose_args, &working_dir).await
            }
            "compose_logs" => {
                let mut compose_args = vec!["logs"];
                compose_args.extend(extra_args.iter().copied());
                if !target.is_empty() {
                    compose_args.push(target);
                }
                self.run_compose(&compose_args, &working_dir).await
            }
            "compose_up" => {
                let mut compose_args = vec!["up", "-d"];
                compose_args.extend(extra_args.iter().copied());
                if !target.is_empty() {
                    compose_args.push(target);
                }
                self.run_compose(&compose_args, &working_dir).await
            }
            "compose_down" => {
                let mut compose_args = vec!["down"];
                compose_args.extend(extra_args.iter().copied());
                self.run_compose(&compose_args, &working_dir).await
            }
            "compose_config" => self.run_compose(&["config"], &working_dir).await,
            other => Err(format!("Unknown docker action: {}", other)),
        };

        match result {
            Ok(output) => Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: output,
                is_error: false,
                metadata: Some(json!({ "action": action, "target": target })),
            }),
            Err(e) => Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: e,
                is_error: true,
                metadata: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short() {
        let s = "hello world";
        assert_eq!(truncate(s, 100), s);
    }

    #[test]
    fn truncate_long() {
        let s = "a".repeat(200);
        let t = truncate(&s, 100);
        assert!(t.contains("truncated"));
        assert!(t.len() < s.len() + 50);
    }

    #[test]
    fn tool_metadata() {
        let t = DockerTool::default();
        assert_eq!(t.name(), "docker");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadWrite);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("action")));
    }

    #[test]
    fn requires_confirmation_destructive() {
        let t = DockerTool::default();
        let make_input = |action: &str| ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({ "action": action }),
            working_directory: "/tmp".into(),
        };
        assert!(t.requires_confirmation(&make_input("rm")));
        assert!(t.requires_confirmation(&make_input("rmi")));
        assert!(t.requires_confirmation(&make_input("prune")));
        assert!(t.requires_confirmation(&make_input("compose_down")));
        assert!(!t.requires_confirmation(&make_input("ps")));
        assert!(!t.requires_confirmation(&make_input("logs")));
        assert!(!t.requires_confirmation(&make_input("images")));
    }

    #[tokio::test]
    async fn execute_missing_target_for_logs() {
        let t = DockerTool::new(10);
        let out = t
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({ "action": "logs" }),
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("required") || out.content.contains("target"));
    }

    #[tokio::test]
    async fn execute_version_attempts_docker() {
        // This test verifies the call flow — docker may or may not be installed
        let t = DockerTool::new(5);
        let out = t
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({ "action": "version" }),
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        // Either succeeds (docker installed) or fails with "not found" — neither panics
        let _ = out;
    }

    #[tokio::test]
    async fn execute_unknown_action_returns_error() {
        let t = DockerTool::new(5);
        let out = t
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({ "action": "unknown_action" }),
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("Unknown"));
    }
}
