//! ExecutionBudget — hard resource limits that guarantee agent termination.
//!
//! ## Design Invariant
//!
//! **I-6.1**: An agent must terminate before exceeding any single hard budget dimension.
//!
//! All five dimensions are independent: a single violation is sufficient to halt
//! execution. The `BudgetTracker` accumulates usage and returns `Err(BudgetExceeded)`
//! on the first violation.
//!
//! ## Usage
//!
//! ```rust
//! use halcon_agent_core::execution_budget::{BudgetTracker, ExecutionBudget};
//!
//! fn example() -> Result<(), Box<dyn std::error::Error>> {
//!     let budget = ExecutionBudget { max_rounds: 20, ..Default::default() };
//!     let mut tracker = BudgetTracker::new(budget);
//!     // In the agent loop:
//!     tracker.consume_round()?; // returns Err if max_rounds exceeded
//!     tracker.consume_tool_calls(1)?;
//!     Ok(())
//! }
//! ```

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── ExecutionBudget ──────────────────────────────────────────────────────────

/// Hard resource limits for a single GDEM agent session.
///
/// All dimensions are enforced independently. Set any value to `u32::MAX` / `u64::MAX`
/// to effectively disable that dimension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionBudget {
    /// Maximum agent rounds (one round = one tool batch + one critic evaluation).
    pub max_rounds: u32,
    /// Maximum total tool calls across all rounds.
    pub max_tool_calls: u32,
    /// Maximum replan invocations (AdaptivePlanner::replan calls).
    pub max_replans: u32,
    /// Maximum tokens consumed (input + output, combined).
    pub max_tokens: u64,
    /// Maximum wall-clock time for the entire session.
    pub max_wall_time: Duration,
}

impl Default for ExecutionBudget {
    fn default() -> Self {
        Self {
            max_rounds: 20,
            max_tool_calls: 100,
            max_replans: 5,
            max_tokens: 100_000,
            max_wall_time: Duration::from_secs(300),
        }
    }
}

impl ExecutionBudget {
    /// Budget appropriate for a sub-agent with tighter constraints.
    pub fn sub_agent() -> Self {
        Self {
            max_rounds: 6,
            max_tool_calls: 30,
            max_replans: 2,
            max_tokens: 20_000,
            max_wall_time: Duration::from_secs(120),
        }
    }

    /// Minimal budget for testing/benchmarking.
    pub fn minimal() -> Self {
        Self {
            max_rounds: 3,
            max_tool_calls: 10,
            max_replans: 1,
            max_tokens: 5_000,
            max_wall_time: Duration::from_secs(30),
        }
    }
}

// ─── BudgetExceeded ───────────────────────────────────────────────────────────

/// A specific budget dimension that was exceeded.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum BudgetExceeded {
    #[error("Round budget exceeded: used {used}/{max}")]
    Rounds { used: u32, max: u32 },

    #[error("Tool call budget exceeded: used {used}/{max}")]
    ToolCalls { used: u32, max: u32 },

    #[error("Replan budget exceeded: used {used}/{max}")]
    Replans { used: u32, max: u32 },

    #[error("Token budget exceeded: used {used}/{max}")]
    Tokens { used: u64, max: u64 },

    #[error("Wall-time budget exceeded: {elapsed_ms}ms > {max_ms}ms")]
    WallTime { elapsed_ms: u64, max_ms: u64 },
}

// ─── BudgetTracker ────────────────────────────────────────────────────────────

/// Tracks budget consumption and enforces hard limits.
///
/// All `consume_*` methods are idempotent in error state — once a budget is exceeded,
/// subsequent calls return the same error without further mutation.
pub struct BudgetTracker {
    budget: ExecutionBudget,
    rounds_used: u32,
    tool_calls_used: u32,
    replans_used: u32,
    tokens_used: u64,
    start_time: Instant,
    exhausted: bool,
}

impl BudgetTracker {
    pub fn new(budget: ExecutionBudget) -> Self {
        Self {
            budget,
            rounds_used: 0,
            tool_calls_used: 0,
            replans_used: 0,
            tokens_used: 0,
            start_time: Instant::now(),
            exhausted: false,
        }
    }

    // ─── Consumption methods ────────────────────────────────────────────────

    /// Consume one agent round. Returns `Err` if max_rounds is exceeded.
    pub fn consume_round(&mut self) -> Result<(), BudgetExceeded> {
        self.rounds_used += 1;
        if self.rounds_used > self.budget.max_rounds {
            self.exhausted = true;
            return Err(BudgetExceeded::Rounds {
                used: self.rounds_used,
                max: self.budget.max_rounds,
            });
        }
        Ok(())
    }

    /// Consume `count` tool calls. Returns `Err` if max_tool_calls is exceeded.
    pub fn consume_tool_calls(&mut self, count: u32) -> Result<(), BudgetExceeded> {
        self.tool_calls_used += count;
        if self.tool_calls_used > self.budget.max_tool_calls {
            self.exhausted = true;
            return Err(BudgetExceeded::ToolCalls {
                used: self.tool_calls_used,
                max: self.budget.max_tool_calls,
            });
        }
        Ok(())
    }

    /// Consume one replan invocation. Returns `Err` if max_replans is exceeded.
    pub fn consume_replan(&mut self) -> Result<(), BudgetExceeded> {
        self.replans_used += 1;
        if self.replans_used > self.budget.max_replans {
            self.exhausted = true;
            return Err(BudgetExceeded::Replans {
                used: self.replans_used,
                max: self.budget.max_replans,
            });
        }
        Ok(())
    }

    /// Consume `tokens` tokens. Returns `Err` if max_tokens is exceeded.
    pub fn consume_tokens(&mut self, tokens: u64) -> Result<(), BudgetExceeded> {
        self.tokens_used += tokens;
        if self.tokens_used > self.budget.max_tokens {
            self.exhausted = true;
            return Err(BudgetExceeded::Tokens {
                used: self.tokens_used,
                max: self.budget.max_tokens,
            });
        }
        Ok(())
    }

    /// Check wall-clock time. Returns `Err` if max_wall_time is exceeded.
    ///
    /// This is a read-only check (does not consume any resource counter).
    pub fn check_wall_time(&self) -> Result<(), BudgetExceeded> {
        let elapsed = self.start_time.elapsed();
        if elapsed > self.budget.max_wall_time {
            return Err(BudgetExceeded::WallTime {
                elapsed_ms: elapsed.as_millis() as u64,
                max_ms: self.budget.max_wall_time.as_millis() as u64,
            });
        }
        Ok(())
    }

    // ─── Status accessors ───────────────────────────────────────────────────

    /// True if any budget dimension has been exceeded.
    pub fn is_exhausted(&self) -> bool {
        self.exhausted
            || self.rounds_used >= self.budget.max_rounds
            || self.tool_calls_used >= self.budget.max_tool_calls
            || self.replans_used >= self.budget.max_replans
            || self.tokens_used >= self.budget.max_tokens
    }

    /// Remaining rounds before budget is hit (returns 0 if already exceeded).
    pub fn rounds_remaining(&self) -> u32 {
        self.budget.max_rounds.saturating_sub(self.rounds_used)
    }

    /// Fraction of round budget consumed [0, 1].
    pub fn round_budget_fraction(&self) -> f32 {
        if self.budget.max_rounds == 0 {
            return 1.0;
        }
        (self.rounds_used as f32 / self.budget.max_rounds as f32).clamp(0.0, 1.0)
    }

    pub fn rounds_used(&self) -> u32 {
        self.rounds_used
    }

    pub fn tool_calls_used(&self) -> u32 {
        self.tool_calls_used
    }

    pub fn replans_used(&self) -> u32 {
        self.replans_used
    }

    pub fn tokens_used(&self) -> u64 {
        self.tokens_used
    }

    pub fn budget(&self) -> &ExecutionBudget {
        &self.budget
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker(max_rounds: u32) -> BudgetTracker {
        BudgetTracker::new(ExecutionBudget {
            max_rounds,
            ..Default::default()
        })
    }

    #[test]
    fn rounds_exhausted_at_max() {
        let mut t = tracker(5);
        for _ in 0..5 {
            assert!(t.consume_round().is_ok());
        }
        assert!(t.consume_round().is_err());
    }

    #[test]
    fn rounds_remaining_accurate() {
        let mut t = tracker(10);
        assert_eq!(t.rounds_remaining(), 10);
        t.consume_round().unwrap();
        assert_eq!(t.rounds_remaining(), 9);
        for _ in 0..9 {
            let _ = t.consume_round();
        }
        assert_eq!(t.rounds_remaining(), 0);
    }

    #[test]
    fn tool_calls_budget_enforced() {
        let budget = ExecutionBudget {
            max_tool_calls: 3,
            ..Default::default()
        };
        let mut t = BudgetTracker::new(budget);
        assert!(t.consume_tool_calls(3).is_ok());
        assert!(t.consume_tool_calls(1).is_err());
    }

    #[test]
    fn replan_budget_enforced() {
        let budget = ExecutionBudget {
            max_replans: 2,
            ..Default::default()
        };
        let mut t = BudgetTracker::new(budget);
        assert!(t.consume_replan().is_ok());
        assert!(t.consume_replan().is_ok());
        assert!(t.consume_replan().is_err());
    }

    #[test]
    fn token_budget_enforced() {
        let budget = ExecutionBudget {
            max_tokens: 1000,
            ..Default::default()
        };
        let mut t = BudgetTracker::new(budget);
        assert!(t.consume_tokens(999).is_ok());
        assert!(t.consume_tokens(2).is_err()); // total = 1001 > 1000
    }

    #[test]
    fn is_exhausted_after_round_limit() {
        let mut t = tracker(1);
        assert!(!t.is_exhausted());
        t.consume_round().unwrap();
        assert!(t.is_exhausted());
    }

    #[test]
    fn round_budget_fraction_increases() {
        let mut t = tracker(4);
        assert_eq!(t.round_budget_fraction(), 0.0);
        t.consume_round().unwrap();
        assert!((t.round_budget_fraction() - 0.25).abs() < 1e-4);
        t.consume_round().unwrap();
        assert!((t.round_budget_fraction() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn wall_time_check_ok_immediately() {
        let t = BudgetTracker::new(ExecutionBudget::default());
        assert!(t.check_wall_time().is_ok());
    }

    #[test]
    fn budget_exceeded_error_message_round() {
        let mut t = tracker(1);
        t.consume_round().unwrap();
        let err = t.consume_round().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Round"), "error message: {}", msg);
    }

    #[test]
    fn sub_agent_budget_is_tighter_than_default() {
        let sub = ExecutionBudget::sub_agent();
        let def = ExecutionBudget::default();
        assert!(sub.max_rounds < def.max_rounds);
        assert!(sub.max_tokens < def.max_tokens);
    }

    #[test]
    fn minimal_budget_allows_few_rounds() {
        let mut t = BudgetTracker::new(ExecutionBudget::minimal());
        for _ in 0..3 {
            assert!(t.consume_round().is_ok());
        }
        assert!(t.consume_round().is_err());
    }

    #[test]
    fn cumulative_tool_calls_aggregated() {
        let budget = ExecutionBudget {
            max_tool_calls: 10,
            ..Default::default()
        };
        let mut t = BudgetTracker::new(budget);
        t.consume_tool_calls(3).unwrap();
        t.consume_tool_calls(3).unwrap();
        assert_eq!(t.tool_calls_used(), 6);
    }
}
