//! Metrics for planning effectiveness.
//!
//! Tracks:
//! - Plan generation success/failure rate
//! - Plan quality indicators (step count, confidence, coherence)
//! - Replanning frequency
//! - Plan execution outcomes

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct PlanningMetrics {
    inner: Arc<PlanningMetricsInner>,
}

#[derive(Debug)]
struct PlanningMetricsInner {
    // Generation
    plans_generated: AtomicU64,
    plan_generation_failures: AtomicU64,

    // Quality
    avg_steps_per_plan: AtomicU64, // Fixed-point * 100
    avg_confidence_per_plan: AtomicU64, // Fixed-point * 100

    // Replanning
    replans_triggered: AtomicU64,
    replan_reasons: [AtomicU64; 4], // [tool_failure, max_rounds, synthesis_forced, manual]

    // Execution outcomes
    plans_completed_successfully: AtomicU64,
    plans_failed: AtomicU64,
    plans_abandoned: AtomicU64,

    // Timing
    total_planning_time_ms: AtomicU64,
}

impl PlanningMetrics {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(PlanningMetricsInner {
                plans_generated: AtomicU64::new(0),
                plan_generation_failures: AtomicU64::new(0),
                avg_steps_per_plan: AtomicU64::new(0),
                avg_confidence_per_plan: AtomicU64::new(0),
                replans_triggered: AtomicU64::new(0),
                replan_reasons: [
                    AtomicU64::new(0), // tool_failure
                    AtomicU64::new(0), // max_rounds
                    AtomicU64::new(0), // synthesis_forced
                    AtomicU64::new(0), // manual
                ],
                plans_completed_successfully: AtomicU64::new(0),
                plans_failed: AtomicU64::new(0),
                plans_abandoned: AtomicU64::new(0),
                total_planning_time_ms: AtomicU64::new(0),
            }),
        }
    }

    pub fn record_plan_generated(&self, step_count: usize, avg_confidence: f64, duration_ms: u64) {
        self.inner.plans_generated.fetch_add(1, Ordering::Relaxed);
        self.inner.avg_steps_per_plan.fetch_add((step_count as u64) * 100, Ordering::Relaxed);
        self.inner.avg_confidence_per_plan.fetch_add((avg_confidence * 100.0) as u64, Ordering::Relaxed);
        self.inner.total_planning_time_ms.fetch_add(duration_ms, Ordering::Relaxed);
    }

    pub fn record_plan_generation_failure(&self) {
        self.inner.plan_generation_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_replan(&self, reason: ReplanReason) {
        self.inner.replans_triggered.fetch_add(1, Ordering::Relaxed);
        self.inner.replan_reasons[reason as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_plan_outcome(&self, outcome: PlanOutcome) {
        match outcome {
            PlanOutcome::CompletedSuccessfully => {
                self.inner.plans_completed_successfully.fetch_add(1, Ordering::Relaxed);
            }
            PlanOutcome::Failed => {
                self.inner.plans_failed.fetch_add(1, Ordering::Relaxed);
            }
            PlanOutcome::Abandoned => {
                self.inner.plans_abandoned.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn snapshot(&self) -> PlanningMetricsSnapshot {
        let total_plans = self.inner.plans_generated.load(Ordering::Relaxed);

        PlanningMetricsSnapshot {
            plans_generated: total_plans,
            plan_generation_failures: self.inner.plan_generation_failures.load(Ordering::Relaxed),

            avg_steps_per_plan: if total_plans > 0 {
                self.inner.avg_steps_per_plan.load(Ordering::Relaxed) as f64 / total_plans as f64 / 100.0
            } else {
                0.0
            },

            avg_confidence_per_plan: if total_plans > 0 {
                self.inner.avg_confidence_per_plan.load(Ordering::Relaxed) as f64 / total_plans as f64 / 100.0
            } else {
                0.0
            },

            replans_triggered: self.inner.replans_triggered.load(Ordering::Relaxed),
            replan_reasons_breakdown: [
                self.inner.replan_reasons[0].load(Ordering::Relaxed),
                self.inner.replan_reasons[1].load(Ordering::Relaxed),
                self.inner.replan_reasons[2].load(Ordering::Relaxed),
                self.inner.replan_reasons[3].load(Ordering::Relaxed),
            ],

            plans_completed_successfully: self.inner.plans_completed_successfully.load(Ordering::Relaxed),
            plans_failed: self.inner.plans_failed.load(Ordering::Relaxed),
            plans_abandoned: self.inner.plans_abandoned.load(Ordering::Relaxed),

            avg_planning_time_ms: if total_plans > 0 {
                (self.inner.total_planning_time_ms.load(Ordering::Relaxed) / total_plans) as f64
            } else {
                0.0
            },
        }
    }
}

impl Default for PlanningMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ReplanReason {
    ToolFailure = 0,
    MaxRounds = 1,
    SynthesisForced = 2,
    Manual = 3,
}

#[derive(Debug, Clone, Copy)]
pub enum PlanOutcome {
    CompletedSuccessfully,
    Failed,
    Abandoned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningMetricsSnapshot {
    pub plans_generated: u64,
    pub plan_generation_failures: u64,
    pub avg_steps_per_plan: f64,
    pub avg_confidence_per_plan: f64,
    pub replans_triggered: u64,
    pub replan_reasons_breakdown: [u64; 4],
    pub plans_completed_successfully: u64,
    pub plans_failed: u64,
    pub plans_abandoned: u64,
    pub avg_planning_time_ms: f64,
}

impl PlanningMetricsSnapshot {
    pub fn plan_success_rate(&self) -> f64 {
        let total = self.plans_completed_successfully + self.plans_failed + self.plans_abandoned;
        if total == 0 {
            return 0.0;
        }
        self.plans_completed_successfully as f64 / total as f64
    }

    pub fn replan_frequency(&self) -> f64 {
        if self.plans_generated == 0 {
            return 0.0;
        }
        self.replans_triggered as f64 / self.plans_generated as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_metrics_initialization() {
        let metrics = PlanningMetrics::new();
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.plans_generated, 0);
    }

    #[test]
    fn record_plan_generated() {
        let metrics = PlanningMetrics::new();
        metrics.record_plan_generated(4, 0.8, 1500);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.plans_generated, 1);
        assert_eq!(snapshot.avg_steps_per_plan, 4.0);
        assert_eq!(snapshot.avg_confidence_per_plan, 0.8);
        assert_eq!(snapshot.avg_planning_time_ms, 1500.0);
    }

    #[test]
    fn replan_tracking() {
        let metrics = PlanningMetrics::new();
        metrics.record_replan(ReplanReason::ToolFailure);
        metrics.record_replan(ReplanReason::ToolFailure);
        metrics.record_replan(ReplanReason::MaxRounds);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.replans_triggered, 3);
        assert_eq!(snapshot.replan_reasons_breakdown[0], 2); // tool_failure
        assert_eq!(snapshot.replan_reasons_breakdown[1], 1); // max_rounds
    }
}
