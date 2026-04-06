//! Phase 3a: FeedbackArbiter — single decision authority per agent turn.
//!
//! ONLY reached when model response has NO tool_use. Tool_use responses
//! bypass this entirely — the loop executes tools and continues immediately.
//!
//! Decision precedence (Xiyo-aligned + Halcon extensions):
//!   1. Hard limits (user cancel, max turns, budget, cost)
//!   2. Recovery (prompt too long, reactive overflow, max output tokens,
//!               stop hook blocked, stagnation, critic feedback)
//!   3. Complete (model says end_turn — Xiyo's primary path)
//!   4. Fallback halt (unknown stop reason)

use halcon_core::types::StopReason;

// ── Turn decision types ──────────────────────────────────────────────────────

/// The single decision returned per agent turn.
/// ONLY evaluated when the model response has no tool_use blocks.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnDecision {
    /// Model completed its task.
    Complete { stop_reason: StopReason },
    /// A recoverable condition — apply the action and retry.
    Recover(RecoveryAction),
    /// An unrecoverable condition — halt the loop.
    Halt(HaltReason),
}

/// Recovery actions. Covers all 4 Xiyo transitions + Halcon extensions.
///
/// Xiyo transitions:
///   - `prompt_too_long_recovery`   → Compact
///   - `reactive_compact_retry`     → ReactiveCompact
///   - `max_output_tokens_escalate` → EscalateTokens
///   - `stop_hook_blocking`         → StopHookBlocked
///
/// Halcon extensions (multi-provider support):
///   - Stagnation detection         → Replan / FallbackProvider
///   - Critic feedback              → ReplanWithFeedback
#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryAction {
    /// Prompt too long (pre-invocation detection) — compact + retry.
    Compact,
    /// Mid-stream context overflow — aggressive compaction + retry.
    ReactiveCompact,
    /// Hit max output tokens — raise limit + retry.
    EscalateTokens,
    /// Primary provider failed — try fallback.
    FallbackProvider,
    /// Lifecycle hook blocked model stop — continue loop.
    StopHookBlocked,
    /// Stagnation detected — force replan with context.
    Replan { reason: String },
    /// Critic identified gaps — replan with feedback.
    ReplanWithFeedback(String),
}

/// Reasons for halting the agent loop.
#[derive(Debug, Clone, PartialEq)]
pub enum HaltReason {
    MaxTurnsReached,
    UserCancelled,
    BudgetExhausted,
    CostLimitExceeded { spent_usd: f64, limit_usd: f64 },
    StagnationAbort { consecutive_stalls: u32 },
    DiminishingReturns,
    UnrecoverableError(String),
}

// ── Aggregated signals ───────────────────────────────────────────────────────

/// Signals collected from the loop state. Kept minimal — arbiter doesn't
/// reach into loop internals.
#[derive(Debug)]
pub struct AggregatedSignals {
    /// Critic/supervisor feedback. Empty/whitespace-only is treated as None.
    pub critic_feedback: Option<String>,
    /// User pressed Ctrl+C or sent cancel via control channel.
    pub user_cancelled: bool,
    /// Lifecycle hook blocked model's stop.
    pub stop_hook_blocked: bool,
    /// Consecutive rounds with no meaningful progress (same tool calls).
    pub consecutive_stalls: u32,
    /// USD spent so far this session.
    pub cost_usd: f64,
    /// USD budget limit (0.0 = unlimited).
    pub cost_limit_usd: f64,
    /// Number of EscalateTokens recoveries so far.
    pub escalation_count: u32,
    /// Maximum allowed escalation attempts.
    pub max_escalation_attempts: u32,
    /// Number of Compact recoveries applied so far (prompt-too-long).
    pub compact_count: u32,
    /// Maximum allowed compaction attempts before halting.
    pub max_compact_attempts: u32,
    /// Number of Replan recoveries applied so far (stagnation/critic).
    pub replan_count: u32,
    /// Maximum allowed replan attempts per session.
    pub max_replan_attempts: u32,
    /// Whether diminishing returns detected (delta < threshold for 2 consecutive).
    pub diminishing_returns: bool,
}

impl Default for AggregatedSignals {
    fn default() -> Self {
        Self {
            critic_feedback: None,
            user_cancelled: false,
            stop_hook_blocked: false,
            consecutive_stalls: 0,
            cost_usd: 0.0,
            cost_limit_usd: 0.0,
            escalation_count: 0,
            max_escalation_attempts: 3, // Xiyo: MAX_OUTPUT_TOKENS_RECOVERY_LIMIT = 3
            compact_count: 0,
            max_compact_attempts: 2, // Spec: max 2 compaction attempts
            replan_count: 0,
            max_replan_attempts: 2, // Spec: max 2 replan attempts
            diminishing_returns: false,
        }
    }
}

// ── Response summary ─────────────────────────────────────────────────────────

/// Minimal view of a model response for the arbiter.
/// No `has_tool_use` — tool_use responses bypass the arbiter entirely.
#[derive(Debug)]
pub struct TurnResponse {
    pub stop_reason: StopReason,
    pub is_prompt_too_long: bool,
    pub hit_max_output_tokens: bool,
    pub is_reactive_overflow: bool,
}

/// Minimal loop state for the arbiter.
#[derive(Debug)]
pub struct TurnState {
    pub turn_count: u32,
    pub max_turns: u32,
    pub budget_exhausted: bool,
}

// ── FeedbackArbiter ──────────────────────────────────────────────────────────

/// Single decision point per turn. Stateless — pure function of inputs.
/// Replaces: TerminationOracle, ConvergenceController, ProgressPolicy,
/// SynthesisGate, BoundaryDecisionEngine.
#[derive(Default)]
pub struct FeedbackArbiter;

/// Stagnation threshold: abort after this many consecutive stalls.
const STAGNATION_ABORT_THRESHOLD: u32 = 5;
/// Stagnation threshold: replan after this many consecutive stalls.
const STAGNATION_REPLAN_THRESHOLD: u32 = 3;

impl FeedbackArbiter {
    pub fn new() -> Self {
        Self
    }

    /// Single decision point. PRECONDITION: response has no tool_use.
    pub fn decide(
        &self,
        response: &TurnResponse,
        state: &TurnState,
        signals: &AggregatedSignals,
    ) -> TurnDecision {
        // ── 1. Hard limits (unconditional halts) ─────────────────────────
        if signals.user_cancelled {
            tracing::debug!(turn = state.turn_count, "arbiter: HALT — user cancelled");
            return TurnDecision::Halt(HaltReason::UserCancelled);
        }

        if state.turn_count >= state.max_turns {
            tracing::debug!(
                turn = state.turn_count,
                max = state.max_turns,
                "arbiter: HALT — max turns"
            );
            return TurnDecision::Halt(HaltReason::MaxTurnsReached);
        }

        if state.budget_exhausted {
            tracing::debug!(turn = state.turn_count, "arbiter: HALT — token budget");
            return TurnDecision::Halt(HaltReason::BudgetExhausted);
        }

        if signals.cost_limit_usd > 0.0 && signals.cost_usd >= signals.cost_limit_usd {
            tracing::debug!(
                spent = signals.cost_usd,
                limit = signals.cost_limit_usd,
                "arbiter: HALT — cost limit"
            );
            return TurnDecision::Halt(HaltReason::CostLimitExceeded {
                spent_usd: signals.cost_usd,
                limit_usd: signals.cost_limit_usd,
            });
        }

        // Stagnation abort (hard halt after too many stalls)
        if signals.consecutive_stalls >= STAGNATION_ABORT_THRESHOLD {
            tracing::warn!(
                stalls = signals.consecutive_stalls,
                "arbiter: HALT — stagnation abort"
            );
            return TurnDecision::Halt(HaltReason::StagnationAbort {
                consecutive_stalls: signals.consecutive_stalls,
            });
        }

        // Diminishing returns — soft signal (Frontier AAA).
        //
        // Token-based diminishing returns alone is unreliable: two short responses
        // may be perfectly productive (e.g., "run test" → "test passed"). Only halt
        // when BOTH token deltas are low AND the stagnation tracker confirms repeated
        // patterns. This prevents false positives on short but productive rounds.
        //
        // Paper: Renze 2024 — "self-reflection most effective when accuracy is low;
        // for easier prompts, reflection can cause performance deterioration."
        if signals.diminishing_returns && signals.consecutive_stalls >= 2 {
            tracing::info!(
                stalls = signals.consecutive_stalls,
                "arbiter: HALT — diminishing returns confirmed by stagnation"
            );
            return TurnDecision::Halt(HaltReason::DiminishingReturns);
        }

        // ── 2. Recovery (typed transitions, bounded) ─────────────────────
        if response.is_prompt_too_long {
            if signals.compact_count >= signals.max_compact_attempts {
                tracing::warn!(
                    attempts = signals.compact_count,
                    "arbiter: HALT — compaction exhausted"
                );
                return TurnDecision::Halt(HaltReason::UnrecoverableError(format!(
                    "prompt_too_long recovery exhausted after {} compaction attempts",
                    signals.compact_count
                )));
            }
            tracing::debug!(
                attempt = signals.compact_count + 1,
                max = signals.max_compact_attempts,
                "arbiter: RECOVER — prompt too long → compact"
            );
            return TurnDecision::Recover(RecoveryAction::Compact);
        }

        if response.is_reactive_overflow {
            if signals.compact_count >= signals.max_compact_attempts {
                tracing::warn!("arbiter: HALT — reactive compaction exhausted");
                return TurnDecision::Halt(HaltReason::UnrecoverableError(
                    "reactive compaction exhausted".to_string(),
                ));
            }
            tracing::debug!("arbiter: RECOVER — reactive overflow → aggressive compact");
            return TurnDecision::Recover(RecoveryAction::ReactiveCompact);
        }

        // Max output tokens with recovery counter (Xiyo: max 3 attempts)
        if response.hit_max_output_tokens {
            if signals.escalation_count >= signals.max_escalation_attempts {
                tracing::warn!(
                    attempts = signals.escalation_count,
                    "arbiter: HALT — escalation exhausted"
                );
                return TurnDecision::Halt(HaltReason::UnrecoverableError(format!(
                    "max_output_tokens recovery exhausted after {} attempts",
                    signals.escalation_count
                )));
            }
            tracing::debug!(
                attempt = signals.escalation_count + 1,
                max = signals.max_escalation_attempts,
                "arbiter: RECOVER — max output tokens → escalate"
            );
            return TurnDecision::Recover(RecoveryAction::EscalateTokens);
        }

        // StopHookBlocked BEFORE critic/stagnation — governance decision
        if signals.stop_hook_blocked {
            tracing::debug!("arbiter: RECOVER — stop hook blocked");
            return TurnDecision::Recover(RecoveryAction::StopHookBlocked);
        }

        // Stagnation replan (soft recovery, bounded by max_replan_attempts)
        if signals.consecutive_stalls >= STAGNATION_REPLAN_THRESHOLD {
            if signals.replan_count >= signals.max_replan_attempts {
                tracing::warn!(
                    replans = signals.replan_count,
                    stalls = signals.consecutive_stalls,
                    "arbiter: stagnation replan exhausted — allowing natural progression"
                );
                // Don't halt — let the stagnation abort threshold (5) handle it
            } else {
                tracing::debug!(
                    stalls = signals.consecutive_stalls,
                    replan_attempt = signals.replan_count + 1,
                    max = signals.max_replan_attempts,
                    "arbiter: RECOVER — stagnation → replan"
                );
                return TurnDecision::Recover(RecoveryAction::Replan {
                    reason: format!(
                        "{} consecutive stalled rounds detected",
                        signals.consecutive_stalls
                    ),
                });
            }
        }

        // Critic feedback (non-empty only, bounded by replan limit)
        if let Some(feedback) = &signals.critic_feedback {
            let trimmed = feedback.trim();
            if !trimmed.is_empty() {
                if signals.replan_count >= signals.max_replan_attempts {
                    tracing::debug!("arbiter: critic replan exhausted — ignoring feedback");
                } else {
                    tracing::debug!(feedback = %trimmed, "arbiter: RECOVER — critic → replan");
                    return TurnDecision::Recover(RecoveryAction::ReplanWithFeedback(
                        trimmed.to_owned(),
                    ));
                }
            }
        }

        // ── 3. Model says end_turn → complete (Xiyo primary path) ────────
        if response.stop_reason == StopReason::EndTurn {
            tracing::debug!(turn = state.turn_count, "arbiter: COMPLETE — end_turn");
            return TurnDecision::Complete {
                stop_reason: StopReason::EndTurn,
            };
        }

        // ── 4. Fallback halt ─────────────────────────────────────────────
        let msg = format!("unexpected stop reason: {:?}", response.stop_reason);
        tracing::warn!(turn = state.turn_count, "{msg}");
        TurnDecision::Halt(HaltReason::UnrecoverableError(msg))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn arbiter() -> FeedbackArbiter {
        FeedbackArbiter::new()
    }

    fn resp() -> TurnResponse {
        TurnResponse {
            stop_reason: StopReason::EndTurn,
            is_prompt_too_long: false,
            hit_max_output_tokens: false,
            is_reactive_overflow: false,
        }
    }

    fn state() -> TurnState {
        TurnState {
            turn_count: 0,
            max_turns: 20,
            budget_exhausted: false,
        }
    }

    fn sigs() -> AggregatedSignals {
        AggregatedSignals::default()
    }

    // ── Hard limits ──────────────────────────────────────────────────────

    #[test]
    fn halt_user_cancelled() {
        let sig = AggregatedSignals {
            user_cancelled: true,
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sig),
            TurnDecision::Halt(HaltReason::UserCancelled)
        );
    }

    #[test]
    fn halt_max_turns() {
        let s = TurnState {
            turn_count: 20,
            max_turns: 20,
            budget_exhausted: false,
        };
        assert_eq!(
            arbiter().decide(&resp(), &s, &sigs()),
            TurnDecision::Halt(HaltReason::MaxTurnsReached)
        );
    }

    #[test]
    fn halt_budget() {
        let s = TurnState {
            turn_count: 5,
            max_turns: 20,
            budget_exhausted: true,
        };
        assert_eq!(
            arbiter().decide(&resp(), &s, &sigs()),
            TurnDecision::Halt(HaltReason::BudgetExhausted)
        );
    }

    #[test]
    fn halt_cost_limit() {
        let sig = AggregatedSignals {
            cost_usd: 5.0,
            cost_limit_usd: 4.0,
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sig),
            TurnDecision::Halt(HaltReason::CostLimitExceeded {
                spent_usd: 5.0,
                limit_usd: 4.0
            })
        );
    }

    #[test]
    fn halt_stagnation_abort() {
        let sig = AggregatedSignals {
            consecutive_stalls: 5,
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sig),
            TurnDecision::Halt(HaltReason::StagnationAbort {
                consecutive_stalls: 5
            })
        );
    }

    // ── Recovery ─────────────────────────────────────────────────────────

    #[test]
    fn recover_compact() {
        let r = TurnResponse {
            is_prompt_too_long: true,
            ..resp()
        };
        assert_eq!(
            arbiter().decide(&r, &state(), &sigs()),
            TurnDecision::Recover(RecoveryAction::Compact)
        );
    }

    #[test]
    fn recover_reactive_compact() {
        let r = TurnResponse {
            is_reactive_overflow: true,
            ..resp()
        };
        assert_eq!(
            arbiter().decide(&r, &state(), &sigs()),
            TurnDecision::Recover(RecoveryAction::ReactiveCompact)
        );
    }

    #[test]
    fn recover_escalate() {
        let r = TurnResponse {
            hit_max_output_tokens: true,
            ..resp()
        };
        assert_eq!(
            arbiter().decide(&r, &state(), &sigs()),
            TurnDecision::Recover(RecoveryAction::EscalateTokens)
        );
    }

    #[test]
    fn recover_stop_hook() {
        let sig = AggregatedSignals {
            stop_hook_blocked: true,
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sig),
            TurnDecision::Recover(RecoveryAction::StopHookBlocked)
        );
    }

    #[test]
    fn recover_stagnation_replan() {
        let sig = AggregatedSignals {
            consecutive_stalls: 3,
            ..Default::default()
        };
        match arbiter().decide(&resp(), &state(), &sig) {
            TurnDecision::Recover(RecoveryAction::Replan { .. }) => {}
            other => panic!("expected Replan, got {other:?}"),
        }
    }

    #[test]
    fn recover_critic_feedback() {
        let sig = AggregatedSignals {
            critic_feedback: Some("missing edge case handling".into()),
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sig),
            TurnDecision::Recover(RecoveryAction::ReplanWithFeedback(
                "missing edge case handling".into()
            ))
        );
    }

    #[test]
    fn empty_critic_feedback_ignored() {
        let sig = AggregatedSignals {
            critic_feedback: Some("   ".into()),
            ..Default::default()
        };
        // Should complete, not replan
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sig),
            TurnDecision::Complete {
                stop_reason: StopReason::EndTurn
            }
        );
    }

    // ── Complete ─────────────────────────────────────────────────────────

    #[test]
    fn complete_end_turn() {
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sigs()),
            TurnDecision::Complete {
                stop_reason: StopReason::EndTurn
            }
        );
    }

    // ── Fallback halt ────────────────────────────────────────────────────

    #[test]
    fn halt_unknown_stop() {
        let r = TurnResponse {
            stop_reason: StopReason::StopSequence,
            ..resp()
        };
        match arbiter().decide(&r, &state(), &sigs()) {
            TurnDecision::Halt(HaltReason::UnrecoverableError(m)) => {
                assert!(m.contains("StopSequence"));
            }
            other => panic!("expected Halt(UnrecoverableError), got {other:?}"),
        }
    }

    // ── Precedence ───────────────────────────────────────────────────────

    #[test]
    fn cancel_beats_max_turns() {
        let s = TurnState {
            turn_count: 20,
            max_turns: 20,
            budget_exhausted: false,
        };
        let sig = AggregatedSignals {
            user_cancelled: true,
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&resp(), &s, &sig),
            TurnDecision::Halt(HaltReason::UserCancelled)
        );
    }

    #[test]
    fn hard_limits_beat_recovery() {
        let r = TurnResponse {
            is_prompt_too_long: true,
            ..resp()
        };
        let s = TurnState {
            turn_count: 20,
            max_turns: 20,
            budget_exhausted: false,
        };
        assert_eq!(
            arbiter().decide(&r, &s, &sigs()),
            TurnDecision::Halt(HaltReason::MaxTurnsReached)
        );
    }

    #[test]
    fn compact_beats_reactive() {
        let r = TurnResponse {
            is_prompt_too_long: true,
            is_reactive_overflow: true,
            ..resp()
        };
        assert_eq!(
            arbiter().decide(&r, &state(), &sigs()),
            TurnDecision::Recover(RecoveryAction::Compact)
        );
    }

    #[test]
    fn hook_beats_stagnation() {
        let sig = AggregatedSignals {
            stop_hook_blocked: true,
            consecutive_stalls: 3,
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sig),
            TurnDecision::Recover(RecoveryAction::StopHookBlocked)
        );
    }

    #[test]
    fn stop_hook_prevents_completion() {
        let sig = AggregatedSignals {
            stop_hook_blocked: true,
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sig),
            TurnDecision::Recover(RecoveryAction::StopHookBlocked)
        );
    }

    #[test]
    fn one_turn_before_max_allows_completion() {
        let s = TurnState {
            turn_count: 19,
            max_turns: 20,
            budget_exhausted: false,
        };
        assert_eq!(
            arbiter().decide(&resp(), &s, &sigs()),
            TurnDecision::Complete {
                stop_reason: StopReason::EndTurn
            }
        );
    }

    #[test]
    fn stagnation_abort_beats_replan() {
        let sig = AggregatedSignals {
            consecutive_stalls: 5,
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sig),
            TurnDecision::Halt(HaltReason::StagnationAbort {
                consecutive_stalls: 5
            })
        );
    }

    // ── Recovery counter tests ───────────────────────────────────────────

    #[test]
    fn escalate_allowed_when_under_limit() {
        let r = TurnResponse {
            hit_max_output_tokens: true,
            ..resp()
        };
        let sig = AggregatedSignals {
            escalation_count: 1,
            max_escalation_attempts: 3,
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&r, &state(), &sig),
            TurnDecision::Recover(RecoveryAction::EscalateTokens)
        );
    }

    #[test]
    fn escalate_halts_when_exhausted() {
        let r = TurnResponse {
            hit_max_output_tokens: true,
            ..resp()
        };
        let sig = AggregatedSignals {
            escalation_count: 3,
            max_escalation_attempts: 3,
            ..Default::default()
        };
        match arbiter().decide(&r, &state(), &sig) {
            TurnDecision::Halt(HaltReason::UnrecoverableError(m)) => {
                assert!(m.contains("exhausted"));
            }
            other => panic!("expected Halt, got {other:?}"),
        }
    }

    #[test]
    fn diminishing_returns_alone_allows_completion() {
        // Frontier AAA: diminishing returns alone is NOT enough to halt.
        // Without stagnation confirmation, model completes normally.
        let sig = AggregatedSignals {
            diminishing_returns: true,
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sig),
            TurnDecision::Complete {
                stop_reason: StopReason::EndTurn
            }
        );
    }

    #[test]
    fn diminishing_returns_with_stagnation_halts() {
        // Frontier AAA: diminishing returns + stagnation = confirmed halt.
        let sig = AggregatedSignals {
            diminishing_returns: true,
            consecutive_stalls: 2,
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sig),
            TurnDecision::Halt(HaltReason::DiminishingReturns)
        );
    }

    #[test]
    fn diminishing_returns_with_stagnation_beats_completion() {
        // Combined signal overrides normal completion
        let sig = AggregatedSignals {
            diminishing_returns: true,
            consecutive_stalls: 3,
            ..Default::default()
        };
        assert_eq!(
            arbiter().decide(&resp(), &state(), &sig),
            TurnDecision::Halt(HaltReason::DiminishingReturns)
        );
    }
}
