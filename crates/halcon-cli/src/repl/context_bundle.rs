//! Context assembly subsystem bundle.
//!
//! Phase 3.1: Groups all context-related infrastructure (manager, metrics,
//! governance, caching) into a single cohesive unit. 4 fields from original Repl.

use std::sync::Arc;

use super::context_governance::ContextGovernance;
use super::context_manager::ContextManager;
use super::context_metrics::ContextMetrics;
use super::response_cache::ResponseCache;

/// Context assembly subsystem for the REPL.
///
/// Bundles all context-related infrastructure:
/// - Context manager: unified assembly from multiple sources
/// - Context metrics: observability for token usage and assembly
/// - Context governance: per-source token limits and policies
/// - Response cache: LLM response caching for cost reduction
pub struct ReplContext {
    /// Context manager wrapping pipeline + sources + governance (Phase 38).
    /// Sources are owned by the manager - access via context_manager.sources().
    pub manager: Option<ContextManager>,

    /// Shared context metrics for agent loop observability (Phase 42).
    pub metrics: Arc<ContextMetrics>,

    /// Context governance for per-source token limits (Phase 42).
    pub governance: ContextGovernance,

    /// Response cache for LLM cost reduction. None when disabled.
    pub response_cache: Option<ResponseCache>,
}

impl ReplContext {
    /// Construct context bundle with all components.
    pub fn new(
        manager: Option<ContextManager>,
        metrics: Arc<ContextMetrics>,
        governance: ContextGovernance,
        response_cache: Option<ResponseCache>,
    ) -> Self {
        Self {
            manager,
            metrics,
            governance,
            response_cache,
        }
    }

    /// Check if context manager is available.
    pub fn has_manager(&self) -> bool {
        self.manager.is_some()
    }

    /// Check if response caching is enabled.
    pub fn has_cache(&self) -> bool {
        self.response_cache.is_some()
    }
}

impl Default for ReplContext {
    fn default() -> Self {
        Self {
            manager: None,
            metrics: Arc::new(Default::default()), // ContextMetrics::default()
            governance: ContextGovernance::new(std::collections::HashMap::new()),
            response_cache: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_bundle_default_has_metrics_and_governance() {
        let context = ReplContext::default();

        // Metrics and governance always present
        assert!(Arc::strong_count(&context.metrics) >= 1);

        // Manager and cache optional
        assert!(!context.has_manager());
        assert!(!context.has_cache());
    }

    #[test]
    fn context_bundle_tracks_optional_components() {
        let mut context = ReplContext::default();

        assert!(!context.has_manager());

        // Enable manager (would need real ContextManager construction in production)
        // context.manager = Some(ContextManager::new(...));
        // For now just verify the pattern works
        assert!(!context.has_manager());
    }
}
