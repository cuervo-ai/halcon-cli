//! Trait interface for SLA budget management.
//!
//! Implementations track time/round/retry budgets and enforce constraints
//! on orchestration, retries, and plan depth.

use std::time::Duration;

/// SLA budget manager — time-budget-aware planning for adaptive execution.
///
/// Implementations configure agent behaviour based on performance tiers,
/// tracking time budgets and exposing constraint checks for the agent loop.
pub trait BudgetManager: Send + Sync {
    /// Check if the time budget has been exceeded.
    fn is_expired(&self) -> bool;

    /// Remaining time budget. `None` if unlimited or expired.
    fn remaining(&self) -> Option<Duration>;

    /// Fraction of time budget consumed. 0.0 = just started, 1.0+ = expired.
    fn fraction_consumed(&self) -> f64;

    /// Whether orchestration (sub-agents) is allowed under this budget.
    fn allows_orchestration(&self) -> bool;

    /// Whether retries are allowed under this budget.
    fn allows_retry(&self, attempt: u32) -> bool;

    /// Clamp a requested plan depth to the SLA maximum.
    fn clamp_plan_depth(&self, depth: u32) -> u32;

    /// Clamp a requested round count to the SLA maximum.
    fn clamp_rounds(&self, rounds: u32) -> u32;
}
