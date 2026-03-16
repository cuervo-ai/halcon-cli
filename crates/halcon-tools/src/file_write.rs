use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

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
            .ok_or_else(|| HalconError::InvalidInput("file_write requires 'path' string".into()))?;

        let content = input.arguments["content"].as_str().ok_or_else(|| {
            HalconError::InvalidInput("file_write requires 'content' string".into())
        })?;

        // Size limit: reject writes that could exhaust disk/memory.
        if content.len() > MAX_WRITE_SIZE {
            return Err(HalconError::InvalidInput(format!(
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
                return Err(HalconError::ToolExecutionFailed {
                    tool: "file_write".into(),
                    message: format!(
                        "refusing to write through symlink: {}",
                        resolved.display()
                    ),
                });
            }
        }

        let bytes = self.fs.atomic_write(&resolved, content.as_bytes()).await?;

        // Post-write verification: confirm the file actually exists on disk with the
        // expected byte count. Atomic rename can succeed but the file can be missing
        // in edge cases (cross-device rename failure, filesystem anomalies, NFS lag).
        // This turns silent data loss into an explicit error the agent can act on.
        match tokio::fs::metadata(&resolved).await {
            Ok(meta) => {
                let on_disk = meta.len();
                if on_disk != bytes {
                    return Err(HalconError::ToolExecutionFailed {
                        tool: "file_write".into(),
                        message: format!(
                            "post-write verification failed: wrote {} bytes but disk shows {} bytes at {}",
                            bytes, on_disk, resolved.display()
                        ),
                    });
                }
            }
            Err(e) => {
                return Err(HalconError::ToolExecutionFailed {
                    tool: "file_write".into(),
                    message: format!(
                        "post-write verification failed: file not found after write to {}: {e}",
                        resolved.display()
                    ),
                });
            }
        }

        let mut output_text = format!("Wrote {} bytes to {} [verified]", bytes, resolved.display());

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
            is_error: has_errors,
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

    #[tokio::test]
    async fn write_rejects_oversized_content() {
        // Content > 10 MB must be rejected
        let dir = TempDir::new().unwrap();
        let big_content = "x".repeat(11 * 1024 * 1024); // 11 MB
        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": "big.txt", "content": big_content }),
        );
        let result = tool().execute(input).await;
        assert!(result.is_err(), "writing >10MB should return an error");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn write_rejects_symlink_target() {
        // Writing to a symlink target is rejected (path traversal protection)
        let dir = TempDir::new().unwrap();
        let real_file = dir.path().join("real.txt");
        std::fs::write(&real_file, "original").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&real_file, &link).unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": "link.txt", "content": "hijack" }),
        );
        let result = tool().execute(input).await;
        assert!(result.is_err(), "writing to a symlink should be rejected");
        // Original file must be untouched
        assert_eq!(std::fs::read_to_string(&real_file).unwrap(), "original");
    }

    #[test]
    fn write_permission_level_is_destructive() {
        use halcon_core::types::PermissionLevel;
        assert_eq!(tool().permission_level(), PermissionLevel::Destructive);
    }

    #[test]
    fn write_requires_confirmation() {
        let dummy = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(tool().requires_confirmation(&dummy));
    }

    #[tokio::test]
    async fn atomic_write_no_temp_files_left_on_success() {
        // After a successful write, no temp files should remain in the directory
        let dir = TempDir::new().unwrap();
        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": "output.txt", "content": "clean write" }),
        );
        tool().execute(input).await.unwrap();

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            entries.iter().all(|n| !n.starts_with(".halcon_tmp_")),
            "no temp files should remain after successful write, found: {entries:?}"
        );
        assert!(entries.contains(&"output.txt".to_string()));
    }
}
