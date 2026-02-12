use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

use crate::path_security;

/// Inspect any file: detect format, extract text, show metadata.
///
/// Unlike `file_read` (which only handles UTF-8 text), this tool handles
/// all file types: PDF, CSV, Excel, images, archives, XML, YAML, Markdown, etc.
pub struct FileInspectTool {
    allowed_dirs: Vec<std::path::PathBuf>,
    blocked_patterns: Vec<String>,
}

impl FileInspectTool {
    pub fn new(allowed_dirs: Vec<std::path::PathBuf>, blocked_patterns: Vec<String>) -> Self {
        Self {
            allowed_dirs,
            blocked_patterns,
        }
    }
}

#[async_trait]
impl Tool for FileInspectTool {
    fn name(&self) -> &str {
        "file_inspect"
    }

    fn description(&self) -> &str {
        "Inspect any file: detect format, extract text content, and show metadata. \
         Handles PDF, CSV, Excel, images, archives, XML, YAML, Markdown, JSON, \
         and all text/source code files. Returns format-specific metadata \
         (headers, dimensions, page count, schema, etc.) alongside extracted text."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let path_str = input.arguments["path"]
            .as_str()
            .ok_or_else(|| CuervoError::InvalidInput("file_inspect requires 'path' string".into()))?;

        let resolved = path_security::resolve_and_validate(
            path_str,
            &input.working_directory,
            &self.allowed_dirs,
            &self.blocked_patterns,
        )?;

        let token_budget = input.arguments["token_budget"]
            .as_u64()
            .unwrap_or(2000) as usize;

        let metadata_only = input.arguments["metadata_only"]
            .as_bool()
            .unwrap_or(false);

        let inspector = cuervo_files::FileInspector::new();

        // Detect file type first.
        let info = inspector.detect(&resolved).await.map_err(|e| {
            CuervoError::ToolExecutionFailed {
                tool: "file_inspect".into(),
                message: format!("detection failed: {e}"),
            }
        })?;

        if metadata_only {
            // Return only detection info, no content extraction.
            let output = format!(
                "File: {}\nType: {}\nMIME: {}\nSize: {} bytes\nBinary: {}\n",
                resolved.display(),
                info.file_type,
                info.mime_type.as_deref().unwrap_or("unknown"),
                info.size_bytes,
                info.is_binary,
            );
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: output,
                is_error: false,
                metadata: Some(json!({
                    "file_type": info.file_type.to_string(),
                    "mime_type": info.mime_type,
                    "size_bytes": info.size_bytes,
                    "is_binary": info.is_binary,
                })),
            });
        }

        // Extract content with the appropriate handler.
        let content = inspector
            .inspect_with_info(&info, token_budget)
            .await
            .map_err(|e| CuervoError::ToolExecutionFailed {
                tool: "file_inspect".into(),
                message: format!("extraction failed: {e}"),
            })?;

        // Budget utilization.
        let budget_used_pct = if token_budget > 0 {
            ((content.estimated_tokens as f64 / token_budget as f64) * 100.0).min(100.0)
        } else {
            100.0
        };

        // Build output with metadata header.
        let mut output = String::new();
        output.push_str(&format!(
            "File: {} ({})\nType: {} | Size: {} bytes | Tokens: ~{} ({:.0}% of budget)\n",
            resolved.display(),
            info.mime_type.as_deref().unwrap_or("unknown"),
            info.file_type,
            info.size_bytes,
            content.estimated_tokens,
            budget_used_pct,
        ));
        if content.truncated {
            output.push_str("[Content truncated to fit token budget]\n");
        }
        output.push_str("---\n");
        output.push_str(&content.text);

        // Merge budget utilization into metadata.
        let mut metadata = match content.metadata {
            serde_json::Value::Object(map) => map,
            other => {
                let mut map = serde_json::Map::new();
                map.insert("original".into(), other);
                map
            }
        };
        metadata.insert("token_budget".into(), json!(token_budget));
        metadata.insert("estimated_tokens".into(), json!(content.estimated_tokens));
        metadata.insert("budget_used_pct".into(), json!(budget_used_pct));
        metadata.insert("truncated".into(), json!(content.truncated));

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: output,
            is_error: false,
            metadata: Some(serde_json::Value::Object(metadata)),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path to inspect (absolute or relative to working directory)"
                },
                "token_budget": {
                    "type": "integer",
                    "description": "Maximum tokens for extracted content (default: 2000)"
                },
                "metadata_only": {
                    "type": "boolean",
                    "description": "If true, return only file type and metadata without content extraction"
                }
            },
            "required": ["path"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tool() -> FileInspectTool {
        FileInspectTool::new(vec![], vec![])
    }

    fn make_input(dir: &str, args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test-id".into(),
            arguments: args,
            working_directory: dir.into(),
        }
    }

    #[tokio::test]
    async fn inspect_text_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "Hello, world!").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap() }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("Hello, world!"));
        assert!(output.content.contains("Type: text"));
    }

    #[tokio::test]
    async fn inspect_json_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("data.json");
        std::fs::write(&file, r#"{"name": "cuervo", "version": 1}"#).unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap() }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("json"));
        assert!(output.metadata.as_ref().unwrap()["valid"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn inspect_metadata_only() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap(), "metadata_only": true }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("Type: source:rust"));
        // Should NOT contain the actual source code in metadata-only mode.
        assert!(!output.content.contains("fn main()"));
    }

    #[tokio::test]
    async fn inspect_with_token_budget() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("big.txt");
        std::fs::write(&file, "x".repeat(50_000)).unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap(), "token_budget": 10 }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("truncated"));
    }

    #[tokio::test]
    async fn inspect_blocked_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join(".env");
        std::fs::write(&file, "SECRET=abc").unwrap();

        let t = FileInspectTool::new(vec![], vec![".env".into()]);
        let input = make_input(dir.path().to_str().unwrap(), json!({ "path": ".env" }));
        let result = t.execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn inspect_missing_path_arg() {
        let input = make_input("/tmp", json!({}));
        let result = tool().execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn inspect_nonexistent_file() {
        let dir = TempDir::new().unwrap();
        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": "nonexistent.txt" }),
        );
        let result = tool().execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn inspect_source_code() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("app.py");
        std::fs::write(&file, "def hello():\n    print('hello')").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap() }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("source:python"));
        assert!(output.content.contains("def hello()"));
    }

    #[tokio::test]
    async fn inspect_empty_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("empty.txt");
        std::fs::write(&file, "").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap() }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
    }

    #[tokio::test]
    async fn inspect_with_small_budget() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("big.txt");
        std::fs::write(&file, "x".repeat(50_000)).unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap(), "token_budget": 5 }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("truncated"));
    }

    #[tokio::test]
    async fn inspect_default_budget() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("small.txt");
        std::fs::write(&file, "hello world").unwrap();

        // No token_budget arg — should use default of 2000
        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap() }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("hello world"));
    }

    #[tokio::test]
    async fn inspect_relative_path() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("sub").join("test.txt");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(&file, "relative path test").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": "sub/test.txt" }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("relative path test"));
    }

    #[test]
    fn tool_schema_has_required_path() {
        let t = tool();
        let schema = t.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("path")));
    }

    #[test]
    fn tool_permission_is_readonly() {
        let t = tool();
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
    }

    #[test]
    fn tool_does_not_require_confirmation() {
        let t = tool();
        let dummy = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(!t.requires_confirmation(&dummy));
    }

    #[tokio::test]
    async fn inspect_includes_budget_utilization() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("data.txt");
        std::fs::write(&file, "hello world").unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap(), "token_budget": 100 }),
        );
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);

        // Check metadata has budget fields
        let meta = output.metadata.unwrap();
        assert_eq!(meta["token_budget"], 100);
        assert!(meta["estimated_tokens"].as_u64().is_some());
        assert!(meta["budget_used_pct"].as_f64().is_some());
        assert!(!meta["truncated"].as_bool().unwrap());

        // Check output includes budget percentage
        assert!(output.content.contains("% of budget"));
    }

    #[tokio::test]
    async fn inspect_budget_100_percent_when_truncated() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("big.txt");
        std::fs::write(&file, "x".repeat(50_000)).unwrap();

        let input = make_input(
            dir.path().to_str().unwrap(),
            json!({ "path": file.to_str().unwrap(), "token_budget": 5 }),
        );
        let output = tool().execute(input).await.unwrap();

        let meta = output.metadata.unwrap();
        assert!(meta["truncated"].as_bool().unwrap());
        // Budget should be near 100% when truncated
        let pct = meta["budget_used_pct"].as_f64().unwrap();
        assert!(pct > 50.0, "budget_used_pct should be significant: {pct}");
    }
}
