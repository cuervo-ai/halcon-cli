//! `git_status` tool: show working tree status.

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::Result;
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

use super::helpers;

/// Show working tree status: branch, staged, modified, and untracked files.
pub struct GitStatusTool;

impl GitStatusTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GitStatusTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Show working tree status: branch, staged, modified, and untracked files."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let working_dir = &input.working_directory;

        if !helpers::is_git_repo(working_dir).await {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "git_status error: not a git repository".to_string(),
                is_error: true,
                metadata: None,
            });
        }

        let output =
            helpers::run_git_command(working_dir, &["status", "--porcelain=v2", "--branch"], None)
                .await?;

        if output.exit_code != 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("git_status error: {}", output.stderr.trim()),
                is_error: true,
                metadata: None,
            });
        }

        let info = helpers::parse_porcelain_v2(&output.stdout);

        // Build human-readable content.
        let mut content = String::new();

        if let Some(ref branch) = info.branch {
            content.push_str(&format!("On branch {branch}\n"));
        }
        if let Some(ref upstream) = info.upstream {
            if info.ahead > 0 || info.behind > 0 {
                content.push_str(&format!(
                    "Your branch is ahead by {}, behind by {} relative to {upstream}\n",
                    info.ahead, info.behind
                ));
            }
        }

        if info.is_clean() {
            content.push_str("Working tree clean\n");
        } else {
            if !info.staged.is_empty() {
                content.push_str("\nChanges staged for commit:\n");
                for f in &info.staged {
                    content.push_str(&format!("  {f}\n"));
                }
            }
            if !info.modified.is_empty() {
                content.push_str("\nChanges not staged:\n");
                for f in &info.modified {
                    content.push_str(&format!("  {f}\n"));
                }
            }
            if !info.untracked.is_empty() {
                content.push_str("\nUntracked files:\n");
                for f in &info.untracked {
                    content.push_str(&format!("  {f}\n"));
                }
            }
            if !info.conflicted.is_empty() {
                content.push_str("\nConflicted files:\n");
                for f in &info.conflicted {
                    content.push_str(&format!("  {f}\n"));
                }
            }
        }

        let metadata = json!({
            "branch": info.branch,
            "upstream": info.upstream,
            "ahead": info.ahead,
            "behind": info.behind,
            "staged": info.staged,
            "modified": info.modified,
            "untracked": info.untracked,
            "conflicted": info.conflicted,
            "is_clean": info.is_clean(),
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
            "properties": {},
            "required": []
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(working_dir: &str) -> ToolInput {
        ToolInput {
            tool_use_id: "test".to_string(),
            arguments: json!({}),
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
        let tool = GitStatusTool::new();
        let output = tool
            .execute(make_input(dir.path().to_str().unwrap()))
            .await
            .unwrap();
        assert!(output.is_error);
        assert!(output.content.contains("not a git repository"));
    }

    #[tokio::test]
    async fn clean_repo() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;

        // Create an initial commit so branch is set.
        let path = dir.path().to_str().unwrap();
        std::fs::write(dir.path().join("README.md"), "hello").unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(path)
            .output()
            .await;

        let tool = GitStatusTool::new();
        let output = tool.execute(make_input(path)).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("clean"));

        let meta = output.metadata.unwrap();
        assert_eq!(meta["is_clean"], true);
    }

    #[tokio::test]
    async fn staged_file() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let path = dir.path().to_str().unwrap();

        // Initial commit.
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(path)
            .output()
            .await;

        // Modify and stage.
        std::fs::write(dir.path().join("a.txt"), "modified").unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(path)
            .output()
            .await;

        let tool = GitStatusTool::new();
        let output = tool.execute(make_input(path)).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("staged"));

        let meta = output.metadata.unwrap();
        assert_eq!(meta["is_clean"], false);
        let staged = meta["staged"].as_array().unwrap();
        assert!(!staged.is_empty());
    }

    #[tokio::test]
    async fn untracked_file() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let path = dir.path().to_str().unwrap();

        // Create initial commit.
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

        // Add untracked file.
        std::fs::write(dir.path().join("new_file.rs"), "fn main() {}").unwrap();

        let tool = GitStatusTool::new();
        let output = tool.execute(make_input(path)).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("Untracked"));

        let meta = output.metadata.unwrap();
        let untracked = meta["untracked"].as_array().unwrap();
        assert!(untracked
            .iter()
            .any(|v| v.as_str().unwrap().contains("new_file.rs")));
    }

    #[tokio::test]
    async fn modified_file() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let path = dir.path().to_str().unwrap();

        // Create and commit.
        std::fs::write(dir.path().join("a.txt"), "original").unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(path)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(path)
            .output()
            .await;

        // Modify without staging.
        std::fs::write(dir.path().join("a.txt"), "changed").unwrap();

        let tool = GitStatusTool::new();
        let output = tool.execute(make_input(path)).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("not staged"));

        let meta = output.metadata.unwrap();
        let modified = meta["modified"].as_array().unwrap();
        assert!(!modified.is_empty());
    }

    #[test]
    fn schema_is_valid() {
        let tool = GitStatusTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"].is_array());
    }
}
