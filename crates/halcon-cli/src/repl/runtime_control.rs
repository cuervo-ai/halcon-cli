//! Runtime coordination and control bundle.
//!
//! Phase 3.1: Groups active runtime subsystems (speculation, telemetry, control
//! channels, load-once guards) into a single cohesive unit. 5+ fields from original Repl.

use std::sync::Arc;

use super::metrics::signal_ingestor::RuntimeSignalIngestor;
use super::tool_speculation::ToolSpeculator;

/// Load-once guards for database queries and one-time checks.
///
/// Prevents repeated DB queries and file-existence checks.
pub struct RuntimeGuards {
    /// Phase 4: Whether cross-session quality stats have been loaded from DB this session.
    /// Prevents repeated DB queries (load-once-per-session).
    pub model_quality_db_loaded: bool,

    /// Whether plugin UCB1 metrics have been loaded from DB this session.
    pub plugin_metrics_db_loaded: bool,

    /// Phase 94: One-time onboarding check performed on first message.
    /// Set to true after the check runs (prevents repeated file-existence checks).
    pub onboarding_checked: bool,

    /// Phase 95: One-time plugin recommendation check on first message.
    pub plugin_recommendation_done: bool,
}

impl RuntimeGuards {
    /// Construct with all guards unset (false).
    pub fn new() -> Self {
        Self {
            model_quality_db_loaded: false,
            plugin_metrics_db_loaded: false,
            onboarding_checked: false,
            plugin_recommendation_done: false,
        }
    }
}

impl Default for RuntimeGuards {
    fn default() -> Self {
        Self::new()
    }
}

/// Runtime coordination and control for the REPL.
///
/// Bundles active runtime subsystems:
/// - Tool speculation engine for pre-execution
/// - Telemetry ingestor for observability
/// - Background task stop signal
/// - TUI control channel (optional)
/// - Load-once guards
pub struct ReplRuntimeControl {
    /// Tool speculation engine for pre-executing read-only tools (Phase 3 remediation).
    /// Shared across rounds to accumulate hit/miss metrics.
    pub speculator: ToolSpeculator,

    /// Phase 7 Dev Ecosystem: Rolling observability window for agent-loop telemetry.
    /// Ingests per-loop spans and exposes p50/p95/p99 + error-rate as a UCB1 reward signal.
    pub runtime_signals: Arc<RuntimeSignalIngestor>,

    /// Phase 4 Dev Ecosystem: Stop signal for the background CI polling task.
    /// Set once during `run()` / `run_tui()` when GITHUB_TOKEN is available.
    /// Notified on session teardown so the polling loop exits gracefully.
    pub ci_stop: Arc<tokio::sync::Notify>,

    /// Control channel receiver from TUI (Phase 43). None in classic REPL mode.
    #[cfg(feature = "tui")]
    pub ctrl_rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::tui::events::ControlEvent>>,

    /// Load-once guards for DB queries and one-time checks.
    pub guards: RuntimeGuards,
}

impl ReplRuntimeControl {
    /// Construct runtime control with all components.
    pub fn new(
        speculator: ToolSpeculator,
        runtime_signals: Arc<RuntimeSignalIngestor>,
        ci_stop: Arc<tokio::sync::Notify>,
        #[cfg(feature = "tui")] ctrl_rx: Option<
            tokio::sync::mpsc::UnboundedReceiver<crate::tui::events::ControlEvent>,
        >,
    ) -> Self {
        Self {
            speculator,
            runtime_signals,
            ci_stop,
            #[cfg(feature = "tui")]
            ctrl_rx,
            guards: RuntimeGuards::new(),
        }
    }

    /// Check if TUI control channel is available.
    #[cfg(feature = "tui")]
    pub fn has_ctrl_channel(&self) -> bool {
        self.ctrl_rx.is_some()
    }

    /// Check if TUI control channel is available (always false without TUI feature).
    #[cfg(not(feature = "tui"))]
    pub fn has_ctrl_channel(&self) -> bool {
        false
    }

    /// Take the control channel with a RAII guard that restores it on drop.
    ///
    /// BRECHA-16 FIX: Previously, `.take()` without a guard meant that if any
    /// code path between take and restore panicked, the channel was permanently
    /// lost. This guard ensures the channel is restored in ALL cases.
    ///
    /// # Xiyo Comparison
    ///
    /// Xiyo uses `AbortController` with a `using` (dispose) pattern that
    /// automatically cleans up on scope exit. This `CtrlRxGuard` is the Rust
    /// equivalent — RAII guarantees cleanup even on panic.
    #[cfg(feature = "tui")]
    pub fn take_ctrl_rx_guarded(&mut self) -> CtrlRxGuard<'_> {
        CtrlRxGuard {
            rx: self.ctrl_rx.take(),
            slot: &mut self.ctrl_rx,
        }
    }
}

/// RAII guard for the TUI control channel receiver.
///
/// On drop, restores the receiver back to `ReplRuntimeControl.ctrl_rx`
/// unless it was explicitly consumed via `into_inner()`.
///
/// This prevents channel loss on panic or early return (`?` operator).
#[cfg(feature = "tui")]
pub struct CtrlRxGuard<'a> {
    rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::tui::events::ControlEvent>>,
    slot: &'a mut Option<tokio::sync::mpsc::UnboundedReceiver<crate::tui::events::ControlEvent>>,
}

#[cfg(feature = "tui")]
impl<'a> CtrlRxGuard<'a> {
    /// Take the receiver out of the guard (for passing to AgentContext).
    /// After calling this, the guard will NOT restore on drop.
    pub fn take(
        &mut self,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<crate::tui::events::ControlEvent>> {
        self.rx.take()
    }

    /// Restore a receiver from an agent loop result back into the guard.
    /// The guard will then restore it to the slot on drop.
    pub fn restore(
        &mut self,
        rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::tui::events::ControlEvent>>,
    ) {
        self.rx = rx;
    }
}

#[cfg(feature = "tui")]
impl<'a> Drop for CtrlRxGuard<'a> {
    fn drop(&mut self) {
        // Restore the channel to the slot if we still have it.
        if self.rx.is_some() {
            *self.slot = self.rx.take();
        }
    }
}

impl Default for ReplRuntimeControl {
    fn default() -> Self {
        Self {
            speculator: ToolSpeculator::new(),
            runtime_signals: Arc::new(RuntimeSignalIngestor::new(1000)), // Default capacity
            ci_stop: Arc::new(tokio::sync::Notify::new()),
            #[cfg(feature = "tui")]
            ctrl_rx: None,
            guards: RuntimeGuards::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_guards_default_all_unset() {
        let guards = RuntimeGuards::default();

        assert!(!guards.model_quality_db_loaded);
        assert!(!guards.plugin_metrics_db_loaded);
        assert!(!guards.onboarding_checked);
        assert!(!guards.plugin_recommendation_done);
    }

    #[test]
    fn runtime_control_default_construction() {
        let runtime = ReplRuntimeControl::default();

        assert!(!runtime.has_ctrl_channel(), "no TUI channel by default");
        assert!(!runtime.guards.onboarding_checked);
    }

    #[test]
    fn runtime_control_guards_are_mutable() {
        let mut runtime = ReplRuntimeControl::default();

        runtime.guards.onboarding_checked = true;
        assert!(runtime.guards.onboarding_checked);

        runtime.guards.model_quality_db_loaded = true;
        assert!(runtime.guards.model_quality_db_loaded);
    }
}
