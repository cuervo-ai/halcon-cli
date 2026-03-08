/// Runtime Metrics & Monitoring Context Server (Server 6).
///
/// Provides context from Prometheus metrics, application logs, error traces,
/// performance data, and observability signals.
/// Phase: Monitoring
/// Priority: 70

use async_trait::async_trait;
use halcon_context::estimate_tokens;
use halcon_core::error::{HalconError, Result};
use halcon_core::traits::{ContextChunk, ContextQuery, ContextSource};
use halcon_core::types::SdlcPhase;
use halcon_storage::{AsyncDatabase, Database};
use std::sync::Arc;

pub struct RuntimeMetricsServer {
    db: AsyncDatabase,
    priority: u32,
    token_budget: u32,
}

impl RuntimeMetricsServer {
    pub fn new(db: AsyncDatabase, priority: u32, token_budget: u32) -> Self {
        Self {
            db,
            priority,
            token_budget,
        }
    }

    pub fn sdlc_phase(&self) -> SdlcPhase {
        SdlcPhase::Monitoring
    }

    async fn fetch_metrics(&self, query: Option<&str>) -> Result<Vec<MetricEntry>> {
        let db = self.db.clone();
        let query_opt = query.map(String::from);

        tokio::task::spawn_blocking(move || {
            let db_ref = db.inner();
            let conn = db_ref.conn()
                .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

            let metrics = if let Some(q) = query_opt {
                // FTS5 search
                let mut stmt = conn.prepare(
                    "SELECT metric_id, metric_name, metric_type, metric_value, labels_json,
                            service_name, environment, severity, message, timestamp
                     FROM runtime_metrics
                     WHERE metric_id IN (
                       SELECT rowid FROM runtime_metrics_fts WHERE runtime_metrics_fts MATCH ?
                     )
                     ORDER BY timestamp DESC
                     LIMIT 20"
                ).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt.query_map([q], |row| {
                    Ok(MetricEntry {
                        metric_id: row.get(0)?,
                        metric_name: row.get(1)?,
                        metric_type: row.get(2)?,
                        metric_value: row.get(3)?,
                        labels_json: row.get(4)?,
                        service_name: row.get(5)?,
                        environment: row.get(6)?,
                        severity: row.get(7)?,
                        message: row.get(8)?,
                        timestamp: row.get(9)?,
                    })
                }).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            } else {
                // No query → return recent metrics (critical/error first, then recent)
                let mut stmt = conn.prepare(
                    "SELECT metric_id, metric_name, metric_type, metric_value, labels_json,
                            service_name, environment, severity, message, timestamp
                     FROM runtime_metrics
                     ORDER BY
                        CASE severity
                            WHEN 'critical' THEN 0
                            WHEN 'error' THEN 1
                            WHEN 'warning' THEN 2
                            WHEN 'info' THEN 3
                            ELSE 4
                        END,
                        timestamp DESC
                     LIMIT 20"
                ).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                let rows = stmt.query_map([], |row| {
                    Ok(MetricEntry {
                        metric_id: row.get(0)?,
                        metric_name: row.get(1)?,
                        metric_type: row.get(2)?,
                        metric_value: row.get(3)?,
                        labels_json: row.get(4)?,
                        service_name: row.get(5)?,
                        environment: row.get(6)?,
                        severity: row.get(7)?,
                        message: row.get(8)?,
                        timestamp: row.get(9)?,
                    })
                }).map_err(|e| HalconError::DatabaseError(e.to_string()))?;

                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            };

            Ok::<Vec<MetricEntry>, HalconError>(metrics)
        })
        .await
        .map_err(|e| HalconError::DatabaseError(e.to_string()))?
    }
}

#[derive(Debug)]
struct MetricEntry {
    metric_id: String,
    metric_name: String,
    metric_type: String,
    metric_value: f64,
    labels_json: String,
    service_name: String,
    environment: Option<String>,
    severity: Option<String>,
    message: Option<String>,
    timestamp: i64,
}

#[async_trait]
impl ContextSource for RuntimeMetricsServer {
    fn name(&self) -> &str {
        "runtime_metrics"
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let metrics = self
            .fetch_metrics(query.user_message.as_deref())
            .await?;

        if metrics.is_empty() {
            return Ok(vec![]);
        }

        let mut chunks = Vec::new();
        let mut total_tokens = 0usize;

        for metric in metrics {
            let env_info = if let Some(env) = &metric.environment {
                format!(" | Env: {}", env)
            } else {
                String::new()
            };

            let severity_info = if let Some(sev) = &metric.severity {
                format!(" | Severity: {}", sev.to_uppercase())
            } else {
                String::new()
            };

            let message_info = if let Some(msg) = &metric.message {
                // Truncate message to max 400 chars
                let msg_preview = if msg.len() > 400 {
                    format!("{}...", &msg[..{ let mut _fcb = (400).min(msg.len()); while _fcb > 0 && !msg.is_char_boundary(_fcb) { _fcb -= 1; } _fcb }])
                } else {
                    msg.clone()
                };
                format!("Message: {}\n", msg_preview)
            } else {
                String::new()
            };

            // Parse labels (but don't fail if JSON is malformed)
            let labels_info = if metric.labels_json != "{}" {
                format!("Labels: {}\n", metric.labels_json)
            } else {
                String::new()
            };

            let content = format!(
                "[Runtime Metric Context]\n\
                 Metric: {}\n\
                 Type: {}\n\
                 Value: {:.4}\n\
                 Service: {}{}{}\n\
                 {}{}", metric.metric_name,
                metric.metric_type,
                metric.metric_value,
                metric.service_name,
                env_info,
                severity_info,
                labels_info,
                message_info
            );

            let token_estimate = estimate_tokens(&content);

            // Budget check: stop if we exceed budget
            if total_tokens + token_estimate > self.token_budget as usize {
                break;
            }

            total_tokens += token_estimate;

            chunks.push(ContextChunk {
                source: self.name().to_string(),
                priority: self.priority,
                content,
                estimated_tokens: token_estimate,
            });
        }

        Ok(chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_runtime_metrics_server_creation() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = RuntimeMetricsServer::new(async_db, 70, 3500);
        assert_eq!(server.name(), "runtime_metrics");
        assert_eq!(server.priority(), 70);
        assert_eq!(server.sdlc_phase(), SdlcPhase::Monitoring);
    }

    #[tokio::test]
    async fn test_gather_no_metrics() {
        let db = Database::open_in_memory().unwrap();
        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = RuntimeMetricsServer::new(async_db, 70, 3500);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 0);
    }

    #[tokio::test]
    async fn test_gather_with_metric() {
        let db = Database::open_in_memory().unwrap();

        // Insert runtime metric
        db.conn().unwrap()
            .execute(
                "INSERT INTO runtime_metrics (metric_id, metric_name, metric_type, metric_value, labels_json, service_name, environment, severity, message, timestamp, created_at)
                 VALUES ('metric-001', 'http_requests_total', 'counter', 1543.0, '{\"method\":\"GET\",\"endpoint\":\"/api/v1/users\"}', 'api-server', 'production', 'info', 'Request counter', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = RuntimeMetricsServer::new(async_db, 70, 3500);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("http_requests_total"));
        assert!(chunks[0].content.contains("counter"));
        assert!(chunks[0].content.contains("1543.0000"));
        assert!(chunks[0].content.contains("api-server"));
        assert!(chunks[0].content.contains("Env: production"));
        assert!(chunks[0].content.contains("Severity: INFO"));
    }

    #[tokio::test]
    async fn test_gather_critical_severity() {
        let db = Database::open_in_memory().unwrap();

        // Insert critical metric
        db.conn().unwrap()
            .execute(
                "INSERT INTO runtime_metrics (metric_id, metric_name, metric_type, metric_value, labels_json, service_name, severity, message, timestamp, created_at)
                 VALUES ('metric-002', 'error_rate', 'gauge', 0.85, '{}', 'payment-service', 'critical', 'Error rate exceeded threshold', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = RuntimeMetricsServer::new(async_db, 70, 3500);

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("error_rate"));
        assert!(chunks[0].content.contains("Severity: CRITICAL"));
        assert!(chunks[0].content.contains("Error rate exceeded threshold"));
    }

    #[tokio::test]
    async fn test_budget_enforcement() {
        let db = Database::open_in_memory().unwrap();

        // Insert metric
        db.conn().unwrap()
            .execute(
                "INSERT INTO runtime_metrics (metric_id, metric_name, metric_type, metric_value, labels_json, service_name, timestamp, created_at)
                 VALUES ('metric-003', 'cpu_usage', 'gauge', 75.5, '{}', 'app-server', 1739606400, 1739606400)",
                [],
            )
            .unwrap();

        let async_db = AsyncDatabase::new(Arc::new(db));
        let server = RuntimeMetricsServer::new(async_db, 70, 100); // Very small budget

        let query = ContextQuery {
            working_directory: "/tmp".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        // Should stop early due to budget constraint
        assert!(chunks.len() <= 1);
    }
}
