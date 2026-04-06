//! Prometheus metrics for Halcon CLI bridge relay.
//!
//! Exports 10+ critical metrics for monitoring production systems.

use lazy_static::lazy_static;
use prometheus::{
    register_counter_vec, register_gauge_vec, register_histogram_vec, register_int_gauge,
    CounterVec, Encoder, GaugeVec, HistogramVec, IntGauge, TextEncoder,
};
use std::sync::Arc;
use tokio::sync::RwLock;

// ───────────────────────────────────────────────────────────────────────────
// METRICS REGISTRY
// ───────────────────────────────────────────────────────────────────────────

lazy_static! {
    // Counter: Events processed total
    pub static ref EVENTS_PROCESSED_TOTAL: CounterVec = register_counter_vec!(
        "halcon_events_processed_total",
        "Total number of events processed by status",
        &["status"]  // success, failed, timeout
    )
    .unwrap();

    // Counter: Events failed total
    pub static ref EVENTS_FAILED_TOTAL: CounterVec = register_counter_vec!(
        "halcon_events_failed_total",
        "Total number of failed events by reason",
        &["reason"]  // tool_error, timeout, ack_timeout, network_error
    )
    .unwrap();

    // Counter: ACK timeouts
    pub static ref ACK_TIMEOUT_TOTAL: CounterVec = register_counter_vec!(
        "halcon_ack_timeout_total",
        "Total number of ACK timeouts by severity",
        &["severity"]  // warning, escalated
    )
    .unwrap();

    // Gauge: DLQ size
    pub static ref DLQ_SIZE: GaugeVec = register_gauge_vec!(
        "halcon_dlq_size",
        "Current size of Dead Letter Queue by status",
        &["status"]  // pending, exhausted, manual
    )
    .unwrap();

    // Counter: Retry count
    pub static ref RETRY_COUNT_TOTAL: CounterVec = register_counter_vec!(
        "halcon_retry_count_total",
        "Total number of retries by outcome",
        &["outcome"]  // success, failed, exhausted
    )
    .unwrap();

    // Histogram: Event latency (created → acked)
    pub static ref EVENT_LATENCY: HistogramVec = register_histogram_vec!(
        "halcon_event_latency_seconds",
        "Event processing latency from created to acked",
        &["status"],  // success, timeout
        vec![0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0]
    )
    .unwrap();

    // Histogram: End-to-end latency (task received → result acked)
    pub static ref E2E_LATENCY: HistogramVec = register_histogram_vec!(
        "halcon_e2e_latency_seconds",
        "End-to-end task latency from delegation to result ACK",
        &["task_type"],  // bash, file_read, etc.
        vec![0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0]
    )
    .unwrap();

    // Gauge: Circuit breaker state
    pub static ref CIRCUIT_BREAKER_STATE: GaugeVec = register_gauge_vec!(
        "halcon_circuit_breaker_state",
        "Circuit breaker state (0=closed, 1=half-open, 2=open)",
        &["resource"]  // redis, backend, etc.
    )
    .unwrap();

    // Gauge: Active connections
    pub static ref ACTIVE_CONNECTIONS: IntGauge = register_int_gauge!(
        "halcon_active_connections",
        "Number of active WebSocket connections"
    )
    .unwrap();

    // Counter: Error rate
    pub static ref ERROR_RATE: CounterVec = register_counter_vec!(
        "halcon_error_rate_total",
        "Total errors by type",
        &["error_type"]  // websocket, db, redis, tool_execution
    )
    .unwrap();

    // Gauge: Event buffer size
    pub static ref EVENT_BUFFER_SIZE: GaugeVec = register_gauge_vec!(
        "halcon_event_buffer_size",
        "Current event buffer size by status",
        &["status"]  // pending, sent, acked
    )
    .unwrap();

    // Histogram: Task execution duration
    pub static ref TASK_EXECUTION_DURATION: HistogramVec = register_histogram_vec!(
        "halcon_task_execution_duration_seconds",
        "Task execution duration by tool",
        &["tool"],
        vec![0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0]
    )
    .unwrap();

    // Counter: WebSocket reconnect attempts
    pub static ref WEBSOCKET_RECONNECT_TOTAL: CounterVec = register_counter_vec!(
        "halcon_websocket_reconnect_total",
        "Total WebSocket reconnect attempts by outcome",
        &["outcome"]  // success, failed
    )
    .unwrap();

    // Gauge: Oldest sent event age
    pub static ref OLDEST_SENT_EVENT_AGE: IntGauge = register_int_gauge!(
        "halcon_oldest_sent_event_age_seconds",
        "Age of oldest event in 'sent' status (awaiting ACK)"
    )
    .unwrap();
}

// ───────────────────────────────────────────────────────────────────────────
// METRICS COLLECTOR
// ───────────────────────────────────────────────────────────────────────────

/// Metrics collector for periodic updates.
pub struct MetricsCollector {
    buffer_stats_fn: Arc<RwLock<Box<dyn Fn() -> BufferMetrics + Send + Sync>>>,
    dlq_stats_fn: Arc<RwLock<Box<dyn Fn() -> DlqMetrics + Send + Sync>>>,
}

#[derive(Debug, Clone)]
pub struct BufferMetrics {
    pub pending: usize,
    pub sent: usize,
    pub acked: usize,
    pub oldest_sent_age_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct DlqMetrics {
    pub pending: usize,
    pub exhausted: usize,
    pub manual: usize,
}

impl MetricsCollector {
    /// Create new metrics collector.
    pub fn new() -> Self {
        Self {
            buffer_stats_fn: Arc::new(RwLock::new(Box::new(|| BufferMetrics {
                pending: 0,
                sent: 0,
                acked: 0,
                oldest_sent_age_secs: None,
            }))),
            dlq_stats_fn: Arc::new(RwLock::new(Box::new(|| DlqMetrics {
                pending: 0,
                exhausted: 0,
                manual: 0,
            }))),
        }
    }

    /// Set buffer stats callback.
    pub async fn set_buffer_stats_fn<F>(&self, f: F)
    where
        F: Fn() -> BufferMetrics + Send + Sync + 'static,
    {
        let mut guard = self.buffer_stats_fn.write().await;
        *guard = Box::new(f);
    }

    /// Set DLQ stats callback.
    pub async fn set_dlq_stats_fn<F>(&self, f: F)
    where
        F: Fn() -> DlqMetrics + Send + Sync + 'static,
    {
        let mut guard = self.dlq_stats_fn.write().await;
        *guard = Box::new(f);
    }

    /// Update gauges (call periodically).
    pub async fn update_gauges(&self) {
        // Update buffer gauges
        let buffer_stats = {
            let guard = self.buffer_stats_fn.read().await;
            guard()
        };

        EVENT_BUFFER_SIZE
            .with_label_values(&["pending"])
            .set(buffer_stats.pending as f64);
        EVENT_BUFFER_SIZE
            .with_label_values(&["sent"])
            .set(buffer_stats.sent as f64);
        EVENT_BUFFER_SIZE
            .with_label_values(&["acked"])
            .set(buffer_stats.acked as f64);

        if let Some(age) = buffer_stats.oldest_sent_age_secs {
            OLDEST_SENT_EVENT_AGE.set(age as i64);
        }

        // Update DLQ gauges
        let dlq_stats = {
            let guard = self.dlq_stats_fn.read().await;
            guard()
        };

        DLQ_SIZE
            .with_label_values(&["pending"])
            .set(dlq_stats.pending as f64);
        DLQ_SIZE
            .with_label_values(&["exhausted"])
            .set(dlq_stats.exhausted as f64);
        DLQ_SIZE
            .with_label_values(&["manual"])
            .set(dlq_stats.manual as f64);
    }

    /// Start background update loop.
    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));

            loop {
                interval.tick().await;
                self.update_gauges().await;
            }
        })
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ───────────────────────────────────────────────────────────────────────────
// METRICS HTTP SERVER
// ───────────────────────────────────────────────────────────────────────────

/// Start Prometheus metrics HTTP server.
pub async fn start_metrics_server(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    use axum::{routing::get, Router};

    let app = Router::new().route("/metrics", get(metrics_handler));

    let addr = format!("0.0.0.0:{}", port);
    tracing::info!("Starting metrics server on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn metrics_handler() -> String {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();

    encoder.encode(&metric_families, &mut buffer).unwrap();

    String::from_utf8(buffer).unwrap()
}

// ───────────────────────────────────────────────────────────────────────────
// HELPER MACROS
// ───────────────────────────────────────────────────────────────────────────

/// Record event processing.
#[macro_export]
macro_rules! record_event {
    (success) => {
        $crate::EVENTS_PROCESSED_TOTAL
            .with_label_values(&["success"])
            .inc();
    };
    (failed, $reason:expr) => {
        $crate::EVENTS_PROCESSED_TOTAL
            .with_label_values(&["failed"])
            .inc();
        $crate::EVENTS_FAILED_TOTAL
            .with_label_values(&[$reason])
            .inc();
    };
    (timeout) => {
        $crate::EVENTS_PROCESSED_TOTAL
            .with_label_values(&["timeout"])
            .inc();
        $crate::EVENTS_FAILED_TOTAL
            .with_label_values(&["timeout"])
            .inc();
    };
}

/// Record ACK timeout.
#[macro_export]
macro_rules! record_ack_timeout {
    (warning) => {
        $crate::ACK_TIMEOUT_TOTAL
            .with_label_values(&["warning"])
            .inc();
    };
    (escalated) => {
        $crate::ACK_TIMEOUT_TOTAL
            .with_label_values(&["escalated"])
            .inc();
    };
}

/// Record retry.
#[macro_export]
macro_rules! record_retry {
    (success) => {
        $crate::RETRY_COUNT_TOTAL
            .with_label_values(&["success"])
            .inc();
    };
    (failed) => {
        $crate::RETRY_COUNT_TOTAL
            .with_label_values(&["failed"])
            .inc();
    };
    (exhausted) => {
        $crate::RETRY_COUNT_TOTAL
            .with_label_values(&["exhausted"])
            .inc();
    };
}
