use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

const MAX_RESULTS: usize = 500;

/// Find files matching a glob pattern.
pub struct GlobTool;

impl GlobTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern. Returns matching file paths sorted by name."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let pattern = input.arguments["pattern"]
            .as_str()
            .ok_or_else(|| HalconError::InvalidInput("glob requires 'pattern' string".into()))?;

        let base_path = input.arguments["path"]
            .as_str()
            .unwrap_or(&input.working_directory);

        // Build the full glob pattern.
        let full_pattern = if std::path::Path::new(pattern).is_absolute() {
            pattern.to_string()
        } else {
            format!("{}/{}", base_path.trim_end_matches('/'), pattern)
        };

        let entries = glob::glob(&full_pattern).map_err(|e| HalconError::ToolExecutionFailed {
            tool: "glob".into(),
            message: format!("invalid glob pattern: {e}"),
        })?;

        let mut paths: Vec<String> = Vec::new();
        let mut error_count = 0;

        for entry in entries {
            match entry {
                Ok(path) => {
                    // Skip symlinks: prevent path traversal outside the intended search scope.
                    // glob::glob() follows symlinks by default; symlink_metadata() detects
                    // them without following, consistent with file_write/file_delete policy.
                    if std::fs::symlink_metadata(&path)
                        .ok()
                        .map(|m| m.file_type().is_symlink())
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    paths.push(path.display().to_string());
                    if paths.len() >= MAX_RESULTS {
                        break;
                    }
                }
                Err(_) => {
                    error_count += 1;
                }
            }
        }

        paths.sort();
        let total = paths.len();
        let truncated = total >= MAX_RESULTS;

        let content = if paths.is_empty() {
            "No matches found.".to_string()
        } else {
            paths.join("\n")
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "count": total,
                "truncated": truncated,
                "errors": error_count,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files (e.g. '**/*.html', 'src/**/*.js', '*.json')"
                },
                "path": {
                    "type": "string",
                    "description": "Base directory to search in (defaults to working directory)"
                }
            },
            "required": ["pattern"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_input(dir: &str, args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test-id".into(),
            arguments: args,
            working_directory: dir.into(),
        }
    }

    #[tokio::test]
    async fn glob_finds_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::write(dir.path().join("b.rs"), "").unwrap();
        std::fs::write(dir.path().join("c.txt"), "").unwrap();

        let input = make_input(dir.path().to_str().unwrap(), json!({ "pattern": "*.rs" }));
        let output = GlobTool::new().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("a.rs"));
        assert!(output.content.contains("b.rs"));
        assert!(!output.content.contains("c.txt"));
    }

    #[tokio::test]
    async fn glob_recursive() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("src");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("lib.rs"), "").unwrap();
        std::fs::write(dir.path().join("main.rs"), "").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "pattern": "**/*.rs" }),
        );
        let output = GlobTool::new().execute(input).await.unwrap();
        assert!(output.content.contains("lib.rs"));
        assert!(output.content.contains("main.rs"));
    }

    #[tokio::test]
    async fn glob_no_matches() {
        let dir = TempDir::new().unwrap();
        let input = make_input(dir.path().to_str().unwrap(), json!({ "pattern": "*.xyz" }));
        let output = GlobTool::new().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("No matches"));
    }

    #[tokio::test]
    async fn glob_with_base_path() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("file.txt"), "").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "pattern": "*.txt", "path": sub.to_str().unwrap() }),
        );
        let output = GlobTool::new().execute(input).await.unwrap();
        assert!(output.content.contains("file.txt"));
    }

    #[tokio::test]
    async fn glob_invalid_pattern() {
        let input = make_input("/tmp", json!({ "pattern": "[invalid" }));
        let result = GlobTool::new().execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn glob_excludes_symlinks() {
        // Symlinks must be excluded: glob follows them by default which can expose
        // paths outside the intended search directory (path traversal).
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("real.rs"), "fn main() {}").unwrap();
        let link = dir.path().join("link.rs");
        std::os::unix::fs::symlink(dir.path().join("real.rs"), &link).unwrap();

        let input = make_input(dir.path().to_str().unwrap(), json!({ "pattern": "*.rs" }));
        let output = GlobTool::new().execute(input).await.unwrap();

        assert!(
            output.content.contains("real.rs"),
            "real file should be included in glob results"
        );
        assert!(
            !output.content.contains("link.rs"),
            "symlink should be excluded from glob results, got: {}",
            output.content
        );
    }
}
