//! Background aggregation worker for time-series metrics.
//!
//! Periodically aggregates query instrumentations into time-series snapshots,
//! computes trends, and detects regressions.

use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time;

use super::{
    AggregationWindow, MetricsSnapshot, ObservabilityStore, QueryMetrics, RegressionDetector,
    SnapshotStore, TimeSeriesMetrics,
};
use crate::Result;

/// Configuration for the metrics aggregator.
#[derive(Debug, Clone)]
pub struct AggregatorConfig {
    /// Aggregation window size.
    pub window: AggregationWindow,

    /// Maximum number of windows to retain in memory.
    pub max_windows: usize,

    /// Aggregation interval (how often to aggregate).
    pub interval_secs: u64,

    /// Enable regression detection.
    pub detect_regressions: bool,

    /// Minimum number of instrumentations per window to compute metrics.
    pub min_samples_per_window: usize,

    /// Enable snapshot persistence.
    pub persist_snapshots: bool,
}

impl Default for AggregatorConfig {
    fn default() -> Self {
        Self {
            window: AggregationWindow::FiveMinutes,
            max_windows: 288,   // 24 hours of 5-minute windows
            interval_secs: 300, // 5 minutes
            detect_regressions: true,
            min_samples_per_window: 1,
            persist_snapshots: true,
        }
    }
}

/// Background worker that periodically aggregates metrics.
pub struct MetricsAggregator {
    config: AggregatorConfig,
    store: Arc<ObservabilityStore>,
    snapshot_store: Arc<SnapshotStore>,
    timeseries: Arc<RwLock<TimeSeriesMetrics>>,
    detector: RegressionDetector,
}

impl MetricsAggregator {
    /// Create a new metrics aggregator.
    pub fn new(
        config: AggregatorConfig,
        store: Arc<ObservabilityStore>,
        snapshot_store: Arc<SnapshotStore>,
    ) -> Self {
        let timeseries = TimeSeriesMetrics::new(config.window, config.max_windows);

        Self {
            config,
            store,
            snapshot_store,
            timeseries: Arc::new(RwLock::new(timeseries)),
            detector: RegressionDetector::new(),
        }
    }

    /// Get a clone of the current time-series (for read-only access).
    pub async fn timeseries_snapshot(&self) -> TimeSeriesMetrics {
        self.timeseries.read().await.clone()
    }

    /// Run the aggregation worker loop.
    ///
    /// This spawns a background task that runs indefinitely, aggregating metrics
    /// at the configured interval.
    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = time::interval(time::Duration::from_secs(self.config.interval_secs));

            loop {
                interval.tick().await;

                if let Err(e) = self.aggregate_once().await {
                    tracing::error!("Metrics aggregation failed: {}", e);
                }
            }
        })
    }

    /// Perform a single aggregation cycle.
    ///
    /// 1. Fetch recent instrumentations from the current window
    /// 2. Compute aggregate metrics (QueryMetrics)
    /// 3. Push to time-series
    /// 4. Detect regressions (if enabled)
    /// 5. Persist alerts
    async fn aggregate_once(&self) -> Result<()> {
        let now = Utc::now();
        let window_start = now - self.config.window.duration();

        tracing::debug!(
            "Aggregating metrics for window: {} to {}",
            window_start.to_rfc3339(),
            now.to_rfc3339()
        );

        // Fetch instrumentations from current window
        let instrumentations = self
            .fetch_window_instrumentations(window_start, now)
            .await?;

        if instrumentations.len() < self.config.min_samples_per_window {
            tracing::debug!(
                "Skipping aggregation: insufficient samples ({} < {})",
                instrumentations.len(),
                self.config.min_samples_per_window
            );
            return Ok(());
        }

        // Compute aggregate metrics
        let metrics = self.compute_window_metrics(&instrumentations, window_start, now);

        tracing::info!(
            "Aggregated {} queries: avg_duration={:.1}ms, success_rate={:.1}%, quality={:.3}",
            metrics.total_queries,
            metrics.avg_duration_ms,
            metrics.success_rate() * 100.0,
            metrics.avg_quality_score.unwrap_or(0.0)
        );

        // Push to time-series
        {
            let mut ts = self.timeseries.write().await;
            ts.push(metrics.clone());
        }

        // Detect regressions
        if self.config.detect_regressions {
            let ts_snapshot = self.timeseries.read().await.clone();
            let alerts = self.detector.detect(&ts_snapshot, &metrics);

            if !alerts.is_empty() {
                tracing::warn!("Detected {} regression(s) in current window", alerts.len());

                for alert in &alerts {
                    tracing::warn!(
                        "Regression: {} (severity: {:?})",
                        alert.message,
                        alert.severity
                    );
                    self.store.record_alert(alert).await?;
                }
            }
        }

        // Persist snapshot
        if self.config.persist_snapshots {
            let ts_snapshot = self.timeseries.read().await.clone();
            let snapshot = MetricsSnapshot::from_timeseries(&ts_snapshot, metrics.clone());
            self.snapshot_store.save(&snapshot).await?;

            tracing::debug!(
                "Persisted metrics snapshot: {} windows, quality trend: {:?}",
                snapshot.window_count,
                snapshot.quality_trend
            );
        }

        Ok(())
    }

    /// Fetch instrumentations from the specified time window.
    async fn fetch_window_instrumentations(
        &self,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<super::QueryInstrumentation>> {
        // For now, we fetch the most recent N instrumentations and filter by time
        // In production, this should be a SQL query with WHERE started_at >= ? AND started_at < ?
        let recent = self.store.get_recent_instrumentations(1000).await?;

        let filtered: Vec<_> = recent
            .into_iter()
            .filter(|instr| instr.started_at >= window_start && instr.started_at < window_end)
            .collect();

        Ok(filtered)
    }

    /// Compute aggregate metrics from a set of instrumentations.
    fn compute_window_metrics(
        &self,
        instrumentations: &[super::QueryInstrumentation],
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> QueryMetrics {
        let mut durations = Vec::new();
        let mut result_counts = Vec::new();
        let mut quality_scores = Vec::new();
        let mut failed_count = 0u64;

        for instr in instrumentations {
            if let Some(duration) = instr.duration_ms {
                if instr.is_success() {
                    durations.push(duration);
                    result_counts.push(instr.result_count);

                    if let (Some(quality), Some(precision), Some(recall), Some(ndcg)) = (
                        instr.quality_score,
                        instr.context_precision,
                        instr.context_recall,
                        instr.ndcg_at_10,
                    ) {
                        quality_scores.push((quality, precision, recall, ndcg));
                    }
                } else {
                    failed_count += 1;
                }
            }
        }

        let quality_scores_opt = if !quality_scores.is_empty() {
            Some(quality_scores.as_slice())
        } else {
            None
        };

        QueryMetrics::from_data(
            &durations,
            failed_count,
            &result_counts,
            quality_scores_opt,
            window_start,
            window_end,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use crate::observability::{QueryInstrumentation, SnapshotStore};
    use halcon_storage::Database;

    async fn setup_test_stores() -> (Arc<ObservabilityStore>, Arc<SnapshotStore>) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.with_connection(|conn| {
            halcon_storage::migrations::run_migrations(conn).unwrap();
            Ok::<(), rusqlite::Error>(())
        })
        .unwrap();
        let obs_store = Arc::new(ObservabilityStore::new(db.clone()));
        let snap_store = Arc::new(SnapshotStore::new(db));
        (obs_store, snap_store)
    }

    #[tokio::test]
    async fn test_aggregator_new() {
        let (store, snap_store) = setup_test_stores().await;
        let config = AggregatorConfig::default();
        let aggregator = MetricsAggregator::new(config.clone(), store, snap_store);

        let ts = aggregator.timeseries_snapshot().await;
        assert_eq!(ts.window, config.window);
        assert_eq!(ts.max_windows, config.max_windows);
        assert_eq!(ts.len(), 0);
    }

    #[tokio::test]
    async fn test_compute_window_metrics() {
        let (store, snap_store) = setup_test_stores().await;
        let config = AggregatorConfig::default();
        let aggregator = MetricsAggregator::new(config, store, snap_store);

        let now = Utc::now();
        let window_start = now - Duration::minutes(5);

        let mut instrumentations = Vec::new();

        // Add 5 successful queries
        for i in 0..5 {
            let mut instr = QueryInstrumentation::new(format!("query {}", i));
            instr.complete(10 + i);
            instr.duration_ms = Some(100 + i as u64 * 10);
            instr.set_quality_metrics(0.85 + i as f64 * 0.01, 0.90, 0.88, 0.82);
            instrumentations.push(instr);
        }

        // Add 1 failed query
        let mut failed = QueryInstrumentation::new("failed".to_string());
        failed.fail("error".to_string());
        failed.duration_ms = Some(50);
        instrumentations.push(failed);

        let metrics = aggregator.compute_window_metrics(&instrumentations, window_start, now);

        assert_eq!(metrics.total_queries, 6);
        assert_eq!(metrics.successful_queries, 5);
        assert_eq!(metrics.failed_queries, 1);
        assert_eq!(metrics.avg_duration_ms, 120.0); // (100+110+120+130+140)/5
        assert!((metrics.avg_quality_score.unwrap() - 0.87).abs() < 0.01);
        assert!((metrics.success_rate() - 5.0 / 6.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_aggregate_once_insufficient_samples() {
        let (store, snap_store) = setup_test_stores().await;
        let mut config = AggregatorConfig::default();
        config.min_samples_per_window = 10;

        let aggregator = Arc::new(MetricsAggregator::new(config, store, snap_store));

        // Should succeed but skip aggregation (no samples in DB)
        let result = aggregator.aggregate_once().await;
        assert!(result.is_ok());

        // Time-series should still be empty
        let ts = aggregator.timeseries_snapshot().await;
        assert_eq!(ts.len(), 0);
    }

    #[tokio::test]
    async fn test_aggregate_once_with_samples() {
        let (store, snap_store) = setup_test_stores().await;
        let config = AggregatorConfig::default();
        let aggregator = Arc::new(MetricsAggregator::new(config, store.clone(), snap_store));

        // Insert some instrumentations
        for i in 0..5 {
            let mut instr = QueryInstrumentation::new(format!("query {}", i));
            instr.complete(10 + i);
            instr.duration_ms = Some(100 + i as u64 * 10);
            instr.set_quality_metrics(0.85, 0.90, 0.88, 0.82);
            store.record_instrumentation(&instr).await.unwrap();
        }

        // Run aggregation
        let result = aggregator.aggregate_once().await;
        assert!(result.is_ok());

        // Time-series should have 1 entry
        let ts = aggregator.timeseries_snapshot().await;
        assert_eq!(ts.len(), 1);

        let latest = ts.latest().unwrap();
        assert_eq!(latest.total_queries, 5);
        assert_eq!(latest.successful_queries, 5);
    }

    #[tokio::test]
    async fn test_fetch_window_instrumentations() {
        let (store, snap_store) = setup_test_stores().await;
        let config = AggregatorConfig::default();
        let aggregator = MetricsAggregator::new(config, store.clone(), snap_store);

        let now = Utc::now();
        let window_start = now - Duration::minutes(5);

        // Insert 3 instrumentations within window
        for i in 0..3 {
            let mut instr = QueryInstrumentation::new(format!("query {}", i));
            instr.started_at = window_start + Duration::minutes(i);
            instr.complete(10);
            store.record_instrumentation(&instr).await.unwrap();
        }

        // Insert 1 instrumentation outside window (older)
        let mut old_instr = QueryInstrumentation::new("old query".to_string());
        old_instr.started_at = window_start - Duration::hours(1);
        old_instr.complete(10);
        store.record_instrumentation(&old_instr).await.unwrap();

        let instrumentations = aggregator
            .fetch_window_instrumentations(window_start, now)
            .await
            .unwrap();

        assert_eq!(instrumentations.len(), 3);
    }
}
