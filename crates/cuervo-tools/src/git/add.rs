//! `git_add` tool: stage files for commit.

use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

use super::helpers;

/// Stage files for commit. Requires explicit file paths (no `git add .` or `-A`).
pub struct GitAddTool;

impl GitAddTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GitAddTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GitAddTool {
    fn name(&self) -> &str {
        "git_add"
    }

    fn description(&self) -> &str {
        "Stage files for commit. Requires explicit file paths — no wildcards or 'add all'."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadWrite
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let working_dir = &input.working_directory;

        if !helpers::is_git_repo(working_dir).await {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "git_add error: not a git repository".to_string(),
                is_error: true,
                metadata: None,
            });
        }

        let paths = input.arguments["paths"]
            .as_array()
            .ok_or_else(|| CuervoError::InvalidInput("git_add requires 'paths' array".into()))?;

        if paths.is_empty() {
            return Err(CuervoError::InvalidInput(
                "git_add requires at least one path".into(),
            ));
        }

        // Extract and validate each path.
        let mut path_strs: Vec<String> = Vec::with_capacity(paths.len());
        for p in paths {
            let path_str = p
                .as_str()
                .ok_or_else(|| CuervoError::InvalidInput("each path must be a string".into()))?;

            // Reject dangerous patterns.
            if path_str == "." || path_str == "-A" || path_str == "--all" {
                return Err(CuervoError::InvalidInput(format!(
                    "'{path_str}' not allowed — use explicit file paths"
                )));
            }

            helpers::validate_repo_path(path_str, working_dir)?;
            path_strs.push(path_str.to_string());
        }

        // Build args: git add -- <path1> <path2> ...
        let mut args: Vec<&str> = vec!["add", "--"];
        for p in &path_strs {
            args.push(p);
        }

        let output = helpers::run_git_command(working_dir, &args, None).await?;

        if output.exit_code != 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("git_add error: {}", output.stderr.trim()),
                is_error: true,
                metadata: Some(json!({"paths": path_strs})),
            });
        }

        let content = format!("Staged {} file(s): {}", path_strs.len(), path_strs.join(", "));

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({"paths": path_strs})),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "File paths to stage for commit."
                }
            },
            "required": ["paths"]
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
        let tool = GitAddTool::new();
        let output = tool
            .execute(make_input(
                dir.path().to_str().unwrap(),
                json!({"paths": ["file.txt"]}),
            ))
            .await
            .unwrap();
        assert!(output.is_error);
        assert!(output.content.contains("not a git repository"));
    }

    #[tokio::test]
    async fn add_single_file() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        std::fs::write(dir.path().join("hello.txt"), "hello").unwrap();

        let tool = GitAddTool::new();
        let output = tool
            .execute(make_input(
                dir.path().to_str().unwrap(),
                json!({"paths": ["hello.txt"]}),
            ))
            .await
            .unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("hello.txt"));
        assert!(output.content.contains("1 file"));
    }

    #[tokio::test]
    async fn add_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b").unwrap();

        let tool = GitAddTool::new();
        let output = tool
            .execute(make_input(
                dir.path().to_str().unwrap(),
                json!({"paths": ["a.txt", "b.txt"]}),
            ))
            .await
            .unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("2 file"));
    }

    #[tokio::test]
    async fn reject_dot_path() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;

        let tool = GitAddTool::new();
        let result = tool
            .execute(make_input(
                dir.path().to_str().unwrap(),
                json!({"paths": ["."]}),
            ))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn reject_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;

        let tool = GitAddTool::new();
        let result = tool
            .execute(make_input(
                dir.path().to_str().unwrap(),
                json!({"paths": ["../../etc/passwd"]}),
            ))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn nonexistent_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;

        let tool = GitAddTool::new();
        let output = tool
            .execute(make_input(
                dir.path().to_str().unwrap(),
                json!({"paths": ["nonexistent.txt"]}),
            ))
            .await
            .unwrap();
        // Git returns error for non-matching pathspec.
        assert!(output.is_error);
    }

    #[test]
    fn schema_is_valid() {
        let tool = GitAddTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["paths"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "paths"));
    }
}
