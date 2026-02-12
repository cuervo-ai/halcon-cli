//! Lightweight observability types using W3C Trace Context format.
//!
//! Custom types (no opentelemetry SDK dependency) that are compatible
//! with the W3C traceparent header format for future integration.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// W3C-compatible trace context for correlating spans across the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceContext {
    /// 32 hex chars (128-bit trace identifier).
    pub trace_id: String,
    /// 16 hex chars (64-bit span identifier).
    pub span_id: String,
    /// Parent span ID (None for root spans).
    pub parent_span_id: Option<String>,
    /// Trace flags (0x01 = sampled).
    pub trace_flags: u8,
}

impl TraceContext {
    /// Create a new root trace context with random IDs.
    pub fn new_root() -> Self {
        let trace_id = hex::encode(uuid::Uuid::new_v4().as_bytes());
        let span_id = hex::encode(&uuid::Uuid::new_v4().as_bytes()[..8]);
        Self {
            trace_id,
            span_id,
            parent_span_id: None,
            trace_flags: 0x01, // sampled
        }
    }

    /// Create a child span inheriting the trace_id.
    pub fn child(&self) -> Self {
        let span_id = hex::encode(&uuid::Uuid::new_v4().as_bytes()[..8]);
        Self {
            trace_id: self.trace_id.clone(),
            span_id,
            parent_span_id: Some(self.span_id.clone()),
            trace_flags: self.trace_flags,
        }
    }

    /// Create a deterministic root context from a seed (for replay).
    pub fn deterministic_root(seed: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(seed.as_bytes());
        hasher.update(b"trace");
        let hash = hasher.finalize();
        let trace_id = hex::encode(&hash[..16]);
        let span_id = hex::encode(&hash[16..24]);
        Self {
            trace_id,
            span_id,
            parent_span_id: None,
            trace_flags: 0x01,
        }
    }

    /// Format as a W3C traceparent header value.
    /// Format: `{version}-{trace_id}-{span_id}-{flags}`
    pub fn traceparent(&self) -> String {
        format!(
            "00-{}-{}-{:02x}",
            self.trace_id, self.span_id, self.trace_flags
        )
    }
}

/// A lightweight observability span for timing and attribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilitySpan {
    /// Human-readable span name (e.g., "agent_loop", "tool_bash").
    pub name: String,
    /// Trace context for this span.
    pub context: TraceContext,
    /// When the span started.
    pub start_time: DateTime<Utc>,
    /// When the span ended (None if still running).
    pub end_time: Option<DateTime<Utc>>,
    /// Duration in milliseconds (computed on end).
    pub duration_ms: Option<u64>,
    /// Arbitrary key-value attributes.
    pub attributes: serde_json::Map<String, serde_json::Value>,
    /// Status: "ok", "error", or "unset".
    pub status: String,
}

impl ObservabilitySpan {
    /// Start a new span.
    pub fn start(name: impl Into<String>, context: TraceContext, now: DateTime<Utc>) -> Self {
        Self {
            name: name.into(),
            context,
            start_time: now,
            end_time: None,
            duration_ms: None,
            attributes: serde_json::Map::new(),
            status: "unset".to_string(),
        }
    }

    /// Set an attribute on this span.
    pub fn set_attribute(&mut self, key: impl Into<String>, value: serde_json::Value) {
        self.attributes.insert(key.into(), value);
    }

    /// End the span, recording duration and status.
    pub fn end(&mut self, now: DateTime<Utc>, status: impl Into<String>) {
        self.end_time = Some(now);
        self.duration_ms = Some(
            (now - self.start_time)
                .num_milliseconds()
                .max(0) as u64,
        );
        self.status = status.into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_context_new_root() {
        let ctx = TraceContext::new_root();
        assert_eq!(ctx.trace_id.len(), 32);
        assert_eq!(ctx.span_id.len(), 16);
        assert!(ctx.parent_span_id.is_none());
        assert_eq!(ctx.trace_flags, 0x01);
    }

    #[test]
    fn trace_context_child_inherits_trace_id() {
        let root = TraceContext::new_root();
        let child = root.child();
        assert_eq!(child.trace_id, root.trace_id);
    }

    #[test]
    fn trace_context_child_new_span_id() {
        let root = TraceContext::new_root();
        let child = root.child();
        assert_ne!(child.span_id, root.span_id);
        assert_eq!(child.parent_span_id.as_deref(), Some(root.span_id.as_str()));
    }

    #[test]
    fn trace_context_deterministic_same_seed() {
        let a = TraceContext::deterministic_root("my-seed");
        let b = TraceContext::deterministic_root("my-seed");
        assert_eq!(a.trace_id, b.trace_id);
        assert_eq!(a.span_id, b.span_id);
    }

    #[test]
    fn trace_context_deterministic_different_seed() {
        let a = TraceContext::deterministic_root("seed-a");
        let b = TraceContext::deterministic_root("seed-b");
        assert_ne!(a.trace_id, b.trace_id);
    }

    #[test]
    fn trace_context_traceparent_format() {
        let ctx = TraceContext::deterministic_root("test");
        let tp = ctx.traceparent();
        let parts: Vec<&str> = tp.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "00"); // version
        assert_eq!(parts[1].len(), 32); // trace_id
        assert_eq!(parts[2].len(), 16); // span_id
        assert_eq!(parts[3], "01"); // flags
    }

    #[test]
    fn span_start_end() {
        let ctx = TraceContext::new_root();
        let start = Utc::now();
        let mut span = ObservabilitySpan::start("test_span", ctx, start);
        assert_eq!(span.name, "test_span");
        assert_eq!(span.status, "unset");
        assert!(span.end_time.is_none());

        let end = start + chrono::TimeDelta::milliseconds(42);
        span.end(end, "ok");
        assert_eq!(span.status, "ok");
        assert_eq!(span.duration_ms, Some(42));
        assert!(span.end_time.is_some());
    }

    #[test]
    fn span_attributes() {
        let ctx = TraceContext::new_root();
        let mut span = ObservabilitySpan::start("attr_span", ctx, Utc::now());
        span.set_attribute("model", serde_json::json!("claude-sonnet"));
        span.set_attribute("round", serde_json::json!(3));
        assert_eq!(span.attributes.len(), 2);
        assert_eq!(span.attributes["model"], "claude-sonnet");
    }

    #[test]
    fn span_duration_calculation() {
        let ctx = TraceContext::new_root();
        let start = Utc::now();
        let mut span = ObservabilitySpan::start("dur", ctx, start);
        let end = start + chrono::TimeDelta::milliseconds(100);
        span.end(end, "ok");
        assert_eq!(span.duration_ms, Some(100));
    }

    #[test]
    fn span_child_hierarchy() {
        let root_ctx = TraceContext::new_root();
        let child_ctx = root_ctx.child();
        let grandchild_ctx = child_ctx.child();

        // All share the same trace_id.
        assert_eq!(root_ctx.trace_id, child_ctx.trace_id);
        assert_eq!(root_ctx.trace_id, grandchild_ctx.trace_id);

        // Parent chain is correct.
        assert_eq!(
            grandchild_ctx.parent_span_id.as_deref(),
            Some(child_ctx.span_id.as_str())
        );
    }

    #[test]
    fn trace_context_serde_roundtrip() {
        let ctx = TraceContext::new_root();
        let json = serde_json::to_string(&ctx).unwrap();
        let parsed: TraceContext = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.trace_id, ctx.trace_id);
        assert_eq!(parsed.span_id, ctx.span_id);
    }

    #[test]
    fn span_json_serializable() {
        let ctx = TraceContext::new_root();
        let mut span = ObservabilitySpan::start("s", ctx, Utc::now());
        span.set_attribute("key", serde_json::json!("val"));
        span.end(Utc::now(), "ok");
        let json = serde_json::to_string(&span).unwrap();
        assert!(json.contains("\"name\":\"s\""));
        assert!(json.contains("\"status\":\"ok\""));
    }

    #[test]
    fn trace_context_in_replay() {
        // Deterministic root should produce valid traceparent.
        let ctx = TraceContext::deterministic_root("replay-seed-123");
        let tp = ctx.traceparent();
        assert!(tp.starts_with("00-"));
        assert!(tp.ends_with("-01"));
    }
}
