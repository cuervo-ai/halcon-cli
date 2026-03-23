//! Intelligent model router for Halcon providers.
//!
//! Selects the optimal provider+model combination for each request based on
//! a pipeline of routing strategies:
//!
//! 1. **ForceOverride** — honour explicit `--provider`/`--model` flags.
//! 2. **BudgetConstraint** — downgrade to Economy when cost budget is low.
//! 3. **LatencyConstraint** — use Fast tier when SLA demands < 500 ms TTFT.
//! 4. **IntentBased** — classify the user query and map intent → tier.
//!
//! A **FallbackChain** is applied post-decision to reroute away from unhealthy
//! providers without retrying the strategy pipeline.
//!
//! # Usage
//!
//! ```rust,ignore
//! use halcon_providers::router::{IntelligentRouter, RoutingRequest};
//!
//! let router = IntelligentRouter::default();
//! let decision = router.route(&RoutingRequest {
//!     messages: &messages,
//!     tenant_tier: "enterprise",
//!     ..Default::default()
//! });
//! println!("→ {} / {}", decision.provider, decision.model);
//! ```

mod intent;
mod policy;

pub use intent::{IntentClassifier, TaskIntent};
pub use policy::{RoutingDecision, RoutingTierExt};

use policy::{
    BudgetConstraintStrategy, FallbackChain, ForceOverrideStrategy, IntentBasedStrategy,
    LatencyConstraintStrategy, ProviderHealth, RoutingContext, RoutingStrategy,
};

/// A routing request — all fields are optional except `messages`.
#[derive(Debug, Default)]
pub struct RoutingRequest<'a> {
    /// Conversation messages (at minimum the latest user turn).
    pub messages: &'a [halcon_core::types::ChatMessage],
    /// Tenant subscription tier: `"enterprise"`, `"standard"`, or `"trial"`.
    pub tenant_tier: &'a str,
    /// Explicit provider override (from `--provider` flag).
    pub force_provider: Option<&'a str>,
    /// Explicit model override (from `--model` flag).
    pub force_model: Option<&'a str>,
    /// Maximum acceptable time-to-first-token in milliseconds.
    pub latency_sla_ms: Option<u32>,
    /// Remaining cost budget in USD.  `None` = unlimited.
    pub cost_budget_remaining: Option<f64>,
}

/// Intelligent router — composes a strategy pipeline to select provider+model.
///
/// Constructed with `IntelligentRouter::default()` for production use.
/// Tests can inject a custom `ProviderHealth` to simulate unhealthy providers.
pub struct IntelligentRouter {
    classifier: IntentClassifier,
    health: ProviderHealth,
}

impl Default for IntelligentRouter {
    fn default() -> Self {
        Self {
            classifier: IntentClassifier::new(),
            health: ProviderHealth::all_healthy(),
        }
    }
}

impl IntelligentRouter {
    /// Create with a custom health snapshot (useful for testing).
    pub fn with_health(health: ProviderHealth) -> Self {
        Self {
            classifier: IntentClassifier::new(),
            health,
        }
    }

    /// Route a request through the strategy pipeline and return the decision.
    ///
    /// This method is synchronous — the intent classifier uses regex only
    /// (no LLM call) and returns in < 1 µs.
    pub fn route(&self, req: &RoutingRequest<'_>) -> RoutingDecision {
        // Extract the last user message text for intent classification.
        let last_user_text = req
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, halcon_core::types::Role::User))
            .and_then(|m| m.content.as_text())
            .unwrap_or("");

        let intent = self.classifier.classify(last_user_text);

        let ctx = RoutingContext {
            intent: &intent,
            tenant_tier: req.tenant_tier,
            force_provider: req.force_provider,
            force_model: req.force_model,
            latency_sla_ms: req.latency_sla_ms,
            cost_budget_remaining: req.cost_budget_remaining,
        };

        // Run strategy pipeline — first non-None claim wins.
        let strategies: &[&dyn RoutingStrategy] = &[
            &ForceOverrideStrategy,
            &BudgetConstraintStrategy,
            &LatencyConstraintStrategy,
            &IntentBasedStrategy,
        ];

        let mut decision = strategies
            .iter()
            .find_map(|s| s.route(&ctx))
            .expect("IntentBasedStrategy always returns a decision");

        // Apply fallback chain if the primary provider is unhealthy.
        decision = FallbackChain::apply(decision, &self.health);
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::{ChatMessage, MessageContent, Role};

    fn user_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
        }
    }

    #[test]
    fn routes_code_generation_to_flagship() {
        let router = IntelligentRouter::default();
        let msgs = [user_msg("Write a Rust function that parses JSON")];
        let decision = router.route(&RoutingRequest {
            messages: &msgs,
            tenant_tier: "enterprise",
            ..Default::default()
        });
        assert_eq!(decision.tier.as_str(), "flagship");
        assert!(!decision.provider.is_empty());
    }

    #[test]
    fn routes_summarization_to_economy() {
        let router = IntelligentRouter::default();
        let msgs = [user_msg("Summarize this document in three bullet points")];
        let decision = router.route(&RoutingRequest {
            messages: &msgs,
            tenant_tier: "enterprise",
            ..Default::default()
        });
        assert_eq!(decision.tier.as_str(), "economy");
    }

    #[test]
    fn force_override_takes_precedence() {
        let router = IntelligentRouter::default();
        let msgs = [user_msg("Summarize this")];
        let decision = router.route(&RoutingRequest {
            messages: &msgs,
            tenant_tier: "enterprise",
            force_provider: Some("openai"),
            force_model: Some("gpt-4o"),
            ..Default::default()
        });
        assert_eq!(decision.provider, "openai");
        assert_eq!(decision.model, "gpt-4o");
    }

    #[test]
    fn budget_constraint_downgrades_to_economy() {
        let router = IntelligentRouter::default();
        let msgs = [user_msg("Write a complex algorithm")];
        let decision = router.route(&RoutingRequest {
            messages: &msgs,
            tenant_tier: "enterprise",
            cost_budget_remaining: Some(0.05), // below threshold
            ..Default::default()
        });
        assert_eq!(decision.tier.as_str(), "economy");
    }

    #[test]
    fn trial_tenant_capped_at_balanced() {
        let router = IntelligentRouter::default();
        let msgs = [user_msg("Implement a full OAuth server")];
        let decision = router.route(&RoutingRequest {
            messages: &msgs,
            tenant_tier: "trial",
            ..Default::default()
        });
        // trial tenants cannot use flagship
        assert_ne!(decision.tier.as_str(), "flagship");
    }

    #[test]
    fn fallback_applied_when_primary_unhealthy() {
        let health = ProviderHealth::with_unhealthy(&["anthropic"]);
        let router = IntelligentRouter::with_health(health);
        let msgs = [user_msg("Write me a poem")];
        let decision = router.route(&RoutingRequest {
            messages: &msgs,
            tenant_tier: "enterprise",
            ..Default::default()
        });
        // anthropic is unhealthy → must route elsewhere
        assert_ne!(decision.provider, "anthropic");
    }
}
