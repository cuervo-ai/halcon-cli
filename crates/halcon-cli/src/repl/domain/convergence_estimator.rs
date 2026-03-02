//! Predictive Convergence Estimator — Forecasts convergence probability (P5.4).
//!
//! Stateless prediction of rounds remaining and convergence probability based on
//! utility trend, evidence rate, and SLA bounds. Used by convergence_phase to
//! inform the termination oracle and boost synthesis urgency when convergence is
//! unlikely.
//!
//! Pure business logic — no I/O.

use super::system_metrics::RoundMetrics;

// ── ForecastBasis ──────────────────────────────────────────────────────────

/// Primary signal underlying a convergence forecast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForecastBasis {
    /// Extrapolating utility improvement rate.
    UtilityTrend,
    /// Evidence gathering trajectory.
    EvidenceRate,
    /// Hard deadline constraint (SLA).
    SLABound,
    /// Too few rounds for meaningful prediction.
    InsufficientData,
}

impl ForecastBasis {
    /// Short label for logging.
    pub fn label(self) -> &'static str {
        match self {
            Self::UtilityTrend => "utility-trend",
            Self::EvidenceRate => "evidence-rate",
            Self::SLABound => "sla-bound",
            Self::InsufficientData => "insufficient-data",
        }
    }
}

// ── ConvergenceForecast ────────────────────────────────────────────────────

/// Prediction result.
#[derive(Debug, Clone)]
pub struct ConvergenceForecast {
    /// Probability of converging before SLA exhaustion [0.0, 1.0].
    pub probability: f64,
    /// Estimated rounds until synthesis threshold is reached.
    pub estimated_rounds_remaining: usize,
    /// Forecast confidence [0.0, 1.0].
    pub confidence: f64,
    /// Which signal was dominant in the prediction.
    pub basis: ForecastBasis,
}

// ── Forecast function ──────────────────────────────────────────────────────

/// Predict convergence from round history and extrapolated trends.
///
/// # Parameters
/// - `metrics`: All round metrics collected so far.
/// - `utility_trend`: Per-round utility improvement rate (positive = improving).
/// - `evidence_rate`: Per-round evidence coverage growth rate.
/// - `sla_remaining_rounds`: Rounds left before SLA exhaustion.
/// - `synthesis_threshold`: Utility score above which synthesis can proceed.
/// - `min_rounds`: Minimum data points for meaningful prediction.
pub fn forecast(
    metrics: &[RoundMetrics],
    utility_trend: f64,
    evidence_rate: f64,
    sla_remaining_rounds: usize,
    synthesis_threshold: f64,
    min_rounds: usize,
) -> ConvergenceForecast {
    // Guard: insufficient data
    if metrics.len() < min_rounds {
        return ConvergenceForecast {
            probability: 0.50,
            estimated_rounds_remaining: sla_remaining_rounds,
            confidence: 0.20,
            basis: ForecastBasis::InsufficientData,
        };
    }

    let current_utility = metrics.last().map(|r| r.utility_score).unwrap_or(0.0);
    let current_evidence = metrics.last().map(|r| r.evidence_coverage).unwrap_or(0.0);

    // Utility extrapolation
    let rounds_by_utility = if utility_trend > 0.001 {
        let gap = (synthesis_threshold - current_utility).max(0.0);
        (gap / utility_trend).ceil() as usize
    } else {
        sla_remaining_rounds // no improvement → need all remaining rounds
    };

    // Evidence extrapolation (adequate evidence = 0.60)
    let adequate_evidence = 0.60;
    let rounds_by_evidence = if evidence_rate > 0.001 && current_evidence < adequate_evidence {
        let gap = adequate_evidence - current_evidence;
        (gap / evidence_rate).ceil() as usize
    } else if current_evidence >= adequate_evidence {
        0 // already sufficient
    } else {
        sla_remaining_rounds // no progress → need all remaining
    };

    // Conservative estimate: use the larger of the two
    let estimated_remaining = rounds_by_utility.max(rounds_by_evidence).min(sla_remaining_rounds);

    // Probability: higher when plenty of budget remains
    let probability = if sla_remaining_rounds == 0 {
        if current_utility >= synthesis_threshold { 1.0 } else { 0.0 }
    } else {
        (1.0 - estimated_remaining as f64 / sla_remaining_rounds as f64).clamp(0.0, 1.0)
    };

    // Dominant basis
    let basis = if rounds_by_utility > rounds_by_evidence {
        ForecastBasis::UtilityTrend
    } else if rounds_by_evidence > 0 && current_evidence < adequate_evidence {
        ForecastBasis::EvidenceRate
    } else {
        ForecastBasis::UtilityTrend
    };

    // Confidence: function of data points, capped at 0.85
    let confidence = ((metrics.len() as f64 / 10.0).min(0.85)).max(0.30);

    ConvergenceForecast {
        probability,
        estimated_rounds_remaining: estimated_remaining,
        confidence,
        basis,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_round(round: usize, utility: f64, evidence: f64) -> RoundMetrics {
        RoundMetrics {
            round,
            tokens_in: 1000,
            tokens_out: 500,
            tool_calls: 3,
            tool_errors: 0,
            combined_score: 0.7,
            utility_score: utility,
            evidence_coverage: evidence,
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

    #[test]
    fn phase5_estimator_insufficient_data() {
        let metrics = vec![make_round(0, 0.3, 0.2)];
        let result = forecast(&metrics, 0.1, 0.05, 10, 0.35, 3);
        assert_eq!(result.basis, ForecastBasis::InsufficientData);
        assert!((result.probability - 0.50).abs() < 1e-4);
        assert!((result.confidence - 0.20).abs() < 1e-4);
    }

    #[test]
    fn phase5_estimator_high_probability_good_trend() {
        let metrics = vec![
            make_round(0, 0.10, 0.20),
            make_round(1, 0.20, 0.35),
            make_round(2, 0.30, 0.50),
        ];
        // Utility trend = 0.10/round, evidence rate = 0.15/round
        let result = forecast(&metrics, 0.10, 0.15, 10, 0.35, 3);
        assert!(result.probability > 0.50, "should be high probability with good trend");
        assert_eq!(result.estimated_rounds_remaining, 1); // gap=0.05, 0.05/0.10=1
        assert!(result.confidence >= 0.30);
    }

    #[test]
    fn phase5_estimator_low_probability_stalled() {
        let metrics = vec![
            make_round(0, 0.10, 0.10),
            make_round(1, 0.10, 0.10),
            make_round(2, 0.10, 0.10),
        ];
        // No improvement
        let result = forecast(&metrics, 0.0, 0.0, 3, 0.35, 3);
        assert!(result.probability < 0.01, "stalled session should have low probability");
        assert_eq!(result.estimated_rounds_remaining, 3);
    }

    #[test]
    fn phase5_estimator_zero_sla_remaining_above_threshold() {
        let metrics = vec![
            make_round(0, 0.10, 0.20),
            make_round(1, 0.20, 0.35),
            make_round(2, 0.40, 0.65), // above threshold
        ];
        let result = forecast(&metrics, 0.10, 0.15, 0, 0.35, 3);
        assert!((result.probability - 1.0).abs() < 1e-4);
    }

    #[test]
    fn phase5_estimator_zero_sla_remaining_below_threshold() {
        let metrics = vec![
            make_round(0, 0.10, 0.10),
            make_round(1, 0.15, 0.15),
            make_round(2, 0.20, 0.20),
        ];
        let result = forecast(&metrics, 0.05, 0.05, 0, 0.35, 3);
        assert!((result.probability - 0.0).abs() < 1e-4);
    }

    #[test]
    fn phase5_estimator_evidence_bottleneck() {
        let metrics = vec![
            make_round(0, 0.30, 0.10),
            make_round(1, 0.33, 0.15),
            make_round(2, 0.35, 0.20),
        ];
        // Utility is at threshold but evidence is way behind (0.20 vs 0.60 adequate)
        let result = forecast(&metrics, 0.025, 0.05, 10, 0.35, 3);
        // rounds_by_evidence = ceil((0.60 - 0.20) / 0.05) = 8
        assert!(result.estimated_rounds_remaining >= 7);
        assert_eq!(result.basis, ForecastBasis::EvidenceRate);
    }

    #[test]
    fn phase5_estimator_confidence_scales_with_data() {
        let metrics3: Vec<_> = (0..3).map(|i| make_round(i, 0.3, 0.4)).collect();
        let metrics8: Vec<_> = (0..8).map(|i| make_round(i, 0.3, 0.4)).collect();
        let r3 = forecast(&metrics3, 0.05, 0.05, 10, 0.35, 3);
        let r8 = forecast(&metrics8, 0.05, 0.05, 10, 0.35, 3);
        assert!(r8.confidence > r3.confidence, "more data should yield higher confidence");
    }

    #[test]
    fn phase5_estimator_forecast_basis_labels_unique() {
        let bases = [
            ForecastBasis::UtilityTrend,
            ForecastBasis::EvidenceRate,
            ForecastBasis::SLABound,
            ForecastBasis::InsufficientData,
        ];
        let labels: Vec<&str> = bases.iter().map(|b| b.label()).collect();
        let unique: std::collections::HashSet<&str> = labels.iter().copied().collect();
        assert_eq!(labels.len(), unique.len());
    }

    #[test]
    fn phase5_estimator_empty_metrics() {
        let result = forecast(&[], 0.1, 0.05, 10, 0.35, 3);
        assert_eq!(result.basis, ForecastBasis::InsufficientData);
    }

    #[test]
    fn phase5_estimator_already_converged() {
        let metrics = vec![
            make_round(0, 0.20, 0.40),
            make_round(1, 0.30, 0.55),
            make_round(2, 0.50, 0.70), // above threshold, adequate evidence
        ];
        let result = forecast(&metrics, 0.15, 0.15, 8, 0.35, 3);
        assert!((result.probability - 1.0).abs() < 1e-4);
        assert_eq!(result.estimated_rounds_remaining, 0);
    }
}
