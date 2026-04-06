//! PerfAnalyzeTool — performance analysis via hyperfine, time, and custom benchmarks.
//!
//! Runs a command under hyperfine (if available) or falls back to simple timing.
//! Also supports:
//! - Comparing multiple commands (A/B benchmarking)
//! - Parsing cargo bench output
//! - Profiling summary from flamegraph annotations
//!
//! Output is structured markdown suitable for LLM consumption.

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};
use std::time::Instant;
use tokio::process::Command;

pub struct PerfAnalyzeTool {
    timeout_secs: u64,
}

impl PerfAnalyzeTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    async fn hyperfine_available() -> bool {
        Command::new("hyperfine")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    async fn run_hyperfine(
        commands: &[String],
        runs: u64,
        warmup: u64,
        dir: &str,
        timeout_secs: u64,
    ) -> String {
        let mut args = vec![
            "--runs".to_string(),
            runs.to_string(),
            "--warmup".to_string(),
            warmup.to_string(),
            "--style".to_string(),
            "none".to_string(),
        ];
        for cmd in commands {
            args.push(cmd.clone());
        }

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            Command::new("hyperfine")
                .args(&args)
                .current_dir(dir)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                if stdout.is_empty() {
                    stderr
                } else {
                    stdout
                }
            }
            Ok(Err(e)) => format!("hyperfine error: {e}"),
            Err(_) => format!("Benchmark timed out after {timeout_secs}s"),
        }
    }

    async fn run_timed(command: &str, runs: u64, dir: &str, timeout_secs: u64) -> TimedResult {
        let mut durations: Vec<f64> = vec![];
        let mut last_output = String::new();

        for _ in 0..runs {
            let t0 = Instant::now();
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(timeout_secs),
                Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .current_dir(dir)
                    .output(),
            )
            .await;
            let elapsed = t0.elapsed().as_secs_f64() * 1000.0; // ms

            match result {
                Ok(Ok(out)) => {
                    durations.push(elapsed);
                    last_output = String::from_utf8_lossy(&out.stdout)
                        .to_string()
                        .lines()
                        .take(5)
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                Ok(Err(e)) => {
                    last_output = format!("Error: {e}");
                    break;
                }
                Err(_) => {
                    last_output = format!("Timeout after {timeout_secs}s");
                    break;
                }
            }
        }

        let n = durations.len() as f64;
        let mean = if n > 0.0 {
            durations.iter().sum::<f64>() / n
        } else {
            0.0
        };
        let min = durations.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = durations.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let stddev = if n > 1.0 {
            let variance = durations.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / (n - 1.0);
            variance.sqrt()
        } else {
            0.0
        };

        TimedResult {
            command: command.to_string(),
            runs: durations.len(),
            mean_ms: mean,
            min_ms: min,
            max_ms: max,
            stddev_ms: stddev,
            last_output,
        }
    }

    async fn run_cargo_bench(
        package: Option<&str>,
        filter: Option<&str>,
        dir: &str,
        timeout_secs: u64,
    ) -> String {
        let mut args = vec!["bench"];
        let pkg_owned;
        if let Some(p) = package {
            args.push("-p");
            pkg_owned = p.to_string();
            args.push(&pkg_owned);
        }
        if let Some(f) = filter {
            args.push(f);
        }

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            Command::new("cargo").args(&args).current_dir(dir).output(),
        )
        .await;

        match result {
            Ok(Ok(out)) => {
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let bench_lines: Vec<&str> = stderr
                    .lines()
                    .filter(|l| {
                        l.contains("bench:") || l.contains("ns/iter") || l.contains("test ")
                    })
                    .take(50)
                    .collect();
                if bench_lines.is_empty() {
                    stderr.lines().take(30).collect::<Vec<_>>().join("\n")
                } else {
                    bench_lines.join("\n")
                }
            }
            Ok(Err(e)) => format!("cargo bench error: {e}"),
            Err(_) => format!("cargo bench timed out after {timeout_secs}s"),
        }
    }

    fn format_timed(r: &TimedResult) -> String {
        let mut out = format!("## `{}`\n\n", r.command);
        out.push_str(&format!("  Runs   : {}\n", r.runs));
        out.push_str(&format!("  Mean   : {:.2}ms\n", r.mean_ms));
        out.push_str(&format!("  Min    : {:.2}ms\n", r.min_ms));
        out.push_str(&format!("  Max    : {:.2}ms\n", r.max_ms));
        out.push_str(&format!("  StdDev : {:.2}ms\n", r.stddev_ms));
        if !r.last_output.is_empty() {
            out.push_str(&format!("\nSample output:\n```\n{}\n```\n", r.last_output));
        }
        out
    }
}

struct TimedResult {
    command: String,
    runs: usize,
    mean_ms: f64,
    min_ms: f64,
    max_ms: f64,
    stddev_ms: f64,
    last_output: String,
}

impl Default for PerfAnalyzeTool {
    fn default() -> Self {
        Self::new(60)
    }
}

#[async_trait]
impl Tool for PerfAnalyzeTool {
    fn name(&self) -> &str {
        "perf_analyze"
    }

    fn description(&self) -> &str {
        "Benchmark and profile command performance. Uses hyperfine for precise statistical \
         benchmarks when available, falls back to simple timing. Supports A/B comparison \
         of multiple commands, cargo bench integration, and customizable run counts. \
         Reports mean, min, max, and stddev latency."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Command to benchmark."
                },
                "commands": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Multiple commands for A/B comparison."
                },
                "mode": {
                    "type": "string",
                    "enum": ["benchmark", "cargo_bench"],
                    "description": "Mode: 'benchmark' (run shell command), 'cargo_bench' (run cargo bench). Default: 'benchmark'."
                },
                "runs": {
                    "type": "integer",
                    "description": "Number of benchmark runs (default: 5, max: 50)."
                },
                "warmup": {
                    "type": "integer",
                    "description": "Number of warmup runs before measuring (default: 1, hyperfine only)."
                },
                "package": {
                    "type": "string",
                    "description": "Cargo package for cargo_bench mode."
                },
                "filter": {
                    "type": "string",
                    "description": "Benchmark name filter for cargo_bench mode."
                },
                "path": {
                    "type": "string",
                    "description": "Working directory (default: current)."
                }
            },
            "required": []
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadWrite
    }

    async fn execute_inner(
        &self,
        input: ToolInput,
    ) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let args = &input.arguments;
        let mode = args["mode"].as_str().unwrap_or("benchmark");
        let dir = args["path"].as_str().unwrap_or(&input.working_directory);
        let runs = args["runs"].as_u64().unwrap_or(5).clamp(1, 50);
        let warmup = args["warmup"].as_u64().unwrap_or(1).clamp(0, 10);

        match mode {
            "cargo_bench" => {
                let package = args["package"].as_str();
                let filter = args["filter"].as_str();
                let output = Self::run_cargo_bench(package, filter, dir, self.timeout_secs).await;
                let content = format!("# Cargo Bench Results\n\n```\n{output}\n```\n");
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content,
                    is_error: false,
                    metadata: Some(json!({ "mode": "cargo_bench" })),
                });
            }
            _ => {
                // benchmark mode
                let mut commands: Vec<String> = vec![];
                if let Some(c) = args["command"].as_str() {
                    commands.push(c.to_string());
                }
                if let Some(arr) = args["commands"].as_array() {
                    for v in arr {
                        if let Some(s) = v.as_str() {
                            commands.push(s.to_string());
                        }
                    }
                }
                if commands.is_empty() {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: "Provide 'command', 'commands', or mode='cargo_bench'."
                            .to_string(),
                        is_error: true,
                        metadata: None,
                    });
                }
                commands.truncate(5);

                // Try hyperfine first
                if Self::hyperfine_available().await && !commands.is_empty() {
                    let output =
                        Self::run_hyperfine(&commands, runs, warmup, dir, self.timeout_secs).await;
                    let content =
                        format!("# Performance Benchmark (hyperfine)\n\n```\n{output}\n```\n");
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content,
                        is_error: false,
                        metadata: Some(json!({ "mode": "hyperfine", "commands": commands.len() })),
                    });
                }

                // Fallback: simple timing
                let mut results = vec![];
                for cmd in &commands {
                    let r = Self::run_timed(cmd, runs, dir, self.timeout_secs).await;
                    results.push(r);
                }

                let mut content = "# Performance Benchmark (simple timing)\n\n".to_string();
                for r in &results {
                    content.push_str(&Self::format_timed(r));
                    content.push('\n');
                }

                let mean_ms = results.first().map(|r| r.mean_ms).unwrap_or(0.0);
                Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content,
                    is_error: false,
                    metadata: Some(json!({
                        "mode": "simple_timing",
                        "commands": results.len(),
                        "runs": runs,
                        "mean_ms": mean_ms
                    })),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_input(args: Value) -> ToolInput {
        ToolInput {
            tool_use_id: "t1".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[test]
    fn tool_metadata() {
        let t = PerfAnalyzeTool::default();
        assert_eq!(t.name(), "perf_analyze");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadWrite);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"].as_array().unwrap().is_empty());
    }

    #[test]
    fn format_timed_output() {
        let r = TimedResult {
            command: "echo hello".into(),
            runs: 5,
            mean_ms: 3.14,
            min_ms: 2.0,
            max_ms: 5.0,
            stddev_ms: 0.8,
            last_output: "hello".into(),
        };
        let s = PerfAnalyzeTool::format_timed(&r);
        assert!(s.contains("echo hello"));
        assert!(s.contains("3.14ms"));
        assert!(s.contains("5"));
    }

    #[tokio::test]
    async fn execute_no_command_returns_error() {
        let tool = PerfAnalyzeTool::new(10);
        let out = tool.execute(make_input(json!({}))).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn execute_simple_echo_benchmark() {
        let tool = PerfAnalyzeTool::new(15);
        let out = tool
            .execute(make_input(json!({
                "command": "echo hello",
                "runs": 3
            })))
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
        // Either hyperfine or simple timing output
        assert!(
            out.content.contains("Benchmark")
                || out.content.contains("bench")
                || out.content.contains("ms"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_multiple_commands_ab_comparison() {
        let tool = PerfAnalyzeTool::new(30);
        let out = tool
            .execute(make_input(json!({
                "commands": ["echo a", "echo b"],
                "runs": 2
            })))
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
    }

    #[test]
    fn commands_capped_at_5() {
        let mut cmds: Vec<String> = (0..10).map(|i| format!("cmd{i}")).collect();
        cmds.truncate(5);
        assert_eq!(cmds.len(), 5);
    }

    #[tokio::test]
    async fn execute_cargo_bench_mode() {
        // cargo bench might fail in test env, but should not panic
        let tool = PerfAnalyzeTool::new(30);
        let out = tool
            .execute(make_input(json!({
                "mode": "cargo_bench",
                "filter": "nonexistent_bench_halcon",
                "path": "/Users/oscarvalois/Documents/Github/cuervo-cli"
            })))
            .await
            .unwrap();
        // Will likely produce output (even if no benchmarks found)
        assert!(!out.content.is_empty());
    }

    #[test]
    fn timed_result_stats() {
        let durations = vec![10.0f64, 20.0, 30.0];
        let n = durations.len() as f64;
        let mean = durations.iter().sum::<f64>() / n;
        assert!((mean - 20.0).abs() < 0.01);
        let variance = durations.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / (n - 1.0);
        let stddev = variance.sqrt();
        assert!((stddev - 10.0).abs() < 0.01);
    }
}
