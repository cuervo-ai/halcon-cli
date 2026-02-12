//! `file_delete` tool: delete a single file.
//!
//! Destructive — always requires confirmation.
//! No recursive delete. Single file only.
//! Uses path_security for traversal prevention and blocked patterns.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

use crate::path_security;

/// Delete a single file. Requires confirmation. No recursive delete.
pub struct FileDeleteTool {
    allowed_directories: Vec<PathBuf>,
    blocked_patterns: Vec<String>,
}

impl FileDeleteTool {
    pub fn new(allowed_directories: Vec<PathBuf>, blocked_patterns: Vec<String>) -> Self {
        Self {
            allowed_directories,
            blocked_patterns,
        }
    }
}

#[async_trait]
impl Tool for FileDeleteTool {
    fn name(&self) -> &str {
        "file_delete"
    }

    fn description(&self) -> &str {
        "Delete a single file. Reports file size before deletion. No recursive directory deletion."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Destructive
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        true
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let path_str = input.arguments["path"]
            .as_str()
            .ok_or_else(|| CuervoError::InvalidInput("file_delete requires 'path' string".into()))?;

        // Resolve and validate via path_security.
        let resolved = path_security::resolve_and_validate(
            path_str,
            &input.working_directory,
            &self.allowed_directories,
            &self.blocked_patterns,
        )?;

        // Use symlink_metadata (lstat) — does NOT follow symlinks.
        // This prevents TOCTOU attacks where a symlink is swapped in after check.
        let metadata = tokio::fs::symlink_metadata(&resolved).await.map_err(|e| {
            CuervoError::ToolExecutionFailed {
                tool: "file_delete".into(),
                message: format!("cannot stat '{}': {e}", resolved.display()),
            }
        })?;

        // Refuse to delete symlinks — prevents following symlink to delete target.
        if metadata.is_symlink() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!(
                    "file_delete error: '{}' is a symlink (refusing to delete through symlinks)",
                    resolved.display()
                ),
                is_error: true,
                metadata: None,
            });
        }

        if metadata.is_dir() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!(
                    "file_delete error: '{}' is a directory (recursive delete not supported)",
                    resolved.display()
                ),
                is_error: true,
                metadata: None,
            });
        }

        let file_size = metadata.len();

        // Delete the file.
        tokio::fs::remove_file(&resolved).await.map_err(|e| {
            CuervoError::ToolExecutionFailed {
                tool: "file_delete".into(),
                message: format!("failed to delete '{}': {e}", resolved.display()),
            }
        })?;

        let content = format!(
            "Deleted: {} ({} bytes)",
            resolved.display(),
            file_size
        );

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "path": resolved.display().to_string(),
                "size_bytes": file_size,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to delete."
                }
            },
            "required": ["path"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(dir: &std::path::Path) -> FileDeleteTool {
        FileDeleteTool::new(
            vec![dir.to_path_buf()],
            vec!["*.env".to_string(), "*.key".to_string()],
        )
    }

    fn make_input(path: &str, working_dir: &str) -> ToolInput {
        ToolInput {
            tool_use_id: "test".to_string(),
            arguments: json!({"path": path}),
            working_directory: working_dir.to_string(),
        }
    }

    #[tokio::test]
    async fn delete_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "content to delete").unwrap();

        let tool = make_tool(dir.path());
        let output = tool
            .execute(make_input(
                file_path.to_str().unwrap(),
                dir.path().to_str().unwrap(),
            ))
            .await
            .unwrap();
        assert!(!output.is_error, "delete should succeed: {}", output.content);
        assert!(output.content.contains("Deleted"));

        let meta = output.metadata.unwrap();
        assert_eq!(meta["size_bytes"], 17); // "content to delete" = 17 bytes
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn delete_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let tool = make_tool(dir.path());
        let result = tool
            .execute(make_input(
                &dir.path().join("nope.txt").to_str().unwrap(),
                dir.path().to_str().unwrap(),
            ))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn reject_directory() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();

        let tool = make_tool(dir.path());
        let output = tool
            .execute(make_input(
                subdir.to_str().unwrap(),
                dir.path().to_str().unwrap(),
            ))
            .await
            .unwrap();
        assert!(output.is_error);
        assert!(output.content.contains("directory"));
    }

    #[tokio::test]
    async fn reject_blocked_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let env_path = dir.path().join("secrets.env");
        std::fs::write(&env_path, "SECRET=x").unwrap();

        let tool = make_tool(dir.path());
        let result = tool
            .execute(make_input(
                env_path.to_str().unwrap(),
                dir.path().to_str().unwrap(),
            ))
            .await;
        // path_security should reject .env files.
        assert!(result.is_err());
        // File should still exist.
        assert!(env_path.exists());
    }

    #[tokio::test]
    async fn reject_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let tool = make_tool(dir.path());
        let result = tool
            .execute(make_input(
                "../../../etc/hosts",
                dir.path().to_str().unwrap(),
            ))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn requires_confirmation_always() {
        let tool = FileDeleteTool::new(vec![], vec![]);
        let dummy = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(tool.requires_confirmation(&dummy));
    }

    #[test]
    fn schema_is_valid() {
        let tool = FileDeleteTool::new(vec![], vec![]);
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["path"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "path"));
    }
}
