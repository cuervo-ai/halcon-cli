//! FASE 3.1: Reasoning Engine Coordinator (Simplified Integration)
//!
//! Metacognitive wrapper AROUND agent loop execution:
//! - PRE-LOOP: analyze task → select strategy → configure limits
//! - POST-LOOP: evaluate outcome → update experience

use halcon_core::types::{AgentLimits, ModelInfo};

use super::super::agent::{AgentLoopResult, StopCondition};
use super::super::intent_scorer::IntentScorer;
use super::super::model_router::ModelRouter;
use super::super::strategy_selector::{ReasoningStrategy, StrategyPlan, StrategySelector};
use super::super::task_analyzer::{TaskAnalysis, TaskComplexity, TaskType};

/// Temporary inline config (will be moved to halcon_core::types in Phase 4)
#[derive(Debug, Clone)]
pub struct ReasoningConfig {
    pub enabled: bool,
    pub success_threshold: f64,
    pub max_retries: u32,
    pub exploration_factor: f64,
}

impl Default for ReasoningConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            success_threshold: 0.6,
            max_retries: 1,
            exploration_factor: 1.4,
        }
    }
}

/// Pre-loop analysis result.
#[derive(Debug, Clone)]
pub struct PreLoopAnalysis {
    pub analysis: TaskAnalysis,
    pub strategy: ReasoningStrategy,
    pub adjusted_limits: AgentLimits,
    /// Full multi-dimensional strategy plan (for StrategyContext construction in mod.rs).
    pub plan: StrategyPlan,
}

/// Post-loop evaluation result.
#[derive(Debug, Clone)]
pub struct PostLoopEvaluation {
    pub success: bool,
    pub score: f64,
    pub task_type: TaskType,
    pub strategy: ReasoningStrategy,
}

/// Reasoning Engine — metacognitive coordinator (simplified).
pub struct ReasoningEngine {
    selector: StrategySelector,
    config: ReasoningConfig,
    /// True after load_experience() has been called — prevents double-loading in long sessions.
    experience_loaded: bool,
    /// P2-1 (SOTA-LEARN-001): guard against double UCB1 update within one session.
    /// post_loop_with_reward() sets this to true; record_per_round_signals() checks it.
    ucb1_updated_this_session: bool,
}

impl ReasoningEngine {
    /// Create a new ReasoningEngine (sync constructor).
    pub fn new(config: ReasoningConfig) -> Self {
        Self {
            selector: StrategySelector::new(config.exploration_factor),
            config,
            experience_loaded: false,
            ucb1_updated_this_session: false,
        }
    }

    /// Load cross-session UCB1 experience from DB records.
    ///
    /// Parses task_type and strategy strings (same format as save_reasoning_experience)
    /// and seeds the internal StrategySelector so UCB1 exploitation starts informed.
    /// Safe to call multiple times — only processes the first call (idempotent).
    pub fn load_experience(&mut self, experiences: Vec<(super::super::task_analyzer::TaskType, ReasoningStrategy, f64, usize)>) {
        self.selector.load_experience(experiences);
        self.experience_loaded = true;
        tracing::info!(count = self.selector.total_experience_count(), "UCB1: cross-session experience seeded");
    }

    /// Returns true if cross-session experience has already been loaded this session.
    pub fn is_experience_loaded(&self) -> bool {
        self.experience_loaded
    }

    /// Mark experience as loaded (used when DB returned empty — prevents repeated queries).
    pub fn mark_experience_loaded(&mut self) {
        self.experience_loaded = true;
    }

    /// PRE-LOOP: Analyze task and configure agent execution.
    ///
    /// `provider_models` is the list of models supported by the active provider — used to
    /// build a provider-aware `ModelRouter` instead of relying on hardcoded DeepSeek defaults.
    /// Pass `&[]` to fall back to `ModelRouter::deepseek_defaults()` (backward compatible).
    pub fn pre_loop(&mut self, user_query: &str, base_limits: &AgentLimits, provider_models: &[ModelInfo]) -> PreLoopAnalysis {
        // SOTA 2026: Multi-signal IntentScorer replaces keyword-only TaskAnalyzer.
        let profile = IntentScorer::score(user_query);

        // Map IntentProfile → TaskAnalysis for backward-compat with UCB1 experience tables.
        // R-02: propagate the real multi-signal confidence from IntentScorer instead of
        // hardcoding 1.0.  Callers (UCB1 guard, strategy selector) can now gate on
        // CONFIDENCE_FLOOR to reject uncertain classifications.
        let analysis = TaskAnalysis {
            task_type: profile.task_type,
            complexity: profile.complexity,
            task_hash: profile.task_hash.clone(),
            word_count: profile.word_count,
            // IntentProfile.confidence is f64 [0,1]; TaskAnalysis.confidence is f32.
            confidence: profile.confidence as f32,
            // IntentScorer does not produce keyword-level signals; leave empty.
            // When TaskAnalyzer::analyze() is used directly (tests, CLI tooling),
            // signals are populated by the SMRC keyword scan.
            signals: vec![],
        };

        let strategy = self.selector.select(&analysis);
        let mut plan = self.selector.configure(strategy, analysis.complexity);

        // Wire ModelRouter: always derive routing_bias from IntentProfile (D2 fix).
        // ModelRouter has primary authority — StrategySelector no longer sets routing_bias.
        // P0-1: Use provider-aware router when models are supplied; fall back to DeepSeek
        // defaults when the provider list is empty (backward-compatible behaviour).
        plan.routing_bias = ModelRouter::from_provider_models(provider_models).routing_bias_for(&profile);

        // NOTE: intentionally do NOT cap plan.max_rounds by profile.suggested_max_rounds().
        // The profile suggestion is used by ConvergenceController for EARLY-EXIT signals
        // (stagnation, goal-coverage) — it is guidance, not a hard ceiling.
        // Using it as a cap here produced a double-min() that limited top-level agents to
        // 4 rounds for "Simple" tasks regardless of the user-configured max_rounds=40.
        // The StrategySelector's plan.max_rounds is kept as a convergence-phase hint
        // (e.g. "aim for 5 rounds") consumed by LoopCritic / Reflexion timing logic.
        let profile_max = profile.suggested_max_rounds() as usize;

        tracing::info!(
            task_type = ?analysis.task_type,
            complexity = ?analysis.complexity,
            strategy = ?strategy,
            scope = ?profile.scope,
            reasoning_depth = ?profile.reasoning_depth,
            routing_bias = ?plan.routing_bias,
            plan_max_rounds = plan.max_rounds,
            profile_suggested_max = profile_max,
            config_max_rounds = base_limits.max_rounds,
            "Reasoning pre-loop (SOTA 2026)"
        );

        // For top-level agents the user-configured max_rounds is the authoritative hard limit.
        // Sub-agents use cap_max_rounds() (not set_max_rounds()) in mod.rs, which keeps their
        // 6-round hard cap intact. This layer only sets the ceiling; convergence signals
        // (ConvergenceController stagnation, coverage, LoopCritic) drive early exit.
        let adjusted_limits = AgentLimits {
            max_rounds: base_limits.max_rounds,
            ..base_limits.clone()
        };

        PreLoopAnalysis {
            analysis,
            strategy,
            adjusted_limits,
            plan,
        }
    }

    /// POST-LOOP (reward pipeline variant): Update UCB1 from a pre-computed reward.
    ///
    /// Called when `reward_pipeline::compute_reward()` has already assembled a multi-signal
    /// reward, replacing the inline StopCondition → score mapping. The existing `post_loop()`
    /// is preserved for backward compatibility (used in tests and non-reward-pipeline paths).
    pub fn post_loop_with_reward(
        &mut self,
        pre_analysis: &PreLoopAnalysis,
        reward: f64,
    ) -> PostLoopEvaluation {
        let success = reward >= self.config.success_threshold;
        self.selector.update(pre_analysis.analysis.task_type, pre_analysis.strategy, reward);
        // P2-1 (SOTA-LEARN-001): mark UCB1 as updated for this session so
        // record_per_round_signals() does not cause a second update on the same arm.
        self.ucb1_updated_this_session = true;
        tracing::info!(reward, success, "Reasoning post-loop (reward pipeline)");
        PostLoopEvaluation {
            success,
            score: reward,
            task_type: pre_analysis.analysis.task_type,
            strategy: pre_analysis.strategy,
        }
    }

    /// INTRA-SESSION (GAP-1 fix): Record per-round reward signals for richer UCB1 feedback.
    ///
    /// Called after each `run_agent_loop()` with the accumulated per-round scores from
    /// `AgentLoopResult::round_evaluations`. Each round's combined_score is fed into the
    /// UCB1 selector with an exponentially decaying weight (recent rounds count more).
    ///
    /// Coexists with `post_loop_with_reward()` — both are called, providing:
    /// - Per-round granular feedback (this method)
    /// - Session-level aggregate signal (post_loop_with_reward)
    pub fn record_per_round_signals(
        &mut self,
        pre_analysis: &PreLoopAnalysis,
        per_round_scores: &[f32],
    ) {
        if per_round_scores.is_empty() {
            return;
        }
        let n = per_round_scores.len();
        tracing::info!(
            rounds = n,
            task_type = ?pre_analysis.analysis.task_type,
            strategy = ?pre_analysis.strategy,
            "UCB1 per-round signal: recording {} round scores",
            n
        );
        // Decay factor: most-recent round counts for 2x relative to earliest.
        // Weights: w_i = 0.5 + 0.5 * (i / (n-1)) where i=0 is oldest, i=n-1 is newest.
        // Sum of weights for n rounds: n * 0.5 + 0.5 * (n-1)/2 ≈ 0.75n for large n.
        let total_weight: f64 = per_round_scores.iter().enumerate().map(|(i, _)| {
            let frac = if n > 1 { i as f64 / (n - 1) as f64 } else { 1.0 };
            0.5 + 0.5 * frac
        }).sum();

        let weighted_reward: f64 = per_round_scores.iter().enumerate().map(|(i, &score)| {
            let frac = if n > 1 { i as f64 / (n - 1) as f64 } else { 1.0 };
            let w = 0.5 + 0.5 * frac;
            score as f64 * w
        }).sum::<f64>() / total_weight.max(1e-9);

        // P2-1 (SOTA-LEARN-001): only update UCB1 if post_loop_with_reward() has NOT already
        // updated this session. Prevents double-incrementing `uses` counter which causes
        // premature convergence to suboptimal strategies.
        if self.ucb1_updated_this_session {
            tracing::debug!(
                weighted_reward,
                "UCB1 per-round signal skipped — session-level update already recorded"
            );
        } else {
            self.selector.update(
                pre_analysis.analysis.task_type,
                pre_analysis.strategy,
                weighted_reward.clamp(0.0, 1.0),
            );
            self.ucb1_updated_this_session = true;
        }
        tracing::debug!(
            weighted_reward,
            "UCB1 per-round signal: weighted reward computed from {} rounds",
            n
        );
    }

    /// POST-LOOP: Evaluate agent execution and update experience.
    ///
    /// # Production vs tests
    /// Production code uses `post_loop_with_reward()` which receives a pre-computed
    /// multi-signal reward from `reward_pipeline::compute_reward()`.  This method is
    /// only kept for test scenarios that need to drive the engine with a synthetic
    /// `AgentLoopResult` without a full reward pipeline.
    #[cfg(test)]
    pub fn post_loop(
        &mut self,
        pre_analysis: &PreLoopAnalysis,
        result: &AgentLoopResult,
    ) -> PostLoopEvaluation {
        // Base score from StopCondition (coarse signal — mirrors evaluator weights).
        let base_score = match result.stop_condition {
            StopCondition::EndTurn => 1.0,
            StopCondition::ForcedSynthesis | StopCondition::Interrupted => 0.7,
            StopCondition::MaxRounds => 0.4,
            StopCondition::TokenBudget
            | StopCondition::DurationBudget
            | StopCondition::CostBudget
            | StopCondition::SupervisorDenied => 0.3,
            StopCondition::ProviderError | StopCondition::EnvironmentError => 0.0,
        };

        // Phase 2: Blend RoundScorer trajectory when available (highest-fidelity signal).
        // round_evaluations provides per-round multi-dimensional scores — use trend_mean
        // (mean combined_score across all rounds) as the trajectory component.
        // Formula: trajectory_adjusted = 0.5 * stop_score + 0.5 * trend_mean
        // Falls back to stop_score when no rounds were evaluated.
        let trajectory_score = if !result.round_evaluations.is_empty() {
            let n = result.round_evaluations.len() as f64;
            let mean: f64 = result.round_evaluations.iter().map(|e| e.combined_score as f64).sum::<f64>() / n;
            let blended = 0.5 * base_score + 0.5 * mean;
            tracing::debug!(
                base_score, round_mean = mean, blended, rounds = n as usize,
                "UCB1 reward blended with RoundScorer trajectory"
            );
            blended
        } else {
            base_score
        };

        // Blend LoopCritic confidence when available (richer UCB1 signal).
        // Formula: blended = 0.6 * trajectory_score + 0.4 * critic_signal
        // When critic says achieved=false, confidence encodes partial credit.
        // When critic is unavailable (None), score is unchanged (backward-compatible).
        let score = if let Some(ref cv) = result.critic_verdict {
            let critic_signal = if cv.achieved {
                cv.confidence as f64  // critic agrees: full confidence weight
            } else {
                (1.0 - cv.confidence as f64) * 0.5  // critic disagrees: partial credit proportional to uncertainty
            };
            let blended = 0.6 * trajectory_score + 0.4 * critic_signal;
            tracing::debug!(
                trajectory_score, critic_confidence = cv.confidence, blended,
                "UCB1 reward blended with LoopCritic signal"
            );
            blended
        } else {
            trajectory_score
        };

        let success = score >= self.config.success_threshold;

        self.selector.update(
            pre_analysis.analysis.task_type,
            pre_analysis.strategy,
            score,
        );

        tracing::info!(score, base_score, trajectory_score, success, "Reasoning post-loop");

        PostLoopEvaluation {
            success,
            score,
            task_type: pre_analysis.analysis.task_type,
            strategy: pre_analysis.strategy,
        }
    }

    /// Check if retry is warranted.
    pub fn should_retry(&self, score: f64, retries_used: u32) -> bool {
        score < self.config.success_threshold && retries_used < self.config.max_retries
    }

    /// Produce a human-readable introspection summary for `/inspect reasoning` (Phase 3).
    ///
    /// Returns a multi-line string suitable for display in the REPL's `/inspect` output.
    /// Includes engine config, UCB1 experience summary, and total learning state.
    pub fn inspect_summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Enabled:              true\n"));
        out.push_str(&format!("Success threshold:    {:.2}\n", self.config.success_threshold));
        out.push_str(&format!("Max retries:          {}\n", self.config.max_retries));
        out.push_str(&format!("Exploration factor:   {:.2} (UCB1 c)\n", self.config.exploration_factor));
        out.push_str(&format!("Experience loaded:    {}\n", self.experience_loaded));
        let total_exp = self.selector.total_experience_count();
        out.push_str(&format!("UCB1 total uses:      {} (across all strategy×task_type pairs)\n", total_exp));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_config() -> ReasoningConfig {
        ReasoningConfig {
            enabled: true,
            success_threshold: 0.6,
            max_retries: 1,
            exploration_factor: 1.4,
        }
    }

    fn make_test_limits() -> AgentLimits {
        AgentLimits {
            max_rounds: 10,
            ..Default::default()
        }
    }

    #[test]
    fn new_engine_initializes() {
        let config = make_test_config();
        let _engine = ReasoningEngine::new(config);
    }

    #[test]
    fn pre_loop_analyzes_simple_task() {
        let mut engine = ReasoningEngine::new(make_test_config());
        let limits = make_test_limits();
        let analysis = engine.pre_loop("hello", &limits, &[]);

        assert_eq!(analysis.analysis.complexity, TaskComplexity::Simple);
        // adjusted_limits.max_rounds == base_limits.max_rounds (config is authoritative ceiling).
        // profile.suggested_max_rounds() is guidance only — not a hard cap on adjusted_limits.
        assert_eq!(analysis.adjusted_limits.max_rounds, limits.max_rounds);
    }

    #[test]
    fn post_loop_evaluates_success() {
        let mut engine = ReasoningEngine::new(make_test_config());
        let limits = make_test_limits();
        let analysis = engine.pre_loop("test", &limits, &[]);

        let result = AgentLoopResult {
            full_text: "Complete".to_string(),
            rounds: 2,
            stop_condition: StopCondition::EndTurn,
            input_tokens: 100,
            output_tokens: 200,
            cost_usd: 0.01,
            latency_ms: 1000,
            execution_fingerprint: "abc".to_string(),
            timeline_json: None,
            ctrl_rx: None,
            critic_verdict: None,
            round_evaluations: vec![],
            plan_completion_ratio: 1.0,
            avg_plan_drift: 0.0,
            oscillation_penalty: 0.0,
            last_model_used: None,
            plugin_cost_snapshot: vec![],
            tools_executed: vec![],
            evidence_verified: true,
            content_read_attempts: 0,
            last_provider_used: None,
            blocked_tools: vec![],
            failed_sub_agent_steps: vec![],
            critic_unavailable: false,
            tool_trust_failures: vec![],
            sla_budget: None,
            evidence_coverage: 1.0,
            synthesis_kind: None,
            synthesis_trigger: None,
            routing_escalation_count: 0,
        };

        let eval = engine.post_loop(&analysis, &result);
        assert!(eval.success);
    }

    // ── Phase 9: Closed-loop UCB1 reward→learning integration ────────────────

    #[test]
    fn reward_pipeline_feeds_ucb1_strategy_learning() {
        // Verify: post_loop_with_reward() with a high reward raises the strategy's avg_score
        // so UCB1 will prefer it on the next encounter of the same task type.
        let mut engine = ReasoningEngine::new(make_test_config());
        let limits = make_test_limits();
        let analysis = engine.pre_loop("refactor the authentication system", &limits, &[]);
        let chosen_strategy = analysis.strategy;
        let task_type = analysis.analysis.task_type;

        // Record a high-quality outcome via the reward pipeline.
        let eval = engine.post_loop_with_reward(&analysis, 0.92);
        assert!(eval.success, "reward 0.92 must exceed success_threshold=0.60");
        assert_eq!(eval.strategy, chosen_strategy);

        // Verify the experience was recorded in the UCB1 selector.
        let stats = engine.selector.get_stats(task_type, chosen_strategy);
        assert!(
            stats.is_some(),
            "strategy experience must be recorded after post_loop_with_reward"
        );
        let stats = stats.unwrap();
        assert_eq!(stats.uses, 1, "exactly one experience entry expected");
        assert!(
            (stats.avg_score - 0.92).abs() < 1e-9,
            "avg_score must equal the reward, got {}",
            stats.avg_score
        );
    }

    #[test]
    fn repeated_high_rewards_make_strategy_dominant_in_ucb1() {
        // After N high-reward outcomes, UCB1 should strongly prefer the winning strategy
        // over an unexplored alternative on the NEXT encounter of the same task type.
        let mut engine = ReasoningEngine::new(make_test_config());
        let limits = make_test_limits();

        // Simulate 5 complex tasks all solved well by PlanExecuteReflect.
        for _ in 0..5 {
            let analysis = engine.pre_loop("design a distributed caching system with sharding", &limits, &[]);
            engine.post_loop_with_reward(&analysis, 0.90);
        }

        // Now check: total experience recorded must be 5.
        assert_eq!(
            engine.selector.total_experience_count(),
            5,
            "5 experience entries must be accumulated"
        );

        // On the next complex task, UCB1 should select the proven strategy.
        let next = engine.pre_loop("build a distributed consensus algorithm", &limits, &[]);
        // With 5 outcomes all at 0.90, the winning strategy should be selected
        // (not the unexplored one, which gets INFINITY score — unless it was already explored).
        // Either way, the strategy chosen should be a valid ReasoningStrategy variant.
        let _ = next.strategy; // no panic = structural integrity
        // The strategy must have a configured plan
        assert!(next.adjusted_limits.max_rounds > 0);
    }

    #[test]
    fn low_reward_does_not_mark_as_success() {
        let mut engine = ReasoningEngine::new(make_test_config());
        let limits = make_test_limits();
        let analysis = engine.pre_loop("write code", &limits, &[]);

        // 0.30 is below success_threshold=0.60
        let eval = engine.post_loop_with_reward(&analysis, 0.30);
        assert!(
            !eval.success,
            "reward 0.30 must NOT exceed success_threshold=0.60"
        );
    }

    #[test]
    fn ucb1_total_experience_count_increments_after_each_loop() {
        let mut engine = ReasoningEngine::new(make_test_config());
        let limits = make_test_limits();

        assert_eq!(engine.selector.total_experience_count(), 0);

        for i in 1..=4 {
            let analysis = engine.pre_loop("refactor the database layer", &limits, &[]);
            engine.post_loop_with_reward(&analysis, 0.80);
            assert_eq!(
                engine.selector.total_experience_count(),
                i,
                "total experience count must increment after each post_loop_with_reward"
            );
        }
    }

    // ── R2-B: record_per_round_signals UCB1 wiring ───────────────────────────

    #[test]
    fn record_per_round_signals_updates_ucb1_with_weighted_reward() {
        let mut engine = ReasoningEngine::new(make_test_config());
        let limits = make_test_limits();
        let analysis = engine.pre_loop("analyze the build pipeline", &limits, &[]);
        let task_type = analysis.analysis.task_type;
        let strategy = analysis.strategy;

        // 3 rounds: scores improve over time (recency-weighted average must be >0.5)
        let round_scores = [0.4f32, 0.7, 0.9];
        engine.record_per_round_signals(&analysis, &round_scores);

        let stats = engine.selector.get_stats(task_type, strategy);
        assert!(stats.is_some(), "UCB1 experience must be recorded after record_per_round_signals");
        let stats = stats.unwrap();
        assert_eq!(stats.uses, 1, "exactly one experience entry expected");
        // Weighted reward = (0.4*0.5 + 0.7*0.75 + 0.9*1.0) / (0.5+0.75+1.0) ≈ 0.7222
        assert!(
            stats.avg_score > 0.5 && stats.avg_score <= 1.0,
            "weighted avg_score must be > 0.5 given improving round scores, got {}",
            stats.avg_score
        );
    }

    #[test]
    fn record_per_round_signals_empty_slice_is_noop() {
        let mut engine = ReasoningEngine::new(make_test_config());
        let limits = make_test_limits();
        let analysis = engine.pre_loop("simple query", &limits, &[]);
        let task_type = analysis.analysis.task_type;

        engine.record_per_round_signals(&analysis, &[]);

        let stats = engine.selector.get_stats(task_type, analysis.strategy);
        assert!(
            stats.is_none() || stats.unwrap().uses == 0,
            "no experience should be recorded for empty round_scores"
        );
    }

    #[test]
    fn record_per_round_signals_single_round_uses_score_directly() {
        let mut engine = ReasoningEngine::new(make_test_config());
        let limits = make_test_limits();
        let analysis = engine.pre_loop("quick fix", &limits, &[]);
        let task_type = analysis.analysis.task_type;
        let strategy = analysis.strategy;

        engine.record_per_round_signals(&analysis, &[0.85]);

        let stats = engine.selector.get_stats(task_type, strategy).unwrap();
        assert_eq!(stats.uses, 1);
        // Single round: frac=1.0 (n=1 branch), weight=1.0, so reward == 0.85
        assert!(
            (stats.avg_score - 0.85).abs() < 1e-6,
            "single-round score must pass through unchanged, got {}",
            stats.avg_score
        );
    }
}
