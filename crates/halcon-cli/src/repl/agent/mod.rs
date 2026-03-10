// MIGRATION-2026: files moved from repl/ root to agent/ (C-8g)
pub mod accumulator;
pub mod agent_scheduler;
pub mod agent_task_manager;
pub mod agent_utils;
pub mod failure_tracker;
mod budget_guards;
// Phase 1: State Externalization — serializable LoopState snapshot, fire-and-forget persist.
mod checkpoint;
// B1: AgentContext sub-struct definitions (AgentInfrastructure, AgentPolicyContext, AgentOptional).
pub mod context;
// B3: Setup helpers extracted from run_agent_loop() prologue.
mod setup;
mod convergence_phase;
pub(crate) mod loop_state;
// Phase 4: LoopState decomposition scaffolding — additive snapshot types.
// Future migration will embed these as owned sub-structs inside LoopState.
mod loop_state_roles;
// Phase 1: Structured loop event emission (round_started, guard_fired, etc.).
mod loop_events;
mod plan_formatter;
mod planning_policy;
mod post_batch;
mod provider_client;
mod provider_round;
// Phase 2: repair engine (feature = "repair-loop", additive only)
pub(crate) mod repair;
mod result_assembly;
mod round_setup;

use loop_state::{AgentEvent, ExecutionIntentPhase, LoopState, SynthesisOrigin, SynthesisTrigger, ToolDecisionSignal};

use plan_formatter::{
    format_plan_for_prompt, update_plan_in_system, validate_plan,
    PLAN_SECTION_END, PLAN_SECTION_START,
};
use provider_client::{check_control, invoke_with_fallback, InvokeAttempt};

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use futures::StreamExt;
use sha2::Digest;
use tracing::instrument;

use halcon_core::context::EXECUTION_CTX;
use halcon_core::traits::{ExecutionPlan, ModelProvider, Planner, StepOutcome, Tool};
use halcon_core::types::{
    AgentLimits, ChatMessage, ContentBlock, DEFAULT_CONTEXT_WINDOW_TOKENS, DomainEvent,
    EventPayload, MessageContent, ModelChunk, ModelRequest, OrchestratorConfig, Phase14Context,
    PlanningConfig, Role, RoutingConfig, Session, StopReason, TaskContext, TokenUsage,
};
use halcon_core::EventSender;
use halcon_providers::ProviderRegistry;
use halcon_storage::{AsyncDatabase, InvocationMetric, TraceStepType};
use halcon_tools::ToolRegistry;

use super::accumulator::ToolUseAccumulator;
use super::anomaly_detector::AgentAnomaly;
use super::compaction::ContextCompactor;
use super::conversational_permission::ConversationalPermissionHandler;
use super::execution_tracker::ExecutionTracker;
use super::executor;
use super::failure_tracker::ToolFailureTracker;
use super::loop_guard::{hash_tool_args, LoopAction, ToolLoopGuard};
use super::resilience::ResilienceManager;
use super::response_cache::ResponseCache;
use crate::render::sink::RenderSink;

// Re-export types that are part of agent's public API.
// External modules reference these as `agent::StopCondition`, `agent::AgentLoopResult`, etc.
pub use super::agent_types::{AgentLoopResult, StopCondition};
pub use super::agent_utils::{classify_error_hint, compute_fingerprint};
use super::agent_utils::{auto_checkpoint, record_trace};

/// Bundled configuration and dependencies for the agent loop.
///
/// Replaces 14+ positional parameters with a single struct.
/// Optional Phase 8 fields default to disabled/empty.
pub struct AgentContext<'a> {
    // Core (always required):
    pub provider: &'a Arc<dyn ModelProvider>,
    pub session: &'a mut Session,
    pub request: &'a ModelRequest,
    pub tool_registry: &'a ToolRegistry,
    pub permissions: &'a mut ConversationalPermissionHandler,
    pub working_dir: &'a str,
    pub event_tx: &'a EventSender,
    pub limits: &'a AgentLimits,

    // Infrastructure (optional):
    pub trace_db: Option<&'a AsyncDatabase>,
    pub response_cache: Option<&'a ResponseCache>,
    pub resilience: &'a mut ResilienceManager,
    pub fallback_providers: &'a [(String, Arc<dyn ModelProvider>)],
    pub routing_config: &'a RoutingConfig,
    pub compactor: Option<&'a ContextCompactor>,
    pub planner: Option<&'a dyn Planner>,
    pub guardrails: &'a [Box<dyn halcon_security::Guardrail>],
    pub reflector: Option<&'a super::reflexion::Reflector>,
    /// Render sink for all UI output (streaming, tools, feedback).
    /// ClassicSink for terminal, SilentSink for sub-agents, TuiSink for TUI.
    pub render_sink: &'a dyn RenderSink,
    /// When Some, tool execution is intercepted with recorded results (replay mode).
    pub replay_tool_executor: Option<&'a super::replay_executor::ReplayToolExecutor>,
    /// Phase 14: deterministic execution, state machine, observability, etc.
    pub phase14: Phase14Context,
    /// Optional model selector for context-aware model selection.
    pub model_selector: Option<&'a super::model_selector::ModelSelector>,
    /// Provider registry for resolving providers when model selection switches provider.
    pub registry: Option<&'a ProviderRegistry>,
    /// Optional episode ID for linking reflections/memories to the current episode.
    pub episode_id: Option<uuid::Uuid>,
    /// Planning configuration (timeout, replans, etc.).
    pub planning_config: &'a PlanningConfig,
    /// Orchestrator configuration for sub-agent delegation.
    pub orchestrator_config: &'a OrchestratorConfig,
    /// Whether dynamic intent-based tool selection is enabled (Phase 38).
    pub tool_selection_enabled: bool,
    /// Optional structured task bridge (Phase 39). None = disabled (default).
    pub task_bridge: Option<&'a mut super::task_bridge::TaskBridge>,
    /// Optional context metrics for assembly observability (Phase 42).
    pub context_metrics: Option<&'a std::sync::Arc<super::context_metrics::ContextMetrics>>,
    /// Optional context manager for gathering context from all sources (Phase 38 + Context Servers).
    /// When Some, context is assembled before each model invocation.
    pub context_manager: Option<&'a mut super::context_manager::ContextManager>,
    /// Optional control channel receiver (Phase 43). TUI sends Pause/Step/Cancel events.
    /// Classic REPL passes None. When Some, agent loop checks at yield points.
    pub ctrl_rx: Option<super::agent_types::ControlReceiver>,
    /// Tool speculation engine for pre-executing read-only tools (Phase 3 remediation).
    /// Shared across rounds to accumulate hit/miss metrics.
    pub speculator: &'a super::tool_speculation::ToolSpeculator,
    /// G2: Security configuration controlling PII handling policy.
    /// When `pii_action == PiiPolicy::Block`, user messages containing PII are
    /// rejected BEFORE being sent to the LLM.
    pub security_config: &'a halcon_core::types::SecurityConfig,
    /// Multi-dimensional strategy context from UCB1 StrategyPlan (Step 8a).
    /// When Some, agent loop applies tightness/sensitivity/routing_bias/enable_reflection.
    /// None = default behaviour (backward compatible).
    pub strategy_context: Option<super::agent_types::StrategyContext>,
    /// Optional separate model provider for LoopCritic (G2 critic separation).
    /// None = use executor provider (prevents self-evaluation bias only when set).
    pub critic_provider: Option<Arc<dyn ModelProvider>>,
    /// Optional separate model name for LoopCritic.
    /// None = use executor model.
    pub critic_model: Option<String>,
    /// Plugin registry for pre/post invoke gates, cost tracking, circuit breakers (Step 7).
    /// None = plugin system disabled (all existing tests, non-plugin sessions).
    /// The critical zero-regression invariant: all plugin code is guarded by `if let Some(pr)`.
    /// Arc<Mutex<>> so it can be cloned cheaply into the AgentContext and shared with executor.
    pub plugin_registry: Option<std::sync::Arc<std::sync::Mutex<super::plugins::PluginRegistry>>>,
    /// Whether this agent is running as a sub-agent under an orchestrator.
    ///
    /// When `true`, the agent loop uses `ConvergenceController::new_for_sub_agent()` with
    /// tighter limits (max_rounds=6, low goal_coverage_threshold, multilingual keywords)
    /// instead of the intent-profile-derived controller.  Set to `false` for all top-level
    /// agents (main REPL loop, retry loop, replay runner).
    pub is_sub_agent: bool,
    /// Provider originally requested by the user (e.g. CLI `-p` arg).
    ///
    /// When `Some` and different from `provider.name()`, a startup fallback occurred in
    /// `provider_factory.rs` before the TUI was initialised — the warning was printed to
    /// stderr and was therefore invisible in TUI mode. `run_agent_loop` re-emits the event
    /// through `render_sink.provider_fallback()` so TUI users see the warning.
    /// `None` disables the check (sub-agents, replay runner, agent bridge).
    pub requested_provider: Option<String>,
    /// Centralized policy thresholds (replaces module-local const values).
    pub policy: std::sync::Arc<halcon_core::types::PolicyConfig>,
}

impl<'a> AgentContext<'a> {
    /// Construct `AgentContext` from the three logical sub-groups defined in `context.rs`.
    ///
    /// Mutable fields (`session`, `permissions`, `resilience`, `task_bridge`,
    /// `context_manager`, `ctrl_rx`) must be provided directly because Rust's
    /// exclusivity rules prevent them from being split across sub-structs.
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        infra: context::AgentInfrastructure<'a>,
        policy_ctx: context::AgentPolicyContext<'a>,
        optional: context::AgentOptional<'a>,
        session: &'a mut Session,
        request: &'a ModelRequest,
        permissions: &'a mut super::conversational_permission::ConversationalPermissionHandler,
        resilience: &'a mut super::resilience::ResilienceManager,
        task_bridge: Option<&'a mut super::task_bridge::TaskBridge>,
        context_manager: Option<&'a mut super::context_manager::ContextManager>,
        ctrl_rx: Option<super::agent_types::ControlReceiver>,
        working_dir: &'a str,
    ) -> Self {
        Self {
            provider: infra.provider,
            tool_registry: infra.tool_registry,
            trace_db: infra.trace_db,
            response_cache: infra.response_cache,
            fallback_providers: infra.fallback_providers,
            event_tx: infra.event_tx,
            render_sink: infra.render_sink,
            registry: infra.registry,
            speculator: infra.speculator,
            limits: policy_ctx.limits,
            routing_config: policy_ctx.routing_config,
            planning_config: policy_ctx.planning_config,
            orchestrator_config: policy_ctx.orchestrator_config,
            policy: policy_ctx.policy,
            security_config: policy_ctx.security_config,
            phase14: policy_ctx.phase14,
            tool_selection_enabled: policy_ctx.tool_selection_enabled,
            is_sub_agent: policy_ctx.is_sub_agent,
            requested_provider: policy_ctx.requested_provider,
            episode_id: policy_ctx.episode_id,
            compactor: optional.compactor,
            planner: optional.planner,
            guardrails: optional.guardrails,
            reflector: optional.reflector,
            replay_tool_executor: optional.replay_tool_executor,
            model_selector: optional.model_selector,
            critic_provider: optional.critic_provider,
            critic_model: optional.critic_model,
            plugin_registry: optional.plugin_registry,
            strategy_context: optional.strategy_context,
            session,
            request,
            permissions,
            resilience,
            task_bridge,
            context_manager,
            ctrl_rx,
            working_dir,
            context_metrics: None,
        }
    }
}

/// Action determined by checking the control channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlAction {
    /// Continue normally.
    Continue,
    /// Execute one more round then auto-pause.
    StepOnce,
    /// Cancel the agent loop immediately.
    Cancel,
}

/// Universal return type for extracted phase functions.
///
/// Phase functions return `Ok(PhaseOutcome)` to communicate loop control
/// to the outer `run_agent_loop()` dispatcher.
#[must_use]
pub(crate) enum PhaseOutcome {
    /// Normal: continue to the next phase / end of this round.
    Continue,
    /// Skip remaining phases and start the next loop iteration.
    NextRound,
    /// Break `'agent_loop`. All `forced_synthesis_detected` mutations must
    /// be applied to `state` BEFORE returning this variant.
    BreakLoop,
}

/// Dispatch macro: translates a `PhaseOutcome` from a phase function into
/// the appropriate `break`/`continue`/fall-through for the outer agent loop.
///
/// Label must be passed explicitly (`'agent_loop`) due to Rust 2021 label hygiene —
/// labels in macro bodies are hygienic and do not implicitly capture outer loop labels.
macro_rules! dispatch {
    ($label:lifetime, $state:expr, $expr:expr) => {
        match $expr {
            PhaseOutcome::Continue => {}
            PhaseOutcome::NextRound => {
                $state.next_round_restarts += 1;
                tracing::debug!(restart = $state.next_round_restarts, "round restart signal — NextRound");
                continue $label;
            }
            PhaseOutcome::BreakLoop => break $label,
        }
    };
}


/// Run the agentic tool-use loop.
///
/// The loop sends a request to the model, streams the response (rendering text
/// and accumulating tool uses), executes tools on `ToolUse` stop, appends
/// results, and re-invokes until `EndTurn`, `MaxTokens`, a guard limit is hit,
/// or the user interrupts.
#[instrument(skip_all, fields(model = %ctx.request.model))]
pub async fn run_agent_loop(ctx: AgentContext<'_>) -> Result<AgentLoopResult> {
    let AgentContext {
        provider,
        session,
        request,
        tool_registry,
        permissions,
        working_dir,
        event_tx,
        limits,
        trace_db,
        response_cache,
        resilience,
        fallback_providers,
        routing_config,
        compactor,
        planner,
        guardrails,
        reflector,
        render_sink,
        replay_tool_executor,
        phase14,
        model_selector,
        registry,
        episode_id,
        planning_config,
        orchestrator_config,
        tool_selection_enabled,
        mut task_bridge,
        context_metrics,
        mut context_manager,
        mut ctrl_rx,
        speculator,
        security_config,
        strategy_context,
        critic_provider,
        critic_model,
        plugin_registry,
        is_sub_agent,
        requested_provider,
        policy,
    } = ctx;

    let silent = render_sink.is_silent();

    // BRECHA-B (startup provider fallback): provider_factory.rs emits a warning to stderr
    // before the TUI initialises — TUI users never see it. Re-emit through the render sink
    // so the TUI can display a visible warning banner.
    if let Some(ref requested) = requested_provider {
        if requested != provider.name() && !silent {
            render_sink.provider_fallback(
                requested,
                provider.name(),
                "requested provider unavailable — fallback at startup",
            );
        }
    }

    // Phase L: token attribution accumulators (filled before LoopState is created).
    let mut pre_loop_tokens_subagents: u64 = 0;
    // BRECHA-R1 + FASE 5: structured failed step context (filled before LoopState is created).
    let mut pre_loop_failed_steps: Vec<crate::repl::agent_types::FailedStepContext> = Vec::new();

    let mut tool_exec_config = executor::ToolExecutionConfig {
        dry_run_mode: phase14.dry_run_mode,
        idempotency: None,
        ..Default::default()
    };
    let exec_clock = &phase14.exec_ctx.clock;
    let mut messages = request.messages.clone();

    // Phase E1: Emit dry-run banner if active.
    if phase14.dry_run_mode != halcon_core::types::DryRunMode::Off {
        render_sink.dry_run_active(true);
    }

    // Pre-LoopState FSM tracking: use a local &str derived from phase until LoopState
    // is constructed.  After LoopState is created, all transitions go through state.phase.
    let mut pre_loop_phase = "idle";

    // Phase E5: Emit agent state transition: Idle → Planning/Executing.
    if !silent {
        render_sink.agent_state_transition("idle", "executing", "agent loop started");
        pre_loop_phase = "executing";
    }

    // Phase 43: auto_pause flag — set by StepOnce control action.
    // When true, the agent pauses before the next model invocation.
    let mut auto_pause = false;
    // Phase 43: set when user cancels via control channel.
    let mut ctrl_cancelled = false;

    // Context pipeline: multi-tiered message management (L0-L4 cascade).
    // Feed initial messages into the pipeline; it manages L0 hot buffer overflow
    // by cascading to L1 (warm) → L2 (compressed) → L3 (semantic) → L4 (archive).
    // The `messages` Vec remains the full history for fingerprinting/checkpointing;
    // `pipeline.build_messages()` produces the token-budgeted view for model requests.
    //
    // REMEDIATION FIX A — Provider context window alignment:
    // The old hardcoded 200K budget caused catastrophic mismatches with providers that
    // have smaller context windows (e.g. DeepSeek: 64K). With 200K budget, the L0 tier
    // alone gets 80K tokens (40% × 200K) — larger than DeepSeek's entire window. This
    // caused "context exceeds model limit" failures on every non-trivial session.
    //
    // Derive the pipeline budget from the selected model's actual context_window:
    //   pipeline_budget = context_window × 0.80  (20% reserved for output tokens)
    // This ensures the pipeline's tier budgets naturally fit within provider limits.
    // B3-a: Context pipeline construction extracted into setup::build_context_pipeline().
    let setup_result = setup::build_context_pipeline(provider, request, limits, working_dir, &messages);
    let model_context_window = setup_result.model_context_window;
    let mut pipeline_budget = setup_result.pipeline_budget;
    let mut context_pipeline = setup_result.pipeline;
    let l4_archive_path = setup_result.l4_archive_path;

    let mut full_text = String::new();
    let mut rounds = 0;
    let session_id = session.id;

    // Initialize trace step index from DB to avoid collisions across messages.
    let mut trace_step_index: u32 = if let Some(db) = trace_db {
        match db.max_step_index(session_id).await {
            Ok(Some(max)) => max + 1,
            _ => 0,
        }
    } else {
        0
    };
    let loop_start = Instant::now();
    let tool_timeout = Duration::from_secs(limits.tool_timeout_secs);

    // Emit AgentStarted event.
    let user_task = messages
        .last()
        .and_then(|m| match &m.content {
            MessageContent::Text(t) => Some(t.chars().take(100).collect::<String>()),
            _ => None,
        })
        .unwrap_or_default();
    let _ = event_tx.send(DomainEvent::new(EventPayload::AgentStarted {
        agent_type: halcon_core::types::AgentType::Chat,
        task: user_task,
    }));

    // Per-call metrics (accumulated across all rounds).
    let mut call_input_tokens: u64 = 0;
    let mut call_output_tokens: u64 = 0;
    let mut call_cost: f64 = 0.0;

    // Extract user message for task analysis and planning.
    // Note: Clone to String to avoid borrow conflicts when mutating messages later.
    let user_msg = messages
        .last()
        .and_then(|m| match &m.content {
            MessageContent::Text(t) => Some(t.clone()),
            _ => None,
        })
        .unwrap_or_default();

    // Assemble context from all sources (Context Servers Phase 1-8 integration).
    // This injects context-aware system prompt before task analysis and planning.
    let context_system_prompt = if let Some(ref mut cm) = context_manager {
        let context_query = halcon_core::traits::ContextQuery {
            working_directory: working_dir.to_string(),
            user_message: Some(user_msg.clone()),
            token_budget: limits.max_total_tokens as usize,
        };

        let assembled = cm.assemble(&context_query).await;

        // Record context metrics if available
        if let Some(metrics) = context_metrics {
            metrics.record_assembly(assembled.total_source_tokens, assembled.assembly_duration_us);
        }

        assembled.system_prompt
    } else {
        None
    };

    // SOTA 2026: Score intent for TUI panel + ConvergenceController configuration.
    // IntentScorer replaces TaskAnalyzer — multi-signal (scope, depth, language, latency)
    // vs old keyword-only analysis. IntentProfile.task_type and .complexity are the same
    // TaskType/TaskComplexity types, so all downstream consumers work unchanged.
    let task_analysis = super::intent_scorer::IntentScorer::score(&user_msg);
    tracing::debug!(
        task_type = ?task_analysis.task_type,
        complexity = ?task_analysis.complexity,
        scope = ?task_analysis.scope,
        reasoning_depth = ?task_analysis.reasoning_depth,
        suggested_max_rounds = task_analysis.suggested_max_rounds(),
        "IntentScorer: task profile for agent loop",
    );

    // Adaptive planning: generate plan before entering tool loop.
    let mut active_plan: Option<ExecutionPlan> = None;
    if let Some(planner) = planner {
        let tool_defs = request.tools.clone();

        // W-4 (PlanningPolicy): Model-aware, intent-driven planning gate.
        // Replaces the static PLANNING_ACTION_KW_RE keyword regex and heuristic
        // word-count/complexity-marker checks with a composable policy pipeline:
        //   1. ToolAwarePlanningPolicy  — hard veto: model without tools → skip
        //   2. ReasoningModelPolicy     — reasoning model → lightweight (thinks internally)
        //   3. IntentDrivenPolicy       — uses IntentProfile.requires_planning + complexity
        let plan_model_info = provider
            .supported_models()
            .iter()
            .find(|m| m.id == request.model)
            .cloned();
        let plan_ctx = planning_policy::PlanningContext {
            user_msg: &user_msg,
            intent: &task_analysis,
            model_info: plan_model_info.as_ref(),
            routing_tier: task_analysis.routing_tier(),
        };
        let mut planning_decision = planning_policy::decide(&plan_ctx);

        // Hard gate: validate planner model against its provider before invoking.
        // Prevents wasted ~2s LLM call on a guaranteed 404 (e.g., claude model on ollama).
        if planning_decision != planning_policy::PlanningDecision::SkipPlanning
            && !planner.supports_model()
        {
            tracing::debug!(
                planner = planner.name(),
                "PlanningPolicy: overriding to SkipPlanning — planner model not available on provider"
            );
            planning_decision = planning_policy::PlanningDecision::SkipPlanning;
        }

        let plan_result = if planning_decision != planning_policy::PlanningDecision::SkipPlanning {
            // Phase E5: Transition to Planning state.
            if !silent {
                render_sink.agent_state_transition(pre_loop_phase, "planning", "generating plan");
                pre_loop_phase = "planning";
                render_sink.phase_started("planning", "Generating execution plan...");
            }
            // Prefix user message with working directory context.
            // This prevents the planning LLM from hallucinating project structure
            // from global HALCON.md context when the CWD is a different project.
            let plan_user_msg = format!(
                "[Working directory: {}]\n\n{}",
                working_dir,
                user_msg
            );
            let plan_timeout = Duration::from_secs(planning_config.timeout_secs);
            let result = tokio::time::timeout(
                plan_timeout,
                planner.plan(&plan_user_msg, &tool_defs),
            )
            .await;
            render_sink.phase_ended(); // always fires regardless of result
            // Phase E5: Transition back to Executing after planning.
            if !silent {
                render_sink.agent_state_transition(pre_loop_phase, "executing", "plan generated");
                pre_loop_phase = "executing";
            }
            result
        } else {
            Ok(Ok(None))
        };

        match plan_result {
            Ok(Ok(Some(plan))) => {
                tracing::info!(goal = %plan.goal, steps = plan.steps.len(), "Plan generated");
                // Emit plan event.
                let _ = event_tx.send(DomainEvent::new(EventPayload::PlanGenerated {
                    plan_id: plan.plan_id,
                    goal: plan.goal.clone(),
                    step_count: plan.steps.len(),
                    replan_count: plan.replan_count,
                }));
                // Persist plan steps.
                if let Some(db) = trace_db {
                    let _ = db.save_plan_steps(&session_id, &plan).await;
                }
                // Ingest plan into task bridge (structured task framework).
                if let Some(ref mut bridge) = task_bridge {
                    let mappings = bridge.ingest_plan(&plan);
                    tracing::info!(
                        task_count = mappings.len(),
                        "TaskBridge ingested plan into structured tasks"
                    );
                    render_sink.task_status(
                        &plan.goal,
                        "Planned",
                        None,
                        0,
                    );
                }
                // Pre-execution plan validation to catch invalid tool references early.
                let validation_warnings = validate_plan(&plan, tool_registry);
                if !validation_warnings.is_empty() {
                    tracing::warn!(
                        warning_count = validation_warnings.len(),
                        "Plan validation detected issues"
                    );
                    for warning in &validation_warnings {
                        tracing::warn!("{}", warning);
                        if !silent {
                            render_sink.warning("plan validation warning", Some(warning));
                        }
                    }
                }

                // Planning V3: Compress plan to ≤MAX_VISIBLE_STEPS before activation.
                // Keeps the active plan focused and prevents context bloat from verbose steps.
                let (plan, _compression_stats) = super::plan_compressor::compress(plan);

                active_plan = Some(plan);
                // Note: Plan hash will be updated on first round iteration (loop_guard doesn't exist yet)
            }
            Ok(Ok(None)) => {
                tracing::debug!("Planner returned no plan (simple query)");
            }

            Ok(Err(e)) => {
                tracing::warn!("Planning failed, proceeding without plan: {e}");
                if !silent {
                    render_sink.warning(
                        "planning unavailable — executing without plan",
                        Some(&format!("{e}")),
                    );
                }
            }
            Err(_elapsed) => {
                tracing::warn!(
                    timeout_secs = planning_config.timeout_secs,
                    "Planning timed out, proceeding without plan"
                );
                if !silent {
                    render_sink.warning(
                        &format!("planning timed out after {}s — executing without plan", planning_config.timeout_secs),
                        Some("increase [planning].timeout_secs in config"),
                    );
                }
            }
        }
    }

    // Emit reasoning status to TUI panel.
    if !silent {
        let strategy = if active_plan.is_some() {
            "PlanExecuteReflect"
        } else {
            "DirectExecution"
        };
        let task_type = task_analysis.task_type.as_str();
        let complexity = match task_analysis.complexity {
            super::task_analyzer::TaskComplexity::Simple => "Simple",
            super::task_analyzer::TaskComplexity::Moderate => "Moderate",
            super::task_analyzer::TaskComplexity::Complex => "Complex",
        };
        render_sink.reasoning_update(strategy, task_type, complexity);
    }

    // TBAC: if adaptive planning produced a plan, create a task context scoping to planned tools.
    let tbac_pushed = if let Some(ref plan) = active_plan {
        if permissions.active_context().is_none() {
            // Only push if TBAC is enabled (check_tbac returns NoContext when disabled).
            let planned_tools: std::collections::HashSet<String> = plan
                .steps
                .iter()
                .filter_map(|s| s.tool_name.clone())
                .collect();
            if !planned_tools.is_empty() {
                let ctx = TaskContext::new(plan.goal.clone(), planned_tools);
                permissions.push_context(ctx);
                true
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    // Centralized plan execution tracker with step timing and state management.
    let mut execution_tracker = active_plan.as_ref().map(|plan| {
        ExecutionTracker::new(plan.clone(), event_tx.clone())
    });

    // Planning V3: ConvergenceDetector calibrated to the provider's context window.
    // Uses 8% of pipeline_budget as synthesis headroom (clamped to [4K, 20K] tokens).
    // Prevents mid-stream truncation and detects diminishing-returns early.
    let mut convergence_detector =
        super::early_convergence::ConvergenceDetector::with_policy_context_window(pipeline_budget as u64, &policy);

    // Planning V3: MacroPlanView for user-facing [N/M] progress display.
    // Wraps the compressed plan; emits a plan summary on creation,
    // then advances step-by-step via the tracker in the agent loop body.
    let mut macro_plan_view: Option<super::macro_feedback::MacroPlanView> =
        active_plan.as_ref().map(|plan| {
            let mode = if silent {
                super::macro_feedback::FeedbackMode::Silent
            } else {
                super::macro_feedback::FeedbackMode::Compact
            };
            super::macro_feedback::MacroPlanView::from_plan(plan, mode)
        });
    // Emit the human-readable plan summary ([1/3] Step A → [2/3] Step B → …)
    // immediately after plan creation so the user knows what's coming.
    if let Some(ref view) = macro_plan_view {
        if !silent {
            render_sink.info(&view.format_plan_summary());
        }
    }
    // Per-round completion ratio from the previous iteration — used for delta computation
    // in the convergence check.
    let mut last_convergence_ratio: f32 = 0.0;

    // Fix: resolve the model to use for context compaction from the active provider.
    // request.model may belong to a different provider (e.g. "claude-sonnet" when using
    // deepseek), which would cause compaction API calls to return 404/400.
    // We select the provider's first available model that can handle text generation.
    // mut: updated when provider fallback changes the active model.
    let mut compaction_model = if provider.validate_model(&request.model).is_ok() {
        request.model.clone()
    } else {
        provider
            .supported_models()
            .first()
            .map(|m| m.id.clone())
            .unwrap_or_else(|| request.model.clone())
    };
    tracing::debug!(
        provider = provider.name(),
        model = %compaction_model,
        "Resolved compaction model for provider"
    );

    // Cache tools outside the loop — tool definitions never change between rounds.
    // Phase 38: Apply intent-based tool selection when dynamic_tool_selection is enabled.
    // Conversational intent (greetings, simple Q&A) returns vec![] — the model responds
    // directly in 1 round without any tool call overhead.
    let is_conversational_intent;
    let mut cached_tools = {
        let all_tools = request.tools.clone();
        let tool_selector = super::tool_selector::ToolSelector::new(
            tool_selection_enabled,
        );
        let user_msg_text = messages
            .last()
            .and_then(|m| match &m.content {
                MessageContent::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let intent = tool_selector.classify_intent(user_msg_text);
        is_conversational_intent = intent == super::tool_selector::TaskIntent::Conversational;
        let selected = tool_selector.select_tools(&intent, &all_tools);
        if selected.len() < all_tools.len() {
            tracing::info!(
                intent = ?intent,
                total = all_tools.len(),
                selected = selected.len(),
                "ToolSelector filtered tools for model request"
            );
        }
        // FASE 5: Environment-aware tool filtering.
        // Remove tools that depend on unavailable environment features (git, CI).
        let env_ctx = super::tool_selector::EnvironmentContext::detect(working_dir);
        let env_filtered = env_ctx.filter_tools(selected);
        if env_filtered.len() < all_tools.len() {
            tracing::info!(
                is_git = env_ctx.is_git_repo,
                has_ci = env_ctx.has_ci_config,
                before = all_tools.len(),
                after = env_filtered.len(),
                "EnvironmentFilter applied"
            );
        }
        // Phase 42: record tool selection metrics.
        if let Some(metrics) = context_metrics {
            metrics.record_tool_selection(all_tools.len(), env_filtered.len());
        }
        // Sprint 0-C: Cached preflight schema validation.
        // Validates each tool's input_schema exactly once per process lifetime.
        // Invalid schemas are logged and excluded — prevents confusing API-level errors.
        super::schema_validator::preflight_validate(env_filtered)
    };
    // B1: InputNormalizer — normalize the raw user message before any scoring.
    // Unicode control chars stripped, whitespace collapsed, language detected.
    // The normalized query is passed to BoundaryDecisionEngine instead of raw user_msg.
    let boundary_input = {
        use super::input_boundary::{InputContext, InputNormalizer};
        let ctx = InputContext {
            available_tool_count: cached_tools.len(),
            is_sub_agent,
            ..InputContext::default()
        };
        InputNormalizer::normalize(user_msg.as_str(), ctx, None)
    };
    // F2 DecisionLayer: classify task complexity for SLA/orchestration gating.
    //
    // When `policy.use_boundary_decision_engine` is true (default), the new
    // BoundaryDecisionEngine pipeline runs instead of the legacy keyword-count
    // estimator. Both paths produce an `OrchestrationDecision` for backward-compat.
    // The BoundaryDecision is stored on LoopState for convergence policy enforcement.
    let (orchestration_decision, boundary_decision) = if !is_sub_agent {
        if policy.use_boundary_decision_engine {
            let bd = super::decision_engine::BoundaryDecisionEngine::evaluate(
                &boundary_input.query,
                cached_tools.len(),
            );
            tracing::info!(
                domain = %bd.trace.domain.label(),
                complexity = %bd.trace.complexity.label(),
                risk = %bd.trace.risk.label(),
                sla_mode = %bd.routing.mode.label(),
                max_rounds = bd.recommended_max_rounds,
                use_orch = bd.use_orchestration,
                "BoundaryDecisionEngine"
            );
            let od = bd.to_orchestration_decision();
            (Some(od), Some(bd))
        } else {
            let d = super::decision_layer::estimate_complexity(&boundary_input.query, &cached_tools);
            tracing::info!(complexity = ?d.complexity, use_orch = d.use_orchestration, "DecisionLayer(legacy)");
            (Some(d), None)
        }
    } else {
        (None, None)
    };

    // F3 SlaManager: derive time/round budget from task complexity.
    let sla_budget = if !is_sub_agent {
        orchestration_decision.as_ref().map(|d| {
            let b = super::sla_manager::SlaBudget::from_complexity(d);
            tracing::info!(mode = ?b.mode, max_rounds = b.max_rounds, "SlaManager");
            b
        })
    } else {
        None
    };

    // IntentPipeline: unified reconciliation of IntentScorer + BoundaryDecisionEngine.
    // Computes `effective_max_rounds` BEFORE `ConvergenceController` is constructed,
    // fixing BV-1 (ConvergenceController calibrated for N rounds but running for M<N).
    // Gated by `policy.use_intent_pipeline` for zero-regression fallback.
    let resolved_intent = if !is_sub_agent && policy.use_intent_pipeline {
        if let Some(ref bd) = boundary_decision {
            let store = super::decision_engine::PolicyStore::from_config(&policy);
            let resolved = super::decision_engine::IntentPipeline::resolve(
                &task_analysis,
                bd,
                ctx.limits.max_rounds,
                &store,
            );
            tracing::info!(
                effective_max_rounds = resolved.effective_max_rounds,
                routing_mode = %resolved.routing_mode.label(),
                confidence = resolved.reconciliation_confidence,
                max_rounds_source = ?resolved.max_rounds_source,
                "IntentPipeline: reconciled routing decision"
            );
            Some(resolved)
        } else {
            None
        }
    } else {
        None
    };

    // System prompt may update mid-session if instruction files (HALCON.md) change on disk.
    // Track instruction content separately for surgical replacement in the full system prompt.
    let mut cached_system = request.system.clone();
    let mut cached_instructions =
        halcon_context::load_instructions(std::path::Path::new(working_dir));

    // Feature 1 (Frontier Roadmap 2026): HALCON.md 4-scope instruction hierarchy.
    // When policy.use_halcon_md = true, load from all 4 scopes (Local→User→Project→Managed),
    // apply path-glob rules, resolve @import directives, and start the hot-reload watcher.
    // Injected text becomes the initial cached_instructions for surgical per-round replacement.
    let mut instruction_store: Option<super::instruction_store::InstructionStore> = None;
    if policy.use_halcon_md {
        let mut store = super::instruction_store::InstructionStore::new(
            std::path::Path::new(working_dir),
        );
        if let Some(instr_text) = store.load() {
            // Inject as a dedicated "## Project Instructions" section.
            match &mut cached_system {
                Some(ref mut sys) => {
                    sys.push_str("\n\n");
                    sys.push_str(&instr_text);
                }
                None => {
                    cached_system = Some(instr_text.clone());
                }
            }
            // Track injected text for surgical per-round replacement.
            cached_instructions = Some(instr_text);
        }
        instruction_store = Some(store);
        tracing::info!(
            working_dir,
            "HALCON.md instruction store initialized (use_halcon_md=true)",
        );
    }

    // Feature 3 (Frontier Roadmap 2026): Auto-memory injection (round 1 only).
    // Injects first 200 lines of .halcon/memory/MEMORY.md as "## Agent Memory" section.
    if policy.enable_auto_memory {
        let repo_name = std::path::Path::new(working_dir)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        if let Some(memory_text) = super::auto_memory::injector::build_injection(
            std::path::Path::new(working_dir),
            repo_name,
        ) {
            match &mut cached_system {
                Some(ref mut sys) => {
                    sys.push_str("\n\n");
                    sys.push_str(&memory_text);
                }
                None => {
                    cached_system = Some(memory_text);
                }
            }
            tracing::debug!("auto_memory: injected memory into system prompt");
        }
    }

    // Feature 2 (Frontier Roadmap 2026): Lifecycle hooks — UserPromptSubmit / PreToolUse / etc.
    // Load hook config from settings.toml (global + project scopes, snapshotted here — never
    // hot-reloaded to prevent hook injection attacks via committed repo files).
    {
        if policy.enable_hooks {
            let hooks_config = super::hooks::config::load_hooks_config(
                policy.allow_managed_hooks_only,
            );
            if hooks_config.enabled && !hooks_config.definitions.is_empty() {
                let runner = std::sync::Arc::new(
                    super::hooks::HookRunner::new(hooks_config),
                );
                // Store session_id string for hook env vars.
                tool_exec_config.session_id_str = session_id.to_string();
                tool_exec_config.hook_runner = Some(runner.clone());
                // Fire UserPromptSubmit hook before the first round.
                if runner.has_hooks_for(super::hooks::HookEventName::UserPromptSubmit) {
                    let hook_event = super::hooks::lifecycle_event(
                        super::hooks::HookEventName::UserPromptSubmit,
                        &session_id.to_string(),
                    );
                    match runner.fire(&hook_event).await {
                        super::hooks::HookOutcome::Deny(reason) => {
                            tracing::warn!(%reason, "UserPromptSubmit hook denied — aborting session");
                            // Return an early error result — agent loop does not start.
                            return Err(anyhow::anyhow!(
                                "UserPromptSubmit hook denied the session: {reason}"
                            ));
                        }
                        super::hooks::HookOutcome::Warn(msg) => {
                            tracing::warn!(%msg, "UserPromptSubmit hook warning");
                        }
                        super::hooks::HookOutcome::Allow => {}
                    }
                }
                tracing::info!("lifecycle hooks initialized (enable_hooks=true)");
            }
        }
    }

    // Feature 4 (Frontier Roadmap 2026): Declarative sub-agent registry.
    // Load agent definitions from .halcon/agents/ + ~/.halcon/agents/ at session start.
    // The routing manifest is injected into the system prompt so the model can delegate
    // by name.  Disabled by default — zero behavioral change until enable_agent_registry=true.
    if policy.enable_agent_registry {
        let session_agent_paths: Vec<std::path::PathBuf> = vec![];
        let registry = super::agent_registry::AgentRegistry::load(
            &session_agent_paths,
            std::path::Path::new(working_dir),
        );
        for warn in registry.warnings() {
            tracing::warn!("agent_registry: {warn}");
        }
        if let Some(manifest) = registry.routing_manifest() {
            if let Some(ref mut sys) = cached_system {
                *sys = format!("{sys}\n\n{manifest}");
            } else {
                cached_system = Some(manifest);
            }
            tracing::info!(
                "agent_registry: loaded {} agent(s); manifest injected",
                registry.len()
            );
        }
    }

    // Feature 7 (Frontier Roadmap 2026): Semantic Memory Vector Store.
    // Upgrades L3 from BM25 max-200 list to cosine-similarity + MMR retrieval over MEMORY.md.
    // When enabled: injects `search_memory` tool + registers SharedVectorStore in tool_exec_config.
    // Only active for the parent agent (not sub-agents) to avoid recursive index locking.
    if policy.enable_semantic_memory && !is_sub_agent {
        let repo_name = std::path::Path::new(working_dir)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Prefer index in the .halcon/memory/ dir found via ancestor walk, fallback to working_dir.
        let index_path = {
            let mut candidate = std::path::PathBuf::from(working_dir);
            let mut found = None;
            loop {
                let p = candidate.join(".halcon").join("memory");
                if p.exists() { found = Some(p); break; }
                match candidate.parent() { Some(p) => candidate = p.to_path_buf(), None => break }
            }
            found.unwrap_or_else(|| {
                let mut p = std::path::PathBuf::from(working_dir);
                p.push(".halcon"); p.push("memory");
                p
            })
        }.join("MEMORY.vindex.json");

        let mut vector_store = halcon_context::VectorMemoryStore::load_from_disk(index_path);
        if vector_store.is_empty() {
            vector_store.load_from_standard_locations(std::path::Path::new(working_dir), repo_name);
            if !vector_store.is_empty() {
                vector_store.save();
                tracing::info!(
                    "semantic_memory: indexed {} entries; index saved",
                    vector_store.len()
                );
            }
        } else {
            tracing::debug!(
                "semantic_memory: loaded {} entries from disk index",
                vector_store.len()
            );
        }

        if !vector_store.is_empty() {
            let shared_store: std::sync::Arc<std::sync::Mutex<halcon_context::VectorMemoryStore>> =
                Arc::new(std::sync::Mutex::new(vector_store));

            // Build the search_memory tool and inject its ToolDefinition into cached_tools.
            let sm_tool = halcon_tools::search_memory::SearchMemoryTool::new(shared_store.clone())
                .with_default_k(policy.semantic_memory_top_k);
            cached_tools.push(halcon_core::types::ToolDefinition {
                name: sm_tool.name().to_string(),
                description: sm_tool.description().to_string(),
                input_schema: sm_tool.input_schema(),
            });
            tool_exec_config.session_tools.push(Arc::new(sm_tool));

            tracing::info!(
                "semantic_memory: search_memory tool injected (top_k={})",
                policy.semantic_memory_top_k
            );
        }
    }

    // Inject context-aware system prompt from Context Servers (if assembled).
    // This adds context from all 8 SDLC-aware servers (requirements, architecture, etc.).
    if let Some(ref context_prompt) = context_system_prompt {
        if let Some(ref mut sys) = cached_system {
            // Prepend context to existing system prompt
            *sys = format!("{}\n\n{}", context_prompt, sys);
        } else {
            // Set as system prompt if none exists
            cached_system = Some(context_prompt.clone());
        }
    }

    // Sprint 1: Conversational directive injection is now handled per-round by the
    // CapabilityOrchestrationLayer (ConversationalDirectiveRule).  Applying it to
    // round_request.system rather than cached_system is functionally equivalent
    // because cached_system is cloned into each round_request.system.

    // Phase 1 Supervisor: closes the temporal Reflexion gap (NeurIPS 2023 Reflexion pattern).
    // Advice generated in round N is injected as a directive at the start of round N+1.
    let mut reflection_injector = super::supervisor::InSessionReflectionInjector::new();

    // Inject plan into system prompt so the model knows its own plan.
    if let Some(ref tracker) = execution_tracker {
        let plan = tracker.plan();
        let plan_section = format_plan_for_prompt(plan, tracker.current_step());
        if let Some(ref mut sys) = cached_system {
            update_plan_in_system(sys, &plan_section);
        }
    }

    // Phase 37: Attempt delegation of eligible plan steps to sub-agents.
    // F2 DecisionLayer gate: skip orchestration for Simple/Structured tasks.
    // Phase 2 SLA: gate orchestration through SLA budget (Fast mode = 0 sub-agents).
    let sla_allows_orch = sla_budget.as_ref().map_or(true, |b| b.allows_orchestration());
    let delegation_enabled = orchestrator_config.enabled
        && orchestration_decision.as_ref().map_or(true, |d| d.use_orchestration)
        && sla_allows_orch;
    if let Some(ref mut tracker) = execution_tracker {
        let delegation_router = super::delegation::DelegationRouter::new(delegation_enabled)
            .with_min_confidence(orchestrator_config.min_delegation_confidence);
        let decisions = delegation_router.analyze_plan(tracker.plan());

        if !decisions.is_empty() {
            let tasks_with_indices =
                delegation_router.build_tasks(tracker.plan(), &decisions, &request.model);

            // Mark steps as delegated in tracker.
            for (step_idx, task) in &tasks_with_indices {
                tracker.mark_delegated(*step_idx, task.task_id, &format!("{:?}", task.agent_type));
            }

            // C2 FIX: Build task_id → original plan-step-index mapping BEFORE
            // tasks_with_indices is consumed by the into_iter() below (which discards step
            // indices). Vec::position() on a plain ID Vec would return the delegation-Vec
            // index (0, 1, 2…) instead of the actual PlanStep index used by the tracker and DB.
            let task_id_to_step: std::collections::HashMap<uuid::Uuid, usize> =
                tasks_with_indices
                    .iter()
                    .map(|(step_idx, t)| (t.task_id, *step_idx))
                    .collect();

            let tasks: Vec<halcon_core::types::SubAgentTask> =
                tasks_with_indices.into_iter().map(|(_, t)| t).collect();

            // Capture task count before tasks is moved into orchestrator.
            let task_count = tasks.len();

            // Emit orchestrator wave header and per-task spawn events.
            render_sink.orchestrator_wave(1, 1, task_count);
            for task in tasks.iter() {
                // Use the same step_idx that task_id_to_step will return for this task
                // so that sub_agent_completed can find the spawned line (step_index must match).
                let step_idx = task_id_to_step.get(&task.task_id).copied().unwrap_or(0);
                // Use the plan step description (truncated) instead of the verbose instruction.
                let step_desc: String = tracker.plan().steps
                    .get(step_idx)
                    .map(|s| {
                        let desc: String = s.description.chars().take(50).collect();
                        format!("{:?} [{}]", task.agent_type, desc)
                    })
                    .unwrap_or_else(|| format!("{:?} [{}/{}]", task.agent_type, step_idx, task_count));
                render_sink.sub_agent_spawned(
                    step_idx,
                    task_count,
                    &step_desc,
                    &format!("{:?}", task.agent_type),
                );
            }

            // Build the permission awaiter so sub-agents display a TUI modal when they
            // need to execute Destructive tools (e.g. file_write).
            // In non-TUI builds (or when TUI sender is absent), pass None so sub-agents
            // fall back to auto-approve (set_non_interactive).
            #[cfg(feature = "tui")]
            let perm_awaiter_for_orch: Option<crate::render::sink::PermissionAwaiter> = {
                render_sink.tui_event_sender().map(|ui_tx| {
                    std::sync::Arc::new(
                        move |tool: &str,
                              args: &serde_json::Value,
                              risk: &str,
                              timeout_secs: u64,
                              reply_tx: tokio::sync::mpsc::UnboundedSender<
                            halcon_core::types::PermissionDecision,
                        >| {
                            let _ = ui_tx.send(crate::tui::events::UiEvent::PermissionAwaiting {
                                tool: tool.to_string(),
                                args: args.clone(),
                                risk_level: risk.to_string(),
                                timeout_secs,
                                reply_tx: Some(reply_tx),
                            });
                        },
                    ) as crate::render::sink::PermissionAwaiter
                })
            };
            #[cfg(not(feature = "tui"))]
            let perm_awaiter_for_orch: Option<crate::render::sink::PermissionAwaiter> = None;

            // Run orchestrator for delegated steps.
            let orch_result = super::orchestrator::run_orchestrator(
                uuid::Uuid::new_v4(),
                tasks,
                provider,
                tool_registry,
                event_tx,
                limits,
                orchestrator_config,
                routing_config,
                trace_db,
                response_cache,
                fallback_providers,
                &request.model,
                working_dir,
                request.system.as_deref(),
                guardrails,
                false, // Sub-agents run non-interactively (perm_awaiter handles TUI).
                false,
                perm_awaiter_for_orch,
                policy.clone(),
            )
            .await;

            // Feed orchestrator results back into tracker.
            if let Ok(orch_result) = orch_result {
                // Emit completion event for each sub-agent result.
                for r in &orch_result.sub_results {
                    // C2 FIX: look up the original plan-step index from the HashMap rather
                    // than using Vec::position(), which would return the delegation-Vec index.
                    let step_index = task_id_to_step.get(&r.task_id).copied().unwrap_or(0);
                    let summary = if !r.output_text.is_empty() {
                        r.output_text.chars().take(120).collect::<String>()
                    } else {
                        r.agent_result.summary.clone()
                    };
                    render_sink.sub_agent_completed(
                        step_index,
                        task_count,
                        r.success,
                        r.latency_ms,
                        &r.agent_result.tools_used,
                        r.rounds,
                        &summary,
                        r.error.as_deref().unwrap_or(""),
                    );
                }

                // Phase L: accumulate sub-agent token and tool attribution into coordinator session.
                // Previously only token counts were aggregated; tool_invocations was never incremented
                // for sub-agent tools, causing the TUI status bar to always show "Tools: 0 calls"
                // even when sub-agents successfully executed file_write, bash, grep, etc.
                for r in &orch_result.sub_results {
                    pre_loop_tokens_subagents += r.input_tokens + r.output_tokens;
                    // Attribute sub-agent tool calls to the coordinator session so the TUI
                    // status bar correctly reports the total tools executed this session.
                    session.tool_invocations += r.agent_result.tools_used.len() as u32;
                }

                // Phase L fix K6: Validate sub-agent outputs against their step contracts
                // before injecting into coordinator context. Rejects meta-questions, missing
                // synthesis, and outputs with no file references for analysis steps.
                //
                // RC-5 FIX (2026-02-28): Include ALL sub-agents in sub_outputs — even those
                // with empty output and no tools (error/timeout/empty-surface failures).
                // Previously these were filtered out, making the failure invisible to the
                // coordinator. The coordinator would synthesize without knowing a step was
                // missing, producing placeholder text that triggers unnecessary retries.
                //
                // Invariant: sub_outputs.len() == orch_result.sub_results.len()
                let sub_outputs: Vec<String> = orch_result.sub_results.iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let status = if r.success { "success" } else { "failed" };

                        // Construct effective_output based on what the sub-agent produced:
                        // 1. Tool-only (no text, tools executed): synthetic completion message
                        // 2. Empty failure (no text, no tools): explicit failure notice
                        // 3. Normal: use output_text as-is
                        let effective_output = if r.output_text.is_empty() && !r.agent_result.tools_used.is_empty() {
                            format!(
                                "Task completed via tool execution: {}. The operation was performed successfully.",
                                r.agent_result.tools_used.join(", ")
                            )
                        } else if r.output_text.is_empty() && r.agent_result.tools_used.is_empty() {
                            // RC-5: Sub-agent produced nothing (error, timeout, or empty tool surface).
                            // Expose the failure explicitly so the coordinator can acknowledge it.
                            if let Some(ref err) = r.error {
                                format!(
                                    "SUB-AGENT FAILED: {}. This task was NOT completed. \
                                     Do NOT fabricate results for this step.",
                                    err.chars().take(200).collect::<String>()
                                )
                            } else {
                                "SUB-AGENT FAILED: produced no output and executed no tools. \
                                 This task was NOT completed. Do NOT fabricate results for this step."
                                    .to_string()
                            }
                        } else {
                            r.output_text.clone()
                        };

                        // Build contract from the plan step this sub-agent was assigned.
                        let step_index = task_id_to_step.get(&r.task_id).copied().unwrap_or(0);
                        let plan = tracker.plan();
                        let step_description = plan.steps.get(step_index)
                            .map(|s| s.description.as_str())
                            .unwrap_or("");
                        let step_tool_name = plan.steps.get(step_index)
                            .and_then(|s| s.tool_name.as_deref());
                        let contract = super::subagent_contract_validator::SubAgentContract::from_step(
                            step_description,
                            step_tool_name,
                        );
                        let validation = super::subagent_contract_validator::SubAgentContractValidator::validate(
                            &effective_output,
                            &contract,
                        );

                        let text = match &validation.status {
                            super::subagent_contract_validator::ValidationStatus::Valid => {
                                // Output is valid — truncate for context budget.
                                // Use char-boundary-safe truncation: `str::floor_char_boundary`
                                // (stable since Rust 1.65) finds the largest byte index ≤ 600
                                // that falls on a valid UTF-8 char boundary, so multi-byte chars
                                // like '├' (3 bytes) are never split mid-sequence.
                                if effective_output.len() > 600 {
                                    let boundary = { let mut _fcb = (600).min(effective_output.len()); while _fcb > 0 && !effective_output.is_char_boundary(_fcb) { _fcb -= 1; } _fcb };
                                    format!("{}…", &effective_output[..boundary])
                                } else {
                                    effective_output.clone()
                                }
                            }
                            super::subagent_contract_validator::ValidationStatus::Rejected(reason) => {
                                // Output rejected — inject corrective notice for coordinator.
                                tracing::warn!(
                                    step = step_description,
                                    reason = ?reason,
                                    output_len = effective_output.len(),
                                    recoverable = reason.is_recoverable(),
                                    "Phase L K6: sub-agent output rejected by contract validator"
                                );
                                if !silent {
                                    render_sink.warning(
                                        &format!("[contract] sub-agent step \'{}\' rejected: {:?}", step_description, reason),
                                        Some("coordinator will receive corrective prompt"),
                                    );
                                }
                                super::subagent_contract_validator::SubAgentContractValidator::corrective_prompt(
                                    &contract,
                                    reason,
                                    &effective_output,
                                )
                            }
                        };
                        // BRECHA-S1: append unverified note when sub-agent produced no
                        // content-read evidence — coordinator is warned the output may be hallucinated.
                        let unverified_note = if !r.evidence_verified && r.content_read_attempts == 0 {
                            "\n[Note: this sub-agent did not read any files — claims may be unverified]"
                        } else {
                            ""
                        };
                        format!("**Sub-agent {} ({}):**\n{}{}", i + 1, status, text, unverified_note)
                    })
                    .collect();

                // Collect tools successfully executed by sub-agents before injection.
                // Used below to (1) add an anti-re-delegation warning and (2) remove those
                // tools from the coordinator's cached_tools so the model cannot call them
                // again even if it hallucinates — root cause of the 131s wasted R2 round.
                let delegated_ok_tools: Vec<String> = orch_result.sub_results
                    .iter()
                    .filter(|r| r.success)
                    .flat_map(|r| r.agent_result.tools_used.iter().cloned())
                    .collect();

                // BRECHA-R1 + FASE 5: Collect structured failed step context.
                // These are propagated to AgentLoopResult and injected into the critic
                // retry message with error categories so the planner can reason about
                // WHY steps failed, not just WHAT failed.
                for r in &orch_result.sub_results {
                    if !r.success {
                        let step_index = task_id_to_step.get(&r.task_id).copied().unwrap_or(0);
                        let plan = tracker.plan();
                        if let Some(step) = plan.steps.get(step_index) {
                            let desc: String = step.description.chars().take(120).collect();
                            if !pre_loop_failed_steps.iter().any(|s| s.description == desc) {
                                let error_msg = r.error.as_deref().unwrap_or("unknown error");
                                let error_category = crate::repl::agent_types::FailedStepErrorCategory::from_error_string(error_msg);
                                pre_loop_failed_steps.push(crate::repl::agent_types::FailedStepContext {
                                    description: desc,
                                    error_category,
                                    error_message: error_msg.chars().take(200).collect(),
                                });
                            }
                        }
                    }
                }

                if !sub_outputs.is_empty() {
                    render_sink.loop_guard_action(
                        "sub_agent_results",
                        &format!("{} sub-agent outputs collected — injecting into coordinator context", sub_outputs.len()),
                    );

                    // FASE 7: Use QuirkRegistry for provider-specific post-delegation injections.
                    // Replaces hardcoded anti-redo logic with composable quirks (AntiRedoQuirk, etc.).
                    let quirk_ctx = crate::repl::model_quirks::QuirkContext {
                        provider_name: provider.name(),
                        model: &request.model,
                        delegated_ok_tools: &delegated_ok_tools,
                        has_tools_in_request: !cached_tools.is_empty(),
                    };
                    let quirk_registry = {
                        let mut r = crate::repl::model_quirks::QuirkRegistry::new();
                        r.register(Box::new(crate::repl::model_quirks::AntiRedoQuirk));
                        r.register(Box::new(crate::repl::model_quirks::XmlArtifactFilterQuirk));
                        r
                    };
                    let injections = quirk_registry.post_delegation_injections(&quirk_ctx);
                    let anti_redo_note = if injections.is_empty() {
                        String::new()
                    } else {
                        injections.join("")
                    };

                    let results_context = format!(
                        "[Sub-Agent Results]\n\
                         SYNTHESIS RULES:\n\
                         • Do NOT repeat or quote sub-agent outputs verbatim.\n\
                         • Do NOT re-state findings already shown above.\n\
                         • Your role: validate coherence, add your own analysis, and provide ONE unified answer.\n\
                         • If results are self-evident, confirm briefly without repeating them.{anti_redo_note}\n\n\
                         {}\n",
                        sub_outputs.join("\n\n")
                    );
                    messages.push(ChatMessage {
                        role: Role::User,
                        content: MessageContent::Text(results_context),
                    });
                }

                let matched =
                    tracker.record_delegation_results(&orch_result.sub_results, rounds);

                // Persist to DB.
                if let Some(db) = trace_db {
                    for m in &matched {
                        let (status, detail) = match &m.outcome {
                            StepOutcome::Success { summary } => ("success", summary.as_str()),
                            StepOutcome::Failed { error } => ("failed", error.as_str()),
                            StepOutcome::Skipped { reason } => ("skipped", reason.as_str()),
                        };
                        let _ = db
                            .update_plan_step_outcome(
                                &tracker.plan().plan_id,
                                m.step_index as u32,
                                status,
                                detail,
                            )
                            .await;
                    }
                }

                // Render updated progress.
                let plan = tracker.plan();
                let (_, _, elapsed) = tracker.progress();
                render_sink.plan_progress_with_timing(
                    &plan.goal,
                    &plan.steps,
                    tracker.current_step(),
                    tracker.tracked_steps(),
                    elapsed,
                );

                // Post-delegation tool retention policy (FASE 1 remediation).
                //
                // Previous behavior: strip ALL delegated tools → coordinator left with
                // tool_count=0 → forced text-only synthesis → speculative shell scripts.
                //
                // New behavior: use tool_policy to classify tools. Only EXECUTION tools
                // (file_write, bash, git_commit, etc.) are removed. READ_ONLY tools
                // (file_read, directory_tree, grep, glob) are RETAINED so the coordinator
                // can verify sub-agent results before synthesising.
                //
                // CORE_RUNTIME_TOOLS: these tools are NEVER stripped, regardless of
                // delegation. They are fundamental runtime capabilities that the
                // coordinator must always be able to invoke (bash for inline execution,
                // file_read/grep for verification). Stripping them causes the coordinator
                // to hallucinate shell scripts instead of actually running commands.
                const CORE_RUNTIME_TOOLS: &[&str] = &["bash", "file_read", "grep"];

                if !delegated_ok_tools.is_empty() {
                    let candidate_remove =
                        crate::repl::tool_policy::tools_to_remove(&delegated_ok_tools);

                    // Never remove core runtime tools — they must always remain available.
                    let to_remove: std::collections::HashSet<String> = candidate_remove
                        .into_iter()
                        .filter(|t| !CORE_RUNTIME_TOOLS.contains(&t.as_str()))
                        .collect();

                    let before = cached_tools.len();
                    cached_tools.retain(|t| !to_remove.contains(&t.name));
                    let removed = before - cached_tools.len();
                    let retained_read_only: Vec<&str> = delegated_ok_tools
                        .iter()
                        .filter(|n| !to_remove.contains(n.as_str()))
                        .map(|s| s.as_str())
                        .collect();
                    if removed > 0 || !retained_read_only.is_empty() {
                        tracing::info!(
                            removed_execution_tools = ?to_remove,
                            removed_count = removed,
                            retained_read_only = ?retained_read_only,
                            core_runtime_protected = ?CORE_RUNTIME_TOOLS,
                            remaining_tools = cached_tools.len(),
                            "Post-delegation: tool policy applied — execution tools removed, core runtime + read-only retained"
                        );
                        if !silent {
                            render_sink.info(&format!(
                                "[tool-policy] removed {} execution tool(s), retained {} read-only + core runtime (bash/file_read/grep)",
                                removed,
                                retained_read_only.len()
                            ));
                        }
                    }
                }

                let delegated_count = matched.len();
                if delegated_count > 0 {
                    tracing::info!(delegated_count, "Steps delegated to sub-agents");
                }
            } else if let Err(ref e) = orch_result {
                let err_str = e.to_string();
                tracing::warn!(
                    "Delegation orchestrator failed: {err_str}, falling back to inline execution"
                );
                // H2 FIX: recover plan state — mark all delegated steps as failed so the
                // agent loop can re-execute them inline.  Without this they remain in
                // `Running` state indefinitely, corrupting progress reporting and blocking
                // completion detection (is_complete() would never return true).
                let failure_results: Vec<halcon_core::types::SubAgentResult> = task_id_to_step
                    .keys()
                    .map(|&task_id| halcon_core::types::SubAgentResult {
                        task_id,
                        success: false,
                        output_text: String::new(),
                        agent_result: halcon_core::types::AgentResult {
                            success: false,
                            summary: String::new(),
                            files_modified: vec![],
                            tools_used: vec![],
                        },
                        input_tokens: 0,
                        output_tokens: 0,
                        cost_usd: 0.0,
                        latency_ms: 0,
                        rounds: 0,
                        error: Some(format!("orchestrator failed: {err_str}")),
                        evidence_verified: false,
                        content_read_attempts: 0,
                        had_tools_available: false,
                    })
                    .collect();
                tracker.record_delegation_results(&failure_results, rounds);
            }
        }
    }

    // ── ExecutionIntentPhase derivation — must run before pre-loop guard ──────────
    //
    // Derive the task's execution intent from the plan. This controls whether synthesis
    // guards are allowed to suppress tools. Execution tasks (bash, file_write, etc.)
    // must keep tools active until all steps are done to avoid premature synthesis.

    /// Tool names that signal an EXECUTION-type task (not read-only analysis).
    const EXECUTION_TOOL_NAMES: &[&str] = &[
        "bash",
        "file_write",
        "edit_file",
        "run_command",
        "terminal",
        "apply_patch",
        "code_execution",
    ];

    let execution_intent = if let Some(ref tracker) = execution_tracker {
        let plan = tracker.plan();
        let has_executable = plan.steps.iter()
            .any(|s| s.tool_name.as_deref()
                .map(|t| EXECUTION_TOOL_NAMES.contains(&t))
                .unwrap_or(false));
        let tool_steps = plan.steps.iter().filter(|s| s.tool_name.is_some()).count();

        if has_executable && tool_steps >= 2 {
            ExecutionIntentPhase::Execution
        } else if tool_steps >= 1 {
            ExecutionIntentPhase::Investigation
        } else {
            ExecutionIntentPhase::Uncategorized
        }
    } else {
        ExecutionIntentPhase::Uncategorized
    };
    tracing::debug!(intent = ?execution_intent, "ExecutionIntent derived from plan");

    // Declared early so Phase 3A can set it before LoopState construction.
    // Flag set when cross-type oscillation detection forces a synthesis break,
    // or when Phase 3A detects all remaining steps are synthesis-only.
    let mut forced_synthesis_detected = false;

    // ── Phase 3A: Pre-loop synthesis guard — MUST RUN BEFORE tool directive injection ──
    //
    // Root cause (vucem3-qa benchmark + session e2adfb4f analysis):
    // When all remaining plan steps are synthesis-only, clearing cached_tools ensures
    // the coordinator API call has tools=[] AND — critically — the tool-mode directives
    // (AUTONOMOUS_AGENT_DIRECTIVE, TOOL_USAGE_POLICY) are NOT injected into the system
    // prompt because they are gated on `!cached_tools.is_empty()` below.
    //
    // Previously this guard ran AFTER the directive injection (lines 1310–1352 in the
    // original file), causing the system prompt to instruct the model to "use tools
    // proactively" even though tools=[] in the API request.  Providers such as
    // deepseek-chat responded by emitting `<function_calls><invoke>` XML embedded in
    // end_turn text — never executed, but polluting full_text and causing the LoopCritic
    // to rate the session at 15% confidence.
    //
    // Fix: move this guard FIRST so directive injection sees the final cached_tools state.
    // ExecutionIntent guard: skip clearing tools when task is Execution-type to prevent
    // premature synthesis before all bash/file_write steps have been executed.
    if let Some(ref tracker) = execution_tracker {
        let plan = tracker.plan();
        let has_pending = plan.steps.iter().any(|s| s.outcome.is_none());

        // BUGFIX (synthesis-premature-strip): The original condition checked whether ALL
        // pending steps had tool_name=None and treated that as "synthesis-only mode".
        // This is WRONG: coordination/analysis steps also have tool_name=None but they
        // appear mid-plan before execution steps. Removing execution tools at that point
        // causes tools_executed=0 and dependency_cascade in subsequent waves.
        //
        // CORRECT condition: only enter synthesis mode when NO pending step has a
        // tool_name (i.e. zero execution steps remain). Coordination steps interspersed
        // with execution steps must NOT trigger this guard.
        //
        // See docs/analysis/addendum-2026-03-08.md BUG-007 for full root cause trace.
        let any_pending_execution = plan.steps.iter()
            .filter(|s| s.outcome.is_none())
            .any(|s| s.tool_name.is_some());
        let all_pending_synthesis = has_pending && !any_pending_execution;

        if all_pending_synthesis
            && execution_intent != ExecutionIntentPhase::Execution
        {
            // FIX: Phase 3A must respect tool_policy — only remove EXECUTION tools,
            // retain READ_ONLY tools so the coordinator can verify sub-agent results
            // before synthesising. Previously `cached_tools.clear()` wiped everything,
            // leaving the coordinator with tool_count=0 and no ability to investigate.
            let before = cached_tools.len();
            let execution_names: std::collections::HashSet<String> = cached_tools.iter()
                .filter(|t| crate::repl::tool_policy::classify(&t.name)
                    == crate::repl::tool_policy::ToolCategory::Execution)
                .map(|t| t.name.clone())
                .collect();
            cached_tools.retain(|t| !execution_names.contains(&t.name));
            let removed = before - cached_tools.len();
            let retained = cached_tools.len();

            // Signal to Phase 3C (round_setup) that this is a synthesis round.
            // Without this flag, Phase 3C sees tools ≠ empty and no synthesis state,
            // so it doesn't apply max_tokens cap or synthesis constraint.
            forced_synthesis_detected = true;

            if removed > 0 || retained > 0 {
                tracing::info!(
                    removed_execution = removed,
                    retained_read_only = retained,
                    "Pre-loop synthesis guard: all pending steps are synthesis-only — \
                     removed execution tools, retained read-only for verification \
                     (tool directives will NOT be injected into system prompt)"
                );
                if !silent {
                    render_sink.info(&format!(
                        "[synthesis] removed {} execution tool(s), retained {} read-only — \
                         all remaining steps are synthesis-only",
                        removed, retained
                    ));
                }
            }
        }
    }

    // AUTONOMY FIX: Inject autonomous agent directive to promote proactive behavior.
    // This instructs the model to plan, execute completely, and solve problems autonomously.
    // INVARIANT: only injected when cached_tools is non-empty (synthesis mode gets no tool directives).
    const AUTONOMOUS_AGENT_DIRECTIVE: &str = "\n\n## Autonomous Agent Behavior\n\
        You are an autonomous coding assistant with planning and execution capabilities.\n\
        \n\
        When given a task:\n\
        1. **PLAN**: If a plan was generated, follow it step-by-step. Otherwise, mentally decompose complex requests.\n\
        2. **EXECUTE**: Use tools proactively to gather ALL necessary information and implement solutions.\n\
        3. **COMPLETE**: Finish the entire task. Don't stop halfway or ask for permission at each step.\n\
        4. **VERIFY**: Check your work using available tools before presenting results.\n\
        \n\
        Be proactive and decisive:\n\
        - If asked to \"analyze\", \"improve\", \"fix\", or \"refactor\" — DO IT COMPLETELY.\n\
        - Use tools strategically to understand context, make changes, and validate results.\n\
        - Execute all necessary steps to solve the problem, not just answer questions about it.\n\
        - Your goal is to DELIVER WORKING SOLUTIONS, not provide guidance.\n\
        \n\
        ANTI-COLLAPSE RULE (CRITICAL):\n\
        - NEVER synthesize or summarize before all executable steps are complete.\n\
        - If the task involves build, install, run, deploy, or test commands — execute ALL of them with tools.\n\
        - Do NOT convert executable work into narrative explanation.\n\
        - Synthesis is only allowed when: (a) no further tool actions are needed AND (b) the objective is fully achieved.\n\
        - Execution loop: PLAN → EXECUTE → ANALYZE → ADAPT → CONTINUE until done.\n";

    // Phase 33: inject tool usage policy into the system prompt.
    // Instructs the model to converge: prefer fewer tool calls, don't repeat,
    // respond directly once enough information is gathered.
    // INVARIANT: only injected when cached_tools is non-empty (synthesis mode gets no tool directives).
    const TOOL_USAGE_POLICY: &str = "\n\n## Tool Usage Policy\n\
        - Only call tools when you need NEW information or need to perform an action.\n\
        - Never call the same tool twice with the same or very similar arguments.\n\
        - For INVESTIGATION tasks: prefer fewer tool calls (1-3 rounds). Stop when you have enough info.\n\
        - For EXECUTION tasks (build/run/install/deploy): call as many tools as needed to complete ALL steps.\n\
        - When the objective is complete, stop calling tools and provide a final summary.\n\
        - If a tool fails, try a different approach or inform the user — do not retry the same call.\n";

    if !cached_tools.is_empty() {
        if let Some(ref mut sys) = cached_system {
            // Inject autonomous agent directive first (sets proactive mindset)
            if !sys.contains("## Autonomous Agent Behavior") {
                sys.push_str(AUTONOMOUS_AGENT_DIRECTIVE);
            }
            // Then inject tool usage policy (sets convergence rules)
            if !sys.contains("## Tool Usage Policy") {
                sys.push_str(TOOL_USAGE_POLICY);
            }
            // IMP-4: Dynamic tool capability detection — inject the list of
            // available tools so the planner generates steps that reference
            // real, registered tool names rather than hallucinated ones.
            // Skipped for sub-agents: they inherit a narrowed tool surface that
            // is already explicit in their system prompt; adding the full catalog
            // would inflate their context window unnecessarily (BUG-IMP4-B).
            if !is_sub_agent && !sys.contains("<!-- HALCON_TOOLS_START -->") {
                use std::fmt::Write as FmtWrite;
                let mut catalog = String::from("\n\n<!-- HALCON_TOOLS_START -->\n## Available Tools\n\n");
                for tool in &cached_tools {
                    let first_line = tool.description.lines().next().unwrap_or("").trim();
                    let _ = writeln!(catalog, "- **{}**: {}", tool.name, first_line);
                }
                catalog.push_str("<!-- HALCON_TOOLS_END -->");
                sys.push_str(&catalog);
                tracing::debug!(
                    tool_count = cached_tools.len(),
                    "IMP-4: tool capability catalog injected into system prompt"
                );
            }
        }
    }

    // Confidence feedback: track the last reflection's entry_id so we can
    // boost it on subsequent success or decay it on repeated failure.
    let mut last_reflection_id: Option<uuid::Uuid> = None;

    // Tool speculation: provided via AgentContext, shared across rounds for metrics.
    // Speculator is already destructured from ctx above, available as `speculator` variable.

    // RC-2 fix: track repeated tool failures to prevent infinite retry loops.
    // Threshold=3: after 3 identical failure patterns, inject a strong directive.
    let mut failure_tracker = ToolFailureTracker::new(3);

    // Phase 30: when fallback adapts the model (e.g., anthropic→ollama),
    // persist the adapted model name so subsequent rounds use it.
    let mut fallback_adapted_model: Option<String> = None;

    // Phase 33: intelligent tool loop guard — multi-layered termination.
    // Replaces the blunt consecutive_tool_rounds >= 5 counter with graduated
    // escalation: synthesis directive → forced tool withdrawal → break.
    let mut loop_guard = ToolLoopGuard::with_policy(&policy);

    // Sprint 1: CapabilityOrchestrationLayer — centralises 5 STRIP points.
    // Replaces: conversational directive injection, force_no_tools ModelRequest
    // assignment, Ollama emulation strip, flag reset, and model capability check.
    let capability_orchestrator =
        super::plugins::capability_orchestrator::CapabilityOrchestrationLayer::with_default_rules();

    // Step 8b: Apply UCB1 StrategyContext tightness to ToolLoopGuard thresholds.
    // DirectExecution+Simple (tightness=0.3) → relaxed thresholds; PlanExecuteReflect+Complex (0.8) → tight.
    if let Some(ref sc) = strategy_context {
        loop_guard.set_tightness(sc.loop_guard_tightness);
        tracing::info!(
            strategy = ?sc.strategy,
            tightness = sc.loop_guard_tightness,
            replan_sensitivity = sc.replan_sensitivity,
            routing_bias = ?sc.routing_bias,
            enable_reflection = sc.enable_reflection,
            "StrategyContext applied to agent loop"
        );
    }

    // Phase 2: RoundScorer — per-round multi-dimensional evaluation.
    // Seeded with the user goal text for coherence scoring.
    // Accumulates RoundEvaluation snapshots fed to the UCB1 reward pipeline.
    let goal_text = request.messages.iter().rev()
        .find(|m| m.role == Role::User)
        .map(|m| match &m.content {
            halcon_core::types::MessageContent::Text(t) => t.as_str(),
            _ => "",
        })
        .unwrap_or("");
    let mut round_scorer = super::round_scorer::RoundScorer::new(goal_text, policy.clone());
    // Phase 2 causal wiring: apply UCB1 replan_sensitivity so the scorer's structural
    // thresholds reflect the strategy plan (DirectExecution stays permissive, complex
    // PlanExecuteReflect strategies become hair-trigger on low-trajectory rounds).
    if let Some(ref sc) = strategy_context {
        round_scorer.set_replan_sensitivity(sc.replan_sensitivity);
    }
    let mut round_evaluations: Vec<super::round_scorer::RoundEvaluation> = Vec::new();

    // Sprint 3: AdaptivePolicy — within-session parameter self-adjustment (L6 enabler).
    // Initialized with the base sensitivity from StrategyContext so escalation builds
    // on top of the UCB1-selected starting point rather than resetting to zero.
    let base_sensitivity = strategy_context.as_ref()
        .map(|sc| sc.replan_sensitivity)
        .unwrap_or(0.0);
    let mut adaptive_policy = super::adaptive_policy::AdaptivePolicy::new(base_sensitivity);

    // Step 8b (continued): PlanCoherenceChecker — Jaccard semantic drift detection.
    // Initialized with the user goal; checked after each structural replan to detect drift.
    let coherence_checker = super::plan_coherence::PlanCoherenceChecker::new_with_threshold(
        goal_text,
        policy.drift_threshold,
    );
    let mut cumulative_drift_score = 0.0f32;
    let mut drift_replan_count = 0usize;

    // P2 FIX: Replan convergence budget.
    // Prevents infinite replan cascade: if ReplanRequired fires repeatedly and each
    // new plan immediately stalls again, we cap total replan attempts (policy.max_replan_attempts)
    // and escalate to forced synthesis so the agent always terminates.
    let mut replan_attempts: u32 = 0;

    // HICON Phase 4: Agent self-corrector for adaptive strategy adjustment.
    let mut self_corrector = super::self_corrector::AgentSelfCorrector::new();

    // HICON Phase 5: ARIMA resource predictor for proactive budget management.
    let mut resource_predictor = super::arima_predictor::ResourcePredictor::new();

    // HICON Phase 6: Metacognitive loop for system-wide coherence monitoring.
    let mut metacognitive_loop = super::metacognitive_loop::MetacognitiveLoop::new();

    // BV-1 fix (complete): Compute the final effective budget BEFORE constructing
    // ConvergenceController so calibration and loop bound share a single source of truth.
    // Legacy path now uses new_with_budget() just like the IntentPipeline path,
    // eliminating the previous multi-step set_max_rounds() overwrite pattern.
    //
    // Order of precedence (highest to lowest):
    //   1. IntentPipeline resolved_intent.effective_max_rounds (unified BDE+SLA reconciliation)
    //   2. SLA-clamped user config (legacy path: clamp_rounds(ctx.limits.max_rounds))
    //   3. User config max_rounds (no SLA) as fallback
    //   4. Sub-agent cap: min(profile_limit=6, parent_limits.max_rounds)
    let mut effective_max_rounds: usize = if let Some(ref ri) = resolved_intent {
        ri.effective_max_rounds as usize
    } else {
        match &sla_budget {
            Some(b) => {
                let clamped = b.clamp_rounds(ctx.limits.max_rounds as u32) as usize;
                if clamped < ctx.limits.max_rounds {
                    tracing::info!(
                        config = ctx.limits.max_rounds,
                        sla_clamped = clamped,
                        mode = ?b.mode,
                        "SlaManager: clamped max_rounds (BV-1 fix: resolved before ConvergenceController construction)"
                    );
                }
                clamped
            }
            None => ctx.limits.max_rounds,
        }
    };

    // SOTA 2026: ConvergenceController — adaptive loop termination driven by IntentProfile.
    // Sub-agents use new_for_sub_agent() with tighter limits + multilingual keyword extraction
    // so Spanish-language instructions don't cause false-negative coverage misses.
    // Top-level agents use new_with_budget() with the final resolved budget (BV-1 fix).
    let mut conv_ctrl = if ctx.is_sub_agent {
        super::convergence_controller::ConvergenceController::new_for_sub_agent(&user_msg)
    } else {
        // Both IntentPipeline and legacy paths now use new_with_budget() with the
        // pre-computed effective_max_rounds — single construction, no post-override needed.
        super::convergence_controller::ConvergenceController::new_with_budget(
            &task_analysis,
            effective_max_rounds as u32,
            &user_msg,
        )
    };
    // Sub-agents only: cap to min(profile_limit=6, parent_limits.max_rounds).
    if ctx.is_sub_agent {
        conv_ctrl.cap_max_rounds(ctx.limits.max_rounds);
    }
    // Fix 2/3: Track plan truncation so the EvidenceThreshold guard (Fix 4) and
    // post-loop telemetry can detect SLA-driven silent truncation.
    let mut plan_was_sla_truncated = false;
    let mut original_plan_step_count = 0usize;

    // Phase 2 SLA: clamp plan depth independently of rounds.
    // max_plan_depth limits step count even if max_rounds would accommodate more.
    if let Some(ref budget) = &sla_budget {
        if let Some(ref mut plan) = active_plan {
            let max_depth = budget.clamp_plan_depth(plan.steps.len() as u32) as usize;
            if plan.steps.len() > max_depth {
                original_plan_step_count = plan.steps.len();
                tracing::info!(
                    original = plan.steps.len(),
                    clamped = max_depth,
                    mode = ?budget.mode,
                    "SLA: clamping plan depth via clamp_plan_depth()"
                );
                plan.steps.truncate(max_depth);
                if let Some(ref mut tracker) = execution_tracker {
                    tracker.truncate_to(max_depth);
                }
                plan_was_sla_truncated = true;
            }
        }
    }

    if !is_sub_agent {
        if let Some(ref mut plan) = active_plan {
            // INVARIANT K5-1: max_rounds ≥ plan.total_steps + critic_retries + 1 (synthesis)
            let max_critic_retries: u32 = 1; // ReasoningConfig::default max_retries
            let required = plan.steps.len() + max_critic_retries as usize + 1;
            if effective_max_rounds < required {
                // Phase 2 SLA: when SLA is active, truncate plan to fit budget
                // instead of expanding rounds unboundedly. Leave room for critic + synthesis.
                if let Some(ref budget) = sla_budget {
                    let sla_max = budget.clamp_rounds(ctx.limits.max_rounds as u32) as usize;
                    let max_plan_steps = sla_max.saturating_sub(2); // room for critic + synthesis
                    if plan.steps.len() > max_plan_steps && max_plan_steps > 0 {
                        if !plan_was_sla_truncated {
                            original_plan_step_count = plan.steps.len();
                        }
                        tracing::warn!(
                            plan_steps = plan.steps.len(),
                            sla_max,
                            truncated_to = max_plan_steps,
                            mode = ?budget.mode,
                            "Phase 2 SLA K5-1: truncating plan to fit SLA budget"
                        );
                        plan.steps.truncate(max_plan_steps);
                        if let Some(ref mut tracker) = execution_tracker {
                            tracker.truncate_to(max_plan_steps);
                        }
                        plan_was_sla_truncated = true;
                        // Don't expand rounds beyond SLA — plan was truncated to fit.
                    } else {
                        // Plan fits within SLA budget or budget too small — expand as before.
                        tracing::warn!(
                            effective_max_rounds,
                            required,
                            plan_steps = plan.steps.len(),
                            max_critic_retries,
                            "Phase L K5-1: expanding effective_max_rounds to satisfy budget invariant"
                        );
                        effective_max_rounds = required;
                        conv_ctrl.set_max_rounds(effective_max_rounds);
                    }
                } else {
                    // No SLA — expand rounds as before.
                    tracing::warn!(
                        effective_max_rounds,
                        required,
                        plan_steps = plan.steps.len(),
                        max_critic_retries,
                        "Phase L K5-1: expanding effective_max_rounds to satisfy budget invariant"
                    );
                    effective_max_rounds = required;
                    conv_ctrl.set_max_rounds(effective_max_rounds);
                }
            }
        }
    }

    // Phase 112 — Unified SOTA 2026 telemetry: log all pipeline signals in one structured span
    // so distributed traces can correlate IntentScorer → ModelRouter → ConvergenceController.
    tracing::info!(
        task_type              = ?task_analysis.task_type,
        complexity             = ?task_analysis.complexity,
        scope                  = ?task_analysis.scope,
        reasoning_depth        = ?task_analysis.reasoning_depth,
        detected_language      = ?task_analysis.detected_language,
        estimated_tool_calls   = task_analysis.estimated_tool_calls,
        latency_tolerance      = ?task_analysis.latency_tolerance,
        ambiguity_score        = task_analysis.ambiguity_score,
        convergence_max_rounds = task_analysis.suggested_max_rounds(),
        "SOTA 2026 pipeline: IntentScorer → ConvergenceController"
    );

    // (forced_synthesis_detected declared earlier, before Phase 3A.)
    // Phase 113 SOTA: Prevents double-synthesis when ConvergenceController injects a Replan
    // directive AND ToolLoopGuard fires InjectSynthesis in the same round.  Both write a
    // User message — two conflicting instructions cause incoherent model behaviour.
    // Rule: InjectSynthesis is suppressed this round if convergence already injected a directive.
    // Note: initial false is immediately overwritten at loop start — the meaningful reads are
    // inside the loop body (line ~4692).  Suppress the "never read" false positive.
    #[allow(unused_assignments)]
    let mut convergence_directive_injected = false;
    // P0-C: Flag set when the ToolFailureTracker detects that ALL active MCP tools are
    // persistently unavailable (circuit breaker trips on "mcp_unavailable" pattern).
    // Continuing to loop burns rounds — halt immediately with EnvironmentError so UCB1
    // receives a clean zero-score and avoids this strategy+env combination in future.
    let mut environment_error_halt = false;
    // Track the model used in the last agent round for post-loop quality recording (Phase 4).
    let mut last_round_model_name = request.model.clone();

    // ── Bundle all owned mutable state into LoopState ─────────────────────
    // From here, the loop and post-loop sections access owned state via `state.xxx`.
    // Borrowed infrastructure (provider, session, limits, render_sink, etc.) remain
    // as local variables and are passed explicitly to phase functions.
    let mut state = LoopState {
        messages,
        context_pipeline,
        full_text,
        rounds,
        session_id,
        trace_step_index,
        active_plan,
        execution_tracker,
        compaction_model,
        cached_tools,
        cached_system,
        cached_instructions,
        instruction_store,
        is_conversational_intent,
        reflection_injector,
        last_reflection_id,
        tbac_pushed,
        tool_trust: super::tool_trust::ToolTrustScorer::new(policy.clone()),
        fallback_adapted_model,
        tokens: loop_state::TokenAccounting {
            call_input_tokens,
            call_output_tokens,
            call_cost,
            pipeline_budget,
            provider_context_window: model_context_window,
            tokens_planning: 0,
            tokens_subagents: pre_loop_tokens_subagents,
            tokens_critic: 0,
            call_input_tokens_prev_round: 0,
            tokens_per_round: Vec::new(),
            consecutive_growth_violations: 0,
            k5_2_compaction_needed: false,
        },
        evidence: loop_state::EvidenceState {
            bundle: Default::default(),
            graph: super::evidence_graph::EvidenceGraph::new(),
            deterministic_boundary_enforced: false,
            blocked_tools: Vec::new(),
        },
        synthesis: loop_state::SynthesisControl::new(
            forced_synthesis_detected,
            ToolDecisionSignal::Allow,
            execution_intent,
            convergence_directive_injected,
        ),
        convergence: loop_state::ConvergenceState {
            convergence_detector,
            conv_ctrl,
            round_scorer,
            round_evaluations,
            adaptive_policy,
            coherence_checker,
            cumulative_drift_score,
            drift_replan_count,
            replan_attempts,
            last_convergence_ratio,
            macro_plan_view,
            mid_loop_critic: super::domain::mid_loop_critic::MidLoopCritic::new(
                policy.clone(),
                effective_max_rounds,
            ),
            complexity_tracker: super::domain::complexity_feedback::ComplexityTracker::new(
                orchestration_decision
                    .as_ref()
                    .map(|d| d.complexity.clone())
                    .unwrap_or(super::decision_layer::TaskComplexity::StructuredTask),
                effective_max_rounds,
                policy.clone(),
            ),
            invariant_checker: super::domain::system_invariants::SystemInvariantChecker::new(),
            decision_trace: super::domain::agent_decision_trace::DecisionTraceCollector::new(),
            metrics_collector: super::domain::system_metrics::MetricsCollector::new(),
            adaptation_bounds: super::domain::adaptation_bounds::AdaptationBoundsChecker::new(
                policy.clone(),
            ),
            problem_classifier: super::domain::problem_classifier::ProblemClassifier::new(
                policy.clone(),
            ),
            strategy_weight_manager: super::domain::strategy_weights::StrategyWeightManager::new(
                policy.clone(),
            ),
            routing_escalation_count: 0,
        },
        guards: loop_state::LoopGuardState {
            loop_guard,
            failure_tracker,
            capability_orchestrator,
            semantic_cycle_detector: super::domain::semantic_cycle::SemanticCycleDetector::from_policy(&policy),
        },
        hicon: loop_state::HiconSubsystems {
            self_corrector,
            resource_predictor,
            metacognitive_loop,
        },
        environment_error_halt,
        auto_pause,
        ctrl_cancelled,
        model_downgrade_advisory_active: false,
        forced_routing_bias: None,
        last_round_model_name,
        next_round_restarts: 0,
        loop_start,
        tool_timeout,
        silent,
        user_msg: user_msg.clone(),
        goal_text: goal_text.to_string(),
        l4_archive_path: l4_archive_path.clone(),
        strategy_context: strategy_context.clone(),
        orchestration_decision,
        sla_budget,
        plan_was_sla_truncated,
        original_plan_step_count,
        boundary_decision,
        tools_executed: Vec::new(),
        failed_sub_agent_steps: pre_loop_failed_steps,
        policy: policy.clone(),
        env_snapshot: super::domain::capability_validator::EnvironmentSnapshot::default(),
        // Phase 3: Goal Progress Control — None until first batch closes.
        last_progress_snapshot: None,
        // Phase 4: Adaptive Control Layer — conservative defaults.
        consecutive_stalls:      0,
        consecutive_regressions: 0,
        progress_policy_config:  loop_state::ProgressPolicyConfig::default(),
    };

    // P5.5: Strategic initialization — data-driven round-0 configuration.
    if state.policy.strategic_init_enabled {
        let task_complexity = state.orchestration_decision.as_ref()
            .map(|d| match d.complexity {
                super::decision_layer::TaskComplexity::SimpleExecution
                    => super::domain::strategic_init::Complexity::Simple,
                super::decision_layer::TaskComplexity::StructuredTask
                    => super::domain::strategic_init::Complexity::Structured,
                super::decision_layer::TaskComplexity::MultiDomain
                    => super::domain::strategic_init::Complexity::MultiDomain,
                super::decision_layer::TaskComplexity::LongHorizon
                    => super::domain::strategic_init::Complexity::LongHorizon,
            })
            .unwrap_or(super::domain::strategic_init::Complexity::Structured);
        let available_tools: Vec<String> = state.cached_tools.iter()
            .map(|t| t.name.clone())
            .collect();
        let profile = super::domain::strategic_init::initialize(
            task_complexity,
            &state.user_msg,
            &available_tools,
        );
        tracing::debug!(
            problem_class = %profile.problem_class.label(),
            granularity = %profile.granularity.label(),
            exploration_budget = %profile.exploration_budget,
            rationale = profile.rationale,
            "Phase5 StrategicInit: data-driven round-0 configuration"
        );
        // Apply profile: set baseline weights
        state.convergence.strategy_weight_manager.set_baseline(profile.weights);
        // Apply profile: set initial sensitivity on AdaptivePolicy
        state.convergence.adaptive_policy = super::domain::adaptive_policy::AdaptivePolicy::new(
            profile.initial_sensitivity,
        );
    }

    // Fire FSM events to replay prologue state transitions through the typed FSM.
    // The plan was generated before LoopState existed — replay the transitions so the
    // FSM properly enters Planning before settling in Executing.
    if state.active_plan.is_some() {
        state.synthesis.advance_phase(AgentEvent::PlanGenerated); // Idle → Planning
        state.synthesis.advance_phase(AgentEvent::PlanGenerated); // Planning → Executing
    } else {
        state.synthesis.advance_phase(AgentEvent::PlanSkipped);   // Idle → Executing
    }

    'agent_loop: for round in 0..effective_max_rounds {
        // Round separator is emitted after model selection (see below) so we can show provider info.
        // Reset per-round coordination flags.
        state.synthesis.convergence_directive_injected = false;

        let _round_span = tracing::info_span!(
            "gen_ai.agent.round",
            "gen_ai.request.model" = %request.model,
            "gen_ai.operation.name" = "agent_round",
            round,
        )
        .entered();
        let round_start = Instant::now();
        let mut round_usage = TokenUsage::default();

        // Phase 2 SLA: warn at 80% budget consumption, force synthesis on expiry.
        if let Some(ref budget) = state.sla_budget {
            let frac = budget.fraction_consumed();
            if frac >= 0.80 && !budget.is_expired() {
                tracing::warn!(
                    mode = ?budget.mode,
                    fraction_consumed = format!("{:.0}%", frac * 100.0),
                    round,
                    "SLA: 80% of time budget consumed — approaching limit"
                );
            }
            if budget.is_expired() {
                tracing::warn!(mode = ?budget.mode, "SLA: time expired, forcing synthesis");
                state.synthesis.tool_decision.set_force_next();
                // Phase 2: route through governance gate (SLA wall-clock expired).
                state.mark_synthesis_forced_with_gate(
                    SynthesisTrigger::ReplanTimeout,
                    SynthesisOrigin::ReplanTimeout,
                );
            }
        }

        // Phase 1: emit RoundStarted event (additive — fire-and-forget, no behavior change).
        loop_events::emit(
            &state.session_id.to_string(),
            round as u32,
            loop_events::LoopEvent::RoundStarted {
                round,
                model: request.model.clone(),
            },
            trace_db,
        );

        // ── Round setup phase ──────────────────────────────────────────────────────────────────
        // Compaction, model selection, capability orchestration, security gates, cache check.
        let round_setup_out = match round_setup::run(
            &mut state,
            session,
            render_sink,
            provider,
            request,
            limits,
            event_tx,
            trace_db,
            response_cache,
            compactor,
            model_selector,
            registry,
            context_metrics,
            working_dir,
            guardrails,
            security_config,
            exec_clock,
            round,
        ).await? {
            round_setup::RoundSetupOutcome::BreakLoop => break 'agent_loop,
            round_setup::RoundSetupOutcome::EarlyReturn(data) => {
                // Assemble AgentLoopResult here so ctrl_rx and plugin_registry stay in scope.
                return Ok(AgentLoopResult {
                    full_text: data.full_text,
                    rounds: data.rounds,
                    stop_condition: data.stop_condition,
                    input_tokens: data.call_input_tokens,
                    output_tokens: data.call_output_tokens,
                    cost_usd: data.call_cost,
                    latency_ms: data.latency_ms,
                    execution_fingerprint: data.execution_fingerprint,
                    timeline_json: None,
                    ctrl_rx,
                    critic_verdict: None,
                    round_evaluations: data.round_evaluations,
                    plan_completion_ratio: 0.0,
                    avg_plan_drift: 0.0,
                    oscillation_penalty: 0.0,
                    last_model_used: None,
                    plugin_cost_snapshot: plugin_registry
                        .as_ref()
                        .and_then(|arc_pr| arc_pr.lock().ok().map(|pr| pr.cost_snapshot()))
                        .unwrap_or_default(),
                    tools_executed: state.tools_executed,
                    evidence_verified: !state.evidence.bundle.evidence_gate_fires(),
                    content_read_attempts: state.evidence.bundle.content_read_attempts,
                    last_provider_used: None,
                    blocked_tools: state.evidence.blocked_tools,
                    failed_sub_agent_steps: state.failed_sub_agent_steps,
                    critic_unavailable: false,
                    tool_trust_failures: state.tool_trust.failure_records(),
                    sla_budget: state.sla_budget,
                    evidence_coverage: state.evidence.graph.synthesis_coverage(),
                    // Phase 2: early-exit path — no synthesis triggered.
                    synthesis_kind:    state.synthesis.last_synthesis_kind,
                    synthesis_trigger: state.synthesis.last_synthesis_trigger,
                    routing_escalation_count: state.convergence.routing_escalation_count,
                });
            }
            round_setup::RoundSetupOutcome::Continue(out) => out,
        };
        let round_setup::RoundSetupOutput { mut round_request, effective_provider, selected_model } = round_setup_out;

        // ── Provider round phase ───────────────────────────────────────────────────────────────
        // Control check, output headroom guard, spinner, speculative pre-exec,
        // provider invocation + streaming + retry, budget guards, non-tool path.
        let provider_out = match provider_round::run(
            &mut state,
            session,
            render_sink,
            &effective_provider,
            round_request,        // moved
            fallback_providers,
            resilience,
            routing_config,
            event_tx,
            trace_db,
            limits,
            guardrails,
            provider,
            request,
            exec_clock,
            round_start,
            &mut ctrl_rx,
            speculator,
            tool_registry,
            working_dir,
            round,
            &selected_model,
            model_selector,
            response_cache,
            replay_tool_executor,
        ).await? {
            provider_round::ProviderRoundOutcome::BreakLoop => break 'agent_loop,
            provider_round::ProviderRoundOutcome::EarlyReturn(data) => {
                return Ok(AgentLoopResult {
                    full_text: data.full_text,
                    rounds: data.rounds,
                    stop_condition: data.stop_condition,
                    input_tokens: data.call_input_tokens,
                    output_tokens: data.call_output_tokens,
                    cost_usd: data.call_cost,
                    latency_ms: data.latency_ms,
                    execution_fingerprint: data.execution_fingerprint,
                    timeline_json: data.timeline_json,
                    ctrl_rx,
                    critic_verdict: None,
                    round_evaluations: data.round_evaluations,
                    plan_completion_ratio: data.plan_completion_ratio,
                    avg_plan_drift: 0.0,
                    oscillation_penalty: 0.0,
                    last_model_used: data.last_model_used,
                    plugin_cost_snapshot: plugin_registry
                        .as_ref()
                        .and_then(|arc_pr| arc_pr.lock().ok().map(|pr| pr.cost_snapshot()))
                        .unwrap_or_default(),
                    tools_executed: state.tools_executed,
                    evidence_verified: !state.evidence.bundle.evidence_gate_fires(),
                    content_read_attempts: state.evidence.bundle.content_read_attempts,
                    last_provider_used: None,
                    blocked_tools: state.evidence.blocked_tools,
                    failed_sub_agent_steps: state.failed_sub_agent_steps,
                    critic_unavailable: false,
                    tool_trust_failures: state.tool_trust.failure_records(),
                    sla_budget: state.sla_budget,
                    evidence_coverage: state.evidence.graph.synthesis_coverage(),
                    // Phase 2: early-return path — capture any gate classification so far.
                    synthesis_kind:    state.synthesis.last_synthesis_kind,
                    synthesis_trigger: state.synthesis.last_synthesis_trigger,
                    routing_escalation_count: state.convergence.routing_escalation_count,
                });
            }
            provider_round::ProviderRoundOutcome::ToolUse(out) => out,
        };
        let provider_round::ProviderRoundOutput {
            completed_tools,
            round_model_name,
            round_provider_name,
            round_usage,
            round_text_for_scorer,
        } = provider_out;


        // ── Post-batch phase ─────────────────────────────────────────────────────────────────
        // Tool dedup, execution, plan tracking, PostBatchSupervisor, reflexion, circuit breaker.
        let (round_tool_log, tool_failures, tool_successes) = match post_batch::run(
            &mut state,
            completed_tools,
            session,
            render_sink,
            tool_registry,
            working_dir,
            event_tx,
            trace_db,
            guardrails,
            permissions,
            &tool_exec_config,
            plugin_registry.as_ref(),
            replay_tool_executor,
            speculator,
            &mut task_bridge,
            reflector,
            planner,
            planning_config,
            request,
            &mut ctrl_rx,
            limits,
            &round_model_name,
            &round_provider_name,
            episode_id,
            round,
        ).await? {
            post_batch::PostBatchOutcome::BreakLoop => break 'agent_loop,
            post_batch::PostBatchOutcome::Continue {
                round_tool_log,
                tool_failures,
                tool_successes,
            } => {
                // Accumulate tool names across rounds for AgentLoopResult.
                state.tools_executed.extend(tool_successes.iter().cloned());
                (round_tool_log, tool_failures, tool_successes)
            }
        };

        // ── Convergence phase ──────────────────────────────────────────────────────────────────
        // ConvergenceController observe, metacognitive monitoring, ctrl_rx yield, RoundScorer,
        // signal assembly, HICON Phase 4, LoopGuard match arms, self-correction injection,
        // speculation cache clear, auto-save.
        dispatch!('agent_loop, state, convergence_phase::run(
            &mut state,
            session,
            render_sink,
            planner,
            planning_config,
            request,
            &mut ctrl_rx,
            speculator,
            trace_db,
            round,
            &round_tool_log,
            &tool_failures,
            &tool_successes,
            &round_usage,
            &round_text_for_scorer,
        ).await?);

        // Phase 1: Save LoopState checkpoint (Continue path — full round completed).
        // Fire-and-forget: errors are logged but never propagate to the loop.
        {
            let cp_data = checkpoint::LoopCheckpointData::snapshot(&state, round);
            let messages_json = serde_json::to_string(&state.messages).unwrap_or_default();
            let usage_json = serde_json::json!({
                "input_tokens": session.total_usage.input_tokens,
                "output_tokens": session.total_usage.output_tokens,
            })
            .to_string();
            let fingerprint = compute_fingerprint(&state.messages);
            checkpoint::save_checkpoint_nonblocking(
                cp_data,
                trace_db,
                &messages_json,
                &usage_json,
                &fingerprint,
                state.trace_step_index,
            );

            loop_events::emit(
                &state.session_id.to_string(),
                round as u32,
                loop_events::LoopEvent::CheckpointSaved { round },
                trace_db,
            );
        }
    }

    // P6: Mark the last synthesis step as Completed so plan JSON shows 100% completion.
    // The synthesis step is tool-less (tool_name == None) and is never matched by
    // record_tool_results (which matches by tool_name). Without this, the plan tracker
    // reports the last step as Pending even after the coordinator finishes synthesis.
    if let Some(ref mut tracker) = state.execution_tracker {
        let plan = tracker.plan();
        let last_idx = plan.steps.len().saturating_sub(1);
        if plan.steps.get(last_idx).map_or(false, |s| s.tool_name.is_none()) {
            tracker.mark_synthesis_complete(last_idx, state.rounds);
        }
    }

    // P5.1: Session retrospective — post-session diagnostic analysis.
    {
        let session_profile = super::domain::session_retrospective::analyze(
            &state.convergence.decision_trace,
            &state.convergence.metrics_collector,
            &state.convergence.adaptation_bounds,
            &state.convergence.invariant_checker,
            &state.policy,
        );
        tracing::info!(
            convergence_efficiency = %format!("{:.2}", session_profile.convergence_efficiency),
            structural_instability = %format!("{:.2}", session_profile.structural_instability_score),
            dominant_failure = ?session_profile.dominant_failure_mode,
            inferred_class = %session_profile.inferred_problem_class.label(),
            evidence_trajectory = ?session_profile.evidence_trajectory,
            wasted_rounds = session_profile.wasted_rounds,
            peak_utility = %format!("{:.3}", session_profile.peak_utility),
            final_utility = %format!("{:.3}", session_profile.final_utility),
            "Phase5 SessionRetrospective: post-session analysis"
        );

        // GAP-2 fix: Persist SessionRetrospective to JSONL file for post-session surfacing.
        // Written to {working_dir}/.halcon/retrospectives/sessions.jsonl (append).
        // Fire-and-forget — never blocks the response path.
        let retro_row = serde_json::json!({
            "timestamp_utc": Utc::now().to_rfc3339(),
            "convergence_efficiency": session_profile.convergence_efficiency,
            "structural_instability_score": session_profile.structural_instability_score,
            "dominant_failure_mode": session_profile.dominant_failure_mode.map(|m| m.label()),
            "inferred_problem_class": session_profile.inferred_problem_class.label(),
            "adaptation_utilization": session_profile.adaptation_utilization,
            "evidence_trajectory": session_profile.evidence_trajectory.label(),
            "decision_density": session_profile.decision_density,
            "wasted_rounds": session_profile.wasted_rounds,
            "peak_utility": session_profile.peak_utility,
            "final_utility": session_profile.final_utility,
        });
        let retro_dir = std::path::Path::new(working_dir).join(".halcon").join("retrospectives");
        let retro_row_str = format!("{}\n", retro_row);
        // P0-3 (STAT-RACE-001): propagate EXECUTION_CTX into spawned task so
        // any DomainEvent::new() calls inside carry the correct session_id.
        let retro_exec_ctx = EXECUTION_CTX.try_with(|c| c.clone()).ok();
        tokio::spawn(async move {
            let do_write = async move {
                if tokio::fs::create_dir_all(&retro_dir).await.is_ok() {
                    let retro_path = retro_dir.join("sessions.jsonl");
                    if let Ok(mut f) = tokio::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&retro_path)
                        .await
                    {
                        use tokio::io::AsyncWriteExt;
                        let _ = f.write_all(retro_row_str.as_bytes()).await;
                    }
                }
            };
            match retro_exec_ctx {
                Some(ctx) => EXECUTION_CTX.scope(ctx, do_write).await,
                None => {
                    // R-04: EXECUTION_CTX unavailable at spawn time — session_id will be None
                    // in any DomainEvent emitted by this task. This should not happen in
                    // normal operation (the main loop runs inside EXECUTION_CTX.scope).
                    tracing::warn!(
                        target: "halcon::agent",
                        task = "retrospective_write",
                        "EXECUTION_CTX not available — retrospective events will have no session_id"
                    );
                    do_write.await;
                }
            }
        });
    }

    // Capture state fields needed for auto-memory before consuming state.
    let auto_memory_working_dir = working_dir.to_string();
    let auto_memory_user_msg = state.user_msg.clone();
    let auto_memory_policy = state.policy.clone();

    let result = result_assembly::build(
        state,
        render_sink,
        event_tx,
        limits,
        provider,
        critic_provider,
        critic_model,
        request,
        ctrl_rx,
        plugin_registry,
        permissions,
    ).await;

    // Feature 3 (Frontier Roadmap 2026): Auto-memory background write.
    // Fire-and-forget: never blocks the response, never surfaces errors to the user.
    if auto_memory_policy.enable_auto_memory {
        if let Ok(ref loop_result) = result {
            let result_clone = crate::repl::auto_memory::MemoryResultSnapshot {
                rounds: loop_result.rounds,
                stop_condition: loop_result.stop_condition,
                critic_verdict: loop_result.critic_verdict.clone(),
                tool_trust_failures: loop_result.tool_trust_failures.clone(),
                tools_executed: loop_result.tools_executed.clone(),
            };
            let repo_name = std::path::Path::new(&auto_memory_working_dir)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            // P0-3 (STAT-RACE-001): propagate EXECUTION_CTX into spawned task.
            let mem_exec_ctx = EXECUTION_CTX.try_with(|c| c.clone()).ok();
            tokio::spawn(async move {
                let do_record = async move {
                    crate::repl::auto_memory::record_session_snapshot(
                        &result_clone,
                        &auto_memory_user_msg,
                        &auto_memory_working_dir,
                        &repo_name,
                        &auto_memory_policy,
                    );
                };
                match mem_exec_ctx {
                    Some(ctx) => EXECUTION_CTX.scope(ctx, do_record).await,
                    None => {
                        tracing::warn!(
                            target: "halcon::agent",
                            task = "auto_memory_write",
                            "EXECUTION_CTX not available — auto-memory events will have no session_id"
                        );
                        do_record.await;
                    }
                }
            });
        }
    }

    // Feature 2 (Frontier Roadmap 2026): Stop lifecycle hook.
    // Fires after the agent loop terminates (any stop reason: EndTurn, convergence, max-rounds).
    // Best-effort: outcome is logged but does not change the result.
    if let Some(ref hook_runner) = tool_exec_config.hook_runner {
        if hook_runner.has_hooks_for(super::hooks::HookEventName::Stop) {
            let session_id_s = tool_exec_config.session_id_str.clone();
            let runner_clone = hook_runner.clone();
            // P0-3 (STAT-RACE-001): propagate EXECUTION_CTX into spawned task.
            let hook_exec_ctx = EXECUTION_CTX.try_with(|c| c.clone()).ok();
            tokio::spawn(async move {
                let do_hook = async move {
                    let event = super::hooks::lifecycle_event(
                        super::hooks::HookEventName::Stop,
                        &session_id_s,
                    );
                    if let super::hooks::HookOutcome::Deny(reason) = runner_clone.fire(&event).await {
                        tracing::warn!(%reason, "Stop hook denied (ignored — loop already ended)");
                    }
                };
                match hook_exec_ctx {
                    Some(ctx) => EXECUTION_CTX.scope(ctx, do_hook).await,
                    None => {
                        tracing::warn!(
                            target: "halcon::agent",
                            task = "stop_hook",
                            "EXECUTION_CTX not available — Stop hook events will have no session_id"
                        );
                        do_hook.await;
                    }
                }
            });
        }
    }

    result
}
#[cfg(test)]
mod tests;
