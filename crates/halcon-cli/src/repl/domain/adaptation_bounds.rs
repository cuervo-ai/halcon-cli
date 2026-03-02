//! Bounded Adaptation Guarantees — Formal limits on runtime self-modification (P4.5).
//!
//! Ensures the agent system cannot infinitely self-modify. Every adaptive behavior
//! (replanning, strategy mutation, parameter shift, model downgrade) has a hard
//! per-session budget. When a budget is exhausted, the corresponding adaptation
//! channel is frozen for the remainder of the session.
//!
//! # Budgets
//!
//! | Channel | Default | PolicyConfig field |
//! |---------|---------|-------------------|
//! | Structural replans | 4 | `max_structural_replans` |
//! | Strategy mutations | 6 | `max_strategy_mutations` |
//! | Sensitivity shift | 0.50 | `max_sensitivity_shift` |
//! | Model downgrades | 2 | `max_model_downgrades` |
//!
//! # Formal Property
//!
//! For any session S: `∀ channel C: usage(C, S) ≤ budget(C, policy)`
//!
//! Pure business logic — no I/O.

use std::sync::Arc;

use halcon_core::types::PolicyConfig;

// ── AdaptationChannel ────────────────────────────────────────────────────────

/// Identifies an adaptation channel subject to bounded guarantees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdaptationChannel {
    /// Structural replans (convergence_phase Replan arm).
    StructuralReplan,
    /// Strategy mutations from MidLoopStrategy (P3.1).
    StrategyMutation,
    /// Cumulative sensitivity shift from AdaptivePolicy.
    SensitivityShift,
    /// Model downgrade decisions.
    ModelDowngrade,
}

impl AdaptationChannel {
    /// Short label for logging.
    pub fn label(self) -> &'static str {
        match self {
            Self::StructuralReplan => "replan",
            Self::StrategyMutation => "strategy-mutation",
            Self::SensitivityShift => "sensitivity-shift",
            Self::ModelDowngrade => "model-downgrade",
        }
    }
}

// ── BudgetStatus ─────────────────────────────────────────────────────────────

/// Status of a single adaptation channel's budget.
#[derive(Debug, Clone)]
pub struct BudgetStatus {
    pub channel: AdaptationChannel,
    /// How much has been used.
    pub used: f64,
    /// Maximum allowed by policy.
    pub budget: f64,
    /// Whether the budget is exhausted.
    pub exhausted: bool,
}

// ── AdaptationBoundsChecker ──────────────────────────────────────────────────

/// Stateful tracker that enforces bounded adaptation guarantees.
pub struct AdaptationBoundsChecker {
    /// Structural replans consumed.
    replans_used: u32,
    /// Strategy mutations consumed.
    mutations_used: u32,
    /// Cumulative sensitivity shift magnitude.
    sensitivity_shift_used: f64,
    /// Model downgrades consumed.
    downgrades_used: u32,
    /// Policy reference for budget limits.
    policy: Arc<PolicyConfig>,
}

impl AdaptationBoundsChecker {
    /// Create a new checker with zero usage.
    pub fn new(policy: Arc<PolicyConfig>) -> Self {
        Self {
            replans_used: 0,
            mutations_used: 0,
            sensitivity_shift_used: 0.0,
            downgrades_used: 0,
            policy,
        }
    }

    /// Attempt to use a structural replan. Returns true if allowed, false if budget exhausted.
    pub fn try_replan(&mut self) -> bool {
        if self.replans_used < self.policy.max_structural_replans {
            self.replans_used += 1;
            true
        } else {
            false
        }
    }

    /// Attempt to use a strategy mutation. Returns true if allowed.
    pub fn try_mutation(&mut self) -> bool {
        if self.mutations_used < self.policy.max_strategy_mutations {
            self.mutations_used += 1;
            true
        } else {
            false
        }
    }

    /// Attempt to accumulate a sensitivity shift. Returns true if within budget.
    pub fn try_sensitivity_shift(&mut self, delta: f64) -> bool {
        let new_total = self.sensitivity_shift_used + delta.abs();
        if new_total <= self.policy.max_sensitivity_shift {
            self.sensitivity_shift_used = new_total;
            true
        } else {
            false
        }
    }

    /// Attempt a model downgrade. Returns true if allowed.
    pub fn try_downgrade(&mut self) -> bool {
        if self.downgrades_used < self.policy.max_model_downgrades {
            self.downgrades_used += 1;
            true
        } else {
            false
        }
    }

    /// Check if a specific channel is exhausted.
    pub fn is_exhausted(&self, channel: AdaptationChannel) -> bool {
        match channel {
            AdaptationChannel::StructuralReplan => {
                self.replans_used >= self.policy.max_structural_replans
            }
            AdaptationChannel::StrategyMutation => {
                self.mutations_used >= self.policy.max_strategy_mutations
            }
            AdaptationChannel::SensitivityShift => {
                self.sensitivity_shift_used >= self.policy.max_sensitivity_shift
            }
            AdaptationChannel::ModelDowngrade => {
                self.downgrades_used >= self.policy.max_model_downgrades
            }
        }
    }

    /// Get status for all channels.
    pub fn all_status(&self) -> Vec<BudgetStatus> {
        vec![
            BudgetStatus {
                channel: AdaptationChannel::StructuralReplan,
                used: self.replans_used as f64,
                budget: self.policy.max_structural_replans as f64,
                exhausted: self.is_exhausted(AdaptationChannel::StructuralReplan),
            },
            BudgetStatus {
                channel: AdaptationChannel::StrategyMutation,
                used: self.mutations_used as f64,
                budget: self.policy.max_strategy_mutations as f64,
                exhausted: self.is_exhausted(AdaptationChannel::StrategyMutation),
            },
            BudgetStatus {
                channel: AdaptationChannel::SensitivityShift,
                used: self.sensitivity_shift_used,
                budget: self.policy.max_sensitivity_shift,
                exhausted: self.is_exhausted(AdaptationChannel::SensitivityShift),
            },
            BudgetStatus {
                channel: AdaptationChannel::ModelDowngrade,
                used: self.downgrades_used as f64,
                budget: self.policy.max_model_downgrades as f64,
                exhausted: self.is_exhausted(AdaptationChannel::ModelDowngrade),
            },
        ]
    }

    /// Number of channels that are exhausted.
    pub fn exhausted_count(&self) -> usize {
        self.all_status().iter().filter(|s| s.exhausted).count()
    }

    /// Summary string for logging.
    pub fn summary(&self) -> String {
        let statuses = self.all_status();
        let parts: Vec<String> = statuses
            .iter()
            .map(|s| format!("{}={:.1}/{:.0}", s.channel.label(), s.used, s.budget))
            .collect();
        format!("adaptation: [{}]", parts.join(", "))
    }

    /// Usage fraction for a specific channel [0.0, 1.0+].
    pub fn usage_fraction(&self, channel: AdaptationChannel) -> f64 {
        match channel {
            AdaptationChannel::StructuralReplan => {
                if self.policy.max_structural_replans == 0 { return 1.0; }
                self.replans_used as f64 / self.policy.max_structural_replans as f64
            }
            AdaptationChannel::StrategyMutation => {
                if self.policy.max_strategy_mutations == 0 { return 1.0; }
                self.mutations_used as f64 / self.policy.max_strategy_mutations as f64
            }
            AdaptationChannel::SensitivityShift => {
                if self.policy.max_sensitivity_shift <= 0.0 { return 1.0; }
                self.sensitivity_shift_used / self.policy.max_sensitivity_shift
            }
            AdaptationChannel::ModelDowngrade => {
                if self.policy.max_model_downgrades == 0 { return 1.0; }
                self.downgrades_used as f64 / self.policy.max_model_downgrades as f64
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_checker() -> AdaptationBoundsChecker {
        AdaptationBoundsChecker::new(Arc::new(PolicyConfig::default()))
    }

    // ── Structural replans ───────────────────────────────────────────────

    #[test]
    fn phase4_bounds_replan_within_budget() {
        let mut checker = make_checker();
        // Default max_structural_replans = 4
        assert!(checker.try_replan());
        assert!(checker.try_replan());
        assert!(checker.try_replan());
        assert!(checker.try_replan());
        assert!(!checker.try_replan(), "5th replan should be blocked");
    }

    #[test]
    fn phase4_bounds_replan_exhausted() {
        let mut checker = make_checker();
        for _ in 0..4 { checker.try_replan(); }
        assert!(checker.is_exhausted(AdaptationChannel::StructuralReplan));
    }

    // ── Strategy mutations ───────────────────────────────────────────────

    #[test]
    fn phase4_bounds_mutation_within_budget() {
        let mut checker = make_checker();
        // Default max_strategy_mutations = 6
        for _ in 0..6 {
            assert!(checker.try_mutation());
        }
        assert!(!checker.try_mutation(), "7th mutation should be blocked");
    }

    // ── Sensitivity shift ────────────────────────────────────────────────

    #[test]
    fn phase4_bounds_sensitivity_shift_cumulative() {
        let mut checker = make_checker();
        // Default max_sensitivity_shift = 0.50
        assert!(checker.try_sensitivity_shift(0.20));
        assert!(checker.try_sensitivity_shift(0.20));
        // Total now 0.40, next 0.15 would be 0.55 > 0.50
        assert!(!checker.try_sensitivity_shift(0.15), "would exceed budget");
        assert!(checker.try_sensitivity_shift(0.10), "0.50 total should still fit");
    }

    #[test]
    fn phase4_bounds_sensitivity_absolute_value() {
        let mut checker = make_checker();
        // Negative deltas should use absolute value
        assert!(checker.try_sensitivity_shift(-0.20));
        assert!((checker.usage_fraction(AdaptationChannel::SensitivityShift) - 0.40).abs() < 0.01);
    }

    // ── Model downgrades ─────────────────────────────────────────────────

    #[test]
    fn phase4_bounds_downgrade_within_budget() {
        let mut checker = make_checker();
        // Default max_model_downgrades = 2
        assert!(checker.try_downgrade());
        assert!(checker.try_downgrade());
        assert!(!checker.try_downgrade(), "3rd downgrade should be blocked");
    }

    // ── Cross-channel independence ───────────────────────────────────────

    #[test]
    fn phase4_bounds_channels_independent() {
        let mut checker = make_checker();
        // Exhaust replans
        for _ in 0..4 { checker.try_replan(); }
        assert!(checker.is_exhausted(AdaptationChannel::StructuralReplan));
        // Other channels should still be available
        assert!(!checker.is_exhausted(AdaptationChannel::StrategyMutation));
        assert!(!checker.is_exhausted(AdaptationChannel::SensitivityShift));
        assert!(!checker.is_exhausted(AdaptationChannel::ModelDowngrade));
        assert!(checker.try_mutation());
        assert!(checker.try_downgrade());
    }

    // ── Status and summary ───────────────────────────────────────────────

    #[test]
    fn phase4_bounds_all_status() {
        let mut checker = make_checker();
        checker.try_replan();
        checker.try_mutation();
        checker.try_mutation();
        let statuses = checker.all_status();
        assert_eq!(statuses.len(), 4);
        assert!(!statuses[0].exhausted); // replan: 1/4
        assert!(!statuses[1].exhausted); // mutation: 2/6
        assert!(!statuses[2].exhausted); // sensitivity: 0/0.5
        assert!(!statuses[3].exhausted); // downgrade: 0/2
    }

    #[test]
    fn phase4_bounds_exhausted_count() {
        let mut checker = make_checker();
        for _ in 0..4 { checker.try_replan(); }
        for _ in 0..2 { checker.try_downgrade(); }
        assert_eq!(checker.exhausted_count(), 2);
    }

    #[test]
    fn phase4_bounds_usage_fraction() {
        let mut checker = make_checker();
        checker.try_replan(); // 1/4 = 0.25
        let frac = checker.usage_fraction(AdaptationChannel::StructuralReplan);
        assert!((frac - 0.25).abs() < 1e-4, "expected 0.25, got {frac}");
    }

    #[test]
    fn phase4_bounds_usage_fraction_zero_budget() {
        let mut policy = PolicyConfig::default();
        policy.max_structural_replans = 0;
        let checker = AdaptationBoundsChecker::new(Arc::new(policy));
        // Zero budget → fraction = 1.0 (exhausted immediately)
        assert_eq!(checker.usage_fraction(AdaptationChannel::StructuralReplan), 1.0);
    }

    #[test]
    fn phase4_bounds_summary_contains_all_channels() {
        let checker = make_checker();
        let summary = checker.summary();
        assert!(summary.contains("replan="));
        assert!(summary.contains("strategy-mutation="));
        assert!(summary.contains("sensitivity-shift="));
        assert!(summary.contains("model-downgrade="));
    }

    #[test]
    fn phase4_bounds_channel_labels_unique() {
        let channels = [
            AdaptationChannel::StructuralReplan,
            AdaptationChannel::StrategyMutation,
            AdaptationChannel::SensitivityShift,
            AdaptationChannel::ModelDowngrade,
        ];
        let labels: Vec<&str> = channels.iter().map(|c| c.label()).collect();
        let unique: std::collections::HashSet<&str> = labels.iter().copied().collect();
        assert_eq!(labels.len(), unique.len());
    }
}
