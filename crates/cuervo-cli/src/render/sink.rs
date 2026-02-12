//! Render sink abstraction — decouples the agent loop from terminal output.
//!
//! The agent loop calls `RenderSink` methods instead of writing directly to
//! stdout/stderr. Two built-in implementations:
//! - `ClassicSink`: delegates to existing render functions (zero behavior change)
//! - `SilentSink`: accumulates text without terminal output (for sub-agents/tests)

use std::cell::RefCell;
use std::sync::Mutex;

use cuervo_core::types::ContentBlock;

use super::feedback;
use super::spinner::Spinner;
use super::stream::StreamRenderer;
use super::tool as tool_render;

/// Trait for rendering agent loop output.
///
/// Implementations can target different backends: terminal (classic),
/// TUI widgets, test harnesses, or silent accumulators.
pub trait RenderSink: Send + Sync {
    /// Push streaming text from the model response.
    fn stream_text(&self, text: &str);
    /// A fenced code block completed during streaming.
    #[allow(dead_code)]
    fn stream_code_block(&self, lang: &str, code: &str);
    /// Model indicated a tool call in the stream.
    fn stream_tool_marker(&self, name: &str);
    /// Streaming response complete — flush any buffered output.
    fn stream_done(&self);
    /// Stream-level error from provider.
    fn stream_error(&self, msg: &str);
    /// A tool execution is starting.
    fn tool_start(&self, name: &str, input: &serde_json::Value);
    /// A tool execution completed (renders the result block).
    fn tool_output(&self, block: &ContentBlock, duration_ms: u64);
    /// A tool was denied by permission system.
    fn tool_denied(&self, name: &str);
    /// Start a spinner (inference waiting indicator).
    fn spinner_start(&self, label: &str);
    /// Stop the spinner.
    fn spinner_stop(&self);
    /// Display a warning message.
    fn warning(&self, message: &str, hint: Option<&str>);
    /// Display an error message.
    fn error(&self, message: &str, hint: Option<&str>);
    /// Print an informational status line (e.g. round separators, compaction notice).
    fn info(&self, message: &str);
    /// Whether this sink suppresses all output (silent mode).
    fn is_silent(&self) -> bool;
    /// Reset the stream renderer state for a new streaming round.
    fn stream_reset(&self);
    /// Get the full accumulated text from the stream renderer (if any).
    fn stream_full_text(&self) -> String;
    /// Display plan progress (step statuses, current step indicator).
    /// Default no-op for backward compatibility.
    fn plan_progress(
        &self,
        _goal: &str,
        _steps: &[cuervo_core::traits::PlanStep],
        _current_step: usize,
    ) {
    }
}

// ---------------------------------------------------------------------------
// ClassicSink — wraps existing render functions for terminal output
// ---------------------------------------------------------------------------

/// Classic terminal renderer — delegates to existing render functions.
///
/// Uses `StreamRenderer` for prose/code streaming, `Spinner` for wait indicators,
/// and `feedback::`/`tool::` functions for structured output. All output goes to
/// stdout (streaming) and stderr (feedback/tools/spinner).
pub struct ClassicSink {
    renderer: Mutex<RefCell<StreamRenderer>>,
    spinner: Mutex<Option<Spinner>>,
}

impl ClassicSink {
    pub fn new() -> Self {
        Self {
            renderer: Mutex::new(RefCell::new(StreamRenderer::new())),
            spinner: Mutex::new(None),
        }
    }
}

impl Default for ClassicSink {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderSink for ClassicSink {
    fn stream_text(&self, text: &str) {
        let guard = self.renderer.lock().unwrap();
        let mut r = guard.borrow_mut();
        let chunk = cuervo_core::types::ModelChunk::TextDelta(text.to_string());
        let _ = r.push(&chunk);
    }

    fn stream_code_block(&self, _lang: &str, _code: &str) {
        // StreamRenderer handles code block detection internally via process_delta,
        // so code blocks flow through stream_text(). This method exists for sinks
        // that need explicit code block notification (e.g. TUI).
    }

    fn stream_tool_marker(&self, name: &str) {
        let guard = self.renderer.lock().unwrap();
        let mut r = guard.borrow_mut();
        let chunk = cuervo_core::types::ModelChunk::ToolUseStart {
            index: 0,
            id: String::new(),
            name: name.to_string(),
        };
        let _ = r.push(&chunk);
    }

    fn stream_done(&self) {
        let guard = self.renderer.lock().unwrap();
        let mut r = guard.borrow_mut();
        let chunk = cuervo_core::types::ModelChunk::Done(cuervo_core::types::StopReason::EndTurn);
        let _ = r.push(&chunk);
    }

    fn stream_error(&self, msg: &str) {
        let guard = self.renderer.lock().unwrap();
        let mut r = guard.borrow_mut();
        let chunk = cuervo_core::types::ModelChunk::Error(msg.to_string());
        let _ = r.push(&chunk);
    }

    fn tool_start(&self, name: &str, input: &serde_json::Value) {
        tool_render::render_tool_start(name, input);
    }

    fn tool_output(&self, block: &ContentBlock, duration_ms: u64) {
        tool_render::render_tool_output(block, duration_ms);
    }

    fn tool_denied(&self, name: &str) {
        tool_render::render_tool_denied(name);
    }

    fn spinner_start(&self, label: &str) {
        let spinner = Spinner::start(label);
        let mut guard = self.spinner.lock().unwrap();
        *guard = Some(spinner);
    }

    fn spinner_stop(&self) {
        let mut guard = self.spinner.lock().unwrap();
        if let Some(ref s) = *guard {
            s.stop();
        }
        *guard = None;
    }

    fn warning(&self, message: &str, hint: Option<&str>) {
        feedback::user_warning(message, hint);
    }

    fn error(&self, message: &str, hint: Option<&str>) {
        feedback::user_error(message, hint);
    }

    fn info(&self, message: &str) {
        eprintln!("{message}");
    }

    fn is_silent(&self) -> bool {
        false
    }

    fn stream_reset(&self) {
        let guard = self.renderer.lock().unwrap();
        *guard.borrow_mut() = StreamRenderer::new();
    }

    fn stream_full_text(&self) -> String {
        let guard = self.renderer.lock().unwrap();
        let r = guard.borrow();
        let text = r.full_text().to_string();
        text
    }

    fn plan_progress(
        &self,
        goal: &str,
        steps: &[cuervo_core::traits::PlanStep],
        current_step: usize,
    ) {
        use cuervo_core::traits::StepOutcome;
        eprintln!("\n  PLAN: {goal}");
        for (i, step) in steps.iter().enumerate() {
            let icon = match &step.outcome {
                Some(StepOutcome::Success { .. }) => "[ok]",
                Some(StepOutcome::Failed { .. }) => "[FAIL]",
                Some(StepOutcome::Skipped { .. }) => "[skip]",
                None if i == current_step => "[>>]",
                None => "[ ]",
            };
            let tool_hint = step
                .tool_name
                .as_deref()
                .map(|t| format!(" ({t})"))
                .unwrap_or_default();
            eprintln!("    {icon} Step {}: {}{tool_hint}", i + 1, step.description);
        }
        eprintln!();
    }
}

// ---------------------------------------------------------------------------
// SilentSink — accumulates text without terminal output
// ---------------------------------------------------------------------------

/// Silent renderer — accumulates text but produces no terminal output.
///
/// Used for background sub-agents, replay mode, and testing.
pub struct SilentSink {
    text: Mutex<String>,
}

impl SilentSink {
    pub fn new() -> Self {
        Self {
            text: Mutex::new(String::new()),
        }
    }

    /// Get the accumulated text.
    #[allow(dead_code)]
    pub fn text(&self) -> String {
        self.text.lock().unwrap().clone()
    }
}

impl Default for SilentSink {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderSink for SilentSink {
    fn stream_text(&self, text: &str) {
        self.text.lock().unwrap().push_str(text);
    }

    fn stream_code_block(&self, _lang: &str, code: &str) {
        self.text.lock().unwrap().push_str(code);
    }

    fn stream_tool_marker(&self, _name: &str) {}
    fn stream_done(&self) {}
    fn stream_error(&self, _msg: &str) {}
    fn tool_start(&self, _name: &str, _input: &serde_json::Value) {}
    fn tool_output(&self, _block: &ContentBlock, _duration_ms: u64) {}
    fn tool_denied(&self, _name: &str) {}
    fn spinner_start(&self, _label: &str) {}
    fn spinner_stop(&self) {}
    fn warning(&self, _message: &str, _hint: Option<&str>) {}
    fn error(&self, _message: &str, _hint: Option<&str>) {}
    fn info(&self, _message: &str) {}

    fn is_silent(&self) -> bool {
        true
    }

    fn stream_reset(&self) {
        self.text.lock().unwrap().clear();
    }

    fn stream_full_text(&self) -> String {
        self.text.lock().unwrap().clone()
    }
}

// ---------------------------------------------------------------------------
// TuiSink — sends UiEvents through a channel to the TUI render loop
// ---------------------------------------------------------------------------

/// TUI renderer — converts all agent output into `UiEvent`s sent through an mpsc channel.
///
/// The TUI render loop receives these events and updates the 3-zone layout.
/// Text accumulation is tracked locally for `stream_full_text()`.
pub struct TuiSink {
    tx: tokio::sync::mpsc::UnboundedSender<crate::tui::events::UiEvent>,
    text: Mutex<String>,
}

impl TuiSink {
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<crate::tui::events::UiEvent>) -> Self {
        Self {
            tx,
            text: Mutex::new(String::new()),
        }
    }

    fn send(&self, event: crate::tui::events::UiEvent) {
        let _ = self.tx.send(event);
    }
}

impl RenderSink for TuiSink {
    fn stream_text(&self, text: &str) {
        self.text.lock().unwrap().push_str(text);
        self.send(crate::tui::events::UiEvent::StreamChunk(text.to_string()));
    }

    fn stream_code_block(&self, lang: &str, code: &str) {
        self.text.lock().unwrap().push_str(code);
        self.send(crate::tui::events::UiEvent::StreamCodeBlock {
            lang: lang.to_string(),
            code: code.to_string(),
        });
    }

    fn stream_tool_marker(&self, name: &str) {
        self.send(crate::tui::events::UiEvent::StreamToolMarker(name.to_string()));
    }

    fn stream_done(&self) {
        self.send(crate::tui::events::UiEvent::StreamDone);
    }

    fn stream_error(&self, msg: &str) {
        self.send(crate::tui::events::UiEvent::StreamError(msg.to_string()));
    }

    fn tool_start(&self, name: &str, input: &serde_json::Value) {
        self.send(crate::tui::events::UiEvent::ToolStart {
            name: name.to_string(),
            input: input.clone(),
        });
    }

    fn tool_output(&self, block: &ContentBlock, duration_ms: u64) {
        let (name, content, is_error) = match block {
            ContentBlock::ToolResult { tool_use_id, content, is_error, .. } => {
                (tool_use_id.clone(), content.clone(), *is_error)
            }
            _ => (String::new(), String::new(), false),
        };
        self.send(crate::tui::events::UiEvent::ToolOutput {
            name,
            content,
            is_error,
            duration_ms,
        });
    }

    fn tool_denied(&self, name: &str) {
        self.send(crate::tui::events::UiEvent::ToolDenied(name.to_string()));
    }

    fn spinner_start(&self, label: &str) {
        self.send(crate::tui::events::UiEvent::SpinnerStart(label.to_string()));
    }

    fn spinner_stop(&self) {
        self.send(crate::tui::events::UiEvent::SpinnerStop);
    }

    fn warning(&self, message: &str, hint: Option<&str>) {
        self.send(crate::tui::events::UiEvent::Warning {
            message: message.to_string(),
            hint: hint.map(|h| h.to_string()),
        });
    }

    fn error(&self, message: &str, hint: Option<&str>) {
        self.send(crate::tui::events::UiEvent::Error {
            message: message.to_string(),
            hint: hint.map(|h| h.to_string()),
        });
    }

    fn info(&self, message: &str) {
        self.send(crate::tui::events::UiEvent::Info(message.to_string()));
    }

    fn is_silent(&self) -> bool {
        false
    }

    fn stream_reset(&self) {
        self.text.lock().unwrap().clear();
    }

    fn stream_full_text(&self) -> String {
        self.text.lock().unwrap().clone()
    }

    fn plan_progress(
        &self,
        goal: &str,
        steps: &[cuervo_core::traits::PlanStep],
        current_step: usize,
    ) {
        use crate::tui::events::{PlanStepDisplayStatus, PlanStepStatus};
        use cuervo_core::traits::StepOutcome;
        let step_statuses: Vec<PlanStepStatus> = steps
            .iter()
            .enumerate()
            .map(|(i, s)| PlanStepStatus {
                description: s.description.clone(),
                tool_name: s.tool_name.clone(),
                status: match &s.outcome {
                    Some(StepOutcome::Success { .. }) => PlanStepDisplayStatus::Succeeded,
                    Some(StepOutcome::Failed { .. }) => PlanStepDisplayStatus::Failed,
                    Some(StepOutcome::Skipped { .. }) => PlanStepDisplayStatus::Skipped,
                    None if i == current_step => PlanStepDisplayStatus::InProgress,
                    None => PlanStepDisplayStatus::Pending,
                },
            })
            .collect();
        self.send(crate::tui::events::UiEvent::PlanProgress {
            goal: goal.to_string(),
            steps: step_statuses,
            current_step,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_sink_no_panic_on_stream() {
        let sink = ClassicSink::new();
        sink.stream_text("hello ");
        sink.stream_text("world");
        let text = sink.stream_full_text();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn classic_sink_no_panic_on_warning() {
        let sink = ClassicSink::new();
        sink.warning("test warning", None);
        sink.warning("with hint", Some("try this"));
    }

    #[test]
    fn classic_sink_no_panic_on_error() {
        let sink = ClassicSink::new();
        sink.error("test error", None);
        sink.error("with hint", Some("check config"));
    }

    #[test]
    fn classic_sink_is_not_silent() {
        let sink = ClassicSink::new();
        assert!(!sink.is_silent());
    }

    #[test]
    fn silent_sink_accumulates_text() {
        let sink = SilentSink::new();
        sink.stream_text("hello ");
        sink.stream_text("world");
        assert_eq!(sink.text(), "hello world");
    }

    #[test]
    fn silent_sink_is_silent() {
        let sink = SilentSink::new();
        assert!(sink.is_silent());
    }

    #[test]
    fn silent_sink_reset_clears_text() {
        let sink = SilentSink::new();
        sink.stream_text("data");
        assert_eq!(sink.stream_full_text(), "data");
        sink.stream_reset();
        assert_eq!(sink.stream_full_text(), "");
    }

    #[test]
    fn classic_sink_stream_reset() {
        let sink = ClassicSink::new();
        sink.stream_text("round 1");
        assert_eq!(sink.stream_full_text(), "round 1");
        sink.stream_reset();
        assert_eq!(sink.stream_full_text(), "");
    }

    #[test]
    fn tui_sink_sends_stream_chunks() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.stream_text("hello ");
        sink.stream_text("world");
        assert_eq!(sink.stream_full_text(), "hello world");
        // Verify events were sent.
        let ev1 = rx.try_recv().unwrap();
        assert!(matches!(ev1, crate::tui::events::UiEvent::StreamChunk(ref s) if s == "hello "));
        let ev2 = rx.try_recv().unwrap();
        assert!(matches!(ev2, crate::tui::events::UiEvent::StreamChunk(ref s) if s == "world"));
    }

    #[test]
    fn tui_sink_sends_warning() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.warning("test", Some("hint"));
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::Warning { ref message, ref hint }
            if message == "test" && hint.as_deref() == Some("hint")));
    }

    #[test]
    fn tui_sink_sends_spinner_events() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.spinner_start("Thinking...");
        sink.spinner_stop();
        let ev1 = rx.try_recv().unwrap();
        assert!(matches!(ev1, crate::tui::events::UiEvent::SpinnerStart(ref s) if s == "Thinking..."));
        let ev2 = rx.try_recv().unwrap();
        assert!(matches!(ev2, crate::tui::events::UiEvent::SpinnerStop));
    }

    #[test]
    fn tui_sink_is_not_silent() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        assert!(!sink.is_silent());
    }

    #[test]
    fn tui_sink_stream_reset() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.stream_text("data");
        assert_eq!(sink.stream_full_text(), "data");
        sink.stream_reset();
        assert_eq!(sink.stream_full_text(), "");
    }

    #[test]
    fn tui_sink_sends_info() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.info("round separator");
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::Info(ref s) if s == "round separator"));
    }

    #[test]
    fn tui_sink_sends_tool_denied() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        sink.tool_denied("bash");
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::ToolDenied(ref s) if s == "bash"));
    }

    #[test]
    fn classic_sink_plan_progress_no_panic() {
        use cuervo_core::traits::PlanStep;
        let sink = ClassicSink::new();
        let steps = vec![
            PlanStep {
                description: "Read file".into(),
                tool_name: Some("file_read".into()),
                parallel: false,
                confidence: 0.9,
                expected_args: None,
                outcome: None,
            },
        ];
        // Should not panic.
        sink.plan_progress("Test goal", &steps, 0);
    }

    #[test]
    fn tui_sink_sends_plan_progress() {
        use cuervo_core::traits::PlanStep;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = TuiSink::new(tx);
        let steps = vec![
            PlanStep {
                description: "Read file".into(),
                tool_name: Some("file_read".into()),
                parallel: false,
                confidence: 0.9,
                expected_args: None,
                outcome: None,
            },
        ];
        sink.plan_progress("Test goal", &steps, 0);
        let ev = rx.try_recv().unwrap();
        assert!(matches!(ev, crate::tui::events::UiEvent::PlanProgress { ref goal, current_step: 0, .. } if goal == "Test goal"));
    }
}
