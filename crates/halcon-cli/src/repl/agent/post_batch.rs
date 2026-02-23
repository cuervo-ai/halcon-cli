//! Post-batch tool execution phase.
//!
//! This module handles everything that happens after the model's assistant turn has been
//! processed and the batch of tool calls is ready to be executed: deduplication, parallel
//! and sequential execution, guardrail scanning, supervisor checks, reflexion, failure
//! tracking, replanning, and tool-result injection back into the conversation.
//!
//! The [`run`] function is called once per agent loop iteration whenever the model returned
//! a `ToolUse` stop reason.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::Utc;
use sha2::Digest;

use halcon_core::traits::{Planner, StepOutcome};
use halcon_core::types::{
    AgentLimits, ChatMessage, ContentBlock, DomainEvent, EventPayload, MessageContent,
    ModelRequest, PlanningConfig, Role, Session,
};
use halcon_core::EventSender;
use halcon_storage::{AsyncDatabase, MemoryEntry, MemoryEntryType};
use halcon_tools::ToolRegistry;

use super::super::agent_types::ControlReceiver;
use super::super::executor;
use super::super::failure_tracker::ToolFailureTracker;
use super::super::loop_guard::hash_tool_args;
use super::super::accumulator::CompletedToolUse;
use super::loop_state::LoopState;
use super::plan_formatter::{format_plan_for_prompt, update_plan_in_system};
use super::provider_client::check_control;
use super::ControlAction;
use crate::render::sink::RenderSink;

/// Outcome returned by [`run`] to the agent loop.
pub(super) enum PostBatchOutcome {
    /// The agent loop should break out immediately (cancel, strict enforcement halt, etc.).
    BreakLoop,
    /// Normal completion — continue to the convergence phase.
    Continue {
        round_tool_log: Vec<(String, u64)>,
        tool_failures: Vec<(String, String)>,
        tool_successes: Vec<String>,
    },
}

/// Execute the post-batch phase for one agent loop iteration.
///
/// This is called after the model has produced a `ToolUse` stop and the accumulated
/// tool calls have been collected into `completed_tools`. It executes the tools,
/// processes results, and returns a [`PostBatchOutcome`] that drives the outer loop.
pub(super) async fn run(
    state: &mut LoopState,
    completed_tools: Vec<CompletedToolUse>,
    session: &mut halcon_core::types::Session,
    render_sink: &dyn crate::render::sink::RenderSink,
    tool_registry: &halcon_tools::ToolRegistry,
    working_dir: &str,
    event_tx: &halcon_core::EventSender,
    trace_db: Option<&halcon_storage::AsyncDatabase>,
    guardrails: &[Box<dyn halcon_security::Guardrail>],
    permissions: &mut super::super::conversational_permission::ConversationalPermissionHandler,
    tool_exec_config: &super::super::executor::ToolExecutionConfig<'_>,
    plugin_registry: Option<&std::sync::Arc<std::sync::Mutex<super::super::plugin_registry::PluginRegistry>>>,
    replay_tool_executor: Option<&super::super::replay_executor::ReplayToolExecutor>,
    speculator: &super::super::tool_speculation::ToolSpeculator,
    task_bridge: &mut Option<&'_ mut super::super::task_bridge::TaskBridge>,
    reflector: Option<&super::super::reflexion::Reflector>,
    planner: Option<&dyn halcon_core::traits::Planner>,
    planning_config: &halcon_core::types::PlanningConfig,
    request: &halcon_core::types::ModelRequest,
    ctrl_rx: &mut Option<super::super::agent_types::ControlReceiver>,
    limits: &halcon_core::types::AgentLimits,
    round_model_name: &str,
    round_provider_name: &str,
    episode_id: Option<uuid::Uuid>,
    round: usize,
) -> anyhow::Result<PostBatchOutcome> {
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
                if state.loop_guard.is_duplicate(&tool.name, args_hash) {
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

        // P2-D: Deduplication Visibility.
        // Make duplicate filtering observable by the user (render_sink) so the TUI shows
        // why tool calls are being blocked. The model already sees the synthetic error
        // ToolResult; the render_sink call makes it visible in the activity panel.
        let round_dedup_count = dedup_result_blocks.len();
        if round_dedup_count > 0 && !state.silent {
            render_sink.loop_guard_action(
                "dedup_filter",
                &format!(
                    "{} duplicate tool call(s) filtered (already executed in a prior round)",
                    round_dedup_count
                ),
            );
        }

        // Execute tools: in replay mode, return recorded results; otherwise execute normally.
        let plan = executor::plan_execution(deduplicated_tools, tool_registry);
        let mut tool_result_blocks: Vec<ContentBlock> = dedup_result_blocks;
        let mut tool_failures: Vec<(String, String)> = Vec::new(); // (tool_name, error)
        let mut tool_successes: Vec<String> = Vec::new(); // tool_name of successful executions
        // P1-A: Flag set inside the `else` (non-replay) branch when ALL parallel tools failed.
        // Declared here so it's in scope for the post-message-add check below.
        let mut parallel_batch_collapsed = false;

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
                        if !state.silent { render_sink.speculative_result(&tool_call.name, true); }
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

            // Phase E5: Enter ToolWait state while tools are executing.
            if !state.silent && (!remaining_parallel.is_empty() || !plan.sequential_batch.is_empty()) {
                render_sink.agent_state_transition(state.current_fsm_state, "tool_wait", "executing tools");
                state.current_fsm_state = "tool_wait";
            }

            // Execute remaining ReadOnly tools in parallel with concurrency cap.
            let parallel_results = executor::execute_parallel_batch(
                &remaining_parallel,
                tool_registry,
                working_dir,
                state.tool_timeout,
                event_tx,
                trace_db,
                state.session_id,
                &mut state.trace_step_index,
                limits.max_parallel_tools,
                &tool_exec_config,
                render_sink,
                plugin_registry.map(|arc| arc.as_ref()),
            )
            .await;
            // Merge speculation hits with real results.
            spec_hits.extend(parallel_results);
            let parallel_results = spec_hits;

            // P1-A: Compute parallel batch collapse flag BEFORE results are consumed.
            // If ALL parallel results are errors, sequential planning is futile this round.
            // We record the flag here (before parallel_results is moved into the for loop below)
            // and act on it after tool_result_blocks is added to state.messages (protocol integrity).
            parallel_batch_collapsed = !parallel_results.is_empty()
                && !plan.parallel_batch.is_empty()
                && parallel_results.iter().all(|r| {
                    matches!(
                        &r.content_block,
                        ContentBlock::ToolResult { is_error: true, .. }
                    )
                });

            // Render parallel results.
            if !state.silent {
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
                    state.tool_timeout,
                    event_tx,
                    trace_db,
                    state.session_id,
                    &mut state.trace_step_index,
                    &tool_exec_config,
                    render_sink,
                    plugin_registry.map(|arc| arc.as_ref()),
                )
                .await;
                sequential_results.push(result);
            }

            // Phase E5: Return to Executing after tools complete.
            if !state.silent && (!parallel_results.is_empty() || !sequential_results.is_empty()) {
                render_sink.agent_state_transition(state.current_fsm_state, "executing", "tools complete");
                state.current_fsm_state = "executing";
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
            let elider_budget = state.context_pipeline.accountant()
                .available(halcon_context::Tier::L0Hot) / 4;
            let elider_budget = elider_budget.max(500);

            for result in parallel_results {
                let mut block = result.content_block;
                if let ContentBlock::ToolResult {
                    ref mut content,
                    is_error: false,
                    ..
                } = block
                {
                    *content = state.context_pipeline.elider().elide(
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
                    *content = state.context_pipeline.elider().elide(
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

        // HICON Phase 3: Feed tool errors to Bayesian detector
        for (tool_name, error_content) in &tool_failures {
            state.loop_guard.record_error(&format!("{}:{}", tool_name, error_content));
        }

        // Guardrail scan on tool results (warn-only — does not block tool output).
        if !guardrails.is_empty() {
            for block in &tool_result_blocks {
                if let ContentBlock::ToolResult { content, .. } = block {
                    let violations = halcon_security::run_guardrails(
                        guardrails,
                        content,
                        halcon_security::GuardrailCheckpoint::PostInvocation,
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
        if let Some(ref mut tracker) = state.execution_tracker {
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

            // Sprint 2: Update plan progress in loop guard for dynamic thresholds
            let (completed, total, elapsed) = tracker.progress();
            state.loop_guard.update_plan_progress(completed, total, elapsed);

            // Phase 33: plan completion → force synthesis on next round.
            if tracker.is_complete() {
                tracing::info!("All plan steps completed — forcing synthesis");
                state.loop_guard.force_synthesis();
            }

            // FIX: When all remaining non-terminal steps have no tool_name (i.e. only
            // synthesis/confirmation steps remain), force no-tools for the next round.
            // Without this, the coordinator makes an API call with all tools available
            // and the model may hallucinate a tool call (e.g. re-calling file_write)
            // instead of just synthesizing. This saves one full API round (~131s for
            // large file generation tasks like the Minecraft benchmark).
            {
                let plan = tracker.plan();
                let pending_are_all_synthesis = plan.steps.iter()
                    .enumerate()
                    .filter(|(_, s)| {
                        // A step is still "active" if its outcome is None (not yet completed).
                        s.outcome.is_none()
                    })
                    .all(|(_, s)| s.tool_name.is_none());

                if pending_are_all_synthesis && !plan.steps.is_empty()
                    && plan.steps.iter().any(|s| s.outcome.is_none())
                {
                    tracing::info!(
                        "All remaining plan steps are synthesis-only (no tool_name) — \
                         suppressing tools for coordinator synthesis round"
                    );
                    state.tool_decision.set_force_next();
                }
            }

            // Planning V3: Early convergence check after each tool round.
            // Computes progress_delta vs previous round to detect diminishing returns.
            let (completed, total, _) = tracker.progress();
            let current_ratio = if total > 0 { completed as f32 / total as f32 } else { 0.0 };
            let progress_delta = current_ratio - state.last_convergence_ratio;
            state.last_convergence_ratio = current_ratio;
            // Phase L INVARIANT K5-2: token_growth_rate < 1.3× per round.
            // Super-linear growth indicates sub-agent output injection is too large
            // (observed: 4836 → 20492 tokens = 4.24× in one round for "analiza" case).
            if state.call_input_tokens_prev_round > 0 {
                let growth_factor = state.call_input_tokens as f64
                    / state.call_input_tokens_prev_round as f64;
                if growth_factor > 1.3 {
                    tracing::warn!(
                        round = state.rounds,
                        prev_tokens = state.call_input_tokens_prev_round,
                        curr_tokens = state.call_input_tokens,
                        growth_factor,
                        "K5-2 INVARIANT: token growth {:.2}× exceeds 1.3× linear bound —                          sub-agent injection may be too large",
                        growth_factor
                    );
                    if !state.silent {
                        render_sink.info(&format!(
                            "[token] context grew {:.1}× this round ({} → {} tokens) —                              consider summarizing sub-agent outputs",
                            growth_factor,
                            state.call_input_tokens_prev_round,
                            state.call_input_tokens
                        ));
                    }
                }
            }
            // Update prev_round tracker for next iteration.
            state.call_input_tokens_prev_round = state.call_input_tokens;
            // Phase L fix B4: use the provider's full context window as denominator,
            // not pipeline_budget (which is only the L0-injection budget ~80% of window).
            // pipeline_budget(~14895) < call_input_tokens(~24504) → saturating_sub=0 → false alarm.
            // provider_context_window(64000) >> call_input_tokens → correct remaining budget.
            let tokens_remaining =
                (state.provider_context_window as u64).saturating_sub(state.call_input_tokens);
            if let Some(reason) = state.convergence_detector.check_with_cost(
                current_ratio,
                tokens_remaining,
                progress_delta,
                state.call_input_tokens,
            ) {
                tracing::info!(
                    reason = %reason.description(),
                    completion_ratio = current_ratio,
                    tokens_remaining,
                    "Early convergence triggered — requesting synthesis now"
                );
                if !state.silent {
                    render_sink.info(&format!(
                        "[convergence] {}",
                        reason.description()
                    ));
                }
                state.loop_guard.force_synthesis();
            }

            // Planning V3: Advance MacroPlanView to emit [N/M] progress to the user.
            if let Some(ref mut view) = state.macro_plan_view {
                let current = tracker.current_step();
                // Advance through any newly completed steps, emitting done lines.
                // NOTE: use step.done_line() (step's own method) rather than view.format_done()
                // to avoid a borrow conflict — advance() returns &MacroStep that borrows view,
                // so calling any other &self method on view while step is alive is rejected by NLL.
                while view.current_idx() < current {
                    match view.advance() {
                        None => break,
                        Some(step) => {
                            let line = step.done_line(); // owned String — borrow ends here
                            if !state.silent {
                                render_sink.info(&line);
                            }
                        }
                    }
                }
                // Emit the "starting" line for the next step.
                if let Some(line) = view.format_start(current) {
                    if !state.silent {
                        render_sink.info(&line);
                    }
                }
            }

            // Update plan section in system prompt with new step statuses.
            let plan = tracker.plan();
            let current = tracker.current_step();
            let plan_section = format_plan_for_prompt(plan, current);
            if let Some(ref mut sys) = state.cached_system {
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

            // P5 FIX: Single TaskBridge sync per round (removed earlier duplicate that used
            // stale request.model/provider.name instead of round-specific actuals).
            // This sync uses round_model_name/round_provider_name for accurate provenance.
            if let Some(ref mut bridge) = task_bridge {
                bridge.sync_from_tracker(
                    tracker,
                    &round_model_name,
                    &round_provider_name,
                    Some(state.session_id),
                );
                tracing::trace!(
                    completed,
                    total,
                    model = %round_model_name,
                    provider = %round_provider_name,
                    "TaskBridge synced with ExecutionTracker (round provenance)"
                );

                // strict_enforcement (Phase 69): when active, permanently failed tasks
                // halt the agent loop immediately rather than burning more rounds.
                // `Retrying` tasks still have budget — only `Failed` (terminal) triggers this.
                if bridge.is_strict() && bridge.has_permanently_failed_tasks() {
                    tracing::warn!(
                        round,
                        "Strict enforcement: structured task permanently failed — halting agent loop"
                    );
                    render_sink.warning(
                        "strict enforcement: task permanently failed",
                        Some("Use --full without --expert to allow continued execution on task failure"),
                    );
                    state.forced_synthesis_detected = true;
                    return Ok(PostBatchOutcome::BreakLoop);
                }
            }
        }

        // Phase 1 Supervisor: PostBatchSupervisor — authority check after tool batch.
        // Operates before ToolLoopGuard thresholds (synthesis_threshold=6, force_threshold=10),
        // providing earlier structural intervention on plan misalignment or critical failures.
        {
            let expected_tool = state.execution_tracker.as_ref().and_then(|t| {
                t.plan().steps.get(t.current_step())
            }).and_then(|s| s.tool_name.as_deref());

            let all_executed: Vec<String> = {
                let mut v = tool_successes.clone();
                v.extend(tool_failures.iter().map(|(n, _)| n.clone()));
                v
            };

            // Only deterministic failures are "critical" for Gate 1.
            // Transient errors (timeout, rate-limit, network) must NOT trigger ForceReplanNow —
            // they resolve on retry and escalating to a full replan burns ~3k tokens unnecessarily.
            let deterministic_failures: Vec<(String, String)> = tool_failures
                .iter()
                .filter(|(_, err)| super::executor::is_deterministic_error(err))
                .cloned()
                .collect();

            let (completed, total, _) = if let Some(ref t) = state.execution_tracker {
                t.progress()
            } else {
                (0, 0, 0)
            };
            let plan_progress_ratio = if total > 0 {
                completed as f32 / total as f32
            } else {
                0.0
            };
            let any_tool_succeeded = !tool_successes.is_empty();

            match super::super::supervisor::PostBatchSupervisor::check(
                round,
                expected_tool,
                &all_executed,
                &deterministic_failures,
                plan_progress_ratio,
                any_tool_succeeded,
                None, // plugin_all_failed: no plugin tracking at this call site
            ) {
                super::super::supervisor::BatchVerdict::Continue => {}
                super::super::supervisor::BatchVerdict::InjectCorrection(msg) => {
                    tracing::info!("PostBatchSupervisor: injecting correction into state.messages");
                    if !state.silent {
                        render_sink.info("[supervisor] correction injected");
                    }
                    state.messages.push(ChatMessage {
                        role: Role::User,
                        content: MessageContent::Text(msg),
                    });
                }
                super::super::supervisor::BatchVerdict::ForceReplanNow(reason) => {
                    tracing::warn!(reason = %reason, "PostBatchSupervisor: forcing replan (AUTHORITY)");
                    if !state.silent {
                        render_sink.info(&format!("[supervisor] forced replan: {reason}"));
                    }

                    // Phase 5: Supervisor authority — call planner.replan() DIRECTLY.
                    // This bypasses MAX_REPLAN_ATTEMPTS (that counter governs model-initiated
                    // replanning via ReplanRequired, not supervisor-forced replans).
                    // Supervisor authority is absolute: the plan MUST be revised.
                    let supervisor_replan_done = if let Some(planner_ref) = planner {
                        // Clone current plan to release the immutable borrow before reset_plan().
                        let (plan_clone, failed_idx) = if let Some(ref tracker) = state.execution_tracker {
                            let plan = tracker.plan().clone();
                            let idx = tracker.current_step().saturating_sub(1);
                            (Some(plan), idx)
                        } else {
                            (None, 0)
                        };

                        if let Some(current_plan) = plan_clone {
                            match planner_ref.replan(&current_plan, failed_idx, &reason, &request.tools).await {
                                Ok(Some(new_plan)) => {
                                    tracing::info!("PostBatchSupervisor: supervisor replan succeeded");
                                    // Step 8g-supervisor: PlanCoherenceChecker — detect goal drift
                                    // caused by supervisor-forced replanning, same as model-initiated
                                    // replanning. Supervisor authority doesn't bypass drift detection.
                                    {
                                        let report = state.coherence_checker.check(&new_plan);
                                        state.cumulative_drift_score += report.drift_score;
                                        state.drift_replan_count += 1;
                                        if report.drift_detected {
                                            tracing::warn!(
                                                drift_score = report.drift_score,
                                                missing_keywords = ?report.missing_keywords,
                                                new_goal = %new_plan.goal,
                                                "PlanCoherenceChecker: goal drift detected in supervisor replan"
                                            );
                                            state.messages.push(ChatMessage {
                                                role: Role::User,
                                                content: MessageContent::Text(format!(
                                                    "[Goal Drift Alert]: The revised plan deviates from the original goal. \
                                                     Original goal: \"{}\". \
                                                     Ensure all plan steps serve the original goal.",
                                                    &request.system.as_deref().unwrap_or("").lines()
                                                        .find(|l| l.starts_with("Goal:"))
                                                        .unwrap_or("(see context)")
                                                )),
                                            });
                                        }
                                    }
                                    if let Some(ref mut tracker) = state.execution_tracker {
                                        tracker.reset_plan(new_plan);
                                    }
                                    // Mirror convergence_phase.rs model-initiated replan resets:
                                    // loop guard + adaptive policy must be reset so supervisor-forced
                                    // replans start with a clean slate (not stale escalation state).
                                    state.loop_guard.reset_on_replan();
                                    state.adaptive_policy.reset_after_replan();
                                    state.messages.push(ChatMessage {
                                        role: Role::User,
                                        content: MessageContent::Text(format!(
                                            "[Supervisor replanned]: {reason}\n\
                                             A revised plan is now active. Follow the new plan exactly."
                                        )),
                                    });
                                    true
                                }
                                Ok(None) => {
                                    tracing::warn!("PostBatchSupervisor: supervisor replan returned no plan");
                                    false
                                }
                                Err(e) => {
                                    tracing::warn!("PostBatchSupervisor: supervisor replan error: {e}");
                                    false
                                }
                            }
                        } else {
                            false // no active plan to replan from
                        }
                    } else {
                        false // no planner available
                    };

                    if !supervisor_replan_done {
                        // Fallback: message injection — model must produce a new plan itself.
                        state.messages.push(ChatMessage {
                            role: Role::User,
                            content: MessageContent::Text(format!(
                                "[Supervisor: immediate replan required]\n\
                                 Reason: {reason}\n\
                                 The current plan cannot proceed. Produce a revised plan now \
                                 that addresses this failure. Do not repeat the failed approach."
                            )),
                        });
                    }
                }
                super::super::supervisor::BatchVerdict::SuspendPlugin { plugin_id, reason } => {
                    // Plugin suspension: call suspend_plugin() on the registry to trip the
                    // circuit breaker and prevent any further invocations of this plugin.
                    tracing::warn!(
                        plugin_id = %plugin_id,
                        reason = %reason,
                        "PostBatchSupervisor: suspending plugin"
                    );
                    if let Some(ref arc_pr) = plugin_registry {
                        if let Ok(mut reg) = arc_pr.lock() {
                            reg.suspend_plugin(&plugin_id, reason.clone());
                        }
                    }
                    if !state.silent {
                        render_sink.info(&format!(
                            "[supervisor] plugin suspended: {plugin_id} — {reason}"
                        ));
                    }
                }
            }
        }

        // Phase 43: Check control channel after plan step completion (yield point 3).
        if let Some(ref mut rx) = ctrl_rx {
            match check_control(rx, render_sink).await {
                ControlAction::Continue => {}
                ControlAction::StepOnce => { state.auto_pause = true; }
                ControlAction::Cancel => {
                    state.ctrl_cancelled = true;
                    return Ok(PostBatchOutcome::BreakLoop);
                }
            }
        }

        // Reflexion: evaluate round and generate reflection on non-success.
        if let Some(reflector) = reflector {
            let outcome = super::super::reflexion::Reflector::evaluate_round(&tool_result_blocks);

            // Confidence feedback: if the previous round generated a reflection and
            // this round succeeded, boost that reflection's relevance. If this round
            // also failed, decay it (the advice didn't help).
            if let (Some(prev_id), Some(db)) = (state.last_reflection_id, trace_db) {
                let delta = if matches!(outcome, super::super::reflexion::RoundOutcome::Success) {
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
                state.last_reflection_id = None;
            }

            // Step 8c: Gate reflector on StrategyContext.enable_reflection.
            // DirectExecution strategy for simple tasks skips reflection to reduce latency.
            // Default (no strategy_context) = always reflect (backward compatible).
            let should_reflect = state.strategy_context
                .as_ref()
                .map(|sc| sc.enable_reflection)
                .unwrap_or(true);

            if should_reflect && !matches!(outcome, super::super::reflexion::RoundOutcome::Success) {
                // Phase E5: Transition to Reflecting state.
                if !state.silent {
                    render_sink.agent_state_transition(state.current_fsm_state, "reflecting", "round had issues");
                    state.current_fsm_state = "reflecting";
                }
                render_sink.reflection_started();
                match reflector.reflect(round, &outcome, &state.messages).await {
                    Ok(Some(reflection)) => {
                        render_sink.reflection_complete(&reflection.analysis, 0.0);
                        tracing::info!(
                            round,
                            analysis = %reflection.analysis,
                            "Self-reflection generated"
                        );
                        // Phase 1 Supervisor: queue advice for injection at round N+1 start.
                        state.reflection_injector.push_advice(&reflection.advice);
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
                            let entry = halcon_storage::MemoryEntry {
                                entry_id: reflection_id,
                                session_id: Some(state.session_id),
                                entry_type: halcon_storage::MemoryEntryType::Reflection,
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
                                state.last_reflection_id = Some(reflection_id);
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
                if !state.silent {
                    render_sink.agent_state_transition(state.current_fsm_state, "executing", "reflection complete");
                    state.current_fsm_state = "executing";
                }
            }
        }

        // RC-2 fix: Record tool failures in the tracker and detect repeated patterns.
        for (failed_tool_name, error_msg) in &tool_failures {
            let tripped = state.failure_tracker.record(failed_tool_name, error_msg);
            if tripped {
                tracing::warn!(
                    tool = %failed_tool_name,
                    error_pattern = %ToolFailureTracker::error_pattern(error_msg),
                    "Tool failure circuit breaker tripped — repeated identical failures"
                );
                if !state.silent {
                    render_sink.loop_guard_action("circuit_breaker", &format!("{failed_tool_name}: repeated failures"));
                }
            }
        }

        // Phase 8-B (C4 Bridge): cross-register plugin tool failures into ToolFailureTracker
        // so the environment-error halt logic works uniformly for plugin failures too.
        // When plugin_registry is None (all existing tests) this block is skipped entirely.
        if let Some(ref arc_pr) = plugin_registry {
            if let Ok(pr) = arc_pr.lock() {
                for (failed_tool_name, error_msg) in &tool_failures {
                    if pr.plugin_id_for_tool(failed_tool_name).is_some() {
                        let pattern = if error_msg.contains("circuit") {
                            "plugin_circuit_open"
                        } else if error_msg.contains("budget") {
                            "plugin_budget"
                        } else if error_msg.contains("denied") {
                            "plugin_permission"
                        } else {
                            "plugin_transport"
                        };
                        // Record using the plugin-specific pattern key so ToolFailureTracker
                        // can trip the circuit breaker on repeated plugin failures.
                        state.failure_tracker.record(failed_tool_name, pattern);
                    }
                }
            } // end lock
        }

        // P0-C: Environment-error halt.
        // If every failed tool this round carries an "mcp_unavailable" pattern AND the circuit
        // breaker has tripped for each of them, the MCP environment is persistently dead.
        // Halt immediately — continuing to loop burns rounds against a non-functional env.
        if !tool_failures.is_empty()
            && tool_failures.iter().all(|(tool, err)| {
                ToolFailureTracker::error_pattern(err) == "mcp_unavailable"
                    && state.failure_tracker.is_tripped(tool, err)
            })
        {
            tracing::error!(
                failed_tools = tool_failures.len(),
                "All active MCP tools are persistently unavailable — halting with EnvironmentError"
            );
            if !state.silent {
                render_sink.error(
                    "MCP environment is unavailable: all tool calls failed after circuit breaker threshold",
                    Some("Check MCP server configuration and connectivity"),
                );
            }
            state.environment_error_halt = true;
            return Ok(PostBatchOutcome::BreakLoop);
        }

        // Adaptive replanning: if a tool failed and we have an active plan, attempt replan.
        // Failure outcomes are already recorded by the tracker above.
        // RC-3/RC-4 fix: skip replan for deterministic errors that will never succeed.
        if let (Some(ref mut tracker), Some(planner)) = (&mut state.execution_tracker, planner) {
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
                if state.failure_tracker.is_tripped(failed_tool_name, error_msg) {
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
                            let _ = db.save_plan_steps(&state.session_id, &new_plan).await;
                        }
                        tracker.reset_plan(new_plan);

                        let plan = tracker.plan();
                        let current = tracker.current_step();
                        let plan_section = format_plan_for_prompt(plan, current);
                        if let Some(ref mut sys) = state.cached_system {
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
        //
        // Strategy: head+tail preservation (SOTA 2026).
        // - 60% of budget → start of output (command invocation, early results)
        // - 30% of budget → end of output (final results, errors, summary lines)
        // - 10% reserved for the truncation notice
        //
        // This is strictly superior to naive head-only truncation because tool output
        // often has critical information at the END (exit code, final status, last error).
        // Also uses char-boundary-safe slicing to avoid broken UTF-8 sequences.
        let max_chars = limits.max_tool_output_chars;
        if max_chars > 0 {
            for block in &mut tool_result_blocks {
                if let ContentBlock::ToolResult { content, .. } = block {
                    let char_count = content.chars().count();
                    if char_count > max_chars {
                        let head_chars = max_chars * 60 / 100;
                        let tail_chars = max_chars * 30 / 100;
                        let skipped = char_count.saturating_sub(head_chars + tail_chars);

                        // Collect head and tail as char-boundary-safe slices.
                        let head: String = content.chars().take(head_chars).collect();
                        let tail: String = content
                            .chars()
                            .skip(char_count.saturating_sub(tail_chars))
                            .collect();

                        *content = format!(
                            "{head}\n\n[... {skipped} chars omitted ({char_count} total → {max_chars} budget) ...]\n\n{tail}"
                        );
                    }
                }
            }
        }

        // Add tool results as a user message (Anthropic API requirement).
        let tool_result_msg = ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(tool_result_blocks),
        };
        state.messages.push(tool_result_msg.clone());
        state.context_pipeline.add_message(tool_result_msg.clone());
        session.add_message(tool_result_msg);

        // P1-A: Parallel Batch Failure Escalation.
        // Tool results are now in state.messages (protocol integrity maintained). If every tool in
        // the parallel batch failed and no sequential tool succeeded, another model invocation
        // will produce the same failing plan — force synthesis instead.
        // The `parallel_batch_collapsed` flag was computed before results were consumed above.
        if parallel_batch_collapsed && tool_successes.is_empty() {
            tracing::error!(
                failed = tool_failures.len(),
                "P1-A: parallel batch collapse — 0% success rate, forcing synthesis"
            );
            if !state.silent {
                render_sink.loop_guard_action(
                    "parallel_batch_collapse",
                    &format!(
                        "all {} tool(s) failed this round; forcing synthesis to avoid futile retry",
                        tool_failures.len()
                    ),
                );
            }
            state.forced_synthesis_detected = true;
            // Use force_no_tools so convergence phase collects final round signals before
            // result_assembly sees forced_synthesis_detected and builds the synthesis result.
            state.tool_decision.set_force_next();
            return Ok(PostBatchOutcome::Continue {
                round_tool_log,
                tool_failures,
                tool_successes,
            });
        }

        // P2-D: Inject model-visible deduplication directive when multiple calls were filtered.
        // The synthetic ToolResult blocks (added above) already tell the model each call was
        // a duplicate; this consolidated User message reinforces convergence pressure when
        // the model is trapped in a repetition loop.
        if round_dedup_count > 1 {
            let dedup_note = ChatMessage {
                role: Role::User,
                content: MessageContent::Text(format!(
                    "[System — Deduplication Guard]: {round_dedup_count} tool calls were \
                     filtered as exact duplicates of prior state.rounds. You are repeating \
                     without progress. Stop calling tools you have already used with the \
                     same arguments. Synthesize what you have gathered and respond directly."
                )),
            };
            state.messages.push(dedup_note.clone());
            state.context_pipeline.add_message(dedup_note.clone());
            session.add_message(dedup_note);
        }

        Ok(PostBatchOutcome::Continue {
            round_tool_log,
            tool_failures,
            tool_successes,
        })
}
