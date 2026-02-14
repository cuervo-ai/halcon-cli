// Context metrics: wired into agent loop (Phase 42) and exposed via /inspect context.
//! Observability counters for context assembly performance.
//!
//! Provides atomic counters that can be safely shared across the agent loop
//! and inspected for diagnostics, logging, or the `/metrics` command.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

/// Atomic counters for context assembly performance.
#[derive(Debug, Default)]
pub(crate) struct ContextMetrics {
    /// Total number of context assemblies performed.
    pub assemblies: AtomicU64,
    /// Cumulative tokens assembled across all assemblies.
    pub total_tokens_assembled: AtomicU64,
    /// Cumulative assembly wall-clock time in microseconds.
    pub total_assembly_duration_us: AtomicU64,
    /// Total source invocations (one per source per assembly).
    pub source_invocations: AtomicU64,
    /// Number of MCP tool calls routed through the pool.
    pub mcp_tool_calls: AtomicU64,
    /// Number of MCP server reconnection attempts.
    pub mcp_reconnects: AtomicU64,
    /// Number of tools filtered out by ToolSelector.
    pub tools_filtered: AtomicU64,
    /// Number of governance truncations applied.
    pub governance_truncations: AtomicU64,
}

impl ContextMetrics {
    /// Record a completed context assembly.
    pub fn record_assembly(&self, tokens: u32, duration_us: u64) {
        self.assemblies.fetch_add(1, Ordering::Relaxed);
        self.total_tokens_assembled
            .fetch_add(u64::from(tokens), Ordering::Relaxed);
        self.total_assembly_duration_us
            .fetch_add(duration_us, Ordering::Relaxed);
    }

    /// Record tool selection filtering.
    pub fn record_tool_selection(&self, total: usize, selected: usize) {
        if total > selected {
            self.tools_filtered
                .fetch_add((total - selected) as u64, Ordering::Relaxed);
        }
    }

    /// Record an MCP tool call.
    pub fn record_mcp_call(&self) {
        self.mcp_tool_calls.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an MCP reconnection attempt.
    pub fn record_mcp_reconnect(&self) {
        self.mcp_reconnects.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a governance truncation.
    pub fn record_governance_truncation(&self) {
        self.governance_truncations.fetch_add(1, Ordering::Relaxed);
    }

    /// Record source invocations for an assembly round.
    pub fn record_source_invocations(&self, count: u64) {
        self.source_invocations
            .fetch_add(count, Ordering::Relaxed);
    }

    /// Take a point-in-time snapshot of all counters.
    pub fn snapshot(&self) -> ContextMetricsSnapshot {
        ContextMetricsSnapshot {
            assemblies: self.assemblies.load(Ordering::Relaxed),
            total_tokens_assembled: self.total_tokens_assembled.load(Ordering::Relaxed),
            total_assembly_duration_us: self.total_assembly_duration_us.load(Ordering::Relaxed),
            source_invocations: self.source_invocations.load(Ordering::Relaxed),
            mcp_tool_calls: self.mcp_tool_calls.load(Ordering::Relaxed),
            mcp_reconnects: self.mcp_reconnects.load(Ordering::Relaxed),
            tools_filtered: self.tools_filtered.load(Ordering::Relaxed),
            governance_truncations: self.governance_truncations.load(Ordering::Relaxed),
        }
    }
}

/// Copyable point-in-time snapshot for logging/display.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ContextMetricsSnapshot {
    pub assemblies: u64,
    pub total_tokens_assembled: u64,
    pub total_assembly_duration_us: u64,
    pub source_invocations: u64,
    pub mcp_tool_calls: u64,
    pub mcp_reconnects: u64,
    pub tools_filtered: u64,
    pub governance_truncations: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_zero() {
        let m = ContextMetrics::default();
        let s = m.snapshot();
        assert_eq!(s.assemblies, 0);
        assert_eq!(s.total_tokens_assembled, 0);
        assert_eq!(s.total_assembly_duration_us, 0);
        assert_eq!(s.source_invocations, 0);
        assert_eq!(s.mcp_tool_calls, 0);
        assert_eq!(s.tools_filtered, 0);
        assert_eq!(s.governance_truncations, 0);
    }

    #[test]
    fn record_assembly_increments() {
        let m = ContextMetrics::default();
        m.record_assembly(100, 5000);
        m.record_assembly(200, 3000);
        let s = m.snapshot();
        assert_eq!(s.assemblies, 2);
        assert_eq!(s.total_tokens_assembled, 300);
        assert_eq!(s.total_assembly_duration_us, 8000);
    }

    #[test]
    fn record_tool_selection_tracks_filtered() {
        let m = ContextMetrics::default();
        m.record_tool_selection(23, 8);
        let s = m.snapshot();
        assert_eq!(s.tools_filtered, 15);
    }

    #[test]
    fn record_tool_selection_no_filter() {
        let m = ContextMetrics::default();
        m.record_tool_selection(10, 10);
        let s = m.snapshot();
        assert_eq!(s.tools_filtered, 0);
    }

    #[test]
    fn mcp_call_counting() {
        let m = ContextMetrics::default();
        m.record_mcp_call();
        m.record_mcp_call();
        m.record_mcp_reconnect();
        let s = m.snapshot();
        assert_eq!(s.mcp_tool_calls, 2);
        assert_eq!(s.mcp_reconnects, 1);
    }

    #[test]
    fn concurrent_increments() {
        use std::sync::Arc;
        use std::thread;

        let m = Arc::new(ContextMetrics::default());
        let mut handles = Vec::new();
        for _ in 0..10 {
            let m = Arc::clone(&m);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    m.record_assembly(1, 10);
                    m.record_mcp_call();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let s = m.snapshot();
        assert_eq!(s.assemblies, 1000);
        assert_eq!(s.total_tokens_assembled, 1000);
        assert_eq!(s.mcp_tool_calls, 1000);
    }

    #[test]
    fn governance_truncation_counting() {
        let m = ContextMetrics::default();
        m.record_governance_truncation();
        m.record_governance_truncation();
        m.record_governance_truncation();
        let s = m.snapshot();
        assert_eq!(s.governance_truncations, 3);
    }

    #[test]
    fn source_invocation_counting() {
        let m = ContextMetrics::default();
        m.record_source_invocations(5);
        m.record_source_invocations(3);
        let s = m.snapshot();
        assert_eq!(s.source_invocations, 8);
    }

    #[test]
    fn snapshot_is_serializable() {
        let m = ContextMetrics::default();
        m.record_assembly(100, 5000);
        m.record_tool_selection(20, 8);
        let s = m.snapshot();
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"assemblies\":1"));
        assert!(json.contains("\"tools_filtered\":12"));
    }
}
