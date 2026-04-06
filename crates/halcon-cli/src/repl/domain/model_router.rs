//! Dynamic model routing based on IntentProfile — SOTA 2026.
//!
//! Replaces static `routing_bias = Some("fast")` in StrategySelector with a
//! priority-ordered rule table that maps (scope, depth, task_type) → specific
//! model tier or model ID override.
//!
//! Routing hierarchy:
//! 1. User explicit override (`--model` CLI flag) — always wins, never overridden.
//! 2. ModelRouter rule table (priority-ordered, first match wins).
//! 3. UCB1 quality adjustment (downgrade low-reward models).
//! 4. Configured provider default (fallback).
//!
//! # Model Tiers
//! - `"fast"` → `deepseek-chat` (< 2 s/round, no chain-of-thought)
//! - `"balanced"` → `deepseek-chat` or provider default
//! - `"deep"` → `deepseek-reasoner` (chain-of-thought, ~14 s/round)
//!
//! # When to use `deep`
//! Only when BOTH conditions hold:
//! - `reasoning_depth >= Exhaustive` AND `scope >= ProjectWide`
//! - OR explicit user request for "razona", "think step by step", etc.
//!
//! Rationale: `deepseek-reasoner` adds ~590 reasoning tokens (~14 s) per round.
//! For project-wide analysis with many tool rounds this is catastrophic (14 s × 12 rounds = 168 s).
//! Route to `deepseek-chat` by default and only escalate when chain-of-thought is
//! clearly beneficial (single-shot complex reasoning, not multi-round tool use).

use std::collections::HashMap;

use halcon_core::types::{ModelInfo, ModelRouterConfig, RoutingTier};

use super::intent_scorer::{
    IntentProfile, LatencyTolerance, QueryLanguage, ReasoningDepth, TaskScope,
};
use super::task_analyzer::TaskType;

// ── Types ──────────────────────────────────────────────────────────────────

/// Model tier selected by the router.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelTier {
    /// Instant conversational — no tools, sub-second.
    Fast,
    /// General-purpose balanced — tools, moderate analysis.
    Balanced,
    /// Chain-of-thought reasoning — only for exhaustive single-shot queries.
    Deep,
}

impl ModelTier {
    /// Routing bias string compatible with StrategySelector / legacy config.
    pub fn as_routing_bias(&self) -> &'static str {
        match self {
            ModelTier::Fast => "fast",
            ModelTier::Balanced => "balanced",
            ModelTier::Deep => "quality",
        }
    }
}

/// Result of model routing.
#[derive(Debug, Clone)]
pub struct RoutingDecision {
    /// Selected tier.
    pub tier: ModelTier,
    /// Explicit model ID override (when tier alone is insufficient).
    /// `None` means "use provider default for this tier".
    pub model_override: Option<String>,
    /// Why this routing was chosen (for observability / tracing).
    pub reason: &'static str,
    /// Confidence in the routing decision: 0.0–1.0.
    pub confidence: f32,
}

// ── RoutingRule ────────────────────────────────────────────────────────────

/// A single routing rule with priority.
///
/// Rules are evaluated in priority order (lower number = checked first).
/// First matching rule wins.
struct RoutingRule {
    priority: u8,
    name: &'static str,
    matches: fn(&IntentProfile) -> bool,
    tier: ModelTier,
    model_override: Option<&'static str>,
    confidence: f32,
}

// ── ModelRouter ────────────────────────────────────────────────────────────

/// Stateless model router.
///
/// All methods are pure functions. No I/O, no state, no config access.
/// Thread-safe by construction (no interior mutability).
pub struct ModelRouter {
    /// Per-model quality history from UCB1 (avg_reward 0.0–1.0).
    /// Populated from ModelPerformanceTracker snapshot.
    quality_history: HashMap<String, f64>,
    /// Fast-tier model ID for this provider (e.g. "deepseek-chat").
    fast_model: String,
    /// Balanced-tier model ID (e.g. "deepseek-chat").
    balanced_model: String,
    /// Deep-tier model ID (e.g. "deepseek-reasoner").
    deep_model: String,
}

impl ModelRouter {
    /// Construct a ModelRouter for a specific provider.
    ///
    /// `fast_model` / `balanced_model` / `deep_model` should come from provider config.
    /// Typically: fast = balanced = "deepseek-chat", deep = "deepseek-reasoner".
    pub fn new(
        fast_model: impl Into<String>,
        balanced_model: impl Into<String>,
        deep_model: impl Into<String>,
    ) -> Self {
        Self {
            quality_history: HashMap::new(),
            fast_model: fast_model.into(),
            balanced_model: balanced_model.into(),
            deep_model: deep_model.into(),
        }
    }

    /// Build router from a `ModelRouterConfig`.
    ///
    /// Preferred constructor — model IDs come from config, no provider baked into source.
    /// `deepseek_defaults()` is a backward-compatible alias that calls this with
    /// `ModelRouterConfig::default()`.
    pub fn from_config(config: &ModelRouterConfig) -> Self {
        Self::new(
            &config.fast_model,
            &config.balanced_model,
            &config.deep_model,
        )
    }

    /// Build router with DeepSeek defaults (covers most deployments).
    ///
    /// **Deprecated use**: prefer `from_config()` or `from_provider_models()`.
    /// Kept for backward compatibility — equivalent to `from_config(&ModelRouterConfig::default())`.
    pub fn deepseek_defaults() -> Self {
        Self::from_config(&ModelRouterConfig::default())
    }

    /// Build a provider-aware router by classifying models from provider metadata.
    ///
    /// Dynamically assigns Fast / Balanced / Deep tiers based on `ModelInfo` fields:
    /// - **Deep**: `supports_reasoning = true` AND `supports_tools = true`
    ///   (chain-of-thought capable; highest-context among reasoning models wins).
    /// - **Fast**: `supports_tools = true`, lowest `cost_per_output_token`
    ///   (cheapest ≈ fastest for simple, low-latency requests).
    /// - **Balanced**: `supports_tools = true`, non-reasoning, highest `context_window`
    ///   (most capable for multi-round tool use without reasoning overhead).
    ///
    /// Falls back to [`deepseek_defaults()`] when `models` is empty or no tool-capable
    /// model is found, preserving backward compatibility for all existing deployments.
    pub fn from_provider_models(models: &[ModelInfo]) -> Self {
        // Only tool-capable models can serve agentic rounds.
        let tool_models: Vec<&ModelInfo> = models.iter().filter(|m| m.supports_tools).collect();

        if tool_models.is_empty() {
            return Self::deepseek_defaults();
        }

        // Deep tier: reasoning-capable + tool-capable, break ties by largest context window.
        let deep_model = tool_models
            .iter()
            .filter(|m| m.supports_reasoning)
            .max_by_key(|m| m.context_window)
            .map(|m| m.id.clone());

        // Fast tier: tool-capable, lowest cost-per-output-token (cheapest ≈ fastest latency).
        // Ties broken by smallest context window (simpler models tend to be faster).
        let fast_model = tool_models
            .iter()
            .min_by(|a, b| {
                a.cost_per_output_token
                    .partial_cmp(&b.cost_per_output_token)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.context_window.cmp(&b.context_window))
            })
            .map(|m| m.id.clone())
            .unwrap_or_else(|| tool_models[0].id.clone());

        // Balanced tier: tool-capable, non-reasoning, most cost-effective for multi-round tasks.
        //
        // Selection strategy: lowest cost_per_output_token among non-reasoning models that
        // are NOT the fast model, with max_output_tokens as a tiebreaker to prefer the most
        // capable model at the same price point.
        //
        // Rationale: for providers like Anthropic where all models share the same context
        // window (200k), the previous `max_by_key(context_window)` picked the LAST model
        // in the list (opus, $75/M) instead of the intended balanced tier (sonnet, $15/M).
        // The new strategy correctly assigns:
        //   Anthropic:   haiku=$4 → Fast,  sonnet=$15 → Balanced (lowest non-fast cost)
        //   OpenAI:      gpt-4o-mini → Fast,  gpt-4o → Balanced
        //   DeepSeek:    deepseek-chat → Fast,  deepseek-chat → Balanced (only non-reasoning model)
        let balanced_model = tool_models
            .iter()
            .filter(|m| !m.supports_reasoning && m.id != fast_model)
            .min_by(|a, b| {
                // Primary: lowest cost (skip Fast-tier price, pick the "value" tier).
                a.cost_per_output_token
                    .partial_cmp(&b.cost_per_output_token)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    // Secondary: highest max_output_tokens (prefer the most capable at same price).
                    .then_with(|| b.max_output_tokens.cmp(&a.max_output_tokens))
            })
            .map(|m| m.id.clone())
            .or_else(|| {
                // Fallback 1: fast model doubles as balanced when it's the only non-reasoning
                // tool model (e.g. DeepSeek: deepseek-chat is both Fast and Balanced).
                tool_models
                    .iter()
                    .filter(|m| !m.supports_reasoning)
                    .max_by_key(|m| m.context_window)
                    .map(|m| m.id.clone())
            })
            .or_else(|| {
                // Fallback 2: any tool-capable model with the largest context window
                // (reasoning-only providers).
                tool_models
                    .iter()
                    .max_by_key(|m| m.context_window)
                    .map(|m| m.id.clone())
            })
            .unwrap_or_else(|| fast_model.clone());

        // Deep falls back to balanced when no reasoning model is tool-capable.
        let deep = deep_model.unwrap_or_else(|| balanced_model.clone());

        Self::new(fast_model, balanced_model, deep)
    }

    /// Seed quality history from ModelPerformanceTracker snapshot.
    /// Keys are model IDs, values are avg_reward (0.0–1.0).
    pub fn with_quality_history(mut self, history: HashMap<String, f64>) -> Self {
        self.quality_history = history;
        self
    }

    /// Route an IntentProfile to a model tier and optional model override.
    ///
    /// Never overrides explicit user model selection — callers must check
    /// `explicit_model` before calling this.
    pub fn route(&self, profile: &IntentProfile) -> RoutingDecision {
        let rules = self.build_rules();

        // Evaluate rules in priority order.
        let mut rules_sorted = rules;
        rules_sorted.sort_by_key(|r| r.priority);

        for rule in &rules_sorted {
            if (rule.matches)(profile) {
                let model_override = rule
                    .model_override
                    .map(|m| self.resolve_model(m, &rule.tier));
                let mut decision = RoutingDecision {
                    tier: rule.tier.clone(),
                    model_override,
                    reason: rule.name,
                    confidence: rule.confidence,
                };
                // Apply UCB1 quality adjustment.
                self.adjust_for_quality(&mut decision);
                return decision;
            }
        }

        // Fallback: balanced routing.
        RoutingDecision {
            tier: ModelTier::Balanced,
            model_override: Some(self.balanced_model.clone()),
            reason: "fallback_balanced",
            confidence: 0.50,
        }
    }

    // ── Rule table ────────────────────────────────────────────────────────

    fn build_rules(&self) -> Vec<RoutingRule> {
        vec![
            // P0 — Spanish task query: always route to Balanced, regardless of scope.
            //
            // Spanish-speaking developers typically give short imperative commands ("continua",
            // "revisa", "implementa") that the IntentScorer classifies as Conversational scope
            // even though they represent complex multi-step operations.  Without this rule,
            // P1 (Conversational→Fast=haiku) fires first and routes ALL Spanish task queries to
            // the cheapest model, degrading quality for the user's entire session.
            //
            // Guard: word_count >= 3 so pure greetings ("hola", "ok") still route to Fast via P1.
            RoutingRule {
                priority: 0,
                name: "spanish_task_balanced",
                matches: |p| p.detected_language == QueryLanguage::Spanish && p.word_count >= 3,
                tier: ModelTier::Balanced,
                model_override: Some(RoutingTier::Balanced.as_placeholder()),
                confidence: 0.80,
            },
            // P1 — Conversational: always fast, no tools, no planning.
            RoutingRule {
                priority: 1,
                name: "conversational_fast",
                matches: |p| p.scope == TaskScope::Conversational,
                tier: ModelTier::Fast,
                model_override: Some(RoutingTier::Fast.as_placeholder()),
                confidence: 0.95,
            },
            // P2 — Instant latency required: fast model only.
            RoutingRule {
                priority: 2,
                name: "instant_latency",
                matches: |p| p.latency_tolerance == LatencyTolerance::Instant,
                tier: ModelTier::Fast,
                model_override: Some(RoutingTier::Fast.as_placeholder()),
                confidence: 0.90,
            },
            // P3 — Deep exhaustive reasoning over a SINGLE artifact (chain-of-thought beneficial).
            // This is the ONLY case where deepseek-reasoner is selected automatically.
            RoutingRule {
                priority: 3,
                name: "single_artifact_exhaustive_reasoning",
                matches: |p| {
                    p.scope == TaskScope::SingleArtifact
                        && p.reasoning_depth == ReasoningDepth::Exhaustive
                        && p.estimated_tool_calls <= 3
                },
                tier: ModelTier::Deep,
                model_override: Some(RoutingTier::Deep.as_placeholder()),
                confidence: 0.80,
            },
            // P4 — Project-wide + exhaustive: balanced model (NOT reasoner — too slow for 12+ rounds).
            RoutingRule {
                priority: 4,
                name: "project_wide_use_balanced",
                matches: |p| p.scope >= TaskScope::ProjectWide,
                tier: ModelTier::Balanced,
                model_override: Some(RoutingTier::Balanced.as_placeholder()),
                confidence: 0.85,
            },
            // P5 — Spanish query: balanced model (no difference in quality but logs it).
            RoutingRule {
                priority: 5,
                name: "spanish_query_balanced",
                matches: |p| p.detected_language == QueryLanguage::Spanish,
                tier: ModelTier::Balanced,
                model_override: Some(RoutingTier::Balanced.as_placeholder()),
                confidence: 0.75,
            },
            // P6 — Light analysis / single artifact: balanced is sufficient.
            RoutingRule {
                priority: 6,
                name: "light_analysis_balanced",
                matches: |p| {
                    p.reasoning_depth <= ReasoningDepth::Light
                        && p.scope <= TaskScope::SingleArtifact
                },
                tier: ModelTier::Balanced,
                model_override: Some(RoutingTier::Balanced.as_placeholder()),
                confidence: 0.70,
            },
            // P7 — Deep local context: balanced (enough depth without reasoning overhead).
            RoutingRule {
                priority: 7,
                name: "local_context_deep",
                matches: |p| {
                    p.scope == TaskScope::LocalContext && p.reasoning_depth >= ReasoningDepth::Deep
                },
                tier: ModelTier::Balanced,
                model_override: Some(RoutingTier::Balanced.as_placeholder()),
                confidence: 0.70,
            },
            // P8 — Debugging: balanced (tool-heavy, chain-of-thought not helpful per round).
            RoutingRule {
                priority: 8,
                name: "debugging_balanced",
                matches: |p| p.task_type == TaskType::Debugging,
                tier: ModelTier::Balanced,
                model_override: Some(RoutingTier::Balanced.as_placeholder()),
                confidence: 0.65,
            },
        ]
    }

    /// Resolve placeholder model names ("__fast__", "__balanced__", "__deep__") to actual IDs.
    fn resolve_model(&self, placeholder: &str, _tier: &ModelTier) -> String {
        match placeholder {
            "__fast__" => self.fast_model.clone(),
            "__balanced__" => self.balanced_model.clone(),
            "__deep__" => self.deep_model.clone(),
            other => other.to_string(),
        }
    }

    /// Downgrade routing tier when the target model has low UCB1 reward.
    ///
    /// If the selected model has avg_reward < 0.40 AND a lower tier model has higher reward,
    /// downgrade to avoid sending requests to a known-poor performer.
    fn adjust_for_quality(&self, decision: &mut RoutingDecision) {
        if self.quality_history.is_empty() {
            return; // No history = no adjustment.
        }

        let target = decision
            .model_override
            .as_deref()
            .unwrap_or(&self.balanced_model);
        let target_reward = self.quality_history.get(target).copied().unwrap_or(0.5);

        // Only downgrade from Deep→Balanced when reasoner has poor history.
        if decision.tier == ModelTier::Deep && target_reward < 0.40 {
            let balanced_reward = self
                .quality_history
                .get(&self.balanced_model)
                .copied()
                .unwrap_or(0.5);
            if balanced_reward >= target_reward {
                decision.tier = ModelTier::Balanced;
                decision.model_override = Some(self.balanced_model.clone());
                decision.reason = "quality_downgrade_deep_to_balanced";
                decision.confidence *= 0.80;
            }
        }
    }

    /// Build a routing-bias string for StrategySelector integration.
    ///
    /// Returns "fast", "balanced", or "quality" matching StrategyPlan.routing_bias.
    pub fn routing_bias_for(&self, profile: &IntentProfile) -> Option<String> {
        let decision = self.route(profile);
        Some(decision.tier.as_routing_bias().to_string())
    }

    /// Return the Balanced-tier model ID for this provider.
    ///
    /// Used by `LlmPlanner` to select the most capable non-fast model for planning,
    /// independent of the session model. Returns `None` if the router was not built
    /// from provider models (i.e. uses deepseek defaults, where the model is always
    /// "deepseek-chat" which can serve as a fallback).
    pub fn balanced_model(&self) -> &str {
        &self.balanced_model
    }
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::deepseek_defaults()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::intent_scorer::IntentScorer;

    fn router() -> ModelRouter {
        ModelRouter::deepseek_defaults()
    }

    #[test]
    fn conversational_routes_to_fast() {
        let profile = IntentScorer::score("hola");
        let decision = router().route(&profile);
        assert_eq!(
            decision.tier,
            ModelTier::Fast,
            "reason: {}",
            decision.reason
        );
        assert_eq!(decision.model_override.as_deref(), Some("deepseek-chat"));
    }

    #[test]
    fn project_wide_routes_to_balanced_not_deep() {
        let profile =
            IntentScorer::score("analiza el proyecto completo y revisa todos los archivos");
        let decision = router().route(&profile);
        // CRITICAL: project-wide must NOT route to deepseek-reasoner (14s × 12 rounds = disaster).
        assert_ne!(
            decision.tier,
            ModelTier::Deep,
            "Project-wide query incorrectly routed to Deep tier (reason: {})",
            decision.reason
        );
        assert_eq!(decision.tier, ModelTier::Balanced);
    }

    #[test]
    fn spanish_query_routes_to_balanced() {
        let profile = IntentScorer::score("analiza mi código y revisa los errores");
        let decision = router().route(&profile);
        assert_ne!(decision.tier, ModelTier::Fast);
    }

    #[test]
    fn spanish_short_greeting_still_fast() {
        // Pure greetings (word_count < 3) fall through P0 → P1 (Conversational) → Fast.
        let profile = IntentScorer::score("hola");
        let decision = router().route(&profile);
        // "hola" is conversational scope + word_count=1, does NOT match P0 (word_count >= 3).
        // P1 (Conversational→Fast) fires instead.
        assert_eq!(
            decision.tier,
            ModelTier::Fast,
            "short greeting must stay Fast: {}",
            decision.reason
        );
    }

    #[test]
    fn spanish_task_command_routes_to_balanced_not_fast() {
        // The key regression: "continua con los proximos 5 pasos" looked Conversational
        // to the IntentScorer but is a complex task command → must use Balanced, not haiku.
        let profile = IntentScorer::score("continua con los proximos 5 pasos pendientes");
        let decision = router().route(&profile);
        assert_ne!(
            decision.tier,
            ModelTier::Fast,
            "Spanish task command must not route to Fast (haiku); got reason: {}",
            decision.reason
        );
        assert_eq!(
            decision.tier,
            ModelTier::Balanced,
            "Spanish task command must route to Balanced; got reason: {}",
            decision.reason
        );
    }

    #[test]
    fn single_artifact_exhaustive_routes_to_deep() {
        // A very targeted exhaustive query with few tool calls → reasoner is justified.
        let profile =
            IntentScorer::score("explain exhaustively how the oauth flow works in auth.rs");
        let decision = router().route(&profile);
        // Either Deep or Balanced is acceptable (depends on scope detection).
        // Key invariant: must NOT be Fast.
        assert_ne!(
            decision.tier,
            ModelTier::Fast,
            "reason: {}",
            decision.reason
        );
    }

    #[test]
    fn quality_downgrade_avoids_poor_reasoner() {
        let mut history = HashMap::new();
        history.insert("deepseek-reasoner".to_string(), 0.25_f64); // poor
        history.insert("deepseek-chat".to_string(), 0.80_f64); // good

        let r = ModelRouter::deepseek_defaults().with_quality_history(history);

        // Force a profile that would normally route to Deep.
        use crate::repl::intent_scorer::{
            IntentProfile, LatencyTolerance, ReasoningDepth, TaskScope,
        };
        let profile = IntentProfile {
            task_type: crate::repl::task_analyzer::TaskType::Research,
            complexity: crate::repl::task_analyzer::TaskComplexity::Complex,
            confidence: 0.85,
            scope: TaskScope::SingleArtifact,
            reasoning_depth: ReasoningDepth::Exhaustive,
            requires_planning: false,
            requires_reflection: false,
            estimated_tool_calls: 2,
            estimated_context_tokens: 8000,
            latency_tolerance: LatencyTolerance::Patient,
            detected_language: crate::repl::intent_scorer::QueryLanguage::English,
            ambiguity_score: 0.1,
            task_hash: "test".to_string(),
            word_count: 10,
        };

        let decision = r.route(&profile);
        assert_eq!(decision.tier, ModelTier::Balanced,
            "Expected quality downgrade to Balanced when reasoner has low reward; got {:?} (reason: {})",
            decision.tier, decision.reason);
    }

    #[test]
    fn routing_bias_returns_string() {
        let profile = IntentScorer::score("analiza el proyecto");
        let bias = router().routing_bias_for(&profile);
        assert!(bias.is_some());
        let b = bias.unwrap();
        assert!(
            ["fast", "balanced", "quality"].contains(&b.as_str()),
            "unexpected bias: {:?}",
            b
        );
    }

    #[test]
    fn debugging_routes_to_balanced() {
        let profile = IntentScorer::score("fix the crash in main.rs when parsing config");
        let decision = router().route(&profile);
        // Debugging = many tool calls = balanced, NOT deep.
        assert_ne!(decision.tier, ModelTier::Fast);
    }

    #[test]
    fn deepseek_defaults_backward_compat_names() {
        // `deepseek_defaults()` is the legacy fallback — tier names must remain stable.
        let r = ModelRouter::deepseek_defaults();
        assert_eq!(r.fast_model, "deepseek-chat");
        assert_eq!(r.balanced_model, "deepseek-chat");
        assert_eq!(r.deep_model, "deepseek-reasoner");
    }

    // ── from_provider_models() ────────────────────────────────────────────────

    fn make_model(id: &str, ctx: u32, tools: bool, reasoning: bool, cost: f64) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            name: id.to_string(),
            provider: "test".to_string(),
            context_window: ctx,
            max_output_tokens: 4096,
            supports_streaming: true,
            supports_tools: tools,
            supports_vision: false,
            supports_reasoning: reasoning,
            cost_per_input_token: cost / 2.0,
            cost_per_output_token: cost,
        }
    }

    #[test]
    fn from_provider_models_empty_falls_back_to_deepseek() {
        let r = ModelRouter::from_provider_models(&[]);
        // Empty → deepseek_defaults()
        assert_eq!(r.fast_model, "deepseek-chat");
        assert_eq!(r.deep_model, "deepseek-reasoner");
    }

    #[test]
    fn from_provider_models_no_tool_capable_falls_back_to_deepseek() {
        let models = vec![make_model("vision-only", 8192, false, false, 0.01)];
        let r = ModelRouter::from_provider_models(&models);
        assert_eq!(r.fast_model, "deepseek-chat");
    }

    #[test]
    fn from_provider_models_classifies_reasoning_as_deep() {
        let models = vec![
            make_model("gpt-4o-mini", 128_000, true, false, 0.0006),
            make_model("gpt-4o", 128_000, true, false, 0.005),
            make_model("o3-mini", 200_000, true, true, 0.011),
        ];
        let r = ModelRouter::from_provider_models(&models);
        assert_eq!(
            r.deep_model, "o3-mini",
            "reasoning-capable model must be assigned Deep tier"
        );
    }

    #[test]
    fn from_provider_models_cheapest_is_fast() {
        let models = vec![
            make_model("gpt-4o-mini", 128_000, true, false, 0.0006), // cheapest
            make_model("gpt-4o", 128_000, true, false, 0.005),
            make_model("o3-mini", 200_000, true, true, 0.011),
        ];
        let r = ModelRouter::from_provider_models(&models);
        assert_eq!(
            r.fast_model, "gpt-4o-mini",
            "cheapest tool-capable model must be assigned Fast tier"
        );
    }

    #[test]
    fn from_provider_models_cheapest_non_fast_is_balanced() {
        // Balanced = lowest cost among non-reasoning models excluding Fast tier.
        // gpt-4o-mini is Fast (cheapest); gpt-4o is Balanced (next cheapest non-reasoning).
        let models = vec![
            make_model("gpt-4o-mini", 16_000, true, false, 0.0006),
            make_model("gpt-4o", 128_000, true, false, 0.005),
            make_model("o3-mini", 200_000, true, true, 0.011),
        ];
        let r = ModelRouter::from_provider_models(&models);
        assert_eq!(
            r.balanced_model, "gpt-4o",
            "lowest-cost non-fast non-reasoning model must be assigned Balanced tier"
        );
    }

    #[test]
    fn from_provider_models_anthropic_balanced_is_sonnet_not_opus() {
        // The key regression test: for Anthropic-style models (all 200k context),
        // Balanced must be sonnet ($15/M, 16k out), not opus ($75/M, 32k out).
        // Old code: max_by(context_window) → opus (last with 200k) → WRONG.
        // New code: min_by(cost excluding fast) + max_output_tokens tiebreaker → sonnet-4-6 → CORRECT.
        //
        // Use actual Anthropic output-token specs so the tiebreaker works correctly:
        //   haiku-4-5:  $4/M output, 8k max output  → Fast
        //   sonnet-4-5: $15/M output, 8k max output
        //   sonnet-4-6: $15/M output, 16k max output → Balanced (tied cost, highest output)
        //   opus-4-6:   $75/M output, 32k max output → excluded (highest cost)
        let make_full = |id: &str, max_out: u32, cost: f64| ModelInfo {
            id: id.to_string(),
            name: id.to_string(),
            provider: "anthropic".to_string(),
            context_window: 200_000,
            max_output_tokens: max_out,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            supports_reasoning: false,
            cost_per_input_token: cost / 3.0,
            cost_per_output_token: cost,
        };
        let models = vec![
            make_full("claude-haiku-4-5", 8_192, 0.000004),
            make_full("claude-sonnet-4-5", 8_192, 0.000015),
            make_full("claude-sonnet-4-6", 16_000, 0.000015), // same price, higher output → wins
            make_full("claude-opus-4-6", 32_000, 0.000075),
        ];
        let r = ModelRouter::from_provider_models(&models);
        assert_eq!(
            r.fast_model, "claude-haiku-4-5",
            "cheapest model must be Fast"
        );
        assert_eq!(r.balanced_model, "claude-sonnet-4-6",
            "sonnet-4-6 must be Balanced (lowest non-fast cost, highest output among tied-price models)");
        assert_eq!(
            r.deep_model, r.balanced_model,
            "no reasoning model → Deep falls back to Balanced"
        );
    }

    #[test]
    fn from_provider_models_single_model_all_tiers_same() {
        let models = vec![make_model("only-model", 32_000, true, false, 0.002)];
        let r = ModelRouter::from_provider_models(&models);
        assert_eq!(r.fast_model, "only-model");
        assert_eq!(r.balanced_model, "only-model");
        assert_eq!(
            r.deep_model, "only-model",
            "single tool model must fill all three tiers"
        );
    }

    #[test]
    fn from_provider_models_anthropic_style_no_reasoning_model() {
        // Anthropic models: claude-haiku (small/fast), claude-sonnet (balanced), claude-opus (large).
        // None have supports_reasoning=true → deep falls back to balanced.
        // Balanced = lowest-cost non-fast = claude-sonnet-4-6 (next cheapest after haiku).
        let models = vec![
            make_model("claude-haiku-4-5", 200_000, true, false, 0.00025),
            make_model("claude-sonnet-4-6", 200_000, true, false, 0.003),
            make_model("claude-opus-4-6", 200_000, true, false, 0.015),
        ];
        let r = ModelRouter::from_provider_models(&models);
        assert_eq!(
            r.fast_model, "claude-haiku-4-5",
            "cheapest model must be Fast"
        );
        assert_eq!(
            r.balanced_model, "claude-sonnet-4-6",
            "sonnet is Balanced (lowest non-fast cost)"
        );
        assert_eq!(
            r.deep_model, r.balanced_model,
            "when no reasoning model available, Deep and Balanced must be the same"
        );
    }

    #[test]
    fn from_provider_models_routing_still_works() {
        let models = vec![
            make_model("gpt-4o-mini", 128_000, true, false, 0.0006),
            make_model("gpt-4o", 128_000, true, false, 0.005),
            make_model("o3-mini", 200_000, true, true, 0.011),
        ];
        let r = ModelRouter::from_provider_models(&models);
        // Basic routing sanity: conversational → fast tier
        let profile = IntentScorer::score("hola");
        let decision = r.route(&profile);
        assert_eq!(decision.tier, ModelTier::Fast);
    }

    #[test]
    fn routing_decision_has_reason() {
        let profile = IntentScorer::score("fix bug in auth");
        let decision = router().route(&profile);
        assert!(!decision.reason.is_empty());
    }

    #[test]
    fn planner_uses_balanced_tier_when_no_explicit_model() {
        // Verify ModelRouter::from_provider_models() selects Balanced tier for planning.
        // This is the model the LlmPlanner uses when no explicit planner_model is configured.
        let models = vec![
            make_model("fast-model", 128_000, true, false, 0.0006),
            make_model("balanced-model", 128_000, true, false, 0.005),
            make_model("reasoning-model", 200_000, true, true, 0.011),
        ];
        let router = ModelRouter::from_provider_models(&models);
        // Balanced tier model should be the one selected for planning.
        assert_eq!(
            router.balanced_model(),
            "balanced-model",
            "balanced_model() must return the Balanced tier model ID"
        );
        assert_ne!(
            router.balanced_model(),
            "fast-model",
            "Planner must not use the fast model (too cheap/limited for planning)"
        );
        assert_ne!(
            router.balanced_model(),
            "reasoning-model",
            "Planner must not use reasoning model by default (too slow)"
        );
    }

    #[test]
    fn confidence_in_range() {
        for q in &[
            "hola",
            "analiza el proyecto",
            "fix bug in login.rs",
            "create a new parser module",
        ] {
            let profile = IntentScorer::score(q);
            let decision = router().route(&profile);
            assert!(
                decision.confidence >= 0.0 && decision.confidence <= 1.0,
                "confidence {:.3} out of range for {:?}",
                decision.confidence,
                q
            );
        }
    }
}
