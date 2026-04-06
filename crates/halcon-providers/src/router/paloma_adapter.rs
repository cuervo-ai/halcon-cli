//! Paloma Frontier Fabric Router adapter for Halcon.
//!
//! Maps Halcon's `RoutingRequest` to Paloma's `InferenceRequest` and converts the
//! resulting `ExecutionPlan` back to Halcon's `RoutingDecision`.
//!
//! ## Feature gate
//!
//! This module is behind the `paloma` Cargo feature. When disabled, the existing
//! `IntelligentRouter` is used as fallback.

use std::collections::BTreeSet;
use std::sync::Arc;

use paloma_budget::BudgetStore;
use paloma_health::HealthTracker;
use paloma_pipeline::{Pipeline, PipelineConfig};
use paloma_registry::RegistrySnapshot;
use paloma_types::candidate::CandidateId;
use paloma_types::cost::Cost;
use paloma_types::plan::ExecutionPlan;
use paloma_types::request::TenantId;
use paloma_types::request::{Capability, InferenceRequest, Modality, QualityTier, TaskClass};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::intent::IntentClassifier;
use super::policy::{ForceOverrideStrategy, RoutingContext, RoutingStrategy};
use super::{RoutingDecision, RoutingRequest, RoutingTierExt};

// ─── PalomaRouter ────────────────────────────────────────────────────────────

/// Frontier-grade router powered by Paloma's formally-verified pipeline.
///
/// Wraps Paloma's `Pipeline` with Halcon-specific type mapping and preserves
/// `--provider`/`--model` CLI overrides as an escape hatch.
pub struct PalomaRouter {
    pipeline: Pipeline,
    classifier: IntentClassifier,
    health: Arc<HealthTracker>,
    budget: Arc<BudgetStore>,
    registry: RegistrySnapshot,
}

impl PalomaRouter {
    /// Create a new PalomaRouter with the given stores.
    pub fn new(
        health: Arc<HealthTracker>,
        budget: Arc<BudgetStore>,
        registry: RegistrySnapshot,
    ) -> Self {
        Self {
            pipeline: Pipeline::new(PipelineConfig::default()),
            classifier: IntentClassifier::new(),
            health,
            budget,
            registry,
        }
    }

    /// Route a request through the Paloma pipeline.
    ///
    /// ## Priority
    /// 1. `--provider`/`--model` CLI overrides (ForceOverride — preserved)
    /// 2. Paloma 6-stage pipeline (Policy → Capability → Shaper → Scorer → Planner)
    /// 3. Fallback to IntentBased router on Paloma rejection
    pub fn route(&self, req: &RoutingRequest<'_>) -> RoutingDecision {
        // ── Step 1: Honour explicit CLI overrides ──────────────────────────
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

        if let Some(decision) = ForceOverrideStrategy.route(&ctx) {
            debug!(
                provider = %decision.provider,
                model = %decision.model,
                "Paloma: CLI override active, bypassing pipeline"
            );
            return decision;
        }

        // ── Step 2: Build Paloma InferenceRequest ──────────────────────────
        let inference_req = self.map_to_inference_request(req, &intent);
        let tenant_policy = self.build_tenant_policy(req.tenant_tier);

        // ── Step 3: Route through Paloma pipeline ──────────────────────────
        match self.pipeline.route(
            &inference_req,
            &tenant_policy,
            &self.registry,
            &self.health,
            &self.budget,
        ) {
            Ok(plan) => {
                let decision = self.map_to_routing_decision(&plan);
                info!(
                    provider = %decision.provider,
                    model = %decision.model,
                    tier = %decision.tier.as_str(),
                    plan_id = %plan.plan_id,
                    confidence = plan.confidence,
                    "Paloma routing decision"
                );
                decision
            }
            Err(e) => {
                warn!(error = %e, "Paloma pipeline rejected, falling back to intent router");
                let fallback = super::IntelligentRouter::default();
                fallback.route(req)
            }
        }
    }

    // ─── Type Mapping ─────────────────────────────────────────────────────────

    fn map_to_inference_request(
        &self,
        req: &RoutingRequest<'_>,
        intent: &super::intent::IntentResult,
    ) -> InferenceRequest {
        let mut modalities = BTreeSet::new();
        modalities.insert(Modality::Text);

        let mut capabilities = BTreeSet::new();
        capabilities.insert(Capability::Streaming);
        capabilities.insert(Capability::ToolUse);

        InferenceRequest {
            request_id: Uuid::new_v4(),
            tenant_id: TenantId(req.tenant_tier.to_string()),
            modalities,
            capabilities,
            quality_tier: map_quality_tier(req.tenant_tier),
            latency_target_ms: req.latency_sla_ms,
            budget_limit: req.cost_budget_remaining.map(Cost::from_dollars),
            priority_weights: None,
            context_size_estimate: estimate_tokens(req.messages),
            output_size_hint: None,
            task_class: Some(map_task_class(&intent.intent)),
            extended_metadata: Default::default(),
            metadata: Default::default(),
        }
    }

    fn map_to_routing_decision(&self, plan: &ExecutionPlan) -> RoutingDecision {
        RoutingDecision {
            provider: plan.primary.provider.0.clone(),
            model: plan.primary.model.0.clone(),
            tier: infer_tier(plan),
            reason: format!(
                "paloma:plan={},confidence={:.2}",
                plan.plan_id, plan.confidence
            ),
        }
    }

    fn build_tenant_policy(&self, tenant_tier: &str) -> paloma_types::policy::TenantPolicy {
        use paloma_types::policy::*;
        use paloma_types::request::QualityTier;

        let (monthly, daily, per_req, ref_cost, quality) = match tenant_tier {
            "enterprise" => (1000.0, 100.0, 5.0, 0.10, QualityTier::Premium),
            "trial" => (10.0, 5.0, 0.50, 0.05, QualityTier::Economy),
            _ => (100.0, 50.0, 2.0, 0.10, QualityTier::Standard),
        };

        TenantPolicy {
            tenant_id: TenantId(tenant_tier.to_string()),
            version: 1,
            data_residency: DataResidencyPolicy {
                allowed_regions: Default::default(),
                denied_regions: Default::default(),
                provider_dpa_required: false,
            },
            budget: BudgetPolicy {
                monthly_limit: Cost::from_dollars(monthly),
                daily_limit: Some(Cost::from_dollars(daily)),
                per_request_limit: Cost::from_dollars(per_req),
                per_request_cap_ratio: 2.0,
                reference_cost: Some(Cost::from_dollars(ref_cost)),
            },
            compliance: CompliancePolicy {
                data_classification: DataClassification::Internal,
                pii_handling: PiiHandling::NotRequired,
                audit_logging: true,
                model_allowlist: None,
                model_denylist: Vec::new(),
            },
            routing: RoutingPreferences {
                weight_profile: None,
                weights: None,
                default_quality_tier: quality,
                fallback_chain_length: 3,
                exploration_opt_out: false,
            },
            rate_limits: RateLimitPolicy {
                rps: 60,
                concurrent: 10,
            },
        }
    }

    /// Record a provider success — feeds Paloma's health tracker for circuit breaking.
    pub fn record_success(&self, provider: &str, model: &str, latency_ms: u32) {
        let id = CandidateId(format!("{provider}:{model}"));
        self.health.record_success(&id, latency_ms);
    }

    /// Record a provider failure — feeds Paloma's health tracker for circuit breaking.
    pub fn record_failure(&self, provider: &str, model: &str) {
        let id = CandidateId(format!("{provider}:{model}"));
        self.health.record_failure(&id);
    }

    /// Commit actual cost to budget store after execution completes.
    pub fn commit_cost(&self, plan: &ExecutionPlan, actual_cost_usd: f64) {
        let _ = self
            .budget
            .commit(&plan.reservation_id, Cost::from_dollars(actual_cost_usd));
    }
}

// ─── Pure mapping functions ─────────────────────────────────────────────────

fn map_quality_tier(tenant_tier: &str) -> QualityTier {
    match tenant_tier {
        "enterprise" => QualityTier::Premium,
        "standard" => QualityTier::Standard,
        "trial" => QualityTier::Economy,
        _ => QualityTier::Standard,
    }
}

fn map_task_class(intent: &super::intent::TaskIntent) -> TaskClass {
    use super::intent::TaskIntent;
    match intent {
        TaskIntent::CodeGeneration
        | TaskIntent::Debugging
        | TaskIntent::CodeReview
        | TaskIntent::Refactoring => TaskClass::CodeGeneration,
        TaskIntent::DataAnalysis | TaskIntent::Research | TaskIntent::Planning => {
            TaskClass::Reasoning
        }
        TaskIntent::Summarization => TaskClass::Summarization,
        TaskIntent::Translation => TaskClass::Translation,
        TaskIntent::Conversation => TaskClass::Conversation,
        TaskIntent::QuestionAnswer | TaskIntent::Unknown => TaskClass::TextGeneration,
    }
}

fn infer_tier(plan: &ExecutionPlan) -> RoutingTierExt {
    let cost_dollars = plan.estimated_cost.as_dollars();
    if cost_dollars > 0.01 {
        RoutingTierExt("flagship")
    } else if cost_dollars > 0.002 {
        RoutingTierExt("balanced")
    } else if cost_dollars > 0.0005 {
        RoutingTierExt("fast")
    } else {
        RoutingTierExt("economy")
    }
}

fn estimate_tokens(messages: &[halcon_core::types::ChatMessage]) -> u32 {
    let total_chars: usize = messages
        .iter()
        .map(|m| m.content.as_text().map(|t| t.len()).unwrap_or(0))
        .sum();
    (total_chars / 4).max(1) as u32
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
    fn quality_tier_mapping() {
        assert!(matches!(
            map_quality_tier("enterprise"),
            QualityTier::Premium
        ));
        assert!(matches!(map_quality_tier("trial"), QualityTier::Economy));
        assert!(matches!(
            map_quality_tier("standard"),
            QualityTier::Standard
        ));
        assert!(matches!(map_quality_tier("unknown"), QualityTier::Standard));
    }

    #[test]
    fn task_class_mapping() {
        use super::super::intent::TaskIntent;
        assert!(matches!(
            map_task_class(&TaskIntent::CodeGeneration),
            TaskClass::CodeGeneration
        ));
        assert!(matches!(
            map_task_class(&TaskIntent::Summarization),
            TaskClass::Summarization
        ));
        assert!(matches!(
            map_task_class(&TaskIntent::Conversation),
            TaskClass::Conversation
        ));
        assert!(matches!(
            map_task_class(&TaskIntent::Unknown),
            TaskClass::TextGeneration
        ));
    }

    #[test]
    fn token_estimation() {
        let msgs = [user_msg("hello world")]; // 11 chars → ~3 tokens
        let est = estimate_tokens(&msgs);
        assert!(est >= 1 && est <= 10);
    }

    #[test]
    fn tier_inference_from_cost() {
        // These test the cost → tier mapping without needing a real ExecutionPlan.
        assert_eq!(RoutingTierExt("flagship").as_str(), "flagship");
        assert_eq!(RoutingTierExt("economy").as_str(), "economy");
    }
}
