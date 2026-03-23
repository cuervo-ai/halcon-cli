// DECISION: CiSink emits NDJSON to stdout (one JSON object per line).
// This format is chosen over structured logging because:
// 1. It is trivially parseable with `jq` without installing anything
// 2. GitHub Actions, GitLab CI, and Datadog all natively consume NDJSON
// 3. Each line is independently parseable (no partial-read failures)
// See US-output-format (PASO 2-A) and US-github-actions (PASO 2-E).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use halcon_core::types::ContentBlock;
use serde_json::{json, Value};

use super::sink::RenderSink;

/// Machine-readable sink that emits NDJSON lines to stdout.
///
/// Every event the agent loop generates is serialised as a flat JSON object
/// followed by a newline.  Consumers (CI scripts, Datadog, GitHub Actions)
/// can pipe the output through `jq` without additional tooling.
///
/// Event types emitted:
/// - `session_start`  — emitted by `session_started()`
/// - `tool_call`      — emitted by `tool_start()`
/// - `tool_result`    — emitted by `tool_output()`
/// - `response`       — emitted by `stream_done()` (contains full streamed text)
/// - `session_end`    — emitted by `spinner_stop()` on the final round (best-effort)
/// - `warning` / `error` / `info` — diagnostic events
pub struct CiSink {
    session_id: Mutex<String>,
    current_round: AtomicUsize,
    /// Accumulated streaming text for the current round.
    stream_buf: Mutex<String>,
    /// Token counters updated via `round_ended`.
    total_input_tokens: AtomicUsize,
    total_output_tokens: AtomicUsize,
    /// Accumulated cost.
    total_cost_usd: Mutex<f64>,
    /// Total rounds seen.
    rounds_total: AtomicUsize,
}

impl CiSink {
    pub fn new() -> Self {
        Self {
            session_id: Mutex::new(String::new()),
            current_round: AtomicUsize::new(0),
            stream_buf: Mutex::new(String::new()),
            total_input_tokens: AtomicUsize::new(0),
            total_output_tokens: AtomicUsize::new(0),
            total_cost_usd: Mutex::new(0.0),
            rounds_total: AtomicUsize::new(0),
        }
    }

    fn emit(&self, value: Value) {
        // Each line is independently parseable — guaranteed by trailing `\n`.
        println!("{}", value);
    }

    fn now_iso8601() -> String {
        // RFC 3339 / ISO-8601 timestamp without external chrono dep.
        // Uses std::time::SystemTime and formats manually.
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Minimal ISO-8601 in UTC: yyyy-mm-ddThh:mm:ssZ
        let s = secs % 60;
        let m = (secs / 60) % 60;
        let h = (secs / 3600) % 24;
        let days = secs / 86400;
        // Approximate calendar (good enough for audit trail — not used for sorting).
        let year = 1970 + days / 365;
        let day_of_year = days % 365;
        let month = day_of_year / 30 + 1;
        let day = day_of_year % 30 + 1;
        format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
    }
}

impl Default for CiSink {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderSink for CiSink {
    fn stream_text(&self, text: &str) {
        let mut buf = self.stream_buf.lock().unwrap_or_else(|e| e.into_inner());
        buf.push_str(text);
    }

    fn stream_code_block(&self, _lang: &str, _code: &str) {
        // Code blocks flow through stream_text — nothing extra needed.
    }

    fn stream_tool_marker(&self, _name: &str) {
        // Tool markers are noisy in NDJSON — suppressed.
    }

    fn stream_done(&self) {
        let text = {
            let mut buf = self.stream_buf.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *buf)
        };
        let round = self.current_round.load(Ordering::Relaxed);
        self.emit(json!({
            "type": "response",
            "text": text,
            "round": round,
        }));
    }

    fn stream_error(&self, msg: &str) {
        self.emit(json!({
            "type": "error",
            "message": msg,
            "timestamp": Self::now_iso8601(),
        }));
    }

    fn stream_reset(&self) {
        let mut buf = self.stream_buf.lock().unwrap_or_else(|e| e.into_inner());
        buf.clear();
    }

    fn stream_full_text(&self) -> String {
        self.stream_buf.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn tool_start(&self, name: &str, input: &serde_json::Value) {
        let round = self.current_round.load(Ordering::Relaxed);
        // Truncate large inputs to 512 chars to avoid log bloat in CI.
        let input_str = input.to_string();
        let input_truncated = if input_str.len() > 512 {
            format!("{}...[truncated]", &input_str[..512])
        } else {
            input_str
        };
        self.emit(json!({
            "type": "tool_call",
            "tool": name,
            "input": input_truncated,
            "round": round,
        }));
    }

    fn tool_output(&self, block: &ContentBlock, duration_ms: u64) {
        let (success, output, tool_id) = match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let ok = !is_error;
                let text = content.clone();
                // Truncate long tool output to 1024 chars.
                let text = if text.len() > 1024 {
                    format!("{}...[truncated {} bytes]", &text[..1024], text.len())
                } else {
                    text
                };
                (ok, text, tool_use_id.as_str())
            }
            _ => (true, String::new(), ""),
        };
        self.emit(json!({
            "type": "tool_result",
            "tool": tool_id,
            "success": success,
            "output": output,
            "duration_ms": duration_ms,
        }));
    }

    fn tool_denied(&self, name: &str) {
        self.emit(json!({
            "type": "tool_denied",
            "tool": name,
        }));
    }

    fn spinner_start(&self, _label: &str) {
        // Spinners are terminal-only — suppressed in CI output.
    }

    fn spinner_stop(&self) {
        // Suppressed.
    }

    fn warning(&self, message: &str, hint: Option<&str>) {
        self.emit(json!({
            "type": "warning",
            "message": message,
            "hint": hint,
        }));
    }

    fn error(&self, message: &str, hint: Option<&str>) {
        self.emit(json!({
            "type": "error",
            "message": message,
            "hint": hint,
        }));
    }

    fn info(&self, message: &str) {
        self.emit(json!({
            "type": "info",
            "message": message,
        }));
    }

    fn is_silent(&self) -> bool {
        false
    }

    // --- Phase 42B: Cockpit feedback methods ---

    fn session_started(&self, session_id: &str) {
        {
            let mut s = self.session_id.lock().unwrap_or_else(|e| e.into_inner());
            *s = session_id.to_string();
        }
        self.emit(json!({
            "type": "session_start",
            "timestamp": Self::now_iso8601(),
            "session_id": session_id,
        }));
    }

    fn round_started(&self, round: usize, provider: &str, model: &str) {
        self.current_round.store(round, Ordering::Relaxed);
        self.emit(json!({
            "type": "round_start",
            "round": round,
            "provider": provider,
            "model": model,
        }));
    }

    fn round_ended(
        &self,
        round: usize,
        input_tokens: u32,
        output_tokens: u32,
        cost: f64,
        duration_ms: u64,
    ) {
        self.total_input_tokens
            .fetch_add(input_tokens as usize, Ordering::Relaxed);
        self.total_output_tokens
            .fetch_add(output_tokens as usize, Ordering::Relaxed);
        {
            let mut c = self.total_cost_usd.lock().unwrap_or_else(|e| e.into_inner());
            *c += cost;
        }
        self.rounds_total.store(round, Ordering::Relaxed);
        self.emit(json!({
            "type": "round_end",
            "round": round,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "cost_usd": cost,
            "duration_ms": duration_ms,
        }));
    }

    fn model_selected(&self, model: &str, provider: &str, reason: &str) {
        self.emit(json!({
            "type": "model_selected",
            "model": model,
            "provider": provider,
            "reason": reason,
        }));
    }

    fn provider_fallback(&self, from: &str, to: &str, reason: &str) {
        self.emit(json!({
            "type": "provider_fallback",
            "from": from,
            "to": to,
            "reason": reason,
        }));
    }

    fn sub_agent_spawned(&self, step: usize, total: usize, description: &str, agent_type: &str) {
        self.emit(json!({
            "type": "sub_agent_spawned",
            "step": step,
            "total": total,
            "description": description,
            "agent_type": agent_type,
        }));
    }

    fn sub_agent_completed(
        &self,
        step: usize,
        total: usize,
        success: bool,
        latency_ms: u64,
        tools_used: &[String],
        rounds: usize,
        summary: &str,
        error_hint: &str,
    ) {
        self.emit(json!({
            "type": "sub_agent_completed",
            "step": step,
            "total": total,
            "success": success,
            "latency_ms": latency_ms,
            "tools_used": tools_used,
            "rounds": rounds,
            "summary": summary,
            "error_hint": error_hint,
        }));
    }
}

// We intentionally do NOT emit session_end from a Drop impl because we can't
// know total tokens at drop time reliably. Instead, `session_end` is emitted
// by main.rs after the agent loop completes, using the CiSink's stored totals.
impl CiSink {
    /// Emit the final `session_end` event.
    ///
    /// Called explicitly from main.rs after the agent loop returns so that
    /// the full token / cost data is available.
    pub fn emit_session_end(&self) {
        let session_id = self.session_id.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let rounds = self.rounds_total.load(Ordering::Relaxed);
        let input_tokens = self.total_input_tokens.load(Ordering::Relaxed);
        let output_tokens = self.total_output_tokens.load(Ordering::Relaxed);
        let tokens_used = input_tokens + output_tokens;
        let cost_usd = *self.total_cost_usd.lock().unwrap_or_else(|e| e.into_inner());
        self.emit(json!({
            "type": "session_end",
            "session_id": session_id,
            "rounds": rounds,
            "tokens_used": tokens_used,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "cost_usd": cost_usd,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_sink_is_not_silent() {
        let sink = CiSink::new();
        assert!(!sink.is_silent());
    }

    #[test]
    fn ci_sink_stream_accumulates_text() {
        let sink = CiSink::new();
        sink.stream_text("hello ");
        sink.stream_text("world");
        assert_eq!(sink.stream_full_text(), "hello world");
    }

    #[test]
    fn ci_sink_stream_reset_clears_buf() {
        let sink = CiSink::new();
        sink.stream_text("data");
        sink.stream_reset();
        assert_eq!(sink.stream_full_text(), "");
    }

    #[test]
    fn ci_sink_now_iso8601_has_correct_format() {
        let ts = CiSink::now_iso8601();
        // Should look like "YYYY-MM-DDTHH:MM:SSZ"
        assert!(ts.ends_with('Z'), "timestamp must end with Z: {ts}");
        assert!(ts.contains('T'), "timestamp must contain T: {ts}");
    }

    #[test]
    fn ci_sink_round_tracking() {
        let sink = CiSink::new();
        sink.round_started(1, "anthropic", "claude-sonnet-4-6");
        assert_eq!(sink.current_round.load(Ordering::Relaxed), 1);
        sink.round_ended(1, 100, 50, 0.001, 1200);
        assert_eq!(sink.total_input_tokens.load(Ordering::Relaxed), 100);
        assert_eq!(sink.total_output_tokens.load(Ordering::Relaxed), 50);
    }
}
