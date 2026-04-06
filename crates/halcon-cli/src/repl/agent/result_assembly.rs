//! Post-loop result assembly: LoopCritic evaluation + `AgentLoopResult` construction.
//!
//! Called once after `'agent_loop` exits normally (non-early-return). Handles:
//! - TBAC context pop
//! - LoopCritic adversarial evaluation (G2 critic separation)
//! - Stop-condition classification
//! - FSM / domain-event emission
//! - L4 archive flush
//! - Final `AgentLoopResult` construction

use std::sync::Arc;

use anyhow::Result;
use halcon_core::traits::ModelProvider;
use halcon_core::types::{
    AgentLimits, CompletionTrace, ConvergenceDecision, DomainEvent, EventPayload, MessageContent,
    ModelRequest, Role, TerminationSource, TracedCriticVerdict,
};
use halcon_core::EventSender;

use super::super::agent_types::{
    AgentLoopResult, ControlReceiver, CriticVerdictSummary, StopCondition,
};
use super::super::agent_utils::compute_fingerprint;
use super::super::conversational_permission::ConversationalPermissionHandler;
use super::super::evidence_pipeline::detect_operational_claim;
use super::super::plugins::PluginRegistry;
use super::loop_state::LoopState;
use crate::render::sink::RenderSink;

/// Assemble the final `AgentLoopResult` after the agent loop exits.
///
/// Consumes `state` (moves owned fields into the result) and takes the remaining
/// external dependencies by reference/value.
pub(super) async fn build(
    mut state: LoopState,
    render_sink: &dyn RenderSink,
    event_tx: &EventSender,
    limits: &AgentLimits,
    provider: &Arc<dyn ModelProvider>,
    critic_provider: Option<Arc<dyn ModelProvider>>,
    critic_model: Option<String>,
    request: &ModelRequest,
    ctrl_rx: Option<ControlReceiver>,
    plugin_registry: Option<Arc<std::sync::Mutex<PluginRegistry>>>,
    permissions: &mut ConversationalPermissionHandler,
) -> Result<AgentLoopResult> {
    // FSM: entering evaluation phase (1C: advance_phase replaces direct assignment).
    state
        .synthesis
        .advance_phase(super::loop_state::AgentEvent::SynthesisComplete);

    // TBAC: pop the plan-derived context if we pushed one.
    if state.tbac_pushed {
        permissions.pop_context();
    }

    // Phase 1 Supervisor: LoopCritic — adversarial post-loop evaluation.
    // Runs only for tasks that completed ≥1 plan step (avoids 20s overhead on simple chat).
    // Phase 1.2: Verdict now includes gaps + retry_instruction so mod.rs can perform
    // an actual in-session retry instead of just logging an advisory message.
    let mut critic_verdict_holder: Option<CriticVerdictSummary> = None;
    // FASE 4: tracks whether the critic was expected to run but failed.
    let mut critic_unavailable = false;
    // progress() returns (completed_steps, total_steps, elapsed). Check total > 0 so the
    // LoopCritic runs even when 0 steps completed (e.g. all steps failed or were skipped) —
    // failed plans still warrant adversarial evaluation.
    let has_plan_execution = state
        .execution_tracker
        .as_ref()
        .map(|t| t.progress().1 > 0)
        .unwrap_or(false);
    // V4 fix (2026-02-27): Previously, LoopCritic was skipped entirely when there was no
    // formal ExecutionPlan (e.g. investigation tasks where the coordinator ran tools without
    // planning, or planning failed). This caused critic_verdict=None for all tool-only
    // sessions, making it impossible to distinguish "not evaluated" from "not achieved" and
    // silently allowing synthetic fabricated responses to pass without adversarial scrutiny.
    //
    // Fix: also run LoopCritic when:
    //   - tools_executed is non-empty (coordinator ran real tools even without a plan), OR
    //   - forced_synthesis_detected (synthesis was injected — oracle decided goal was reached)
    //
    // Guard: still skip for purely conversational turns (no plan AND no tools AND no forced
    // synthesis) to avoid the 20s critic overhead on simple greetings/questions.
    let has_tool_work = !state.tools_executed.is_empty();
    let has_forced_synthesis = state.synthesis.is_synthesis_forced();
    let should_run_critic = (has_plan_execution || has_tool_work || has_forced_synthesis)
        && !state.full_text.is_empty();
    if should_run_critic {
        tracing::debug!(
            has_plan_execution,
            has_tool_work,
            has_forced_synthesis,
            tools_count = state.tools_executed.len(),
            "LoopCritic: running adversarial evaluation (V4 extended conditions)"
        );
    } else {
        tracing::debug!(
            "LoopCritic: skipping (conversational turn — no plan, no tools, no forced synthesis)"
        );
    }
    if should_run_critic {
        let original_request = state
            .messages
            .iter()
            .rev() // Use LAST user message (current task), not first (may be greeting)
            .find(|m| m.role == Role::User)
            .map(|m| match &m.content {
                MessageContent::Text(t) => t.as_str(),
                _ => "",
            })
            .unwrap_or("")
            .to_string();

        if !original_request.is_empty() {
            let step_summaries: Vec<String> = state
                .execution_tracker
                .as_ref()
                .map(|t| {
                    t.tracked_steps()
                        .iter()
                        .map(|s| s.step.description.clone())
                        .collect()
                })
                .unwrap_or_default();

            // Step 8h: Use critic_provider/critic_model if configured (G2 — critic separation).
            // Falls back to executor provider/model when not configured (backward compatible).
            // Fallback logic: if configured critic_provider fails (returns None), retry with
            // the session provider so cross-provider sessions (deepseek/openai) still get
            // adversarial evaluation even when critic_provider="anthropic" has no credits.
            let critic_prov_ref = critic_provider.as_ref().unwrap_or(provider);
            let critic_mdl_str = critic_model.as_deref().unwrap_or(&request.model);
            let critic = super::super::supervisor::LoopCritic::new(
                critic_prov_ref.clone(),
                critic_mdl_str.to_string(),
            );

            // PolicyConfig-driven timeouts: per-call and overall envelope.
            let critic_timeout_secs = state.policy.critic_timeout_secs;
            let critic_excerpt_len = state.policy.excerpt_len;
            // Per-call timeout is half the overall envelope (leaves room for fallback + backoff).
            let per_call_timeout = (critic_timeout_secs / 2).max(10);
            let critic_future = async {
                let mut verdict_opt = critic
                    .evaluate(
                        &original_request,
                        &state.full_text,
                        &step_summaries,
                        critic_excerpt_len,
                        per_call_timeout,
                    )
                    .await;

                // Fallback: if critic_provider was explicitly set (G2 separation) but failed,
                // retry with the session provider so non-anthropic sessions aren't left unverified.
                // FASE 4: 2s backoff before fallback to avoid hammering the provider.
                if verdict_opt.is_none() && critic_provider.is_some() {
                    tracing::info!(
                        session_provider = %provider.name(),
                        "LoopCritic: configured critic_provider failed — backoff 2s then retrying with session provider"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                    let fallback_critic = super::super::supervisor::LoopCritic::new(
                        provider.clone(),
                        request.model.clone(),
                    );
                    verdict_opt = fallback_critic
                        .evaluate(
                            &original_request,
                            &state.full_text,
                            &step_summaries,
                            critic_excerpt_len,
                            per_call_timeout,
                        )
                        .await;
                }
                verdict_opt
            };

            let verdict_opt = match tokio::time::timeout(
                std::time::Duration::from_secs(critic_timeout_secs),
                critic_future,
            )
            .await
            {
                Ok(v) => v,
                Err(_) => {
                    tracing::error!(
                        timeout_secs = critic_timeout_secs,
                        "LoopCritic: overall timeout exceeded — treating as unavailable"
                    );
                    None
                }
            };

            match verdict_opt {
                Some(verdict) => {
                    // Phase 1.2: Propagate FULL verdict (achieved + confidence + gaps + retry_instruction).
                    // Previously only (achieved, confidence) was stored — gaps and retry_instruction
                    // were LOST here. This was the root cause of advisory-only retry behavior.
                    critic_verdict_holder = Some(CriticVerdictSummary {
                        achieved: verdict.achieved,
                        confidence: verdict.confidence,
                        gaps: verdict.gaps.clone(),
                        retry_instruction: verdict.retry_instruction.clone(),
                    });
                    if verdict.achieved {
                        tracing::debug!(
                            confidence = verdict.confidence,
                            "LoopCritic: goal achieved"
                        );
                    } else {
                        tracing::warn!(
                            confidence = verdict.confidence,
                            gaps = ?verdict.gaps,
                            retry_has_instruction = verdict.retry_instruction.is_some(),
                            "LoopCritic: goal NOT achieved"
                        );
                        if !state.silent {
                            render_sink.warning(
                                &format!(
                                    "[critic] goal not fully achieved ({:.0}% confidence): {}",
                                    verdict.confidence * 100.0,
                                    verdict.gaps.join("; ")
                                ),
                                None,
                            );
                        }
                    }
                }
                None => {
                    // Phase 1 fix: LoopCritic returned None (provider failure, timeout, or
                    // empty response). Previously this was a silent no-op — the verdict was
                    // simply skipped and `critic_verdict_holder` stayed None. This made it
                    // impossible to distinguish "not evaluated" from "not achieved" in telemetry.
                    // FASE 4: set critic_unavailable flag for reward penalty.
                    critic_unavailable = true;
                    tracing::warn!(
                        session_id = %state.session_id,
                        "LoopCritic: evaluate() returned None — provider failure or timeout; \
                         goal completion status is UNKNOWN for this session (critic_unavailable=true)"
                    );
                    if !state.silent {
                        render_sink.warning(
                            "[critic] evaluation unavailable — goal completion unverified",
                            Some("LoopCritic provider did not respond; result may be incomplete"),
                        );
                    }
                }
            }
        }
    }

    // EBS-B2 invariant assertion (debug builds only).
    // If the deterministic boundary was enforced, synthesis_blocked must also be set.
    // This catches any path that sets deterministic_boundary_enforced without setting
    // synthesis_blocked (which would indicate a broken gate implementation).
    #[cfg(debug_assertions)]
    if state.evidence.deterministic_boundary_enforced {
        debug_assert!(
            state.evidence.bundle.synthesis_blocked,
            "INVARIANT: deterministic_boundary_enforced=true implies synthesis_blocked=true \
             (EBS-B2 gate must set both flags atomically)"
        );
        debug_assert!(
            matches!(state.synthesis.synthesis_origin, Some(super::loop_state::SynthesisOrigin::SupervisorFailure)),
            "INVARIANT: EBS-B2 gate must set synthesis_origin=SupervisorFailure for reward dampening"
        );
    }

    // Determine stop condition: max_rounds, forced synthesis, or normal end.
    // If the loop guard forced a break (oscillation/plan completion) or forced no-tools,
    // and the loop ended due to that, use ForcedSynthesis.
    let stop_condition = if state.environment_error_halt {
        // P0-C: MCP environment persistently dead — report EnvironmentError so UCB1 gets
        // a zero-reward signal for the strategy that dispatched these tools.
        StopCondition::EnvironmentError
    } else if state.ctrl_cancelled {
        StopCondition::Interrupted
    } else if state.rounds >= limits.max_rounds {
        tracing::warn!(max_rounds = limits.max_rounds, "Max agent rounds reached");
        if !state.silent {
            render_sink.warning(
                &format!("max rounds reached: {}", limits.max_rounds),
                Some("Increase max_rounds in config to allow more iterations"),
            );
        }
        StopCondition::MaxRounds
    } else if state.synthesis.is_synthesis_forced()
        || state.guards.loop_guard.plan_complete()
        || state.guards.loop_guard.detect_oscillation()
    {
        StopCondition::ForcedSynthesis
    } else {
        StopCondition::EndTurn
    };

    // ── PHASE-1 INSTRUMENTATION ────────────────────────────────────────────
    // Build and log a CompletionTrace for every agent turn. This is purely
    // observability — it does not affect any return path or caller behavior.
    {
        let termination_source = match stop_condition {
            StopCondition::EndTurn => TerminationSource::ModelEndTurn,
            StopCondition::ForcedSynthesis => {
                if state.guards.loop_guard.plan_complete() {
                    TerminationSource::PlanComplete
                } else {
                    TerminationSource::ConvergenceForced
                }
            }
            StopCondition::MaxRounds => TerminationSource::MaxRounds,
            StopCondition::TokenBudget
            | StopCondition::DurationBudget
            | StopCondition::CostBudget => TerminationSource::Budget,
            StopCondition::Interrupted => TerminationSource::UserInterrupt,
            StopCondition::ProviderError => TerminationSource::ProviderError,
            StopCondition::EnvironmentError => TerminationSource::EnvironmentError,
            StopCondition::SupervisorDenied => TerminationSource::SupervisorDenied,
        };
        let convergence_decision = if state.rounds >= limits.max_rounds {
            Some(ConvergenceDecision::MaxRoundsExhausted)
        } else if state.guards.loop_guard.detect_oscillation() {
            Some(ConvergenceDecision::Stagnated {
                consecutive_rounds: state.guards.loop_guard.consecutive_rounds() as u32,
            })
        } else if state.guards.loop_guard.plan_complete() {
            Some(ConvergenceDecision::NaturalEnd)
        } else if state.synthesis.is_synthesis_forced() {
            Some(ConvergenceDecision::OracleForcedSynthesis)
        } else {
            None
        };
        let traced_critic = critic_verdict_holder.as_ref().map(|v| TracedCriticVerdict {
            achieved: v.achieved,
            confidence: v.confidence,
            gap_count: v.gaps.len(),
        });
        let plan_ratio = state
            .execution_tracker
            .as_ref()
            .map(|t| {
                let (completed, total, _) = t.progress();
                if total > 0 {
                    (completed as f32 / total as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            })
            .unwrap_or(0.0);
        let semantic_success = matches!(
            stop_condition,
            StopCondition::EndTurn | StopCondition::ForcedSynthesis
        );
        let trace = CompletionTrace::new(
            state.rounds as u32,
            termination_source,
            convergence_decision,
            traced_critic,
            plan_ratio,
            state.tools_executed.len(),
            0, // tool_failure_count not available here; tracked in post_batch
            semantic_success,
        );
        trace.log();
    }
    // ── END PHASE-1 INSTRUMENTATION ───────────────────────────────────────

    // Phase E5: Emit final agent state transition (Complete or Failed).
    // P4 FIX: Use state.synthesis.phase.as_str() (typed FSM) as from_state instead of
    // hardcoded "executing". The loop may have exited from "reflecting", "planning",
    // or "tool_wait" — emitting the wrong from_state caused "[state] INVALID" TUI warnings.
    if !state.silent {
        // NOTE: ProviderError, TokenBudget, DurationBudget, and CostBudget all use early
        // `return Ok(...)` paths inside the loop and therefore never reach this point.
        // They are listed explicitly so the compiler enforces exhaustiveness — any future
        // StopCondition variant that is added will cause a compile error here, preventing
        // it from silently falling through to a wrong FSM state.
        let (to_state, reason) = match stop_condition {
            StopCondition::EndTurn | StopCondition::ForcedSynthesis => {
                ("complete", "task finished")
            }
            StopCondition::Interrupted => ("idle", "user cancelled"),
            StopCondition::MaxRounds => ("failed", "max rounds reached"),
            StopCondition::EnvironmentError => ("failed", "environment unavailable"),
            // The following variants exit via early return and never reach this match,
            // but are listed to keep the match exhaustive and prevent future regressions.
            StopCondition::ProviderError => ("failed", "provider error"),
            StopCondition::TokenBudget => ("failed", "token budget exceeded"),
            StopCondition::DurationBudget => ("failed", "duration budget exceeded"),
            StopCondition::CostBudget => ("failed", "cost budget exceeded"),
            StopCondition::SupervisorDenied => ("complete", "supervisor denied write"),
        };
        // FASE 6: Ensure any open synthesis phase is closed before FSM transition.
        render_sink.phase_ended();
        render_sink.agent_state_transition(state.synthesis.phase_str(), to_state, reason);
    }

    // Emit AgentCompleted event.
    halcon_core::emit_event(
        event_tx,
        DomainEvent::new(EventPayload::AgentCompleted {
            agent_type: halcon_core::types::AgentType::Chat,
            result: halcon_core::types::AgentResult {
                success: matches!(
                    stop_condition,
                    StopCondition::EndTurn | StopCondition::ForcedSynthesis
                ),
                summary: format!("{} rounds, {:?}", state.rounds, stop_condition),
                files_modified: vec![],
                tools_used: vec![],
            },
        }),
    );

    // ── PHASE-2 COMPLETION VALIDATOR (advisory only, feature-gated) ──────────
    // When feature = "completion-validator" is enabled, run a keyword-based
    // semantic check and log the verdict. This does NOT alter stop_condition
    // or any return value — it is purely observability for Phase 2.
    // Phase 3+ may use the verdict to trigger repair or clarification.
    #[cfg(feature = "completion-validator")]
    {
        use halcon_core::traits::{
            CompletionEvidence, CompletionValidator, KeywordCompletionValidator,
        };
        // Extract goal text from the last user message.
        let goal_text = state
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| match &m.content {
                MessageContent::Text(t) => t.as_str(),
                _ => "",
            })
            .unwrap_or("");

        if !goal_text.is_empty() && !state.full_text.is_empty() {
            let validator = KeywordCompletionValidator::from_goal_text(goal_text, 0.6);
            let evidence = CompletionEvidence {
                goal_text,
                tool_successes: &state.tools_executed,
                tool_failures: &[],
                final_text: &state.full_text,
                round: state.rounds as u32,
                plan_steps_completed: state
                    .execution_tracker
                    .as_ref()
                    .map(|t| t.progress().0)
                    .unwrap_or(0),
                plan_steps_total: state
                    .execution_tracker
                    .as_ref()
                    .map(|t| t.progress().1)
                    .unwrap_or(0),
            };
            let verdict = validator.validate(&evidence).await;
            tracing::debug!(
                validator = validator.name(),
                verdict_coverage = verdict.coverage(),
                verdict_success = verdict.is_success(),
                stop_condition = ?stop_condition,
                "completion_validator result (advisory)"
            );
        }
    }
    // ── END PHASE-2 COMPLETION VALIDATOR ──────────────────────────────────

    // Flush L4 archive to disk (persist cross-session knowledge).
    if let Some(parent) = state.l4_archive_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Some(bytes) = state.context_pipeline.flush_l4_archive() {
        tracing::debug!(bytes, "L4 archive flushed to disk");
    }

    // Log plan execution summary with timing.
    if let Some(ref tracker) = state.execution_tracker {
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
        if !state.silent {
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

    let execution_fingerprint = compute_fingerprint(&state.messages);
    let plan_completion_ratio = state
        .execution_tracker
        .as_ref()
        .map(|t| {
            let (completed, total, _) = t.progress();
            if total > 0 {
                (completed as f32 / total as f32).clamp(0.0, 1.0)
            } else {
                0.0
            }
        })
        .unwrap_or(0.0);

    // Phase 7: Wire plan_coherence_score (avg drift from PlanCoherenceChecker) and
    // oscillation_penalty (fraction of force_threshold from ToolLoopGuard) into
    // AgentLoopResult so mod.rs can pass them to reward_pipeline::RawRewardSignals.
    let avg_plan_drift = if state.convergence.drift_replan_count > 0 {
        (state.convergence.cumulative_drift_score / state.convergence.drift_replan_count as f32)
            .clamp(0.0, 1.0)
    } else {
        0.0 // No replanning occurred — coherence is undefined (treated as perfect in reward_pipeline)
    };
    // Oscillation intensity: consecutive tool rounds as fraction of the force threshold (8 rounds).
    // 0.0 = no sustained tool looping; 1.0 = at/above the hard-limit threshold.
    let oscillation_penalty =
        (state.guards.loop_guard.consecutive_rounds() as f32 / 8.0).clamp(0.0, 1.0);

    // Phase 2 reward unification: `record_outcome()` has been MOVED to mod.rs so it uses
    // the reward_pipeline's continuous 5-signal reward instead of the coarse 4-value
    // stop-condition mapping that was here. `last_model_used` carries the model ID.
    // mod.rs calls record_outcome() after compute_reward() when reasoning engine is active,
    // or falls back to the coarse formula when the engine is disabled.

    // Step 7 (Phase 7 plugin): collect per-plugin cost snapshots for UCB1 reward blending.
    // When plugin_registry is None (all existing tests, non-plugin sessions), snapshot is empty.
    let plugin_cost_snapshot = plugin_registry
        .as_ref()
        .and_then(|arc_pr| arc_pr.lock().ok().map(|pr| pr.cost_snapshot()))
        .unwrap_or_default();

    // BRECHA-S1: populate evidence metadata from loop state.
    let evidence_verified = !state.evidence.bundle.evidence_gate_fires();
    let content_read_attempts = state.evidence.bundle.content_read_attempts;

    // BRECHA-S2: provider name for cost attribution.
    let last_provider_used = Some(provider.name().to_string());

    // Phase 3 EvidenceGraph: heuristically mark Good nodes as referenced if the
    // full_text contains fragments from tool output. This closes the write-only gap
    // by giving synthesis_coverage() a realistic baseline before reward computation.
    {
        let text = &state.full_text;
        let good_ids: Vec<_> = state
            .evidence
            .graph
            .unreferenced_evidence()
            .iter()
            .filter(|node| {
                // Heuristic: if the tool_args_summary (e.g. file path) appears in
                // the synthesis text, assume the evidence was incorporated.
                !node.tool_args_summary.is_empty() && text.contains(&node.tool_args_summary)
            })
            .map(|node| node.id)
            .collect();
        if !good_ids.is_empty() {
            state.evidence.graph.mark_referenced_batch(&good_ids);
        }
    }

    // Phase A: capture trust signals before state fields are moved.
    let tools_executed_count = state.tools_executed.len();
    let tools_suppressed_last_round = state.tools_suppressed_last_round;
    let last_tool_execution_round = state.last_tool_execution_round;
    let current_rounds = state.rounds;

    let mut result = AgentLoopResult {
        full_text: state.full_text,
        rounds: state.rounds,
        stop_condition,
        input_tokens: state.tokens.call_input_tokens,
        output_tokens: state.tokens.call_output_tokens,
        cost_usd: state.tokens.call_cost,
        latency_ms: state.loop_start.elapsed().as_millis() as u64,
        execution_fingerprint,
        timeline_json: state
            .execution_tracker
            .as_ref()
            .map(|t| t.to_json().to_string()),
        ctrl_rx,
        critic_verdict: critic_verdict_holder,
        round_evaluations: state.convergence.round_evaluations,
        plan_completion_ratio,
        avg_plan_drift,
        oscillation_penalty,
        last_model_used: Some(state.last_round_model_name),
        plugin_cost_snapshot,
        tools_executed: state.tools_executed,
        evidence_verified,
        content_read_attempts,
        last_provider_used,
        // BRECHA-S3: propagate blocked tools for cross-turn session persistence.
        blocked_tools: state.evidence.blocked_tools,
        // BRECHA-R1: propagate failed sub-agent steps for retry planner awareness.
        failed_sub_agent_steps: state.failed_sub_agent_steps,
        // FASE 4: propagate critic unavailability for reward penalty.
        critic_unavailable,
        // F4 RetryMutation: propagate tool failure records for structured retry.
        tool_trust_failures: state.tool_trust.failure_records(),
        // Phase 2 SLA: propagate budget so mod.rs can gate retries via allows_retry().
        sla_budget: state.sla_budget,
        // Phase 3 EvidenceGraph: propagate synthesis coverage for reward signal.
        evidence_coverage: state.evidence.graph.synthesis_coverage(),
        // Phase 2 Synthesis Governance: propagate gate classification for reward pipeline.
        synthesis_kind: state.synthesis.last_synthesis_kind,
        synthesis_trigger: state.synthesis.last_synthesis_trigger,
        // GAP-4: propagate routing escalation count for post-session surfacing.
        routing_escalation_count: state.convergence.routing_escalation_count,
        // Phase A: compute response trust from provenance signals.
        response_trust: halcon_core::types::ResponseTrust::compute(
            tools_executed_count,
            tools_suppressed_last_round,
            last_tool_execution_round,
            current_rounds,
            None,
        ),
        decision_log: state.decision_log.clone(),
    };

    // BRECHA-A: DirectExecution hallucination guard.
    // If no tools were used and the text makes operational claims, append unverified notice.
    if !state.is_conversational_intent
        && result.tools_executed.is_empty()
        && content_read_attempts == 0
        && state.active_plan.is_none()
        && !result.full_text.is_empty()
        && detect_operational_claim(&result.full_text)
    {
        let notice = "\n\n---\n\u{26a0}\u{fe0f} [Unverified: this response was generated without \
                      reading any files or executing tools. Claims about code, \
                      file contents, or line counts may be inaccurate.]";
        result.full_text.push_str(notice);
    }

    // FSM: evaluation complete → Completed.
    // Note: `state` fields were moved above, so we don't update state.synthesis.phase here.
    // The phase transition is documented but not persisted since state is consumed.

    Ok(result)
}
