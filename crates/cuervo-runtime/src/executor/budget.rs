//! Shared budget tracking for runtime execution.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Atomic shared budget for tracking resource usage across concurrent tasks.
pub struct RuntimeBudget {
    tokens_used: AtomicU64,
    cost_cents: AtomicU64, // cost in 1/100 cents for atomic precision
    token_limit: u64,
    cost_limit_cents: u64,
    start: Instant,
    duration_limit: Duration,
}

impl RuntimeBudget {
    pub fn new(token_limit: u64, cost_limit_usd: f64, duration_limit: Duration) -> Self {
        Self {
            tokens_used: AtomicU64::new(0),
            cost_cents: AtomicU64::new(0),
            token_limit,
            cost_limit_cents: (cost_limit_usd * 10_000.0) as u64,
            start: Instant::now(),
            duration_limit,
        }
    }

    /// Record token usage. Returns Err if budget exceeded.
    pub fn record_tokens(&self, tokens: u64) -> Result<(), BudgetExceeded> {
        let new = self.tokens_used.fetch_add(tokens, Ordering::AcqRel) + tokens;
        if self.token_limit > 0 && new > self.token_limit {
            return Err(BudgetExceeded::Tokens {
                used: new,
                limit: self.token_limit,
            });
        }
        Ok(())
    }

    /// Record cost. Returns Err if budget exceeded.
    pub fn record_cost(&self, cost_usd: f64) -> Result<(), BudgetExceeded> {
        let cents = (cost_usd * 10_000.0) as u64;
        let new = self.cost_cents.fetch_add(cents, Ordering::AcqRel) + cents;
        if self.cost_limit_cents > 0 && new > self.cost_limit_cents {
            return Err(BudgetExceeded::Cost {
                used_usd: new as f64 / 10_000.0,
                limit_usd: self.cost_limit_cents as f64 / 10_000.0,
            });
        }
        Ok(())
    }

    /// Check if duration limit has been exceeded.
    pub fn is_duration_exceeded(&self) -> bool {
        self.duration_limit > Duration::ZERO && self.start.elapsed() > self.duration_limit
    }

    /// Check if any budget has been exceeded.
    pub fn is_exceeded(&self) -> bool {
        if self.is_duration_exceeded() {
            return true;
        }
        if self.token_limit > 0 && self.tokens_used.load(Ordering::Acquire) > self.token_limit {
            return true;
        }
        if self.cost_limit_cents > 0
            && self.cost_cents.load(Ordering::Acquire) > self.cost_limit_cents
        {
            return true;
        }
        false
    }

    pub fn tokens_used(&self) -> u64 {
        self.tokens_used.load(Ordering::Acquire)
    }

    pub fn cost_usd(&self) -> f64 {
        self.cost_cents.load(Ordering::Acquire) as f64 / 10_000.0
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    pub fn tokens_remaining(&self) -> u64 {
        if self.token_limit == 0 {
            return u64::MAX;
        }
        self.token_limit.saturating_sub(self.tokens_used.load(Ordering::Acquire))
    }
}

#[derive(Debug)]
pub enum BudgetExceeded {
    Tokens { used: u64, limit: u64 },
    Cost { used_usd: f64, limit_usd: f64 },
    Duration,
}

impl std::fmt::Display for BudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BudgetExceeded::Tokens { used, limit } => {
                write!(f, "token budget exceeded: {used}/{limit}")
            }
            BudgetExceeded::Cost { used_usd, limit_usd } => {
                write!(f, "cost budget exceeded: ${used_usd:.4}/${limit_usd:.4}")
            }
            BudgetExceeded::Duration => write!(f, "duration budget exceeded"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_unlimited() {
        let b = RuntimeBudget::new(0, 0.0, Duration::ZERO);
        b.record_tokens(999_999).unwrap();
        b.record_cost(999.0).unwrap();
        assert!(!b.is_exceeded());
    }

    #[test]
    fn budget_token_limit() {
        let b = RuntimeBudget::new(100, 0.0, Duration::ZERO);
        b.record_tokens(50).unwrap();
        assert!(!b.is_exceeded());
        assert_eq!(b.tokens_remaining(), 50);
        let result = b.record_tokens(60);
        assert!(result.is_err());
    }

    #[test]
    fn budget_cost_limit() {
        let b = RuntimeBudget::new(0, 1.0, Duration::ZERO);
        b.record_cost(0.5).unwrap();
        assert!(!b.is_exceeded());
        let result = b.record_cost(0.6);
        assert!(result.is_err());
    }

    #[test]
    fn budget_tokens_used() {
        let b = RuntimeBudget::new(1000, 0.0, Duration::ZERO);
        b.record_tokens(100).unwrap();
        b.record_tokens(200).unwrap();
        assert_eq!(b.tokens_used(), 300);
    }

    #[test]
    fn budget_cost_usd() {
        let b = RuntimeBudget::new(0, 10.0, Duration::ZERO);
        b.record_cost(0.5).unwrap();
        assert!((b.cost_usd() - 0.5).abs() < 0.001);
    }

    #[test]
    fn budget_exceeded_display() {
        let e = BudgetExceeded::Tokens {
            used: 150,
            limit: 100,
        };
        assert!(e.to_string().contains("150/100"));

        let e = BudgetExceeded::Cost {
            used_usd: 1.5,
            limit_usd: 1.0,
        };
        assert!(e.to_string().contains("cost budget exceeded"));
    }

    #[test]
    fn budget_remaining_with_no_limit() {
        let b = RuntimeBudget::new(0, 0.0, Duration::ZERO);
        assert_eq!(b.tokens_remaining(), u64::MAX);
    }

    #[test]
    fn budget_remaining_saturates() {
        let b = RuntimeBudget::new(100, 0.0, Duration::ZERO);
        // Even after exceeding, remaining saturates to 0
        let _ = b.record_tokens(150);
        assert_eq!(b.tokens_remaining(), 0);
    }
}
