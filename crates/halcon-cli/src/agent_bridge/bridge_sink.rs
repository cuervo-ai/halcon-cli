//! BridgeSink — adapts the RenderSink interface to the StreamEmitter protocol.
//!
//! Converts all pipeline callbacks to AgentStreamEvent and forwards them
//! to the StreamEmitter. Does NOT import ratatui or UiEvent.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::Value;
use uuid::Uuid;

use crate::render::sink::RenderSink;
use halcon_core::types::ContentBlock;

use super::traits::StreamEmitter;
use super::types::AgentStreamEvent;
use crate::render::sink::PermissionAwaiter;

/// Sink that converts RenderSink callbacks to AgentStreamEvent for headless streaming.
pub struct BridgeSink {
    emitter: Arc<dyn StreamEmitter>,
    perm_awaiter: Option<PermissionAwaiter>,
    perm_reply_tx: Option<tokio::sync::mpsc::UnboundedSender<halcon_core::types::PermissionDecision>>,
    /// Accumulated text for stream_full_text().
    text: Mutex<String>,
    /// Token sequence counter.
    sequence: AtomicU64,
    /// Thinking char count for ThinkingProgressUpdate throttling.
    thinking_chars: AtomicUsize,
    /// Thinking start time for elapsed calculation.
    thinking_start: Mutex<Option<Instant>>,
    /// Last thinking progress emit time.
    last_thinking_emit: Mutex<Option<Instant>>,
}

impl BridgeSink {
    pub fn new(emitter: Arc<dyn StreamEmitter>) -> Self {
        Self {
            emitter,
            perm_awaiter: None,
            perm_reply_tx: None,
            text: Mutex::new(String::new()),
            sequence: AtomicU64::new(0),
            thinking_chars: AtomicUsize::new(0),
            thinking_start: Mutex::new(None),
            last_thinking_emit: Mutex::new(None),
        }
    }

    pub fn with_permission_awaiter(
        mut self,
        awaiter: PermissionAwaiter,
        reply_tx: tokio::sync::mpsc::UnboundedSender<halcon_core::types::PermissionDecision>,
    ) -> Self {
        self.perm_awaiter = Some(awaiter);
        self.perm_reply_tx = Some(reply_tx);
        self
    }
}

impl RenderSink for BridgeSink {
    fn stream_text(&self, text: &str) {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut t) = self.text.lock() {
            t.push_str(text);
        }
        self.emitter.emit(AgentStreamEvent::OutputToken {
            token: text.to_string(),
            sequence_num: seq,
        });
    }

    fn stream_thinking(&self, text: &str) {
        let chars = self.thinking_chars.fetch_add(text.len(), Ordering::Relaxed) + text.len();

        // Initialize thinking start time on first thinking token.
        {
            let mut start = self.thinking_start.lock().unwrap_or_else(|e| e.into_inner());
            if start.is_none() {
                *start = Some(Instant::now());
            }
        }

        self.emitter.emit(AgentStreamEvent::ThinkingToken {
            token: text.to_string(),
        });

        // Throttle progress updates to every 500ms.
        let should_emit_progress = {
            let mut last = self.last_thinking_emit.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            match *last {
                None => {
                    *last = Some(now);
                    true
                }
                Some(prev) if now.duration_since(prev).as_millis() >= 500 => {
                    *last = Some(now);
                    true
                }
                _ => false,
            }
        };

        if should_emit_progress {
            let elapsed = self
                .thinking_start
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .map(|s| s.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            self.emitter.emit(AgentStreamEvent::ThinkingProgressUpdate {
                chars_so_far: chars,
                elapsed_secs: elapsed,
            });
        }
    }

    fn stream_code_block(&self, _lang: &str, code: &str) {
        self.stream_text(code);
    }

    fn stream_tool_marker(&self, _name: &str) {}

    fn stream_done(&self) {}

    fn stream_error(&self, msg: &str) {
        self.emitter.emit(AgentStreamEvent::TurnFailed {
            error_code: "stream_error".to_string(),
            message: msg.to_string(),
            recoverable: false,
        });
    }

    fn tool_start(&self, name: &str, input: &Value) {
        self.emitter.emit(AgentStreamEvent::ToolStarted {
            name: name.to_string(),
            risk_level: "unknown".to_string(),
            input: input.clone(),
        });
    }

    fn tool_output(&self, _block: &ContentBlock, duration_ms: u64) {
        // Extract tool name from block if possible; use generic marker.
        self.emitter.emit(AgentStreamEvent::ToolCompleted {
            name: "tool".to_string(),
            duration_ms,
            success: true,
        });
    }

    fn tool_denied(&self, name: &str) {
        self.emitter.emit(AgentStreamEvent::ToolCompleted {
            name: name.to_string(),
            duration_ms: 0,
            success: false,
        });
    }

    fn spinner_start(&self, _label: &str) {}
    fn spinner_stop(&self) {}

    fn warning(&self, _message: &str, _hint: Option<&str>) {}
    fn error(&self, _message: &str, _hint: Option<&str>) {}
    fn info(&self, _message: &str) {}

    fn is_silent(&self) -> bool {
        false
    }

    fn stream_reset(&self) {
        if let Ok(mut t) = self.text.lock() {
            t.clear();
        }
        self.sequence.store(0, Ordering::Relaxed);
    }

    fn stream_full_text(&self) -> String {
        self.text
            .lock()
            .map(|t| t.clone())
            .unwrap_or_default()
    }

    fn permission_awaiting(&self, tool: &str, args: &Value, risk_level: &str) {
        use crate::render::sink::timeout_for_risk;
        let timeout_secs = timeout_for_risk(risk_level);
        let request_id = Uuid::new_v4();

        // Build args preview (first 5 keys, truncated values).
        let args_preview: std::collections::HashMap<String, String> = args
            .as_object()
            .map(|obj| {
                obj.iter()
                    .take(5)
                    .map(|(k, v)| {
                        let val = v.as_str().unwrap_or(&v.to_string()).to_string();
                        let val = if val.len() > 80 {
                            format!("{}...", &val[..{ let mut _fcb = (77).min(val.len()); while _fcb > 0 && !val.is_char_boundary(_fcb) { _fcb -= 1; } _fcb }])
                        } else {
                            val
                        };
                        (k.clone(), val)
                    })
                    .collect()
            })
            .unwrap_or_default();

        self.emitter.emit(AgentStreamEvent::PermissionRequested {
            request_id,
            tool_name: tool.to_string(),
            risk_level: risk_level.to_string(),
            args_preview,
            description: format!("Tool '{}' requires {} permission", tool, risk_level),
            deadline_secs: timeout_secs,
        });

        // Invoke the PermissionAwaiter callback so PermissionChecker gets the reply channel.
        if let (Some(awaiter), Some(reply_tx)) = (&self.perm_awaiter, &self.perm_reply_tx) {
            (awaiter)(tool, args, risk_level, timeout_secs, reply_tx.clone());
        }
    }

    fn sub_agent_spawned(&self, step: usize, _total: usize, description: &str, agent_type: &str) {
        self.emitter.emit(AgentStreamEvent::SubAgentStarted {
            sub_agent_id: format!("{agent_type}-{step}"),
            task_description: description.to_string(),
            wave: step,
            allowed_tools: Vec::new(),
        });
    }

    fn sub_agent_completed(
        &self,
        step: usize,
        _total: usize,
        success: bool,
        latency_ms: u64,
        tools_used: &[String],
        _rounds: usize,
        summary: &str,
    ) {
        self.emitter.emit(AgentStreamEvent::SubAgentCompleted {
            sub_agent_id: format!("sub-agent-{step}"),
            success,
            summary: summary.to_string(),
            tools_used: tools_used.to_vec(),
            duration_ms: latency_ms,
        });
    }
}
