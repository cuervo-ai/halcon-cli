//! Decision Traceability Layer — Per-round decision audit trail (P4.2).
//!
//! Records every significant decision made during the agent loop with its
//! input signals, output action, and rationale. Enables post-session analysis,
//! debugging of convergence behavior, and formal verification of decision paths.
//!
//! # Decision Points
//!
//! | Point | Component | Significance |
//! |-------|-----------|-------------|
//! | OracleAdjudication | TerminationOracle | Loop continuation/termination |
//! | StrategyMutation | MidLoopStrategy | Replan structural choice |
//! | CapabilitySkip | CapabilityValidator | Plan step skip |
//! | CriticCheckpoint | MidLoopCritic | Mid-loop intervention |
//! | ComplexityUpgrade | ComplexityTracker | SLA/budget escalation |
//! | UtilityEvaluation | ConvergenceUtility | Synthesis timing |
//! | AdaptivePolicyShift | AdaptivePolicy | Runtime parameter change |
//! | ToolTrustAction | ToolTrustScorer | Tool hide/deprioritize |
//! | SynthesisForced | Various | Forced synthesis origin |
//! | EbsGateFired | EvidencePipeline | Evidence gate intercept |
//!
//! Pure business logic — no I/O.

// ── DecisionPoint ────────────────────────────────────────────────────────────

/// Identifies which component made a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecisionPoint {
    /// TerminationOracle adjudicated round feedback.
    OracleAdjudication,
    /// MidLoopStrategy selected a replan mutation.
    StrategyMutation,
    /// CapabilityValidator skipped a plan step.
    CapabilitySkip,
    /// MidLoopCritic issued a checkpoint action.
    CriticCheckpoint,
    /// ComplexityTracker triggered a complexity upgrade.
    ComplexityUpgrade,
    /// ConvergenceUtility evaluated synthesis timing.
    UtilityEvaluation,
    /// AdaptivePolicy shifted runtime parameters.
    AdaptivePolicyShift,
    /// ToolTrustScorer hid or deprioritized a tool.
    ToolTrustAction,
    /// Forced synthesis from any origin.
    SynthesisForced,
    /// Evidence gate fired (EBS intercept).
    EbsGateFired,
}

impl DecisionPoint {
    /// Short label for structured logging.
    pub fn label(self) -> &'static str {
        match self {
            Self::OracleAdjudication => "oracle",
            Self::StrategyMutation => "strategy",
            Self::CapabilitySkip => "capability-skip",
            Self::CriticCheckpoint => "critic-checkpoint",
            Self::ComplexityUpgrade => "complexity-upgrade",
            Self::UtilityEvaluation => "utility",
            Self::AdaptivePolicyShift => "adaptive-policy",
            Self::ToolTrustAction => "tool-trust",
            Self::SynthesisForced => "synthesis-forced",
            Self::EbsGateFired => "ebs-gate",
        }
    }
}

// ── DecisionRecord ───────────────────────────────────────────────────────────

/// A single recorded decision with full context.
#[derive(Debug, Clone)]
pub struct DecisionRecord {
    /// Which component made the decision.
    pub point: DecisionPoint,
    /// Round at which the decision was made.
    pub round: usize,
    /// Key input signals that influenced the decision (human-readable).
    pub inputs: Vec<(String, String)>,
    /// The action taken (e.g., "Continue", "Replan", "ForceSynthesis").
    pub action: String,
    /// Why this action was chosen (component's rationale string).
    pub rationale: String,
    /// Confidence level if applicable (0.0–1.0). None for deterministic decisions.
    pub confidence: Option<f32>,
}

impl DecisionRecord {
    /// Create a new record with the given point, round, and action.
    pub fn new(point: DecisionPoint, round: usize, action: impl Into<String>) -> Self {
        Self {
            point,
            round,
            inputs: Vec::new(),
            action: action.into(),
            rationale: String::new(),
            confidence: None,
        }
    }

    /// Add an input signal to the record.
    pub fn with_input(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.inputs.push((key.into(), value.into()));
        self
    }

    /// Set the rationale.
    pub fn with_rationale(mut self, rationale: impl Into<String>) -> Self {
        self.rationale = rationale.into();
        self
    }

    /// Set the confidence.
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = Some(confidence);
        self
    }
}

// ── DecisionTraceCollector ───────────────────────────────────────────────────

/// Stateful collector that accumulates decision records across the session.
///
/// Capacity-bounded to prevent unbounded memory growth in long sessions.
pub struct DecisionTraceCollector {
    records: Vec<DecisionRecord>,
    max_records: usize,
}

impl DecisionTraceCollector {
    /// Create a new collector with default capacity (500 records).
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            max_records: 500,
        }
    }

    /// Create a collector with custom capacity.
    pub fn with_capacity(max_records: usize) -> Self {
        Self {
            records: Vec::new(),
            max_records,
        }
    }

    /// Record a decision. If at capacity, drops the oldest record.
    pub fn record(&mut self, record: DecisionRecord) {
        if self.records.len() >= self.max_records {
            self.records.remove(0);
        }
        self.records.push(record);
    }

    /// All recorded decisions.
    pub fn records(&self) -> &[DecisionRecord] {
        &self.records
    }

    /// Decisions for a specific round.
    pub fn records_for_round(&self, round: usize) -> Vec<&DecisionRecord> {
        self.records.iter().filter(|r| r.round == round).collect()
    }

    /// Decisions from a specific component.
    pub fn records_for_point(&self, point: DecisionPoint) -> Vec<&DecisionRecord> {
        self.records.iter().filter(|r| r.point == point).collect()
    }

    /// Total number of recorded decisions.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the collector is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Count of decisions grouped by point.
    pub fn counts_by_point(&self) -> std::collections::HashMap<DecisionPoint, usize> {
        let mut counts = std::collections::HashMap::new();
        for r in &self.records {
            *counts.entry(r.point).or_insert(0) += 1;
        }
        counts
    }

    /// The last decision of a given type, if any.
    pub fn last_of(&self, point: DecisionPoint) -> Option<&DecisionRecord> {
        self.records.iter().rev().find(|r| r.point == point)
    }

    /// Summary line for post-session reporting.
    pub fn summary(&self) -> String {
        let counts = self.counts_by_point();
        let mut parts: Vec<String> = counts
            .iter()
            .map(|(p, c)| format!("{}={}", p.label(), c))
            .collect();
        parts.sort();
        format!("decisions: {} total [{}]", self.records.len(), parts.join(", "))
    }

    /// Clear all records.
    pub fn reset(&mut self) {
        self.records.clear();
    }
}

impl Default for DecisionTraceCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase4_trace_record_construction() {
        let record = DecisionRecord::new(DecisionPoint::OracleAdjudication, 3, "Continue")
            .with_input("combined_score", "0.65")
            .with_input("convergence_action", "Continue")
            .with_rationale("no authority fired")
            .with_confidence(0.95);

        assert_eq!(record.point, DecisionPoint::OracleAdjudication);
        assert_eq!(record.round, 3);
        assert_eq!(record.action, "Continue");
        assert_eq!(record.inputs.len(), 2);
        assert_eq!(record.rationale, "no authority fired");
        assert_eq!(record.confidence, Some(0.95));
    }

    #[test]
    fn phase4_trace_collector_empty() {
        let collector = DecisionTraceCollector::new();
        assert!(collector.is_empty());
        assert_eq!(collector.len(), 0);
    }

    #[test]
    fn phase4_trace_collector_record_and_retrieve() {
        let mut collector = DecisionTraceCollector::new();
        collector.record(DecisionRecord::new(DecisionPoint::OracleAdjudication, 0, "Continue"));
        collector.record(DecisionRecord::new(DecisionPoint::UtilityEvaluation, 0, "continue"));
        collector.record(DecisionRecord::new(DecisionPoint::OracleAdjudication, 1, "Replan"));

        assert_eq!(collector.len(), 3);

        let round_0 = collector.records_for_round(0);
        assert_eq!(round_0.len(), 2);

        let oracle = collector.records_for_point(DecisionPoint::OracleAdjudication);
        assert_eq!(oracle.len(), 2);
    }

    #[test]
    fn phase4_trace_collector_capacity_bounded() {
        let mut collector = DecisionTraceCollector::with_capacity(3);
        for i in 0..5 {
            collector.record(DecisionRecord::new(DecisionPoint::OracleAdjudication, i, "Continue"));
        }
        assert_eq!(collector.len(), 3, "should not exceed max_records");
        // Oldest records should have been dropped
        assert_eq!(collector.records()[0].round, 2);
        assert_eq!(collector.records()[2].round, 4);
    }

    #[test]
    fn phase4_trace_collector_counts_by_point() {
        let mut collector = DecisionTraceCollector::new();
        collector.record(DecisionRecord::new(DecisionPoint::OracleAdjudication, 0, "Continue"));
        collector.record(DecisionRecord::new(DecisionPoint::OracleAdjudication, 1, "Replan"));
        collector.record(DecisionRecord::new(DecisionPoint::UtilityEvaluation, 0, "productive"));
        collector.record(DecisionRecord::new(DecisionPoint::EbsGateFired, 2, "intercepted"));

        let counts = collector.counts_by_point();
        assert_eq!(counts[&DecisionPoint::OracleAdjudication], 2);
        assert_eq!(counts[&DecisionPoint::UtilityEvaluation], 1);
        assert_eq!(counts[&DecisionPoint::EbsGateFired], 1);
        assert!(!counts.contains_key(&DecisionPoint::StrategyMutation));
    }

    #[test]
    fn phase4_trace_collector_last_of() {
        let mut collector = DecisionTraceCollector::new();
        collector.record(DecisionRecord::new(DecisionPoint::OracleAdjudication, 0, "Continue"));
        collector.record(DecisionRecord::new(DecisionPoint::OracleAdjudication, 3, "Halt"));

        let last = collector.last_of(DecisionPoint::OracleAdjudication).unwrap();
        assert_eq!(last.round, 3);
        assert_eq!(last.action, "Halt");

        assert!(collector.last_of(DecisionPoint::ComplexityUpgrade).is_none());
    }

    #[test]
    fn phase4_trace_collector_summary() {
        let mut collector = DecisionTraceCollector::new();
        collector.record(DecisionRecord::new(DecisionPoint::OracleAdjudication, 0, "Continue"));
        collector.record(DecisionRecord::new(DecisionPoint::UtilityEvaluation, 0, "productive"));

        let summary = collector.summary();
        assert!(summary.contains("2 total"));
        assert!(summary.contains("oracle=1"));
        assert!(summary.contains("utility=1"));
    }

    #[test]
    fn phase4_trace_collector_reset() {
        let mut collector = DecisionTraceCollector::new();
        collector.record(DecisionRecord::new(DecisionPoint::OracleAdjudication, 0, "Continue"));
        assert_eq!(collector.len(), 1);
        collector.reset();
        assert!(collector.is_empty());
    }

    #[test]
    fn phase4_trace_all_decision_points_have_unique_labels() {
        let points = [
            DecisionPoint::OracleAdjudication,
            DecisionPoint::StrategyMutation,
            DecisionPoint::CapabilitySkip,
            DecisionPoint::CriticCheckpoint,
            DecisionPoint::ComplexityUpgrade,
            DecisionPoint::UtilityEvaluation,
            DecisionPoint::AdaptivePolicyShift,
            DecisionPoint::ToolTrustAction,
            DecisionPoint::SynthesisForced,
            DecisionPoint::EbsGateFired,
        ];
        let labels: Vec<&str> = points.iter().map(|p| p.label()).collect();
        let unique: std::collections::HashSet<&str> = labels.iter().copied().collect();
        assert_eq!(labels.len(), unique.len(), "all labels must be unique");
        assert_eq!(labels.len(), 10, "must have exactly 10 decision points");
    }

    #[test]
    fn phase4_trace_record_builder_pattern() {
        let record = DecisionRecord::new(DecisionPoint::StrategyMutation, 5, "ReplanWithDecomposition")
            .with_input("tool_failure_clustering", "0.65")
            .with_input("plan_completion", "0.20")
            .with_rationale("high failure clustering with low completion");

        assert_eq!(record.inputs.len(), 2);
        assert!(record.confidence.is_none());
        assert_eq!(record.action, "ReplanWithDecomposition");
    }

    #[test]
    fn phase4_trace_records_preserve_insertion_order() {
        let mut collector = DecisionTraceCollector::new();
        let actions = ["A", "B", "C", "D", "E"];
        for (i, action) in actions.iter().enumerate() {
            collector.record(DecisionRecord::new(DecisionPoint::OracleAdjudication, i, *action));
        }
        for (i, record) in collector.records().iter().enumerate() {
            assert_eq!(record.action, actions[i]);
            assert_eq!(record.round, i);
        }
    }
}
