//! JSON-RPC event sink — emits each `RuntimeEvent` as a newline-delimited JSON
//! record to stdout.
//!
//! # Wire format
//!
//! Each record is a single line of compact JSON (no embedded newlines) followed
//! by `\n`. The VS Code extension reads this stream from the subprocess stdout.
//!
//! ```json
//! {"event_id":"…","timestamp":"…","session_id":"…","type":"round_started","round":1,…}
//! ```
//!
//! The `"type"` field is the serde tag from `RuntimeEventKind` — identical to
//! `RuntimeEvent::type_name()`. TypeScript consumers switch on this field.
//!
//! # Backward compatibility
//!
//! This sink is the **structured replacement** for the legacy `JsonRpcSink` in
//! `halcon-cli/src/commands/json_rpc.rs`. The legacy sink emits opaque
//! `{"event":"token","data":{"text":"…"}}` records. The new sink emits fully
//! typed records. Both can coexist during the Phase 0 → Phase 1 transition via
//! the `MultiSink` composition pattern.

use std::io::Write;
use std::sync::Mutex;

use crate::bus::EventSink;
use crate::event::RuntimeEvent;

/// Writes each `RuntimeEvent` as a compact JSON line to stdout.
///
/// Stdout is protected by a `Mutex` so concurrent async tasks cannot
/// interleave partial lines. The lock is held for the minimum duration
/// (serialize + write + flush — typically < 10µs).
pub struct JsonRpcEventSink {
    stdout: Mutex<std::io::Stdout>,
}

impl JsonRpcEventSink {
    #[must_use]
    pub fn new() -> Self {
        Self {
            stdout: Mutex::new(std::io::stdout()),
        }
    }
}

impl Default for JsonRpcEventSink {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSink for JsonRpcEventSink {
    fn emit(&self, event: &RuntimeEvent) {
        // Serialise outside the lock to minimise contention.
        let json = match serde_json::to_string(event) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    event_type = event.type_name(),
                    "JsonRpcEventSink: serialise failed — skipping event"
                );
                return;
            }
        };

        if let Ok(mut out) = self.stdout.lock() {
            // writeln is atomic for lines ≤ PIPE_BUF (4096 on Linux, 512+ on macOS).
            // Our JSON lines are typically 200–800 bytes, safely within PIPE_BUF.
            let _ = writeln!(out, "{json}");
            let _ = out.flush();
        }
    }
}

/// An alternative to `JsonRpcEventSink` that writes to an in-memory `Vec<u8>`.
///
/// Used exclusively in unit tests to capture emitted events without requiring
/// a real stdout.
pub struct MemoryJsonSink {
    buffer: Mutex<Vec<u8>>,
}

impl MemoryJsonSink {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: Mutex::new(Vec::new()),
        }
    }

    /// Return all emitted lines as a `Vec<String>`.
    pub fn lines(&self) -> Vec<String> {
        let buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        let s = std::str::from_utf8(&buf).unwrap_or("");
        s.lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect()
    }

    /// Parse all emitted lines as `RuntimeEvent` and return them.
    pub fn events(&self) -> Vec<RuntimeEvent> {
        self.lines()
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

impl Default for MemoryJsonSink {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSink for MemoryJsonSink {
    fn emit(&self, event: &RuntimeEvent) {
        if let Ok(json) = serde_json::to_string(event) {
            if let Ok(mut buf) = self.buffer.lock() {
                buf.extend_from_slice(json.as_bytes());
                buf.push(b'\n');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{RuntimeEventKind, ToolBatchKind};
    use uuid::Uuid;

    fn session() -> Uuid {
        Uuid::new_v4()
    }

    #[test]
    fn memory_sink_captures_events() {
        let sink = MemoryJsonSink::new();

        sink.emit(&RuntimeEvent::new(
            session(),
            RuntimeEventKind::RoundStarted {
                round: 3,
                model: "claude-sonnet-4-6".into(),
                tools_allowed: true,
                token_budget_remaining: 7_000,
            },
        ));

        let lines = sink.lines();
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("\"type\":\"round_started\""),
            "line={}",
            lines[0]
        );
        assert!(lines[0].contains("\"round\":3"), "line={}", lines[0]);
    }

    #[test]
    fn memory_sink_parses_events() {
        let sink = MemoryJsonSink::new();
        let session_id = session();

        sink.emit(&RuntimeEvent::new(
            session_id,
            RuntimeEventKind::ToolBatchStarted {
                round: 1,
                batch_kind: ToolBatchKind::Parallel,
                tool_names: vec!["file_read".into(), "bash".into()],
            },
        ));
        sink.emit(&RuntimeEvent::new(
            session_id,
            RuntimeEventKind::ToolBatchCompleted {
                round: 1,
                batch_kind: ToolBatchKind::Parallel,
                success_count: 2,
                failure_count: 0,
                total_duration_ms: 450,
            },
        ));

        let events = sink.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].type_name(), "tool_batch_started");
        assert_eq!(events[1].type_name(), "tool_batch_completed");
    }

    #[test]
    fn memory_sink_roundtrips_complex_event() {
        let sink = MemoryJsonSink::new();
        let session_id = session();
        let edit_id = Uuid::new_v4();

        sink.emit(&RuntimeEvent::new(
            session_id,
            RuntimeEventKind::EditProposed {
                round: 2,
                file_uri: "file:///project/src/lib.rs".into(),
                diff: "--- a/lib.rs\n+++ b/lib.rs\n@@ -1 +1 @@\n-old\n+new".into(),
                original_hash: "sha256:abc".into(),
                edit_id,
            },
        ));

        let events = sink.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].type_name(), "edit_proposed");
        assert_eq!(events[0].session_id, session_id);
    }
}
