//! Production-grade agent loop (Xiyo-aligned + Halcon hardened).
//!
//! Invariants:
//!   - tool_use → continue (arbiter never sees tool rounds)
//!   - turn_count only increments on tool rounds (not recovery)
//!   - recovery counters wired explicitly (no ..Default::default())
//!   - every tool_use has a matching tool_result (synthetic on cancel)
//!   - consecutive-batch execution preserves causal ordering

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures::StreamExt;
use tracing::instrument;

use halcon_core::traits::ModelProvider;
use halcon_core::types::{
    AgentLimits, ChatMessage, ContentBlock, MessageContent, ModelChunk, ModelRequest, Role,
    StopReason, TokenUsage,
};
use halcon_tools::ToolRegistry;

use super::accumulator::ToolUseAccumulator;
use super::feedback_arbiter::{
    AggregatedSignals, FeedbackArbiter, HaltReason, RecoveryAction, TurnDecision, TurnResponse,
    TurnState,
};
use super::tool_executor;
use crate::render::sink::RenderSink;
use crate::repl::agent_types::{AgentLoopResult, ControlReceiver, StopCondition};
use crate::repl::context::compaction::ContextCompactor;
use crate::repl::conversational_permission::ConversationalPermissionHandler;
use crate::repl::hooks::{HookEventName, HookOutcome, HookRunner};

const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 4096;
const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 120;
const DEFAULT_STREAM_TIMEOUT_SECS: u64 = 300;
const DEFAULT_MAX_PARALLEL: usize = 10;
const COMPACTION_THRESHOLD: f64 = 0.90;
const DIMINISHING_RETURNS_THRESHOLD: u64 = 500;

pub struct SimplifiedLoopConfig<'a> {
    pub provider: &'a Arc<dyn ModelProvider>,
    pub request: &'a ModelRequest,
    pub tool_registry: &'a ToolRegistry,
    pub limits: &'a AgentLimits,
    pub render_sink: &'a dyn RenderSink,
    pub working_dir: &'a str,
    pub compactor: Option<&'a ContextCompactor>,
    pub ctrl_rx: Option<ControlReceiver>,
    pub hook_runner: Option<Arc<HookRunner>>,
    pub permissions: &'a mut ConversationalPermissionHandler,
    pub max_cost_usd: f64,
    pub cancel_token: Option<tokio_util::sync::CancellationToken>,
}

struct SpinnerGuard<'a> { sink: &'a dyn RenderSink, active: bool }
impl<'a> SpinnerGuard<'a> {
    fn start(sink: &'a dyn RenderSink, label: &str) -> Self { sink.spinner_start(label); Self { sink, active: true } }
    fn stop(&mut self) { if self.active { self.sink.spinner_stop(); self.active = false; } }
}
impl Drop for SpinnerGuard<'_> { fn drop(&mut self) { self.stop(); } }

struct StagnationTracker { recent_hashes: Vec<u64>, consecutive_stalls: u32 }
impl StagnationTracker {
    fn new() -> Self { Self { recent_hashes: Vec::new(), consecutive_stalls: 0 } }
    fn observe_round(&mut self, tool_names: &[String]) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for n in tool_names { n.hash(&mut h); }
        let hash = h.finish();
        if !tool_names.is_empty() && self.recent_hashes.last() == Some(&hash) { self.consecutive_stalls += 1; }
        else { self.consecutive_stalls = 0; }
        self.recent_hashes.push(hash);
    }
}

struct BudgetTracker { ic: f64, oc: f64, total_usd: f64, last_out: u64, prev_delta: u64, diminishing: bool }
impl BudgetTracker {
    fn new(provider: &dyn ModelProvider, model: &str) -> Self {
        let (ic, oc) = provider.supported_models().iter().find(|m| m.id == model)
            .map(|m| (m.cost_per_input_token, m.cost_per_output_token)).unwrap_or((0.0, 0.0));
        Self { ic, oc, total_usd: 0.0, last_out: 0, prev_delta: u64::MAX, diminishing: false }
    }
    fn record(&mut self, u: &TokenUsage, total_out: u64) {
        self.total_usd += u.input_tokens as f64 * self.ic + u.output_tokens as f64 * self.oc;
        let d = total_out.saturating_sub(self.last_out);
        if d < DIMINISHING_RETURNS_THRESHOLD && self.prev_delta < DIMINISHING_RETURNS_THRESHOLD { self.diminishing = true; }
        self.prev_delta = d;
        self.last_out = total_out;
    }
}

fn is_cancelled(ct: &Option<tokio_util::sync::CancellationToken>, rx: &mut Option<ControlReceiver>) -> bool {
    if let Some(t) = ct { if t.is_cancelled() { return true; } }
    poll_ctrl(rx)
}

fn poll_ctrl(rx: &mut Option<ControlReceiver>) -> bool {
    let Some(rx) = rx.as_mut() else { return false };
    loop {
        match rx.try_recv() {
            #[cfg(feature = "tui")]
            Ok(e) => { if matches!(e, crate::tui::events::ControlEvent::CancelAgent) { return true; } }
            #[cfg(not(feature = "tui"))]
            Ok(s) => { if matches!(s, crate::repl::agent_types::ClassicCancelSignal::Cancel) { return true; } }
            Err(_) => return false,
        }
    }
}

async fn stop_hook_blocked(hr: &Option<Arc<HookRunner>>) -> bool {
    let Some(r) = hr else { return false };
    matches!(r.fire(&crate::repl::hooks::lifecycle_event(HookEventName::Stop, "")).await, HookOutcome::Deny(_))
}

fn build_result(text: String, rounds: usize, stop: StopCondition, inp: u64, out: u64, cost: f64, start: Instant, tools: Vec<String>, model: &str) -> AgentLoopResult {
    AgentLoopResult {
        full_text: text, rounds, stop_condition: stop, input_tokens: inp, output_tokens: out, cost_usd: cost,
        latency_ms: start.elapsed().as_millis() as u64, execution_fingerprint: String::new(),
        timeline_json: None, ctrl_rx: None, critic_verdict: None, round_evaluations: Vec::new(),
        plan_completion_ratio: 0.0, avg_plan_drift: 0.0, oscillation_penalty: 0.0,
        last_model_used: Some(model.to_owned()), plugin_cost_snapshot: Vec::new(), tools_executed: tools,
        evidence_verified: true, content_read_attempts: 0, last_provider_used: None, blocked_tools: Vec::new(),
        failed_sub_agent_steps: Vec::new(), critic_unavailable: false, tool_trust_failures: Vec::new(),
        sla_budget: None, evidence_coverage: 1.0, synthesis_kind: None, synthesis_trigger: None,
        routing_escalation_count: 0, response_trust: halcon_core::types::ResponseTrust::Unverified,
    }
}

#[instrument(skip_all, fields(model = %config.request.model))]
pub async fn run_simplified_loop(mut config: SimplifiedLoopConfig<'_>) -> Result<AgentLoopResult> {
    let start = Instant::now();
    let arbiter = FeedbackArbiter::new();
    let mut messages = config.request.messages.clone();
    let mut full_text = String::new();
    let mut turn_count: u32 = 0;
    let (mut total_in, mut total_out): (u64, u64) = (0, 0);
    let mut max_tokens = config.request.max_tokens.or(Some(DEFAULT_MAX_OUTPUT_TOKENS));
    let mut tools_executed: Vec<String> = Vec::new();
    let (mut esc_count, mut compact_count, mut replan_count): (u32, u32, u32) = (0, 0, 0);

    let max_turns = config.limits.max_rounds as u32;
    let ttimeout = Duration::from_secs(if config.limits.tool_timeout_secs > 0 { config.limits.tool_timeout_secs } else { DEFAULT_TOOL_TIMEOUT_SECS });
    let stimeout = Duration::from_secs(DEFAULT_STREAM_TIMEOUT_SECS);
    let max_par = if config.limits.max_parallel_tools > 0 { config.limits.max_parallel_tools } else { DEFAULT_MAX_PARALLEL };
    let mut budget = BudgetTracker::new(config.provider.as_ref(), &config.request.model);
    let mut stag = StagnationTracker::new();
    let ctx_budget = config.provider.model_context_window(&config.request.model).unwrap_or(200_000);
    let model = config.request.model.clone();

    loop {
        if is_cancelled(&config.cancel_token, &mut config.ctrl_rx) {
            return Ok(build_result(full_text, turn_count as usize, StopCondition::Interrupted, total_in, total_out, budget.total_usd, start, tools_executed, &model));
        }

        if let Some(c) = config.compactor {
            let est = ContextCompactor::estimate_message_tokens(&messages);
            if est > (ctx_budget as f64 * COMPACTION_THRESHOLD) as usize {
                c.apply_compaction(&mut messages, "[Context compacted proactively]");
            }
        }

        let req = ModelRequest { model: model.clone(), messages: messages.clone(), tools: config.request.tools.clone(), max_tokens, temperature: config.request.temperature, system: config.request.system.clone(), stream: true };
        let mut spin = SpinnerGuard::start(config.render_sink, "Thinking...");
        let inv = tokio::time::timeout(stimeout, config.provider.invoke(&req)).await;
        let mut stream = match inv {
            Ok(Ok(s)) => { spin.stop(); s }
            Ok(Err(e)) => { spin.stop(); return Err(e.into()); }
            Err(_) => { spin.stop(); return Ok(build_result(full_text, turn_count as usize, StopCondition::ProviderError, total_in, total_out, budget.total_usd, start, tools_executed, &model)); }
        };

        let mut acc = ToolUseAccumulator::new();
        let mut rtxt = String::new();
        let mut usage: Option<TokenUsage> = None;
        let mut stop = StopReason::EndTurn;
        let mut serr = false;

        loop {
            tokio::select! {
                biased;
                co = stream.next() => { match co {
                    Some(Ok(c)) => { match &c {
                        ModelChunk::TextDelta(t) => { rtxt.push_str(t); config.render_sink.stream_text(t); }
                        ModelChunk::ThinkingDelta(t) => { config.render_sink.stream_thinking(t); }
                        ModelChunk::Usage(u) => { usage = Some(u.clone()); }
                        ModelChunk::Done(r) => { stop = *r; acc.process(&c); break; }
                        ModelChunk::Error(e) => { config.render_sink.stream_error(e); serr = true; break; }
                        _ => {}
                    } acc.process(&c); }
                    Some(Err(e)) => { tracing::warn!(error=%e, "stream error"); serr = true; break; }
                    None => break,
                }}
                _ = async {
                    if let Some(ref t) = config.cancel_token { t.cancelled().await; }
                    else { loop { tokio::time::sleep(Duration::from_millis(100)).await; if poll_ctrl(&mut config.ctrl_rx) { return; } } }
                } => {
                    config.render_sink.stream_done();
                    return Ok(build_result(full_text, turn_count as usize, StopCondition::Interrupted, total_in, total_out, budget.total_usd, start, tools_executed, &model));
                }
            }
        }
        config.render_sink.stream_done();
        let tus = acc.finalize();
        if !rtxt.is_empty() { full_text.push_str(&rtxt); }
        if let Some(ref u) = usage { total_in += u.input_tokens as u64; total_out += u.output_tokens as u64; budget.record(u, total_out); }
        else if !rtxt.is_empty() { total_out += (rtxt.len() / 4).max(1) as u64; }

        if !tus.is_empty() {
            let mut blks: Vec<ContentBlock> = Vec::new();
            if !rtxt.is_empty() { blks.push(ContentBlock::Text { text: rtxt.clone() }); }
            for tu in &tus { blks.push(ContentBlock::ToolUse { id: tu.id.clone(), name: tu.name.clone(), input: tu.input.clone() }); }
            messages.push(ChatMessage { role: Role::Assistant, content: MessageContent::Blocks(blks) });

            let names: Vec<String> = tus.iter().map(|t| t.name.clone()).collect();
            tools_executed.extend(names.iter().cloned());
            stag.observe_round(&names);

            let ct = config.cancel_token.clone();
            let results = tool_executor::execute_tools_partitioned(
                &tus, config.tool_registry, config.permissions, config.working_dir,
                ttimeout, max_par, config.render_sink,
                move || ct.as_ref().map_or(false, |t| t.is_cancelled()),
            ).await;
            messages.push(ChatMessage { role: Role::User, content: MessageContent::Blocks(results) });
            turn_count += 1;
            esc_count = 0;
            continue;
        }

        let ptl = stop == StopReason::MaxTokens && usage.as_ref().map_or(false, |u| u.output_tokens < max_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS) / 4);
        let hmo = stop == StopReason::MaxTokens && usage.as_ref().map_or(false, |u| u.output_tokens >= max_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS));
        let shb = if stop == StopReason::EndTurn { stop_hook_blocked(&config.hook_runner).await } else { false };

        let sigs = AggregatedSignals {
            user_cancelled: is_cancelled(&config.cancel_token, &mut config.ctrl_rx),
            stop_hook_blocked: shb, critic_feedback: None,
            consecutive_stalls: stag.consecutive_stalls,
            cost_usd: budget.total_usd, cost_limit_usd: config.max_cost_usd,
            escalation_count: esc_count, max_escalation_attempts: 3,
            compact_count, max_compact_attempts: 2,
            replan_count, max_replan_attempts: 2,
            diminishing_returns: budget.diminishing,
        };

        match arbiter.decide(
            &TurnResponse { stop_reason: stop, is_prompt_too_long: ptl, hit_max_output_tokens: hmo, is_reactive_overflow: serr && !ptl },
            &TurnState { turn_count, max_turns, budget_exhausted: config.limits.max_total_tokens > 0 && (total_in + total_out) >= config.limits.max_total_tokens as u64 },
            &sigs,
        ) {
            TurnDecision::Complete { .. } => return Ok(build_result(full_text, turn_count as usize, StopCondition::EndTurn, total_in, total_out, budget.total_usd, start, tools_executed, &model)),
            TurnDecision::Recover(act) => apply_recovery(&mut messages, &act, &mut max_tokens, &mut esc_count, &mut compact_count, &mut replan_count, config.compactor),
            TurnDecision::Halt(reason) => {
                let sc = match &reason {
                    HaltReason::MaxTurnsReached => StopCondition::MaxRounds,
                    HaltReason::UserCancelled => StopCondition::Interrupted,
                    HaltReason::BudgetExhausted => StopCondition::TokenBudget,
                    HaltReason::CostLimitExceeded { .. } => StopCondition::CostBudget,
                    HaltReason::StagnationAbort { .. } => StopCondition::ForcedSynthesis,
                    HaltReason::DiminishingReturns => StopCondition::EndTurn,
                    HaltReason::UnrecoverableError(_) => StopCondition::ProviderError,
                };
                return Ok(build_result(full_text, turn_count as usize, sc, total_in, total_out, budget.total_usd, start, tools_executed, &model));
            }
        }
    }
}

fn apply_recovery(msgs: &mut Vec<ChatMessage>, act: &RecoveryAction, mt: &mut Option<u32>, esc: &mut u32, cmp: &mut u32, rpl: &mut u32, compactor: Option<&ContextCompactor>) {
    match act {
        RecoveryAction::Compact | RecoveryAction::ReactiveCompact => {
            *cmp += 1; *esc = 0;
            if let Some(c) = compactor { c.apply_compaction(msgs, "[Context compacted]"); }
            else { let k = 8.min(msgs.len()); if msgs.len() > k { let d = msgs.len()-k; msgs.drain(..msgs.len()-k); msgs.insert(0, ChatMessage { role: Role::User, content: MessageContent::Text(format!("[Compacted: {d} msgs removed]")) }); } }
        }
        RecoveryAction::EscalateTokens => {
            *esc += 1;
            let cur = mt.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS);
            *mt = Some(cur.saturating_mul(2));
            msgs.push(ChatMessage { role: Role::User, content: MessageContent::Text("Output token limit hit. Resume directly — no apology, no recap. Pick up mid-thought. Break remaining work into smaller pieces.".into()) });
        }
        RecoveryAction::FallbackProvider => { tracing::warn!("FallbackProvider not wired"); }
        RecoveryAction::StopHookBlocked => {}
        RecoveryAction::Replan { reason } => { *rpl += 1; msgs.push(ChatMessage { role: Role::User, content: MessageContent::Text(format!("Stagnation: {reason}. Try a different approach.")) }); }
        RecoveryAction::ReplanWithFeedback(fb) => { *rpl += 1; msgs.push(ChatMessage { role: Role::User, content: MessageContent::Text(format!("Feedback: {fb}. Adjust your approach.")) }); }
    }
}
