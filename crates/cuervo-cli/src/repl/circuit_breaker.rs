//! Circuit breaker per provider: isolates failing providers with Closed → Open → HalfOpen states.
//!
//! When a provider accumulates `failure_threshold` failures within a sliding `window_secs`,
//! the breaker trips to Open. After `open_duration_secs` it transitions to HalfOpen,
//! allowing `half_open_probes` probe requests. If probes succeed, the breaker closes.
//! If any probe fails, it reopens.

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use cuervo_core::types::CircuitBreakerConfig;

/// Circuit breaker states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Normal operation — all requests pass through.
    Closed,
    /// Provider is isolated — requests fail-fast.
    Open,
    /// Allowing probe requests to test recovery.
    HalfOpen,
}

impl fmt::Display for BreakerState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BreakerState::Closed => write!(f, "closed"),
            BreakerState::Open => write!(f, "open"),
            BreakerState::HalfOpen => write!(f, "half_open"),
        }
    }
}

/// Returned when a breaker state changes.
#[derive(Debug, Clone)]
pub struct BreakerTransition {
    #[allow(dead_code)] // Used by resilience event persistence
    pub provider: String,
    pub from: BreakerState,
    pub to: BreakerState,
}

/// Error returned when the breaker is open and rejecting requests.
#[derive(Debug, Clone)]
pub struct BreakerOpen {
    pub provider: String,
    pub retry_after: Duration,
}

impl fmt::Display for BreakerOpen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "circuit breaker open for '{}' (retry after {:.1}s)",
            self.provider,
            self.retry_after.as_secs_f64()
        )
    }
}

/// Per-provider circuit breaker with sliding-window failure tracking.
pub struct ProviderBreaker {
    provider: String,
    state: BreakerState,
    config: CircuitBreakerConfig,
    /// Timestamps of recent failures within the sliding window.
    failure_timestamps: VecDeque<Instant>,
    /// When the breaker was opened (used for cooldown calculation).
    opened_at: Option<Instant>,
    /// Successful probe count in half-open state.
    half_open_successes: u32,
    /// Remaining probes allowed in half-open state.
    half_open_probes_remaining: u32,
    /// Consecutive trip count for exponential backoff.
    consecutive_trips: u32,
}

impl ProviderBreaker {
    pub fn new(provider: String, config: CircuitBreakerConfig) -> Self {
        Self {
            provider,
            state: BreakerState::Closed,
            config,
            failure_timestamps: VecDeque::new(),
            opened_at: None,
            half_open_successes: 0,
            half_open_probes_remaining: 0,
            consecutive_trips: 0,
        }
    }

    /// Check if a request is allowed through the breaker.
    ///
    /// - `Closed`: always allows.
    /// - `Open`: checks cooldown. If expired, transitions to HalfOpen and allows.
    ///   Otherwise returns `Err(BreakerOpen)`.
    /// - `HalfOpen`: allows if probes remain, otherwise rejects.
    pub fn check(&mut self) -> Result<(), BreakerOpen> {
        self.check_at(Instant::now())
    }

    /// Check with an explicit timestamp (for testing).
    pub fn check_at(&mut self, now: Instant) -> Result<(), BreakerOpen> {
        match self.state {
            BreakerState::Closed => Ok(()),
            BreakerState::Open => {
                let opened = self.opened_at.unwrap_or(now);
                let cooldown = self.current_open_duration();
                if now >= opened + cooldown {
                    // Cooldown expired → transition to HalfOpen.
                    // This first request IS a probe, so consume one slot.
                    self.state = BreakerState::HalfOpen;
                    self.half_open_successes = 0;
                    self.half_open_probes_remaining =
                        self.config.half_open_probes.saturating_sub(1);
                    tracing::info!(
                        provider = %self.provider,
                        "Circuit breaker transitioning Open → HalfOpen"
                    );
                    Ok(())
                } else {
                    let remaining = (opened + cooldown) - now;
                    Err(BreakerOpen {
                        provider: self.provider.clone(),
                        retry_after: remaining,
                    })
                }
            }
            BreakerState::HalfOpen => {
                if self.half_open_probes_remaining > 0 {
                    self.half_open_probes_remaining -= 1;
                    Ok(())
                } else {
                    Err(BreakerOpen {
                        provider: self.provider.clone(),
                        retry_after: Duration::ZERO,
                    })
                }
            }
        }
    }

    /// Record a successful invocation.
    ///
    /// In HalfOpen state, accumulates successes. When enough probes succeed,
    /// transitions back to Closed.
    pub fn record_success(&mut self) -> Option<BreakerTransition> {
        match self.state {
            BreakerState::Closed => None,
            BreakerState::HalfOpen => {
                self.half_open_successes += 1;
                if self.half_open_successes >= self.config.half_open_probes {
                    let transition = BreakerTransition {
                        provider: self.provider.clone(),
                        from: BreakerState::HalfOpen,
                        to: BreakerState::Closed,
                    };
                    self.state = BreakerState::Closed;
                    self.failure_timestamps.clear();
                    self.opened_at = None;
                    self.consecutive_trips = 0;
                    tracing::info!(
                        provider = %self.provider,
                        "Circuit breaker transitioning HalfOpen → Closed (recovered)"
                    );
                    Some(transition)
                } else {
                    None
                }
            }
            BreakerState::Open => None, // Shouldn't happen (check() rejects)
        }
    }

    /// Record a failed invocation.
    ///
    /// In Closed state, adds to the sliding window. Trips to Open if threshold reached.
    /// In HalfOpen state, immediately reopens.
    pub fn record_failure(&mut self) -> Option<BreakerTransition> {
        self.record_failure_at(Instant::now())
    }

    /// Record failure with an explicit timestamp (for testing).
    pub fn record_failure_at(&mut self, now: Instant) -> Option<BreakerTransition> {
        match self.state {
            BreakerState::Closed => {
                self.failure_timestamps.push_back(now);
                self.prune_window(now);

                if self.failure_timestamps.len() >= self.config.failure_threshold as usize {
                    self.consecutive_trips += 1;
                    let transition = BreakerTransition {
                        provider: self.provider.clone(),
                        from: BreakerState::Closed,
                        to: BreakerState::Open,
                    };
                    self.state = BreakerState::Open;
                    self.opened_at = Some(now);
                    tracing::warn!(
                        provider = %self.provider,
                        failures = self.failure_timestamps.len(),
                        consecutive_trips = self.consecutive_trips,
                        open_duration_secs = self.current_open_duration().as_secs(),
                        "Circuit breaker TRIPPED: Closed → Open"
                    );
                    Some(transition)
                } else {
                    None
                }
            }
            BreakerState::HalfOpen => {
                self.consecutive_trips += 1;
                let transition = BreakerTransition {
                    provider: self.provider.clone(),
                    from: BreakerState::HalfOpen,
                    to: BreakerState::Open,
                };
                self.state = BreakerState::Open;
                self.opened_at = Some(now);
                tracing::warn!(
                    provider = %self.provider,
                    consecutive_trips = self.consecutive_trips,
                    open_duration_secs = self.current_open_duration().as_secs(),
                    "Circuit breaker probe failed: HalfOpen → Open"
                );
                Some(transition)
            }
            BreakerState::Open => None, // Already open
        }
    }

    /// Get the current breaker state.
    pub fn state(&self) -> BreakerState {
        self.state
    }

    /// Get the provider name.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Number of failures in the current sliding window.
    pub fn failure_count(&self) -> usize {
        self.failure_timestamps.len()
    }

    /// Compute the current open duration using exponential backoff with ±20% jitter.
    ///
    /// `base * 2^min(consecutive_trips - 1, 5) * jitter`, capped at 300s (5 minutes).
    /// Jitter prevents thundering herd when multiple providers recover simultaneously.
    fn current_open_duration(&self) -> Duration {
        let base = self.config.open_duration_secs;
        let exponent = self.consecutive_trips.saturating_sub(1).min(5);
        let duration_secs = base.saturating_mul(1u64 << exponent).min(300);
        Self::apply_jitter(duration_secs)
    }

    /// Apply ±20% jitter to a duration in seconds.
    ///
    /// In test mode, jitter is disabled for deterministic behavior.
    fn apply_jitter(secs: u64) -> Duration {
        let base_ms = secs * 1000;

        #[cfg(not(test))]
        {
            use rand::Rng;
            let jitter_factor = 0.8 + rand::rng().random_range(0.0..0.4);
            let jittered_ms = (base_ms as f64 * jitter_factor) as u64;
            Duration::from_millis(jittered_ms.min(300_000))
        }

        #[cfg(test)]
        {
            // No jitter in tests for deterministic behavior
            Duration::from_millis(base_ms.min(300_000))
        }
    }

    /// Number of consecutive trips (for diagnostics).
    #[allow(dead_code)]
    pub fn consecutive_trips(&self) -> u32 {
        self.consecutive_trips
    }

    /// Remove failure timestamps outside the sliding window.
    fn prune_window(&mut self, now: Instant) {
        let window = Duration::from_secs(self.config.window_secs);
        while let Some(&oldest) = self.failure_timestamps.front() {
            if now.duration_since(oldest) > window {
                self.failure_timestamps.pop_front();
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 3,
            window_secs: 10,
            open_duration_secs: 5,
            half_open_probes: 2,
        }
    }

    #[test]
    fn breaker_starts_closed() {
        let breaker = ProviderBreaker::new("test".into(), test_config());
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert_eq!(breaker.failure_count(), 0);
    }

    #[test]
    fn breaker_allows_requests_when_closed() {
        let mut breaker = ProviderBreaker::new("test".into(), test_config());
        assert!(breaker.check().is_ok());
        assert!(breaker.check().is_ok());
    }

    #[test]
    fn breaker_opens_after_threshold_failures() {
        let mut breaker = ProviderBreaker::new("test".into(), test_config());
        let now = Instant::now();

        // First two failures — no trip.
        assert!(breaker.record_failure_at(now).is_none());
        assert!(breaker.record_failure_at(now).is_none());
        assert_eq!(breaker.state(), BreakerState::Closed);

        // Third failure — trip!
        let transition = breaker.record_failure_at(now);
        assert!(transition.is_some());
        let t = transition.unwrap();
        assert_eq!(t.from, BreakerState::Closed);
        assert_eq!(t.to, BreakerState::Open);
        assert_eq!(breaker.state(), BreakerState::Open);
    }

    #[test]
    fn breaker_rejects_when_open() {
        let mut breaker = ProviderBreaker::new("test".into(), test_config());
        let now = Instant::now();

        // Trip the breaker.
        for _ in 0..3 {
            breaker.record_failure_at(now);
        }
        assert_eq!(breaker.state(), BreakerState::Open);

        // Requests should be rejected.
        let err = breaker.check_at(now + Duration::from_secs(1));
        assert!(err.is_err());
        let breaker_open = err.unwrap_err();
        assert_eq!(breaker_open.provider, "test");
        assert!(breaker_open.retry_after.as_secs() <= 5);
    }

    #[test]
    fn breaker_transitions_to_half_open_after_cooldown() {
        let mut breaker = ProviderBreaker::new("test".into(), test_config());
        let now = Instant::now();

        // Trip the breaker.
        for _ in 0..3 {
            breaker.record_failure_at(now);
        }
        assert_eq!(breaker.state(), BreakerState::Open);

        // After cooldown, check should transition to HalfOpen.
        let after_cooldown = now + Duration::from_secs(6);
        assert!(breaker.check_at(after_cooldown).is_ok());
        assert_eq!(breaker.state(), BreakerState::HalfOpen);
    }

    #[test]
    fn half_open_success_closes_breaker() {
        let mut breaker = ProviderBreaker::new("test".into(), test_config());
        let now = Instant::now();

        // Trip → Open → HalfOpen.
        for _ in 0..3 {
            breaker.record_failure_at(now);
        }
        let after_cooldown = now + Duration::from_secs(6);
        breaker.check_at(after_cooldown).unwrap();
        assert_eq!(breaker.state(), BreakerState::HalfOpen);

        // First probe success — not enough yet (need 2).
        assert!(breaker.record_success().is_none());

        // Second probe success — closes.
        let transition = breaker.record_success();
        assert!(transition.is_some());
        let t = transition.unwrap();
        assert_eq!(t.from, BreakerState::HalfOpen);
        assert_eq!(t.to, BreakerState::Closed);
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert_eq!(breaker.failure_count(), 0);
    }

    #[test]
    fn half_open_failure_reopens_breaker() {
        let mut breaker = ProviderBreaker::new("test".into(), test_config());
        let now = Instant::now();

        // Trip → Open → HalfOpen.
        for _ in 0..3 {
            breaker.record_failure_at(now);
        }
        let after_cooldown = now + Duration::from_secs(6);
        breaker.check_at(after_cooldown).unwrap();
        assert_eq!(breaker.state(), BreakerState::HalfOpen);

        // Probe failure — back to Open.
        let transition = breaker.record_failure_at(after_cooldown);
        assert!(transition.is_some());
        let t = transition.unwrap();
        assert_eq!(t.from, BreakerState::HalfOpen);
        assert_eq!(t.to, BreakerState::Open);
        assert_eq!(breaker.state(), BreakerState::Open);
    }

    #[test]
    fn failures_outside_window_dont_count() {
        let mut breaker = ProviderBreaker::new("test".into(), test_config());
        let now = Instant::now();

        // Two failures at time 0.
        breaker.record_failure_at(now);
        breaker.record_failure_at(now);
        assert_eq!(breaker.failure_count(), 2);

        // One more failure at time 15 (outside the 10s window).
        let later = now + Duration::from_secs(15);
        let transition = breaker.record_failure_at(later);

        // The old failures should have been pruned, so only 1 failure in window.
        // Threshold is 3, so no trip.
        assert!(transition.is_none());
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert_eq!(breaker.failure_count(), 1);
    }

    #[test]
    fn breaker_display_formats() {
        assert_eq!(BreakerState::Closed.to_string(), "closed");
        assert_eq!(BreakerState::Open.to_string(), "open");
        assert_eq!(BreakerState::HalfOpen.to_string(), "half_open");

        let err = BreakerOpen {
            provider: "anthropic".into(),
            retry_after: Duration::from_secs(10),
        };
        let display = format!("{err}");
        assert!(display.contains("anthropic"));
        assert!(display.contains("10.0s"));
    }

    #[test]
    fn success_in_closed_state_is_noop() {
        let mut breaker = ProviderBreaker::new("test".into(), test_config());
        assert!(breaker.record_success().is_none());
        assert_eq!(breaker.state(), BreakerState::Closed);
    }

    #[test]
    fn exponential_backoff_increases_duration() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            window_secs: 60,
            open_duration_secs: 5,
            half_open_probes: 1,
        };
        let mut breaker = ProviderBreaker::new("backoff".into(), config);
        let now = Instant::now();

        // First trip: open_duration = ~5s (base * 2^0 = 5, ±20% jitter).
        breaker.record_failure_at(now);
        breaker.record_failure_at(now);
        assert_eq!(breaker.state(), BreakerState::Open);
        assert_eq!(breaker.consecutive_trips(), 1);
        let d1 = breaker.current_open_duration();
        assert!(d1.as_millis() >= 4000 && d1.as_millis() <= 6000,
            "expected ~5s ±20%, got {:?}", d1);

        // Wait for generous cooldown, transition to HalfOpen, then fail probe.
        let t1 = now + Duration::from_secs(7);
        breaker.check_at(t1).unwrap();
        assert_eq!(breaker.state(), BreakerState::HalfOpen);
        breaker.record_failure_at(t1);
        assert_eq!(breaker.state(), BreakerState::Open);
        assert_eq!(breaker.consecutive_trips(), 2);
        // Second trip: open_duration = ~10s (5 * 2^1, ±20% jitter).
        let d2 = breaker.current_open_duration();
        assert!(d2.as_millis() >= 8000 && d2.as_millis() <= 12000,
            "expected ~10s ±20%, got {:?}", d2);

        // Wait for generous cooldown, fail probe again.
        let t2 = t1 + Duration::from_secs(13);
        breaker.check_at(t2).unwrap();
        breaker.record_failure_at(t2);
        assert_eq!(breaker.consecutive_trips(), 3);
        // Third trip: ~20s (5 * 2^2, ±20% jitter).
        let d3 = breaker.current_open_duration();
        assert!(d3.as_millis() >= 16000 && d3.as_millis() <= 24000,
            "expected ~20s ±20%, got {:?}", d3);
    }

    #[test]
    fn backoff_resets_on_close() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            window_secs: 60,
            open_duration_secs: 5,
            half_open_probes: 1,
        };
        let mut breaker = ProviderBreaker::new("reset".into(), config);
        let now = Instant::now();

        // Trip once.
        breaker.record_failure_at(now);
        breaker.record_failure_at(now);
        assert_eq!(breaker.consecutive_trips(), 1);

        // Recover: cooldown → HalfOpen → success → Closed.
        let t1 = now + Duration::from_secs(7);
        breaker.check_at(t1).unwrap();
        breaker.record_success();
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert_eq!(breaker.consecutive_trips(), 0);

        // Trip again: should be back to base duration (~5s ±20% jitter).
        let t2 = t1 + Duration::from_secs(1);
        breaker.record_failure_at(t2);
        breaker.record_failure_at(t2);
        assert_eq!(breaker.consecutive_trips(), 1);
        let d = breaker.current_open_duration();
        assert!(d.as_millis() >= 4000 && d.as_millis() <= 6000,
            "expected ~5s ±20%, got {:?}", d);
    }

    #[test]
    fn backoff_caps_at_300_seconds() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            window_secs: 600,
            open_duration_secs: 100,
            half_open_probes: 1,
        };
        let mut breaker = ProviderBreaker::new("cap".into(), config);
        let now = Instant::now();

        // Trip multiple times to test cap.
        breaker.record_failure_at(now);
        assert_eq!(breaker.consecutive_trips(), 1);
        // 100 * 2^0 = 100 (±20% jitter: 80-120s)
        let d = breaker.current_open_duration();
        assert!(d.as_millis() >= 80_000 && d.as_millis() <= 120_000,
            "expected ~100s ±20%, got {:?}", d);

        // Simulate 5 more trips to hit cap (exponent capped at 5).
        for i in 1..=5 {
            breaker.consecutive_trips += 1;
            let expected_base_s = (100u64 * (1u64 << i)).min(300);
            let d = breaker.current_open_duration();
            let min_ms = (expected_base_s as f64 * 1000.0 * 0.8) as u128;
            let max_ms = (expected_base_s as f64 * 1000.0 * 1.2).min(300_000.0) as u128;
            assert!(d.as_millis() >= min_ms && d.as_millis() <= max_ms,
                "trip {} expected ~{}s ±20%, got {:?}",
                breaker.consecutive_trips(), expected_base_s, d);
        }
        // At consecutive_trips=6: 100 * 2^5 = 3200, but capped at 300.
        // With jitter: 240-300s (capped at 300s)
        let d = breaker.current_open_duration();
        assert!(d.as_millis() <= 300_000, "should be capped at 300s, got {:?}", d);
    }

    #[test]
    fn breaker_full_lifecycle_trip_recover_trip() {
        let mut breaker = ProviderBreaker::new("lifecycle".into(), test_config());
        let now = Instant::now();

        // Phase 1: Trip the breaker.
        for _ in 0..3 {
            breaker.record_failure_at(now);
        }
        assert_eq!(breaker.state(), BreakerState::Open);

        // Phase 2: Wait for cooldown, transition to HalfOpen.
        let t1 = now + Duration::from_secs(6);
        breaker.check_at(t1).unwrap();
        assert_eq!(breaker.state(), BreakerState::HalfOpen);

        // Phase 3: Two successful probes → recover.
        breaker.record_success();
        breaker.record_success();
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert_eq!(breaker.failure_count(), 0);

        // Phase 4: Trip again.
        let t2 = t1 + Duration::from_secs(1);
        for _ in 0..3 {
            breaker.record_failure_at(t2);
        }
        assert_eq!(breaker.state(), BreakerState::Open);
    }

    #[test]
    fn jitter_deterministic_in_tests() {
        // In test mode, jitter is disabled for deterministic behavior.
        // 10s base should always return exactly 10000ms.
        for _ in 0..10 {
            let d = ProviderBreaker::apply_jitter(10);
            assert_eq!(
                d.as_millis(),
                10000,
                "jitter should be deterministic in tests"
            );
        }

        // Verify other durations are also deterministic.
        assert_eq!(ProviderBreaker::apply_jitter(5).as_millis(), 5000);
        assert_eq!(ProviderBreaker::apply_jitter(20).as_millis(), 20000);
    }

    #[test]
    fn half_open_rejects_after_probes_exhausted() {
        let mut breaker = ProviderBreaker::new("test".into(), test_config());
        let now = Instant::now();

        // Trip → Open → HalfOpen.
        for _ in 0..3 {
            breaker.record_failure_at(now);
        }
        let after_cooldown = now + Duration::from_secs(6);

        // First check transitions to HalfOpen and uses a probe.
        breaker.check_at(after_cooldown).unwrap();
        assert_eq!(breaker.state(), BreakerState::HalfOpen);

        // Second check uses the second probe.
        breaker.check_at(after_cooldown).unwrap();

        // Third check — no probes remaining — rejected.
        let err = breaker.check_at(after_cooldown);
        assert!(err.is_err());
    }
}
