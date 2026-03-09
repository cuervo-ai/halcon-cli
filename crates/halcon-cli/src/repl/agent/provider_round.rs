//! Provider round phase: control check, output headroom guard, spinner,
//! speculative pre-execution, provider invocation + streaming + retry,
//! budget guards, non-tool-use path, tool-use path.
//!
//! Called once per round, after `round_setup::run()`, in `run_agent_loop()`.
//! Returns `Ok(ProviderRoundOutcome)` to communicate loop control to the caller.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use futures::StreamExt;
use halcon_core::traits::ModelProvider;
use halcon_core::types::{
    AgentLimits, AgentResult, AgentType, ChatMessage, ContentBlock, DEFAULT_CONTEXT_WINDOW_TOKENS,
    DomainEvent, EventPayload, MessageContent, ModelChunk, ModelRequest, Role, RoutingConfig,
    Session, StopReason, TokenUsage,
};
use halcon_core::EventSender;
use halcon_storage::{AsyncDatabase, InvocationMetric, TraceStepType};
use halcon_tools::ToolRegistry;

use super::super::accumulator::{CompletedToolUse, ToolUseAccumulator};
use super::super::agent_utils::{auto_checkpoint, classify_error_hint, compute_fingerprint, record_trace};
use super::super::resilience::ResilienceManager;
use super::super::round_scorer::RoundEvaluation;
use super::super::tool_speculation::ToolSpeculator;
use super::loop_state::{LoopState, SynthesisOrigin, SynthesisPriority, SynthesisTrigger};
use super::provider_client::{check_control, invoke_with_fallback};
use super::budget_guards;
use crate::render::sink::RenderSink;

use super::super::agent_types::StopCondition;

// ── Phase 3C: XML artifact filter ─────────────────────────────────────────────
//
// Some providers (deepseek-chat, certain Ollama models) emit XML tool-call syntax
// as plain text when the system prompt contains tool-mode instructions but the API
// request has tools=[].  Example:
//
//   <function_calls><invoke name="file_write">...</invoke></function_calls>
//
// This text is never executed (stop_reason=end_turn, not tool_use), but it:
//   1. Contaminates `state.full_text` — pollutes session output shown to the user
//   2. Poisons the response cache — subsequent requests return XML garbage instantly
//   3. Confuses the LoopCritic — critic sees XML as "final response" and rates 15%
//
// The filter removes these artifacts from synthesis-round text before any of the
// downstream consumers (full_text accumulation, cache storage, trace recording).
//
// Pattern coverage:
//   • `<function_calls>` ... `</function_calls>` (Anthropic/halcon XML format)
//   • `<invoke name="...">` ... `</invoke>` (standalone invoke blocks)
//   • `<halcon::tool_call>` ... `</halcon::tool_call>` (legacy halcon format)
//   • Residual `<parameters>`, `<parameter>` tags that follow stripped blocks
//
// The filter is applied ONLY to non-tool-use rounds (synthesis path) to avoid
// accidentally stripping valid tool output from tool-use rounds.

// FASE 7: XML artifact functions moved to `model_quirks` module.
// Aliases for backward-compat within this file.
use super::super::model_quirks::contains_tool_xml_artifacts;


/// Plain-data struct for early returns from the provider round phase.
/// The caller in `mod.rs` reconstructs the full `AgentLoopResult` adding
/// `ctrl_rx` and `plugin_registry` (which must not be moved here).
pub(super) struct ProviderEarlyReturnData {
    pub full_text: String,
    pub rounds: usize,
    pub stop_condition: StopCondition,
    pub call_input_tokens: u64,
    pub call_output_tokens: u64,
    pub call_cost: f64,
    pub latency_ms: u64,
    pub execution_fingerprint: String,
    pub round_evaluations: Vec<RoundEvaluation>,
    pub timeline_json: Option<String>,
    /// RC-2: set to `Some(model_name)` on budget-forced exits to preserve last model used.
    pub last_model_used: Option<String>,
    /// RC-2: plan completion ratio at time of early return (0.0 for non-budget exits).
    pub plan_completion_ratio: f32,
}

/// Data produced by the provider round that is needed by `post_batch` and
/// `convergence_phase`.
pub(super) struct ProviderRoundOutput {
    pub completed_tools: Vec<CompletedToolUse>,
    pub round_model_name: String,
    pub round_provider_name: String,
    pub round_usage: TokenUsage,
    pub round_text_for_scorer: String,
}

/// Outcome of the provider round phase.
pub(super) enum ProviderRoundOutcome {
    /// Break `'agent_loop` (non-tool-use path or headroom/ctrl guard).
    BreakLoop,
    /// Early return from `run_agent_loop` (provider error, budget exceeded, etc.).
    EarlyReturn(Box<ProviderEarlyReturnData>),
    /// Tool-use path — continue to post_batch phase.
    ToolUse(ProviderRoundOutput),
}

/// Run the provider invocation phase for one iteration of the agent loop.
///
/// `ctrl_rx` is passed as `&mut Option<ControlReceiver>` rather than being
/// moved, so it remains available to the caller for the final `AgentLoopResult`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    state: &mut LoopState,
    session: &mut Session,
    render_sink: &dyn RenderSink,
    effective_provider: &Arc<dyn ModelProvider>,
    mut round_request: ModelRequest,
    fallback_providers: &[(String, Arc<dyn ModelProvider>)],
    resilience: &mut ResilienceManager,
    routing_config: &RoutingConfig,
    event_tx: &EventSender,
    trace_db: Option<&AsyncDatabase>,
    limits: &AgentLimits,
    guardrails: &[Box<dyn halcon_security::Guardrail>],
    provider: &Arc<dyn ModelProvider>,
    request: &ModelRequest,
    exec_clock: &halcon_core::types::ExecutionClock,
    round_start: Instant,
    ctrl_rx: &mut Option<super::super::agent_types::ControlReceiver>,
    speculator: &ToolSpeculator,
    tool_registry: &ToolRegistry,
    working_dir: &str,
    round: usize,
    selected_model: &str,
    model_selector: Option<&super::super::model_selector::ModelSelector>,
    response_cache: Option<&super::super::response_cache::ResponseCache>,
    replay_tool_executor: Option<&super::super::replay_executor::ReplayToolExecutor>,
) -> Result<ProviderRoundOutcome> {
    // Reset stream renderer state for a new round.
    if !state.silent { render_sink.stream_reset(); }
    let mut silent_text = String::new(); // text accumulator for state.silent mode
    let mut accumulator = ToolUseAccumulator::new();
    let mut stop_reason = StopReason::EndTurn;

    // Track actual provider/model used this round (may differ from request due to fallback).
    // Assigned inside the 'invoke_retry Ok(attempt) branch before first use — the dummy
    // String::new() initial value is never read (Rust requires initialization before use).
    #[allow(unused_assignments)]
    let mut round_provider_name = String::new();
    let round_model_name = round_request.model.clone();
    state.last_round_model_name = round_model_name.clone(); // Phase 4: track for post-loop quality recording
    // Track the actual provider Arc for cost estimation (updated on fallback).
    let mut round_cost_provider: Arc<dyn ModelProvider> = Arc::clone(effective_provider);

    // Phase 43: Check control channel before model invocation (yield point 1).
    #[cfg(feature = "tui")]
    if let Some(ref mut rx) = ctrl_rx {
        // If state.auto_pause is set (from previous StepOnce), pause before this round.
        if state.auto_pause {
            state.auto_pause = false;
            render_sink.info("  [paused] Step complete — Space to resume, N to step");
            loop {
                match rx.recv().await {
                    Some(crate::tui::events::ControlEvent::Resume) => break,
                    Some(crate::tui::events::ControlEvent::Step) => {
                        state.auto_pause = true;
                        break;
                    }
                    Some(crate::tui::events::ControlEvent::CancelAgent) | None => {
                        state.ctrl_cancelled = true;
                        break;
                    }
                    _ => continue,
                }
            }
            if state.ctrl_cancelled {
                return Ok(ProviderRoundOutcome::BreakLoop);
            }
        }
        match check_control(rx, render_sink).await {
            super::ControlAction::Continue => {}
            super::ControlAction::StepOnce => { state.auto_pause = true; }
            super::ControlAction::Cancel => {
                state.ctrl_cancelled = true;
                return Ok(ProviderRoundOutcome::BreakLoop);
            }
        }
    }

    // GAP-5: Classic REPL (non-TUI) cancel check.
    // try_recv() is non-blocking — it only fires if Ctrl-C was already pressed.
    #[cfg(not(feature = "tui"))]
    if let Some(ref mut rx) = ctrl_rx {
        if rx.try_recv().is_ok() {
            tracing::info!(round, "Classic REPL: Ctrl-C received — cancelling agent loop gracefully");
            render_sink.info("Session cancelled by user (Ctrl-C). Partial results saved.");
            state.ctrl_cancelled = true;
            return Ok(ProviderRoundOutcome::BreakLoop);
        }
    }

    // Pre-invocation output headroom guard (RC-1a).
    //
    // Prevent mid-word response truncation by refusing to invoke the model when the
    // remaining token budget is insufficient for a complete response.  The provider
    // will truncate its output the moment cumulative usage exceeds max_total_tokens,
    // so we must verify BEFORE streaming that there is meaningful room left.
    //
    // MIN_OUTPUT_HEADROOM_TOKENS = 5 000 tokens (≈ 20 KB of text, enough for a
    // complete synthesis response on typical tasks).  If remaining < this value,
    // force an early synthesis instead of starting a new model invocation.
    // Guard only fires when `used > 0` (at least one round has been completed).
    // On round 0, the full budget is available and no truncation risk exists.
    let min_output_headroom = state.policy.output_headroom_tokens as u64;
    if limits.max_total_tokens > 0 {
        let used = session.total_usage.total() as u64;
        let budget = limits.max_total_tokens as u64;
        let remaining = budget.saturating_sub(used);
        if used > 0 && remaining < min_output_headroom {
            tracing::warn!(
                used,
                budget,
                remaining,
                "Output headroom below minimum — forcing synthesis to prevent truncation"
            );
            if !state.silent {
                render_sink.warning(
                    &format!(
                        "output headroom critical ({remaining} tokens remaining of {budget}) \
                         — synthesizing early to prevent truncation"
                    ),
                    Some("Increase max_total_tokens for complex tasks"),
                );
            }
            // Phase 2: route through governance gate (response cache failure → tool exhaustion).
            state.request_synthesis_with_gate(
                SynthesisTrigger::ToolExhaustion,
                SynthesisOrigin::CacheCorruption,
                SynthesisPriority::High,
            );
            // EBS-R1 (OutputHeadroomCritical): if evidence gate fires, override origin to
            // SupervisorFailure so reward pipeline applies synthesis penalty.
            if state.evidence.bundle.evidence_gate_fires() {
                state.evidence.bundle.synthesis_blocked = true;
                state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                tracing::warn!(
                    session_id = %state.session_id,
                    text_bytes_extracted = state.evidence.bundle.text_bytes_extracted,
                    "EvidenceGate FIRED (OutputHeadroomCritical): origin overridden to SupervisorFailure"
                );
            }
            return Ok(ProviderRoundOutcome::BreakLoop);
        }
    }

    // ── EBS-B2: Deterministic Pre-Invocation Synthesis Gate (BRECHA-2 fix) ─────────────────
    //
    // Target: model-initiated EndTurn synthesis on evidence-failing sessions.
    //
    // Scenario: coordinator called read_file (or similar) on files that turned out to be
    // binary (PDF, images) or empty → EvidenceBundle.text_bytes_extracted < MIN_EVIDENCE_BYTES
    // → convergence oracle may have already fired EBS-1/EBS-2 and set synthesis_blocked, OR
    // the model may EndTurn on its own without the oracle forcing synthesis (BRECHA-2).
    //
    // When ALL of these hold:
    //   (a) This is a synthesis/text round: round_request.tools.is_empty()
    //   (b) Evidence gate fires: content-read tools ran but extracted < threshold
    //   (c) Gate not already handled: synthesis_blocked == false
    //       (EBS-1/EBS-2/EBS-R1 set synthesis_blocked when they fire — skip double-intercept)
    //
    // Action: skip the LLM call entirely. Produce the limitation notice directly and
    // return BreakLoop. The model never sees the synthesis request; no token cost; no
    // streaming. This is deterministic: evidence-anchoring cannot be bypassed by
    // model-initiated EndTurn even when the convergence oracle chose not to force synthesis.
    //
    // NOT a dependency on LoopCritic (probabilistic). LoopCritic remains as second line.
    if round_request.tools.is_empty()
        && state.evidence.bundle.evidence_gate_fires()
        && !state.evidence.bundle.synthesis_blocked
    {
        use super::super::evidence_pipeline::MIN_EVIDENCE_BYTES;
        let gate_msg = state.evidence.bundle.gate_message();
        state.evidence.bundle.synthesis_blocked = true;
        state.evidence.deterministic_boundary_enforced = true;
        // Phase 2: route through governance gate (EBS-B2 boundary, tool-free round).
        state.request_synthesis_with_gate(
            SynthesisTrigger::ToolExhaustion,
            SynthesisOrigin::SupervisorFailure,
            SynthesisPriority::Critical,
        );
        tracing::warn!(
            session_id = %state.session_id,
            round,
            text_bytes_extracted = state.evidence.bundle.text_bytes_extracted,
            content_read_attempts = state.evidence.bundle.content_read_attempts,
            binary_file_count = state.evidence.bundle.binary_file_count,
            min_threshold = MIN_EVIDENCE_BYTES,
            "EBS-B2: Pre-invocation synthesis gate fired — LLM call skipped, \
             limitation notice injected directly (BRECHA-2 deterministic fix)"
        );
        if !state.silent {
            render_sink.warning(
                "[evidence-gate] synthesis blocked before LLM invocation — files returned no readable text",
                Some("Files appear to be binary (PDF) or inaccessible. \
                      Limitation notice generated without model call."),
            );
            render_sink.stream_text(&gate_msg);
            render_sink.stream_done();
        }
        state.full_text.push_str(&gate_msg);
        let msg = ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text(gate_msg.clone()),
        };
        state.messages.push(msg.clone());
        state.context_pipeline.add_message(msg.clone());
        session.add_message(msg);
        state.rounds = round + 1;
        session.agent_rounds += 1;
        return Ok(ProviderRoundOutcome::BreakLoop);
    }
    // ── End EBS-B2 ───────────────────────────────────────────────────────────────────────────

    // Show spinner during model inference (appears after 200ms delay).
    if !state.silent {
        let label = if routing_config.mode == "speculative" && !fallback_providers.is_empty() {
            let count = 1 + fallback_providers.len();
            format!("Racing {count} providers...")
        } else {
            format!("Thinking... [{}]", effective_provider.name())
        };
        render_sink.spinner_start(&label);
    }
    let mut spinner_active = !state.silent;

    // Speculative tool pre-execution: predict read-only tools the model will
    // likely call and pre-execute them in background while the model streams.
    if replay_tool_executor.is_none() {
        let spec_count = speculator
            .speculate(&state.messages, tool_registry, working_dir)
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

    let mut round_usage = TokenUsage::default();

    'invoke_retry: loop {
    let invoke_attempt = tokio::time::timeout(
        provider_timeout,
        invoke_with_fallback(
            effective_provider,
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
                    session_id: Some(state.session_id.to_string()),
                    created_at: Utc::now(),
                };
                if let Err(me) = db.inner().insert_metric(&metric) {
                    tracing::warn!("Failed to persist timeout metric: {me}");
                }
            }
            record_trace(
                trace_db,
                state.session_id,
                &mut state.trace_step_index,
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
                if !state.silent {
                    render_sink.warning(
                        "provider timed out, retrying...",
                        None,
                    );
                }
                tokio::time::sleep(Duration::from_secs(2u64.pow(round_retry_count))).await;
                spinner_active = !state.silent;
                continue 'invoke_retry;
            }
            if !state.silent {
                render_sink.error(
                    &format!("provider timed out after {}s", limits.provider_timeout_secs),
                    Some("Increase provider_timeout_secs or check network connectivity"),
                );
            }
            // P3 FIX: Emit AgentCompleted on early return (provider timeout).
            let _ = event_tx.send(DomainEvent::new(EventPayload::AgentCompleted {
                agent_type: AgentType::Chat,
                result: AgentResult {
                    success: false,
                    summary: format!("ProviderError: timeout after {}s", limits.provider_timeout_secs),
                    files_modified: vec![],
                    tools_used: vec![],
                },
            }));
            return Ok(ProviderRoundOutcome::EarlyReturn(Box::new(ProviderEarlyReturnData {
                full_text: state.full_text.clone(),
                rounds: state.rounds,
                stop_condition: StopCondition::ProviderError,
                call_input_tokens: state.tokens.call_input_tokens,
                call_output_tokens: state.tokens.call_output_tokens,
                call_cost: state.tokens.call_cost,
                latency_ms: state.loop_start.elapsed().as_millis() as u64,
                execution_fingerprint: compute_fingerprint(&round_request.messages),
                round_evaluations: state.convergence.round_evaluations.clone(),
                timeline_json: None,
                last_model_used: None,
                plan_completion_ratio: 0.0,
            })));
        }
    };

    match invoke_attempt {
        Ok(attempt) => {
            let _permit = attempt.permit;
            let used_provider_name = attempt.provider_name.clone();
            round_provider_name = attempt.provider_name.clone();
            if attempt.is_fallback {
                if !state.silent {
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
                                "Adapted model for fallback provider on subsequent state.rounds"
                            );
                            if !state.silent {
                                render_sink.model_selected(&default_model.id, &attempt.provider_name, "adapted for fallback provider");
                            }
                            round_request.model = default_model.id.clone();
                            state.fallback_adapted_model = Some(default_model.id.clone());
                        }
                    }

                    // ── Dynamic Budget Reconciliation ──────────────────────────────────
                    // The state.tokens.pipeline_budget was computed pre-loop from the PRIMARY provider's
                    // context_window. After fallback to a provider with a SMALLER window
                    // (e.g., Anthropic 200K → Ollama 32K), the old budget is too large:
                    // L0 alone (40% × 200K = 80K) would exceed Ollama's full context window.
                    //
                    // Reconciliation: look up the fallback model's context_window, recompute
                    // the budget, and propagate the change to the pipeline's TokenAccountant.
                    // This prevents context overflow on the NEXT round's model invocation.
                    let fallback_context_window: u32 = fb_prov
                        .supported_models()
                        .iter()
                        .find(|m| m.id == round_request.model)
                        .map(|m| m.context_window)
                        .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS);
                    let new_pipeline_budget = {
                        let input_fraction = (fallback_context_window as f64 * 0.80) as u32;
                        if limits.max_total_tokens > 0 {
                            input_fraction.min(limits.max_total_tokens)
                        } else {
                            input_fraction
                        }
                    };
                    if new_pipeline_budget != state.tokens.pipeline_budget {
                        tracing::info!(
                            old_budget = state.tokens.pipeline_budget,
                            new_budget = new_pipeline_budget,
                            fallback_context_window,
                            provider = %attempt.provider_name,
                            model = %round_request.model,
                            "Dynamic Budget Reconciliation: adjusting pipeline budget for fallback provider"
                        );
                        state.tokens.pipeline_budget = new_pipeline_budget;
                        state.context_pipeline.update_budget(new_pipeline_budget);
                    }
                    // Keep state.compaction_model in sync with the now-active model.
                    state.compaction_model = round_request.model.clone();
                }
            }
            let mut stream = attempt.stream;
            let mut stream_had_error = false;
            // FIX: track Done separately so we can drain post-Done chunks (e.g. the
            // OpenAI-compat Usage chunk that DeepSeek/OpenAI send AFTER the finish_reason
            // chunk but BEFORE [DONE]). Without this drain, output_tokens stays 0 because
            // the Usage chunk arrives after Done but the old code broke immediately on Done.
            let mut stream_done_seen = false;
            let cancelled = loop {
                tokio::select! {
                    chunk_opt = stream.next() => {
                        match chunk_opt {
                            Some(Ok(chunk)) => {
                                // Stop spinner on first non-thinking content.
                                // ThinkingDelta keeps the spinner alive so its label can be
                                // updated live ("Razonando... N chars") during reasoning.
                                // Spinner stops when actual text/tool/error begins streaming.
                                if spinner_active
                                    && matches!(
                                        chunk,
                                        ModelChunk::TextDelta(_)
                                            | ModelChunk::ToolUseStart { .. }
                                            | ModelChunk::Error(_)
                                    )
                                {
                                    render_sink.spinner_stop();
                                    spinner_active = false;
                                }
                                // Track usage (session cumulative + per-round).
                                // Must happen BEFORE render so token_delta() reflects
                                // any Usage chunk that arrives after Done.
                                if let ModelChunk::Usage(ref u) = chunk {
                                    session.total_usage.input_tokens += u.input_tokens;
                                    session.total_usage.output_tokens += u.output_tokens;
                                    round_usage.input_tokens += u.input_tokens;
                                    round_usage.output_tokens += u.output_tokens;
                                    // Propagate reasoning_tokens for cost transparency.
                                    // Thinking tokens are billed as output tokens but tracked
                                    // separately so the status bar can display "🧠 N tok".
                                    if let Some(rt) = u.reasoning_tokens {
                                        *session.total_usage.reasoning_tokens.get_or_insert(0) += rt;
                                        *round_usage.reasoning_tokens.get_or_insert(0) += rt;
                                    }
                                    // Phase 45B: Emit real-time token delta for live status bar.
                                    if !state.silent {
                                        render_sink.token_delta(
                                            round_usage.input_tokens,
                                            round_usage.output_tokens,
                                            session.total_usage.input_tokens,
                                            session.total_usage.output_tokens,
                                        );
                                    }
                                    // If we already saw Done, this was the post-Done Usage
                                    // chunk (standard OpenAI include_usage behavior). Break now.
                                    if stream_done_seen {
                                        break false;
                                    }
                                }
                                // Capture stop reason.
                                if let ModelChunk::Done(reason) = &chunk {
                                    stop_reason = *reason;
                                }
                                // Feed to accumulator first.
                                accumulator.process(&chunk);
                                // Render via sink (or silently accumulate).
                                if !state.silent {
                                    match &chunk {
                                        ModelChunk::TextDelta(t) => render_sink.stream_text(t),
                                        // ThinkingDelta: visual distinction (dim/italic).
                                        // NOT added to state.full_text — thinking stays out of
                                        // episodic memory and plan-completion scoring.
                                        ModelChunk::ThinkingDelta(t) => render_sink.stream_thinking(t),
                                        ModelChunk::ToolUseStart { name, .. } => render_sink.stream_tool_marker(name),
                                        ModelChunk::Error(msg) => render_sink.stream_error(msg),
                                        ModelChunk::Done(_) => {
                                            render_sink.stream_done();
                                            // Don't break yet — a Usage chunk may follow.
                                            stream_done_seen = true;
                                        }
                                        _ => {}
                                    }
                                } else {
                                    // Silent: accumulate text, detect done.
                                    // ThinkingDelta intentionally excluded — same as non-state.silent.
                                    if let ModelChunk::TextDelta(t) = &chunk {
                                        silent_text.push_str(t);
                                    }
                                    if matches!(chunk, ModelChunk::Done(_)) {
                                        // Don't break yet — a Usage chunk may follow.
                                        stream_done_seen = true;
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                if !state.silent {
                                    render_sink.stream_error(&format!("{e}"));
                                }
                                // Record stream failure for health scoring.
                                if resilience.is_enabled() {
                                    resilience.record_failure(&used_provider_name).await;
                                    // Phase E3/E4: emit provider health as degraded after failure.
                                    if !state.silent {
                                        render_sink.provider_health_update(
                                            &used_provider_name, "degraded", 0.0,
                                            round_start.elapsed().as_millis() as u64,
                                        );
                                    }
                                }
                                stream_had_error = true;
                                break false;
                            }
                            // Stream exhausted (includes post-[DONE] None) — always safe to exit.
                            None => break false,
                        }
                    }
                    _ = tokio::signal::ctrl_c() => {
                        break true;
                    }
                }
            };

            // P0 FIX: Stream finalization barrier.
            // Guarantee spinner_stop() runs whenever the stream exits — regardless of
            // whether the stream was empty, hit a guardrail, was cancelled, or had an
            // error. Without this, an empty response (Done with no prior TextDelta or
            // ToolUseStart) left the spinner active forever.
            if spinner_active {
                render_sink.spinner_stop();
                spinner_active = false;
            }

            if cancelled {
                if !state.silent { render_sink.warning("response interrupted by user", None); }
                drop(stream);
                // P3 FIX: Emit AgentCompleted on early return (user cancellation).
                let _ = event_tx.send(DomainEvent::new(EventPayload::AgentCompleted {
                    agent_type: AgentType::Chat,
                    result: AgentResult {
                        success: false,
                        summary: format!("Interrupted: user cancelled at round {round}"),
                        files_modified: vec![],
                        tools_used: vec![],
                    },
                }));
                return Ok(ProviderRoundOutcome::EarlyReturn(Box::new(ProviderEarlyReturnData {
                    full_text: state.full_text.clone(),
                    rounds: state.rounds,
                    stop_condition: StopCondition::Interrupted,
                    call_input_tokens: state.tokens.call_input_tokens,
                    call_output_tokens: state.tokens.call_output_tokens,
                    call_cost: state.tokens.call_cost,
                    latency_ms: state.loop_start.elapsed().as_millis() as u64,
                    execution_fingerprint: compute_fingerprint(&round_request.messages),
                    round_evaluations: state.convergence.round_evaluations.clone(),
                    timeline_json: None,
                    last_model_used: None,
                    plan_completion_ratio: 0.0,
                })));
            }

            // Resilience: record success for the provider that was used.
            if resilience.is_enabled() && !stream_had_error {
                resilience.record_success(&used_provider_name).await;
                // Phase E3/E4: emit provider health as healthy after success.
                if !state.silent {
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
                        session_id: Some(state.session_id.to_string()),
                        created_at: Utc::now(),
                    };
                    if let Err(me) = db.inner().insert_metric(&metric) {
                        tracing::warn!("Failed to persist stream error metric: {me}");
                    }
                }
                if round_retry_count < MAX_ROUND_RETRIES {
                    round_retry_count += 1;
                    tracing::info!(retry = round_retry_count, "Retrying round after stream error");
                    if !state.silent {
                        render_sink.warning(
                            "stream error, retrying...",
                            None,
                        );
                    }
                    // Reset round-level accumulators for retry.
                    accumulator = ToolUseAccumulator::new();
                    if !state.silent { render_sink.stream_reset(); }
                    silent_text.clear();
                    round_usage = TokenUsage::default();
                    spinner_active = !state.silent;
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
                state.session_id,
                &mut state.trace_step_index,
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
                    session_id: Some(state.session_id.to_string()),
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
                if !state.silent {
                    render_sink.warning(
                        &format!("provider error, retrying... ({e})"),
                        None,
                    );
                }
                spinner_active = !state.silent;
                tokio::time::sleep(Duration::from_secs(2u64.pow(round_retry_count))).await;
                continue 'invoke_retry;
            }
            if !state.silent {
                render_sink.info("");
                let hint = classify_error_hint(&format!("{e}"));
                render_sink.error(
                    &format!("provider request failed — {e}"),
                    Some(hint),
                );
            }
            // P3 FIX: Emit AgentCompleted on early return (provider request failure).
            let _ = event_tx.send(DomainEvent::new(EventPayload::AgentCompleted {
                agent_type: AgentType::Chat,
                result: AgentResult {
                    success: false,
                    summary: format!("ProviderError: {e}"),
                    files_modified: vec![],
                    tools_used: vec![],
                },
            }));
            return Ok(ProviderRoundOutcome::EarlyReturn(Box::new(ProviderEarlyReturnData {
                full_text: state.full_text.clone(),
                rounds: state.rounds,
                stop_condition: StopCondition::ProviderError,
                call_input_tokens: state.tokens.call_input_tokens,
                call_output_tokens: state.tokens.call_output_tokens,
                call_cost: state.tokens.call_cost,
                latency_ms: state.loop_start.elapsed().as_millis() as u64,
                execution_fingerprint: compute_fingerprint(&round_request.messages),
                round_evaluations: state.convergence.round_evaluations.clone(),
                timeline_json: None,
                last_model_used: None,
                plan_completion_ratio: 0.0,
            })));
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
    state.tokens.call_input_tokens += round_usage.input_tokens as u64;
    state.tokens.call_output_tokens += round_usage.output_tokens as u64;
    state.tokens.call_cost += round_cost.estimated_cost_usd;

    // HICON Phase 3: Feed token metrics to Bayesian detector
    state.guards.loop_guard.update_token_counts(
        round_usage.input_tokens as u64,
        round_usage.output_tokens as u64,
        (round_usage.input_tokens + round_usage.output_tokens) as u64,
    );

    // HICON Phase 5: Feed metrics to ARIMA predictor for resource forecasting
    state.hicon.resource_predictor.observe(
        round + 1,
        round_usage.input_tokens as u64,
        round_usage.output_tokens as u64,
        round_cost.estimated_cost_usd,
    );

    // HICON Phase 5: Budget overflow detection (check every 5 rounds)
    if state.hicon.resource_predictor.is_ready() && (round + 1) % 5 == 0 {
        let prediction = state.hicon.resource_predictor.predict_resources(5); // Predict next 5 state.rounds

        // Check token budget overflow
        if let Some(total_tokens) = prediction.total_tokens_mean() {
            let projected_total = state.tokens.call_input_tokens + state.tokens.call_output_tokens + total_tokens as u64;
            let token_limit = limits.max_total_tokens;
            if token_limit > 0 && projected_total > token_limit as u64 {
                tracing::warn!(
                    round = round + 1,
                    current_tokens = state.tokens.call_input_tokens + state.tokens.call_output_tokens,
                    predicted_total = projected_total,
                    limit = token_limit,
                    "ARIMA: Token budget overflow predicted within 5 state.rounds"
                );
                // Remediation Phase 1.2: Make ARIMA warnings visible to user
                render_sink.hicon_budget_warning(
                    5,
                    state.tokens.call_input_tokens + state.tokens.call_output_tokens,
                    projected_total,
                );
            }
        }

        // Check cost budget overflow (if budget configured)
        if let Some(total_cost) = prediction.total_cost_mean() {
            let projected_cost = state.tokens.call_cost + total_cost;
            // Note: Cost budget not in limits struct yet, would need AgentConfig integration
            tracing::debug!(
                round = round + 1,
                current_cost = state.tokens.call_cost,
                predicted_total = projected_cost,
                "ARIMA: Cost projection"
            );
        }
    }

    if round_cost.estimated_cost_usd > 0.0 {
        tracing::debug!(
            cost = format!("${:.4}", round_cost.estimated_cost_usd),
            cumulative = format!("${:.4}", session.estimated_cost_usd),
            "Round cost"
        );
    }

    // Emit round-end metrics to sink.
    // When provider didn't emit ModelChunk::Usage (some DeepSeek/Ollama configs),
    // fall back to pre-computed token estimate so the status bar shows non-zero values.
    if !state.silent {
        let report_input = if round_usage.input_tokens > 0 {
            round_usage.input_tokens
        } else {
            // Estimate-based fallback: cost estimator already computed this from message sizes.
            round_cost.estimated_input_tokens
        };
        // Patch session totals with estimation when actual usage was missing.
        if round_usage.input_tokens == 0 && report_input > 0 {
            session.total_usage.input_tokens += report_input;
        }
        render_sink.round_ended(
            round + 1,
            report_input,
            round_usage.output_tokens,
            round_cost.estimated_cost_usd,
            round_latency_ms,
        );
    }

    // Phase E2: Emit token budget update after each round.
    // Always emit — use model's context window as limit when max_total_tokens is 0.
    // This makes the budget bar useful even without explicit token limits configured.
    if !state.silent {
        let used_tokens = session.total_usage.total() as u64;
        let limit_tokens = if limits.max_total_tokens > 0 {
            limits.max_total_tokens as u64
        } else {
            // Fallback: use model's declared context window (e.g. 64k, 128k, 200k).
            effective_provider
                .model_context_window(selected_model)
                .unwrap_or(128_000) as u64
        };
        let elapsed_secs = state.loop_start.elapsed().as_secs_f64().max(0.001);
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
            session_id: Some(state.session_id.to_string()),
            created_at: Utc::now(),
        };
        if let Err(e) = db.inner().insert_metric(&metric) {
            tracing::warn!("Failed to persist invocation metric: {e}");
        }

        // Advisory optimizer logging: recommend optimal model for this workload.
        if let Ok(sys) = db.inner().system_metrics() {
            let ranked = super::super::optimizer::CostLatencyOptimizer::rank_from_metrics(
                &sys,
                super::super::optimizer::OptimizeStrategy::from_str(&routing_config.strategy),
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

    // Phase 1.3: Feed observed round latency back into ModelSelector's live override map.
    // This closes the Optimizer → Routing feedback loop: the "fast" strategy now uses
    // EMA-smoothed live latency instead of stale DB p95 from prior sessions.
    // model_selector is Option<&ModelSelector> — record_observed_latency() uses interior
    // mutability (Mutex<HashMap>) so this works without &mut.
    if let Some(sel) = model_selector {
        sel.record_observed_latency(&round_model_name, round_latency_ms);
    }

    // Accumulate text from this round.
    let round_text = if !state.silent {
        render_sink.stream_full_text()
    } else {
        std::mem::take(&mut silent_text)
    };

    // Phase 3C: apply XML artifact filter before accumulating into state.full_text.
    // Only applied on non-tool-use rounds (synthesis path) where XML in text is always
    // a hallucination artifact.  Tool-use rounds go through the ToolUse branch below
    // and never reach this code path.
    // Note: `round_request.tools` reflects the tool list sent to the provider this round.
    // When tools=[], any XML tool syntax in the response is spurious.
    let round_text_clean: std::borrow::Cow<'_, str> = if round_request.tools.is_empty() {
        let filtered = super::super::model_quirks::strip_tool_xml_artifacts(&round_text);
        if matches!(filtered, std::borrow::Cow::Owned(_)) {
            tracing::warn!(
                round,
                provider = %round_provider_name,
                model = %round_model_name,
                "Phase 3C: stripped XML tool-call artifacts from synthesis round text \
                 (tools=[] but provider emitted tool XML in end_turn response)"
            );
            if !state.silent {
                render_sink.warning(
                    "[synthesis] XML tool artifacts removed from response \
                     (provider emitted tool syntax in text-only round)",
                    Some("This is a provider hallucination — the model ignored tools=[] context"),
                );
            }
        }
        filtered
    } else {
        std::borrow::Cow::Borrowed(&round_text)
    };
    state.full_text.push_str(&round_text_clean);
    // Phase 2: save a copy for RoundScorer coherence scoring (round_text may be moved later).
    let round_text_for_scorer = round_text_clean.as_ref().to_string();

    // Guardrail post-invocation check on model output.
    if !guardrails.is_empty() && !round_text.is_empty() {
        let violations = halcon_security::run_guardrails(
            guardrails,
            &round_text,
            halcon_security::GuardrailCheckpoint::PostInvocation,
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
        if halcon_security::has_blocking_violation(&violations) {
            if !state.silent { render_sink.info("\n[response blocked by guardrail]"); }
            return Ok(ProviderRoundOutcome::BreakLoop);
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
    //
    // Phase 3C: skip caching when the synthesis response contains XML tool artifacts.
    // Caching a contaminated response would poison future requests with the same
    // conversation fingerprint, causing them to receive XML garbage instantly (5s)
    // without any tool execution.  Better to re-invoke the provider on the next request.
    if let Some(cache) = response_cache {
        let has_xml_artifacts = contains_tool_xml_artifacts(&round_text);
        if has_xml_artifacts {
            tracing::warn!(
                round,
                "Phase 3C: skipping response cache — XML tool artifacts detected in synthesis text \
                 (caching would poison future requests with the same conversation fingerprint)"
            );
        } else {
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
    }

    // Note: state.messages Vec is preserved (not moved into round_request).
    // Pipeline manages L0-L4 context; state.messages Vec is full history for fingerprinting.


    // --- Budget guards (token / duration / cost) ---
    // Extracted to budget_guards.rs for clarity. Returns Some(StopCondition) on first breach.
    if let Some(stop) = budget_guards::check(
        limits,
        session,
        state.loop_start,
        state.silent,
        render_sink,
        &round_text,
        &mut state.messages,
        &mut state.context_pipeline,
    ) {
        // Record the deferred trace before exiting — budget exits would otherwise skip it.
        record_trace(
            trace_db, state.session_id, &mut state.trace_step_index,
            TraceStepType::ModelResponse,
            serde_json::json!({
                "round": pending_trace_round,
                "text": &pending_trace_text,
                "stop_reason": &pending_trace_stop,
                "usage": { "input_tokens": pending_trace_usage.input_tokens, "output_tokens": pending_trace_usage.output_tokens },
                "latency_ms": pending_trace_latency,
                "tool_uses": [],
                "budget_exit": true,
            }).to_string(),
            pending_trace_latency,
            exec_clock,
        );
        // RC-2: Capture actual plan progress on budget-forced exit.
        // The post-loop calculation is unreachable on early returns; compute inline.
        let budget_exit_plan_ratio = state.execution_tracker.as_ref().map(|t| {
            let (completed, total, _) = t.progress();
            if total > 0 { completed as f32 / total as f32 } else { 0.0 }
        }).unwrap_or(0.0);
        return Ok(ProviderRoundOutcome::EarlyReturn(Box::new(ProviderEarlyReturnData {
            full_text: state.full_text.clone(),
            rounds: state.rounds,
            stop_condition: stop,
            call_input_tokens: state.tokens.call_input_tokens,
            call_output_tokens: state.tokens.call_output_tokens,
            call_cost: state.tokens.call_cost,
            latency_ms: state.loop_start.elapsed().as_millis() as u64,
            execution_fingerprint: compute_fingerprint(&state.messages),
            round_evaluations: state.convergence.round_evaluations.clone(),
            timeline_json: state.execution_tracker.as_ref().map(|t| t.to_json().to_string()),
            last_model_used: Some(state.last_round_model_name.clone()),
            plan_completion_ratio: budget_exit_plan_ratio,
        })));
    }

    // ── FASE 2: Alternative format recovery ────────────────────────────────
    // Some providers emit tool calls as text in alternative XML formats (e.g. DSML)
    // instead of structured ToolUseStart/ToolUseDelta chunks. When `stop_reason`
    // is EndTurn but the text contains a recoverable format, extract the tool calls
    // and redirect to the tool-use path. Provider-agnostic: any model that emits
    // these patterns gets the same treatment. P1-B still applies.
    //
    // FSM GUARD: skip format-recovery when already in Synthesizing phase.
    // Injecting tool execution during Synthesizing causes an invalid
    // Synthesizing→Executing transition (race condition: GovernanceRescue fired
    // just before the provider responded with XML tool calls). Let the round
    // fall through as EndTurn so synthesis can proceed uninterrupted.
    let fsm_in_synthesizing = state.synthesis.phase() == super::loop_state::AgentPhase::Synthesizing;
    if fsm_in_synthesizing {
        tracing::warn!(
            round,
            "FASE 2: format-recovery SUPPRESSED — FSM is Synthesizing; XML tool calls discarded to preserve FSM invariant"
        );
        if !state.silent {
            render_sink.warning("[format-recovery] suppressed: synthesis in progress — tool calls deferred", None);
        }
    }
    if stop_reason != StopReason::ToolUse && !fsm_in_synthesizing {
        if let Some(recovered) = super::super::model_quirks::try_recover_any_tool_call(&round_text) {
            tracing::info!(
                round,
                recovered_count = recovered.len(),
                "FASE 2: recovered tool calls from alternative text format — redirecting to tool-use path"
            );
            if !state.silent {
                render_sink.info(&format!(
                    "[format-recovery] recovered {} tool call(s) from text format",
                    recovered.len()
                ));
            }
            // Convert recovered calls to CompletedToolUse and redirect to tool-use path.
            // Use synthetic IDs since the provider didn't generate structured ones.
            let mut synthetic_tools: Vec<super::super::accumulator::CompletedToolUse> = Vec::new();
            for (i, call) in recovered.iter().enumerate() {
                synthetic_tools.push(super::super::accumulator::CompletedToolUse {
                    id: format!("recovered_{round}_{i}"),
                    name: call.name.clone(),
                    input: call.input.clone(),
                });
            }

            // RP-2: Validate recovered tool names against the round's allowed tool surface.
            // DeepSeek DSML recovery can produce a tool name that is NOT in the sub-agent's
            // narrowed `round_request.tools` (e.g., model emits `directory_tree` but the
            // planner allocated only `read_multiple_files`). The executor will silently reject
            // any tool not in the allowed surface, producing 0 tool executions.
            //
            // When exactly one tool is allowed and the recovered name is wrong, remap to the
            // intended tool. This handles the common "model knows one allowed tool but generates
            // DSML for a different tool" hallucination pattern.
            let allowed_tool_names: std::collections::HashSet<&str> =
                round_request.tools.iter().map(|t| t.name.as_str()).collect();
            for tool in &mut synthetic_tools {
                if !allowed_tool_names.contains(tool.name.as_str()) {
                    if round_request.tools.len() == 1 {
                        let intended = round_request.tools[0].name.clone();
                        tracing::warn!(
                            recovered = %tool.name,
                            remapped_to = %intended,
                            round,
                            "RP-2: DSML tool name not in allowed surface — remapping to single allowed tool"
                        );
                        if !state.silent {
                            render_sink.info(&format!(
                                "[format-recovery] RP-2: remapped `{}` → `{}` (not in sub-agent surface)",
                                tool.name, intended
                            ));
                        }
                        tool.name = intended;
                    } else {
                        tracing::warn!(
                            recovered = %tool.name,
                            allowed = ?allowed_tool_names,
                            round,
                            "RP-2: DSML tool name not in allowed surface — cannot remap (multiple allowed tools)"
                        );
                    }
                }
            }

            // RP-1: Validate and coerce recovered args against tool input_schema.
            // DeepSeek DSML frequently emits `paths: "/dir"` (string) when schema declares
            // `type: "array"`. Coerce string → [string] to prevent tool failures with score 0.00.
            for tool in &mut synthetic_tools {
                if let Some(tool_def) = round_request.tools.iter().find(|t| t.name == tool.name) {
                    let props = tool_def.input_schema
                        .get("properties")
                        .and_then(|p| p.as_object());
                    if let Some(props_map) = props {
                        if let serde_json::Value::Object(ref mut args) = tool.input {
                            // Collect keys to coerce (avoid double-borrow during mutation).
                            let to_coerce: Vec<String> = props_map
                                .iter()
                                .filter(|(prop_name, prop_schema)| {
                                    let expects_array = prop_schema
                                        .get("type")
                                        .and_then(|t| t.as_str())
                                        .map_or(false, |t| t == "array");
                                    expects_array
                                        && args
                                            .get(prop_name.as_str())
                                            .map_or(false, |v| v.is_string())
                                })
                                .map(|(k, _)| k.clone())
                                .collect();
                            for prop_name in to_coerce {
                                if let Some(val) = args.get(&prop_name).cloned() {
                                    tracing::warn!(
                                        tool = %tool.name,
                                        prop = %prop_name,
                                        "RP-1: DSML arg coercion — string → array (schema mismatch)"
                                    );
                                    args.insert(
                                        prop_name,
                                        serde_json::Value::Array(vec![val]),
                                    );
                                }
                            }
                        }
                    }
                }
            }

            // Strip the DSML text from round_text so it doesn't contaminate full_text.
            let clean_text = super::super::model_quirks::strip_tool_xml_artifacts(&round_text);
            let clean_round_text = if clean_text.contains('\u{ff5c}') {
                // DSML markers still present — strip the entire DSML block
                round_text
                    .split("<\u{ff5c}DSML\u{ff5c}")
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string()
            } else {
                clean_text.into_owned()
            };

            // Record trace with the recovered tool calls.
            record_trace(
                trace_db, state.session_id, &mut state.trace_step_index,
                TraceStepType::ModelResponse,
                serde_json::json!({
                    "round": pending_trace_round,
                    "text": &clean_round_text,
                    "stop_reason": "recovered_tool_use",
                    "usage": { "input_tokens": pending_trace_usage.input_tokens, "output_tokens": pending_trace_usage.output_tokens },
                    "latency_ms": pending_trace_latency,
                    "tool_uses": synthetic_tools.iter().map(|t| serde_json::json!({
                        "id": t.id, "name": t.name, "input": t.input,
                    })).collect::<Vec<_>>(),
                }).to_string(),
                pending_trace_latency,
                exec_clock,
            );

            // Build assistant message with tool use blocks.
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            if !clean_round_text.is_empty() {
                assistant_blocks.push(ContentBlock::Text { text: clean_round_text });
            }
            for tool in &synthetic_tools {
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
            state.messages.push(assistant_msg.clone());
            state.context_pipeline.add_message(assistant_msg.clone());
            session.add_message(assistant_msg);

            state.rounds = round + 1;
            session.agent_rounds += 1;

            return Ok(ProviderRoundOutcome::ToolUse(ProviderRoundOutput {
                completed_tools: synthetic_tools,
                round_model_name,
                round_provider_name,
                round_usage,
                round_text_for_scorer,
            }));
        }
    }
    // ── End FASE 2 ───────────────────────────────────────────────────────────

    if stop_reason != StopReason::ToolUse {
        // Record deferred trace with empty tool_uses for non-tool-use rounds.
        record_trace(
            trace_db, state.session_id, &mut state.trace_step_index,
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
            state.messages.push(msg.clone());
            state.context_pipeline.add_message(msg.clone());
            session.add_message(msg);
        }

        // Fix: count every LLM invocation as a round, not only tool-use rounds.
        // Before this fix text-only responses left rounds=0, making session summaries
        // misleading ("0 rounds" even when the model replied successfully).
        state.rounds = round + 1;
        session.agent_rounds += 1;

        // Sprint 1 Fix: Reset loop guard counter on text rounds.
        // Step 8e: Use record_text_round() instead of reset_on_text_round() to track
        // RoundType::Text in the sliding window for cross-type oscillation detection.
        state.guards.loop_guard.record_text_round();
        if state.guards.loop_guard.detect_cross_type_oscillation() {
            render_sink.warning("[loop-guard] cross-type Tool↔Text oscillation — forcing synthesis", None);
            // Phase 2: route through governance gate (Tool↔Text oscillation detected).
            state.request_synthesis_with_gate(
                SynthesisTrigger::LoopGuard,
                SynthesisOrigin::OscillationDetected,
                SynthesisPriority::High,
            );
            // EBS-R1 (CrossTypeOscillationDetected): if evidence gate fires, override origin to
            // SupervisorFailure so reward pipeline applies synthesis penalty on unreadable files.
            if state.evidence.bundle.evidence_gate_fires() {
                state.evidence.bundle.synthesis_blocked = true;
                state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                tracing::warn!(
                    session_id = %state.session_id,
                    text_bytes_extracted = state.evidence.bundle.text_bytes_extracted,
                    "EvidenceGate FIRED (CrossTypeOscillationDetected): \
                     origin overridden to SupervisorFailure"
                );
            }
            return Ok(ProviderRoundOutcome::BreakLoop);
        }

        // Auto-checkpoint after non-tool-use round (crash protection).
        auto_checkpoint(trace_db, state.session_id, state.rounds, &state.messages, session, state.trace_step_index);
        return Ok(ProviderRoundOutcome::BreakLoop);
    }

    // --- Tool use round ---
    state.rounds = round + 1;
    session.agent_rounds += 1;
    let completed_tools = accumulator.finalize();

    if completed_tools.is_empty() {
        return Ok(ProviderRoundOutcome::BreakLoop);
    }

    // Record deferred trace with tool_uses for tool-use rounds.
    record_trace(
        trace_db, state.session_id, &mut state.trace_step_index,
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
    state.messages.push(assistant_msg.clone());
    state.context_pipeline.add_message(assistant_msg.clone());
    session.add_message(assistant_msg);

    Ok(ProviderRoundOutcome::ToolUse(ProviderRoundOutput {
        completed_tools,
        round_model_name,
        round_provider_name,
        round_usage,
        round_text_for_scorer,
    }))
}
