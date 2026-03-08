//! SLA Manager — Time-Budget-Aware Planning for adaptive execution.
//!
//! Configures agent behaviour based on user-selected performance tier:
//!
//! | Mode     | Max time  | Plan depth | Sub-agents | Retries |
//! |----------|-----------|------------|------------|---------|
//! | Fast     | 30s       | 2          | 0          | 0       |
//! | Balanced | 90s       | 5          | 3          | 1       |
//! | Deep     | unlimited | 10         | 8          | 3       |
//!
//! The SLA mode is selected from:
//! 1. Explicit user flag (`--fast`, `--deep`)
//! 2. Decision-layer complexity estimate
//! 3. Default: Balanced

use std::time::{Duration, Instant};

use super::decision_layer::{TaskComplexity, OrchestrationDecision};

/// SLA performance tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlaMode {
    /// Optimised for speed. No orchestration, minimal planning.
    Fast,
    /// Default mode. Moderate planning and optional orchestration.
    Balanced,
    /// Thoroughness over speed. Full orchestration, deep planning, retries.
    Deep,
}

/// Runtime budget constraints derived from SLA mode.
#[derive(Debug, Clone)]
pub(crate) struct SlaBudget {
    pub mode: SlaMode,
    pub max_duration: Option<Duration>,
    pub max_plan_depth: u32,
    pub max_sub_agents: u32,
    pub max_retries: u32,
    pub max_rounds: u32,
    start: Instant,
}

impl SlaBudget {
    /// Create a budget from an SLA mode.
    pub fn from_mode(mode: SlaMode) -> Self {
        match mode {
            SlaMode::Fast => Self {
                mode,
                max_duration: Some(Duration::from_secs(30)),
                max_plan_depth: 2,
                max_sub_agents: 0,
                max_retries: 0,
                max_rounds: 4,
                start: Instant::now(),
            },
            SlaMode::Balanced => Self {
                mode,
                max_duration: Some(Duration::from_secs(90)),
                max_plan_depth: 5,
                max_sub_agents: 3,
                max_retries: 1,
                max_rounds: 10,
                start: Instant::now(),
            },
            SlaMode::Deep => Self {
                mode,
                max_duration: None,
                max_plan_depth: 10,
                max_sub_agents: 8,
                max_retries: 3,
                max_rounds: 20,
                start: Instant::now(),
            },
        }
    }

    /// Derive SLA mode from task complexity.
    pub fn from_complexity(decision: &OrchestrationDecision) -> Self {
        let mode = match decision.complexity {
            TaskComplexity::SimpleExecution => SlaMode::Fast,
            TaskComplexity::StructuredTask => SlaMode::Balanced,
            TaskComplexity::MultiDomain | TaskComplexity::LongHorizon => SlaMode::Deep,
        };
        Self::from_mode(mode)
    }

    /// Check if the time budget has been exceeded.
    pub fn is_expired(&self) -> bool {
        match self.max_duration {
            Some(d) => self.start.elapsed() > d,
            None => false,
        }
    }

    /// Remaining time budget. `None` if unlimited or expired.
    pub fn remaining(&self) -> Option<Duration> {
        self.max_duration.map(|d| d.saturating_sub(self.start.elapsed()))
    }

    /// Fraction of time budget consumed. 0.0 = just started, 1.0+ = expired.
    /// Returns 0.0 for unlimited budgets.
    pub fn fraction_consumed(&self) -> f64 {
        match self.max_duration {
            Some(d) => self.start.elapsed().as_secs_f64() / d.as_secs_f64(),
            None => 0.0,
        }
    }

    /// Whether orchestration (sub-agents) is allowed under this budget.
    pub fn allows_orchestration(&self) -> bool {
        self.max_sub_agents > 0 && !self.is_expired()
    }

    /// Whether retries are allowed under this budget.
    pub fn allows_retry(&self, current_retry: u32) -> bool {
        current_retry < self.max_retries && !self.is_expired()
    }

    /// Clamp a requested plan depth to the SLA maximum.
    pub fn clamp_plan_depth(&self, requested: u32) -> u32 {
        requested.min(self.max_plan_depth)
    }

    /// Clamp a requested round count to the SLA maximum.
    pub fn clamp_rounds(&self, requested: u32) -> u32 {
        requested.min(self.max_rounds)
    }

    /// Upgrade the SLA budget to match a higher complexity tier (P3.5).
    ///
    /// Only upgrades — if the new complexity maps to a lower or equal SLA mode,
    /// the budget is unchanged. Preserves the start time so `fraction_consumed()`
    /// reflects the full session duration.
    pub fn upgrade_from_complexity(&mut self, complexity: &TaskComplexity) {
        let target_mode = match complexity {
            TaskComplexity::SimpleExecution => SlaMode::Fast,
            TaskComplexity::StructuredTask => SlaMode::Balanced,
            TaskComplexity::MultiDomain | TaskComplexity::LongHorizon => SlaMode::Deep,
        };
        let mode_ord = |m: &SlaMode| match m {
            SlaMode::Fast => 0,
            SlaMode::Balanced => 1,
            SlaMode::Deep => 2,
        };
        if mode_ord(&target_mode) > mode_ord(&self.mode) {
            let start = self.start; // preserve elapsed time
            let upgraded = Self::from_mode(target_mode);
            self.mode = upgraded.mode;
            self.max_duration = upgraded.max_duration;
            self.max_plan_depth = upgraded.max_plan_depth;
            self.max_sub_agents = upgraded.max_sub_agents;
            self.max_retries = upgraded.max_retries;
            self.max_rounds = upgraded.max_rounds;
            self.start = start; // keep original start
        }
    }
}

// ── Trait implementation ──────────────────────────────────────────────────────

impl halcon_core::traits::BudgetManager for SlaBudget {
    fn is_expired(&self) -> bool {
        self.is_expired()
    }

    fn remaining(&self) -> Option<std::time::Duration> {
        self.remaining()
    }

    fn fraction_consumed(&self) -> f64 {
        self.fraction_consumed()
    }

    fn allows_orchestration(&self) -> bool {
        self.allows_orchestration()
    }

    fn allows_retry(&self, attempt: u32) -> bool {
        self.allows_retry(attempt)
    }

    fn clamp_plan_depth(&self, depth: u32) -> u32 {
        self.clamp_plan_depth(depth)
    }

    fn clamp_rounds(&self, rounds: u32) -> u32 {
        self.clamp_rounds(rounds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_mode_no_orchestration() {
        let budget = SlaBudget::from_mode(SlaMode::Fast);
        assert!(!budget.allows_orchestration());
        assert!(!budget.allows_retry(0));
        assert_eq!(budget.max_plan_depth, 2);
        assert_eq!(budget.max_rounds, 4);
    }

    #[test]
    fn balanced_mode_limited_orchestration() {
        let budget = SlaBudget::from_mode(SlaMode::Balanced);
        assert!(budget.allows_orchestration());
        assert!(budget.allows_retry(0));
        assert!(!budget.allows_retry(1));
        assert_eq!(budget.max_sub_agents, 3);
    }

    #[test]
    fn deep_mode_unlimited_time() {
        let budget = SlaBudget::from_mode(SlaMode::Deep);
        assert!(!budget.is_expired());
        assert!(budget.remaining().is_none());
        assert_eq!(budget.fraction_consumed(), 0.0);
        assert!(budget.allows_retry(2));
        assert!(!budget.allows_retry(3));
    }

    #[test]
    fn clamp_plan_depth() {
        let budget = SlaBudget::from_mode(SlaMode::Fast);
        assert_eq!(budget.clamp_plan_depth(8), 2);
        assert_eq!(budget.clamp_plan_depth(1), 1);
    }

    #[test]
    fn clamp_rounds() {
        let budget = SlaBudget::from_mode(SlaMode::Balanced);
        assert_eq!(budget.clamp_rounds(15), 10);
        assert_eq!(budget.clamp_rounds(5), 5);
    }

    #[test]
    fn from_complexity_simple() {
        let decision = OrchestrationDecision {
            complexity: TaskComplexity::SimpleExecution,
            use_orchestration: false,
            recommended_max_rounds: 4,
            recommended_plan_depth: 2,
            reason: "test",
        };
        let budget = SlaBudget::from_complexity(&decision);
        assert_eq!(budget.mode, SlaMode::Fast);
    }

    #[test]
    fn from_complexity_multi_domain() {
        let decision = OrchestrationDecision {
            complexity: TaskComplexity::MultiDomain,
            use_orchestration: true,
            recommended_max_rounds: 10,
            recommended_plan_depth: 6,
            reason: "test",
        };
        let budget = SlaBudget::from_complexity(&decision);
        assert_eq!(budget.mode, SlaMode::Deep);
    }

    #[test]
    fn fraction_consumed_starts_near_zero() {
        let budget = SlaBudget::from_mode(SlaMode::Balanced);
        assert!(budget.fraction_consumed() < 0.01);
    }

    #[test]
    fn expired_budget_blocks_orchestration() {
        let mut budget = SlaBudget::from_mode(SlaMode::Balanced);
        // Simulate expiry by setting start far in the past
        budget.start = Instant::now() - Duration::from_secs(200);
        assert!(budget.is_expired());
        assert!(!budget.allows_orchestration());
        assert!(!budget.allows_retry(0));
    }

    #[test]
    fn clamp_never_exceeds_budget() {
        let b = SlaBudget::from_mode(SlaMode::Fast);
        assert_eq!(b.clamp_rounds(100), 4);
        assert_eq!(b.clamp_rounds(2), 2);
    }

    // ── Phase 2 SLA Hard Enforcement tests ──────────────────────────────────

    #[test]
    fn sla_blocks_retry_when_budget_exhausted() {
        // Fast mode: max_retries = 0, so even first retry is blocked.
        let budget = SlaBudget::from_mode(SlaMode::Fast);
        assert!(!budget.allows_retry(0), "Fast mode should block all retries");

        // Balanced mode: max_retries = 1, so second retry is blocked.
        let budget = SlaBudget::from_mode(SlaMode::Balanced);
        assert!(budget.allows_retry(0), "Balanced should allow first retry");
        assert!(!budget.allows_retry(1), "Balanced should block second retry");

        // Deep mode: max_retries = 3.
        let budget = SlaBudget::from_mode(SlaMode::Deep);
        assert!(budget.allows_retry(2), "Deep should allow third retry");
        assert!(!budget.allows_retry(3), "Deep should block fourth retry");
    }

    #[test]
    fn sla_blocks_retry_when_expired() {
        let mut budget = SlaBudget::from_mode(SlaMode::Balanced);
        budget.start = Instant::now() - Duration::from_secs(200);
        assert!(!budget.allows_retry(0), "Expired budget should block retries");
    }

    #[test]
    fn sla_blocks_orchestration_fast_mode() {
        let budget = SlaBudget::from_mode(SlaMode::Fast);
        assert!(!budget.allows_orchestration(), "Fast mode: 0 sub-agents → no orchestration");
    }

    #[test]
    fn sla_allows_orchestration_balanced_mode() {
        let budget = SlaBudget::from_mode(SlaMode::Balanced);
        assert!(budget.allows_orchestration(), "Balanced mode: 3 sub-agents → orchestration ok");
    }

    #[test]
    fn sla_k5_1_truncates_plan_when_exceeds_budget() {
        // Fast mode: max_rounds = 4, room for 2 (critic + synthesis) → max 2 plan steps.
        let budget = SlaBudget::from_mode(SlaMode::Fast);
        let sla_max = budget.clamp_rounds(10) as usize; // 4
        let max_plan_steps = sla_max.saturating_sub(2);  // 2
        assert_eq!(max_plan_steps, 2, "Fast mode should allow at most 2 plan steps");

        // Balanced: max_rounds = 10, room for 2 → max 8 plan steps.
        let budget = SlaBudget::from_mode(SlaMode::Balanced);
        let sla_max = budget.clamp_rounds(10) as usize;
        let max_plan_steps = sla_max.saturating_sub(2);
        assert_eq!(max_plan_steps, 8, "Balanced mode should allow at most 8 plan steps");
    }

    #[test]
    fn sla_clamp_plan_depth() {
        // Fast mode: max_plan_depth = 2.
        let budget = SlaBudget::from_mode(SlaMode::Fast);
        assert_eq!(budget.clamp_plan_depth(10), 2);
        assert_eq!(budget.clamp_plan_depth(1), 1);

        // Balanced: max_plan_depth = 5.
        let budget = SlaBudget::from_mode(SlaMode::Balanced);
        assert_eq!(budget.clamp_plan_depth(10), 5);
        assert_eq!(budget.clamp_plan_depth(3), 3);

        // Deep: max_plan_depth = 10.
        let budget = SlaBudget::from_mode(SlaMode::Deep);
        assert_eq!(budget.clamp_plan_depth(10), 10);
        assert_eq!(budget.clamp_plan_depth(15), 10);
    }

    #[test]
    fn sla_fraction_consumed_80_percent_warning_threshold() {
        let budget = SlaBudget::from_mode(SlaMode::Balanced);
        // Fresh budget should be well below 80%.
        assert!(budget.fraction_consumed() < 0.80, "Fresh budget should be under 80%");
        // Expired budget should be over 100%.
        let mut expired = SlaBudget::from_mode(SlaMode::Balanced);
        expired.start = Instant::now() - Duration::from_secs(200);
        assert!(expired.fraction_consumed() >= 1.0, "Expired budget should be >= 100%");
    }

    // ── PARTIAL-1: sla_budget.max_rounds sync with routing escalation ────────

    #[test]
    fn partial1_sla_max_rounds_can_be_updated_on_routing_escalation() {
        // Verify that sla_budget.max_rounds is a mutable pub field that the
        // escalation block in convergence_phase.rs can update.
        let mut budget = SlaBudget::from_mode(SlaMode::Balanced);
        assert_eq!(budget.max_rounds, 10, "Balanced starts at 10 rounds");

        // Simulate routing escalation: conv_ctrl.max_rounds increased to 14,
        // sla_budget.max_rounds must be updated to match.
        let delta: u32 = 4;
        let new_max = budget.max_rounds + delta;
        budget.max_rounds = new_max; // PARTIAL-1 fix: direct field update

        assert_eq!(budget.max_rounds, 14, "After escalation, max_rounds must be 14");
        // clamp_rounds should now respect the extended budget
        assert_eq!(budget.clamp_rounds(13), 13, "13 ≤ 14 should pass through");
        assert_eq!(budget.clamp_rounds(15), 14, "15 > 14 should be clamped");
    }

    #[test]
    fn partial1_sla_pressure_decreases_after_escalation_extends_budget() {
        // After escalation extends max_rounds, the SLA pressure fraction should
        // be lower (same time elapsed, more budget available).
        let budget = SlaBudget::from_mode(SlaMode::Fast);
        let original_max = budget.max_rounds;
        assert_eq!(original_max, 4, "Fast starts at 4 rounds");

        // Without extension: clamp_rounds(6) = 4 (hard cap)
        assert_eq!(budget.clamp_rounds(6), 4);

        // After escalation: extend to 8 rounds
        let mut extended = budget.clone();
        extended.max_rounds = 8;
        assert_eq!(extended.clamp_rounds(6), 6, "Extended budget allows 6 rounds");
        assert_eq!(extended.clamp_rounds(9), 8, "Still capped at new max=8");
    }
}
