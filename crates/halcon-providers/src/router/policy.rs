//! Routing strategy pipeline and decision types.
//!
//! Each `RoutingStrategy` either claims routing (returns `Some(RoutingDecision)`)
//! or passes to the next strategy (returns `None`).  The pipeline runs strategies
//! in order; the first claim wins.  A `FallbackChain` is applied after the
//! primary decision to reroute away from unhealthy providers.
//!
//! ## Tier mapping (2026 defaults)
//!
//! | Tier     | Provider    | Model                      | Cost/1K tokens (output) |
//! |----------|-------------|----------------------------|-------------------------|
//! | FLAGSHIP | anthropic   | claude-opus-4-6            | $0.015                  |
//! | BALANCED | anthropic   | claude-sonnet-4-6          | $0.003                  |
//! | FAST     | anthropic   | claude-haiku-4-5-20251001  | $0.00025                |
//! | ECONOMY  | ollama      | llama3.2:3b                | $0.000 (local)          |

use std::collections::HashSet;

use super::intent::{IntentResult, TaskIntent};

// ── RoutingDecision ──────────────────────────────────────────────────────────

/// The result of routing — provider + model + metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingDecision {
    pub provider: String,
    pub model: String,
    /// Tier string: `"flagship"`, `"balanced"`, `"fast"`, or `"economy"`.
    pub tier: RoutingTierExt,
    /// Human-readable explanation (for tracing/debugging).
    pub reason: String,
}

/// Extended tier type for routing decisions.
///
/// Separate from `halcon_core::types::RoutingTier` to avoid coupling the
/// provider crate to the core type for an internal routing concern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingTierExt(pub &'static str);

impl RoutingTierExt {
    pub fn flagship() -> Self {
        Self("flagship")
    }
    pub fn balanced() -> Self {
        Self("balanced")
    }
    pub fn fast() -> Self {
        Self("fast")
    }
    pub fn economy() -> Self {
        Self("economy")
    }

    pub fn as_str(&self) -> &str {
        self.0
    }
}

// ── Provider → model defaults per tier ──────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct TierDefaults {
    provider: &'static str,
    model: &'static str,
    tier: &'static str,
}

const FLAGSHIP: TierDefaults = TierDefaults {
    provider: "anthropic",
    model: "claude-opus-4-6",
    tier: "flagship",
};
const BALANCED: TierDefaults = TierDefaults {
    provider: "anthropic",
    model: "claude-sonnet-4-6",
    tier: "balanced",
};
const FAST: TierDefaults = TierDefaults {
    provider: "anthropic",
    model: "claude-haiku-4-5-20251001",
    tier: "fast",
};
const ECONOMY: TierDefaults = TierDefaults {
    provider: "ollama",
    model: "llama3.2:3b",
    tier: "economy",
};

fn decision_for(defaults: TierDefaults, reason: impl Into<String>) -> RoutingDecision {
    RoutingDecision {
        provider: defaults.provider.to_string(),
        model: defaults.model.to_string(),
        tier: RoutingTierExt(defaults.tier),
        reason: reason.into(),
    }
}

// ── RoutingContext ────────────────────────────────────────────────────────────

/// All inputs available to routing strategies.
pub struct RoutingContext<'a> {
    pub intent: &'a IntentResult,
    pub tenant_tier: &'a str,
    pub force_provider: Option<&'a str>,
    pub force_model: Option<&'a str>,
    pub latency_sla_ms: Option<u32>,
    pub cost_budget_remaining: Option<f64>,
}

// ── RoutingStrategy trait ────────────────────────────────────────────────────

pub trait RoutingStrategy: Send + Sync {
    /// Return a routing decision or `None` to pass to the next strategy.
    fn route(&self, ctx: &RoutingContext<'_>) -> Option<RoutingDecision>;
}

// ── Strategies ───────────────────────────────────────────────────────────────

/// Honours explicit `--provider` / `--model` overrides.
pub struct ForceOverrideStrategy;

impl RoutingStrategy for ForceOverrideStrategy {
    fn route(&self, ctx: &RoutingContext<'_>) -> Option<RoutingDecision> {
        let provider = ctx.force_provider?;
        let model = ctx.force_model?;
        Some(RoutingDecision {
            provider: provider.to_string(),
            model: model.to_string(),
            tier: RoutingTierExt::balanced(), // tier unknown for manual overrides
            reason: "force_override".to_string(),
        })
    }
}

/// Downgrades to Economy tier when the cost budget is nearly exhausted.
///
/// Threshold: < $0.10 remaining → Economy.
pub struct BudgetConstraintStrategy;

impl RoutingStrategy for BudgetConstraintStrategy {
    fn route(&self, ctx: &RoutingContext<'_>) -> Option<RoutingDecision> {
        const THRESHOLD: f64 = 0.10;
        let budget = ctx.cost_budget_remaining?;
        if budget >= THRESHOLD {
            return None;
        }
        Some(decision_for(ECONOMY, "budget_exhausted"))
    }
}

/// Uses Fast tier when the caller needs sub-500 ms time-to-first-token.
pub struct LatencyConstraintStrategy;

impl RoutingStrategy for LatencyConstraintStrategy {
    fn route(&self, ctx: &RoutingContext<'_>) -> Option<RoutingDecision> {
        const FAST_SLA_MS: u32 = 500;
        let sla = ctx.latency_sla_ms?;
        if sla > FAST_SLA_MS {
            return None;
        }
        Some(decision_for(FAST, "latency_sla"))
    }
}

/// Maps task intent → routing tier, respecting tenant capability caps.
pub struct IntentBasedStrategy;

impl RoutingStrategy for IntentBasedStrategy {
    fn route(&self, ctx: &RoutingContext<'_>) -> Option<RoutingDecision> {
        let tier = intent_to_tier(ctx.intent.intent);

        // Trial tenants are capped at BALANCED — they cannot reach FLAGSHIP.
        let tier = if ctx.tenant_tier == "trial" && tier.tier == "flagship" {
            BALANCED
        } else {
            tier
        };

        let reason = format!("intent:{}", ctx.intent.intent.as_str());
        Some(decision_for(tier, reason))
    }
}

fn intent_to_tier(intent: TaskIntent) -> TierDefaults {
    match intent {
        // High reasoning demand → Flagship
        TaskIntent::CodeGeneration
        | TaskIntent::CodeReview
        | TaskIntent::Debugging
        | TaskIntent::Refactoring => FLAGSHIP,

        // Moderate reasoning → Balanced
        TaskIntent::DataAnalysis
        | TaskIntent::Research
        | TaskIntent::Planning
        | TaskIntent::QuestionAnswer
        | TaskIntent::Unknown => BALANCED,

        // Cheap, repetitive → Economy
        TaskIntent::Summarization | TaskIntent::Translation => ECONOMY,

        // Very short, interactive → Fast
        TaskIntent::Conversation => FAST,
    }
}

// ── Provider health ──────────────────────────────────────────────────────────

/// Snapshot of provider health used by the fallback chain.
///
/// In production this is updated by a background health-check loop.
/// In tests it can be constructed directly.
#[derive(Debug, Clone)]
pub struct ProviderHealth {
    unhealthy: HashSet<String>,
}

impl ProviderHealth {
    /// All providers are healthy (production default).
    pub fn all_healthy() -> Self {
        Self {
            unhealthy: HashSet::new(),
        }
    }

    /// Construct with a set of unhealthy provider names (for testing).
    pub fn with_unhealthy(providers: &[&str]) -> Self {
        Self {
            unhealthy: providers.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn is_healthy(&self, provider: &str) -> bool {
        !self.unhealthy.contains(provider)
    }
}

// ── FallbackChain ─────────────────────────────────────────────────────────────

/// Ordered fallback list per provider.  First healthy provider in the list wins.
const FALLBACKS: &[(&str, &[&str])] = &[
    ("anthropic", &["openai", "ollama"]),
    ("openai", &["anthropic", "ollama"]),
    ("ollama", &["anthropic"]),
    ("cenzontle", &["anthropic", "openai", "ollama"]),
];

/// Default model for a fallback provider.
fn fallback_model(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "claude-sonnet-4-6",
        "openai" => "gpt-4o-mini",
        "ollama" => "llama3.2:3b",
        _ => "gpt-4o-mini",
    }
}

pub struct FallbackChain;

impl FallbackChain {
    /// Apply the fallback chain to `decision` if the primary provider is unhealthy.
    ///
    /// Returns the original decision if the primary is healthy, or the first
    /// healthy fallback otherwise.  If all fallbacks are unhealthy, returns
    /// the original decision (fail-open — callers handle connection errors).
    pub fn apply(decision: RoutingDecision, health: &ProviderHealth) -> RoutingDecision {
        if health.is_healthy(&decision.provider) {
            return decision;
        }

        let fallbacks = FALLBACKS
            .iter()
            .find(|(p, _)| *p == decision.provider.as_str())
            .map(|(_, fb)| *fb)
            .unwrap_or(&[]);

        for fallback in fallbacks {
            if health.is_healthy(fallback) {
                return RoutingDecision {
                    provider: fallback.to_string(),
                    model: fallback_model(fallback).to_string(),
                    tier: decision.tier.clone(),
                    reason: format!("fallback_from:{}", decision.provider),
                };
            }
        }

        // All fallbacks unhealthy — return original and let the caller surface the error.
        decision
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::super::intent::{IntentClassifier, IntentResult, TaskIntent};
    use super::*;

    fn intent(task: TaskIntent) -> IntentResult {
        IntentResult {
            intent: task,
            confidence: 0.85,
            method: "test",
        }
    }

    fn ctx_for(task: TaskIntent) -> RoutingContext<'static> {
        // Leak the IntentResult so it lives long enough for the static lifetime.
        // Only done in tests.
        let r: &'static IntentResult = Box::leak(Box::new(intent(task)));
        RoutingContext {
            intent: r,
            tenant_tier: "enterprise",
            force_provider: None,
            force_model: None,
            latency_sla_ms: None,
            cost_budget_remaining: None,
        }
    }

    #[test]
    fn force_override_claims_routing() {
        let ctx = RoutingContext {
            intent: Box::leak(Box::new(intent(TaskIntent::CodeGeneration))),
            tenant_tier: "enterprise",
            force_provider: Some("openai"),
            force_model: Some("gpt-4o"),
            latency_sla_ms: None,
            cost_budget_remaining: None,
        };
        let decision = ForceOverrideStrategy.route(&ctx).unwrap();
        assert_eq!(decision.provider, "openai");
        assert_eq!(decision.model, "gpt-4o");
        assert_eq!(decision.reason, "force_override");
    }

    #[test]
    fn force_override_passes_without_both_fields() {
        let ctx = RoutingContext {
            intent: Box::leak(Box::new(intent(TaskIntent::Unknown))),
            tenant_tier: "enterprise",
            force_provider: Some("openai"),
            force_model: None, // model missing → pass
            latency_sla_ms: None,
            cost_budget_remaining: None,
        };
        assert!(ForceOverrideStrategy.route(&ctx).is_none());
    }

    #[test]
    fn budget_constraint_fires_below_threshold() {
        let ctx = RoutingContext {
            intent: Box::leak(Box::new(intent(TaskIntent::CodeGeneration))),
            tenant_tier: "enterprise",
            force_provider: None,
            force_model: None,
            latency_sla_ms: None,
            cost_budget_remaining: Some(0.05),
        };
        let decision = BudgetConstraintStrategy.route(&ctx).unwrap();
        assert_eq!(decision.tier.as_str(), "economy");
        assert_eq!(decision.reason, "budget_exhausted");
    }

    #[test]
    fn budget_constraint_passes_above_threshold() {
        let ctx = RoutingContext {
            intent: Box::leak(Box::new(intent(TaskIntent::CodeGeneration))),
            tenant_tier: "enterprise",
            force_provider: None,
            force_model: None,
            latency_sla_ms: None,
            cost_budget_remaining: Some(5.0),
        };
        assert!(BudgetConstraintStrategy.route(&ctx).is_none());
    }

    #[test]
    fn latency_constraint_fires_at_or_below_500ms() {
        let ctx = RoutingContext {
            intent: Box::leak(Box::new(intent(TaskIntent::Research))),
            tenant_tier: "enterprise",
            force_provider: None,
            force_model: None,
            latency_sla_ms: Some(300),
            cost_budget_remaining: None,
        };
        let decision = LatencyConstraintStrategy.route(&ctx).unwrap();
        assert_eq!(decision.tier.as_str(), "fast");
    }

    #[test]
    fn intent_based_code_generation_is_flagship() {
        let d = IntentBasedStrategy
            .route(&ctx_for(TaskIntent::CodeGeneration))
            .unwrap();
        assert_eq!(d.tier.as_str(), "flagship");
        assert_eq!(d.provider, "anthropic");
    }

    #[test]
    fn intent_based_summarization_is_economy() {
        let d = IntentBasedStrategy
            .route(&ctx_for(TaskIntent::Summarization))
            .unwrap();
        assert_eq!(d.tier.as_str(), "economy");
    }

    #[test]
    fn trial_tenant_capped_at_balanced() {
        let r: &'static IntentResult = Box::leak(Box::new(intent(TaskIntent::CodeGeneration)));
        let ctx = RoutingContext {
            intent: r,
            tenant_tier: "trial",
            force_provider: None,
            force_model: None,
            latency_sla_ms: None,
            cost_budget_remaining: None,
        };
        let d = IntentBasedStrategy.route(&ctx).unwrap();
        assert_ne!(
            d.tier.as_str(),
            "flagship",
            "trial tenants must not use flagship"
        );
        assert_eq!(d.tier.as_str(), "balanced");
    }

    #[test]
    fn fallback_chain_reroutes_unhealthy_primary() {
        let health = ProviderHealth::with_unhealthy(&["anthropic"]);
        let decision = RoutingDecision {
            provider: "anthropic".to_string(),
            model: "claude-opus-4-6".to_string(),
            tier: RoutingTierExt::flagship(),
            reason: "intent:code_generation".to_string(),
        };
        let result = FallbackChain::apply(decision, &health);
        assert_ne!(result.provider, "anthropic");
        assert!(result.reason.starts_with("fallback_from:"));
    }

    #[test]
    fn fallback_chain_preserves_healthy_primary() {
        let health = ProviderHealth::all_healthy();
        let decision = RoutingDecision {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            tier: RoutingTierExt::balanced(),
            reason: "intent:research".to_string(),
        };
        let result = FallbackChain::apply(decision.clone(), &health);
        assert_eq!(result, decision);
    }
}
