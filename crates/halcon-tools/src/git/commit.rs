//! `git_commit` tool: create a git commit.

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

use super::helpers;

/// Create a git commit with a message. Destructive — always requires confirmation.
/// No `--amend` or `--no-verify` flags are supported.
pub struct GitCommitTool;

impl GitCommitTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GitCommitTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn description(&self) -> &str {
        "Create a git commit with a message. Always requires confirmation. No --amend or --no-verify."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Destructive
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        true
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let working_dir = &input.working_directory;

        if !helpers::is_git_repo(working_dir).await {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "git_commit error: not a git repository".to_string(),
                is_error: true,
                metadata: None,
            });
        }

        let message = input.arguments["message"].as_str().ok_or_else(|| {
            HalconError::InvalidInput("git_commit requires 'message' string".into())
        })?;

        if message.trim().is_empty() {
            return Err(HalconError::InvalidInput(
                "commit message must not be empty".into(),
            ));
        }

        // Commit with the message. Using Command::arg() for each argument — safe from injection.
        let output =
            helpers::run_git_command(working_dir, &["commit", "-m", message], None).await?;

        if output.exit_code != 0 {
            let stderr = output.stderr.trim();
            // Check for "nothing to commit" (common case).
            if stderr.contains("nothing to commit") || output.stdout.contains("nothing to commit") {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: "git_commit error: nothing to commit (staging area is empty)"
                        .to_string(),
                    is_error: true,
                    metadata: None,
                });
            }
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("git_commit error: {stderr}"),
                is_error: true,
                metadata: None,
            });
        }

        // Extract commit hash from output or via rev-parse.
        let hash_output =
            helpers::run_git_command(working_dir, &["rev-parse", "HEAD"], None).await?;
        let commit_hash = hash_output.stdout.trim().to_string();

        let content = format!(
            "Committed: {}\nHash: {}",
            message,
            &commit_hash[..8.min(commit_hash.len())]
        );

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "commit_hash": commit_hash,
                "message": message,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The commit message."
                }
            },
            "required": ["message"]
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

    async fn init_repo(dir: &std::path::Path) {
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
    }

    #[tokio::test]
    async fn not_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let tool = GitCommitTool::new();
        let output = tool
            .execute(make_input(
                dir.path().to_str().unwrap(),
                json!({"message": "test"}),
            ))
            .await
            .unwrap();
        assert!(output.is_error);
        assert!(output.content.contains("not a git repository"));
    }

    #[tokio::test]
    async fn basic_commit() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let path = dir.path().to_str().unwrap();

        std::fs::write(dir.path().join("file.txt"), "content").unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(path)
            .output()
            .await;

        let tool = GitCommitTool::new();
        let output = tool
            .execute(make_input(path, json!({"message": "Add file"})))
            .await
            .unwrap();
        assert!(
            !output.is_error,
            "commit should succeed: {}",
            output.content
        );
        assert!(output.content.contains("Add file"));

        let meta = output.metadata.unwrap();
        let hash = meta["commit_hash"].as_str().unwrap();
        assert!(!hash.is_empty());
        assert!(hash.len() >= 7);
    }

    #[tokio::test]
    async fn empty_staging_area() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let path = dir.path().to_str().unwrap();

        // Create initial commit so repo has commits.
        std::fs::write(dir.path().join("init.txt"), "x").unwrap();
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

        // Now try to commit with nothing staged.
        let tool = GitCommitTool::new();
        let output = tool
            .execute(make_input(path, json!({"message": "empty commit"})))
            .await
            .unwrap();
        assert!(output.is_error);
        assert!(output.content.contains("nothing to commit"));
    }

    #[tokio::test]
    async fn message_with_special_chars() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let path = dir.path().to_str().unwrap();

        std::fs::write(dir.path().join("file.txt"), "content").unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(path)
            .output()
            .await;

        let tool = GitCommitTool::new();
        let message = "Fix \"quotes\" & <special> 'chars' (parens) $var `backtick`";
        let output = tool
            .execute(make_input(path, json!({"message": message})))
            .await
            .unwrap();
        assert!(
            !output.is_error,
            "special chars should work: {}",
            output.content
        );

        let meta = output.metadata.unwrap();
        assert_eq!(meta["message"].as_str().unwrap(), message);
    }

    #[tokio::test]
    async fn commit_hash_in_metadata() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let path = dir.path().to_str().unwrap();

        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(path)
            .output()
            .await;

        let tool = GitCommitTool::new();
        let output = tool
            .execute(make_input(path, json!({"message": "test commit hash"})))
            .await
            .unwrap();
        assert!(!output.is_error);

        let meta = output.metadata.unwrap();
        let hash = meta["commit_hash"].as_str().unwrap();
        // Hash should be a hex string of 40 chars.
        assert_eq!(hash.len(), 40, "full SHA-1 hash expected");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn requires_confirmation_always() {
        let tool = GitCommitTool::new();
        let dummy = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(tool.requires_confirmation(&dummy));
    }

    #[test]
    fn schema_is_valid() {
        let tool = GitCommitTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["message"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "message"));
    }
}
