//! ToolTelemetry — per-invocation confidence, latency, and success delta.
//!
//! Every tool call produces an [`InvocationRecord`] that feeds:
//! - [`crate::strategy::StrategyLearner`] (UCB1 reward signal)
//! - [`crate::critic::InLoopCritic`] (alignment scoring input)
//! - Observability dashboards (token budget, latency histograms)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

// ─── InvocationRecord ────────────────────────────────────────────────────────

/// A single tool invocation's telemetry record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationRecord {
    /// Unique invocation ID.
    pub id: Uuid,
    /// Name of the tool that was called.
    pub tool_name: String,
    /// The user intent / goal at time of invocation.
    pub intent: String,
    /// Goal confidence *before* this invocation (from GoalVerificationEngine).
    pub pre_confidence: f32,
    /// Goal confidence *after* this invocation. `None` until finalized.
    pub post_confidence: Option<f32>,
    /// `post_confidence - pre_confidence`. Positive ⇒ progress, negative ⇒ regression.
    pub success_delta: Option<f32>,
    /// Wall-clock latency of the tool call.
    pub latency_ms: u64,
    /// Whether the tool returned `is_error: true`.
    pub is_error: bool,
    /// Approximate total tokens consumed (input + output combined).
    pub token_cost: u32,
    /// Session owning this record.
    pub session_id: Uuid,
    /// Agent loop round number.
    pub round: u32,
    /// UTC timestamp of the call.
    pub timestamp: DateTime<Utc>,
}

impl InvocationRecord {
    /// Finalize the record once post-confidence is known.
    pub fn finalize(
        &mut self,
        post_confidence: f32,
        latency_ms: u64,
        is_error: bool,
        token_cost: u32,
    ) {
        self.post_confidence = Some(post_confidence);
        self.success_delta = Some(post_confidence - self.pre_confidence);
        self.latency_ms = latency_ms;
        self.is_error = is_error;
        self.token_cost = token_cost;
    }

    /// Returns `true` if the invocation moved goal confidence up by at least `min_delta`.
    pub fn made_progress(&self, min_delta: f32) -> bool {
        self.success_delta.is_some_and(|d| d >= min_delta)
    }

    /// Latency as a [`Duration`].
    pub fn latency(&self) -> Duration {
        Duration::from_millis(self.latency_ms)
    }
}

// ─── ToolTelemetry ───────────────────────────────────────────────────────────

/// Thread-safe telemetry collector scoped to one agent session.
#[derive(Clone, Debug)]
pub struct ToolTelemetry {
    session_id: Uuid,
    records: Arc<Mutex<Vec<InvocationRecord>>>,
}

impl ToolTelemetry {
    pub fn new(session_id: Uuid) -> Self {
        Self {
            session_id,
            records: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Record the **start** of a tool invocation.
    ///
    /// Returns the new record's [`Uuid`] — use it to finalize via [`record_end`].
    pub fn record_start(
        &self,
        tool_name: impl Into<String>,
        intent: impl Into<String>,
        pre_confidence: f32,
        round: u32,
    ) -> Uuid {
        let id = Uuid::new_v4();
        let record = InvocationRecord {
            id,
            tool_name: tool_name.into(),
            intent: intent.into(),
            pre_confidence,
            post_confidence: None,
            success_delta: None,
            latency_ms: 0,
            is_error: false,
            token_cost: 0,
            session_id: self.session_id,
            round,
            timestamp: Utc::now(),
        };
        self.records.lock().unwrap_or_else(|e| e.into_inner()).push(record);
        id
    }

    /// Finalize a previously started record once the tool call completes.
    pub fn record_end(
        &self,
        id: Uuid,
        post_confidence: f32,
        latency_ms: u64,
        is_error: bool,
        token_cost: u32,
    ) {
        let mut recs = self.records.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(rec) = recs.iter_mut().find(|r| r.id == id) {
            rec.finalize(post_confidence, latency_ms, is_error, token_cost);
        }
    }

    /// All records in this session (cloned).
    pub fn all_records(&self) -> Vec<InvocationRecord> {
        self.records.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Per-tool aggregated statistics.
    pub fn tool_stats(&self) -> HashMap<String, ToolStats> {
        let recs = self.records.lock().unwrap_or_else(|e| e.into_inner());
        let mut stats: HashMap<String, ToolStats> = HashMap::new();
        for r in recs.iter() {
            let s = stats.entry(r.tool_name.clone()).or_default();
            s.calls += 1;
            if r.is_error {
                s.errors += 1;
            }
            s.total_latency_ms += r.latency_ms;
            s.total_tokens += r.token_cost as u64;
            if let Some(delta) = r.success_delta {
                s.total_delta += delta;
                s.delta_samples += 1;
            }
        }
        stats
    }

    /// Average success delta for a given tool across all recorded invocations.
    pub fn avg_delta_for_tool(&self, tool_name: &str) -> Option<f32> {
        let recs = self.records.lock().unwrap_or_else(|e| e.into_inner());
        let deltas: Vec<f32> = recs
            .iter()
            .filter(|r| r.tool_name == tool_name)
            .filter_map(|r| r.success_delta)
            .collect();
        if deltas.is_empty() {
            None
        } else {
            Some(deltas.iter().sum::<f32>() / deltas.len() as f32)
        }
    }

    /// Last N records (most recent first), useful for critic.
    pub fn recent(&self, n: usize) -> Vec<InvocationRecord> {
        let recs = self.records.lock().unwrap_or_else(|e| e.into_inner());
        recs.iter().rev().take(n).cloned().collect()
    }

    /// Total token spend in this session.
    pub fn total_tokens(&self) -> u64 {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.token_cost as u64)
            .sum()
    }
}

// ─── ToolStats ───────────────────────────────────────────────────────────────

/// Aggregated per-tool statistics derived from [`ToolTelemetry`].
#[derive(Debug, Default, Clone)]
pub struct ToolStats {
    pub calls: u32,
    pub errors: u32,
    pub total_latency_ms: u64,
    pub total_tokens: u64,
    pub total_delta: f32,
    pub delta_samples: u32,
}

impl ToolStats {
    pub fn avg_latency_ms(&self) -> u64 {
        if self.calls == 0 {
            0
        } else {
            self.total_latency_ms / self.calls as u64
        }
    }

    pub fn error_rate(&self) -> f32 {
        if self.calls == 0 {
            0.0
        } else {
            self.errors as f32 / self.calls as f32
        }
    }

    pub fn avg_delta(&self) -> f32 {
        if self.delta_samples == 0 {
            0.0
        } else {
            self.total_delta / self.delta_samples as f32
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn telem() -> ToolTelemetry {
        ToolTelemetry::new(Uuid::new_v4())
    }

    #[test]
    fn record_start_creates_pending_entry() {
        let t = telem();
        let id = t.record_start("bash", "run tests", 0.3, 1);
        let recs = t.all_records();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].id, id);
        assert_eq!(recs[0].tool_name, "bash");
        assert_eq!(recs[0].pre_confidence, 0.3);
        assert!(recs[0].post_confidence.is_none());
    }

    #[test]
    fn record_end_finalizes_delta() {
        let t = telem();
        let id = t.record_start("bash", "run tests", 0.3, 1);
        t.record_end(id, 0.7, 250, false, 100);
        let recs = t.all_records();
        let delta = recs[0].success_delta.unwrap();
        assert!((delta - 0.4).abs() < 1e-6, "expected ~0.4, got {}", delta);
        assert_eq!(recs[0].latency_ms, 250);
        assert!(!recs[0].is_error);
    }

    #[test]
    fn made_progress_threshold() {
        let t = telem();
        let id = t.record_start("grep", "search", 0.5, 2);
        t.record_end(id, 0.6, 100, false, 50);
        let recs = t.all_records();
        assert!(recs[0].made_progress(0.05));
        assert!(!recs[0].made_progress(0.2)); // 0.1 < 0.2
    }

    #[test]
    fn tool_stats_aggregates_multiple() {
        let t = telem();
        let id1 = t.record_start("bash", "test", 0.0, 1);
        t.record_end(id1, 0.5, 100, false, 50);
        let id2 = t.record_start("bash", "test", 0.5, 2);
        t.record_end(id2, 0.8, 200, true, 60);
        let stats = t.tool_stats();
        let bash = &stats["bash"];
        assert_eq!(bash.calls, 2);
        assert_eq!(bash.errors, 1);
        assert_eq!(bash.error_rate(), 0.5);
    }

    #[test]
    fn avg_delta_for_tool_correct() {
        let t = telem();
        let id1 = t.record_start("file_read", "read", 0.0, 1);
        t.record_end(id1, 0.4, 50, false, 20);
        let id2 = t.record_start("file_read", "read", 0.4, 2);
        t.record_end(id2, 0.6, 50, false, 20);
        // deltas: 0.4, 0.2 → avg 0.3
        let avg = t.avg_delta_for_tool("file_read").unwrap();
        assert!((avg - 0.3).abs() < 1e-6, "avg={}", avg);
    }

    #[test]
    fn total_tokens_sums_all() {
        let t = telem();
        let id1 = t.record_start("bash", "x", 0.0, 1);
        t.record_end(id1, 0.1, 10, false, 100);
        let id2 = t.record_start("grep", "x", 0.1, 2);
        t.record_end(id2, 0.2, 10, false, 200);
        assert_eq!(t.total_tokens(), 300);
    }
}
