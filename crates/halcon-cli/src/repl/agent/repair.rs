//! `RepairEngine` — one attempt at loop recovery before synthesis injection.
//!
//! Phase 2 addition behind `feature = "repair-loop"`.
//!
//! # Problem
//! When `InLoopCritic` signals `Terminate` (stall detected, no delta progress),
//! the convergence phase immediately injects synthesis. This means:
//! - Stall may be recoverable with a targeted hint.
//! - One hint attempt is cheaper than synthesis + user retry.
//!
//! # Solution
//! Before injecting synthesis on `Terminate`, `RepairEngine::attempt_repair()`
//! is called once. If the repair produces a hint (InjectHint) or triggers a
//! lightweight replan, the round continues. If the repair is exhausted or fails,
//! synthesis proceeds as normal.
//!
//! # Invariants
//! - Repair is attempted AT MOST `max_attempts` times per agent turn.
//! - A failed repair does NOT alter any existing return path.
//! - Repair cannot call tools — it only injects messages and optionally replans.
//! - The existing `MAX_REPLAN_ATTEMPTS` counter is respected.

use serde::{Deserialize, Serialize};

/// Critic signal subset relevant to repair decisions.
///
/// This is a local mirror of the critic signal — avoids importing the full
/// `CriticSignal` type from halcon-agent-core here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RepairTrigger {
    /// Critic signaled Terminate due to stall.
    CriticStall {
        stall_rounds: usize,
        avg_delta: f32,
    },
    /// Critic signaled Replan with a specific reason.
    CriticReplan {
        reason: String,
        alignment_score: f32,
    },
    /// ConvergenceController signaled Halt.
    ConvergenceHalt,
}

/// Outcome of a repair attempt.
#[derive(Debug, Clone)]
pub enum RepairOutcome {
    /// A hint was injected into the message context. Next round will see it.
    HintInjected { hint: String },
    /// A lightweight replan was initiated.
    ReplanTriggered { reason: String },
    /// Repair is exhausted — synthesis should proceed as normal.
    Exhausted { reason: String },
}

impl RepairOutcome {
    /// Whether the repair produced actionable recovery (hint or replan).
    pub fn is_actionable(&self) -> bool {
        matches!(self, Self::HintInjected { .. } | Self::ReplanTriggered { .. })
    }
}

/// Configuration for the repair engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairConfig {
    /// Maximum repair attempts per agent turn. Default: 1.
    pub max_attempts: u32,
    /// Maximum tokens in an injected hint message. Default: 256.
    pub max_hint_tokens: u32,
    /// Whether to allow lightweight replan as a repair action. Default: true.
    pub allow_replan: bool,
}

impl Default for RepairConfig {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            max_hint_tokens: 256,
            allow_replan: true,
        }
    }
}

/// Repair state tracked across the agent turn.
///
/// Stored in `LoopState` (Phase 4 integration). For Phase 2, it is constructed
/// and consulted locally inside `convergence_phase.rs`.
#[derive(Debug, Clone)]
pub struct RepairState {
    pub config: RepairConfig,
    pub attempts_used: u32,
    pub last_trigger: Option<RepairTrigger>,
}

impl RepairState {
    pub fn new(config: RepairConfig) -> Self {
        Self { config, attempts_used: 0, last_trigger: None }
    }

    /// Whether a repair attempt can be made.
    pub fn can_attempt(&self) -> bool {
        self.attempts_used < self.config.max_attempts
    }

    /// Record a repair attempt.
    pub fn record_attempt(&mut self, trigger: RepairTrigger) {
        self.attempts_used += 1;
        self.last_trigger = Some(trigger);
    }
}

/// Stateless repair engine — produces a `RepairOutcome` from a trigger and context.
///
/// Stateless by design: the `RepairState` (mutable) is owned by the caller
/// and passed separately, avoiding borrow issues with `LoopState`.
pub struct RepairEngine;

impl RepairEngine {
    pub fn new() -> Self {
        Self
    }

    /// Attempt recovery from a critic/convergence signal.
    ///
    /// Returns `RepairOutcome::Exhausted` if:
    /// - `state.can_attempt()` is false.
    /// - The trigger does not warrant a recoverable repair.
    ///
    /// Returns `RepairOutcome::HintInjected` with a targeted hint message
    /// that the caller should push into `messages` before the next round.
    ///
    /// Returns `RepairOutcome::ReplanTriggered` when the trigger suggests
    /// structural failure (caller must initiate replan separately).
    ///
    /// # Parameters
    /// - `trigger`: What signal drove the repair request.
    /// - `goal_text`: The user's original goal (for hint construction).
    /// - `round`: Current round number (for context in hint messages).
    /// - `tool_successes`: Tools that ran successfully (for gap analysis).
    /// - `state`: Mutable repair state (tracks attempt count).
    pub fn attempt_repair(
        &self,
        trigger: &RepairTrigger,
        goal_text: &str,
        round: u32,
        tool_successes: &[String],
        state: &mut RepairState,
    ) -> RepairOutcome {
        if !state.can_attempt() {
            return RepairOutcome::Exhausted {
                reason: format!(
                    "repair budget exhausted ({}/{} attempts used)",
                    state.attempts_used, state.config.max_attempts
                ),
            };
        }

        state.record_attempt(trigger.clone());

        match trigger {
            RepairTrigger::CriticStall { stall_rounds, avg_delta } => {
                // Stall: inject a focused re-direction hint.
                let hint = if tool_successes.is_empty() {
                    format!(
                        "[System — Repair] Round {round}: No tools have been called yet. \
                         To make progress on: \"{goal_text}\", \
                         select and call the most relevant tool now. \
                         Do not describe what you will do — invoke the tool immediately."
                    )
                } else {
                    let last_tool = tool_successes.last().map(|s| s.as_str()).unwrap_or("unknown");
                    format!(
                        "[System — Repair] Round {round}: Progress has stalled \
                         (delta={avg_delta:.3}, {stall_rounds} consecutive low-delta rounds). \
                         Last successful tool: {last_tool}. \
                         Focus on the remaining gap in: \"{goal_text}\". \
                         Call the next required tool directly."
                    )
                };

                tracing::debug!(
                    round,
                    stall_rounds,
                    avg_delta,
                    "RepairEngine: injecting stall-recovery hint"
                );

                RepairOutcome::HintInjected { hint }
            }

            RepairTrigger::CriticReplan { reason, alignment_score } => {
                if state.config.allow_replan {
                    tracing::debug!(
                        round,
                        reason,
                        alignment_score,
                        "RepairEngine: triggering lightweight replan"
                    );
                    RepairOutcome::ReplanTriggered {
                        reason: format!(
                            "RepairEngine lightweight replan (alignment={alignment_score:.2}): {reason}"
                        ),
                    }
                } else {
                    RepairOutcome::Exhausted {
                        reason: "replan disabled in RepairConfig".into(),
                    }
                }
            }

            RepairTrigger::ConvergenceHalt => {
                // Convergence halt is a hard stop — repair cannot help.
                RepairOutcome::Exhausted {
                    reason: "ConvergenceHalt is non-recoverable".into(),
                }
            }
        }
    }
}

impl Default for RepairEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_state() -> RepairState {
        RepairState::new(RepairConfig::default())
    }

    #[test]
    fn repair_engine_injects_hint_on_stall() {
        let engine = RepairEngine::new();
        let mut state = default_state();
        let trigger = RepairTrigger::CriticStall { stall_rounds: 3, avg_delta: 0.002 };
        let outcome = engine.attempt_repair(&trigger, "create a file", 4, &[], &mut state);
        assert!(outcome.is_actionable());
        assert!(matches!(outcome, RepairOutcome::HintInjected { .. }));
    }

    #[test]
    fn repair_engine_exhausted_after_max_attempts() {
        let engine = RepairEngine::new();
        let mut state = default_state(); // max_attempts = 1
        let trigger = RepairTrigger::CriticStall { stall_rounds: 2, avg_delta: 0.001 };
        // First attempt should succeed.
        let first = engine.attempt_repair(&trigger, "goal", 2, &[], &mut state);
        assert!(first.is_actionable());
        // Second attempt should be exhausted.
        let second = engine.attempt_repair(&trigger, "goal", 3, &[], &mut state);
        assert!(!second.is_actionable());
        assert!(matches!(second, RepairOutcome::Exhausted { .. }));
    }

    #[test]
    fn repair_engine_convergence_halt_non_recoverable() {
        let engine = RepairEngine::new();
        let mut state = default_state();
        let trigger = RepairTrigger::ConvergenceHalt;
        let outcome = engine.attempt_repair(&trigger, "goal", 5, &[], &mut state);
        assert!(!outcome.is_actionable());
    }

    #[test]
    fn repair_state_tracks_attempts() {
        let mut state = default_state();
        assert!(state.can_attempt());
        state.record_attempt(RepairTrigger::ConvergenceHalt);
        assert!(!state.can_attempt());
        assert_eq!(state.attempts_used, 1);
    }

    #[test]
    fn repair_hint_mentions_tool_when_available() {
        let engine = RepairEngine::new();
        let mut state = default_state();
        let trigger = RepairTrigger::CriticStall { stall_rounds: 1, avg_delta: 0.0 };
        let tools = vec!["file_read".to_string()];
        let outcome = engine.attempt_repair(&trigger, "analyze code", 3, &tools, &mut state);
        if let RepairOutcome::HintInjected { hint } = outcome {
            assert!(hint.contains("file_read"));
        } else {
            panic!("expected HintInjected");
        }
    }

    #[test]
    fn repair_replan_when_allowed() {
        let engine = RepairEngine::new();
        let mut state = RepairState::new(RepairConfig { allow_replan: true, ..Default::default() });
        let trigger = RepairTrigger::CriticReplan { reason: "model diverged".into(), alignment_score: 0.2 };
        let outcome = engine.attempt_repair(&trigger, "goal", 5, &[], &mut state);
        assert!(outcome.is_actionable());
        assert!(matches!(outcome, RepairOutcome::ReplanTriggered { .. }));
    }

    #[test]
    fn repair_replan_exhausted_when_disabled() {
        let engine = RepairEngine::new();
        let mut state = RepairState::new(RepairConfig { allow_replan: false, ..Default::default() });
        let trigger = RepairTrigger::CriticReplan { reason: "drift".into(), alignment_score: 0.1 };
        let outcome = engine.attempt_repair(&trigger, "goal", 5, &[], &mut state);
        assert!(!outcome.is_actionable());
    }
}
