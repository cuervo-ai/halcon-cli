//! Plugin permission gate — evaluates whether a plugin capability may be invoked.
//!
//! Decisions are based on:
//! 1. The tool's declared [`RiskTier`]
//! 2. The session's configured `global_max_risk` ceiling
//! 3. Per-plugin supervisor restrictions (can only tighten, never relax)
//! 4. Whether the cost budget is exhausted

use std::collections::HashMap;
use super::manifest::{RiskTier, ToolCapabilityDescriptor};

// ─── Decision ─────────────────────────────────────────────────────────────────

/// Access decision for a single plugin capability invocation attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginPermissionDecision {
    /// Proceed immediately — no confirmation needed.
    Allowed,
    /// Tool is risky enough to require an explicit user confirmation step.
    NeedsConfirmation,
    /// Invocation denied; the gate returns a synthetic error result.
    Denied { reason: String },
}

// ─── Gate ─────────────────────────────────────────────────────────────────────

/// Stateful permission gate for all plugins in a session.
pub struct PluginPermissionGate {
    /// Session-wide maximum allowed risk tier.
    global_max_risk: RiskTier,
    /// Per-plugin tighter restrictions imposed by the supervisor.
    supervisor_restrictions: HashMap<String, RiskTier>,
}

impl PluginPermissionGate {
    /// Create a new gate with the given session-wide risk ceiling.
    pub fn new(global_max_risk: RiskTier) -> Self {
        Self {
            global_max_risk,
            supervisor_restrictions: HashMap::new(),
        }
    }

    /// Default gate: allows up to `High` risk (requires confirmation), blocks `Critical`.
    pub fn default_permissive() -> Self {
        Self::new(RiskTier::High)
    }

    /// Evaluate whether `capability` of `plugin_id` may be invoked.
    ///
    /// - `budget_low`: when true, any call is denied to prevent cost overshoot.
    pub fn evaluate(
        &self,
        plugin_id: &str,
        capability: &ToolCapabilityDescriptor,
        budget_low: bool,
    ) -> PluginPermissionDecision {
        // Budget exhaustion — deny all plugin calls
        if budget_low {
            return PluginPermissionDecision::Denied {
                reason: format!(
                    "plugin '{}' budget exhausted — invocation denied",
                    plugin_id
                ),
            };
        }

        // Determine effective ceiling: the tighter of global and per-plugin restriction
        let effective_ceiling = self
            .supervisor_restrictions
            .get(plugin_id)
            .copied()
            .map(|r| r.min(self.global_max_risk))
            .unwrap_or(self.global_max_risk);

        // Critical is always denied regardless of configuration
        if capability.risk_tier == RiskTier::Critical {
            return PluginPermissionDecision::Denied {
                reason: format!(
                    "plugin '{}' tool '{}' has Critical risk tier — always denied",
                    plugin_id, capability.name
                ),
            };
        }

        // Tool exceeds effective ceiling
        if capability.risk_tier > effective_ceiling {
            return PluginPermissionDecision::Denied {
                reason: format!(
                    "plugin '{}' tool '{}' risk tier {:?} exceeds session ceiling {:?}",
                    plugin_id, capability.name, capability.risk_tier, effective_ceiling
                ),
            };
        }

        // High risk (within ceiling) → requires explicit confirmation
        if capability.risk_tier == RiskTier::High {
            return PluginPermissionDecision::NeedsConfirmation;
        }

        PluginPermissionDecision::Allowed
    }

    /// Apply a supervisor restriction to a specific plugin.
    ///
    /// Restrictions can only tighten permissions (lower `max_risk`).
    /// Calling this twice keeps the tighter of the two values.
    pub fn supervisor_restrict(&mut self, plugin_id: &str, max_risk: RiskTier) {
        let entry = self
            .supervisor_restrictions
            .entry(plugin_id.to_string())
            .or_insert(max_risk);
        // Keep the more restrictive value
        if max_risk < *entry {
            *entry = max_risk;
        }
    }

    /// Remove a supervisor-imposed restriction for a plugin.
    ///
    /// After lifting, the plugin reverts to the global ceiling.
    pub fn lift_restriction(&mut self, plugin_id: &str) {
        self.supervisor_restrictions.remove(plugin_id);
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::PermissionLevel;

    fn cap(name: &str, risk: RiskTier) -> ToolCapabilityDescriptor {
        ToolCapabilityDescriptor {
            name: name.to_string(),
            description: "test capability".into(),
            risk_tier: risk,
            idempotent: true,
            permission_level: PermissionLevel::ReadOnly,
            budget_tokens_per_call: 0,
        }
    }

    #[test]
    fn low_risk_allowed() {
        let gate = PluginPermissionGate::default_permissive();
        let result = gate.evaluate("plugin-a", &cap("tool", RiskTier::Low), false);
        assert_eq!(result, PluginPermissionDecision::Allowed);
    }

    #[test]
    fn high_risk_needs_confirmation_within_ceiling() {
        let gate = PluginPermissionGate::default_permissive(); // ceiling = High
        let result = gate.evaluate("plugin-a", &cap("tool", RiskTier::High), false);
        assert_eq!(result, PluginPermissionDecision::NeedsConfirmation);
    }

    #[test]
    fn critical_always_denied() {
        let gate = PluginPermissionGate::new(RiskTier::Critical);
        let result = gate.evaluate("plugin-a", &cap("tool", RiskTier::Critical), false);
        assert!(matches!(result, PluginPermissionDecision::Denied { .. }));
    }

    #[test]
    fn budget_low_denies_any_risk() {
        let gate = PluginPermissionGate::default_permissive();
        let result = gate.evaluate("plugin-a", &cap("tool", RiskTier::Low), true);
        assert!(matches!(result, PluginPermissionDecision::Denied { reason } if reason.contains("budget exhausted")));
    }

    #[test]
    fn supervisor_restriction_tightens_ceiling() {
        let mut gate = PluginPermissionGate::default_permissive(); // global = High
        gate.supervisor_restrict("plugin-a", RiskTier::Low); // restrict to Low
        // Medium now exceeds the per-plugin ceiling
        let result = gate.evaluate("plugin-a", &cap("tool", RiskTier::Medium), false);
        assert!(matches!(result, PluginPermissionDecision::Denied { .. }));
    }

    #[test]
    fn lift_restriction_reverts_to_global() {
        let mut gate = PluginPermissionGate::default_permissive();
        gate.supervisor_restrict("plugin-a", RiskTier::Low);
        gate.lift_restriction("plugin-a");
        // Medium should now be Allowed again (within global High ceiling)
        let result = gate.evaluate("plugin-a", &cap("tool", RiskTier::Medium), false);
        assert_eq!(result, PluginPermissionDecision::Allowed);
    }
}
