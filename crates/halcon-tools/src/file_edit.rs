use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

use crate::fs_service::{FsService, MAX_WRITE_SIZE};

/// Maximum source file size for edits (10 MB).
const MAX_EDIT_FILE_SIZE: usize = MAX_WRITE_SIZE;

/// Edit a file by replacing exact string matches.
pub struct FileEditTool {
    fs: Arc<FsService>,
}

impl FileEditTool {
    pub fn new(fs: Arc<FsService>) -> Self {
        Self { fs }
    }
}

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "file_edit"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing an exact string match with new content. The old_string must be unique in the file unless replace_all is true."
    }

    fn permission_level(&self) -> PermissionLevel {
        // Destructive: file_edit performs a read-modify-write cycle that can
        // corrupt files on crash. Requires user confirmation in interactive mode.
        PermissionLevel::Destructive
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let path_str = input.arguments["path"]
            .as_str()
            .ok_or_else(|| HalconError::InvalidInput("file_edit requires 'path' string".into()))?;
        let old_string = input.arguments["old_string"].as_str().ok_or_else(|| {
            HalconError::InvalidInput("file_edit requires 'old_string' string".into())
        })?;
        let new_string = input.arguments["new_string"].as_str().ok_or_else(|| {
            HalconError::InvalidInput("file_edit requires 'new_string' string".into())
        })?;
        if old_string.is_empty() {
            return Err(HalconError::InvalidInput(
                "file_edit: old_string must not be empty".into(),
            ));
        }
        let replace_all = input.arguments["replace_all"].as_bool().unwrap_or(false);

        let resolved = self.fs.resolve_path(path_str, &input.working_directory)?;

        // Reject symlinks: prevent editing through symlinks to escape sandbox.
        let meta = self.fs.symlink_metadata(&resolved).await.map_err(|_e| {
            HalconError::ToolExecutionFailed {
                tool: "file_edit".into(),
                message: format!("failed to stat {}: file not found", resolved.display()),
            }
        })?;

        if meta.is_symlink() {
            return Err(HalconError::ToolExecutionFailed {
                tool: "file_edit".into(),
                message: format!("refusing to edit through symlink: {}", resolved.display()),
            });
        }

        // Size limit: reject files too large to edit safely in memory.
        if meta.len() as usize > MAX_EDIT_FILE_SIZE {
            return Err(HalconError::InvalidInput(format!(
                "file_edit: file size {} bytes exceeds limit of {} bytes",
                meta.len(),
                MAX_EDIT_FILE_SIZE
            )));
        }

        let content = self.fs.read_to_string(&resolved).await?;

        let match_count = content.matches(old_string).count();

        if match_count == 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("Error: old_string not found in {}", resolved.display()),
                is_error: true,
                metadata: None,
            });
        }

        if !replace_all && match_count > 1 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!(
                    "Error: old_string has {} matches in {} — provide more context to make it unique or use replace_all",
                    match_count,
                    resolved.display()
                ),
                is_error: true,
                metadata: None,
            });
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        self.fs
            .atomic_write(&resolved, new_content.as_bytes())
            .await?;

        let replacements = if replace_all { match_count } else { 1 };
        let mut output_text = format!(
            "Replaced {} occurrence(s) in {}",
            replacements,
            resolved.display()
        );

        // Run syntax verification on the resulting file.
        let path_str_for_check = resolved.display().to_string();
        let warnings = crate::syntax_check::check_syntax(&new_content, &path_str_for_check);
        output_text.push_str(&crate::syntax_check::format_warnings(&warnings));

        let has_errors = warnings
            .iter()
            .any(|w| w.severity == crate::syntax_check::Severity::Error);

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: output_text,
            is_error: has_errors,
            metadata: Some(json!({
                "replacements": replacements,
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
                    "description": "The file path to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact string to find and replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement string"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences instead of requiring uniqueness"
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_service::FsService;
    use tempfile::TempDir;

    fn tool() -> FileEditTool {
        FileEditTool::new(Arc::new(FsService::new(vec![], vec![])))
    }

    fn make_input(dir: &str, args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test-id".into(),
            arguments: args,
            working_directory: dir.into(),
        }
    }

    #[tokio::test]
    async fn edit_unique_match() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(&file, "fn hello() {}\nfn world() {}\n").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "fn hello() {}",
                "new_string": "fn greet() {}"
            }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);

        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("fn greet() {}"));
        assert!(!content.contains("fn hello() {}"));
    }

    #[tokio::test]
    async fn edit_non_unique_without_replace_all() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "foo bar foo baz foo").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "foo",
                "new_string": "qux"
            }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(output.is_error);
        assert!(output.content.contains("3 matches"));
    }

    #[tokio::test]
    async fn edit_replace_all() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "foo bar foo baz foo").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "foo",
                "new_string": "qux",
                "replace_all": true
            }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("3 occurrence(s)"));

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "qux bar qux baz qux");
    }

    #[tokio::test]
    async fn edit_no_match() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "xyz",
                "new_string": "abc"
            }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(output.is_error);
        assert!(output.content.contains("not found"));
    }

    #[tokio::test]
    async fn edit_preserves_rest_of_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "aaa\nbbb\nccc\n").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "bbb",
                "new_string": "BBB"
            }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "aaa\nBBB\nccc\n");
    }

    #[tokio::test]
    async fn edit_blocked_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join(".env");
        std::fs::write(&file, "SECRET=abc").unwrap();

        let t = FileEditTool::new(Arc::new(FsService::new(vec![], vec![".env".into()])));
        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({
                "path": ".env",
                "old_string": "abc",
                "new_string": "xyz"
            }),
        );
        let result = t.execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn edit_missing_old_string_arg() {
        let input = make_input("/tmp", json!({ "path": "test.txt", "new_string": "x" }));
        let result = tool().execute(input).await;
        assert!(result.is_err());
    }

    // === Phase 30: Fix 5a — reject empty old_string ===

    #[tokio::test]
    async fn edit_empty_old_string_rejected() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({
                "path": file.to_str().unwrap(),
                "old_string": "",
                "new_string": "replacement"
            }),
        );
        let result = tool().execute(input).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("old_string must not be empty"), "Error: {err}");
    }
}
