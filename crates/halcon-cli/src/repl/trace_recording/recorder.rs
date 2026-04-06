//! Unified trace recording — single source of truth for all execution tracing.
//!
//! Xiyo pattern: analytics as leaf service (fire-and-forget, never blocks hot path).

use chrono::Utc;
use halcon_storage::{AsyncDatabase, TraceStep, TraceStepType};
use tokio::sync::mpsc;

/// Events that can be recorded in the trace.
#[derive(Debug, Clone)]
pub enum TraceEvent {
    /// A tool was called (before execution).
    ToolCall {
        session_id: uuid::Uuid,
        tool_use_id: String,
        tool_name: String,
        input: serde_json::Value,
    },
    /// A tool returned a result.
    ToolResult {
        session_id: uuid::Uuid,
        tool_use_id: String,
        tool_name: String,
        content: String,
        is_error: bool,
        duration_ms: u64,
        parallel: bool,
    },
    /// A parallel batch was started.
    ParallelBatch {
        session_id: uuid::Uuid,
        tool_count: usize,
        tool_ids: Vec<String>,
        tool_names: Vec<String>,
    },
}

/// Fire-and-forget trace recorder.
///
/// Sends trace events over an unbounded channel to a background task
/// that writes them to the database. Recording failures are logged
/// but never propagate to the caller.
#[derive(Clone)]
pub struct TraceRecorder {
    tx: mpsc::UnboundedSender<(TraceEvent, u32)>,
}

impl TraceRecorder {
    /// Create a new TraceRecorder backed by the given database.
    ///
    /// Spawns a background task that drains events and writes trace steps.
    pub fn new(db: AsyncDatabase) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<(TraceEvent, u32)>();

        tokio::spawn(async move {
            while let Some((event, step_index)) = rx.recv().await {
                let step = match &event {
                    TraceEvent::ToolCall {
                        session_id,
                        tool_use_id,
                        tool_name,
                        input,
                    } => TraceStep {
                        session_id: *session_id,
                        step_index,
                        step_type: TraceStepType::ToolCall,
                        data_json: serde_json::json!({
                            "tool_use_id": tool_use_id,
                            "tool_name": tool_name,
                            "input": input,
                        })
                        .to_string(),
                        duration_ms: 0,
                        timestamp: Utc::now(),
                    },
                    TraceEvent::ToolResult {
                        session_id,
                        tool_use_id,
                        tool_name,
                        content,
                        is_error,
                        duration_ms,
                        parallel,
                    } => TraceStep {
                        session_id: *session_id,
                        step_index,
                        step_type: TraceStepType::ToolResult,
                        data_json: serde_json::json!({
                            "tool_use_id": tool_use_id,
                            "tool_name": tool_name,
                            "content": content,
                            "is_error": is_error,
                            "duration_ms": duration_ms,
                            "parallel": parallel,
                        })
                        .to_string(),
                        duration_ms: *duration_ms,
                        timestamp: Utc::now(),
                    },
                    TraceEvent::ParallelBatch {
                        session_id,
                        tool_count,
                        tool_ids,
                        tool_names,
                    } => TraceStep {
                        session_id: *session_id,
                        step_index,
                        step_type: TraceStepType::ToolCall,
                        data_json: serde_json::json!({
                            "parallel_batch": true,
                            "tool_count": tool_count,
                            "tool_ids": tool_ids,
                            "tool_names": tool_names,
                        })
                        .to_string(),
                        duration_ms: 0,
                        timestamp: Utc::now(),
                    },
                };

                if let Err(e) = db.inner().append_trace_step(&step) {
                    tracing::warn!("trace recording failed (step {}): {e}", step.step_index);
                }
            }
        });

        Self { tx }
    }

    /// Record a trace event (fire-and-forget).
    ///
    /// If the receiver has been dropped, the event is silently discarded.
    pub fn record(&self, event: TraceEvent, step_index: u32) {
        let _ = self.tx.send((event, step_index));
    }
}
