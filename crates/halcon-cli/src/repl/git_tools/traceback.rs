//! Structured traceback parsing for test execution errors.
//!
//! Extracts actionable information from test failures:
//! - File:line location
//! - Assertion type (expected vs actual)
//! - Stack trace context
//! - Error classification
//!
//! Supports: pytest, cargo test, unittest, jest, node.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Classification of test failure types for targeted remediation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureType {
    /// Assertion failure: expected vs actual mismatch.
    Assertion,
    /// Runtime exception (KeyError, IndexError, NullPointer, etc.).
    Exception,
    /// Timeout during test execution.
    Timeout,
    /// Syntax error in test code.
    SyntaxError,
    /// Import/module not found.
    ImportError,
    /// Compilation error (Rust, C++, etc.).
    CompileError,
    /// Unknown/unparsed failure type.
    Unknown,
}

/// Structured representation of a test failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedFailure {
    /// Type of failure for classification.
    pub failure_type: FailureType,
    /// Primary file where failure occurred.
    pub file: Option<PathBuf>,
    /// Line number in file.
    pub line: Option<u32>,
    /// Function/test name that failed.
    pub test_name: Option<String>,
    /// Expected value (for assertions).
    pub expected: Option<String>,
    /// Actual value (for assertions).
    pub actual: Option<String>,
    /// Exception type (e.g., "KeyError", "NullPointerException").
    pub exception_type: Option<String>,
    /// Full stack trace (limited to 10 frames).
    pub stack_trace: Vec<StackFrame>,
    /// Raw error message (first 500 chars).
    pub raw_message: String,
}

/// A single stack frame in a traceback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackFrame {
    pub file: PathBuf,
    pub line: u32,
    pub function: Option<String>,
    pub code_snippet: Option<String>,
}

impl ParsedFailure {
    /// Create a minimal failure from raw output when parsing fails.
    pub fn from_raw(output: &str) -> Self {
        Self {
            failure_type: FailureType::Unknown,
            file: None,
            line: None,
            test_name: None,
            expected: None,
            actual: None,
            exception_type: None,
            stack_trace: Vec::new(),
            raw_message: output.chars().take(500).collect(),
        }
    }

    /// Check if this failure has actionable location info (file + line).
    pub fn has_location(&self) -> bool {
        self.file.is_some() && self.line.is_some()
    }

    /// Generate a concise summary for LLM context injection.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();

        // Location
        if let (Some(file), Some(line)) = (&self.file, self.line) {
            parts.push(format!("{}:{}", file.display(), line));
        }

        // Test name
        if let Some(name) = &self.test_name {
            parts.push(format!("test: {}", name));
        }

        // Failure details
        match self.failure_type {
            FailureType::Assertion => {
                if let (Some(exp), Some(act)) = (&self.expected, &self.actual) {
                    parts.push(format!("expected: {}, got: {}", exp, act));
                }
            }
            FailureType::Exception => {
                if let Some(exc) = &self.exception_type {
                    parts.push(format!("exception: {}", exc));
                }
            }
            _ => {
                parts.push(format!("type: {:?}", self.failure_type));
            }
        }

        parts.join(" | ")
    }
}

/// Parse pytest output into structured failures.
///
/// Recognizes patterns like:
/// ```text
/// tests/test_foo.py:42: AssertionError
/// assert x == 5
///     where x = 3
/// ```
pub fn parse_pytest(output: &str) -> Vec<ParsedFailure> {
    let mut failures = Vec::new();
    let lines: Vec<&str> = output.lines().collect();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        // Pattern: "tests/foo.py:123: AssertionError"
        if let Some(cap) = extract_pytest_location(line) {
            let mut failure = ParsedFailure::from_raw(line);
            failure.file = Some(PathBuf::from(cap.0));
            failure.line = Some(cap.1);
            failure.exception_type = Some(cap.2.to_string());

            // Look ahead for assertion details
            if cap.2 == "AssertionError" {
                failure.failure_type = FailureType::Assertion;
                if i + 1 < lines.len() && lines[i + 1].trim().starts_with("assert ") {
                    failure.raw_message = lines[i + 1].trim().to_string();
                    // Try to extract "where x = ..." pattern
                    if i + 2 < lines.len() && lines[i + 2].trim().starts_with("where ") {
                        let where_line = lines[i + 2].trim();
                        if let Some((var, val)) = parse_where_clause(where_line) {
                            failure.actual = Some(format!("{} = {}", var, val));
                        }
                    }
                }
            } else {
                failure.failure_type = FailureType::Exception;
            }

            failures.push(failure);
        }

        i += 1;
    }

    if failures.is_empty() {
        // Fallback: create generic failure
        failures.push(ParsedFailure::from_raw(output));
    }

    failures
}

/// Parse cargo test output into structured failures.
///
/// Recognizes patterns like:
/// ```text
/// thread 'test_foo' panicked at src/lib.rs:42:5:
/// assertion `left == right` failed
///   left: 3
///  right: 5
/// ```
pub fn parse_cargo_test(output: &str) -> Vec<ParsedFailure> {
    let mut failures = Vec::new();
    let lines: Vec<&str> = output.lines().collect();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        // Pattern: "thread 'test_name' panicked at src/file.rs:42:5:"
        if line.contains("panicked at ") {
            if let Some((file, line_num, test_name)) = extract_cargo_panic(line) {
                let mut failure = ParsedFailure::from_raw(line);
                failure.file = Some(PathBuf::from(file));
                failure.line = Some(line_num);
                failure.test_name = test_name;

                // Look ahead for assertion details
                if i + 1 < lines.len() {
                    let next = lines[i + 1].trim();
                    if next.starts_with("assertion") {
                        failure.failure_type = FailureType::Assertion;
                        failure.raw_message = next.to_string();

                        // Extract left/right values
                        if i + 2 < lines.len() && lines[i + 2].trim().starts_with("left:") {
                            failure.actual = Some(lines[i + 2].trim().to_string());
                        }
                        if i + 3 < lines.len() && lines[i + 3].trim().starts_with("right:") {
                            failure.expected = Some(lines[i + 3].trim().to_string());
                        }
                    } else {
                        failure.failure_type = FailureType::Exception;
                        failure.raw_message = next.to_string();
                    }
                }

                failures.push(failure);
            }
        }

        i += 1;
    }

    if failures.is_empty() {
        failures.push(ParsedFailure::from_raw(output));
    }

    failures
}

/// Parse unittest (Python) output.
pub fn parse_unittest(output: &str) -> Vec<ParsedFailure> {
    // Similar to pytest but with different format
    // "FAIL: test_something (tests.test_module.TestClass)"
    // "  File \"/path/file.py\", line 123, in test_something"
    parse_pytest(output) // Reuse pytest parser for now (compatible format)
}

/// Parse jest (JavaScript) output.
pub fn parse_jest(output: &str) -> Vec<ParsedFailure> {
    let mut failures = Vec::new();
    let lines: Vec<&str> = output.lines().collect();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        // Pattern: "● test_name › subtest"
        if line.trim().starts_with('●') {
            let test_name = line.trim().trim_start_matches('●').trim().to_string();
            let mut failure = ParsedFailure::from_raw(line);
            failure.test_name = Some(test_name);
            failure.failure_type = FailureType::Assertion;

            // Look for file location in next lines
            for loc_line in &lines[(i + 1)..lines.len().min(i + 10)] {
                if let Some((file, line_num)) = extract_jest_location(loc_line) {
                    failure.file = Some(PathBuf::from(file));
                    failure.line = Some(line_num);
                    break;
                }
            }

            failures.push(failure);
        }

        i += 1;
    }

    if failures.is_empty() {
        failures.push(ParsedFailure::from_raw(output));
    }

    failures
}

/// Auto-detect test framework and parse accordingly.
pub fn parse_auto(output: &str) -> Vec<ParsedFailure> {
    // Detection heuristics
    if output.contains("pytest") || output.contains("AssertionError") {
        parse_pytest(output)
    } else if output.contains("panicked at ") || output.contains("cargo test") {
        parse_cargo_test(output)
    } else if output.contains("FAIL:") || output.contains("unittest") {
        parse_unittest(output)
    } else if output.contains("FAIL") && (output.contains("jest") || output.contains(".test.js")) {
        parse_jest(output)
    } else {
        // Unknown format
        vec![ParsedFailure::from_raw(output)]
    }
}

// --- Helper functions ---

/// Extract file:line:exception from pytest output.
/// Returns: (file, line, exception_type)
fn extract_pytest_location(line: &str) -> Option<(&str, u32, &str)> {
    // Pattern: "tests/test_foo.py:42: AssertionError"
    let parts: Vec<&str> = line.splitn(2, ':').collect();
    if parts.len() != 2 {
        return None;
    }

    let file_part = parts[0].trim();
    let rest = parts[1];

    let parts2: Vec<&str> = rest.splitn(2, ':').collect();
    if parts2.len() != 2 {
        return None;
    }

    let line_num: u32 = parts2[0].trim().parse().ok()?;
    let exception = parts2[1].trim();

    Some((file_part, line_num, exception))
}

/// Parse "where x = 3" clause from pytest assertion.
fn parse_where_clause(line: &str) -> Option<(String, String)> {
    // "where x = 3" or "where len(s) = 0"
    let trimmed = line.trim_start_matches("where ").trim();
    let parts: Vec<&str> = trimmed.splitn(2, '=').collect();
    if parts.len() == 2 {
        Some((parts[0].trim().to_string(), parts[1].trim().to_string()))
    } else {
        None
    }
}

/// Extract panic location from cargo test output.
/// Returns: (file, line, test_name)
fn extract_cargo_panic(line: &str) -> Option<(&str, u32, Option<String>)> {
    // "thread 'test_foo' panicked at src/lib.rs:42:5:"
    let test_name = if line.contains("thread '") {
        let start = line.find("thread '")?;
        let end = line[start + 8..].find('\'')?;
        Some(line[start + 8..start + 8 + end].to_string())
    } else {
        None
    };

    let panic_idx = line.find("panicked at ")?;
    let loc_part = &line[panic_idx + 12..];

    // "src/lib.rs:42:5:"
    let parts: Vec<&str> = loc_part.split(':').collect();
    if parts.len() < 2 {
        return None;
    }

    let file = parts[0].trim();
    let line_num: u32 = parts[1].parse().ok()?;

    Some((file, line_num, test_name))
}

/// Extract file:line from jest output.
fn extract_jest_location(line: &str) -> Option<(&str, u32)> {
    // "  at Object.<anonymous> (tests/foo.test.js:42:5)"
    if !line.contains('(') || !line.contains(')') {
        return None;
    }

    let start = line.rfind('(')?;
    let end = line.rfind(')')?;
    let loc = &line[start + 1..end];

    let parts: Vec<&str> = loc.split(':').collect();
    if parts.len() < 2 {
        return None;
    }

    let file = parts[0].trim();
    let line_num: u32 = parts[1].parse().ok()?;

    Some((file, line_num))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pytest_assertion() {
        let output = r#"
tests/test_math.py:42: AssertionError
assert result == 5
    where result = 3
"#;
        let failures = parse_pytest(output);
        assert_eq!(failures.len(), 1);
        let f = &failures[0];
        assert_eq!(f.failure_type, FailureType::Assertion);
        assert_eq!(
            f.file.as_ref().unwrap().to_str().unwrap(),
            "tests/test_math.py"
        );
        assert_eq!(f.line, Some(42));
        assert!(f.actual.is_some());
    }

    #[test]
    fn test_parse_cargo_panic() {
        let output = r#"
thread 'test_add' panicked at src/lib.rs:15:5:
assertion `left == right` failed
  left: 3
 right: 5
"#;
        let failures = parse_cargo_test(output);
        assert_eq!(failures.len(), 1);
        let f = &failures[0];
        assert_eq!(f.failure_type, FailureType::Assertion);
        assert_eq!(f.file.as_ref().unwrap().to_str().unwrap(), "src/lib.rs");
        assert_eq!(f.line, Some(15));
        assert_eq!(f.test_name, Some("test_add".to_string()));
    }

    #[test]
    fn test_auto_detect_pytest() {
        let output = "tests/foo.py:10: AssertionError";
        let failures = parse_auto(output);
        assert!(!failures.is_empty());
        assert_eq!(failures[0].failure_type, FailureType::Assertion);
    }

    #[test]
    fn test_auto_detect_cargo() {
        let output = "thread 'test' panicked at src/lib.rs:5:1:";
        let failures = parse_auto(output);
        assert!(!failures.is_empty());
    }

    #[test]
    fn test_failure_summary() {
        let failure = ParsedFailure {
            failure_type: FailureType::Assertion,
            file: Some(PathBuf::from("tests/foo.py")),
            line: Some(42),
            test_name: Some("test_bar".to_string()),
            expected: Some("5".to_string()),
            actual: Some("3".to_string()),
            exception_type: None,
            stack_trace: Vec::new(),
            raw_message: String::new(),
        };

        let summary = failure.summary();
        assert!(summary.contains("tests/foo.py:42"));
        assert!(summary.contains("test: test_bar"));
        assert!(summary.contains("expected: 5"));
        assert!(summary.contains("got: 3"));
    }
}
