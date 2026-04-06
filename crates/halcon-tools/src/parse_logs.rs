//! ParseLogsTool — parse and analyze structured and unstructured log files.
//!
//! Reads log files and provides:
//! - Auto-detection of format (JSON lines, logfmt, syslog, nginx/apache access logs, plain text)
//! - Filtering by log level, time range, or search pattern
//! - Error/warning extraction
//! - Statistics: log level counts, top error messages, request rates
//! - Tail/head operations
//!
//! Supports common formats:
//! - JSON lines (`{"level":"info","msg":"...","ts":"..."}`)
//! - Logfmt (`level=info msg="..." ts="..."`)
//! - Syslog (`Feb 20 10:00:00 host daemon[pid]: msg`)
//! - Nginx/Apache access logs
//! - Generic line-based text with timestamp detection

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};

/// Maximum lines to read from a log file without explicit limits.
const DEFAULT_MAX_LINES: usize = 1000;
/// Maximum file size to read (64MB).
const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
enum LogFormat {
    JsonLines,
    Logfmt,
    Syslog,
    NginxAccess,
    #[allow(dead_code)]
    ApacheAccess,
    Plain,
}

#[derive(Debug, Clone)]
struct LogEntry {
    raw: String,
    level: Option<String>,
    message: Option<String>,
    timestamp: Option<String>,
}

pub struct ParseLogsTool;

impl ParseLogsTool {
    pub fn new() -> Self {
        Self
    }

    fn detect_format(sample: &str) -> LogFormat {
        let lines: Vec<&str> = sample.lines().take(5).collect();
        let json_count = lines
            .iter()
            .filter(|l| l.trim_start().starts_with('{'))
            .count();
        if json_count >= 2 {
            return LogFormat::JsonLines;
        }

        let logfmt_count = lines
            .iter()
            .filter(|l| l.contains("level=") || l.contains("msg=") || l.contains("ts="))
            .count();
        if logfmt_count >= 2 {
            return LogFormat::Logfmt;
        }

        // Nginx: "IP - - [date] \"METHOD /path HTTP/1.1\" 200 1234"
        let nginx_count = lines
            .iter()
            .filter(|l| {
                let l = l.trim();
                // IP address pattern at start + typical nginx access log structure
                l.contains("\" ")
                    && l.contains('[')
                    && l.contains(']')
                    && (l.contains("GET ")
                        || l.contains("POST ")
                        || l.contains("PUT ")
                        || l.contains("DELETE "))
            })
            .count();
        if nginx_count >= 2 {
            return LogFormat::NginxAccess;
        }

        // Syslog: "Feb 20 10:00:00 hostname process[pid]: message"
        let syslog_count = lines
            .iter()
            .filter(|l| {
                let parts: Vec<&str> = l.splitn(4, ' ').collect();
                parts.len() >= 3 && parts[2].contains(':')
            })
            .count();
        if syslog_count >= 3 {
            return LogFormat::Syslog;
        }

        LogFormat::Plain
    }

    fn parse_json_line(line: &str) -> LogEntry {
        let mut entry = LogEntry {
            raw: line.to_string(),
            level: None,
            message: None,
            timestamp: None,
        };

        if let Ok(v) = serde_json::from_str::<Value>(line) {
            // Common level field names
            entry.level = ["level", "severity", "lvl", "log_level"]
                .iter()
                .find_map(|&k| v[k].as_str().map(|s| s.to_uppercase()));

            // Common message field names
            entry.message = ["msg", "message", "text", "log", "body"]
                .iter()
                .find_map(|&k| v[k].as_str().map(|s| s.to_string()));

            // Common timestamp field names
            entry.timestamp = ["ts", "timestamp", "time", "@timestamp", "created_at"]
                .iter()
                .find_map(|&k| {
                    v[k].as_str()
                        .map(|s| s.to_string())
                        .or_else(|| v[k].as_f64().map(|f| f.to_string()))
                });
        }

        entry
    }

    fn parse_logfmt_line(line: &str) -> LogEntry {
        let mut entry = LogEntry {
            raw: line.to_string(),
            level: None,
            message: None,
            timestamp: None,
        };

        // Simple logfmt key=value parser
        let mut remaining = line;
        while !remaining.is_empty() {
            let (key, rest) = match remaining.find('=') {
                Some(i) => (&remaining[..i], &remaining[i + 1..]),
                None => break,
            };
            let key = key.trim();

            let (value, next) = if let Some(stripped) = rest.strip_prefix('"') {
                // Quoted value
                match stripped.find('"') {
                    Some(end) => (&stripped[..end], &stripped[end + 1..]),
                    None => (rest, ""),
                }
            } else {
                match rest.find(' ') {
                    Some(i) => (&rest[..i], &rest[i + 1..]),
                    None => (rest, ""),
                }
            };

            match key {
                "level" | "lvl" => entry.level = Some(value.to_uppercase()),
                "msg" | "message" => entry.message = Some(value.to_string()),
                "ts" | "time" | "timestamp" => entry.timestamp = Some(value.to_string()),
                _ => {}
            }
            remaining = next;
        }

        entry
    }

    fn parse_plain_line(line: &str) -> LogEntry {
        // Try to detect log level in plain text lines
        let level = detect_level_in_text(line);
        LogEntry {
            raw: line.to_string(),
            level,
            message: Some(line.to_string()),
            timestamp: None,
        }
    }

    fn parse_entries(content: &str, format: &LogFormat, max_lines: usize) -> Vec<LogEntry> {
        content
            .lines()
            .take(max_lines)
            .map(|line| match format {
                LogFormat::JsonLines => Self::parse_json_line(line),
                LogFormat::Logfmt => Self::parse_logfmt_line(line),
                _ => Self::parse_plain_line(line),
            })
            .collect()
    }

    fn filter_entries<'a>(
        entries: &'a [LogEntry],
        level_filter: Option<&str>,
        pattern: Option<&str>,
    ) -> Vec<&'a LogEntry> {
        entries
            .iter()
            .filter(|e| {
                if let Some(lvl) = level_filter {
                    let lvl = lvl.to_uppercase();
                    if !e.level.as_deref().unwrap_or("").contains(&lvl) {
                        return false;
                    }
                }
                if let Some(pat) = pattern {
                    if !e.raw.to_lowercase().contains(&pat.to_lowercase()) {
                        return false;
                    }
                }
                true
            })
            .collect()
    }

    fn compute_stats(entries: &[LogEntry]) -> LogStats {
        let mut level_counts: HashMap<String, usize> = HashMap::new();
        let mut error_messages: HashMap<String, usize> = HashMap::new();

        for entry in entries {
            let level = entry.level.clone().unwrap_or_else(|| "UNKNOWN".to_string());
            *level_counts.entry(level.clone()).or_insert(0) += 1;

            if level == "ERROR" || level == "CRITICAL" || level == "FATAL" {
                if let Some(ref msg) = entry.message {
                    let short = msg.chars().take(100).collect::<String>();
                    *error_messages.entry(short).or_insert(0) += 1;
                }
            }
        }

        let mut top_errors: Vec<(String, usize)> = error_messages.into_iter().collect();
        top_errors.sort_by(|a, b| b.1.cmp(&a.1));
        top_errors.truncate(10);

        LogStats {
            level_counts,
            top_errors,
        }
    }
}

impl Default for ParseLogsTool {
    fn default() -> Self {
        Self::new()
    }
}

struct LogStats {
    level_counts: HashMap<String, usize>,
    top_errors: Vec<(String, usize)>,
}

fn detect_level_in_text(line: &str) -> Option<String> {
    let upper = line.to_uppercase();
    for level in &[
        "CRITICAL", "FATAL", "ERROR", "WARN", "WARNING", "INFO", "DEBUG", "TRACE",
    ] {
        if upper.contains(level) {
            return Some((*level).to_string());
        }
    }
    None
}

#[async_trait]
impl Tool for ParseLogsTool {
    fn name(&self) -> &str {
        "parse_logs"
    }

    fn description(&self) -> &str {
        "Parse and analyze log files. Auto-detects format (JSON lines, logfmt, syslog, nginx/apache, plain text). \
         Supports filtering by log level (ERROR, WARN, INFO, DEBUG), text pattern search, \
         and operations: tail (last N lines), head (first N lines), errors (only error-level entries), \
         stats (level counts + top errors). \
         Useful for debugging application issues and understanding log patterns."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to log file to analyze."
                },
                "action": {
                    "type": "string",
                    "enum": ["tail", "head", "errors", "stats", "search", "full"],
                    "description": "Action: 'tail' (last N lines), 'head' (first N lines), 'errors' (only ERROR level), 'stats' (level statistics), 'search' (filter by pattern), 'full' (read all with stats). Default: 'tail'."
                },
                "lines": {
                    "type": "integer",
                    "description": "Number of lines for tail/head (default: 50, max: 1000)."
                },
                "level": {
                    "type": "string",
                    "enum": ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"],
                    "description": "Filter by minimum log level."
                },
                "pattern": {
                    "type": "string",
                    "description": "Text pattern to search for (case-insensitive)."
                },
                "format": {
                    "type": "string",
                    "enum": ["auto", "json", "logfmt", "syslog", "plain"],
                    "description": "Log format. 'auto' (default) detects automatically."
                }
            },
            "required": ["path"]
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
        let working_dir = PathBuf::from(&input.working_directory);

        let log_path = match args["path"].as_str() {
            Some(p) => {
                let p = Path::new(p);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    working_dir.join(p)
                }
            }
            None => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: "'path' is required".to_string(),
                    is_error: true,
                    metadata: None,
                });
            }
        };

        if !log_path.exists() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("File not found: {}", log_path.display()),
                is_error: true,
                metadata: None,
            });
        }

        // Check file size
        if let Ok(meta) = std::fs::metadata(&log_path) {
            if meta.len() > MAX_FILE_SIZE {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!(
                        "File too large ({} MB). Maximum is {} MB. Use 'tail' or 'head' with a smaller lines count.",
                        meta.len() / 1024 / 1024,
                        MAX_FILE_SIZE / 1024 / 1024
                    ),
                    is_error: true,
                    metadata: None,
                });
            }
        }

        let content = match std::fs::read_to_string(&log_path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!("Failed to read {}: {}", log_path.display(), e),
                    is_error: true,
                    metadata: None,
                });
            }
        };

        let action = args["action"].as_str().unwrap_or("tail");
        let lines_n = args["lines"]
            .as_u64()
            .map(|n| (n as usize).min(1000))
            .unwrap_or(50);
        let level_filter = args["level"].as_str();
        let pattern = args["pattern"].as_str();

        let format_override = args["format"].as_str().filter(|&s| s != "auto");
        let format = match format_override {
            Some("json") => LogFormat::JsonLines,
            Some("logfmt") => LogFormat::Logfmt,
            Some("syslog") => LogFormat::Syslog,
            Some("plain") => LogFormat::Plain,
            _ => Self::detect_format(&content),
        };

        let total_lines = content.lines().count();

        let output = match action {
            "tail" => {
                let start = total_lines.saturating_sub(lines_n);
                let tail: Vec<&str> = content.lines().skip(start).collect();
                format!(
                    "Last {} lines of {} (format: {:?}):\n\n{}",
                    tail.len(),
                    log_path.display(),
                    format,
                    tail.join("\n")
                )
            }
            "head" => {
                let head: Vec<&str> = content.lines().take(lines_n).collect();
                format!(
                    "First {} lines of {} (format: {:?}):\n\n{}",
                    head.len(),
                    log_path.display(),
                    format,
                    head.join("\n")
                )
            }
            "errors" => {
                let entries = Self::parse_entries(&content, &format, DEFAULT_MAX_LINES);
                let errors = Self::filter_entries(&entries, Some("ERROR"), pattern);
                if errors.is_empty() {
                    format!(
                        "No ERROR-level entries found in {} ({} total lines)",
                        log_path.display(),
                        total_lines
                    )
                } else {
                    let lines_out: Vec<&str> = errors.iter().map(|e| e.raw.as_str()).collect();
                    format!(
                        "{} error(s) in {} ({} total lines):\n\n{}",
                        errors.len(),
                        log_path.display(),
                        total_lines,
                        lines_out.join("\n")
                    )
                }
            }
            "search" => {
                if pattern.is_none() {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: "'pattern' is required for search action".to_string(),
                        is_error: true,
                        metadata: None,
                    });
                }
                let entries = Self::parse_entries(&content, &format, DEFAULT_MAX_LINES);
                let matches = Self::filter_entries(&entries, level_filter, pattern);
                let lines_out: Vec<&str> = matches.iter().map(|e| e.raw.as_str()).collect();
                format!(
                    "{} match(es) for {:?}:\n\n{}",
                    matches.len(),
                    pattern.unwrap_or(""),
                    lines_out.join("\n")
                )
            }
            _ => {
                let entries = Self::parse_entries(&content, &format, DEFAULT_MAX_LINES);
                let stats = Self::compute_stats(&entries);

                let mut out = format!(
                    "Log file: {}\nFormat: {:?}\nTotal lines: {}\nParsed: {} entries\n\n",
                    log_path.display(),
                    format,
                    total_lines,
                    entries.len()
                );

                out.push_str("Level distribution:\n");
                let mut levels: Vec<(&String, &usize)> = stats.level_counts.iter().collect();
                levels.sort_by(|a, b| b.1.cmp(a.1));
                for (level, count) in &levels {
                    let bar = "█".repeat((*count * 20 / entries.len().max(1)).min(20));
                    out.push_str(&format!("  {:10} {:5} {}\n", level, count, bar));
                }

                if !stats.top_errors.is_empty() {
                    out.push_str("\nTop errors:\n");
                    for (msg, count) in &stats.top_errors {
                        out.push_str(&format!("  [{}x] {}\n", count, &msg[..msg.len().min(80)]));
                    }
                }

                if action == "full" {
                    let filtered = Self::filter_entries(&entries, level_filter, pattern);
                    let tail: Vec<&str> = filtered
                        .iter()
                        .rev()
                        .take(20)
                        .map(|e| e.raw.as_str())
                        .collect();
                    if !tail.is_empty() {
                        out.push_str("\nLast entries:\n");
                        for line in tail.iter().rev() {
                            out.push_str(line);
                            out.push('\n');
                        }
                    }
                }

                out
            }
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: output.chars().take(16000).collect(),
            is_error: false,
            metadata: Some(json!({
                "total_lines": total_lines,
                "format": format!("{:?}", format)
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const JSON_LOG: &str = r#"{"level":"info","msg":"Server started","ts":"2025-01-01T10:00:00Z"}
{"level":"debug","msg":"Request received","ts":"2025-01-01T10:00:01Z"}
{"level":"error","msg":"Database connection failed","ts":"2025-01-01T10:00:02Z","error":"timeout"}
{"level":"warn","msg":"High memory usage","ts":"2025-01-01T10:00:03Z"}
{"level":"error","msg":"Database connection failed","ts":"2025-01-01T10:00:04Z","error":"timeout"}
{"level":"info","msg":"Request completed","ts":"2025-01-01T10:00:05Z"}"#;

    const LOGFMT_LOG: &str = r#"ts=2025-01-01T10:00:00Z level=info msg="Starting server" port=8080
ts=2025-01-01T10:00:01Z level=error msg="Failed to connect" host=db timeout=30s
ts=2025-01-01T10:00:02Z level=warn msg="Retry attempt" count=3"#;

    const PLAIN_LOG: &str = r#"2025-01-01 10:00:00 INFO Starting application
2025-01-01 10:00:01 DEBUG Loading configuration
2025-01-01 10:00:02 ERROR Failed to connect to database
2025-01-01 10:00:03 WARN Retry limit exceeded"#;

    #[test]
    fn detect_json_format() {
        assert_eq!(ParseLogsTool::detect_format(JSON_LOG), LogFormat::JsonLines);
    }

    #[test]
    fn detect_logfmt_format() {
        assert_eq!(ParseLogsTool::detect_format(LOGFMT_LOG), LogFormat::Logfmt);
    }

    #[test]
    fn detect_plain_format() {
        assert_eq!(ParseLogsTool::detect_format(PLAIN_LOG), LogFormat::Plain);
    }

    #[test]
    fn parse_json_line_extracts_fields() {
        let entry = ParseLogsTool::parse_json_line(
            r#"{"level":"error","msg":"test error","ts":"2025-01-01"}"#,
        );
        assert_eq!(entry.level.as_deref(), Some("ERROR"));
        assert_eq!(entry.message.as_deref(), Some("test error"));
        assert_eq!(entry.timestamp.as_deref(), Some("2025-01-01"));
    }

    #[test]
    fn parse_logfmt_line_extracts_fields() {
        let entry =
            ParseLogsTool::parse_logfmt_line("level=warn msg=\"High memory\" ts=2025-01-01");
        assert_eq!(entry.level.as_deref(), Some("WARN"));
        assert!(entry.message.is_some());
    }

    #[test]
    fn parse_entries_json() {
        let entries = ParseLogsTool::parse_entries(JSON_LOG, &LogFormat::JsonLines, 100);
        assert_eq!(entries.len(), 6);
        let errors: Vec<_> = entries
            .iter()
            .filter(|e| e.level.as_deref() == Some("ERROR"))
            .collect();
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn filter_by_level() {
        let entries = ParseLogsTool::parse_entries(JSON_LOG, &LogFormat::JsonLines, 100);
        let errors = ParseLogsTool::filter_entries(&entries, Some("ERROR"), None);
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn filter_by_pattern() {
        let entries = ParseLogsTool::parse_entries(JSON_LOG, &LogFormat::JsonLines, 100);
        let matches = ParseLogsTool::filter_entries(&entries, None, Some("database"));
        assert!(matches.len() >= 2, "should find db messages");
    }

    #[test]
    fn compute_stats_counts_levels() {
        let entries = ParseLogsTool::parse_entries(JSON_LOG, &LogFormat::JsonLines, 100);
        let stats = ParseLogsTool::compute_stats(&entries);
        assert_eq!(*stats.level_counts.get("INFO").unwrap_or(&0), 2);
        assert_eq!(*stats.level_counts.get("ERROR").unwrap_or(&0), 2);
        assert!(!stats.top_errors.is_empty());
    }

    #[tokio::test]
    async fn execute_tail_action() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("app.log"), JSON_LOG).unwrap();
        let tool = ParseLogsTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({ "path": "app.log", "action": "tail", "lines": 3 }),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
        assert!(out.content.contains("Last"), "content: {}", out.content);
    }

    #[tokio::test]
    async fn execute_errors_action() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("app.log"), JSON_LOG).unwrap();
        let tool = ParseLogsTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({ "path": "app.log", "action": "errors" }),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("error") || out.content.contains("2 error"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_stats_action() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("app.log"), JSON_LOG).unwrap();
        let tool = ParseLogsTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({ "path": "app.log", "action": "stats" }),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("ERROR") || out.content.contains("Level"),
            "content: {}",
            out.content
        );
    }

    #[test]
    fn tool_metadata() {
        let t = ParseLogsTool::default();
        assert_eq!(t.name(), "parse_logs");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("path")));
    }
}
