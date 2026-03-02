//! Session Retrospective Analyzer — Post-session diagnostic analysis (P5.1).
//!
//! Stateless analyzer that consumes all Phase 4 collectors to produce a
//! `SessionProfile` summarizing session quality, failure modes, and
//! adaptation utilization. Designed for logging and future warm-start.
//!
//! Pure business logic — no I/O.

use super::adaptation_bounds::{AdaptationBoundsChecker, AdaptationChannel};
use super::decision_trace::{DecisionPoint, DecisionTraceCollector};
use super::problem_classifier::{self, ProblemClass};
use super::system_invariants::SystemInvariantChecker;
use super::system_metrics::MetricsCollector;

use halcon_core::types::PolicyConfig;

// ── FailureMode ────────────────────────────────────────────────────────────

/// Dominant failure pattern detected in the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureMode {
    /// Dominant tool failure pattern.
    ToolErrors,
    /// Couldn't gather sufficient evidence.
    EvidenceStarvation,
    /// Stuck in semantic cycles.
    CycleTrapping,
    /// Progressive plan drift.
    DriftAccumulation,
    /// Ran out of time/token budget.
    SLAExhaustion,
    /// Correctness property breaches.
    InvariantViolation,
}

impl FailureMode {
    /// Short label for logging.
    pub fn label(self) -> &'static str {
        match self {
            Self::ToolErrors => "tool-errors",
            Self::EvidenceStarvation => "evidence-starvation",
            Self::CycleTrapping => "cycle-trapping",
            Self::DriftAccumulation => "drift-accumulation",
            Self::SLAExhaustion => "sla-exhaustion",
            Self::InvariantViolation => "invariant-violation",
        }
    }
}

// ── EvidenceTrajectory ─────────────────────────────────────────────────────

/// Characterization of evidence gathering pattern over the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceTrajectory {
    /// Steady increase across rounds.
    Monotonic,
    /// Stopped gaining after initial growth.
    Plateaued,
    /// Losing coverage over time.
    Declining,
    /// Unstable pattern.
    Erratic,
}

impl EvidenceTrajectory {
    /// Short label for logging.
    pub fn label(self) -> &'static str {
        match self {
            Self::Monotonic => "monotonic",
            Self::Plateaued => "plateaued",
            Self::Declining => "declining",
            Self::Erratic => "erratic",
        }
    }
}

// ── SessionProfile ─────────────────────────────────────────────────────────

/// Comprehensive post-session diagnostic profile.
#[derive(Debug, Clone)]
pub struct SessionProfile {
    /// 1.0 = optimal, < 1.0 = wasted effort.
    pub convergence_efficiency: f64,
    /// (replans + mutations) / total_rounds.
    pub structural_instability_score: f64,
    /// Dominant failure pattern, if any.
    pub dominant_failure_mode: Option<FailureMode>,
    /// Retrospective problem classification.
    pub inferred_problem_class: ProblemClass,
    /// Adaptation budget used / budget available (avg across channels).
    pub adaptation_utilization: f64,
    /// Evidence gathering trajectory pattern.
    pub evidence_trajectory: EvidenceTrajectory,
    /// Decisions per round.
    pub decision_density: f64,
    /// Rounds with combined_score below wasted threshold.
    pub wasted_rounds: usize,
    /// Highest utility score achieved.
    pub peak_utility: f64,
    /// Last utility score.
    pub final_utility: f64,
}

// ── Analysis ───────────────────────────────────────────────────────────────

/// Produce a `SessionProfile` from Phase 4 collectors.
///
/// Stateless function — all inputs are borrowed references.
pub fn analyze(
    trace: &DecisionTraceCollector,
    metrics: &MetricsCollector,
    bounds: &AdaptationBoundsChecker,
    invariants: &SystemInvariantChecker,
    policy: &PolicyConfig,
) -> SessionProfile {
    let rounds = metrics.rounds();
    let total_rounds = rounds.len();

    // Convergence efficiency
    let convergence_efficiency = compute_convergence_efficiency(rounds, policy);

    // Structural instability
    let structural_instability_score = compute_structural_instability(trace, total_rounds);

    // Dominant failure mode
    let summary = metrics.summarize(0.0, invariants.has_critical());
    let dominant_failure_mode = detect_failure_mode(&summary, invariants);

    // Inferred problem class (retrospective — uses full history)
    let sla_fraction = rounds.last().map(|r| r.sla_fraction).unwrap_or(0.0);
    let inferred_problem_class = {
        let mut classifier = problem_classifier::ProblemClassifier::new(
            std::sync::Arc::new(policy.clone()),
        );
        classifier.classify(rounds, sla_fraction).class
    };

    // Adaptation utilization
    let adaptation_utilization = compute_adaptation_utilization(bounds);

    // Evidence trajectory
    let evidence_trajectory = classify_evidence_trajectory(rounds);

    // Decision density
    let decision_density = if total_rounds > 0 {
        trace.len() as f64 / total_rounds as f64
    } else {
        0.0
    };

    // Wasted rounds
    let wasted_rounds = rounds
        .iter()
        .filter(|r| (r.combined_score as f64) < policy.wasted_round_threshold)
        .count();

    // Peak and final utility
    let peak_utility = rounds
        .iter()
        .map(|r| r.utility_score)
        .fold(0.0f64, f64::max);
    let final_utility = rounds.last().map(|r| r.utility_score).unwrap_or(0.5);

    SessionProfile {
        convergence_efficiency,
        structural_instability_score,
        dominant_failure_mode,
        inferred_problem_class,
        adaptation_utilization,
        evidence_trajectory,
        decision_density,
        wasted_rounds,
        peak_utility,
        final_utility,
    }
}

// ── Internal helpers ───────────────────────────────────────────────────────

fn compute_convergence_efficiency(
    rounds: &[super::system_metrics::RoundMetrics],
    policy: &PolicyConfig,
) -> f64 {
    if rounds.is_empty() {
        return 1.0;
    }
    // Optimal = first round where utility exceeds synthesis threshold
    let optimal = rounds
        .iter()
        .position(|r| r.utility_score > policy.utility_synthesis_threshold)
        .map(|i| i + 1)
        .unwrap_or(rounds.len());
    (optimal as f64 / rounds.len() as f64).min(1.0)
}

fn compute_structural_instability(
    trace: &DecisionTraceCollector,
    total_rounds: usize,
) -> f64 {
    if total_rounds == 0 {
        return 0.0;
    }
    let counts = trace.counts_by_point();
    let replans = counts.get(&DecisionPoint::OracleAdjudication)
        .copied()
        .unwrap_or(0);
    let mutations = counts.get(&DecisionPoint::StrategyMutation)
        .copied()
        .unwrap_or(0);
    (replans + mutations) as f64 / total_rounds as f64
}

fn detect_failure_mode(
    summary: &super::system_metrics::SessionSummary,
    invariants: &SystemInvariantChecker,
) -> Option<FailureMode> {
    // Priority: InvariantViolation > SLAExhaustion > ToolErrors > CycleTrapping >
    // DriftAccumulation > EvidenceStarvation
    if invariants.has_critical() {
        return Some(FailureMode::InvariantViolation);
    }
    // Check tool error rate
    let tool_error_rate = if summary.total_tool_calls > 0 {
        summary.total_tool_errors as f64 / summary.total_tool_calls as f64
    } else {
        0.0
    };
    if tool_error_rate > 0.30 {
        return Some(FailureMode::ToolErrors);
    }
    if summary.total_cycles > 3 {
        return Some(FailureMode::CycleTrapping);
    }
    if summary.peak_drift > 3.0 {
        return Some(FailureMode::DriftAccumulation);
    }
    if summary.final_evidence_coverage < 0.20 && summary.total_rounds > 1 {
        return Some(FailureMode::EvidenceStarvation);
    }
    None
}

fn compute_adaptation_utilization(bounds: &AdaptationBoundsChecker) -> f64 {
    let channels = [
        AdaptationChannel::StructuralReplan,
        AdaptationChannel::StrategyMutation,
        AdaptationChannel::SensitivityShift,
        AdaptationChannel::ModelDowngrade,
    ];
    let sum: f64 = channels.iter().map(|c| bounds.usage_fraction(*c)).sum();
    sum / channels.len() as f64
}

fn classify_evidence_trajectory(
    rounds: &[super::system_metrics::RoundMetrics],
) -> EvidenceTrajectory {
    if rounds.len() < 2 {
        return EvidenceTrajectory::Monotonic;
    }

    let coverages: Vec<f64> = rounds.iter().map(|r| r.evidence_coverage).collect();
    let n = coverages.len() as f64;

    // Simple linear regression slope
    let x_mean = (n - 1.0) / 2.0;
    let y_mean: f64 = coverages.iter().sum::<f64>() / n;
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for (i, &y) in coverages.iter().enumerate() {
        let x = i as f64;
        num += (x - x_mean) * (y - y_mean);
        den += (x - x_mean).powi(2);
    }
    let slope = if den > 0.0 { num / den } else { 0.0 };

    // Late-half slope for plateau detection
    let half = coverages.len() / 2;
    let late_half = &coverages[half..];
    let late_slope = if late_half.len() >= 2 {
        let last = late_half.last().unwrap();
        let first = late_half.first().unwrap();
        (last - first) / (late_half.len() - 1) as f64
    } else {
        slope
    };

    if slope > 0.02 {
        EvidenceTrajectory::Monotonic
    } else if slope >= -0.01 && slope <= 0.02 && late_slope < 0.005 {
        EvidenceTrajectory::Plateaued
    } else if slope < -0.01 {
        EvidenceTrajectory::Declining
    } else {
        EvidenceTrajectory::Erratic
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn make_round(round: usize) -> super::super::system_metrics::RoundMetrics {
        super::super::system_metrics::RoundMetrics {
            round,
            tokens_in: 1000,
            tokens_out: 500,
            tool_calls: 3,
            tool_errors: 0,
            combined_score: 0.7,
            utility_score: 0.6,
            evidence_coverage: 0.5,
            drift_score: 0.2,
            sla_fraction: 0.3,
            token_fraction: 0.25,
            replan_attempts: 0,
            invariant_violations: 0,
            cycle_count: 0,
            round_duration: Duration::from_millis(500),
            oracle_decision: "Continue".into(),
        }
    }

    fn default_policy() -> PolicyConfig {
        PolicyConfig::default()
    }

    fn empty_collectors() -> (DecisionTraceCollector, MetricsCollector, AdaptationBoundsChecker, SystemInvariantChecker) {
        let trace = DecisionTraceCollector::new();
        let metrics = MetricsCollector::new();
        let bounds = AdaptationBoundsChecker::new(Arc::new(default_policy()));
        let invariants = SystemInvariantChecker::new();
        (trace, metrics, bounds, invariants)
    }

    #[test]
    fn phase5_retro_failure_mode_labels_unique() {
        let modes = [
            FailureMode::ToolErrors,
            FailureMode::EvidenceStarvation,
            FailureMode::CycleTrapping,
            FailureMode::DriftAccumulation,
            FailureMode::SLAExhaustion,
            FailureMode::InvariantViolation,
        ];
        let labels: Vec<&str> = modes.iter().map(|m| m.label()).collect();
        let unique: std::collections::HashSet<&str> = labels.iter().copied().collect();
        assert_eq!(labels.len(), unique.len());
    }

    #[test]
    fn phase5_retro_evidence_trajectory_labels() {
        let trajs = [
            EvidenceTrajectory::Monotonic,
            EvidenceTrajectory::Plateaued,
            EvidenceTrajectory::Declining,
            EvidenceTrajectory::Erratic,
        ];
        for t in &trajs {
            assert!(!t.label().is_empty());
        }
    }

    #[test]
    fn phase5_retro_empty_session() {
        let (trace, metrics, bounds, invariants) = empty_collectors();
        let profile = analyze(&trace, &metrics, &bounds, &invariants, &default_policy());
        assert!((profile.convergence_efficiency - 1.0).abs() < 1e-4);
        assert_eq!(profile.wasted_rounds, 0);
        assert!(profile.dominant_failure_mode.is_none());
    }

    #[test]
    fn phase5_retro_convergence_efficiency_optimal() {
        let (trace, mut metrics, bounds, invariants) = empty_collectors();
        // Round where utility > synthesis_threshold (0.35) immediately
        let mut r0 = make_round(0);
        r0.utility_score = 0.50;
        r0.evidence_coverage = 0.40;
        metrics.record_round(r0);
        let profile = analyze(&trace, &metrics, &bounds, &invariants, &default_policy());
        assert!((profile.convergence_efficiency - 1.0).abs() < 1e-4);
    }

    #[test]
    fn phase5_retro_convergence_efficiency_suboptimal() {
        let (trace, mut metrics, bounds, invariants) = empty_collectors();
        let mut r0 = make_round(0);
        r0.utility_score = 0.10; // below threshold
        r0.evidence_coverage = 0.10;
        let mut r1 = make_round(1);
        r1.utility_score = 0.20; // below threshold
        r1.evidence_coverage = 0.20;
        let mut r2 = make_round(2);
        r2.utility_score = 0.50; // above threshold
        r2.evidence_coverage = 0.50;
        let mut r3 = make_round(3);
        r3.utility_score = 0.60;
        r3.evidence_coverage = 0.60;
        metrics.record_round(r0);
        metrics.record_round(r1);
        metrics.record_round(r2);
        metrics.record_round(r3);
        let profile = analyze(&trace, &metrics, &bounds, &invariants, &default_policy());
        // Optimal at round 3 (index 2+1=3), total 4 rounds → 3/4 = 0.75
        assert!((profile.convergence_efficiency - 0.75).abs() < 1e-4);
    }

    #[test]
    fn phase5_retro_wasted_rounds_counted() {
        let (trace, mut metrics, bounds, invariants) = empty_collectors();
        let mut r0 = make_round(0);
        r0.combined_score = 0.05; // below 0.10 threshold
        r0.evidence_coverage = 0.30;
        let mut r1 = make_round(1);
        r1.combined_score = 0.70;
        r1.evidence_coverage = 0.50;
        let mut r2 = make_round(2);
        r2.combined_score = 0.02; // below threshold
        r2.evidence_coverage = 0.60;
        metrics.record_round(r0);
        metrics.record_round(r1);
        metrics.record_round(r2);
        let profile = analyze(&trace, &metrics, &bounds, &invariants, &default_policy());
        assert_eq!(profile.wasted_rounds, 2);
    }

    #[test]
    fn phase5_retro_tool_errors_failure_mode() {
        let (trace, mut metrics, bounds, invariants) = empty_collectors();
        let mut r0 = make_round(0);
        r0.tool_calls = 10;
        r0.tool_errors = 5;
        r0.evidence_coverage = 0.30;
        let mut r1 = make_round(1);
        r1.tool_calls = 10;
        r1.tool_errors = 5;
        r1.evidence_coverage = 0.50;
        metrics.record_round(r0);
        metrics.record_round(r1);
        let profile = analyze(&trace, &metrics, &bounds, &invariants, &default_policy());
        assert_eq!(profile.dominant_failure_mode, Some(FailureMode::ToolErrors));
    }

    #[test]
    fn phase5_retro_evidence_starvation_failure_mode() {
        let (trace, mut metrics, bounds, invariants) = empty_collectors();
        let mut r0 = make_round(0);
        r0.evidence_coverage = 0.05;
        let mut r1 = make_round(1);
        r1.evidence_coverage = 0.10;
        metrics.record_round(r0);
        metrics.record_round(r1);
        let profile = analyze(&trace, &metrics, &bounds, &invariants, &default_policy());
        assert_eq!(profile.dominant_failure_mode, Some(FailureMode::EvidenceStarvation));
    }

    #[test]
    fn phase5_retro_evidence_trajectory_monotonic() {
        let rounds = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let trajectory = classify_evidence_trajectory_from_coverages(&rounds);
        assert_eq!(trajectory, EvidenceTrajectory::Monotonic);
    }

    #[test]
    fn phase5_retro_evidence_trajectory_declining() {
        let rounds = vec![0.5, 0.4, 0.3, 0.2, 0.1];
        let trajectory = classify_evidence_trajectory_from_coverages(&rounds);
        assert_eq!(trajectory, EvidenceTrajectory::Declining);
    }

    #[test]
    fn phase5_retro_adaptation_utilization_zero() {
        let (_, _, bounds, _) = empty_collectors();
        let utilization = compute_adaptation_utilization(&bounds);
        assert!((utilization - 0.0).abs() < 1e-4);
    }

    #[test]
    fn phase5_retro_decision_density() {
        let (mut trace, mut metrics, bounds, invariants) = empty_collectors();
        let mut r0 = make_round(0);
        r0.evidence_coverage = 0.30;
        let mut r1 = make_round(1);
        r1.evidence_coverage = 0.50;
        metrics.record_round(r0);
        metrics.record_round(r1);
        use super::super::decision_trace::{DecisionRecord, DecisionPoint};
        trace.record(DecisionRecord::new(DecisionPoint::OracleAdjudication, 0, "Continue"));
        trace.record(DecisionRecord::new(DecisionPoint::UtilityEvaluation, 0, "productive"));
        trace.record(DecisionRecord::new(DecisionPoint::OracleAdjudication, 1, "Continue"));
        let profile = analyze(&trace, &metrics, &bounds, &invariants, &default_policy());
        assert!((profile.decision_density - 1.5).abs() < 1e-4, "3 decisions / 2 rounds = 1.5");
    }

    // Helper to test trajectory classification with raw coverages
    fn classify_evidence_trajectory_from_coverages(coverages: &[f64]) -> EvidenceTrajectory {
        let rounds: Vec<super::super::system_metrics::RoundMetrics> = coverages
            .iter()
            .enumerate()
            .map(|(i, &cov)| {
                let mut r = make_round(i);
                r.evidence_coverage = cov;
                r
            })
            .collect();
        classify_evidence_trajectory(&rounds)
    }
}
