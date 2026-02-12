use chrono::Utc;

use cuervo_core::error::{CuervoError, Result};

use super::Database;

impl Database {
    /// Record a model invocation metric.
    pub fn insert_metric(&self, metric: &crate::metrics::InvocationMetric) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "INSERT INTO invocation_metrics (provider, model, latency_ms, input_tokens, output_tokens,
             estimated_cost_usd, success, stop_reason, session_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                metric.provider,
                metric.model,
                metric.latency_ms as i64,
                metric.input_tokens,
                metric.output_tokens,
                metric.estimated_cost_usd,
                metric.success as i32,
                metric.stop_reason,
                metric.session_id,
                metric.created_at.to_rfc3339(),
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("insert metric: {e}")))?;

        Ok(())
    }

    /// Get aggregated stats for a specific model (2 queries: 1 compound + 1 P95).
    pub fn model_stats(&self, provider: &str, model: &str) -> Result<crate::metrics::ModelStats> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        // Single compound query replaces 5 individual queries.
        let (total_invocations, successful_invocations, avg_latency_ms, total_tokens, total_cost_usd): (u64, u64, f64, u64, f64) = conn
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END), 0),
                        COALESCE(AVG(latency_ms), 0.0),
                        COALESCE(SUM(input_tokens + output_tokens), 0),
                        COALESCE(SUM(estimated_cost_usd), 0.0)
                 FROM invocation_metrics WHERE provider = ?1 AND model = ?2",
                rusqlite::params![provider, model],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        // P95 latency (2nd query — SQLite lacks PERCENTILE_CONT).
        let p95_latency_ms: u64 = if total_invocations > 0 {
            let offset = (total_invocations as f64 * 0.95).ceil() as u64 - 1;
            conn.query_row(
                "SELECT latency_ms FROM invocation_metrics WHERE provider = ?1 AND model = ?2
                 ORDER BY latency_ms ASC LIMIT 1 OFFSET ?3",
                rusqlite::params![provider, model, offset],
                |row| row.get(0),
            )
            .unwrap_or(0)
        } else {
            0
        };

        let avg_cost = if total_invocations > 0 {
            total_cost_usd / total_invocations as f64
        } else {
            0.0
        };

        let success_rate = if total_invocations > 0 {
            successful_invocations as f64 / total_invocations as f64
        } else {
            0.0
        };

        Ok(crate::metrics::ModelStats {
            provider: provider.to_string(),
            model: model.to_string(),
            total_invocations,
            successful_invocations,
            avg_latency_ms,
            p95_latency_ms,
            total_tokens,
            total_cost_usd,
            avg_cost_per_invocation: avg_cost,
            success_rate,
        })
    }

    /// Get system-wide metrics summary (2 queries: 1 GROUP BY + 1 batched P95).
    pub fn system_metrics(&self) -> Result<crate::metrics::SystemMetrics> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        // Query 1: Single GROUP BY for all aggregate stats.
        let mut stmt = conn
            .prepare(
                "SELECT provider, model,
                        COUNT(*) AS total_invocations,
                        SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END) AS successful_invocations,
                        COALESCE(AVG(latency_ms), 0.0) AS avg_latency_ms,
                        COALESCE(SUM(input_tokens + output_tokens), 0) AS total_tokens,
                        COALESCE(SUM(estimated_cost_usd), 0.0) AS total_cost_usd
                 FROM invocation_metrics GROUP BY provider, model ORDER BY provider, model",
            )
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        struct PerModelRow {
            provider: String,
            model: String,
            total_invocations: u64,
            successful_invocations: u64,
            avg_latency_ms: f64,
            total_tokens: u64,
            total_cost_usd: f64,
        }

        let rows: Vec<PerModelRow> = stmt
            .query_map([], |row| {
                Ok(PerModelRow {
                    provider: row.get(0)?,
                    model: row.get(1)?,
                    total_invocations: row.get(2)?,
                    successful_invocations: row.get(3)?,
                    avg_latency_ms: row.get(4)?,
                    total_tokens: row.get(5)?,
                    total_cost_usd: row.get(6)?,
                })
            })
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        drop(stmt);

        // Query 2: Batched P95 for ALL models in one query using window functions.
        // This replaces N individual P95 queries with a single pass.
        let mut p95_map: std::collections::HashMap<(String, String), u64> =
            std::collections::HashMap::new();
        {
            let mut p95_stmt = conn
                .prepare(
                    "SELECT provider, model, latency_ms FROM (
                        SELECT provider, model, latency_ms,
                               ROW_NUMBER() OVER (PARTITION BY provider, model ORDER BY latency_ms ASC) AS rn,
                               COUNT(*) OVER (PARTITION BY provider, model) AS total
                        FROM invocation_metrics
                    ) sub
                    WHERE rn = CAST(total * 95 / 100 + (CASE WHEN total * 95 % 100 > 0 THEN 1 ELSE 0 END) AS INTEGER)",
                )
                .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

            let p95_rows = p95_stmt
                .query_map([], |row| {
                    let provider: String = row.get(0)?;
                    let model: String = row.get(1)?;
                    let latency: u64 = row.get(2)?;
                    Ok((provider, model, latency))
                })
                .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

            for (p, m, lat) in p95_rows.flatten() {
                p95_map.insert((p, m), lat);
            }
        }

        // Compute global totals from per-model results (no extra queries).
        let mut global_invocations: u64 = 0;
        let mut global_cost: f64 = 0.0;
        let mut global_tokens: u64 = 0;

        let mut models = Vec::with_capacity(rows.len());
        for r in &rows {
            global_invocations += r.total_invocations;
            global_cost += r.total_cost_usd;
            global_tokens += r.total_tokens;

            let p95_latency_ms = p95_map
                .get(&(r.provider.clone(), r.model.clone()))
                .copied()
                .unwrap_or(0);

            let avg_cost = if r.total_invocations > 0 {
                r.total_cost_usd / r.total_invocations as f64
            } else {
                0.0
            };
            let success_rate = if r.total_invocations > 0 {
                r.successful_invocations as f64 / r.total_invocations as f64
            } else {
                0.0
            };

            models.push(crate::metrics::ModelStats {
                provider: r.provider.clone(),
                model: r.model.clone(),
                total_invocations: r.total_invocations,
                successful_invocations: r.successful_invocations,
                avg_latency_ms: r.avg_latency_ms,
                p95_latency_ms,
                total_tokens: r.total_tokens,
                total_cost_usd: r.total_cost_usd,
                avg_cost_per_invocation: avg_cost,
                success_rate,
            });
        }

        Ok(crate::metrics::SystemMetrics {
            total_invocations: global_invocations,
            total_cost_usd: global_cost,
            total_tokens: global_tokens,
            models,
        })
    }

    /// Get provider-level metrics within a time window (for health scoring).
    ///
    /// Aggregates all models for a given provider within the last `window_minutes` minutes.
    /// Uses 2 queries: 1 compound aggregate + 1 P95 (down from 5 queries).
    pub fn provider_metrics_windowed(
        &self,
        provider: &str,
        window_minutes: u64,
    ) -> Result<crate::metrics::ProviderWindowedMetrics> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let cutoff = Utc::now() - chrono::Duration::minutes(window_minutes as i64);
        let cutoff_str = cutoff.to_rfc3339();

        // Single compound query replaces 4 individual queries.
        let (total_invocations, successful_invocations, timeout_count, avg_latency_ms): (u64, u64, u64, f64) = conn
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN stop_reason = 'timeout' THEN 1 ELSE 0 END), 0),
                        COALESCE(AVG(latency_ms), 0.0)
                 FROM invocation_metrics WHERE provider = ?1 AND created_at >= ?2",
                rusqlite::params![provider, cutoff_str],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        if total_invocations == 0 {
            return Ok(crate::metrics::ProviderWindowedMetrics {
                provider: provider.to_string(),
                ..Default::default()
            });
        }

        // P95 latency (2nd query).
        let p95_latency_ms: u64 = {
            let offset = (total_invocations as f64 * 0.95).ceil() as u64 - 1;
            conn.query_row(
                "SELECT latency_ms FROM invocation_metrics WHERE provider = ?1 AND created_at >= ?2
                 ORDER BY latency_ms ASC LIMIT 1 OFFSET ?3",
                rusqlite::params![provider, cutoff_str, offset],
                |row| row.get(0),
            )
            .unwrap_or(0)
        };

        let failed = total_invocations - successful_invocations;
        let error_rate = failed as f64 / total_invocations as f64;
        let timeout_rate = timeout_count as f64 / total_invocations as f64;

        Ok(crate::metrics::ProviderWindowedMetrics {
            provider: provider.to_string(),
            total_invocations,
            successful_invocations,
            failed_invocations: failed,
            timeout_count,
            avg_latency_ms,
            p95_latency_ms,
            error_rate,
            timeout_rate,
        })
    }

    /// Record a tool execution metric.
    pub fn insert_tool_metric(&self, metric: &crate::metrics::ToolExecutionMetric) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        conn.execute(
            "INSERT INTO tool_execution_metrics (tool_name, session_id, duration_ms, success, is_parallel, input_summary, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                metric.tool_name,
                metric.session_id,
                metric.duration_ms as i64,
                metric.success as i32,
                metric.is_parallel as i32,
                metric.input_summary,
                metric.created_at.to_rfc3339(),
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("insert tool metric: {e}")))?;

        Ok(())
    }

    /// Get aggregated stats for a specific tool.
    pub fn tool_stats(&self, tool_name: &str) -> Result<crate::metrics::ToolStats> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let (total, avg_dur, successes): (u64, f64, u64) = conn
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(AVG(duration_ms), 0.0),
                        COALESCE(SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END), 0)
                 FROM tool_execution_metrics WHERE tool_name = ?1",
                rusqlite::params![tool_name],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let success_rate = if total > 0 {
            successes as f64 / total as f64
        } else {
            0.0
        };

        Ok(crate::metrics::ToolStats {
            tool_name: tool_name.to_string(),
            total_executions: total,
            avg_duration_ms: avg_dur,
            success_rate,
        })
    }

    /// Get top tools by execution count (for doctor command).
    pub fn top_tool_stats(&self, limit: usize) -> Result<Vec<crate::metrics::ToolStats>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT tool_name,
                        COUNT(*) AS total,
                        COALESCE(AVG(duration_ms), 0.0) AS avg_dur,
                        COALESCE(SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END), 0) AS successes
                 FROM tool_execution_metrics
                 GROUP BY tool_name
                 ORDER BY total DESC
                 LIMIT ?1",
            )
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let results = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                let total: u64 = row.get(1)?;
                let successes: u64 = row.get(3)?;
                let success_rate = if total > 0 {
                    successes as f64 / total as f64
                } else {
                    0.0
                };
                Ok(crate::metrics::ToolStats {
                    tool_name: row.get(0)?,
                    total_executions: total,
                    avg_duration_ms: row.get(2)?,
                    success_rate,
                })
            })
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    /// Batch-insert multiple invocation metrics in a single transaction.
    ///
    /// Significantly faster than calling `insert_metric()` N times because
    /// only one lock acquisition and one WAL sync are needed.
    pub fn batch_insert_metrics(&self, metrics: &[crate::metrics::InvocationMetric]) -> Result<()> {
        if metrics.is_empty() {
            return Ok(());
        }
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let tx = conn.unchecked_transaction()
            .map_err(|e| CuervoError::DatabaseError(format!("begin batch metrics tx: {e}")))?;

        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO invocation_metrics (provider, model, latency_ms, input_tokens, output_tokens,
                 estimated_cost_usd, success, stop_reason, session_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            ).map_err(|e| CuervoError::DatabaseError(format!("prepare batch metric: {e}")))?;

            for metric in metrics {
                stmt.execute(rusqlite::params![
                    metric.provider,
                    metric.model,
                    metric.latency_ms as i64,
                    metric.input_tokens,
                    metric.output_tokens,
                    metric.estimated_cost_usd,
                    metric.success as i32,
                    metric.stop_reason,
                    metric.session_id,
                    metric.created_at.to_rfc3339(),
                ]).map_err(|e| CuervoError::DatabaseError(format!("batch metric insert: {e}")))?;
            }
        }

        tx.commit()
            .map_err(|e| CuervoError::DatabaseError(format!("commit batch metrics: {e}")))?;
        Ok(())
    }

    /// Batch-insert multiple tool execution metrics in a single transaction.
    pub fn batch_insert_tool_metrics(&self, metrics: &[crate::metrics::ToolExecutionMetric]) -> Result<()> {
        if metrics.is_empty() {
            return Ok(());
        }
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let tx = conn.unchecked_transaction()
            .map_err(|e| CuervoError::DatabaseError(format!("begin batch tool tx: {e}")))?;

        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO tool_execution_metrics (tool_name, session_id, duration_ms, success, is_parallel, input_summary, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            ).map_err(|e| CuervoError::DatabaseError(format!("prepare batch tool metric: {e}")))?;

            for metric in metrics {
                stmt.execute(rusqlite::params![
                    metric.tool_name,
                    metric.session_id,
                    metric.duration_ms as i64,
                    metric.success as i32,
                    metric.is_parallel as i32,
                    metric.input_summary,
                    metric.created_at.to_rfc3339(),
                ]).map_err(|e| CuervoError::DatabaseError(format!("batch tool metric insert: {e}")))?;
            }
        }

        tx.commit()
            .map_err(|e| CuervoError::DatabaseError(format!("commit batch tool metrics: {e}")))?;
        Ok(())
    }

    /// Delete metrics older than the given number of days.
    pub fn prune_metrics(&self, max_age_days: u32) -> Result<u64> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let cutoff = Utc::now() - chrono::Duration::days(max_age_days as i64);
        let deleted = conn
            .execute(
                "DELETE FROM invocation_metrics WHERE created_at < ?1",
                rusqlite::params![cutoff.to_rfc3339()],
            )
            .map_err(|e| CuervoError::DatabaseError(format!("prune metrics: {e}")))?;

        Ok(deleted as u64)
    }
}
