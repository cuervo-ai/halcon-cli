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
pub(crate) enum ExecutionIntentPhase {
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

// ── Agent FSM ─────────────────────────────────────────────────────────────────

/// Typed agent loop phase — replaces stringly-typed `current_fsm_state`.
///
/// Each variant maps 1:1 to a stage in the agent loop lifecycle. Transitions are
/// validated by `transition()` — invalid events log a warning and stay in the
/// current phase (no silent no-ops).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentPhase {
    /// Loop not yet started. Callsite: `LoopState` construction in `agent/mod.rs`.
    Idle,
    /// Planner is generating or updating an execution plan.
    /// Callsite: `agent/mod.rs` prologue fires `PlanGenerated` after plan creation.
    Planning,
    /// Tools are being executed (provider rounds + post_batch).
    /// Callsite: `agent/mod.rs` prologue fires `PlanGenerated`/`PlanSkipped` → Executing.
    Executing,
    /// Waiting for tool batch results (tools submitted, not yet completed).
    /// Callsite: `post_batch.rs` fires `ToolsSubmitted` when tools dispatched.
    ToolWait,
    /// Reflector / supervisor is evaluating round results.
    /// Callsite: `result_assembly.rs` fires `ReflectionComplete` after supervisor eval.
    Reflecting,
    /// Model is generating final synthesis output (no tools).
    /// Callsite: `convergence_phase.rs` / `round_setup.rs` fire `SynthesisStarted`.
    Synthesizing,
    /// LoopCritic is evaluating final output quality.
    /// Callsite: `result_assembly.rs` fires `SynthesisComplete` after synthesis.
    Evaluating,
    /// Loop completed successfully.
    /// Callsite: `result_assembly.rs` fires `EvaluationComplete`.
    Completed,
    /// Loop halted due to error, cancellation, or budget exhaustion.
    /// Callsite: any phase fires `ErrorOccurred` / `Cancelled`.
    Halted,
}

impl AgentPhase {
    /// Backward-compatible string representation matching `current_fsm_state` values.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Planning => "planning",
            Self::Executing => "executing",
            Self::ToolWait => "tool_wait",
            Self::Reflecting => "reflecting",
            Self::Synthesizing => "synthesizing",
            Self::Evaluating => "evaluating",
            Self::Completed => "completed",
            Self::Halted => "halted",
        }
    }
}

/// Events that drive FSM transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentEvent {
    PlanGenerated,
    PlanSkipped,
    ToolsSubmitted,
    ToolBatchComplete,
    ReflectionComplete,
    SynthesisStarted,
    SynthesisComplete,
    EvaluationComplete,
    ErrorOccurred,
    Cancelled,
}

/// Deterministic FSM transition function.
///
/// Returns the new phase given the current phase and an event. Invalid transitions
/// are no-ops (return `current` unchanged). Terminal states (`Completed`, `Halted`)
/// are sticky — only `ErrorOccurred` / `Cancelled` can move out.
pub(super) fn transition(current: AgentPhase, event: AgentEvent) -> AgentPhase {
    use AgentEvent::*;
    use AgentPhase::*;

    match (current, event) {
        // From Idle
        (Idle, PlanGenerated) => Planning,
        (Idle, PlanSkipped) => Executing,
        (Idle, ErrorOccurred) | (Idle, Cancelled) => Halted,

        // From Planning
        (Planning, PlanGenerated) => Executing, // plan produced → start executing
        (Planning, PlanSkipped) => Executing,
        (Planning, ErrorOccurred) | (Planning, Cancelled) => Halted,

        // From Executing
        (Executing, ToolsSubmitted) => ToolWait,
        (Executing, ToolBatchComplete) => Executing, // stays executing across rounds
        (Executing, ReflectionComplete) => Reflecting,
        (Executing, SynthesisStarted) => Synthesizing,
        (Executing, ErrorOccurred) | (Executing, Cancelled) => Halted,

        // From ToolWait
        (ToolWait, ToolBatchComplete) => Executing,
        (ToolWait, SynthesisStarted) => Synthesizing,
        (ToolWait, ErrorOccurred) | (ToolWait, Cancelled) => Halted,

        // From Reflecting
        (Reflecting, ToolBatchComplete) => Executing,
        (Reflecting, ReflectionComplete) => Reflecting,
        (Reflecting, SynthesisStarted) => Synthesizing,
        (Reflecting, ErrorOccurred) | (Reflecting, Cancelled) => Halted,

        // From Synthesizing
        (Synthesizing, SynthesisComplete) => Evaluating,
        (Synthesizing, EvaluationComplete) => Completed,
        (Synthesizing, ErrorOccurred) | (Synthesizing, Cancelled) => Halted,

        // From Evaluating
        (Evaluating, EvaluationComplete) => Completed,
        (Evaluating, ErrorOccurred) | (Evaluating, Cancelled) => Halted,

        // Any state → Halted on error/cancel
        (_, ErrorOccurred) | (_, Cancelled) => Halted,

        // Invalid / unhandled → stay in current state (log for observability).
        _ => {
            tracing::warn!(
                from = %current.as_str(),
                event = ?event,
                "FSM: invalid transition dropped"
            );
            current
        }
    }
}

impl AgentPhase {
    /// Fire an event and return the new phase (convenience for LoopState mutations).
    pub fn fire(self, event: AgentEvent) -> Self {
        transition(self, event)
    }
}

// ── Domain Sub-Structs ──────────────────────────────────────────────────────

/// HICON subsystems — self-corrector, resource predictor, metacognitive loop.
/// Consumers: convergence_phase.rs (dominant), provider_round.rs (resource_predictor).
pub(super) struct HiconSubsystems {
    pub self_corrector: super::super::self_corrector::AgentSelfCorrector,
    pub resource_predictor: super::super::arima_predictor::ResourcePredictor,
    pub metacognitive_loop: super::super::metacognitive_loop::MetacognitiveLoop,
}

/// Token/cost accounting — all per-round token tracking, cost, budget, and K5-2 growth.
/// Consumers: provider_round (writes), round_setup (reads), post_batch (growth),
///            result_assembly (reads), checkpoint (reads).
pub(super) struct TokenAccounting {
    pub call_input_tokens: u64,
    pub call_output_tokens: u64,
    pub call_cost: f64,
    pub pipeline_budget: u32,
    /// Provider's full context window in tokens (e.g. 64_000 for deepseek-chat).
    pub provider_context_window: u32,
    pub tokens_planning: u64,
    pub tokens_subagents: u64,
    pub tokens_critic: u64,
    pub call_input_tokens_prev_round: u64,
    pub tokens_per_round: Vec<u64>,
    pub consecutive_growth_violations: u32,
    pub k5_2_compaction_needed: bool,
}

/// Evidence Boundary System state — evidence tracking, blocked tools, enforcement flags.
/// Consumers: all 5 phase files + mod.rs.
pub(super) struct EvidenceState {
    pub bundle: super::super::evidence_pipeline::EvidenceBundle,
    pub graph: super::super::evidence_graph::EvidenceGraph,
    pub deterministic_boundary_enforced: bool,
    pub blocked_tools: Vec<(String, String)>,
}

/// Synthesis control — FSM phase, tool decision signal, forced synthesis flags.
/// Consumers: all 5 phase files.
pub(super) struct SynthesisControl {
    pub forced_synthesis_detected: bool,
    pub synthesis_origin: Option<SynthesisOrigin>,
    pub tool_decision: ToolDecisionSignal,
    pub execution_intent: ExecutionIntentPhase,
    pub phase: AgentPhase,
    pub convergence_directive_injected: bool,
}

/// Convergence state — scoring, evaluation, replanning, drift detection.
/// Consumers: convergence_phase (dominant), round_setup (evaluations), result_assembly.
pub(super) struct ConvergenceState {
    pub convergence_detector: super::super::early_convergence::ConvergenceDetector,
    pub conv_ctrl: super::super::convergence_controller::ConvergenceController,
    pub round_scorer: super::super::round_scorer::RoundScorer,
    pub round_evaluations: Vec<super::super::round_scorer::RoundEvaluation>,
    pub adaptive_policy: super::super::adaptive_policy::AdaptivePolicy,
    pub coherence_checker: super::super::plan_coherence::PlanCoherenceChecker,
    pub cumulative_drift_score: f32,
    pub drift_replan_count: usize,
    pub replan_attempts: u32,
    pub last_convergence_ratio: f32,
    pub macro_plan_view: Option<super::super::macro_feedback::MacroPlanView>,
    // Phase 3: Mid-loop critic checkpoints (P3.4)
    pub mid_loop_critic: super::super::domain::mid_loop_critic::MidLoopCritic,
    // Phase 3: Complexity feedback loop (P3.5)
    pub complexity_tracker: super::super::domain::complexity_feedback::ComplexityTracker,
    // Phase 4: System invariant checker (P4.1)
    pub invariant_checker: super::super::domain::system_invariants::SystemInvariantChecker,
    // Phase 4: Decision trace collector (P4.2)
    pub decision_trace: super::super::domain::decision_trace::DecisionTraceCollector,
    // Phase 4: Metrics collector (P4.3)
    pub metrics_collector: super::super::domain::system_metrics::MetricsCollector,
    // Phase 4: Adaptation bounds checker (P4.5)
    pub adaptation_bounds: super::super::domain::adaptation_bounds::AdaptationBoundsChecker,
    // Phase 5: Problem classifier (P5.2)
    pub problem_classifier: super::super::domain::problem_classifier::ProblemClassifier,
    // Phase 5: Strategy weight manager (P5.3)
    pub strategy_weight_manager: super::super::domain::strategy_weights::StrategyWeightManager,
}

/// Loop guard state — oscillation detection, failure tracking, capability orchestration.
/// Consumers: round_setup, post_batch, convergence_phase, result_assembly.
pub(super) struct LoopGuardState {
    pub loop_guard: super::super::loop_guard::ToolLoopGuard,
    pub failure_tracker: super::super::failure_tracker::ToolFailureTracker,
    pub capability_orchestrator: super::super::capability_orchestrator::CapabilityOrchestrationLayer,
    // Phase 3: Semantic cycle detection (P3.3)
    pub semantic_cycle_detector: super::super::domain::semantic_cycle::SemanticCycleDetector,
}

// ── LoopState ───────────────────────────────────────────────────────────────

pub(super) struct LoopState {
    // ── Messaging ──────────────────────────────────────────────────────────
    pub messages: Vec<ChatMessage>,
    pub context_pipeline: halcon_context::ContextPipeline,
    pub full_text: String,
    pub rounds: usize,

    // ── Domain sub-structs ────────────────────────────────────────────────
    pub tokens: TokenAccounting,
    pub evidence: EvidenceState,
    pub synthesis: SynthesisControl,
    pub convergence: ConvergenceState,
    pub hicon: HiconSubsystems,
    pub guards: LoopGuardState,

    // ── Cross-cutting messaging ────────────────────────────────────────────
    pub session_id: uuid::Uuid,
    pub trace_step_index: u32,

    // ── Plan & execution tracking ──────────────────────────────────────────
    pub active_plan: Option<ExecutionPlan>,
    pub execution_tracker: Option<super::super::execution_tracker::ExecutionTracker>,

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

    // ── Dynamic tool trust scoring ────────────────────────────────────────
    pub tool_trust: super::super::tool_trust::ToolTrustScorer,

    // ── Runtime control ────────────────────────────────────────────────────
    pub fallback_adapted_model: Option<String>,
    pub environment_error_halt: bool,
    pub auto_pause: bool,
    pub ctrl_cancelled: bool,
    pub model_downgrade_advisory_active: bool,
    pub forced_routing_bias: Option<String>,
    pub last_round_model_name: String,
    pub next_round_restarts: usize,

    // ── Timing ─────────────────────────────────────────────────────────────
    pub loop_start: Instant,
    pub tool_timeout: Duration,

    // ── Render flags ───────────────────────────────────────────────────────
    pub silent: bool,

    // ── Shared context ──────────────────────────────────────────────────────
    pub user_msg: String,
    pub goal_text: String,
    pub l4_archive_path: std::path::PathBuf,
    pub strategy_context: Option<super::super::agent_types::StrategyContext>,
    pub orchestration_decision: Option<super::super::decision_layer::OrchestrationDecision>,
    pub sla_budget: Option<super::super::sla_manager::SlaBudget>,

    // ── Tool execution tracking ─────────────────────────────────────────────
    pub tools_executed: Vec<String>,
    pub failed_sub_agent_steps: Vec<crate::repl::agent_types::FailedStepContext>,

    // ── Centralized policy ──────────────────────────────────────────────
    pub policy: std::sync::Arc<halcon_core::types::PolicyConfig>,

    // ── Phase 3: Environment snapshot (P3.2) ──────────────────────────
    pub env_snapshot: super::super::domain::capability_validator::EnvironmentSnapshot,
}

// ── Cross-domain invariant methods ──────────────────────────────────────────

impl LoopState {
    /// Mark synthesis as forced from a specific origin.
    pub(super) fn mark_synthesis_forced(&mut self, origin: SynthesisOrigin) {
        self.synthesis.forced_synthesis_detected = true;
        self.synthesis.synthesis_origin = Some(origin);
    }

    /// Mark EBS-B2 pre-invocation boundary enforcement.
    pub(super) fn mark_ebs_b2_enforced(&mut self) {
        self.evidence.bundle.synthesis_blocked = true;
        self.evidence.deterministic_boundary_enforced = true;
        self.mark_synthesis_forced(SynthesisOrigin::SupervisorFailure);
    }

    /// Check evidence gate; if it fires, mark synthesis forced and return the gate message.
    pub(super) fn check_evidence_gate(&mut self) -> Option<String> {
        if self.evidence.bundle.evidence_gate_fires() {
            self.evidence.bundle.synthesis_blocked = true;
            self.mark_synthesis_forced(SynthesisOrigin::SupervisorFailure);
            Some(self.evidence.bundle.gate_message())
        } else {
            None
        }
    }
}

// ── ToolDecisionSignal tests ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{
        AgentEvent, AgentPhase, ExecutionIntentPhase, SynthesisOrigin, ToolDecisionSignal,
        transition,
    };

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

    // ── AgentPhase FSM tests ────────────────────────────────────────────────

    #[test]
    fn fsm_idle_to_planning() {
        assert_eq!(
            transition(AgentPhase::Idle, AgentEvent::PlanGenerated),
            AgentPhase::Planning,
        );
    }

    #[test]
    fn fsm_idle_to_executing_on_plan_skipped() {
        assert_eq!(
            transition(AgentPhase::Idle, AgentEvent::PlanSkipped),
            AgentPhase::Executing,
        );
    }

    #[test]
    fn fsm_executing_stays_executing() {
        // ToolBatchComplete keeps us in Executing (multiple rounds).
        assert_eq!(
            transition(AgentPhase::Executing, AgentEvent::ToolBatchComplete),
            AgentPhase::Executing,
        );
    }

    #[test]
    fn fsm_any_to_halted_on_error() {
        let phases = [
            AgentPhase::Idle,
            AgentPhase::Planning,
            AgentPhase::Executing,
            AgentPhase::Reflecting,
            AgentPhase::Synthesizing,
            AgentPhase::Evaluating,
        ];
        for phase in phases {
            assert_eq!(
                transition(phase, AgentEvent::ErrorOccurred),
                AgentPhase::Halted,
                "{:?} + ErrorOccurred should → Halted",
                phase,
            );
        }
    }

    #[test]
    fn fsm_any_to_halted_on_cancel() {
        let phases = [
            AgentPhase::Idle,
            AgentPhase::Planning,
            AgentPhase::Executing,
            AgentPhase::Reflecting,
            AgentPhase::Synthesizing,
            AgentPhase::Evaluating,
        ];
        for phase in phases {
            assert_eq!(
                transition(phase, AgentEvent::Cancelled),
                AgentPhase::Halted,
                "{:?} + Cancelled should → Halted",
                phase,
            );
        }
    }

    #[test]
    fn fsm_invalid_transition_stays() {
        // PlanGenerated in Executing is invalid — should stay in Executing.
        assert_eq!(
            transition(AgentPhase::Executing, AgentEvent::PlanGenerated),
            AgentPhase::Executing,
        );
        // ToolBatchComplete in Idle is invalid — should stay in Idle.
        assert_eq!(
            transition(AgentPhase::Idle, AgentEvent::ToolBatchComplete),
            AgentPhase::Idle,
        );
        // EvaluationComplete in Planning is invalid — should stay in Planning.
        assert_eq!(
            transition(AgentPhase::Planning, AgentEvent::EvaluationComplete),
            AgentPhase::Planning,
        );
    }

    #[test]
    fn fsm_completed_is_sticky() {
        // Once Completed, most events are no-ops (only Error/Cancel can leave).
        assert_eq!(
            transition(AgentPhase::Completed, AgentEvent::ToolBatchComplete),
            AgentPhase::Completed,
        );
        assert_eq!(
            transition(AgentPhase::Completed, AgentEvent::SynthesisStarted),
            AgentPhase::Completed,
        );
        // But error can still move to Halted.
        assert_eq!(
            transition(AgentPhase::Completed, AgentEvent::ErrorOccurred),
            AgentPhase::Halted,
        );
    }

    #[test]
    fn fsm_halted_is_sticky() {
        // Halted is terminal for all events (error/cancel map back to Halted).
        assert_eq!(
            transition(AgentPhase::Halted, AgentEvent::PlanGenerated),
            AgentPhase::Halted,
        );
        assert_eq!(
            transition(AgentPhase::Halted, AgentEvent::ErrorOccurred),
            AgentPhase::Halted,
        );
        assert_eq!(
            transition(AgentPhase::Halted, AgentEvent::Cancelled),
            AgentPhase::Halted,
        );
    }

    #[test]
    fn fsm_as_str_backward_compat() {
        assert_eq!(AgentPhase::Idle.as_str(), "idle");
        assert_eq!(AgentPhase::Planning.as_str(), "planning");
        assert_eq!(AgentPhase::Executing.as_str(), "executing");
        assert_eq!(AgentPhase::Reflecting.as_str(), "reflecting");
        assert_eq!(AgentPhase::Synthesizing.as_str(), "synthesizing");
        assert_eq!(AgentPhase::Evaluating.as_str(), "evaluating");
        assert_eq!(AgentPhase::Completed.as_str(), "completed");
        assert_eq!(AgentPhase::Halted.as_str(), "halted");
    }

    #[test]
    fn fsm_full_lifecycle() {
        // Happy path: Idle → Planning → Executing → Synthesizing → Evaluating → Completed
        let mut phase = AgentPhase::Idle;
        phase = phase.fire(AgentEvent::PlanGenerated);
        assert_eq!(phase, AgentPhase::Planning);

        phase = phase.fire(AgentEvent::PlanGenerated); // plan produced → executing
        assert_eq!(phase, AgentPhase::Executing);

        phase = phase.fire(AgentEvent::ToolBatchComplete);
        assert_eq!(phase, AgentPhase::Executing); // stays executing

        phase = phase.fire(AgentEvent::ToolBatchComplete);
        assert_eq!(phase, AgentPhase::Executing); // multiple rounds

        phase = phase.fire(AgentEvent::SynthesisStarted);
        assert_eq!(phase, AgentPhase::Synthesizing);

        phase = phase.fire(AgentEvent::SynthesisComplete);
        assert_eq!(phase, AgentPhase::Evaluating);

        phase = phase.fire(AgentEvent::EvaluationComplete);
        assert_eq!(phase, AgentPhase::Completed);
    }

    #[test]
    fn fsm_fire_convenience_matches_transition() {
        let phase = AgentPhase::Executing;
        let event = AgentEvent::SynthesisStarted;
        assert_eq!(phase.fire(event), transition(phase, event));
    }

    // ── Phase 5 K5-2 Growth Invariant tests ─────────────────────────────────

    #[test]
    fn k5_2_single_violation_warns_only() {
        // Growth 1.5× > 1.3× threshold, but only one violation — no compaction.
        let policy = halcon_core::types::PolicyConfig::default();
        let growth_threshold = policy.growth_threshold; // 1.3
        let growth_trigger = policy.growth_consecutive_trigger; // 2

        let prev = 1000u64;
        let curr = 1500u64;
        let factor = curr as f64 / prev as f64;
        assert!(factor > growth_threshold);

        // Simulate: 1 violation → should NOT trigger compaction.
        let mut violations: u32 = 0;
        if factor > growth_threshold {
            violations += 1;
        }
        assert_eq!(violations, 1);
        assert!(violations < growth_trigger, "single violation should not trigger compaction");
    }

    #[test]
    fn k5_2_consecutive_violations_trigger_compaction() {
        let policy = halcon_core::types::PolicyConfig::default();
        let growth_threshold = policy.growth_threshold; // 1.3
        let growth_trigger = policy.growth_consecutive_trigger; // 2

        // Simulate 2 consecutive super-linear rounds.
        let rounds: Vec<(u64, u64)> = vec![(1000, 1500), (1500, 2100)];
        let mut violations: u32 = 0;
        let mut compaction_needed = false;

        for (prev, curr) in &rounds {
            let factor = *curr as f64 / *prev as f64;
            if factor > growth_threshold {
                violations += 1;
                if violations >= growth_trigger {
                    compaction_needed = true;
                }
            } else {
                violations = 0;
            }
        }
        assert!(compaction_needed, "2 consecutive violations should trigger compaction");
    }

    #[test]
    fn k5_2_linear_round_resets_counter() {
        let policy = halcon_core::types::PolicyConfig::default();
        let growth_threshold = policy.growth_threshold;

        // Round 1: violation (1.5×), Round 2: linear (1.1×), Round 3: violation (1.5×)
        let rounds: Vec<(u64, u64)> = vec![
            (1000, 1500), // 1.5× — violation
            (1500, 1650), // 1.1× — linear
            (1650, 2475), // 1.5× — violation
        ];
        let mut violations: u32 = 0;
        let mut compaction_needed = false;

        for (prev, curr) in &rounds {
            let factor = *curr as f64 / *prev as f64;
            if factor > growth_threshold {
                violations += 1;
                if violations >= 2 {
                    compaction_needed = true;
                }
            } else {
                violations = 0; // reset on linear round
            }
        }
        assert!(!compaction_needed, "interleaved linear round should prevent compaction trigger");
        assert_eq!(violations, 1, "counter should be 1 after reset + 1 violation");
    }

    #[test]
    fn k5_2_default_thresholds() {
        let policy = halcon_core::types::PolicyConfig::default();
        assert!((policy.growth_threshold - 1.3).abs() < f64::EPSILON);
        assert_eq!(policy.growth_consecutive_trigger, 2);
    }

    // ── Phase 6 Mini-Critic decision logic tests ────────────────────────────

    /// Simulate mini-critic decision logic (mirrors convergence_phase::mini_critic_check).
    fn simulate_mini_critic(
        round: usize,
        max_rounds: f64,
        progress: f64,
        recent_scores: &[f32],
        interval: usize,
        budget_fraction_threshold: f64,
    ) -> Option<&'static str> {
        if interval == 0 || round < interval || round % interval != 0 {
            return None;
        }
        let budget_fraction = round as f64 / max_rounds;
        let avg_recent = if recent_scores.is_empty() {
            0.5
        } else {
            recent_scores.iter().sum::<f32>() / recent_scores.len() as f32
        };
        let declining = recent_scores.len() >= 2
            && recent_scores[0] < recent_scores[recent_scores.len() - 1];

        if budget_fraction > 0.80 && progress < 0.80 {
            return Some("ForceSynthesis");
        }
        if budget_fraction > budget_fraction_threshold && progress < 0.30 && avg_recent < 0.40 {
            return Some("ForceReplan");
        }
        if budget_fraction > budget_fraction_threshold && declining {
            return Some("ReduceTools");
        }
        None
    }

    #[test]
    fn mini_critic_no_intervention_early() {
        // Round 2 with interval 3 — too early for mini-critic.
        let action = simulate_mini_critic(2, 10.0, 0.0, &[], 3, 0.50);
        assert!(action.is_none(), "Mini-critic should not fire before interval");
    }

    #[test]
    fn mini_critic_force_replan_on_stall() {
        // Round 6 out of 10 (60% budget), 10% progress, low scores → ForceReplan.
        let action = simulate_mini_critic(6, 10.0, 0.10, &[0.3, 0.2, 0.25], 3, 0.50);
        assert_eq!(action, Some("ForceReplan"), "Stalled session should trigger replan");
    }

    #[test]
    fn mini_critic_force_synthesis_late() {
        // Round 9 out of 10 (90% budget), 50% progress → ForceSynthesis.
        let action = simulate_mini_critic(9, 10.0, 0.50, &[0.6, 0.5, 0.4], 3, 0.50);
        assert_eq!(action, Some("ForceSynthesis"), ">80% budget + <80% progress should force synthesis");
    }

    #[test]
    fn mini_critic_no_action_when_progressing() {
        // Round 6 out of 10 (60% budget), 70% progress, rising scores → no action.
        // Scores: latest=0.9 > oldest=0.8 → not declining.
        let action = simulate_mini_critic(6, 10.0, 0.70, &[0.9, 0.85, 0.8], 3, 0.50);
        assert!(action.is_none(), "Good progress with rising scores should not trigger intervention");
    }

    #[test]
    fn mini_critic_default_interval() {
        let policy = halcon_core::types::PolicyConfig::default();
        assert_eq!(policy.mini_critic_interval, 3);
        assert!((policy.mini_critic_budget_fraction - 0.50).abs() < f64::EPSILON);
    }
}
