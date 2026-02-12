//! Cost/latency optimizer: ranks models by historical performance.
//!
//! Uses invocation metrics from the database to recommend the best
//! model for a given strategy (balanced, fast, cheap).

use std::sync::Arc;

use cuervo_storage::{Database, ModelStats, SystemMetrics};

/// A model ranking produced by the optimizer.
#[derive(Debug, Clone)]
pub struct RankedModel {
    pub provider: String,
    pub model: String,
    pub score: f64,
    pub avg_latency_ms: f64,
    pub avg_cost: f64,
    pub success_rate: f64,
}

/// Strategy for ranking models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizeStrategy {
    /// Balance cost and latency equally.
    Balanced,
    /// Prefer lowest latency.
    Fast,
    /// Prefer lowest cost.
    Cheap,
}

impl OptimizeStrategy {
    pub fn from_str(s: &str) -> Self {
        match s {
            "fast" => Self::Fast,
            "cheap" => Self::Cheap,
            _ => Self::Balanced,
        }
    }
}

/// Optimizer that ranks models based on historical metrics.
pub struct CostLatencyOptimizer {
    db: Arc<Database>,
}

impl CostLatencyOptimizer {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Rank all known models by the given strategy.
    ///
    /// Returns models sorted by score (highest = best).
    /// Models with < 3 invocations are excluded (insufficient data).
    pub fn rank_models(&self, strategy: OptimizeStrategy) -> Vec<RankedModel> {
        let sys = match self.db.system_metrics() {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        Self::rank_from_metrics(&sys, strategy)
    }

    /// Rank models from pre-fetched system metrics (no DB access needed).
    ///
    /// Useful when you already have a `&Database` reference (not `Arc<Database>`)
    /// and can call `system_metrics()` directly.
    pub fn rank_from_metrics(sys: &SystemMetrics, strategy: OptimizeStrategy) -> Vec<RankedModel> {
        let mut ranked: Vec<RankedModel> = sys
            .models
            .iter()
            .filter(|m| m.total_invocations >= 3)
            .map(|m| {
                let score = compute_score(m, strategy);
                RankedModel {
                    provider: m.provider.clone(),
                    model: m.model.clone(),
                    score,
                    avg_latency_ms: m.avg_latency_ms,
                    avg_cost: m.avg_cost_per_invocation,
                    success_rate: m.success_rate,
                }
            })
            .collect();

        // Sort by score descending (higher = better).
        ranked.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        ranked
    }
}

/// Compute a score for a model based on the strategy.
///
/// Score components (all normalized to [0, 1]):
/// - success_rate: directly used (higher = better)
/// - latency_score: 1.0 / (1.0 + avg_latency_ms / 1000.0) — lower latency = higher score
/// - cost_score: 1.0 / (1.0 + avg_cost * 1000.0) — lower cost = higher score
fn compute_score(stats: &ModelStats, strategy: OptimizeStrategy) -> f64 {
    let success = stats.success_rate;
    let latency_score = 1.0 / (1.0 + stats.avg_latency_ms / 1000.0);
    let cost_score = 1.0 / (1.0 + stats.avg_cost_per_invocation * 1000.0);

    let (w_success, w_latency, w_cost) = match strategy {
        OptimizeStrategy::Balanced => (0.4, 0.3, 0.3),
        OptimizeStrategy::Fast => (0.3, 0.6, 0.1),
        OptimizeStrategy::Cheap => (0.3, 0.1, 0.6),
    };

    w_success * success + w_latency * latency_score + w_cost * cost_score
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cuervo_storage::InvocationMetric;

    fn insert_metrics(db: &Database, provider: &str, model: &str, count: usize, latency: u64, cost: f64) {
        for _ in 0..count {
            db.insert_metric(&InvocationMetric {
                provider: provider.to_string(),
                model: model.to_string(),
                latency_ms: latency,
                input_tokens: 100,
                output_tokens: 50,
                estimated_cost_usd: cost,
                success: true,
                stop_reason: "end_turn".to_string(),
                session_id: None,
                created_at: Utc::now(),
            })
            .unwrap();
        }
    }

    #[test]
    fn rank_models_empty_db() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let optimizer = CostLatencyOptimizer::new(db);
        let ranked = optimizer.rank_models(OptimizeStrategy::Balanced);
        assert!(ranked.is_empty());
    }

    #[test]
    fn rank_models_filters_low_count() {
        let db = Arc::new(Database::open_in_memory().unwrap());

        // Only 2 invocations — below threshold of 3.
        insert_metrics(&db, "a", "model_few", 2, 100, 0.001);
        // 5 invocations — above threshold.
        insert_metrics(&db, "a", "model_enough", 5, 200, 0.002);

        let optimizer = CostLatencyOptimizer::new(db);
        let ranked = optimizer.rank_models(OptimizeStrategy::Balanced);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].model, "model_enough");
    }

    #[test]
    fn rank_models_fast_prefers_low_latency() {
        let db = Arc::new(Database::open_in_memory().unwrap());

        insert_metrics(&db, "a", "fast_model", 5, 100, 0.01);
        insert_metrics(&db, "a", "slow_model", 5, 5000, 0.001);

        let optimizer = CostLatencyOptimizer::new(db);
        let ranked = optimizer.rank_models(OptimizeStrategy::Fast);
        assert_eq!(ranked[0].model, "fast_model");
    }

    #[test]
    fn rank_models_cheap_prefers_low_cost() {
        let db = Arc::new(Database::open_in_memory().unwrap());

        insert_metrics(&db, "a", "cheap_model", 5, 2000, 0.0001);
        insert_metrics(&db, "a", "expensive_model", 5, 500, 0.01);

        let optimizer = CostLatencyOptimizer::new(db);
        let ranked = optimizer.rank_models(OptimizeStrategy::Cheap);
        assert_eq!(ranked[0].model, "cheap_model");
    }

    #[test]
    fn rank_models_balanced() {
        let db = Arc::new(Database::open_in_memory().unwrap());

        insert_metrics(&db, "a", "balanced", 5, 500, 0.002);
        insert_metrics(&db, "a", "fast_expensive", 5, 100, 0.05);
        insert_metrics(&db, "a", "slow_cheap", 5, 5000, 0.0001);

        let optimizer = CostLatencyOptimizer::new(db);
        let ranked = optimizer.rank_models(OptimizeStrategy::Balanced);
        // The balanced model should rank well (good in both dimensions).
        assert!(!ranked.is_empty());
        // All models should appear.
        assert_eq!(ranked.len(), 3);
    }

    #[test]
    fn optimize_strategy_from_str() {
        assert_eq!(OptimizeStrategy::from_str("fast"), OptimizeStrategy::Fast);
        assert_eq!(OptimizeStrategy::from_str("cheap"), OptimizeStrategy::Cheap);
        assert_eq!(OptimizeStrategy::from_str("balanced"), OptimizeStrategy::Balanced);
        assert_eq!(OptimizeStrategy::from_str("unknown"), OptimizeStrategy::Balanced);
    }

    #[test]
    fn score_components_are_bounded() {
        let stats = ModelStats {
            provider: "test".into(),
            model: "test".into(),
            total_invocations: 10,
            successful_invocations: 10,
            avg_latency_ms: 500.0,
            p95_latency_ms: 1000,
            total_tokens: 1000,
            total_cost_usd: 0.01,
            avg_cost_per_invocation: 0.001,
            success_rate: 1.0,
        };

        for strategy in [OptimizeStrategy::Balanced, OptimizeStrategy::Fast, OptimizeStrategy::Cheap] {
            let score = compute_score(&stats, strategy);
            assert!(score >= 0.0, "score should be non-negative");
            assert!(score <= 1.0, "score should be at most 1.0");
        }
    }
}
