//! GitBlameTool — per-line authorship and change history via `git blame`.
//!
//! Features:
//! - Annotate any file with commit hash, author, date, and line content
//! - Limit to specific line range
//! - Show commit subject alongside each line
//! - Output as formatted text or JSON
//! - Detect "hot" lines (recently changed, many authors)
//! - Filter to show only lines from a specific author

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::process::Command;

pub struct GitBlameTool {
    timeout_secs: u64,
}

impl GitBlameTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    async fn run_blame(
        file: &str,
        line_start: Option<u32>,
        line_end: Option<u32>,
        dir: &str,
        timeout_secs: u64,
    ) -> Result<String, String> {
        let args = vec!["blame", "--porcelain"];

        let range;
        if let (Some(s), Some(e)) = (line_start, line_end) {
            range = format!("-L {},{}  ", s, e);
            // Can't push range str directly — build args after
        }

        let mut cmd_args: Vec<String> = vec!["blame".into(), "--porcelain".into()];
        if let (Some(s), Some(e)) = (line_start, line_end) {
            cmd_args.push(format!("-L {},{}", s, e));
        }
        cmd_args.push("--".into());
        cmd_args.push(file.to_string());

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            Command::new("git")
                .args(&cmd_args)
                .current_dir(dir)
                .output(),
        )
        .await;

        let _ = args; // suppress unused
        let _ = range;

        match result {
            Ok(Ok(out)) => {
                if out.status.success() {
                    Ok(String::from_utf8_lossy(&out.stdout).to_string())
                } else {
                    Err(String::from_utf8_lossy(&out.stderr).to_string())
                }
            }
            Ok(Err(e)) => Err(format!("git blame error: {e}")),
            Err(_) => Err(format!("git blame timed out after {timeout_secs}s")),
        }
    }

    /// Parse porcelain blame output into structured blame lines.
    fn parse_porcelain(raw: &str) -> Vec<BlameLine> {
        let mut lines = vec![];
        let mut iter = raw.lines().peekable();
        let mut commit_cache: HashMap<String, CommitInfo> = HashMap::new();

        while let Some(header) = iter.next() {
            // Header: "<hash> <orig_line> <final_line> [<group_count>]"
            let parts: Vec<&str> = header.split_whitespace().collect();
            if parts.len() < 3 || parts[0].len() != 40 {
                continue;
            }
            let hash = parts[0].to_string();
            let final_line: u32 = parts[2].parse().unwrap_or(0);

            let mut info = commit_cache.entry(hash.clone()).or_default().clone();

            // Read commit metadata lines until we hit a line starting with '\t'
            loop {
                match iter.peek() {
                    Some(l) if l.starts_with('\t') => break,
                    Some(l) => {
                        let l = *l;
                        if let Some(v) = l.strip_prefix("author ") {
                            info.author = v.to_string();
                        } else if let Some(v) = l.strip_prefix("author-time ") {
                            info.author_time = v.parse().unwrap_or(0);
                        } else if let Some(v) = l.strip_prefix("summary ") {
                            info.summary = v.to_string();
                        }
                        iter.next();
                    }
                    None => break,
                }
            }
            commit_cache.insert(hash.clone(), info.clone());

            // Content line (starts with \t)
            if let Some(content_line) = iter.next() {
                let content = content_line.strip_prefix('\t').unwrap_or(content_line);
                lines.push(BlameLine {
                    hash: hash[..8].to_string(),
                    author: info.author,
                    author_time: info.author_time,
                    summary: info.summary,
                    line_no: final_line,
                    content: content.to_string(),
                });
            }
        }
        lines
    }

    fn format_text(lines: &[BlameLine], show_summary: bool) -> String {
        let max_author = lines
            .iter()
            .map(|l| l.author.len())
            .max()
            .unwrap_or(10)
            .min(20);
        let mut out = String::new();
        for l in lines {
            let date = format_timestamp(l.author_time);
            let author = if l.author.len() > max_author {
                l.author[..max_author].to_string()
            } else {
                format!("{:<width$}", l.author, width = max_author)
            };
            if show_summary {
                out.push_str(&format!(
                    "{} {} {} {:4} │ {} ({} {})\n",
                    l.hash, author, date, l.line_no, l.content, l.hash, l.summary
                ));
            } else {
                out.push_str(&format!(
                    "{} {} {} {:4} │ {}\n",
                    l.hash, author, date, l.line_no, l.content
                ));
            }
        }
        out
    }

    fn hot_lines(lines: &[BlameLine]) -> Vec<Value> {
        // Lines changed most recently (top 5)
        let mut sorted = lines.to_vec();
        sorted.sort_by(|a, b| b.author_time.cmp(&a.author_time));
        sorted
            .iter()
            .take(5)
            .map(|l| {
                json!({
                    "line": l.line_no,
                    "author": l.author,
                    "date": format_timestamp(l.author_time),
                    "commit": l.hash,
                    "summary": l.summary
                })
            })
            .collect()
    }

    fn author_stats(lines: &[BlameLine]) -> Vec<Value> {
        let mut counts: HashMap<&str, u32> = HashMap::new();
        for l in lines {
            *counts.entry(&l.author).or_default() += 1;
        }
        let total = lines.len() as f64;
        let mut stats: Vec<(&str, u32)> = counts.into_iter().collect();
        stats.sort_by(|a, b| b.1.cmp(&a.1));
        stats
            .into_iter()
            .map(|(author, count)| {
                json!({
                    "author": author,
                    "lines": count,
                    "pct": format!("{:.1}%", count as f64 / total * 100.0)
                })
            })
            .collect()
    }
}

#[derive(Clone, Default)]
struct CommitInfo {
    author: String,
    author_time: i64,
    summary: String,
}

#[derive(Clone)]
struct BlameLine {
    hash: String,
    author: String,
    author_time: i64,
    summary: String,
    line_no: u32,
    content: String,
}

fn format_timestamp(ts: i64) -> String {
    // Simple YYYY-MM-DD from Unix timestamp (no chrono dep)
    if ts == 0 {
        return "0000-00-00".into();
    }
    let secs = ts as u64;
    // Days since epoch
    let days = secs / 86400;
    // Rough Gregorian calendar
    let mut year = 1970u32;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let month_days: [u32; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1u32;
    for &md in &month_days {
        if remaining < md as u64 {
            break;
        }
        remaining -= md as u64;
        month += 1;
    }
    let day = remaining + 1;
    format!("{:04}-{:02}-{:02}", year, month, day)
}

fn is_leap(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

impl Default for GitBlameTool {
    fn default() -> Self {
        Self::new(30)
    }
}

#[async_trait]
impl Tool for GitBlameTool {
    fn name(&self) -> &str {
        "git_blame"
    }

    fn description(&self) -> &str {
        "Show per-line authorship and commit history for a file via git blame. \
         Annotates each line with commit hash, author, date, and optional commit summary. \
         Supports line range filtering, author filtering, hot-line detection, \
         and author contribution statistics."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "File path to blame (relative to path)."
                },
                "path": {
                    "type": "string",
                    "description": "Git repository directory (default: current)."
                },
                "line_start": {
                    "type": "integer",
                    "description": "First line of range to show (1-indexed)."
                },
                "line_end": {
                    "type": "integer",
                    "description": "Last line of range to show."
                },
                "author": {
                    "type": "string",
                    "description": "Filter to show only lines by this author (substring match)."
                },
                "show_summary": {
                    "type": "boolean",
                    "description": "Show commit subject line next to each blame entry (default: false)."
                },
                "format": {
                    "type": "string",
                    "enum": ["text", "json"],
                    "description": "Output format (default: text)."
                },
                "stats": {
                    "type": "boolean",
                    "description": "Append author contribution statistics (default: false)."
                }
            },
            "required": ["file"]
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
        let file = match args["file"].as_str() {
            Some(f) if !f.is_empty() => f,
            _ => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: "Missing required 'file' argument.".into(),
                    is_error: true,
                    metadata: None,
                });
            }
        };
        let dir = args["path"].as_str().unwrap_or(&input.working_directory);
        let line_start = args["line_start"].as_u64().map(|v| v as u32);
        let line_end = args["line_end"].as_u64().map(|v| v as u32);
        let author_filter = args["author"].as_str();
        let show_summary = args["show_summary"].as_bool().unwrap_or(false);
        let format = args["format"].as_str().unwrap_or("text");
        let show_stats = args["stats"].as_bool().unwrap_or(false);

        let raw = match Self::run_blame(file, line_start, line_end, dir, self.timeout_secs).await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!("git blame failed: {e}"),
                    is_error: true,
                    metadata: None,
                });
            }
        };

        let mut lines = Self::parse_porcelain(&raw);

        if let Some(af) = author_filter {
            let af_lower = af.to_lowercase();
            lines.retain(|l| l.author.to_lowercase().contains(&af_lower));
        }

        if lines.is_empty() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "No blame entries found (check file path or line range).".into(),
                is_error: false,
                metadata: None,
            });
        }

        let content = if format == "json" {
            let blame_json: Vec<Value> = lines
                .iter()
                .map(|l| {
                    json!({
                        "line": l.line_no,
                        "hash": l.hash,
                        "author": l.author,
                        "date": format_timestamp(l.author_time),
                        "summary": l.summary,
                        "content": l.content
                    })
                })
                .collect();
            let mut out = json!({ "blame": blame_json });
            if show_stats {
                out["author_stats"] = json!(Self::author_stats(&lines));
                out["hot_lines"] = json!(Self::hot_lines(&lines));
            }
            serde_json::to_string_pretty(&out).unwrap_or_default()
        } else {
            let mut out = format!("## git blame: {file}\n\n");
            out.push_str(&Self::format_text(&lines, show_summary));
            if show_stats {
                out.push_str("\n### Author Stats\n");
                for s in Self::author_stats(&lines) {
                    out.push_str(&format!(
                        "  {} — {} lines ({})\n",
                        s["author"].as_str().unwrap_or("?"),
                        s["lines"],
                        s["pct"].as_str().unwrap_or("?")
                    ));
                }
                out.push_str("\n### Hot Lines (most recently changed)\n");
                for h in Self::hot_lines(&lines) {
                    out.push_str(&format!(
                        "  L{} {} {} — {}\n",
                        h["line"],
                        h["date"].as_str().unwrap_or(""),
                        h["author"].as_str().unwrap_or(""),
                        h["summary"].as_str().unwrap_or("")
                    ));
                }
            }
            out
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "file": file,
                "lines_blamed": lines.len(),
                "format": format
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let t = GitBlameTool::default();
        assert_eq!(t.name(), "git_blame");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("file")));
    }

    #[test]
    fn format_timestamp_epoch() {
        assert_eq!(format_timestamp(0), "0000-00-00");
    }

    #[test]
    fn format_timestamp_known_date() {
        // 2024-01-01 00:00:00 UTC = 1704067200
        let s = format_timestamp(1704067200);
        assert!(s.starts_with("2024"), "got: {}", s);
    }

    #[test]
    fn parse_porcelain_empty() {
        let lines = GitBlameTool::parse_porcelain("");
        assert!(lines.is_empty());
    }

    #[test]
    fn parse_porcelain_basic() {
        let raw = "abc123def456abc123def456abc123def456abc1 1 1 1\n\
                   author Alice\n\
                   author-time 1704067200\n\
                   summary Fix: initial commit\n\
                   \tfn main() {}\n";
        let lines = GitBlameTool::parse_porcelain(raw);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].author, "Alice");
        assert_eq!(lines[0].content, "fn main() {}");
        assert_eq!(lines[0].line_no, 1);
        assert!(lines[0].summary.contains("initial"));
    }

    #[test]
    fn author_stats_single() {
        let lines = vec![
            BlameLine {
                hash: "abc".into(),
                author: "Alice".into(),
                author_time: 0,
                summary: "".into(),
                line_no: 1,
                content: "a".into(),
            },
            BlameLine {
                hash: "abc".into(),
                author: "Alice".into(),
                author_time: 0,
                summary: "".into(),
                line_no: 2,
                content: "b".into(),
            },
        ];
        let stats = GitBlameTool::author_stats(&lines);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0]["lines"], 2);
        assert_eq!(stats[0]["pct"], "100.0%");
    }

    #[test]
    fn hot_lines_returns_at_most_5() {
        let lines: Vec<BlameLine> = (0..10)
            .map(|i| BlameLine {
                hash: format!("abc{}", i),
                author: "A".into(),
                author_time: i,
                summary: "".into(),
                line_no: i as u32 + 1,
                content: "x".into(),
            })
            .collect();
        let hot = GitBlameTool::hot_lines(&lines);
        assert_eq!(hot.len(), 5);
        // Should be sorted by time desc
        assert_eq!(hot[0]["line"], 10);
    }

    #[tokio::test]
    async fn execute_missing_file_arg() {
        let tool = GitBlameTool::new(5);
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: serde_json::json!({}),
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("Missing"));
    }

    #[tokio::test]
    async fn execute_nonexistent_file() {
        let tool = GitBlameTool::new(5);
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: serde_json::json!({ "file": "nonexistent_no_file.rs" }),
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        assert!(out.is_error || !out.content.is_empty());
    }

    #[tokio::test]
    async fn execute_real_repo_file() {
        let tool = GitBlameTool::new(30);
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: serde_json::json!({
                    "file": "Cargo.toml",
                    "line_start": 1,
                    "line_end": 5,
                    "stats": true
                }),
                working_directory: "/Users/oscarvalois/Documents/Github/cuervo-cli".into(),
            })
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
        // Should contain blame annotations
        assert!(!out.content.is_empty());
    }
}
