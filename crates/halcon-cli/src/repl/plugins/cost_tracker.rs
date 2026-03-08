//! Per-plugin cost and budget tracker.
//!
//! Tracks tokens, USD and call counts for a single plugin.
//! `check_budget()` is called in the pre-invoke gate; `record_call()` is called
//! post-invocation regardless of success.

// ─── Budget Error ─────────────────────────────────────────────────────────────

/// Reason a plugin invocation was denied by its budget limits.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginBudgetError {
    /// Token quota exceeded.
    TokensExceeded { used: u64, limit: u64 },
    /// USD spending cap exceeded.
    UsdExceeded { used: f64, limit: f64 },
    /// Per-session call count cap exceeded.
    CallsExceeded { count: u32 },
}

impl std::fmt::Display for PluginBudgetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginBudgetError::TokensExceeded { used, limit } => {
                write!(f, "plugin token budget exceeded ({used}/{limit})")
            }
            PluginBudgetError::UsdExceeded { used, limit } => {
                write!(f, "plugin USD budget exceeded ({used:.4}/{limit:.4})")
            }
            PluginBudgetError::CallsExceeded { count } => {
                write!(f, "plugin call limit exceeded ({count} calls)")
            }
        }
    }
}

// ─── Cost Snapshot ────────────────────────────────────────────────────────────

/// Serializable snapshot of a plugin's accumulated costs (used in AgentLoopResult).
#[derive(Debug, Clone, Default)]
pub struct PluginCostSnapshot {
    pub plugin_id: String,
    pub tokens_used: u64,
    pub usd_spent: f64,
    pub calls_made: u32,
    pub calls_failed: u32,
}

// ─── Cost Tracker ─────────────────────────────────────────────────────────────

/// Mutable accumulator for one plugin's resource consumption.
pub struct PluginCostTracker {
    /// Plugin identifier for snapshot labelling.
    pub plugin_id: String,
    /// Total tokens consumed across all calls this session.
    pub tokens_used: u64,
    /// Total USD spent this session.
    pub usd_spent: f64,
    /// Total calls attempted (successful + failed).
    pub calls_made: u32,
    /// Total calls that returned an error result.
    pub calls_failed: u32,
    /// Optional hard cap on tokens (None = unlimited).
    max_tokens: Option<u64>,
    /// Optional hard cap on USD (None = unlimited).
    max_usd: Option<f64>,
    /// Optional hard cap on call count (None = unlimited).
    max_calls: Option<u32>,
}

impl PluginCostTracker {
    /// Create a new tracker with optional limits.
    pub fn new(
        plugin_id: String,
        max_tokens: Option<u64>,
        max_usd: Option<f64>,
        max_calls: Option<u32>,
    ) -> Self {
        Self {
            plugin_id,
            tokens_used: 0,
            usd_spent: 0.0,
            calls_made: 0,
            calls_failed: 0,
            max_tokens,
            max_usd,
            max_calls,
        }
    }

    /// Create an unlimited tracker (no budget caps).
    pub fn unlimited(plugin_id: String) -> Self {
        Self::new(plugin_id, None, None, None)
    }

    /// Check whether the next call would exceed any configured budget.
    ///
    /// Returns `Some(error)` if a limit has been reached — the pre-invoke gate
    /// should deny the call and return a synthetic error result.
    /// Returns `None` when within all limits.
    pub fn check_budget(&self) -> Option<PluginBudgetError> {
        if let Some(max) = self.max_calls {
            if self.calls_made >= max {
                return Some(PluginBudgetError::CallsExceeded { count: self.calls_made });
            }
        }
        if let Some(max) = self.max_tokens {
            if self.tokens_used >= max {
                return Some(PluginBudgetError::TokensExceeded {
                    used: self.tokens_used,
                    limit: max,
                });
            }
        }
        if let Some(max) = self.max_usd {
            if self.usd_spent >= max {
                return Some(PluginBudgetError::UsdExceeded {
                    used: self.usd_spent,
                    limit: max,
                });
            }
        }
        None
    }

    /// Record one completed invocation.
    ///
    /// Uses saturating arithmetic for integer fields to prevent wrap-around on
    /// pathological inputs. For the USD field, non-finite values (NaN, ±Inf) from
    /// a misbehaving plugin are silently ignored to keep the tracker consistent.
    pub fn record_call(&mut self, tokens: u64, usd: f64, success: bool) {
        self.calls_made = self.calls_made.saturating_add(1);
        self.tokens_used = self.tokens_used.saturating_add(tokens);
        // Guard against NaN / Inf from a buggy plugin cost estimate.
        if usd.is_finite() {
            self.usd_spent += usd;
        } else {
            tracing::warn!(
                plugin_id = %self.plugin_id,
                usd = %usd,
                "Non-finite USD cost from plugin — ignoring to prevent tracker corruption"
            );
        }
        if !success {
            self.calls_failed = self.calls_failed.saturating_add(1);
        }
    }

    /// Export an immutable snapshot for inclusion in [`AgentLoopResult`].
    pub fn snapshot(&self) -> PluginCostSnapshot {
        PluginCostSnapshot {
            plugin_id: self.plugin_id.clone(),
            tokens_used: self.tokens_used,
            usd_spent: self.usd_spent,
            calls_made: self.calls_made,
            calls_failed: self.calls_failed,
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_call_accumulates() {
        let mut tracker = PluginCostTracker::unlimited("p1".into());
        tracker.record_call(100, 0.01, true);
        tracker.record_call(200, 0.02, false);
        assert_eq!(tracker.tokens_used, 300);
        assert!((tracker.usd_spent - 0.03).abs() < 1e-9);
        assert_eq!(tracker.calls_made, 2);
        assert_eq!(tracker.calls_failed, 1);
    }

    #[test]
    fn budget_gate_tokens() {
        let mut tracker = PluginCostTracker::new("p".into(), Some(100), None, None);
        tracker.record_call(100, 0.0, true);
        let err = tracker.check_budget();
        assert!(matches!(err, Some(PluginBudgetError::TokensExceeded { .. })));
    }

    #[test]
    fn budget_gate_usd() {
        let mut tracker = PluginCostTracker::new("p".into(), None, Some(1.0), None);
        tracker.record_call(0, 1.0, true);
        let err = tracker.check_budget();
        assert!(matches!(err, Some(PluginBudgetError::UsdExceeded { .. })));
    }

    #[test]
    fn budget_gate_calls() {
        let mut tracker = PluginCostTracker::new("p".into(), None, None, Some(2));
        tracker.record_call(0, 0.0, true);
        tracker.record_call(0, 0.0, true);
        let err = tracker.check_budget();
        assert!(matches!(err, Some(PluginBudgetError::CallsExceeded { count: 2 })));
    }

    #[test]
    fn snapshot_matches_tracker_state() {
        let mut tracker = PluginCostTracker::unlimited("snap-test".into());
        tracker.record_call(50, 0.005, true);
        let snap = tracker.snapshot();
        assert_eq!(snap.plugin_id, "snap-test");
        assert_eq!(snap.tokens_used, 50);
        assert_eq!(snap.calls_made, 1);
        assert_eq!(snap.calls_failed, 0);
    }

    // ── Audit fixes: saturating arithmetic + finite USD validation ─────────────

    #[test]
    fn record_call_saturating_add_tokens_no_overflow() {
        // Audit fix: tokens_used uses saturating_add to prevent u64 wraparound.
        // A misbehaving plugin reporting u64::MAX tokens must not corrupt the counter.
        let mut tracker = PluginCostTracker::unlimited("overflow-tokens".into());
        tracker.tokens_used = u64::MAX - 1;
        tracker.record_call(u64::MAX, 0.0, true);
        assert_eq!(
            tracker.tokens_used,
            u64::MAX,
            "tokens_used must saturate at u64::MAX, not wrap to 0"
        );
    }

    #[test]
    fn record_call_saturating_add_calls_no_overflow() {
        // Audit fix: calls_made uses saturating_add to prevent u32 wraparound.
        let mut tracker = PluginCostTracker::unlimited("overflow-calls".into());
        tracker.calls_made = u32::MAX;
        tracker.record_call(0, 0.0, true);
        assert_eq!(
            tracker.calls_made,
            u32::MAX,
            "calls_made must saturate at u32::MAX, not wrap to 0"
        );
    }

    #[test]
    fn record_call_saturating_add_failures_no_overflow() {
        // Audit fix: calls_failed uses saturating_add to prevent u32 wraparound.
        let mut tracker = PluginCostTracker::unlimited("overflow-failures".into());
        tracker.calls_failed = u32::MAX;
        tracker.record_call(0, 0.0, false); // failure
        assert_eq!(
            tracker.calls_failed,
            u32::MAX,
            "calls_failed must saturate at u32::MAX, not wrap to 0"
        );
    }

    #[test]
    fn record_call_ignores_nan_usd() {
        // Audit fix: NaN USD from a buggy plugin must NOT corrupt usd_spent.
        // Previously usd_spent += NaN would propagate NaN to all subsequent checks.
        let mut tracker = PluginCostTracker::unlimited("nan-usd".into());
        tracker.record_call(0, f64::NAN, true);
        assert_eq!(tracker.usd_spent, 0.0, "NaN USD must be ignored, not accumulated");
    }

    #[test]
    fn record_call_ignores_positive_infinity_usd() {
        // +Inf USD must be rejected — it would make usd_spent permanently infinite.
        let mut tracker = PluginCostTracker::unlimited("inf-usd".into());
        tracker.record_call(0, f64::INFINITY, true);
        assert_eq!(tracker.usd_spent, 0.0, "+Inf USD must be ignored");
    }

    #[test]
    fn record_call_ignores_negative_infinity_usd() {
        // -Inf USD must also be rejected — it would make usd_spent permanently negative infinite.
        let mut tracker = PluginCostTracker::unlimited("neg-inf-usd".into());
        tracker.record_call(0, f64::NEG_INFINITY, true);
        assert_eq!(tracker.usd_spent, 0.0, "-Inf USD must be ignored");
    }

    #[test]
    fn record_call_finite_usd_still_accumulates_after_non_finite() {
        // Regression guard: after rejecting non-finite values, finite values must still work.
        let mut tracker = PluginCostTracker::unlimited("mixed-usd".into());
        tracker.record_call(0, f64::NAN, true);      // rejected
        tracker.record_call(0, f64::INFINITY, true); // rejected
        tracker.record_call(0, 0.05, true);           // must accumulate
        assert!(
            (tracker.usd_spent - 0.05).abs() < 1e-10,
            "finite USD must accumulate normally even after non-finite values were rejected"
        );
    }
}
