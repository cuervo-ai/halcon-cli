//! Security and resilience subsystem bundle.
//!
//! Phase 3.1: Groups permission handling, security policies, and resilience
//! management into a single cohesive unit. 3 fields from original Repl struct.

use super::conversational_permission::ConversationalPermissionHandler;
use super::resilience::ResilienceManager;
use super::security::permission_pipeline::PermissionPipeline;

/// Security policies, permission handling, and resilience for the REPL.
///
/// Bundles all security-related subsystems that control:
/// - User permission requests (conversational prompts)
/// - Unified permission pipeline (7-phase cascade)
/// - Retry/timeout/circuit breaker logic
pub struct ReplSecurity {
    /// Conversational permission handler for user confirmation dialogs.
    pub permissions: ConversationalPermissionHandler,

    /// Phase 1: Unified permission pipeline (single authority for all decisions).
    /// Owns DenialTracker so denial state persists across tool invocations.
    pub permission_pipeline: PermissionPipeline,

    /// Resilience manager for retry, timeout, circuit breaker, fallback logic.
    pub resilience: ResilienceManager,
}

impl ReplSecurity {
    /// Construct security bundle with required components.
    pub fn new(
        permissions: ConversationalPermissionHandler,
        permission_pipeline: PermissionPipeline,
        resilience: ResilienceManager,
    ) -> Self {
        Self {
            permissions,
            permission_pipeline,
            resilience,
        }
    }
}

impl Default for ReplSecurity {
    fn default() -> Self {
        Self {
            permissions: ConversationalPermissionHandler::new(false), // auto_approve=false
            permission_pipeline: PermissionPipeline::new(),
            resilience: ResilienceManager::new(Default::default()), // ResilienceConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_bundle_default_construction() {
        let security = ReplSecurity::default();

        // Should construct without panicking
        assert!(true, "Security bundle constructed successfully");
    }

    #[test]
    fn security_bundle_custom_construction() {
        let permissions = ConversationalPermissionHandler::new(true);
        let pipeline = PermissionPipeline::new();
        let resilience = ResilienceManager::new(halcon_core::types::ResilienceConfig::default());

        let security = ReplSecurity::new(permissions, pipeline, resilience);

        // Custom construction should work
        assert!(true, "Security bundle custom construction successful");
    }
}
