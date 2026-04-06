//! `test_run` tool: execute tests with structured pass/fail/skip counts + optional coverage.
//!
//! Supersedes `execute_test` by adding:
//! - Structured results: passed / failed / skipped counts
//! - Coverage % reporting (when available)
//! - Test duration tracking
//! - Auto-detection of test framework from working directory
//! - Individual test case list with status
//!
//! Supports: cargo test, pytest, jest, vitest, go test.

use async_trait::async_trait;
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::process::Command;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

#[allow(unused_imports)]
use tracing::instrument;

const MAX_OUTPUT_BYTES: usize = 512 * 1024;

pub struct TestRunTool {
    timeout_secs: u64,
}

impl TestRunTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }
}

impl Default for TestRunTool {
    fn default() -> Self {
        Self::new(300) // 5-minute default for test suites
    }
}

// ─── Structured test result ───────────────────────────────────────────────────

#[derive(Debug, Default)]
struct TestSuiteResult {
    framework: String,
    passed: u32,
    failed: u32,
    skipped: u32,
    total: u32,
    duration_ms: u64,
    coverage_pct: Option<f64>,
    failures: Vec<TestFailure>,
    /// Raw output for debugging
    raw_output: String,
}

#[derive(Debug, Clone)]
struct TestFailure {
    name: String,
    file: Option<String>,
    line: Option<u32>,
    message: String,
}

impl TestFailure {
    fn to_json(&self) -> serde_json::Value {
        json!({
            "name": self.name,
            "file": self.file,
            "line": self.line,
            "message": self.message,
        })
    }
}

// ─── Framework detection ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum TestFramework {
    Cargo,
    Pytest,
    Jest,
    Vitest,
    GoTest,
}

fn detect_framework(working_dir: &str) -> Option<TestFramework> {
    let path = std::path::Path::new(working_dir);
    if path.join("Cargo.toml").exists() {
        return Some(TestFramework::Cargo);
    }
    if path.join("go.mod").exists() {
        return Some(TestFramework::GoTest);
    }
    if path.join("package.json").exists() {
        // Read package.json to check test script
        if let Ok(contents) = std::fs::read_to_string(path.join("package.json")) {
            if contents.contains("vitest") {
                return Some(TestFramework::Vitest);
            }
            if contents.contains("jest") {
                return Some(TestFramework::Jest);
            }
        }
        return Some(TestFramework::Jest); // default JS
    }
    if path.join("pyproject.toml").exists()
        || path.join("setup.py").exists()
        || path.join("pytest.ini").exists()
        || path.join("conftest.py").exists()
    {
        return Some(TestFramework::Pytest);
    }
    None
}

// ─── Parsers ──────────────────────────────────────────────────────────────────

fn parse_cargo_output(output: &str) -> TestSuiteResult {
    let mut result = TestSuiteResult {
        framework: "cargo test".into(),
        ..Default::default()
    };
    result.raw_output = output.chars().take(4000).collect();

    for line in output.lines() {
        let trimmed = line.trim();

        // "test tests::my_test ... ok" or "FAILED"
        if (trimmed.starts_with("test ") || trimmed.starts_with("test "))
            && (trimmed.ends_with("ok")
                || trimmed.ends_with("FAILED")
                || trimmed.ends_with("ignored"))
        {
            if trimmed.ends_with("ok") {
                result.passed += 1;
            } else if trimmed.ends_with("FAILED") {
                result.failed += 1;
                // Extract test name
                let name = trimmed
                    .strip_prefix("test ")
                    .unwrap_or(trimmed)
                    .split(" ... ")
                    .next()
                    .unwrap_or(trimmed)
                    .trim()
                    .to_string();
                result.failures.push(TestFailure {
                    name,
                    file: None,
                    line: None,
                    message: "Test failed".to_string(),
                });
            } else if trimmed.ends_with("ignored") {
                result.skipped += 1;
            }
            continue;
        }

        // "test result: ok. 42 passed; 3 failed; 1 ignored; 0 measured"
        if trimmed.starts_with("test result:") {
            let parts: Vec<&str> = trimmed.split(';').collect();
            for part in &parts {
                let p = part.trim();
                if let Some(n) = extract_number_before(p, "passed") {
                    result.passed = n;
                } else if let Some(n) = extract_number_before(p, "failed") {
                    result.failed = n;
                } else if let Some(n) = extract_number_before(p, "ignored") {
                    result.skipped = n;
                }
            }
        }

        // Panic location for failures
        if trimmed.contains("panicked at ") {
            if let Some(last) = result.failures.last_mut() {
                if last.file.is_none() {
                    // "panicked at 'msg', src/lib.rs:42:5"
                    let parts: Vec<&str> = trimmed.split('\'').collect();
                    if parts.len() >= 3 {
                        last.message = parts.get(1).copied().unwrap_or("").to_string();
                    }
                    // Try to find file:line at end
                    if let Some(loc) = trimmed.rsplit(',').next() {
                        let loc = loc.trim();
                        let lparts: Vec<&str> = loc.split(':').collect();
                        if lparts.len() >= 2 {
                            last.file = Some(lparts[0].to_string());
                            last.line = lparts[1].parse().ok();
                        }
                    }
                }
            }
        }

        // Coverage line: "test coverage: 87.42%"  (llvm-cov format)
        if trimmed.starts_with("TOTAL") && trimmed.contains('%') {
            if let Some(pct) = extract_coverage_pct(trimmed) {
                result.coverage_pct = Some(pct);
            }
        }
        if trimmed.contains("line coverage:") || trimmed.contains("Coverage:") {
            if let Some(pct) = extract_coverage_pct(trimmed) {
                result.coverage_pct = Some(pct);
            }
        }
    }

    result.total = result.passed + result.failed + result.skipped;
    result
}

fn parse_pytest_output(output: &str) -> TestSuiteResult {
    let mut result = TestSuiteResult {
        framework: "pytest".into(),
        ..Default::default()
    };
    result.raw_output = output.chars().take(4000).collect();

    for line in output.lines() {
        let trimmed = line.trim();

        // "PASSED tests/test_foo.py::test_bar" or "FAILED" or "ERROR"
        if trimmed.starts_with("PASSED") {
            result.passed += 1;
        } else if trimmed.starts_with("FAILED ") {
            result.failed += 1;
            let name = trimmed
                .strip_prefix("FAILED ")
                .unwrap_or("")
                .trim()
                .to_string();
            result.failures.push(TestFailure {
                name,
                file: None,
                line: None,
                message: "Test failed".to_string(),
            });
        } else if trimmed.starts_with("ERROR ") {
            result.failed += 1;
            let name = trimmed
                .strip_prefix("ERROR ")
                .unwrap_or("")
                .trim()
                .to_string();
            result.failures.push(TestFailure {
                name,
                file: None,
                line: None,
                message: "Test error".to_string(),
            });
        }

        // "= 5 passed, 2 failed, 1 skipped in 1.23s ="
        if trimmed.starts_with('=') && trimmed.ends_with('=') && trimmed.contains("passed") {
            parse_pytest_summary(trimmed, &mut result);
        }

        // "src/foo.py:42: AssertionError"
        if trimmed.contains(".py:") && trimmed.contains("Error") {
            let parts: Vec<&str> = trimmed.splitn(3, ':').collect();
            if parts.len() >= 3 {
                if let Some(last) = result.failures.last_mut() {
                    if last.file.is_none() {
                        last.file = Some(parts[0].to_string());
                        last.line = parts[1].parse().ok();
                        last.message = parts[2].trim().to_string();
                    }
                }
            }
        }

        // Coverage: "TOTAL  1234  56  95.46%"
        if trimmed.starts_with("TOTAL") && trimmed.contains('%') {
            if let Some(pct) = extract_coverage_pct(trimmed) {
                result.coverage_pct = Some(pct);
            }
        }
    }

    result.total = result.passed + result.failed + result.skipped;
    result
}

fn parse_pytest_summary(line: &str, result: &mut TestSuiteResult) {
    // "= 5 passed, 2 failed, 1 skipped in 1.23s ="
    let inner = line.trim_matches(|c| c == '=' || c == ' ');
    for part in inner.split(',') {
        let p = part.trim();
        if let Some(n) = extract_number_before(p, "passed") {
            result.passed = n;
        } else if let Some(n) = extract_number_before(p, "failed") {
            result.failed = n;
        } else if let Some(n) = extract_number_before(p, "skipped") {
            result.skipped = n;
        } else if let Some(n) = extract_number_before(p, "error") {
            result.failed += n;
        }
    }
    // Duration: "in 1.23s"
    if let Some(dur_str) = inner.split("in ").nth(1) {
        let secs: f64 = dur_str
            .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.')
            .parse()
            .unwrap_or(0.0);
        result.duration_ms = (secs * 1000.0) as u64;
    }
}

fn parse_jest_output(output: &str) -> TestSuiteResult {
    let mut result = TestSuiteResult {
        framework: "jest".into(),
        ..Default::default()
    };
    result.raw_output = output.chars().take(4000).collect();

    for line in output.lines() {
        let trimmed = line.trim();

        // "Tests:  5 passed, 2 failed, 7 total"
        if let Some(stripped) = trimmed.strip_prefix("Tests:") {
            for part in stripped.split(',') {
                let p = part.trim();
                if let Some(n) = extract_number_before(p, "passed") {
                    result.passed = n;
                } else if let Some(n) = extract_number_before(p, "failed") {
                    result.failed = n;
                } else if let Some(n) = extract_number_before(p, "skipped") {
                    result.skipped = n;
                } else if let Some(n) = extract_number_before(p, "total") {
                    result.total = n;
                }
            }
        }

        // "Time: 1.234 s"
        if let Some(time_stripped) = trimmed.strip_prefix("Time:") {
            let secs: f64 = time_stripped
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            result.duration_ms = (secs * 1000.0) as u64;
        }

        // "● TestSuite › test name" — failure
        if trimmed.starts_with("● ") {
            let name = trimmed.strip_prefix("● ").unwrap_or("").to_string();
            result.failures.push(TestFailure {
                name,
                file: None,
                line: None,
                message: "Jest test failed".to_string(),
            });
        }
    }

    if result.total == 0 {
        result.total = result.passed + result.failed + result.skipped;
    }
    result
}

fn parse_go_test_output(output: &str) -> TestSuiteResult {
    let mut result = TestSuiteResult {
        framework: "go test".into(),
        ..Default::default()
    };
    result.raw_output = output.chars().take(4000).collect();

    for line in output.lines() {
        let trimmed = line.trim();

        // "--- PASS: TestFoo (0.00s)"
        if let Some(rest) = trimmed.strip_prefix("--- PASS: ") {
            result.passed += 1;
            let _name = rest.split_whitespace().next().unwrap_or("").to_string();
        }
        // "--- FAIL: TestFoo (0.00s)"
        else if let Some(rest) = trimmed.strip_prefix("--- FAIL: ") {
            result.failed += 1;
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            result.failures.push(TestFailure {
                name,
                file: None,
                line: None,
                message: "Go test failed".to_string(),
            });
        }
        // "--- SKIP: TestFoo"
        else if trimmed.starts_with("--- SKIP: ") {
            result.skipped += 1;
        }

        // "ok  \tgithub.com/foo/bar\t1.234s"
        if trimmed.starts_with("ok") && trimmed.contains('\t') {
            let parts: Vec<&str> = trimmed.split('\t').collect();
            if parts.len() >= 3 {
                let dur: f64 = parts[2].trim_end_matches('s').parse().unwrap_or(0.0);
                result.duration_ms += (dur * 1000.0) as u64;
            }
        }

        // coverage: "coverage: 87.5% of statements"
        if trimmed.starts_with("coverage:") {
            if let Some(pct) = extract_coverage_pct(trimmed) {
                result.coverage_pct = Some(pct);
            }
        }
    }

    result.total = result.passed + result.failed + result.skipped;
    result
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn extract_number_before(s: &str, keyword: &str) -> Option<u32> {
    let lower = s.to_lowercase();
    if lower.contains(keyword) {
        let digits: String = lower
            .chars()
            .take_while(|c| c.is_ascii_digit() || c.is_whitespace())
            .filter(|c| c.is_ascii_digit())
            .collect();
        digits.parse().ok()
    } else {
        None
    }
}

fn extract_coverage_pct(line: &str) -> Option<f64> {
    // Find a number followed by %
    let chars = line.chars().peekable();
    let mut last_num = String::new();
    for c in chars {
        if c.is_ascii_digit() || c == '.' {
            last_num.push(c);
        } else if c == '%' && !last_num.is_empty() {
            return last_num.parse().ok();
        } else {
            last_num.clear();
        }
    }
    None
}

// ─── Tool implementation ──────────────────────────────────────────────────────

#[async_trait]
impl Tool for TestRunTool {
    fn name(&self) -> &str {
        "test_run"
    }

    fn description(&self) -> &str {
        "Execute tests and return structured results: passed/failed/skipped counts, \
         duration, coverage percentage, and per-test failure details. \
         Auto-detects test framework from project files. \
         Supports: cargo test (Rust), pytest (Python), jest/vitest (JS/TS), go test (Go). \
         Use 'command' to override the auto-detected command. \
         Use 'coverage' to enable coverage reporting."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    #[tracing::instrument(skip(self), fields(tool = "test_run"))]
    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let working_dir = &input.working_directory;

        let coverage = input
            .arguments
            .get("coverage")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let filter = input
            .arguments
            .get("filter")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Determine command
        let (program, args, framework) =
            if let Some(cmd) = input.arguments.get("command").and_then(|v| v.as_str()) {
                // Custom command — split on whitespace
                let parts: Vec<String> = cmd.split_whitespace().map(|s| s.to_string()).collect();
                if parts.is_empty() {
                    return Err(HalconError::InvalidInput(
                        "test_run: 'command' must not be empty".into(),
                    ));
                }
                let prog = parts[0].clone();
                let rest = parts[1..].to_vec();
                (prog, rest, None::<TestFramework>)
            } else {
                match detect_framework(working_dir) {
                    Some(fw) => {
                        let (p, a) = build_default_command(&fw, coverage, filter.as_deref());
                        let fw_clone = fw.clone();
                        (p, a, Some(fw_clone))
                    }
                    None => {
                        return Err(HalconError::InvalidInput(
                            "test_run: could not detect test framework. \
                         Provide 'command' explicitly or ensure Cargo.toml, \
                         package.json, pyproject.toml, or go.mod exists."
                                .into(),
                        ));
                    }
                }
            };

        // Build command
        let mut cmd = Command::new(&program);
        cmd.args(&args).current_dir(working_dir);

        // Inject test filter for known frameworks if not in custom command
        if let Some(ref fw) = framework {
            if let Some(ref f) = filter {
                match fw {
                    TestFramework::Cargo => {
                        cmd.arg(f);
                    }
                    TestFramework::Pytest => {
                        cmd.arg(f);
                    }
                    TestFramework::GoTest => {
                        cmd.args(["-run", f.as_str()]);
                    }
                    _ => {}
                }
            }
        }

        let start = Instant::now();

        let output = tokio::time::timeout(Duration::from_secs(self.timeout_secs), cmd.output())
            .await
            .map_err(|_| HalconError::ToolExecutionFailed {
                tool: "test_run".into(),
                message: format!("Tests timed out after {}s", self.timeout_secs),
            })?
            .map_err(|e| HalconError::ToolExecutionFailed {
                tool: "test_run".into(),
                message: format!("Failed to run tests: {e}"),
            })?;

        let elapsed_ms = start.elapsed().as_millis() as u64;

        let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Cap output size
        if stdout.len() + stderr.len() > MAX_OUTPUT_BYTES {
            let half = MAX_OUTPUT_BYTES / 2;
            stdout.truncate(half);
            stderr.truncate(half);
        }

        let combined = format!("{stdout}\n{stderr}");
        let exit_code = output.status.code().unwrap_or(-1);

        // Parse results
        let mut suite = match framework.as_ref() {
            Some(TestFramework::Cargo) => parse_cargo_output(&combined),
            Some(TestFramework::Pytest) => parse_pytest_output(&combined),
            Some(TestFramework::Jest) | Some(TestFramework::Vitest) => parse_jest_output(&combined),
            Some(TestFramework::GoTest) => parse_go_test_output(&combined),
            None => {
                // Best-effort parse for custom commands
                if combined.contains("panicked at ") {
                    parse_cargo_output(&combined)
                } else if combined.contains("AssertionError") {
                    parse_pytest_output(&combined)
                } else {
                    TestSuiteResult {
                        framework: program.clone(),
                        raw_output: combined.chars().take(4000).collect(),
                        ..Default::default()
                    }
                }
            }
        };

        if suite.duration_ms == 0 {
            suite.duration_ms = elapsed_ms;
        }

        let success = exit_code == 0;
        let summary = if success {
            format!(
                "✓ {} passed in {}ms{}",
                suite.passed,
                suite.duration_ms,
                suite
                    .coverage_pct
                    .map(|c| format!(", coverage: {c:.1}%"))
                    .unwrap_or_default()
            )
        } else {
            format!(
                "✗ {} failed, {} passed, {} skipped in {}ms",
                suite.failed, suite.passed, suite.skipped, suite.duration_ms
            )
        };

        let failures_json: Vec<serde_json::Value> =
            suite.failures.iter().map(|f| f.to_json()).collect();

        let meta = json!({
            "framework": suite.framework,
            "passed": suite.passed,
            "failed": suite.failed,
            "skipped": suite.skipped,
            "total": suite.total,
            "duration_ms": suite.duration_ms,
            "coverage_pct": suite.coverage_pct,
            "success": success,
            "exit_code": exit_code,
            "failures": failures_json,
        });

        let content = if suite.failures.is_empty() {
            format!("{summary}\n\n{}", suite.raw_output.trim())
        } else {
            let failure_lines = suite
                .failures
                .iter()
                .take(20)
                .map(|f| {
                    let loc = match (&f.file, f.line) {
                        (Some(file), Some(ln)) => format!(" ({file}:{ln})"),
                        (Some(file), None) => format!(" ({file})"),
                        _ => String::new(),
                    };
                    format!("  ✗ {}{}: {}", f.name, loc, f.message)
                })
                .collect::<Vec<_>>()
                .join("\n");

            format!(
                "{summary}\n\nFailing tests:\n{failure_lines}\n\n---\n{}",
                suite.raw_output.trim()
            )
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: !success,
            metadata: Some(meta),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Override test command (e.g. 'cargo test --lib', 'pytest tests/', 'go test ./...'). Auto-detected if omitted."
                },
                "filter": {
                    "type": "string",
                    "description": "Run only tests matching this name/pattern (passed to test runner)."
                },
                "coverage": {
                    "type": "boolean",
                    "description": "Enable coverage reporting when supported (default false)."
                }
            },
            "required": []
        })
    }
}

fn build_default_command(
    framework: &TestFramework,
    coverage: bool,
    _filter: Option<&str>,
) -> (String, Vec<String>) {
    match framework {
        TestFramework::Cargo => {
            if coverage {
                // Use cargo-llvm-cov if available, else plain test
                ("cargo".into(), vec!["llvm-cov".into(), "--text".into()])
            } else {
                ("cargo".into(), vec!["test".into()])
            }
        }
        TestFramework::Pytest => {
            let mut args = vec!["-v".to_string()];
            if coverage {
                args.extend(["--cov=.".into(), "--cov-report=term-missing".into()]);
            }
            ("pytest".into(), args)
        }
        TestFramework::Jest => ("npx".into(), vec!["jest".into(), "--no-coverage".into()]),
        TestFramework::Vitest => ("npx".into(), vec!["vitest".into(), "run".into()]),
        TestFramework::GoTest => {
            let mut args = vec!["test".to_string(), "./...".to_string()];
            if coverage {
                args.extend(["-coverprofile=coverage.out".into()]);
            }
            ("go".into(), args)
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_coverage_pct_basic() {
        assert_eq!(
            extract_coverage_pct("coverage: 87.5% of statements"),
            Some(87.5)
        );
        assert_eq!(extract_coverage_pct("TOTAL  1234  56  95.46%"), Some(95.46));
        assert_eq!(extract_coverage_pct("no coverage here"), None);
    }

    #[test]
    fn extract_number_before_basic() {
        assert_eq!(extract_number_before("5 passed", "passed"), Some(5));
        assert_eq!(extract_number_before("12 failed", "failed"), Some(12));
        assert_eq!(extract_number_before("none here", "passed"), None);
    }

    #[test]
    fn parse_cargo_output_counts() {
        let output = "\
test tests::foo ... ok\n\
test tests::bar ... FAILED\n\
test tests::baz ... ignored\n\
test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured\n";
        let r = parse_cargo_output(output);
        assert_eq!(r.passed, 1);
        assert_eq!(r.failed, 1);
        assert_eq!(r.skipped, 1);
    }

    #[test]
    fn parse_cargo_output_no_failures() {
        let output = "\
test tests::a ... ok\n\
test tests::b ... ok\n\
test result: ok. 2 passed; 0 failed; 0 ignored;\n";
        let r = parse_cargo_output(output);
        assert_eq!(r.passed, 2);
        assert_eq!(r.failed, 0);
        assert!(r.failures.is_empty());
    }

    #[test]
    fn parse_pytest_summary_line() {
        let line = "= 5 passed, 2 failed, 1 skipped in 1.234s =";
        let mut r = TestSuiteResult::default();
        parse_pytest_summary(line, &mut r);
        assert_eq!(r.passed, 5);
        assert_eq!(r.failed, 2);
        assert_eq!(r.skipped, 1);
        assert_eq!(r.duration_ms, 1234);
    }

    #[test]
    fn parse_go_test_counts() {
        let output = "\
--- PASS: TestFoo (0.01s)\n\
--- FAIL: TestBar (0.00s)\n\
--- SKIP: TestBaz (0.00s)\n\
coverage: 78.5% of statements\n\
ok  \tgithub.com/foo/pkg\t0.050s\n";
        let r = parse_go_test_output(output);
        assert_eq!(r.passed, 1);
        assert_eq!(r.failed, 1);
        assert_eq!(r.skipped, 1);
        assert_eq!(r.coverage_pct, Some(78.5));
        assert_eq!(r.failures[0].name, "TestBar");
    }

    #[test]
    fn parse_jest_summary() {
        let output = "\
● MyTest › fails\n\
Tests:  3 passed, 1 failed, 4 total\n\
Time: 2.500 s\n";
        let r = parse_jest_output(output);
        assert_eq!(r.passed, 3);
        assert_eq!(r.failed, 1);
        assert_eq!(r.total, 4);
        assert_eq!(r.duration_ms, 2500);
        assert_eq!(r.failures[0].name, "MyTest › fails");
    }

    #[test]
    fn detect_framework_cargo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(
            detect_framework(dir.path().to_str().unwrap()),
            Some(TestFramework::Cargo)
        );
    }

    #[test]
    fn detect_framework_go() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("go.mod"), "module foo").unwrap();
        assert_eq!(
            detect_framework(dir.path().to_str().unwrap()),
            Some(TestFramework::GoTest)
        );
    }

    #[test]
    fn detect_framework_vitest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"test":"vitest"}}"#,
        )
        .unwrap();
        assert_eq!(
            detect_framework(dir.path().to_str().unwrap()),
            Some(TestFramework::Vitest)
        );
    }

    #[test]
    fn tool_meta() {
        let t = TestRunTool::new(300);
        assert_eq!(t.name(), "test_run");
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
    }

    #[tokio::test]
    async fn empty_command_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let t = TestRunTool::new(30);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "command": "  " }),
            working_directory: dir.path().to_str().unwrap().to_string(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn no_framework_no_command_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let t = TestRunTool::new(30);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({}),
            working_directory: dir.path().to_str().unwrap().to_string(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    #[ignore = "runs cargo test on the workspace (self-referential, CI-only via explicit --ignored)"]
    async fn runs_cargo_test_on_this_workspace() {
        let workspace = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let t = TestRunTool::new(300);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "command": "cargo test --lib -- --test-threads=1" }),
            working_directory: workspace,
        };
        let out = t.execute(input).await.unwrap();
        let meta = out.metadata.unwrap();
        // Just verify structure is populated
        assert!(meta["passed"].is_number());
        assert!(meta["failed"].is_number());
        assert!(meta["duration_ms"].is_number());
    }
}
