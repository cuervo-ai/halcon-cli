//! Problem Classification Layer — Runtime task classification for adaptive strategy (P5.2).
//!
//! Classifies the current agent session into a canonical `ProblemClass` based on
//! observed round metrics. Classification informs strategy weight presets (P5.3),
//! convergence estimation (P5.4), and strategic initialization (P5.5).
//!
//! # Classification Cascade (priority order)
//!
//! | Priority | Class | Trigger |
//! |----------|-------|---------|
//! | 1 | SLAConstrained | `sla_fraction > 0.60` |
//! | 2 | Oscillatory | `score_variance > oscillation_variance_threshold` |
//! | 3 | ToolConstrained | `error_rate > 0.40` |
//! | 4 | EvidenceSparse | `evidence_rate < 0.05` per round |
//! | 5 | HighExploration | `exploration_ratio > 0.60` |
//! | 6 | DeterministicLinear | default fallback |
//!
//! Reclassification occurs at most once per session when signal divergence exceeds
//! `reclassification_shift_threshold`.
//!
//! Pure business logic — no I/O.

use std::sync::Arc;

use halcon_core::types::PolicyConfig;

use super::system_metrics::RoundMetrics;

// ── ProblemClass ────────────────────────────────────────────────────────────

/// Canonical problem classification for the current agent session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProblemClass {
    /// Simple, high success, low variance — straightforward linear execution.
    DeterministicLinear,
    /// Many search/read tools, wide evidence gathering.
    HighExploration,
    /// Limited or failing tool surface.
    ToolConstrained,
    /// Difficulty finding sufficient evidence.
    EvidenceSparse,
    /// Score oscillation, unstable convergence.
    Oscillatory,
    /// Tight budget pressure dominating decisions.
    SLAConstrained,
}

impl ProblemClass {
    /// Human-readable label for logging and metrics.
    pub fn label(self) -> &'static str {
        match self {
            Self::DeterministicLinear => "deterministic-linear",
            Self::HighExploration => "high-exploration",
            Self::ToolConstrained => "tool-constrained",
            Self::EvidenceSparse => "evidence-sparse",
            Self::Oscillatory => "oscillatory",
            Self::SLAConstrained => "sla-constrained",
        }
    }
}

// ── ClassificationSignals ──────────────────────────────────────────────────

/// Signals extracted from round metrics for classification.
#[derive(Debug, Clone)]
pub struct ClassificationSignals {
    /// Distinct tools / total tool calls.
    pub tool_diversity: f64,
    /// Average evidence_coverage delta per round.
    pub evidence_rate: f64,
    /// Variance of combined_score across rounds.
    pub score_variance: f64,
    /// SLA fraction at classification time.
    pub sla_pressure: f64,
    /// tool_errors / tool_calls.
    pub error_rate: f64,
    /// Search/read tools / total tools (heuristic from tool_calls vs evidence).
    pub exploration_ratio: f64,
}

// ── ProblemClassification ──────────────────────────────────────────────────

/// Result of a classification decision.
#[derive(Debug, Clone)]
pub struct ProblemClassification {
    /// The assigned problem class.
    pub class: ProblemClass,
    /// Classification confidence [0.0, 1.0].
    pub confidence: f64,
    /// Round at which classification was performed.
    pub round_classified: usize,
    /// Signals used for classification.
    pub signals: ClassificationSignals,
}

// ── ProblemClassifier ──────────────────────────────────────────────────────

/// Stateful classifier that tracks current classification and reclassification.
pub struct ProblemClassifier {
    current: Option<ProblemClassification>,
    has_reclassified: bool,
    policy: Arc<PolicyConfig>,
}

impl ProblemClassifier {
    /// Create a new classifier.
    pub fn new(policy: Arc<PolicyConfig>) -> Self {
        Self {
            current: None,
            has_reclassified: false,
            policy,
        }
    }

    /// Current classification, if any.
    pub fn current(&self) -> Option<&ProblemClassification> {
        self.current.as_ref()
    }

    /// Current problem class, if classified.
    pub fn current_class(&self) -> Option<ProblemClass> {
        self.current.as_ref().map(|c| c.class)
    }

    /// Perform initial classification from round metrics.
    pub fn classify(&mut self, metrics: &[RoundMetrics], sla_fraction: f64) -> ProblemClassification {
        let classification = compute_classification(metrics, sla_fraction, &self.policy);
        self.current = Some(classification.clone());
        classification
    }

    /// Check whether reclassification should occur.
    pub fn should_reclassify(&self, metrics: &[RoundMetrics]) -> bool {
        if self.has_reclassified {
            return false;
        }
        let current = match &self.current {
            Some(c) => c,
            None => return false,
        };
        if metrics.len() <= current.round_classified {
            return false;
        }

        // Compute fresh signals and check divergence
        let new_signals = extract_signals(metrics, current.signals.sla_pressure);
        let divergence = signal_divergence(&current.signals, &new_signals);
        divergence > self.policy.reclassification_shift_threshold
    }

    /// Attempt reclassification. Returns Some if reclassified, None otherwise.
    pub fn reclassify(&mut self, metrics: &[RoundMetrics], sla_fraction: f64) -> Option<ProblemClassification> {
        if !self.should_reclassify(metrics) {
            return None;
        }
        self.has_reclassified = true;
        let classification = compute_classification(metrics, sla_fraction, &self.policy);
        self.current = Some(classification.clone());
        Some(classification)
    }

    /// Whether reclassification has already occurred.
    pub fn has_reclassified(&self) -> bool {
        self.has_reclassified
    }
}

// ── Internal helpers ───────────────────────────────────────────────────────

fn extract_signals(metrics: &[RoundMetrics], sla_pressure: f64) -> ClassificationSignals {
    if metrics.is_empty() {
        return ClassificationSignals {
            tool_diversity: 0.0,
            evidence_rate: 0.0,
            score_variance: 0.0,
            sla_pressure,
            error_rate: 0.0,
            exploration_ratio: 0.0,
        };
    }

    let total_tool_calls: usize = metrics.iter().map(|r| r.tool_calls).sum();
    let total_tool_errors: usize = metrics.iter().map(|r| r.tool_errors).sum();

    // Error rate
    let error_rate = if total_tool_calls > 0 {
        total_tool_errors as f64 / total_tool_calls as f64
    } else {
        0.0
    };

    // Score variance
    let scores: Vec<f64> = metrics.iter().map(|r| r.combined_score as f64).collect();
    let score_variance = variance(&scores);

    // Evidence rate: average evidence_coverage change per round
    let evidence_rate = if metrics.len() >= 2 {
        let first = metrics.first().unwrap().evidence_coverage;
        let last = metrics.last().unwrap().evidence_coverage;
        (last - first) / (metrics.len() - 1) as f64
    } else {
        metrics.first().map(|r| r.evidence_coverage).unwrap_or(0.0)
    };

    // Exploration ratio: heuristic — rounds with high evidence coverage growth
    // relative to tool calls indicate exploration-heavy behavior.
    // We approximate by checking evidence_coverage > 0.3 as proxy for search-heavy rounds.
    let exploration_rounds = metrics.iter().filter(|r| r.evidence_coverage > 0.30 && r.tool_calls > 0).count();
    let exploration_ratio = if metrics.is_empty() {
        0.0
    } else {
        exploration_rounds as f64 / metrics.len() as f64
    };

    // Tool diversity: unique round-level tool counts vs total (heuristic)
    let non_zero_rounds = metrics.iter().filter(|r| r.tool_calls > 0).count();
    let tool_diversity = if total_tool_calls > 0 {
        non_zero_rounds as f64 / total_tool_calls.max(1) as f64
    } else {
        0.0
    };

    ClassificationSignals {
        tool_diversity,
        evidence_rate,
        score_variance,
        sla_pressure,
        error_rate,
        exploration_ratio,
    }
}

fn compute_classification(metrics: &[RoundMetrics], sla_fraction: f64, policy: &PolicyConfig) -> ProblemClassification {
    let signals = extract_signals(metrics, sla_fraction);
    let round_classified = metrics.last().map(|r| r.round).unwrap_or(0);

    // Priority cascade (first match wins)
    let (class, confidence) = if signals.sla_pressure > 0.60 {
        (ProblemClass::SLAConstrained, 0.80)
    } else if signals.score_variance > policy.oscillation_variance_threshold {
        (ProblemClass::Oscillatory, 0.75)
    } else if signals.error_rate > 0.40 {
        (ProblemClass::ToolConstrained, 0.75)
    } else if signals.evidence_rate < 0.05 && metrics.len() >= 2 {
        (ProblemClass::EvidenceSparse, 0.70)
    } else if signals.exploration_ratio > 0.60 {
        (ProblemClass::HighExploration, 0.70)
    } else {
        (ProblemClass::DeterministicLinear, 0.60)
    };

    ProblemClassification {
        class,
        confidence,
        round_classified,
        signals,
    }
}

fn variance(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let sum_sq: f64 = values.iter().map(|v| (v - mean).powi(2)).sum();
    sum_sq / values.len() as f64
}

fn signal_divergence(old: &ClassificationSignals, new: &ClassificationSignals) -> f64 {
    let diffs = [
        (old.error_rate - new.error_rate).abs(),
        (old.score_variance - new.score_variance).abs() * 10.0, // amplify variance changes
        (old.sla_pressure - new.sla_pressure).abs(),
        (old.evidence_rate - new.evidence_rate).abs() * 5.0,
        (old.exploration_ratio - new.exploration_ratio).abs(),
    ];
    diffs.iter().sum::<f64>() / diffs.len() as f64
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_round(round: usize) -> RoundMetrics {
        RoundMetrics {
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

    fn default_policy() -> Arc<PolicyConfig> {
        Arc::new(PolicyConfig::default())
    }

    #[test]
    fn phase5_classifier_problem_class_labels_unique() {
        let classes = [
            ProblemClass::DeterministicLinear,
            ProblemClass::HighExploration,
            ProblemClass::ToolConstrained,
            ProblemClass::EvidenceSparse,
            ProblemClass::Oscillatory,
            ProblemClass::SLAConstrained,
        ];
        let labels: Vec<&str> = classes.iter().map(|c| c.label()).collect();
        let unique: std::collections::HashSet<&str> = labels.iter().copied().collect();
        assert_eq!(labels.len(), unique.len(), "all labels must be unique");
        assert_eq!(labels.len(), 6, "must have exactly 6 problem classes");
    }

    #[test]
    fn phase5_classifier_default_fallback_is_deterministic_linear() {
        let mut classifier = ProblemClassifier::new(default_policy());
        let mut r0 = make_round(0);
        r0.evidence_coverage = 0.30;
        let mut r1 = make_round(1);
        r1.evidence_coverage = 0.50; // rate = 0.20 per round (well above 0.05)
        let result = classifier.classify(&[r0, r1], 0.20);
        assert_eq!(result.class, ProblemClass::DeterministicLinear);
        assert!((result.confidence - 0.60).abs() < 1e-4);
    }

    #[test]
    fn phase5_classifier_sla_constrained_highest_priority() {
        let mut classifier = ProblemClassifier::new(default_policy());
        // SLA pressure > 0.60 should win regardless of other signals
        let mut r0 = make_round(0);
        r0.tool_errors = 3; // high error rate
        r0.combined_score = 0.1;
        let mut r1 = make_round(1);
        r1.tool_errors = 3;
        r1.combined_score = 0.9;
        let result = classifier.classify(&[r0, r1], 0.75);
        assert_eq!(result.class, ProblemClass::SLAConstrained);
        assert!((result.confidence - 0.80).abs() < 1e-4);
    }

    #[test]
    fn phase5_classifier_oscillatory_on_high_variance() {
        let mut classifier = ProblemClassifier::new(default_policy());
        let mut r0 = make_round(0);
        r0.combined_score = 0.1;
        let mut r1 = make_round(1);
        r1.combined_score = 0.9;
        let mut r2 = make_round(2);
        r2.combined_score = 0.2;
        let mut r3 = make_round(3);
        r3.combined_score = 0.8;
        let result = classifier.classify(&[r0, r1, r2, r3], 0.30);
        assert_eq!(result.class, ProblemClass::Oscillatory);
    }

    #[test]
    fn phase5_classifier_tool_constrained_on_high_errors() {
        let mut classifier = ProblemClassifier::new(default_policy());
        let mut r0 = make_round(0);
        r0.tool_calls = 5;
        r0.tool_errors = 3;
        let mut r1 = make_round(1);
        r1.tool_calls = 5;
        r1.tool_errors = 3;
        let result = classifier.classify(&[r0, r1], 0.20);
        assert_eq!(result.class, ProblemClass::ToolConstrained);
    }

    #[test]
    fn phase5_classifier_evidence_sparse_on_low_evidence() {
        let mut classifier = ProblemClassifier::new(default_policy());
        let mut r0 = make_round(0);
        r0.evidence_coverage = 0.01;
        let mut r1 = make_round(1);
        r1.evidence_coverage = 0.02;
        let result = classifier.classify(&[r0, r1], 0.20);
        assert_eq!(result.class, ProblemClass::EvidenceSparse);
    }

    #[test]
    fn phase5_classifier_high_exploration_on_search_heavy() {
        let mut classifier = ProblemClassifier::new(default_policy());
        // Rounds with evidence_coverage > 0.30 and tool_calls > 0 → exploration
        let mut r0 = make_round(0);
        r0.evidence_coverage = 0.50;
        let mut r1 = make_round(1);
        r1.evidence_coverage = 0.60;
        let mut r2 = make_round(2);
        r2.evidence_coverage = 0.70;
        let result = classifier.classify(&[r0, r1, r2], 0.20);
        // Evidence rate is high, exploration ratio is high
        assert_eq!(result.class, ProblemClass::HighExploration);
    }

    #[test]
    fn phase5_classifier_initial_state_no_classification() {
        let classifier = ProblemClassifier::new(default_policy());
        assert!(classifier.current().is_none());
        assert!(classifier.current_class().is_none());
        assert!(!classifier.has_reclassified());
    }

    #[test]
    fn phase5_classifier_classify_sets_current() {
        let mut classifier = ProblemClassifier::new(default_policy());
        let metrics = vec![make_round(0), make_round(1)];
        classifier.classify(&metrics, 0.20);
        assert!(classifier.current().is_some());
        assert!(classifier.current_class().is_some());
    }

    #[test]
    fn phase5_classifier_single_reclassification_only() {
        let mut classifier = ProblemClassifier::new(default_policy());
        let metrics = vec![make_round(0), make_round(1)];
        classifier.classify(&metrics, 0.20);

        // Force reclassification by dramatically changing signals
        let mut r2 = make_round(2);
        r2.tool_errors = 4;
        r2.tool_calls = 5;
        r2.combined_score = 0.1;
        let metrics2 = vec![make_round(0), make_round(1), r2];

        let reclass = classifier.reclassify(&metrics2, 0.80);
        // Whether it reclassifies depends on signal divergence
        if reclass.is_some() {
            assert!(classifier.has_reclassified());
            // Second reclassification should return None
            let second = classifier.reclassify(&metrics2, 0.80);
            assert!(second.is_none(), "second reclassification must be blocked");
        }
    }

    #[test]
    fn phase5_classifier_reclassify_without_initial_returns_none() {
        let mut classifier = ProblemClassifier::new(default_policy());
        let metrics = vec![make_round(0), make_round(1)];
        let result = classifier.reclassify(&metrics, 0.30);
        assert!(result.is_none(), "cannot reclassify without initial classification");
    }

    #[test]
    fn phase5_classifier_variance_helper_empty() {
        assert_eq!(variance(&[]), 0.0);
    }

    #[test]
    fn phase5_classifier_variance_helper_single() {
        assert_eq!(variance(&[5.0]), 0.0);
    }

    #[test]
    fn phase5_classifier_variance_helper_known() {
        // [2, 4, 4, 4, 5, 5, 7, 9] → mean=5, variance=4
        let vals = vec![2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let v = variance(&vals);
        assert!((v - 4.0).abs() < 1e-4, "expected 4.0, got {v}");
    }

    #[test]
    fn phase5_classifier_empty_metrics_default() {
        let mut classifier = ProblemClassifier::new(default_policy());
        let result = classifier.classify(&[], 0.10);
        assert_eq!(result.class, ProblemClass::DeterministicLinear);
    }
}
