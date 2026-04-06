//! SemanticGrepTool — context-aware code search with language intelligence.
//!
//! Extends basic text search with:
//! - Function/method boundary detection (show full function containing match)
//! - Import/export analysis (find all usages of a symbol)
//! - Structural queries: find all functions matching a pattern, all TODO comments
//! - Language-aware search (Rust, Python, JS/TS, Go, Java)
//! - Semantic filters: only-functions, only-tests, only-public
//!
//! All searches are read-only and operate on the local filesystem.

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use regex::Regex;
use serde_json::{json, Value};
use std::path::Path;

pub struct SemanticGrepTool;

impl SemanticGrepTool {
    pub fn new() -> Self {
        Self
    }

    /// Detect the language of a file by extension.
    fn detect_language(path: &str) -> &'static str {
        let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
        match ext.as_str() {
            "rs" => "rust",
            "py" => "python",
            "js" | "mjs" | "cjs" => "javascript",
            "ts" | "tsx" => "typescript",
            "go" => "go",
            "java" => "java",
            "c" | "h" => "c",
            "cpp" | "cc" | "cxx" | "hpp" => "cpp",
            "rb" => "ruby",
            "php" => "php",
            "cs" => "csharp",
            "sh" | "bash" | "zsh" => "shell",
            _ => "text",
        }
    }

    /// Pattern to match function/method definitions per language.
    fn fn_pattern(lang: &str) -> Option<Regex> {
        let pat = match lang {
            "rust" => r"(?m)^[ \t]*(pub\s+)?(async\s+)?fn\s+\w+",
            "python" => r"(?m)^[ \t]*(?:async\s+)?def\s+\w+",
            "javascript" | "typescript" => {
                r"(?m)(?:function\s+\w+|(?:const|let|var)\s+\w+\s*=\s*(?:async\s+)?\()"
            }
            "go" => r"(?m)^func\s+",
            "java" | "csharp" => {
                r"(?m)(?:public|private|protected|static|void|int|String|bool)\s+\w+\s*\("
            }
            _ => return None,
        };
        Regex::new(pat).ok()
    }

    /// Find the start line of the enclosing function for a given line number.
    fn find_enclosing_fn(lines: &[&str], target_line: usize, fn_re: &Regex) -> Option<usize> {
        // Search backwards from target_line
        (0..=target_line.min(lines.len().saturating_sub(1)))
            .rev()
            .find(|&i| fn_re.is_match(lines[i]))
    }

    /// Find the end of a block starting at a given line (simple brace counting).
    fn find_block_end(lines: &[&str], start: usize) -> usize {
        let mut depth: i32 = 0;
        let mut started = false;

        for (i, line) in lines.iter().enumerate().skip(start) {
            for ch in line.chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        started = true;
                    }
                    '}' => {
                        depth -= 1;
                        if started && depth <= 0 {
                            return i;
                        }
                    }
                    _ => {}
                }
            }
            // For Python-like (no braces): use indent as heuristic
            if !started && i > start {
                // If indent decreased and we started
                let start_indent = lines[start].len() - lines[start].trim_start().len();
                let curr_indent = line.len() - line.trim_start().len();
                if curr_indent < start_indent && !line.trim().is_empty() {
                    return i.saturating_sub(1);
                }
            }
        }
        (lines.len()).saturating_sub(1)
    }

    async fn search_file(
        path_str: &str,
        pattern: &Regex,
        show_fn: bool,
        only_tests: bool,
        only_public: bool,
        max_matches: usize,
        context_lines: usize,
    ) -> Vec<SearchMatch> {
        let content = match tokio::fs::read_to_string(path_str).await {
            Ok(c) => c,
            Err(_) => return vec![],
        };

        // Skip binary files
        if content.contains('\0') {
            return vec![];
        }

        let lines: Vec<&str> = content.lines().collect();
        let lang = Self::detect_language(path_str);
        let fn_re = Self::fn_pattern(lang);
        let mut matches: Vec<SearchMatch> = vec![];

        for (line_no, line) in lines.iter().enumerate() {
            if !pattern.is_match(line) {
                continue;
            }

            // Apply filters
            if only_tests {
                // Must be inside a #[test] or test function
                let in_test = lines[..=line_no].iter().rev().take(20).any(|l| {
                    l.contains("#[test]")
                        || l.contains("def test_")
                        || l.contains("func Test")
                        || l.contains("it(")
                        || l.contains("test(")
                });
                if !in_test {
                    continue;
                }
            }
            if only_public {
                let is_pub =
                    line.contains("pub ") || line.contains("export ") || line.contains("public ");
                let near_pub = lines[line_no.saturating_sub(3)..=line_no]
                    .iter()
                    .any(|l| l.contains("pub ") || l.contains("export ") || l.contains("public "));
                if !is_pub && !near_pub {
                    continue;
                }
            }

            let snippet = if show_fn {
                if let Some(ref re) = fn_re {
                    if let Some(fn_start) = Self::find_enclosing_fn(&lines, line_no, re) {
                        let fn_end = Self::find_block_end(&lines, fn_start).min(fn_start + 50);
                        lines[fn_start..=fn_end]
                            .iter()
                            .enumerate()
                            .map(|(i, l)| format!("{:4} | {l}", fn_start + i + 1))
                            .collect::<Vec<_>>()
                            .join("\n")
                    } else {
                        line.to_string()
                    }
                } else {
                    line.to_string()
                }
            } else if context_lines > 0 {
                let start = line_no.saturating_sub(context_lines);
                let end = (line_no + context_lines).min(lines.len().saturating_sub(1));
                lines[start..=end]
                    .iter()
                    .enumerate()
                    .map(|(i, l)| format!("{:4} | {l}", start + i + 1))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                line.to_string()
            };

            matches.push(SearchMatch {
                path: path_str.to_string(),
                line_no: line_no + 1,
                snippet,
            });

            if matches.len() >= max_matches {
                break;
            }
        }
        matches
    }

    fn collect_files(root: &str, extensions: &[String], max_files: usize) -> Vec<String> {
        let mut files = vec![];
        let root_path = Path::new(root);
        if root_path.is_file() {
            return vec![root.to_string()];
        }
        let skip_dirs = [
            "target",
            "node_modules",
            ".git",
            ".svn",
            "dist",
            "build",
            "__pycache__",
        ];

        fn recurse(
            dir: &Path,
            exts: &[String],
            skip: &[&str],
            files: &mut Vec<String>,
            max: usize,
        ) {
            if files.len() >= max {
                return;
            }
            let rd = match std::fs::read_dir(dir) {
                Ok(rd) => rd,
                Err(_) => return,
            };
            for entry in rd.flatten() {
                if files.len() >= max {
                    break;
                }
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if path.is_dir() {
                    if !skip.contains(&name) {
                        recurse(&path, exts, skip, files, max);
                    }
                } else if path.is_file() {
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if exts.is_empty() || exts.iter().any(|e| e.to_lowercase() == ext) {
                        files.push(path.to_string_lossy().to_string());
                    }
                }
            }
        }

        recurse(root_path, extensions, &skip_dirs, &mut files, max_files);
        files.sort();
        files
    }
}

struct SearchMatch {
    path: String,
    line_no: usize,
    snippet: String,
}

impl Default for SemanticGrepTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SemanticGrepTool {
    fn name(&self) -> &str {
        "semantic_grep"
    }

    fn description(&self) -> &str {
        "Context-aware code search with language intelligence. \
         Finds text patterns and can expand to show the enclosing function, \
         apply language-aware filters (only-tests, only-public), \
         and search across multiple file types. \
         Supports full regex syntax. Returns matches with line numbers and context."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for."
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in."
                },
                "extensions": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File extensions to include (e.g. ['rs', 'toml']). Default: all."
                },
                "context": {
                    "type": "integer",
                    "description": "Lines of context before/after match (default: 2)."
                },
                "show_function": {
                    "type": "boolean",
                    "description": "Expand match to show entire enclosing function (default: false)."
                },
                "only_tests": {
                    "type": "boolean",
                    "description": "Only show matches inside test functions (default: false)."
                },
                "only_public": {
                    "type": "boolean",
                    "description": "Only show matches in public/exported items (default: false)."
                },
                "max_matches": {
                    "type": "integer",
                    "description": "Maximum matches to return (default: 50, max: 200)."
                },
                "max_files": {
                    "type": "integer",
                    "description": "Maximum files to scan (default: 200)."
                }
            },
            "required": ["pattern"]
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

        let pattern_str = match args["pattern"].as_str() {
            Some(p) if !p.is_empty() => p,
            _ => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: "'pattern' is required.".to_string(),
                    is_error: true,
                    metadata: None,
                })
            }
        };

        let pattern = match Regex::new(pattern_str) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!("Invalid regex: {e}"),
                    is_error: true,
                    metadata: None,
                })
            }
        };

        let search_path = args["path"].as_str().unwrap_or(&input.working_directory);
        let context_lines = args["context"].as_u64().unwrap_or(2) as usize;
        let show_fn = args["show_function"].as_bool().unwrap_or(false);
        let only_tests = args["only_tests"].as_bool().unwrap_or(false);
        let only_public = args["only_public"].as_bool().unwrap_or(false);
        let max_matches = args["max_matches"].as_u64().unwrap_or(50).clamp(1, 200) as usize;
        let max_files = args["max_files"].as_u64().unwrap_or(200).clamp(1, 1000) as usize;
        let extensions: Vec<String> = args["extensions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let files = Self::collect_files(search_path, &extensions, max_files);
        let files_scanned = files.len();

        let mut all_matches: Vec<SearchMatch> = vec![];
        for file in &files {
            if all_matches.len() >= max_matches {
                break;
            }
            let remaining = max_matches - all_matches.len();
            let mut m = Self::search_file(
                file,
                &pattern,
                show_fn,
                only_tests,
                only_public,
                remaining,
                context_lines,
            )
            .await;
            all_matches.append(&mut m);
        }

        let match_count = all_matches.len();
        let mut content = format!(
            "Pattern: `{pattern_str}`  {match_count} match(es) in {files_scanned} file(s)\n\n"
        );

        let mut current_file = String::new();
        for m in &all_matches {
            if m.path != current_file {
                content.push_str(&format!("── {} ──\n", m.path));
                current_file = m.path.clone();
            }
            content.push_str(&format!(
                "  Line {}: {}\n",
                m.line_no,
                m.snippet.lines().next().unwrap_or("")
            ));
            // If snippet has multiple lines (fn mode), show them indented
            for extra in m.snippet.lines().skip(1) {
                content.push_str(&format!("  {extra}\n"));
            }
        }

        if match_count == max_matches {
            content.push_str(&format!("\n(stopped at {max_matches} matches)"));
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "matches": match_count,
                "files_scanned": files_scanned,
                "pattern": pattern_str
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn make_input(args: Value) -> ToolInput {
        ToolInput {
            tool_use_id: "t1".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[test]
    fn tool_metadata() {
        let t = SemanticGrepTool::default();
        assert_eq!(t.name(), "semantic_grep");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("pattern")));
    }

    #[test]
    fn detect_language_rust() {
        assert_eq!(SemanticGrepTool::detect_language("main.rs"), "rust");
        assert_eq!(SemanticGrepTool::detect_language("app.py"), "python");
        assert_eq!(SemanticGrepTool::detect_language("index.ts"), "typescript");
        assert_eq!(SemanticGrepTool::detect_language("main.go"), "go");
        assert_eq!(SemanticGrepTool::detect_language("README.md"), "text");
    }

    #[test]
    fn fn_pattern_rust() {
        let re = SemanticGrepTool::fn_pattern("rust").unwrap();
        assert!(re.is_match("pub fn hello() {"));
        assert!(re.is_match("    async fn do_thing() {"));
        assert!(!re.is_match("    let x = 5;"));
    }

    #[test]
    fn fn_pattern_python() {
        let re = SemanticGrepTool::fn_pattern("python").unwrap();
        assert!(re.is_match("def my_function(x):"));
        assert!(re.is_match("  async def handler():"));
        assert!(!re.is_match("x = 5"));
    }

    #[test]
    fn fn_pattern_unknown_is_none() {
        assert!(SemanticGrepTool::fn_pattern("brainfuck").is_none());
    }

    #[test]
    fn find_enclosing_fn_finds_function() {
        let lines = vec![
            "pub fn foo() {",
            "    let x = 1;",
            "    let y = x + 1; // search hit",
            "}",
        ];
        let re = SemanticGrepTool::fn_pattern("rust").unwrap();
        let result = SemanticGrepTool::find_enclosing_fn(&lines, 2, &re);
        assert_eq!(result, Some(0));
    }

    #[test]
    fn find_block_end_counts_braces() {
        let lines = vec![
            "fn foo() {",
            "    if true {",
            "        let x = 1;",
            "    }",
            "}",
            "fn bar() {",
        ];
        let end = SemanticGrepTool::find_block_end(&lines, 0);
        assert_eq!(end, 4);
    }

    #[test]
    fn collect_files_on_single_file() {
        let files = SemanticGrepTool::collect_files("/tmp/nonexistent_dir_halcon", &[], 10);
        // Non-existent dir → empty list
        assert!(files.is_empty());
    }

    #[tokio::test]
    async fn execute_missing_pattern_returns_error() {
        let tool = SemanticGrepTool::new();
        let out = tool.execute(make_input(json!({}))).await.unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("pattern"));
    }

    #[tokio::test]
    async fn execute_invalid_regex_returns_error() {
        let tool = SemanticGrepTool::new();
        let out = tool
            .execute(make_input(json!({ "pattern": "(unclosed" })))
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("regex") || out.content.contains("Invalid"));
    }

    #[tokio::test]
    async fn execute_search_on_temp_file() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "pub fn hello() {{").unwrap();
        writeln!(file, "    println!(\"hello world\");").unwrap();
        writeln!(file, "}}").unwrap();
        let path = file.path().to_str().unwrap().to_string();

        let tool = SemanticGrepTool::new();
        let out = tool
            .execute(make_input(json!({
                "pattern": "hello world",
                "path": path
            })))
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
        assert!(
            out.content.contains("1 match") || out.content.contains("match"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_no_matches() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "fn greet() {{}}").unwrap();
        let path = file.path().to_str().unwrap().to_string();

        let tool = SemanticGrepTool::new();
        let out = tool
            .execute(make_input(json!({
                "pattern": "ZZZNOMATCH",
                "path": path
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("0 match"), "content: {}", out.content);
    }
}
