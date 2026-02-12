//! UI event protocol for TUI rendering.
//!
//! These events flow from the agent loop (via `TuiSink`) to the TUI render
//! loop over an mpsc channel, decoupling business logic from display.

use serde_json::Value;

/// Events sent from the agent loop to the TUI render loop.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum UiEvent {
    /// Incremental text from the streaming model response.
    StreamChunk(String),
    /// A fenced code block completed (language, full code).
    StreamCodeBlock { lang: String, code: String },
    /// Model indicated a tool call is coming (marker in stream).
    StreamToolMarker(String),
    /// Streaming response complete for this round.
    StreamDone,
    /// Stream-level error from provider.
    StreamError(String),
    /// A tool execution is starting.
    ToolStart { name: String, input: Value },
    /// A tool execution completed.
    ToolOutput {
        name: String,
        content: String,
        is_error: bool,
        duration_ms: u64,
    },
    /// A tool was denied by the user or permission system.
    ToolDenied(String),
    /// Spinner should start (inference waiting).
    SpinnerStart(String),
    /// Spinner should stop.
    SpinnerStop,
    /// A warning message for display.
    Warning {
        message: String,
        hint: Option<String>,
    },
    /// An error message for display.
    Error {
        message: String,
        hint: Option<String>,
    },
    /// An informational status line (round separators, compaction notices, etc.).
    Info(String),
    /// Status bar update (provider, model, tokens, cost, etc.).
    StatusUpdate {
        provider: Option<String>,
        model: Option<String>,
        round: Option<usize>,
        tokens: Option<u64>,
        cost: Option<f64>,
        session_id: Option<String>,
        elapsed_ms: Option<u64>,
        tool_count: Option<u32>,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
    },
    /// A new agent round is starting.
    RoundStart(usize),
    /// An agent round has completed.
    RoundEnd(usize),
    /// Force a redraw of the TUI.
    Redraw,
    /// The agent loop has finished — TUI should show prompt again.
    AgentDone,
    /// Request to quit the TUI application.
    Quit,
    /// Plan progress update — shows/updates the plan overview in the activity zone.
    PlanProgress {
        goal: String,
        steps: Vec<PlanStepStatus>,
        current_step: usize,
    },
}

/// Display status for a single plan step in the TUI.
#[derive(Debug, Clone)]
pub struct PlanStepStatus {
    pub description: String,
    pub tool_name: Option<String>,
    pub status: PlanStepDisplayStatus,
}

/// Visual state of a plan step.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanStepDisplayStatus {
    Pending,
    InProgress,
    Succeeded,
    Failed,
    Skipped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_chunk_construction() {
        let ev = UiEvent::StreamChunk("hello".into());
        assert!(matches!(ev, UiEvent::StreamChunk(ref s) if s == "hello"));
    }

    #[test]
    fn stream_code_block_construction() {
        let ev = UiEvent::StreamCodeBlock {
            lang: "rust".into(),
            code: "fn main() {}".into(),
        };
        assert!(matches!(ev, UiEvent::StreamCodeBlock { ref lang, .. } if lang == "rust"));
    }

    #[test]
    fn tool_start_construction() {
        let ev = UiEvent::ToolStart {
            name: "file_read".into(),
            input: serde_json::json!({"path": "test.rs"}),
        };
        assert!(matches!(ev, UiEvent::ToolStart { ref name, .. } if name == "file_read"));
    }

    #[test]
    fn tool_output_construction() {
        let ev = UiEvent::ToolOutput {
            name: "bash".into(),
            content: "output".into(),
            is_error: false,
            duration_ms: 42,
        };
        assert!(matches!(ev, UiEvent::ToolOutput { duration_ms: 42, .. }));
    }

    #[test]
    fn warning_with_hint() {
        let ev = UiEvent::Warning {
            message: "something".into(),
            hint: Some("try this".into()),
        };
        assert!(matches!(ev, UiEvent::Warning { hint: Some(_), .. }));
    }

    #[test]
    fn info_construction() {
        let ev = UiEvent::Info("round separator".into());
        assert!(matches!(ev, UiEvent::Info(ref s) if s == "round separator"));
    }

    #[test]
    fn status_update_partial() {
        let ev = UiEvent::StatusUpdate {
            provider: Some("anthropic".into()),
            model: None,
            round: Some(1),
            tokens: None,
            cost: None,
            session_id: Some("abc12345".into()),
            elapsed_ms: Some(1500),
            tool_count: Some(3),
            input_tokens: Some(1200),
            output_tokens: Some(450),
        };
        assert!(matches!(ev, UiEvent::StatusUpdate { round: Some(1), .. }));
    }

    #[test]
    fn plan_progress_construction() {
        let ev = UiEvent::PlanProgress {
            goal: "Fix bug".into(),
            steps: vec![
                PlanStepStatus {
                    description: "Read file".into(),
                    tool_name: Some("file_read".into()),
                    status: PlanStepDisplayStatus::Succeeded,
                },
                PlanStepStatus {
                    description: "Edit file".into(),
                    tool_name: Some("file_edit".into()),
                    status: PlanStepDisplayStatus::InProgress,
                },
            ],
            current_step: 1,
        };
        assert!(matches!(ev, UiEvent::PlanProgress { current_step: 1, .. }));
    }

    #[test]
    fn plan_step_display_status_eq() {
        assert_eq!(PlanStepDisplayStatus::Pending, PlanStepDisplayStatus::Pending);
        assert_ne!(PlanStepDisplayStatus::Succeeded, PlanStepDisplayStatus::Failed);
    }
}
