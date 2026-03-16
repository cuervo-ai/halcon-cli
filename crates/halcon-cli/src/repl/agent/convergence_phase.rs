//! Convergence phase: ConvergenceController observe, metacognitive monitoring,
//! control yield point, RoundScorer + signal assembly, HICON Phase 4 self-correction,
//! LoopGuard match arms, self-correction injection, round cleanup.
//!
//! Called after tool execution and deduplication in `run_agent_loop()`.
//! Returns `PhaseOutcome::Continue` to proceed to the next round,
//! `PhaseOutcome::BreakLoop` to exit the loop, or `PhaseOutcome::NextRound` to skip to next round.

use std::time::Duration;

use anyhow::Result;
use halcon_core::traits::Planner;
use halcon_core::types::{
    ChatMessage, ContentBlock, MessageContent, ModelRequest, PlanningConfig, Role, Session,
    TokenUsage,
};
use halcon_storage::AsyncDatabase;

use super::super::agent_types::ControlReceiver;
use super::super::anomaly_detector::AgentAnomaly;
use super::super::loop_guard::LoopAction;
use super::loop_state::{ExecutionIntentPhase, LoopState, SynthesisKind, SynthesisOrigin, SynthesisTrigger, ToolDecisionSignal};
use super::plan_formatter::{format_plan_for_prompt, update_plan_in_system};
use super::provider_client::check_control;
use super::PhaseOutcome;
use crate::render::sink::RenderSink;

// NOTE: MAX_REPLAN_ATTEMPTS now read from state.policy.max_replan_attempts (PolicyConfig).

// ── B2 Migration: ConvergenceInput view struct ────────────────────────────────
//
// Planned: replace `state: &mut LoopState` parameter with `input: &mut ConvergenceInput<'_>`.
// Blocked: convergence_phase::run() accesses 20+ distinct LoopState sub-fields with mixed
// mutability. Safely splitting these into a view struct requires resolving lifetime conflicts
// between the mutable borrows of state.convergence, state.guards, state.synthesis, state.tokens,
// state.hicon, and the shared borrows of state.policy, state.active_plan, state.execution_tracker.
// Borrow checker prevents holding `&mut ConvergenceState` and `&mut LoopGuardState` simultaneously
// through a view struct without unsafe code or splitting LoopState into separate owned sub-structs.
//
// Partial extraction: the struct definition below captures the INTENT; full migration will follow
// when LoopState is split into owned sub-structs (ARCH-001 decomposition).
//
/// View struct projecting the fields that convergence_phase reads from `LoopState`.
///
/// This is a B2 migration placeholder. Full migration will replace the `state: &mut LoopState`
/// parameter with `input: &mut ConvergenceInput<'_>` once LoopState is decomposed into
/// owned sub-structs (B2 is blocked on ARCH-001 LoopState decomposition).
pub(super) struct ConvergenceInput<'a> {
    pub convergence: &'a mut super::loop_state::ConvergenceState,
    pub guards: &'a mut super::loop_state::LoopGuardState,
    pub synthesis: &'a mut super::loop_state::SynthesisControl,
    pub policy: &'a halcon_core::types::PolicyConfig,
    pub round_number: u32,
    pub silent: bool,
    pub auto_pause: &'a mut bool,
    pub ctrl_cancelled: &'a mut bool,
    pub strategy_context: Option<&'a super::super::agent_types::StrategyContext>,
}

/// Run the convergence phase for one tool-use round.
///
/// Called after tool execution + deduplication. Handles ConvergenceController,
/// metacognitive monitoring, ctrl_rx yield, RoundScorer + signal assembly,
/// HICON Phase 4 anomaly correction, LoopGuard match arms, self-correction injection,
/// speculation cache clear, and auto-save.
pub(super) async fn run(
    state: &mut LoopState,
    session: &mut Session,
    render_sink: &dyn RenderSink,
    planner: Option<&dyn Planner>,
    planning_config: &PlanningConfig,
    request: &ModelRequest,
    ctrl_rx: &mut Option<ControlReceiver>,
    speculator: &super::super::tool_speculation::ToolSpeculator,
    trace_db: Option<&AsyncDatabase>,
    round: usize,
    round_tool_log: &[(String, u64)],
    tool_failures: &[(String, String)],
    tool_successes: &[String],
    round_usage: &TokenUsage,
    round_text_for_scorer: &str,
) -> Result<PhaseOutcome> {
    // SOTA 2026: ConvergenceController — observe this tool round for stagnation / over-run.
    // Uses round_tool_log (collected above) which contains (tool_name, args_hash) pairs
    // identical to those used by ToolLoopGuard's deduplication logic.
    // Sprint 2: capture convergence action for RoundFeedback construction below.
    let mut round_convergence_action =
        super::super::convergence_controller::ConvergenceAction::Continue;
    {
        use super::super::convergence_controller::ConvergenceAction;
        let conv_names: Vec<String> =
            round_tool_log.iter().map(|(n, _)| n.clone()).collect();
        let conv_hashes: Vec<u64> =
            round_tool_log.iter().map(|(_, h)| *h).collect();
        let had_errors = !tool_failures.is_empty();

        let ca = state.convergence.conv_ctrl.observe_round(
            round as u32,
            &conv_names,
            &conv_hashes,
            &state.full_text,
            had_errors,
        );
        round_convergence_action = ca.clone();
        // Phase 1: emit ConvergenceDecided for every round so offline analysis
        // can correlate controller decisions with oracle outcomes.
        super::loop_events::emit(
            &state.session_id.to_string(),
            round as u32,
            super::loop_events::LoopEvent::ConvergenceDecided {
                round,
                action: format!("{:?}", round_convergence_action),
                coverage: state.convergence.mid_loop_critic.evidence_rate() as f32,
            },
            trace_db,
        );
        match ca {
            ConvergenceAction::Synthesize => {
                // P0-2: Do NOT early-return here — oracle adjudicates after all signals
                // are collected. Render and flag; oracle dispatch handles the BreakLoop.
                tracing::info!(round, "ConvergenceController: Synthesize — stagnation confirmed");
                if !state.silent {
                    render_sink.loop_guard_action(
                        "convergence_synthesize",
                        "stagnation detected; synthesizing accumulated results",
                    );
                }
                // Mark convergence_directive_injected so oracle InjectSynthesis handler
                // (if oracle picks InjectSynthesis from a lower-priority source) will not
                // inject a duplicate synthesis message this round.
                state.synthesis.convergence_directive_injected = true;
            }
            ConvergenceAction::Replan => {
                tracing::info!(round, "ConvergenceController: Replan — injecting directive");
                if !state.silent {
                    render_sink.loop_guard_action(
                        "convergence_replan",
                        "stagnation detected; injecting replan directive",
                    );
                }
                // Inject a User-visible directive to force a new approach.
                // Does NOT consume a max_replan_attempts slot (that counter governs
                // model-initiated ReplanRequired, not convergence-driven nudges).
                state.messages.push(ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text(
                        "[ConvergenceController]: You are repeating the same tool calls \
                         without making progress toward the goal. Revise your approach: \
                         stop calling tools you have already used with the same arguments, \
                         reconsider your plan, and try a different strategy."
                            .to_string(),
                    ),
                });
                // Phase 113: Signal that a convergence directive was injected this round.
                // ToolLoopGuard's InjectSynthesis will check this and skip if set —
                // preventing two conflicting User directives in the same round.
                state.synthesis.convergence_directive_injected = true;
            }
            ConvergenceAction::Halt => {
                // P0-2: Do NOT early-return here — oracle adjudicates after all signals
                // are collected. Render; oracle dispatch handles the BreakLoop.
                tracing::warn!(round, "ConvergenceController: Halt — max state.rounds exceeded");
                if !state.silent {
                    render_sink.loop_guard_action(
                        "convergence_halt",
                        "maximum convergence state.rounds reached; halting",
                    );
                }
            }
            ConvergenceAction::Continue => {}
        }
    }

    // Phase 1: Mid-session intent over-run detector (additive — detection only, no behavior change).
    //
    // When `tools_executed` exceeds `plan_steps_total * 2`, the loop is doing significantly more
    // work than initially estimated. Emit a structured `IntentRescored` event for telemetry;
    // Phase 6 will act on this via `IntentLock`. No state mutation here.
    {
        let plan_steps_total = state.active_plan.as_ref().map(|p| p.steps.len()).unwrap_or(0);
        let tools_count = state.tools_executed.len();
        if plan_steps_total > 0 && tools_count >= plan_steps_total.saturating_mul(2) {
            let intent_str = format!("{:?}", state.synthesis.execution_intent);
            tracing::info!(
                round,
                tools_count,
                plan_steps_total,
                intent = %intent_str,
                "Phase1/intent-overscan: tools_executed ({tools_count}) \
                 >= plan_steps ({plan_steps_total}) * 2 — scope may need re-evaluation"
            );
            super::loop_events::emit(
                &state.session_id.to_string(),
                round as u32,
                super::loop_events::LoopEvent::IntentRescored {
                    old_scope: intent_str.clone(),
                    new_scope: intent_str,
                    trigger: "tools_overrun_2x".into(),
                    tools_executed_count: tools_count,
                    plan_steps_total,
                },
                trace_db,
            );
        }
    }

    // HICON Phase 6: Metacognitive monitoring (collect component observations)
    {
        use super::super::metacognitive_loop::{ComponentObservation, SystemComponent};
        use std::collections::HashMap;

        // Observe loop guard health
        let loop_guard_health = if state.guards.loop_guard.consecutive_rounds() == 0 {
            1.0
        } else {
            1.0 - (state.guards.loop_guard.consecutive_rounds() as f64 / 10.0).min(1.0)
        };

        let mut metrics = HashMap::new();
        metrics.insert("consecutive_rounds".to_string(), state.guards.loop_guard.consecutive_rounds() as f64);

        state.hicon.metacognitive_loop.monitor(ComponentObservation {
            component: SystemComponent::LoopGuard,
            round: round + 1,
            metrics,
            health: loop_guard_health,
        });

        // Observe self-corrector health
        let corrector_stats = state.hicon.self_corrector.stats();
        let corrector_health = if corrector_stats.total_corrections > 0 {
            corrector_stats.success_rate
        } else {
            1.0
        };

        let mut corrector_metrics = HashMap::new();
        corrector_metrics.insert("corrections".to_string(), corrector_stats.total_corrections as f64);
        corrector_metrics.insert("success_rate".to_string(), corrector_stats.success_rate);

        state.hicon.metacognitive_loop.monitor(ComponentObservation {
            component: SystemComponent::SelfCorrector,
            round: round + 1,
            metrics: corrector_metrics,
            health: corrector_health,
        });

        // Observe resource predictor health
        let predictor_health = if state.hicon.resource_predictor.is_ready() { 1.0 } else { 0.5 };

        state.hicon.metacognitive_loop.monitor(ComponentObservation {
            component: SystemComponent::ResourcePredictor,
            round: round + 1,
            metrics: HashMap::new(),
            health: predictor_health,
        });
    }

    // HICON Phase 6: Run full metacognitive cycle every 10 rounds
    if state.hicon.metacognitive_loop.should_run_cycle(round + 1) {
        let analysis = state.hicon.metacognitive_loop.analyze(round + 1);
        let plan = state.hicon.metacognitive_loop.adapt(&analysis);
        let insight = state.hicon.metacognitive_loop.reflect(&plan);

        tracing::info!(
            round = round + 1,
            phi = insight.phi.phi,
            integration = insight.phi.integration,
            differentiation = insight.phi.differentiation,
            quality = ?insight.phi.quality(),
            trend = ?insight.trend,
            meets_target = insight.meets_target,
            "Metacognitive cycle: Φ coherence measured"
        );

        state.hicon.metacognitive_loop.integrate(&insight, round + 1);

        // Remediation Phase 1.2: Make Phi coherence visible to user
        let status = if insight.phi.phi >= 0.7 {
            "healthy"
        } else if insight.phi.phi >= 0.5 {
            "degraded"
        } else {
            "critical"
        };
        render_sink.hicon_coherence(insight.phi.phi, round + 1, status);
    }

    // Phase 43: Check control channel after tool execution (yield point 2).
    if let Some(ref mut rx) = ctrl_rx {
        match check_control(rx, render_sink).await {
            super::ControlAction::Continue => {}
            super::ControlAction::StepOnce => { state.auto_pause = true; }
            super::ControlAction::Cancel => {
                state.ctrl_cancelled = true;
                return Ok(PhaseOutcome::BreakLoop);
            }
        }
    }

    // Phase 33: intelligent tool loop guard — graduated escalation.
    // Uses the round_tool_log collected before dedup (above) for full
    // (tool_name, args_hash) tracking.
    // `mut` required so Phase 2 causal wiring can override to ReplanRequired when
    // state.convergence.round_scorer.should_trigger_replan() fires (low-trajectory override path).
    let mut loop_action = state.guards.loop_guard.record_round(round_tool_log);

    // P0-2: Declare oracle_decision before the RoundScorer+RoundFeedback scoped block so
    // it survives into the oracle dispatch section below (after HICON Phase 4).
    let mut oracle_decision: Option<super::super::termination_oracle::TerminationDecision> = None;

    // Phase 2: RoundScorer — score this round and accumulate for reward pipeline.
    // Collect anomaly flags from the loop guard BEFORE take_last_anomaly() consumes them.
    {
        let (rs_completed, rs_total, _) = if let Some(ref t) = state.execution_tracker {
            t.progress()
        } else { (0, 1, 0) };
        let rs_progress_ratio = if rs_total > 0 {
            rs_completed as f32 / rs_total as f32
        } else { 0.0 };
        // Reflect loop_action into anomaly flags for RoundScorer coherence.
        // (take_last_anomaly() is called below by HICON Phase 4 — don't consume here.)
        let anomaly_flags: Vec<String> = match loop_action {
            super::super::loop_guard::LoopAction::Break => vec!["LoopBreak".to_string()],
            super::super::loop_guard::LoopAction::ReplanRequired => vec!["Stagnation".to_string()],
            super::super::loop_guard::LoopAction::ForceNoTools => vec!["ForceNoTools".to_string()],
            _ => vec![],
        };
        let eval = state.convergence.round_scorer.score_round(
            round,
            tool_successes.len(),
            tool_successes.len() + tool_failures.len(),
            round_usage.output_tokens as u64,
            round_usage.input_tokens as u64,
            rs_progress_ratio,
            anomaly_flags,
            round_text_for_scorer,
        );
        // Use RoundScorer structural signals to reinforce LoopGuard:
        // consecutive regressions → force synthesis early (before escalation threshold).
        if state.convergence.round_scorer.should_inject_synthesis() {
            tracing::info!(round, "RoundScorer: consecutive regressions → reinforcing synthesis directive");
            state.guards.loop_guard.force_synthesis();
        }
        // Phase 2 causal wiring: should_trigger_replan() was previously computed but
        // NEVER applied to loop_action — a phantom signal. Wire it here so persistent
        // low-trajectory rounds drive structural replanning through the existing
        // ReplanRequired handler (with its budget guard at max_replan_attempts).
        // Only override when loop_action is still Continue/ForceNoTools — do NOT
        // override Break (loop guard terminal) or InjectSynthesis (synthesis takes
        // priority over replan: synthesis is a softer signal that may resolve stagnation
        // without the cost of a full LLM replan call) or ReplanRequired (already set).
        if state.convergence.round_scorer.should_trigger_replan()
            && !matches!(
                loop_action,
                super::super::loop_guard::LoopAction::Break
                    | super::super::loop_guard::LoopAction::ReplanRequired
                    | super::super::loop_guard::LoopAction::InjectSynthesis
            )
        {
            tracing::info!(
                round,
                replan_sensitivity = ?state.strategy_context.as_ref().map(|sc| sc.replan_sensitivity),
                "RoundScorer: persistent low trajectory → structural replan triggered"
            );
            loop_action = super::super::loop_guard::LoopAction::ReplanRequired;
        }
        tracing::debug!(
            round,
            combined_score = eval.combined_score,
            progress_delta = eval.progress_delta,
            tool_efficiency = eval.tool_efficiency,
            stagnation = eval.stagnation_flag,
            "RoundScorer evaluation"
        );
        // Sprint 1-3: Assemble formal RoundFeedback entity (infrastructure → domain boundary).
        // Aggregates signals from RoundScorer, ConvergenceController, and LoopGuard into a
        // single typed domain value consumed by TerminationOracle and AdaptivePolicy.
        {
            use super::super::round_feedback::{LoopSignal, RoundFeedback};
            let loop_sig = match &loop_action {
                super::super::loop_guard::LoopAction::Break => LoopSignal::Break,
                super::super::loop_guard::LoopAction::ReplanRequired => LoopSignal::ReplanRequired,
                super::super::loop_guard::LoopAction::InjectSynthesis => LoopSignal::InjectSynthesis,
                super::super::loop_guard::LoopAction::ForceNoTools => LoopSignal::ForceNoTools,
                super::super::loop_guard::LoopAction::Continue => LoopSignal::Continue,
            };
            // Phase 5 Governance: mini-critic runs BEFORE oracle — its signal becomes
            // INPUT to the oracle adjudication rather than a post-override.
            let mini_critic_signal = mini_critic_check(state, round);
            // ReduceTools does not affect oracle (just sets force_next), so only encode
            // ForceReplan and ForceSynthesis as boolean signals.
            let mini_critic_replan = matches!(mini_critic_signal, Some(MiniCriticAction::ForceReplan));
            let mini_critic_synthesis = matches!(mini_critic_signal, Some(MiniCriticAction::ForceSynthesis));
            // Apply ReduceTools side-effect immediately (does not affect oracle decision).
            if matches!(mini_critic_signal, Some(MiniCriticAction::ReduceTools)) {
                tracing::info!(round, "MiniCritic: declining trend → reducing tool scope");
                state.synthesis.tool_decision.set_force_next();
            }

            // ── Phase 3: Populate domain intelligence signals ──────────────
            let effective_max_rounds = state.sla_budget.as_ref()
                .map(|s| s.max_rounds as usize)
                .unwrap_or(20);
            let evidence_coverage = state.evidence.graph.synthesis_coverage();

            // P3.3: Semantic cycle detection (already fed in post_batch)
            let semantic_cycle_detected = state.guards.semantic_cycle_detector.has_cycle();
            let cycle_severity = state.guards.semantic_cycle_detector.severity().as_f32();

            // P3.4: Mid-loop critic checkpoints
            let plan_progress = state.execution_tracker.as_ref()
                .map(|t| { let (done, total, _) = t.progress(); if total > 0 { done as f64 / total as f64 } else { 0.0 } })
                .unwrap_or(0.0);
            state.convergence.mid_loop_critic.record_snapshot(
                round, plan_progress, evidence_coverage, eval.combined_score,
                !(tool_successes.is_empty() && tool_failures.is_empty()),
            );
            let mid_critic_action = if state.convergence.mid_loop_critic.is_checkpoint(round) {
                let cp = state.convergence.mid_loop_critic.evaluate(
                    round,
                    effective_max_rounds,
                    plan_progress,
                    evidence_coverage,
                    state.convergence.cumulative_drift_score as f64,
                );
                tracing::debug!(
                    round, ?cp.action, deficit = %cp.progress_deficit,
                    "Phase3 MidLoopCritic checkpoint"
                );
                // Surface structured checkpoint summary to the user (skip on Continue — no noise).
                if cp.action != super::super::domain::mid_loop_critic::CriticAction::Continue
                    && !state.silent
                {
                    render_sink.info(&cp.format_summary());
                }
                Some(cp.action)
            } else {
                None
            };

            // P3.5: Complexity feedback
            let complexity_upgraded = {
                let obs = super::super::domain::complexity_feedback::ComplexityObservation {
                    rounds_used: round + 1,
                    replans_triggered: state.convergence.replan_attempts as usize,
                    distinct_tools_used: state.tools_executed.iter()
                        .collect::<std::collections::HashSet<_>>().len(),
                    domains_touched: 1, // simplified — single domain default
                    elapsed_secs: state.loop_start.elapsed().as_secs_f64(),
                    orchestration_used: state.orchestration_decision.is_some(),
                    tool_errors: tool_failures.len(),
                };
                match state.convergence.complexity_tracker.evaluate(&obs) {
                    Some(adj) if adj.was_upgraded => {
                        tracing::info!(
                            original = ?adj.original, adjusted = ?adj.adjusted,
                            confidence = %adj.confidence,
                            "Phase3 ComplexityTracker: upgrade triggered"
                        );
                        true
                    }
                    _ => false,
                }
            };

            // P5.2: Problem classification (after classification_min_rounds)
            let problem_class = if round >= state.policy.classification_min_rounds {
                if state.convergence.problem_classifier.current().is_none() {
                    let metrics = state.convergence.metrics_collector.rounds();
                    let sla_frac = state.sla_budget.as_ref()
                        .map(|s| s.fraction_consumed()).unwrap_or(0.0);
                    let result = state.convergence.problem_classifier.classify(metrics, sla_frac);
                    tracing::debug!(
                        class = %result.class.label(),
                        confidence = %result.confidence,
                        "Phase5 ProblemClassifier: initial classification"
                    );
                    // P5.3: Apply class-specific weight preset
                    state.convergence.strategy_weight_manager.set_baseline(
                        super::super::domain::strategy_weights::StrategyWeights::for_class(result.class),
                    );
                    Some(result.class)
                } else {
                    // Check for reclassification
                    let metrics = state.convergence.metrics_collector.rounds();
                    let sla_frac = state.sla_budget.as_ref()
                        .map(|s| s.fraction_consumed()).unwrap_or(0.0);
                    if let Some(reclass) = state.convergence.problem_classifier.reclassify(metrics, sla_frac) {
                        tracing::debug!(
                            class = %reclass.class.label(),
                            "Phase5 ProblemClassifier: reclassified"
                        );
                        state.convergence.strategy_weight_manager.set_baseline(
                            super::super::domain::strategy_weights::StrategyWeights::for_class(reclass.class),
                        );
                    }
                    state.convergence.problem_classifier.current_class()
                }
            } else {
                state.convergence.problem_classifier.current_class()
            };

            // P3.6: Convergence utility function
            let sla_fraction = state.sla_budget.as_ref()
                .map(|s| s.fraction_consumed())
                .unwrap_or(0.0);
            let token_fraction = if state.tokens.pipeline_budget > 0 {
                (state.tokens.call_input_tokens + state.tokens.call_output_tokens) as f64
                    / state.tokens.pipeline_budget as f64
            } else {
                0.0
            };
            let utility_inputs = super::super::domain::convergence_utility::UtilityInputs {
                evidence_coverage,
                coherence_score: eval.combined_score as f64,
                plan_progress,
                time_pressure: sla_fraction.max(token_fraction),
                retry_cost: if state.tokens.pipeline_budget > 0 {
                    (state.tokens.call_input_tokens as f64) / (state.tokens.pipeline_budget as f64)
                } else { 0.0 },
                drift_penalty: state.convergence.cumulative_drift_score as f64,
                evidence_rate: state.convergence.mid_loop_critic.evidence_rate(),
            };
            // P5.3: Use strategy-managed weights for utility computation
            let strategy_utility_weights = state.convergence.strategy_weight_manager.current()
                .to_utility_weights(state.policy.utility_w_progress);
            let utility_result = super::super::domain::convergence_utility::compute_utility(
                &utility_inputs,
                &strategy_utility_weights,
                state.policy.utility_synthesis_threshold,
                state.policy.utility_marginal_threshold,
            );
            let utility_score = utility_result.utility;

            // P5.4: Convergence forecast
            let utility_trend = state.convergence.metrics_collector.utility_trend();
            let sla_remaining = state.sla_budget.as_ref()
                .map(|s| (s.max_rounds as usize).saturating_sub(round + 1))
                .unwrap_or(10);
            let evidence_rate_val = state.convergence.mid_loop_critic.evidence_rate();
            let forecast = super::super::domain::convergence_estimator::forecast(
                state.convergence.metrics_collector.rounds(),
                utility_trend,
                evidence_rate_val,
                sla_remaining,
                state.policy.utility_synthesis_threshold,
                state.policy.forecast_min_rounds,
            );
            let forecast_rounds_remaining = Some(forecast.estimated_rounds_remaining);

            // ARCH-SYNC-1: Compute GovernanceRescue gate BEFORE TerminationOracle runs.
            // DECISION (ARCH-SYNC-1): Call SynthesisGate::evaluate() directly instead of
            // duplicating the suppression logic inline. This ensures that if the suppression
            // thresholds change in synthesis_gate.rs, convergence_phase.rs automatically
            // picks up the change — eliminating the risk of the two copies diverging.
            // The gate must run BEFORE adjudicate() so TerminationOracle receives the
            // governance_rescue_active flag and can downgrade InjectSynthesis to Continue.
            let governance_rescue_active = {
                let sg_ctx = state.build_synthesis_context();
                let verdict = super::super::domain::synthesis_gate::evaluate(
                    SynthesisTrigger::GovernanceRescue,
                    &sg_ctx,
                );
                // governance_rescue_active = true when the gate would block synthesis
                !verdict.allow
            };

            let round_feedback = RoundFeedback {
                round,
                combined_score: eval.combined_score,
                convergence_action: round_convergence_action.clone(),
                loop_signal: loop_sig,
                trajectory_trend: state.convergence.round_scorer.trend_score(),
                oscillation: state.convergence.round_scorer.oscillation_penalty(),
                replan_advised: state.convergence.round_scorer.should_trigger_replan(),
                synthesis_advised: state.convergence.round_scorer.should_inject_synthesis(),
                tool_round: !(tool_successes.is_empty() && tool_failures.is_empty()),
                had_errors: !tool_failures.is_empty(),
                mini_critic_replan,
                mini_critic_synthesis,
                evidence_coverage,
                semantic_cycle_detected,
                cycle_severity,
                utility_score,
                mid_critic_action,
                complexity_upgraded,
                problem_class,
                forecast_rounds_remaining,
                utility_should_synthesize: false,
                synthesis_request_count: state.synthesis.request_count() as u32,
                fsm_error_count: state.synthesis.fsm_error_count,
                budget_iteration_count: 0,
                budget_stagnation_count: 0,
                budget_token_growth: 0,
                budget_exhausted: false,
                executive_signal_count: 0,
                executive_force_reason: None,
                capability_violation: None,
                security_signals_detected: false,
                tool_call_count: (tool_successes.len() + tool_failures.len()) as u32,
                tool_failure_count: tool_failures.len() as u32,
                governance_rescue_active,
            };

            // P0-2: TerminationOracle — AUTHORITATIVE (shadow mode removed).
            // Both ConvergenceController and LoopGuard have set their signals into
            // round_feedback. Oracle adjudicates with explicit precedence ordering.
            // Dispatch happens after HICON Phase 4 (below) to preserve anomaly correction.
            let termination =
                super::super::termination_oracle::TerminationOracle::adjudicate(&round_feedback);
            tracing::info!(
                ?termination,
                round,
                "TerminationOracle: authoritative decision"
            );
            oracle_decision = Some(termination.clone());
            // Phase 1: emit OracleDecided so every adjudication is persisted for offline analysis.
            super::loop_events::emit(
                &state.session_id.to_string(),
                round as u32,
                super::loop_events::LoopEvent::OracleDecided {
                    round,
                    decision: format!("{:?}", termination),
                    combined_score: round_feedback.combined_score,
                    evidence_coverage: evidence_coverage as f32,
                },
                trace_db,
            );

            // B2: RoutingAdaptor — mid-session escalation check.
            // Fires after TerminationOracle so oracle retains final say on loop termination.
            // Only runs when a BoundaryDecision is present (use_boundary_decision_engine=true).
            // On escalation, updates boundary_decision.routing.mode so downstream convergence
            // policy and future RoutingAdaptor calls see the escalated mode.
            if let Some(ref mut bd) = state.boundary_decision {
                use super::super::decision_engine::{PolicyStore, RoutingAdaptor};
                let store = PolicyStore::from_config(&state.policy);
                if let Some(escalation) = RoutingAdaptor::check(
                    bd.routing.mode,
                    round as u32,
                    &round_feedback,
                    &store,
                ) {
                    tracing::warn!(
                        from = %escalation.from.label(),
                        to = %escalation.to.label(),
                        rationale = escalation.rationale,
                        round_budget_increase = escalation.round_budget_increase,
                        round,
                        "RoutingAdaptor: mid-session escalation"
                    );
                    bd.routing.mode = escalation.to;
                    // Extend the loop's max-rounds budget so the escalated mode gets
                    // the rounds it needs (adds the delta from the PolicyStore).
                    if escalation.round_budget_increase > 0 {
                        let new_max = state.convergence.conv_ctrl.max_rounds() as usize
                            + escalation.round_budget_increase as usize;
                        state.convergence.conv_ctrl.set_max_rounds(new_max);
                        // PARTIAL-1 fix: keep sla_budget.max_rounds in sync with conv_ctrl
                        // so SLA pressure metrics (sla_fraction, allows_retry) see the extended budget.
                        if let Some(ref mut sla) = state.sla_budget {
                            sla.max_rounds = new_max as u32;
                        }
                    }
                    // GAP-4 fix: record escalation for result_assembly surfacing.
                    state.convergence.routing_escalation_count += 1;
                }
            }

            // P4.2: Record oracle decision in trace
            {
                use super::super::domain::agent_decision_trace::{DecisionRecord, DecisionPoint};
                let trace_record = DecisionRecord::new(
                    DecisionPoint::OracleAdjudication,
                    round,
                    format!("{:?}", termination),
                )
                .with_input("combined_score", format!("{:.3}", round_feedback.combined_score))
                .with_input("convergence_action", format!("{:?}", round_feedback.convergence_action))
                .with_input("loop_signal", format!("{:?}", round_feedback.loop_signal))
                .with_input("utility_score", format!("{:.3}", round_feedback.utility_score))
                .with_rationale(format!(
                    "synthesis_advised={}, replan_advised={}, evidence_cov={:.2}",
                    round_feedback.synthesis_advised,
                    round_feedback.replan_advised,
                    round_feedback.evidence_coverage,
                ));
                state.convergence.decision_trace.record(trace_record);
            }

            // Sprint 3: AdaptivePolicy — within-session parameter adaptation (active, L6).
            // Observes the round's trajectory and adjusts replan_sensitivity if declining.
            let policy_adj = state.convergence.adaptive_policy.observe(&round_feedback);
            if policy_adj.replan_sensitivity_delta > 0.0 {
                state.convergence.round_scorer
                    .set_replan_sensitivity(state.convergence.adaptive_policy.current_sensitivity());
                tracing::info!(
                    delta = policy_adj.replan_sensitivity_delta,
                    new_sensitivity = state.convergence.adaptive_policy.current_sensitivity(),
                    ?policy_adj.rationale,
                    "AdaptivePolicy: replan_sensitivity escalated within session",
                );
            }
            // Wire synthesis_urgency_boost → ConvergenceController (Phase 134).
            // When AdaptivePolicy detects oscillation it returns a non-zero boost;
            // forwarding it lowers the synthesis trigger threshold so the loop exits
            // sooner instead of continuing to oscillate.  Domain-pure: no infra imports.
            if policy_adj.synthesis_urgency_boost > 0.0 {
                state.convergence.conv_ctrl.boost_synthesis_urgency(policy_adj.synthesis_urgency_boost);
                tracing::debug!(
                    boost = policy_adj.synthesis_urgency_boost,
                    round,
                    "AdaptivePolicy → ConvergenceController: synthesis urgency boosted (oscillation detected)",
                );
            }
            if policy_adj.model_downgrade_advisory {
                // Wire model_downgrade_advisory → LoopState flag (Phase 134).
                // round_setup.rs reads the flag next round to log a structured advisory
                // and (Phase 135+) act on it with per-round ModelRouter re-evaluation.
                state.model_downgrade_advisory_active = true;
                tracing::info!(
                    trend = round_feedback.trajectory_trend,
                    round,
                    "AdaptivePolicy: model downgrade advisory — current tier underperforming",
                );
            }

            // P5.3: Per-round strategy weight micro-adjustment for next round
            if let Some(pc) = problem_class {
                if let Some(adj) = state.convergence.strategy_weight_manager.adjust(&round_feedback, &pc) {
                    tracing::debug!(
                        total_shift = %adj.total_shift,
                        bounded = adj.bounded,
                        rationale = adj.rationale,
                        "Phase5 StrategyWeights: per-round adjustment"
                    );
                }
            }

            // ── P4.1: System Invariants — per-round validation ──────────────
            {
                use super::super::domain::system_invariants::InvariantSnapshot;

                let sla_ordinal = state.sla_budget.as_ref().map(|s| {
                    match s.mode {
                        super::super::sla_manager::SlaMode::Fast => 0u8,
                        super::super::sla_manager::SlaMode::Balanced => 1,
                        super::super::sla_manager::SlaMode::Deep => 2,
                    }
                }).unwrap_or(1);

                let snap = InvariantSnapshot {
                    synthesis_blocked: state.evidence.bundle.synthesis_blocked,
                    synthesis_origin_is_supervisor: state.synthesis.synthesis_origin
                        .as_ref()
                        .map(|o| matches!(o, super::loop_state::SynthesisOrigin::SupervisorFailure))
                        .unwrap_or(false),
                    evidence_bytes: state.evidence.bundle.text_bytes_extracted,
                    min_evidence_bytes: state.policy.min_evidence_bytes,
                    content_read_attempts: state.evidence.bundle.content_read_attempts,
                    is_synthesis_round: state.synthesis.is_synthesis_forced()
                        || state.cached_tools.is_empty(),
                    forced_synthesis_detected: state.synthesis.is_synthesis_forced(),
                    sla_mode_ordinal: sla_ordinal,
                    complexity_upgrade_count: if state.convergence.complexity_tracker.was_upgraded() {
                        1
                    } else {
                        0
                    },
                    replan_attempts: state.convergence.replan_attempts,
                    max_replan_attempts: state.policy.max_replan_attempts,
                    round,
                    utility_score: round_feedback.utility_score,
                    accumulated_cost: state.tokens.call_cost,
                    fsm_phase: state.synthesis.phase_str(),
                    tool_suppression_active: state.synthesis.tool_decision.is_active(),
                    cumulative_drift: state.convergence.cumulative_drift_score,
                    max_drift_bound: state.policy.max_drift_bound,
                };
                let violations = state.convergence.invariant_checker.check_round(&snap);
                if violations > 0 {
                    tracing::warn!(
                        round,
                        violations,
                        "P4.1: system invariant violations detected"
                    );
                }

                // ── P4.3: Structured metrics collection ─────────────────────
                {
                    use super::super::domain::system_metrics::RoundMetrics;

                    let round_metrics = RoundMetrics {
                        round,
                        tokens_in: state.tokens.call_input_tokens,
                        tokens_out: state.tokens.call_output_tokens,
                        tool_calls: tool_successes.len() + tool_failures.len(),
                        tool_errors: tool_failures.len(),
                        combined_score: round_feedback.combined_score,
                        utility_score: round_feedback.utility_score,
                        evidence_coverage: round_feedback.evidence_coverage,
                        drift_score: state.convergence.cumulative_drift_score,
                        sla_fraction: state.sla_budget.as_ref()
                            .map(|s| s.fraction_consumed())
                            .unwrap_or(0.0),
                        token_fraction: if state.tokens.pipeline_budget > 0 {
                            (state.tokens.call_input_tokens + state.tokens.call_output_tokens) as f64
                                / state.tokens.pipeline_budget as f64
                        } else { 0.0 },
                        replan_attempts: state.convergence.replan_attempts,
                        invariant_violations: violations,
                        cycle_count: state.guards.semantic_cycle_detector.cycle_count(),
                        round_duration: state.loop_start.elapsed(),
                        oracle_decision: format!("{:?}", oracle_decision.as_ref().unwrap_or(
                            &super::super::domain::termination_oracle::TerminationDecision::Continue
                        )),
                    };
                    state.convergence.metrics_collector.record_round(round_metrics);
                }
            }
        }

        state.convergence.round_evaluations.push(eval);
    }

    // HICON Phase 4: Check for detected anomaly and apply self-correction.
    if let Some(anomaly_result) = state.guards.loop_guard.take_last_anomaly() {
        tracing::info!(
            round,
            anomaly_type = ?anomaly_result.anomaly,
            severity = ?anomaly_result.severity,
            "Anomaly detected — applying self-correction"
        );

        // Remediation Phase 1.2: Make anomaly visible to user
        let anomaly_type_str = format!("{:?}", anomaly_result.anomaly);
        let severity_str = format!("{:?}", anomaly_result.severity);
        let details = format!("Detected at round {round}");
        // Extract confidence from anomaly variant if available, else use high confidence (0.85)
        let confidence = match &anomaly_result.anomaly {
            AgentAnomaly::ReadSaturation { probability, .. } => *probability,
            _ => 0.85, // High confidence for other detected anomalies
        };
        render_sink.hicon_anomaly(&anomaly_type_str, &severity_str, &details, confidence);

        // Select appropriate correction strategy
        if let Some(strategy) = state.hicon.self_corrector.select_strategy(
            &anomaly_result.anomaly,
            anomaly_result.severity,
            round,
        ) {
            // Remediation Phase 1.2: Make correction visible to user (before apply consumes strategy)
            let strategy_name = format!("{:?}", strategy);
            let reason = format!("Responding to {:?} anomaly", anomaly_result.anomaly);
            render_sink.hicon_correction(&strategy_name, &reason, round);

            // Apply correction (may modify system prompt and/or inject message)
            let current_system = state.cached_system.as_deref().unwrap_or("");
            let (new_system, injected_msg) = state.hicon.self_corrector.apply_strategy(
                strategy,
                current_system,
                round,
                anomaly_result.severity,
            );

            // Update system prompt if modified
            if let Some(updated_system) = new_system {
                state.cached_system = Some(updated_system);
                tracing::debug!(round, "System prompt updated by self-corrector");
            }

            // Inject message if provided
            if let Some(msg) = injected_msg {
                state.messages.push(msg.clone());
                state.context_pipeline.add_message(msg.clone());
                session.add_message(msg);
                tracing::debug!(round, "Message injected by self-corrector");
            }
        }
    }

    // P0-2: TerminationOracle authoritative dispatch.
    // oracle_decision was computed from the assembled RoundFeedback above (after both
    // ConvergenceController and LoopGuard have set their signals). Loop_action is still
    // logged for observability alongside the oracle verdict.
    let is_loop_guard_break = matches!(loop_action, LoopAction::Break);
    tracing::info!(
        round,
        consecutive_tool_rounds = state.guards.loop_guard.consecutive_rounds(),
        underlying_loop_action = ?loop_action,
        oscillation = state.guards.loop_guard.detect_oscillation(),
        read_saturation = state.guards.loop_guard.detect_read_saturation(),
        "TerminationOracle dispatching (authoritative)"
    );

    use super::super::termination_oracle::{ReplanReason, SynthesisReason, TerminationDecision};

    // Phase 5 Governance fix: mini-critic signals are now encoded in RoundFeedback
    // and consumed by the oracle's adjudication logic.  No post-override occurs here.
    // ReduceTools side-effect was already applied above (before RoundFeedback construction).

    match oracle_decision.expect("oracle_decision always set in RoundFeedback block above") {
        // ── Precedence 1: Halt ──────────────────────────────────────────────
        TerminationDecision::Halt => {
            // Capture pre-mutation forced state before mark_synthesis_forced_with_gate()
            // sets forced_synthesis_detected=true. The guard below must reflect whether
            // synthesis was ALREADY forced before this oracle Halt arm ran, not after.
            let synthesis_was_already_forced = state.synthesis.is_synthesis_forced();
            if is_loop_guard_break {
                // LoopSignal::Break = oscillation / plan complete → ForcedSynthesis.
                tracing::warn!(
                    consecutive_tool_rounds = state.guards.loop_guard.consecutive_rounds(),
                    "Oracle Halt: loop guard break (oscillation or plan complete)"
                );
                if !state.silent {
                    render_sink.warning(
                        &format!(
                            "auto-stopped after {} consecutive tool state.rounds (pattern detected)",
                            state.guards.loop_guard.consecutive_rounds()
                        ),
                        Some("Oscillation or plan completion detected — synthesizing response."),
                    );
                }
                // Mark as ForcedSynthesis so post-loop correctly classifies this stop.
                // Phase 2: route through governance gate (oracle halt decision).
                state.mark_synthesis_forced_with_gate(
                    SynthesisTrigger::OracleConvergence,
                    SynthesisOrigin::OracleConvergence,
                );
            }
            // FIX (RC-1): Use pre-mutation snapshot. Previously this checked
            // is_synthesis_forced() AFTER mark_synthesis_forced_with_gate() set it to
            // true, making the condition always false when is_loop_guard_break=true —
            // the synthesis injection branch was dead code and the agent always exited
            // without producing a final response. Guard: if synthesis was ALREADY forced
            // before this arm ran (we already did a synthesis sub-round but oracle fired
            // Halt again), break for real.
            if !synthesis_was_already_forced {
                // V5 fix (2026-02-27): Investigative task synthesis guard.
                // If the oracle is attempting to force synthesis on an Investigation task
                // but ZERO real tool calls were executed, this is suspicious — it likely
                // means the plan was generated but tools never ran (empty tool surface,
                // sub-agent failure, or MCP server unavailable). Emit a structured WARN
                // so the session is visible in telemetry; synthesis still proceeds but
                // the LoopCritic (V4 fix) will evaluate the output adversarially.
                // Restriction: does NOT block synthesis (avoiding deadlock) but marks the
                // session with `synthesis_origin = SupervisorFailure` for reward dampening.
                if matches!(state.synthesis.execution_intent, ExecutionIntentPhase::Investigation)
                    && state.tools_executed.is_empty()
                {
                    tracing::warn!(
                        session_id = %state.session_id,
                        intent = "Investigation",
                        tools_executed = 0,
                        round = round,
                        "AUDIT: Oracle injecting synthesis on Investigation task with 0 real tool calls. \
                         Possible causes: empty tool surface, MCP unavailable, sub-agent spawn failure. \
                         Marking synthesis_origin=SupervisorFailure for critic dampening."
                    );
                    if !state.silent {
                        render_sink.warning(
                            "[audit] synthesizing without tool execution — investigation task had 0 real tool calls",
                            Some("Check MCP server availability and tool surface configuration"),
                        );
                    }
                    // Mark as SupervisorFailure so reward pipeline applies synthesis penalty
                    // AND LoopCritic (V4) will adversarially evaluate the fabricated output.
                    state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                } else {
                    state.synthesis.synthesis_origin = Some(SynthesisOrigin::OracleConvergence);
                }
                // EBS Evidence Gate (EBS-1): check if sufficient readable content was
                // extracted before allowing synthesis. Gate fires when content-read tools
                // (read_file, read_multiple_files) were attempted but returned < threshold
                // bytes — indicating binary files (PDF), empty files, or permission errors.
                // When gate fires, the synthesis directive is replaced with a limitation
                // report directive so the model honestly reports it cannot read the files
                // instead of fabricating content from prior knowledge.
                let synth_text_halt = {
                    use super::super::evidence_pipeline::MIN_EVIDENCE_BYTES;
                    if state.evidence.bundle.evidence_gate_fires() {
                        state.evidence.bundle.synthesis_blocked = true;
                        // Gate fires → always mark as SupervisorFailure for reward dampening.
                        state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                        tracing::warn!(
                            session_id = %state.session_id,
                            text_bytes_extracted = state.evidence.bundle.text_bytes_extracted,
                            content_read_attempts = state.evidence.bundle.content_read_attempts,
                            binary_file_count = state.evidence.bundle.binary_file_count,
                            min_threshold = MIN_EVIDENCE_BYTES,
                            "EvidenceGate FIRED (Halt): synthesis replaced with limitation \
                             report directive. Content-read tools ran but extracted \
                             insufficient text (likely binary PDFs)."
                        );
                        if !state.silent {
                            render_sink.warning(
                                "[evidence-gate] synthesis blocked — file tools returned no readable text",
                                Some("Files may be binary (PDF). Injecting limitation report directive."),
                            );
                        }
                        state.evidence.bundle.gate_message()
                    } else {
                        let mut directive = "[System: You have gathered sufficient information. \
                         Please synthesize all your findings into a comprehensive \
                         final response for the user. Do not call any more tools.]"
                            .to_string();
                        // Phase 3 EvidenceGraph: advisory hint for unreferenced evidence.
                        let coverage = state.evidence.graph.synthesis_coverage();
                        if coverage < state.policy.min_synthesis_coverage
                            && state.evidence.graph.good_node_count() > 0
                        {
                            let unreferenced = state.evidence.graph.unreferenced_evidence();
                            let hints: Vec<String> = unreferenced.iter()
                                .take(5)
                                .map(|n| format!("- {} ({})", n.tool_args_summary, n.tool_name))
                                .collect();
                            if !hints.is_empty() {
                                directive.push_str(&format!(
                                    "\n\n[Note: The following evidence items were gathered but may not yet be reflected in your response:\n{}\nPlease consider incorporating them.]",
                                    hints.join("\n")
                                ));
                            }
                        }
                        directive
                    }
                };
                // Phase 2: annotate with gate classification (origin already set above).
                state.synthesis.annotate_synthesis_trigger(
                    SynthesisKind::Organic,
                    SynthesisTrigger::OracleConvergence,
                );
                state.synthesis.fire_synthesis();
                let synth_msg = ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text(synth_text_halt.into()),
                };
                state.messages.push(synth_msg.clone());
                state.context_pipeline.add_message(synth_msg.clone());
                session.add_message(synth_msg);
                state.synthesis.tool_decision = ToolDecisionSignal::ForcedByOracle;
                // FASE 6: TUI synthesis phase visibility.
                if !state.silent {
                    render_sink.phase_started("synthesis", "Oracle halt — synthesising final answer...");
                }
                return Ok(PhaseOutcome::NextRound);
            }
            // Already performed synthesis round — exit for real.
            if !state.silent {
                render_sink.phase_ended();
            }
            return Ok(PhaseOutcome::BreakLoop);
        }

        // ── Precedence 2: InjectSynthesis ───────────────────────────────────
        TerminationDecision::InjectSynthesis { reason } => {
            match reason {
                SynthesisReason::ConvergenceControllerSynthesizeAction => {
                    // Hard stop: stagnation confirmed by ConvergenceController.
                    // FIX: Same pattern as Halt — inject synthesis directive and allow
                    // one final tool-free round so the model produces a real response.
                    if !state.synthesis.is_synthesis_forced() {
                        // Plan-completion guard: if plan steps are still Pending (never attempted),
                        // the convergence controller may have fired early due to tool-output flooding
                        // (e.g., a huge directory_tree result contains all goal keywords, giving false
                        // high coverage). Defer synthesis and direct the model to execute them first.
                        let unattempted = state.execution_tracker.as_ref()
                            .map(|t| t.tracked_steps().iter()
                                .filter(|s| s.status == halcon_core::traits::TaskStatus::Pending)
                                .count())
                            .unwrap_or(0);
                        if unattempted > 0 {
                            tracing::info!(
                                round,
                                unattempted,
                                "Oracle: deferring ConvergenceCtrl synthesis — {unattempted} plan step(s) still Pending"
                            );
                            if !state.silent {
                                render_sink.info(&format!(
                                    "[plan-guard] deferring synthesis — {unattempted} step(s) not yet attempted"
                                ));
                            }
                            let plan_hint = ChatMessage {
                                role: Role::User,
                                content: MessageContent::Text(format!(
                                    "[System: Your plan has {unattempted} step(s) that have not been executed yet. \
                                     Please execute the remaining plan steps before synthesizing a response. \
                                     Continue using the required tools.]"
                                ).into()),
                            };
                            state.messages.push(plan_hint.clone());
                            state.context_pipeline.add_message(plan_hint.clone());
                            session.add_message(plan_hint);
                            return Ok(PhaseOutcome::NextRound);
                        }
                        // EBS Evidence Gate (EBS-2): same gate check for ConvergenceController
                        // hard-stop path. Prevents fabrication when stagnation is caused by
                        // unreadable binary files (tools ran in circles, found nothing).
                        let synth_text_conv = {
                            use super::super::evidence_pipeline::MIN_EVIDENCE_BYTES;
                            if state.evidence.bundle.evidence_gate_fires() {
                                state.evidence.bundle.synthesis_blocked = true;
                                tracing::warn!(
                                    session_id = %state.session_id,
                                    text_bytes_extracted = state.evidence.bundle.text_bytes_extracted,
                                    content_read_attempts = state.evidence.bundle.content_read_attempts,
                                    binary_file_count = state.evidence.bundle.binary_file_count,
                                    min_threshold = MIN_EVIDENCE_BYTES,
                                    "EvidenceGate FIRED (ConvergenceCtrl): replacing synthesis \
                                     with limitation report. Binary/unreadable files detected."
                                );
                                if !state.silent {
                                    render_sink.warning(
                                        "[evidence-gate] synthesis blocked — file tools returned no readable text",
                                        Some("Files may be binary (PDF). Injecting limitation report directive."),
                                    );
                                }
                                // Gate fires → SupervisorFailure for reward dampening.
                                state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                                state.evidence.bundle.gate_message()
                            } else {
                                state.synthesis.synthesis_origin = Some(SynthesisOrigin::OracleConvergence);
                                let mut directive = "[System: You have gathered sufficient information. \
                                 Please synthesize all your findings into a comprehensive \
                                 final response for the user. Do not call any more tools.]"
                                    .to_string();
                                // Phase 3 EvidenceGraph: advisory hint for unreferenced evidence.
                                let coverage = state.evidence.graph.synthesis_coverage();
                                if coverage < state.policy.min_synthesis_coverage
                                    && state.evidence.graph.good_node_count() > 0
                                {
                                    let unreferenced = state.evidence.graph.unreferenced_evidence();
                                    let hints: Vec<String> = unreferenced.iter()
                                        .take(5)
                                        .map(|n| format!("- {} ({})", n.tool_args_summary, n.tool_name))
                                        .collect();
                                    if !hints.is_empty() {
                                        directive.push_str(&format!(
                                            "\n\n[Note: The following evidence items were gathered but may not yet be reflected in your response:\n{}\nPlease consider incorporating them.]",
                                            hints.join("\n")
                                        ));
                                    }
                                }
                                directive
                            }
                        };
                        // Phase 2: annotate with gate classification (oracle convergence path).
                        state.synthesis.annotate_synthesis_trigger(
                            SynthesisKind::Organic,
                            SynthesisTrigger::OracleConvergence,
                        );
                        state.synthesis.fire_synthesis();
                        let synth_msg = ChatMessage {
                            role: Role::User,
                            content: MessageContent::Text(synth_text_conv.into()),
                        };
                        state.messages.push(synth_msg.clone());
                        state.context_pipeline.add_message(synth_msg.clone());
                        session.add_message(synth_msg);
                        state.synthesis.tool_decision = ToolDecisionSignal::ForcedByOracle;
                        return Ok(PhaseOutcome::NextRound);
                    }
                    return Ok(PhaseOutcome::BreakLoop);
                }
                SynthesisReason::LoopGuardInjectSynthesis
                | SynthesisReason::RoundScorerConsecutiveRegression
                | SynthesisReason::MidLoopCriticForceSynthesis => {
                    // Soft hint: inject synthesis directive, continue to next round.
                    // Suppress if convergence directive was already injected this round
                    // (ConvergenceController::Replan injects a conflicting directive).
                    //
                    // Plan-completion guard: same logic as ConvergenceControllerSynthesizeAction.
                    // LoopGuard and RoundScorer can fire synthesis based on round count or score
                    // patterns — they don't inspect the plan. If there are unattempted steps,
                    // redirect the model to execute them instead of synthesizing.
                    let unattempted_loopguard = state.execution_tracker.as_ref()
                        .map(|t| t.tracked_steps().iter()
                            .filter(|s| s.status == halcon_core::traits::TaskStatus::Pending)
                            .count())
                        .unwrap_or(0);
                    if unattempted_loopguard > 0 {
                        tracing::info!(
                            round,
                            unattempted = unattempted_loopguard,
                            ?reason,
                            "Oracle: deferring LoopGuard/RoundScorer synthesis — {unattempted_loopguard} plan step(s) still Pending"
                        );
                        if !state.silent {
                            render_sink.info(&format!(
                                "[plan-guard] deferring synthesis hint — {unattempted_loopguard} step(s) not yet attempted"
                            ));
                        }
                        let plan_hint = ChatMessage {
                            role: Role::User,
                            content: MessageContent::Text(format!(
                                "[System: Your plan has {unattempted_loopguard} step(s) that have not been executed yet. \
                                 Please execute the remaining plan steps before synthesizing a response. \
                                 Continue using the required tools.]"
                            ).into()),
                        };
                        state.messages.push(plan_hint.clone());
                        state.context_pipeline.add_message(plan_hint.clone());
                        session.add_message(plan_hint);
                        // Skip the synthesis hint injection — fall through to NextRound via
                        // the normal end-of-dispatch path.
                        return Ok(PhaseOutcome::NextRound);
                    }
                    if state.synthesis.convergence_directive_injected {
                        tracing::debug!(
                            round,
                            "Oracle InjectSynthesis suppressed: convergence directive active this round"
                        );
                    } else {
                        tracing::info!(
                            consecutive_tool_rounds = state.guards.loop_guard.consecutive_rounds(),
                            ?reason,
                            "Oracle: injecting synthesis directive"
                        );
                        if !state.silent {
                            render_sink.loop_guard_action(
                                "inject_synthesis",
                                "hinting model to synthesize",
                            );
                        }
                        // EBS-R1 (LoopGuardInjectSynthesis): enforce evidence boundary on soft hint.
                        // Gate fires when content-read tools ran but returned insufficient text.
                        let synth_text_loopguard = {
                            use super::super::evidence_pipeline::MIN_EVIDENCE_BYTES;
                            if let Some(gate_msg) = super::super::evidence_pipeline::enforce_evidence_boundary(
                                &mut state.evidence.bundle,
                            ) {
                                state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                                tracing::warn!(
                                    session_id = %state.session_id,
                                    text_bytes_extracted = state.evidence.bundle.text_bytes_extracted,
                                    min_threshold = MIN_EVIDENCE_BYTES,
                                    "EvidenceGate FIRED (LoopGuardInjectSynthesis): hint replaced with limitation report"
                                );
                                gate_msg
                            } else {
                                "[System: You have been calling tools for several state.rounds. \
                                 Consider whether you already have enough information to respond. \
                                 If so, respond directly to the user instead of calling more tools.]"
                                    .to_string()
                            }
                        };
                        let synth_msg = ChatMessage {
                            role: Role::User,
                            content: MessageContent::Text(synth_text_loopguard.into()),
                        };
                        state.messages.push(synth_msg.clone());
                        state.context_pipeline.add_message(synth_msg.clone());
                        session.add_message(synth_msg);
                    }
                }
            }
        }

        // ── Precedence 3: Replan ────────────────────────────────────────────
        TerminationDecision::Replan { reason } => {
            // P3.1: Mid-loop strategy mutation — decides HOW to replan.
            let p3_evidence_cov = state.evidence.graph.synthesis_coverage();
            let p3_plan_progress = state.execution_tracker.as_ref()
                .map(|t| { let (done, total, _) = t.progress(); if total > 0 { done as f64 / total as f64 } else { 0.0 } })
                .unwrap_or(0.0);
            let p3_sla_frac = state.sla_budget.as_ref()
                .map(|s| s.fraction_consumed())
                .unwrap_or(0.0);
            let p3_cycle_detected = state.guards.semantic_cycle_detector.has_cycle();
            let p3_max_rounds = state.sla_budget.as_ref()
                .map(|s| s.max_rounds as usize)
                .unwrap_or(20);
            let strategy_signals = super::super::domain::mid_loop_strategy::StrategySignals {
                evidence_coverage: p3_evidence_cov,
                drift_score: state.convergence.cumulative_drift_score,
                replan_attempts: state.convergence.replan_attempts,
                max_replan_attempts: state.policy.max_replan_attempts,
                consecutive_errors: tool_failures.len() as u32,
                tool_failure_clustering: if !tool_failures.is_empty() && !tool_successes.is_empty() {
                    tool_failures.len() as f32 / (tool_failures.len() + tool_successes.len()) as f32
                } else if !tool_failures.is_empty() {
                    1.0
                } else {
                    0.0
                },
                sla_fraction_consumed: p3_sla_frac,
                execution_intent: state.synthesis.execution_intent,
                plan_completion_fraction: p3_plan_progress as f32,
                cycle_detected: p3_cycle_detected,
                round,
                max_rounds: p3_max_rounds,
            };
            let strategy_thresholds = super::super::domain::mid_loop_strategy::StrategyThresholds::from_policy(&state.policy);
            let strategy_rationale = super::super::domain::mid_loop_strategy::select_mutation(
                &strategy_signals, &strategy_thresholds,
            );
            tracing::info!(
                round,
                mutation = ?strategy_rationale.mutation,
                signal = strategy_rationale.primary_signal,
                confidence = %strategy_rationale.confidence,
                "Phase3 MidLoopStrategy: selected mutation"
            );

            // Apply strategy mutation overrides
            match strategy_rationale.mutation {
                super::super::domain::mid_loop_strategy::StrategyMutation::ForceSynthesis => {
                    // Skip replan entirely — force synthesis
                    tracing::info!(round, "Phase3 Strategy: ForceSynthesis — bypassing replan");
                    // Phase 2: route through governance gate (ForceSynthesis strategy mutation).
                    state.mark_synthesis_forced_with_gate(
                        SynthesisTrigger::OracleConvergence,
                        SynthesisOrigin::OracleConvergence,
                    );
                    state.synthesis.tool_decision = ToolDecisionSignal::ForcedByOracle;
                    return Ok(PhaseOutcome::NextRound);
                }
                super::super::domain::mid_loop_strategy::StrategyMutation::ContinueCurrentPlan => {
                    // Skip replan — ambiguous signals
                    tracing::info!(round, "Phase3 Strategy: ContinueCurrentPlan — skipping replan");
                    return Ok(PhaseOutcome::NextRound);
                }
                _ => {
                    // All other mutations: proceed with existing replan logic
                    // (ReplanSameGranularity, ReplanWithDecomposition, CollapsePlan,
                    //  SwitchInvestigation/ExecutionMode)
                }
            }

            match reason {
                ReplanReason::ConvergenceControllerReplanAction => {
                    // ConvergenceController::Replan already injected the directive and set
                    // convergence_directive_injected = true in the block above.
                    // Next round will receive the injected message — nothing more to do.
                    tracing::debug!(
                        round,
                        "Oracle Replan (ConvergenceController): directive already injected"
                    );
                }
                ReplanReason::LoopGuardStagnationDetected
                | ReplanReason::RoundScorerLowTrajectory
                | ReplanReason::MidLoopCriticReplan => {
                    // Full stagnation replan: enforce budget then attempt replan.
                    state.convergence.replan_attempts += 1;
                    if state.convergence.replan_attempts > state.policy.max_replan_attempts {
                        tracing::warn!(
                            attempts = state.convergence.replan_attempts,
                            max = state.policy.max_replan_attempts,
                            "Replan budget exhausted — escalating directly to synthesis"
                        );
                        if !state.silent {
                            render_sink.warning(
                                &format!(
                                    "replan budget exhausted ({} attempts) — synthesizing response",
                                    state.convergence.replan_attempts,
                                ),
                                Some("Agent replanned repeatedly without convergence; falling back to direct response"),
                            );
                        }
                        // EBS-R1 (ReplanBudgetExhausted): enforce evidence boundary before
                        // synthesis injection. Gate fires when content-read tools ran but
                        // extracted < MIN_EVIDENCE_BYTES — prevents fabrication on unreadable files.
                        let synth_text_replan_budget = {
                            use super::super::evidence_pipeline::MIN_EVIDENCE_BYTES;
                            if let Some(gate_msg) = super::super::evidence_pipeline::enforce_evidence_boundary(
                                &mut state.evidence.bundle,
                            ) {
                                state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                                tracing::warn!(
                                    session_id = %state.session_id,
                                    text_bytes_extracted = state.evidence.bundle.text_bytes_extracted,
                                    min_threshold = MIN_EVIDENCE_BYTES,
                                    "EvidenceGate FIRED (ReplanBudgetExhausted): synthesis replaced with limitation report"
                                );
                                gate_msg
                            } else {
                                state.synthesis.synthesis_origin = Some(SynthesisOrigin::ReplanTimeout);
                                "[System: Maximum replanning attempts reached without convergence. \
                                 Synthesize all information gathered so far and respond to the user directly. \
                                 Do NOT call any more tools.]"
                                    .to_string()
                            }
                        };
                        let synth_msg = ChatMessage {
                            role: Role::User,
                            content: MessageContent::Text(synth_text_replan_budget.into()),
                        };
                        state.messages.push(synth_msg.clone());
                        state.context_pipeline.add_message(synth_msg.clone());
                        session.add_message(synth_msg);
                        // V2 fix (2026-02-27): Previously used set_force_next() which can
                        // be downgraded by subsequent heuristics. Replan budget exhaustion
                        // is an oracle-level decision — use ForcedByOracle so no subsequent
                        // heuristic (e.g. ConversationalDirectiveRule, ForceNoToolsRule) can
                        // override this synthesis injection with a weaker signal.
                        state.synthesis.tool_decision = ToolDecisionSignal::ForcedByOracle;
                        tracing::warn!(
                            attempts = state.convergence.replan_attempts,
                            "Replan budget exhausted → ForcedByOracle (oracle-level, non-downgradable)"
                        );
                        return Ok(PhaseOutcome::NextRound);
                    }

                    // Budget not exhausted — attempt stagnation replan.
                    tracing::warn!(
                        consecutive_rounds = state.guards.loop_guard.consecutive_rounds(),
                        attempt = state.convergence.replan_attempts,
                        max = state.policy.max_replan_attempts,
                        ?reason,
                        "Stagnation detected: read saturation with 0% plan progress — attempting replan"
                    );
                    if !state.silent {
                        render_sink.warning(
                            "Task appears stalled. Regenerating plan with gathered context...",
                            Some("Read tools used repeatedly without progress."),
                        );
                    }

                    // Build replan prompt with accumulated context from recent assistant messages.
                    let context_summary = {
                        let gathered_texts: Vec<String> = state.messages
                            .iter()
                            .rev()
                            .take(5)
                            .filter(|m| m.role == Role::Assistant)
                            .filter_map(|m| match &m.content {
                                MessageContent::Text(t) => Some(t.clone()),
                                MessageContent::Blocks(blocks) => {
                                    let text: String = blocks
                                        .iter()
                                        .filter_map(|b| match b {
                                            ContentBlock::Text { text } => Some(text.as_str()),
                                            _ => None,
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    if text.is_empty() { None } else { Some(text) }
                                }
                            })
                            .collect();
                        if !gathered_texts.is_empty() {
                            gathered_texts.join("\n\n")
                        } else {
                            "No prior context available.".to_string()
                        }
                    };

                    // BRECHA-C: Inject blocked tools so the planner avoids retry loops.
                    let blocked_tools_note = if state.evidence.blocked_tools.is_empty() {
                        String::new()
                    } else {
                        let tools_list = state.evidence.blocked_tools
                            .iter()
                            .map(|(name, reason)| format!("  - `{name}`: {reason}"))
                            .collect::<Vec<_>>()
                            .join("\n");
                        format!(
                            "\n\nCRITICAL CONSTRAINTS — These tools were BLOCKED by security guardrails \
                             and MUST NOT be used in the new plan:\n{tools_list}\n\
                             Generate a plan that achieves the goal WITHOUT using these blocked tools.",
                        )
                    };

                    // PHASE-3 FIX: Include tool_failures from the current round in the
                    // replan prompt so the planner understands WHY the previous strategy
                    // failed and can generate a meaningfully different plan.
                    // Previously only blocked_tools (security-denied tools) were injected;
                    // transient runtime failures were invisible to the planner.
                    let tool_failures_note = if tool_failures.is_empty() {
                        String::new()
                    } else {
                        let failures_list = tool_failures
                            .iter()
                            .map(|(name, err)| {
                                // Truncate long errors to avoid bloating the replan prompt.
                                let short_err = if err.len() > 200 {
                                    format!("{}…", &err[..200])
                                } else {
                                    err.clone()
                                };
                                format!("  - `{name}`: {short_err}")
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        format!(
                            "\n\nTOOL FAILURES this round (do NOT retry these approaches):\n{failures_list}\n\
                             Adapt the new plan to avoid these specific failure modes.",
                        )
                    };

                    let replan_prompt = format!(
                        "The current approach has stalled (read-only tools used repeatedly with no progress). \
                         Based on the information gathered so far:\n\n{context_summary}{tool_failures_note}{blocked_tools_note}\n\n\
                         Generate a NEW plan with a DIFFERENT strategy to achieve the original goal: {}\n\n\
                         Focus on actionable steps that make progress toward the goal.",
                        state.user_msg
                    );

                    let replan_result = if let Some(ref planner) = planner {
                        let plan_timeout = Duration::from_secs(planning_config.timeout_secs);
                        let tool_defs = request.tools.clone();
                        let replan_future = planner.plan(&replan_prompt, &tool_defs);
                        tokio::time::timeout(plan_timeout, replan_future).await
                    } else {
                        tracing::error!("Replan requested but no planner available");
                        if !state.silent {
                            render_sink.warning("No planner available", Some("Falling back to synthesis."));
                        }
                        // EBS-R1 (ReplanNoPlannerAvailable): enforce evidence boundary before synthesis.
                        let synth_text_no_planner = {
                            use super::super::evidence_pipeline::MIN_EVIDENCE_BYTES;
                            if let Some(gate_msg) = super::super::evidence_pipeline::enforce_evidence_boundary(
                                &mut state.evidence.bundle,
                            ) {
                                state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                                tracing::warn!(
                                    session_id = %state.session_id,
                                    text_bytes_extracted = state.evidence.bundle.text_bytes_extracted,
                                    min_threshold = MIN_EVIDENCE_BYTES,
                                    "EvidenceGate FIRED (ReplanNoPlannerAvailable): synthesis replaced with limitation report"
                                );
                                gate_msg
                            } else {
                                state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                                "[System: Cannot regenerate plan (no planner). \
                                 Synthesize your findings and respond to the user.]"
                                    .to_string()
                            }
                        };
                        let synth_msg = ChatMessage {
                            role: Role::User,
                            content: MessageContent::Text(synth_text_no_planner.into()),
                        };
                        state.messages.push(synth_msg.clone());
                        state.context_pipeline.add_message(synth_msg.clone());
                        session.add_message(synth_msg);
                        // P2-A fix (2026-02-27): No planner available is a structural oracle-level
                        // constraint — promote to ForcedByOracle (was set_force_next). The planner
                        // is either not configured or was exhausted; this is not a transient heuristic.
                        state.synthesis.tool_decision = ToolDecisionSignal::ForcedByOracle;
                        return Ok(PhaseOutcome::NextRound);
                    };

                    match replan_result {
                        Ok(Ok(Some(new_plan))) if !new_plan.steps.is_empty() => {
                            let (new_plan, _) = super::super::plan_compressor::compress(new_plan);
                            tracing::info!(
                                new_steps = new_plan.steps.len(),
                                goal = %new_plan.goal,
                                "Replan succeeded — continuing with new strategy"
                            );

                            let plan_hash = {
                                use std::collections::hash_map::DefaultHasher;
                                use std::hash::{Hash, Hasher};
                                let mut hasher = DefaultHasher::new();
                                for step in &new_plan.steps {
                                    step.description.hash(&mut hasher);
                                    step.tool_name.hash(&mut hasher);
                                }
                                hasher.finish()
                            };
                            state.guards.loop_guard.update_plan_hash(plan_hash);

                            state.active_plan = Some(new_plan.clone());
                            if let Some(ref mut tracker) = state.execution_tracker {
                                tracker.reset_plan(new_plan.clone());
                                let plan_section = format_plan_for_prompt(&new_plan, tracker.current_step());
                                if let Some(ref mut sys) = state.cached_system {
                                    update_plan_in_system(sys, &plan_section);
                                }
                                let (_, _, elapsed) = tracker.progress();
                                render_sink.plan_progress_with_timing(
                                    &new_plan.goal, &new_plan.steps,
                                    tracker.current_step(), tracker.tracked_steps(), elapsed,
                                );
                            }

                            state.guards.loop_guard.reset_on_replan();
                            state.convergence.adaptive_policy.reset_after_replan();

                            state.convergence.convergence_detector =
                                super::super::early_convergence::ConvergenceDetector::with_context_window(
                                    state.tokens.pipeline_budget as u64,
                                );
                            state.convergence.last_convergence_ratio = 0.0;
                            state.convergence.macro_plan_view = {
                                let mode = if state.silent {
                                    super::super::macro_feedback::FeedbackMode::Silent
                                } else {
                                    super::super::macro_feedback::FeedbackMode::Compact
                                };
                                let view = super::super::macro_feedback::MacroPlanView::from_plan(&new_plan, mode);
                                if !state.silent { render_sink.info(&view.format_plan_summary()); }
                                Some(view)
                            };

                            {
                                let report = state.convergence.coherence_checker.check(&new_plan);
                                state.convergence.cumulative_drift_score += report.drift_score;
                                state.convergence.drift_replan_count += 1;
                                if report.drift_detected {
                                    tracing::warn!(
                                        drift_score = report.drift_score,
                                        missing_keywords = ?report.missing_keywords,
                                        "Plan coherence drift detected after replan"
                                    );
                                    render_sink.warning("[coherence] plan drifted from original goal", None);
                                    state.messages.push(ChatMessage {
                                        role: Role::User,
                                        content: MessageContent::Text(format!(
                                            "[Goal restoration]: Your plan has drifted from the original goal.\n\
                                             Original goal: {}\n\
                                             Missing focus areas: {:?}\n\
                                             Please realign the plan with the original intent.",
                                            state.goal_text, report.missing_keywords
                                        )),
                                    });
                                }
                            }

                            if !state.silent {
                                render_sink.info(&format!("New plan generated: {} steps", new_plan.steps.len()));
                            }
                        }
                        Ok(Ok(Some(_))) | Ok(Ok(None)) => {
                            tracing::error!("Replan produced empty/no plan — falling back to synthesis");
                            if !state.silent {
                                render_sink.warning(
                                    "Plan regeneration produced empty plan",
                                    Some("Synthesizing findings from gathered information."),
                                );
                            }
                            // EBS-R1 (ReplanEmptyPlan): enforce evidence boundary before synthesis.
                            let synth_text_empty_plan = {
                                use super::super::evidence_pipeline::MIN_EVIDENCE_BYTES;
                                if let Some(gate_msg) = super::super::evidence_pipeline::enforce_evidence_boundary(
                                    &mut state.evidence.bundle,
                                ) {
                                    state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                                    tracing::warn!(
                                        session_id = %state.session_id,
                                        text_bytes_extracted = state.evidence.bundle.text_bytes_extracted,
                                        min_threshold = MIN_EVIDENCE_BYTES,
                                        "EvidenceGate FIRED (ReplanEmptyPlan): synthesis replaced with limitation report"
                                    );
                                    gate_msg
                                } else {
                                    state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                                    "[System: Plan regeneration did not succeed. \
                                     Synthesize the information you have gathered and respond to the user.]"
                                        .to_string()
                                }
                            };
                            let synth_msg = ChatMessage {
                                role: Role::User,
                                content: MessageContent::Text(synth_text_empty_plan.into()),
                            };
                            state.messages.push(synth_msg.clone());
                            state.context_pipeline.add_message(synth_msg.clone());
                            session.add_message(synth_msg);
                            // P2-B fix (2026-02-27): Replan produced empty/no plan is a structural
                            // oracle-level failure — promote to ForcedByOracle (was set_force_next).
                            // Prevents subsequent heuristics from downgrading the synthesis directive.
                            state.synthesis.tool_decision = ToolDecisionSignal::ForcedByOracle;
                        }
                        Ok(Err(e)) => {
                            tracing::error!(error = %e, "Replan failed — falling back to synthesis");
                            if !state.silent {
                                render_sink.warning(
                                    "Plan regeneration failed",
                                    Some("Synthesizing findings from gathered information."),
                                );
                            }
                            // EBS-R1 (ReplanError): enforce evidence boundary before synthesis.
                            let synth_text_replan_err = {
                                use super::super::evidence_pipeline::MIN_EVIDENCE_BYTES;
                                if let Some(gate_msg) = super::super::evidence_pipeline::enforce_evidence_boundary(
                                    &mut state.evidence.bundle,
                                ) {
                                    state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                                    tracing::warn!(
                                        session_id = %state.session_id,
                                        text_bytes_extracted = state.evidence.bundle.text_bytes_extracted,
                                        min_threshold = MIN_EVIDENCE_BYTES,
                                        "EvidenceGate FIRED (ReplanError): synthesis replaced with limitation report"
                                    );
                                    gate_msg
                                } else {
                                    state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                                    "[System: Plan regeneration failed. \
                                     Synthesize the information you have gathered and respond to the user.]"
                                        .to_string()
                                }
                            };
                            let synth_msg = ChatMessage {
                                role: Role::User,
                                content: MessageContent::Text(synth_text_replan_err.into()),
                            };
                            state.messages.push(synth_msg.clone());
                            state.context_pipeline.add_message(synth_msg.clone());
                            session.add_message(synth_msg);
                            // P2-C fix (2026-02-27): Replan error is a hard structural failure —
                            // promote to ForcedByOracle. Planner returning Err is definitive.
                            state.synthesis.tool_decision = ToolDecisionSignal::ForcedByOracle;
                        }
                        Err(_timeout) => {
                            tracing::error!(
                                timeout_secs = planning_config.timeout_secs,
                                "Replan timeout — falling back to synthesis"
                            );
                            if !state.silent {
                                render_sink.warning(
                                    "Plan regeneration timed out",
                                    Some("Synthesizing findings from gathered information."),
                                );
                            }
                            // EBS-R1 (ReplanTimeout): enforce evidence boundary before synthesis.
                            let synth_text_replan_timeout = {
                                use super::super::evidence_pipeline::MIN_EVIDENCE_BYTES;
                                if let Some(gate_msg) = super::super::evidence_pipeline::enforce_evidence_boundary(
                                    &mut state.evidence.bundle,
                                ) {
                                    state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                                    tracing::warn!(
                                        session_id = %state.session_id,
                                        text_bytes_extracted = state.evidence.bundle.text_bytes_extracted,
                                        min_threshold = MIN_EVIDENCE_BYTES,
                                        "EvidenceGate FIRED (ReplanTimeout): synthesis replaced with limitation report"
                                    );
                                    gate_msg
                                } else {
                                    state.synthesis.synthesis_origin = Some(SynthesisOrigin::ReplanTimeout);
                                    "[System: Plan regeneration timed out. \
                                     Synthesize the information you have gathered and respond to the user.]"
                                        .to_string()
                                }
                            };
                            let synth_msg = ChatMessage {
                                role: Role::User,
                                content: MessageContent::Text(synth_text_replan_timeout.into()),
                            };
                            state.messages.push(synth_msg.clone());
                            state.context_pipeline.add_message(synth_msg.clone());
                            session.add_message(synth_msg);
                            // P2-D fix (2026-02-27): Replan timeout is deterministic and structural —
                            // promote to ForcedByOracle. Timeout will recur if heuristics allow retry.
                            state.synthesis.tool_decision = ToolDecisionSignal::ForcedByOracle;
                        }
                    }
                }
            }
        }

        // ── Precedence 4: ForceNoTools ──────────────────────────────────────
        TerminationDecision::ForceNoTools => {
            tracing::warn!(
                consecutive_tool_rounds = state.guards.loop_guard.consecutive_rounds(),
                "Oracle: ForceNoTools — removing tools for next round"
            );
            if !state.silent {
                render_sink.loop_guard_action("force_no_tools", "removing tools for next round");
            }
            // EBS-R1 (ForceNoToolsOracle): enforce evidence boundary before synthesis injection.
            let synth_text_force_no_tools = {
                use super::super::evidence_pipeline::MIN_EVIDENCE_BYTES;
                if let Some(gate_msg) = super::super::evidence_pipeline::enforce_evidence_boundary(
                    &mut state.evidence.bundle,
                ) {
                    state.synthesis.synthesis_origin = Some(SynthesisOrigin::SupervisorFailure);
                    tracing::warn!(
                        session_id = %state.session_id,
                        text_bytes_extracted = state.evidence.bundle.text_bytes_extracted,
                        min_threshold = MIN_EVIDENCE_BYTES,
                        "EvidenceGate FIRED (ForceNoToolsOracle): synthesis replaced with limitation report"
                    );
                    gate_msg
                } else {
                    "[System: You have gathered sufficient information across multiple tool state.rounds. \
                     SYNTHESIZE your findings and respond directly to the user. \
                     Do NOT call any more tools unless absolutely necessary for NEW information.]"
                        .to_string()
                }
            };
            let synth_msg = ChatMessage {
                role: Role::User,
                content: MessageContent::Text(synth_text_force_no_tools.into()),
            };
            state.messages.push(synth_msg.clone());
            state.context_pipeline.add_message(synth_msg.clone());
            session.add_message(synth_msg);
            // Oracle ForceNoTools is highest-authority — use ForcedByOracle so
            // subsequent heuristic set_force_next() calls cannot downgrade it.
            state.synthesis.tool_decision = ToolDecisionSignal::ForcedByOracle;
            // FASE 6: TUI synthesis phase visibility.
            if !state.silent {
                render_sink.phase_started("synthesis", "Forced no-tools — synthesising...");
            }
        }

        // ── Precedence 5: Continue ──────────────────────────────────────────
        TerminationDecision::Continue => {}
    }

    // ExecutionIntent transition: Execution → Complete when all plan steps finish.
    // This unblocks synthesis guards for the final synthesis round.
    //
    // V1 fix (2026-02-27): Previously used `tracker.plan().steps.iter().all(|s| s.outcome.is_some())`.
    // That condition stalled when steps were deduped, skipped, or had no outcome record even
    // though they were terminal (e.g. depended on a failed step). Now uses
    // `tracker.is_complete()` which checks `all(status.is_terminal())` — correctly covers
    // Completed + Failed + Skipped states, preventing the Execution→Complete transition
    // from stalling and synthesis from being permanently suppressed.
    if state.synthesis.execution_intent == ExecutionIntentPhase::Execution {
        if let Some(ref tracker) = state.execution_tracker {
            let all_done = tracker.is_complete();
            if all_done {
                state.synthesis.execution_intent = ExecutionIntentPhase::Complete;
                tracing::info!(
                    steps_total = tracker.tracked_steps().len(),
                    "ExecutionIntent: Execution → Complete (tracker.is_complete())"
                );
            } else {
                // Structured audit: log non-terminal step count for observability
                let pending = tracker.tracked_steps().iter().filter(|s| !s.status.is_terminal()).count();
                tracing::debug!(
                    pending_steps = pending,
                    total_steps = tracker.tracked_steps().len(),
                    "ExecutionIntent: still Execution ({} steps pending)",
                    pending
                );
            }
        }
    }

    // Self-correction context injection: when tools fail, inject a structured
    // hint to help the model recover (SOTA pattern from Windsurf/Cursor).
    // RC-2 fix: inject a STRONGER directive when the circuit breaker has tripped.
    if !tool_failures.is_empty() {
        let failure_details: Vec<String> = tool_failures
            .iter()
            .map(|(name, err)| format!("- {name}: {err}"))
            .collect();

        let tripped_tools = state.guards.failure_tracker.tripped_tools();
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
        state.messages.push(correction_msg.clone());
        state.context_pipeline.add_message(correction_msg.clone());
        session.add_message(correction_msg);
    }

    // Clear speculation cache at round boundary (predictions are per-round).
    speculator.clear().await;

    // REMEDIATION FIX D — Mid-session reflection consolidation.
    // Without this, reflections accumulate indefinitely during long sessions and are
    // only consolidated after the loop exits (in mod.rs). This causes:
    //   1. Redundant reflections consuming episodic memory slots
    //   2. Slow consolidation at session end instead of incremental cleanup
    //   3. Similar failure patterns not recognized across rounds
    // Fire consolidation every 5 rounds if we have DB access. Fire-and-forget
    // (tokio::spawn) to avoid blocking the agent loop.
    if state.rounds % 5 == 0 && state.rounds > 0 {
        if let Some(db) = trace_db {
            let db_clone = db.clone();
            tokio::spawn(async move {
                match super::super::memory_consolidator::maybe_consolidate(&db_clone).await {
                    Some(result) if result.merged > 0 || result.pruned > 0 => {
                        tracing::info!(
                            merged = result.merged,
                            pruned = result.pruned,
                            remaining = result.remaining,
                            "Mid-session reflection consolidation complete"
                        );
                    }
                    _ => {}
                }
            });
        }
    }

    // Auto-save session + checkpoint after each tool-use round (crash protection).
    if let Some(db) = trace_db {
        if let Err(e) = db.save_session(session).await {
            tracing::warn!("Auto-save session failed: {e}");
        }
    }
    super::super::agent_utils::auto_checkpoint(
        trace_db,
        state.session_id,
        state.rounds,
        &state.messages,
        session,
        state.trace_step_index,
    );

    // FSM: if synthesis was injected this phase, transition to Synthesizing.
    if state.synthesis.is_synthesis_forced() {
        state.synthesis.advance_phase(super::loop_state::AgentEvent::SynthesisStarted);
    }

    Ok(PhaseOutcome::Continue)
}

// ── Phase 6: Mini-Critic ────────────────────────────────────────────────────

/// Action the mini-critic can recommend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MiniCriticAction {
    /// Force structural replanning — session is stalled.
    ForceReplan,
    /// Reduce tool scope (set force_next) — declining trajectory.
    ReduceTools,
    /// Force synthesis — budget nearly exhausted with insufficient progress.
    ForceSynthesis,
}

/// Lightweight heuristic progress check — no LLM call.
///
/// Runs every `policy.mini_critic_interval` rounds (default 3). Uses 3 signals:
/// - Plan completion ratio vs budget consumed (SLA or round-based)
/// - Round scorer trend (rising vs declining)
/// - Recent round scores (last 3)
///
/// Returns `None` when no intervention is needed (the common case).
fn mini_critic_check(state: &LoopState, round: usize) -> Option<MiniCriticAction> {
    let interval = state.policy.mini_critic_interval;
    if interval == 0 || round < interval || round % interval != 0 {
        return None;
    }

    // Compute budget fraction consumed (round-based).
    // Use SLA max_rounds when available, else fall back to a conservative default.
    let max_rounds = state.sla_budget.as_ref()
        .map(|b| b.max_rounds as f64)
        .unwrap_or(10.0)
        .max(1.0);
    let budget_fraction = round as f64 / max_rounds;

    // Compute progress fraction (plan completion ratio).
    let progress = if let Some(ref tracker) = state.execution_tracker {
        let (completed, total, _) = tracker.progress();
        if total > 0 { completed as f64 / total as f64 } else { 0.0 }
    } else {
        0.0
    };

    // Check round score trend: average of last 3 evaluations.
    let recent_scores: Vec<f32> = state.convergence.round_evaluations.iter()
        .rev()
        .take(3)
        .map(|e| e.combined_score)
        .collect();
    let avg_recent = if recent_scores.is_empty() {
        0.5 // neutral
    } else {
        recent_scores.iter().sum::<f32>() / recent_scores.len() as f32
    };
    let declining = recent_scores.len() >= 2
        && recent_scores[0] < recent_scores[recent_scores.len() - 1]; // latest < oldest

    // Decision thresholds (Phase 6 spec):
    // >80% budget + <80% complete → ForceSynthesis
    if budget_fraction > 0.80 && progress < 0.80 {
        return Some(MiniCriticAction::ForceSynthesis);
    }

    // >50% budget + <30% progress + low avg score → ForceReplan
    let budget_threshold = state.policy.mini_critic_budget_fraction;
    if budget_fraction > budget_threshold && progress < 0.30 && avg_recent < 0.40 {
        return Some(MiniCriticAction::ForceReplan);
    }

    // >50% budget + declining trend → ReduceTools
    if budget_fraction > budget_threshold && declining {
        return Some(MiniCriticAction::ReduceTools);
    }

    None
}
