use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

use crate::fs_service::FsService;

/// Read the contents of a file.
pub struct FileReadTool {
    fs: Arc<FsService>,
}

impl FileReadTool {
    pub fn new(fs: Arc<FsService>) -> Self {
        Self { fs }
    }
}

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Supports optional line offset and limit."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let path_str = input.arguments["path"]
            .as_str()
            .ok_or_else(|| HalconError::InvalidInput("file_read requires 'path' string".into()))?;

        let resolved = self.fs.resolve_path(path_str, &input.working_directory)?;
        // Close TOCTOU: reject symlinks before reading, consistent with file_write.
        self.fs.check_not_symlink(&resolved).await?;

        let offset = input.arguments["offset"].as_u64().unwrap_or(0) as usize;
        let limit = input.arguments["limit"].as_u64().unwrap_or(0) as usize;

        // Use streaming read_lines when offset/limit specified, full read otherwise.
        if offset > 0 || limit > 0 {
            let (numbered, total) = self.fs.read_lines(&resolved, offset, limit).await?;
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: numbered,
                is_error: false,
                metadata: Some(json!({ "total_lines": total })),
            });
        }

        let content = self.fs.read_to_string(&resolved).await?;

        // Count total lines without collecting into Vec.
        let total = content.lines().count();

        // Build numbered output directly without intermediate Vec.
        let mut numbered = String::new();
        for (i, line) in content.lines().enumerate() {
            if i > 0 {
                numbered.push('\n');
            }
            use std::fmt::Write;
            let _ = write!(numbered, "{:>6}\t{}", i + 1, line);
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: numbered,
            is_error: false,
            metadata: Some(json!({ "total_lines": total })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to read (absolute or relative to working directory)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (0-indexed)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read"
                }
            },
            "required": ["path"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_service::FsService;
    use tempfile::TempDir;

    fn tool() -> FileReadTool {
        FileReadTool::new(Arc::new(FsService::new(vec![], vec![])))
    }

    fn make_input(dir: &str, args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test-id".into(),
            arguments: args,
            working_directory: dir.into(),
        }
    }

    #[tokio::test]
    async fn read_file_contents() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "line1\nline2\nline3\n").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap() }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("line1"));
        assert!(output.content.contains("line2"));
        assert!(output.content.contains("line3"));
    }

    #[tokio::test]
    async fn read_with_offset_and_limit() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("lines.txt");
        std::fs::write(&file, "a\nb\nc\nd\ne\n").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap(), "offset": 1, "limit": 2 }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(output.content.contains("b"));
        assert!(output.content.contains("c"));
        assert!(!output.content.contains("\ta\n"));
        assert!(!output.content.contains("\td\n"));
    }

    #[tokio::test]
    async fn read_missing_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": "nonexistent.txt" }),
        );
        let result = tool().execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_blocked_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join(".env");
        std::fs::write(&file, "SECRET=abc").unwrap();

        let t = FileReadTool::new(Arc::new(FsService::new(vec![], vec![".env".into()])));
        let input = make_input(dir.path().to_str().unwrap(), json!({ "path": ".env" }));
        let result = t.execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_missing_path_arg() {
        let input = make_input("/tmp", json!({}));
        let result = tool().execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_relative_path() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("file.txt"), "hello").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": "sub/file.txt" }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(output.content.contains("hello"));
    }

    #[tokio::test]
    async fn read_binary_file_is_handled_gracefully() {
        // Binary files may fail to_string conversion — the tool should either return
        // an error or produce some output, but NOT panic.
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("binary.bin");
        // Write bytes that are invalid UTF-8
        std::fs::write(&file, b"\xFF\xFE\x00\x01\x80\xC0\xFE\xFF").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap() }),
        );
        // Must not panic — either succeeds with replacement chars or returns an error
        let _ = tool().execute(input).await;
    }

    #[tokio::test]
    async fn read_permission_level_is_readonly() {
        use halcon_core::types::PermissionLevel;
        assert_eq!(tool().permission_level(), PermissionLevel::ReadOnly);
    }

    #[tokio::test]
    async fn read_does_not_require_confirmation() {
        let dummy = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(!tool().requires_confirmation(&dummy));
    }
}
