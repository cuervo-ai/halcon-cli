//! CodeMetricsTool — static code metrics for source files and directories.
//!
//! Computes:
//! - Lines of Code (LOC), blank lines, comment lines, code lines
//! - File count and size distribution
//! - Function/struct/class count per language
//! - Cyclomatic complexity approximation (decision points)
//! - Per-extension aggregation
//!
//! Supports: Rust, Python, JS/TS, Go, Java, C/C++, Shell, and more.

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub struct CodeMetricsTool;

impl CodeMetricsTool {
    pub fn new() -> Self {
        Self
    }

    fn detect_language(path: &Path) -> &'static str {
        match path.extension().and_then(|e| e.to_str()) {
            Some("rs") => "rust",
            Some("py") => "python",
            Some("js") => "javascript",
            Some("ts") | Some("tsx") => "typescript",
            Some("go") => "go",
            Some("java") => "java",
            Some("c") | Some("h") => "c",
            Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => "cpp",
            Some("sh") | Some("bash") | Some("zsh") => "shell",
            Some("rb") => "ruby",
            Some("php") => "php",
            Some("cs") => "csharp",
            Some("swift") => "swift",
            Some("kt") => "kotlin",
            _ => "other",
        }
    }

    fn is_code_file(path: &Path) -> bool {
        let code_exts = [
            "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "c", "h", "cpp", "cc", "cxx",
            "hpp", "sh", "bash", "rb", "php", "cs", "swift", "kt", "lua", "scala", "clj", "ex",
            "exs", "zig", "nim",
        ];
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| code_exts.contains(&e))
            .unwrap_or(false)
    }

    fn skip_dir(name: &str) -> bool {
        matches!(
            name,
            "target"
                | "node_modules"
                | ".git"
                | "dist"
                | "build"
                | "__pycache__"
                | ".cache"
                | "vendor"
                | ".gradle"
                | "out"
                | "bin"
                | "obj"
        )
    }

    fn analyze_file_content(content: &str, lang: &'static str) -> FileMetrics {
        let mut total = 0u32;
        let mut blank = 0u32;
        let mut comment = 0u32;
        let mut functions = 0u32;
        let mut complexity = 0u32; // decision points
        let mut in_block_comment = false;

        for line in content.lines() {
            total += 1;
            let trimmed = line.trim();

            if trimmed.is_empty() {
                blank += 1;
                continue;
            }

            // Block comment handling (C-style)
            if in_block_comment {
                comment += 1;
                if trimmed.contains("*/") {
                    in_block_comment = false;
                }
                continue;
            }

            // Single-line detection
            let is_comment = match lang {
                "rust" | "go" | "java" | "javascript" | "typescript" | "c" | "cpp" | "csharp"
                | "swift" | "kotlin" => {
                    if trimmed.starts_with("//") || trimmed.starts_with("///") {
                        true
                    } else if trimmed.starts_with("/*") {
                        in_block_comment =
                            !trimmed.contains("*/") || trimmed.ends_with("*/") && trimmed.len() > 2;
                        // Count as comment if it starts with /*
                        true
                    } else {
                        false
                    }
                }
                "python" => {
                    trimmed.starts_with('#')
                        || trimmed.starts_with("\"\"\"")
                        || trimmed.starts_with("'''")
                }
                "ruby" => trimmed.starts_with('#'),
                "shell" => trimmed.starts_with('#'),
                _ => false,
            };

            if is_comment {
                comment += 1;
                continue;
            }

            // Function detection (approximate)
            let is_fn = match lang {
                "rust" => {
                    trimmed.starts_with("fn ")
                        || trimmed.starts_with("pub fn ")
                        || trimmed.starts_with("async fn ")
                        || trimmed.starts_with("pub async fn ")
                }
                "python" => trimmed.starts_with("def ") || trimmed.starts_with("async def "),
                "javascript" | "typescript" => {
                    trimmed.starts_with("function ")
                        || trimmed.contains("=> {")
                        || trimmed.starts_with("async function ")
                }
                "go" => trimmed.starts_with("func "),
                "java" | "csharp" | "kotlin" => {
                    (trimmed.contains("void ")
                        || trimmed.contains("int ")
                        || trimmed.contains("String ")
                        || trimmed.contains("bool "))
                        && trimmed.contains('(')
                        && trimmed.contains(')')
                        && !trimmed.ends_with(';')
                }
                "c" | "cpp" => {
                    trimmed.contains('(')
                        && trimmed.contains(')')
                        && !trimmed.ends_with(';')
                        && !trimmed.starts_with("if")
                        && !trimmed.starts_with("while")
                        && !trimmed.starts_with("for")
                }
                _ => false,
            };
            if is_fn {
                functions += 1;
            }

            // Complexity: count decision points
            let keywords = [
                "if ", "else if", "elif ", "while ", "for ", "match ", "case ", "&&", "||", "? ",
            ];
            for kw in &keywords {
                if trimmed.contains(kw) {
                    complexity += trimmed.matches(kw).count() as u32;
                }
            }
        }

        let code_lines = total.saturating_sub(blank).saturating_sub(comment);
        FileMetrics {
            total_lines: total,
            blank_lines: blank,
            comment_lines: comment,
            code_lines,
            functions,
            cyclomatic_complexity: complexity + 1, // +1 for base path
        }
    }

    fn scan_dir(root: &Path, max_files: usize) -> Vec<(std::path::PathBuf, &'static str)> {
        let mut stack = vec![root.to_path_buf()];
        let mut files = vec![];

        while let Some(dir) = stack.pop() {
            if files.len() >= max_files {
                break;
            }
            let rd = match std::fs::read_dir(&dir) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if !Self::skip_dir(name) {
                            stack.push(path);
                        }
                    }
                } else if Self::is_code_file(&path) && files.len() < max_files {
                    let lang = Self::detect_language(&path);
                    files.push((path, lang));
                }
            }
        }
        files
    }

    fn format_report(
        agg: &AggMetrics,
        per_ext: &HashMap<&'static str, FileMetrics>,
        total_files: usize,
    ) -> String {
        let mut out = String::new();
        out.push_str("# Code Metrics Report\n\n");
        out.push_str(&format!(
            "**Files analyzed**: {}\n\
             **Total lines**: {}\n\
             **Code lines**: {} ({:.0}%)\n\
             **Comment lines**: {} ({:.0}%)\n\
             **Blank lines**: {} ({:.0}%)\n\
             **Functions/methods**: {}\n\
             **Avg cyclomatic complexity**: {:.1}\n\n",
            total_files,
            agg.total_lines,
            agg.code_lines,
            if agg.total_lines > 0 {
                agg.code_lines as f64 / agg.total_lines as f64 * 100.0
            } else {
                0.0
            },
            agg.comment_lines,
            if agg.total_lines > 0 {
                agg.comment_lines as f64 / agg.total_lines as f64 * 100.0
            } else {
                0.0
            },
            agg.blank_lines,
            if agg.total_lines > 0 {
                agg.blank_lines as f64 / agg.total_lines as f64 * 100.0
            } else {
                0.0
            },
            agg.functions,
            if total_files > 0 {
                agg.total_complexity as f64 / total_files as f64
            } else {
                0.0
            },
        ));

        if !per_ext.is_empty() {
            out.push_str("## By Language\n\n");
            out.push_str(&format!(
                "{:<15} {:>6} {:>6} {:>7} {:>5}\n",
                "Language", "LOC", "Fns", "Comment%", "CC"
            ));
            out.push_str(&format!(
                "{:-<15} {:->6} {:->6} {:->7} {:->5}\n",
                "", "", "", "", ""
            ));
            let mut langs: Vec<(&str, &FileMetrics)> =
                per_ext.iter().map(|(k, v)| (*k, v)).collect();
            langs.sort_by(|a, b| b.1.code_lines.cmp(&a.1.code_lines));
            for (lang, m) in &langs {
                let comment_pct = if m.total_lines > 0 {
                    m.comment_lines as f64 / m.total_lines as f64 * 100.0
                } else {
                    0.0
                };
                out.push_str(&format!(
                    "{:<15} {:>6} {:>6} {:>6.0}%  {:>4}\n",
                    lang, m.code_lines, m.functions, comment_pct, m.cyclomatic_complexity
                ));
            }
        }
        out
    }
}

#[derive(Default, Clone)]
struct FileMetrics {
    total_lines: u32,
    blank_lines: u32,
    comment_lines: u32,
    code_lines: u32,
    functions: u32,
    cyclomatic_complexity: u32,
}

#[derive(Default)]
struct AggMetrics {
    total_lines: u32,
    blank_lines: u32,
    comment_lines: u32,
    code_lines: u32,
    functions: u32,
    total_complexity: u32,
}

impl Default for CodeMetricsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for CodeMetricsTool {
    fn name(&self) -> &str {
        "code_metrics"
    }

    fn description(&self) -> &str {
        "Compute static code metrics for files or directories: lines of code, \
         blank and comment lines, function count, cyclomatic complexity estimates, \
         and per-language breakdown. Supports Rust, Python, JS/TS, Go, Java, C/C++, and more."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File or directory to analyze (default: current directory)."
                },
                "max_files": {
                    "type": "integer",
                    "description": "Maximum files to scan (default: 500)."
                },
                "format": {
                    "type": "string",
                    "enum": ["text", "json"],
                    "description": "Output format (default: text)."
                },
                "language": {
                    "type": "string",
                    "description": "Filter to a specific language (e.g. rust, python, javascript)."
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
        let target = args["path"].as_str().unwrap_or(&input.working_directory);
        let max_files = args["max_files"].as_u64().unwrap_or(500).clamp(1, 2000) as usize;
        let format = args["format"].as_str().unwrap_or("text");
        let lang_filter = args["language"].as_str().map(|s| s.to_lowercase());

        let target_path = std::path::Path::new(target);

        let candidates: Vec<(std::path::PathBuf, &'static str)> = if target_path.is_file() {
            let lang = Self::detect_language(target_path);
            vec![(target_path.to_path_buf(), lang)]
        } else {
            Self::scan_dir(target_path, max_files)
        };

        // Filter by language if requested
        let candidates: Vec<_> = if let Some(ref lf) = lang_filter {
            candidates
                .into_iter()
                .filter(|(_, l)| *l == lf.as_str())
                .collect()
        } else {
            candidates
        };

        if candidates.is_empty() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("No code files found in '{target}'."),
                is_error: false,
                metadata: None,
            });
        }

        let mut agg = AggMetrics::default();
        let mut per_ext: HashMap<&'static str, FileMetrics> = HashMap::new();
        let total_files = candidates.len();

        for (path, lang) in &candidates {
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let m = Self::analyze_file_content(&content, lang);
            agg.total_lines += m.total_lines;
            agg.blank_lines += m.blank_lines;
            agg.comment_lines += m.comment_lines;
            agg.code_lines += m.code_lines;
            agg.functions += m.functions;
            agg.total_complexity += m.cyclomatic_complexity;

            let entry = per_ext.entry(lang).or_default();
            entry.total_lines += m.total_lines;
            entry.blank_lines += m.blank_lines;
            entry.comment_lines += m.comment_lines;
            entry.code_lines += m.code_lines;
            entry.functions += m.functions;
            entry.cyclomatic_complexity += m.cyclomatic_complexity;
        }

        let content = if format == "json" {
            let langs: Vec<Value> = per_ext
                .iter()
                .map(|(lang, m)| {
                    json!({
                        "language": lang,
                        "total_lines": m.total_lines,
                        "code_lines": m.code_lines,
                        "comment_lines": m.comment_lines,
                        "blank_lines": m.blank_lines,
                        "functions": m.functions,
                        "cyclomatic_complexity": m.cyclomatic_complexity
                    })
                })
                .collect();
            serde_json::to_string_pretty(&json!({
                "files": total_files,
                "total_lines": agg.total_lines,
                "code_lines": agg.code_lines,
                "comment_lines": agg.comment_lines,
                "blank_lines": agg.blank_lines,
                "functions": agg.functions,
                "avg_complexity": if total_files > 0 { agg.total_complexity as f64 / total_files as f64 } else { 0.0 },
                "by_language": langs
            }))
            .unwrap_or_default()
        } else {
            Self::format_report(&agg, &per_ext, total_files)
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "files_analyzed": total_files,
                "total_lines": agg.total_lines,
                "code_lines": agg.code_lines,
                "functions": agg.functions
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let t = CodeMetricsTool::new();
        assert_eq!(t.name(), "code_metrics");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"].as_array().unwrap().is_empty());
    }

    #[test]
    fn detect_language_rust() {
        assert_eq!(
            CodeMetricsTool::detect_language(Path::new("main.rs")),
            "rust"
        );
        assert_eq!(
            CodeMetricsTool::detect_language(Path::new("script.py")),
            "python"
        );
        assert_eq!(
            CodeMetricsTool::detect_language(Path::new("app.ts")),
            "typescript"
        );
    }

    #[test]
    fn analyze_rust_file() {
        let code = r#"
// This is a comment
fn hello() {
    if true {
        println!("hi");
    }
}

fn world() {}
"#;
        let m = CodeMetricsTool::analyze_file_content(code, "rust");
        assert!(
            m.functions >= 2,
            "expected >=2 functions, got {}",
            m.functions
        );
        assert!(m.comment_lines >= 1);
        assert!(m.blank_lines >= 1);
        assert!(m.code_lines >= 3);
    }

    #[test]
    fn analyze_python_file() {
        let code = "# comment\ndef foo():\n    pass\n\n\ndef bar():\n    if x:\n        pass\n";
        let m = CodeMetricsTool::analyze_file_content(code, "python");
        assert!(m.functions >= 2);
        assert!(m.comment_lines >= 1);
        assert!(m.blank_lines >= 1);
    }

    #[test]
    fn analyze_empty_file() {
        let m = CodeMetricsTool::analyze_file_content("", "rust");
        assert_eq!(m.total_lines, 0);
        assert_eq!(m.code_lines, 0);
        assert_eq!(m.cyclomatic_complexity, 1); // base path
    }

    #[test]
    fn analyze_blank_only() {
        let m = CodeMetricsTool::analyze_file_content("\n\n\n", "rust");
        assert_eq!(m.blank_lines, 3);
        assert_eq!(m.code_lines, 0);
    }

    #[test]
    fn skip_dir_patterns() {
        assert!(CodeMetricsTool::skip_dir("target"));
        assert!(CodeMetricsTool::skip_dir("node_modules"));
        assert!(!CodeMetricsTool::skip_dir("src"));
    }

    #[test]
    fn is_code_file() {
        assert!(CodeMetricsTool::is_code_file(Path::new("main.rs")));
        assert!(CodeMetricsTool::is_code_file(Path::new("app.py")));
        assert!(!CodeMetricsTool::is_code_file(Path::new("README.md")));
        assert!(!CodeMetricsTool::is_code_file(Path::new("image.png")));
    }

    #[test]
    fn format_report_basic() {
        let agg = AggMetrics {
            total_lines: 100,
            blank_lines: 10,
            comment_lines: 20,
            code_lines: 70,
            functions: 5,
            total_complexity: 30,
        };
        let mut per_ext = HashMap::new();
        per_ext.insert(
            "rust",
            FileMetrics {
                total_lines: 100,
                blank_lines: 10,
                comment_lines: 20,
                code_lines: 70,
                functions: 5,
                cyclomatic_complexity: 30,
            },
        );
        let r = CodeMetricsTool::format_report(&agg, &per_ext, 3);
        assert!(r.contains("Code Metrics"));
        assert!(r.contains("70"));
        assert!(r.contains("rust"));
    }

    #[tokio::test]
    async fn execute_current_dir() {
        let tool = CodeMetricsTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: serde_json::json!({ "max_files": 10 }),
                working_directory:
                    "/Users/oscarvalois/Documents/Github/cuervo-cli/crates/halcon-tools/src".into(),
            })
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
        assert!(out.content.contains("Code Metrics") || out.content.contains("code"));
    }

    #[tokio::test]
    async fn execute_json_format() {
        let tool = CodeMetricsTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: serde_json::json!({ "format": "json", "max_files": 5 }),
                working_directory:
                    "/Users/oscarvalois/Documents/Github/cuervo-cli/crates/halcon-tools/src".into(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        let v: Value = serde_json::from_str(&out.content).expect("should be valid JSON");
        assert!(v["files"].as_u64().unwrap_or(0) > 0);
    }
}
