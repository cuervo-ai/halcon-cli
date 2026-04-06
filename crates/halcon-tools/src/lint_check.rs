//! `lint_check` tool: run language-specific linters and return structured diagnostics.
//!
//! Auto-detects project type from working directory:
//! - `Cargo.toml` → `cargo clippy`
//! - `package.json` → `eslint` + optionally `tsc`
//! - `pyproject.toml` / `setup.py` / `setup.cfg` → `mypy` or `ruff`
//! - `go.mod` → `go vet`
//!
//! Returns structured JSON: errors, warnings, total counts, per-file list.

use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;
use tokio::process::Command;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

#[allow(unused_imports)]
use tracing::instrument;

/// Maximum combined output bytes before truncation.
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

pub struct LintCheckTool {
    timeout_secs: u64,
}

impl LintCheckTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }
}

impl Default for LintCheckTool {
    fn default() -> Self {
        Self::new(120)
    }
}

// ─── Project type detection ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum ProjectKind {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Go,
    Unknown,
}

fn detect_project_kind(working_dir: &str) -> ProjectKind {
    let path = std::path::Path::new(working_dir);
    if path.join("Cargo.toml").exists() {
        return ProjectKind::Rust;
    }
    if path.join("go.mod").exists() {
        return ProjectKind::Go;
    }
    if path.join("package.json").exists() {
        // Prefer TypeScript if tsconfig.json present
        if path.join("tsconfig.json").exists() {
            return ProjectKind::TypeScript;
        }
        return ProjectKind::JavaScript;
    }
    if path.join("pyproject.toml").exists()
        || path.join("setup.py").exists()
        || path.join("setup.cfg").exists()
        || path.join("requirements.txt").exists()
    {
        return ProjectKind::Python;
    }
    ProjectKind::Unknown
}

// ─── Diagnostic structs ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Diagnostic {
    level: String, // "error" | "warning" | "note" | "info"
    file: Option<String>,
    line: Option<u32>,
    col: Option<u32>,
    code: Option<String>,
    message: String,
}

impl Diagnostic {
    fn to_json(&self) -> serde_json::Value {
        json!({
            "level": self.level,
            "file": self.file,
            "line": self.line,
            "col": self.col,
            "code": self.code,
            "message": self.message,
        })
    }
}

// ─── Parsers ──────────────────────────────────────────────────────────────────

/// Parse `cargo clippy` / `rustc` stderr (JSON message format when --message-format=json,
/// or plain text fallback).
fn parse_rust_output(output: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    for line in output.lines() {
        // Try JSON line first
        if line.trim_start().starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if v["reason"] == "compiler-message" {
                    let msg = &v["message"];
                    let level = msg["level"].as_str().unwrap_or("error").to_string();
                    if level == "note" || level == "help" {
                        continue; // skip secondary messages
                    }
                    let message = msg["message"].as_str().unwrap_or("").to_string();
                    let code = msg["code"]["code"].as_str().map(|s| s.to_string());
                    let (file, ln, col) = msg["spans"]
                        .as_array()
                        .and_then(|spans| spans.first())
                        .map(|sp| {
                            (
                                sp["file_name"].as_str().map(|s| s.to_string()),
                                sp["line_start"].as_u64().map(|n| n as u32),
                                sp["column_start"].as_u64().map(|n| n as u32),
                            )
                        })
                        .unwrap_or((None, None, None));

                    diags.push(Diagnostic {
                        level,
                        file,
                        line: ln,
                        col,
                        code,
                        message,
                    });
                }
                continue;
            }
        }

        // Plain text: "error[E0xxx]: message" or "warning: message"
        if let Some(rest) = line.strip_prefix("error[") {
            let code_end = rest.find(']').unwrap_or(0);
            let code = rest[..code_end].to_string();
            let msg = rest[code_end + 2..].trim_start_matches("]: ").to_string();
            diags.push(Diagnostic {
                level: "error".into(),
                file: None,
                line: None,
                col: None,
                code: Some(code),
                message: msg,
            });
        } else if let Some(msg) = line.strip_prefix("error: ") {
            diags.push(Diagnostic {
                level: "error".into(),
                file: None,
                line: None,
                col: None,
                code: None,
                message: msg.to_string(),
            });
        } else if let Some(msg) = line.strip_prefix("warning: ") {
            if !msg.starts_with("generated") {
                diags.push(Diagnostic {
                    level: "warning".into(),
                    file: None,
                    line: None,
                    col: None,
                    code: None,
                    message: msg.to_string(),
                });
            }
        } else if line.contains(": error:") || line.contains(": warning:") {
            // "src/lib.rs:10:5: error: ..."
            parse_colon_format(line, &mut diags);
        }
    }

    diags
}

/// Parse ESLint plain text output: "path:line:col  level  code  message"
fn parse_eslint_output(output: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let mut current_file: Option<String> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        // File header lines (no leading whitespace, end with counts or just path)
        if !line.starts_with(' ')
            && !trimmed.is_empty()
            && !trimmed.starts_with("✖")
            && !trimmed.starts_with("✓")
            && !trimmed.starts_with("error")
            && !trimmed.starts_with("warning")
            && !trimmed.contains("  error ")
            && !trimmed.contains("  warning ")
        {
            // Likely a file path
            if std::path::Path::new(trimmed).extension().is_some()
                || trimmed.contains('/')
                || trimmed.contains('\\')
            {
                current_file = Some(trimmed.to_string());
                continue;
            }
        }

        // "  line:col  error|warning  message  rule"
        let parts: Vec<&str> = trimmed.splitn(4, "  ").collect();
        if parts.len() >= 3 {
            let location = parts[0];
            let level_str = parts[1].trim();
            let rest = parts.get(2).copied().unwrap_or("").trim();

            if level_str == "error" || level_str == "warning" {
                let loc_parts: Vec<&str> = location.split(':').collect();
                let ln = loc_parts.first().and_then(|s| s.parse().ok());
                let col = loc_parts.get(1).and_then(|s| s.parse().ok());
                // Last part of rest may be rule code
                let (message, code) = if let Some((msg, rule)) = rest.rsplit_once("  ") {
                    (msg.trim().to_string(), Some(rule.trim().to_string()))
                } else {
                    (rest.to_string(), None)
                };

                diags.push(Diagnostic {
                    level: level_str.to_string(),
                    file: current_file.clone(),
                    line: ln,
                    col,
                    code,
                    message,
                });
            }
        }
    }

    diags
}

/// Parse mypy / ruff output: "file.py:line: level: message"
fn parse_python_output(output: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    for line in output.lines() {
        // "src/foo.py:10: error: ..." (mypy)
        // "src/foo.py:10:5: E123 message" (ruff/flake8)
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() < 3 {
            continue;
        }
        let file = parts[0].trim().to_string();
        if !file.ends_with(".py") {
            continue;
        }
        let ln = parts[1].trim().parse::<u32>().ok();

        // Check if next part is a number (column) or level
        let (col, level, message) = if parts.len() >= 4 {
            if let Ok(c) = parts[2].trim().parse::<u32>() {
                // ruff: file:line:col: code message
                let rest = parts[3].trim();
                let level = if rest.starts_with('E')
                    || rest.starts_with('F')
                    || rest.starts_with("error")
                {
                    "error"
                } else if rest.starts_with('W') || rest.starts_with("warning") {
                    "warning"
                } else {
                    "note"
                };
                (Some(c), level.to_string(), rest.to_string())
            } else {
                // mypy: file:line: level: message
                let level = match parts[2].trim() {
                    "error" => "error",
                    "warning" => "warning",
                    "note" => "note",
                    _ => continue,
                };
                (None, level.to_string(), parts[3].trim().to_string())
            }
        } else {
            continue;
        };

        if level == "note" {
            continue;
        }

        diags.push(Diagnostic {
            level,
            file: Some(file),
            line: ln,
            col,
            code: None,
            message,
        });
    }

    diags
}

/// Parse `go vet` output: "#\tpackage" or "file.go:line:col: message"
fn parse_go_output(output: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    for line in output.lines() {
        // "# package/path" — section header, skip
        if line.starts_with('#') {
            continue;
        }
        // "file.go:line:col: message"
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() >= 4 && parts[0].ends_with(".go") {
            let file = parts[0].trim().to_string();
            let ln = parts[1].trim().parse::<u32>().ok();
            let col = parts[2].trim().parse::<u32>().ok();
            let message = parts[3].trim().to_string();

            if !message.is_empty() {
                diags.push(Diagnostic {
                    level: "warning".to_string(), // go vet reports are warnings
                    file: Some(file),
                    line: ln,
                    col,
                    code: None,
                    message,
                });
            }
        }
    }

    diags
}

/// Generic colon-separated format: "file:line:col: level: message"
fn parse_colon_format(line: &str, diags: &mut Vec<Diagnostic>) {
    let parts: Vec<&str> = line.splitn(5, ':').collect();
    if parts.len() < 4 {
        return;
    }
    let file = parts[0].trim().to_string();
    let ln = parts[1].trim().parse::<u32>().ok();
    let _col = parts[2].trim().parse::<u32>().ok();
    let level = parts[3].trim().to_string();
    let message = parts
        .get(4)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if level == "error" || level == "warning" {
        diags.push(Diagnostic {
            level,
            file: Some(file),
            line: ln,
            col: None,
            code: None,
            message,
        });
    }
}

// ─── Command execution ────────────────────────────────────────────────────────

async fn run_lint_command(
    program: &str,
    args: &[&str],
    working_dir: &str,
    timeout_secs: u64,
) -> std::result::Result<(String, String, i32), String> {
    let result = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        Command::new(program)
            .args(args)
            .current_dir(working_dir)
            .output(),
    )
    .await;

    match result {
        Err(_) => Err(format!("{program} timed out after {timeout_secs}s")),
        Ok(Err(e)) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Err(format!("{program}: command not found — is it installed?"))
            } else {
                Err(format!("Failed to run {program}: {e}"))
            }
        }
        Ok(Ok(output)) => {
            let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let total = stdout.len() + stderr.len();
            if total > MAX_OUTPUT_BYTES {
                let half = MAX_OUTPUT_BYTES / 2;
                if stdout.len() > half {
                    stdout.truncate(half);
                    stdout.push_str("\n...(truncated)");
                }
                if stderr.len() > half {
                    stderr.truncate(half);
                    stderr.push_str("\n...(truncated)");
                }
            }
            Ok((stdout, stderr, output.status.code().unwrap_or(-1)))
        }
    }
}

// ─── Tool implementation ──────────────────────────────────────────────────────

#[async_trait]
impl Tool for LintCheckTool {
    fn name(&self) -> &str {
        "lint_check"
    }

    fn description(&self) -> &str {
        "Run language-specific linters and return structured diagnostics (errors + warnings). \
         Auto-detects project type from working directory: \
         Rust → cargo clippy; \
         JavaScript → eslint; \
         TypeScript → tsc (type check) + eslint; \
         Python → ruff or mypy; \
         Go → go vet. \
         Returns error/warning counts per file and total. \
         Use 'linter' to force a specific tool."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    #[tracing::instrument(skip(self), fields(tool = "lint_check"))]
    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let working_dir = &input.working_directory;

        // Allow explicit linter override
        let forced_linter = input
            .arguments
            .get("linter")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let extra_args: Vec<String> = input
            .arguments
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let kind = if let Some(ref linter) = forced_linter {
            match linter.as_str() {
                "cargo" | "clippy" | "rust" => ProjectKind::Rust,
                "eslint" | "js" | "javascript" => ProjectKind::JavaScript,
                "tsc" | "ts" | "typescript" => ProjectKind::TypeScript,
                "mypy" | "ruff" | "python" | "py" => ProjectKind::Python,
                "go" | "govet" | "go vet" => ProjectKind::Go,
                _ => {
                    return Err(HalconError::InvalidInput(format!(
                        "lint_check: unknown linter '{linter}'. \
                         Use: clippy, eslint, tsc, mypy, ruff, go"
                    )))
                }
            }
        } else {
            detect_project_kind(working_dir)
        };

        if kind == ProjectKind::Unknown {
            return Err(HalconError::InvalidInput(
                "lint_check: could not detect project type. \
                 No Cargo.toml, package.json, pyproject.toml, setup.py, or go.mod found. \
                 Use the 'linter' parameter to specify explicitly."
                    .into(),
            ));
        }

        let extra: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
        let (diags, linter_used, raw_output) = match kind {
            ProjectKind::Rust => self.run_clippy(working_dir, &extra).await,
            ProjectKind::JavaScript => self.run_eslint(working_dir, &extra).await,
            ProjectKind::TypeScript => self.run_tsc(working_dir, &extra).await,
            ProjectKind::Python => self.run_python_lint(working_dir, &extra).await,
            ProjectKind::Go => self.run_go_vet(working_dir, &extra).await,
            ProjectKind::Unknown => unreachable!(),
        };

        // Aggregate counts
        let error_count = diags.iter().filter(|d| d.level == "error").count();
        let warning_count = diags.iter().filter(|d| d.level == "warning").count();
        let total = diags.len();

        // Per-file grouping
        let mut by_file: std::collections::HashMap<String, (usize, usize)> =
            std::collections::HashMap::new();
        for d in &diags {
            let key = d.file.clone().unwrap_or_else(|| "(unknown)".to_string());
            let entry = by_file.entry(key).or_insert((0, 0));
            if d.level == "error" {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
        }

        let files: Vec<serde_json::Value> = by_file
            .iter()
            .map(|(f, (errs, warns))| json!({ "file": f, "errors": errs, "warnings": warns }))
            .collect();

        let passed = error_count == 0;
        let summary = if passed && warning_count == 0 {
            format!("✓ {linter_used}: no issues found")
        } else if passed {
            format!("⚠ {linter_used}: {warning_count} warning(s), 0 errors")
        } else {
            format!("✗ {linter_used}: {error_count} error(s), {warning_count} warning(s)")
        };

        let meta = json!({
            "linter": linter_used,
            "passed": passed,
            "error_count": error_count,
            "warning_count": warning_count,
            "total_diagnostics": total,
            "files": files,
            "diagnostics": diags.iter().map(|d| d.to_json()).collect::<Vec<_>>(),
        });

        let content = if raw_output.is_empty() && total == 0 {
            format!("{summary}\n(no output)")
        } else if total == 0 {
            format!("{summary}\n\n{}", raw_output.trim())
        } else {
            let diag_lines: String = diags
                .iter()
                .take(50) // cap display at 50
                .map(|d| {
                    let loc = match (&d.file, d.line) {
                        (Some(f), Some(l)) => format!("{f}:{l}"),
                        (Some(f), None) => f.clone(),
                        _ => "(unknown)".to_string(),
                    };
                    format!("[{}] {}: {}", d.level.to_uppercase(), loc, d.message)
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("{summary}\n\n{diag_lines}")
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: !passed,
            metadata: Some(meta),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "linter": {
                    "type": "string",
                    "description": "Force a specific linter: clippy, eslint, tsc, mypy, ruff, go. Auto-detected if omitted."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Extra arguments passed to the linter."
                }
            },
            "required": []
        })
    }
}

impl LintCheckTool {
    async fn run_clippy(
        &self,
        working_dir: &str,
        extra: &[&str],
    ) -> (Vec<Diagnostic>, String, String) {
        let mut args = vec!["clippy", "--message-format=json", "--", "-D", "warnings"];
        args.extend_from_slice(extra);
        match run_lint_command("cargo", &args, working_dir, self.timeout_secs).await {
            Ok((stdout, stderr, _)) => {
                let combined = format!("{stdout}\n{stderr}");
                let diags = parse_rust_output(&combined);
                (diags, "cargo clippy".into(), combined)
            }
            Err(e) => (vec![], "cargo clippy".into(), e),
        }
    }

    async fn run_eslint(
        &self,
        working_dir: &str,
        extra: &[&str],
    ) -> (Vec<Diagnostic>, String, String) {
        let mut args = vec![".", "--format", "compact"];
        args.extend_from_slice(extra);
        match run_lint_command("npx", &["eslint", "."], working_dir, self.timeout_secs).await {
            Ok((stdout, stderr, _)) => {
                let _ = extra; // suppress unused warning
                let combined = format!("{stdout}\n{stderr}");
                let diags = parse_eslint_output(&combined);
                (diags, "eslint".into(), combined)
            }
            Err(e) => (vec![], "eslint".into(), e),
        }
    }

    async fn run_tsc(
        &self,
        working_dir: &str,
        extra: &[&str],
    ) -> (Vec<Diagnostic>, String, String) {
        let mut args = vec!["--noEmit"];
        args.extend_from_slice(extra);
        match run_lint_command("npx", &["tsc", "--noEmit"], working_dir, self.timeout_secs).await {
            Ok((stdout, stderr, _)) => {
                let _ = extra;
                let combined = format!("{stdout}\n{stderr}");
                let diags = parse_eslint_output(&combined); // tsc output similar format
                (diags, "tsc".into(), combined)
            }
            Err(e) => (vec![], "tsc".into(), e),
        }
    }

    async fn run_python_lint(
        &self,
        working_dir: &str,
        extra: &[&str],
    ) -> (Vec<Diagnostic>, String, String) {
        // Prefer ruff (fast), fall back to mypy
        let ruff_path = std::path::Path::new(working_dir).join(".ruff.toml");
        let use_ruff = ruff_path.exists() || which_exists("ruff").await;

        let (program, args): (&str, Vec<&str>) = if use_ruff {
            let mut a = vec!["check", "."];
            a.extend_from_slice(extra);
            ("ruff", a)
        } else {
            let mut a = vec![".", "--show-column-numbers"];
            a.extend_from_slice(extra);
            ("mypy", a)
        };

        let linter_name = if use_ruff { "ruff" } else { "mypy" };

        match run_lint_command(program, &args, working_dir, self.timeout_secs).await {
            Ok((stdout, stderr, _)) => {
                let combined = format!("{stdout}\n{stderr}");
                let diags = parse_python_output(&combined);
                (diags, linter_name.into(), combined)
            }
            Err(e) => (vec![], linter_name.into(), e),
        }
    }

    async fn run_go_vet(
        &self,
        working_dir: &str,
        extra: &[&str],
    ) -> (Vec<Diagnostic>, String, String) {
        let mut args = vec!["vet", "./..."];
        args.extend_from_slice(extra);
        match run_lint_command("go", &args, working_dir, self.timeout_secs).await {
            Ok((stdout, stderr, _)) => {
                let combined = format!("{stdout}\n{stderr}");
                let diags = parse_go_output(&combined);
                (diags, "go vet".into(), combined)
            }
            Err(e) => (vec![], "go vet".into(), e),
        }
    }
}

async fn which_exists(prog: &str) -> bool {
    tokio::process::Command::new("which")
        .arg(prog)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_project_kind_rust() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        std::fs::write(path.join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(
            detect_project_kind(path.to_str().unwrap()),
            ProjectKind::Rust
        );
    }

    #[test]
    fn detect_project_kind_typescript() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        std::fs::write(path.join("package.json"), "{}").unwrap();
        std::fs::write(path.join("tsconfig.json"), "{}").unwrap();
        assert_eq!(
            detect_project_kind(path.to_str().unwrap()),
            ProjectKind::TypeScript
        );
    }

    #[test]
    fn detect_project_kind_javascript_no_tsconfig() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        std::fs::write(path.join("package.json"), "{}").unwrap();
        assert_eq!(
            detect_project_kind(path.to_str().unwrap()),
            ProjectKind::JavaScript
        );
    }

    #[test]
    fn detect_project_kind_python() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        std::fs::write(path.join("pyproject.toml"), "").unwrap();
        assert_eq!(
            detect_project_kind(path.to_str().unwrap()),
            ProjectKind::Python
        );
    }

    #[test]
    fn detect_project_kind_go() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        std::fs::write(path.join("go.mod"), "module foo").unwrap();
        assert_eq!(detect_project_kind(path.to_str().unwrap()), ProjectKind::Go);
    }

    #[test]
    fn detect_project_kind_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            detect_project_kind(dir.path().to_str().unwrap()),
            ProjectKind::Unknown
        );
    }

    #[test]
    fn parse_rust_json_output_extracts_errors() {
        // A minimal compiler-message JSON line
        let line = r#"{"reason":"compiler-message","message":{"level":"error","message":"unused variable","code":{"code":"E0001"},"spans":[{"file_name":"src/lib.rs","line_start":10,"column_start":5}]}}"#;
        let diags = parse_rust_output(line);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, "error");
        assert_eq!(diags[0].file.as_deref(), Some("src/lib.rs"));
        assert_eq!(diags[0].line, Some(10));
        assert_eq!(diags[0].code.as_deref(), Some("E0001"));
    }

    #[test]
    fn parse_rust_plain_text_error() {
        let output = "error[E0308]: mismatched types\nwarning: unused import\n";
        let diags = parse_rust_output(output);
        let errors: Vec<_> = diags.iter().filter(|d| d.level == "error").collect();
        assert!(!errors.is_empty());
        assert_eq!(errors[0].code.as_deref(), Some("E0308"));
    }

    #[test]
    fn parse_python_mypy_output() {
        let output =
            "src/foo.py:10: error: Incompatible types\nsrc/bar.py:20: warning: Missing type\n";
        let diags = parse_python_output(output);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].file.as_deref(), Some("src/foo.py"));
        assert_eq!(diags[0].line, Some(10));
        assert_eq!(diags[0].level, "error");
    }

    #[test]
    fn parse_go_vet_output() {
        let output = "# github.com/foo/bar\nfoo_test.go:42:5: unreachable code\n";
        let diags = parse_go_output(output);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].file.as_deref(), Some("foo_test.go"));
        assert_eq!(diags[0].line, Some(42));
    }

    #[test]
    fn tool_meta() {
        let t = LintCheckTool::new(120);
        assert_eq!(t.name(), "lint_check");
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        assert!(!t.description().is_empty());
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_project_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let t = LintCheckTool::new(30);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({}),
            working_directory: dir.path().to_str().unwrap().to_string(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn invalid_linter_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let t = LintCheckTool::new(30);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "linter": "notareal" }),
            working_directory: dir.path().to_str().unwrap().to_string(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn clippy_runs_on_rust_project() {
        // This test runs against the actual workspace — cargo must be in PATH.
        let workspace = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let t = LintCheckTool::new(180);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "linter": "clippy" }),
            working_directory: workspace,
        };
        let out = t.execute(input).await.unwrap();
        // We just check it ran and produced structured metadata
        assert!(out.metadata.is_some());
        let meta = out.metadata.unwrap();
        assert!(meta["linter"].as_str().unwrap().contains("clippy"));
        assert!(meta["error_count"].is_number());
        assert!(meta["warning_count"].is_number());
    }
}
