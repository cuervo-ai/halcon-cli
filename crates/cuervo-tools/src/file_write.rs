use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

use crate::path_security;

/// Maximum allowed file size for writes (10 MB).
const MAX_WRITE_SIZE: usize = 10 * 1024 * 1024;

/// Write content to a file, creating it if it doesn't exist.
pub struct FileWriteTool {
    allowed_dirs: Vec<std::path::PathBuf>,
    blocked_patterns: Vec<String>,
}

impl FileWriteTool {
    pub fn new(allowed_dirs: Vec<std::path::PathBuf>, blocked_patterns: Vec<String>) -> Self {
        Self {
            allowed_dirs,
            blocked_patterns,
        }
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

        let resolved = path_security::resolve_and_validate(
            path_str,
            &input.working_directory,
            &self.allowed_dirs,
            &self.blocked_patterns,
        )?;

        // Reject symlinks: prevent writing through symlinks to escape sandbox.
        if let Ok(meta) = tokio::fs::symlink_metadata(&resolved).await {
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
        // If symlink_metadata fails (file doesn't exist), that's fine — we'll create it.

        // Create parent directories if needed.
        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                CuervoError::ToolExecutionFailed {
                    tool: "file_write".into(),
                    message: format!("failed to create directories: {e}"),
                }
            })?;
        }

        let bytes = content.len();

        // Atomic write: temp file in same directory + fsync + rename.
        // This ensures the file is never in a partially-written state.
        let parent_dir = resolved.parent().unwrap_or(std::path::Path::new("."));
        let temp_path = parent_dir.join(format!(
            ".cuervo_tmp_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));

        // Write to temp file.
        tokio::fs::write(&temp_path, content).await.map_err(|e| {
            CuervoError::ToolExecutionFailed {
                tool: "file_write".into(),
                message: format!("failed to write temp file {}: {e}", temp_path.display()),
            }
        })?;

        // Fsync the temp file for durability.
        let temp_path_for_sync = temp_path.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let f = std::fs::File::open(&temp_path_for_sync)?;
            f.sync_all()?;
            Ok(())
        })
        .await
        .map_err(|e| CuervoError::ToolExecutionFailed {
            tool: "file_write".into(),
            message: format!("fsync task failed: {e}"),
        })? {
            // Clean up temp file on fsync failure.
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(CuervoError::ToolExecutionFailed {
                tool: "file_write".into(),
                message: format!("failed to fsync: {e}"),
            });
        }

        // Atomic rename: replaces target file in one operation.
        if let Err(e) = tokio::fs::rename(&temp_path, &resolved).await {
            // Clean up temp file on rename failure.
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(CuervoError::ToolExecutionFailed {
                tool: "file_write".into(),
                message: format!("failed to write {}: {e}", resolved.display()),
            });
        }

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
    use tempfile::TempDir;

    fn tool() -> FileWriteTool {
        FileWriteTool::new(vec![], vec![])
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
        let t = FileWriteTool::new(vec![], vec![".env".into()]);
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
