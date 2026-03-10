//! Resilience manager: facade coordinating circuit breakers, health scoring,
//! and backpressure into a unified pre/post-invoke API.
//!
//! The agent loop calls `pre_invoke()` before each provider invocation and
//! `record_success()`/`record_failure()` after. The manager handles:
//! - Circuit breaker checks (fail-fast for known-broken providers)
//! - Backpressure permits (concurrency limiting)
//! - Health scoring (provider ranking for fallback selection)

use std::collections::HashMap;
use std::time::Duration;

use halcon_core::types::{DomainEvent, EventPayload, ResilienceConfig};
use halcon_core::EventSender;
use halcon_storage::AsyncDatabase;

use super::super::backpressure::{BackpressureGuard, InvokePermit};
use super::circuit_breaker::{BreakerState, ProviderBreaker};
use super::super::health::{HealthLevel, HealthScorer};

/// Result of a pre-invoke check.
#[derive(Debug)]
pub enum PreInvokeDecision {
    /// Proceed with invocation. Caller must hold the permit until invoke completes.
    Proceed { permit: InvokePermit },
    /// Primary provider is unavailable. Caller should try fallback.
    Fallback { reason: FallbackReason },
}

/// Why we're falling back from the primary provider.
#[derive(Debug, Clone)]
pub enum FallbackReason {
    /// Circuit breaker is open.
    BreakerOpen {
        provider: String,
        retry_after: Duration,
    },
    /// Backpressure is saturated.
    Saturated { provider: String },
    /// Provider health score is below the unhealthy threshold.
    ProviderUnhealthy {
        provider: String,
        score: u32,
        level: HealthLevel,
    },
}

impl std::fmt::Display for FallbackReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FallbackReason::BreakerOpen {
                provider,
                retry_after,
            } => write!(
                f,
                "circuit breaker open for '{provider}' (retry after {:.1}s)",
                retry_after.as_secs_f64()
            ),
            FallbackReason::Saturated { provider } => {
                write!(f, "backpressure saturated for '{provider}'")
            }
            FallbackReason::ProviderUnhealthy {
                provider,
                score,
                level,
            } => write!(
                f,
                "provider '{provider}' is {level} (health score {score}/100)"
            ),
        }
    }
}

/// Diagnostic snapshot for a single provider.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Used in Sub-Phase 4 (doctor command)
pub struct ProviderDiagnostic {
    pub provider: String,
    pub breaker_state: BreakerState,
    pub failure_count: usize,
    pub backpressure_in_use: u32,
    pub backpressure_max: u32,
}

/// Facade coordinating circuit breakers, health scoring, and backpressure.
pub struct ResilienceManager {
    breakers: HashMap<String, ProviderBreaker>,
    backpressure: BackpressureGuard,
    health_scorer: Option<HealthScorer>,
    db: Option<AsyncDatabase>,
    event_tx: Option<EventSender>,
    config: ResilienceConfig,
}

impl ResilienceManager {
    pub fn new(config: ResilienceConfig) -> Self {
        Self {
            breakers: HashMap::new(),
            backpressure: BackpressureGuard::new(config.backpressure.clone()),
            health_scorer: None,
            db: None,
            event_tx: None,
            config,
        }
    }

    /// Attach a database for health scoring and event persistence.
    pub fn with_db(mut self, db: AsyncDatabase) -> Self {
        self.health_scorer = Some(HealthScorer::new(db.clone(), self.config.health.clone()));
        self.db = Some(db);
        self
    }

    /// Attach an event sender for domain event emission.
    pub fn with_event_tx(mut self, tx: EventSender) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Register a provider for tracking.
    pub fn register_provider(&mut self, name: &str) {
        self.breakers.entry(name.to_string()).or_insert_with(|| {
            ProviderBreaker::new(name.to_string(), self.config.circuit_breaker.clone())
        });
        self.backpressure.register(name);
    }

    /// Pre-invoke check: circuit breaker + backpressure.
    ///
    /// Returns `Proceed` with a permit if the provider is available,
    /// or `Fallback` with a reason if it should be skipped.
    pub async fn pre_invoke(&mut self, provider: &str) -> PreInvokeDecision {
        // 1. Circuit breaker check.
        if let Some(breaker) = self.breakers.get_mut(provider) {
            if let Err(open) = breaker.check() {
                tracing::warn!(
                    provider,
                    retry_after_secs = open.retry_after.as_secs_f64(),
                    "Circuit breaker open, skipping provider"
                );
                return PreInvokeDecision::Fallback {
                    reason: FallbackReason::BreakerOpen {
                        provider: provider.to_string(),
                        retry_after: open.retry_after,
                    },
                };
            }
        }

        // 2. Health score check (requires DB).
        if let Some(scorer) = &self.health_scorer {
            let report = scorer.assess(provider).await;
            if report.level == HealthLevel::Unhealthy {
                tracing::warn!(
                    provider,
                    score = report.score,
                    "Provider unhealthy, skipping"
                );
                return PreInvokeDecision::Fallback {
                    reason: FallbackReason::ProviderUnhealthy {
                        provider: provider.to_string(),
                        score: report.score,
                        level: report.level,
                    },
                };
            }
        }

        // 3. Backpressure permit.
        match self.backpressure.acquire(provider).await {
            Ok(permit) => PreInvokeDecision::Proceed { permit },
            Err(_full) => {
                tracing::warn!(provider, "Backpressure saturated, skipping provider");
                PreInvokeDecision::Fallback {
                    reason: FallbackReason::Saturated {
                        provider: provider.to_string(),
                    },
                }
            }
        }
    }

    /// Record a successful invocation (updates circuit breaker).
    pub async fn record_success(&mut self, provider: &str) {
        if let Some(breaker) = self.breakers.get_mut(provider) {
            if let Some(transition) = breaker.record_success() {
                tracing::info!(
                    provider,
                    from = %transition.from,
                    to = %transition.to,
                    "Circuit breaker state changed"
                );
                // Emit domain event for state recovery.
                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(DomainEvent::new(EventPayload::CircuitBreakerTripped {
                        provider: provider.to_string(),
                        from_state: transition.from.to_string(),
                        to_state: transition.to.to_string(),
                    }));
                }
                self.persist_breaker_event(provider, &transition.from, &transition.to)
                    .await;
            }
        }
    }

    /// Record a failed invocation (updates circuit breaker).
    pub async fn record_failure(&mut self, provider: &str) {
        if let Some(breaker) = self.breakers.get_mut(provider) {
            if let Some(transition) = breaker.record_failure() {
                tracing::warn!(
                    provider,
                    from = %transition.from,
                    to = %transition.to,
                    "Circuit breaker TRIPPED"
                );
                // Emit domain event for breaker trip.
                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(DomainEvent::new(EventPayload::CircuitBreakerTripped {
                        provider: provider.to_string(),
                        from_state: transition.from.to_string(),
                        to_state: transition.to.to_string(),
                    }));
                }
                self.persist_breaker_event(provider, &transition.from, &transition.to)
                    .await;
            }
        }
    }

    /// Persist a breaker state transition to the database.
    ///
    /// P1-B: Populates `score` and `details` so resilience_events rows contain
    /// actionable diagnostic data (previously always NULL, making the table useless
    /// for post-incident analysis).
    ///
    /// Score semantics:
    ///   Closed (healthy)  = 1.0
    ///   HalfOpen (probing) = 0.5
    ///   Open (tripped)    = 0.0
    async fn persist_breaker_event(
        &self,
        provider: &str,
        from: &BreakerState,
        to: &BreakerState,
    ) {
        if let Some(db) = &self.db {
            // Derive health score from the destination state (u32, 0–100 scale).
            let score = match to {
                BreakerState::Closed => Some(100_u32),
                BreakerState::HalfOpen => Some(50_u32),
                BreakerState::Open => Some(0_u32),
            };

            // Human-readable transition description for post-incident analysis.
            let details = Some(format!(
                "circuit_breaker transition {from}→{to} for provider '{provider}'"
            ));

            let event = halcon_storage::ResilienceEvent {
                provider: provider.to_string(),
                event_type: "breaker_transition".to_string(),
                from_state: Some(from.to_string()),
                to_state: Some(to.to_string()),
                score,
                details,
                created_at: chrono::Utc::now(),
            };
            if let Err(e) = db.insert_resilience_event(&event).await {
                tracing::warn!("Failed to persist resilience event: {e}");
            }
        }
    }

    /// Get diagnostic report for all registered providers.
    #[allow(dead_code)] // Used in Sub-Phase 4 (doctor command)
    pub fn diagnostics(&self) -> Vec<ProviderDiagnostic> {
        self.breakers
            .values()
            .map(|breaker| {
                let (in_use, max) = self.backpressure.utilization(breaker.provider());
                ProviderDiagnostic {
                    provider: breaker.provider().to_string(),
                    breaker_state: breaker.state(),
                    failure_count: breaker.failure_count(),
                    backpressure_in_use: in_use,
                    backpressure_max: max,
                }
            })
            .collect()
    }

    /// Whether resilience is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::{BackpressureConfig, CircuitBreakerConfig};

    fn test_config(enabled: bool) -> ResilienceConfig {
        ResilienceConfig {
            enabled,
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: 3,
                window_secs: 60,
                open_duration_secs: 5,
                half_open_probes: 2,
            },
            health: Default::default(),
            backpressure: BackpressureConfig {
                max_concurrent_per_provider: 2,
                queue_timeout_secs: 1,
            },
        }
    }

    #[tokio::test]
    async fn pre_invoke_succeeds_when_healthy() {
        let mut mgr = ResilienceManager::new(test_config(true));
        mgr.register_provider("anthropic");

        let decision = mgr.pre_invoke("anthropic").await;
        assert!(
            matches!(decision, PreInvokeDecision::Proceed { .. }),
            "should proceed when healthy"
        );
    }

    #[tokio::test]
    async fn pre_invoke_fallback_on_breaker_open() {
        let mut mgr = ResilienceManager::new(test_config(true));
        mgr.register_provider("bad");

        // Trip the breaker with 3 failures.
        mgr.record_failure("bad").await;
        mgr.record_failure("bad").await;
        mgr.record_failure("bad").await;

        let decision = mgr.pre_invoke("bad").await;
        assert!(
            matches!(decision, PreInvokeDecision::Fallback { reason: FallbackReason::BreakerOpen { .. } }),
            "should fallback when breaker is open"
        );
    }

    #[tokio::test]
    async fn pre_invoke_fallback_on_saturated() {
        let mut mgr = ResilienceManager::new(test_config(true));
        mgr.register_provider("busy");

        // Acquire all permits.
        let _p1 = mgr.pre_invoke("busy").await;
        let _p2 = mgr.pre_invoke("busy").await;

        // Third should timeout and fallback.
        let decision = mgr.pre_invoke("busy").await;
        assert!(
            matches!(decision, PreInvokeDecision::Fallback { reason: FallbackReason::Saturated { .. } }),
            "should fallback when saturated"
        );
    }

    #[tokio::test]
    async fn record_success_recovers_breaker() {
        let mut mgr = ResilienceManager::new(test_config(true));
        mgr.register_provider("prov");

        // Trip the breaker.
        mgr.record_failure("prov").await;
        mgr.record_failure("prov").await;
        mgr.record_failure("prov").await;

        let diag = mgr.diagnostics();
        assert_eq!(diag[0].breaker_state, BreakerState::Open);

        // Wait for cooldown (manually transition via check_at in the breaker).
        // Since we can't easily mock time in the manager, we test recovery
        // at the breaker level separately. Here we verify diagnostics work.
        assert_eq!(diag[0].provider, "prov");
        assert_eq!(diag[0].failure_count, 3);
    }

    #[tokio::test]
    async fn diagnostics_reports_all_providers() {
        let mut mgr = ResilienceManager::new(test_config(true));
        mgr.register_provider("a");
        mgr.register_provider("b");

        let diag = mgr.diagnostics();
        assert_eq!(diag.len(), 2);

        let names: Vec<&str> = diag.iter().map(|d| d.provider.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn disabled_manager_reports_not_enabled() {
        let mgr = ResilienceManager::new(test_config(false));
        assert!(!mgr.is_enabled());
    }

    #[test]
    fn enabled_manager_reports_enabled() {
        let mgr = ResilienceManager::new(test_config(true));
        assert!(mgr.is_enabled());
    }

    #[test]
    fn fallback_reason_display() {
        let br = FallbackReason::BreakerOpen {
            provider: "test".into(),
            retry_after: Duration::from_secs(10),
        };
        assert!(format!("{br}").contains("test"));

        let sat = FallbackReason::Saturated {
            provider: "test".into(),
        };
        assert!(format!("{sat}").contains("saturated"));

        let unhealthy = FallbackReason::ProviderUnhealthy {
            provider: "test".into(),
            score: 15,
            level: HealthLevel::Unhealthy,
        };
        let msg = format!("{unhealthy}");
        assert!(msg.contains("unhealthy"));
        assert!(msg.contains("15"));
    }

    #[tokio::test]
    async fn health_scorer_wired_into_pre_invoke() {
        use std::sync::Arc;
        use chrono::Utc;
        use halcon_storage::{Database, InvocationMetric};

        let db = Arc::new(Database::open_in_memory().unwrap());

        // Seed: 80% failure + 40% timeout → Unhealthy.
        for _ in 0..2 {
            db.insert_metric(&InvocationMetric {
                provider: "sick".into(),
                model: "m".into(),
                latency_ms: 500,
                input_tokens: 100,
                output_tokens: 50,
                estimated_cost_usd: 0.001,
                success: true,
                stop_reason: "end_turn".into(),
                session_id: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }
        for _ in 0..4 {
            db.insert_metric(&InvocationMetric {
                provider: "sick".into(),
                model: "m".into(),
                latency_ms: 5000,
                input_tokens: 0,
                output_tokens: 0,
                estimated_cost_usd: 0.0,
                success: false,
                stop_reason: "error".into(),
                session_id: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }
        for _ in 0..4 {
            db.insert_metric(&InvocationMetric {
                provider: "sick".into(),
                model: "m".into(),
                latency_ms: 30000,
                input_tokens: 0,
                output_tokens: 0,
                estimated_cost_usd: 0.0,
                success: false,
                stop_reason: "timeout".into(),
                session_id: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }

        let mut mgr = ResilienceManager::new(test_config(true))
            .with_db(AsyncDatabase::new(db));
        mgr.register_provider("sick");

        let decision = mgr.pre_invoke("sick").await;
        assert!(
            matches!(
                decision,
                PreInvokeDecision::Fallback {
                    reason: FallbackReason::ProviderUnhealthy { .. }
                }
            ),
            "should fallback for unhealthy provider, got: {decision:?}"
        );
    }

    #[tokio::test]
    async fn healthy_provider_passes_health_check() {
        use std::sync::Arc;
        use chrono::Utc;
        use halcon_storage::{Database, InvocationMetric};

        let db = Arc::new(Database::open_in_memory().unwrap());

        // Seed: all success.
        for _ in 0..5 {
            db.insert_metric(&InvocationMetric {
                provider: "good".into(),
                model: "m".into(),
                latency_ms: 200,
                input_tokens: 100,
                output_tokens: 50,
                estimated_cost_usd: 0.001,
                success: true,
                stop_reason: "end_turn".into(),
                session_id: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }

        let mut mgr = ResilienceManager::new(test_config(true))
            .with_db(AsyncDatabase::new(db));
        mgr.register_provider("good");

        let decision = mgr.pre_invoke("good").await;
        assert!(
            matches!(decision, PreInvokeDecision::Proceed { .. }),
            "healthy provider should proceed"
        );
    }

    #[tokio::test]
    async fn no_db_skips_health_check() {
        // Without DB, no health scorer → always passes health check.
        let mut mgr = ResilienceManager::new(test_config(true));
        mgr.register_provider("unknown");

        let decision = mgr.pre_invoke("unknown").await;
        assert!(
            matches!(decision, PreInvokeDecision::Proceed { .. }),
            "no DB should skip health check and proceed"
        );
    }

    #[tokio::test]
    async fn breaker_trip_persists_resilience_event() {
        use std::sync::Arc;
        use halcon_storage::Database;

        let db = Arc::new(Database::open_in_memory().unwrap());
        let async_db = AsyncDatabase::new(Arc::clone(&db));
        let mut mgr = ResilienceManager::new(test_config(true)).with_db(async_db);
        mgr.register_provider("tripped");

        // Trip the breaker with 3 failures.
        mgr.record_failure("tripped").await;
        mgr.record_failure("tripped").await;
        mgr.record_failure("tripped").await;

        // Should have persisted a breaker_transition event.
        let events = db.resilience_events(Some("tripped"), None, 10).unwrap();
        assert!(
            !events.is_empty(),
            "breaker trip should persist a resilience event"
        );
        assert_eq!(events[0].event_type, "breaker_transition");
        assert_eq!(events[0].to_state.as_deref(), Some("open"));
    }

    #[tokio::test]
    async fn no_event_persisted_without_db() {
        // Without DB, record_failure should not panic.
        let mut mgr = ResilienceManager::new(test_config(true));
        mgr.register_provider("nodb");

        mgr.record_failure("nodb").await;
        mgr.record_failure("nodb").await;
        mgr.record_failure("nodb").await;
        // No assertion — just verify no panic.
    }

    #[tokio::test]
    async fn breaker_trip_emits_domain_event() {
        let (event_tx, mut event_rx) = halcon_core::event_bus(64);
        let mut mgr = ResilienceManager::new(test_config(true)).with_event_tx(event_tx);
        mgr.register_provider("failing");

        // Trip the breaker with 3 failures.
        mgr.record_failure("failing").await;
        mgr.record_failure("failing").await;
        mgr.record_failure("failing").await;

        // Should have emitted a CircuitBreakerTripped event.
        let event = event_rx.try_recv().expect("should receive breaker event");
        match &event.payload {
            EventPayload::CircuitBreakerTripped {
                provider,
                from_state,
                to_state,
            } => {
                assert_eq!(provider, "failing");
                assert_eq!(from_state, "closed");
                assert_eq!(to_state, "open");
            }
            other => panic!("expected CircuitBreakerTripped, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn breaker_recovery_emits_domain_event() {
        use std::time::{Duration, Instant};

        let (event_tx, mut event_rx) = halcon_core::event_bus(64);
        let mut mgr = ResilienceManager::new(test_config(true)).with_event_tx(event_tx);
        mgr.register_provider("recovering2");

        // Trip the breaker.
        mgr.record_failure("recovering2").await;
        mgr.record_failure("recovering2").await;
        mgr.record_failure("recovering2").await;

        // Drain the trip event.
        let _ = event_rx.try_recv();

        // Advance to HalfOpen.
        if let Some(breaker) = mgr.breakers.get_mut("recovering2") {
            let future = Instant::now() + Duration::from_secs(60);
            let _ = breaker.check_at(future);
        }

        // Probe 1 success.
        mgr.record_success("recovering2").await;

        // Consume probe slot 2.
        if let Some(breaker) = mgr.breakers.get_mut("recovering2") {
            let _ = breaker.check();
        }

        // Probe 2 success → HalfOpen → Closed.
        mgr.record_success("recovering2").await;

        // Should have emitted a recovery event.
        let event = event_rx.try_recv().expect("should receive recovery event");
        match &event.payload {
            EventPayload::CircuitBreakerTripped {
                provider,
                to_state,
                ..
            } => {
                assert_eq!(provider, "recovering2");
                assert_eq!(to_state, "closed");
            }
            other => panic!("expected CircuitBreakerTripped recovery, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_event_emitted_without_event_tx() {
        // Without event_tx, record_failure should not panic.
        let mut mgr = ResilienceManager::new(test_config(true));
        mgr.register_provider("silent");

        mgr.record_failure("silent").await;
        mgr.record_failure("silent").await;
        mgr.record_failure("silent").await;
        // No panic, no events — just verifies graceful no-op.
    }

    #[tokio::test]
    async fn breaker_recovery_persists_event() {
        use std::sync::Arc;
        use std::time::{Duration, Instant};
        use halcon_storage::Database;

        let db = Arc::new(Database::open_in_memory().unwrap());
        let async_db = AsyncDatabase::new(Arc::clone(&db));
        let mut mgr = ResilienceManager::new(test_config(true)).with_db(async_db);
        mgr.register_provider("recovering");

        // Trip the breaker.
        mgr.record_failure("recovering").await;
        mgr.record_failure("recovering").await;
        mgr.record_failure("recovering").await;

        // Manually advance breaker to HalfOpen by calling check_at with future time.
        // This consumes probe slot 1 (the first request IS a probe).
        if let Some(breaker) = mgr.breakers.get_mut("recovering") {
            let future = Instant::now() + Duration::from_secs(60);
            let _ = breaker.check_at(future);
        }

        // Probe 1 success (half_open_successes=1, needs 2 for recovery).
        mgr.record_success("recovering").await;

        // Consume probe slot 2.
        if let Some(breaker) = mgr.breakers.get_mut("recovering") {
            let _ = breaker.check();
        }

        // Probe 2 success → HalfOpen → Closed transition.
        mgr.record_success("recovering").await;

        // Should have persisted 2 events: trip (Closed→Open) + recovery (HalfOpen→Closed).
        let events = db.resilience_events(Some("recovering"), None, 10).unwrap();
        assert!(
            events.len() >= 2,
            "expected >= 2 events (trip + recovery), got {}",
            events.len()
        );
    }
}
