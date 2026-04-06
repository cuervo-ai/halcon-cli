use async_trait::async_trait;
use serde_json::json;
use std::path::Path;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

const MAX_RESULTS: usize = 200;

/// Search file contents using regex patterns.
pub struct GrepTool;

impl GrepTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents for lines matching a regex pattern. Returns matching lines with file paths and line numbers."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let pattern_str = input.arguments["pattern"]
            .as_str()
            .ok_or_else(|| HalconError::InvalidInput("grep requires 'pattern' string".into()))?;

        let regex =
            regex::Regex::new(pattern_str).map_err(|e| HalconError::ToolExecutionFailed {
                tool: "grep".into(),
                message: format!("invalid regex: {e}"),
            })?;

        let base_path = input.arguments["path"]
            .as_str()
            .unwrap_or(&input.working_directory);

        let file_glob = input.arguments["glob"].as_str().unwrap_or("**/*");

        let context_lines = input.arguments["context"].as_u64().unwrap_or(0) as usize;

        let full_pattern = if Path::new(file_glob).is_absolute() {
            file_glob.to_string()
        } else {
            format!("{}/{}", base_path.trim_end_matches('/'), file_glob)
        };

        let entries = glob::glob(&full_pattern).map_err(|e| HalconError::ToolExecutionFailed {
            tool: "grep".into(),
            message: format!("invalid file glob: {e}"),
        })?;

        let mut results: Vec<String> = Vec::new();
        let mut files_searched = 0u32;
        let mut total_matches = 0u32;

        for entry in entries {
            let path = match entry {
                Ok(p) if p.is_file() => p,
                _ => continue,
            };

            // Non-blocking file read (avoids blocking the tokio worker thread).
            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            files_searched += 1;
            let path_display = path.display().to_string();

            if context_lines > 0 {
                // Context mode: need random access so collect lines into Vec.
                let lines: Vec<&str> = content.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    if regex.is_match(line) {
                        total_matches += 1;
                        if results.len() >= MAX_RESULTS {
                            break;
                        }
                        let start = i.saturating_sub(context_lines);
                        let end = (i + context_lines + 1).min(lines.len());
                        for (j, line_text) in lines.iter().enumerate().take(end).skip(start) {
                            let marker = if j == i { ">" } else { " " };
                            results.push(format!(
                                "{}{}:{}: {}",
                                marker,
                                path_display,
                                j + 1,
                                line_text
                            ));
                        }
                        results.push("--".to_string());
                    }
                }
            } else {
                // No context: iterate lines directly without collecting into Vec.
                for (i, line) in content.lines().enumerate() {
                    if regex.is_match(line) {
                        total_matches += 1;
                        if results.len() >= MAX_RESULTS {
                            break;
                        }
                        results.push(format!("{}:{}: {}", path_display, i + 1, line));
                    }
                }
            }

            if results.len() >= MAX_RESULTS {
                break;
            }
        }

        let truncated = results.len() >= MAX_RESULTS;
        let content = if results.is_empty() {
            "No matches found.".to_string()
        } else {
            results.join("\n")
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "total_matches": total_matches,
                "files_searched": files_searched,
                "truncated": truncated,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for in file contents"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (defaults to working directory)"
                },
                "glob": {
                    "type": "string",
                    "description": "File glob pattern to filter files (e.g. '**/*.rs', default: '**/*')"
                },
                "context": {
                    "type": "integer",
                    "description": "Number of context lines to show before and after each match"
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
    async fn grep_finds_match() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.rs"), "fn main() {}\nfn hello() {}\n").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "pattern": "fn hello" }),
        );
        let output = GrepTool::new().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("fn hello"));
        assert!(output.content.contains(":2:"));
    }

    #[tokio::test]
    async fn grep_no_match() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world\n").unwrap();

        let input = make_input(dir.path().to_str().unwrap(), json!({ "pattern": "xyz123" }));
        let output = GrepTool::new().execute(input).await.unwrap();
        assert!(output.content.contains("No matches"));
    }

    #[tokio::test]
    async fn grep_with_file_glob() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn test() {}\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "fn test() {}\n").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "pattern": "fn test", "glob": "*.rs" }),
        );
        let output = GrepTool::new().execute(input).await.unwrap();
        assert!(output.content.contains("a.rs"));
        assert!(!output.content.contains("b.txt"));
    }

    #[tokio::test]
    async fn grep_with_context() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("test.txt"),
            "line1\nline2\nTARGET\nline4\nline5\n",
        )
        .unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "pattern": "TARGET", "context": 1 }),
        );
        let output = GrepTool::new().execute(input).await.unwrap();
        assert!(output.content.contains("line2"));
        assert!(output.content.contains("TARGET"));
        assert!(output.content.contains("line4"));
    }

    #[tokio::test]
    async fn grep_invalid_regex() {
        let input = make_input("/tmp", json!({ "pattern": "[invalid" }));
        let result = GrepTool::new().execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn grep_regex_pattern() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "foo123\nbar456\nfoo789\n").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "pattern": "foo\\d+" }),
        );
        let output = GrepTool::new().execute(input).await.unwrap();
        assert!(output.content.contains("foo123"));
        assert!(output.content.contains("foo789"));
        assert!(!output.content.contains("bar456"));
    }
}
