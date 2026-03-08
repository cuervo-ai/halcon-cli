//! Formal per-round intelligence aggregate — Sprint 1 of SOTA 2026 L6 architecture.
//!
//! `RoundFeedback` replaces 4 scattered signal types with a single domain entity
//! consumed by `TerminationOracle` (Sprint 2) and `AdaptivePolicy` (Sprint 3).
//!
//! # Data flow
//! ```text
//! RoundScorer.score_round()       → combined_score, replan_advised, synthesis_advised
//! ConvergenceController.observe() → convergence_action
//! LoopGuard.record_round()        → loop_signal (mapped from LoopAction)
//! ──────────────────────────────────────────────────────────────────────
//! RoundFeedback (this type)       → TerminationOracle + AdaptivePolicy
//! ```

use super::convergence_controller::ConvergenceAction;

// ── LoopSignal ────────────────────────────────────────────────────────────────

/// Domain abstraction of the infrastructure `LoopAction`.
///
/// Mapping from `LoopAction` → `LoopSignal` happens at the infrastructure boundary
/// in `agent/mod.rs`. The domain layer never imports `LoopAction` directly, preserving
/// domain purity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopSignal {
    /// Normal — proceed to next round.
    Continue,
    /// Suppress tools next round (Ollama emulation or force-no-tools flag).
    ForceNoTools,
    /// Force the model to synthesize and end the current tool phase.
    InjectSynthesis,
    /// Current approach is stagnating — request a plan regeneration.
    ReplanRequired,
    /// Hard stop — no further rounds should execute.
    Break,
}

impl LoopSignal {
    /// Returns `true` if this signal requires the agent loop to terminate immediately.
    ///
    /// Only `Break` is terminal — all other signals allow at least one more round.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Break)
    }

    /// Returns `true` if this signal causes a disruptive change to the loop's trajectory.
    ///
    /// Disruptive signals are: `Break` (halt), `ReplanRequired` (new plan generated),
    /// and `InjectSynthesis` (model forced to synthesize). `ForceNoTools` and `Continue`
    /// are non-disruptive — they modify the next round without changing the overall plan.
    pub fn is_disruptive(&self) -> bool {
        matches!(self, Self::Break | Self::ReplanRequired | Self::InjectSynthesis)
    }
}

// ── RoundFeedback ─────────────────────────────────────────────────────────────

/// Consolidated per-round intelligence signal.
///
/// Aggregates signals from three independent decision systems:
/// - `RoundScorer` → `combined_score`, `trajectory_trend`, `oscillation`, `replan_advised`, `synthesis_advised`
/// - `ConvergenceController` → `convergence_action`
/// - `ToolLoopGuard` → `loop_signal`
///
/// This is the data contract consumed by `TerminationOracle` and `AdaptivePolicy`.
/// Constructed once per round at the infrastructure boundary (`agent/mod.rs`) after
/// all three evaluation systems have run.
#[derive(Debug, Clone)]
pub struct RoundFeedback {
    /// 0-based round index within the current agent session.
    pub round: usize,
    /// Weighted blend score for this round from `RoundScorer.score_round()`.
    /// Range: [0.0, 1.0]. Higher = better round performance.
    pub combined_score: f32,
    /// Recommended action from `ConvergenceController.observe_round()`.
    pub convergence_action: ConvergenceAction,
    /// Mapped action from `ToolLoopGuard.record_round()` via `LoopAction → LoopSignal`.
    pub loop_signal: LoopSignal,
    /// Mean combined score over last 5 rounds from `RoundScorer.trend_score()`.
    /// Range: [0.0, 1.0]. Falling trend → declining trajectory.
    pub trajectory_trend: f32,
    /// Score variance over full history from `RoundScorer.oscillation_penalty()`.
    /// Range: [0.0, ∞). High value → erratic / oscillating rounds.
    pub oscillation: f32,
    /// Whether `RoundScorer.should_trigger_replan()` fired this round.
    pub replan_advised: bool,
    /// Whether `RoundScorer.should_inject_synthesis()` fired this round.
    pub synthesis_advised: bool,
    /// `true` when at least one tool executed this round (vs text-only round).
    pub tool_round: bool,
    /// `true` when at least one tool returned an error this round.
    pub had_errors: bool,
    /// Mini-critic recommends forcing a replan (stalled session).
    /// Fed as INPUT to oracle — oracle Halt always overrides this.
    pub mini_critic_replan: bool,
    /// Mini-critic recommends forcing synthesis (budget pressure).
    /// Fed as INPUT to oracle — oracle Halt always overrides this.
    pub mini_critic_synthesis: bool,
    /// EvidenceGraph synthesis coverage [0.0, 1.0].
    /// Low coverage signals the oracle to prefer Continue over InjectSynthesis
    /// when budget remains (delay synthesis until more evidence is collected).
    pub evidence_coverage: f64,

    // ── Phase 3 fields ──────────────────────────────────────────────────
    /// Semantic cycle detected this round (P3.3). Default: false.
    pub semantic_cycle_detected: bool,
    /// Cycle severity [0.0, 1.0] (P3.3). Default: 0.0.
    pub cycle_severity: f32,
    /// Convergence utility score (P3.6). Default: 0.5.
    pub utility_score: f64,
    /// Mid-loop critic action recommendation (P3.4). Default: None.
    pub mid_critic_action: Option<super::mid_loop_critic::CriticAction>,
    /// Whether complexity was upgraded this round (P3.5). Default: false.
    pub complexity_upgraded: bool,

    // ── Phase 5 fields ──────────────────────────────────────────────────
    /// Problem class inferred for this session (P5.2). Default: None.
    pub problem_class: Option<super::problem_classifier::ProblemClass>,
    /// Estimated rounds remaining to convergence (P5.4). Default: None.
    pub forecast_rounds_remaining: Option<usize>,

    // ── Phase 4 fields ──────────────────────────────────────────────────
    /// Whether the utility function recommends synthesizing this round (P3.6/P4). Default: false.
    pub utility_should_synthesize: bool,
    /// Number of `request_synthesis()` calls recorded this round (P4). Default: 0.
    pub synthesis_request_count: u32,
    /// Number of FSM transition errors this round (P4). Default: 0.
    pub fsm_error_count: u32,
    /// Budget manager iteration count (P4). Default: 0.
    pub budget_iteration_count: u32,
    /// Number of consecutive stagnation rounds detected (P4). Default: 0.
    pub budget_stagnation_count: u32,
    /// K5-2 token growth metric this round (P4). Default: 0.
    pub budget_token_growth: u32,
    /// Whether the token budget is fully exhausted (P4). Default: false.
    pub budget_exhausted: bool,
    /// Number of executive override signals fired this round (P4). Default: 0.
    pub executive_signal_count: u32,
    /// Reason for executive force, if any (P4). Default: None.
    pub executive_force_reason: Option<String>,
    /// Capability violation description, if any (P3.2/P4). Default: None.
    pub capability_violation: Option<String>,

    // ── Routing-adaptor fields ───────────────────────────────────────────
    /// Security-related signals discovered in tool results this round. Default: false.
    pub security_signals_detected: bool,
    /// Total tool calls executed this round. Default: 0.
    pub tool_call_count: u32,
    /// Tool calls that returned an error this round. Default: 0.
    pub tool_failure_count: u32,

    // ── GovernanceRescue gate (ARCH-SYNC-1 fix) ──────────────────────────
    /// `true` when SynthesisGate::GovernanceRescue would block synthesis.
    ///
    /// Set in `convergence_phase` BEFORE `TerminationOracle::adjudicate()`.
    /// When `true`, the oracle MUST downgrade any `InjectSynthesis` decision to `Continue`
    /// because synthesis quality would be below the minimum reflection threshold.
    /// Conditions: `reflection_score < 0.15 AND rounds_executed < 3`.
    pub governance_rescue_active: bool,
}

// ── Default ───────────────────────────────────────────────────────────────────

impl Default for RoundFeedback {
    fn default() -> Self {
        Self {
            round: 0,
            combined_score: 0.5,
            convergence_action: ConvergenceAction::Continue,
            loop_signal: LoopSignal::Continue,
            trajectory_trend: 0.5,
            oscillation: 0.0,
            replan_advised: false,
            synthesis_advised: false,
            tool_round: false,
            had_errors: false,
            mini_critic_replan: false,
            mini_critic_synthesis: false,
            evidence_coverage: 1.0,
            semantic_cycle_detected: false,
            cycle_severity: 0.0,
            utility_score: 0.5,
            mid_critic_action: None,
            complexity_upgraded: false,
            problem_class: None,
            forecast_rounds_remaining: None,
            utility_should_synthesize: false,
            synthesis_request_count: 0,
            fsm_error_count: 0,
            budget_iteration_count: 0,
            budget_stagnation_count: 0,
            budget_token_growth: 0,
            budget_exhausted: false,
            executive_signal_count: 0,
            executive_force_reason: None,
            capability_violation: None,
            security_signals_detected: false,
            tool_call_count: 0,
            tool_failure_count: 0,
            governance_rescue_active: false,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_feedback(
        loop_signal: LoopSignal,
        convergence_action: ConvergenceAction,
        replan_advised: bool,
        synthesis_advised: bool,
    ) -> RoundFeedback {
        RoundFeedback {
            round: 0,
            combined_score: 0.5,
            convergence_action,
            loop_signal,
            trajectory_trend: 0.5,
            oscillation: 0.0,
            replan_advised,
            synthesis_advised,
            tool_round: true,
            had_errors: false,
            mini_critic_replan: false,
            mini_critic_synthesis: false,
            evidence_coverage: 1.0,
            semantic_cycle_detected: false,
            cycle_severity: 0.0,
            utility_score: 0.5,
            mid_critic_action: None,
            complexity_upgraded: false,
            problem_class: None,
            forecast_rounds_remaining: None,
            utility_should_synthesize: false,
            synthesis_request_count: 0,
            fsm_error_count: 0,
            budget_iteration_count: 0,
            budget_stagnation_count: 0,
            budget_token_growth: 0,
            budget_exhausted: false,
            executive_signal_count: 0,
            executive_force_reason: None,
            capability_violation: None,
            security_signals_detected: false,
            tool_call_count: 0,
            tool_failure_count: 0,
            governance_rescue_active: false,
        }
    }

    #[test]
    fn loop_signal_is_terminal_only_for_break() {
        assert!(LoopSignal::Break.is_terminal());
        assert!(!LoopSignal::Continue.is_terminal());
        assert!(!LoopSignal::ForceNoTools.is_terminal());
        assert!(!LoopSignal::InjectSynthesis.is_terminal());
        assert!(!LoopSignal::ReplanRequired.is_terminal());
    }

    #[test]
    fn loop_signal_is_disruptive_for_break_replan_synthesis() {
        assert!(LoopSignal::Break.is_disruptive());
        assert!(LoopSignal::ReplanRequired.is_disruptive());
        assert!(LoopSignal::InjectSynthesis.is_disruptive());
        assert!(!LoopSignal::Continue.is_disruptive());
        assert!(!LoopSignal::ForceNoTools.is_disruptive());
    }

    #[test]
    fn round_feedback_construction_defaults_sane() {
        let fb = make_feedback(LoopSignal::Continue, ConvergenceAction::Continue, false, false);
        assert_eq!(fb.round, 0);
        assert!((fb.combined_score - 0.5).abs() < f32::EPSILON);
        assert!(!fb.replan_advised);
        assert!(!fb.synthesis_advised);
        assert!(fb.tool_round);
        assert!(!fb.had_errors);
    }

    #[test]
    fn continue_feedback_is_not_disruptive() {
        let fb = make_feedback(LoopSignal::Continue, ConvergenceAction::Continue, false, false);
        assert!(!fb.loop_signal.is_disruptive());
        assert!(!fb.loop_signal.is_terminal());
    }

    #[test]
    fn break_feedback_has_terminal_signal() {
        let fb = make_feedback(LoopSignal::Break, ConvergenceAction::Continue, false, false);
        assert!(fb.loop_signal.is_terminal());
        assert!(fb.loop_signal.is_disruptive());
    }

    #[test]
    fn replan_feedback_convergence_and_loop_signal_both_set() {
        let fb = make_feedback(
            LoopSignal::ReplanRequired,
            ConvergenceAction::Replan,
            true,
            false,
        );
        assert_eq!(fb.convergence_action, ConvergenceAction::Replan);
        assert_eq!(fb.loop_signal, LoopSignal::ReplanRequired);
        assert!(fb.replan_advised);
        assert!(fb.loop_signal.is_disruptive());
    }

    #[test]
    fn synthesis_feedback_correctly_classified() {
        let fb = make_feedback(
            LoopSignal::InjectSynthesis,
            ConvergenceAction::Synthesize,
            false,
            true,
        );
        assert_eq!(fb.convergence_action, ConvergenceAction::Synthesize);
        assert_eq!(fb.loop_signal, LoopSignal::InjectSynthesis);
        assert!(fb.synthesis_advised);
        assert!(fb.loop_signal.is_disruptive());
    }

    #[test]
    fn zero_score_feedback_flagged_as_low() {
        let mut fb = make_feedback(LoopSignal::Continue, ConvergenceAction::Continue, false, false);
        fb.combined_score = 0.0;
        assert!(fb.combined_score < 0.15, "zero score should be below replan threshold");
    }

    #[test]
    fn oscillation_feedback_correctly_tracked() {
        let mut fb = make_feedback(LoopSignal::Continue, ConvergenceAction::Continue, false, false);
        fb.oscillation = 0.25;
        assert!(fb.oscillation > 0.15, "oscillation above threshold should be detectable");
    }

    #[test]
    fn tool_round_vs_text_round_distinguished() {
        let mut tool_fb = make_feedback(LoopSignal::Continue, ConvergenceAction::Continue, false, false);
        tool_fb.tool_round = true;
        assert!(tool_fb.tool_round);

        let mut text_fb = make_feedback(LoopSignal::Continue, ConvergenceAction::Continue, false, false);
        text_fb.tool_round = false;
        assert!(!text_fb.tool_round);
    }

    #[test]
    fn force_no_tools_signal_is_not_terminal_but_not_disruptive() {
        let fb = make_feedback(LoopSignal::ForceNoTools, ConvergenceAction::Continue, false, false);
        assert!(!fb.loop_signal.is_terminal());
        assert!(!fb.loop_signal.is_disruptive());
    }

    #[test]
    fn had_errors_flag_propagates_correctly() {
        let mut fb = make_feedback(LoopSignal::Continue, ConvergenceAction::Continue, false, false);
        fb.had_errors = true;
        assert!(fb.had_errors);
    }
}
