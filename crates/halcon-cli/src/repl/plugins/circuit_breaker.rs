//! Per-plugin isolated circuit breaker.
//!
//! Completely separate from the global [`ToolFailureTracker`] — each plugin gets its
//! own threshold, state and cooldown clock.  This prevents a misbehaving plugin from
//! affecting the global failure budget, and allows independent recovery timelines.

use std::time::{Duration, Instant};

// ─── Circuit State ────────────────────────────────────────────────────────────

/// FSM state of one circuit breaker instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — calls are allowed.
    Closed,
    /// Tripped — calls are rejected immediately.
    Open,
    /// Cooldown elapsed — next call is allowed as a probe.
    HalfOpen,
}

// ─── Circuit Breaker ──────────────────────────────────────────────────────────

/// Per-plugin circuit breaker with configurable threshold and cooldown.
///
/// State machine:
/// ```text
///   Closed ──(failures >= threshold)──► Open ──(cooldown elapsed)──► HalfOpen
///     ▲                                                                   │
///     └───────────────────(success recorded)─────────────────────────────┘
/// ```
pub struct PluginCircuitBreaker {
    consecutive_failures: u32,
    threshold: u32,
    state: CircuitState,
    last_failure: Option<Instant>,
    recovery_cooldown: Duration,
}

impl PluginCircuitBreaker {
    /// Create a new circuit breaker for a plugin.
    ///
    /// - `threshold`: consecutive failures before tripping (from `SupervisorPolicy.halt_on_failures`).
    /// - `recovery_cooldown`: how long to wait before attempting `HalfOpen`.
    pub fn new(threshold: u32, recovery_cooldown: Duration) -> Self {
        Self {
            consecutive_failures: 0,
            threshold: threshold.max(1),
            state: CircuitState::Closed,
            last_failure: None,
            recovery_cooldown,
        }
    }

    /// Default breaker: threshold=3, cooldown=60s.
    pub fn with_defaults() -> Self {
        Self::new(3, Duration::from_secs(60))
    }

    /// Record a failed invocation.
    ///
    /// Returns `true` when the circuit trips (`Open`) as a result of this failure.
    pub fn record_failure(&mut self) -> bool {
        self.consecutive_failures += 1;
        self.last_failure = Some(Instant::now());

        let tripped = self.consecutive_failures >= self.threshold;
        if tripped {
            self.state = CircuitState::Open;
        }
        tripped
    }

    /// Record a successful invocation — resets the failure counter and closes the circuit.
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.state = CircuitState::Closed;
    }

    /// Returns `true` if the circuit is currently open (calls should be rejected).
    ///
    /// Automatically transitions `Open → HalfOpen` when the recovery cooldown has elapsed.
    pub fn is_open(&self) -> bool {
        match self.state {
            CircuitState::Closed | CircuitState::HalfOpen => false,
            CircuitState::Open => {
                // Don't mutate self here; use try_half_open to mutate
                if let Some(last) = self.last_failure {
                    last.elapsed() < self.recovery_cooldown
                } else {
                    false
                }
            }
        }
    }

    /// Attempt to transition from `Open → HalfOpen` if cooldown has elapsed.
    ///
    /// Returns `true` when the transition succeeds (probe call is allowed).
    pub fn try_half_open(&mut self) -> bool {
        if self.state != CircuitState::Open {
            return false;
        }
        if let Some(last) = self.last_failure {
            if last.elapsed() >= self.recovery_cooldown {
                self.state = CircuitState::HalfOpen;
                return true;
            }
        }
        false
    }

    /// Force reset to `Closed` (used by supervisor manual recovery commands).
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.state = CircuitState::Closed;
        self.last_failure = None;
    }

    /// Current circuit state.
    pub fn state(&self) -> CircuitState {
        self.state
    }

    /// Current consecutive failure count.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_trip_opens_circuit() {
        let mut cb = PluginCircuitBreaker::new(3, Duration::from_secs(60));
        assert_eq!(cb.state(), CircuitState::Closed);

        let first = cb.record_failure();
        assert!(!first);
        let second = cb.record_failure();
        assert!(!second);
        let third = cb.record_failure();
        assert!(third, "third failure must trip the circuit");
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(cb.is_open());
    }

    #[test]
    fn success_resets_counter_and_closes() {
        let mut cb = PluginCircuitBreaker::new(2, Duration::from_secs(60));
        cb.record_failure();
        cb.record_success();
        assert_eq!(cb.consecutive_failures(), 0);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn cooldown_transition_to_half_open() {
        // Zero cooldown so we can test immediately
        let mut cb = PluginCircuitBreaker::new(1, Duration::from_millis(0));
        cb.record_failure(); // trips at threshold=1
        assert_eq!(cb.state(), CircuitState::Open);

        // After zero cooldown, try_half_open should succeed
        std::thread::sleep(Duration::from_millis(1));
        let probing = cb.try_half_open();
        assert!(probing);
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn half_open_success_closes_circuit() {
        let mut cb = PluginCircuitBreaker::new(1, Duration::from_millis(0));
        cb.record_failure();
        std::thread::sleep(Duration::from_millis(1));
        cb.try_half_open();
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(!cb.is_open());
    }

    #[test]
    fn reset_clears_all_state() {
        let mut cb = PluginCircuitBreaker::new(1, Duration::from_secs(60));
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
        assert!(!cb.is_open());
    }

    #[test]
    fn is_open_false_before_threshold() {
        let mut cb = PluginCircuitBreaker::new(3, Duration::from_secs(60));
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_open(), "circuit should still be closed before threshold");
    }

    #[test]
    fn try_half_open_noop_when_closed() {
        let mut cb = PluginCircuitBreaker::new(3, Duration::from_secs(0));
        let result = cb.try_half_open();
        assert!(!result, "try_half_open on Closed should return false");
    }
}
