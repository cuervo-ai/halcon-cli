//! Round setup phase: reflection injection, plan hash init, context compaction,
//! token budget check, model selection, instruction refresh, plan section update,
//! context tier update, request construction, capability orchestration,
//! provider normalization, model validation, context window guard, protocol
//! validation, trace recording, guardrail check, PII check, and response cache lookup.
//!
//! Called once per round at the top of `'agent_loop` in `run_agent_loop()`.
//! Returns `Ok(RoundSetupOutcome::Continue(out))` if the round should proceed,
//! `Ok(RoundSetupOutcome::BreakLoop)` if the outer loop should break, or
//! `Ok(RoundSetupOutcome::EarlyReturn(..))` for a full early return.

use std::sync::Arc;

use anyhow::Result;
use halcon_core::traits::ModelProvider;
use halcon_core::types::{
    AgentLimits, AgentResult, AgentType, ChatMessage, ContentBlock, DomainEvent, EventPayload,
    MessageContent, ModelRequest, Role, RoutingTier, Session,
};
use halcon_core::EventSender;
use halcon_providers::ProviderRegistry;
use halcon_storage::{AsyncDatabase, TraceStepType};

use super::super::agent_types::StopCondition;
use super::super::agent_utils::{compute_fingerprint, record_trace};
use super::super::compaction::ContextCompactor;
use super::super::context_metrics::ContextMetrics;
use super::super::model_selector::ModelSelector;
use super::super::response_cache::ResponseCache;
use super::super::round_scorer::RoundEvaluation;
use super::loop_state::LoopState;
use super::plan_formatter::{format_plan_for_prompt, update_plan_in_system};
use crate::render::sink::RenderSink;
// AgentLoopResult, ControlReceiver, PluginRegistry are NOT imported here:
// the EarlyReturnData variant only carries plain data; the caller assembles AgentLoopResult.

/// Data produced by round setup that is needed by the provider invocation phase.
pub(super) struct RoundSetupOutput {
    pub round_request: ModelRequest,
    pub effective_provider: Arc<dyn ModelProvider>,
    /// The resolved model ID for this round (may differ from `request.model` due to
    /// model selector overrides or fallback adaptation).
    pub selected_model: String,
}

/// Data for an early return (model validation failure).
/// The caller in mod.rs assembles the full `AgentLoopResult` using these fields
/// plus its own `ctrl_rx` and `plugin_registry` (to avoid moving them into the submodule).
pub(super) struct EarlyReturnData {
    pub full_text: String,
    pub rounds: usize,
    pub stop_condition: StopCondition,
    pub call_input_tokens: u64,
    pub call_output_tokens: u64,
    pub call_cost: f64,
    pub latency_ms: u64,
    pub execution_fingerprint: String,
    pub round_evaluations: Vec<RoundEvaluation>,
}

/// Outcome of the round setup phase.
pub(super) enum RoundSetupOutcome {
    /// Break `'agent_loop` immediately.
    BreakLoop,
    /// An early return from `run_agent_loop` is required (e.g. model validation failed).
    /// Caller uses `EarlyReturnData` + its own `ctrl_rx` + `plugin_registry` to build `AgentLoopResult`.
    EarlyReturn(Box<EarlyReturnData>),
    /// Continue with the provider invocation using the setup output.
    Continue(RoundSetupOutput),
}

/// Run the round setup phase for one iteration of the agent loop.
///
/// Takes all outer-scope references needed by the setup logic.
/// Converts loop-breaking and early-returning logic into the `RoundSetupOutcome` enum.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    state: &mut LoopState,
    session: &mut Session,
    render_sink: &dyn RenderSink,
    provider: &Arc<dyn ModelProvider>,
    request: &ModelRequest,
    limits: &AgentLimits,
    event_tx: &EventSender,
    trace_db: Option<&AsyncDatabase>,
    response_cache: Option<&ResponseCache>,
    compactor: Option<&ContextCompactor>,
    model_selector: Option<&ModelSelector>,
    registry: Option<&ProviderRegistry>,
    context_metrics: Option<&Arc<ContextMetrics>>,
    working_dir: &str,
    guardrails: &[Box<dyn halcon_security::Guardrail>],
    security_config: &halcon_core::types::SecurityConfig,
    exec_clock: &halcon_core::types::ExecutionClock,
    round: usize,
    paloma_router: Option<&halcon_providers::PalomaRouter>,
) -> Result<RoundSetupOutcome> {
    // FASE F / Phase 136: Consume model_downgrade_advisory_active and force fast routing.
    // When AdaptivePolicy signals low trajectory (consecutive low-reward rounds), we
    // switch the per-round ModelRouter tier to "fast" for this round only, giving the
    // loop a cheaper, lower-latency turn to break a stagnation pattern.
    // `forced_routing_bias` is consumed (take) in the model selection block below.
    if state.model_downgrade_advisory_active {
        state.model_downgrade_advisory_active = false;
        state.forced_routing_bias = Some(RoutingTier::Fast.as_bias_str().to_string());
        tracing::warn!(
            round,
            "round_setup: model downgrade advisory — forcing 'fast' routing tier for this round \
             (AdaptivePolicy detected low trajectory; forced_routing_bias=Some(\"fast\"))"
        );
    }

    // Phase 1 Supervisor: inject prior-round reflection advice into system prompt.
    // This closes the temporal gap: advice from round N is prepended at round N+1 start,
    // ensuring the model acts on self-reflections immediately (not just cross-session).
    if let Some(directive) = state.reflection_injector.take_directive() {
        match &mut state.cached_system {
            Some(ref mut sys) => sys.push_str(&directive),
            None => state.cached_system = Some(directive),
        }
    }

    // HICON Phase 3: Initialize plan hash on first round if we have a plan
    if round == 0 {
        if let Some(ref plan) = state.active_plan {
            let plan_hash = {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                for step in &plan.steps {
                    step.description.hash(&mut hasher);
                    step.tool_name.hash(&mut hasher);
                }
                hasher.finish()
            };
            state.guards.loop_guard.update_plan_hash(plan_hash);
        }
    }

    // Phase 5 K5-2: compaction fallback for sustained super-linear context growth.
    // When post_batch detects consecutive growth violations exceeding the trigger,
    // this fallback truncates old messages and caps tool outputs to reduce context size.
    // This runs BEFORE the normal compaction check, which may follow with summarization.
    if state.tokens.k5_2_compaction_needed {
        state.tokens.k5_2_compaction_needed = false; // reset after handling
        tracing::warn!(
            round,
            messages = state.messages.len(),
            "K5-2 compaction fallback: truncating old messages + capping tool outputs"
        );
        // Truncation fallback: remove oldest non-system messages, keep last 60%.
        let keep_count = (state.messages.len() * 3) / 5; // ~60%
        if keep_count > 0 && state.messages.len() > keep_count {
            let remove_count = state.messages.len() - keep_count;
            // Remove from the front, but preserve the first message (system/user prompt).
            let mut preserved = Vec::with_capacity(keep_count + 1);
            preserved.push(state.messages[0].clone()); // keep system/first user message
            preserved.extend(state.messages[remove_count + 1..].iter().cloned());
            state.messages = preserved;
            tracing::info!(
                removed = remove_count,
                remaining = state.messages.len(),
                "K5-2: truncated old messages"
            );
        }
        // Cap tool output content to 2000 chars to reduce per-message bloat.
        for msg in state.messages.iter_mut() {
            if let halcon_core::types::MessageContent::Blocks(ref mut blocks) = msg.content {
                for block in blocks.iter_mut() {
                    if let halcon_core::types::ContentBlock::ToolResult {
                        ref mut content, ..
                    } = block
                    {
                        if content.len() > 2000 {
                            // UTF-8 safe truncation: walk back to a char boundary
                            let mut boundary = 2000;
                            while boundary > 0 && !content.is_char_boundary(boundary) {
                                boundary -= 1;
                            }
                            content.truncate(boundary);
                            content.push_str("\n[... truncated by K5-2 compaction]");
                        }
                    }
                }
            }
        }
        if !state.silent {
            render_sink.info(
                "[K5-2] context compaction applied — truncated old messages and tool outputs",
            );
        }
    }

    // Context compaction check: summarize old state.messages if approaching context limit.
    // Wrapped in a 15s timeout to prevent indefinite blocking on slow providers.
    if let Some(compactor) = compactor {
        // REMEDIATION FIX B — Use pipeline budget for compaction threshold.
        // `needs_compaction()` uses the stale config value (default 200K) which fires at
        // 80% × 200K = 160K. For DeepSeek (64K context), that threshold is never reached
        // before the provider rejects the request. Instead use `needs_compaction_with_budget()`
        // which applies a 70% threshold on the actual state.tokens.pipeline_budget derived from the model
        // context window (Fix A): trigger at 70% × 80% × 64K ≈ 35.8K tokens — safe, early.
        if compactor.needs_compaction_with_budget(&state.messages, state.tokens.pipeline_budget) {
            if !state.silent {
                render_sink.spinner_start("Compacting context...");
            }
            tracing::info!(
                round,
                message_count = state.messages.len(),
                estimated_tokens = ContextCompactor::estimate_message_tokens(&state.messages),
                "Context compaction triggered"
            );
            let pre_compact_count = state.messages.len();

            let compaction_result = tokio::time::timeout(
                std::time::Duration::from_secs(state.policy.compaction_timeout_secs),
                async {
                    // Build a compaction request using the same provider.
                    // Use state.compaction_model (resolved pre-loop) so cross-provider
                    // mismatches (e.g. claude model on deepseek) don't cause API errors.
                    let summary_prompt = compactor.compaction_prompt(&state.messages);
                    let compaction_request = ModelRequest {
                        model: state.compaction_model.clone(),
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
                            use futures::StreamExt;
                            while let Some(chunk_result) = stream.next().await {
                                if let Ok(halcon_core::types::ModelChunk::TextDelta(delta)) = chunk_result {
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
            if !state.silent {
                render_sink.spinner_stop();
            }

            match compaction_result {
                Ok(summary_text) if !summary_text.is_empty() => {
                    // Use budget-adaptive keep_recent so the preserved window scales
                    // with the provider's actual context window (Fix B extension).
                    compactor.apply_compaction_with_budget(
                        &mut state.messages,
                        &summary_text,
                        state.tokens.pipeline_budget,
                    );
                    // Sync session state.messages and re-seed pipeline.
                    session.messages = state.messages.clone();
                    // REMEDIATION FIX C — Preserve L1-L4 on compaction.
                    // The old `state.context_pipeline.reset()` destroyed all L1-L4 compressed,
                    // semantic, and archive content — erasing valuable distilled knowledge
                    // that took multiple rounds to build up. Instead, only clear L0 (the
                    // hot buffer) and re-seed it with the compacted state.messages. L1-L4 tiers
                    // retain their segments, providing historical context even post-compaction.
                    state.context_pipeline.reset_hot_only();
                    for msg in &state.messages {
                        state.context_pipeline.add_message(msg.clone());
                    }
                    let tokens_saved = ContextCompactor::estimate_message_tokens(&state.messages);
                    if !state.silent {
                        render_sink.compaction_complete(
                            pre_compact_count,
                            state.messages.len(),
                            tokens_saved as u64,
                        );
                    }
                    tracing::info!(
                        new_message_count = state.messages.len(),
                        "Context compacted successfully (L1-L4 tiers preserved)"
                    );
                }
                Err(_) => {
                    // P1-B: Compaction Failure Escalation.
                    // A state.silent skip is dangerous when the context is near capacity — the next
                    // round invocation may hit a provider context-window error.
                    // Compute utilization and escalate proportionally.
                    let current_tokens =
                        ContextCompactor::estimate_message_tokens(&state.messages) as u32;
                    let utilization_pct = if state.tokens.pipeline_budget > 0 {
                        (current_tokens as f64 / state.tokens.pipeline_budget as f64 * 100.0) as u32
                    } else {
                        100
                    };
                    tracing::warn!(
                        utilization_pct,
                        current_tokens,
                        state.tokens.pipeline_budget,
                        "Context compaction timed out after {}s — context at {}% capacity",
                        state.policy.compaction_timeout_secs,
                        utilization_pct
                    );
                    if !state.silent {
                        render_sink.warning(
                            &format!(
                                "context compaction timed out ({}% full, {}/{} tokens) — \
                                 disabling tools next round to prevent context overflow",
                                utilization_pct, current_tokens, state.tokens.pipeline_budget
                            ),
                            Some("Provider may be slow; tools suppressed to allow synthesis"),
                        );
                    }
                    // High utilization: suppress tools next round to create room for synthesis.
                    // This prevents the model from adding more tool results to an already
                    // nearly-full context, which would trigger a provider context-window error.
                    if utilization_pct >= 70 {
                        tracing::info!(
                            "P1-B: context ≥70% after compaction timeout — \
                             suppressing tools next round"
                        );
                        state.synthesis.tool_decision.set_force_next();
                    }
                }
                _ => {}
            }
        }
    }

    // Token budget pre-check: skip invocation if already over budget.
    if limits.max_total_tokens > 0 && session.total_usage.total() >= limits.max_total_tokens {
        if !state.silent {
            render_sink.warning(
                &format!(
                    "token budget exceeded before round: {} / {} tokens",
                    session.total_usage.total(),
                    limits.max_total_tokens
                ),
                Some("Reduce prompt size or increase max_total_tokens"),
            );
        }
        return Ok(RoundSetupOutcome::BreakLoop);
    }

    // Build pipeline state.messages once per round — used for both model selection and
    // the actual API request. set_round() must fire first so segment metadata is
    // correct. build_messages() is O(n) over all tiers; calling it twice per round
    // was the hot-path bottleneck identified in the SOTA 2026 performance audit.
    state.context_pipeline.set_round(round as u32);
    let built_messages = state.context_pipeline.build_messages();

    // ── Paloma Routing (formally-verified decision) ──────────────────────────
    // When PalomaRouter is available, consult it first for model/provider selection.
    // Paloma enforces policy, capability, budget, and scoring constraints.
    // If Paloma returns a decision, use it. Otherwise, fall through to ModelSelector.
    let paloma_decision = paloma_router.map(|router| {
        let routing_req = halcon_providers::router::RoutingRequest {
            messages: &built_messages,
            tenant_tier: "standard",
            force_provider: None,
            force_model: None,
            latency_sla_ms: None,
            cost_budget_remaining: Some(session.estimated_cost_usd.max(0.0) as f64)
                .filter(|&c| c > 0.0),
        };
        router.route(&routing_req)
    });

    // Optional: context-aware model selection with mid-session re-evaluation.
    // Uses the pipeline's context-managed state.messages for accurate complexity scoring,
    // not the original request (which only has the first user message).
    let (mut selected_model, effective_provider) = if let Some(ref decision) = paloma_decision {
        // Paloma decision takes priority — formally verified routing.
        tracing::info!(
            model = %decision.model,
            provider = %decision.provider,
            tier = ?decision.tier,
            reason = %decision.reason,
            "Paloma routing decision"
        );
        if !state.silent {
            render_sink.model_selected(&decision.model, &decision.provider, &decision.reason);
        }
        let resolved_provider = registry
            .and_then(|r| r.get(&decision.provider))
            .map(Arc::clone)
            .unwrap_or_else(|| Arc::clone(provider));
        (decision.model.clone(), resolved_provider)
    } else if let Some(selector) = model_selector {
        let spend = session.estimated_cost_usd;
        // Reuse built_messages (already constructed above) for complexity scoring.
        let round_context_request = ModelRequest {
            model: request.model.clone(),
            messages: built_messages.clone(),
            tools: state.cached_tools.clone(),
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            system: state.cached_system.clone(),
            stream: true,
        };
        // FASE F / Phase 136: forced_routing_bias (set by model_downgrade_advisory) overrides
        // the UCB1 strategy bias for this round only. Consumed via take() so subsequent
        // rounds revert to normal StrategyContext routing unless advisory fires again.
        let forced_bias = state.forced_routing_bias.take();
        let routing_bias_hint: Option<&str> = forced_bias.as_deref().or_else(|| {
            state
                .strategy_context
                .as_ref()
                .and_then(|sc| sc.routing_bias.as_deref())
        });
        if let Some(selection) =
            selector.select_model(&round_context_request, spend, routing_bias_hint)
        {
            tracing::debug!(
                model = %selection.model_id,
                provider = %selection.provider_name,
                reason = %selection.reason,
                "Model selector override"
            );
            if !state.silent {
                render_sink.model_selected(
                    &selection.model_id,
                    &selection.provider_name,
                    &selection.reason,
                );
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
    if let Some(ref adapted) = state.fallback_adapted_model {
        selected_model = adapted.clone();
    }

    // Phase 32: persist model selector override for cross-round stability.
    // When the selector picks a different model (e.g., deepseek-coder-v2 on ollama)
    // and the selector returns None on a later round, we reuse the last working model
    // instead of request.model (which may not be valid on the current provider).
    if selected_model != request.model && state.fallback_adapted_model.is_none() {
        state.fallback_adapted_model = Some(selected_model.clone());
    }

    // Round separator: emit for all rounds (including round 0) so status bar gets provider/model.
    // Round 0 needs this to populate the status bar initially.
    if !state.silent {
        render_sink.round_started(round + 1, effective_provider.name(), &selected_model);
    }

    // Per-round instruction refresh: check if HALCON.md files changed on disk.
    // Feature 1 path: use InstructionStore (hot-reload via PollWatcher, 250 ms interval).
    // Legacy path: mtime-polling via ContextPipeline (used when use_halcon_md = false).
    if let Some(ref mut store) = state.instruction_store {
        // Feature 1: InstructionStore hot-reload (use_halcon_md = true).
        if let Some(new_instr) = store.check_and_reload() {
            if let Some(ref mut sys) = state.cached_system {
                if let Some(ref old_instr) = state.cached_instructions {
                    // Surgical replacement: swap instruction section in-place.
                    *sys = sys.replacen(old_instr.as_str(), &new_instr, 1);
                } else if !new_instr.is_empty() {
                    sys.push_str("\n\n");
                    sys.push_str(&new_instr);
                }
            } else if !new_instr.is_empty() {
                state.cached_system = Some(new_instr.clone());
            }
            if !new_instr.is_empty() {
                tracing::info!(
                    round,
                    "HALCON.md changed — system prompt updated (Feature 1)"
                );
                state.cached_instructions = Some(new_instr);
            }
        }
    } else {
        // Legacy path: mtime-based polling via ContextPipeline.
        // Performs a stat syscall (~10μs) per instruction file — negligible overhead.
        if let Some(new_instr) = state
            .context_pipeline
            .refresh_instructions(std::path::Path::new(working_dir))
        {
            if let Some(ref mut sys) = state.cached_system {
                if let Some(ref old_instr) = state.cached_instructions {
                    // Surgically replace the instruction portion within the full system prompt.
                    *sys = sys.replacen(old_instr.as_str(), &new_instr, 1);
                }
            }
            tracing::info!(
                round,
                "Instruction files changed on disk — system prompt updated"
            );
            state.cached_instructions = Some(new_instr);
        }
    }

    // Per-round plan section update: refresh step statuses and current step indicator.
    if let Some(ref tracker) = state.execution_tracker {
        let plan = tracker.plan();
        let plan_section = format_plan_for_prompt(plan, tracker.current_step());
        if let Some(ref mut sys) = state.cached_system {
            update_plan_in_system(sys, &plan_section);
        }
    }

    // G1.5: Optional system prompt PII scan (opt-in via SecurityConfig.scan_system_prompts).
    // System prompts are internal artifacts — no blocking, warn-only so operators notice accidental
    // credential/key injection into system context without disrupting the round.
    if security_config.scan_system_prompts {
        if let Some(sys) = &state.cached_system {
            let pii_types = halcon_security::pii::PII_DETECTOR.detect(sys);
            if !pii_types.is_empty() {
                tracing::warn!(
                    pii_types = ?pii_types,
                    "G1.5: PII pattern detected in system prompt (scan_system_prompts=true, warn-only)"
                );
            }
        }
    }

    // built_messages was already constructed above (before model selection).
    // Phase 42: record context assembly metrics.
    if let Some(metrics) = context_metrics {
        let approx_tokens = built_messages
            .iter()
            .map(|m| match &m.content {
                MessageContent::Text(t) => t.len() / 4,
                MessageContent::Blocks(blocks) => blocks
                    .iter()
                    .map(|b| match b {
                        ContentBlock::Text { text, .. } => text.len() / 4,
                        _ => 20,
                    })
                    .sum(),
            })
            .sum::<usize>();
        metrics.record_assembly(approx_tokens as u32, 0);
    }
    // Phase 43D: Emit context tier data for TUI panel.
    if !state.silent {
        let l0_tokens = state.context_pipeline.l0().token_count();
        // FIX: Use actual L0 budget from TokenAccountant instead of slot * 50 approximation
        let l0_cap = state
            .context_pipeline
            .accountant()
            .tier_budget(halcon_context::Tier::L0Hot);
        let l1_tokens = state.context_pipeline.l1().token_count();
        let l1_entries = state.context_pipeline.l1().len();
        let l2_entries = state.context_pipeline.l2().len();
        let l3_entries = state.context_pipeline.l3().len();
        let l4_entries = state.context_pipeline.l4().len();
        let total = state.context_pipeline.estimated_tokens();
        render_sink.context_tier_update(
            l0_tokens, l0_cap, l1_tokens, l1_entries, l2_entries, l3_entries, l4_entries, total,
        );
    }
    // F1 ToolTrust: filter low-trust tools after round 0.
    if state.rounds > 0 {
        let (trusted, hidden) = state.tool_trust.filter_tools(state.cached_tools.clone());
        if hidden > 0 {
            tracing::info!(
                hidden = hidden,
                remaining = trusted.len(),
                "ToolTrust: removed low-trust tools from surface"
            );
            state.cached_tools = trusted;
        }
    }

    let mut round_request = ModelRequest {
        model: selected_model.clone(),
        messages: built_messages,
        // Sprint 1: always start with the full cached tool list; the
        // CapabilityOrchestrationLayer below clears it when any rule fires.
        tools: state.cached_tools.clone(),
        max_tokens: request.max_tokens,
        temperature: request.temperature,
        system: state.cached_system.clone(),
        stream: true,
    };

    // Sprint 1: CapabilityOrchestrationLayer — evaluate all filter rules in one pass.
    // Replaces the 5 scattered STRIP points that previously appeared here:
    //   • state.synthesis.tool_decision.is_active() → tools: vec![] assignment
    //   • state.synthesis.tool_decision.is_active() → Ollama emulation block strip
    //   • state.synthesis.tool_decision.consume() reset
    //   • !model.supports_tools → round_request.tools.clear()
    //   • state.is_conversational_intent → state.cached_system directive injection
    {
        use super::super::plugins::capability_orchestrator::{RoundContext, SuppressReason};

        // Pre-compute model capability from provider registry.
        //
        // B3 remediation: When the model is not in the provider's known list,
        // use the provider's tool_format() as a secondary signal instead of
        // blindly assuming true. Providers with a known tool format (Anthropic,
        // OpenAI-compat, Gemini) can safely assume unknown models support tools.
        // Providers with Unknown format should NOT assume tool support.
        let model_supports_tools = effective_provider
            .supported_models()
            .iter()
            .find(|m| m.id == selected_model)
            .map(|m| m.supports_tools)
            .unwrap_or_else(|| {
                let format = effective_provider.tool_format();
                let assumed = format != halcon_core::types::ToolFormat::Unknown;
                if !assumed {
                    tracing::warn!(
                        model = %selected_model,
                        provider = %effective_provider.name(),
                        tool_format = %format.label(),
                        "B3: unknown model + Unknown tool format — assuming no tool support"
                    );
                } else {
                    tracing::debug!(
                        model = %selected_model,
                        provider = %effective_provider.name(),
                        tool_format = %format.label(),
                        "B3: unknown model but known tool format — assuming tool support"
                    );
                }
                assumed
            });

        let orch_ctx = RoundContext {
            force_no_tools_next_round: state.synthesis.tool_decision.is_active(),
            selected_model: &selected_model,
            model_supports_tools,
            is_conversational_intent: state.is_conversational_intent,
            tools_non_empty: !round_request.tools.is_empty(),
        };
        let orch_decision = state.guards.capability_orchestrator.evaluate(&orch_ctx);

        // Apply — Ollama tool emulation strip.
        if orch_decision.strip_ollama_emulation {
            const OLLAMA_TOOL_EMUL_MARKER: &str = "\n\n# TOOL USE INSTRUCTIONS\n\n";
            if let Some(ref mut sys) = round_request.system {
                if let Some(pos) = sys.find(OLLAMA_TOOL_EMUL_MARKER) {
                    tracing::debug!(
                        pos,
                        "CapabilityOrch[ForceNoTools]: stripping Ollama tool emulation block"
                    );
                    sys.truncate(pos);
                }
            }
        }

        // Apply — system directive injection (conversational mode, etc.).
        if let Some(ref directive) = orch_decision.system_directive {
            if let Some(ref mut sys) = round_request.system {
                sys.push_str(directive);
            } else {
                round_request.system = Some(directive.clone());
            }
            tracing::debug!("CapabilityOrch: system directive injected");
        }

        // Apply — tool suppression.
        if let Some(ref reason) = orch_decision.suppress {
            match reason {
                SuppressReason::ForcedByLoop => {
                    tracing::debug!(
                        "CapabilityOrch[ForceNoTools]: suppressing tools \
                         (loop guard / compaction / replan)"
                    );
                }
                SuppressReason::ModelCapability { model } => {
                    tracing::debug!(
                        model = %model,
                        provider = effective_provider.name(),
                        "CapabilityOrch[ModelCapability]: model does not support \
                         tool_use protocol — running in direct-response mode"
                    );
                    if !state.silent {
                        render_sink.info(&format!(
                            "[model] '{}' does not support tools — running in \
                             direct-response mode",
                            model
                        ));
                    }
                }
            }
            round_request.tools.clear();
        }

        // Consume tool_decision — orchestration has processed it; resets to Allow.
        state.synthesis.tool_decision.consume();
    }

    // Phase 3B: Synthesis phase system prompt sanitization.
    //
    // Defense-in-depth: whenever tools are absent from the request (regardless of HOW
    // they were removed — pre-loop synthesis guard, ForceNoToolsRule, ModelCapabilityRule,
    // conversational mode, or compaction timeout), strip the halcon-native tool-mode
    // directives from the system prompt.
    //
    // Without this, the system prompt may still contain:
    //   "## Autonomous Agent Behavior — Use tools proactively..."
    //   "## Tool Usage Policy — Only call tools when you need NEW information..."
    //
    // Those instructions cause providers like deepseek-chat to emit XML tool-call syntax
    // (`<function_calls><invoke name="...">`) as plain text even with tools=[], because
    // the model correctly follows the system instructions to use tools — just without a
    // schema to validate against.  The result is phantom XML in full_text and a 15%
    // LoopCritic confidence score despite all real tools having executed successfully.
    //
    // Invariant enforced: `round_request.tools.is_empty()` ⟹ system prompt contains no
    // `## Autonomous Agent Behavior` or `## Tool Usage Policy` sections.
    if round_request.tools.is_empty() {
        if let Some(ref mut sys) = round_request.system {
            // Both directives are injected together; find the first occurrence.
            // They appear at the END of the system prompt so truncation is safe.
            const AUTONOMOUS_MARKER: &str = "\n\n## Autonomous Agent Behavior\n";
            const TOOL_POLICY_MARKER: &str = "\n\n## Tool Usage Policy\n";
            let trunc_pos = [sys.find(AUTONOMOUS_MARKER), sys.find(TOOL_POLICY_MARKER)]
                .into_iter()
                .flatten()
                .min();
            if let Some(pos) = trunc_pos {
                tracing::debug!(
                    pos,
                    provider = effective_provider.name(),
                    model = %selected_model,
                    round,
                    "Phase 3B: stripped tool-mode directives from system prompt (tools=[])"
                );
                sys.truncate(pos);
            }
        }
    }

    // Phase 3C: Synthesis guard — max_tokens cap + concise synthesis constraint.
    //
    // When tools are absent and this is NOT a simple conversational turn, the LLM is
    // in "synthesis mode" — summarising sub-agent results or producing a final answer
    // without tool access. Without guardrails, models (especially DeepSeek) generate
    // runaway bash/shell script templates that hit max_tokens (8192) and produce
    // unusable output.
    //
    // Guards applied:
    //   1. Cap max_tokens to SYNTHESIS_MAX_TOKENS (4096) — enough for a rich summary,
    //      too short for a full script template.
    //   2. Inject SYNTHESIS_CONSTRAINT directive into system prompt — explicitly
    //      prohibits code generation and requires concise, evidence-based synthesis.
    //
    // Conditions: tools empty AND (has plan execution OR forced synthesis OR has tool
    // work history) — i.e., this is an agent synthesis round, not a casual chat.
    if round_request.tools.is_empty() && !state.is_conversational_intent {
        let synthesis_max_tokens = state.policy.synthesis_max_tokens;

        // Detect post-orchestration synthesis: if the execution tracker has any
        // steps with outcomes, sub-agents ran and we're now synthesising.
        let has_orchestrator_results = state
            .execution_tracker
            .as_ref()
            .is_some_and(|t| t.plan().steps.iter().any(|s| s.outcome.is_some()));

        let is_synthesis_round = state.synthesis.is_synthesis_forced()
            || state.synthesis.synthesis_origin.is_some()
            || !state.tools_executed.is_empty()
            || state.evidence.bundle.content_read_attempts > 0
            || has_orchestrator_results;

        if is_synthesis_round {
            // Guard 1: Cap max_tokens.
            if round_request
                .max_tokens
                .is_some_and(|mt| mt > synthesis_max_tokens)
            {
                tracing::debug!(
                    original = round_request.max_tokens,
                    capped = synthesis_max_tokens,
                    round,
                    "Phase 3C: capped max_tokens for synthesis round"
                );
                round_request.max_tokens = Some(synthesis_max_tokens);
            }

            // Guard 2: Inject synthesis constraint into system prompt.
            const SYNTHESIS_CONSTRAINT: &str = "\n\n## Synthesis Mode\n\
                You are now in SYNTHESIS MODE. Follow these rules strictly:\n\
                - Summarise the results of completed tool executions concisely.\n\
                - Do NOT generate bash scripts, shell commands, or executable code blocks.\n\
                - Do NOT emit XML tool-call syntax or function invocations.\n\
                - If evidence is insufficient, state what is missing rather than speculating.\n\
                - Keep your response under 3000 tokens.\n\
                - Use structured markdown (headers, bullets) for clarity.";

            if let Some(ref mut sys) = round_request.system {
                sys.push_str(SYNTHESIS_CONSTRAINT);
            } else {
                round_request.system = Some(SYNTHESIS_CONSTRAINT.to_string());
            }
            tracing::debug!(
                round,
                provider = effective_provider.name(),
                "Phase 3C: injected synthesis constraint (tools=[], synthesis_round=true)"
            );

            // FASE 6: TUI observability — signal synthesis phase to the UI.
            if !state.silent {
                render_sink.phase_started("synthesis", "Synthesising results (no tools)...");
                render_sink.agent_state_transition(
                    "executing",
                    "synthesising",
                    "tools stripped — synthesis mode",
                );
            }
        }
    }

    // Sprint 2: ProviderNormalizationAdapter — log wire format + validate schema compat.
    //
    // Phase 2 escalation: MissingTypeField and RequiredFieldMissing are structural
    // defects that cause provider-side 400 errors. Strip the offending tool from the
    // request rather than sending a malformed definition that will fail silently.
    {
        let norm_adapter =
            super::super::provider_normalization::ProviderNormalizationAdapter::for_provider(
                effective_provider.name(),
            );
        let norm_result = norm_adapter.validate(&round_request.tools);
        norm_adapter.trace_result(&norm_result, round as u32);

        // Escalate critical warnings: collect names of structurally broken tools.
        let mut tools_to_strip: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for warning in &norm_result.warnings {
            match warning {
                super::super::provider_normalization::NormalizationWarning::MissingTypeField {
                    tool,
                } => {
                    tracing::warn!(
                        tool = %tool,
                        "Phase2: stripping tool with missing 'type' field — would cause provider error"
                    );
                    tools_to_strip.insert(tool.clone());
                }
                super::super::provider_normalization::NormalizationWarning::RequiredFieldMissing {
                    tool,
                    field,
                } => {
                    tracing::warn!(
                        tool = %tool,
                        field = %field,
                        "Phase2: stripping tool with missing required field — would cause provider error"
                    );
                    tools_to_strip.insert(tool.clone());
                }
                _ => {} // UnsupportedSchemaType and OllamaEmulationMode remain warnings
            }
        }
        if !tools_to_strip.is_empty() {
            let before = round_request.tools.len();
            round_request
                .tools
                .retain(|t| !tools_to_strip.contains(&t.name));
            tracing::info!(
                stripped = tools_to_strip.len(),
                before,
                after = round_request.tools.len(),
                "Phase2: stripped malformed tools from request"
            );
        }
    }

    // Pre-invoke validation: ensure model is supported by the effective provider.
    if let Err(e) = effective_provider.validate_model(&selected_model) {
        tracing::error!(
            model = %selected_model,
            provider = effective_provider.name(),
            "Model validation failed: {e}"
        );
        if !state.silent {
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
        // P3 FIX: Emit AgentCompleted on early return so listeners always see the event.
        halcon_core::emit_event(
            event_tx,
            DomainEvent::new(EventPayload::AgentCompleted {
                agent_type: AgentType::Chat,
                result: AgentResult {
                    success: false,
                    summary: format!("ProviderError: model validation failed at round {round}"),
                    files_modified: vec![],
                    tools_used: vec![],
                },
            }),
        );
        // Return EarlyReturnData; caller assembles AgentLoopResult with its own ctrl_rx + plugin_registry.
        let data = EarlyReturnData {
            full_text: state.full_text.clone(),
            rounds: state.rounds,
            stop_condition: StopCondition::ProviderError,
            call_input_tokens: state.tokens.call_input_tokens,
            call_output_tokens: state.tokens.call_output_tokens,
            call_cost: state.tokens.call_cost,
            latency_ms: state.loop_start.elapsed().as_millis() as u64,
            execution_fingerprint: compute_fingerprint(&round_request.messages),
            round_evaluations: state.convergence.round_evaluations.clone(),
        };
        return Ok(RoundSetupOutcome::EarlyReturn(Box::new(data)));
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
            if !state.silent {
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
        let violations = halcon_core::types::validation::validate_message_sequence(
            &round_request.messages,
            false, // no trailing tool use expected — we're about to invoke the model
        );
        let critical: Vec<_> =
            violations
                .iter()
                .filter(|v| {
                    matches!(
                v,
                halcon_core::types::validation::ProtocolViolation::OrphanedToolResult { .. }
                | halcon_core::types::validation::ProtocolViolation::ToolResultWrongRole { .. }
                | halcon_core::types::validation::ProtocolViolation::DuplicateToolUseId { .. }
            )
                })
                .collect();

        if !critical.is_empty() {
            for v in &critical {
                tracing::error!("Protocol violation in round {round}: {v}");
            }
            // Auto-repair: strip orphaned results to prevent provider 400s.
            let repaired = halcon_core::types::validation::strip_orphaned_tool_results(
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
        state.session_id,
        &mut state.trace_step_index,
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
        let input_text = round_request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| match &m.content {
                MessageContent::Text(t) => t.as_str(),
                _ => "",
            })
            .unwrap_or("");

        let violations = halcon_security::run_guardrails(
            guardrails,
            input_text,
            halcon_security::GuardrailCheckpoint::PreInvocation,
        );
        for v in &violations {
            tracing::warn!(
                guardrail = %v.guardrail,
                matched = %v.matched,
                "Guardrail triggered: {}",
                v.reason
            );
            halcon_core::emit_event(
                event_tx,
                DomainEvent::new(EventPayload::GuardrailTriggered {
                    guardrail: v.guardrail.clone(),
                    checkpoint: "pre".into(),
                    action: format!("{:?}", v.action),
                }),
            );
        }
        if halcon_security::has_blocking_violation(&violations) {
            if !state.silent {
                render_sink.info("\n[blocked by guardrail]");
            }
            return Ok(RoundSetupOutcome::BreakLoop);
        }
    }

    // G2: PII hard block — independent of the guardrails pipeline.
    // Runs on the same input_text extracted above (last User message).
    // When pii_action == Block, detected PII stops the request cold.
    {
        use halcon_core::types::PiiPolicy;
        let input_text = round_request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| match &m.content {
                MessageContent::Text(t) => t.as_str(),
                _ => "",
            })
            .unwrap_or("");

        if !input_text.is_empty() {
            let detected = halcon_security::pii::PII_DETECTOR.detect(input_text);
            if !detected.is_empty() {
                match security_config.pii_action {
                    PiiPolicy::Block => {
                        tracing::error!(
                            pii_types = ?detected,
                            "G2: PII detected in user input — blocking request (PiiPolicy::Block)"
                        );
                        if !state.silent {
                            render_sink.error(
                                &format!("[G2] Request blocked: PII detected ({}). Remove sensitive data and retry.",
                                    detected.join(", ")),
                                None,
                            );
                        }
                        return Ok(RoundSetupOutcome::BreakLoop);
                    }
                    PiiPolicy::Redact => {
                        // Handled downstream (guardrails + redact_credentials).
                        tracing::warn!(pii_types = ?detected, "G2: PII detected in user input (redact mode — logged only)");
                    }
                    PiiPolicy::Warn => {
                        tracing::warn!(pii_types = ?detected, "G2: PII detected in user input (warn mode)");
                    }
                }
            }
        }
    }

    // Check response cache before invoking provider.
    if let Some(cache) = response_cache {
        if let Some(entry) = cache.lookup(&round_request).await {
            tracing::info!(round, "Response cache hit");
            if !state.silent {
                render_sink.cache_status(true, "response_cache");
            }
            let round_text = entry.response_text.clone();

            // Render the cached response (only if visible).
            if !state.silent {
                render_sink.stream_text(&round_text);
                render_sink.stream_done();
            }

            // Record cache hit in trace.
            record_trace(
                trace_db,
                state.session_id,
                &mut state.trace_step_index,
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

            state.full_text.push_str(&round_text);
            if !round_text.is_empty() {
                let msg = ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text(round_text),
                };
                state.messages.push(msg.clone());
                state.context_pipeline.add_message(msg.clone());
                session.add_message(msg);
            }
            // Cache never stores tool_use responses, so this is always terminal.
            return Ok(RoundSetupOutcome::BreakLoop);
        }
    }

    Ok(RoundSetupOutcome::Continue(RoundSetupOutput {
        round_request,
        effective_provider,
        selected_model,
    }))
}
