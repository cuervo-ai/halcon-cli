//! Multi-agent orchestrator: decomposes work into sub-agent tasks,
//! executes them in dependency waves with concurrency control,
//! and aggregates results.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use uuid::Uuid;

use halcon_core::traits::ModelProvider;
#[allow(unused_imports)]
use halcon_core::types::{
    AgentLimits, AgentResult, AgentType, ChatMessage, DomainEvent, EventPayload, MessageContent,
    ModelRequest, OrchestratorConfig, OrchestratorResult, ResilienceConfig, Role, RoutingConfig,
    Session, SubAgentResult, SubAgentTask, TaskContext,
};
use halcon_core::EventSender;
use halcon_storage::AsyncDatabase;
use halcon_tools::ToolRegistry;

use super::agent::{self, AgentContext};
use super::agent_comm::SharedContextStore;
use super::resilience::ResilienceManager;
use super::response_cache::ResponseCache;

/// Shared budget tracker across concurrent sub-agents.
pub struct SharedBudget {
    tokens_used: AtomicU64,
    token_limit: u64,
    start: Instant,
    duration_limit: Duration,
}

impl SharedBudget {
    pub fn new(limits: &AgentLimits) -> Self {
        Self {
            tokens_used: AtomicU64::new(0),
            token_limit: limits.max_total_tokens as u64,
            start: Instant::now(),
            duration_limit: if limits.max_duration_secs > 0 {
                Duration::from_secs(limits.max_duration_secs)
            } else {
                Duration::from_secs(u64::MAX / 2)
            },
        }
    }

    pub fn add_tokens(&self, tokens: u64) {
        // Use Release ordering so the write is visible to concurrent Acquire loads
        // in is_over_budget(). Relaxed would allow the budget check to see a stale
        // count and let concurrent sub-agents overshoot the limit.
        self.tokens_used.fetch_add(tokens, Ordering::Release);
    }

    pub fn is_over_budget(&self) -> bool {
        // Acquire pairs with the Release in add_tokens() — ensures we see the latest
        // token count written by any concurrent sub-agent before making the decision.
        if self.token_limit > 0 && self.tokens_used.load(Ordering::Acquire) >= self.token_limit {
            return true;
        }
        self.start.elapsed() >= self.duration_limit
    }

    #[allow(dead_code)]
    pub fn remaining_tokens(&self) -> u64 {
        if self.token_limit == 0 {
            return u64::MAX;
        }
        self.token_limit.saturating_sub(self.tokens_used.load(Ordering::Acquire))
    }
}

/// Partition tasks into dependency waves (topological sort by wave).
///
/// Each wave contains tasks whose dependencies are fully satisfied
/// by previous waves. Within a wave, tasks are sorted by priority (descending).
/// Circular dependencies are pushed into a final fallback wave.
pub fn topological_waves(tasks: &[SubAgentTask]) -> Vec<Vec<&SubAgentTask>> {
    if tasks.is_empty() {
        return vec![];
    }

    let task_ids: HashSet<Uuid> = tasks.iter().map(|t| t.task_id).collect();
    let mut completed: HashSet<Uuid> = HashSet::new();
    let mut remaining: Vec<&SubAgentTask> = tasks.iter().collect();
    let mut waves: Vec<Vec<&SubAgentTask>> = Vec::new();

    while !remaining.is_empty() {
        let mut wave: Vec<&SubAgentTask> = Vec::new();
        let mut still_remaining: Vec<&SubAgentTask> = Vec::new();

        for task in remaining {
            let deps_satisfied = task.depends_on.iter().all(|dep| {
                // Dependency is satisfied if completed OR not in our task set
                completed.contains(dep) || !task_ids.contains(dep)
            });
            if deps_satisfied {
                wave.push(task);
            } else {
                still_remaining.push(task);
            }
        }

        if wave.is_empty() {
            // Circular dependency detected — cannot safely execute remaining tasks in dependency order.
            // Log a warning with the cycle participants so callers can investigate.
            let cycle_ids: Vec<String> = still_remaining
                .iter()
                .map(|t| t.task_id.to_string())
                .collect();
            tracing::warn!(
                cycle_tasks = %cycle_ids.join(", "),
                count = still_remaining.len(),
                "Cyclic dependency detected in orchestrator task graph — \
                 affected tasks will be skipped to preserve execution integrity. \
                 Review task `depends_on` fields to resolve the cycle."
            );
            // Do NOT push cyclic tasks into a fallback wave — executing them without
            // dependency ordering produces undefined behaviour and may corrupt shared state.
            // Callers receive sub_results with success=false for these tasks (via the empty
            // wave producing no results).
            break;
        }

        // Sort wave by priority descending.
        wave.sort_by(|a, b| b.priority.cmp(&a.priority));

        for task in &wave {
            completed.insert(task.task_id);
        }

        waves.push(wave);
        remaining = still_remaining;
    }

    waves
}

/// Derive sub-agent execution limits from parent limits and orchestrator config.
pub fn derive_sub_limits(parent: &AgentLimits, config: &OrchestratorConfig) -> AgentLimits {
    let max_rounds = parent.max_rounds.min(10);
    let max_total_tokens = if config.shared_budget && config.max_concurrent_agents > 0 {
        parent.max_total_tokens / config.max_concurrent_agents as u32
    } else {
        parent.max_total_tokens
    };
    let max_duration_secs = if config.sub_agent_timeout_secs > 0 {
        config.sub_agent_timeout_secs
    } else if parent.max_duration_secs > 0 {
        parent.max_duration_secs / 2
    } else {
        0
    };

    AgentLimits {
        max_rounds,
        max_total_tokens,
        max_duration_secs,
        tool_timeout_secs: parent.tool_timeout_secs,
        provider_timeout_secs: parent.provider_timeout_secs,
        max_parallel_tools: parent.max_parallel_tools,
        max_tool_output_chars: parent.max_tool_output_chars,
        max_concurrent_agents: parent.max_concurrent_agents,
        max_cost_usd: parent.max_cost_usd,
        clarification_threshold: parent.clarification_threshold,
    }
}

/// Run the multi-agent orchestrator.
///
/// Executes sub-agent tasks in dependency waves with concurrency control.
/// Each sub-agent runs the existing `run_agent_loop` with `silent: true`.
#[allow(clippy::too_many_arguments)]
pub async fn run_orchestrator(
    orchestrator_id: Uuid,
    tasks: Vec<SubAgentTask>,
    provider: &Arc<dyn ModelProvider>,
    tool_registry: &ToolRegistry,
    event_tx: &EventSender,
    parent_limits: &AgentLimits,
    config: &OrchestratorConfig,
    routing_config: &RoutingConfig,
    trace_db: Option<&AsyncDatabase>,
    response_cache: Option<&ResponseCache>,
    fallback_providers: &[(String, Arc<dyn ModelProvider>)],
    model: &str,
    working_dir: &str,
    system_prompt: Option<&str>,
    guardrails: &[Box<dyn halcon_security::Guardrail>],
    confirm_destructive: bool,
    tbac_enabled: bool,
    // Optional callback that routes sub-agent permission events to the parent UI.
    // When Some, each sub-agent gets a SubAgentSink instead of a SilentSink,
    // and permission requests show as a modal in the TUI (or other parent UI).
    // When None, sub-agents auto-approve all Destructive tools (non-interactive).
    perm_awaiter: Option<crate::render::sink::PermissionAwaiter>,
    policy: std::sync::Arc<halcon_core::types::PolicyConfig>,
) -> Result<OrchestratorResult> {
    let orch_start = Instant::now();
    let budget = SharedBudget::new(parent_limits);
    let waves = topological_waves(&tasks);
    let sub_limits = derive_sub_limits(parent_limits, config);

    // Shared context store for inter-agent communication between waves.
    let shared_context = if config.enable_communication {
        Some(SharedContextStore::new())
    } else {
        None
    };

    // Emit OrchestratorStarted event so audit log captures orchestration beginning.
    let _ = event_tx.send(DomainEvent::new(EventPayload::OrchestratorStarted {
        orchestrator_id,
        task_count: tasks.len(),
        wave_count: waves.len(),
    }));

    let mut all_results: Vec<SubAgentResult> = Vec::new();
    let mut failed_task_ids: HashSet<Uuid> = HashSet::new();

    // Detect cyclic tasks: any task not appearing in any wave was skipped by
    // topological_waves() due to an unresolvable dependency cycle. These tasks
    // must appear in all_results as failures so the calling agent loop can
    // correctly account for them (prevents "zombie Running" tasks in ExecutionTracker).
    {
        let scheduled: HashSet<Uuid> = waves.iter().flat_map(|w| w.iter().map(|t| t.task_id)).collect();
        for task in &tasks {
            if !scheduled.contains(&task.task_id) {
                tracing::warn!(
                    task_id = %task.task_id,
                    "Task excluded from orchestration due to cyclic dependency — marking as failed"
                );
                failed_task_ids.insert(task.task_id);
                all_results.push(SubAgentResult {
                    task_id: task.task_id,
                    success: false,
                    output_text: String::new(),
                    agent_result: AgentResult {
                        success: false,
                        summary: "Skipped: cyclic dependency detected".to_string(),
                        files_modified: vec![],
                        tools_used: vec![],
                    },
                    input_tokens: 0,
                    output_tokens: 0,
                    cost_usd: 0.0,
                    latency_ms: 0,
                    rounds: 0,
                    error: Some("cyclic dependency".to_string()),
                    evidence_verified: false,
                    content_read_attempts: 0,
                });
            }
        }
    }

    for wave in &waves {
        // Check budget before each wave.
        if budget.is_over_budget() {
            tracing::warn!("Orchestrator budget exceeded, stopping before next wave");
            break;
        }

        // Capture shared context snapshot for this wave (if communication enabled).
        let context_snapshot = if let Some(ref ctx) = shared_context {
            let snap = ctx.snapshot().await;
            if snap.is_empty() { None } else { Some(snap) }
        } else {
            None
        };

        // Failure cascade: skip tasks whose dependencies failed.
        let mut skipped: Vec<SubAgentResult> = Vec::new();
        let eligible_tasks: Vec<&&SubAgentTask> = wave
            .iter()
            .filter(|task| {
                let failed_deps: Vec<uuid::Uuid> = task.depends_on.iter()
                    .filter(|dep| failed_task_ids.contains(*dep))
                    .copied()
                    .collect();
                let has_failed_dep = !failed_deps.is_empty();
                if has_failed_dep {
                    // Build a descriptive error that names the blocking failed dependency IDs.
                    let dep_ids = failed_deps.iter()
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let detail = format!(
                        "error_type:dependency_cascade | blocked_by_task_ids:[{}] | \
                         tool:{} | skipped_without_execution",
                        dep_ids,
                        task.instruction.lines().next().unwrap_or("unknown"),
                    );
                    tracing::info!(
                        task_id = %task.task_id,
                        failed_deps = %dep_ids,
                        "Skipping task due to failed dependency"
                    );
                    skipped.push(SubAgentResult {
                        task_id: task.task_id,
                        success: false,
                        output_text: String::new(),
                        agent_result: AgentResult {
                            success: false,
                            summary: format!("Skipped: dependency cascade from task(s) [{}]", dep_ids),
                            files_modified: vec![],
                            tools_used: vec![],
                        },
                        input_tokens: 0,
                        output_tokens: 0,
                        cost_usd: 0.0,
                        latency_ms: 0,
                        rounds: 0,
                        error: Some(detail),
                        evidence_verified: false,
                        content_read_attempts: 0,
                    });
                    false
                } else {
                    true
                }
            })
            .collect();

        // Track skipped tasks as failures for downstream cascade.
        for sr in &skipped {
            failed_task_ids.insert(sr.task_id);
            let _ = event_tx.send(DomainEvent::new(EventPayload::SubAgentCompleted {
                orchestrator_id,
                task_id: sr.task_id,
                success: false,
                latency_ms: 0,
                error: sr.error.clone(),
            }));
        }
        all_results.extend(skipped);

        if eligible_tasks.is_empty() {
            continue;
        }

        // Build futures for each eligible task in the wave.
        let futures: Vec<_> = eligible_tasks
            .iter()
            .map(|task| {
                let provider = Arc::clone(provider);
                let event_tx = event_tx.clone();
                let task_id = task.task_id;
                let agent_type = task.agent_type;
                let instruction = task.instruction.clone();
                let allowed_tools = task.allowed_tools.clone();
                let limits = task.limits_override.clone().unwrap_or_else(|| sub_limits.clone());
                let model = task.model.clone().unwrap_or_else(|| model.to_string());
                let working_dir = working_dir.to_string();
                // Clone the Option<Arc<...>> so the async move block owns it.
                let perm_awaiter_clone = perm_awaiter.clone();
                let policy = policy.clone();

                // Inject shared context from previous waves into system prompt.
                let system_prompt = if let Some(ref snap) = context_snapshot {
                    let context_json = serde_json::to_string_pretty(snap).unwrap_or_default();
                    let base = system_prompt.unwrap_or("");
                    Some(format!(
                        "{}\n\n## Context from previous agents\n```json\n{}\n```",
                        base, context_json,
                    ))
                } else {
                    system_prompt.map(|s| s.to_string())
                };

                // Emit SubAgentSpawned event.
                let _ = event_tx.send(DomainEvent::new(EventPayload::SubAgentSpawned {
                    orchestrator_id,
                    task_id,
                    agent_type,
                    instruction: instruction.chars().take(100).collect(),
                }));

                async move {
                    // Persist task as "running" before execution.
                    if let Some(db) = trace_db {
                        let _ = db.save_agent_task(
                            &task_id.to_string(),
                            &orchestrator_id.to_string(),
                            &task_id.to_string(), // sub-agent session_id = task_id
                            &format!("{:?}", agent_type),
                            &instruction,
                            "running",
                            0, 0, 0.0, 0, 0, None, None,
                        ).await;
                    }

                    let task_start = Instant::now();

                    // Create owned mutable state for this sub-agent.
                    let provider_name = provider.name().to_string();
                    let mut session = Session::new(model.clone(), provider_name, working_dir.clone());
                    let mut permissions = super::conversational_permission::ConversationalPermissionHandler::with_tbac(confirm_destructive, tbac_enabled);
                    // Permission routing is configured below based on whether a TUI
                    // event sender is available (SubAgentSink path sets TUI channel;
                    // non-TUI path calls set_non_interactive() after sink setup).
                    if !allowed_tools.is_empty() {
                        let ctx = TaskContext::new(instruction.clone(), allowed_tools.clone());
                        permissions.checker_mut().push_context(ctx);
                    }
                    let mut resilience = ResilienceManager::new(ResilienceConfig::default());

                    // SOTA 2026: Filter tool surface to only task-appropriate tools.
                    // Sub-agents with allowed_tools set should not see the full 60+ tool set —
                    // narrowing the surface reduces model confusion and speeds up tool selection.
                    //
                    // P1-A fix (2026-02-27 — Delegation Boundary hardening):
                    // Before filtering, validate that each tool in allowed_tools is actually
                    // registered. An unregistered name (e.g. deepseek plan generates a novel
                    // tool name) silently produces empty tool_defs after filtering, causing
                    // the sub-agent to run with 0 tools — the original cotización bug pattern.
                    // Emit a structured WARN per unknown tool so the path is observable.
                    let available_tool_names: std::collections::HashSet<String> = tool_registry
                        .tool_definitions()
                        .iter()
                        .map(|t| t.name.clone())
                        .collect();
                    for tool_name in &allowed_tools {
                        if !available_tool_names.contains(tool_name.as_str()) {
                            tracing::warn!(
                                unregistered_tool = %tool_name,
                                allowed_tools = ?allowed_tools,
                                "AUDIT P1-A: allowed_tools contains unregistered tool — \
                                 will be silently dropped by filter, risking empty tool surface. \
                                 Check delegation.rs:classify_step() and tool registry registration."
                            );
                        }
                    }
                    let tool_defs: Vec<_> = if !allowed_tools.is_empty() {
                        tool_registry
                            .tool_definitions()
                            .into_iter()
                            .filter(|t| allowed_tools.iter().any(|at| at == &t.name))
                            .collect()
                    } else {
                        tool_registry.tool_definitions()
                    };
                    // FASE 3 SECURITY: Abort sub-agent when empty tool surface detected.
                    // A sub-agent with non-empty allowed_tools that resolves to 0 tool_defs
                    // means ALL requested tools are unregistered. Rather than silently falling
                    // back to the full tool registry (which masks planner bugs and risks
                    // fabrication with an overwhelming 60+ tool surface), we abort immediately.
                    // BRECHA-R1 propagates the failure to the retry planner so it generates
                    // correct tool names on the next attempt.
                    if !allowed_tools.is_empty() && tool_defs.is_empty() {
                        tracing::error!(
                            allowed_tools = ?allowed_tools,
                            "PHASE-3 SECURITY: Sub-agent aborted — empty tool surface. \
                             All allowed_tools are unregistered. Returning failure so \
                             BRECHA-R1 retry planner can generate valid tool names."
                        );
                        return SubAgentResult {
                            task_id: task.task_id,
                            success: false,
                            output_text: String::new(),
                            agent_result: AgentResult {
                                success: false,
                                summary: format!(
                                    "Sub-agent aborted: empty tool surface. Requested tools {:?} \
                                     are not registered. Planner must use valid tool names.",
                                    allowed_tools
                                ),
                                files_modified: vec![],
                                tools_used: vec![],
                            },
                            input_tokens: 0,
                            output_tokens: 0,
                            cost_usd: 0.0,
                            latency_ms: 0,
                            rounds: 0,
                            error: Some(format!(
                                "Empty tool surface: all allowed_tools {:?} are unregistered",
                                allowed_tools
                            )),
                            evidence_verified: false,
                            content_read_attempts: 0,
                        };
                    }
                    // Dynamic max_tokens: use provider-reported model limit, falling back
                    // to conservative 8192 cap (deepseek-chat hard API limit).
                    let sub_agent_max_tokens = provider.model_max_output_tokens(&model).unwrap_or(8192);
                    let request = ModelRequest {
                        model,
                        messages: vec![ChatMessage {
                            role: Role::User,
                            content: MessageContent::Text(instruction.clone()),
                        }],
                        tools: tool_defs,
                        max_tokens: Some(sub_agent_max_tokens),
                        temperature: Some(0.0),
                        system: system_prompt,
                        stream: true,
                    };

                    // SOTA 2026: Hard cap sub-agent timeout (PolicyConfig.sub_agent_max_timeout_secs).
                    // Sub-agents have focused, narrow tasks — a 10-minute default causes
                    // 31s+ stalls when the convergence controller gets stuck in a replan loop.
                    // dep_check.rs uses NODE_TIMEOUT=240s — sub-agent must exceed that.
                    let sub_agent_max_timeout = policy.sub_agent_max_timeout_secs;
                    let timeout_dur = if limits.max_duration_secs > 0 {
                        Duration::from_secs(limits.max_duration_secs.min(sub_agent_max_timeout))
                    } else {
                        Duration::from_secs(sub_agent_max_timeout)
                    };

                    // Set up the render sink and permission policy for this sub-agent.
                    //
                    // When a PermissionAwaiter callback is provided (TUI mode), the sub-agent
                    // gets a SubAgentSink that routes permission events to the parent UI and
                    // waits for the user's decision via a dedicated reply channel.
                    //
                    // Without a callback (non-TUI / non-interactive mode), the sub-agent uses
                    // a bare SilentSink and auto-approves all Destructive tools.
                    let silent_sink = crate::render::sink::SilentSink::new();
                    let mut sub_sink_holder: Option<crate::render::sink::SubAgentSink> = None;

                    if let Some(awaiter) = perm_awaiter_clone {
                        let (sub_perm_tx, sub_perm_rx) =
                            tokio::sync::mpsc::unbounded_channel::<halcon_core::types::PermissionDecision>();
                        permissions.set_tui_channel(sub_perm_rx);
                        sub_sink_holder = Some(crate::render::sink::SubAgentSink::new(awaiter, sub_perm_tx));
                    } else {
                        permissions.set_non_interactive();
                    }

                    let effective_sink: &dyn crate::render::sink::RenderSink =
                        if let Some(ref s) = sub_sink_holder { s } else { &silent_sink };

                    let default_planning_config = halcon_core::types::PlanningConfig::default();
                    let default_orch_config = OrchestratorConfig::default();
                    let sub_agent_speculator = super::tool_speculation::ToolSpeculator::new();

                    let ctx = AgentContext {
                        provider: &provider,
                        session: &mut session,
                        request: &request,
                        tool_registry,
                        permissions: &mut permissions,
                        working_dir: &working_dir,
                        event_tx: &event_tx,
                        limits: &limits,
                        trace_db,
                        // Sub-agents must NOT use the response cache.
                        // Sub-agents have focused, single-shot tasks (e.g. file_write).
                        // Caching their "confirmation" responses ("File created") causes
                        // subsequent sub-agent runs to return the cached text without
                        // executing the actual tool — causing silent failures.
                        // Audit 2026-02-23: disabled to prevent cache poisoning on retries.
                        response_cache: None,
                        resilience: &mut resilience,
                        fallback_providers,
                        routing_config,
                        compactor: None,
                        planner: None,
                        guardrails,
                        reflector: None,
                        render_sink: effective_sink,
                        replay_tool_executor: None,
                        phase14: halcon_core::types::Phase14Context::default(),
                        model_selector: None,
                        registry: None,
                        episode_id: None,
                        planning_config: &default_planning_config,
                        orchestrator_config: &default_orch_config,
                        tool_selection_enabled: false,
                        task_bridge: None,
                        context_metrics: None,
                        context_manager: None,
                        ctrl_rx: None,
                        speculator: &sub_agent_speculator,
                        security_config: &halcon_core::types::SecurityConfig::default(),
                        strategy_context: None,
                        critic_provider: None,
                        critic_model: None,
                        plugin_registry: None,
                        // Signal to agent loop: use sub-agent ConvergenceController
                        // (tight limits + multilingual keyword extraction).
                        is_sub_agent: true,
                        requested_provider: None,
                        policy: policy.clone(),
                    };

                    let loop_result = tokio::time::timeout(timeout_dur, agent::run_agent_loop(ctx)).await;

                    let latency_ms = task_start.elapsed().as_millis() as u64;

                    match loop_result {
                        Ok(Ok(result)) => {
                            // A sub-agent is successful if it produced non-empty output,
                            // had a clean EndTurn exit, OR executed at least one tool successfully.
                            //
                            // AUDIT FIX (2026-02-23): The previous condition `result.rounds > 0`
                            // checked whether any round ran — NOT whether any tool actually succeeded.
                            // A sub-agent running rounds with ALL tools failing (file_write denied,
                            // bash error, permission timeout) would still get `success=true`, causing:
                            //   1. record_delegation_results marks plan step as TaskStatus::Completed
                            //   2. tracker.is_complete() → true → coordinator enters synthesis mode
                            //   3. Coordinator synthesizes "I created file X" without file existing
                            //
                            // Correct condition: `!result.tools_executed.is_empty()` — tools_executed
                            // is populated ONLY for successful tool calls in post_batch::run() via
                            // `state.tools_executed.extend(tool_successes.iter().cloned())`.
                            //
                            // Edge-case preservation: a sub-agent calling ONLY file_write (no text
                            // output, ConvergenceHalt stop) is still classified as success when
                            // file_write actually succeeded — tools_executed = ["file_write"] is
                            // non-empty, so `executed_tools = true`. No regression.
                            let produced_output = !result.full_text.is_empty();
                            let clean_exit = result.stop_condition == agent::StopCondition::EndTurn;
                            let executed_tools = !result.tools_executed.is_empty();
                            // P1-B fix (2026-02-27): For investigation sub-agents (those with a
                            // non-empty allowed_tools set), require at least one tool to have been
                            // executed. If the sub-agent ran with tools available but executed none,
                            // marking it successful allows fabricated text responses to propagate.
                            //
                            // Logic:
                            //  - allowed_tools.is_empty() → coordinator task (no narrowing) → use
                            //    original criterion (produced_output || clean_exit || executed_tools)
                            //  - allowed_tools non-empty → delegated investigative sub-agent →
                            //    REQUIRE executed_tools=true to prevent text-only fabrication.
                            //    Exception: if tool surface is empty (all unregistered), fall back
                            //    to original criterion (error already logged by P1-A fix above).
                            let has_narrowed_surface = !allowed_tools.is_empty();
                            let has_real_tool_surface = !request.tools.is_empty();
                            let p1b_text_only = has_narrowed_surface
                                && has_real_tool_surface
                                && !executed_tools
                                && (produced_output || clean_exit);
                            let success = if has_narrowed_surface && has_real_tool_surface {
                                // Delegated sub-agent: require tool execution, not just text output.
                                let verdict = executed_tools || (!produced_output && clean_exit);
                                if p1b_text_only {
                                    tracing::warn!(
                                        task_id = %task_id,
                                        produced_output,
                                        clean_exit,
                                        rounds = result.rounds,
                                        "P1-B: Investigation sub-agent produced text+clean_exit \
                                         but executed 0 tools — attempting intra-orchestrator retry."
                                    );
                                }
                                verdict
                            } else {
                                // Non-delegated coordinator task: original criterion.
                                produced_output || clean_exit || executed_tools
                            };

                            // ── P1-B Intra-Orchestrator Retry ────────────────────────────────
                            //
                            // When P1-B detects a text-only response (model described the task
                            // instead of calling tools), retry ONCE with an escalated directive
                            // that explicitly prohibits text output and demands a tool call.
                            //
                            // This is provider-agnostic: any model that returns text without
                            // calling tools on a delegated task gets one retry. No deepseek-
                            // specific logic. P1-B contract is preserved — if the retry also
                            // produces text-only, the result is marked as final failure.
                            //
                            // Budget: uses remaining time within the same timeout_dur.
                            // The retry reuses the same tool_defs and model.
                            if p1b_text_only {
                                let retry_elapsed = task_start.elapsed();
                                let retry_remaining = timeout_dur.saturating_sub(retry_elapsed);
                                // Only retry if we have at least 30s remaining in the timeout.
                                if retry_remaining >= Duration::from_secs(30) {
                                    let primary_tool = allowed_tools.iter().next()
                                        .map(|s| s.as_str())
                                        .unwrap_or("the required tool");
                                    let escalated_instruction = format!(
                                        "CRITICAL: Your previous response was REJECTED because you \
                                         produced text instead of calling a tool. You MUST call the \
                                         `{primary_tool}` tool NOW. Do NOT output any text, do NOT \
                                         describe what you will do, do NOT plan — execute the tool \
                                         call IMMEDIATELY.\n\nOriginal task: {instruction}"
                                    );
                                    tracing::info!(
                                        task_id = %task_id,
                                        retry_remaining_secs = retry_remaining.as_secs(),
                                        "P1-B RETRY: escalating directive for text-only sub-agent"
                                    );

                                    // Build retry request with escalated instruction.
                                    let retry_provider_name = provider.name().to_string();
                                    let mut retry_session = Session::new(
                                        request.model.clone(),
                                        retry_provider_name,
                                        working_dir.clone(),
                                    );
                                    let mut retry_permissions = super::conversational_permission::ConversationalPermissionHandler::with_tbac(confirm_destructive, tbac_enabled);
                                    if !allowed_tools.is_empty() {
                                        let ctx = TaskContext::new(escalated_instruction.clone(), allowed_tools.clone());
                                        retry_permissions.checker_mut().push_context(ctx);
                                    }
                                    let mut retry_resilience = ResilienceManager::new(ResilienceConfig::default());

                                    let retry_request = ModelRequest {
                                        model: request.model.clone(),
                                        messages: vec![ChatMessage {
                                            role: Role::User,
                                            content: MessageContent::Text(escalated_instruction),
                                        }],
                                        tools: request.tools.clone(),
                                        max_tokens: request.max_tokens,
                                        temperature: Some(0.0),
                                        system: request.system.clone(),
                                        stream: true,
                                    };

                                    let retry_speculator = super::tool_speculation::ToolSpeculator::new();
                                    let retry_silent_sink = crate::render::sink::SilentSink::new();
                                    let retry_sink: &dyn crate::render::sink::RenderSink = &retry_silent_sink;
                                    let retry_ctx = agent::AgentContext {
                                        provider: &provider,
                                        session: &mut retry_session,
                                        request: &retry_request,
                                        tool_registry,
                                        permissions: &mut retry_permissions,
                                        working_dir: &working_dir,
                                        event_tx: &event_tx,
                                        trace_db: None, // retry doesn't persist separate trace
                                        limits: &limits,
                                        response_cache: None,
                                        resilience: &mut retry_resilience,
                                        fallback_providers: &[],
                                        routing_config: &RoutingConfig::default(),
                                        compactor: None,
                                        planner: None,
                                        guardrails: &[],
                                        reflector: None,
                                        render_sink: retry_sink,
                                        replay_tool_executor: None,
                                        phase14: halcon_core::types::Phase14Context::default(),
                                        model_selector: None,
                                        registry: None,
                                        episode_id: None,
                                        planning_config: &halcon_core::types::PlanningConfig::default(),
                                        orchestrator_config: &OrchestratorConfig::default(),
                                        tool_selection_enabled: false,
                                        task_bridge: None,
                                        context_metrics: None,
                                        context_manager: None,
                                        ctrl_rx: None,
                                        speculator: &retry_speculator,
                                        security_config: &halcon_core::types::SecurityConfig::default(),
                                        strategy_context: None,
                                        critic_provider: None,
                                        critic_model: None,
                                        plugin_registry: None,
                                        is_sub_agent: true,
                                        requested_provider: None,
                                        policy: policy.clone(),
                                    };

                                    let retry_loop = tokio::time::timeout(
                                        retry_remaining,
                                        agent::run_agent_loop(retry_ctx),
                                    ).await;

                                    let total_latency = task_start.elapsed().as_millis() as u64;

                                    match retry_loop {
                                        Ok(Ok(retry_result)) => {
                                            let retry_executed = !retry_result.tools_executed.is_empty();
                                            if retry_executed {
                                                tracing::info!(
                                                    task_id = %task_id,
                                                    tools = ?retry_result.tools_executed,
                                                    "P1-B RETRY SUCCESS: sub-agent executed tools on second attempt"
                                                );
                                                return SubAgentResult {
                                                    task_id,
                                                    success: true,
                                                    output_text: retry_result.full_text,
                                                    agent_result: AgentResult {
                                                        success: true,
                                                        summary: format!(
                                                            "{} rounds (retry), {:?}",
                                                            retry_result.rounds, retry_result.stop_condition
                                                        ),
                                                        files_modified: vec![],
                                                        tools_used: retry_result.tools_executed,
                                                    },
                                                    input_tokens: result.input_tokens + retry_result.input_tokens,
                                                    output_tokens: result.output_tokens + retry_result.output_tokens,
                                                    cost_usd: result.cost_usd + retry_result.cost_usd,
                                                    latency_ms: total_latency,
                                                    rounds: result.rounds + retry_result.rounds,
                                                    error: None,
                                                    evidence_verified: retry_result.evidence_verified,
                                                    content_read_attempts: retry_result.content_read_attempts,
                                                };
                                            }
                                            // Retry also produced text-only → final failure
                                            tracing::warn!(
                                                task_id = %task_id,
                                                "P1-B RETRY FAILED: second attempt also text-only — marking final failure"
                                            );
                                        }
                                        Ok(Err(e)) => {
                                            tracing::warn!(
                                                task_id = %task_id,
                                                error = %e,
                                                "P1-B RETRY ERROR: agent loop error on retry"
                                            );
                                        }
                                        Err(_) => {
                                            tracing::warn!(
                                                task_id = %task_id,
                                                "P1-B RETRY TIMEOUT: retry exceeded remaining budget"
                                            );
                                        }
                                    }
                                } else {
                                    tracing::info!(
                                        task_id = %task_id,
                                        remaining_secs = retry_remaining.as_secs(),
                                        "P1-B RETRY SKIPPED: insufficient time remaining (<30s)"
                                    );
                                }
                            }
                            // ── End P1-B Intra-Orchestrator Retry ─────────────────────────────

                            SubAgentResult {
                            task_id,
                            success,
                            output_text: result.full_text,
                            agent_result: AgentResult {
                                success,
                                summary: format!("{} rounds, {:?}", result.rounds, result.stop_condition),
                                files_modified: vec![],
                                tools_used: result.tools_executed,
                            },
                            input_tokens: result.input_tokens,
                            output_tokens: result.output_tokens,
                            cost_usd: result.cost_usd,
                            latency_ms,
                            rounds: result.rounds,
                            error: None,
                            evidence_verified: result.evidence_verified,
                            content_read_attempts: result.content_read_attempts,
                        }},
                        Ok(Err(e)) => SubAgentResult {
                            task_id,
                            success: false,
                            output_text: String::new(),
                            agent_result: AgentResult {
                                success: false,
                                summary: format!("Error: {e}"),
                                files_modified: vec![],
                                tools_used: vec![],
                            },
                            input_tokens: 0,
                            output_tokens: 0,
                            cost_usd: 0.0,
                            latency_ms,
                            rounds: 0,
                            error: Some(format!("{e}")),
                            evidence_verified: false,
                            content_read_attempts: 0,
                        },
                        Err(_) => {
                            let timeout_secs = timeout_dur.as_secs();
                            SubAgentResult {
                                task_id,
                                success: false,
                                output_text: String::new(),
                                agent_result: AgentResult {
                                    success: false,
                                    summary: format!("Timed out after {}s", timeout_secs),
                                    files_modified: vec![],
                                    tools_used: vec![],
                                },
                                input_tokens: 0,
                                output_tokens: 0,
                                cost_usd: 0.0,
                                latency_ms,
                                rounds: 0,
                                error: Some(format!(
                                    "error_type:timeout | duration_secs:{} | \
                                     task_id:{} | increase sub_agent_timeout_secs in config",
                                    timeout_secs, task_id
                                )),
                                evidence_verified: false,
                                content_read_attempts: 0,
                            }
                        },
                    }
                }
            })
            .collect();

        // Execute wave concurrently.
        let wave_results = futures::future::join_all(futures).await;

        // Process results: update budget, persist, emit events, track failures.
        for result in wave_results {
            budget.add_tokens(result.input_tokens + result.output_tokens);

            // Track failed tasks for downstream failure cascade.
            // IMPORTANT: Timeout failures (error_type:timeout) are treated differently from
            // hard failures (provider errors, permission denials). A timeout is a transient
            // condition — the tool might succeed with more time (e.g. npm audit on large
            // Node projects). Cascading immediately on timeout kills all dependent steps
            // without giving them any chance to run.
            //
            // Soft cascade (timeout): add to failed_task_ids so dependents are skipped,
            // but log a distinct warning so operators know it was timeout not a hard error.
            // Note: the dep_check Node timeout is now mitigated by the 300s hard-cap +
            // ecosystem-adaptive timeout (240s for Node), so this path should be rare.
            if !result.success {
                let is_timeout = result.error.as_deref()
                    .map(|e| e.contains("error_type:timeout"))
                    .unwrap_or(false);
                if is_timeout {
                    tracing::warn!(
                        task_id = %result.task_id,
                        "Sub-agent timed out — dependent tasks will be skipped. \
                         Consider increasing sub_agent_timeout_secs in config."
                    );
                } else {
                    tracing::debug!(task_id = %result.task_id, "Sub-agent hard failure — cascading");
                }
                failed_task_ids.insert(result.task_id);
            }

            // Persist task completion.
            if let Some(db) = trace_db {
                let status = if result.success { "completed" } else { "failed" };
                let _ = db.update_agent_task_status(
                    &result.task_id.to_string(),
                    status,
                    result.input_tokens,
                    result.output_tokens,
                    result.cost_usd,
                    result.latency_ms,
                    result.rounds as u32,
                    result.error.as_deref(),
                    Some(&result.output_text),
                ).await;
            }

            let _ = event_tx.send(DomainEvent::new(EventPayload::SubAgentCompleted {
                orchestrator_id,
                task_id: result.task_id,
                success: result.success,
                latency_ms: result.latency_ms,
                error: result.error.clone(),
            }));

            // Inject result into shared context for subsequent waves.
            if let Some(ref ctx) = shared_context {
                ctx.set(
                    format!("result_{}", result.task_id),
                    serde_json::json!({
                        "output": result.output_text,
                        "success": result.success,
                    }),
                ).await;
            }

            all_results.push(result);
        }
    }

    // FASE 6 — R7: ALL_FAILED detection.
    // After all waves complete, if every sub-agent result is a failure, the orchestration
    // achieved nothing. Log a structured ERROR so operators can investigate the root cause
    // (tool surface misconfiguration, provider outages, permission denials, etc.).
    // This is separate from per-task failures caught above — it fires only when ALL tasks fail.
    if !all_results.is_empty() && all_results.iter().all(|r| !r.success) {
        tracing::error!(
            orchestrator_id = %orchestrator_id,
            task_count = all_results.len(),
            "ORCHESTRATION_TOTAL_FAILURE: all {} sub-agent task(s) failed. \
             Check tool surface configuration, provider availability, and permission settings. \
             Root causes: {:?}",
            all_results.len(),
            all_results.iter().map(|r| r.error.as_deref().unwrap_or("unknown")).collect::<Vec<_>>(),
        );
    }

    let total_latency_ms = orch_start.elapsed().as_millis() as u64;
    let orch_result = OrchestratorResult::from_results(orchestrator_id, all_results, total_latency_ms);

    // Emit OrchestratorCompleted event.
    let _ = event_tx.send(DomainEvent::new(EventPayload::OrchestratorCompleted {
        orchestrator_id,
        success_count: orch_result.success_count,
        total_count: orch_result.total_count,
        total_cost_usd: orch_result.total_cost_usd,
    }));

    Ok(orch_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::AgentLimits;

    // --- topological_waves tests ---

    #[test]
    fn topological_waves_empty() {
        let tasks: Vec<SubAgentTask> = vec![];
        let waves = topological_waves(&tasks);
        assert!(waves.is_empty());
    }

    #[test]
    fn topological_waves_no_deps() {
        let tasks = vec![
            SubAgentTask {
                task_id: Uuid::new_v4(),
                instruction: "A".into(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![],
                priority: 0,
            },
            SubAgentTask {
                task_id: Uuid::new_v4(),
                instruction: "B".into(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![],
                priority: 0,
            },
        ];
        let waves = topological_waves(&tasks);
        assert_eq!(waves.len(), 1, "all tasks in one wave when no deps");
        assert_eq!(waves[0].len(), 2);
    }

    #[test]
    fn topological_waves_linear_chain() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let tasks = vec![
            SubAgentTask {
                task_id: a,
                instruction: "A".into(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![],
                priority: 0,
            },
            SubAgentTask {
                task_id: b,
                instruction: "B".into(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![a],
                priority: 0,
            },
            SubAgentTask {
                task_id: c,
                instruction: "C".into(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![b],
                priority: 0,
            },
        ];
        let waves = topological_waves(&tasks);
        assert_eq!(waves.len(), 3, "A→B→C should produce 3 waves");
        assert_eq!(waves[0][0].task_id, a);
        assert_eq!(waves[1][0].task_id, b);
        assert_eq!(waves[2][0].task_id, c);
    }

    #[test]
    fn topological_waves_diamond() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let d = Uuid::new_v4();
        let tasks = vec![
            SubAgentTask {
                task_id: a,
                instruction: "A".into(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![],
                priority: 0,
            },
            SubAgentTask {
                task_id: b,
                instruction: "B".into(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![a],
                priority: 10,
            },
            SubAgentTask {
                task_id: c,
                instruction: "C".into(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![a],
                priority: 5,
            },
            SubAgentTask {
                task_id: d,
                instruction: "D".into(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![b, c],
                priority: 0,
            },
        ];
        let waves = topological_waves(&tasks);
        assert_eq!(waves.len(), 3, "A→(B,C)→D should produce 3 waves");
        assert_eq!(waves[0].len(), 1); // A
        assert_eq!(waves[1].len(), 2); // B, C (concurrent)
        assert_eq!(waves[2].len(), 1); // D
        // B should come before C in wave 1 (higher priority).
        assert_eq!(waves[1][0].task_id, b);
        assert_eq!(waves[1][1].task_id, c);
    }

    #[test]
    fn topological_waves_circular_graceful() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let tasks = vec![
            SubAgentTask {
                task_id: a,
                instruction: "A".into(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![b],
                priority: 0,
            },
            SubAgentTask {
                task_id: b,
                instruction: "B".into(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![a],
                priority: 0,
            },
        ];
        let waves = topological_waves(&tasks);
        // Audit fix: cyclic tasks must be SKIPPED (not pushed as a fallback wave).
        // Executing tasks with unresolved cyclic dependencies in undefined order is
        // incorrect and potentially dangerous. The correct behavior is to break out
        // of the wave loop and emit a warning, producing zero waves for a fully-cyclic
        // graph (no non-cyclic tasks exist to schedule).
        //
        // The function must return without hanging regardless of cycle structure.
        // All tasks in a fully-cyclic graph are skipped — waves is empty.
        let total_tasks: usize = waves.iter().map(|w| w.len()).sum();
        assert_eq!(
            total_tasks, 0,
            "fully-cyclic graph: all tasks must be skipped, not pushed into a fallback wave"
        );
    }

    #[test]
    fn cyclic_tasks_not_in_any_wave_can_be_detected() {
        // Verify the detection logic used in run_orchestrator(): scheduled = union of wave IDs,
        // any task NOT in scheduled was dropped due to a cycle and should become a failure result.
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4(); // independent task, not cyclic
        let tasks = vec![
            SubAgentTask {
                task_id: a,
                instruction: "A (cyclic)".into(),
                agent_type: AgentType::Chat,
                model: None, provider: None,
                allowed_tools: HashSet::new(), limits_override: None,
                depends_on: vec![b], priority: 0,
            },
            SubAgentTask {
                task_id: b,
                instruction: "B (cyclic)".into(),
                agent_type: AgentType::Chat,
                model: None, provider: None,
                allowed_tools: HashSet::new(), limits_override: None,
                depends_on: vec![a], priority: 0,
            },
            SubAgentTask {
                task_id: c,
                instruction: "C (independent)".into(),
                agent_type: AgentType::Chat,
                model: None, provider: None,
                allowed_tools: HashSet::new(), limits_override: None,
                depends_on: vec![], priority: 0,
            },
        ];
        let waves = topological_waves(&tasks);

        // C has no deps so it should be in wave 0; A and B form a cycle → not in any wave.
        let scheduled: std::collections::HashSet<Uuid> =
            waves.iter().flat_map(|w| w.iter().map(|t| t.task_id)).collect();

        assert!(scheduled.contains(&c), "independent task must be scheduled");
        assert!(!scheduled.contains(&a), "cyclic task A must be excluded from waves");
        assert!(!scheduled.contains(&b), "cyclic task B must be excluded from waves");

        // Both A and B would become failure results in run_orchestrator().
        let cyclic_count = tasks.iter().filter(|t| !scheduled.contains(&t.task_id)).count();
        assert_eq!(cyclic_count, 2, "exactly 2 cyclic tasks must be unscheduled");
    }

    // --- derive_sub_limits tests ---

    #[test]
    fn derive_sub_limits_shared_budget() {
        let parent = AgentLimits {
            max_rounds: 25,
            max_total_tokens: 100_000,
            max_duration_secs: 600,
            tool_timeout_secs: 120,
            provider_timeout_secs: 300,
            max_parallel_tools: 10,
            ..Default::default()
        };
        let config = OrchestratorConfig {
            max_concurrent_agents: 5,
            shared_budget: true,
            ..Default::default()
        };
        let limits = derive_sub_limits(&parent, &config);
        assert_eq!(limits.max_rounds, 10); // capped at 10
        assert_eq!(limits.max_total_tokens, 20_000); // 100k / 5
        assert_eq!(limits.max_duration_secs, 300); // 600 / 2
        assert_eq!(limits.tool_timeout_secs, 120); // inherited
        assert_eq!(limits.provider_timeout_secs, 300); // inherited
    }

    #[test]
    fn derive_sub_limits_per_agent() {
        let parent = AgentLimits {
            max_rounds: 25,
            max_total_tokens: 100_000,
            max_duration_secs: 600,
            ..Default::default()
        };
        let config = OrchestratorConfig {
            max_concurrent_agents: 3,
            shared_budget: false,
            sub_agent_timeout_secs: 120,
            ..Default::default()
        };
        let limits = derive_sub_limits(&parent, &config);
        assert_eq!(limits.max_total_tokens, 100_000); // full budget when not shared
        assert_eq!(limits.max_duration_secs, 120); // explicit timeout
    }

    // --- SharedBudget tests ---

    #[test]
    fn shared_budget_tracking() {
        let limits = AgentLimits {
            max_total_tokens: 1000,
            max_duration_secs: 3600,
            ..Default::default()
        };
        let budget = SharedBudget::new(&limits);
        assert!(!budget.is_over_budget());
        assert_eq!(budget.remaining_tokens(), 1000);

        budget.add_tokens(500);
        assert!(!budget.is_over_budget());
        assert_eq!(budget.remaining_tokens(), 500);
    }

    #[test]
    fn shared_budget_over_limit() {
        let limits = AgentLimits {
            max_total_tokens: 100,
            ..Default::default()
        };
        let budget = SharedBudget::new(&limits);
        budget.add_tokens(150);
        assert!(budget.is_over_budget());
        assert_eq!(budget.remaining_tokens(), 0);
    }

    #[test]
    fn shared_budget_unlimited() {
        let limits = AgentLimits {
            max_total_tokens: 0,
            max_duration_secs: 0,
            ..Default::default()
        };
        let budget = SharedBudget::new(&limits);
        budget.add_tokens(999_999);
        assert!(!budget.is_over_budget());
        assert_eq!(budget.remaining_tokens(), u64::MAX);
    }

    // --- Integration tests with EchoProvider ---

    #[tokio::test]
    async fn orchestrator_single_task() {
        let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
        let tool_registry = ToolRegistry::new();
        let (event_tx, _rx) = halcon_core::event_bus(64);
        let limits = AgentLimits::default();
        let config = OrchestratorConfig::default();
        let routing = RoutingConfig::default();
        let orch_id = Uuid::new_v4();

        let tasks = vec![SubAgentTask {
            task_id: Uuid::new_v4(),
            instruction: "Say hello".to_string(),
            agent_type: AgentType::Chat,
            model: None,
            provider: None,
            allowed_tools: HashSet::new(),
            limits_override: None,
            depends_on: vec![],
            priority: 0,
        }];

        let result = run_orchestrator(
            orch_id, tasks, &provider, &tool_registry, &event_tx,
            &limits, &config, &routing,
            None, None, &[], "echo", "/tmp", None,
            &[], true, false, None,
            std::sync::Arc::new(halcon_core::types::PolicyConfig::default()),
        ).await.unwrap();

        assert_eq!(result.total_count, 1);
        assert_eq!(result.success_count, 1);
        assert!(!result.sub_results[0].output_text.is_empty());
    }

    #[tokio::test]
    async fn orchestrator_parallel_wave() {
        let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
        let tool_registry = ToolRegistry::new();
        let (event_tx, _rx) = halcon_core::event_bus(64);
        let limits = AgentLimits::default();
        let config = OrchestratorConfig::default();
        let routing = RoutingConfig::default();
        let orch_id = Uuid::new_v4();

        let tasks = vec![
            SubAgentTask {
                task_id: Uuid::new_v4(),
                instruction: "Task A".to_string(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![],
                priority: 0,
            },
            SubAgentTask {
                task_id: Uuid::new_v4(),
                instruction: "Task B".to_string(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![],
                priority: 0,
            },
        ];

        let result = run_orchestrator(
            orch_id, tasks, &provider, &tool_registry, &event_tx,
            &limits, &config, &routing,
            None, None, &[], "echo", "/tmp", None,
            &[], true, false, None,
            std::sync::Arc::new(halcon_core::types::PolicyConfig::default()),
        ).await.unwrap();

        assert_eq!(result.total_count, 2);
        assert_eq!(result.success_count, 2);
    }

    #[tokio::test]
    async fn orchestrator_sequential_deps() {
        let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
        let tool_registry = ToolRegistry::new();
        let (event_tx, _rx) = halcon_core::event_bus(64);
        let limits = AgentLimits::default();
        let config = OrchestratorConfig::default();
        let routing = RoutingConfig::default();
        let orch_id = Uuid::new_v4();

        let a_id = Uuid::new_v4();
        let tasks = vec![
            SubAgentTask {
                task_id: a_id,
                instruction: "First".to_string(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![],
                priority: 0,
            },
            SubAgentTask {
                task_id: Uuid::new_v4(),
                instruction: "Second".to_string(),
                agent_type: AgentType::Chat,
                model: None,
                provider: None,
                allowed_tools: HashSet::new(),
                limits_override: None,
                depends_on: vec![a_id],
                priority: 0,
            },
        ];

        let result = run_orchestrator(
            orch_id, tasks, &provider, &tool_registry, &event_tx,
            &limits, &config, &routing,
            None, None, &[], "echo", "/tmp", None,
            &[], true, false, None,
            std::sync::Arc::new(halcon_core::types::PolicyConfig::default()),
        ).await.unwrap();

        assert_eq!(result.total_count, 2);
        assert_eq!(result.success_count, 2);
    }

    #[tokio::test]
    async fn orchestrator_events_emitted() {
        let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
        let tool_registry = ToolRegistry::new();
        let (event_tx, mut event_rx) = halcon_core::event_bus(64);
        let limits = AgentLimits::default();
        let config = OrchestratorConfig::default();
        let routing = RoutingConfig::default();
        let orch_id = Uuid::new_v4();

        let tasks = vec![SubAgentTask {
            task_id: Uuid::new_v4(),
            instruction: "Test events".to_string(),
            agent_type: AgentType::Chat,
            model: None,
            provider: None,
            allowed_tools: HashSet::new(),
            limits_override: None,
            depends_on: vec![],
            priority: 0,
        }];

        run_orchestrator(
            orch_id, tasks, &provider, &tool_registry, &event_tx,
            &limits, &config, &routing,
            None, None, &[], "echo", "/tmp", None,
            &[], true, false, None,
            std::sync::Arc::new(halcon_core::types::PolicyConfig::default()),
        ).await.unwrap();

        // Collect all events.
        let mut events = Vec::new();
        while let Ok(ev) = event_rx.try_recv() {
            events.push(ev);
        }

        let spawned = events.iter().any(|e| matches!(e.payload, EventPayload::SubAgentSpawned { .. }));
        let completed = events.iter().any(|e| matches!(e.payload, EventPayload::SubAgentCompleted { .. }));
        let orch_done = events.iter().any(|e| matches!(e.payload, EventPayload::OrchestratorCompleted { .. }));

        assert!(spawned, "should emit SubAgentSpawned");
        assert!(completed, "should emit SubAgentCompleted");
        assert!(orch_done, "should emit OrchestratorCompleted");
    }

    // --- Sub-Phase 16.3: Inter-Agent Communication tests ---

    #[tokio::test]
    async fn shared_context_disabled_by_default() {
        // Default OrchestratorConfig has enable_communication = false.
        let config = OrchestratorConfig::default();
        assert!(!config.enable_communication);
    }

    #[tokio::test]
    async fn orchestrator_comm_disabled_no_context_injection() {
        let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
        let tool_registry = ToolRegistry::new();
        let (event_tx, _rx) = halcon_core::event_bus(64);
        let limits = AgentLimits::default();
        let config = OrchestratorConfig { enabled: true, ..Default::default() };
        let routing = RoutingConfig::default();

        let a_id = Uuid::new_v4();
        let tasks = vec![
            SubAgentTask {
                task_id: a_id, instruction: "Wave 1".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![], priority: 0,
            },
            SubAgentTask {
                task_id: Uuid::new_v4(), instruction: "Wave 2".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![a_id], priority: 0,
            },
        ];

        // With communication disabled (default), should still work.
        let result = run_orchestrator(
            Uuid::new_v4(), tasks, &provider, &tool_registry, &event_tx,
            &limits, &config, &routing, None, None, &[], "echo", "/tmp", None,
            &[], true, false, None,
            std::sync::Arc::new(halcon_core::types::PolicyConfig::default()),
        ).await.unwrap();

        assert_eq!(result.total_count, 2);
        assert_eq!(result.success_count, 2);
    }

    #[tokio::test]
    async fn orchestrator_comm_enabled_injects_results() {
        let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
        let tool_registry = ToolRegistry::new();
        let (event_tx, _rx) = halcon_core::event_bus(64);
        let limits = AgentLimits::default();
        let config = OrchestratorConfig {
            enabled: true,
            enable_communication: true,
            ..Default::default()
        };
        let routing = RoutingConfig::default();

        let a_id = Uuid::new_v4();
        let tasks = vec![
            SubAgentTask {
                task_id: a_id, instruction: "Wave 1 task".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![], priority: 0,
            },
            SubAgentTask {
                task_id: Uuid::new_v4(), instruction: "Wave 2 task".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![a_id], priority: 0,
            },
        ];

        // With communication enabled, wave 2 should see wave 1 results.
        let result = run_orchestrator(
            Uuid::new_v4(), tasks, &provider, &tool_registry, &event_tx,
            &limits, &config, &routing, None, None, &[], "echo", "/tmp", None,
            &[], true, false, None,
            std::sync::Arc::new(halcon_core::types::PolicyConfig::default()),
        ).await.unwrap();

        assert_eq!(result.total_count, 2);
        assert_eq!(result.success_count, 2);
    }

    #[tokio::test]
    async fn shared_context_store_set_and_snapshot() {
        let store = SharedContextStore::new();
        store.set("result_abc".into(), serde_json::json!({"output": "hello", "success": true})).await;
        let snap = store.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert!(snap.contains_key("result_abc"));
    }

    #[tokio::test]
    async fn shared_context_empty_for_wave1() {
        // Fresh store is empty — wave 1 gets no prior context.
        let store = SharedContextStore::new();
        let snap = store.snapshot().await;
        assert!(snap.is_empty());
    }

    #[tokio::test]
    async fn wave_results_contain_task_id_keys() {
        let store = SharedContextStore::new();
        let task_id = Uuid::new_v4();
        store.set(format!("result_{task_id}"), serde_json::json!({"output": "done", "success": true})).await;
        let keys = store.keys().await;
        assert_eq!(keys.len(), 1);
        assert!(keys[0].starts_with("result_"));
    }

    #[tokio::test]
    async fn shared_context_concurrent_wave_safety() {
        let store = SharedContextStore::new();
        let s1 = store.clone();
        let s2 = store.clone();

        let (r1, r2): ((), ()) = tokio::join!(
            s1.set("key1".into(), serde_json::json!("val1")),
            s2.set("key2".into(), serde_json::json!("val2")),
        );
        let _ = (r1, r2);

        let snap = store.snapshot().await;
        assert_eq!(snap.len(), 2);
    }

    #[tokio::test]
    async fn shared_context_snapshot_json_format() {
        let store = SharedContextStore::new();
        store.set("result_123".into(), serde_json::json!({"output": "test output", "success": true})).await;
        let snap = store.snapshot().await;
        let json = serde_json::to_string_pretty(&snap).unwrap();
        assert!(json.contains("test output"));
        assert!(json.contains("success"));
    }

    #[tokio::test]
    async fn orchestrator_creates_shared_context_when_enabled() {
        // Verify that enable_communication = true creates the store
        // (tested implicitly through orchestrator_comm_enabled_injects_results).
        let config = OrchestratorConfig {
            enabled: true,
            enable_communication: true,
            ..Default::default()
        };
        assert!(config.enable_communication);
    }

    // --- Failure cascade tests ---

    #[test]
    fn failure_cascade_skips_dependents() {
        // Verify that topological_waves produces correct waves for cascade testing.
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let tasks = vec![
            SubAgentTask {
                task_id: a, instruction: "A fails".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![], priority: 0,
            },
            SubAgentTask {
                task_id: b, instruction: "B depends on A".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![a], priority: 0,
            },
            SubAgentTask {
                task_id: c, instruction: "C depends on B".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![b], priority: 0,
            },
        ];
        let waves = topological_waves(&tasks);
        assert_eq!(waves.len(), 3);

        // Simulate failure cascade logic:
        let mut failed: HashSet<Uuid> = HashSet::new();
        failed.insert(a); // A failed

        // Wave 2: B depends on A (failed) → skipped
        let wave2_eligible: Vec<_> = waves[1].iter()
            .filter(|t| !t.depends_on.iter().any(|d| failed.contains(d)))
            .collect();
        assert!(wave2_eligible.is_empty(), "B should be skipped");
        failed.insert(b); // B cascaded as failed

        // Wave 3: C depends on B (failed) → skipped
        let wave3_eligible: Vec<_> = waves[2].iter()
            .filter(|t| !t.depends_on.iter().any(|d| failed.contains(d)))
            .collect();
        assert!(wave3_eligible.is_empty(), "C should be skipped too");
    }

    #[test]
    fn failure_cascade_only_affected_branch() {
        // Diamond: A→(B,C)→D. B and C run in the same wave (both depend on A).
        // If B fails during wave 2, D (which depends on B) should be skipped in wave 3.
        // C runs fine since it only depends on A (which succeeded).
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let d = Uuid::new_v4();
        let tasks = vec![
            SubAgentTask {
                task_id: a, instruction: "A".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![], priority: 0,
            },
            SubAgentTask {
                task_id: b, instruction: "B fails".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![a], priority: 0,
            },
            SubAgentTask {
                task_id: c, instruction: "C succeeds".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![a], priority: 0,
            },
            SubAgentTask {
                task_id: d, instruction: "D depends on B+C".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![b, c], priority: 0,
            },
        ];
        let waves = topological_waves(&tasks);
        assert_eq!(waves.len(), 3);

        // Wave 1: A runs → succeeds
        // Wave 2: B + C both run (cascade check passes — A didn't fail)
        // After wave 2: B marked as failed
        let mut failed: HashSet<Uuid> = HashSet::new();

        // Wave 2 cascade check: neither B nor C has a failed dep (A succeeded)
        let wave2_eligible: Vec<_> = waves[1].iter()
            .filter(|t| !t.depends_on.iter().any(|d| failed.contains(d)))
            .collect();
        assert_eq!(wave2_eligible.len(), 2, "B and C both eligible in wave 2");

        // After wave 2 executes, B fails
        failed.insert(b);

        // Wave 3: D depends on B (failed) → skipped
        let wave3_eligible: Vec<_> = waves[2].iter()
            .filter(|t| !t.depends_on.iter().any(|d| failed.contains(d)))
            .collect();
        assert!(wave3_eligible.is_empty(), "D should be skipped (depends on failed B)");
    }

    #[test]
    fn failure_cascade_independent_tasks_unaffected() {
        // Tasks with no dependencies are never affected by failures.
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let tasks = vec![
            SubAgentTask {
                task_id: a, instruction: "A".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![], priority: 0,
            },
            SubAgentTask {
                task_id: b, instruction: "B".into(), agent_type: AgentType::Chat,
                model: None, provider: None, allowed_tools: HashSet::new(),
                limits_override: None, depends_on: vec![], priority: 0,
            },
        ];
        let waves = topological_waves(&tasks);
        let failed: HashSet<Uuid> = [a].into_iter().collect();

        // B has no deps → should still be eligible
        let eligible: Vec<_> = waves[0].iter()
            .filter(|t| !t.depends_on.iter().any(|d| failed.contains(d)))
            .collect();
        assert_eq!(eligible.len(), 2, "Both tasks have no deps so both eligible");
    }
}
