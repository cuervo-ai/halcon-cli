//! `git_diff` tool: show file changes between commits, index, and working tree.

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::Result;
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

use super::helpers;

/// Maximum diff output size in characters before truncation.
const MAX_DIFF_CHARS: usize = 16_000;

/// Show file changes. Supports staged, unstaged, and commit-based diffs.
pub struct GitDiffTool;

impl GitDiffTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GitDiffTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Show file changes. Supports staged vs unstaged changes, specific files, and diffs against a commit."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let working_dir = &input.working_directory;

        if !helpers::is_git_repo(working_dir).await {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "git_diff error: not a git repository".to_string(),
                is_error: true,
                metadata: None,
            });
        }

        let staged = input.arguments["staged"].as_bool().unwrap_or(false);
        let path = input.arguments["path"].as_str();
        let commit = input.arguments["commit"].as_str();

        // Validate path if provided.
        if let Some(p) = path {
            helpers::validate_repo_path(p, working_dir)?;
        }

        // Build diff command args.
        let mut args = vec!["diff"];
        if staged {
            args.push("--staged");
        }
        if let Some(c) = commit {
            args.push(c);
        }
        args.push("--stat");
        // First get stat summary.
        let stat_output = helpers::run_git_command(working_dir, &args, None).await?;

        // Now get the full diff.
        let mut full_args = vec!["diff"];
        if staged {
            full_args.push("--staged");
        }
        if let Some(c) = commit {
            full_args.push(c);
        }
        if let Some(p) = path {
            full_args.push("--");
            full_args.push(p);
        }
        let diff_output = helpers::run_git_command(working_dir, &full_args, None).await?;

        if diff_output.exit_code != 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("git_diff error: {}", diff_output.stderr.trim()),
                is_error: true,
                metadata: None,
            });
        }

        let stat_info = helpers::parse_diff_stat(&stat_output.stdout);
        let truncated = diff_output.stdout.len() > MAX_DIFF_CHARS;

        // Build content: if truncated, show stat + truncated diff.
        let content = if truncated {
            let mut c = String::new();
            c.push_str("(diff truncated — showing stat summary + first ");
            c.push_str(&MAX_DIFF_CHARS.to_string());
            c.push_str(" chars)\n\n");
            c.push_str("Stat summary:\n");
            c.push_str(&stat_output.stdout);
            c.push_str("\nDiff (truncated):\n");
            let truncated_diff: String = diff_output.stdout.chars().take(MAX_DIFF_CHARS).collect();
            c.push_str(&truncated_diff);
            c
        } else if diff_output.stdout.is_empty() {
            "No changes".to_string()
        } else {
            diff_output.stdout
        };

        let metadata = json!({
            "files_changed": stat_info.files_changed,
            "insertions": stat_info.insertions,
            "deletions": stat_info.deletions,
            "truncated": truncated,
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
                "staged": {
                    "type": "boolean",
                    "description": "Show staged (cached) changes instead of unstaged. Default: false."
                },
                "path": {
                    "type": "string",
                    "description": "Diff a specific file path."
                },
                "commit": {
                    "type": "string",
                    "description": "Diff against a specific commit or branch (e.g. 'main', 'HEAD~3')."
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

    async fn init_repo_with_commit(dir: &std::path::Path) {
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
        std::fs::write(dir.join("file.txt"), "original content").unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(path)
            .output()
            .await;
    }

    #[tokio::test]
    async fn not_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let tool = GitDiffTool::new();
        let output = tool
            .execute(make_input(dir.path().to_str().unwrap(), json!({})))
            .await
            .unwrap();
        assert!(output.is_error);
        assert!(output.content.contains("not a git repository"));
    }

    #[tokio::test]
    async fn no_changes() {
        let dir = tempfile::tempdir().unwrap();
        init_repo_with_commit(dir.path()).await;
        let tool = GitDiffTool::new();
        let output = tool
            .execute(make_input(dir.path().to_str().unwrap(), json!({})))
            .await
            .unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("No changes"));
    }

    #[tokio::test]
    async fn unstaged_change() {
        let dir = tempfile::tempdir().unwrap();
        init_repo_with_commit(dir.path()).await;
        std::fs::write(dir.path().join("file.txt"), "modified content").unwrap();

        let tool = GitDiffTool::new();
        let output = tool
            .execute(make_input(dir.path().to_str().unwrap(), json!({})))
            .await
            .unwrap();
        assert!(!output.is_error);
        // Should show a diff with the change.
        assert!(
            output.content.contains("modified content") || output.content.contains("file.txt"),
            "diff should mention the changed file or content"
        );
    }

    #[tokio::test]
    async fn staged_change() {
        let dir = tempfile::tempdir().unwrap();
        init_repo_with_commit(dir.path()).await;
        let path = dir.path().to_str().unwrap();

        std::fs::write(dir.path().join("file.txt"), "staged content").unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(path)
            .output()
            .await;

        let tool = GitDiffTool::new();
        let output = tool
            .execute(make_input(path, json!({"staged": true})))
            .await
            .unwrap();
        assert!(!output.is_error);
        assert!(
            output.content.contains("staged content") || output.content.contains("file.txt"),
            "staged diff should show the staged change"
        );
    }

    #[tokio::test]
    async fn specific_file() {
        let dir = tempfile::tempdir().unwrap();
        init_repo_with_commit(dir.path()).await;
        let path = dir.path().to_str().unwrap();

        std::fs::write(dir.path().join("file.txt"), "changed").unwrap();
        std::fs::write(dir.path().join("other.txt"), "other").unwrap();

        let tool = GitDiffTool::new();
        let output = tool
            .execute(make_input(path, json!({"path": "file.txt"})))
            .await
            .unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("file.txt") || output.content.contains("changed"));
    }

    #[tokio::test]
    async fn metadata_includes_stat() {
        let dir = tempfile::tempdir().unwrap();
        init_repo_with_commit(dir.path()).await;
        std::fs::write(dir.path().join("file.txt"), "changed content here").unwrap();

        let tool = GitDiffTool::new();
        let output = tool
            .execute(make_input(dir.path().to_str().unwrap(), json!({})))
            .await
            .unwrap();
        assert!(!output.is_error);
        let meta = output.metadata.unwrap();
        assert!(meta["truncated"].is_boolean());
    }

    #[test]
    fn schema_is_valid() {
        let tool = GitDiffTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["staged"].is_object());
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["commit"].is_object());
        assert!(schema["required"].is_array());
    }
}
