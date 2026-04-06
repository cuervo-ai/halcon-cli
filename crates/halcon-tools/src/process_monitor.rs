//! ProcessMonitorTool — list and inspect running system processes.
//!
//! Uses `ps aux` to list processes with CPU, memory and VSZ/RSS stats.
//! Supports filtering by name or PID, sorting, and aggregate statistics.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

pub struct ProcessMonitorTool {
    #[allow(dead_code)]
    timeout_secs: u64,
}

impl ProcessMonitorTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }
}

impl Default for ProcessMonitorTool {
    fn default() -> Self {
        Self::new(30)
    }
}

// ─── ps parsing helpers ───────────────────────────────────────────────────────

#[derive(Debug)]
struct ProcessInfo {
    pid: u32,
    user: String,
    cpu_pct: f32,
    mem_pct: f32,
    vsz_kb: u64,
    rss_kb: u64,
    command: String,
}

/// Parse one line of `ps aux` output.
/// Columns: USER PID %CPU %MEM VSZ RSS TTY STAT START TIME COMMAND
fn parse_ps_line(line: &str) -> Option<ProcessInfo> {
    let mut parts = line.split_whitespace();
    let user = parts.next()?.to_string();
    let pid: u32 = parts.next()?.parse().ok()?;
    let cpu_pct: f32 = parts.next()?.parse().ok()?;
    let mem_pct: f32 = parts.next()?.parse().ok()?;
    let vsz_kb: u64 = parts.next()?.parse().ok()?;
    let rss_kb: u64 = parts.next()?.parse().ok()?;
    // Skip TTY, STAT, START, TIME (4 fields)
    for _ in 0..4 {
        parts.next()?;
    }
    let command: String = parts.collect::<Vec<_>>().join(" ");
    if command.is_empty() {
        return None;
    }
    Some(ProcessInfo {
        pid,
        user,
        cpu_pct,
        mem_pct,
        vsz_kb,
        rss_kb,
        command,
    })
}

fn run_ps(
    name_filter: Option<&str>,
    pid_filter: Option<u32>,
    sort_by: &str,
    limit: usize,
) -> std::result::Result<Vec<ProcessInfo>, String> {
    use std::process::Command;
    let output = Command::new("ps")
        .arg("aux")
        .output()
        .map_err(|e| format!("Failed to run ps: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "ps exited with status {}",
            output.status.code().unwrap_or(-1)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut procs: Vec<ProcessInfo> = stdout
        .lines()
        .skip(1) // skip header
        .filter_map(parse_ps_line)
        .filter(|p| {
            if let Some(n) = name_filter {
                if !p.command.to_lowercase().contains(&n.to_lowercase()) {
                    return false;
                }
            }
            if let Some(pid) = pid_filter {
                if p.pid != pid {
                    return false;
                }
            }
            true
        })
        .collect();

    match sort_by {
        "mem" => procs.sort_by(|a, b| {
            b.mem_pct
                .partial_cmp(&a.mem_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "pid" => procs.sort_by_key(|p| p.pid),
        "rss" => procs.sort_by(|a, b| b.rss_kb.cmp(&a.rss_kb)),
        _ => procs.sort_by(|a, b| {
            b.cpu_pct
                .partial_cmp(&a.cpu_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }

    if limit > 0 {
        procs.truncate(limit);
    }

    Ok(procs)
}

fn format_table(procs: &[ProcessInfo]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<8} {:<12} {:>6} {:>6} {:>10} {:>10}  {}\n",
        "PID", "USER", "%CPU", "%MEM", "VSZ(KB)", "RSS(KB)", "COMMAND"
    ));
    out.push_str(&"-".repeat(80));
    out.push('\n');
    for p in procs {
        let cmd = if p.command.len() > 50 {
            format!("{}...", &p.command[..47])
        } else {
            p.command.clone()
        };
        out.push_str(&format!(
            "{:<8} {:<12} {:>6.1} {:>6.1} {:>10} {:>10}  {}\n",
            p.pid, p.user, p.cpu_pct, p.mem_pct, p.vsz_kb, p.rss_kb, cmd
        ));
    }
    out
}

fn compute_totals(procs: &[ProcessInfo]) -> HashMap<String, f64> {
    let mut map = HashMap::new();
    map.insert("count".to_string(), procs.len() as f64);
    map.insert(
        "total_cpu".to_string(),
        procs.iter().map(|p| p.cpu_pct as f64).sum(),
    );
    map.insert(
        "total_mem".to_string(),
        procs.iter().map(|p| p.mem_pct as f64).sum(),
    );
    map.insert(
        "total_rss_kb".to_string(),
        procs.iter().map(|p| p.rss_kb as f64).sum(),
    );
    map
}

// ─── Tool impl ────────────────────────────────────────────────────────────────

#[async_trait]
impl Tool for ProcessMonitorTool {
    fn name(&self) -> &str {
        "process_monitor"
    }

    fn description(&self) -> &str {
        "List and inspect running system processes. Supports filtering by name or PID, \
         sorting by CPU/memory usage, and showing resource totals."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["list", "top", "find", "stats"],
                    "description": "list=all processes, top=highest resource users, find=search by name/pid, stats=aggregate totals"
                },
                "name_filter": {
                    "type": "string",
                    "description": "Filter processes whose command contains this string (case-insensitive)"
                },
                "pid": {
                    "type": "integer",
                    "description": "Show only the process with this PID"
                },
                "sort_by": {
                    "type": "string",
                    "enum": ["cpu", "mem", "pid", "rss"],
                    "description": "Sort column (default: cpu)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of processes to return (default: 20 for list, 10 for top)"
                }
            },
            "required": ["operation"]
        })
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let args = &input.arguments;
        let operation = args["operation"]
            .as_str()
            .ok_or_else(|| HalconError::InvalidInput("operation required".into()))?;

        let name_filter = args["name_filter"].as_str();
        let pid_filter = args["pid"].as_u64().map(|v| v as u32);
        let sort_by = args["sort_by"].as_str().unwrap_or("cpu");

        let default_limit: u64 = match operation {
            "top" => 10,
            "list" => 20,
            _ => 0,
        };
        let limit = args["limit"].as_u64().unwrap_or(default_limit) as usize;

        let content = match operation {
            "list" | "top" | "find" => {
                let actual_name = name_filter;

                let procs = run_ps(actual_name, pid_filter, sort_by, limit).map_err(|e| {
                    HalconError::ToolExecutionFailed {
                        tool: "process_monitor".into(),
                        message: e,
                    }
                })?;

                if procs.is_empty() {
                    "No processes found matching the criteria.".to_string()
                } else {
                    let table = format_table(&procs);
                    format!("{} process(es) found:\n\n{}", procs.len(), table)
                }
            }

            "stats" => {
                let procs = run_ps(name_filter, pid_filter, sort_by, 0).map_err(|e| {
                    HalconError::ToolExecutionFailed {
                        tool: "process_monitor".into(),
                        message: e,
                    }
                })?;

                let totals = compute_totals(&procs);
                let mut lines = vec![
                    "Process Statistics:".to_string(),
                    format!("  Total processes : {}", totals["count"] as usize),
                    format!("  Total CPU%      : {:.1}%", totals["total_cpu"]),
                    format!("  Total MEM%      : {:.1}%", totals["total_mem"]),
                    format!(
                        "  Total RSS       : {} KB ({:.1} MB)",
                        totals["total_rss_kb"] as u64,
                        totals["total_rss_kb"] / 1024.0
                    ),
                ];

                if let Some(top) = procs.first() {
                    lines.push(format!(
                        "  Top CPU process : {} (PID {}, {:.1}%)",
                        top.command, top.pid, top.cpu_pct
                    ));
                }

                lines.join("\n")
            }

            _ => {
                return Err(HalconError::InvalidInput(format!(
                    "Unknown operation: {operation}"
                )))
            }
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content,
            is_error: false,
            metadata: None,
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ps_line_valid() {
        let line = "oscarv           12345   0.1   0.5 123456  4096 s001  S+    10:00   0:01.23 /usr/bin/cargo test";
        let p = parse_ps_line(line).expect("should parse");
        assert_eq!(p.pid, 12345);
        assert_eq!(p.user, "oscarv");
        assert!((p.cpu_pct - 0.1).abs() < 0.001);
        assert!((p.mem_pct - 0.5).abs() < 0.001);
        assert_eq!(p.vsz_kb, 123456);
        assert_eq!(p.rss_kb, 4096);
        assert!(p.command.contains("cargo"));
    }

    #[test]
    fn parse_ps_line_invalid_too_short() {
        assert!(parse_ps_line("root 1 0.0").is_none());
    }

    #[test]
    fn parse_ps_line_invalid_pid() {
        let line = "root  notapid  0.0   0.0   1234  5678 ??  Ss    10:00   0:00.00 launchd";
        assert!(parse_ps_line(line).is_none());
    }

    #[test]
    fn format_table_empty() {
        let table = format_table(&[]);
        assert!(table.contains("PID"));
        assert!(table.contains("COMMAND"));
    }

    #[test]
    fn format_table_truncates_long_command() {
        let p = ProcessInfo {
            pid: 1,
            user: "root".to_string(),
            cpu_pct: 0.0,
            mem_pct: 0.1,
            vsz_kb: 1024,
            rss_kb: 512,
            command: "a".repeat(60),
        };
        let table = format_table(&[p]);
        assert!(table.contains("..."));
    }

    #[test]
    fn compute_totals_basic() {
        let procs = vec![
            ProcessInfo {
                pid: 1,
                user: "a".into(),
                cpu_pct: 1.0,
                mem_pct: 2.0,
                vsz_kb: 100,
                rss_kb: 50,
                command: "a".into(),
            },
            ProcessInfo {
                pid: 2,
                user: "b".into(),
                cpu_pct: 3.0,
                mem_pct: 4.0,
                vsz_kb: 200,
                rss_kb: 100,
                command: "b".into(),
            },
        ];
        let t = compute_totals(&procs);
        assert_eq!(t["count"] as usize, 2);
        assert!((t["total_cpu"] - 4.0).abs() < 0.001);
        assert!((t["total_mem"] - 6.0).abs() < 0.001);
        assert!((t["total_rss_kb"] - 150.0).abs() < 0.001);
    }

    #[test]
    fn tool_name_and_permission() {
        let tool = ProcessMonitorTool::new(30);
        assert_eq!(tool.name(), "process_monitor");
        assert!(matches!(tool.permission_level(), PermissionLevel::ReadOnly));
    }

    #[test]
    fn input_schema_has_required_operation() {
        let tool = ProcessMonitorTool::new(30);
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("operation")));
    }

    #[tokio::test]
    async fn execute_list_returns_output() {
        let tool = ProcessMonitorTool::new(30);
        let input = ToolInput {
            tool_use_id: "t1".into(),
            arguments: serde_json::json!({"operation": "list", "limit": 5}),
            working_directory: ".".into(),
        };
        let result = tool.execute(input).await;
        assert!(result.is_ok(), "execute should succeed: {:?}", result.err());
        let out = result.unwrap();
        assert!(!out.is_error);
    }

    #[tokio::test]
    async fn execute_stats_returns_output() {
        let tool = ProcessMonitorTool::new(30);
        let input = ToolInput {
            tool_use_id: "t2".into(),
            arguments: serde_json::json!({"operation": "stats"}),
            working_directory: ".".into(),
        };
        let out = tool.execute(input).await.unwrap();
        assert!(out.content.contains("Total processes"));
    }

    #[tokio::test]
    async fn execute_missing_operation_errors() {
        let tool = ProcessMonitorTool::new(30);
        let input = ToolInput {
            tool_use_id: "t3".into(),
            arguments: serde_json::json!({}),
            working_directory: ".".into(),
        };
        assert!(tool.execute(input).await.is_err());
    }
}
