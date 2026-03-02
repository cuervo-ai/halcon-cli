//! Context-aware model selector.
//!
//! Selects the optimal model per request based on task complexity,
//! historical metrics, tool requirements, and budget constraints.

use std::collections::HashMap;

use halcon_core::types::{
    ChatMessage, MessageContent, ModelInfo, ModelRequest, ModelSelectionConfig,
};
use halcon_providers::ProviderRegistry;
use tracing::debug;

/// Detected task complexity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskComplexity {
    Simple,
    Standard,
    Complex,
}

/// Model selection result.
#[derive(Debug, Clone)]
pub struct ModelSelection {
    pub model_id: String,
    pub provider_name: String,
    pub reason: String,
}

/// Per-model quality statistics tracked within a session (Phase 4).
///
/// Accumulates reward signals from each agent loop completion to produce
/// a quality-adjusted effective cost used in the "balanced" routing strategy.
#[derive(Debug, Default)]
struct ModelPerformanceStats {
    /// Number of successful completions (stop_condition = EndTurn or ForcedSynthesis).
    success_count: u32,
    /// Number of failed completions (error, max_rounds, etc.).
    failure_count: u32,
    /// Cumulative reward signal from the reward pipeline (Phase 2).
    total_reward: f64,
}

impl ModelPerformanceStats {
    /// Average reward over all recorded outcomes (0.0 when no data).
    fn avg_reward(&self) -> f64 {
        let total = (self.success_count + self.failure_count) as f64;
        if total == 0.0 { return 0.5; } // Prior: neutral quality before any data
        self.total_reward / total
    }

    /// Quality penalty multiplier applied to effective cost in "balanced" routing.
    ///
    /// Returns a value in (0.5, 2.0]:
    /// - `1.0` → neutral (no adjustment, avg_reward ≈ 0.5)
    /// - `< 1.0` → bonus (high quality → effectively cheaper)
    /// - `> 1.0` → penalty (low quality → effectively more expensive)
    fn cost_multiplier(&self) -> f64 {
        // `2.0 - 2*avg` → at avg=1.0: mult=0.0 (free), at avg=0.5: mult=1.0 (neutral),
        // at avg=0.0: mult=2.0 (double cost penalty). Clamp to [0.5, 2.0] for stability.
        (2.0 - 2.0 * self.avg_reward()).clamp(0.5, 2.0)
    }
}

/// Selects the optimal model based on context.
///
/// IMPORTANT: By default, the selector is **provider-scoped** — it only considers
/// models from the active provider. This prevents silent cross-provider switching
/// (e.g., selecting gemini-2.0-flash when the user configured deepseek).
pub struct ModelSelector {
    config: ModelSelectionConfig,
    available_models: Vec<ModelInfo>,
    /// The provider to scope selection to. When set, only models from this provider
    /// are considered. When None, all providers are eligible (legacy behavior).
    scoped_provider: Option<String>,
    /// Per-model p95 latency hints in milliseconds (model_id → latency_ms).
    /// Populated from DB metrics at session start. Used by "fast" strategy.
    /// If a model has no hint, a default of 100ms is assumed.
    latency_hints: HashMap<String, u64>,
    /// Live latency observations from the current session (Phase 1.3).
    ///
    /// Updated on every agent round via `record_observed_latency()`.
    /// Values here override `latency_hints` so the "fast" strategy uses
    /// actual observed latency rather than stale DB p95 values from prior sessions.
    /// Uses interior mutability because AgentContext holds `Option<&ModelSelector>` (shared ref).
    live_latency_overrides: std::sync::Mutex<HashMap<String, u64>>,
    /// Per-model quality stats accumulated within this session (Phase 4).
    ///
    /// Updated by `record_outcome()` after each agent loop completion.
    /// Used to quality-adjust the "balanced" routing strategy so models that
    /// consistently fail or produce low-quality output are down-ranked.
    quality_stats: std::sync::Mutex<HashMap<String, ModelPerformanceStats>>,
    /// Recent model selection history for diversity enforcement (Phase 8).
    ///
    /// Records the last `REPETITION_WINDOW` model IDs chosen by `select_model()`.
    /// When the same model fills all `REPETITION_WINDOW` slots AND an alternative
    /// model with `avg_reward >= DIVERSITY_MIN_REWARD` exists, the guard swaps the
    /// selection to the best available alternative. Prevents routing from getting stuck
    /// on a single model even when it scored well in the past.
    selection_history: std::sync::Mutex<std::collections::VecDeque<String>>,
}

impl ModelSelector {
    /// Create a new model selector scoped to a specific provider.
    ///
    /// Only models from `active_provider` will be considered for selection,
    /// preventing silent cross-provider switching.
    pub fn new(config: ModelSelectionConfig, registry: &ProviderRegistry) -> Self {
        let mut available_models = Vec::new();
        for provider_name in registry.list() {
            if let Some(provider) = registry.get(provider_name) {
                available_models.extend_from_slice(provider.supported_models());
            }
        }
        Self {
            config,
            available_models,
            scoped_provider: None,
            latency_hints: HashMap::new(),
            live_latency_overrides: std::sync::Mutex::new(HashMap::new()),
            quality_stats: std::sync::Mutex::new(HashMap::new()),
            selection_history: std::sync::Mutex::new(std::collections::VecDeque::new()),
        }
    }

    /// Scope selection to a specific provider.
    ///
    /// When set, `select_model()` only considers models from this provider.
    /// This is the recommended default to prevent cross-provider model switching.
    pub fn with_provider_scope(mut self, provider_name: &str) -> Self {
        self.scoped_provider = Some(provider_name.to_string());
        self
    }

    /// Provide p95 latency hints (model_id → ms) from historical DB metrics.
    ///
    /// When set, the "fast" strategy sorts by actual observed latency instead
    /// of using context_window as a proxy. Call at session start after loading
    /// the DB metrics table.
    pub fn with_latency_hints(mut self, hints: HashMap<String, u64>) -> Self {
        self.latency_hints = hints;
        self
    }

    /// Record an observed round latency for a model (Phase 1.3 — live optimizer feedback).
    ///
    /// Updates the live override map using an exponential moving average (α=0.3).
    /// This makes the "fast" strategy progressively smarter within a session by
    /// feeding observed latency back into routing decisions.
    ///
    /// Uses interior mutability (`Mutex`) so the shared `&ModelSelector` reference
    /// in `AgentContext` can be updated without requiring `&mut`.
    pub fn record_observed_latency(&self, model_id: &str, latency_ms: u64) {
        if let Ok(mut overrides) = self.live_latency_overrides.lock() {
            let current = overrides.get(model_id).copied()
                .or_else(|| self.latency_hints.get(model_id).copied());
            let updated = if let Some(prev) = current {
                // EMA: new = 0.3 * observed + 0.7 * previous
                ((latency_ms as f64 * 0.3) + (prev as f64 * 0.7)) as u64
            } else {
                latency_ms
            };
            overrides.insert(model_id.to_string(), updated);
            tracing::debug!(model = %model_id, ema_latency_ms = updated, "ModelSelector: live latency updated");
        }
    }

    /// Record an agent loop outcome for quality-adjusted routing (Phase 4).
    ///
    /// Called after every agent loop completion with the model that ran the session
    /// and the final reward from the reward pipeline (0.0–1.0). The reward feeds
    /// a running quality average that adjusts the "balanced" strategy's effective cost:
    /// high-reward models get a routing bonus; low-reward models get a penalty.
    pub fn record_outcome(&self, model_id: &str, reward: f64, success: bool) {
        if let Ok(mut stats) = self.quality_stats.lock() {
            let entry = stats.entry(model_id.to_string()).or_default();
            if success {
                entry.success_count += 1;
            } else {
                entry.failure_count += 1;
            }
            entry.total_reward += reward.clamp(0.0, 1.0);
            tracing::debug!(
                model = %model_id,
                avg_reward = entry.avg_reward(),
                cost_mult = entry.cost_multiplier(),
                "ModelSelector: quality stats updated"
            );
        }
    }

    /// Check for provider-level quality degradation (Phase 7).
    ///
    /// Returns `Some(warning_message)` when the provider appears to be degraded:
    /// **all** tracked models with ≥ `min_interactions` outcomes have `avg_reward`
    /// below `QUALITY_GATE_THRESHOLD`. This fires only when there is enough data to
    /// draw a reliable conclusion (to prevent false positives on first use).
    ///
    /// Returns `None` when:
    /// - No models have been tracked yet (nothing to assess)
    /// - Any tracked model has `avg_reward ≥ QUALITY_GATE_THRESHOLD` (provider is healthy)
    /// - All tracked models have fewer than `min_interactions` outcomes (insufficient data)
    ///
    /// Callers should emit the returned string as a render_sink warning so the user
    /// is informed about provider quality issues without crashing or auto-switching.
    pub fn quality_gate_check(&self, min_interactions: u32) -> Option<String> {
        self.quality_gate_check_with_threshold(min_interactions, 0.35)
    }

    /// Quality gate with configurable threshold (from PolicyConfig.model_quality_gate).
    pub fn quality_gate_check_with_threshold(
        &self,
        min_interactions: u32,
        quality_gate_threshold: f64,
    ) -> Option<String> {
        let Ok(stats) = self.quality_stats.lock() else {
            return None;
        };

        // Collect models that have enough data to assess
        let assessed: Vec<f64> = stats
            .values()
            .filter(|s| s.success_count + s.failure_count >= min_interactions)
            .map(|s| s.avg_reward())
            .collect();

        if assessed.is_empty() {
            return None; // Not enough data yet
        }

        let best = assessed.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if best < quality_gate_threshold {
            let avg = assessed.iter().sum::<f64>() / assessed.len() as f64;
            Some(format!(
                "Provider quality degradation detected: best avg_reward={:.2} (threshold={:.2}), \
                 mean={:.2} across {} model(s). Consider switching providers.",
                best, quality_gate_threshold, avg, assessed.len()
            ))
        } else {
            None
        }
    }

    /// Snapshot current quality stats for session-level persistence (Phase 3).
    ///
    /// Returns a serializable tuple map: `model_id → (success_count, failure_count, total_reward)`.
    /// This snapshot is stored on the `Repl` struct and injected into the next fresh `ModelSelector`
    /// via `with_quality_seeds()` so quality tracking accumulates across messages within a session
    /// (rather than resetting to neutral every message).
    ///
    /// Tuple layout: `(success_count, failure_count, total_reward)` — matches `ModelPerformanceStats`.
    pub fn snapshot_quality_stats(&self) -> HashMap<String, (u32, u32, f64)> {
        if let Ok(stats) = self.quality_stats.lock() {
            stats.iter().map(|(k, v)| {
                (k.clone(), (v.success_count, v.failure_count, v.total_reward))
            }).collect()
        } else {
            HashMap::new()
        }
    }

    /// Seed quality stats from a prior session snapshot (Phase 3).
    ///
    /// Injects previously accumulated `(success_count, failure_count, total_reward)` tuples
    /// into the fresh selector so `balanced` routing immediately applies learned quality
    /// adjustments without waiting for new outcomes this message.
    ///
    /// Models not present in the snapshot start with the neutral prior (avg_reward = 0.5).
    pub fn with_quality_seeds(self, seeds: HashMap<String, (u32, u32, f64)>) -> Self {
        if let Ok(mut stats) = self.quality_stats.lock() {
            for (model_id, (success_count, failure_count, total_reward)) in seeds {
                let entry = stats.entry(model_id).or_default();
                entry.success_count = success_count;
                entry.failure_count = failure_count;
                entry.total_reward = total_reward;
            }
        }
        self
    }

    /// Get the quality-adjusted cost multiplier for a model (Phase 4).
    ///
    /// Returns 1.0 (neutral) when no data is available.
    /// < 1.0 = quality bonus (cheaper effective cost), > 1.0 = quality penalty.
    fn quality_cost_multiplier(&self, model_id: &str) -> f64 {
        if let Ok(stats) = self.quality_stats.lock() {
            if let Some(s) = stats.get(model_id) {
                return s.cost_multiplier();
            }
        }
        1.0 // Neutral prior — no data yet
    }

    /// Get average reward for a model from quality stats (Phase 5).
    ///
    /// Used by the "quality" routing strategy to rank models by demonstrated
    /// output quality (from the 5-signal reward pipeline). Returns the neutral
    /// prior (0.5) when no data exists — quality-naive models are treated as
    /// average until proven otherwise.
    fn avg_reward_for(&self, model_id: &str) -> f64 {
        if let Ok(stats) = self.quality_stats.lock() {
            if let Some(s) = stats.get(model_id) {
                return s.avg_reward();
            }
        }
        0.5 // Neutral prior — treat quality-naive models as average
    }

    /// Apply the diversity guard and record the selection in history (Phase 8).
    ///
    /// Checks whether the same model has been selected `REPETITION_WINDOW` consecutive times.
    /// If so, and an alternative with `avg_reward >= DIVERSITY_MIN_REWARD` exists among the
    /// available candidates, overrides the selection to break the repetition.
    ///
    /// The guard is intentionally lightweight:
    /// - Only fires when the window is fully saturated (never on cold start).
    /// - Requires the alternative to have a proven quality floor (`avg_reward >= 0.40`).
    /// - Falls back to the original selection when no qualified alternative exists.
    ///
    /// The selected model ID is always appended to `selection_history` (capped at
    /// `REPETITION_WINDOW` entries) regardless of whether the guard fired.
    fn apply_diversity_guard(
        &self,
        current: ModelSelection,
        candidates: &[&ModelInfo],
    ) -> ModelSelection {
        /// Number of consecutive identical selections that triggers the guard.
        const REPETITION_WINDOW: usize = 3;
        /// Minimum avg_reward an alternative must have to be considered (proven quality floor).
        const DIVERSITY_MIN_REWARD: f64 = 0.40;

        let Ok(mut history) = self.selection_history.lock() else {
            return current;
        };

        // Check if the guard should fire: last REPETITION_WINDOW are all the same model.
        let should_diversify = history.len() >= REPETITION_WINDOW
            && history
                .iter()
                .rev()
                .take(REPETITION_WINDOW)
                .all(|id| id == &current.model_id);

        let current_reward = self.avg_reward_for(&current.model_id);

        let result = if should_diversify {
            // Find the best alternative (highest avg_reward, above the quality floor AND
            // strictly better than the repeating model). The second condition prevents
            // swapping from a proven high-reward model (e.g. 0.95) to an untracked model
            // with the neutral prior (0.5) — which would cause A→B→A oscillation when both
            // models get selected consecutively enough to re-trigger the guard.
            let alternative = candidates
                .iter()
                .filter(|m| m.id != current.model_id)
                .filter_map(|m| {
                    let reward = self.avg_reward_for(&m.id);
                    if reward >= DIVERSITY_MIN_REWARD && reward > current_reward {
                        Some((m, reward))
                    } else {
                        None
                    }
                })
                .max_by(|(_, ra), (_, rb)| ra.partial_cmp(rb).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(m, _)| ModelSelection {
                    model_id: m.id.clone(),
                    provider_name: m.provider.clone(),
                    reason: format!(
                        "diversity guard: broke {}-consecutive repetition of '{}' — routing to '{}'",
                        REPETITION_WINDOW, current.model_id, m.id
                    ),
                });

            if let Some(alt) = alternative {
                tracing::debug!(
                    from = %current.model_id,
                    to = %alt.model_id,
                    window = REPETITION_WINDOW,
                    "Phase 8: diversity guard activated"
                );
                alt
            } else {
                // No qualified alternative — keep current (better than routing blindly).
                current
            }
        } else {
            current
        };

        // Record the final selection (after possible override) in history.
        history.push_back(result.model_id.clone());
        if history.len() > REPETITION_WINDOW {
            history.pop_front();
        }

        result
    }

    /// Get the effective latency for a model (Phase 1.3).
    ///
    /// Priority: live override (EMA) > static DB hint > default (100ms).
    fn effective_latency_for(&self, model_id: &str) -> u64 {
        const DEFAULT_LATENCY_MS: u64 = 100;
        if let Ok(overrides) = self.live_latency_overrides.lock() {
            if let Some(&live) = overrides.get(model_id) {
                return live;
            }
        }
        self.latency_hints.get(model_id).copied().unwrap_or(DEFAULT_LATENCY_MS)
    }

    /// Select the best model for a given request context.
    ///
    /// `routing_bias` is an optional UCB1 StrategyContext preference ("fast" | "cheap" |
    /// "quality").  When set, it **overrides** the complexity-based strategy selection
    /// so the agent loop's learned routing preference takes effect.  This closes the
    /// Phase 2 causal gap where `StrategyContext.routing_bias` was populated by UCB1
    /// but never passed to the selector, making it advisory-only.
    pub fn select_model(
        &self,
        request: &ModelRequest,
        session_spend_usd: f64,
        routing_bias: Option<&str>,
    ) -> Option<ModelSelection> {
        if !self.config.enabled {
            return None;
        }

        // Budget gate: if spending >= 90% of cap, force cheapest (budget always wins).
        if self.config.budget_cap_usd > 0.0
            && session_spend_usd >= self.config.budget_cap_usd * 0.9
        {
            return self.cheapest_model(request);
        }

        // Vision gate: if the request contains image data, only vision-capable models.
        let vision_required = Self::needs_vision(request);
        if vision_required {
            debug!("Vision content detected — restricting to supports_vision=true models");
        }

        // routing_bias override: UCB1 StrategyContext preference takes priority over
        // complexity detection when explicitly set.  Maps to the strategy strings used
        // by select_by_strategy(): "fast", "cheap", "quality", or "balanced".
        // Phase 5: "quality" now maps to the dedicated quality strategy (highest avg_reward
        // from ModelPerformanceTracker), NOT balanced.  PlanExecuteReflect+Complex tasks
        // that have learned quality preferences will route to the best-proven model.
        if let Some(bias) = routing_bias {
            let strategy = match bias {
                "fast" => "fast",
                "cheap" => "cheap",
                "quality" => "quality", // Phase 5: dedicated quality-first routing
                _ => "balanced",
            };
            debug!(%bias, %strategy, "routing_bias override applied — skipping complexity detection");
            return self.select_by_strategy(request, strategy, vision_required);
        }

        let complexity = Self::detect_complexity(request, self.config.complexity_token_threshold);
        debug!(?complexity, vision_required, "Detected task complexity");

        match complexity {
            TaskComplexity::Simple => {
                if let Some(ref model_id) = self.config.simple_model {
                    return self.find_model(model_id, "config override (simple)");
                }
                self.select_by_strategy(request, "cheap", vision_required)
            }
            TaskComplexity::Standard => self.select_by_strategy(request, "balanced", vision_required),
            TaskComplexity::Complex => {
                if let Some(ref model_id) = self.config.complex_model {
                    return self.find_model(model_id, "config override (complex)");
                }
                self.select_by_strategy(request, "fast", vision_required)
            }
        }
    }

    /// Detect if the request contains image/vision content requiring a vision-capable model.
    ///
    /// Checks for base64 image data URIs or explicit image markers in the system
    /// prompt and messages. When image blocks are added to ContentBlock, this can
    /// be extended to match them directly.
    pub fn needs_vision(request: &ModelRequest) -> bool {
        let image_markers = ["data:image/", "[image]", "[screenshot]", "<image>"];

        if let Some(sys) = &request.system {
            if image_markers.iter().any(|m| sys.contains(m)) {
                return true;
            }
        }
        request.messages.iter().any(|msg| {
            let text = match &msg.content {
                MessageContent::Text(t) => t.as_str(),
                _ => return false,
            };
            image_markers.iter().any(|m| text.contains(m))
        })
    }

    /// Detect task complexity from request content using weighted multi-signal scoring.
    ///
    /// Scores 0-100 across 6 dimensions:
    /// - Token volume (0-30): based on estimated token count vs threshold
    /// - Conversation depth (0-20): message count and multi-turn patterns
    /// - Tool interaction (0-15): presence of tools or tool results
    /// - Semantic keywords (0-25): reasoning/analysis/design keywords
    /// - Multi-step indicators (0-10): "then", "after that", "step", numbered lists
    /// - Question count (0-10): number of questions in last user message
    ///
    /// Thresholds: ≥25=Complex, ≥10=Standard, <10=Simple.
    pub fn detect_complexity(request: &ModelRequest, threshold: u32) -> TaskComplexity {
        let score = Self::complexity_score(request, threshold);
        debug!(score, "Complexity score");

        if score >= 25 {
            TaskComplexity::Complex
        } else if score >= 10 {
            TaskComplexity::Standard
        } else {
            TaskComplexity::Simple
        }
    }

    /// Compute the raw complexity score (0-100) for a request.
    pub fn complexity_score(request: &ModelRequest, threshold: u32) -> u32 {
        let estimated_tokens = estimate_message_tokens(&request.messages);
        let last_user_text = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == halcon_core::types::Role::User)
            .and_then(|m| m.content.as_text())
            .unwrap_or("");
        let lower = last_user_text.to_lowercase();

        // 1. Token volume (0-30): linear scale up to threshold
        let token_score = if threshold > 0 {
            std::cmp::min(30, (estimated_tokens * 30 / threshold).min(30))
        } else {
            0
        };

        // 2. Conversation depth (0-20)
        let msg_count = request.messages.len();
        let depth_score = if msg_count > 10 {
            20
        } else if msg_count > 6 {
            15
        } else if msg_count > 3 {
            8
        } else {
            0
        };

        // 3. Tool interaction (0-15)
        let has_tools = !request.tools.is_empty();
        let has_tool_results = request.messages.iter().any(|m| {
            matches!(&m.content, MessageContent::Blocks(blocks) if blocks.iter().any(|b|
                matches!(b, halcon_core::types::ContentBlock::ToolResult { .. })
            ))
        });
        let tool_score = if has_tool_results {
            15
        } else if has_tools {
            10
        } else {
            0
        };

        // 4. Semantic keywords (0-25)
        let keyword_patterns = [
            "explain", "analyze", "reason", "think step", "complex",
            "architecture", "design", "implement", "refactor", "debug",
            "optimize", "investigate", "compare",
        ];
        let keyword_hits: u32 = keyword_patterns
            .iter()
            .filter(|kw| lower.contains(**kw))
            .count() as u32;
        let keyword_score = std::cmp::min(25, keyword_hits * 8);

        // 5. Multi-step indicators (0-10)
        let multistep_patterns = [
            "then ", "after that", "step ", "first ", "next ",
            "finally ", "1.", "2.", "3.",
        ];
        let multistep_hits: u32 = multistep_patterns
            .iter()
            .filter(|p| lower.contains(**p))
            .count() as u32;
        let multistep_score = std::cmp::min(10, multistep_hits * 5);

        // 6. Question count (0-10)
        let question_count = last_user_text.matches('?').count() as u32;
        let question_score = std::cmp::min(10, question_count * 5);

        token_score + depth_score + tool_score + keyword_score + multistep_score + question_score
    }

    /// Get models eligible for selection, respecting provider scope.
    fn eligible_models(&self) -> impl Iterator<Item = &ModelInfo> {
        let scope = self.scoped_provider.clone();
        self.available_models
            .iter()
            .filter(move |m| match &scope {
                Some(p) => m.provider == *p,
                None => true,
            })
    }

    fn cheapest_model(&self, request: &ModelRequest) -> Option<ModelSelection> {
        let needs_tools = !request.tools.is_empty();
        let vision_required = Self::needs_vision(request);
        self.eligible_models()
            .filter(|m| !needs_tools || m.supports_tools)
            .filter(|m| !vision_required || m.supports_vision)
            .min_by(|a, b| {
                a.cost_per_input_token
                    .partial_cmp(&b.cost_per_input_token)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|m| ModelSelection {
                model_id: m.id.clone(),
                provider_name: m.provider.clone(),
                reason: "budget limit (≥90% spent)".into(),
            })
    }

    fn select_by_strategy(
        &self,
        request: &ModelRequest,
        strategy: &str,
        vision_required: bool,
    ) -> Option<ModelSelection> {
        let needs_tools = !request.tools.is_empty();

        let mut candidates: Vec<&ModelInfo> = self
            .eligible_models()
            .filter(|m| !needs_tools || m.supports_tools)
            .filter(|m| !vision_required || m.supports_vision)
            .collect();

        if candidates.is_empty() {
            return None;
        }

        match strategy {
            "cheap" => {
                candidates.sort_by(|a, b| {
                    a.cost_per_input_token
                        .partial_cmp(&b.cost_per_input_token)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            "fast" => {
                // Sort by effective latency: live EMA override (Phase 1.3) → static DB hint → default.
                // live_latency_overrides are updated every round via record_observed_latency(),
                // so within a session the "fast" strategy gets progressively better routing.
                candidates.sort_by(|a, b| {
                    let lat_a = self.effective_latency_for(&a.id);
                    let lat_b = self.effective_latency_for(&b.id);
                    lat_a
                        .cmp(&lat_b)
                        .then_with(|| b.context_window.cmp(&a.context_window))
                });
            }
            "quality" => {
                // quality: rank by avg_reward() from ModelPerformanceTracker (Phase 5).
                // Primary sort: avg_reward descending (highest quality model first).
                // Tiebreak: balanced score (cost efficiency + context window) so models
                // with no quality data yet (prior = 0.5) are ordered sensibly.
                // This makes routing_bias="quality" (UCB1 preference for PlanExecuteReflect+Complex)
                // actually use the best-proven model, not just balanced cost-efficiency.
                candidates.sort_by(|a, b| {
                    let qa = self.avg_reward_for(&a.id);
                    let qb = self.avg_reward_for(&b.id);
                    // Quality descending; break ties by balanced score
                    qb.partial_cmp(&qa)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| {
                            let sa = balance_score(a);
                            let sb = balance_score(b);
                            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
                        })
                });
            }
            _ => {
                // balanced: mid-range cost, prefer tools support, quality-adjusted (Phase 4).
                // quality_cost_multiplier() < 1.0 = bonus for high-quality models (lower eff cost),
                // > 1.0 = penalty for low-quality/failing models (higher eff cost → down-ranked).
                candidates.sort_by(|a, b| {
                    let mult_a = self.quality_cost_multiplier(&a.id);
                    let mult_b = self.quality_cost_multiplier(&b.id);
                    let score_a = balance_score_adjusted(a, mult_a);
                    let score_b = balance_score_adjusted(b, mult_b);
                    score_b
                        .partial_cmp(&score_a)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        candidates.first().map(|m| {
            let initial = ModelSelection {
                model_id: m.id.clone(),
                provider_name: m.provider.clone(),
                reason: format!("strategy={strategy}"),
            };
            // Phase 8: Apply the diversity guard — breaks 3-consecutive-repetition when
            // a quality-proven alternative exists. Guard is a no-op on cold start or when
            // all alternatives are below the DIVERSITY_MIN_REWARD floor.
            self.apply_diversity_guard(initial, &candidates)
        })
    }

    fn find_model(&self, model_id: &str, reason: &str) -> Option<ModelSelection> {
        self.eligible_models()
            .find(|m| m.id == model_id)
            .map(|m| ModelSelection {
                model_id: m.id.clone(),
                provider_name: m.provider.clone(),
                reason: reason.into(),
            })
    }
}

/// Balanced score: moderate cost, wide context, tool support bonus.
///
/// Used as tiebreaker in the "quality" routing strategy (Phase 5) when two
/// models have the same avg_reward (e.g. both at the neutral prior 0.5).
fn balance_score(model: &ModelInfo) -> f64 {
    balance_score_adjusted(model, 1.0)
}

/// Quality-adjusted balanced score (Phase 4).
///
/// `quality_multiplier` from `ModelPerformanceStats::cost_multiplier()`:
/// - 1.0 = neutral (no prior data or avg_reward ≈ 0.5)
/// - < 1.0 = quality bonus (effectively cheaper → higher score)
/// - > 1.0 = quality penalty (effectively more expensive → lower score)
fn balance_score_adjusted(model: &ModelInfo, quality_multiplier: f64) -> f64 {
    let adjusted_cost = model.cost_per_input_token * quality_multiplier;
    let cost_efficiency = 1.0 / (1.0 + adjusted_cost * 1_000_000.0);
    let context_score = (model.context_window as f64).log2() / 20.0;
    let tool_bonus = if model.supports_tools { 0.2 } else { 0.0 };
    cost_efficiency * 0.4 + context_score * 0.4 + tool_bonus
}

/// Rough token estimate: ~4 chars per token.
fn estimate_message_tokens(messages: &[ChatMessage]) -> u32 {
    let chars: usize = messages
        .iter()
        .map(|m| match &m.content {
            MessageContent::Text(t) => t.len(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    halcon_core::types::ContentBlock::Text { text } => text.len(),
                    halcon_core::types::ContentBlock::ToolResult { content, .. } => content.len(),
                    halcon_core::types::ContentBlock::ToolUse { input, .. } => {
                        serde_json::to_string(input).map(|s| s.len()).unwrap_or(0)
                    }
                    halcon_core::types::ContentBlock::Image { .. } => 1024,
                    halcon_core::types::ContentBlock::AudioTranscript { text, .. } => text.len(),
                })
                .sum(),
        })
        .sum();
    (chars / 4) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::{
        ChatMessage, ContentBlock, MessageContent, ModelRequest, Role, ToolDefinition,
    };

    fn make_request(msg: &str) -> ModelRequest {
        ModelRequest {
            model: "test".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(msg.into()),
            }],
            tools: vec![],
            max_tokens: Some(1024),
            temperature: None,
            system: None,
            stream: true,
        }
    }

    fn make_long_request() -> ModelRequest {
        let long_text = "x".repeat(10000); // ~2500 tokens
        make_request(&long_text)
    }

    fn make_request_with_tools(msg: &str) -> ModelRequest {
        ModelRequest {
            model: "test".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(msg.into()),
            }],
            tools: vec![ToolDefinition {
                name: "bash".into(),
                description: "Run".into(),
                input_schema: serde_json::json!({}),
            }],
            max_tokens: Some(1024),
            temperature: None,
            system: None,
            stream: true,
        }
    }

    fn make_multi_round_request() -> ModelRequest {
        ModelRequest {
            model: "test".into(),
            messages: vec![
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text("hello".into()),
                },
                ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text("hi".into()),
                },
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text("do something".into()),
                },
                ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "bash".into(),
                        input: serde_json::json!({}),
                    }]),
                },
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id: "t1".into(),
                        content: "result".into(),
                        is_error: false,
                    }]),
                },
                ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Text("done".into()),
                },
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Text("now do more".into()),
                },
            ],
            tools: vec![],
            max_tokens: Some(1024),
            temperature: None,
            system: None,
            stream: true,
        }
    }

    fn test_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "cheap-model".into(),
                name: "Cheap".into(),
                provider: "test".into(),
                context_window: 32_000,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.1 / 1_000_000.0,
                cost_per_output_token: 0.2 / 1_000_000.0,
            },
            ModelInfo {
                id: "mid-model".into(),
                name: "Mid".into(),
                provider: "test".into(),
                context_window: 128_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 2.5 / 1_000_000.0,
                cost_per_output_token: 10.0 / 1_000_000.0,
            },
            ModelInfo {
                id: "expensive-model".into(),
                name: "Expensive".into(),
                provider: "test".into(),
                context_window: 200_000,
                max_output_tokens: 32_000,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: true,
                cost_per_input_token: 15.0 / 1_000_000.0,
                cost_per_output_token: 75.0 / 1_000_000.0,
            },
            ModelInfo {
                id: "no-tools-model".into(),
                name: "NoTools".into(),
                provider: "test".into(),
                context_window: 64_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: false,
                supports_vision: false,
                supports_reasoning: true,
                cost_per_input_token: 0.5 / 1_000_000.0,
                cost_per_output_token: 2.0 / 1_000_000.0,
            },
        ]
    }

    fn make_selector(config: ModelSelectionConfig) -> ModelSelector {
        ModelSelector {
            config,
            available_models: test_models(),
            scoped_provider: None,
            latency_hints: HashMap::new(),
            live_latency_overrides: std::sync::Mutex::new(HashMap::new()),
            quality_stats: std::sync::Mutex::new(HashMap::new()),
            selection_history: std::sync::Mutex::new(std::collections::VecDeque::new()),
        }
    }

    fn enabled_config() -> ModelSelectionConfig {
        ModelSelectionConfig {
            enabled: true,
            ..Default::default()
        }
    }

    // --- detect_complexity tests ---

    #[test]
    fn detect_simple_short_message() {
        let req = make_request("hello");
        assert_eq!(
            ModelSelector::detect_complexity(&req, 2000),
            TaskComplexity::Simple
        );
    }

    #[test]
    fn detect_complex_long_message() {
        let req = make_long_request();
        assert_eq!(
            ModelSelector::detect_complexity(&req, 2000),
            TaskComplexity::Complex
        );
    }

    #[test]
    fn detect_standard_with_tools() {
        let req = make_request_with_tools("run ls");
        assert_eq!(
            ModelSelector::detect_complexity(&req, 2000),
            TaskComplexity::Standard
        );
    }

    #[test]
    fn detect_complex_multi_round() {
        let req = make_multi_round_request();
        assert_eq!(
            ModelSelector::detect_complexity(&req, 2000),
            TaskComplexity::Complex
        );
    }

    // --- select_model tests ---

    #[test]
    fn disabled_returns_none() {
        let config = ModelSelectionConfig { enabled: false, ..Default::default() };
        let selector = make_selector(config);
        let req = make_request("hello");
        assert!(selector.select_model(&req, 0.0, None).is_none());
    }

    #[test]
    fn budget_exceeded_forces_cheapest() {
        let config = ModelSelectionConfig {
            enabled: true,
            budget_cap_usd: 1.0,
            ..Default::default()
        };
        let selector = make_selector(config);
        let req = make_request("hello");
        let selection = selector.select_model(&req, 0.95, None).unwrap();
        assert_eq!(selection.model_id, "cheap-model");
        assert!(selection.reason.contains("budget"));
    }

    #[test]
    fn simple_selects_cheap() {
        let selector = make_selector(enabled_config());
        let req = make_request("hi");
        let selection = selector.select_model(&req, 0.0, None).unwrap();
        assert_eq!(selection.model_id, "cheap-model");
    }

    #[test]
    fn complex_selects_capable() {
        let selector = make_selector(enabled_config());
        let req = make_long_request();
        let selection = selector.select_model(&req, 0.0, None).unwrap();
        // For "fast" strategy, prefers largest context window
        assert_eq!(selection.model_id, "expensive-model");
    }

    #[test]
    fn tools_filter_excludes_no_tools_model() {
        let selector = make_selector(enabled_config());
        let req = make_request_with_tools("run something");
        let selection = selector.select_model(&req, 0.0, None).unwrap();
        // no-tools-model should be filtered out
        assert_ne!(selection.model_id, "no-tools-model");
    }

    #[test]
    fn simple_model_override() {
        let config = ModelSelectionConfig {
            enabled: true,
            simple_model: Some("mid-model".into()),
            ..Default::default()
        };
        let selector = make_selector(config);
        let req = make_request("hi");
        let selection = selector.select_model(&req, 0.0, None).unwrap();
        assert_eq!(selection.model_id, "mid-model");
        assert!(selection.reason.contains("override"));
    }

    #[test]
    fn complex_model_override() {
        let config = ModelSelectionConfig {
            enabled: true,
            complex_model: Some("cheap-model".into()),
            ..Default::default()
        };
        let selector = make_selector(config);
        let req = make_long_request();
        let selection = selector.select_model(&req, 0.0, None).unwrap();
        assert_eq!(selection.model_id, "cheap-model");
        assert!(selection.reason.contains("override"));
    }

    // --- config tests ---

    #[test]
    fn config_defaults() {
        let config = ModelSelectionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.budget_cap_usd, 0.0);
        assert_eq!(config.complexity_token_threshold, 2000);
        assert!(config.simple_model.is_none());
        assert!(config.complex_model.is_none());
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = ModelSelectionConfig {
            enabled: true,
            budget_cap_usd: 10.0,
            complexity_token_threshold: 3000,
            simple_model: Some("gpt-4o-mini".into()),
            complex_model: Some("claude-opus-4-6".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let roundtrip: ModelSelectionConfig = serde_json::from_str(&json).unwrap();
        assert!(roundtrip.enabled);
        assert_eq!(roundtrip.budget_cap_usd, 10.0);
        assert_eq!(roundtrip.simple_model.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn reasoning_keywords_trigger_complex() {
        let req = make_request("Please analyze this architecture and explain the design");
        assert_eq!(
            ModelSelector::detect_complexity(&req, 2000),
            TaskComplexity::Complex
        );
    }

    // --- Phase 18: Enhanced complexity detection tests ---

    #[test]
    fn multi_question_complex() {
        let req = make_request(
            "What is the architecture? How does routing work? Can you explain the fallback?"
        );
        let score = ModelSelector::complexity_score(&req, 2000);
        assert!(score >= 25, "3 questions + keywords should be complex, got {score}");
    }

    #[test]
    fn implement_and_test_complex() {
        let req = make_request(
            "Implement a new parser module. Then add unit tests. After that, refactor the existing code to use it."
        );
        let score = ModelSelector::complexity_score(&req, 2000);
        assert!(score >= 25, "implement+refactor+multi-step should be complex, got {score}");
    }

    #[test]
    fn greeting_simple() {
        let req = make_request("hello");
        let score = ModelSelector::complexity_score(&req, 2000);
        assert!(score < 10, "greeting should be simple, got {score}");
    }

    #[test]
    fn code_review_standard() {
        let req = make_request_with_tools("Review this function for bugs");
        let score = ModelSelector::complexity_score(&req, 2000);
        assert!(
            score >= 10,
            "code review with tools should be at least standard, got {score}"
        );
    }

    #[test]
    fn score_accumulation() {
        // Test that scores accumulate from multiple dimensions.
        let req_simple = make_request("hi");
        let score_simple = ModelSelector::complexity_score(&req_simple, 2000);

        let req_complex = make_request(
            "Explain and analyze the architecture. Design a new system. Then implement it step by step. What are the tradeoffs?"
        );
        let score_complex = ModelSelector::complexity_score(&req_complex, 2000);

        assert!(
            score_complex > score_simple,
            "complex request ({score_complex}) should score higher than simple ({score_simple})"
        );
        assert!(score_complex >= 25, "multi-signal request should be complex, got {score_complex}");
    }

    // --- Provider scoping tests ---

    #[test]
    fn scoped_selector_only_picks_from_scoped_provider() {
        // Add models from two providers.
        let models = vec![
            ModelInfo {
                id: "deepseek-chat".into(),
                name: "DeepSeek Chat".into(),
                provider: "deepseek".into(),
                context_window: 64_000,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.1 / 1_000_000.0,
                cost_per_output_token: 0.2 / 1_000_000.0,
            },
            ModelInfo {
                id: "gemini-flash".into(),
                name: "Gemini Flash".into(),
                provider: "gemini".into(),
                context_window: 128_000,
                max_output_tokens: 8192,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                cost_per_input_token: 0.05 / 1_000_000.0,
                cost_per_output_token: 0.1 / 1_000_000.0,
            },
        ];
        let selector = ModelSelector {
            config: enabled_config(),
            available_models: models,
            scoped_provider: Some("deepseek".into()),
            latency_hints: HashMap::new(),
            live_latency_overrides: std::sync::Mutex::new(HashMap::new()),
            quality_stats: std::sync::Mutex::new(HashMap::new()),
            selection_history: std::sync::Mutex::new(std::collections::VecDeque::new()),
        };
        let req = make_request("hi");
        let selection = selector.select_model(&req, 0.0, None).unwrap();
        // Must select from deepseek, not gemini (even though gemini is cheaper).
        assert_eq!(selection.provider_name, "deepseek");
        assert_eq!(selection.model_id, "deepseek-chat");
    }

    #[test]
    fn unscoped_selector_can_pick_cross_provider() {
        let models = vec![
            ModelInfo {
                id: "expensive".into(),
                name: "Expensive".into(),
                provider: "provider-a".into(),
                context_window: 32_000,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 10.0 / 1_000_000.0,
                cost_per_output_token: 20.0 / 1_000_000.0,
            },
            ModelInfo {
                id: "cheap".into(),
                name: "Cheap".into(),
                provider: "provider-b".into(),
                context_window: 32_000,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.01 / 1_000_000.0,
                cost_per_output_token: 0.02 / 1_000_000.0,
            },
        ];
        let selector = ModelSelector {
            config: enabled_config(),
            available_models: models,
            scoped_provider: None, // no scope — legacy behavior
            latency_hints: HashMap::new(),
            live_latency_overrides: std::sync::Mutex::new(HashMap::new()),
            quality_stats: std::sync::Mutex::new(HashMap::new()),
            selection_history: std::sync::Mutex::new(std::collections::VecDeque::new()),
        };
        let req = make_request("hi");
        let selection = selector.select_model(&req, 0.0, None).unwrap();
        // Without scoping, cheapest model from any provider wins.
        assert_eq!(selection.model_id, "cheap");
        assert_eq!(selection.provider_name, "provider-b");
    }

    #[test]
    fn scoped_to_empty_provider_returns_none() {
        let selector = ModelSelector {
            config: enabled_config(),
            available_models: test_models(), // all models are from "test" provider
            scoped_provider: Some("nonexistent".into()),
            latency_hints: HashMap::new(),
            live_latency_overrides: std::sync::Mutex::new(HashMap::new()),
            quality_stats: std::sync::Mutex::new(HashMap::new()),
            selection_history: std::sync::Mutex::new(std::collections::VecDeque::new()),
        };
        let req = make_request("hi");
        // No models match scope → returns None.
        assert!(selector.select_model(&req, 0.0, None).is_none());
    }

    // --- P1: Vision routing tests ---

    #[test]
    fn needs_vision_detects_image_uri() {
        let mut req = make_request("analyze this");
        req.system = Some("Here is the image data:image/png;base64,abc123".into());
        assert!(ModelSelector::needs_vision(&req));
    }

    #[test]
    fn needs_vision_detects_image_marker_in_message() {
        let req = make_request("Please review [screenshot] and describe what you see");
        assert!(ModelSelector::needs_vision(&req));
    }

    #[test]
    fn needs_vision_false_for_plain_text() {
        let req = make_request("Write a function that sorts a list");
        assert!(!ModelSelector::needs_vision(&req));
    }

    #[test]
    fn vision_required_filters_non_vision_models() {
        let selector = make_selector(enabled_config());
        // Request mentioning an image — only mid-model and expensive-model support vision
        let req = make_request("[image] Can you describe what's in this screenshot?");
        let selection = selector.select_model(&req, 0.0, None).unwrap();
        // cheap-model has supports_vision=false, should not be selected
        assert_ne!(selection.model_id, "cheap-model");
        assert_ne!(selection.model_id, "no-tools-model");
        // Must be a vision-capable model
        let selected = test_models().into_iter().find(|m| m.id == selection.model_id).unwrap();
        assert!(selected.supports_vision);
    }

    // --- P1: Latency hints tests ---

    #[test]
    fn latency_hints_prefer_faster_model() {
        // Build a selector where "cheap-model" has very low latency (20ms)
        // and "expensive-model" has high latency (500ms).
        let hints: HashMap<String, u64> = [
            ("cheap-model".to_string(), 20u64),
            ("mid-model".to_string(), 80u64),
            ("expensive-model".to_string(), 500u64),
        ].into_iter().collect();

        let selector = ModelSelector {
            config: enabled_config(),
            available_models: test_models(),
            scoped_provider: None,
            latency_hints: hints,
            live_latency_overrides: std::sync::Mutex::new(HashMap::new()),
            quality_stats: std::sync::Mutex::new(HashMap::new()),
            selection_history: std::sync::Mutex::new(std::collections::VecDeque::new()),
        };

        // Complex task → "fast" strategy → should pick fastest model (cheap-model at 20ms)
        let req = make_long_request();
        let selection = selector.select_model(&req, 0.0, None).unwrap();
        assert_eq!(selection.model_id, "cheap-model",
            "fast strategy with latency hints should pick the lowest-latency model");
    }

    #[test]
    fn fast_strategy_without_hints_falls_back_to_context_window() {
        // No latency hints → all models get DEFAULT_LATENCY_MS=100, tie broken by context_window
        let selector = make_selector(enabled_config());
        let req = make_long_request();
        let selection = selector.select_model(&req, 0.0, None).unwrap();
        // expensive-model has the largest context_window (200_000)
        assert_eq!(selection.model_id, "expensive-model");
    }

    #[test]
    fn with_latency_hints_builder() {
        let hints: HashMap<String, u64> = [("m1".to_string(), 50u64)].into_iter().collect();
        let selector = make_selector(enabled_config()).with_latency_hints(hints.clone());
        assert_eq!(selector.latency_hints.get("m1").copied(), Some(50));
    }

    // ── Phase 4: ModelPerformanceTracker tests ──────────────────────────────

    #[test]
    fn quality_multiplier_neutral_with_no_data() {
        let selector = make_selector(enabled_config());
        // No outcomes recorded → neutral prior (1.0)
        assert_eq!(selector.quality_cost_multiplier("unknown-model"), 1.0);
    }

    #[test]
    fn quality_multiplier_decreases_after_successes() {
        let selector = make_selector(enabled_config());
        // Record 3 high-reward successes
        selector.record_outcome("my-model", 0.95, true);
        selector.record_outcome("my-model", 0.90, true);
        selector.record_outcome("my-model", 0.85, true);
        // avg_reward ≈ 0.9 → multiplier = 2.0 - 2*0.9 = 0.2 → clamped to 0.5
        let mult = selector.quality_cost_multiplier("my-model");
        assert!(mult < 1.0, "high-quality model should get routing bonus (mult < 1.0), got {mult}");
    }

    #[test]
    fn quality_multiplier_increases_after_failures() {
        let selector = make_selector(enabled_config());
        // Record 3 zero-reward failures
        selector.record_outcome("bad-model", 0.0, false);
        selector.record_outcome("bad-model", 0.0, false);
        selector.record_outcome("bad-model", 0.0, false);
        // avg_reward = 0.0 → multiplier = 2.0 - 0 = 2.0
        let mult = selector.quality_cost_multiplier("bad-model");
        assert!(mult > 1.0, "low-quality model should get routing penalty (mult > 1.0), got {mult}");
        assert_eq!(mult, 2.0);
    }

    #[test]
    fn balanced_strategy_favors_high_quality_model() {
        // Two models with identical costs: one has high-quality history, one has failures.
        let models = vec![
            ModelInfo {
                id: "good-model".into(),
                name: "Good".into(),
                provider: "test".into(),
                context_window: 32_000,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 1.0 / 1_000_000.0,
                cost_per_output_token: 2.0 / 1_000_000.0,
            },
            ModelInfo {
                id: "bad-model".into(),
                name: "Bad".into(),
                provider: "test".into(),
                context_window: 32_000,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 1.0 / 1_000_000.0, // same cost as good-model
                cost_per_output_token: 2.0 / 1_000_000.0,
            },
        ];
        let selector = ModelSelector {
            config: enabled_config(),
            available_models: models,
            scoped_provider: None,
            latency_hints: HashMap::new(),
            live_latency_overrides: std::sync::Mutex::new(HashMap::new()),
            quality_stats: std::sync::Mutex::new(HashMap::new()),
            selection_history: std::sync::Mutex::new(std::collections::VecDeque::new()),
        };
        // Record quality outcomes
        selector.record_outcome("good-model", 0.95, true);
        selector.record_outcome("good-model", 0.90, true);
        selector.record_outcome("bad-model", 0.0, false);
        selector.record_outcome("bad-model", 0.0, false);

        // "balanced" strategy (default for simple requests) should prefer good-model
        let req = make_request("hi");
        let selection = selector.select_model(&req, 0.0, None).unwrap();
        assert_eq!(selection.model_id, "good-model",
            "balanced strategy should prefer high-quality model over identical-cost failing model");
    }

    #[test]
    fn performance_stats_avg_reward_neutral_prior() {
        let stats = ModelPerformanceStats::default();
        assert_eq!(stats.avg_reward(), 0.5, "no-data prior should be 0.5 (neutral)");
        assert_eq!(stats.cost_multiplier(), 1.0, "neutral avg_reward → neutral multiplier");
    }

    // ── Phase 2: routing_bias causal wiring tests ──────────────────────────

    #[test]
    fn routing_bias_fast_overrides_complexity() {
        // Even a simple short request (would normally pick "cheap") should get
        // the fast-strategy model when routing_bias="fast" is supplied.
        let selector = make_selector(enabled_config());
        let req = make_request("hi"); // Simple → normally "cheap" → cheap-model
        let selection = selector.select_model(&req, 0.0, Some("fast")).unwrap();
        // "fast" strategy sorts by latency (no hints → context_window tie-break).
        // expensive-model has largest context_window (200K) → wins the tie.
        assert_eq!(
            selection.model_id, "expensive-model",
            "routing_bias=fast should route to the widest-context (fastest) model, got {}",
            selection.model_id
        );
        assert!(selection.reason.contains("fast"), "reason should reflect fast strategy");
    }

    #[test]
    fn routing_bias_cheap_overrides_complexity() {
        // Complex request (would normally pick "fast") should get cheap model
        // when routing_bias="cheap".
        let selector = make_selector(enabled_config());
        let req = make_long_request(); // Complex → normally "fast"
        let selection = selector.select_model(&req, 0.0, Some("cheap")).unwrap();
        assert_eq!(
            selection.model_id, "cheap-model",
            "routing_bias=cheap should route to cheapest model regardless of complexity"
        );
    }

    #[test]
    fn routing_bias_quality_routes_quality_strategy() {
        // Phase 5: "quality" bias maps to the dedicated "quality" strategy (not "balanced").
        // With no prior quality data (all priors = 0.5), quality strategy falls back to
        // balance_score tiebreaker — the reason should say "quality" not "balanced".
        let selector = make_selector(enabled_config());
        let req = make_request("hi");
        let selection = selector.select_model(&req, 0.0, Some("quality")).unwrap();
        assert!(
            selection.reason.contains("quality"),
            "routing_bias=quality should use quality strategy, got: {}",
            selection.reason
        );
    }

    #[test]
    fn routing_bias_none_falls_through_to_complexity() {
        // No bias → complexity-based as before. Simple request → cheap strategy.
        let selector = make_selector(enabled_config());
        let req = make_request("hi");
        let without_bias = selector.select_model(&req, 0.0, None).unwrap();
        assert_eq!(
            without_bias.model_id, "cheap-model",
            "no routing_bias → complexity-based selection (simple → cheap)"
        );
    }

    // ── Phase 3: snapshot/seed session persistence ───────────────────────────

    #[test]
    fn snapshot_quality_stats_empty_when_no_outcomes() {
        let selector = make_selector(enabled_config());
        let snapshot = selector.snapshot_quality_stats();
        assert!(snapshot.is_empty(), "fresh selector should have empty quality snapshot");
    }

    #[test]
    fn snapshot_quality_stats_captures_recorded_outcomes() {
        let selector = make_selector(enabled_config());
        selector.record_outcome("model-a", 0.9, true);
        selector.record_outcome("model-a", 0.8, true);
        selector.record_outcome("model-b", 0.2, false);
        let snapshot = selector.snapshot_quality_stats();
        assert_eq!(snapshot.len(), 2);
        let (succ_a, fail_a, reward_a) = snapshot["model-a"];
        assert_eq!(succ_a, 2);
        assert_eq!(fail_a, 0);
        assert!((reward_a - 1.7).abs() < 0.01, "total reward should be 0.9+0.8=1.7, got {reward_a}");
        let (succ_b, fail_b, _) = snapshot["model-b"];
        assert_eq!(succ_b, 0);
        assert_eq!(fail_b, 1);
    }

    #[test]
    fn with_quality_seeds_restores_prior_stats() {
        // First selector: records outcomes.
        let sel1 = make_selector(enabled_config());
        sel1.record_outcome("model-x", 0.95, true);
        sel1.record_outcome("model-x", 0.85, true);
        let snapshot = sel1.snapshot_quality_stats();

        // Second selector: starts from snapshot — should have informed quality prior.
        let sel2 = make_selector(enabled_config()).with_quality_seeds(snapshot);
        // quality_cost_multiplier should reflect the seeded high quality (< 1.0 = bonus)
        let mult = sel2.quality_cost_multiplier("model-x");
        assert!(mult < 1.0, "seeded high-quality model should have routing bonus, got {mult}");
    }

    #[test]
    fn with_quality_seeds_empty_map_is_noop() {
        let sel = make_selector(enabled_config()).with_quality_seeds(HashMap::new());
        // No panic, snapshot is still empty.
        assert!(sel.snapshot_quality_stats().is_empty());
    }

    #[test]
    fn session_persistence_roundtrip() {
        // Simulate: message 1 records outcomes → snapshot → message 2 inherits quality.
        let msg1_sel = make_selector(enabled_config());
        msg1_sel.record_outcome("fast-model", 0.1, false); // bad model
        msg1_sel.record_outcome("slow-model", 0.9, true);  // good model
        let cache = msg1_sel.snapshot_quality_stats();

        // Message 2: fresh selector seeded from cache.
        let msg2_sel = make_selector(enabled_config()).with_quality_seeds(cache);
        let mult_fast = msg2_sel.quality_cost_multiplier("fast-model");
        let mult_slow = msg2_sel.quality_cost_multiplier("slow-model");
        // Bad model should have penalty (mult > 1.0), good model should have bonus (mult < 1.0)
        assert!(
            mult_fast > 1.0,
            "low-quality model should have routing penalty in msg2, got {mult_fast}"
        );
        assert!(
            mult_slow < 1.0,
            "high-quality model should have routing bonus in msg2, got {mult_slow}"
        );
    }

    // ── Phase 5: quality routing strategy ────────────────────────────────────

    #[test]
    fn quality_strategy_selects_highest_avg_reward() {
        // When quality stats show expensive-model has high reward and cheap-model has low reward,
        // routing_bias="quality" should select expensive-model (quality > cost consideration).
        let selector = make_selector(enabled_config());
        // Record multiple outcomes to establish quality signal above the neutral 0.5 prior.
        selector.record_outcome("expensive-model", 0.9, true);
        selector.record_outcome("expensive-model", 0.95, true);
        selector.record_outcome("cheap-model", 0.1, false);
        selector.record_outcome("cheap-model", 0.15, false);
        // mid-model stays at neutral prior (0.5) — expensive-model (0.925) should win.

        let req = make_request("hi");
        let selection = selector.select_model(&req, 0.0, Some("quality")).unwrap();
        assert_eq!(
            selection.model_id, "expensive-model",
            "quality strategy should select model with highest avg_reward, got {}",
            selection.model_id
        );
        assert!(
            selection.reason.contains("quality"),
            "reason should reflect quality strategy, got: {}",
            selection.reason
        );
    }

    #[test]
    fn quality_strategy_no_data_falls_back_to_balance_score() {
        // When no quality data exists (all priors = 0.5), quality strategy should
        // use balance_score as tiebreaker — producing a valid selection (not None).
        let selector = make_selector(enabled_config());
        let req = make_request("hi");
        let selection = selector.select_model(&req, 0.0, Some("quality"));
        assert!(selection.is_some(), "quality strategy with no data should still select a model");
    }

    #[test]
    fn quality_differs_from_balanced_when_stats_exist() {
        // When a high-reward model exists, quality routing should select it
        // even if balanced routing would pick a different model based on cost.
        let selector = make_selector(enabled_config());
        // expensive-model (high cost) gets proven quality signal.
        selector.record_outcome("expensive-model", 0.95, true);
        selector.record_outcome("expensive-model", 0.90, true);
        // cheap-model has bad quality.
        selector.record_outcome("cheap-model", 0.1, false);
        selector.record_outcome("cheap-model", 0.2, false);

        let req = make_request("some query");
        let quality_sel = selector.select_model(&req, 0.0, Some("quality")).unwrap();
        let balanced_sel = selector.select_model(&req, 0.0, Some("balanced")).unwrap();

        // Quality routing picks expensive-model (best proven quality).
        assert_eq!(
            quality_sel.model_id, "expensive-model",
            "quality strategy should select proven high-quality model"
        );
        // Balanced routing should NOT pick expensive-model (too costly even with quality bonus).
        // This verifies quality and balanced strategies can disagree.
        assert_ne!(
            quality_sel.model_id, balanced_sel.model_id,
            "quality and balanced strategies should produce different selections when quality data exists"
        );
    }

    // ── Phase 7: Provider quality gate ───────────────────────────────────────

    #[test]
    fn quality_gate_returns_none_with_no_data() {
        let selector = make_selector(enabled_config());
        // No outcomes recorded — gate cannot fire (insufficient data)
        assert!(
            selector.quality_gate_check(5).is_none(),
            "quality gate must return None when no model has been tracked"
        );
    }

    #[test]
    fn quality_gate_fires_when_all_models_degraded() {
        let selector = make_selector(enabled_config());
        // Record 5 bad outcomes for two models (all below 0.35 threshold)
        for _ in 0..5 {
            selector.record_outcome("cheap-model", 0.10, false);
            selector.record_outcome("mid-model", 0.15, false);
        }
        let warning = selector.quality_gate_check(5);
        assert!(
            warning.is_some(),
            "quality gate must fire when all tracked models have avg_reward < 0.35"
        );
        let msg = warning.unwrap();
        assert!(
            msg.contains("degradation"),
            "warning must mention degradation, got: {msg}"
        );
        assert!(
            msg.contains("Consider switching"),
            "warning must suggest switching providers, got: {msg}"
        );
    }

    #[test]
    fn quality_gate_silent_when_any_model_healthy() {
        let selector = make_selector(enabled_config());
        // cheap-model is degraded (avg ≈ 0.12)
        for _ in 0..5 {
            selector.record_outcome("cheap-model", 0.12, false);
        }
        // expensive-model is healthy (avg ≈ 0.90) — gate must NOT fire
        for _ in 0..5 {
            selector.record_outcome("expensive-model", 0.90, true);
        }
        assert!(
            selector.quality_gate_check(5).is_none(),
            "quality gate must be silent when at least one model is healthy (avg >= 0.35)"
        );
    }

    #[test]
    fn quality_gate_requires_min_interactions() {
        let selector = make_selector(enabled_config());
        // Only 3 bad outcomes — below min_interactions threshold of 5
        for _ in 0..3 {
            selector.record_outcome("cheap-model", 0.05, false);
        }
        assert!(
            selector.quality_gate_check(5).is_none(),
            "quality gate must NOT fire when model has fewer than min_interactions outcomes"
        );
    }

    // ── Phase 8: Consecutive model repetition guard ──────────────────────────

    #[test]
    fn diversity_guard_inactive_before_window_fills() {
        // Less than REPETITION_WINDOW=3 selections → guard cannot fire.
        let selector = make_selector(enabled_config());
        // Give a quality signal to mid-model so the alternative is viable.
        for _ in 0..3 {
            selector.record_outcome("mid-model", 0.9, true);
        }
        let req = make_request("hi");
        // First 2 selections should be cheap-model (Simple → cheap strategy).
        let s1 = selector.select_model(&req, 0.0, None).unwrap();
        let s2 = selector.select_model(&req, 0.0, None).unwrap();
        assert_eq!(s1.model_id, s2.model_id, "first two selections must stay consistent");
        // No diversity guard — both should be cheap-model still.
        assert_eq!(s1.model_id, "cheap-model");
    }

    #[test]
    fn diversity_guard_fires_after_three_consecutive_repetitions() {
        // After 3 consecutive identical selections AND an alternative with avg_reward >= 0.40,
        // the 4th selection should be a different model.
        let selector = make_selector(enabled_config());
        // Seed mid-model with high quality so it passes the DIVERSITY_MIN_REWARD floor.
        for _ in 0..5 {
            selector.record_outcome("mid-model", 0.85, true);
        }
        let req = make_request("hi");
        // Select 3 times to fill the window — all will be cheap-model (Simple → cheap strategy).
        let _s1 = selector.select_model(&req, 0.0, None).unwrap();
        let _s2 = selector.select_model(&req, 0.0, None).unwrap();
        let _s3 = selector.select_model(&req, 0.0, None).unwrap();
        // 4th selection should trigger the guard and break the repetition.
        let s4 = selector.select_model(&req, 0.0, None).unwrap();
        assert_ne!(
            s4.model_id, "cheap-model",
            "4th consecutive cheap-model selection must be broken by diversity guard"
        );
        assert!(
            s4.reason.contains("diversity guard"),
            "reason must indicate diversity guard fired, got: {}",
            s4.reason
        );
    }

    #[test]
    fn diversity_guard_silent_when_no_qualified_alternative() {
        // Guard fires internally but falls back to original when ALL alternatives have
        // avg_reward below DIVERSITY_MIN_REWARD=0.40. Since untracked models have neutral
        // prior 0.5 (>= 0.40), we must explicitly degrade ALL alternatives.
        let selector = make_selector(enabled_config());
        // Record BAD outcomes for ALL non-cheap models (push them below 0.40).
        for _ in 0..5 {
            selector.record_outcome("mid-model", 0.05, false);
            selector.record_outcome("expensive-model", 0.05, false);
            selector.record_outcome("no-tools-model", 0.05, false);
        }
        let req = make_request("hi");
        // Fill window + attempt 4th (no qualified alternative for guard to route to).
        let _s1 = selector.select_model(&req, 0.0, None).unwrap();
        let _s2 = selector.select_model(&req, 0.0, None).unwrap();
        let _s3 = selector.select_model(&req, 0.0, None).unwrap();
        let s4 = selector.select_model(&req, 0.0, None).unwrap();
        // Guard fires internally but finds no alternative above the quality floor → fallback.
        assert_eq!(
            s4.model_id, "cheap-model",
            "guard must fall back to original when no alternative passes quality floor, got {}",
            s4.model_id
        );
        assert!(
            !s4.reason.contains("diversity guard"),
            "reason should NOT mention diversity guard when fallback occurs, got: {}",
            s4.reason
        );
    }
}
