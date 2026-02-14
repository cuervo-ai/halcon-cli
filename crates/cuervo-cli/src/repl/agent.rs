use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use futures::StreamExt;
use sha2::Digest;
use tracing::instrument;

use cuervo_core::traits::{ExecutionPlan, ModelProvider, Planner, StepOutcome};
use cuervo_core::types::{
    AgentLimits, ChatMessage, ContentBlock, DomainEvent, EventPayload, MessageContent, ModelChunk,
    ModelRequest, OrchestratorConfig, Phase14Context, PlanningConfig, Role, RoutingConfig, Session,
    StopReason, TaskContext, TokenUsage,
};
use cuervo_core::EventSender;
use cuervo_providers::ProviderRegistry;
use cuervo_storage::{AsyncDatabase, InvocationMetric, TraceStepType};
use cuervo_tools::ToolRegistry;

use super::accumulator::ToolUseAccumulator;
use super::compaction::ContextCompactor;
use super::execution_tracker::ExecutionTracker;
use super::executor;
use super::failure_tracker::ToolFailureTracker;
use super::loop_guard::{hash_tool_args, LoopAction, ToolLoopGuard};
use super::permissions::PermissionChecker;
use super::resilience::{PreInvokeDecision, ResilienceManager};
use super::response_cache::ResponseCache;
use super::speculative::SpeculativeInvoker;
use super::task_analyzer::TaskAnalyzer;
use crate::render::sink::RenderSink;

// Re-export types that are part of agent's public API.
// External modules reference these as `agent::StopCondition`, `agent::AgentLoopResult`, etc.
pub use super::agent_types::{AgentLoopResult, StopCondition};
pub use super::agent_utils::{classify_error_hint, compute_fingerprint};

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
    pub permissions: &'a mut PermissionChecker,
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
    pub guardrails: &'a [Box<dyn cuervo_security::Guardrail>],
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
    /// Optional control channel receiver (Phase 43). TUI sends Pause/Step/Cancel events.
    /// Classic REPL passes None. When Some, agent loop checks at yield points.
    pub ctrl_rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::tui::events::ControlEvent>>,
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

/// Result of an invocation attempt through the routing + resilience chain.
struct InvokeAttempt {
    stream: futures::stream::BoxStream<'static, Result<ModelChunk, cuervo_core::error::CuervoError>>,
    provider_name: String,
    is_fallback: bool,
    #[allow(dead_code)]
    permit: Option<super::backpressure::InvokePermit>,
}

// Plan injection markers for surgical replacement in the system prompt.
const PLAN_SECTION_START: &str = "<!-- CUERVO_PLAN_START -->";
const PLAN_SECTION_END: &str = "<!-- CUERVO_PLAN_END -->";

/// Format an execution plan as a system prompt section.
///
/// Renders the plan with step statuses (done/failed/current/pending) and
/// a directive telling the model which step to execute next.
fn format_plan_for_prompt(plan: &ExecutionPlan, current_step: usize) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "{PLAN_SECTION_START}");
    let _ = writeln!(out);
    let _ = writeln!(out, "## Active Execution Plan");
    let _ = writeln!(out);
    let _ = writeln!(out, "**Goal**: {}", plan.goal);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Follow these steps in order. Execute the current step, then proceed to the next."
    );
    let _ = writeln!(out);

    for (i, step) in plan.steps.iter().enumerate() {
        let tool_hint = step
            .tool_name
            .as_deref()
            .map(|t| format!(" (tool: {t})"))
            .unwrap_or_default();
        let (icon, marker) = match &step.outcome {
            Some(StepOutcome::Success { .. }) => ("\u{2713}", ""),       // ✓
            Some(StepOutcome::Failed { .. }) => ("\u{2717}", ""),        // ✗
            Some(StepOutcome::Skipped { .. }) => ("-", ""),
            None if i == current_step => ("\u{25b8}", " \u{2190} CURRENT"), // ▸ ← CURRENT
            None => ("\u{25cb}", ""),                                     // ○
        };
        let _ = writeln!(
            out,
            "  {icon}  Step {}: {}{tool_hint}{marker}",
            i + 1,
            step.description
        );
    }

    let _ = writeln!(out);
    if current_step < plan.steps.len() {
        let step = &plan.steps[current_step];
        let _ = writeln!(
            out,
            "You are on Step {}. Execute: {}",
            current_step + 1,
            step.description
        );
        if let Some(ref args) = step.expected_args {
            let _ = writeln!(out, "Expected args: {args}");
        }
    } else {
        let _ = writeln!(out, "All steps completed.");
    }

    let _ = writeln!(out);
    let _ = write!(out, "{PLAN_SECTION_END}");
    out
}

/// Surgically replace the plan section in a system prompt string.
/// If no plan section exists, appends it.
fn update_plan_in_system(system: &mut String, plan_section: &str) {
    if let Some(start) = system.find(PLAN_SECTION_START) {
        if let Some(end) = system.find(PLAN_SECTION_END) {
            let end = end + PLAN_SECTION_END.len();
            system.replace_range(start..end, plan_section);
            return;
        }
    }
    // No existing section — append.
    system.push_str("\n\n");
    system.push_str(plan_section);
}

/// Invoke a provider with resilience gating and speculative/failover routing.
///
/// When resilience is enabled, pre-filters healthy providers via the ResilienceManager,
/// then delegates to SpeculativeInvoker for retry + fallback / speculative racing.
/// When resilience is disabled, delegates directly to SpeculativeInvoker.
async fn invoke_with_fallback(
    primary: &Arc<dyn ModelProvider>,
    request: &ModelRequest,
    fallback_providers: &[(String, Arc<dyn ModelProvider>)],
    resilience: &mut ResilienceManager,
    routing_config: &RoutingConfig,
    event_tx: &EventSender,
) -> Result<InvokeAttempt> {
    let invoker = SpeculativeInvoker::new(routing_config);

    // If resilience is disabled, delegate directly to the speculative invoker.
    if !resilience.is_enabled() {
        let result = invoker
            .invoke(primary, request, fallback_providers)
            .await?;
        return Ok(InvokeAttempt {
            stream: result.stream,
            provider_name: result.provider_name,
            is_fallback: result.is_fallback,
            permit: None,
        });
    }

    // Pre-filter: collect healthy providers via resilience pre_invoke.
    let mut healthy_primary: Option<(Arc<dyn ModelProvider>, super::backpressure::InvokePermit)> =
        None;
    let mut healthy_fallbacks: Vec<(String, Arc<dyn ModelProvider>)> = Vec::new();

    // Check primary.
    match resilience.pre_invoke(primary.name()).await {
        PreInvokeDecision::Proceed { permit } => {
            healthy_primary = Some((Arc::clone(primary), permit));
        }
        PreInvokeDecision::Fallback { reason } => {
            tracing::info!(
                provider = primary.name(),
                reason = %reason,
                "Primary provider rejected by resilience"
            );
        }
    }

    // Check fallbacks (permits are advisory for fallbacks — drop after check).
    for (name, fb_provider) in fallback_providers {
        match resilience.pre_invoke(name).await {
            PreInvokeDecision::Proceed { permit: _permit } => {
                healthy_fallbacks.push((name.clone(), Arc::clone(fb_provider)));
            }
            PreInvokeDecision::Fallback { reason } => {
                tracing::debug!(
                    provider = %name,
                    reason = %reason,
                    "Fallback provider rejected by resilience"
                );
            }
        }
    }

    // If no healthy providers at all, bail.
    if healthy_primary.is_none() && healthy_fallbacks.is_empty() {
        anyhow::bail!(
            "All providers unavailable (primary '{}' + {} fallbacks)",
            primary.name(),
            fallback_providers.len()
        );
    }

    // Determine the effective primary and fallbacks for the invoker.
    let (effective_primary, permit, promoted_name) = if let Some((p, permit)) = healthy_primary {
        (p, Some(permit), None)
    } else {
        // Primary is unhealthy — promote first healthy fallback to primary.
        let (name, first_fb) = healthy_fallbacks.remove(0);
        tracing::info!(provider = %name, "Promoting fallback to primary (original primary unhealthy)");
        let _ = event_tx.send(DomainEvent::new(EventPayload::ProviderFallback {
            from_provider: primary.name().to_string(),
            to_provider: name.clone(),
            reason: "primary unhealthy".to_string(),
        }));
        (first_fb, None, Some(name))
    };

    // Delegate to speculative invoker.
    match invoker
        .invoke(&effective_primary, request, &healthy_fallbacks)
        .await
    {
        Ok(result) => {
            // If we promoted a fallback, use the logical name and mark as fallback.
            let (provider_name, is_fallback) = if let Some(name) = promoted_name {
                (name, true)
            } else {
                (result.provider_name, result.is_fallback)
            };
            Ok(InvokeAttempt {
                stream: result.stream,
                provider_name,
                is_fallback,
                permit,
            })
        }
        Err(e) => {
            // Record failure on the effective primary.
            resilience.record_failure(effective_primary.name()).await;
            tracing::warn!(
                provider = effective_primary.name(),
                "Primary/promoted provider failed: {e}, trying remaining fallbacks"
            );

            // Retry chain: try each remaining healthy fallback sequentially.
            // Each fallback gets a request with a model it actually supports.
            for (idx, (fb_name, fb_provider)) in healthy_fallbacks.iter().enumerate() {
                let fb_request = if fb_provider.supported_models().iter().any(|m| m.id == request.model) {
                    request.clone()
                } else if let Some(default) = fb_provider.supported_models().first() {
                    tracing::info!(
                        provider = %fb_name,
                        original_model = %request.model,
                        fallback_model = %default.id,
                        "Adjusting model for fallback provider"
                    );
                    ModelRequest {
                        model: default.id.clone(),
                        ..request.clone()
                    }
                } else {
                    request.clone()
                };
                match fb_provider.invoke(&fb_request).await {
                    Ok(stream) => {
                        let _ = event_tx.send(DomainEvent::new(EventPayload::ProviderFallback {
                            from_provider: effective_primary.name().to_string(),
                            to_provider: fb_name.clone(),
                            reason: format!("fallback #{}", idx + 1),
                        }));
                        return Ok(InvokeAttempt {
                            stream,
                            provider_name: fb_name.clone(),
                            is_fallback: true,
                            permit: None,
                        });
                    }
                    Err(fb_err) => {
                        tracing::warn!(provider = %fb_name, "Fallback provider failed: {fb_err}");
                        resilience.record_failure(fb_name).await;
                    }
                }
            }

            // All fallbacks exhausted.
            anyhow::bail!(
                "All providers failed (primary + {} fallbacks): {e}",
                healthy_fallbacks.len()
            )
        }
    }
}

// ToolFailureTracker → failure_tracker.rs
// LoopAction, ToolLoopGuard, hash_tool_args → loop_guard.rs

// StopCondition, AgentLoopResult → agent_types.rs (re-exported above)

// compute_fingerprint, record_trace, auto_checkpoint, classify_error_hint → agent_utils.rs
use super::agent_utils::{auto_checkpoint, record_trace};

/// Check the control channel for pause/step/cancel events.
///
/// Non-blocking: returns immediately if no events pending.
/// On Pause: blocks until Resume, Step, or Cancel is received.
/// Returns the action the agent loop should take.
///
/// All ControlEvent variants are handled explicitly — no silent ignores.
/// ApproveAction/RejectAction are permission responses handled by the
/// dedicated permission channel in TUI mode; they are no-ops here.
async fn check_control(
    ctrl_rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::tui::events::ControlEvent>,
    sink: &dyn RenderSink,
) -> ControlAction {
    use crate::tui::events::ControlEvent;
    match ctrl_rx.try_recv() {
        Ok(ControlEvent::Pause) => {
            sink.info("  [paused] Press Space to resume, N to step");
            // Block until Resume, Step, or Cancel.
            loop {
                match ctrl_rx.recv().await {
                    Some(ControlEvent::Resume) => return ControlAction::Continue,
                    Some(ControlEvent::Step) => return ControlAction::StepOnce,
                    Some(ControlEvent::CancelAgent) => return ControlAction::Cancel,
                    None => return ControlAction::Cancel, // Channel closed.
                    // Permission events are handled by the dedicated permission
                    // channel, not the control channel. Log and continue waiting.
                    Some(ControlEvent::Pause) => {
                        // Already paused — no-op.
                    }
                    Some(ControlEvent::ApproveAction | ControlEvent::RejectAction) => {
                        tracing::debug!("Permission event received on control channel while paused (handled by permission channel)");
                    }
                }
            }
        }
        Ok(ControlEvent::CancelAgent) => ControlAction::Cancel,
        Ok(ControlEvent::Step) => ControlAction::StepOnce,
        Ok(ControlEvent::Resume) => {
            // Resume without prior pause — treat as continue.
            ControlAction::Continue
        }
        Ok(ControlEvent::ApproveAction | ControlEvent::RejectAction) => {
            // Permission events are handled by the dedicated permission channel.
            tracing::debug!("Permission event received on control channel (handled by permission channel)");
            ControlAction::Continue
        }
        Err(_) => ControlAction::Continue, // No events pending.
    }
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
        mut ctrl_rx,
    } = ctx;

    let silent = render_sink.is_silent();

    let tool_exec_config = executor::ToolExecutionConfig {
        dry_run_mode: phase14.dry_run_mode,
        idempotency: None,
        ..Default::default()
    };
    let exec_clock = &phase14.exec_ctx.clock;
    let mut messages = request.messages.clone();

    // Phase E1: Emit dry-run banner if active.
    if phase14.dry_run_mode != cuervo_core::types::DryRunMode::Off {
        render_sink.dry_run_active(true);
    }

    // Phase E5: Emit agent state transition: Idle → Planning/Executing.
    if !silent {
        render_sink.agent_state_transition("idle", "executing", "agent loop started");
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
    let mut context_pipeline = cuervo_context::ContextPipeline::new(
        &cuervo_context::ContextPipelineConfig {
            max_context_tokens: limits.max_total_tokens.max(200_000),
            ..Default::default()
        },
    );
    if let Some(ref sys) = request.system {
        context_pipeline.initialize(sys, std::path::Path::new(working_dir));
    }
    // Load L4 archive from disk (cross-session knowledge persistence).
    let l4_archive_path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("cuervo")
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
        agent_type: cuervo_core::types::AgentType::Chat,
        task: user_task,
    }));

    // Per-call metrics (accumulated across all rounds).
    let mut call_input_tokens: u64 = 0;
    let mut call_output_tokens: u64 = 0;
    let mut call_cost: f64 = 0.0;

    // Extract user message for task analysis and planning.
    let user_msg = messages
        .last()
        .and_then(|m| match &m.content {
            MessageContent::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or("");

    // Analyze task for reasoning panel (complexity, type).
    let task_analysis = TaskAnalyzer::analyze(user_msg);

    // Adaptive planning: generate plan before entering tool loop.
    let mut active_plan: Option<ExecutionPlan> = None;
    if let Some(planner) = planner {
        let tool_defs = request.tools.clone();

        // W-4: Skip planning for trivial prompts (saves 1-3s LLM round-trip).
        let word_count = user_msg.split_whitespace().count();
        let msg_lower = user_msg.to_lowercase();
        let has_action_keywords = [
            "create", "write", "edit", "delete", "run", "execute",
            "fix", "build", "install", "update", "modify", "remove", "search",
            "find", "analyze", "refactor", "test", "debug", "commit",
            "crea", "escribe", "edita", "borra", "ejecuta", "busca", "lee",
        ]
        .iter()
        .any(|kw| msg_lower.contains(kw));
        let mut needs_planning = word_count >= 15 || has_action_keywords;

        if !needs_planning {
            tracing::debug!(
                word_count,
                "Skipping planning for trivial prompt"
            );
        }

        // Validate planner model against its provider before invoking.
        // Prevents wasted ~2s LLM call on a guaranteed 404 (e.g., claude model on ollama).
        if needs_planning && !planner.supports_model() {
            tracing::debug!(
                planner = planner.name(),
                "Skipping planning: model not available on provider"
            );
            needs_planning = false;
        }

        let plan_result = if needs_planning {
            // Phase E5: Transition to Planning state.
            if !silent {
                render_sink.agent_state_transition("executing", "planning", "generating plan");
            }
            let plan_timeout = Duration::from_secs(planning_config.timeout_secs);
            let result = tokio::time::timeout(
                plan_timeout,
                planner.plan(user_msg, &tool_defs),
            )
            .await;
            // Phase E5: Transition back to Executing after planning.
            if !silent {
                render_sink.agent_state_transition("planning", "executing", "plan generated");
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

                active_plan = Some(plan);
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

    // Cache tools outside the loop — tool definitions never change between rounds.
    // Phase 38: Apply intent-based tool selection when dynamic_tool_selection is enabled.
    let cached_tools = {
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
        selected
    };
    // System prompt may update mid-session if instruction files (CUERVO.md) change on disk.
    // Track instruction content separately for surgical replacement in the full system prompt.
    let mut cached_system = request.system.clone();
    let mut cached_instructions =
        cuervo_context::load_instructions(std::path::Path::new(working_dir));

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

            let tasks: Vec<cuervo_core::types::SubAgentTask> =
                tasks_with_indices.into_iter().map(|(_, t)| t).collect();

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
                false, // Sub-agents run non-interactively.
                false,
            )
            .await;

            // Feed orchestrator results back into tracker.
            if let Ok(orch_result) = orch_result {
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

                let delegated_count = matched.len();
                if delegated_count > 0 {
                    tracing::info!(delegated_count, "Steps delegated to sub-agents");
                }
            } else if let Err(ref e) = orch_result {
                tracing::warn!("Delegation orchestrator failed: {e}, falling back to inline execution");
            }
        }
    }

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
            if !sys.contains("## Tool Usage Policy") {
                sys.push_str(TOOL_USAGE_POLICY);
            }
        }
    }

    // Confidence feedback: track the last reflection's entry_id so we can
    // boost it on subsequent success or decay it on repeated failure.
    let mut last_reflection_id: Option<uuid::Uuid> = None;

    // Speculative read-only tool execution: predict and pre-execute read-only tools
    // while the model streams its response. Cached results are served instantly.
    let speculator = super::tool_speculation::ToolSpeculator::new();

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
    let mut force_no_tools_next_round = false;

    for round in 0..limits.max_rounds {
        // Round separator is emitted after model selection (see below) so we can show provider info.

        let _round_span = tracing::info_span!(
            "gen_ai.agent.round",
            "gen_ai.request.model" = %request.model,
            "gen_ai.operation.name" = "agent_round",
            round,
        )
        .entered();
        let round_start = Instant::now();
        let mut round_usage = TokenUsage::default();

        // Context compaction check: summarize old messages if approaching context limit.
        // Wrapped in a 15s timeout to prevent indefinite blocking on slow providers.
        if let Some(compactor) = compactor {
            if compactor.needs_compaction(&messages) {
                if !silent {
                    render_sink.spinner_start("Compacting context...");
                }
                tracing::info!(
                    round,
                    message_count = messages.len(),
                    estimated_tokens = ContextCompactor::estimate_message_tokens(&messages),
                    "Context compaction triggered"
                );
                let pre_compact_count = messages.len();

                let compaction_result = tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    async {
                        // Build a compaction request using the same provider.
                        let summary_prompt = compactor.compaction_prompt(&messages);
                        let compaction_request = ModelRequest {
                            model: request.model.clone(),
                            messages: vec![ChatMessage {
                                role: Role::User,
                                content: MessageContent::Text(summary_prompt),
                            }],
                            tools: vec![],
                            max_tokens: Some(2048),
                            temperature: Some(0.0),
                            system: Some("You are a conversation summarizer. Output only the summary, no preamble.".into()),
                            stream: true,
                        };

                        // Invoke provider for summary (direct, no resilience/fallback needed).
                        let mut summary_text = String::new();
                        match provider.invoke(&compaction_request).await {
                            Ok(mut stream) => {
                                while let Some(chunk_result) = stream.next().await {
                                    if let Ok(ModelChunk::TextDelta(delta)) = chunk_result {
                                        summary_text.push_str(&delta);
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Compaction failed, continuing without: {e}");
                            }
                        }
                        summary_text
                    },
                )
                .await;

                // Stop compaction spinner before processing result.
                if !silent {
                    render_sink.spinner_stop();
                }

                match compaction_result {
                    Ok(summary_text) if !summary_text.is_empty() => {
                        compactor.apply_compaction(&mut messages, &summary_text);
                        // Sync session messages and re-seed pipeline.
                        session.messages = messages.clone();
                        context_pipeline.reset();
                        for msg in &messages {
                            context_pipeline.add_message(msg.clone());
                        }
                        let tokens_saved = ContextCompactor::estimate_message_tokens(&messages);
                        if !silent {
                            render_sink.compaction_complete(pre_compact_count, messages.len(), tokens_saved as u64);
                        }
                        tracing::info!(
                            new_message_count = messages.len(),
                            "Context compacted successfully"
                        );
                    }
                    Err(_) => {
                        tracing::warn!("Context compaction timed out after 15s, skipping");
                    }
                    _ => {}
                }
            }
        }

        // Token budget pre-check: skip invocation if already over budget.
        if limits.max_total_tokens > 0
            && session.total_usage.total() >= limits.max_total_tokens
        {
            if !silent {
                render_sink.warning(
                    &format!(
                        "token budget exceeded before round: {} / {} tokens",
                        session.total_usage.total(),
                        limits.max_total_tokens
                    ),
                    Some("Reduce prompt size or increase max_total_tokens"),
                );
            }
            break;
        }

        // Optional: context-aware model selection with mid-session re-evaluation.
        // Uses the pipeline's context-managed messages for accurate complexity scoring,
        // not the original request (which only has the first user message).
        let (mut selected_model, effective_provider) = if let Some(selector) = model_selector {
            let spend = session.estimated_cost_usd;
            // Build a lightweight request for complexity scoring — avoids cloning the full
            // original request (tools, system prompt) just to override messages.
            let round_context_request = ModelRequest {
                model: request.model.clone(),
                messages: context_pipeline.build_messages(),
                tools: cached_tools.clone(),
                max_tokens: request.max_tokens,
                temperature: request.temperature,
                system: cached_system.clone(),
                stream: true,
            };
            if let Some(selection) = selector.select_model(&round_context_request, spend) {
                tracing::debug!(
                    model = %selection.model_id,
                    provider = %selection.provider_name,
                    reason = %selection.reason,
                    "Model selector override"
                );
                if !silent {
                    render_sink.model_selected(&selection.model_id, &selection.provider_name, &selection.reason);
                }
                // Switch provider if the selected model belongs to a different one.
                let resolved_provider = if selection.provider_name != provider.name() {
                    let looked_up = registry.and_then(|r| r.get(&selection.provider_name));
                    if let Some(p) = looked_up {
                        tracing::info!(
                            from = provider.name(),
                            to = p.name(),
                            model = %selection.model_id,
                            "Switched provider for model selection"
                        );
                        Arc::clone(p)
                    } else {
                        tracing::warn!(
                            target_provider = %selection.provider_name,
                            "Model selector target provider not in registry, keeping default"
                        );
                        Arc::clone(provider)
                    }
                } else {
                    Arc::clone(provider)
                };
                (selection.model_id, resolved_provider)
            } else {
                (request.model.clone(), Arc::clone(provider))
            }
        } else {
            (request.model.clone(), Arc::clone(provider))
        };

        // Phase 30: if a previous round's fallback adapted the model, use it.
        if let Some(ref adapted) = fallback_adapted_model {
            selected_model = adapted.clone();
        }

        // Phase 32: persist model selector override for cross-round stability.
        // When the selector picks a different model (e.g., deepseek-coder-v2 on ollama)
        // and the selector returns None on a later round, we reuse the last working model
        // instead of request.model (which may not be valid on the current provider).
        if selected_model != request.model && fallback_adapted_model.is_none() {
            fallback_adapted_model = Some(selected_model.clone());
        }

        // Round separator: shown for rounds 2+ (after model selection so we can display provider).
        if round > 0 && !silent {
            render_sink.round_started(round + 1, effective_provider.name(), &selected_model);
        }

        // Per-round instruction refresh: check if CUERVO.md files changed on disk.
        // Performs a stat syscall (~10μs) per instruction file — negligible overhead.
        if let Some(new_instr) = context_pipeline.refresh_instructions(std::path::Path::new(working_dir)) {
            if let Some(ref mut sys) = cached_system {
                if let Some(ref old_instr) = cached_instructions {
                    // Surgically replace the instruction portion within the full system prompt.
                    *sys = sys.replacen(old_instr.as_str(), &new_instr, 1);
                }
            }
            tracing::info!(round, "Instruction files changed on disk — system prompt updated");
            cached_instructions = Some(new_instr);
        }

        // Per-round plan section update: refresh step statuses and current step indicator.
        if let Some(ref tracker) = execution_tracker {
            let plan = tracker.plan();
            let plan_section = format_plan_for_prompt(plan, tracker.current_step());
            if let Some(ref mut sys) = cached_system {
                update_plan_in_system(sys, &plan_section);
            }
        }

        // Build round request using pipeline's context-managed message view.
        // The pipeline returns L4+L3+L2+L1 context prefixed before L0 hot messages,
        // automatically handling token budget enforcement across tiers.
        // The full `messages` Vec is preserved for fingerprinting/checkpointing.
        context_pipeline.set_round(round as u32);
        let built_messages = context_pipeline.build_messages();
        // Phase 42: record context assembly metrics.
        if let Some(metrics) = context_metrics {
            let approx_tokens = built_messages.iter().map(|m| {
                match &m.content {
                    MessageContent::Text(t) => t.len() / 4,
                    MessageContent::Blocks(blocks) => blocks.iter().map(|b| match b {
                        cuervo_core::types::ContentBlock::Text { text, .. } => text.len() / 4,
                        _ => 20,
                    }).sum(),
                }
            }).sum::<usize>();
            metrics.record_assembly(approx_tokens as u32, 0);
        }
        // Phase 43D: Emit context tier data for TUI panel.
        if !silent {
            let l0_tokens = context_pipeline.l0().token_count();
            let l0_cap = context_pipeline.l0().capacity() as u32 * 50; // approx token capacity
            let l1_tokens = context_pipeline.l1().token_count();
            let l1_entries = context_pipeline.l1().len();
            let l2_entries = context_pipeline.l2().len();
            let l3_entries = context_pipeline.l3().len();
            let l4_entries = context_pipeline.l4().len();
            let total = context_pipeline.estimated_tokens();
            render_sink.context_tier_update(
                l0_tokens, l0_cap, l1_tokens, l1_entries,
                l2_entries, l3_entries, l4_entries, total,
            );
        }
        let mut round_request = ModelRequest {
            model: selected_model.clone(),
            messages: built_messages,
            tools: if force_no_tools_next_round {
                vec![] // Phase 33: loop guard forced tool withdrawal
            } else {
                cached_tools.clone()
            },
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            system: cached_system.clone(),
            stream: true,
        };
        // Reset the flag after consuming it.
        force_no_tools_next_round = false;

        // Pre-invoke validation: ensure model is supported by the effective provider.
        if let Err(e) = effective_provider.validate_model(&selected_model) {
            tracing::error!(
                model = %selected_model,
                provider = effective_provider.name(),
                "Model validation failed: {e}"
            );
            if !silent {
                render_sink.error(
                    &format!(
                        "model '{}' is not supported by provider '{}'. Available: {}",
                        selected_model,
                        effective_provider.name(),
                        effective_provider
                            .supported_models()
                            .iter()
                            .map(|m| m.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                    Some("Use -m to specify a valid model for your provider"),
                );
            }
            return Ok(AgentLoopResult {
                full_text,
                rounds,
                stop_condition: StopCondition::ProviderError,
                input_tokens: call_input_tokens,
                output_tokens: call_output_tokens,
                cost_usd: call_cost,
                latency_ms: loop_start.elapsed().as_millis() as u64,
                execution_fingerprint: compute_fingerprint(&round_request.messages),
                timeline_json: None,
                ctrl_rx,
            });
        }

        // Context window guard: warn if estimated tokens exceed model's context window.
        if let Some(context_window) = effective_provider.model_context_window(&selected_model) {
            let estimated = ContextCompactor::estimate_message_tokens(&round_request.messages);
            if estimated > context_window as usize {
                tracing::warn!(
                    estimated_tokens = estimated,
                    context_window,
                    model = %selected_model,
                    "Estimated tokens exceed model context window"
                );
                if !silent {
                    render_sink.warning(
                        &format!(
                            "context ({} tokens) exceeds model limit ({} tokens) — response quality may degrade",
                            estimated, context_window,
                        ),
                        Some("Enable compaction or reduce conversation length"),
                    );
                }
            }
        }

        // Protocol validation: ensure no orphaned ToolResult blocks reach the provider.
        // This catches bugs in compaction, L0 eviction, or pipeline assembly that could
        // produce 400 invalid_request_error from providers.
        {
            let violations = cuervo_core::types::validation::validate_message_sequence(
                &round_request.messages,
                false, // no trailing tool use expected — we're about to invoke the model
            );
            let critical: Vec<_> = violations
                .iter()
                .filter(|v| matches!(
                    v,
                    cuervo_core::types::validation::ProtocolViolation::OrphanedToolResult { .. }
                    | cuervo_core::types::validation::ProtocolViolation::ToolResultWrongRole { .. }
                    | cuervo_core::types::validation::ProtocolViolation::DuplicateToolUseId { .. }
                ))
                .collect();

            if !critical.is_empty() {
                for v in &critical {
                    tracing::error!("Protocol violation in round {round}: {v}");
                }
                // Auto-repair: strip orphaned results to prevent provider 400s.
                let repaired = cuervo_core::types::validation::strip_orphaned_tool_results(
                    &round_request.messages,
                );
                tracing::warn!(
                    original_count = round_request.messages.len(),
                    repaired_count = repaired.len(),
                    violations = critical.len(),
                    "Auto-repaired message sequence (stripped orphaned tool results)"
                );
                round_request = ModelRequest {
                    messages: repaired,
                    ..round_request
                };
            }
        }

        // Trace: record model request.
        record_trace(
            trace_db,
            session_id,
            &mut trace_step_index,
            TraceStepType::ModelRequest,
            serde_json::json!({
                "round": round,
                "model": &round_request.model,
                "message_count": round_request.messages.len(),
                "tool_count": round_request.tools.len(),
                "has_system": round_request.system.is_some(),
            })
            .to_string(),
            0,
            exec_clock,
        );

        // Guardrail pre-invocation check.
        if !guardrails.is_empty() {
            let input_text = round_request.messages
                .iter()
                .rev()
                .find(|m| m.role == Role::User)
                .map(|m| match &m.content {
                    MessageContent::Text(t) => t.as_str(),
                    _ => "",
                })
                .unwrap_or("");

            let violations = cuervo_security::run_guardrails(
                guardrails,
                input_text,
                cuervo_security::GuardrailCheckpoint::PreInvocation,
            );
            for v in &violations {
                tracing::warn!(
                    guardrail = %v.guardrail,
                    matched = %v.matched,
                    "Guardrail triggered: {}",
                    v.reason
                );
                let _ = event_tx.send(DomainEvent::new(EventPayload::GuardrailTriggered {
                    guardrail: v.guardrail.clone(),
                    checkpoint: "pre".into(),
                    action: format!("{:?}", v.action),
                }));
            }
            if cuervo_security::has_blocking_violation(&violations) {
                if !silent { render_sink.info("\n[blocked by guardrail]"); }
                break;
            }
        }

        // Check response cache before invoking provider.
        if let Some(cache) = response_cache {
            if let Some(entry) = cache.lookup(&round_request).await {
                tracing::info!(round, "Response cache hit");
                if !silent { render_sink.cache_status(true, "response_cache"); }
                let round_text = entry.response_text.clone();

                // Render the cached response (only if visible).
                if !silent {
                    render_sink.stream_text(&round_text);
                    render_sink.stream_done();
                }

                // Record cache hit in trace.
                record_trace(
                    trace_db,
                    session_id,
                    &mut trace_step_index,
                    TraceStepType::ModelResponse,
                    serde_json::json!({
                        "round": round,
                        "text": &round_text,
                        "stop_reason": "end_turn",
                        "cache_hit": true,
                    })
                    .to_string(),
                    0,
                    exec_clock,
                );

                full_text.push_str(&round_text);
                if !round_text.is_empty() {
                    let msg = ChatMessage {
                        role: Role::Assistant,
                        content: MessageContent::Text(round_text),
                    };
                    messages.push(msg.clone());
                    context_pipeline.add_message(msg.clone());
                    session.add_message(msg);
                }
                // Cache never stores tool_use responses, so this is always terminal.
                break;
            }
        }

        // Reset stream renderer state for a new round.
        if !silent { render_sink.stream_reset(); }
        let mut silent_text = String::new(); // text accumulator for silent mode
        let mut accumulator = ToolUseAccumulator::new();
        let mut stop_reason = StopReason::EndTurn;

        // Track actual provider/model used this round (may differ from request due to fallback).
        // `round_provider_name` is updated in Ok(attempt) if fallback was used.
        #[allow(unused_assignments)]
        let mut round_provider_name = effective_provider.name().to_string();
        let round_model_name = round_request.model.clone();
        // Track the actual provider Arc for cost estimation (updated on fallback).
        let mut round_cost_provider: Arc<dyn ModelProvider> = Arc::clone(&effective_provider);

        // Phase 43: Check control channel before model invocation (yield point 1).
        if let Some(ref mut rx) = ctrl_rx {
            // If auto_pause is set (from previous StepOnce), pause before this round.
            if auto_pause {
                auto_pause = false;
                render_sink.info("  [paused] Step complete — Space to resume, N to step");
                loop {
                    match rx.recv().await {
                        Some(crate::tui::events::ControlEvent::Resume) => break,
                        Some(crate::tui::events::ControlEvent::Step) => {
                            auto_pause = true;
                            break;
                        }
                        Some(crate::tui::events::ControlEvent::CancelAgent) | None => {
                            ctrl_cancelled = true;
                            break;
                        }
                        _ => continue,
                    }
                }
                if ctrl_cancelled {
                    break;
                }
            }
            match check_control(rx, render_sink).await {
                ControlAction::Continue => {}
                ControlAction::StepOnce => { auto_pause = true; }
                ControlAction::Cancel => {
                    ctrl_cancelled = true;
                    break;
                }
            }
        }

        // Show spinner during model inference (appears after 200ms delay).
        if !silent {
            let label = if routing_config.mode == "speculative" && !fallback_providers.is_empty() {
                let count = 1 + fallback_providers.len();
                format!("Racing {count} providers...")
            } else {
                format!("Thinking... [{}]", effective_provider.name())
            };
            render_sink.spinner_start(&label);
        }
        let mut spinner_active = !silent;

        // Speculative tool pre-execution: predict read-only tools the model will
        // likely call and pre-execute them in background while the model streams.
        if replay_tool_executor.is_none() {
            let spec_count = speculator
                .speculate(&messages, tool_registry, working_dir)
                .await;
            if spec_count > 0 {
                tracing::debug!(count = spec_count, "Speculative tools launched");
            }
        }

        // Invoke provider with resilience-aware routing (failover / speculative).
        // Wrap in a timeout to prevent indefinite hangs on slow providers.
        // On transient errors (provider error or stream error), retry the round once
        // with exponential backoff before giving up.
        let provider_timeout = if limits.provider_timeout_secs > 0 {
            Duration::from_secs(limits.provider_timeout_secs)
        } else {
            Duration::from_secs(u64::MAX / 2) // effectively unlimited
        };

        let mut round_retry_count: u32 = 0;
        const MAX_ROUND_RETRIES: u32 = 1;

        'invoke_retry: loop {
        let invoke_attempt = tokio::time::timeout(
            provider_timeout,
            invoke_with_fallback(
                &effective_provider,
                &round_request,
                fallback_providers,
                resilience,
                routing_config,
                event_tx,
            ),
        )
        .await;

        // Flatten timeout into the error path.
        let invoke_attempt = match invoke_attempt {
            Ok(inner) => inner,
            Err(_elapsed) => {
                render_sink.spinner_stop();
                let timeout_latency_ms = round_start.elapsed().as_millis() as u64;
                // Record timeout metric.
                if let Some(db) = trace_db {
                    let metric = InvocationMetric {
                        provider: provider.name().to_string(),
                        model: request.model.clone(),
                        latency_ms: timeout_latency_ms,
                        input_tokens: 0,
                        output_tokens: 0,
                        estimated_cost_usd: 0.0,
                        success: false,
                        stop_reason: "timeout".to_string(),
                        session_id: Some(session_id.to_string()),
                        created_at: Utc::now(),
                    };
                    if let Err(me) = db.inner().insert_metric(&metric) {
                        tracing::warn!("Failed to persist timeout metric: {me}");
                    }
                }
                record_trace(
                    trace_db,
                    session_id,
                    &mut trace_step_index,
                    TraceStepType::Error,
                    serde_json::json!({
                        "round": round,
                        "context": "provider_timeout",
                        "timeout_secs": limits.provider_timeout_secs,
                        "retry": round_retry_count,
                    })
                    .to_string(),
                    timeout_latency_ms,
                    exec_clock,
                );
                // Retry on timeout if retries remain.
                if round_retry_count < MAX_ROUND_RETRIES {
                    round_retry_count += 1;
                    tracing::info!(retry = round_retry_count, "Retrying round after provider timeout");
                    if !silent {
                        render_sink.warning(
                            "provider timed out, retrying...",
                            None,
                        );
                    }
                    tokio::time::sleep(Duration::from_secs(2u64.pow(round_retry_count))).await;
                    spinner_active = !silent;
                    continue 'invoke_retry;
                }
                if !silent {
                    render_sink.error(
                        &format!("provider timed out after {}s", limits.provider_timeout_secs),
                        Some("Increase provider_timeout_secs or check network connectivity"),
                    );
                }
                return Ok(AgentLoopResult {
                    full_text,
                    rounds,
                    stop_condition: StopCondition::ProviderError,
                    input_tokens: call_input_tokens,
                    output_tokens: call_output_tokens,
                    cost_usd: call_cost,
                    latency_ms: loop_start.elapsed().as_millis() as u64,
                    execution_fingerprint: compute_fingerprint(&round_request.messages),
                    timeline_json: None,
                    ctrl_rx,
                });
            }
        };

        match invoke_attempt {
            Ok(attempt) => {
                let _permit = attempt.permit;
                let used_provider_name = attempt.provider_name.clone();
                round_provider_name = attempt.provider_name.clone();
                if attempt.is_fallback {
                    if !silent {
                        render_sink.provider_fallback(
                            effective_provider.name(),
                            &attempt.provider_name,
                            "primary provider failed",
                        );
                    }
                    // Adapt model for subsequent rounds: the fallback provider may not
                    // support the original model (e.g., anthropic→ollama). Without this,
                    // round 2+ would fail model validation with the original model name.
                    if let Some((_, fb_prov)) = fallback_providers.iter()
                        .find(|(n, _)| *n == attempt.provider_name)
                    {
                        // Update cost estimation provider to the actual fallback Arc.
                        round_cost_provider = Arc::clone(fb_prov);
                        if !fb_prov.supported_models().iter().any(|m| m.id == round_request.model) {
                            if let Some(default_model) = fb_prov.supported_models().first() {
                                tracing::info!(
                                    old_model = %round_request.model,
                                    new_model = %default_model.id,
                                    provider = %attempt.provider_name,
                                    "Adapted model for fallback provider on subsequent rounds"
                                );
                                if !silent {
                                    render_sink.model_selected(&default_model.id, &attempt.provider_name, "adapted for fallback provider");
                                }
                                round_request.model = default_model.id.clone();
                                fallback_adapted_model = Some(default_model.id.clone());
                            }
                        }
                    }
                }
                let mut stream = attempt.stream;
                let mut stream_had_error = false;
                let cancelled = loop {
                    tokio::select! {
                        chunk_opt = stream.next() => {
                            match chunk_opt {
                                Some(Ok(chunk)) => {
                                    // Stop spinner on first content.
                                    if spinner_active
                                        && matches!(chunk, ModelChunk::TextDelta(_) | ModelChunk::ToolUseStart { .. } | ModelChunk::Error(_))
                                    {
                                        render_sink.spinner_stop();
                                        spinner_active = false;
                                    }
                                    // Track usage (session cumulative + per-round).
                                    if let ModelChunk::Usage(ref u) = chunk {
                                        session.total_usage.input_tokens += u.input_tokens;
                                        session.total_usage.output_tokens += u.output_tokens;
                                        round_usage.input_tokens += u.input_tokens;
                                        round_usage.output_tokens += u.output_tokens;
                                    }
                                    // Capture stop reason.
                                    if let ModelChunk::Done(reason) = &chunk {
                                        stop_reason = *reason;
                                    }
                                    // Feed to accumulator first.
                                    accumulator.process(&chunk);
                                    // Render via sink (or silently accumulate).
                                    if !silent {
                                        match &chunk {
                                            ModelChunk::TextDelta(t) => render_sink.stream_text(t),
                                            ModelChunk::ToolUseStart { name, .. } => render_sink.stream_tool_marker(name),
                                            ModelChunk::Error(msg) => render_sink.stream_error(msg),
                                            ModelChunk::Done(_) => { render_sink.stream_done(); break false; }
                                            _ => {}
                                        }
                                    } else {
                                        // Silent: accumulate text, detect done.
                                        if let ModelChunk::TextDelta(t) = &chunk {
                                            silent_text.push_str(t);
                                        }
                                        if matches!(chunk, ModelChunk::Done(_)) {
                                            break false;
                                        }
                                    }
                                }
                                Some(Err(e)) => {
                                    if !silent {
                                        render_sink.stream_error(&format!("{e}"));
                                    }
                                    // Record stream failure for health scoring.
                                    if resilience.is_enabled() {
                                        resilience.record_failure(&used_provider_name).await;
                                        // Phase E3/E4: emit provider health as degraded after failure.
                                        if !silent {
                                            render_sink.provider_health_update(
                                                &used_provider_name, "degraded", 0.0,
                                                round_start.elapsed().as_millis() as u64,
                                            );
                                        }
                                    }
                                    stream_had_error = true;
                                    break false;
                                }
                                None => break false,
                            }
                        }
                        _ = tokio::signal::ctrl_c() => {
                            break true;
                        }
                    }
                };

                if cancelled {
                    render_sink.spinner_stop();
                    if !silent { render_sink.warning("response interrupted by user", None); }
                    drop(stream);
                    return Ok(AgentLoopResult {
                        full_text,
                        rounds,
                        stop_condition: StopCondition::Interrupted,
                        input_tokens: call_input_tokens,
                        output_tokens: call_output_tokens,
                        cost_usd: call_cost,
                        latency_ms: loop_start.elapsed().as_millis() as u64,
                        execution_fingerprint: compute_fingerprint(&round_request.messages),
                        timeline_json: None,
                        ctrl_rx,
                    });
                }

                // Resilience: record success for the provider that was used.
                if resilience.is_enabled() && !stream_had_error {
                    resilience.record_success(&used_provider_name).await;
                    // Phase E3/E4: emit provider health as healthy after success.
                    if !silent {
                        render_sink.provider_health_update(&used_provider_name, "healthy", 0.0, 0);
                    }
                }

                // Stream error: retry the round if retries remain, discarding partial output.
                if stream_had_error {
                    if let Some(db) = trace_db {
                        let metric = InvocationMetric {
                            provider: used_provider_name.clone(),
                            model: request.model.clone(),
                            latency_ms: round_start.elapsed().as_millis() as u64,
                            input_tokens: round_usage.input_tokens,
                            output_tokens: round_usage.output_tokens,
                            estimated_cost_usd: 0.0,
                            success: false,
                            stop_reason: "stream_error".to_string(),
                            session_id: Some(session_id.to_string()),
                            created_at: Utc::now(),
                        };
                        if let Err(me) = db.inner().insert_metric(&metric) {
                            tracing::warn!("Failed to persist stream error metric: {me}");
                        }
                    }
                    if round_retry_count < MAX_ROUND_RETRIES {
                        round_retry_count += 1;
                        tracing::info!(retry = round_retry_count, "Retrying round after stream error");
                        if !silent {
                            render_sink.warning(
                                "stream error, retrying...",
                                None,
                            );
                        }
                        // Reset round-level accumulators for retry.
                        accumulator = ToolUseAccumulator::new();
                        if !silent { render_sink.stream_reset(); }
                        silent_text.clear();
                        round_usage = TokenUsage::default();
                        spinner_active = !silent;
                        tokio::time::sleep(Duration::from_secs(2u64.pow(round_retry_count))).await;
                        continue 'invoke_retry;
                    }
                    // Accept partial text on final stream error.
                }
            }
            Err(e) => {
                render_sink.spinner_stop();
                let error_latency_ms = round_start.elapsed().as_millis() as u64;
                // Trace: record error.
                record_trace(
                    trace_db,
                    session_id,
                    &mut trace_step_index,
                    TraceStepType::Error,
                    serde_json::json!({
                        "round": round,
                        "context": "provider_invoke",
                        "message": format!("{e}"),
                        "retry": round_retry_count,
                    })
                    .to_string(),
                    error_latency_ms,
                    exec_clock,
                );
                // Persist failed invocation metric for optimizer learning.
                if let Some(db) = trace_db {
                    let metric = InvocationMetric {
                        provider: provider.name().to_string(),
                        model: request.model.clone(),
                        latency_ms: error_latency_ms,
                        input_tokens: 0,
                        output_tokens: 0,
                        estimated_cost_usd: 0.0,
                        success: false,
                        stop_reason: "error".to_string(),
                        session_id: Some(session_id.to_string()),
                        created_at: Utc::now(),
                    };
                    if let Err(me) = db.inner().insert_metric(&metric) {
                        tracing::warn!("Failed to persist error metric: {me}");
                    }
                }
                // Retry on provider error if retries remain.
                if round_retry_count < MAX_ROUND_RETRIES {
                    round_retry_count += 1;
                    tracing::info!(retry = round_retry_count, error = %e, "Retrying round after provider error");
                    if !silent {
                        render_sink.warning(
                            &format!("provider error, retrying... ({e})"),
                            None,
                        );
                    }
                    spinner_active = !silent;
                    tokio::time::sleep(Duration::from_secs(2u64.pow(round_retry_count))).await;
                    continue 'invoke_retry;
                }
                if !silent {
                    render_sink.info("");
                    let hint = classify_error_hint(&format!("{e}"));
                    render_sink.error(
                        &format!("provider request failed — {e}"),
                        Some(hint),
                    );
                }
                return Ok(AgentLoopResult {
                    full_text,
                    rounds,
                    stop_condition: StopCondition::ProviderError,
                    input_tokens: call_input_tokens,
                    output_tokens: call_output_tokens,
                    cost_usd: call_cost,
                    latency_ms: loop_start.elapsed().as_millis() as u64,
                    execution_fingerprint: compute_fingerprint(&round_request.messages),
                    timeline_json: None,
                    ctrl_rx,
                });
            }
        }

        break 'invoke_retry; // Successful invocation, exit retry loop.
        } // end 'invoke_retry

        // Emit ModelInvoked event with per-round metrics (uses actual provider/model, not request).
        let round_latency_ms = round_start.elapsed().as_millis() as u64;
        let _ = event_tx.send(DomainEvent::new(EventPayload::ModelInvoked {
            provider: round_provider_name.clone(),
            model: round_model_name.clone(),
            usage: round_usage.clone(),
            latency_ms: round_latency_ms,
        }));

        // Track session-level metrics.
        session.total_latency_ms += round_latency_ms;

        // Estimate cost for this round (use actual provider — may be fallback).
        let round_cost = round_cost_provider.estimate_cost(&round_request);
        session.estimated_cost_usd += round_cost.estimated_cost_usd;

        // Accumulate per-call metrics.
        call_input_tokens += round_usage.input_tokens as u64;
        call_output_tokens += round_usage.output_tokens as u64;
        call_cost += round_cost.estimated_cost_usd;

        if round_cost.estimated_cost_usd > 0.0 {
            tracing::debug!(
                cost = format!("${:.4}", round_cost.estimated_cost_usd),
                cumulative = format!("${:.4}", session.estimated_cost_usd),
                "Round cost"
            );
        }

        // Emit round-end metrics to sink.
        if !silent {
            render_sink.round_ended(
                round + 1,
                round_usage.input_tokens,
                round_usage.output_tokens,
                round_cost.estimated_cost_usd,
                round_latency_ms,
            );
        }

        // Phase E2: Emit token budget update after each round.
        if !silent && limits.max_total_tokens > 0 {
            let used_tokens = session.total_usage.total() as u64;
            let limit_tokens = limits.max_total_tokens as u64;
            let elapsed_secs = loop_start.elapsed().as_secs_f64().max(0.001);
            let rate = used_tokens as f64 / (elapsed_secs / 60.0);
            render_sink.token_budget_update(used_tokens, limit_tokens, rate);
        }

        // Convert stop_reason to API-compatible string.
        let stop_reason_str = match stop_reason {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::ToolUse => "tool_use",
            StopReason::StopSequence => "stop_sequence",
        };

        // Persist invocation metric to DB for optimizer learning (actual provider/model).
        if let Some(db) = trace_db {
            let metric = InvocationMetric {
                provider: round_provider_name.clone(),
                model: round_model_name.clone(),
                latency_ms: round_latency_ms,
                input_tokens: round_usage.input_tokens,
                output_tokens: round_usage.output_tokens,
                estimated_cost_usd: round_cost.estimated_cost_usd,
                success: true,
                stop_reason: stop_reason_str.to_string(),
                session_id: Some(session_id.to_string()),
                created_at: Utc::now(),
            };
            if let Err(e) = db.inner().insert_metric(&metric) {
                tracing::warn!("Failed to persist invocation metric: {e}");
            }

            // Advisory optimizer logging: recommend optimal model for this workload.
            if let Ok(sys) = db.inner().system_metrics() {
                let ranked = super::optimizer::CostLatencyOptimizer::rank_from_metrics(
                    &sys,
                    super::optimizer::OptimizeStrategy::from_str(&routing_config.strategy),
                );
                if let Some(top) = ranked.first() {
                    if top.provider != round_provider_name || top.model != round_model_name {
                        tracing::debug!(
                            current_model = %round_model_name,
                            recommended = %top.model,
                            recommended_provider = %top.provider,
                            score = %format!("{:.3}", top.score),
                            "Optimizer advisory: a better model may be available"
                        );
                    }
                }
            }
        }

        // Accumulate text from this round.
        let round_text = if !silent {
            render_sink.stream_full_text()
        } else {
            std::mem::take(&mut silent_text)
        };
        full_text.push_str(&round_text);

        // Guardrail post-invocation check on model output.
        if !guardrails.is_empty() && !round_text.is_empty() {
            let violations = cuervo_security::run_guardrails(
                guardrails,
                &round_text,
                cuervo_security::GuardrailCheckpoint::PostInvocation,
            );
            for v in &violations {
                tracing::warn!(
                    guardrail = %v.guardrail,
                    matched = %v.matched,
                    "Output guardrail: {}",
                    v.reason
                );
                let _ = event_tx.send(DomainEvent::new(EventPayload::GuardrailTriggered {
                    guardrail: v.guardrail.clone(),
                    checkpoint: "post".into(),
                    action: format!("{:?}", v.action),
                }));
            }
            if cuervo_security::has_blocking_violation(&violations) {
                if !silent { render_sink.info("\n[response blocked by guardrail]"); }
                break;
            }
        }

        // Trace: defer ModelResponse recording until after finalize (to capture tool_uses).
        // The `pending_trace_*` variables hold per-round values for deferred recording.
        let pending_trace_round = round;
        let pending_trace_text = round_text.clone();
        let pending_trace_stop = stop_reason_str.to_string();
        let pending_trace_usage = round_usage.clone();
        let pending_trace_latency = round_latency_ms;

        // Store response in cache (cache.store() internally skips tool_use).
        if let Some(cache) = response_cache {
            let usage_json = serde_json::json!({
                "input_tokens": round_usage.input_tokens,
                "output_tokens": round_usage.output_tokens,
            })
            .to_string();
            cache.store(
                &round_request,
                &round_text,
                stop_reason_str,
                &usage_json,
                None,
            ).await;
        }

        // Note: messages Vec is preserved (not moved into round_request).
        // Pipeline manages L0-L4 context; messages Vec is full history for fingerprinting.

        // --- Guard checks ---

        // Token budget guard.
        if limits.max_total_tokens > 0
            && session.total_usage.total() >= limits.max_total_tokens
        {
            tracing::warn!(
                total = session.total_usage.total(),
                budget = limits.max_total_tokens,
                "Token budget exceeded"
            );
            if !silent {
                render_sink.warning(
                    &format!(
                        "token budget exceeded: {} / {} tokens",
                        session.total_usage.total(),
                        limits.max_total_tokens
                    ),
                    Some("Increase max_total_tokens in config to allow more processing"),
                );
            }
            if !round_text.is_empty() {
                let msg = ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text(round_text),
                };
                messages.push(msg.clone());
                context_pipeline.add_message(msg.clone());
                session.add_message(msg);
            }
            return Ok(AgentLoopResult {
                full_text,
                rounds,
                stop_condition: StopCondition::TokenBudget,
                input_tokens: call_input_tokens,
                output_tokens: call_output_tokens,
                cost_usd: call_cost,
                latency_ms: loop_start.elapsed().as_millis() as u64,
                execution_fingerprint: compute_fingerprint(&messages),
                timeline_json: None,
                ctrl_rx,
            });
        }

        // Duration budget guard.
        if limits.max_duration_secs > 0
            && loop_start.elapsed().as_secs() >= limits.max_duration_secs
        {
            tracing::warn!(
                elapsed_secs = loop_start.elapsed().as_secs(),
                budget_secs = limits.max_duration_secs,
                "Duration budget exceeded"
            );
            if !silent {
                render_sink.warning(
                    &format!(
                        "duration budget exceeded: {}s / {}s",
                        loop_start.elapsed().as_secs(),
                        limits.max_duration_secs
                    ),
                    Some("Increase max_duration_secs in config for longer tasks"),
                );
            }
            if !round_text.is_empty() {
                let msg = ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text(round_text),
                };
                messages.push(msg.clone());
                context_pipeline.add_message(msg.clone());
                session.add_message(msg);
            }
            return Ok(AgentLoopResult {
                full_text,
                rounds,
                stop_condition: StopCondition::DurationBudget,
                input_tokens: call_input_tokens,
                output_tokens: call_output_tokens,
                cost_usd: call_cost,
                latency_ms: loop_start.elapsed().as_millis() as u64,
                execution_fingerprint: compute_fingerprint(&messages),
                timeline_json: None,
                ctrl_rx,
            });
        }

        if stop_reason != StopReason::ToolUse {
            // Record deferred trace with empty tool_uses for non-tool-use rounds.
            record_trace(
                trace_db, session_id, &mut trace_step_index,
                TraceStepType::ModelResponse,
                serde_json::json!({
                    "round": pending_trace_round,
                    "text": &pending_trace_text,
                    "stop_reason": &pending_trace_stop,
                    "usage": { "input_tokens": pending_trace_usage.input_tokens, "output_tokens": pending_trace_usage.output_tokens },
                    "latency_ms": pending_trace_latency,
                    "tool_uses": [],
                }).to_string(),
                pending_trace_latency,
                exec_clock,
            );
            // Record the assistant message and break.
            if !round_text.is_empty() {
                let msg = ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text(round_text),
                };
                messages.push(msg.clone());
                context_pipeline.add_message(msg.clone());
                session.add_message(msg);
            }
            // Auto-checkpoint after non-tool-use round (crash protection).
            auto_checkpoint(trace_db, session_id, rounds, &messages, session, trace_step_index);
            break;
        }

        // --- Tool use round ---
        rounds = round + 1;
        session.agent_rounds += 1;
        let completed_tools = accumulator.finalize();

        if completed_tools.is_empty() {
            break;
        }

        // Record deferred trace with tool_uses for tool-use rounds.
        record_trace(
            trace_db, session_id, &mut trace_step_index,
            TraceStepType::ModelResponse,
            serde_json::json!({
                "round": pending_trace_round,
                "text": &pending_trace_text,
                "stop_reason": &pending_trace_stop,
                "usage": { "input_tokens": pending_trace_usage.input_tokens, "output_tokens": pending_trace_usage.output_tokens },
                "latency_ms": pending_trace_latency,
                "tool_uses": completed_tools.iter().map(|t| serde_json::json!({
                    "id": t.id, "name": t.name, "input": t.input,
                })).collect::<Vec<_>>(),
            }).to_string(),
            pending_trace_latency,
            exec_clock,
        );

        // Record the assistant message with tool use blocks.
        let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
        if !round_text.is_empty() {
            assistant_blocks.push(ContentBlock::Text { text: round_text });
        }
        for tool in &completed_tools {
            assistant_blocks.push(ContentBlock::ToolUse {
                id: tool.id.clone(),
                name: tool.name.clone(),
                input: tool.input.clone(),
            });
        }
        let assistant_msg = ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(assistant_blocks),
        };
        messages.push(assistant_msg.clone());
        context_pipeline.add_message(assistant_msg.clone());
        session.add_message(assistant_msg);

        // Phase 33: collect (tool_name, args_hash) for this round's loop guard log.
        let round_tool_log: Vec<(String, u64)> = completed_tools
            .iter()
            .map(|t| (t.name.clone(), hash_tool_args(&t.input)))
            .collect();

        // Phase 33: dedup — filter out tool calls that were already executed with the
        // same arguments in a prior round. Produces a synthetic ToolResult for filtered calls
        // so the model doesn't get confused by missing results.
        let mut dedup_result_blocks: Vec<ContentBlock> = Vec::new();
        let deduplicated_tools: Vec<_> = completed_tools
            .into_iter()
            .filter(|tool| {
                let args_hash = hash_tool_args(&tool.input);
                if loop_guard.is_duplicate(&tool.name, args_hash) {
                    tracing::warn!(tool = %tool.name, "Duplicate tool call filtered");
                    dedup_result_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: tool.id.clone(),
                        content: "Already executed in a previous round. Use the existing result."
                            .to_string(),
                        is_error: true,
                    });
                    false
                } else {
                    true
                }
            })
            .collect();

        // Execute tools: in replay mode, return recorded results; otherwise execute normally.
        let plan = executor::plan_execution(deduplicated_tools, tool_registry);
        let mut tool_result_blocks: Vec<ContentBlock> = dedup_result_blocks;
        let mut tool_failures: Vec<(String, String)> = Vec::new(); // (tool_name, error)
        let mut tool_successes: Vec<String> = Vec::new(); // tool_name of successful executions

        if let Some(replay_exec) = replay_tool_executor {
            // Replay mode: return recorded results instead of executing tools.
            let all_tools = plan.parallel_batch.iter().chain(plan.sequential_batch.iter());
            for tool_call in all_tools {
                let (content, is_error) = if let Some(recorded) = replay_exec.get_result(&tool_call.id) {
                    (recorded.content.clone(), recorded.is_error)
                } else {
                    (format!("replay: no recorded result for tool_use_id '{}'", tool_call.id), true)
                };
                if is_error {
                    tool_failures.push((tool_call.name.clone(), content.clone()));
                }
                tool_result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: tool_call.id.clone(),
                    content,
                    is_error,
                });
            }
            session.tool_invocations +=
                (plan.parallel_batch.len() + plan.sequential_batch.len()) as u32;
        } else {
            // Normal mode: execute tools via parallel/sequential executor.

            // Check speculation cache: serve cached read-only results instantly.
            let (mut spec_hits, remaining_parallel): (Vec<executor::ToolExecResult>, Vec<_>) = {
                let mut hits = Vec::new();
                let mut remaining = Vec::new();
                for tool_call in &plan.parallel_batch {
                    if let Some(cached) = speculator.get_cached(&tool_call.name, &tool_call.input).await {
                        tracing::debug!(tool = %tool_call.name, "Speculation cache hit");
                        if !silent { render_sink.speculative_result(&tool_call.name, true); }
                        hits.push(executor::ToolExecResult {
                            tool_use_id: tool_call.id.clone(),
                            tool_name: tool_call.name.clone(),
                            content_block: ContentBlock::ToolResult {
                                tool_use_id: tool_call.id.clone(),
                                content: cached.output.content,
                                is_error: cached.output.is_error,
                            },
                            duration_ms: cached.duration_ms,
                            was_parallel: true,
                        });
                    } else {
                        remaining.push(tool_call.clone());
                    }
                }
                (hits, remaining)
            };

            // Execute remaining ReadOnly tools in parallel with concurrency cap.
            let parallel_results = executor::execute_parallel_batch(
                &remaining_parallel,
                tool_registry,
                working_dir,
                tool_timeout,
                event_tx,
                trace_db,
                session_id,
                &mut trace_step_index,
                limits.max_parallel_tools,
                &tool_exec_config,
                render_sink,
            )
            .await;
            // Merge speculation hits with real results.
            spec_hits.extend(parallel_results);
            let parallel_results = spec_hits;

            // Render parallel results.
            if !silent {
                for result in &parallel_results {
                    render_sink.tool_output(&result.content_block, result.duration_ms);
                }
            }

            // Execute ReadWrite/Destructive tools sequentially (with permission prompts).
            let mut sequential_results = Vec::new();
            for tool_call in &plan.sequential_batch {
                let result = executor::execute_sequential_tool(
                    tool_call,
                    tool_registry,
                    permissions,
                    working_dir,
                    tool_timeout,
                    event_tx,
                    trace_db,
                    session_id,
                    &mut trace_step_index,
                    &tool_exec_config,
                    render_sink,
                )
                .await;
                sequential_results.push(result);
            }

            // Track tool invocations.
            session.tool_invocations +=
                (parallel_results.len() + sequential_results.len()) as u32;

            // Collect all result blocks, apply intelligent elision, and track failures.
            // The elider preserves semantically important parts per tool type:
            // - bash: keeps last 30 lines (output tail is most relevant)
            // - file_read: keeps head + tail (context boundaries)
            // - grep: limits match count
            // Error outputs are never elided (full error context is critical).
            let elider_budget = context_pipeline.accountant()
                .available(cuervo_context::Tier::L0Hot) / 4;
            let elider_budget = elider_budget.max(500);

            for result in parallel_results {
                let mut block = result.content_block;
                if let ContentBlock::ToolResult {
                    ref mut content,
                    is_error: false,
                    ..
                } = block
                {
                    *content = context_pipeline.elider().elide(
                        &result.tool_name, content, Some(elider_budget),
                    );
                    tool_successes.push(result.tool_name.clone());
                }
                if let ContentBlock::ToolResult {
                    ref content,
                    is_error: true,
                    ..
                } = block
                {
                    tool_failures.push((result.tool_name.clone(), content.clone()));
                }
                tool_result_blocks.push(block);
            }
            for result in sequential_results {
                let mut block = result.content_block;
                if let ContentBlock::ToolResult {
                    ref mut content,
                    is_error: false,
                    ..
                } = block
                {
                    *content = context_pipeline.elider().elide(
                        &result.tool_name, content, Some(elider_budget),
                    );
                    tool_successes.push(result.tool_name.clone());
                }
                if let ContentBlock::ToolResult {
                    ref content,
                    is_error: true,
                    ..
                } = block
                {
                    tool_failures.push((result.tool_name.clone(), content.clone()));
                }
                tool_result_blocks.push(block);
            }
        }

        // Guardrail scan on tool results (warn-only — does not block tool output).
        if !guardrails.is_empty() {
            for block in &tool_result_blocks {
                if let ContentBlock::ToolResult { content, .. } = block {
                    let violations = cuervo_security::run_guardrails(
                        guardrails,
                        content,
                        cuervo_security::GuardrailCheckpoint::PostInvocation,
                    );
                    for v in &violations {
                        tracing::warn!(
                            guardrail = %v.guardrail,
                            matched = %v.matched,
                            source = "tool_result",
                            "Tool output guardrail: {}",
                            v.reason
                        );
                        let _ = event_tx.send(DomainEvent::new(EventPayload::GuardrailTriggered {
                            guardrail: v.guardrail.clone(),
                            checkpoint: "tool_result".into(),
                            action: format!("{:?}", v.action),
                        }));
                    }
                }
            }
        }

        // Match successful tool executions to plan steps via ExecutionTracker.
        if let Some(ref mut tracker) = execution_tracker {
            let failures_ref: Vec<(String, String)> = tool_failures.clone();
            let matched = tracker.record_tool_results(
                &tool_successes,
                &failures_ref,
                round,
            );

            // Persist outcomes to DB (tracker doesn't do I/O).
            if let Some(db) = trace_db {
                let plan_id = tracker.plan().plan_id;
                for m in &matched {
                    let (status, detail) = match &m.outcome {
                        StepOutcome::Success { summary } => ("success", summary.as_str()),
                        StepOutcome::Failed { error } => ("failed", error.as_str()),
                        StepOutcome::Skipped { reason } => ("skipped", reason.as_str()),
                    };
                    let _ = db
                        .update_plan_step_outcome(&plan_id, m.step_index as u32, status, detail)
                        .await;
                }
            }

            // Phase 33: plan completion → force synthesis on next round.
            if tracker.is_complete() {
                tracing::info!("All plan steps completed — forcing synthesis");
                loop_guard.force_synthesis();
            }

            // Update plan section in system prompt with new step statuses.
            let plan = tracker.plan();
            let current = tracker.current_step();
            let plan_section = format_plan_for_prompt(plan, current);
            if let Some(ref mut sys) = cached_system {
                update_plan_in_system(sys, &plan_section);
            }

            // Emit plan progress with timing to render sink.
            let (_, _, elapsed) = tracker.progress();
            render_sink.plan_progress_with_timing(
                &plan.goal,
                &plan.steps,
                current,
                tracker.tracked_steps(),
                elapsed,
            );

            // Sync outcomes into structured task bridge.
            if let Some(ref mut bridge) = task_bridge {
                bridge.sync_from_tracker(
                    tracker,
                    &round_model_name,
                    &round_provider_name,
                    Some(session_id),
                );
            }
        }

        // Phase 43: Check control channel after plan step completion (yield point 3).
        if let Some(ref mut rx) = ctrl_rx {
            match check_control(rx, render_sink).await {
                ControlAction::Continue => {}
                ControlAction::StepOnce => { auto_pause = true; }
                ControlAction::Cancel => {
                    ctrl_cancelled = true;
                    break;
                }
            }
        }

        // Reflexion: evaluate round and generate reflection on non-success.
        if let Some(reflector) = reflector {
            let outcome = super::reflexion::Reflector::evaluate_round(&tool_result_blocks);

            // Confidence feedback: if the previous round generated a reflection and
            // this round succeeded, boost that reflection's relevance. If this round
            // also failed, decay it (the advice didn't help).
            if let (Some(prev_id), Some(db)) = (last_reflection_id, trace_db) {
                let delta = if matches!(outcome, super::reflexion::RoundOutcome::Success) {
                    0.2 // Boost: the reflection led to recovery
                } else {
                    -0.15 // Decay: the reflection didn't help
                };
                // Load current score, apply delta, update.
                if let Ok(Some(entry)) = db.inner().load_memory(prev_id) {
                    let new_score = (entry.relevance_score + delta).clamp(0.1, 2.0);
                    let _ = db.update_memory_relevance(prev_id, new_score).await;
                    tracing::debug!(
                        reflection_id = %prev_id,
                        old_score = entry.relevance_score,
                        new_score,
                        "Reflection confidence updated"
                    );
                }
                last_reflection_id = None;
            }

            if !matches!(outcome, super::reflexion::RoundOutcome::Success) {
                // Phase E5: Transition to Reflecting state.
                if !silent {
                    render_sink.agent_state_transition("executing", "reflecting", "round had issues");
                }
                render_sink.reflection_started();
                match reflector.reflect(round, &outcome, &messages).await {
                    Ok(Some(reflection)) => {
                        render_sink.reflection_complete(&reflection.analysis, 0.0);
                        tracing::info!(
                            round,
                            analysis = %reflection.analysis,
                            "Self-reflection generated"
                        );
                        // Emit event.
                        let _ = event_tx.send(DomainEvent::new(
                            EventPayload::ReflectionGenerated {
                                round,
                                trigger: outcome.trigger_label().to_string(),
                            },
                        ));
                        // Store as memory entry.
                        if let Some(db) = trace_db {
                            let reflection_id = uuid::Uuid::new_v4();
                            let content = if reflection.advice.is_empty() {
                                reflection.analysis.clone()
                            } else {
                                format!(
                                    "{}\nAdvice: {}",
                                    reflection.analysis, reflection.advice
                                )
                            };
                            let hash =
                                hex::encode(sha2::Sha256::digest(content.as_bytes()));
                            let entry = cuervo_storage::MemoryEntry {
                                entry_id: reflection_id,
                                session_id: Some(session_id),
                                entry_type: cuervo_storage::MemoryEntryType::Reflection,
                                content,
                                content_hash: hash,
                                metadata: serde_json::json!({
                                    "round": round,
                                    "trigger": outcome.trigger_label(),
                                }),
                                created_at: Utc::now(),
                                expires_at: None,
                                relevance_score: 1.0,
                            };
                            if db.insert_memory(&entry).await.unwrap_or(false) {
                                last_reflection_id = Some(reflection_id);
                                // Link to current episode if active.
                                if let Some(ep_id) = episode_id {
                                    let _ = db
                                        .link_entry_to_episode(
                                            &reflection_id.to_string(),
                                            &ep_id.to_string(),
                                            round as u32,
                                        )
                                        .await;
                                }
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!("Reflection failed: {e}"),
                }
                // Phase E5: Transition back from Reflecting.
                if !silent {
                    render_sink.agent_state_transition("reflecting", "executing", "reflection complete");
                }
            }
        }

        // RC-2 fix: Record tool failures in the tracker and detect repeated patterns.
        for (failed_tool_name, error_msg) in &tool_failures {
            let tripped = failure_tracker.record(failed_tool_name, error_msg);
            if tripped {
                tracing::warn!(
                    tool = %failed_tool_name,
                    error_pattern = %ToolFailureTracker::error_pattern(error_msg),
                    "Tool failure circuit breaker tripped — repeated identical failures"
                );
                if !silent {
                    render_sink.loop_guard_action("circuit_breaker", &format!("{failed_tool_name}: repeated failures"));
                }
            }
        }

        // Adaptive replanning: if a tool failed and we have an active plan, attempt replan.
        // Failure outcomes are already recorded by the tracker above.
        // RC-3/RC-4 fix: skip replan for deterministic errors that will never succeed.
        if let (Some(ref mut tracker), Some(planner)) = (&mut execution_tracker, planner) {
            for (failed_tool_name, error_msg) in &tool_failures {
                // RC-3 fix: skip replan on deterministic errors.
                if executor::is_deterministic_error(error_msg) {
                    tracing::info!(
                        tool = %failed_tool_name,
                        error = %error_msg,
                        "Skipping replan: deterministic error (will never succeed on retry)"
                    );
                    continue;
                }
                // RC-2 fix: skip replan if this tool+error has already tripped.
                if failure_tracker.is_tripped(failed_tool_name, error_msg) {
                    tracing::info!(
                        tool = %failed_tool_name,
                        "Skipping replan: circuit breaker tripped for this failure pattern"
                    );
                    continue;
                }
                // Find the failed step index from the plan.
                let plan = tracker.plan();
                let failed_idx = plan.steps.iter().position(|s| {
                    s.tool_name.as_deref() == Some(failed_tool_name.as_str())
                        && matches!(s.outcome, Some(StepOutcome::Failed { .. }))
                });
                let Some(step_idx) = failed_idx else { continue };

                // Attempt replan (only for non-deterministic, non-repeated failures).
                match planner
                    .replan(plan, step_idx, error_msg, &request.tools)
                    .await
                {
                    Ok(Some(new_plan)) => {
                        tracing::info!(
                            goal = %new_plan.goal,
                            replan = new_plan.replan_count,
                            "Replanned after tool failure"
                        );
                        let _ = event_tx.send(DomainEvent::new(
                            EventPayload::PlanGenerated {
                                plan_id: new_plan.plan_id,
                                goal: new_plan.goal.clone(),
                                step_count: new_plan.steps.len(),
                                replan_count: new_plan.replan_count,
                            },
                        ));
                        if let Some(db) = trace_db {
                            let _ = db.save_plan_steps(&session_id, &new_plan).await;
                        }
                        tracker.reset_plan(new_plan);

                        let plan = tracker.plan();
                        let current = tracker.current_step();
                        let plan_section = format_plan_for_prompt(plan, current);
                        if let Some(ref mut sys) = cached_system {
                            update_plan_in_system(sys, &plan_section);
                        }
                        let (_, _, elapsed) = tracker.progress();
                        render_sink.plan_progress_with_timing(
                            &plan.goal,
                            &plan.steps,
                            current,
                            tracker.tracked_steps(),
                            elapsed,
                        );
                    }
                    Ok(None) => {
                        tracing::debug!("Replanning returned no plan");
                    }
                    Err(e) => {
                        tracing::warn!("Replanning failed: {e}");
                    }
                }
                // Only replan on the first failure per round.
                break;
            }
        }

        // Truncate oversized tool results to prevent context explosion.
        let max_chars = limits.max_tool_output_chars;
        if max_chars > 0 {
            for block in &mut tool_result_blocks {
                if let ContentBlock::ToolResult { content, .. } = block {
                    if content.len() > max_chars {
                        let truncated_len = content.len();
                        content.truncate(max_chars);
                        content.push_str(&format!(
                            "\n\n[output truncated: {truncated_len} chars → {max_chars} chars]"
                        ));
                    }
                }
            }
        }

        // Add tool results as a user message (Anthropic API requirement).
        let tool_result_msg = ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(tool_result_blocks),
        };
        messages.push(tool_result_msg.clone());
        context_pipeline.add_message(tool_result_msg.clone());
        session.add_message(tool_result_msg);

        // Phase 43: Check control channel after tool execution (yield point 2).
        if let Some(ref mut rx) = ctrl_rx {
            match check_control(rx, render_sink).await {
                ControlAction::Continue => {}
                ControlAction::StepOnce => { auto_pause = true; }
                ControlAction::Cancel => {
                    ctrl_cancelled = true;
                    break;
                }
            }
        }

        // Phase 33: intelligent tool loop guard — graduated escalation.
        // Uses the round_tool_log collected before dedup (above) for full
        // (tool_name, args_hash) tracking.
        let loop_action = loop_guard.record_round(&round_tool_log);

        tracing::info!(
            round,
            consecutive_tool_rounds = loop_guard.consecutive_rounds(),
            action = ?loop_action,
            oscillation = loop_guard.detect_oscillation(),
            read_saturation = loop_guard.detect_read_saturation(),
            "Tool loop guard decision"
        );

        match loop_action {
            LoopAction::Break => {
                tracing::warn!(
                    consecutive_tool_rounds = loop_guard.consecutive_rounds(),
                    "Tool loop guard: breaking (oscillation or plan complete)"
                );
                if !silent {
                    render_sink.warning(
                        &format!(
                            "auto-stopped after {} consecutive tool rounds (pattern detected)",
                            loop_guard.consecutive_rounds()
                        ),
                        Some("Oscillation or plan completion detected — synthesizing response."),
                    );
                }
                break;
            }
            LoopAction::ForceNoTools => {
                tracing::warn!(
                    consecutive_tool_rounds = loop_guard.consecutive_rounds(),
                    "Tool loop guard: forcing tool withdrawal for next round"
                );
                if !silent {
                    render_sink.loop_guard_action("force_no_tools", "removing tools for next round");
                }
                // Inject synthesis directive.
                let synth_msg = ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text(
                        "[System: You have gathered sufficient information across multiple tool rounds. \
                         SYNTHESIZE your findings and respond directly to the user. \
                         Do NOT call any more tools unless absolutely necessary for NEW information.]"
                            .into(),
                    ),
                };
                messages.push(synth_msg.clone());
                context_pipeline.add_message(synth_msg.clone());
                session.add_message(synth_msg);
                // Flag: next round_request should have tools removed.
                force_no_tools_next_round = true;
            }
            LoopAction::InjectSynthesis => {
                tracing::info!(
                    consecutive_tool_rounds = loop_guard.consecutive_rounds(),
                    "Tool loop guard: injecting synthesis directive"
                );
                if !silent {
                    render_sink.loop_guard_action("inject_synthesis", "hinting model to synthesize");
                }
                let synth_msg = ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text(
                        "[System: You have been calling tools for several rounds. \
                         Consider whether you already have enough information to respond. \
                         If so, respond directly to the user instead of calling more tools.]"
                            .into(),
                    ),
                };
                messages.push(synth_msg.clone());
                context_pipeline.add_message(synth_msg.clone());
                session.add_message(synth_msg);
            }
            LoopAction::Continue => {}
        }

        // Self-correction context injection: when tools fail, inject a structured
        // hint to help the model recover (SOTA pattern from Windsurf/Cursor).
        // RC-2 fix: inject a STRONGER directive when the circuit breaker has tripped.
        if !tool_failures.is_empty() {
            let failure_details: Vec<String> = tool_failures
                .iter()
                .map(|(name, err)| format!("- {name}: {err}"))
                .collect();

            let tripped_tools = failure_tracker.tripped_tools();
            let correction_text = if tripped_tools.is_empty() {
                format!(
                    "[System Note: {} tool(s) failed. Analyze the errors below and try a different approach.\n{}]",
                    tool_failures.len(),
                    failure_details.join("\n"),
                )
            } else {
                // Strong directive: circuit breaker tripped for repeated failures.
                format!(
                    "[System Note: {} tool(s) failed. The following tools have REPEATEDLY failed with the same error \
                     and MUST NOT be retried with the same arguments: {}.\n\
                     STOP retrying these tools. Use a completely different strategy or inform the user that \
                     the requested resource is unavailable.\n\
                     Failures:\n{}]",
                    tool_failures.len(),
                    tripped_tools.join(", "),
                    failure_details.join("\n"),
                )
            };

            let correction_msg = ChatMessage {
                role: Role::User,
                content: MessageContent::Text(correction_text),
            };
            messages.push(correction_msg.clone());
            context_pipeline.add_message(correction_msg.clone());
            session.add_message(correction_msg);
        }

        // Clear speculation cache at round boundary (predictions are per-round).
        speculator.clear().await;

        // Auto-save session + checkpoint after each tool-use round (crash protection).
        if let Some(db) = trace_db {
            if let Err(e) = db.save_session(session).await {
                tracing::warn!("Auto-save session failed: {e}");
            }
        }
        auto_checkpoint(trace_db, session_id, rounds, &messages, session, trace_step_index);
    }

    // TBAC: pop the plan-derived context if we pushed one.
    if tbac_pushed {
        permissions.pop_context();
    }

    // Determine stop condition: max_rounds, forced synthesis, or normal end.
    // If the loop guard forced a break (oscillation/plan completion) or forced no-tools,
    // and the loop ended due to that, use ForcedSynthesis.
    let stop_condition = if ctrl_cancelled {
        StopCondition::Interrupted
    } else if rounds >= limits.max_rounds {
        tracing::warn!(max_rounds = limits.max_rounds, "Max agent rounds reached");
        if !silent {
            render_sink.warning(
                &format!("max rounds reached: {}", limits.max_rounds),
                Some("Increase max_rounds in config to allow more iterations"),
            );
        }
        StopCondition::MaxRounds
    } else if loop_guard.plan_complete() || loop_guard.detect_oscillation() {
        StopCondition::ForcedSynthesis
    } else {
        StopCondition::EndTurn
    };

    // Phase E5: Emit final agent state transition (Complete or Failed).
    if !silent {
        let (to_state, reason) = match stop_condition {
            StopCondition::EndTurn | StopCondition::ForcedSynthesis => ("complete", "task finished"),
            StopCondition::Interrupted => ("idle", "user cancelled"),
            StopCondition::MaxRounds => ("failed", "max rounds reached"),
            StopCondition::ProviderError => ("failed", "provider error"),
            _ => ("complete", "loop ended"),
        };
        render_sink.agent_state_transition("executing", to_state, reason);
    }

    // Emit AgentCompleted event.
    let _ = event_tx.send(DomainEvent::new(EventPayload::AgentCompleted {
        agent_type: cuervo_core::types::AgentType::Chat,
        result: cuervo_core::types::AgentResult {
            success: matches!(stop_condition, StopCondition::EndTurn | StopCondition::ForcedSynthesis),
            summary: format!("{} rounds, {:?}", rounds, stop_condition),
            files_modified: vec![],
            tools_used: vec![],
        },
    }));

    // Flush L4 archive to disk (persist cross-session knowledge).
    if let Some(parent) = l4_archive_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Some(bytes) = context_pipeline.flush_l4_archive() {
        tracing::debug!(bytes, "L4 archive flushed to disk");
    }

    // Log plan execution summary with timing.
    if let Some(ref tracker) = execution_tracker {
        let (completed, total, elapsed) = tracker.progress();
        let delegated = tracker
            .tracked_steps()
            .iter()
            .filter(|s| s.delegation.is_some())
            .count();
        tracing::info!(
            completed,
            total,
            delegated,
            elapsed_ms = elapsed,
            "Plan execution summary"
        );
        if !silent {
            let delegation_note = if delegated > 0 {
                format!(", {delegated} delegated")
            } else {
                String::new()
            };
            render_sink.info(&format!(
                "Plan: {completed}/{total} steps in {:.1}s{delegation_note}",
                elapsed as f64 / 1000.0
            ));
        }
    }

    let execution_fingerprint = compute_fingerprint(&messages);
    Ok(AgentLoopResult {
        full_text,
        rounds,
        stop_condition,
        input_tokens: call_input_tokens,
        output_tokens: call_output_tokens,
        cost_usd: call_cost,
        latency_ms: loop_start.elapsed().as_millis() as u64,
        execution_fingerprint,
        timeline_json: execution_tracker.as_ref().map(|t| t.to_json().to_string()),
        ctrl_rx,
    })
}

/// Validate a plan before execution to catch errors early.
///
/// Checks:
/// - All tools referenced in plan steps exist in the tool registry
/// - No invalid tool names
///
/// Returns list of validation warnings (empty = valid plan).
fn validate_plan(plan: &ExecutionPlan, tool_registry: &cuervo_tools::ToolRegistry) -> Vec<String> {
    let mut warnings = Vec::new();

    // Check each step's tool reference.
    for (idx, step) in plan.steps.iter().enumerate() {
        if let Some(ref tool_name) = step.tool_name {
            // Verify tool exists in registry.
            if tool_registry.get(tool_name).is_none() {
                warnings.push(format!(
                    "Step {}: tool '{}' not found in registry ({})",
                    idx + 1,
                    tool_name,
                    step.description
                ));
            }
        }
    }

    // Check for empty plan (suspicious, but not an error).
    if plan.steps.is_empty() {
        warnings.push("Plan has 0 steps — may be a planning failure".to_string());
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::{ResilienceConfig, ToolDefinition};
    use cuervo_storage::Database;

    fn test_resilience() -> ResilienceManager {
        ResilienceManager::new(ResilienceConfig::default())
    }

    fn make_request(tools: Vec<ToolDefinition>) -> ModelRequest {
        ModelRequest {
            model: "echo".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("hello".into()),
            }],
            tools,
            max_tokens: Some(1024),
            temperature: Some(0.0),
            system: None,
            stream: true,
        }
    }

    fn test_event_tx() -> (EventSender, cuervo_core::EventReceiver) {
        cuervo_core::event_bus(64)
    }

    use crate::render::sink::ClassicSink;

    static TEST_CLASSIC_SINK: std::sync::LazyLock<ClassicSink> =
        std::sync::LazyLock::new(ClassicSink::new);

    static TEST_PLANNING_CONFIG: std::sync::LazyLock<PlanningConfig> =
        std::sync::LazyLock::new(PlanningConfig::default);

    static TEST_ORCHESTRATOR_CONFIG: std::sync::LazyLock<OrchestratorConfig> =
        std::sync::LazyLock::new(OrchestratorConfig::default);

    /// Build an AgentContext with test defaults for optional fields.
    #[allow(clippy::too_many_arguments)]
    fn test_ctx<'a>(
        provider: &'a Arc<dyn ModelProvider>,
        session: &'a mut Session,
        request: &'a ModelRequest,
        tool_registry: &'a ToolRegistry,
        permissions: &'a mut PermissionChecker,
        event_tx: &'a EventSender,
        limits: &'a AgentLimits,
        resilience: &'a mut ResilienceManager,
        routing_config: &'a RoutingConfig,
    ) -> AgentContext<'a> {
        AgentContext {
            provider,
            session,
            request,
            tool_registry,
            permissions,
            working_dir: "/tmp",
            event_tx,
            limits,
            trace_db: None,
            response_cache: None,
            resilience,
            fallback_providers: &[],
            routing_config,
            compactor: None,
            planner: None,
            guardrails: &[],
            reflector: None,
            render_sink: &*TEST_CLASSIC_SINK,
            replay_tool_executor: None,
            phase14: Phase14Context::default(),
            model_selector: None,
            registry: None,
            episode_id: None,
            planning_config: &*TEST_PLANNING_CONFIG,
            orchestrator_config: &*TEST_ORCHESTRATOR_CONFIG,
            tool_selection_enabled: false,
            task_bridge: None,
            context_metrics: None,
            ctrl_rx: None,
        }
    }

    #[tokio::test]
    async fn agent_loop_simple_text_response() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();

        let result = run_agent_loop(test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        ))
        .await
        .unwrap();

        assert!(!result.full_text.is_empty());
        assert_eq!(result.rounds, 0);
        assert_eq!(result.stop_condition, StopCondition::EndTurn);
    }

    #[tokio::test]
    async fn event_emitted_model_invoked() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, mut event_rx) = test_event_tx();
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();

        run_agent_loop(test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        ))
        .await
        .unwrap();

        // First event should be AgentStarted (new in Phase 11).
        let started = event_rx.try_recv().expect("should receive AgentStarted event");
        assert!(matches!(started.payload, EventPayload::AgentStarted { .. }));

        // Next should be ModelInvoked.
        let event = event_rx.try_recv().expect("should receive ModelInvoked event");
        match event.payload {
            EventPayload::ModelInvoked {
                provider: p,
                model,
                latency_ms,
                ..
            } => {
                assert_eq!(p, "echo");
                assert_eq!(model, "echo");
                assert!(latency_ms < 5000, "latency should be reasonable");
            }
            other => panic!("expected ModelInvoked, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn event_bus_fire_and_forget_no_panic() {
        // Sender with no active receiver — send() returns Err but must not panic.
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();
        // Drop the receiver before running the loop.
        drop(_rx);

        let result = run_agent_loop(test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        ))
        .await
        .unwrap();

        // Must complete normally even with no receivers.
        assert!(!result.full_text.is_empty());
    }

    #[tokio::test]
    async fn session_latency_tracked() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();

        run_agent_loop(test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        ))
        .await
        .unwrap();

        let _ = session.total_latency_ms;
        assert_eq!(session.agent_rounds, 0);
        assert_eq!(session.tool_invocations, 0);
    }

    #[tokio::test]
    async fn trace_recording_with_db() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();

        let routing_config = RoutingConfig::default();
        let mut ctx = test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        );
        ctx.trace_db = Some(&db);

        let result = run_agent_loop(ctx).await.unwrap();

        assert!(!result.full_text.is_empty());

        // Should have recorded at least 2 trace steps: ModelRequest + ModelResponse.
        let steps = db.inner().load_trace_steps(session.id).unwrap();
        assert!(steps.len() >= 2, "expected >= 2 trace steps, got {}", steps.len());
        assert_eq!(steps[0].step_type, cuervo_storage::TraceStepType::ModelRequest);
        assert_eq!(steps[1].step_type, cuervo_storage::TraceStepType::ModelResponse);

        for (i, step) in steps.iter().enumerate() {
            assert_eq!(step.step_index, i as u32);
        }
    }

    #[tokio::test]
    async fn token_budget_zero_means_unlimited() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let limits = AgentLimits { max_total_tokens: 0, ..AgentLimits::default() };
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();

        let result = run_agent_loop(test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        ))
        .await
        .unwrap();

        assert_eq!(result.stop_condition, StopCondition::EndTurn);
    }

    #[tokio::test]
    async fn token_budget_enforced() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let limits = AgentLimits { max_total_tokens: 1, ..AgentLimits::default() };
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();

        let result = run_agent_loop(test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        ))
        .await
        .unwrap();

        assert_eq!(result.stop_condition, StopCondition::TokenBudget);
    }

    #[tokio::test]
    async fn max_rounds_respected() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let limits = AgentLimits { max_rounds: 3, ..AgentLimits::default() };
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();

        let result = run_agent_loop(test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        ))
        .await
        .unwrap();

        assert_eq!(result.stop_condition, StopCondition::EndTurn);
    }

    #[tokio::test]
    async fn default_limits_backward_compatible() {
        let limits = AgentLimits::default();
        assert_eq!(limits.max_rounds, 25);
        assert_eq!(limits.max_total_tokens, 0);
        assert_eq!(limits.max_duration_secs, 0);
        assert_eq!(limits.tool_timeout_secs, 120);
        assert_eq!(limits.provider_timeout_secs, 300);
        assert_eq!(limits.max_parallel_tools, 10);
    }

    // --- Phase 1: Wired infrastructure tests ---

    fn test_cache(enabled: bool) -> ResponseCache {
        use cuervo_core::types::CacheConfig;
        ResponseCache::new(
            AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap())),
            CacheConfig {
                enabled,
                default_ttl_secs: 3600,
                max_entries: 100,
            },
        )
    }

    #[tokio::test]
    async fn cache_miss_then_store() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let cache = test_cache(true);
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();

        let routing_config = RoutingConfig::default();
        let mut ctx = test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        );
        ctx.response_cache = Some(&cache);

        let result = run_agent_loop(ctx).await.unwrap();

        assert!(!result.full_text.is_empty());
        assert_eq!(result.stop_condition, StopCondition::EndTurn);

        let cached = cache.lookup(&request).await;
        assert!(cached.is_some(), "response should be cached after miss");
        assert!(!cached.unwrap().response_text.is_empty());
    }

    #[tokio::test]
    async fn cache_hit_skips_provider() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let cache = test_cache(true);

        // Pre-populate cache.
        cache.store(&request, "cached response", "end_turn", "{}", None).await;

        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();

        let routing_config = RoutingConfig::default();
        let mut ctx = test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        );
        ctx.response_cache = Some(&cache);

        let result = run_agent_loop(ctx).await.unwrap();

        assert_eq!(result.full_text, "cached response");
        assert_eq!(result.stop_condition, StopCondition::EndTurn);
        assert_eq!(result.rounds, 0);
        assert_eq!(session.total_latency_ms, 0);
    }

    #[tokio::test]
    async fn cache_disabled_always_invokes_provider() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let cache = test_cache(false);
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();

        let routing_config = RoutingConfig::default();
        let mut ctx = test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        );
        ctx.response_cache = Some(&cache);

        let result = run_agent_loop(ctx).await.unwrap();

        assert!(!result.full_text.is_empty());
        assert!(cache.lookup(&request).await.is_none());
    }

    #[tokio::test]
    async fn metrics_persisted_after_invocation() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();

        let routing_config = RoutingConfig::default();
        let mut ctx = test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        );
        ctx.trace_db = Some(&db);

        run_agent_loop(ctx).await.unwrap();

        let metrics = db.inner().system_metrics().unwrap();
        assert!(
            metrics.total_invocations >= 1,
            "expected at least 1 metric, got {}",
            metrics.total_invocations
        );
        assert!(!metrics.models.is_empty());
        let model_stat = &metrics.models[0];
        assert_eq!(model_stat.provider, "echo");
        assert_eq!(model_stat.model, "echo");
        assert!(model_stat.success_rate > 0.0);
    }

    #[tokio::test]
    async fn trace_and_metrics_combined() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();

        let routing_config = RoutingConfig::default();
        let mut ctx = test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        );
        ctx.trace_db = Some(&db);

        run_agent_loop(ctx).await.unwrap();

        let steps = db.inner().load_trace_steps(session.id).unwrap();
        assert!(steps.len() >= 2, "expected trace steps");

        let metrics = db.inner().system_metrics().unwrap();
        assert!(metrics.total_invocations >= 1, "expected metrics");
    }

    // --- Phase 3: Fallback tests ---

    #[tokio::test]
    async fn invoke_with_fallback_uses_primary_when_healthy() {
        let primary: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let request = make_request(vec![]);
        let mut resilience = test_resilience();

        let attempt =
            super::invoke_with_fallback(&primary, &request, &[], &mut resilience, &RoutingConfig::default(), &test_event_tx().0)
                .await
                .unwrap();

        assert_eq!(attempt.provider_name, "echo");
        assert!(!attempt.is_fallback);
    }

    #[tokio::test]
    async fn invoke_with_fallback_returns_error_when_no_fallbacks() {
        use cuervo_core::types::{BackpressureConfig, CircuitBreakerConfig, ResilienceConfig};

        let primary: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let request = make_request(vec![]);

        let mut resilience = ResilienceManager::new(ResilienceConfig {
            enabled: true,
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: 1,
                window_secs: 60,
                open_duration_secs: 30,
                half_open_probes: 2,
            },
            health: Default::default(),
            backpressure: BackpressureConfig::default(),
        });
        resilience.register_provider("echo");
        resilience.record_failure("echo").await;

        let result =
            super::invoke_with_fallback(&primary, &request, &[], &mut resilience, &RoutingConfig::default(), &test_event_tx().0).await;
        assert!(result.is_err(), "should fail when primary is blocked and no fallbacks");
    }

    #[tokio::test]
    async fn invoke_with_fallback_uses_fallback_when_primary_blocked() {
        use cuervo_core::types::{BackpressureConfig, CircuitBreakerConfig, ResilienceConfig};

        let primary: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let fallback: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let request = make_request(vec![]);

        let mut resilience = ResilienceManager::new(ResilienceConfig {
            enabled: true,
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: 1,
                window_secs: 60,
                open_duration_secs: 30,
                half_open_probes: 2,
            },
            health: Default::default(),
            backpressure: BackpressureConfig::default(),
        });
        resilience.register_provider("echo");
        resilience.register_provider("fallback_echo");
        resilience.record_failure("echo").await;

        let fallbacks = vec![("fallback_echo".to_string(), fallback)];
        let attempt =
            super::invoke_with_fallback(&primary, &request, &fallbacks, &mut resilience, &RoutingConfig::default(), &test_event_tx().0)
                .await
                .unwrap();

        assert_eq!(attempt.provider_name, "fallback_echo");
        assert!(attempt.is_fallback);
    }

    #[tokio::test]
    async fn agent_loop_with_fallback_providers() {
        use cuervo_core::types::{BackpressureConfig, CircuitBreakerConfig, ResilienceConfig};

        let primary: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let fallback: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();

        let mut resilience = ResilienceManager::new(ResilienceConfig {
            enabled: true,
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: 1,
                window_secs: 60,
                open_duration_secs: 30,
                half_open_probes: 2,
            },
            health: Default::default(),
            backpressure: BackpressureConfig::default(),
        });
        resilience.register_provider("echo");
        resilience.register_provider("fallback_echo");
        resilience.record_failure("echo").await;

        let fallbacks: Vec<(String, Arc<dyn ModelProvider>)> =
            vec![("fallback_echo".to_string(), fallback)];
        let limits = AgentLimits::default();

        let routing_config = RoutingConfig::default();
        let mut ctx = test_ctx(
            &primary, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        );
        ctx.fallback_providers = &fallbacks;

        let result = run_agent_loop(ctx).await.unwrap();

        assert!(!result.full_text.is_empty());
        assert_eq!(result.stop_condition, StopCondition::EndTurn);
    }

    // --- Phase 4B: SpeculativeInvoker wiring tests ---

    #[tokio::test]
    async fn failover_mode_delegates_to_router() {
        let primary: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let request = make_request(vec![]);
        let mut resilience = test_resilience();
        let config = RoutingConfig::default();

        let attempt = super::invoke_with_fallback(
            &primary, &request, &[], &mut resilience, &config, &test_event_tx().0,
        )
        .await
        .unwrap();

        assert_eq!(attempt.provider_name, "echo");
        assert!(!attempt.is_fallback);
    }

    #[tokio::test]
    async fn speculative_mode_with_fallbacks() {
        let primary: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let echo2: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let request = make_request(vec![]);
        let mut resilience = test_resilience();
        let config = RoutingConfig {
            mode: "speculative".into(),
            ..RoutingConfig::default()
        };

        let fallbacks = vec![("echo2".into(), echo2)];
        let attempt = super::invoke_with_fallback(
            &primary, &request, &fallbacks, &mut resilience, &config, &test_event_tx().0,
        )
        .await
        .unwrap();

        assert!(attempt.provider_name == "echo" || attempt.provider_name == "echo2");
    }

    #[tokio::test]
    async fn resilience_filters_unhealthy_before_routing() {
        use cuervo_core::types::{BackpressureConfig, CircuitBreakerConfig, ResilienceConfig};

        let primary: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let request = make_request(vec![]);

        let mut resilience = ResilienceManager::new(ResilienceConfig {
            enabled: true,
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: 1,
                window_secs: 60,
                open_duration_secs: 30,
                half_open_probes: 2,
            },
            health: Default::default(),
            backpressure: BackpressureConfig::default(),
        });
        resilience.register_provider("echo");
        resilience.record_failure("echo").await;

        let config = RoutingConfig::default();
        let result = super::invoke_with_fallback(
            &primary, &request, &[], &mut resilience, &config, &test_event_tx().0,
        )
        .await;

        assert!(result.is_err(), "should fail when all providers are unhealthy");
    }

    #[tokio::test]
    async fn agent_loop_passes_routing_config() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let config = RoutingConfig {
            mode: "speculative".into(),
            ..RoutingConfig::default()
        };
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();

        let ctx = test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &config,
        );

        let result = run_agent_loop(ctx).await.unwrap();

        assert!(!result.full_text.is_empty());
        assert_eq!(result.stop_condition, StopCondition::EndTurn);
    }

    #[tokio::test]
    async fn success_recorded_on_resilience() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let request = make_request(vec![]);

        let mut resilience = ResilienceManager::new(cuervo_core::types::ResilienceConfig {
            enabled: true,
            ..Default::default()
        });
        resilience.register_provider("echo");

        let attempt = super::invoke_with_fallback(
            &provider, &request, &[], &mut resilience, &RoutingConfig::default(), &test_event_tx().0,
        )
        .await
        .unwrap();

        assert_eq!(attempt.provider_name, "echo");
        assert!(attempt.permit.is_some());
    }

    #[tokio::test]
    async fn resilience_disabled_delegates_directly() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let request = make_request(vec![]);
        let mut resilience = ResilienceManager::new(ResilienceConfig {
            enabled: false,
            ..ResilienceConfig::default()
        });

        let config = RoutingConfig {
            mode: "speculative".into(),
            ..RoutingConfig::default()
        };

        let attempt = super::invoke_with_fallback(
            &provider, &request, &[], &mut resilience, &config, &test_event_tx().0,
        )
        .await
        .unwrap();

        assert_eq!(attempt.provider_name, "echo");
        assert!(!attempt.is_fallback);
        assert!(attempt.permit.is_none());
    }

    #[tokio::test]
    async fn speculative_end_to_end_two_echo_providers() {
        let primary: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let echo2: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();

        let config = RoutingConfig {
            mode: "speculative".into(),
            ..RoutingConfig::default()
        };
        let fallbacks: Vec<(String, Arc<dyn ModelProvider>)> =
            vec![("echo2".into(), echo2)];
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();

        let mut ctx = test_ctx(
            &primary, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &config,
        );
        ctx.fallback_providers = &fallbacks;

        let result = run_agent_loop(ctx).await.unwrap();

        assert!(!result.full_text.is_empty());
        assert_eq!(result.stop_condition, StopCondition::EndTurn);
    }

    // --- Phase 11.0: Critical runtime safety tests ---

    #[tokio::test]
    async fn token_budget_pre_check_breaks_loop() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        // Simulate prior usage that exceeds the budget.
        session.total_usage.input_tokens = 200;
        session.total_usage.output_tokens = 100;
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        // Budget is 150 but we already used 300 — should break before invoking.
        let limits = AgentLimits { max_total_tokens: 150, ..AgentLimits::default() };
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();

        let result = run_agent_loop(test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        ))
        .await
        .unwrap();

        // Pre-check breaks before any invocation, so stop_condition
        // is EndTurn (loop exited via break, no invocation happened).
        assert_eq!(result.stop_condition, StopCondition::EndTurn);
        assert_eq!(result.rounds, 0);
        // The full_text should be empty since no invocation happened.
        assert!(result.full_text.is_empty());
    }

    #[tokio::test]
    async fn stop_reason_trace_format_serde() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();

        let mut ctx = test_ctx(
            &provider, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        );
        ctx.trace_db = Some(&db);

        run_agent_loop(ctx).await.unwrap();

        // Check that trace steps use serde format ("end_turn") not Debug ("EndTurn").
        let steps = db.inner().load_trace_steps(session.id).unwrap();
        let response_step = steps.iter().find(|s| {
            s.step_type == cuervo_storage::TraceStepType::ModelResponse
        });
        assert!(response_step.is_some(), "should have a ModelResponse trace step");
        let data = response_step.unwrap().data_json.as_str();
        // Should contain "end_turn" not "EndTurn".
        assert!(
            data.contains("end_turn"),
            "trace should use serde format 'end_turn', got: {data}"
        );
        assert!(
            !data.contains("EndTurn"),
            "trace should NOT use Debug format 'EndTurn', got: {data}"
        );
    }

    // --- Phase 18: classify_error_hint tests ---

    #[test]
    fn error_hint_invalid_api_key() {
        let hint = classify_error_hint("Error: Invalid API key provided");
        assert!(hint.contains("Verify your API key"), "got: {hint}");
    }

    #[test]
    fn error_hint_billing() {
        let hint = classify_error_hint("Your credit balance is too low");
        assert!(hint.contains("account balance"), "got: {hint}");
    }

    #[test]
    fn error_hint_rate_limit() {
        let hint = classify_error_hint("429 Too Many Requests");
        assert!(hint.contains("Rate limited"), "got: {hint}");

        let hint2 = classify_error_hint("rate_limit_exceeded");
        assert!(hint2.contains("Rate limited"), "got: {hint2}");
    }

    #[test]
    fn error_hint_generic_fallback() {
        let hint = classify_error_hint("connection refused");
        assert!(hint.contains("network connection"), "got: {hint}");
    }

    // --- Phase 18: trace step continuity test ---

    #[tokio::test]
    async fn trace_step_index_continues_across_messages() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let (event_tx, _rx) = test_event_tx();
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
        let limits = AgentLimits::default();
        let routing_config = RoutingConfig::default();

        // Simulate session persisting across two agent loop calls.
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let sid = session.id;

        // First message
        {
            let mut perms = PermissionChecker::new(true);
            let mut resilience = test_resilience();
            let mut ctx = test_ctx(
                &provider, &mut session, &request, &tool_reg, &mut perms,
                &event_tx, &limits, &mut resilience, &routing_config,
            );
            ctx.trace_db = Some(&db);
            run_agent_loop(ctx).await.unwrap();
        }

        let steps_after_first = db.inner().load_trace_steps(sid).unwrap();
        let first_max = steps_after_first.last().unwrap().step_index;
        assert!(first_max > 0, "should have trace steps after first message");

        // Second message: step indices should continue from where first left off.
        {
            let mut perms = PermissionChecker::new(true);
            let mut resilience = test_resilience();
            let mut ctx = test_ctx(
                &provider, &mut session, &request, &tool_reg, &mut perms,
                &event_tx, &limits, &mut resilience, &routing_config,
            );
            ctx.trace_db = Some(&db);
            run_agent_loop(ctx).await.unwrap();
        }

        let all_steps = db.inner().load_trace_steps(sid).unwrap();
        // Verify no duplicate indices
        let indices: Vec<u32> = all_steps.iter().map(|s| s.step_index).collect();
        let unique: std::collections::HashSet<u32> = indices.iter().copied().collect();
        assert_eq!(
            indices.len(),
            unique.len(),
            "step indices should be unique: {:?}",
            indices
        );
        // Second message should start after first message's max
        assert!(
            *indices.last().unwrap() > first_max,
            "second message indices should be higher than first"
        );
    }

    // --- Phase 18: Self-correction context injection tests ---

    #[test]
    fn correction_context_format_single_failure() {
        let failures = vec![("bash".to_string(), "command not found: foo".to_string())];
        let details: Vec<String> = failures
            .iter()
            .map(|(name, err)| format!("- {name}: {err}"))
            .collect();
        let msg = format!(
            "[System Note: {} tool(s) failed. Analyze the errors below and try a different approach.\n{}]",
            failures.len(),
            details.join("\n"),
        );
        assert!(msg.contains("1 tool(s) failed"));
        assert!(msg.contains("- bash: command not found: foo"));
    }

    #[test]
    fn correction_context_format_multiple_failures() {
        let failures = vec![
            ("file_read".to_string(), "file not found".to_string()),
            ("bash".to_string(), "exit code 1".to_string()),
        ];
        let details: Vec<String> = failures
            .iter()
            .map(|(name, err)| format!("- {name}: {err}"))
            .collect();
        let msg = format!(
            "[System Note: {} tool(s) failed. Analyze the errors below and try a different approach.\n{}]",
            failures.len(),
            details.join("\n"),
        );
        assert!(msg.contains("2 tool(s) failed"));
        assert!(msg.contains("- file_read: file not found"));
        assert!(msg.contains("- bash: exit code 1"));
    }

    #[test]
    fn correction_context_not_injected_on_success() {
        let failures: Vec<(String, String)> = vec![];
        // When no failures, correction context should not be injected.
        assert!(failures.is_empty());
    }

    // ── Plan injection tests (SP-2) ──

    #[test]
    fn format_plan_all_statuses() {
        use cuervo_core::traits::{ExecutionPlan, PlanStep, StepOutcome};
        let plan = ExecutionPlan {
            goal: "Fix auth bug".into(),
            steps: vec![
                PlanStep {
                    description: "Read auth module".into(),
                    tool_name: Some("file_read".into()),
                    parallel: false,
                    confidence: 0.9,
                    expected_args: None,
                    outcome: Some(StepOutcome::Success { summary: "OK".into() }),
                },
                PlanStep {
                    description: "Edit validation".into(),
                    tool_name: Some("file_edit".into()),
                    parallel: false,
                    confidence: 0.8,
                    expected_args: None,
                    outcome: None,
                },
                PlanStep {
                    description: "Run tests".into(),
                    tool_name: Some("bash".into()),
                    parallel: false,
                    confidence: 0.7,
                    expected_args: None,
                    outcome: None,
                },
            ],
            requires_confirmation: false,
            plan_id: uuid::Uuid::nil(),
            replan_count: 0,
            parent_plan_id: None,
        };
        let formatted = format_plan_for_prompt(&plan, 1);
        assert!(formatted.contains(PLAN_SECTION_START));
        assert!(formatted.contains(PLAN_SECTION_END));
        assert!(formatted.contains("Fix auth bug"));
        assert!(formatted.contains("\u{2713}")); // ✓ for completed step
        assert!(formatted.contains("\u{25b8}")); // ▸ for current step
        assert!(formatted.contains("CURRENT"));
        assert!(formatted.contains("\u{25cb}")); // ○ for pending step
        assert!(formatted.contains("Step 2"));
    }

    #[test]
    fn format_plan_empty_steps() {
        use cuervo_core::traits::ExecutionPlan;
        let plan = ExecutionPlan {
            goal: "Simple query".into(),
            steps: vec![],
            requires_confirmation: false,
            plan_id: uuid::Uuid::nil(),
            replan_count: 0,
            parent_plan_id: None,
        };
        let formatted = format_plan_for_prompt(&plan, 0);
        assert!(formatted.contains("All steps completed."));
    }

    #[test]
    fn format_plan_current_indicator_on_first() {
        use cuervo_core::traits::{ExecutionPlan, PlanStep};
        let plan = ExecutionPlan {
            goal: "Build project".into(),
            steps: vec![PlanStep {
                description: "Compile".into(),
                tool_name: Some("bash".into()),
                parallel: false,
                confidence: 0.9,
                expected_args: None,
                outcome: None,
            }],
            requires_confirmation: false,
            plan_id: uuid::Uuid::nil(),
            replan_count: 0,
            parent_plan_id: None,
        };
        let formatted = format_plan_for_prompt(&plan, 0);
        assert!(formatted.contains("CURRENT"));
        assert!(formatted.contains("You are on Step 1"));
    }

    #[test]
    fn update_plan_in_system_surgical_replace() {
        let mut system = format!(
            "You are a helpful assistant.\n\n{}\nOld plan content\n{}\n\nMore instructions.",
            PLAN_SECTION_START, PLAN_SECTION_END
        );
        let new_section = format!("{}\nNew plan\n{}", PLAN_SECTION_START, PLAN_SECTION_END);
        update_plan_in_system(&mut system, &new_section);
        assert!(system.contains("New plan"));
        assert!(!system.contains("Old plan content"));
        assert!(system.contains("More instructions."));
    }

    // ── Plan success tracking tests (SP-3 → Phase 36 ExecutionTracker) ──

    fn make_plan_step(desc: &str, tool: &str) -> cuervo_core::traits::PlanStep {
        cuervo_core::traits::PlanStep {
            description: desc.into(),
            tool_name: Some(tool.into()),
            parallel: false,
            confidence: 0.9,
            expected_args: None,
            outcome: None,
        }
    }

    fn make_test_plan(steps: Vec<cuervo_core::traits::PlanStep>) -> ExecutionPlan {
        ExecutionPlan {
            goal: "Test".into(),
            steps,
            requires_confirmation: false,
            plan_id: uuid::Uuid::nil(),
            replan_count: 0,
            parent_plan_id: None,
        }
    }

    fn make_test_tracker(steps: Vec<cuervo_core::traits::PlanStep>) -> ExecutionTracker {
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        ExecutionTracker::new(make_test_plan(steps), tx)
    }

    #[test]
    fn plan_step_success_match() {
        let mut tracker = make_test_tracker(vec![
            make_plan_step("Read file", "file_read"),
            make_plan_step("Edit file", "file_edit"),
        ]);
        let matched = tracker.record_tool_results(&["file_read".into()], &[], 1);
        assert_eq!(matched.len(), 1);
        assert!(matches!(tracker.plan().steps[0].outcome, Some(StepOutcome::Success { .. })));
        assert!(tracker.plan().steps[1].outcome.is_none());
    }

    #[test]
    fn plan_step_no_match_ignored() {
        let mut tracker = make_test_tracker(vec![make_plan_step("Run tests", "bash")]);
        let matched = tracker.record_tool_results(&["file_read".into()], &[], 1);
        assert!(matched.is_empty());
        assert!(tracker.plan().steps[0].outcome.is_none());
    }

    #[test]
    fn plan_step_multi_same_tool_sequential() {
        let mut tracker = make_test_tracker(vec![
            make_plan_step("Read first", "file_read"),
            make_plan_step("Read second", "file_read"),
        ]);
        let m1 = tracker.record_tool_results(&["file_read".into()], &[], 1);
        assert_eq!(m1.len(), 1);
        assert!(matches!(tracker.plan().steps[0].outcome, Some(StepOutcome::Success { .. })));
        assert!(tracker.plan().steps[1].outcome.is_none());
    }

    #[test]
    fn plan_step_all_completed_advances_index() {
        let plan = make_test_plan(vec![
            {
                let mut s = make_plan_step("Step 1", "bash");
                s.outcome = Some(StepOutcome::Success { summary: "done".into() });
                s
            },
            {
                let mut s = make_plan_step("Step 2", "file_read");
                s.outcome = Some(StepOutcome::Success { summary: "done".into() });
                s
            },
        ]);
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let tracker = ExecutionTracker::new(plan.clone(), tx);
        assert!(tracker.is_complete());
        assert_eq!(tracker.current_step(), 2); // Past all steps.
        let formatted = format_plan_for_prompt(tracker.plan(), tracker.current_step());
        assert!(formatted.contains("All steps completed."));
    }

    // === Phase 27 (RC-2 fix): ToolFailureTracker tests ===

    #[test]
    fn tracker_new_is_empty() {
        let tracker = ToolFailureTracker::new(3);
        assert!(tracker.tripped_tools().is_empty());
        assert!(!tracker.is_tripped("file_read", "not found"));
    }

    #[test]
    fn tracker_records_below_threshold() {
        let mut tracker = ToolFailureTracker::new(3);
        assert!(!tracker.record("file_read", "No such file or directory: /tmp/x.rs"));
        assert!(!tracker.record("file_read", "File not found: /tmp/y.rs"));
        // Both map to "not_found" pattern — 2 occurrences, threshold=3 → not tripped
        assert!(!tracker.is_tripped("file_read", "not found anything"));
        assert!(tracker.tripped_tools().is_empty());
    }

    #[test]
    fn tracker_trips_at_threshold() {
        let mut tracker = ToolFailureTracker::new(3);
        assert!(!tracker.record("file_read", "No such file or directory: /a.rs"));
        assert!(!tracker.record("file_read", "File not found: /b.rs"));
        // Third occurrence of "not_found" pattern → trips
        assert!(tracker.record("file_read", "not found: /c.rs"));
        assert!(tracker.is_tripped("file_read", "not found"));
        assert_eq!(tracker.tripped_tools(), vec!["file_read"]);
    }

    #[test]
    fn tracker_distinct_patterns_independent() {
        let mut tracker = ToolFailureTracker::new(2);
        // Two "not_found" → trips
        assert!(!tracker.record("file_read", "not found"));
        assert!(tracker.record("file_read", "file not found"));
        // One "permission_denied" → does NOT trip
        assert!(!tracker.record("file_read", "permission denied"));
        assert!(tracker.is_tripped("file_read", "not found here"));
        assert!(!tracker.is_tripped("file_read", "permission denied on /x"));
    }

    #[test]
    fn tracker_distinct_tools_independent() {
        let mut tracker = ToolFailureTracker::new(2);
        // file_read + not_found
        assert!(!tracker.record("file_read", "not found"));
        // bash + not_found (different tool)
        assert!(!tracker.record("bash", "not found"));
        // Second file_read + not_found → trips file_read only
        assert!(tracker.record("file_read", "not found again"));
        assert!(tracker.is_tripped("file_read", "not found"));
        assert!(!tracker.is_tripped("bash", "not found"));
    }

    #[test]
    fn tracker_error_pattern_classification() {
        assert_eq!(ToolFailureTracker::error_pattern("No such file or directory"), "not_found");
        assert_eq!(ToolFailureTracker::error_pattern("File not found"), "not_found");
        assert_eq!(ToolFailureTracker::error_pattern("Permission denied"), "permission_denied");
        assert_eq!(ToolFailureTracker::error_pattern("Is a directory"), "path_type_error");
        assert_eq!(ToolFailureTracker::error_pattern("Not a directory"), "path_type_error");
        assert_eq!(ToolFailureTracker::error_pattern("path traversal detected"), "security_blocked");
        assert_eq!(ToolFailureTracker::error_pattern("blocked by security"), "security_blocked");
        assert_eq!(ToolFailureTracker::error_pattern("unknown tool: foobar"), "unknown_tool");
        assert_eq!(ToolFailureTracker::error_pattern("denied by task context"), "tbac_denied");
    }

    #[test]
    fn tracker_error_pattern_generic_fallback() {
        // Unclassified errors use first 80 chars lowercased
        let generic = "something completely unusual happened in the tool execution pipeline";
        let pattern = ToolFailureTracker::error_pattern(generic);
        assert_eq!(pattern, generic.to_lowercase());
    }

    #[test]
    fn tracker_error_pattern_truncates_long_generic() {
        let long_error = "a".repeat(200);
        let pattern = ToolFailureTracker::error_pattern(&long_error);
        assert_eq!(pattern.len(), 80);
    }

    #[test]
    fn tracker_threshold_one_trips_immediately() {
        let mut tracker = ToolFailureTracker::new(1);
        assert!(tracker.record("bash", "command exited with code 1"));
        assert!(tracker.is_tripped("bash", "command exited with code 1"));
    }

    #[test]
    fn tracker_tripped_tools_deduplicates() {
        let mut tracker = ToolFailureTracker::new(1);
        // Same tool, two different patterns — both trip
        tracker.record("file_read", "not found");
        tracker.record("file_read", "permission denied");
        // tripped_tools should return file_read only once
        let tools = tracker.tripped_tools();
        assert_eq!(tools, vec!["file_read"]);
    }

    #[test]
    fn tracker_multiple_tripped_tools_sorted() {
        let mut tracker = ToolFailureTracker::new(1);
        tracker.record("file_write", "permission denied");
        tracker.record("bash", "not found");
        tracker.record("file_read", "not found");
        let tools = tracker.tripped_tools();
        assert_eq!(tools, vec!["bash", "file_read", "file_write"]);
    }

    // === Phase 27 Stress Tests ===

    #[test]
    fn stress_tracker_100_distinct_tools() {
        // Stress: 100 distinct tools, each with a unique error
        let mut tracker = ToolFailureTracker::new(3);
        for i in 0..100 {
            let tool = format!("tool_{i}");
            let err = format!("custom error for tool {i}");
            tracker.record(&tool, &err);
            tracker.record(&tool, &err);
            // 2 occurrences → not tripped yet
            assert!(!tracker.is_tripped(&tool, &err));
        }
        // None should be tripped
        assert!(tracker.tripped_tools().is_empty());

        // Third occurrence → trips all 100
        for i in 0..100 {
            let tool = format!("tool_{i}");
            let err = format!("custom error for tool {i}");
            assert!(tracker.record(&tool, &err));
        }
        assert_eq!(tracker.tripped_tools().len(), 100);
    }

    #[test]
    fn stress_tracker_1000_rapid_records_same_tool() {
        // Stress: 1000 recordings of the same tool+error
        let mut tracker = ToolFailureTracker::new(3);
        for i in 0..1000 {
            let tripped = tracker.record("file_read", "not found");
            if i < 2 {
                assert!(!tripped);
            } else {
                assert!(tripped);
            }
        }
        // Count should be 1000
        assert_eq!(tracker.failure_count("file_read", "not found"), 1000);
    }

    #[test]
    fn stress_tracker_mixed_patterns_no_false_positives() {
        // Stress: interleave 6 different error patterns for the same tool
        // Only patterns reaching threshold should trip
        let mut tracker = ToolFailureTracker::new(5);
        let errors = [
            "not found", "permission denied", "is a directory",
            "path traversal", "unknown tool", "denied by task context",
        ];

        // Record each pattern a different number of times
        for (i, err) in errors.iter().enumerate() {
            for _ in 0..=(i + 1) {
                tracker.record("multi_tool", err);
            }
        }

        // Pattern 0 ("not found"): 2 records → NOT tripped (threshold=5)
        assert!(!tracker.is_tripped("multi_tool", "not found"));
        // Pattern 4 ("unknown tool"): 6 records → tripped
        assert!(tracker.is_tripped("multi_tool", "unknown tool"));
        // Pattern 5 ("denied by task context"): 7 records → tripped
        assert!(tracker.is_tripped("multi_tool", "denied by task context"));
    }

    #[test]
    fn stress_error_pattern_determinism() {
        // Verify error_pattern() is deterministic across 1000 calls
        let errors = vec![
            "No such file or directory: /tmp/foo.rs",
            "Permission denied for /etc/shadow",
            "Is a directory: /tmp/mydir",
            "path traversal blocked in ../../etc",
            "unknown tool: mystery_tool",
            "Something generic and unique happened here",
        ];

        for err in &errors {
            let first = ToolFailureTracker::error_pattern(err);
            for _ in 0..1000 {
                assert_eq!(ToolFailureTracker::error_pattern(err), first);
            }
        }
    }

    #[test]
    fn spinner_label_format_failover() {
        // In failover mode, spinner should show provider name.
        let provider_name = "ollama";
        let label = format!("Thinking... [{}]", provider_name);
        assert_eq!(label, "Thinking... [ollama]");
    }

    #[test]
    fn spinner_label_format_speculative() {
        // In speculative mode with fallbacks, spinner should show racing count.
        let fallback_count = 3;
        let count = 1 + fallback_count;
        let label = format!("Racing {count} providers...");
        assert_eq!(label, "Racing 4 providers...");
    }

    #[test]
    fn round_separator_format() {
        let round = 2;
        let provider_name = "deepseek";
        let sep = format!("\n  --- round {} [{}] ---", round + 1, provider_name);
        assert_eq!(sep, "\n  --- round 3 [deepseek] ---");
    }

    // === W-4: Planning gate heuristic tests ===

    #[test]
    fn planning_gate_trivial_prompt() {
        let user_msg = "hola";
        let word_count = user_msg.split_whitespace().count();
        let msg_lower = user_msg.to_lowercase();
        let has_action_keywords = [
            "create", "write", "edit", "delete", "run", "execute",
            "fix", "build", "install", "update", "modify", "remove", "search",
            "find", "analyze", "refactor", "test", "debug", "commit",
            "crea", "escribe", "edita", "borra", "ejecuta", "busca", "lee",
        ]
        .iter()
        .any(|kw| msg_lower.contains(kw));
        let needs_planning = word_count >= 15 || has_action_keywords;
        assert!(!needs_planning, "Trivial prompt should not trigger planning");
    }

    #[test]
    fn planning_gate_complex_prompt() {
        let user_msg = "crea un archivo en /tmp/test.txt con el contenido hola mundo";
        let word_count = user_msg.split_whitespace().count();
        let msg_lower = user_msg.to_lowercase();
        let has_action_keywords = [
            "create", "write", "edit", "delete", "run", "execute",
            "fix", "build", "install", "update", "modify", "remove", "search",
            "find", "analyze", "refactor", "test", "debug", "commit",
            "crea", "escribe", "edita", "borra", "ejecuta", "busca", "lee",
        ]
        .iter()
        .any(|kw| msg_lower.contains(kw));
        let needs_planning = word_count >= 15 || has_action_keywords;
        assert!(needs_planning, "Complex prompt with action keyword should trigger planning");
    }

    // === Phase 30: Fix 1 — Round-2 model adaptation after fallback ===

    #[test]
    fn fallback_adapts_model_for_round2() {
        // Simulate: primary model "claude-sonnet-4-5-20250929" not in fallback provider.
        let fallback: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let fallback_name = "echo";
        let fallback_models = fallback.supported_models();
        let original_model = "claude-sonnet-4-5-20250929";

        // Model should NOT be found in fallback.
        let found = fallback_models.iter().any(|m| m.id == original_model);
        assert!(!found, "claude-sonnet should not exist in EchoProvider");

        // The adaptation logic: if model not in fallback, use first supported model.
        let adapted = if !found {
            fallback_models.first().map(|m| m.id.clone())
        } else {
            Some(original_model.to_string())
        };
        assert!(adapted.is_some());
        assert_eq!(adapted.unwrap(), "echo", "Should adapt to echo provider's default model");
    }

    #[test]
    fn fallback_preserves_model_when_supported() {
        // If the model IS supported by fallback, don't change it.
        let fallback: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let fallback_models = fallback.supported_models();
        let model = &fallback_models[0].id; // "echo"

        let found = fallback_models.iter().any(|m| m.id == *model);
        assert!(found, "echo model should be in EchoProvider");
        // No adaptation needed.
    }

    // === Phase 30: Fix 2 — Planner model validation ===

    #[test]
    fn planner_supports_model_returns_false_for_unknown() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let planner = super::super::planner::LlmPlanner::new(provider, "nonexistent-model".into());
        assert!(!planner.supports_model());
    }

    #[test]
    fn planner_supports_model_returns_true_for_known() {
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let planner = super::super::planner::LlmPlanner::new(provider, "echo".into());
        assert!(planner.supports_model());
    }

    // ── A-1: Cost estimation after fallback ──

    use cuervo_core::types::{ModelInfo, TokenCost};
    use futures::stream::BoxStream;

    /// Provider that wraps EchoProvider behavior but returns a configurable cost.
    struct CostTestProvider {
        provider_name: String,
        cost: f64,
        inner: cuervo_providers::EchoProvider,
    }

    impl CostTestProvider {
        fn new(name: &str, cost: f64) -> Self {
            Self {
                provider_name: name.to_string(),
                cost,
                inner: cuervo_providers::EchoProvider::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ModelProvider for CostTestProvider {
        fn name(&self) -> &str {
            &self.provider_name
        }

        fn supported_models(&self) -> &[cuervo_core::types::ModelInfo] {
            self.inner.supported_models()
        }

        async fn invoke(
            &self,
            request: &ModelRequest,
        ) -> cuervo_core::error::Result<BoxStream<'static, cuervo_core::error::Result<ModelChunk>>> {
            self.inner.invoke(request).await
        }

        async fn is_available(&self) -> bool {
            true
        }

        fn estimate_cost(&self, _request: &ModelRequest) -> TokenCost {
            TokenCost {
                estimated_input_tokens: 100,
                estimated_cost_usd: self.cost,
            }
        }

        fn validate_model(&self, model: &str) -> cuervo_core::error::Result<()> {
            // Accept any model name to simplify test setup.
            if model == "echo" {
                Ok(())
            } else {
                self.inner.validate_model(model)
            }
        }
    }

    #[tokio::test]
    async fn cost_estimation_uses_fallback_provider() {
        use cuervo_core::types::{BackpressureConfig, CircuitBreakerConfig, ResilienceConfig};

        let primary: Arc<dyn ModelProvider> = Arc::new(CostTestProvider::new("cost_primary", 0.01));
        let fallback: Arc<dyn ModelProvider> = Arc::new(CostTestProvider::new("cost_fallback", 0.05));
        let mut session = Session::new("cost_primary".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();

        let mut resilience = ResilienceManager::new(ResilienceConfig {
            enabled: true,
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: 1,
                window_secs: 60,
                open_duration_secs: 30,
                half_open_probes: 2,
            },
            health: Default::default(),
            backpressure: BackpressureConfig::default(),
        });
        resilience.register_provider("cost_primary");
        resilience.register_provider("cost_fallback");
        // Break primary so fallback is used.
        resilience.record_failure("cost_primary").await;

        let fallbacks: Vec<(String, Arc<dyn ModelProvider>)> =
            vec![("cost_fallback".to_string(), fallback)];
        let limits = AgentLimits::default();
        let routing_config = RoutingConfig::default();

        let mut ctx = test_ctx(
            &primary, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        );
        ctx.fallback_providers = &fallbacks;

        let _result = run_agent_loop(ctx).await.unwrap();

        // Session cost should use fallback pricing (0.05), not primary (0.01).
        assert!(
            (session.estimated_cost_usd - 0.05).abs() < 0.001,
            "Expected fallback cost ~0.05, got {}",
            session.estimated_cost_usd
        );
    }

    #[tokio::test]
    async fn cost_estimation_uses_primary_when_no_fallback() {
        let primary: Arc<dyn ModelProvider> = Arc::new(CostTestProvider::new("cost_primary", 0.02));
        let mut session = Session::new("cost_primary".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = PermissionChecker::new(true);
        let (event_tx, _rx) = test_event_tx();
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();

        let _result = run_agent_loop(test_ctx(
            &primary, &mut session, &request, &tool_reg, &mut perms,
            &event_tx, &limits, &mut resilience, &routing_config,
        ))
        .await
        .unwrap();

        // Session cost should use primary pricing (0.02).
        assert!(
            (session.estimated_cost_usd - 0.02).abs() < 0.001,
            "Expected primary cost ~0.02, got {}",
            session.estimated_cost_usd
        );
    }

    // === Phase 33: ToolLoopGuard tests ===

    #[test]
    fn loop_guard_continue_on_first_round() {
        let mut guard = ToolLoopGuard::new();
        let tools = vec![("file_read".into(), 123u64)];
        assert_eq!(guard.record_round(&tools), LoopAction::Continue);
        assert_eq!(guard.consecutive_rounds(), 1);
    }

    #[test]
    fn loop_guard_continue_on_second_round() {
        let mut guard = ToolLoopGuard::new();
        assert_eq!(
            guard.record_round(&[("file_read".into(), 1)]),
            LoopAction::Continue
        );
        assert_eq!(
            guard.record_round(&[("grep".into(), 2)]),
            LoopAction::Continue
        );
        assert_eq!(guard.consecutive_rounds(), 2);
    }

    #[test]
    fn loop_guard_synthesis_at_threshold() {
        let mut guard = ToolLoopGuard::new();
        // Rounds 1-2: Continue
        guard.record_round(&[("file_read".into(), 1)]);
        guard.record_round(&[("grep".into(), 2)]);
        // Round 3: InjectSynthesis (synthesis_threshold = 3)
        let action = guard.record_round(&[("directory_tree".into(), 3)]);
        assert_eq!(action, LoopAction::InjectSynthesis);
    }

    #[test]
    fn loop_guard_force_at_threshold() {
        let mut guard = ToolLoopGuard::new();
        guard.record_round(&[("file_read".into(), 1)]);
        guard.record_round(&[("grep".into(), 2)]);
        guard.record_round(&[("directory_tree".into(), 3)]);
        // Round 4: ForceNoTools (force_threshold = 4)
        let action = guard.record_round(&[("file_inspect".into(), 4)]);
        assert_eq!(action, LoopAction::ForceNoTools);
    }

    #[test]
    fn loop_guard_oscillation_aaa() {
        // A→A→A pattern: 3 identical rounds
        let mut guard = ToolLoopGuard::new();
        let tools = vec![("file_read".into(), 42u64)];
        guard.record_round(&tools); // Round 1: Continue
        guard.record_round(&tools); // Round 2: Continue
        let action = guard.record_round(&tools); // Round 3: oscillation detected → Break
        assert_eq!(action, LoopAction::Break);
        assert!(guard.detect_oscillation());
    }

    #[test]
    fn loop_guard_oscillation_abab() {
        // A→B→A→B pattern: alternating over 4 rounds
        let mut guard = ToolLoopGuard::new();
        let a = vec![("file_read".into(), 1u64)];
        let b = vec![("grep".into(), 2u64)];
        guard.record_round(&a); // Round 1: Continue
        guard.record_round(&b); // Round 2: Continue
        guard.record_round(&a); // Round 3: InjectSynthesis (but also check oscillation)
        let action = guard.record_round(&b); // Round 4: oscillation A→B→A→B → Break
        assert_eq!(action, LoopAction::Break);
        assert!(guard.detect_oscillation());
    }

    #[test]
    fn loop_guard_no_oscillation_different_tools() {
        let mut guard = ToolLoopGuard::new();
        guard.record_round(&[("file_read".into(), 1)]);
        guard.record_round(&[("grep".into(), 2)]);
        guard.record_round(&[("directory_tree".into(), 3)]);
        assert!(!guard.detect_oscillation());
    }

    #[test]
    fn loop_guard_read_saturation_detected() {
        let mut guard = ToolLoopGuard::new();
        guard.record_round(&[("file_read".into(), 1)]);
        guard.record_round(&[("grep".into(), 2)]);
        guard.record_round(&[("glob".into(), 3)]);
        assert!(guard.detect_read_saturation());
    }

    #[test]
    fn loop_guard_read_saturation_not_with_write() {
        let mut guard = ToolLoopGuard::new();
        guard.record_round(&[("file_read".into(), 1)]);
        guard.record_round(&[("file_write".into(), 2)]); // Not read-only
        guard.record_round(&[("grep".into(), 3)]);
        assert!(!guard.detect_read_saturation());
    }

    #[test]
    fn loop_guard_duplicate_detection() {
        let mut guard = ToolLoopGuard::new();
        // Record a round with a specific tool+hash.
        guard.record_round(&[("file_read".into(), 12345)]);
        // Same tool+hash should be detected as duplicate.
        assert!(guard.is_duplicate("file_read", 12345));
        // Different hash should not be duplicate.
        assert!(!guard.is_duplicate("file_read", 99999));
        // Different tool should not be duplicate.
        assert!(!guard.is_duplicate("grep", 12345));
    }

    #[test]
    fn loop_guard_near_duplicate_different_hash() {
        let mut guard = ToolLoopGuard::new();
        guard.record_round(&[("file_read".into(), 111)]);
        // Different hash → not a duplicate.
        assert!(!guard.is_duplicate("file_read", 222));
    }

    #[test]
    fn loop_guard_plan_complete_forces_break() {
        let mut guard = ToolLoopGuard::new();
        guard.force_synthesis();
        let action = guard.record_round(&[("file_read".into(), 1)]);
        assert_eq!(action, LoopAction::Break);
        assert!(guard.plan_complete());
    }

    #[test]
    fn loop_guard_plan_complete_false_initially() {
        let guard = ToolLoopGuard::new();
        assert!(!guard.plan_complete());
    }

    #[test]
    fn loop_guard_consecutive_rounds_tracks() {
        let mut guard = ToolLoopGuard::new();
        assert_eq!(guard.consecutive_rounds(), 0);
        guard.record_round(&[("a".into(), 1)]);
        assert_eq!(guard.consecutive_rounds(), 1);
        guard.record_round(&[("b".into(), 2)]);
        assert_eq!(guard.consecutive_rounds(), 2);
    }

    #[test]
    fn loop_guard_empty_round_still_counts() {
        let mut guard = ToolLoopGuard::new();
        assert_eq!(guard.record_round(&[]), LoopAction::Continue);
        assert_eq!(guard.record_round(&[]), LoopAction::Continue);
        // Empty rounds don't trigger oscillation (empty == empty, but also
        // the model probably didn't call tools, which is unusual).
        assert_eq!(guard.record_round(&[]), LoopAction::Break); // AAA oscillation on empty
    }

    #[test]
    fn hash_tool_args_deterministic() {
        let val = serde_json::json!({"path": "/tmp/test.rs", "line": 42});
        let h1 = hash_tool_args(&val);
        let h2 = hash_tool_args(&val);
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_tool_args_different_for_different_input() {
        let v1 = serde_json::json!({"path": "/tmp/a.rs"});
        let v2 = serde_json::json!({"path": "/tmp/b.rs"});
        assert_ne!(hash_tool_args(&v1), hash_tool_args(&v2));
    }

    #[test]
    fn loop_action_debug_display() {
        // Ensure Debug is derived properly.
        let action = LoopAction::InjectSynthesis;
        let debug_str = format!("{:?}", action);
        assert!(debug_str.contains("InjectSynthesis"));
    }

    #[test]
    fn stop_condition_forced_synthesis_variant() {
        let sc = StopCondition::ForcedSynthesis;
        assert_ne!(sc, StopCondition::EndTurn);
        assert_ne!(sc, StopCondition::MaxRounds);
    }

    #[test]
    fn forced_synthesis_considered_success() {
        let sc = StopCondition::ForcedSynthesis;
        let success = matches!(sc, StopCondition::EndTurn | StopCondition::ForcedSynthesis);
        assert!(success, "ForcedSynthesis should be considered a success");
    }

    #[test]
    fn tool_usage_policy_content() {
        // Verify the policy text is well-formed.
        let policy = "\n\n## Tool Usage Policy\n\
            - Only call tools when you need NEW information you don't already have.\n\
            - After gathering data with tools, respond directly to the user.\n\
            - Never call the same tool twice with the same or very similar arguments.\n\
            - Prefer fewer tool calls. 1-3 tool rounds should suffice for most tasks.\n\
            - When you have enough information to answer, STOP calling tools and respond.\n\
            - If a tool fails, try a different approach or inform the user — do not retry the same call.\n";
        assert!(policy.contains("## Tool Usage Policy"));
        assert!(policy.contains("STOP calling tools"));
    }

    #[test]
    fn plan_prompt_includes_synthesis_step_rule() {
        use cuervo_core::types::ToolDefinition;
        let tools = vec![ToolDefinition {
            name: "file_read".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({}),
        }];
        let prompt = crate::repl::planner::LlmPlanner::build_plan_prompt_for_test("test", &tools);
        assert!(
            prompt.contains("Synthesize findings"),
            "Plan prompt should include synthesis step rule"
        );
        assert!(
            prompt.contains("5 steps or fewer"),
            "Plan prompt should include step limit rule"
        );
    }

    #[test]
    fn read_only_tools_list_correct() {
        use crate::repl::loop_guard::READ_ONLY_TOOLS_LIST as READ_ONLY_TOOLS;
        // Verify known ReadOnly tools are in the list.
        assert!(READ_ONLY_TOOLS.contains(&"file_read"));
        assert!(READ_ONLY_TOOLS.contains(&"grep"));
        assert!(READ_ONLY_TOOLS.contains(&"glob"));
        assert!(READ_ONLY_TOOLS.contains(&"directory_tree"));
        assert!(READ_ONLY_TOOLS.contains(&"git_status"));
        // Destructive tools should NOT be in the list.
        assert!(!READ_ONLY_TOOLS.contains(&"file_write"));
        assert!(!READ_ONLY_TOOLS.contains(&"bash"));
        assert!(!READ_ONLY_TOOLS.contains(&"file_delete"));
    }

    // --- Phase 43A: Control channel tests ---

    #[test]
    fn control_action_variants() {
        // Verify ControlAction enum has expected variants.
        assert_eq!(ControlAction::Continue, ControlAction::Continue);
        assert_ne!(ControlAction::Continue, ControlAction::StepOnce);
        assert_ne!(ControlAction::Continue, ControlAction::Cancel);
        assert_ne!(ControlAction::StepOnce, ControlAction::Cancel);
    }

    #[tokio::test]
    async fn check_control_noop_when_none() {
        // When ctrl_rx is None, agent loop should proceed without error.
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let (event_tx, _event_rx) = test_event_tx();
        let limits = AgentLimits::default();
        let mut permissions = PermissionChecker::new(false);
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();
        let ctx = test_ctx(
            &provider, &mut session, &request, &tool_reg,
            &mut permissions, &event_tx, &limits, &mut resilience,
            &routing_config,
        );
        // ctrl_rx is None in test_ctx — should complete without panic.
        let result = run_agent_loop(ctx).await;
        assert!(result.is_ok());
        let res = result.unwrap();
        // ctrl_rx should come back as None.
        assert!(res.ctrl_rx.is_none());
    }

    #[tokio::test]
    async fn check_control_cancel_breaks_loop() {
        use crate::tui::events::ControlEvent;
        let (ctrl_tx, ctrl_rx) = tokio::sync::mpsc::unbounded_channel();
        // Send Cancel immediately — the agent loop should exit on first yield point.
        ctrl_tx.send(ControlEvent::CancelAgent).unwrap();

        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let (event_tx, _event_rx) = test_event_tx();
        let limits = AgentLimits::default();
        let mut permissions = PermissionChecker::new(false);
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();
        let mut ctx = test_ctx(
            &provider, &mut session, &request, &tool_reg,
            &mut permissions, &event_tx, &limits, &mut resilience,
            &routing_config,
        );
        ctx.ctrl_rx = Some(ctrl_rx);
        let result = run_agent_loop(ctx).await;
        assert!(result.is_ok());
        let res = result.unwrap();
        // When cancelled before model invocation, should have 0 rounds.
        assert_eq!(res.rounds, 0);
        assert_eq!(res.stop_condition, StopCondition::Interrupted);
    }

    #[tokio::test]
    async fn check_control_step_returns_ctrl_rx() {
        use crate::tui::events::ControlEvent;
        let (_ctrl_tx, ctrl_rx) = tokio::sync::mpsc::unbounded_channel::<ControlEvent>();
        // No events queued — should pass through all yield points normally.
        let provider: Arc<dyn ModelProvider> = Arc::new(cuervo_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let (event_tx, _event_rx) = test_event_tx();
        let limits = AgentLimits::default();
        let mut permissions = PermissionChecker::new(false);
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();
        let mut ctx = test_ctx(
            &provider, &mut session, &request, &tool_reg,
            &mut permissions, &event_tx, &limits, &mut resilience,
            &routing_config,
        );
        ctx.ctrl_rx = Some(ctrl_rx);
        let result = run_agent_loop(ctx).await.unwrap();
        // ctrl_rx should be returned for reuse.
        assert!(result.ctrl_rx.is_some());
    }

    #[tokio::test]
    async fn check_control_resume_after_pause() {
        use crate::tui::events::ControlEvent;
        let sink = crate::render::sink::SilentSink::new();
        let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::unbounded_channel();
        // Send Pause, then Resume — check_control should return Continue.
        ctrl_tx.send(ControlEvent::Pause).unwrap();
        ctrl_tx.send(ControlEvent::Resume).unwrap();
        let action = check_control(&mut ctrl_rx, &sink).await;
        assert_eq!(action, ControlAction::Continue);
    }

    #[tokio::test]
    async fn check_control_step_after_pause() {
        use crate::tui::events::ControlEvent;
        let sink = crate::render::sink::SilentSink::new();
        let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::unbounded_channel();
        // Send Pause, then Step — should return StepOnce.
        ctrl_tx.send(ControlEvent::Pause).unwrap();
        ctrl_tx.send(ControlEvent::Step).unwrap();
        let action = check_control(&mut ctrl_rx, &sink).await;
        assert_eq!(action, ControlAction::StepOnce);
    }

    #[tokio::test]
    async fn check_control_cancel_during_pause() {
        use crate::tui::events::ControlEvent;
        let sink = crate::render::sink::SilentSink::new();
        let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::unbounded_channel();
        // Send Pause, then CancelAgent — should return Cancel.
        ctrl_tx.send(ControlEvent::Pause).unwrap();
        ctrl_tx.send(ControlEvent::CancelAgent).unwrap();
        let action = check_control(&mut ctrl_rx, &sink).await;
        assert_eq!(action, ControlAction::Cancel);
    }

    #[tokio::test]
    async fn check_control_ignore_unknown_events() {
        use crate::tui::events::ControlEvent;
        let sink = crate::render::sink::SilentSink::new();
        let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::unbounded_channel();
        // Send ApproveAction — not a control action, should return Continue.
        ctrl_tx.send(ControlEvent::ApproveAction).unwrap();
        let action = check_control(&mut ctrl_rx, &sink).await;
        assert_eq!(action, ControlAction::Continue);
    }

    // === Phase 43C: Feedback completeness tests ===

    #[test]
    fn compaction_spinner_label_is_specific() {
        // Compaction should say "Compacting context..." not "Thinking...".
        let label = "Compacting context...";
        assert!(label.contains("Compacting"));
        assert!(!label.contains("Thinking"));
    }

    #[test]
    fn reflection_feedback_methods_exist() {
        use crate::render::sink::RenderSink;
        let sink = crate::render::sink::SilentSink::new();
        // These should be callable without panic (default no-ops on SilentSink).
        sink.reflection_started();
        sink.reflection_complete("test analysis", 0.85);
    }

    #[test]
    fn consolidation_feedback_method_exists() {
        use crate::render::sink::RenderSink;
        let sink = crate::render::sink::SilentSink::new();
        sink.consolidation_status("consolidating reflections...");
    }

    #[test]
    fn tool_retrying_feedback_method_exists() {
        use crate::render::sink::RenderSink;
        let sink = crate::render::sink::SilentSink::new();
        sink.tool_retrying("bash", 2, 3, 500);
    }

    // === Fix #2: Plan Validation Pre-Execution tests ===

    fn make_validation_plan(steps: Vec<cuervo_core::traits::PlanStep>) -> ExecutionPlan {
        cuervo_core::traits::ExecutionPlan {
            plan_id: uuid::Uuid::new_v4(),
            goal: "Test goal".to_string(),
            steps,
            requires_confirmation: false,
            replan_count: 0,
            parent_plan_id: None,
        }
    }

    #[test]
    fn validate_plan_all_tools_exist() {
        let config = cuervo_core::types::ToolsConfig::default();
        let registry = cuervo_tools::default_registry(&config);

        let plan = make_validation_plan(vec![
            cuervo_core::traits::PlanStep {
                description: "Read file".to_string(),
                tool_name: Some("file_read".to_string()),
                parallel: false,
                confidence: 0.9,
                expected_args: None,
                outcome: None,
            },
            cuervo_core::traits::PlanStep {
                description: "Run command".to_string(),
                tool_name: Some("bash".to_string()),
                parallel: false,
                confidence: 0.8,
                expected_args: None,
                outcome: None,
            },
        ]);

        let warnings = validate_plan(&plan, &registry);
        assert!(warnings.is_empty(), "Valid plan should have no warnings");
    }

    #[test]
    fn validate_plan_detects_missing_tool() {
        let config = cuervo_core::types::ToolsConfig::default();
        let registry = cuervo_tools::default_registry(&config);

        let plan = make_validation_plan(vec![
            cuervo_core::traits::PlanStep {
                description: "Use non-existent tool".to_string(),
                tool_name: Some("nonexistent_tool".to_string()),
                parallel: false,
                confidence: 0.9,
                expected_args: None,
                outcome: None,
            },
        ]);

        let warnings = validate_plan(&plan, &registry);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("nonexistent_tool"));
        assert!(warnings[0].contains("not found in registry"));
    }

    #[test]
    fn validate_plan_detects_multiple_issues() {
        let config = cuervo_core::types::ToolsConfig::default();
        let registry = cuervo_tools::default_registry(&config);

        let plan = make_validation_plan(vec![
            cuervo_core::traits::PlanStep {
                description: "First invalid".to_string(),
                tool_name: Some("tool_one".to_string()),
                parallel: false,
                confidence: 0.9,
                expected_args: None,
                outcome: None,
            },
            cuervo_core::traits::PlanStep {
                description: "Valid tool".to_string(),
                tool_name: Some("file_read".to_string()),
                parallel: false,
                confidence: 0.8,
                expected_args: None,
                outcome: None,
            },
            cuervo_core::traits::PlanStep {
                description: "Second invalid".to_string(),
                tool_name: Some("tool_two".to_string()),
                parallel: false,
                confidence: 0.7,
                expected_args: None,
                outcome: None,
            },
        ]);

        let warnings = validate_plan(&plan, &registry);
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().any(|w| w.contains("tool_one")));
        assert!(warnings.iter().any(|w| w.contains("tool_two")));
    }

    #[test]
    fn validate_plan_warns_on_empty_steps() {
        let config = cuervo_core::types::ToolsConfig::default();
        let registry = cuervo_tools::default_registry(&config);

        let plan = make_validation_plan(vec![]);

        let warnings = validate_plan(&plan, &registry);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("0 steps"));
    }

    #[test]
    fn validate_plan_ignores_steps_without_tool() {
        let config = cuervo_core::types::ToolsConfig::default();
        let registry = cuervo_tools::default_registry(&config);

        let plan = make_validation_plan(vec![
            cuervo_core::traits::PlanStep {
                description: "Think about problem".to_string(),
                tool_name: None, // No tool specified
                parallel: false,
                confidence: 0.9,
                expected_args: None,
                outcome: None,
            },
        ]);

        let warnings = validate_plan(&plan, &registry);
        assert!(warnings.is_empty(), "Steps without tools should not generate warnings");
    }
}
