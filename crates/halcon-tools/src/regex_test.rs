//! RegexTestTool — test regular expressions against input strings.
//!
//! Provides interactive regex testing without requiring any external tools:
//! - Test a pattern against input text
//! - Show all matches with capture group names/values
//! - Replace matches with a template string
//! - Validate regex syntax
//! - Supports Rust/PCRE-like syntax via the `regex` crate
//!
//! Useful for developing and debugging regex patterns in code.

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use regex::Regex;
use serde_json::{json, Value};

pub struct RegexTestTool;

impl RegexTestTool {
    pub fn new() -> Self {
        Self
    }

    fn build_regex(pattern: &str, flags: &str) -> Result<Regex, String> {
        let mut builder = regex::RegexBuilder::new(pattern);
        for c in flags.chars() {
            match c {
                'i' => {
                    builder.case_insensitive(true);
                }
                'm' => {
                    builder.multi_line(true);
                }
                's' => {
                    builder.dot_matches_new_line(true);
                }
                'x' => {
                    builder.ignore_whitespace(true);
                }
                ' ' | '\t' => {}
                other => return Err(format!("Unknown flag: '{}'", other)),
            }
        }
        builder.build().map_err(|e| format!("Invalid regex: {}", e))
    }

    fn find_all_matches(re: &Regex, input: &str) -> Vec<MatchResult> {
        let capture_names: Vec<Option<String>> = re
            .capture_names()
            .map(|n| n.map(|s| s.to_string()))
            .collect();

        re.captures_iter(input)
            .map(|caps| {
                let full_match = caps.get(0).map(|m| MatchSpan {
                    text: m.as_str().to_string(),
                    start: m.start(),
                    end: m.end(),
                });

                let groups: Vec<CaptureGroup> = caps
                    .iter()
                    .skip(1)
                    .enumerate()
                    .map(|(i, m)| {
                        let name = capture_names.get(i + 1).and_then(|n| n.clone());
                        CaptureGroup {
                            index: i + 1,
                            name,
                            value: m.map(|m| m.as_str().to_string()),
                        }
                    })
                    .collect();

                MatchResult { full_match, groups }
            })
            .take(100) // Cap at 100 matches
            .collect()
    }

    fn format_matches(matches: &[MatchResult], _input: &str) -> String {
        if matches.is_empty() {
            return "No matches found.".to_string();
        }

        let mut out = format!("{} match(es):\n\n", matches.len());
        for (i, m) in matches.iter().enumerate() {
            if let Some(ref span) = m.full_match {
                out.push_str(&format!(
                    "Match {}: {:?}  (pos {}..{})\n",
                    i + 1,
                    span.text,
                    span.start,
                    span.end
                ));
            }
            for group in &m.groups {
                let name_hint = group
                    .name
                    .as_deref()
                    .map(|n| format!(" ({})", n))
                    .unwrap_or_default();
                match &group.value {
                    Some(v) => {
                        out.push_str(&format!("  Group {}{}: {:?}\n", group.index, name_hint, v))
                    }
                    None => out.push_str(&format!(
                        "  Group {}{}: <no match>\n",
                        group.index, name_hint
                    )),
                }
            }
        }
        out
    }

    /// Annotate the input string showing where matches occur.
    fn annotate_input(re: &Regex, input: &str) -> String {
        if input.len() > 2000 {
            return "(input too long to annotate)".to_string();
        }

        let lines: Vec<&str> = input.lines().collect();
        let mut output = String::new();

        for (line_no, line) in lines.iter().enumerate() {
            output.push_str(&format!("{:3} | {}\n", line_no + 1, line));

            // Build annotation line
            let mut ann = String::from("    | ");
            let mut pos = 0;
            let line_start = input
                .lines()
                .take(line_no)
                .map(|l| l.len() + 1)
                .sum::<usize>();
            for m in re.find_iter(line) {
                let local_start = m.start();
                let local_end = m.end();
                while pos < local_start {
                    ann.push(' ');
                    pos += 1;
                }
                let span_len = local_end - local_start;
                if span_len > 0 {
                    ann.push('^');
                    for _ in 1..span_len {
                        ann.push('~');
                    }
                }
                pos = local_end;
            }
            if ann != "    | " {
                output.push_str(&ann);
                output.push('\n');
            }
            let _ = line_start; // suppress warning
        }

        output
    }
}

impl Default for RegexTestTool {
    fn default() -> Self {
        Self::new()
    }
}

struct MatchSpan {
    text: String,
    start: usize,
    end: usize,
}

struct CaptureGroup {
    index: usize,
    name: Option<String>,
    value: Option<String>,
}

struct MatchResult {
    full_match: Option<MatchSpan>,
    groups: Vec<CaptureGroup>,
}

#[async_trait]
impl Tool for RegexTestTool {
    fn name(&self) -> &str {
        "regex_test"
    }

    fn description(&self) -> &str {
        "Test regular expressions against input text. Shows all matches with capture groups, \
         can perform find/replace operations, and validates regex syntax. \
         Supports Rust regex syntax (similar to PCRE): named groups (?P<name>...), \
         non-capturing groups (?:...), lookahead (?=...) / (?!...), \
         flags: i (case-insensitive), m (multiline), s (dot-all), x (verbose). \
         Useful for developing and debugging regex patterns in code."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to test."
                },
                "input": {
                    "type": "string",
                    "description": "Text to match against the pattern."
                },
                "flags": {
                    "type": "string",
                    "description": "Regex flags: 'i' (case-insensitive), 'm' (multiline), 's' (dot-all), 'x' (verbose). Combine: 'im'."
                },
                "action": {
                    "type": "string",
                    "enum": ["test", "replace", "split", "validate"],
                    "description": "Action: 'test' (find matches), 'replace' (substitute), 'split' (split by pattern), 'validate' (check if pattern is valid). Default: 'test'."
                },
                "replacement": {
                    "type": "string",
                    "description": "Replacement string for 'replace' action. Use $1, $2 or ${name} for capture groups."
                },
                "annotate": {
                    "type": "boolean",
                    "description": "Show annotated input with match positions highlighted (default: false)."
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

        let pattern = match args["pattern"].as_str() {
            Some(p) => p,
            None => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: "'pattern' is required".to_string(),
                    is_error: true,
                    metadata: None,
                });
            }
        };

        let flags = args["flags"].as_str().unwrap_or("");
        let action = args["action"].as_str().unwrap_or("test");

        // Always validate the pattern first
        let re = match Self::build_regex(pattern, flags) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!("❌ Invalid regex pattern:\n{}", e),
                    is_error: true,
                    metadata: Some(json!({ "valid": false, "error": e })),
                });
            }
        };

        if action == "validate" {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!(
                    "✅ Valid regex pattern: {}\n\nCapture groups: {}",
                    pattern,
                    re.captures_len().saturating_sub(1)
                ),
                is_error: false,
                metadata: Some(
                    json!({ "valid": true, "capture_groups": re.captures_len().saturating_sub(1) }),
                ),
            });
        }

        let text = args["input"].as_str().unwrap_or("");
        if text.is_empty() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "✅ Pattern is valid. Provide 'input' text to test matches.".to_string(),
                is_error: false,
                metadata: Some(json!({ "valid": true })),
            });
        }

        let annotate = args["annotate"].as_bool().unwrap_or(false);

        let content = match action {
            "replace" => {
                let replacement = args["replacement"].as_str().unwrap_or("");
                let result = re.replace_all(text, replacement);
                format!(
                    "Pattern: {}\nInput:   {:?}\nResult:  {:?}\n\nReplacement: {:?}",
                    pattern,
                    text,
                    result.as_ref(),
                    replacement
                )
            }
            "split" => {
                let parts: Vec<&str> = re.split(text).collect();
                let mut out = format!(
                    "Pattern: {}\nInput:   {:?}\n\n{} part(s):\n",
                    pattern,
                    text,
                    parts.len()
                );
                for (i, part) in parts.iter().enumerate() {
                    out.push_str(&format!("  [{}] {:?}\n", i, part));
                }
                out
            }
            _ => {
                // "test"
                let matches = Self::find_all_matches(&re, text);
                let match_count = matches.len();
                let mut out = format!("Pattern: {}\nInput:   {:?}\n\n", pattern, text);

                if annotate {
                    out.push_str("Annotated:\n");
                    out.push_str(&Self::annotate_input(&re, text));
                    out.push('\n');
                }

                out.push_str(&Self::format_matches(&matches, text));

                if match_count == 100 {
                    out.push_str("\n(stopped at 100 matches)");
                }

                out
            }
        };

        let match_count = re.find_iter(text).count();
        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "pattern": pattern,
                "matches": match_count,
                "valid": true
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(args: Value) -> ToolInput {
        ToolInput {
            tool_use_id: "t1".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[test]
    fn build_regex_basic() {
        let re = RegexTestTool::build_regex(r"\d+", "").unwrap();
        assert!(re.is_match("hello 42 world"));
    }

    #[test]
    fn build_regex_case_insensitive() {
        let re = RegexTestTool::build_regex("hello", "i").unwrap();
        assert!(re.is_match("HELLO world"));
    }

    #[test]
    fn build_regex_invalid_returns_error() {
        let err = RegexTestTool::build_regex("(unclosed", "").unwrap_err();
        assert!(err.contains("Invalid regex") || err.contains("regex"));
    }

    #[test]
    fn find_all_matches_basic() {
        let re = RegexTestTool::build_regex(r"\d+", "").unwrap();
        let matches = RegexTestTool::find_all_matches(&re, "12 and 34 and 56");
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].full_match.as_ref().unwrap().text, "12");
    }

    #[test]
    fn find_all_matches_capture_groups() {
        let re = RegexTestTool::build_regex(r"(\w+)@(\w+)\.(\w+)", "").unwrap();
        let matches = RegexTestTool::find_all_matches(&re, "user@example.com");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].groups.len(), 3);
        assert_eq!(matches[0].groups[0].value.as_deref(), Some("user"));
    }

    #[tokio::test]
    async fn execute_test_action_finds_matches() {
        let tool = RegexTestTool::new();
        let out = tool
            .execute(make_input(json!({
                "pattern": r"\b\w{4}\b",
                "input": "hello from the code"
            })))
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
        assert!(out.content.contains("match"), "content: {}", out.content);
    }

    #[tokio::test]
    async fn execute_no_matches() {
        let tool = RegexTestTool::new();
        let out = tool
            .execute(make_input(json!({
                "pattern": r"\d{10}",
                "input": "no digits here"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("No matches") || out.content.contains("0 match"));
    }

    #[tokio::test]
    async fn execute_replace_action() {
        let tool = RegexTestTool::new();
        let out = tool
            .execute(make_input(json!({
                "pattern": r"\d+",
                "input": "foo 123 bar 456",
                "action": "replace",
                "replacement": "NUM"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("NUM"), "content: {}", out.content);
    }

    #[tokio::test]
    async fn execute_split_action() {
        let tool = RegexTestTool::new();
        let out = tool
            .execute(make_input(json!({
                "pattern": r"\s+",
                "input": "hello   world   foo",
                "action": "split"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("3 part") || out.content.contains("[0]"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_validate_valid_pattern() {
        let tool = RegexTestTool::new();
        let out = tool
            .execute(make_input(json!({
                "pattern": r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})",
                "action": "validate"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("Valid") || out.content.contains("valid"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_invalid_pattern_returns_error() {
        let tool = RegexTestTool::new();
        let out = tool
            .execute(make_input(json!({
                "pattern": "(unclosed group",
                "input": "test"
            })))
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(
            out.content.contains("Invalid") || out.content.contains("regex"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_no_input_returns_hint() {
        let tool = RegexTestTool::new();
        let out = tool
            .execute(make_input(json!({
                "pattern": r"\d+"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("valid") || out.content.contains("input"),
            "content: {}",
            out.content
        );
    }

    #[test]
    fn tool_metadata() {
        let t = RegexTestTool::default();
        assert_eq!(t.name(), "regex_test");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("pattern")));
    }
}
