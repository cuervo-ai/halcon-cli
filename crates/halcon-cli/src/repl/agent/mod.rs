mod budget_guards;
mod convergence_phase;
mod loop_state;
mod plan_formatter;
mod planning_policy;
mod post_batch;
mod provider_client;
mod provider_round;
mod result_assembly;
mod round_setup;

use loop_state::{LoopState, ToolDecisionSignal};

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

use halcon_core::traits::{ExecutionPlan, ModelProvider, Planner, StepOutcome};
use halcon_core::types::{
    AgentLimits, ChatMessage, ContentBlock, DomainEvent, EventPayload, MessageContent, ModelChunk,
    ModelRequest, OrchestratorConfig, Phase14Context, PlanningConfig, Role, RoutingConfig, Session,
    StopReason, TaskContext, TokenUsage,
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
    pub plugin_registry: Option<std::sync::Arc<std::sync::Mutex<super::plugin_registry::PluginRegistry>>>,
    /// Whether this agent is running as a sub-agent under an orchestrator.
    ///
    /// When `true`, the agent loop uses `ConvergenceController::new_for_sub_agent()` with
    /// tighter limits (max_rounds=6, low goal_coverage_threshold, multilingual keywords)
    /// instead of the intent-profile-derived controller.  Set to `false` for all top-level
    /// agents (main REPL loop, retry loop, replay runner).
    pub is_sub_agent: bool,
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
    } = ctx;

    let silent = render_sink.is_silent();
    // Phase L: token attribution accumulators (filled before LoopState is created).
    let mut pre_loop_tokens_subagents: u64 = 0;

    let tool_exec_config = executor::ToolExecutionConfig {
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

    // P4 FIX: Track real FSM state so every agent_state_transition call uses
    // the actual from_state rather than a hardcoded value.
    // Without this, the final transition at loop exit always emits "executing"
    // as from_state even if the last state was "reflecting", "planning", etc.
    let mut current_fsm_state = "idle";

    // Phase E5: Emit agent state transition: Idle → Planning/Executing.
    if !silent {
        render_sink.agent_state_transition("idle", "executing", "agent loop started");
        current_fsm_state = "executing";
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
    let model_context_window: u32 = provider
        .supported_models()
        .iter()
        .find(|m| m.id == request.model)
        .map(|m| m.context_window)
        .unwrap_or(64_000); // Conservative fallback — 64K covers most modern providers.
    // 20% output reservation: prevents the model from running out of output budget
    // when input fills the entire context window.
    // mut: Dynamic Budget Reconciliation may shrink this on provider fallback.
    let mut pipeline_budget = {
        let input_fraction = (model_context_window as f64 * 0.80) as u32;
        if limits.max_total_tokens > 0 {
            input_fraction.min(limits.max_total_tokens)
        } else {
            input_fraction
        }
    };
    tracing::debug!(
        model = %request.model,
        context_window = model_context_window,
        pipeline_budget,
        "Context pipeline budget derived from model context window"
    );
    let mut context_pipeline = halcon_context::ContextPipeline::new(
        &halcon_context::ContextPipelineConfig {
            max_context_tokens: pipeline_budget,
            ..Default::default()
        },
    );
    if let Some(ref sys) = request.system {
        context_pipeline.initialize(sys, std::path::Path::new(working_dir));
    }
    // Load L4 archive from disk (cross-session knowledge persistence).
    let l4_archive_path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("halcon")
        .join("l4_archive.bin");
    context_pipeline.load_l4_archive(&l4_archive_path);

    for msg in &messages {
        context_pipeline.add_message(msg.clone());
    }

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
                render_sink.agent_state_transition(current_fsm_state, "planning", "generating plan");
                current_fsm_state = "planning";
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
                render_sink.agent_state_transition(current_fsm_state, "executing", "plan generated");
                current_fsm_state = "executing";
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

                // Send plan to UI (TUI panel + classic rendering)
                if !silent {
                    render_sink.plan_progress(&plan.goal, &plan.steps, 0);
                }

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
        super::early_convergence::ConvergenceDetector::with_context_window(pipeline_budget as u64);

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
        // Phase 42: record tool selection metrics.
        if let Some(metrics) = context_metrics {
            metrics.record_tool_selection(all_tools.len(), selected.len());
        }
        // Sprint 0-C: Cached preflight schema validation.
        // Validates each tool's input_schema exactly once per process lifetime.
        // Invalid schemas are logged and excluded — prevents confusing API-level errors.
        super::schema_validator::preflight_validate(selected)
    };
    // System prompt may update mid-session if instruction files (HALCON.md) change on disk.
    // Track instruction content separately for surgical replacement in the full system prompt.
    let mut cached_system = request.system.clone();
    let mut cached_instructions =
        halcon_context::load_instructions(std::path::Path::new(working_dir));

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
        // Emit initial plan progress with timing.
        let (_, _, elapsed) = tracker.progress();
        render_sink.plan_progress_with_timing(
            &plan.goal,
            &plan.steps,
            tracker.current_step(),
            tracker.tracked_steps(),
            elapsed,
        );
    }

    // Phase 37: Attempt delegation of eligible plan steps to sub-agents.
    if let Some(ref mut tracker) = execution_tracker {
        let delegation_router = super::delegation::DelegationRouter::new(orchestrator_config.enabled)
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
            for (i, task) in tasks.iter().enumerate() {
                render_sink.sub_agent_spawned(
                    i + 1,
                    task_count,
                    &task.instruction.chars().take(60).collect::<String>(),
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
                              reply_tx: tokio::sync::mpsc::UnboundedSender<
                            halcon_core::types::PermissionDecision,
                        >| {
                            let _ = ui_tx.send(crate::tui::events::UiEvent::PermissionAwaiting {
                                tool: tool.to_string(),
                                args: args.clone(),
                                risk_level: risk.to_string(),
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
                // BUG FIX (tool-only sub-agents): Previously filtered `r.output_text.is_empty()`,
                // which excluded sub-agents that ONLY executed a tool (e.g. file_write) and
                // produced no text output (common for file-creation tasks). The coordinator then
                // had NO context that the task was completed, so it tried to re-execute the tool
                // itself (visible as ToolMarker(file_write) at coordinator step 2). Fix: include
                // tool-only sub-agents with a synthetic completion message derived from tools_used.
                let sub_outputs: Vec<String> = orch_result.sub_results.iter()
                    .filter(|r| !r.output_text.is_empty() || !r.agent_result.tools_used.is_empty())
                    .enumerate()
                    .map(|(i, r)| {
                        let status = if r.success { "success" } else { "failed" };

                        // When the sub-agent executed tools but produced no text output
                        // (e.g. file_write completed silently), synthesize a completion message
                        // so the coordinator knows the task was done. Without this, the coordinator
                        // would attempt to redo the same work, causing duplicate tool calls.
                        let effective_output = if r.output_text.is_empty() && !r.agent_result.tools_used.is_empty() {
                            format!(
                                "Task completed via tool execution: {}. The operation was performed successfully.",
                                r.agent_result.tools_used.join(", ")
                            )
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
                                if effective_output.len() > 600 {
                                    format!("{}…", &effective_output[..600])
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
                        format!("**Sub-agent {} ({}):**\n{}", i + 1, status, text)
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

                if !sub_outputs.is_empty() {
                    render_sink.loop_guard_action(
                        "sub_agent_results",
                        &format!("{} sub-agent outputs collected — injecting into coordinator context", sub_outputs.len()),
                    );

                    // FIX: When destructive tools (file_write, bash) were already executed by
                    // sub-agents, inject a strong directive so the coordinator does NOT re-execute
                    // them. Without this, deepseek-chat hallucinates a second file_write call
                    // containing the full file content (~6K tokens, ~131s) even after the
                    // sub-agent already wrote the file. This was the #1 source of wasted time
                    // (176s = 51% of total session duration in the Minecraft benchmark).
                    let anti_redo_note = if delegated_ok_tools.iter().any(|t| {
                        matches!(t.as_str(), "file_write" | "bash" | "shell" | "patch_apply")
                    }) {
                        format!(
                            "\n⚠️  CRITICAL: The following tools were already executed by sub-agents \
                             and must NOT be called again: [{}]. \
                             Your ONLY task now is to synthesize the results and confirm to the user \
                             what was created. Do NOT regenerate or re-write any files.\n",
                            delegated_ok_tools
                                .iter()
                                .filter(|t| matches!(t.as_str(), "file_write" | "bash" | "shell" | "patch_apply"))
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    } else {
                        String::new()
                    };

                    let results_context = format!(
                        "[Sub-Agent Results — please synthesize these into your final response]{anti_redo_note}\n\n{}\n",
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

                // FIX: Remove delegation-completed tools from coordinator's cached_tools.
                // Prevents the coordinator from calling a tool (e.g. file_write) that a
                // sub-agent already executed. In the Minecraft benchmark, deepseek-chat
                // ignored the "don't redo" injection and called file_write a second time
                // (131s wasted API + 45s permission timeout = 176s = 51% of total time).
                // By removing the tool from the tool list, the model physically cannot
                // call it — eliminating the hallucination at the protocol level.
                if !delegated_ok_tools.is_empty() {
                    let before = cached_tools.len();
                    cached_tools.retain(|t| !delegated_ok_tools.contains(&t.name));
                    let removed = before - cached_tools.len();
                    if removed > 0 {
                        tracing::info!(
                            removed_tools = ?delegated_ok_tools,
                            removed_count = removed,
                            remaining_tools = cached_tools.len(),
                            "Post-delegation: removed completed tools from coordinator tool list"
                        );
                        if !silent {
                            render_sink.info(&format!(
                                "[post-delegation] removed {} completed tool(s) from coordinator list: [{}]",
                                removed,
                                delegated_ok_tools
                                    .iter()
                                    .filter(|n| !cached_tools.iter().any(|ct| &ct.name == *n))
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(", ")
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
                    })
                    .collect();
                tracker.record_delegation_results(&failure_results, rounds);
            }
        }
    }

    // AUTONOMY FIX: Inject autonomous agent directive to promote proactive behavior.
    // This instructs the model to plan, execute completely, and solve problems autonomously.
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
        - Your goal is to DELIVER WORKING SOLUTIONS, not provide guidance.\n";

    // Phase 33: inject tool usage policy into the system prompt.
    // Instructs the model to converge: prefer fewer tool calls, don't repeat,
    // respond directly once enough information is gathered.
    const TOOL_USAGE_POLICY: &str = "\n\n## Tool Usage Policy\n\
        - Only call tools when you need NEW information you don't already have.\n\
        - After gathering data with tools, respond directly to the user.\n\
        - Never call the same tool twice with the same or very similar arguments.\n\
        - Prefer fewer tool calls. 1-3 tool rounds should suffice for most tasks.\n\
        - When you have enough information to answer, STOP calling tools and respond.\n\
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
    let mut loop_guard = ToolLoopGuard::new();

    // Sprint 1: CapabilityOrchestrationLayer — centralises 5 STRIP points.
    // Replaces: conversational directive injection, force_no_tools ModelRequest
    // assignment, Ollama emulation strip, flag reset, and model capability check.
    let capability_orchestrator =
        super::capability_orchestrator::CapabilityOrchestrationLayer::with_default_rules();

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
    let mut round_scorer = super::round_scorer::RoundScorer::new(goal_text);
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
    let coherence_checker = super::plan_coherence::PlanCoherenceChecker::new(goal_text);
    let mut cumulative_drift_score = 0.0f32;
    let mut drift_replan_count = 0usize;

    // P2 FIX: Replan convergence budget.
    // Prevents infinite replan cascade: if ReplanRequired fires repeatedly and each
    // new plan immediately stalls again, we cap total replan attempts and escalate
    // to forced synthesis so the agent always terminates.
    let mut replan_attempts: u32 = 0;
    const MAX_REPLAN_ATTEMPTS: u32 = 2;

    // HICON Phase 4: Agent self-corrector for adaptive strategy adjustment.
    let mut self_corrector = super::self_corrector::AgentSelfCorrector::new();

    // HICON Phase 5: ARIMA resource predictor for proactive budget management.
    let mut resource_predictor = super::arima_predictor::ResourcePredictor::new();

    // HICON Phase 6: Metacognitive loop for system-wide coherence monitoring.
    let mut metacognitive_loop = super::metacognitive_loop::MetacognitiveLoop::new();

    // SOTA 2026: ConvergenceController — adaptive loop termination driven by IntentProfile.
    // Sub-agents use new_for_sub_agent() with tighter limits + multilingual keyword extraction
    // so Spanish-language instructions don't cause false-negative coverage misses.
    // Top-level agents use new() with the IntentProfile-derived budget.
    let mut conv_ctrl = if ctx.is_sub_agent {
        super::convergence_controller::ConvergenceController::new_for_sub_agent(&user_msg)
    } else {
        // Reuses the IntentProfile already computed above (task_analysis) — avoids double scoring.
        // IntentProfile.suggested_max_rounds() provides the initial convergence window; then
        // cap_max_rounds() aligns it with the reasoning engine's adjusted limit (D3 fix) so
        // ConvergenceController and the outer loop share a single source of truth for max rounds.
        super::convergence_controller::ConvergenceController::new(&task_analysis, &user_msg)
    };
    conv_ctrl.cap_max_rounds(ctx.limits.max_rounds);

    // Phase L fix B6+B3: Enforce budget invariant — max_rounds must cover all plan steps.
    // Called AFTER plan creation (active_plan is set) and AFTER conv_ctrl is created.
    // Uses a local mutable copy so we can expand the budget without mutating AgentContext.
    let mut effective_max_rounds = ctx.limits.max_rounds;
    if !is_sub_agent {
        if let Some(ref plan) = active_plan {
            // INVARIANT K5-1: max_rounds ≥ plan.total_steps + critic_retries + 1 (synthesis)
            let max_critic_retries: u32 = 1; // ReasoningConfig::default max_retries
            let required = plan.steps.len() + max_critic_retries as usize + 1;
            if effective_max_rounds < required {
                tracing::warn!(
                    effective_max_rounds,
                    required,
                    plan_steps = plan.steps.len(),
                    max_critic_retries,
                    "Phase L K5-1: expanding effective_max_rounds to satisfy budget invariant"
                );
                effective_max_rounds = required;
                conv_ctrl.cap_max_rounds(effective_max_rounds);
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

    // Flag set when cross-type oscillation detection forces a synthesis break inside the loop.
    let mut forced_synthesis_detected = false;
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
        call_input_tokens,
        call_output_tokens,
        call_cost,
        session_id,
        trace_step_index,
        pipeline_budget,
        provider_context_window: model_context_window,
        active_plan,
        execution_tracker,
        convergence_detector,
        macro_plan_view,
        last_convergence_ratio,
        compaction_model,
        cached_tools,
        cached_system,
        cached_instructions,
        is_conversational_intent,
        reflection_injector,
        last_reflection_id,
        tbac_pushed,
        failure_tracker,
        fallback_adapted_model,
        loop_guard,
        capability_orchestrator,
        round_scorer,
        round_evaluations,
        adaptive_policy,
        coherence_checker,
        cumulative_drift_score,
        drift_replan_count,
        replan_attempts,
        self_corrector,
        resource_predictor,
        metacognitive_loop,
        conv_ctrl,
        forced_synthesis_detected,
        convergence_directive_injected,
        environment_error_halt,
        auto_pause,
        ctrl_cancelled,
        model_downgrade_advisory_active: false,
        forced_routing_bias: None,
        tool_decision: ToolDecisionSignal::Allow,
        current_fsm_state,
        last_round_model_name,
        next_round_restarts: 0,
        loop_start,
        tool_timeout,
        silent,
        user_msg: user_msg.clone(),
        goal_text: goal_text.to_string(),
        l4_archive_path: l4_archive_path.clone(),
        strategy_context: strategy_context.clone(),
        tools_executed: Vec::new(),
        tokens_planning: 0,
        tokens_subagents: pre_loop_tokens_subagents,
        tokens_critic: 0,
        call_input_tokens_prev_round: 0,
    };

    'agent_loop: for round in 0..effective_max_rounds {
        // Round separator is emitted after model selection (see below) so we can show provider info.
        // Reset per-round coordination flags.
        state.convergence_directive_injected = false;

        let _round_span = tracing::info_span!(
            "gen_ai.agent.round",
            "gen_ai.request.model" = %request.model,
            "gen_ai.operation.name" = "agent_round",
            round,
        )
        .entered();
        let round_start = Instant::now();
        let mut round_usage = TokenUsage::default();

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
    }

    result_assembly::build(
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
    ).await
}
#[cfg(test)]
mod tests;
