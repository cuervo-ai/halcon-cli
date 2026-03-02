//! Semantic role types for `LoopState` decomposition — Phase 4 scaffolding.
//!
//! `LoopState` is a 40-field god object. This module defines the six semantic
//! groups that `LoopState` will be decomposed into in a future migration phase:
//!
//! | Target type        | Semantic role                              |
//! |--------------------|--------------------------------------------|
//! | `ControlSignals`   | Boolean flags and directives               |
//! | `LoopAccumulator`  | Accumulated metrics and outputs            |
//! | `TokenBudget`      | Token and cost accounting                  |
//! | `RoundContext`     | Per-round setup data                       |
//! | `SessionMetadata`  | Stable session-level identity              |
//! | `SubsystemHealth`  | Health snapshot from HICON subsystems      |
//!
//! ## Migration strategy
//!
//! Phase 4 creates these types as *additive snapshot views* — constructed from
//! `&LoopState` for read-only analysis (e.g. observability, invariant checks).
//! Phase 5 will embed them into `LoopState` as owned sub-structs, migrating
//! field access one group at a time while keeping compilation green.
//!
//! No `LoopState` field is moved or renamed in this phase.

use super::loop_state::LoopState;
use crate::repl::agent::loop_state::ToolDecisionSignal;

// ── ControlSignals ────────────────────────────────────────────────────────────

/// Snapshot of `LoopState` boolean control flags for a single round.
///
/// Captures the seven flags that govern loop control flow — created at the
/// start of each round for observability and invariant validation.
#[derive(Debug, Clone)]
pub(super) struct ControlSignals {
    /// `LoopState::forced_synthesis_detected`
    pub forced_synthesis_detected: bool,
    /// `LoopState::convergence_directive_injected`
    pub convergence_directive_injected: bool,
    /// `LoopState::environment_error_halt`
    pub environment_error_halt: bool,
    /// `LoopState::auto_pause`
    pub auto_pause: bool,
    /// `LoopState::ctrl_cancelled`
    pub ctrl_cancelled: bool,
    /// `LoopState::model_downgrade_advisory_active`
    pub model_downgrade_advisory_active: bool,
    /// `LoopState::tool_decision`
    pub tool_decision: ToolDecisionSignal,
}

impl ControlSignals {
    /// Construct a snapshot from the current loop state.
    pub(super) fn from_loop_state(state: &LoopState) -> Self {
        Self {
            forced_synthesis_detected:      state.synthesis.forced_synthesis_detected,
            convergence_directive_injected: state.synthesis.convergence_directive_injected,
            environment_error_halt:         state.environment_error_halt,
            auto_pause:                     state.auto_pause,
            ctrl_cancelled:                 state.ctrl_cancelled,
            model_downgrade_advisory_active: state.model_downgrade_advisory_active,
            tool_decision:                  state.synthesis.tool_decision,
        }
    }

    /// Returns `true` when any halt signal is active.
    pub(super) fn any_halt(&self) -> bool {
        self.environment_error_halt || self.auto_pause || self.ctrl_cancelled
    }

    /// Returns `true` when the next round should suppress tool calls.
    pub(super) fn tools_suppressed(&self) -> bool {
        self.tool_decision.is_active()
    }
}

// ── LoopAccumulator ───────────────────────────────────────────────────────────

/// Snapshot of accumulated loop outputs after each round.
///
/// Captures the values written throughout the loop that appear in
/// `AgentLoopResult` — useful for mid-loop telemetry without touching
/// the result assembly path.
#[derive(Debug, Clone)]
pub(super) struct LoopAccumulator {
    /// `LoopState::full_text`
    pub full_text_len: usize,
    /// `LoopState::rounds`
    pub rounds_completed: usize,
    /// `LoopState::tools_executed`
    pub tools_executed: Vec<String>,
    /// `LoopState::replan_attempts`
    pub replan_attempts: u32,
    /// `LoopState::drift_replan_count`
    pub drift_replan_count: usize,
    /// `LoopState::cumulative_drift_score`
    pub cumulative_drift_score: f32,
}

impl LoopAccumulator {
    /// Construct a snapshot from the current loop state.
    pub(super) fn from_loop_state(state: &LoopState) -> Self {
        Self {
            full_text_len:        state.full_text.len(),
            rounds_completed:     state.rounds,
            tools_executed:       state.tools_executed.clone(),
            replan_attempts:      state.convergence.replan_attempts,
            drift_replan_count:   state.convergence.drift_replan_count,
            cumulative_drift_score: state.convergence.cumulative_drift_score,
        }
    }

    /// `true` when at least one tool was successfully executed.
    pub(super) fn has_tool_output(&self) -> bool {
        !self.tools_executed.is_empty()
    }
}

// ── TokenBudget ───────────────────────────────────────────────────────────────

/// Snapshot of the token and cost accounting state.
///
/// Groups all the `u64 / f64` accounting fields that are accumulated across
/// rounds and reported in `AgentLoopResult`. Corresponds to the natural
/// "accounting" semantic cluster in `LoopState`.
#[derive(Debug, Clone, Copy)]
pub(super) struct TokenBudget {
    /// `LoopState::call_input_tokens`
    pub input_tokens: u64,
    /// `LoopState::call_output_tokens`
    pub output_tokens: u64,
    /// `LoopState::call_cost`
    pub cost: f64,
    /// `LoopState::pipeline_budget`
    pub pipeline_budget: u32,
    /// `LoopState::provider_context_window`
    pub context_window: u32,
    /// `LoopState::tokens_planning`
    pub tokens_planning: u64,
    /// `LoopState::tokens_subagents`
    pub tokens_subagents: u64,
    /// `LoopState::tokens_critic`
    pub tokens_critic: u64,
}

impl TokenBudget {
    /// Construct a snapshot from the current loop state.
    pub(super) fn from_loop_state(state: &LoopState) -> Self {
        Self {
            input_tokens:      state.tokens.call_input_tokens,
            output_tokens:     state.tokens.call_output_tokens,
            cost:              state.tokens.call_cost,
            pipeline_budget:   state.tokens.pipeline_budget,
            context_window:    state.tokens.provider_context_window,
            tokens_planning:   state.tokens.tokens_planning,
            tokens_subagents:  state.tokens.tokens_subagents,
            tokens_critic:     state.tokens.tokens_critic,
        }
    }

    /// Total tokens consumed (input + output).
    pub(super) fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Fraction of the pipeline budget consumed by input tokens (0.0–1.0).
    /// Returns 0.0 when `pipeline_budget` is zero.
    pub(super) fn budget_utilization(&self) -> f32 {
        if self.pipeline_budget == 0 { return 0.0; }
        (self.input_tokens as f32) / (self.pipeline_budget as f32)
    }
}

// ── SessionMetadata ───────────────────────────────────────────────────────────

/// Snapshot of stable session-level identity fields.
///
/// These do not change during the loop — they identify the session, user goal,
/// and working context. Useful for log enrichment without coupling to LoopState.
#[derive(Debug, Clone)]
pub(super) struct SessionMetadata {
    /// `LoopState::session_id`
    pub session_id: uuid::Uuid,
    /// `LoopState::user_msg`
    pub user_msg: String,
    /// `LoopState::goal_text`
    pub goal_text: String,
}

impl SessionMetadata {
    /// Construct a snapshot from the current loop state.
    pub(super) fn from_loop_state(state: &LoopState) -> Self {
        Self {
            session_id: state.session_id,
            user_msg:   state.user_msg.clone(),
            goal_text:  state.goal_text.clone(),
        }
    }
}

// ── SubsystemHealth ───────────────────────────────────────────────────────────

/// Lightweight health indicators from HICON subsystems.
///
/// Extracted for observability without exposing the subsystem types directly.
/// Phase 5 will embed the full subsystem handles here.
#[derive(Debug, Clone, Copy)]
pub(super) struct SubsystemHealth {
    /// Whether the metacognitive loop should run this round.
    pub metacognitive_should_run: bool,
    /// Convergence ratio from the last round (0.0–1.0).
    pub last_convergence_ratio: f32,
    /// Number of NextRound loop restarts — high values indicate oscillation.
    pub next_round_restarts: usize,
}

impl SubsystemHealth {
    /// Construct a snapshot from the current loop state.
    pub(super) fn from_loop_state(state: &LoopState, round: usize) -> Self {
        Self {
            metacognitive_should_run: state.hicon.metacognitive_loop.should_run_cycle(round + 1),
            last_convergence_ratio:  state.convergence.last_convergence_ratio,
            next_round_restarts:     state.next_round_restarts,
        }
    }

    /// `true` when oscillation indicators are elevated.
    pub(super) fn shows_oscillation(&self) -> bool {
        self.next_round_restarts >= 3
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_signals_any_halt_true_when_cancelled() {
        let sig = ControlSignals {
            forced_synthesis_detected: false,
            convergence_directive_injected: false,
            environment_error_halt: false,
            auto_pause: false,
            ctrl_cancelled: true,
            model_downgrade_advisory_active: false,
            tool_decision: ToolDecisionSignal::Allow,
        };
        assert!(sig.any_halt());
    }

    #[test]
    fn control_signals_any_halt_false_when_clean() {
        let sig = ControlSignals {
            forced_synthesis_detected: false,
            convergence_directive_injected: false,
            environment_error_halt: false,
            auto_pause: false,
            ctrl_cancelled: false,
            model_downgrade_advisory_active: false,
            tool_decision: ToolDecisionSignal::Allow,
        };
        assert!(!sig.any_halt());
    }

    #[test]
    fn control_signals_tools_suppressed_when_force_no_next() {
        let sig = ControlSignals {
            forced_synthesis_detected: false,
            convergence_directive_injected: false,
            environment_error_halt: false,
            auto_pause: false,
            ctrl_cancelled: false,
            model_downgrade_advisory_active: false,
            tool_decision: ToolDecisionSignal::ForceNoNext,
        };
        assert!(sig.tools_suppressed());
    }

    #[test]
    fn loop_accumulator_has_tool_output_false_when_empty() {
        let acc = LoopAccumulator {
            full_text_len: 0,
            rounds_completed: 1,
            tools_executed: vec![],
            replan_attempts: 0,
            drift_replan_count: 0,
            cumulative_drift_score: 0.0,
        };
        assert!(!acc.has_tool_output());
    }

    #[test]
    fn loop_accumulator_has_tool_output_true_when_non_empty() {
        let acc = LoopAccumulator {
            full_text_len: 100,
            rounds_completed: 2,
            tools_executed: vec!["file_read".into()],
            replan_attempts: 0,
            drift_replan_count: 0,
            cumulative_drift_score: 0.0,
        };
        assert!(acc.has_tool_output());
    }

    #[test]
    fn token_budget_total_tokens() {
        let b = TokenBudget {
            input_tokens: 1000,
            output_tokens: 500,
            cost: 0.001,
            pipeline_budget: 50_000,
            context_window: 64_000,
            tokens_planning: 100,
            tokens_subagents: 200,
            tokens_critic: 50,
        };
        assert_eq!(b.total_tokens(), 1500);
    }

    #[test]
    fn token_budget_utilization_zero_budget() {
        let b = TokenBudget {
            input_tokens: 1000,
            output_tokens: 500,
            cost: 0.001,
            pipeline_budget: 0,
            context_window: 64_000,
            tokens_planning: 0,
            tokens_subagents: 0,
            tokens_critic: 0,
        };
        assert_eq!(b.budget_utilization(), 0.0);
    }

    #[test]
    fn token_budget_utilization_half() {
        let b = TokenBudget {
            input_tokens: 25_000,
            output_tokens: 0,
            cost: 0.0,
            pipeline_budget: 50_000,
            context_window: 64_000,
            tokens_planning: 0,
            tokens_subagents: 0,
            tokens_critic: 0,
        };
        let util = b.budget_utilization();
        assert!((util - 0.5).abs() < 1e-4, "expected 0.5 got {util}");
    }

    #[test]
    fn subsystem_health_shows_oscillation_at_three_restarts() {
        let h = SubsystemHealth {
            metacognitive_should_run: false,
            last_convergence_ratio: 0.5,
            next_round_restarts: 3,
        };
        assert!(h.shows_oscillation());
    }

    #[test]
    fn subsystem_health_no_oscillation_below_three() {
        let h = SubsystemHealth {
            metacognitive_should_run: false,
            last_convergence_ratio: 0.5,
            next_round_restarts: 2,
        };
        assert!(!h.shows_oscillation());
    }
}
