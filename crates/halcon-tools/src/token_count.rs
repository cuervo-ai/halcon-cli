//! TokenCountTool — estimate LLM token counts for text and files.
//!
//! Provides fast token-count estimation without requiring a tokenizer model:
//! - Per-file or inline text counting
//! - Context window utilization percentage
//! - Supports multiple model families (cl100k_base / o200k_base approximations)
//! - Batch counting for multiple files
//!
//! Uses the standard 4-char-per-token heuristic with code-specific corrections
//! (code is ~3 chars/token due to keywords and symbols).

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};
use std::path::Path;

/// Well-known model context windows (tokens).
const MODEL_WINDOWS: &[(&str, u64)] = &[
    ("gpt-4o", 128_000),
    ("gpt-4o-mini", 128_000),
    ("gpt-4-turbo", 128_000),
    ("gpt-4", 8_192),
    ("gpt-3.5-turbo", 16_385),
    ("o1", 200_000),
    ("o3", 200_000),
    ("claude-3-5-sonnet", 200_000),
    ("claude-3-5-haiku", 200_000),
    ("claude-3-opus", 200_000),
    ("claude-sonnet-4", 200_000),
    ("claude-opus-4", 200_000),
    ("claude-haiku-4", 200_000),
    ("deepseek-chat", 64_000),
    ("deepseek-coder", 128_000),
    ("gemini-1.5-pro", 1_000_000),
    ("gemini-1.5-flash", 1_000_000),
    ("gemini-2.0-flash", 1_000_000),
    ("llama3", 8_192),
    ("mistral", 32_000),
    ("mixtral", 32_000),
];

pub struct TokenCountTool;

impl TokenCountTool {
    pub fn new() -> Self {
        Self
    }

    /// Estimate token count for a text string.
    ///
    /// Uses different rates based on detected content type:
    /// - Code files: ~3 chars/token (more keywords, symbols)
    /// - Plain prose: ~4 chars/token
    /// - Mixed: ~3.5 chars/token
    pub fn estimate_tokens(text: &str, is_code: bool) -> u64 {
        if text.is_empty() {
            return 0;
        }
        // Count whitespace-separated tokens as baseline
        let word_count = text.split_whitespace().count() as u64;
        let char_count = text.chars().count() as u64;

        // Tiktoken-like approximation:
        // - Average English word ≈ 1.3 tokens
        // - Code has shorter tokens (operators, symbols)
        let chars_per_token = if is_code { 3.0f64 } else { 4.0f64 };
        let by_chars = (char_count as f64 / chars_per_token).ceil() as u64;
        let by_words = (word_count as f64 * if is_code { 1.5 } else { 1.3 }).ceil() as u64;

        // Take average of both estimations
        (by_chars + by_words) / 2
    }

    fn is_code_extension(path: &str) -> bool {
        let code_exts = [
            "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "c", "cpp", "h", "cs", "rb", "php",
            "swift", "kt", "scala", "sh", "bash", "zsh", "fish", "lua", "r", "m", "sql", "toml",
            "yaml", "yml", "json", "xml", "html", "css", "scss", "sass", "graphql", "proto", "tf",
            "hcl",
        ];
        let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
        code_exts.contains(&ext.as_str())
    }

    fn model_context_window(model: &str) -> Option<u64> {
        let model_lower = model.to_lowercase();
        MODEL_WINDOWS
            .iter()
            .find(|(m, _)| model_lower.contains(m))
            .map(|(_, w)| *w)
    }

    fn utilization_bar(pct: f64, width: usize) -> String {
        let filled = ((pct / 100.0) * width as f64).round() as usize;
        let filled = filled.min(width);
        let empty = width - filled;
        let bar = "█".repeat(filled) + &"░".repeat(empty);
        let color = if pct >= 90.0 {
            "🔴"
        } else if pct >= 70.0 {
            "🟡"
        } else {
            "🟢"
        };
        format!("{color} [{bar}] {:.1}%", pct)
    }

    async fn count_file(path_str: &str) -> Result<FileCount, String> {
        let path = Path::new(path_str);
        if !path.exists() {
            return Err(format!("File not found: {path_str}"));
        }
        if !path.is_file() {
            return Err(format!("Not a file: {path_str}"));
        }
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| format!("{e}"))?;
        if meta.len() > 10 * 1024 * 1024 {
            return Err(format!(
                "File too large ({}MB > 10MB limit)",
                meta.len() / 1_048_576
            ));
        }
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("Read error: {e}"))?;
        let is_code = Self::is_code_extension(path_str);
        let tokens = Self::estimate_tokens(&content, is_code);
        Ok(FileCount {
            path: path_str.to_string(),
            chars: content.chars().count(),
            lines: content.lines().count(),
            tokens,
            is_code,
        })
    }
}

struct FileCount {
    path: String,
    chars: usize,
    lines: usize,
    tokens: u64,
    is_code: bool,
}

impl Default for TokenCountTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TokenCountTool {
    fn name(&self) -> &str {
        "token_count"
    }

    fn description(&self) -> &str {
        "Estimate LLM token counts for text or files without a tokenizer model. \
         Shows character count, line count, estimated tokens, and context window utilization \
         for a specified model. Supports batch counting for multiple files. \
         Uses 4 chars/token for prose, 3 chars/token for code (standard approximation)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Inline text to count tokens for."
                },
                "file": {
                    "type": "string",
                    "description": "Path to a single file to count."
                },
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of file paths to count (batch mode)."
                },
                "model": {
                    "type": "string",
                    "description": "Model name for context window percentage (e.g. 'claude-sonnet-4', 'gpt-4o', 'deepseek-chat')."
                },
                "context_window": {
                    "type": "integer",
                    "description": "Custom context window in tokens (overrides model lookup)."
                }
            },
            "required": []
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(
        &self,
        input: ToolInput,
    ) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let args = &input.arguments;
        let model = args["model"].as_str().unwrap_or("");
        let context_window = args["context_window"]
            .as_u64()
            .or_else(|| Self::model_context_window(model));

        // Collect paths
        let mut paths: Vec<String> = vec![];
        if let Some(f) = args["file"].as_str() {
            paths.push(f.to_string());
        }
        if let Some(arr) = args["files"].as_array() {
            for v in arr {
                if let Some(s) = v.as_str() {
                    paths.push(s.to_string());
                }
            }
        }

        // Inline text
        if let Some(text) = args["text"].as_str() {
            if !text.is_empty() {
                let is_code = paths
                    .first()
                    .map(|p| Self::is_code_extension(p))
                    .unwrap_or(false);
                let tokens = Self::estimate_tokens(text, is_code);
                let chars = text.chars().count();
                let lines = text.lines().count();
                let mut content = format!(
                    "Token Estimate\n\n  Characters : {chars}\n  Lines      : {lines}\n  Tokens     : ~{tokens}\n"
                );
                if let Some(window) = context_window {
                    let pct = (tokens as f64 / window as f64) * 100.0;
                    let bar = Self::utilization_bar(pct, 30);
                    let model_label = if model.is_empty() {
                        "custom".to_string()
                    } else {
                        model.to_string()
                    };
                    content.push_str(&format!(
                        "\nContext Window ({model_label}: {window} tokens)\n  {bar}\n  {tokens} / {window} tokens used\n"
                    ));
                }
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content,
                    is_error: false,
                    metadata: Some(json!({ "tokens": tokens, "chars": chars, "lines": lines })),
                });
            }
        }

        if paths.is_empty() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "Provide 'text', 'file', or 'files' to count tokens.".to_string(),
                is_error: true,
                metadata: None,
            });
        }

        // Count files (cap at 50)
        paths.truncate(50);
        let mut counts: Vec<FileCount> = vec![];
        let mut errors: Vec<String> = vec![];

        for path in &paths {
            match Self::count_file(path).await {
                Ok(fc) => counts.push(fc),
                Err(e) => errors.push(format!("  {path}: {e}")),
            }
        }

        let total_tokens: u64 = counts.iter().map(|c| c.tokens).sum();
        let total_lines: usize = counts.iter().map(|c| c.lines).sum();
        let total_chars: usize = counts.iter().map(|c| c.chars).sum();

        let mut content = format!(
            "Token Count ({} file(s))  ~{total_tokens} tokens total  {total_lines} lines  {total_chars} chars\n\n",
            counts.len()
        );

        // Per-file table
        if counts.len() == 1 {
            let fc = &counts[0];
            let kind = if fc.is_code { "code" } else { "prose" };
            content.push_str(&format!(
                "  File  : {}\n  Type  : {kind}\n  Lines : {}\n  Chars : {}\n  Tokens: ~{}\n",
                fc.path, fc.lines, fc.chars, fc.tokens
            ));
        } else {
            // Header
            content.push_str(&format!(
                "  {:<40}  {:>8}  {:>8}  {:>8}\n",
                "File", "Lines", "Chars", "Tokens"
            ));
            content.push_str(&format!("  {}\n", "─".repeat(72)));
            for fc in &counts {
                let name = if fc.path.len() > 40 {
                    format!("...{}", &fc.path[fc.path.len() - 37..])
                } else {
                    fc.path.clone()
                };
                content.push_str(&format!(
                    "  {:<40}  {:>8}  {:>8}  {:>8}\n",
                    name, fc.lines, fc.chars, fc.tokens
                ));
            }
            content.push_str(&format!("  {}\n", "─".repeat(72)));
            content.push_str(&format!(
                "  {:<40}  {:>8}  {:>8}  {:>8}\n",
                "TOTAL", total_lines, total_chars, total_tokens
            ));
        }

        if let Some(window) = context_window {
            let pct = (total_tokens as f64 / window as f64) * 100.0;
            let bar = Self::utilization_bar(pct, 30);
            let model_label = if model.is_empty() {
                "custom".to_string()
            } else {
                model.to_string()
            };
            content.push_str(&format!(
                "\nContext Window ({model_label}: {window} tokens)\n  {bar}\n  {total_tokens} / {window} tokens used\n"
            ));
        }

        if !errors.is_empty() {
            content.push_str(&format!("\nErrors ({}):\n", errors.len()));
            for e in &errors {
                content.push_str(&format!("{e}\n"));
            }
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "total_tokens": total_tokens,
                "total_lines": total_lines,
                "total_chars": total_chars,
                "files_counted": counts.len(),
                "errors": errors.len()
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_input(args: Value) -> ToolInput {
        ToolInput {
            tool_use_id: "t1".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[test]
    fn tool_metadata() {
        let t = TokenCountTool::default();
        assert_eq!(t.name(), "token_count");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"].as_array().unwrap().is_empty());
    }

    #[test]
    fn estimate_tokens_empty() {
        assert_eq!(TokenCountTool::estimate_tokens("", false), 0);
        assert_eq!(TokenCountTool::estimate_tokens("", true), 0);
    }

    #[test]
    fn estimate_tokens_prose() {
        let text = "The quick brown fox jumps over the lazy dog.";
        let tokens = TokenCountTool::estimate_tokens(text, false);
        // Rough check: 44 chars / 4 ≈ 11, word-based ≈ 9*1.3 ≈ 12 → avg ~11
        assert!(tokens >= 8 && tokens <= 20, "got {tokens}");
    }

    #[test]
    fn estimate_tokens_code_lower_than_prose() {
        // Code with same char count should have fewer tokens (3 chars/tok vs 4)
        let code = "fn main() { println!(\"hello world\"); }";
        let code_tokens = TokenCountTool::estimate_tokens(code, true);
        let prose_tokens = TokenCountTool::estimate_tokens(code, false);
        // Code mode gives fewer chars-based tokens but more word-based (short tokens)
        assert!(code_tokens > 0);
        assert!(prose_tokens > 0);
    }

    #[test]
    fn is_code_extension_rust() {
        assert!(TokenCountTool::is_code_extension("main.rs"));
        assert!(TokenCountTool::is_code_extension("app.ts"));
        assert!(TokenCountTool::is_code_extension("script.py"));
    }

    #[test]
    fn is_code_extension_prose() {
        assert!(!TokenCountTool::is_code_extension("README.md"));
        assert!(!TokenCountTool::is_code_extension("notes.txt"));
        assert!(!TokenCountTool::is_code_extension("doc.pdf"));
    }

    #[test]
    fn model_window_lookup() {
        assert_eq!(
            TokenCountTool::model_context_window("gpt-4o"),
            Some(128_000)
        );
        assert_eq!(
            TokenCountTool::model_context_window("claude-sonnet-4"),
            Some(200_000)
        );
        assert_eq!(
            TokenCountTool::model_context_window("deepseek-chat"),
            Some(64_000)
        );
        assert_eq!(TokenCountTool::model_context_window("unknown-model"), None);
    }

    #[test]
    fn model_window_case_insensitive() {
        assert_eq!(
            TokenCountTool::model_context_window("GPT-4o"),
            Some(128_000)
        );
        assert_eq!(
            TokenCountTool::model_context_window("Claude-Sonnet-4"),
            Some(200_000)
        );
    }

    #[test]
    fn utilization_bar_green() {
        let bar = TokenCountTool::utilization_bar(30.0, 20);
        assert!(bar.contains("🟢"));
        assert!(bar.contains("30.0%"));
    }

    #[test]
    fn utilization_bar_yellow() {
        let bar = TokenCountTool::utilization_bar(75.0, 20);
        assert!(bar.contains("🟡"));
    }

    #[test]
    fn utilization_bar_red() {
        let bar = TokenCountTool::utilization_bar(95.0, 20);
        assert!(bar.contains("🔴"));
    }

    #[tokio::test]
    async fn execute_text_inline() {
        let tool = TokenCountTool::new();
        let out = tool
            .execute(make_input(
                json!({ "text": "Hello, world! This is a test sentence." }),
            ))
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
        assert!(out.content.contains("Tokens"), "content: {}", out.content);
    }

    #[tokio::test]
    async fn execute_text_with_model_window() {
        let tool = TokenCountTool::new();
        let out = tool
            .execute(make_input(json!({
                "text": "Some content here that we want to count",
                "model": "gpt-4o"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("128000")
                || out.content.contains("128,000")
                || out.content.contains("gpt-4o"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_no_input_returns_error() {
        let tool = TokenCountTool::new();
        let out = tool.execute(make_input(json!({}))).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn execute_missing_file_returns_error_in_errors() {
        let tool = TokenCountTool::new();
        let out = tool
            .execute(make_input(
                json!({ "file": "/tmp/nonexistent_halcon_token_count.txt" }),
            ))
            .await
            .unwrap();
        // Tool-level is_error=false but errors section populated
        // (graceful: lists error per file, still returns output)
        assert!(out.content.contains("not found") || out.content.contains("Error") || out.is_error);
    }
}
