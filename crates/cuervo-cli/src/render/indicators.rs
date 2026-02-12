//! Visual indicators — provider status, circuit breakers, streaming metrics,
//! tool timelines, retry status, and budget tracking.
//!
//! Designed to be rendered inline during REPL streaming or as standalone
//! status panels. All functions accept `&mut impl Write` for testability.

use std::io::Write;

use super::color;
use super::components::BadgeLevel;
use super::theme;

// ── Provider & Circuit Breaker ─────────────────────────────────

/// Provider health state for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderState {
    /// Normal operation — circuit breaker closed.
    Healthy,
    /// Degraded — elevated errors but still operational.
    Degraded,
    /// Circuit breaker open — provider isolated.
    Open,
    /// Half-open — probing for recovery.
    Probing,
    /// Fallback active — using alternate provider.
    Fallback,
}

/// Provider status for rendering.
#[derive(Debug, Clone)]
pub struct ProviderStatus {
    pub name: String,
    pub state: ProviderState,
    pub failure_count: usize,
    pub backpressure_pct: f64,
    pub health_score: Option<u32>,
}

/// Render a provider status indicator (single line, inline).
pub fn render_provider_indicator(status: &ProviderStatus, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();

    let (icon, color, label) = match status.state {
        ProviderState::Healthy => {
            let icon = if color::unicode_enabled() { "●" } else { "*" };
            (icon, t.palette.success.fg(), "healthy")
        }
        ProviderState::Degraded => {
            let icon = if color::unicode_enabled() { "◐" } else { "~" };
            (icon, t.palette.warning.fg(), "degraded")
        }
        ProviderState::Open => {
            let icon = if color::unicode_enabled() { "○" } else { "x" };
            (icon, t.palette.error.fg(), "open")
        }
        ProviderState::Probing => {
            let icon = if color::unicode_enabled() { "◔" } else { "?" };
            (icon, t.palette.primary.fg(), "probing")
        }
        ProviderState::Fallback => {
            let icon = if color::unicode_enabled() { "↻" } else { ">" };
            (icon, t.palette.warning.fg(), "fallback")
        }
    };

    let _ = write!(out, "{color}{icon} {}{r}", status.name);

    // Append health score if available.
    if let Some(score) = status.health_score {
        let score_color = if score >= 80 {
            t.palette.success.fg()
        } else if score >= 50 {
            t.palette.warning.fg()
        } else {
            t.palette.error.fg()
        };
        let _ = write!(out, " {score_color}{score}%{r}");
    }

    // Append failure count if non-zero.
    if status.failure_count > 0 {
        let _ = write!(
            out,
            " {muted}({} fail){r}",
            status.failure_count
        );
    }

    let _ = write!(out, " {muted}[{label}]{r}");
}

/// Render a compact provider status bar (multiple providers on one line).
pub fn render_provider_bar(statuses: &[ProviderStatus], out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();

    let _ = write!(out, "  ");
    for (i, status) in statuses.iter().enumerate() {
        if i > 0 {
            let _ = write!(out, " {muted}│{r} ");
        }
        render_provider_indicator(status, out);
    }
    let _ = writeln!(out);
}

/// Render circuit breaker status panel.
pub fn render_circuit_breaker_panel(
    diagnostics: &[ProviderDiagnosticView],
    out: &mut impl Write,
) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();
    let accent = t.palette.accent.fg();
    let bold = if color::color_enabled() { "\x1b[1m" } else { "" };

    let _ = writeln!(out, "\n  {bold}{accent}Circuit Breakers{r}");

    for diag in diagnostics {
        let (icon, color, state_label) = match diag.state.as_str() {
            "closed" => {
                let icon = if color::unicode_enabled() { "●" } else { "*" };
                (icon, t.palette.success.fg(), "CLOSED")
            }
            "open" => {
                let icon = if color::unicode_enabled() { "○" } else { "x" };
                (icon, t.palette.error.fg(), "OPEN")
            }
            "half_open" => {
                let icon = if color::unicode_enabled() { "◔" } else { "?" };
                (icon, t.palette.primary.fg(), "HALF_OPEN")
            }
            _ => {
                ("?", t.palette.muted.fg(), "UNKNOWN")
            }
        };

        let bp_pct = if diag.backpressure_max > 0 {
            (diag.backpressure_in_use as f64 / diag.backpressure_max as f64) * 100.0
        } else {
            0.0
        };
        let bp_color = if bp_pct > 80.0 {
            t.palette.error.fg()
        } else if bp_pct > 50.0 {
            t.palette.warning.fg()
        } else {
            t.palette.success.fg()
        };

        let _ = writeln!(
            out,
            "  {color}{icon} {}{r} {color}[{state_label}]{r} {muted}failures:{r}{} {bp_color}bp:{:.0}%{r}",
            diag.provider, diag.failure_count, bp_pct,
        );
    }
}

/// View model for a provider diagnostic (decoupled from repl types).
#[derive(Debug, Clone)]
pub struct ProviderDiagnosticView {
    pub provider: String,
    pub state: String,
    pub failure_count: usize,
    pub backpressure_in_use: u32,
    pub backpressure_max: u32,
}

// ── Streaming Metrics ──────────────────────────────────────────

/// Live streaming metrics for display.
#[derive(Debug, Clone, Default)]
pub struct StreamingMetrics {
    pub tokens_received: u64,
    pub elapsed_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
    pub is_streaming: bool,
}

impl StreamingMetrics {
    pub fn tokens_per_sec(&self) -> f64 {
        if self.elapsed_ms == 0 {
            return 0.0;
        }
        (self.tokens_received as f64 / self.elapsed_ms as f64) * 1000.0
    }
}

/// Render streaming metrics inline (during active streaming).
pub fn render_streaming_metrics(metrics: &StreamingMetrics, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();

    if !metrics.is_streaming {
        return;
    }

    let tps = metrics.tokens_per_sec();
    let tps_color = if tps > 50.0 {
        t.palette.success.fg()
    } else if tps > 20.0 {
        t.palette.primary.fg()
    } else if tps > 5.0 {
        t.palette.warning.fg()
    } else {
        t.palette.error.fg()
    };

    let _ = write!(out, "  {tps_color}{:.0} tok/s{r}", tps);
    let _ = write!(out, " {muted}{}tok{r}", metrics.tokens_received);

    if metrics.estimated_cost_usd > 0.0 {
        let _ = write!(out, " {muted}${:.4}{r}", metrics.estimated_cost_usd);
    }

    let dur = if metrics.elapsed_ms < 1000 {
        format!("{}ms", metrics.elapsed_ms)
    } else {
        format!("{:.1}s", metrics.elapsed_ms as f64 / 1000.0)
    };
    let _ = write!(out, " {muted}{dur}{r}");

    let _ = writeln!(out);
}

// ── Retry Status ───────────────────────────────────────────────

/// Retry attempt info for display.
#[derive(Debug, Clone)]
pub struct RetryInfo {
    pub attempt: u32,
    pub max_attempts: u32,
    pub tool_name: String,
    pub delay_ms: u64,
    pub reason: String,
}

/// Render retry indicator.
pub fn render_retry_indicator(info: &RetryInfo, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let warn = t.palette.warning.fg();
    let muted = t.palette.text_dim.fg();

    let icon = if color::unicode_enabled() { "↻" } else { ">" };

    let _ = writeln!(
        out,
        "  {warn}{icon} Retry {}/{}{r} {muted}{} — {} ({}ms delay){r}",
        info.attempt, info.max_attempts, info.tool_name, info.reason, info.delay_ms,
    );
}

// ── Tool Timeline ──────────────────────────────────────────────

/// A tool execution entry for the timeline.
#[derive(Debug, Clone)]
pub struct ToolTimelineEntry {
    pub name: String,
    pub duration_ms: u64,
    pub success: bool,
    pub is_parallel: bool,
}

/// Render a tool execution timeline.
pub fn render_tool_timeline(entries: &[ToolTimelineEntry], out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();
    let accent = t.palette.accent.fg();
    let bold = if color::color_enabled() { "\x1b[1m" } else { "" };

    if entries.is_empty() {
        return;
    }

    let _ = writeln!(out, "\n  {bold}{accent}Tool Timeline{r}");

    let total_ms: u64 = entries.iter().map(|e| e.duration_ms).sum();

    for entry in entries {
        let status_color = if entry.success {
            t.palette.success.fg()
        } else {
            t.palette.error.fg()
        };
        let status_icon = if entry.success {
            if color::unicode_enabled() { "✓" } else { "+" }
        } else if color::unicode_enabled() {
            "✖"
        } else {
            "x"
        };

        let dur_str = if entry.duration_ms < 1000 {
            format!("{}ms", entry.duration_ms)
        } else {
            format!("{:.1}s", entry.duration_ms as f64 / 1000.0)
        };

        // Proportional bar.
        let bar_width = 20;
        let proportion = if total_ms > 0 {
            (entry.duration_ms as f64 / total_ms as f64 * bar_width as f64) as usize
        } else {
            0
        };
        let bar_char = if color::unicode_enabled() { "█" } else { "#" };
        let bar = bar_char.repeat(proportion.max(1));

        let parallel_tag = if entry.is_parallel {
            format!(" {muted}[parallel]{r}")
        } else {
            String::new()
        };

        let _ = writeln!(
            out,
            "  {status_color}{status_icon}{r} {muted}{:<12}{r} {status_color}{bar}{r} {muted}{dur_str}{r}{parallel_tag}",
            entry.name,
        );
    }

    // Total.
    let total_str = if total_ms < 1000 {
        format!("{total_ms}ms")
    } else {
        format!("{:.1}s", total_ms as f64 / 1000.0)
    };
    let _ = writeln!(out, "  {muted}{:>14} total: {total_str}{r}", "");
}

// ── Round Summary ──────────────────────────────────────────────

/// Summary of an agent round for display.
#[derive(Debug, Clone)]
pub struct RoundSummary {
    pub round: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub latency_ms: u64,
    pub cost_usd: f64,
    pub tool_calls: u32,
    pub provider: String,
}

/// Render a round summary line.
pub fn render_round_summary(summary: &RoundSummary, out: &mut impl Write) {
    let t = theme::active();
    let r = theme::reset();
    let muted = t.palette.text_dim.fg();
    let accent = t.palette.accent.fg();

    let latency_color = if summary.latency_ms < 500 {
        t.palette.success.fg()
    } else if summary.latency_ms < 2000 {
        t.palette.warning.fg()
    } else {
        t.palette.error.fg()
    };

    let total_tok = summary.input_tokens + summary.output_tokens;

    let _ = write!(
        out,
        "  {accent}R{}{r} {muted}{}{r} {latency_color}{}ms{r} {muted}{total_tok}tok{r}",
        summary.round, summary.provider, summary.latency_ms,
    );

    if summary.tool_calls > 0 {
        let _ = write!(out, " {muted}{}tools{r}", summary.tool_calls);
    }

    if summary.cost_usd > 0.0 {
        let _ = write!(out, " {muted}${:.4}{r}", summary.cost_usd);
    }

    let _ = writeln!(out);
}

// ── Fallback Indicator ─────────────────────────────────────────

/// Render a provider fallback notification.
pub fn render_fallback_indicator(
    from_provider: &str,
    to_provider: &str,
    reason: &str,
    out: &mut impl Write,
) {
    let t = theme::active();
    let r = theme::reset();
    let warn = t.palette.warning.fg();
    let muted = t.palette.text_dim.fg();

    let icon = if color::unicode_enabled() { "↻" } else { ">" };

    let _ = writeln!(
        out,
        "  {warn}{icon} Fallback{r}: {warn}{from_provider}{r} → {warn}{to_provider}{r} {muted}({reason}){r}",
    );
}

/// Convert a badge level to a ProviderState.
pub fn badge_to_provider_state(level: BadgeLevel) -> ProviderState {
    match level {
        BadgeLevel::Success => ProviderState::Healthy,
        BadgeLevel::Warning => ProviderState::Degraded,
        BadgeLevel::Error => ProviderState::Open,
        BadgeLevel::Info => ProviderState::Probing,
        BadgeLevel::Muted => ProviderState::Healthy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture() -> Vec<u8> {
        Vec::new()
    }

    fn output_str(buf: &[u8]) -> String {
        String::from_utf8_lossy(buf).to_string()
    }

    // ── Provider Indicator ──────────────────────────────────────

    #[test]
    fn provider_healthy() {
        let status = ProviderStatus {
            name: "deepseek".into(),
            state: ProviderState::Healthy,
            failure_count: 0,
            backpressure_pct: 0.0,
            health_score: Some(95),
        };
        let mut buf = capture();
        render_provider_indicator(&status, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("deepseek"));
        assert!(out.contains("95%"));
        assert!(out.contains("healthy"));
    }

    #[test]
    fn provider_degraded() {
        let status = ProviderStatus {
            name: "openai".into(),
            state: ProviderState::Degraded,
            failure_count: 3,
            backpressure_pct: 0.5,
            health_score: Some(65),
        };
        let mut buf = capture();
        render_provider_indicator(&status, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("openai"));
        assert!(out.contains("3 fail"));
        assert!(out.contains("degraded"));
    }

    #[test]
    fn provider_open() {
        let status = ProviderStatus {
            name: "anthropic".into(),
            state: ProviderState::Open,
            failure_count: 10,
            backpressure_pct: 1.0,
            health_score: Some(20),
        };
        let mut buf = capture();
        render_provider_indicator(&status, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("open"));
        assert!(out.contains("10 fail"));
    }

    #[test]
    fn provider_fallback_state() {
        let status = ProviderStatus {
            name: "gemini".into(),
            state: ProviderState::Fallback,
            failure_count: 1,
            backpressure_pct: 0.0,
            health_score: None,
        };
        let mut buf = capture();
        render_provider_indicator(&status, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("gemini"));
        assert!(out.contains("fallback"));
        // No health score rendered.
        assert!(!out.contains("%"));
    }

    #[test]
    fn provider_no_failures_hides_count() {
        let status = ProviderStatus {
            name: "echo".into(),
            state: ProviderState::Healthy,
            failure_count: 0,
            backpressure_pct: 0.0,
            health_score: None,
        };
        let mut buf = capture();
        render_provider_indicator(&status, &mut buf);
        let out = output_str(&buf);
        assert!(!out.contains("fail"));
    }

    // ── Provider Bar ────────────────────────────────────────────

    #[test]
    fn provider_bar_multiple() {
        let statuses = vec![
            ProviderStatus {
                name: "a".into(),
                state: ProviderState::Healthy,
                failure_count: 0,
                backpressure_pct: 0.0,
                health_score: None,
            },
            ProviderStatus {
                name: "b".into(),
                state: ProviderState::Open,
                failure_count: 5,
                backpressure_pct: 0.0,
                health_score: None,
            },
        ];
        let mut buf = capture();
        render_provider_bar(&statuses, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("a"));
        assert!(out.contains("b"));
    }

    // ── Circuit Breaker Panel ───────────────────────────────────

    #[test]
    fn circuit_breaker_panel_closed() {
        let diags = vec![ProviderDiagnosticView {
            provider: "deepseek".into(),
            state: "closed".into(),
            failure_count: 0,
            backpressure_in_use: 0,
            backpressure_max: 10,
        }];
        let mut buf = capture();
        render_circuit_breaker_panel(&diags, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("Circuit Breakers"));
        assert!(out.contains("deepseek"));
        assert!(out.contains("CLOSED"));
    }

    #[test]
    fn circuit_breaker_panel_open() {
        let diags = vec![ProviderDiagnosticView {
            provider: "openai".into(),
            state: "open".into(),
            failure_count: 5,
            backpressure_in_use: 8,
            backpressure_max: 10,
        }];
        let mut buf = capture();
        render_circuit_breaker_panel(&diags, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("OPEN"));
        assert!(out.contains("5"));
        assert!(out.contains("bp:"));
    }

    #[test]
    fn circuit_breaker_panel_empty() {
        let mut buf = capture();
        render_circuit_breaker_panel(&[], &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("Circuit Breakers"));
    }

    // ── Streaming Metrics ───────────────────────────────────────

    #[test]
    fn streaming_metrics_tokens_per_sec() {
        let metrics = StreamingMetrics {
            tokens_received: 100,
            elapsed_ms: 2000,
            is_streaming: true,
            ..Default::default()
        };
        assert!((metrics.tokens_per_sec() - 50.0).abs() < 0.1);
    }

    #[test]
    fn streaming_metrics_zero_elapsed() {
        let metrics = StreamingMetrics {
            tokens_received: 100,
            elapsed_ms: 0,
            is_streaming: true,
            ..Default::default()
        };
        assert_eq!(metrics.tokens_per_sec(), 0.0);
    }

    #[test]
    fn render_streaming_not_active() {
        let metrics = StreamingMetrics {
            is_streaming: false,
            ..Default::default()
        };
        let mut buf = capture();
        render_streaming_metrics(&metrics, &mut buf);
        let out = output_str(&buf);
        assert!(out.is_empty());
    }

    #[test]
    fn render_streaming_active() {
        let metrics = StreamingMetrics {
            tokens_received: 500,
            elapsed_ms: 5000,
            estimated_cost_usd: 0.0012,
            is_streaming: true,
            ..Default::default()
        };
        let mut buf = capture();
        render_streaming_metrics(&metrics, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("tok/s"));
        assert!(out.contains("500tok"));
        assert!(out.contains("$0.0012"));
    }

    // ── Retry Indicator ─────────────────────────────────────────

    #[test]
    fn render_retry() {
        let info = RetryInfo {
            attempt: 2,
            max_attempts: 3,
            tool_name: "bash".into(),
            delay_ms: 500,
            reason: "timeout".into(),
        };
        let mut buf = capture();
        render_retry_indicator(&info, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("Retry 2/3"));
        assert!(out.contains("bash"));
        assert!(out.contains("timeout"));
        assert!(out.contains("500ms"));
    }

    // ── Tool Timeline ───────────────────────────────────────────

    #[test]
    fn tool_timeline_renders() {
        let entries = vec![
            ToolTimelineEntry {
                name: "file_read".into(),
                duration_ms: 15,
                success: true,
                is_parallel: false,
            },
            ToolTimelineEntry {
                name: "bash".into(),
                duration_ms: 2500,
                success: true,
                is_parallel: false,
            },
            ToolTimelineEntry {
                name: "grep".into(),
                duration_ms: 100,
                success: false,
                is_parallel: true,
            },
        ];
        let mut buf = capture();
        render_tool_timeline(&entries, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("Tool Timeline"));
        assert!(out.contains("file_read"));
        assert!(out.contains("bash"));
        assert!(out.contains("grep"));
        assert!(out.contains("[parallel]"));
        assert!(out.contains("total"));
    }

    #[test]
    fn tool_timeline_empty() {
        let mut buf = capture();
        render_tool_timeline(&[], &mut buf);
        let out = output_str(&buf);
        assert!(out.is_empty());
    }

    #[test]
    fn tool_timeline_single() {
        let entries = vec![ToolTimelineEntry {
            name: "glob".into(),
            duration_ms: 42,
            success: true,
            is_parallel: false,
        }];
        let mut buf = capture();
        render_tool_timeline(&entries, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("glob"));
        assert!(out.contains("42ms"));
    }

    // ── Round Summary ───────────────────────────────────────────

    #[test]
    fn round_summary_renders() {
        let summary = RoundSummary {
            round: 3,
            input_tokens: 500,
            output_tokens: 200,
            latency_ms: 350,
            cost_usd: 0.005,
            tool_calls: 2,
            provider: "deepseek".into(),
        };
        let mut buf = capture();
        render_round_summary(&summary, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("R3"));
        assert!(out.contains("deepseek"));
        assert!(out.contains("350ms"));
        assert!(out.contains("700tok"));
        assert!(out.contains("2tools"));
        assert!(out.contains("$0.005"));
    }

    #[test]
    fn round_summary_no_tools_no_cost() {
        let summary = RoundSummary {
            round: 1,
            input_tokens: 100,
            output_tokens: 50,
            latency_ms: 1500,
            cost_usd: 0.0,
            tool_calls: 0,
            provider: "echo".into(),
        };
        let mut buf = capture();
        render_round_summary(&summary, &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("R1"));
        assert!(!out.contains("tools"));
        assert!(!out.contains("$"));
    }

    // ── Fallback Indicator ──────────────────────────────────────

    #[test]
    fn fallback_indicator_renders() {
        let mut buf = capture();
        render_fallback_indicator("openai", "deepseek", "circuit breaker open", &mut buf);
        let out = output_str(&buf);
        assert!(out.contains("Fallback"));
        assert!(out.contains("openai"));
        assert!(out.contains("deepseek"));
        assert!(out.contains("circuit breaker open"));
    }

    // ── Badge to Provider State ─────────────────────────────────

    #[test]
    fn badge_level_conversion() {
        assert_eq!(
            badge_to_provider_state(BadgeLevel::Success),
            ProviderState::Healthy
        );
        assert_eq!(
            badge_to_provider_state(BadgeLevel::Warning),
            ProviderState::Degraded
        );
        assert_eq!(
            badge_to_provider_state(BadgeLevel::Error),
            ProviderState::Open
        );
        assert_eq!(
            badge_to_provider_state(BadgeLevel::Info),
            ProviderState::Probing
        );
    }
}
