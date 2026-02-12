//! Health scoring: computes a composite health score (0-100) per provider
//! from recent invocation metrics.
//!
//! Score components:
//! - (1 - error_rate)    × 30  — reliability
//! - latency_score       × 25  — speed (normalized)
//! - (1 - timeout_rate)  × 25  — availability
//! - success_rate        × 20  — consistency

use cuervo_core::types::HealthConfig;
use cuervo_storage::AsyncDatabase;

/// Health level derived from composite score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthLevel {
    /// Score 80-100: provider is operating normally.
    Healthy,
    /// Score between unhealthy_threshold and degraded_threshold.
    Degraded,
    /// Score below unhealthy_threshold.
    Unhealthy,
}

impl std::fmt::Display for HealthLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthLevel::Healthy => write!(f, "healthy"),
            HealthLevel::Degraded => write!(f, "degraded"),
            HealthLevel::Unhealthy => write!(f, "unhealthy"),
        }
    }
}

/// Health assessment for a provider.
#[derive(Debug, Clone)]
pub struct HealthReport {
    pub provider: String,
    pub score: u32,
    pub level: HealthLevel,
    pub error_rate: f64,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: u64,
    pub timeout_rate: f64,
    pub invocation_count: u64,
}

/// Computes health scores from the metrics database.
pub struct HealthScorer {
    db: AsyncDatabase,
    config: HealthConfig,
}

impl HealthScorer {
    pub fn new(db: AsyncDatabase, config: HealthConfig) -> Self {
        Self { db, config }
    }

    /// Compute health for a specific provider.
    ///
    /// If no metrics are available in the window, returns Healthy with score 100
    /// (no data = assume healthy, don't punish new providers).
    pub async fn assess(&self, provider: &str) -> HealthReport {
        let metrics = self
            .db
            .provider_metrics_windowed(provider, self.config.window_minutes)
            .await
            .unwrap_or_default();

        if metrics.total_invocations == 0 {
            return HealthReport {
                provider: provider.to_string(),
                score: 100,
                level: HealthLevel::Healthy,
                error_rate: 0.0,
                avg_latency_ms: 0.0,
                p95_latency_ms: 0,
                timeout_rate: 0.0,
                invocation_count: 0,
            };
        }

        let score = compute_health_score(&metrics);
        let level = self.level_from_score(score);

        HealthReport {
            provider: provider.to_string(),
            score,
            level,
            error_rate: metrics.error_rate,
            avg_latency_ms: metrics.avg_latency_ms,
            p95_latency_ms: metrics.p95_latency_ms,
            timeout_rate: metrics.timeout_rate,
            invocation_count: metrics.total_invocations,
        }
    }

    /// Get the health level for a provider.
    #[allow(dead_code)] // Convenience API — assess() is preferred
    pub async fn level(&self, provider: &str) -> HealthLevel {
        self.assess(provider).await.level
    }

    fn level_from_score(&self, score: u32) -> HealthLevel {
        if score <= self.config.unhealthy_threshold {
            HealthLevel::Unhealthy
        } else if score <= self.config.degraded_threshold {
            HealthLevel::Degraded
        } else {
            HealthLevel::Healthy
        }
    }
}

/// Compute a composite health score from windowed metrics.
///
/// Shared formula used by both the async `HealthScorer` and sync `assess_sync` in doctor.
/// Returns a value in 0-100.
pub fn compute_health_score(metrics: &cuervo_storage::ProviderWindowedMetrics) -> u32 {
    let success_rate = if metrics.total_invocations > 0 {
        metrics.successful_invocations as f64 / metrics.total_invocations as f64
    } else {
        1.0
    };

    let reliability = 1.0 - metrics.error_rate;
    let latency_score = 1.0 / (1.0 + metrics.avg_latency_ms / 2000.0);
    let availability = 1.0 - metrics.timeout_rate;
    let consistency = success_rate;

    let raw = reliability * 30.0
        + latency_score * 25.0
        + availability * 25.0
        + consistency * 20.0;

    // raw is in [0, 100] since each component is [0,1] and weights sum to 100.
    (raw.round() as u32).min(100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use chrono::Utc;
    use cuervo_storage::{Database, InvocationMetric};

    fn test_async_db() -> AsyncDatabase {
        AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()))
    }

    fn insert_metric(db: &cuervo_storage::Database, provider: &str, success: bool, latency_ms: u64, stop_reason: &str) {
        db.insert_metric(&InvocationMetric {
            provider: provider.to_string(),
            model: "test-model".to_string(),
            latency_ms,
            input_tokens: 100,
            output_tokens: 50,
            estimated_cost_usd: 0.001,
            success,
            stop_reason: stop_reason.to_string(),
            session_id: None,
            created_at: Utc::now(),
        })
        .unwrap();
    }

    #[tokio::test]
    async fn healthy_provider_scores_high() {
        let db = test_async_db();
        for _ in 0..10 {
            insert_metric(db.inner(), "good", true, 200, "end_turn");
        }

        let scorer = HealthScorer::new(db, HealthConfig::default());
        let report = scorer.assess("good").await;

        assert!(report.score >= 80, "score={} should be >= 80", report.score);
        assert_eq!(report.level, HealthLevel::Healthy);
        assert_eq!(report.invocation_count, 10);
        assert!(report.error_rate < 0.01);
    }

    #[tokio::test]
    async fn failing_provider_scores_low() {
        let db = test_async_db();
        // 80% failure rate, half of failures are timeouts.
        for _ in 0..2 {
            insert_metric(db.inner(), "flaky", true, 500, "end_turn");
        }
        for _ in 0..4 {
            insert_metric(db.inner(), "flaky", false, 5000, "error");
        }
        for _ in 0..4 {
            insert_metric(db.inner(), "flaky", false, 30000, "timeout");
        }

        let scorer = HealthScorer::new(db, HealthConfig::default());
        let report = scorer.assess("flaky").await;

        // error_rate=0.8→reliability=0.2→6, timeout_rate=0.4→availability=0.6→15,
        // latency high→≈3, success=0.2→consistency=4 → total≈28 → Unhealthy
        assert!(report.score <= 30, "score={} should be <= 30", report.score);
        assert_eq!(report.level, HealthLevel::Unhealthy);
    }

    #[tokio::test]
    async fn slow_provider_degrades_score() {
        let db = test_async_db();
        // All success but very slow (10 seconds avg).
        for _ in 0..10 {
            insert_metric(db.inner(), "slow", true, 10000, "end_turn");
        }

        let scorer = HealthScorer::new(db, HealthConfig::default());
        let report = scorer.assess("slow").await;

        // Reliability+availability+consistency are perfect (75 points max).
        // Latency drags it down: 1/(1+10000/2000) = 1/6 ≈ 0.167 → 0.167 * 25 ≈ 4.
        // Total ≈ 79 → Degraded.
        assert!(report.score < 85, "score={} should be < 85 (slow penalty)", report.score);
    }

    #[tokio::test]
    async fn timeout_rate_impacts_score() {
        let db = test_async_db();
        // 5 success, 5 timeouts.
        for _ in 0..5 {
            insert_metric(db.inner(), "timing_out", true, 200, "end_turn");
        }
        for _ in 0..5 {
            insert_metric(db.inner(), "timing_out", false, 30000, "timeout");
        }

        let scorer = HealthScorer::new(db, HealthConfig::default());
        let report = scorer.assess("timing_out").await;

        assert_eq!(report.timeout_rate, 0.5);
        assert!(report.score <= 50, "score={} should be <= 50 (timeouts)", report.score);
    }

    #[tokio::test]
    async fn empty_metrics_returns_healthy() {
        let db = test_async_db();
        let scorer = HealthScorer::new(db, HealthConfig::default());
        let report = scorer.assess("unknown_provider").await;

        assert_eq!(report.score, 100);
        assert_eq!(report.level, HealthLevel::Healthy);
        assert_eq!(report.invocation_count, 0);
    }

    #[test]
    fn health_level_display() {
        assert_eq!(HealthLevel::Healthy.to_string(), "healthy");
        assert_eq!(HealthLevel::Degraded.to_string(), "degraded");
        assert_eq!(HealthLevel::Unhealthy.to_string(), "unhealthy");
    }

    #[test]
    fn compute_score_shared_formula() {
        use cuervo_storage::ProviderWindowedMetrics;

        // Perfect provider: 0 errors, 0 timeouts, low latency.
        let perfect = ProviderWindowedMetrics {
            provider: "test".to_string(),
            total_invocations: 10,
            successful_invocations: 10,
            failed_invocations: 0,
            timeout_count: 0,
            error_rate: 0.0,
            avg_latency_ms: 100.0,
            p95_latency_ms: 150,
            timeout_rate: 0.0,
        };
        let score = compute_health_score(&perfect);
        assert!(score >= 90, "perfect provider should score >= 90, got {score}");

        // Terrible provider: 100% errors, 100% timeouts.
        let terrible = ProviderWindowedMetrics {
            provider: "bad".to_string(),
            total_invocations: 10,
            successful_invocations: 0,
            failed_invocations: 10,
            timeout_count: 10,
            error_rate: 1.0,
            avg_latency_ms: 30000.0,
            p95_latency_ms: 60000,
            timeout_rate: 1.0,
        };
        let terrible_score = compute_health_score(&terrible);
        assert!(
            terrible_score <= 5,
            "terrible provider should score <= 5, got {terrible_score}"
        );
    }
}
