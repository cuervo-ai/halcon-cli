use std::io::{self, Write};

use cuervo_core::types::ToolOutput;

use super::color;
use super::theme;

const MAX_RESULT_LINES: usize = 50;

/// Format a duration in milliseconds for display.
fn format_duration(duration_ms: u64) -> String {
    if duration_ms < 1000 {
        format!("{duration_ms}ms")
    } else {
        format!("{:.1}s", duration_ms as f64 / 1000.0)
    }
}

/// Render the start of a tool invocation.
pub fn render_tool_start(name: &str, args: &serde_json::Value) {
    let summary = summarize_tool_args(name, args);
    let t = theme::active();
    let r = theme::reset();
    let tl = color::box_top_left();
    let h = color::box_horiz();
    let muted = t.palette.muted.fg();
    let accent = t.palette.accent.fg();
    let mut out = io::stderr().lock();
    let _ = writeln!(out, "\n  {muted}{tl}{h}{r} {accent}{name}{r}({summary})");
    let _ = out.flush();
}

/// Render a tool execution result with duration.
#[allow(dead_code)]
pub fn render_tool_result(output: &ToolOutput, duration_ms: u64) {
    let t = theme::active();
    let r = theme::reset();
    let bl = color::box_bottom_left();
    let h = color::box_horiz();
    let v = color::box_vert();
    let muted = t.palette.muted.fg();
    let dur = format_duration(duration_ms);
    let dur_color = t.palette.text_dim.fg();

    let status_str = if output.is_error {
        let c = t.palette.error.fg();
        format!("{c}ERROR{r}")
    } else {
        let c = t.palette.success.fg();
        format!("{c}OK{r}")
    };

    let mut out = io::stderr().lock();
    let _ = write!(out, "  {muted}{bl}{h}{r} [{status_str} {dur_color}{dur}{r}] ");

    // Truncate long output.
    let lines: Vec<&str> = output.content.lines().collect();
    if lines.len() <= MAX_RESULT_LINES {
        let _ = writeln!(out, "{}", output.content);
    } else {
        for line in &lines[..MAX_RESULT_LINES] {
            let _ = writeln!(out, "  {muted}{v}{r} {line}");
        }
        let _ = writeln!(
            out,
            "  {muted}{v}{r} ... ({} more lines)",
            lines.len() - MAX_RESULT_LINES
        );
    }
    let _ = out.flush();
}

/// Render a tool execution error with duration.
#[allow(dead_code)]
pub fn render_tool_error(name: &str, error: &str, duration_ms: u64) {
    let t = theme::active();
    let r = theme::reset();
    let bl = color::box_bottom_left();
    let h = color::box_horiz();
    let muted = t.palette.muted.fg();
    let error_color = t.palette.error.fg();
    let dur_color = t.palette.text_dim.fg();
    let dur = format_duration(duration_ms);
    let mut out = io::stderr().lock();
    let _ = writeln!(
        out,
        "  {muted}{bl}{h}{r} [{error_color}ERROR{r} {dur_color}{dur}{r}] {name}: {error}",
    );
    let _ = out.flush();
}

/// Render when a tool is denied by the user.
pub fn render_tool_denied(name: &str) {
    let t = theme::active();
    let r = theme::reset();
    let bl = color::box_bottom_left();
    let h = color::box_horiz();
    let muted = t.palette.muted.fg();
    let warn = t.palette.warning.fg();
    let mut out = io::stderr().lock();
    let _ = writeln!(out, "  {muted}{bl}{h}{r} [{warn}DENIED{r}] {name}");
    let _ = out.flush();
}

/// Render tool output from a ContentBlock with duration (used by the executor).
pub fn render_tool_output(block: &cuervo_core::types::ContentBlock, duration_ms: u64) {
    if let cuervo_core::types::ContentBlock::ToolResult {
        content, is_error, ..
    } = block
    {
        let t = theme::active();
        let r = theme::reset();
        let bl = color::box_bottom_left();
        let h = color::box_horiz();
        let v = color::box_vert();
        let muted = t.palette.muted.fg();
        let dur_color = t.palette.text_dim.fg();
        let dur = format_duration(duration_ms);

        let status_str = if *is_error {
            let c = t.palette.error.fg();
            format!("{c}ERROR{r}")
        } else {
            let c = t.palette.success.fg();
            format!("{c}OK{r}")
        };

        let mut out = io::stderr().lock();
        let _ = write!(out, "  {muted}{bl}{h}{r} [{status_str} {dur_color}{dur}{r}] ");
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() <= MAX_RESULT_LINES {
            let _ = writeln!(out, "{content}");
        } else {
            for line in &lines[..MAX_RESULT_LINES] {
                let _ = writeln!(out, "  {muted}{v}{r} {line}");
            }
            let _ = writeln!(
                out,
                "  {muted}{v}{r} ... ({} more lines)",
                lines.len() - MAX_RESULT_LINES
            );
        }
        let _ = out.flush();
    }
}

/// Create a human-readable summary of tool arguments for display.
fn summarize_tool_args(name: &str, args: &serde_json::Value) -> String {
    match name {
        "file_read" | "file_write" | "file_edit" => {
            args["path"].as_str().unwrap_or("?").to_string()
        }
        "bash" => {
            let cmd = args["command"].as_str().unwrap_or("?");
            if cmd.len() > 50 {
                format!("{}...", &cmd[..47])
            } else {
                cmd.to_string()
            }
        }
        "glob" => args["pattern"].as_str().unwrap_or("?").to_string(),
        "grep" => {
            let pattern = args["pattern"].as_str().unwrap_or("?");
            let glob = args["glob"].as_str().unwrap_or("**/*");
            format!("/{pattern}/ in {glob}")
        }
        _ => {
            let s = serde_json::to_string(args).unwrap_or_default();
            if s.len() > 60 {
                format!("{}...", &s[..57])
            } else {
                s
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summarize_file_read() {
        assert_eq!(
            summarize_tool_args("file_read", &json!({"path": "src/main.rs"})),
            "src/main.rs"
        );
    }

    #[test]
    fn summarize_bash() {
        assert_eq!(
            summarize_tool_args("bash", &json!({"command": "echo hello"})),
            "echo hello"
        );
    }

    #[test]
    fn summarize_bash_truncates() {
        let long = "a".repeat(100);
        let summary = summarize_tool_args("bash", &json!({"command": long}));
        assert!(summary.len() <= 53);
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn summarize_grep() {
        assert_eq!(
            summarize_tool_args("grep", &json!({"pattern": "fn main", "glob": "*.rs"})),
            "/fn main/ in *.rs"
        );
    }

    #[test]
    fn summarize_glob() {
        assert_eq!(
            summarize_tool_args("glob", &json!({"pattern": "**/*.rs"})),
            "**/*.rs"
        );
    }

    #[test]
    fn render_tool_start_does_not_panic() {
        render_tool_start("file_read", &json!({"path": "test.rs"}));
    }

    #[test]
    fn render_tool_result_does_not_panic() {
        let output = ToolOutput {
            tool_use_id: "test".into(),
            content: "file contents here".into(),
            is_error: false,
            metadata: None,
        };
        render_tool_result(&output, 42);
    }

    #[test]
    fn render_tool_error_does_not_panic() {
        render_tool_error("bash", "command failed", 100);
    }

    #[test]
    fn format_duration_sub_second() {
        assert_eq!(format_duration(42), "42ms");
        assert_eq!(format_duration(999), "999ms");
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(1000), "1.0s");
        assert_eq!(format_duration(1500), "1.5s");
        assert_eq!(format_duration(12345), "12.3s");
    }

    #[test]
    fn render_tool_denied_does_not_panic() {
        render_tool_denied("bash");
    }
}
