//! Stage 5: Post-Processing — critic retry, quality recording, plugin metrics, playbook learning, memory.
//!
//! # Xiyo Comparison
//!
//! Xiyo's post-processing is minimal: `accumulateUsage()` and `flushSessionStorage()`
//! (QueryEngine.ts:812, 450). Halcon provides:
//! - **LoopCritic retry**: Re-runs agent loop when critic detects incomplete task
//! - **UCB1 quality recording**: Cross-session model performance learning
//! - **Plugin UCB1 metrics**: Per-plugin success rate tracking
//! - **Playbook auto-learning**: Saves successful plans as reusable YAML
//! - **Memory consolidation**: Merges and prunes reflections
//! - **Runtime telemetry**: Fire-and-forget signal ingestion
//!
//! # Side Effects
//!
//! - May invoke a second agent loop (critic retry)
//! - Fire-and-forget DB writes (quality, plugins, reasoning)
//! - Fire-and-forget telemetry ingestion
//! - Memory consolidation with 30s timeout

use std::sync::{Arc, Mutex};

use halcon_core::types::AppConfig;
use halcon_storage::AsyncDatabase;

use crate::render::sink::RenderSink;
use crate::repl::agent;
use crate::repl::agent_types;
use crate::repl::cache_state::ReplCacheState;
use crate::repl::context::consolidator as memory_consolidator;
use crate::repl::feature_bundle::ReplFeatures;
use crate::repl::model_selector;
use crate::repl::plugins;
use crate::repl::runtime_signal_ingestor;

/// Parameters for the critic retry decision.
///
/// Extracted from the inline critic retry block (~217 LOC) in `handle_message_with_sink`.
/// The pure logic (text building, mutation computation) lives here;
/// the AgentContext construction remains in mod.rs due to Rust borrow constraints.
pub struct CriticRetryDecision {
    /// Whether a retry is warranted.
    pub should_retry: bool,
    /// Confidence of the critic verdict (0.0-1.0).
    pub confidence: f32,
    /// Gaps identified by the critic.
    pub gaps: Vec<String>,
    /// Optional retry instruction from the critic.
    pub retry_instruction: Option<String>,
}

/// Output of the Post-Processing stage.
pub struct PostProcessOutput {
    /// Whether a critic retry was performed.
    pub retry_performed: bool,
    /// Whether quality stats were recorded to ModelSelector.
    pub quality_recorded: bool,
    /// Whether memory consolidation completed.
    pub memory_consolidated: bool,
}

/// Stage 5: Post-Processing.
pub struct PostProcessStage;

impl PostProcessStage {
    /// Record model quality via ModelSelector (unified reward from pipeline or coarse fallback).
    pub fn record_model_quality(
        selector: &Option<model_selector::ModelSelector>,
        result: &agent::AgentLoopResult,
        captured_pipeline_reward: Option<(f64, bool)>,
        cache: &mut ReplCacheState,
        config: &AppConfig,
        async_db: &Option<AsyncDatabase>,
        provider_name: &str,
        sink: &dyn RenderSink,
    ) {
        let sel = match selector {
            Some(s) => s,
            None => return,
        };
        let model_id = match result.last_model_used.as_deref() {
            Some(id) => id,
            None => return,
        };

        let (reward, success) = if let Some((pr, ps)) = captured_pipeline_reward {
            (pr, ps)
        } else {
            let coarse_success = matches!(
                result.stop_condition,
                agent_types::StopCondition::EndTurn | agent_types::StopCondition::ForcedSynthesis
            );
            let coarse_reward = match result.stop_condition {
                agent_types::StopCondition::EndTurn => 0.85,
                agent_types::StopCondition::ForcedSynthesis => 0.65,
                agent_types::StopCondition::MaxRounds => 0.40,
                agent_types::StopCondition::TokenBudget
                | agent_types::StopCondition::DurationBudget
                | agent_types::StopCondition::CostBudget
                | agent_types::StopCondition::SupervisorDenied => 0.30,
                agent_types::StopCondition::Interrupted => 0.50,
                _ => 0.0,
            };
            (coarse_reward, coarse_success)
        };

        sel.record_outcome(model_id, reward, success);
        tracing::debug!(
            model_id,
            reward,
            success,
            via = if captured_pipeline_reward.is_some() {
                "pipeline"
            } else {
                "coarse"
            },
            "Phase 2: ModelSelector quality record unified"
        );

        cache.model_quality = sel.snapshot_quality_stats();

        // Quality gate check.
        if let Some(warning) =
            sel.quality_gate_check_with_threshold(5, config.policy.model_quality_gate)
        {
            sink.warning(&warning, None);
        }

        // Fire-and-forget persist to DB.
        if let Some(ref adb) = async_db {
            let adb_clone = adb.clone();
            let prov = provider_name.to_string();
            // Wave 10: Snapshot includes compliance fields but DB schema only stores (s, f, r).
            // Compliance data persists within the session via cache; cross-session persistence
            // is deferred to Wave 11.
            let snapshot: Vec<(String, u32, u32, f64)> = cache
                .model_quality
                .iter()
                .map(|(k, &(s, f, r, _te, _ts))| (k.clone(), s, f, r))
                .collect();
            tokio::spawn(async move {
                if let Err(e) = adb_clone.save_model_quality_stats(&prov, snapshot).await {
                    tracing::warn!(error = %e, "Phase 4: model quality persist failed");
                }
            });
        }
    }

    /// Record per-plugin UCB1 rewards and persist (fire-and-forget).
    pub fn record_plugin_metrics(
        plugin_registry: &Option<Arc<Mutex<plugins::PluginRegistry>>>,
        result: &agent::AgentLoopResult,
        async_db: &Option<AsyncDatabase>,
    ) {
        let arc_reg = match plugin_registry {
            Some(r) => r,
            None => return,
        };

        let snapshot_data: Vec<halcon_storage::db::PluginMetricsRecord> =
            if let Ok(mut reg) = arc_reg.lock() {
                for snapshot in &result.plugin_cost_snapshot {
                    let rate = if snapshot.calls_made > 0 {
                        let succeeded = snapshot.calls_made.saturating_sub(snapshot.calls_failed);
                        succeeded as f64 / snapshot.calls_made as f64
                    } else {
                        0.5
                    };
                    reg.record_reward(&snapshot.plugin_id, rate);
                }
                reg.ucb1_snapshot()
                    .into_iter()
                    .map(|(plugin_id, n_uses, sum_rewards)| {
                        halcon_storage::db::PluginMetricsRecord {
                            plugin_id,
                            calls_made: 0,
                            calls_failed: 0,
                            tokens_used: 0,
                            ucb1_n_uses: n_uses as i64,
                            ucb1_sum_rewards: sum_rewards,
                            updated_at: String::new(),
                        }
                    })
                    .collect()
            } else {
                vec![]
            };

        if !snapshot_data.is_empty() {
            if let Some(ref adb) = async_db {
                let adb_clone = adb.clone();
                tokio::spawn(async move {
                    if let Err(e) = adb_clone.save_plugin_metrics(snapshot_data).await {
                        tracing::warn!(error = %e, "Phase 8-E: plugin metrics persist failed");
                    }
                });
            }
        }
    }

    /// Auto-learn successful plans as reusable playbooks.
    pub fn maybe_learn_playbook(
        features: &mut ReplFeatures,
        input: &str,
        result: &agent::AgentLoopResult,
        config: &AppConfig,
    ) {
        if !config.planning.auto_learn_playbooks {
            return;
        }
        if !matches!(
            result.stop_condition,
            agent_types::StopCondition::EndTurn | agent_types::StopCondition::ForcedSynthesis
        ) {
            return;
        }
        if features.playbook_planner.find_match(input).is_some() {
            return; // PlaybookPlanner already matched — don't re-learn
        }
        if let Some(ref timeline_json) = result.timeline_json {
            if let Some(saved_path) = features
                .playbook_planner
                .record_from_timeline(input, timeline_json)
            {
                tracing::info!(
                    path = %saved_path.display(),
                    "P3: Auto-saved plan as playbook for future reuse"
                );
            }
        }
    }

    /// Fire-and-forget runtime signal ingestion for telemetry.
    pub fn ingest_runtime_signals(
        runtime_signals: &Arc<runtime_signal_ingestor::RuntimeSignalIngestor>,
        result: &agent::AgentLoopResult,
    ) {
        let rt_signals = Arc::clone(runtime_signals);
        let loop_ms = result.latency_ms as f64;
        let had_error = matches!(
            result.stop_condition,
            agent_types::StopCondition::ProviderError
                | agent_types::StopCondition::EnvironmentError
        );
        tokio::spawn(async move {
            rt_signals
                .ingest(runtime_signal_ingestor::RuntimeSignal::span(
                    "agent_loop",
                    loop_ms,
                    had_error,
                ))
                .await;
        });
    }

    /// Display result summary (tokens, latency, cost, rounds).
    pub fn display_summary(result: &agent::AgentLoopResult, sink: &dyn RenderSink) {
        let total_tokens = result.input_tokens + result.output_tokens;
        if total_tokens > 0 || result.latency_ms > 0 {
            let cost_str = if result.cost_usd > 0.0 {
                format!(" | ${:.4}", result.cost_usd)
            } else {
                String::new()
            };
            let rounds_str = if result.rounds > 0 {
                format!(
                    " | {} tool {}",
                    result.rounds,
                    if result.rounds == 1 {
                        "round"
                    } else {
                        "rounds"
                    },
                )
            } else {
                String::new()
            };
            sink.info(&format!(
                "  [{} tokens | {:.1}s{}{}]",
                total_tokens,
                result.latency_ms as f64 / 1000.0,
                cost_str,
                rounds_str,
            ));
        }
    }

    /// Build the critic retry instruction text from gaps and failed steps.
    ///
    /// This is a pure function — no side effects, no `&mut self`.
    /// Extracted from the inline critic retry block in `handle_message_with_sink`.
    pub fn build_retry_text(
        decision: &CriticRetryDecision,
        failed_sub_agent_steps: &[crate::repl::agent_types::FailedStepContext],
    ) -> String {
        let instr = decision.retry_instruction.as_deref().unwrap_or(
            "Your previous response did not fully complete the task. Please address all missing elements.",
        );
        let failed_steps_note = if !failed_sub_agent_steps.is_empty() {
            format!(
                "\n\nFAILED APPROACHES (do NOT repeat these — use a different method):\n{}",
                failed_sub_agent_steps
                    .iter()
                    .map(|ctx| {
                        format!(
                            "  - [{}] {}: {}",
                            ctx.error_category.label(),
                            ctx.description,
                            ctx.error_message
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        } else {
            String::new()
        };
        format!(
            "[Critic retry]: Task incomplete. Missing: {}. Instruction: {}{}",
            if decision.gaps.is_empty() {
                "see previous response".to_string()
            } else {
                decision.gaps.join("; ")
            },
            instr,
            failed_steps_note
        )
    }

    /// Derive plan depth from timeline JSON (pure function).
    pub fn derive_plan_depth(timeline_json: &Option<String>) -> u32 {
        timeline_json
            .as_ref()
            .and_then(|json| {
                serde_json::from_str::<serde_json::Value>(json)
                    .ok()
                    .and_then(|v| v.get("steps")?.as_array().map(|a| a.len() as u32))
            })
            .unwrap_or(5)
    }

    /// Apply retry mutation axes to request parameters (pure function).
    ///
    /// Returns `(model, temperature, tools, max_rounds, fallback_provider_name)`.
    pub fn apply_mutation_axes(
        mutation: &Option<crate::repl::retry_mutation::MutationRecord>,
        request: &halcon_core::types::ModelRequest,
        max_rounds: usize,
    ) -> (
        String,
        Option<f32>,
        Vec<halcon_core::types::ToolDefinition>,
        usize,
        Option<String>,
    ) {
        let mut r_model = request.model.clone();
        let mut r_temp = request.temperature;
        let mut r_tools = request.tools.clone();
        let mut r_max_rounds = max_rounds;
        let mut fallback_name: Option<String> = None;

        if let Some(ref m) = mutation {
            for axis in &m.mutations {
                match axis {
                    crate::repl::retry_mutation::MutationAxis::ModelFallback { to, .. } => {
                        r_model = to.clone();
                        fallback_name = Some(to.clone());
                        tracing::info!(to = %to, "RetryMutation: switching provider for retry");
                    }
                    crate::repl::retry_mutation::MutationAxis::TemperatureIncreased {
                        to, ..
                    } => {
                        r_temp = Some(*to);
                    }
                    crate::repl::retry_mutation::MutationAxis::ToolExposureReduced { removed } => {
                        r_tools.retain(|t| !removed.contains(&t.name));
                    }
                    crate::repl::retry_mutation::MutationAxis::PlanDepthReduced { from, to } => {
                        if *from > 0 {
                            let ratio = *to as f64 / *from as f64;
                            r_max_rounds = ((r_max_rounds as f64 * ratio).ceil() as usize).max(3);
                            tracing::info!(
                                from_depth = from,
                                to_depth = to,
                                new_max_rounds = r_max_rounds,
                                "RetryMutation: PlanDepthReduced — clamped max_rounds"
                            );
                        }
                    }
                }
            }
            tracing::info!(axes = m.mutations.len(), "RetryMutation: applied");
        }

        (r_model, r_temp, r_tools, r_max_rounds, fallback_name)
    }

    /// Auto-consolidate reflections after each agent interaction.
    pub async fn consolidate_memory(async_db: &Option<AsyncDatabase>, sink: &dyn RenderSink) {
        let adb = match async_db {
            Some(db) => db,
            None => return,
        };

        sink.consolidation_status("consolidating reflections...");
        let consolidation_timeout = std::time::Duration::from_secs(30);
        let start = std::time::Instant::now();

        match tokio::time::timeout(
            consolidation_timeout,
            memory_consolidator::maybe_consolidate(adb),
        )
        .await
        {
            Ok(Some(result)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                tracing::debug!(
                    merged = result.merged,
                    pruned = result.pruned,
                    duration_ms,
                    "Memory consolidation completed"
                );
                sink.consolidation_complete(result.merged, result.pruned, duration_ms);
            }
            Ok(None) => {
                tracing::debug!("Memory consolidation skipped");
            }
            Err(_) => {
                tracing::warn!(
                    timeout_secs = consolidation_timeout.as_secs(),
                    "Memory consolidation timed out"
                );
                sink.warning(
                    "Memory consolidation took too long and was skipped",
                    Some("This is safe but may accumulate more reflections."),
                );
            }
        }
    }
}
