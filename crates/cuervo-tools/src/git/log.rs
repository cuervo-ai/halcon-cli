//! `git_log` tool: show commit history.

use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::Result;
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

use super::helpers;

/// Maximum number of log entries.
const MAX_COUNT: u64 = 50;
/// Default number of log entries.
const DEFAULT_COUNT: u64 = 10;

/// Show commit history. Supports oneline and detailed formats, file filtering, and count limits.
pub struct GitLogTool;

impl GitLogTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GitLogTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GitLogTool {
    fn name(&self) -> &str {
        "git_log"
    }

    fn description(&self) -> &str {
        "Show commit history with hashes, authors, and messages. Supports count limit and file filtering."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let working_dir = &input.working_directory;

        if !helpers::is_git_repo(working_dir).await {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "git_log error: not a git repository".to_string(),
                is_error: true,
                metadata: None,
            });
        }

        let count = input.arguments["count"]
            .as_u64()
            .unwrap_or(DEFAULT_COUNT)
            .clamp(1, MAX_COUNT);

        let oneline = input.arguments["oneline"].as_bool().unwrap_or(true);
        let path = input.arguments["path"].as_str();

        // Validate path if provided.
        if let Some(p) = path {
            helpers::validate_repo_path(p, working_dir)?;
        }

        let count_str = format!("-{count}");
        let mut args: Vec<&str> = vec!["log", &count_str];

        if oneline {
            args.push("--oneline");
        } else {
            args.push("--format=%H %an <%ae> %ai%n  %s%n");
        }

        if let Some(p) = path {
            args.push("--");
            args.push(p);
        }

        let output = helpers::run_git_command(working_dir, &args, None).await?;

        if output.exit_code != 0 {
            // Empty repo with no commits returns non-zero; handle gracefully.
            if output.stderr.contains("does not have any commits") {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: "No commits yet".to_string(),
                    is_error: false,
                    metadata: Some(json!({"commit_count": 0})),
                });
            }
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("git_log error: {}", output.stderr.trim()),
                is_error: true,
                metadata: None,
            });
        }

        if output.stdout.trim().is_empty() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "No commits found".to_string(),
                is_error: false,
                metadata: Some(json!({"commit_count": 0})),
            });
        }

        // Parse commit hashes from output for metadata.
        let lines: Vec<&str> = output.stdout.lines().collect();
        let commit_count = if oneline {
            lines.len()
        } else {
            // In full format, each commit takes 3 lines (hash line, subject line, blank).
            lines.iter().filter(|l| !l.trim().is_empty() && !l.starts_with("  ")).count()
        };

        let newest_hash = if oneline {
            lines.first().and_then(|l| l.split_whitespace().next())
        } else {
            lines.first().and_then(|l| l.split_whitespace().next())
        };

        let oldest_hash = if oneline {
            lines.last().and_then(|l| l.split_whitespace().next())
        } else {
            lines
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty() && !l.starts_with("  "))
                .and_then(|l| l.split_whitespace().next())
        };

        let metadata = json!({
            "commit_count": commit_count,
            "oldest_hash": oldest_hash,
            "newest_hash": newest_hash,
        });

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: output.stdout,
            is_error: false,
            metadata: Some(metadata),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "count": {
                    "type": "integer",
                    "description": "Number of commits to show (1-50, default 10)."
                },
                "path": {
                    "type": "string",
                    "description": "Show log for a specific file path."
                },
                "oneline": {
                    "type": "boolean",
                    "description": "Use oneline format (hash + subject). Default: true."
                }
            },
            "required": []
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(working_dir: &str, args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test".to_string(),
            arguments: args,
            working_directory: working_dir.to_string(),
        }
    }

    async fn init_repo_with_commits(dir: &std::path::Path, count: u32) {
        let path = dir.to_str().unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(path)
            .output()
            .await;

        for i in 0..count {
            std::fs::write(dir.join("file.txt"), format!("content {i}")).unwrap();
            let _ = tokio::process::Command::new("git")
                .args(["add", "."])
                .current_dir(path)
                .output()
                .await;
            let _ = tokio::process::Command::new("git")
                .args(["commit", "-m", &format!("commit {i}")])
                .current_dir(path)
                .output()
                .await;
        }
    }

    #[tokio::test]
    async fn not_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let tool = GitLogTool::new();
        let output = tool
            .execute(make_input(dir.path().to_str().unwrap(), json!({})))
            .await
            .unwrap();
        assert!(output.is_error);
        assert!(output.content.contains("not a git repository"));
    }

    #[tokio::test]
    async fn basic_log() {
        let dir = tempfile::tempdir().unwrap();
        init_repo_with_commits(dir.path(), 3).await;

        let tool = GitLogTool::new();
        let output = tool
            .execute(make_input(dir.path().to_str().unwrap(), json!({})))
            .await
            .unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("commit 2"));
        assert!(output.content.contains("commit 0"));

        let meta = output.metadata.unwrap();
        assert_eq!(meta["commit_count"], 3);
    }

    #[tokio::test]
    async fn log_with_count() {
        let dir = tempfile::tempdir().unwrap();
        init_repo_with_commits(dir.path(), 5).await;

        let tool = GitLogTool::new();
        let output = tool
            .execute(make_input(dir.path().to_str().unwrap(), json!({"count": 2})))
            .await
            .unwrap();
        assert!(!output.is_error);

        let meta = output.metadata.unwrap();
        assert_eq!(meta["commit_count"], 2);
    }

    #[tokio::test]
    async fn log_specific_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(path)
            .output()
            .await;

        // Create two files, commit each separately.
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["commit", "-m", "add a"])
            .current_dir(path)
            .output()
            .await;

        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "b.txt"])
            .current_dir(path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["commit", "-m", "add b"])
            .current_dir(path)
            .output()
            .await;

        let tool = GitLogTool::new();
        let output = tool
            .execute(make_input(path, json!({"path": "a.txt"})))
            .await
            .unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("add a"));
        // Should NOT contain the commit for b.txt
        assert!(!output.content.contains("add b"));
    }

    #[tokio::test]
    async fn empty_repo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .await;

        let tool = GitLogTool::new();
        let output = tool
            .execute(make_input(path, json!({})))
            .await
            .unwrap();
        assert!(!output.is_error);
        // Either "No commits yet" or "No commits found"
        assert!(
            output.content.contains("No commit") || output.content.contains("no commit"),
            "should handle empty repo gracefully: {}",
            output.content
        );
    }

    #[tokio::test]
    async fn full_format() {
        let dir = tempfile::tempdir().unwrap();
        init_repo_with_commits(dir.path(), 2).await;

        let tool = GitLogTool::new();
        let output = tool
            .execute(make_input(
                dir.path().to_str().unwrap(),
                json!({"oneline": false, "count": 2}),
            ))
            .await
            .unwrap();
        assert!(!output.is_error);
        // Full format should include email.
        assert!(output.content.contains("test@test.com"));
    }

    #[test]
    fn schema_is_valid() {
        let tool = GitLogTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["count"].is_object());
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["oneline"].is_object());
        assert!(schema["required"].is_array());
    }
}
