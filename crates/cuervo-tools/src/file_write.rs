use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

use crate::fs_service::{FsService, MAX_WRITE_SIZE};

/// Write content to a file, creating it if it doesn't exist.
pub struct FileWriteTool {
    fs: Arc<FsService>,
}

impl FileWriteTool {
    pub fn new(fs: Arc<FsService>) -> Self {
        Self { fs }
    }
}

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Creates parent directories as needed."
    }

    fn permission_level(&self) -> PermissionLevel {
        // Destructive: file_write can overwrite existing files and create new ones.
        // Users must confirm before any file creation/overwrite in interactive mode.
        PermissionLevel::Destructive
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let path_str = input.arguments["path"]
            .as_str()
            .ok_or_else(|| CuervoError::InvalidInput("file_write requires 'path' string".into()))?;

        let content = input.arguments["content"].as_str().ok_or_else(|| {
            CuervoError::InvalidInput("file_write requires 'content' string".into())
        })?;

        // Size limit: reject writes that could exhaust disk/memory.
        if content.len() > MAX_WRITE_SIZE {
            return Err(CuervoError::InvalidInput(format!(
                "file_write: content size {} bytes exceeds limit of {} bytes",
                content.len(),
                MAX_WRITE_SIZE
            )));
        }

        let resolved = self.fs.resolve_path(path_str, &input.working_directory)?;

        // Reject symlinks: prevent writing through symlinks to escape sandbox.
        // If symlink_metadata fails (file doesn't exist), that's fine — we'll create it.
        if let Ok(meta) = self.fs.symlink_metadata(&resolved).await {
            if meta.is_symlink() {
                return Err(CuervoError::ToolExecutionFailed {
                    tool: "file_write".into(),
                    message: format!(
                        "refusing to write through symlink: {}",
                        resolved.display()
                    ),
                });
            }
        }

        let bytes = self.fs.atomic_write(&resolved, content.as_bytes()).await?;

        let mut output_text = format!("Wrote {} bytes to {}", bytes, resolved.display());

        // Run syntax verification on the written file.
        let path_str_for_check = resolved.display().to_string();
        let warnings = crate::syntax_check::check_syntax(content, &path_str_for_check);
        output_text.push_str(&crate::syntax_check::format_warnings(&warnings));

        let has_errors = warnings
            .iter()
            .any(|w| w.severity == crate::syntax_check::Severity::Error);

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: output_text,
            is_error: false,
            metadata: Some(json!({
                "bytes_written": bytes,
                "path": resolved.display().to_string(),
                "syntax_warnings": warnings.len(),
                "syntax_errors": has_errors,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to write to (absolute or relative to working directory)"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_service::FsService;
    use tempfile::TempDir;

    fn tool() -> FileWriteTool {
        FileWriteTool::new(Arc::new(FsService::new(vec![], vec![])))
    }

    fn make_input(dir: &str, args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test-id".into(),
            arguments: args,
            working_directory: dir.into(),
        }
    }

    #[tokio::test]
    async fn write_new_file() {
        let dir = TempDir::new().unwrap();
        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": "new.txt", "content": "hello world" }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("11 bytes"));

        let content = std::fs::read_to_string(dir.path().join("new.txt")).unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn write_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": "a/b/c/deep.txt", "content": "nested" }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);

        let content = std::fs::read_to_string(dir.path().join("a/b/c/deep.txt")).unwrap();
        assert_eq!(content, "nested");
    }

    #[tokio::test]
    async fn write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("exists.txt"), "old").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": "exists.txt", "content": "new" }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);

        let content = std::fs::read_to_string(dir.path().join("exists.txt")).unwrap();
        assert_eq!(content, "new");
    }

    #[tokio::test]
    async fn write_blocked_file() {
        let dir = TempDir::new().unwrap();
        let t = FileWriteTool::new(Arc::new(FsService::new(vec![], vec![".env".into()])));
        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": ".env", "content": "SECRET=x" }),
        );
        let result = t.execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn write_missing_content_arg() {
        let input = make_input("/tmp", json!({ "path": "test.txt" }));
        let result = tool().execute(input).await;
        assert!(result.is_err());
    }
}
