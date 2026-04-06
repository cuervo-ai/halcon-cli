//! Structured test execution tool with traceback parsing.
//!
//! Executes test commands (pytest, cargo test, npm test/jest/vitest, go test)
//! and returns structured failure information for the agent to act on.

use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;
use tokio::process::Command;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

/// Maximum summary characters for failure messages (raised from 200).
const MAX_SUMMARY_CHARS: usize = 2000;

/// Simple failure representation for JSON serialization.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParsedFailure {
    pub failure_type: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub test_name: Option<String>,
    pub summary: String,
}

impl ParsedFailure {
    fn from_raw(output: &str) -> Self {
        Self {
            failure_type: "Unknown".to_string(),
            file: None,
            line: None,
            test_name: None,
            summary: output.chars().take(MAX_SUMMARY_CHARS).collect(),
        }
    }
}

/// Parse test output auto-detecting the test framework.
fn parse_test_output(output: &str) -> Vec<ParsedFailure> {
    if output.contains("AssertionError") || output.contains("FAILED") && output.contains(".py") {
        parse_pytest_simple(output)
    } else if output.contains("panicked at ") {
        parse_cargo_simple(output)
    } else if output.contains("FAIL\t") || output.contains("--- FAIL") {
        parse_go_test(output)
    } else if output.contains("● ")
        && (output.contains("jest") || output.contains("JEST") || output.contains("Test Suites:"))
    {
        parse_jest(output)
    } else if output.contains("FAIL") && output.contains("vitest") {
        parse_vitest(output)
    } else if output.contains("<testsuites") || output.contains("<testsuite") {
        parse_junit_xml(output)
    } else {
        vec![ParsedFailure::from_raw(output)]
    }
}

// ─── pytest ───────────────────────────────────────────────────────────────────

fn parse_pytest_simple(output: &str) -> Vec<ParsedFailure> {
    let mut failures = Vec::new();

    for line in output.lines() {
        if line.contains(".py:") && (line.contains("Error") || line.contains("FAILED")) {
            let parts: Vec<&str> = line.splitn(3, ':').collect();
            if parts.len() >= 3 {
                let file = parts[0].trim();
                let line_num = parts[1].trim().parse::<u32>().ok();
                let error_type = parts[2].trim();

                failures.push(ParsedFailure {
                    failure_type: "Assertion".to_string(),
                    file: Some(file.to_string()),
                    line: line_num,
                    test_name: None,
                    summary: format!("{} at {}:{}", error_type, file, line_num.unwrap_or(0)),
                });
            }
        }
        // Also capture FAILED test::path lines
        if line.trim_start().starts_with("FAILED ") {
            let test_name = line
                .trim_start()
                .strip_prefix("FAILED ")
                .unwrap_or("")
                .trim();
            if !test_name.is_empty()
                && failures
                    .last()
                    .map(|f: &ParsedFailure| f.test_name.as_deref())
                    != Some(Some(test_name))
            {
                if let Some(last) = failures.last_mut() {
                    last.test_name = Some(test_name.to_string());
                } else {
                    failures.push(ParsedFailure {
                        failure_type: "Assertion".to_string(),
                        file: None,
                        line: None,
                        test_name: Some(test_name.to_string()),
                        summary: format!("Test failed: {test_name}"),
                    });
                }
            }
        }
    }

    if failures.is_empty() {
        failures.push(ParsedFailure::from_raw(output));
    }

    failures
}

// ─── cargo test ───────────────────────────────────────────────────────────────

fn parse_cargo_simple(output: &str) -> Vec<ParsedFailure> {
    let mut failures = Vec::new();

    for line in output.lines() {
        if line.contains("panicked at ") {
            if let Some(start) = line.find("panicked at ") {
                let rest = &line[start + 12..];
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() >= 2 {
                    let file = parts[0].trim();
                    let line_num = parts[1].parse::<u32>().ok();

                    failures.push(ParsedFailure {
                        failure_type: "Panic".to_string(),
                        file: Some(file.to_string()),
                        line: line_num,
                        test_name: None,
                        summary: format!("panic at {}:{}", file, line_num.unwrap_or(0)),
                    });
                }
            }
        }
        // Capture "FAILED tests::module::test_name" lines
        if line.starts_with("FAILED ") || line.contains(" FAILED") && !line.contains(".py:") {
            let test_name = line
                .trim()
                .strip_prefix("FAILED")
                .unwrap_or(line.trim())
                .trim();
            if !test_name.is_empty() {
                if let Some(last) = failures.last_mut() {
                    if last.test_name.is_none() {
                        last.test_name = Some(test_name.to_string());
                    }
                }
            }
        }
    }

    if failures.is_empty() {
        failures.push(ParsedFailure::from_raw(output));
    }

    failures
}

// ─── go test ─────────────────────────────────────────────────────────────────

fn parse_go_test(output: &str) -> Vec<ParsedFailure> {
    let mut failures = Vec::new();

    for line in output.lines() {
        // "--- FAIL: TestFunctionName (0.00s)"
        if let Some(rest) = line.trim().strip_prefix("--- FAIL: ") {
            let test_name = rest.split_whitespace().next().unwrap_or(rest).to_string();
            failures.push(ParsedFailure {
                failure_type: "GoTestFail".to_string(),
                file: None,
                line: None,
                test_name: Some(test_name.clone()),
                summary: format!("Go test failed: {test_name}"),
            });
        }
        // "FAIL\tpackage/path [build failed]"
        if line.starts_with("FAIL\t") {
            let pkg = line.strip_prefix("FAIL\t").unwrap_or("").trim();
            failures.push(ParsedFailure {
                failure_type: "GoBuildFail".to_string(),
                file: None,
                line: None,
                test_name: Some(pkg.to_string()),
                summary: format!("Go package failed: {pkg}"),
            });
        }
        // File:line error messages like "foo_test.go:42: some error"
        if line.contains("_test.go:") {
            let parts: Vec<&str> = line.trim().splitn(3, ':').collect();
            if parts.len() >= 3 {
                let file = parts[0];
                let ln = parts[1].parse::<u32>().ok();
                let msg = parts[2].trim();
                if let Some(last) = failures.last_mut() {
                    if last.file.is_none() {
                        last.file = Some(file.to_string());
                        last.line = ln;
                        last.summary = format!("{msg} at {file}:{}", ln.unwrap_or(0));
                    }
                } else {
                    failures.push(ParsedFailure {
                        failure_type: "GoTestError".to_string(),
                        file: Some(file.to_string()),
                        line: ln,
                        test_name: None,
                        summary: msg.chars().take(MAX_SUMMARY_CHARS).collect(),
                    });
                }
            }
        }
    }

    if failures.is_empty() {
        failures.push(ParsedFailure::from_raw(output));
    }

    failures
}

// ─── Jest / Vitest ────────────────────────────────────────────────────────────

fn parse_jest(output: &str) -> Vec<ParsedFailure> {
    let mut failures = Vec::new();
    let lines: Vec<&str> = output.lines().collect();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();

        // "● TestSuite › test name"  — Jest failure header
        if line.starts_with("● ") {
            let test_path = line.strip_prefix("● ").unwrap_or(line);
            let mut file: Option<String> = None;
            let mut ln: Option<u32> = None;
            let mut summary_lines: Vec<&str> = vec![test_path];

            // Scan forward for the error details and "at Object.<anon> (file:line)"
            for detail_line in lines.iter().take(lines.len().min(i + 30)).skip(i + 1) {
                let detail = detail_line.trim();
                if detail.is_empty() || detail.starts_with("● ") {
                    break;
                }
                // Location like "  at Object.<anon> (/path/file.test.ts:42:5)"
                if detail.contains(".test.") || detail.contains(".spec.") {
                    if let Some(start) = detail.find('(') {
                        if let Some(end) = detail.rfind(')') {
                            let loc = &detail[start + 1..end];
                            let parts: Vec<&str> = loc.rsplitn(3, ':').collect();
                            if parts.len() >= 2 {
                                ln = parts[1].parse::<u32>().ok();
                                file = Some(parts[2].to_string());
                            }
                        }
                    }
                }
                summary_lines.push(detail);
            }

            let summary: String = summary_lines
                .join("\n")
                .chars()
                .take(MAX_SUMMARY_CHARS)
                .collect();
            failures.push(ParsedFailure {
                failure_type: "JestFail".to_string(),
                file,
                line: ln,
                test_name: Some(test_path.to_string()),
                summary,
            });
        }

        i += 1;
    }

    if failures.is_empty() {
        failures.push(ParsedFailure::from_raw(output));
    }

    failures
}

fn parse_vitest(output: &str) -> Vec<ParsedFailure> {
    let mut failures = Vec::new();

    for line in output.lines() {
        // "FAIL  src/foo.test.ts" or "× test name"
        if line.trim_start().starts_with("FAIL ")
            && (line.contains(".test.") || line.contains(".spec."))
        {
            let file = line
                .trim()
                .strip_prefix("FAIL")
                .unwrap_or("")
                .trim()
                .to_string();
            failures.push(ParsedFailure {
                failure_type: "VitestFail".to_string(),
                file: Some(file.clone()),
                line: None,
                test_name: None,
                summary: format!("Vitest file failed: {file}"),
            });
        }
        // "× test name" is a vitest failure indicator
        if line.trim_start().starts_with("× ") || line.trim_start().starts_with("✗ ") {
            let name = line
                .trim()
                .trim_start_matches(['×', '✗', ' '])
                .trim()
                .to_string();
            if let Some(last) = failures.last_mut() {
                if last.test_name.is_none() {
                    last.test_name = Some(name);
                    continue;
                }
            }
            failures.push(ParsedFailure {
                failure_type: "VitestFail".to_string(),
                file: None,
                line: None,
                test_name: Some(name.clone()),
                summary: format!("Test failed: {name}"),
            });
        }
    }

    if failures.is_empty() {
        failures.push(ParsedFailure::from_raw(output));
    }

    failures
}

// ─── JUnit XML ───────────────────────────────────────────────────────────────

fn parse_junit_xml(output: &str) -> Vec<ParsedFailure> {
    let mut failures = Vec::new();

    // Simple XML text scan: find <failure> and nearby <testcase> attributes
    // without a full XML parser (keeps dependencies minimal)
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.contains("<failure") || trimmed.contains("<error") {
            // Look for testcase name attribute in the same or previous lines
            failures.push(ParsedFailure {
                failure_type: "JUnit".to_string(),
                file: None,
                line: None,
                test_name: extract_xml_attr(trimmed, "classname")
                    .or_else(|| extract_xml_attr(trimmed, "name")),
                summary: trimmed.chars().take(MAX_SUMMARY_CHARS).collect(),
            });
        }
        // <testcase name="..." classname="..." (with nested failure)
        if trimmed.starts_with("<testcase") && !trimmed.ends_with("/>") {
            let name = extract_xml_attr(trimmed, "name");
            let class = extract_xml_attr(trimmed, "classname");
            if let Some(last) = failures.last_mut() {
                if last.test_name.is_none() {
                    last.test_name = name.or(class);
                }
            }
        }
    }

    if failures.is_empty() {
        failures.push(ParsedFailure::from_raw(output));
    }

    failures
}

/// Extract an attribute value from a simple XML element string.
fn extract_xml_attr(s: &str, attr: &str) -> Option<String> {
    let needle = format!(" {attr}=\"");
    let start = s.find(&needle)? + needle.len();
    let end = s[start..].find('"')?;
    let val = &s[start..start + end];
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

// ─── Tool implementation ───────────────────────────────────────────────────────

/// Tool for executing tests with structured output parsing.
pub struct ExecuteTestTool {
    timeout_secs: u64,
}

impl ExecuteTestTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }
}

#[async_trait]
impl Tool for ExecuteTestTool {
    fn name(&self) -> &str {
        "execute_test"
    }

    fn description(&self) -> &str {
        "Execute test commands and parse failures into structured format. \
         Returns file locations and actionable summaries. \
         Supports: pytest, cargo test, npm test, jest, vitest, go test, JUnit XML."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Test command to execute (e.g., 'pytest tests/', 'cargo test', 'go test ./...', 'npx jest')"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory (optional)"
                }
            },
            "required": ["command"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let command = input
            .arguments
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| HalconError::ToolExecutionFailed {
                tool: self.name().to_string(),
                message: "Missing 'command' parameter".to_string(),
            })?;

        let working_dir = input.arguments.get("working_dir").and_then(|v| v.as_str());

        let parts: Vec<&str> = command.split_whitespace().collect();
        if parts.is_empty() {
            return Err(HalconError::ToolExecutionFailed {
                tool: self.name().to_string(),
                message: "Empty command".to_string(),
            });
        }

        let program = parts[0];
        let args = &parts[1..];

        let mut cmd = Command::new(program);
        cmd.args(args);

        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        let output = tokio::time::timeout(Duration::from_secs(self.timeout_secs), cmd.output())
            .await
            .map_err(|_| HalconError::ToolExecutionFailed {
                tool: self.name().to_string(),
                message: format!("Command timed out after {}s", self.timeout_secs),
            })?
            .map_err(|e| HalconError::ToolExecutionFailed {
                tool: self.name().to_string(),
                message: format!("Failed to execute: {}", e),
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}\n{}", stdout, stderr);

        let exit_code = output.status.code().unwrap_or(-1);
        let success = output.status.success();

        let failures = if !success {
            parse_test_output(&combined)
        } else {
            Vec::new()
        };

        let summary = if success {
            "All tests passed".to_string()
        } else {
            format!("{} test failure(s) detected", failures.len())
        };

        let result = json!({
            "success": success,
            "exit_code": exit_code,
            "stdout": stdout.to_string(),
            "stderr": stderr.to_string(),
            "failures": failures,
            "summary": summary
        });

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: serde_json::to_string_pretty(&result).unwrap_or_default(),
            is_error: !success,
            metadata: Some(result),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pytest_output() {
        let output = "tests/test_math.py:42: AssertionError\nFAILED tests/test_math.py::test_add";
        let failures = parse_test_output(output);

        assert!(!failures.is_empty());
        assert_eq!(failures[0].file, Some("tests/test_math.py".to_string()));
        assert_eq!(failures[0].line, Some(42));
    }

    #[test]
    fn test_parse_cargo_output() {
        let output = "thread 'test' panicked at src/lib.rs:15:5:\nassertion failed";
        let failures = parse_test_output(output);

        assert!(!failures.is_empty());
        assert_eq!(failures[0].file, Some("src/lib.rs".to_string()));
        assert_eq!(failures[0].line, Some(15));
    }

    #[tokio::test]
    async fn test_tool_schema() {
        let tool = ExecuteTestTool::new(120);
        assert_eq!(tool.name(), "execute_test");
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);

        let schema = tool.input_schema();
        assert!(schema["properties"]["command"].is_object());
    }

    // ─── Go test parsing ─────────────────────────────────────────────────────

    #[test]
    fn parse_go_fail_line() {
        let output = "--- FAIL: TestAddition (0.00s)\nFAIL\tgithub.com/foo/bar [build failed]";
        let failures = parse_test_output(output);
        assert!(!failures.is_empty());
        assert_eq!(failures[0].failure_type, "GoTestFail");
        assert_eq!(failures[0].test_name.as_deref(), Some("TestAddition"));
    }

    #[test]
    fn parse_go_build_fail() {
        let output = "FAIL\tgithub.com/example/pkg [build failed]";
        let failures = parse_test_output(output);
        assert!(!failures.is_empty());
        assert!(failures.iter().any(|f| f.failure_type == "GoBuildFail"));
    }

    // ─── Jest parsing ─────────────────────────────────────────────────────────

    #[test]
    fn parse_jest_failure() {
        let output = "● MyComponent › renders correctly\n\
                      Expected: true\n  Received: false\n\
                      Test Suites: 1 failed, 1 total";
        let failures = parse_test_output(output);
        assert!(!failures.is_empty());
        assert_eq!(failures[0].failure_type, "JestFail");
        assert!(failures[0]
            .test_name
            .as_ref()
            .map(|n| n.contains("renders"))
            .unwrap_or(false));
    }

    // ─── JUnit XML parsing ────────────────────────────────────────────────────

    #[test]
    fn parse_junit_xml_failure() {
        let output = r#"<testsuites>
  <testsuite name="MyTests">
    <testcase name="testFoo" classname="com.example.MyTest">
      <failure message="expected true but was false">assertion error</failure>
    </testcase>
  </testsuite>
</testsuites>"#;
        let failures = parse_test_output(output);
        assert!(!failures.is_empty());
        assert!(failures.iter().any(|f| f.failure_type == "JUnit"));
    }

    // ─── Truncation limit ─────────────────────────────────────────────────────

    #[test]
    fn summary_truncation_increased_to_2000() {
        // Generate output longer than 2000 chars
        let long_output = "x".repeat(5000);
        let failures = parse_test_output(&long_output);
        for f in &failures {
            assert!(
                f.summary.len() <= MAX_SUMMARY_CHARS,
                "summary too long: {} chars",
                f.summary.len()
            );
        }
    }

    // ─── extract_xml_attr ─────────────────────────────────────────────────────

    #[test]
    fn extract_xml_attr_basic() {
        let s = r#"<testcase name="testFoo" classname="com.example.Test">"#;
        assert_eq!(extract_xml_attr(s, "name"), Some("testFoo".to_string()));
        assert_eq!(
            extract_xml_attr(s, "classname"),
            Some("com.example.Test".to_string())
        );
        assert_eq!(extract_xml_attr(s, "time"), None);
    }

    // ─── description ─────────────────────────────────────────────────────────

    #[test]
    fn description_mentions_supported_frameworks() {
        let tool = ExecuteTestTool::new(120);
        let desc = tool.description();
        assert!(desc.contains("pytest"));
        assert!(desc.contains("cargo test"));
        assert!(desc.contains("jest"));
        assert!(desc.contains("go test"));
        assert!(desc.contains("JUnit"));
    }
}
