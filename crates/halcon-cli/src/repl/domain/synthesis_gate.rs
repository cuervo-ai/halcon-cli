//! Synthesis governance gate — Phase 2: Synthesis Governance.
//!
//! Centralizes all synthesis decisions under a single pure classification function.
//! Every synthesis callsite routes through `evaluate()` before mutating `LoopState`.
//!
//! # Why a gate?
//! Previously, synthesis was triggered by 10+ independent callsites each making
//! ad-hoc decisions. This creates:
//! - Silent Organic vs Rescue confusion (oracle convergence ≠ tool failure)
//! - No audit trail of WHY synthesis was triggered
//! - Reward pipeline can't distinguish graceful completion from error recovery
//!
//! # Design
//! - `SynthesisTrigger`: 8 typed variants covering all synthesis paths
//! - `SynthesisKind`: Organic (natural completion) vs Rescue (failure recovery)
//! - `SynthesisContext`: pure snapshot of decision-relevant LoopState fields
//! - `SynthesisVerdict`: `{ allow, kind, trigger }` — returned by `evaluate()`
//! - `evaluate()`: pure function, no side effects
//!
//! # Phase 2 constraint
//! `allow` is always `true` in Phase 2 — the gate is classification-only.
//! No synthesis is suppressed. Phase 3 will add suppression logic based on
//! `SynthesisKind` and context signals.

use serde::{Deserialize, Serialize};

// ── SynthesisTrigger ──────────────────────────────────────────────────────────

/// All semantic reasons a synthesis can be triggered.
///
/// Each variant maps to a `SynthesisOrigin` for the existing request queue,
/// but carries richer semantic meaning for classification and observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SynthesisTrigger {
    /// Termination oracle decided convergence is complete (authoritative signal).
    OracleConvergence,
    /// Max rounds reached without natural convergence (budget exhaustion).
    MaxRoundsReached,
    /// Tool loop guard detected Tool↔Text oscillation (heuristic).
    LoopGuard,
    /// All parallel batch subagent steps failed — replan budget exhausted.
    ParallelBatchCollapse,
    /// Supervisor reflection strict-mode permanent failure.
    ReflectionCollapse,
    /// All viable tools exhausted: cache corruption, EBS-B2 boundary, or
    /// evidence gate fired after tool attempts.
    ToolExhaustion,
    /// SLA wall-clock budget expired before convergence.
    ReplanTimeout,
    /// Manual user interrupt (Ctrl+C or cancellation signal).
    ManualInterrupt,
    /// Adaptive control layer detected structural stall or regression.
    ///
    /// Fired by `ProgressPolicy::evaluate_policy()` when `consecutive_stalls >=
    /// stall_threshold` OR `consecutive_regressions >= regression_threshold`.
    /// Always classified as `Rescue` — progress-driven synthesis is recovery.
    GovernanceRescue,
}

// ── SynthesisKind ─────────────────────────────────────────────────────────────

/// Semantic classification of a synthesis event.
///
/// Used by the reward pipeline to distinguish graceful completion from
/// failure recovery. Organic synthesis carries full reward weight; Rescue
/// synthesis applies a partial-credit discount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SynthesisKind {
    /// Natural convergence — model reached its goal through normal execution.
    /// Expected outcome: full coverage, high reward signal.
    Organic,
    /// Failure recovery — synthesis forced due to errors, budget, or guards.
    /// Expected outcome: partial coverage, reduced reward signal.
    Rescue,
}

// ── SynthesisContext ──────────────────────────────────────────────────────────

/// Pure snapshot of LoopState fields relevant to synthesis classification.
///
/// All fields are value types — constructed by `LoopState::build_synthesis_context()`
/// without holding any borrows. Safe to pass across async boundaries.
#[derive(Debug, Clone)]
pub struct SynthesisContext {
    /// Rounds executed so far (0-based, read from `LoopState::rounds`).
    pub rounds_executed: usize,
    /// Configured max rounds for this session (0 = unknown/uncapped).
    pub max_rounds: usize,
    /// Number of replan attempts so far (proxy for parallel failure count).
    pub parallel_failures: usize,
    /// Last convergence ratio (0.0–1.0, from `ConvergenceController`).
    pub reflection_score: f32,
    /// Count of FSM transition errors this session.
    pub fsm_error_count: u32,
    /// Whether tool suppression is NOT active (i.e., tools would run next round).
    pub has_pending_tools: bool,
    /// Convergence stagnation score (0.0 = progressing, 1.0 = fully stagnant).
    pub stagnation_score: f32,
    /// Whether the forced synthesis flag was already set before this evaluation.
    pub forced_flag: bool,
}

// ── SynthesisVerdict ──────────────────────────────────────────────────────────

/// Decision returned by the synthesis gate for a single synthesis event.
///
/// Callsites check `allow` before proceeding with synthesis mutations.
/// In Phase 2 `allow` is always `true` — the gate is observability-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SynthesisVerdict {
    /// Whether synthesis should proceed (Phase 2: always true).
    pub allow: bool,
    /// Semantic classification of this synthesis event.
    pub kind: SynthesisKind,
    /// The trigger that initiated this synthesis evaluation.
    pub trigger: SynthesisTrigger,
}

// ── evaluate ──────────────────────────────────────────────────────────────────

/// Evaluate whether synthesis should proceed and classify it semantically.
///
/// Pure function — no side effects, no `LoopState` mutations.
/// Every synthesis callsite calls this before any state changes.
///
/// # Suppression rule (Phase 3)
/// `GovernanceRescue` synthesis is suppressed (`allow=false`) when the context
/// indicates insufficient data: `reflection_score < 0.15` AND `rounds_executed < 3`.
/// This prevents the adaptive-control layer from forcing a synthesis response
/// immediately after the first stall when the agent has barely started working.
///
/// All other triggers are unconditionally allowed — they represent hard stops
/// (budget exhaustion, tool failure, user interrupt) that must produce a response
/// regardless of how much work has been done.
pub fn evaluate(trigger: SynthesisTrigger, ctx: &SynthesisContext) -> SynthesisVerdict {
    let kind = classify_kind(trigger, ctx);

    // B8 remediation: Block synthesis when there are pending execution tools,
    // UNLESS the trigger is a hard stop that cannot wait.
    //
    // Rationale: Synthesizing while tools are pending produces incomplete results.
    // The agent should execute remaining tools first, then synthesize.
    //
    // Hard stops that override pending tools:
    //   - OracleConvergence: Goal achieved — remaining tools are unnecessary
    //   - ManualInterrupt: User explicitly wants to stop
    //   - MaxRoundsReached: Budget exhaustion, cannot continue
    //   - ReplanTimeout: Wall-clock deadline, must respond now
    let hard_stop = matches!(
        trigger,
        SynthesisTrigger::OracleConvergence
            | SynthesisTrigger::ManualInterrupt
            | SynthesisTrigger::MaxRoundsReached
            | SynthesisTrigger::ReplanTimeout
    );
    let pending_tool_block = ctx.has_pending_tools && !hard_stop;
    if pending_tool_block {
        tracing::info!(
            trigger = ?trigger,
            rounds_executed = ctx.rounds_executed,
            "B8: synthesis suppressed — pending execution tools exist"
        );
    }

    // Suppress GovernanceRescue when convergence ratio is too low to synthesize usefully.
    // Threshold: reflection_score < 0.15 (agent has <15% coverage) AND rounds_executed < 3
    // (very early session). Prevents empty/fabricated synthesis on first-round stalls.
    let governance_suppress = matches!(trigger, SynthesisTrigger::GovernanceRescue)
        && ctx.reflection_score < 0.15
        && ctx.rounds_executed < 3;

    let allow = !pending_tool_block && !governance_suppress;

    // Phase 2: Observability metric for synthesis decisions.
    if !allow {
        tracing::info!(
            metric.synthesis_suppressed = true,
            trigger = ?trigger,
            pending_tool_block,
            governance_suppress,
            rounds_executed = ctx.rounds_executed,
            has_pending_tools = ctx.has_pending_tools,
            "metric: synthesis suppressed by gate"
        );
    }

    SynthesisVerdict {
        allow,
        kind,
        trigger,
    }
}

/// Classify synthesis as Organic or Rescue based on trigger semantics.
///
/// Classification rules (deterministic, context-independent in Phase 2):
/// - `OracleConvergence` → Organic: the oracle decided the goal is met
/// - All others → Rescue: synthesis was forced by a failure or budget constraint
fn classify_kind(trigger: SynthesisTrigger, _ctx: &SynthesisContext) -> SynthesisKind {
    match trigger {
        SynthesisTrigger::OracleConvergence => SynthesisKind::Organic,
        SynthesisTrigger::MaxRoundsReached => SynthesisKind::Rescue,
        SynthesisTrigger::LoopGuard => SynthesisKind::Rescue,
        SynthesisTrigger::ParallelBatchCollapse => SynthesisKind::Rescue,
        SynthesisTrigger::ReflectionCollapse => SynthesisKind::Rescue,
        SynthesisTrigger::ToolExhaustion => SynthesisKind::Rescue,
        SynthesisTrigger::ReplanTimeout => SynthesisKind::Rescue,
        SynthesisTrigger::ManualInterrupt => SynthesisKind::Rescue,
        SynthesisTrigger::GovernanceRescue => SynthesisKind::Rescue,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_default() -> SynthesisContext {
        SynthesisContext {
            rounds_executed: 3,
            max_rounds: 10,
            parallel_failures: 0,
            reflection_score: 0.7,
            fsm_error_count: 0,
            has_pending_tools: false,
            stagnation_score: 0.3,
            forced_flag: false,
        }
    }

    // ── One test per SynthesisTrigger variant ──────────────────────────────

    #[test]
    fn oracle_convergence_is_organic() {
        let v = evaluate(SynthesisTrigger::OracleConvergence, &ctx_default());
        assert!(v.allow);
        assert_eq!(v.kind, SynthesisKind::Organic);
        assert_eq!(v.trigger, SynthesisTrigger::OracleConvergence);
    }

    #[test]
    fn max_rounds_reached_is_rescue() {
        let v = evaluate(SynthesisTrigger::MaxRoundsReached, &ctx_default());
        assert!(v.allow);
        assert_eq!(v.kind, SynthesisKind::Rescue);
    }

    #[test]
    fn loop_guard_is_rescue() {
        let v = evaluate(SynthesisTrigger::LoopGuard, &ctx_default());
        assert!(v.allow);
        assert_eq!(v.kind, SynthesisKind::Rescue);
    }

    #[test]
    fn parallel_batch_collapse_is_rescue() {
        let v = evaluate(SynthesisTrigger::ParallelBatchCollapse, &ctx_default());
        assert!(v.allow);
        assert_eq!(v.kind, SynthesisKind::Rescue);
    }

    #[test]
    fn reflection_collapse_is_rescue() {
        let v = evaluate(SynthesisTrigger::ReflectionCollapse, &ctx_default());
        assert!(v.allow);
        assert_eq!(v.kind, SynthesisKind::Rescue);
    }

    #[test]
    fn tool_exhaustion_is_rescue() {
        let v = evaluate(SynthesisTrigger::ToolExhaustion, &ctx_default());
        assert!(v.allow);
        assert_eq!(v.kind, SynthesisKind::Rescue);
    }

    #[test]
    fn replan_timeout_is_rescue() {
        let v = evaluate(SynthesisTrigger::ReplanTimeout, &ctx_default());
        assert!(v.allow);
        assert_eq!(v.kind, SynthesisKind::Rescue);
    }

    #[test]
    fn manual_interrupt_is_rescue() {
        let v = evaluate(SynthesisTrigger::ManualInterrupt, &ctx_default());
        assert!(v.allow);
        assert_eq!(v.kind, SynthesisKind::Rescue);
    }

    // ── All triggers allowed when no pending tools ─────────────────────────

    #[test]
    fn allow_true_when_no_pending_tools() {
        let all_triggers = [
            SynthesisTrigger::OracleConvergence,
            SynthesisTrigger::MaxRoundsReached,
            SynthesisTrigger::LoopGuard,
            SynthesisTrigger::ParallelBatchCollapse,
            SynthesisTrigger::ReflectionCollapse,
            SynthesisTrigger::ToolExhaustion,
            SynthesisTrigger::ReplanTimeout,
            SynthesisTrigger::ManualInterrupt,
            SynthesisTrigger::GovernanceRescue,
        ];
        let ctx = ctx_default(); // has_pending_tools = false
        for trigger in all_triggers {
            let v = evaluate(trigger, &ctx);
            assert!(
                v.allow,
                "allow must be true for {:?} with no pending tools",
                trigger
            );
        }
    }

    // ── B8: Pending tools suppress non-hard-stop triggers ────────────────

    #[test]
    fn pending_tools_suppress_soft_triggers() {
        let ctx = SynthesisContext {
            has_pending_tools: true,
            ..ctx_default()
        };
        // Soft triggers should be suppressed when tools are pending.
        let soft_triggers = [
            SynthesisTrigger::LoopGuard,
            SynthesisTrigger::ParallelBatchCollapse,
            SynthesisTrigger::ReflectionCollapse,
            SynthesisTrigger::ToolExhaustion,
            SynthesisTrigger::GovernanceRescue,
        ];
        for trigger in soft_triggers {
            let v = evaluate(trigger, &ctx);
            assert!(
                !v.allow,
                "B8: {:?} must be suppressed with pending tools",
                trigger
            );
        }
    }

    #[test]
    fn hard_stops_override_pending_tools() {
        let ctx = SynthesisContext {
            has_pending_tools: true,
            ..ctx_default()
        };
        // Hard stops must always be allowed, even with pending tools.
        // OracleConvergence is a hard stop: goal achieved, remaining tools unnecessary.
        let hard_stops = [
            SynthesisTrigger::OracleConvergence,
            SynthesisTrigger::ManualInterrupt,
            SynthesisTrigger::MaxRoundsReached,
            SynthesisTrigger::ReplanTimeout,
        ];
        for trigger in hard_stops {
            let v = evaluate(trigger, &ctx);
            assert!(
                v.allow,
                "B8: hard stop {:?} must override pending tools",
                trigger
            );
        }
    }

    // ── Verdict carries the trigger that was passed ───────────────────────

    #[test]
    fn verdict_trigger_matches_input() {
        let v = evaluate(SynthesisTrigger::LoopGuard, &ctx_default());
        assert_eq!(v.trigger, SynthesisTrigger::LoopGuard);
    }

    // ── Serde round-trip for SynthesisTrigger and SynthesisKind ──────────

    #[test]
    fn synthesis_trigger_serde_roundtrip() {
        let t = SynthesisTrigger::ParallelBatchCollapse;
        let json = serde_json::to_string(&t).unwrap();
        let back: SynthesisTrigger = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn synthesis_kind_serde_roundtrip() {
        for kind in [SynthesisKind::Organic, SynthesisKind::Rescue] {
            let json = serde_json::to_string(&kind).unwrap();
            let back: SynthesisKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind, "serde roundtrip for {:?}", kind);
        }
    }

    // ── Context fields do not affect classification in Phase 2 ───────────

    #[test]
    fn governance_rescue_is_rescue() {
        let v = evaluate(SynthesisTrigger::GovernanceRescue, &ctx_default());
        assert!(v.allow);
        assert_eq!(v.kind, SynthesisKind::Rescue);
        assert_eq!(v.trigger, SynthesisTrigger::GovernanceRescue);
    }

    #[test]
    fn oracle_convergence_organic_regardless_of_stagnation() {
        let mut ctx = ctx_default();
        ctx.stagnation_score = 1.0; // fully stagnant
        ctx.fsm_error_count = 5;
        let v = evaluate(SynthesisTrigger::OracleConvergence, &ctx);
        assert_eq!(v.kind, SynthesisKind::Organic);
    }

    // ── B3: GovernanceRescue suppression on insufficient data ─────────────

    #[test]
    fn governance_rescue_suppressed_when_insufficient_data() {
        let ctx = SynthesisContext {
            rounds_executed: 1,
            reflection_score: 0.05, // below threshold
            ..ctx_default()
        };
        let v = evaluate(SynthesisTrigger::GovernanceRescue, &ctx);
        assert!(
            !v.allow,
            "GovernanceRescue must be suppressed when data is insufficient"
        );
        assert_eq!(v.kind, SynthesisKind::Rescue);
    }

    #[test]
    fn governance_rescue_allowed_after_enough_rounds() {
        let ctx = SynthesisContext {
            rounds_executed: 3, // at threshold
            reflection_score: 0.05,
            ..ctx_default()
        };
        let v = evaluate(SynthesisTrigger::GovernanceRescue, &ctx);
        assert!(v.allow, "GovernanceRescue must be allowed after 3+ rounds");
    }

    #[test]
    fn governance_rescue_allowed_with_enough_coverage() {
        let ctx = SynthesisContext {
            rounds_executed: 1,
            reflection_score: 0.20, // above threshold
            ..ctx_default()
        };
        let v = evaluate(SynthesisTrigger::GovernanceRescue, &ctx);
        assert!(
            v.allow,
            "GovernanceRescue must be allowed when reflection_score >= 0.15"
        );
    }

    #[test]
    fn non_governance_rescue_triggers_never_suppressed() {
        // All hard-stop triggers are always allowed regardless of context.
        let insufficient_ctx = SynthesisContext {
            rounds_executed: 0,
            reflection_score: 0.0,
            ..ctx_default()
        };
        let hard_stops = [
            SynthesisTrigger::MaxRoundsReached,
            SynthesisTrigger::LoopGuard,
            SynthesisTrigger::ParallelBatchCollapse,
            SynthesisTrigger::ReflectionCollapse,
            SynthesisTrigger::ToolExhaustion,
            SynthesisTrigger::ReplanTimeout,
            SynthesisTrigger::ManualInterrupt,
            SynthesisTrigger::OracleConvergence,
        ];
        for trigger in hard_stops {
            let v = evaluate(trigger, &insufficient_ctx);
            assert!(
                v.allow,
                "Hard-stop trigger {:?} must always be allowed",
                trigger
            );
        }
    }
}
