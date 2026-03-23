//! Metrics sink — accumulates runtime diagnostics from the event stream.
//!
//! `MetricsSink` implements `EventSink` and builds a live `DiagnosticsSnapshot`
//! that is queryable at any point during or after a session.
//!
//! # Collected metrics
//!
//! | Metric                      | Source event(s)                        |
//! |-----------------------------|----------------------------------------|
//! | Per-round latency (ms)      | `RoundCompleted`                       |
//! | Tool call success rate      | `ToolCallCompleted`                    |
//! | Tool call count by name     | `ToolCallCompleted`                    |
//! | Model provider latency      | `ModelResponseCompleted`               |
//! | Planning vs execution ratio | `PlanCreated` + `RoundCompleted`       |
//! | Budget consumption          | `BudgetWarning`, `BudgetExhausted`     |
//! | Replan count                | `PlanReplanned`                        |
//!
//! # Thread-safety
//!
//! The inner `DiagnosticsStore` is behind `Arc<Mutex<_>>` so `MetricsSink` can
//! be cloned and shared freely — `snapshot()` takes a read lock and returns
//! owned data, safe to call from any thread.
//!
//! # Example
//!
//! ```rust
//! use halcon_runtime_events::sinks::MetricsSink;
//! use halcon_runtime_events::bus::EventSink;
//! use halcon_runtime_events::event::{RuntimeEvent, RuntimeEventKind};
//! use uuid::Uuid;
//!
//! let sink = MetricsSink::new();
//! let session = Uuid::new_v4();
//!
//! sink.emit(&RuntimeEvent::new(session, RuntimeEventKind::RoundCompleted {
//!     round: 1,
//!     action: halcon_runtime_events::ConvergenceAction::Continue,
//!     fsm_phase: "execute".into(),
//!     duration_ms: 1200,
//! }));
//!
//! let snap = sink.snapshot();
//! assert_eq!(snap.total_rounds, 1);
//! assert_eq!(snap.avg_round_latency_ms, 1200.0);
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::bus::EventSink;
use crate::event::{RuntimeEvent, RuntimeEventKind};

// ─── DiagnosticsSnapshot ─────────────────────────────────────────────────────

/// A point-in-time snapshot of all accumulated metrics for one session.
///
/// All fields are derived from events observed by `MetricsSink`. Fields
/// default to zero/empty until the relevant events arrive.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticsSnapshot {
    // ── Round metrics ─────────────────────────────────────────────────────────
    /// Number of rounds completed (`RoundCompleted` events seen).
    pub total_rounds: usize,
    /// Latency (ms) for each completed round, index 0 = round 1.
    pub round_latencies_ms: Vec<u64>,
    /// Average round latency in milliseconds (0.0 if no rounds completed).
    pub avg_round_latency_ms: f64,
    /// Maximum round latency observed.
    pub max_round_latency_ms: u64,

    // ── Tool metrics ──────────────────────────────────────────────────────────
    /// Total individual tool calls completed.
    pub total_tool_calls: usize,
    /// Number of successful tool calls.
    pub tool_calls_succeeded: usize,
    /// Number of failed tool calls.
    pub tool_calls_failed: usize,
    /// Tool call success rate in [0.0, 1.0]. `None` if no calls yet.
    pub tool_success_rate: Option<f64>,
    /// Per-tool-name call counts (name → total calls).
    pub tool_call_counts: HashMap<String, usize>,
    /// Per-tool-name failure counts (name → failures).
    pub tool_failure_counts: HashMap<String, usize>,
    /// Number of tool calls that were blocked.
    pub tool_calls_blocked: usize,

    // ── Model provider metrics ────────────────────────────────────────────────
    /// Total model responses completed.
    pub total_model_responses: usize,
    /// Latency (ms) for each model response.
    pub model_response_latencies_ms: Vec<u64>,
    /// Average model response latency in milliseconds.
    pub avg_model_latency_ms: f64,
    /// Per-provider call counts.
    pub provider_call_counts: HashMap<String, usize>,
    /// Per-provider total latency (ms) — divide by call count for per-provider avg.
    pub provider_total_latency_ms: HashMap<String, u64>,

    // ── Planning vs execution ─────────────────────────────────────────────────
    /// Number of plans created (including replans).
    pub plans_created: usize,
    /// Number of times the planner replanned.
    pub replan_count: u32,
    /// Rounds where the FSM phase was "plan" (planning rounds).
    pub planning_rounds: usize,
    /// Rounds where the FSM phase was "execute" (execution rounds).
    pub execution_rounds: usize,
    /// Ratio of planning rounds to total rounds (0.0–1.0). `None` if no rounds.
    pub planning_ratio: Option<f64>,

    // ── Budget ────────────────────────────────────────────────────────────────
    /// Number of budget warnings received.
    pub budget_warnings: usize,
    /// Whether budget was fully exhausted.
    pub budget_exhausted: bool,
    /// Total tokens used (from `SessionEnded` or `ModelResponseCompleted` sum).
    pub total_tokens_used: u64,
    /// Estimated cost in USD (`SessionEnded`).
    pub estimated_cost_usd: f64,
}

impl DiagnosticsSnapshot {
    /// Average model response latency for a specific provider. Returns `None`
    /// if the provider has no recorded calls.
    #[must_use]
    pub fn avg_provider_latency_ms(&self, provider: &str) -> Option<f64> {
        let count = *self.provider_call_counts.get(provider)? as f64;
        if count == 0.0 {
            return None;
        }
        let total = *self.provider_total_latency_ms.get(provider).unwrap_or(&0) as f64;
        Some(total / count)
    }
}

// ─── DiagnosticsStore (internal mutable state) ────────────────────────────────

#[derive(Debug, Default)]
struct DiagnosticsStore {
    // Rounds
    round_latencies: Vec<u64>,
    planning_rounds: usize,
    execution_rounds: usize,

    // Tools
    tool_calls_succeeded: usize,
    tool_calls_failed: usize,
    tool_calls_blocked: usize,
    tool_call_counts: HashMap<String, usize>,
    tool_failure_counts: HashMap<String, usize>,

    // Model
    model_latencies: Vec<u64>,
    provider_call_counts: HashMap<String, usize>,
    provider_total_latency: HashMap<String, u64>,
    total_tokens: u64,
    estimated_cost_usd: f64,

    // Planning
    plans_created: usize,
    replan_count: u32,

    // Budget
    budget_warnings: usize,
    budget_exhausted: bool,
}

impl DiagnosticsStore {
    fn handle(&mut self, event: &RuntimeEvent) {
        match &event.kind {
            // ── Rounds ──────────────────────────────────────────────────────
            RuntimeEventKind::RoundCompleted {
                duration_ms,
                fsm_phase,
                ..
            } => {
                self.round_latencies.push(*duration_ms);
                let phase = fsm_phase.to_lowercase();
                if phase.contains("plan") {
                    self.planning_rounds += 1;
                } else {
                    self.execution_rounds += 1;
                }
            }

            // ── Tools ────────────────────────────────────────────────────────
            RuntimeEventKind::ToolCallCompleted {
                tool_name, success, ..
            } => {
                *self.tool_call_counts.entry(tool_name.clone()).or_default() += 1;
                if *success {
                    self.tool_calls_succeeded += 1;
                } else {
                    self.tool_calls_failed += 1;
                    *self
                        .tool_failure_counts
                        .entry(tool_name.clone())
                        .or_default() += 1;
                }
            }
            RuntimeEventKind::ToolBlocked { .. } => {
                self.tool_calls_blocked += 1;
            }

            // ── Model ────────────────────────────────────────────────────────
            RuntimeEventKind::ModelResponseCompleted {
                latency_ms,
                provider,
                ..
            } => {
                self.model_latencies.push(*latency_ms);
                *self
                    .provider_call_counts
                    .entry(provider.clone())
                    .or_default() += 1;
                *self
                    .provider_total_latency
                    .entry(provider.clone())
                    .or_default() += latency_ms;
            }

            // ── Planning ────────────────────────────────────────────────────
            RuntimeEventKind::PlanCreated { .. } => {
                self.plans_created += 1;
            }
            RuntimeEventKind::PlanReplanned { replan_count, .. } => {
                self.replan_count = *replan_count;
            }

            // ── Budget ───────────────────────────────────────────────────────
            RuntimeEventKind::BudgetWarning { .. } => {
                self.budget_warnings += 1;
            }
            RuntimeEventKind::BudgetExhausted { tokens_used, .. } => {
                self.budget_exhausted = true;
                self.total_tokens = (*tokens_used).max(self.total_tokens);
            }
            RuntimeEventKind::SessionEnded {
                total_tokens,
                estimated_cost_usd,
                ..
            } => {
                self.total_tokens = *total_tokens;
                self.estimated_cost_usd = *estimated_cost_usd;
            }

            _ => {}
        }
    }

    fn snapshot(&self) -> DiagnosticsSnapshot {
        let total_rounds = self.round_latencies.len();
        let avg_round = if total_rounds > 0 {
            self.round_latencies.iter().sum::<u64>() as f64 / total_rounds as f64
        } else {
            0.0
        };
        let max_round = self.round_latencies.iter().copied().max().unwrap_or(0);

        let total_tool_calls = self.tool_calls_succeeded + self.tool_calls_failed;
        let tool_success_rate = if total_tool_calls > 0 {
            Some(self.tool_calls_succeeded as f64 / total_tool_calls as f64)
        } else {
            None
        };

        let total_model = self.model_latencies.len();
        let avg_model = if total_model > 0 {
            self.model_latencies.iter().sum::<u64>() as f64 / total_model as f64
        } else {
            0.0
        };

        let planning_ratio = if total_rounds > 0 {
            Some(self.planning_rounds as f64 / total_rounds as f64)
        } else {
            None
        };

        DiagnosticsSnapshot {
            total_rounds,
            round_latencies_ms: self.round_latencies.clone(),
            avg_round_latency_ms: avg_round,
            max_round_latency_ms: max_round,

            total_tool_calls,
            tool_calls_succeeded: self.tool_calls_succeeded,
            tool_calls_failed: self.tool_calls_failed,
            tool_success_rate,
            tool_call_counts: self.tool_call_counts.clone(),
            tool_failure_counts: self.tool_failure_counts.clone(),
            tool_calls_blocked: self.tool_calls_blocked,

            total_model_responses: total_model,
            model_response_latencies_ms: self.model_latencies.clone(),
            avg_model_latency_ms: avg_model,
            provider_call_counts: self.provider_call_counts.clone(),
            provider_total_latency_ms: self.provider_total_latency.clone(),

            plans_created: self.plans_created,
            replan_count: self.replan_count,
            planning_rounds: self.planning_rounds,
            execution_rounds: self.execution_rounds,
            planning_ratio,

            budget_warnings: self.budget_warnings,
            budget_exhausted: self.budget_exhausted,
            total_tokens_used: self.total_tokens,
            estimated_cost_usd: self.estimated_cost_usd,
        }
    }
}

// ─── MetricsSink ─────────────────────────────────────────────────────────────

/// `EventSink` that accumulates runtime diagnostics for observability.
///
/// Call `MetricsSink::snapshot()` at any time to read a point-in-time copy of
/// all accumulated metrics without interrupting the event stream.
#[derive(Clone)]
pub struct MetricsSink {
    store: Arc<Mutex<DiagnosticsStore>>,
}

impl std::fmt::Debug for MetricsSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsSink").finish()
    }
}

impl MetricsSink {
    /// Create a new empty metrics sink.
    #[must_use]
    pub fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(DiagnosticsStore::default())),
        }
    }

    /// Return a point-in-time snapshot of all accumulated diagnostics.
    ///
    /// This call is O(n) in the number of distinct tool names and providers, but
    /// O(1) in the number of events (counters are maintained incrementally).
    #[must_use]
    pub fn snapshot(&self) -> DiagnosticsSnapshot {
        self.store
            .lock()
            .expect("MetricsSink lock poisoned")
            .snapshot()
    }

    /// Reset all accumulated metrics (useful between sessions in long-running processes).
    pub fn reset(&self) {
        *self.store.lock().unwrap_or_else(|e| e.into_inner()) = DiagnosticsStore::default();
    }
}

impl Default for MetricsSink {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSink for MetricsSink {
    fn emit(&self, event: &RuntimeEvent) {
        if let Ok(mut store) = self.store.lock() {
            store.handle(event);
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{
        BudgetExhaustionReason, ConvergenceAction, RuntimeEventKind, ToolBatchKind, ToolBlockReason,
    };
    use uuid::Uuid;

    fn session() -> Uuid {
        Uuid::new_v4()
    }

    fn round_completed(round: usize, ms: u64, phase: &str) -> RuntimeEventKind {
        RuntimeEventKind::RoundCompleted {
            round,
            action: ConvergenceAction::Continue,
            fsm_phase: phase.into(),
            duration_ms: ms,
        }
    }

    fn tool_completed(name: &str, success: bool, ms: u64) -> RuntimeEventKind {
        RuntimeEventKind::ToolCallCompleted {
            round: 1,
            tool_use_id: Uuid::new_v4().to_string(),
            tool_name: name.into(),
            success,
            duration_ms: ms,
            output_preview: String::new(),
            output_tokens: 0,
        }
    }

    #[test]
    fn empty_snapshot_has_zero_defaults() {
        let sink = MetricsSink::new();
        let snap = sink.snapshot();
        assert_eq!(snap.total_rounds, 0);
        assert_eq!(snap.total_tool_calls, 0);
        assert!(snap.tool_success_rate.is_none());
        assert!(snap.planning_ratio.is_none());
        assert_eq!(snap.avg_round_latency_ms, 0.0);
    }

    #[test]
    fn round_latency_is_accumulated() {
        let sink = MetricsSink::new();
        let s = session();
        sink.emit(&RuntimeEvent::new(s, round_completed(1, 500, "execute")));
        sink.emit(&RuntimeEvent::new(s, round_completed(2, 1500, "execute")));

        let snap = sink.snapshot();
        assert_eq!(snap.total_rounds, 2);
        assert_eq!(snap.avg_round_latency_ms, 1000.0);
        assert_eq!(snap.max_round_latency_ms, 1500);
    }

    #[test]
    fn tool_success_rate_computed_correctly() {
        let sink = MetricsSink::new();
        let s = session();
        sink.emit(&RuntimeEvent::new(s, tool_completed("bash", true, 100)));
        sink.emit(&RuntimeEvent::new(s, tool_completed("bash", true, 200)));
        sink.emit(&RuntimeEvent::new(
            s,
            tool_completed("file_read", false, 50),
        ));

        let snap = sink.snapshot();
        assert_eq!(snap.total_tool_calls, 3);
        assert_eq!(snap.tool_calls_succeeded, 2);
        assert_eq!(snap.tool_calls_failed, 1);
        let rate = snap.tool_success_rate.unwrap();
        assert!((rate - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn per_tool_call_counts() {
        let sink = MetricsSink::new();
        let s = session();
        sink.emit(&RuntimeEvent::new(s, tool_completed("bash", true, 100)));
        sink.emit(&RuntimeEvent::new(s, tool_completed("bash", false, 50)));
        sink.emit(&RuntimeEvent::new(s, tool_completed("file_read", true, 30)));

        let snap = sink.snapshot();
        assert_eq!(*snap.tool_call_counts.get("bash").unwrap(), 2);
        assert_eq!(*snap.tool_call_counts.get("file_read").unwrap(), 1);
        assert_eq!(*snap.tool_failure_counts.get("bash").unwrap(), 1);
        assert!(snap.tool_failure_counts.get("file_read").is_none());
    }

    #[test]
    fn model_provider_latency() {
        let sink = MetricsSink::new();
        let s = session();
        sink.emit(&RuntimeEvent::new(
            s,
            RuntimeEventKind::ModelResponseCompleted {
                round: 1,
                input_tokens: 100,
                output_tokens: 50,
                latency_ms: 800,
                model: "claude-sonnet-4-6".into(),
                provider: "anthropic".into(),
            },
        ));
        sink.emit(&RuntimeEvent::new(
            s,
            RuntimeEventKind::ModelResponseCompleted {
                round: 2,
                input_tokens: 200,
                output_tokens: 80,
                latency_ms: 1200,
                model: "claude-sonnet-4-6".into(),
                provider: "anthropic".into(),
            },
        ));

        let snap = sink.snapshot();
        assert_eq!(snap.total_model_responses, 2);
        assert_eq!(snap.avg_model_latency_ms, 1000.0);
        assert_eq!(snap.avg_provider_latency_ms("anthropic"), Some(1000.0));
        assert_eq!(snap.avg_provider_latency_ms("bedrock"), None);
    }

    #[test]
    fn planning_vs_execution_ratio() {
        let sink = MetricsSink::new();
        let s = session();
        // 1 planning round + 3 execution rounds
        sink.emit(&RuntimeEvent::new(s, round_completed(1, 300, "plan")));
        sink.emit(&RuntimeEvent::new(s, round_completed(2, 400, "execute")));
        sink.emit(&RuntimeEvent::new(s, round_completed(3, 500, "execute")));
        sink.emit(&RuntimeEvent::new(s, round_completed(4, 600, "execute")));

        let snap = sink.snapshot();
        assert_eq!(snap.planning_rounds, 1);
        assert_eq!(snap.execution_rounds, 3);
        let ratio = snap.planning_ratio.unwrap();
        assert!((ratio - 0.25).abs() < 1e-6);
    }

    #[test]
    fn plan_created_and_replanned_counts() {
        let sink = MetricsSink::new();
        let s = session();
        let plan_id = Uuid::new_v4();
        let new_plan_id = Uuid::new_v4();
        sink.emit(&RuntimeEvent::new(
            s,
            RuntimeEventKind::PlanCreated {
                plan_id,
                goal: "Refactor auth".into(),
                steps: vec![],
                replan_count: 0,
                requires_confirmation: false,
                mode: crate::event::PlanMode::PlanExecuteReflect,
            },
        ));
        sink.emit(&RuntimeEvent::new(
            s,
            RuntimeEventKind::PlanReplanned {
                old_plan_id: plan_id,
                new_plan_id,
                reason: "tool_failure".into(),
                replan_count: 1,
            },
        ));

        let snap = sink.snapshot();
        assert_eq!(snap.plans_created, 1);
        assert_eq!(snap.replan_count, 1);
    }

    #[test]
    fn budget_warning_and_exhaustion() {
        let sink = MetricsSink::new();
        let s = session();
        sink.emit(&RuntimeEvent::new(
            s,
            RuntimeEventKind::BudgetWarning {
                tokens_used: 6000,
                tokens_total: 8000,
                pct_used: 0.75,
                time_elapsed_ms: 60_000,
                time_limit_ms: 120_000,
            },
        ));
        sink.emit(&RuntimeEvent::new(
            s,
            RuntimeEventKind::BudgetExhausted {
                reason: BudgetExhaustionReason::TokenLimit,
                tokens_used: 8000,
                tokens_total: 8000,
                time_elapsed_ms: 90_000,
            },
        ));

        let snap = sink.snapshot();
        assert_eq!(snap.budget_warnings, 1);
        assert!(snap.budget_exhausted);
        assert_eq!(snap.total_tokens_used, 8000);
    }

    #[test]
    fn session_ended_records_cost_and_tokens() {
        let sink = MetricsSink::new();
        let s = session();
        sink.emit(&RuntimeEvent::new(
            s,
            RuntimeEventKind::SessionEnded {
                rounds_completed: 5,
                stop_condition: "end_turn".into(),
                total_tokens: 12_500,
                estimated_cost_usd: 0.0045,
                duration_ms: 30_000,
                fingerprint: None,
            },
        ));

        let snap = sink.snapshot();
        assert_eq!(snap.total_tokens_used, 12_500);
        assert!((snap.estimated_cost_usd - 0.0045).abs() < 1e-9);
    }

    #[test]
    fn tool_blocked_increments_blocked_counter() {
        let sink = MetricsSink::new();
        let s = session();
        sink.emit(&RuntimeEvent::new(
            s,
            RuntimeEventKind::ToolBlocked {
                round: 1,
                tool_use_id: Uuid::new_v4().to_string(),
                tool_name: "bash".into(),
                reason: ToolBlockReason::GuardrailBlocked,
                message: "dangerous pattern".into(),
            },
        ));

        let snap = sink.snapshot();
        assert_eq!(snap.tool_calls_blocked, 1);
        // Blocked calls do NOT count toward success/failure totals.
        assert_eq!(snap.total_tool_calls, 0);
    }

    #[test]
    fn reset_clears_all_metrics() {
        let sink = MetricsSink::new();
        let s = session();
        sink.emit(&RuntimeEvent::new(s, round_completed(1, 1000, "execute")));
        sink.emit(&RuntimeEvent::new(s, tool_completed("bash", true, 100)));
        assert_eq!(sink.snapshot().total_rounds, 1);

        sink.reset();
        let snap = sink.snapshot();
        assert_eq!(snap.total_rounds, 0);
        assert_eq!(snap.total_tool_calls, 0);
    }

    #[test]
    fn multi_provider_averages_are_independent() {
        let sink = MetricsSink::new();
        let s = session();
        // Anthropic: 2 calls at 500ms + 1000ms = avg 750ms
        for ms in [500u64, 1000] {
            sink.emit(&RuntimeEvent::new(
                s,
                RuntimeEventKind::ModelResponseCompleted {
                    round: 1,
                    input_tokens: 0,
                    output_tokens: 0,
                    latency_ms: ms,
                    model: "m".into(),
                    provider: "anthropic".into(),
                },
            ));
        }
        // Bedrock: 1 call at 2000ms
        sink.emit(&RuntimeEvent::new(
            s,
            RuntimeEventKind::ModelResponseCompleted {
                round: 1,
                input_tokens: 0,
                output_tokens: 0,
                latency_ms: 2000,
                model: "m".into(),
                provider: "bedrock".into(),
            },
        ));

        let snap = sink.snapshot();
        assert_eq!(snap.avg_provider_latency_ms("anthropic"), Some(750.0));
        assert_eq!(snap.avg_provider_latency_ms("bedrock"), Some(2000.0));
        // Overall average: (500 + 1000 + 2000) / 3 = 1166.66...
        assert!((snap.avg_model_latency_ms - (3500.0 / 3.0)).abs() < 0.01);
    }

    #[test]
    fn tool_batch_events_do_not_double_count() {
        // ToolBatchStarted / ToolBatchCompleted should not affect tool call counts —
        // only ToolCallCompleted does.
        let sink = MetricsSink::new();
        let s = session();
        sink.emit(&RuntimeEvent::new(
            s,
            RuntimeEventKind::ToolBatchStarted {
                round: 1,
                batch_kind: ToolBatchKind::Parallel,
                tool_names: vec!["bash".into(), "file_read".into()],
            },
        ));
        sink.emit(&RuntimeEvent::new(
            s,
            RuntimeEventKind::ToolBatchCompleted {
                round: 1,
                batch_kind: ToolBatchKind::Parallel,
                success_count: 2,
                failure_count: 0,
                total_duration_ms: 500,
            },
        ));

        let snap = sink.snapshot();
        assert_eq!(
            snap.total_tool_calls, 0,
            "batch events must not count as individual calls"
        );
    }
}
