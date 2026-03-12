//! Decision Policy Store — runtime-configurable decision constants.
//!
//! Breaks the hardcoded SLA constants (Fast=4, Balanced=10, Deep=20 rounds)
//! out of `sla_manager.rs` and `sla_router.rs` into values loaded from
//! `PolicyConfig` at agent startup.
//!
//! # Zero-regression guarantee
//! All default values EXACTLY match the current hardcoded literals:
//!   - `sla_manager.rs`: Fast.max_rounds=4, Balanced.max_rounds=10, Deep.max_rounds=20
//!   - `sla_router.rs`: Quick=(4,2,0), Extended=(10,5,1), DeepAnalysis=(20,10,3)
//!
//! Existing configs with no SLA fields will use these defaults unchanged.
//!
//! # Usage
//! ```text
//! let store = PolicyStore::from_config(&policy_config);
//! let params = store.sla_params(RoutingMode::DeepAnalysis);
//! assert_eq!(params.max_rounds, 20); // default
//! ```

use super::sla_router::RoutingMode;
use halcon_core::types::PolicyConfig;
use std::sync::Arc;

// ── SlaParams ─────────────────────────────────────────────────────────────────

/// SLA execution parameters for a specific routing mode.
///
/// Single source of truth — replaces the identical constant tables
/// duplicated in `sla_manager.rs` and `sla_router.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlaParams {
    /// Hard maximum number of agent loop rounds.
    pub max_rounds: u32,
    /// Hard maximum plan depth (step count).
    pub max_plan_depth: u32,
    /// Maximum retries per round.
    pub max_retries: u32,
    /// Whether sub-agent orchestration is recommended.
    pub use_orchestration: bool,
}

// ── DecisionPolicy ────────────────────────────────────────────────────────────

/// Runtime-resolved decision constants.
///
/// Constructed once per agent session from `Arc<PolicyConfig>`.
/// All fields have defaults matching current hardcoded values.
#[derive(Debug, Clone)]
pub struct DecisionPolicy {
    // ── SLA mode budgets ─────────────────────────────────────────────────────
    pub sla_fast_max_rounds: u32,
    pub sla_fast_max_plan_depth: u32,
    pub sla_balanced_max_rounds: u32,
    pub sla_balanced_max_plan_depth: u32,
    pub sla_balanced_max_retries: u32,
    pub sla_deep_max_rounds: u32,
    pub sla_deep_max_plan_depth: u32,
    pub sla_deep_max_retries: u32,

    // ── Intent pipeline ──────────────────────────────────────────────────────
    /// Confidence threshold above which IntentScorer dominates max_rounds.
    pub intent_high_confidence_threshold: f32,
    /// Confidence threshold below which BoundaryDecision dominates max_rounds.
    pub intent_low_confidence_threshold: f32,
    /// Whether to use unified IntentPipeline reconciliation.
    pub use_intent_pipeline: bool,
}

impl Default for DecisionPolicy {
    /// Defaults MUST match current hardcoded values for zero-regression guarantee.
    fn default() -> Self {
        Self {
            sla_fast_max_rounds: 4,
            sla_fast_max_plan_depth: 2,
            sla_balanced_max_rounds: 10,
            sla_balanced_max_plan_depth: 5,
            sla_balanced_max_retries: 1,
            sla_deep_max_rounds: 20,
            sla_deep_max_plan_depth: 10,
            sla_deep_max_retries: 3,
            intent_high_confidence_threshold: 0.75,
            intent_low_confidence_threshold: 0.40,
            use_intent_pipeline: true,
        }
    }
}

impl DecisionPolicy {
    /// Build from `PolicyConfig`, using field defaults for any absent values.
    pub fn from_config(config: &PolicyConfig) -> Self {
        Self {
            sla_fast_max_rounds: config.sla_fast_max_rounds,
            sla_fast_max_plan_depth: config.sla_fast_max_plan_depth,
            sla_balanced_max_rounds: config.sla_balanced_max_rounds,
            sla_balanced_max_plan_depth: config.sla_balanced_max_plan_depth,
            sla_balanced_max_retries: config.sla_balanced_max_retries,
            sla_deep_max_rounds: config.sla_deep_max_rounds,
            sla_deep_max_plan_depth: config.sla_deep_max_plan_depth,
            sla_deep_max_retries: config.sla_deep_max_retries,
            intent_high_confidence_threshold: config.intent_high_confidence_threshold,
            intent_low_confidence_threshold: config.intent_low_confidence_threshold,
            use_intent_pipeline: config.use_intent_pipeline,
        }
    }

    /// SLA parameters for a routing mode.
    pub fn sla_params(&self, mode: RoutingMode) -> SlaParams {
        match mode {
            RoutingMode::Quick => SlaParams {
                max_rounds: self.sla_fast_max_rounds,
                max_plan_depth: self.sla_fast_max_plan_depth,
                max_retries: 0,
                use_orchestration: false,
            },
            RoutingMode::Extended => SlaParams {
                max_rounds: self.sla_balanced_max_rounds,
                max_plan_depth: self.sla_balanced_max_plan_depth,
                max_retries: self.sla_balanced_max_retries,
                use_orchestration: false,
            },
            RoutingMode::DeepAnalysis => SlaParams {
                max_rounds: self.sla_deep_max_rounds,
                max_plan_depth: self.sla_deep_max_plan_depth,
                max_retries: self.sla_deep_max_retries,
                use_orchestration: true,
            },
        }
    }
}

// ── PolicyStore ───────────────────────────────────────────────────────────────

/// Shared, session-scoped policy store.
///
/// Constructed from `Arc<PolicyConfig>` at agent startup. All phase functions
/// receive an `Arc<PolicyStore>` (or borrow from `LoopState.policy_store`) and
/// call `.sla_params()` instead of using literal constants.
#[derive(Debug)]
pub struct PolicyStore {
    pub policy: DecisionPolicy,
}

impl PolicyStore {
    /// Build from a `PolicyConfig` reference.
    pub fn from_config(config: &PolicyConfig) -> Self {
        Self {
            policy: DecisionPolicy::from_config(config),
        }
    }

    /// Build with all-default values (matches current hardcoded behavior exactly).
    pub fn default_store() -> Self {
        Self {
            policy: DecisionPolicy::default(),
        }
    }

    /// SLA parameters for the given routing mode.
    #[inline]
    pub fn sla_params(&self, mode: RoutingMode) -> SlaParams {
        self.policy.sla_params(mode)
    }

    /// Intent pipeline confidence thresholds.
    #[inline]
    pub fn intent_high_confidence(&self) -> f32 {
        self.policy.intent_high_confidence_threshold
    }

    #[inline]
    pub fn intent_low_confidence(&self) -> f32 {
        self.policy.intent_low_confidence_threshold
    }

    #[inline]
    pub fn use_intent_pipeline(&self) -> bool {
        self.policy.use_intent_pipeline
    }
}

impl From<Arc<PolicyConfig>> for PolicyStore {
    fn from(config: Arc<PolicyConfig>) -> Self {
        Self::from_config(&config)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_sla_manager_hardcoded_values() {
        let store = PolicyStore::default_store();

        let fast = store.sla_params(RoutingMode::Quick);
        assert_eq!(fast.max_rounds, 4, "Fast/Quick max_rounds must be 4");
        assert_eq!(
            fast.max_plan_depth, 2,
            "Fast/Quick max_plan_depth must be 2"
        );
        assert_eq!(fast.max_retries, 0);
        assert!(!fast.use_orchestration);

        let balanced = store.sla_params(RoutingMode::Extended);
        assert_eq!(
            balanced.max_rounds, 10,
            "Balanced/Extended max_rounds must be 10"
        );
        assert_eq!(balanced.max_plan_depth, 5);
        assert_eq!(balanced.max_retries, 1);
        assert!(!balanced.use_orchestration);

        let deep = store.sla_params(RoutingMode::DeepAnalysis);
        assert_eq!(deep.max_rounds, 20, "Deep max_rounds must be 20");
        assert_eq!(deep.max_plan_depth, 10);
        assert_eq!(deep.max_retries, 3);
        assert!(deep.use_orchestration);
    }

    #[test]
    fn from_config_reads_policy_fields() {
        let mut config = PolicyConfig::default();
        config.sla_deep_max_rounds = 30; // operator override
        let store = PolicyStore::from_config(&config);
        assert_eq!(store.sla_params(RoutingMode::DeepAnalysis).max_rounds, 30);
    }

    #[test]
    fn from_config_preserves_other_defaults() {
        let config = PolicyConfig::default();
        let store = PolicyStore::from_config(&config);
        // Other modes unchanged when only deep was overridden (default config case).
        assert_eq!(store.sla_params(RoutingMode::Quick).max_rounds, 4);
        assert_eq!(store.sla_params(RoutingMode::Extended).max_rounds, 10);
    }

    #[test]
    fn intent_pipeline_defaults() {
        let store = PolicyStore::default_store();
        assert_eq!(store.intent_high_confidence(), 0.75);
        assert_eq!(store.intent_low_confidence(), 0.40);
        assert!(store.use_intent_pipeline());
    }

    #[test]
    fn policy_from_arc_config() {
        let config = Arc::new(PolicyConfig::default());
        let store = PolicyStore::from(config);
        assert_eq!(store.sla_params(RoutingMode::Quick).max_rounds, 4);
    }
}
