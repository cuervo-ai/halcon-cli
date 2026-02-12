//! Agent Operating Console — rendering helpers for all console commands.
//!
//! All output functions accept `&mut impl Write` for testability
//! (write to `Vec<u8>` in tests, `stderr` in production).

use std::io::Write;

use crate::render::{components, theme};
use cuervo_core::types::OrchestratorResult;
use cuervo_storage::{CacheStats, MemoryStats, SystemMetrics, TraceStep, TraceStepType};

// --- Observability: /status, /metrics, /logs ---

/// Status info for render_status.
pub struct StatusInfo<'a> {
    pub session_id: &'a str,
    pub rounds: u32,
    pub tokens: u64,
    pub cost: f64,
    pub provider: &'a str,
    pub model: &'a str,
    pub provider_diagnostics: &'a [(String, String, usize)],
    pub registered_models: &'a [String],
}

/// Render live system status: session info, provider health, registered models.
pub fn render_status(info: &StatusInfo<'_>, out: &mut impl Write) {
    let session_id = info.session_id;
    let rounds = info.rounds;
    let tokens = info.tokens;
    let cost = info.cost;
    let provider = info.provider;
    let model = info.model;
    let provider_diagnostics = info.provider_diagnostics;
    let registered_models = info.registered_models;
    components::section_header("Live Status", out);

    let cost_str = format!("${cost:.4}");
    let tokens_str = tokens.to_string();
    let rounds_str = rounds.to_string();
    components::kv_table(
        &[
            ("Session", session_id),
            ("Provider", provider),
            ("Model", model),
            ("Rounds", &rounds_str),
            ("Tokens", &tokens_str),
            ("Cost", &cost_str),
        ],
        4,
        out,
    );

    if !provider_diagnostics.is_empty() {
        let _ = writeln!(out);
        let t = theme::active();
        let r = theme::reset();
        let dim = t.palette.text_dim.fg();
        let _ = writeln!(out, "    {dim}Provider Health:{r}");
        for (name, state, failures) in provider_diagnostics {
            let badge = match state.as_str() {
                "closed" => components::badge("OK", components::BadgeLevel::Success),
                "half_open" => components::badge("HALF", components::BadgeLevel::Warning),
                "open" => components::badge("OPEN", components::BadgeLevel::Error),
                _ => components::badge(state, components::BadgeLevel::Muted),
            };
            let _ = writeln!(out, "      {badge} {name} ({failures} failures)");
        }
    }

    if !registered_models.is_empty() {
        let _ = writeln!(out);
        let t = theme::active();
        let r = theme::reset();
        let dim = t.palette.text_dim.fg();
        let _ = writeln!(
            out,
            "    {dim}Registered:{r} {}",
            registered_models.join(", ")
        );
    }
}

/// Render aggregated metrics: per-model token/cost tables, cache stats, memory stats.
pub fn render_metrics(
    sys: &SystemMetrics,
    cache: Option<&CacheStats>,
    memory: Option<&MemoryStats>,
    out: &mut impl Write,
) {
    components::section_header("System Metrics", out);

    let inv_str = sys.total_invocations.to_string();
    let tokens_str = sys.total_tokens.to_string();
    let cost_str = format!("${:.4}", sys.total_cost_usd);
    components::kv_table(
        &[
            ("Invocations", &inv_str),
            ("Tokens", &tokens_str),
            ("Cost", &cost_str),
        ],
        4,
        out,
    );

    if !sys.models.is_empty() {
        let _ = writeln!(out);
        let t = theme::active();
        let r = theme::reset();
        let dim = t.palette.text_dim.fg();
        let _ = writeln!(out, "    {dim}Per-Model Breakdown:{r}");
        for m in &sys.models {
            let rate = format!("{:.0}%", m.success_rate * 100.0);
            let badge = if m.success_rate >= 0.95 {
                components::badge(&rate, components::BadgeLevel::Success)
            } else if m.success_rate >= 0.8 {
                components::badge(&rate, components::BadgeLevel::Warning)
            } else {
                components::badge(&rate, components::BadgeLevel::Error)
            };
            let _ = writeln!(
                out,
                "      {badge} {}/{}: {} inv, {} tok, ${:.4}, P95 {}ms",
                m.provider,
                m.model,
                m.total_invocations,
                m.total_tokens,
                m.total_cost_usd,
                m.p95_latency_ms,
            );
        }
    }

    if let Some(cs) = cache {
        let _ = writeln!(out);
        let entries_str = cs.total_entries.to_string();
        let hits_str = cs.total_hits.to_string();
        let t = theme::active();
        let r = theme::reset();
        let dim = t.palette.text_dim.fg();
        let _ = writeln!(out, "    {dim}Cache:{r}");
        let _ = writeln!(out, "      Entries: {entries_str}  Hits: {hits_str}");
    }

    if let Some(ms) = memory {
        let _ = writeln!(out);
        let t = theme::active();
        let r = theme::reset();
        let dim = t.palette.text_dim.fg();
        let _ = writeln!(out, "    {dim}Memory:{r}");
        let _ = writeln!(out, "      Total entries: {}", ms.total_entries);
        for (entry_type, count) in &ms.by_type {
            let _ = writeln!(out, "        {entry_type}: {count}");
        }
    }
}

/// Render logs/trace timeline for a session.
pub fn render_logs(
    steps: &[TraceStep],
    filter_task_id: Option<&str>,
    out: &mut impl Write,
) {
    components::section_header("Trace Timeline", out);

    if steps.is_empty() {
        let _ = writeln!(out, "    (no trace steps recorded)");
        return;
    }

    let t = theme::active();
    let r = theme::reset();
    let dim = t.palette.text_dim.fg();

    for step in steps {
        // If filtering by task_id, check data_json for it.
        if let Some(tid) = filter_task_id {
            if !step.data_json.contains(tid) {
                continue;
            }
        }

        let type_badge = step_type_badge(step.step_type);
        let time = step.timestamp.format("%H:%M:%S");
        let preview: String = step.data_json.chars().take(80).collect();
        let _ = writeln!(
            out,
            "    {dim}{time}{r} {type_badge} #{} {dim}{}ms{r} {preview}",
            step.step_index, step.duration_ms,
        );
    }
}

// --- Inspect: /inspect ---

/// Render runtime inspection.
pub fn inspect_runtime(out: &mut impl Write) {
    components::section_header("Runtime", out);

    let parallelism = std::thread::available_parallelism()
        .map(|p| p.get().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    components::kv_table(
        &[
            ("Parallelism", &parallelism),
            ("Arch", std::env::consts::ARCH),
            ("OS", std::env::consts::OS),
        ],
        4,
        out,
    );
}

/// Render memory subsystem inspection.
pub fn inspect_memory(stats: Option<&MemoryStats>, episode_count: u64, out: &mut impl Write) {
    components::section_header("Memory Subsystem", out);

    match stats {
        Some(ms) => {
            let entries_str = ms.total_entries.to_string();
            let episodes_str = episode_count.to_string();
            let types_str = ms.by_type.len().to_string();
            components::kv_table(
                &[
                    ("Total entries", &entries_str),
                    ("Episodes", &episodes_str),
                    ("Types", &types_str),
                ],
                4,
                out,
            );
        }
        None => {
            let _ = writeln!(out, "    (no database configured)");
        }
    }
}

/// Render database inspection.
pub fn inspect_db(
    db_info: Option<&[(String, String)]>,
    out: &mut impl Write,
) {
    components::section_header("Database", out);

    match db_info {
        Some(info) => {
            let pairs: Vec<(&str, &str)> = info.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            components::kv_table(&pairs, 4, out);
        }
        None => {
            let _ = writeln!(out, "    (no database configured)");
        }
    }
}

/// Render traces inspection: recent sessions with trace step counts.
pub fn inspect_traces(
    session_traces: &[(String, usize, String)], // (session_id_short, step_count, date)
    out: &mut impl Write,
) {
    components::section_header("Traces", out);

    if session_traces.is_empty() {
        let _ = writeln!(out, "    (no traces recorded)");
        return;
    }

    let t = theme::active();
    let r = theme::reset();
    let dim = t.palette.text_dim.fg();

    for (id, count, date) in session_traces {
        let _ = writeln!(out, "    {id}  {count} steps  {dim}{date}{r}");
    }
}

/// Render trace steps for a specific session (browse mode).
pub fn browse_trace(steps: &[TraceStep], out: &mut impl Write) {
    if steps.is_empty() {
        let _ = writeln!(out, "    (no trace steps found)");
        return;
    }

    components::section_header(
        &format!("Trace: {} ({} steps)", &steps[0].session_id.to_string()[..8], steps.len()),
        out,
    );

    let t = theme::active();
    let r = theme::reset();
    let dim = t.palette.text_dim.fg();

    for step in steps {
        let type_badge = step_type_badge(step.step_type);
        let time = step.timestamp.format("%H:%M:%S%.3f");
        let preview: String = step.data_json.chars().take(100).collect();
        let _ = writeln!(
            out,
            "    {dim}{time}{r} {type_badge} #{:<3} {dim}{}ms{r}  {preview}",
            step.step_index, step.duration_ms,
        );
    }
}

// --- Plan + Execute: /plan, /run ---

/// Render an execution plan as a step list.
pub fn render_plan(
    plan_id: &str,
    goal: &str,
    steps: &[(String, Option<String>, f64)], // (description, tool_name, confidence)
    out: &mut impl Write,
) {
    components::section_header("Execution Plan", out);

    let _ = writeln!(out, "    Plan ID: {plan_id}");
    let _ = writeln!(out, "    Goal: {goal}");
    let _ = writeln!(out);

    let t = theme::active();
    let r = theme::reset();
    let dim = t.palette.text_dim.fg();

    for (i, (desc, tool, confidence)) in steps.iter().enumerate() {
        let conf_pct = format!("{:.0}%", confidence * 100.0);
        let badge = if *confidence >= 0.8 {
            components::badge(&conf_pct, components::BadgeLevel::Success)
        } else if *confidence >= 0.5 {
            components::badge(&conf_pct, components::BadgeLevel::Warning)
        } else {
            components::badge(&conf_pct, components::BadgeLevel::Error)
        };

        let tool_str = tool.as_deref().unwrap_or("(none)");
        let _ = writeln!(
            out,
            "    {badge} {}. {desc} {dim}[{tool_str}]{r}",
            i + 1,
        );
    }
}

/// Render orchestrator results.
#[allow(dead_code)]
pub fn render_orchestrator_results(result: &OrchestratorResult, out: &mut impl Write) {
    components::section_header("Orchestration Results", out);

    let success_str = format!("{}/{}", result.success_count, result.total_count);
    let cost_str = format!("${:.4}", result.total_cost_usd);
    let latency_str = format!("{:.1}s", result.total_latency_ms as f64 / 1000.0);
    components::kv_table(
        &[
            ("Tasks", &success_str),
            ("Cost", &cost_str),
            ("Latency", &latency_str),
        ],
        4,
        out,
    );

    let _ = writeln!(out);
    for sub in &result.sub_results {
        let badge = if sub.success {
            components::badge("OK", components::BadgeLevel::Success)
        } else {
            components::badge("FAIL", components::BadgeLevel::Error)
        };
        let preview: String = sub.output_text.chars().take(80).collect();
        let _ = writeln!(
            out,
            "    {badge} {} ({} rounds, {}ms) {}",
            &sub.task_id.to_string()[..8],
            sub.rounds,
            sub.latency_ms,
            if preview.is_empty() {
                sub.error.as_deref().unwrap_or("").to_string()
            } else {
                preview
            },
        );
    }
}

// --- Debug: /replay, /step, /snapshot, /diff ---

/// Replay result info for render_replay_result.
pub struct ReplayInfo<'a> {
    pub original_id: &'a str,
    pub replay_id: &'a str,
    pub original_fp: Option<&'a str>,
    pub replay_fp: &'a str,
    pub fp_match: bool,
    pub rounds: usize,
    pub steps: usize,
}

/// Render replay result with fingerprint comparison.
pub fn render_replay_result(info: &ReplayInfo<'_>, out: &mut impl Write) {
    let original_id = info.original_id;
    let replay_id = info.replay_id;
    let original_fp = info.original_fp;
    let replay_fp = info.replay_fp;
    let fp_match = info.fp_match;
    let rounds = info.rounds;
    let steps = info.steps;
    components::section_header("Replay Result", out);

    let match_badge = if fp_match {
        components::badge("MATCH", components::BadgeLevel::Success)
    } else {
        components::badge("MISMATCH", components::BadgeLevel::Error)
    };

    let rounds_str = rounds.to_string();
    let steps_str = steps.to_string();
    components::kv_table(
        &[
            ("Original", original_id),
            ("Replay", replay_id),
            ("Fingerprint", &match_badge),
            ("Rounds", &rounds_str),
            ("Steps", &steps_str),
        ],
        4,
        out,
    );

    if !fp_match {
        let _ = writeln!(out);
        let t = theme::active();
        let r = theme::reset();
        let dim = t.palette.text_dim.fg();
        let _ = writeln!(
            out,
            "    {dim}Original FP:{r} {}",
            original_fp.unwrap_or("(none)")
        );
        let _ = writeln!(out, "    {dim}Replay FP:{r}   {replay_fp}");
    }
}

/// Render a single trace step in detail.
pub fn render_trace_step(step: &TraceStep, position: usize, total: usize, out: &mut impl Write) {
    let type_badge = step_type_badge(step.step_type);
    let time = step.timestamp.format("%Y-%m-%d %H:%M:%S%.3f");

    let _ = writeln!(out, "    [step {}/{}]", position + 1, total);
    let _ = writeln!(out, "    Type:    {type_badge}");
    let _ = writeln!(out, "    Index:   {}", step.step_index);
    let _ = writeln!(out, "    Time:    {time}");
    let _ = writeln!(out, "    Duration: {}ms", step.duration_ms);

    // Pretty-print data JSON (truncated).
    let preview: String = step.data_json.chars().take(500).collect();
    let _ = writeln!(out, "    Data:    {preview}");
}

/// Diff two sessions side-by-side.
pub fn diff_sessions(
    session_a: &[(String, String)], // kv pairs
    session_b: &[(String, String)],
    out: &mut impl Write,
) {
    components::section_header("Session Diff", out);

    let t = theme::active();
    let r = theme::reset();
    let dim = t.palette.text_dim.fg();

    let max_rows = session_a.len().max(session_b.len());
    let rule = "─".repeat(60);
    let _ = writeln!(out, "    {dim}{rule}{r}");
    let _ = writeln!(out, "    {dim}{:^28}  {:^28}{r}", "A", "B");
    let _ = writeln!(out, "    {dim}{rule}{r}");

    for i in 0..max_rows {
        let (ka, va) = session_a.get(i).map(|(k, v)| (k.as_str(), v.as_str())).unwrap_or(("", ""));
        let (_, vb) = session_b.get(i).map(|(k, v)| (k.as_str(), v.as_str())).unwrap_or(("", ""));

        let delta = if va != vb && !va.is_empty() && !vb.is_empty() {
            components::badge("!=", components::BadgeLevel::Warning)
        } else {
            "  ".to_string()
        };

        let _ = writeln!(
            out,
            "    {dim}{ka:<12}{r} {va:<14} {delta} {vb:<14}",
        );
    }
}

// --- Research: /research ---

/// Decompose a research query into parallel sub-agent tasks.
pub fn decompose_research(query: &str) -> Vec<(String, String)> {
    // Deterministic decomposition: 3 parallel research agents.
    vec![
        (
            format!("Search and analyze: {query}"),
            "Research and find relevant information, code, and documentation.".to_string(),
        ),
        (
            format!("Identify patterns: {query}"),
            "Analyze findings and identify recurring patterns and themes.".to_string(),
        ),
        (
            format!("Summarize insights: {query}"),
            "Synthesize key insights and provide actionable recommendations.".to_string(),
        ),
    ]
}

/// Render a research report from sub-agent results.
pub fn render_research_report(
    query: &str,
    results: &[(String, String, bool)], // (agent_label, output, success)
    total_tokens: u64,
    total_cost: f64,
    out: &mut impl Write,
) {
    components::section_header(&format!("Research: {query}"), out);

    for (label, output, success) in results {
        let badge = if *success {
            components::badge("OK", components::BadgeLevel::Success)
        } else {
            components::badge("FAIL", components::BadgeLevel::Error)
        };
        let _ = writeln!(out, "\n    {badge} {label}");
        if !output.is_empty() {
            for line in output.lines().take(10) {
                let _ = writeln!(out, "      {line}");
            }
        }
    }

    let _ = writeln!(out);
    let t = theme::active();
    let r = theme::reset();
    let dim = t.palette.text_dim.fg();
    let _ = writeln!(
        out,
        "    {dim}Total: {} tokens | ${:.4}{r}",
        total_tokens, total_cost
    );
}

// --- Self-Improvement: /benchmark, /optimize, /analyze ---

/// Render benchmark results.
pub fn render_benchmark(
    workload: &str,
    results: &[(String, String)], // (metric_name, value)
    out: &mut impl Write,
) {
    components::section_header(&format!("Benchmark: {workload}"), out);

    if results.is_empty() {
        let _ = writeln!(out, "    (no results)");
        return;
    }

    let pairs: Vec<(&str, &str)> = results.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    components::kv_table(&pairs, 4, out);
}

/// Render optimization recommendations.
pub fn render_optimize(
    recommendations: &[(String, String)], // (issue, suggestion)
    out: &mut impl Write,
) {
    components::section_header("Optimization Recommendations", out);

    if recommendations.is_empty() {
        let badge = components::badge("OK", components::BadgeLevel::Success);
        let _ = writeln!(out, "    {badge} No issues detected");
        return;
    }

    for (issue, suggestion) in recommendations {
        components::alert(
            components::BadgeLevel::Warning,
            issue,
            Some(suggestion),
            out,
        );
    }
}

/// Render analysis: model/tool rankings and bottlenecks.
pub fn render_analyze(
    model_rankings: &[(String, u64, f64, f64)], // (name, invocations, cost, avg_latency)
    tool_rankings: &[(String, u64, f64, bool)],  // (name, executions, avg_duration, is_bottleneck)
    out: &mut impl Write,
) {
    components::section_header("Analysis", out);

    if !model_rankings.is_empty() {
        let t = theme::active();
        let r = theme::reset();
        let dim = t.palette.text_dim.fg();
        let _ = writeln!(out, "    {dim}Model Rankings (by cost):{r}");
        for (name, invocations, cost, avg_lat) in model_rankings {
            let _ = writeln!(
                out,
                "      {name}: {invocations} inv, ${cost:.4}, {avg_lat:.0}ms avg",
            );
        }
    }

    if !tool_rankings.is_empty() {
        let _ = writeln!(out);
        let t = theme::active();
        let r = theme::reset();
        let dim = t.palette.text_dim.fg();
        let _ = writeln!(out, "    {dim}Tool Rankings (by usage):{r}");
        for (name, executions, avg_dur, is_bottleneck) in tool_rankings {
            let suffix = if *is_bottleneck {
                format!(" {}", components::badge("BOTTLENECK", components::BadgeLevel::Error))
            } else {
                String::new()
            };
            let _ = writeln!(
                out,
                "      {name}: {executions} exec, {avg_dur:.0}ms avg{suffix}",
            );
        }
    }

    if model_rankings.is_empty() && tool_rankings.is_empty() {
        let _ = writeln!(out, "    (no data to analyze)");
    }
}

// --- Helpers ---

/// Create a colored badge for a trace step type.
fn step_type_badge(step_type: TraceStepType) -> String {
    match step_type {
        TraceStepType::ModelRequest => components::badge("REQ", components::BadgeLevel::Info),
        TraceStepType::ModelResponse => components::badge("RSP", components::BadgeLevel::Success),
        TraceStepType::ToolCall => components::badge("CALL", components::BadgeLevel::Warning),
        TraceStepType::ToolResult => components::badge("RES", components::BadgeLevel::Muted),
        TraceStepType::Error => components::badge("ERR", components::BadgeLevel::Error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture() -> Vec<u8> {
        Vec::new()
    }

    // --- render_status ---

    #[test]
    fn render_status_basic() {
        let mut buf = capture();
        let info = StatusInfo {
            session_id: "abc12345", rounds: 3, tokens: 1500, cost: 0.042,
            provider: "echo", model: "echo", provider_diagnostics: &[], registered_models: &[],
        };
        render_status(&info, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("abc12345"));
        assert!(output.contains("echo"));
        assert!(output.contains("0.0420"));
    }

    #[test]
    fn render_status_with_diagnostics() {
        let mut buf = capture();
        let diag = vec![
            ("echo".to_string(), "closed".to_string(), 0usize),
            ("anthropic".to_string(), "open".to_string(), 5usize),
        ];
        let models = vec!["echo".to_string()];
        let info = StatusInfo {
            session_id: "abc", rounds: 1, tokens: 100, cost: 0.0,
            provider: "echo", model: "echo", provider_diagnostics: &diag, registered_models: &models,
        };
        render_status(&info, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("echo"));
        assert!(output.contains("anthropic"));
        assert!(output.contains("Provider Health"));
    }

    #[test]
    fn render_status_with_models() {
        let mut buf = capture();
        let models = vec!["echo".to_string(), "gpt-4o".to_string()];
        let info = StatusInfo {
            session_id: "x", rounds: 0, tokens: 0, cost: 0.0,
            provider: "echo", model: "echo", provider_diagnostics: &[], registered_models: &models,
        };
        render_status(&info, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("echo, gpt-4o"));
    }

    // --- render_metrics ---

    #[test]
    fn render_metrics_empty() {
        let mut buf = capture();
        let sys = SystemMetrics {
            total_invocations: 0,
            total_cost_usd: 0.0,
            total_tokens: 0,
            models: vec![],
        };
        render_metrics(&sys, None, None, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("System Metrics"));
        assert!(output.contains("0"));
    }

    #[test]
    fn render_metrics_populated() {
        let mut buf = capture();
        let sys = SystemMetrics {
            total_invocations: 50,
            total_cost_usd: 1.234,
            total_tokens: 10000,
            models: vec![cuervo_storage::ModelStats {
                provider: "echo".into(),
                model: "echo".into(),
                total_invocations: 50,
                successful_invocations: 48,
                avg_latency_ms: 150.0,
                p95_latency_ms: 300,
                total_tokens: 10000,
                total_cost_usd: 1.234,
                avg_cost_per_invocation: 0.025,
                success_rate: 0.96,
            }],
        };
        let cache = CacheStats {
            total_entries: 10,
            total_hits: 42,
            oldest_entry: None,
            newest_entry: None,
        };
        let memory = MemoryStats {
            total_entries: 5,
            by_type: vec![("fact".into(), 3), ("summary".into(), 2)],
            oldest_entry: None,
            newest_entry: None,
        };
        render_metrics(&sys, Some(&cache), Some(&memory), &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("50"));
        assert!(output.contains("10000"));
        assert!(output.contains("1.2340"));
        assert!(output.contains("echo"));
        assert!(output.contains("Cache"));
        assert!(output.contains("42"));
        assert!(output.contains("Memory"));
        assert!(output.contains("fact"));
    }

    // --- render_logs ---

    #[test]
    fn render_logs_empty() {
        let mut buf = capture();
        render_logs(&[], None, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("no trace steps"));
    }

    #[test]
    fn render_logs_with_steps() {
        let mut buf = capture();
        let steps = vec![TraceStep {
            session_id: uuid::Uuid::nil(),
            step_index: 0,
            step_type: TraceStepType::ModelRequest,
            data_json: r#"{"model":"echo"}"#.to_string(),
            duration_ms: 42,
            timestamp: chrono::Utc::now(),
        }];
        render_logs(&steps, None, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("REQ"));
        assert!(output.contains("42ms"));
    }

    // --- inspect ---

    #[test]
    fn inspect_runtime_produces_output() {
        let mut buf = capture();
        inspect_runtime(&mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Runtime"));
        assert!(output.contains("Arch"));
    }

    #[test]
    fn inspect_memory_no_db() {
        let mut buf = capture();
        inspect_memory(None, 0, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("no database"));
    }

    #[test]
    fn inspect_memory_with_stats() {
        let mut buf = capture();
        let stats = MemoryStats {
            total_entries: 10,
            by_type: vec![("fact".into(), 10)],
            oldest_entry: None,
            newest_entry: None,
        };
        inspect_memory(Some(&stats), 3, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("10"));
        assert!(output.contains("3"));
    }

    #[test]
    fn inspect_db_no_db() {
        let mut buf = capture();
        inspect_db(None, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("no database"));
    }

    #[test]
    fn inspect_db_with_data() {
        let mut buf = capture();
        let data = vec![
            ("page_count".to_string(), "42".to_string()),
            ("page_size".to_string(), "4096".to_string()),
            ("journal_mode".to_string(), "wal".to_string()),
        ];
        inspect_db(Some(&data), &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("42"), "should contain page_count value");
        assert!(output.contains("4096"), "should contain page_size value");
        assert!(output.contains("wal"), "should contain journal_mode");
        assert!(!output.contains("unknown"), "should not contain 'unknown'");
    }

    #[test]
    fn inspect_traces_empty() {
        let mut buf = capture();
        inspect_traces(&[], &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("no traces"));
    }

    #[test]
    fn browse_trace_empty() {
        let mut buf = capture();
        browse_trace(&[], &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("no trace steps"));
    }

    // --- plan ---

    #[test]
    fn render_plan_format() {
        let mut buf = capture();
        let steps = vec![
            ("Read the file".to_string(), Some("read_file".to_string()), 0.9),
            ("Edit the file".to_string(), Some("edit_file".to_string()), 0.6),
            ("Test".to_string(), None, 0.3),
        ];
        render_plan("plan-abc", "Fix the bug", &steps, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("plan-abc"));
        assert!(output.contains("Fix the bug"));
        assert!(output.contains("Read the file"));
        assert!(output.contains("read_file"));
    }

    // --- replay ---

    #[test]
    fn render_replay_result_match() {
        let mut buf = capture();
        let info = ReplayInfo {
            original_id: "aaa", replay_id: "bbb", original_fp: Some("fp1"),
            replay_fp: "fp1", fp_match: true, rounds: 3, steps: 10,
        };
        render_replay_result(&info, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("MATCH"));
        assert!(output.contains("aaa"));
        assert!(output.contains("bbb"));
    }

    #[test]
    fn render_replay_result_mismatch() {
        let mut buf = capture();
        let info = ReplayInfo {
            original_id: "aaa", replay_id: "bbb", original_fp: Some("fp1"),
            replay_fp: "fp2", fp_match: false, rounds: 3, steps: 10,
        };
        render_replay_result(&info, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("MISMATCH"));
        assert!(output.contains("fp1"));
        assert!(output.contains("fp2"));
    }

    // --- step ---

    #[test]
    fn render_trace_step_format() {
        let mut buf = capture();
        let step = TraceStep {
            session_id: uuid::Uuid::nil(),
            step_index: 2,
            step_type: TraceStepType::ToolCall,
            data_json: r#"{"tool":"bash"}"#.to_string(),
            duration_ms: 100,
            timestamp: chrono::Utc::now(),
        };
        render_trace_step(&step, 2, 5, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("[step 3/5]"));
        assert!(output.contains("100ms"));
        assert!(output.contains("bash"));
    }

    // --- diff ---

    #[test]
    fn diff_sessions_format() {
        let mut buf = capture();
        let a = vec![("Messages".into(), "10".into()), ("Tokens".into(), "500".into())];
        let b = vec![("Messages".into(), "12".into()), ("Tokens".into(), "500".into())];
        diff_sessions(&a, &b, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Session Diff"));
        assert!(output.contains("10"));
        assert!(output.contains("12"));
    }

    // --- research ---

    #[test]
    fn decompose_research_count() {
        let tasks = decompose_research("how does auth work");
        assert_eq!(tasks.len(), 3);
        assert!(tasks[0].0.contains("auth work"));
    }

    #[test]
    fn render_research_report_format() {
        let mut buf = capture();
        let results = vec![
            ("Agent 1".to_string(), "Found auth module".to_string(), true),
            ("Agent 2".to_string(), "Pattern: JWT".to_string(), true),
        ];
        render_research_report("auth", &results, 500, 0.01, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("auth"));
        assert!(output.contains("Found auth module"));
        assert!(output.contains("JWT"));
    }

    // --- benchmark ---

    #[test]
    fn render_benchmark_format() {
        let mut buf = capture();
        let results = vec![
            ("P50 latency".into(), "150ms".into()),
            ("P95 latency".into(), "300ms".into()),
        ];
        render_benchmark("inference", &results, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("inference"));
        assert!(output.contains("150ms"));
    }

    #[test]
    fn render_benchmark_empty() {
        let mut buf = capture();
        render_benchmark("cache", &[], &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("no results"));
    }

    // --- optimize ---

    #[test]
    fn render_optimize_no_issues() {
        let mut buf = capture();
        render_optimize(&[], &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("No issues"));
    }

    #[test]
    fn render_optimize_with_issues() {
        let mut buf = capture();
        let recs = vec![("High P95 latency".into(), "Consider a faster model".into())];
        render_optimize(&recs, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("High P95"));
        assert!(output.contains("faster model"));
    }

    // --- analyze ---

    #[test]
    fn render_analyze_empty() {
        let mut buf = capture();
        render_analyze(&[], &[], &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("no data"));
    }

    #[test]
    fn render_analyze_with_data() {
        let mut buf = capture();
        let models = vec![("echo/echo".to_string(), 50u64, 1.0f64, 150.0f64)];
        let tools = vec![("bash".to_string(), 20u64, 200.0f64, false)];
        render_analyze(&models, &tools, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("echo/echo"));
        assert!(output.contains("bash"));
    }

    #[test]
    fn render_analyze_bottleneck() {
        let mut buf = capture();
        let tools = vec![("slow_tool".to_string(), 10u64, 6000.0f64, true)];
        render_analyze(&[], &tools, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("BOTTLENECK"));
    }

    // --- step_type_badge ---

    #[test]
    fn step_type_badges_all_types() {
        for st in [
            TraceStepType::ModelRequest,
            TraceStepType::ModelResponse,
            TraceStepType::ToolCall,
            TraceStepType::ToolResult,
            TraceStepType::Error,
        ] {
            let badge = step_type_badge(st);
            assert!(!badge.is_empty());
        }
    }
}
