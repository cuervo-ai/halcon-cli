/// Owned mutable state for the agent loop.
///
/// Bundles all `let mut` variables declared in the `run_agent_loop()` prologue
/// so that phase functions can take a single `&mut LoopState` parameter instead
/// of 30+ individual `&mut` references. Borrowed infrastructure (provider, session,
/// limits, render_sink, etc.) remain in the outer `run_agent_loop()` scope.
use std::time::{Duration, Instant};

use halcon_core::traits::ExecutionPlan;
use halcon_core::types::{ChatMessage, ToolDefinition};

// ── ToolDecisionSignal ────────────────────────────────────────────────────────

/// Typed per-round tool suppression decision.
///
/// Replaces the implicit `force_no_tools_next_round: bool` with explicit semantics
/// and a distinction between oracle-mandated and heuristic-initiated suppression.
///
/// `ForcedByOracle` is the highest-authority signal — once set, heuristic producers
/// cannot downgrade it via `set_force_next()` within the same round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ToolDecisionSignal {
    /// Tools allowed this round (default).
    #[default]
    Allow,
    /// Suppress tools next round — heuristic producers.
    ///
    /// Sources: loop guard stagnation, replan budget exhaustion, replan fallback
    /// (success/fail/timeout paths), compaction timeout, parallel batch collapse,
    /// supervisor-forced replan.
    ForceNoNext,
    /// Suppress tools next round — oracle's explicit `ForceNoTools` arm.
    ///
    /// Higher authority than `ForceNoNext`: oracle producers assign this variant
    /// directly; heuristic producers use `set_force_next()` which preserves it.
    ForcedByOracle,
}

impl ToolDecisionSignal {
    /// Returns `true` when tools should be stripped from the round request.
    pub(super) fn is_active(self) -> bool {
        matches!(self, Self::ForceNoNext | Self::ForcedByOracle)
    }

    /// Consume the signal: returns `true` if active, then resets to `Allow`.
    ///
    /// Called by `round_setup` after applying suppression — clears for next round.
    pub(super) fn consume(&mut self) -> bool {
        let active = self.is_active();
        *self = Self::Allow;
        active
    }

    /// Upgrade to `ForceNoNext` unless already elevated to `ForcedByOracle`.
    ///
    /// Heuristic producers call this. `ForcedByOracle` is assigned directly by
    /// oracle dispatch and is never downgraded by heuristic sources.
    pub(super) fn set_force_next(&mut self) {
        if *self != Self::ForcedByOracle {
            *self = Self::ForceNoNext;
        }
    }
}

// ── ExecutionIntentPhase ──────────────────────────────────────────────────────

/// Phase of the agent's execution intent, derived from the plan at loop start.
///
/// Controls whether synthesis guards are allowed to suppress tools mid-task.
/// `Execution` tasks (bash/file_write/etc.) keep tools active until all steps
/// are finished; only then does the intent transition to `Complete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ExecutionIntentPhase {
    /// No plan, or plan not yet analyzed.
    #[default]
    Uncategorized,
    /// analyze/explore/understand — synthesis allowed when goal is covered.
    Investigation,
    /// build/run/install/deploy — synthesis LOCKED until all steps complete.
    Execution,
    /// All executable steps finished — synthesis now permitted.
    Complete,
}

// ── SynthesisOrigin ───────────────────────────────────────────────────────────

/// Documents the origin of a forced synthesis decision (for tracing/metrics).
///
/// Set whenever `forced_synthesis_detected` transitions to `true`. Enables
/// root-cause debugging without changing control flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SynthesisOrigin {
    /// convergence_phase oracle dispatch (authoritative convergence signal).
    OracleConvergence,
    /// post_batch supervisor strict-mode failure (heuristic).
    SupervisorFailure,
    /// post_batch parallel batch collapse / replan timeout (heuristic fallback).
    ReplanTimeout,
    /// provider_round response cache failure (heuristic).
    CacheCorruption,
    /// provider_round cross-type Tool↔Text oscillation (heuristic).
    OscillationDetected,
}

pub(super) struct LoopState {
    // ── Messaging ──────────────────────────────────────────────────────────
    pub messages: Vec<ChatMessage>,
    pub context_pipeline: halcon_context::ContextPipeline,
    pub full_text: String,
    pub rounds: usize,

    // ── Token / cost accounting ────────────────────────────────────────────
    pub call_input_tokens: u64,
    pub call_output_tokens: u64,
    pub call_cost: f64,
    pub session_id: uuid::Uuid,
    pub trace_step_index: u32,

    // ── Context window ─────────────────────────────────────────────────────
    pub pipeline_budget: u32,
    /// Provider's full context window in tokens (e.g. 64_000 for deepseek-chat).
    /// Used by TokenHeadroom guard as the denominator — NOT pipeline_budget, which
    /// is only the L0-injection budget (~80% of the window). Phase L fix B4.
    pub provider_context_window: u32,

    // ── Plan & execution tracking ──────────────────────────────────────────
    pub active_plan: Option<ExecutionPlan>,
    pub execution_tracker: Option<super::super::execution_tracker::ExecutionTracker>,
    pub convergence_detector: super::super::early_convergence::ConvergenceDetector,
    pub macro_plan_view: Option<super::super::macro_feedback::MacroPlanView>,
    pub last_convergence_ratio: f32,

    // ── Round-setup state ──────────────────────────────────────────────────
    pub compaction_model: String,
    pub cached_tools: Vec<ToolDefinition>,
    pub cached_system: Option<String>,
    pub cached_instructions: Option<String>,
    pub is_conversational_intent: bool,

    // ── Reflection & supervision ───────────────────────────────────────────
    pub reflection_injector: super::super::supervisor::InSessionReflectionInjector,
    pub last_reflection_id: Option<uuid::Uuid>,

    // ── TBAC context flag ──────────────────────────────────────────────────
    pub tbac_pushed: bool,

    // ── Failure & loop guard ───────────────────────────────────────────────
    pub failure_tracker: super::super::failure_tracker::ToolFailureTracker,
    pub fallback_adapted_model: Option<String>,
    pub loop_guard: super::super::loop_guard::ToolLoopGuard,
    pub capability_orchestrator:
        super::super::capability_orchestrator::CapabilityOrchestrationLayer,

    // ── Scoring & evaluation ───────────────────────────────────────────────
    pub round_scorer: super::super::round_scorer::RoundScorer,
    pub round_evaluations: Vec<super::super::round_scorer::RoundEvaluation>,
    pub adaptive_policy: super::super::adaptive_policy::AdaptivePolicy,
    pub coherence_checker: super::super::plan_coherence::PlanCoherenceChecker,
    pub cumulative_drift_score: f32,
    pub drift_replan_count: usize,
    pub replan_attempts: u32,

    // ── HICON subsystems ───────────────────────────────────────────────────
    pub self_corrector: super::super::self_corrector::AgentSelfCorrector,
    pub resource_predictor: super::super::arima_predictor::ResourcePredictor,
    pub metacognitive_loop: super::super::metacognitive_loop::MetacognitiveLoop,

    // ── Convergence ────────────────────────────────────────────────────────
    pub conv_ctrl: super::super::convergence_controller::ConvergenceController,

    // ── Loop control flags ─────────────────────────────────────────────────
    pub forced_synthesis_detected: bool,
    pub convergence_directive_injected: bool,
    pub environment_error_halt: bool,
    pub auto_pause: bool,
    pub ctrl_cancelled: bool,
    /// Set by convergence_phase when AdaptivePolicy fires `model_downgrade_advisory`.
    /// Cleared by round_setup after logging the structured advisory.
    pub model_downgrade_advisory_active: bool,
    /// Forced routing bias for the next round — set by round_setup when
    /// `model_downgrade_advisory_active` fires. Consumed (take) in model selection so it
    /// only applies to a single round before resetting to None.
    ///
    /// Value is always `Some("fast")` when set — the ModelRouter "fast" tier maps to
    /// the provider's lowest-latency tool-capable model.
    pub forced_routing_bias: Option<String>,
    /// Typed tool-suppression signal — replaces `force_no_tools_next_round: bool`.
    ///
    /// `ForceNoNext` = heuristic sources (loop guard, replan fallback, compaction, post_batch).
    /// `ForcedByOracle` = oracle's explicit ForceNoTools arm (highest authority).
    /// Cleared by `round_setup::run()` via `tool_decision.consume()`.
    pub tool_decision: ToolDecisionSignal,
    /// Execution intent phase derived from the plan at loop start.
    ///
    /// `Execution` blocks synthesis guards from suppressing tools mid-task.
    /// Transitions to `Complete` when all executable plan steps finish.
    pub execution_intent: ExecutionIntentPhase,
    /// Origin of the most recent forced synthesis decision — for tracing/metrics.
    ///
    /// Set whenever `forced_synthesis_detected` transitions to `true`.
    /// Enables root-cause debugging without changing control flow.
    pub synthesis_origin: Option<SynthesisOrigin>,

    // ── FSM & telemetry ────────────────────────────────────────────────────
    pub current_fsm_state: &'static str,
    pub last_round_model_name: String,
    /// Count of `PhaseOutcome::NextRound` restarts this loop — for diagnosing oscillation.
    pub next_round_restarts: usize,

    // ── Timing ─────────────────────────────────────────────────────────────
    pub loop_start: Instant,
    pub tool_timeout: Duration,

    // ── Render flags ───────────────────────────────────────────────────────
    pub silent: bool,

    // ── Shared context (owned copies of values also used in prologue) ───────
    /// Original user message text — used in replan prompts inside the loop.
    pub user_msg: String,
    /// Original goal text (&str borrow would tie lifetime to request; store as String).
    pub goal_text: String,
    /// L4 archive path — flushed post-loop in result_assembly.
    pub l4_archive_path: std::path::PathBuf,
    /// UCB1 strategy context — applied in round_setup and convergence handling.
    pub strategy_context: Option<super::super::agent_types::StrategyContext>,

    // ── Tool execution tracking ─────────────────────────────────────────────
    /// Names of all tools successfully executed in this agent loop (accumulated
    /// across all rounds). Populated from PostBatchOutcome::Continue.tool_successes.
    /// Surfaced in AgentLoopResult so orchestrator / TUI can show real tool counts.
    pub tools_executed: Vec<String>,

    // ── Evidence Boundary System ───────────────────────────────────────────
    /// Evidence bundle — tracks text evidence extracted from file-reading tools.
    ///
    /// Records bytes extracted by `read_file`/`read_multiple_files` and detects
    /// binary-file indicators (PDF headers, "Binary file" grep messages).
    /// `EvidenceGate` in convergence_phase checks this before synthesis injection
    /// and replaces the synthesis directive with a limitation report when the
    /// gate fires (`content_read_attempts > 0 && text_bytes_extracted < threshold`).
    pub evidence_bundle: super::super::evidence_pipeline::EvidenceBundle,

    /// EBS-B2: set to `true` when the deterministic pre-invocation boundary gate fires.
    ///
    /// Records that EBS-B2 (provider_round.rs, BRECHA-2 fix) intercepted a model-initiated
    /// synthesis attempt on a session with insufficient evidence and short-circuited the
    /// LLM call entirely. Distinct from `evidence_bundle.synthesis_blocked` (set by all EBS
    /// paths); this flag records only the pre-invocation intercept.
    ///
    /// Used by: debug_assert in result_assembly, telemetry, tests.
    pub deterministic_boundary_enforced: bool,

    // ── Guardrail tracking ──────────────────────────────────────────────────
    /// Tools blocked by security guardrails/TBAC during this session.
    ///
    /// Populated from tool error messages that indicate guardrail/permission denial.
    /// Injected into replan prompts to prevent the planner from selecting the same
    /// tools in retry cycles (BRECHA-C fix).
    pub blocked_tools: Vec<(String, String)>, // (tool_name, reason)

    /// Structured context for sub-agent tasks that failed during orchestration (BRECHA-R1 + FASE 5).
    ///
    /// Collected after each orchestrator wave when `SubAgentResult.success == false`.
    /// Propagated to `AgentLoopResult.failed_sub_agent_steps` so the critic retry
    /// injection in `mod.rs` can tell the planner "these approaches already failed,
    /// use an alternative" — preventing the retry from generating the same plan.
    pub failed_sub_agent_steps: Vec<crate::repl::agent_types::FailedStepContext>,

    // ── Token attribution (Phase L) ────────────────────────────────────────
    /// Tokens consumed by planner LLM call (before entering the loop).
    pub tokens_planning: u64,
    /// Tokens consumed by sub-agent execution across all waves.
    pub tokens_subagents: u64,
    /// Tokens consumed by LoopCritic evaluation call.
    pub tokens_critic: u64,
    /// call_input_tokens from the PREVIOUS round — used by the rolling growth monitor
    /// to detect super-linear context growth (INVARIANT K5-2: growth < 1.3× per round).
    pub call_input_tokens_prev_round: u64,
}

// ── ToolDecisionSignal tests ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{ExecutionIntentPhase, SynthesisOrigin, ToolDecisionSignal};

    #[test]
    fn allow_is_not_active() {
        assert!(!ToolDecisionSignal::Allow.is_active());
    }

    #[test]
    fn force_no_next_is_active() {
        assert!(ToolDecisionSignal::ForceNoNext.is_active());
    }

    #[test]
    fn forced_by_oracle_is_active() {
        assert!(ToolDecisionSignal::ForcedByOracle.is_active());
    }

    #[test]
    fn consume_active_returns_true_and_resets_to_allow() {
        let mut sig = ToolDecisionSignal::ForceNoNext;
        assert!(sig.consume(), "consume must return true when active");
        assert_eq!(sig, ToolDecisionSignal::Allow, "must reset to Allow after consume");
    }

    #[test]
    fn consume_allow_returns_false_and_stays_allow() {
        let mut sig = ToolDecisionSignal::Allow;
        assert!(!sig.consume(), "consume on Allow must return false");
        assert_eq!(sig, ToolDecisionSignal::Allow);
    }

    #[test]
    fn set_force_next_on_allow_produces_force_no_next() {
        let mut sig = ToolDecisionSignal::Allow;
        sig.set_force_next();
        assert_eq!(sig, ToolDecisionSignal::ForceNoNext);
    }

    #[test]
    fn set_force_next_on_force_no_next_is_idempotent() {
        let mut sig = ToolDecisionSignal::ForceNoNext;
        sig.set_force_next();
        assert_eq!(sig, ToolDecisionSignal::ForceNoNext);
    }

    #[test]
    fn set_force_next_cannot_downgrade_forced_by_oracle() {
        // Oracle's explicit ForceNoTools must not be overridable by heuristic producers.
        let mut sig = ToolDecisionSignal::ForcedByOracle;
        sig.set_force_next();
        assert_eq!(
            sig,
            ToolDecisionSignal::ForcedByOracle,
            "ForcedByOracle must not be downgraded by set_force_next()"
        );
    }

    #[test]
    fn default_is_allow() {
        let sig: ToolDecisionSignal = Default::default();
        assert_eq!(sig, ToolDecisionSignal::Allow);
    }

    // ── forced_routing_bias field tests ──────────────────────────────────────

    #[test]
    fn forced_routing_bias_initial_none() {
        // The field starts as None; no bias forced before advisory fires.
        let mut bias: Option<String> = None;
        // Simulates take() in round_setup — consuming an unset bias returns None.
        assert!(bias.take().is_none(), "initial take must yield None");
        assert!(bias.is_none(), "must remain None after take");
    }

    #[test]
    fn forced_routing_bias_set_then_consumed() {
        let mut bias: Option<String> = Some("fast".to_string());
        // First take() consumes and returns the value.
        let consumed = bias.take();
        assert_eq!(consumed.as_deref(), Some("fast"), "must yield 'fast' on first take");
        // Second take() returns None — single-round activation.
        assert!(bias.take().is_none(), "second take must yield None (cleared after first use)");
    }

    #[test]
    fn forced_routing_bias_as_deref_yields_str() {
        let bias: Option<String> = Some("fast".to_string());
        assert_eq!(bias.as_deref(), Some("fast"), "as_deref must yield &str");
    }

    #[test]
    fn forced_routing_bias_or_strategy_when_forced_absent() {
        let forced: Option<String> = None;
        let strategy_bias: Option<&str> = Some("quality");
        // When forced is None, strategy_bias should win.
        let result: Option<&str> = forced.as_deref().or(strategy_bias);
        assert_eq!(result, Some("quality"), "strategy bias must win when forced is absent");
    }

    #[test]
    fn forced_routing_bias_takes_priority_over_strategy() {
        let forced: Option<String> = Some("fast".to_string());
        let strategy_bias: Option<&str> = Some("quality");
        // When forced is Some("fast"), it must override strategy_bias.
        let result: Option<&str> = forced.as_deref().or(strategy_bias);
        assert_eq!(result, Some("fast"), "forced bias must override strategy bias");
    }

    #[test]
    fn both_absent_yields_none() {
        let forced: Option<String> = None;
        let strategy_bias: Option<&str> = None;
        let result: Option<&str> = forced.as_deref().or(strategy_bias);
        assert!(result.is_none(), "both absent must yield None");
    }

    // ── ExecutionIntentPhase tests ────────────────────────────────────────────

    #[test]
    fn execution_intent_defaults_to_uncategorized() {
        let intent: ExecutionIntentPhase = Default::default();
        assert_eq!(intent, ExecutionIntentPhase::Uncategorized);
    }

    #[test]
    fn execution_intent_execution_ne_investigation() {
        assert_ne!(ExecutionIntentPhase::Execution, ExecutionIntentPhase::Investigation);
        assert_ne!(ExecutionIntentPhase::Execution, ExecutionIntentPhase::Complete);
    }

    #[test]
    fn execution_intent_complete_ne_execution() {
        // Complete allows synthesis; Execution does not.
        assert_ne!(ExecutionIntentPhase::Complete, ExecutionIntentPhase::Execution);
    }

    #[test]
    fn synthesis_origin_distinct_variants() {
        // All variants are distinct — no accidental aliasing.
        assert_ne!(SynthesisOrigin::OracleConvergence, SynthesisOrigin::SupervisorFailure);
        assert_ne!(SynthesisOrigin::ReplanTimeout, SynthesisOrigin::CacheCorruption);
        assert_ne!(SynthesisOrigin::OscillationDetected, SynthesisOrigin::OracleConvergence);
    }
}
