//! ConvergenceState extensions — Session-wide convergence and termination signals.
//!
//! Phase 2.2: The base `ConvergenceState` already exists in loop_state.rs.
//! This module extends it with additional session-level termination tracking.

use super::super::domain::progress_policy::ProgressPolicyConfig;
use super::loop_state::ConvergenceState;

/// Extended convergence tracking with session-wide counters and policy.
///
/// Maximum 15 fields (Phase 2 constraint). The base `ConvergenceState` handles
/// per-round convergence metrics. This extension adds session-level counters
/// for adaptive rescue synthesis (Phase 4 Progress Policy).
///
/// NOTE: `base` is Option<> because ConvergenceState cannot implement Default
/// (too many complex domain types). Bridge methods create SessionConvergenceState
/// with None base during Phase 2.3 migration.
pub(super) struct SessionConvergenceState {
    /// Base convergence state (from loop_state.rs, existing sub-struct).
    /// None during Phase 2.3 bridge method construction.
    pub base: Option<ConvergenceState>,

    /// Consecutive rounds classified as Stalled (reset on Progressing verdict).
    pub consecutive_stalls: u32,

    /// Consecutive rounds classified as Regressing (reset on Progressing verdict).
    pub consecutive_regressions: u32,

    /// Progress policy configuration (thresholds for rescue synthesis).
    pub policy_config: ProgressPolicyConfig,

    /// Environment error halt flag (set when environment capabilities lost).
    pub environment_error_halt: bool,

    /// Auto-pause flag (set by TUI step-once mode).
    pub auto_pause: bool,

    /// Control cancellation flag (set when user cancels via TUI).
    pub ctrl_cancelled: bool,
}

impl SessionConvergenceState {
    /// Construct with base convergence state and default counters.
    pub(super) fn new(base: ConvergenceState, policy_config: ProgressPolicyConfig) -> Self {
        Self {
            base: Some(base),
            consecutive_stalls: 0,
            consecutive_regressions: 0,
            policy_config,
            environment_error_halt: false,
            auto_pause: false,
            ctrl_cancelled: false,
        }
    }

    /// Reset stall/regression counters (called after rescue synthesis or progress).
    pub(super) fn reset_adaptive_counters(&mut self) {
        self.consecutive_stalls = 0;
        self.consecutive_regressions = 0;
    }

    /// Increment stall counter.
    pub(super) fn record_stall(&mut self) {
        self.consecutive_stalls += 1;
    }

    /// Increment regression counter.
    pub(super) fn record_regression(&mut self) {
        self.consecutive_regressions += 1;
    }

    /// Check if any halt condition is active.
    pub(super) fn is_halt_requested(&self) -> bool {
        self.environment_error_halt || self.ctrl_cancelled
    }
}

impl Default for SessionConvergenceState {
    fn default() -> Self {
        Self {
            base: None, // ConvergenceState too complex for Default (Phase 2.3 limitation)
            consecutive_stalls: 0,
            consecutive_regressions: 0,
            policy_config: ProgressPolicyConfig::default(),
            environment_error_halt: false,
            auto_pause: false,
            ctrl_cancelled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_convergence_default() {
        let state = SessionConvergenceState::default();
        assert_eq!(state.consecutive_stalls, 0);
        assert_eq!(state.consecutive_regressions, 0);
        assert!(!state.is_halt_requested());
    }

    #[test]
    fn session_convergence_record_stalls() {
        let mut state = SessionConvergenceState::default();

        state.record_stall();
        assert_eq!(state.consecutive_stalls, 1);

        state.record_stall();
        assert_eq!(state.consecutive_stalls, 2);
    }

    #[test]
    fn session_convergence_reset_counters() {
        let mut state = SessionConvergenceState::default();

        state.record_stall();
        state.record_stall();
        state.record_regression();
        assert_eq!(state.consecutive_stalls, 2);
        assert_eq!(state.consecutive_regressions, 1);

        state.reset_adaptive_counters();
        assert_eq!(state.consecutive_stalls, 0);
        assert_eq!(state.consecutive_regressions, 0);
    }

    #[test]
    fn session_convergence_halt_conditions() {
        let mut state = SessionConvergenceState::default();
        assert!(!state.is_halt_requested());

        state.environment_error_halt = true;
        assert!(state.is_halt_requested());

        state.environment_error_halt = false;
        state.ctrl_cancelled = true;
        assert!(state.is_halt_requested());
    }
}
