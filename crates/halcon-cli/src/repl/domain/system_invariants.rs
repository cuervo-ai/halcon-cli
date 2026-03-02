//! System Invariants — Formalized global correctness properties (P4.1).
//!
//! Declares and enforces system-wide invariants that must hold at all times
//! during agent loop execution. Each invariant is identified, checked against
//! an [`InvariantSnapshot`], and violations are collected with severity levels.
//!
//! # Invariant Catalog
//!
//! | ID | Name | Property |
//! |----|------|----------|
//! | I1 | EBS Consistency | `synthesis_blocked ⇒ origin == SupervisorFailure` |
//! | I2 | Evidence Before Synthesis | Non-failure synthesis ⇒ evidence ≥ threshold |
//! | I3 | SLA Upgrade Monotonicity | SLA mode only increases (Fast→Balanced→Deep) |
//! | I4 | Single Complexity Upgrade | At most 1 complexity upgrade per session |
//! | I5 | Replan Budget Bounded | `replan_attempts ≤ max_replan_attempts + 1` |
//! | I6 | Round Counter Monotonicity | Round counter never decreases |
//! | I7 | Utility Score Bounded | `0.0 ≤ utility_score ≤ 1.5` |
//! | I8 | Cost Non-Negative | Accumulated cost ≥ 0.0 |
//! | I9 | FSM Terminal Consistency | Terminal phase ⇒ no active tool suppression |
//! | I10 | Drift Bounded | `cumulative_drift ≤ max_drift_bound` |
//!
//! # Usage
//!
//! The checker is stateful: it tracks previous-round values to validate
//! monotonicity properties (I3, I6). Call [`SystemInvariantChecker::check_round()`]
//! once per round with a fresh [`InvariantSnapshot`] built at the agent boundary.
//!
//! In debug builds, violations trigger `tracing::error!` and are collected for
//! post-session analysis. In release builds, only `tracing::warn!` is emitted.
//!
//! Pure business logic — no I/O.

// ── InvariantId ──────────────────────────────────────────────────────────────

/// Identifies each formalized system invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvariantId {
    /// I1: `synthesis_blocked ⇒ synthesis_origin == SupervisorFailure`
    EbsConsistency,
    /// I2: Non-failure synthesis requires evidence ≥ threshold
    EvidenceBeforeSynthesis,
    /// I3: SLA mode only increases across rounds
    SlaUpgradeMonotonicity,
    /// I4: At most one complexity upgrade per session
    SingleComplexityUpgrade,
    /// I5: Replan attempts within policy budget
    ReplanBudgetBounded,
    /// I6: Round counter never decreases
    RoundMonotonicity,
    /// I7: Utility score within valid range
    UtilityScoreBounded,
    /// I8: Accumulated cost is non-negative
    CostNonNegative,
    /// I9: Terminal FSM phase consistency
    FsmTerminalConsistency,
    /// I10: Cumulative drift within acceptable bounds
    DriftBounded,
}

impl InvariantId {
    /// Short human-readable label for logging.
    pub fn label(self) -> &'static str {
        match self {
            Self::EbsConsistency => "I1:EBS-consistency",
            Self::EvidenceBeforeSynthesis => "I2:evidence-before-synthesis",
            Self::SlaUpgradeMonotonicity => "I3:SLA-upgrade-only",
            Self::SingleComplexityUpgrade => "I4:single-complexity-upgrade",
            Self::ReplanBudgetBounded => "I5:replan-budget-bounded",
            Self::RoundMonotonicity => "I6:round-monotonicity",
            Self::UtilityScoreBounded => "I7:utility-bounded",
            Self::CostNonNegative => "I8:cost-non-negative",
            Self::FsmTerminalConsistency => "I9:FSM-terminal-consistency",
            Self::DriftBounded => "I10:drift-bounded",
        }
    }
}

// ── ViolationSeverity ────────────────────────────────────────────────────────

/// Severity level for an invariant violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ViolationSeverity {
    /// Unexpected but recoverable — may indicate a transient race or edge case.
    Warning,
    /// Correctness breach — the system is operating outside its design envelope.
    Error,
    /// System integrity compromised — deterministic guarantees violated.
    Critical,
}

// ── InvariantViolation ───────────────────────────────────────────────────────

/// A detected invariant violation with context for diagnosis.
#[derive(Debug, Clone)]
pub struct InvariantViolation {
    /// Which invariant was violated.
    pub id: InvariantId,
    /// Human-readable description of the violation.
    pub message: String,
    /// Round at which the violation was detected.
    pub round: usize,
    /// Severity level.
    pub severity: ViolationSeverity,
}

// ── InvariantSnapshot ────────────────────────────────────────────────────────

/// Cross-boundary data transfer for invariant validation.
///
/// Constructed from `LoopState` at the agent boundary (`convergence_phase.rs`).
/// Contains only the signals needed for invariant checks — no reference to
/// LoopState or any infrastructure type.
#[derive(Debug, Clone)]
pub struct InvariantSnapshot {
    // ── EBS (I1, I2) ─────────────────────────────────────────────────────
    /// Whether `EvidenceBundle.synthesis_blocked` is true.
    pub synthesis_blocked: bool,
    /// Whether `synthesis_origin == Some(SupervisorFailure)`.
    pub synthesis_origin_is_supervisor: bool,
    /// Total text bytes extracted by content-reading tools.
    pub evidence_bytes: usize,
    /// Minimum evidence threshold from PolicyConfig.
    pub min_evidence_bytes: usize,
    /// Number of content-read tool invocations.
    pub content_read_attempts: usize,
    /// Whether this is a synthesis round (tools stripped).
    pub is_synthesis_round: bool,
    /// Whether forced synthesis was detected.
    pub forced_synthesis_detected: bool,

    // ── SLA (I3) ─────────────────────────────────────────────────────────
    /// Ordinal of current SLA mode: 0=Fast, 1=Balanced, 2=Deep.
    pub sla_mode_ordinal: u8,

    // ── Complexity (I4) ──────────────────────────────────────────────────
    /// Number of complexity upgrades performed this session.
    pub complexity_upgrade_count: u32,

    // ── Replan (I5) ──────────────────────────────────────────────────────
    /// Current replan attempt count.
    pub replan_attempts: u32,
    /// Max replans allowed by policy.
    pub max_replan_attempts: u32,

    // ── Round (I6) ───────────────────────────────────────────────────────
    /// Current round number (0-based).
    pub round: usize,

    // ── Utility (I7) ─────────────────────────────────────────────────────
    /// Convergence utility score from P3.6.
    pub utility_score: f64,

    // ── Cost (I8) ────────────────────────────────────────────────────────
    /// Accumulated cost in USD.
    pub accumulated_cost: f64,

    // ── FSM (I9) ─────────────────────────────────────────────────────────
    /// Current FSM phase label (from `AgentPhase.as_str()`).
    pub fsm_phase: &'static str,
    /// Whether tool suppression is active (ForceNoNext or ForcedByOracle).
    pub tool_suppression_active: bool,

    // ── Drift (I10) ──────────────────────────────────────────────────────
    /// Cumulative drift score across all rounds.
    pub cumulative_drift: f32,
    /// Maximum allowed cumulative drift (from PolicyConfig or default).
    pub max_drift_bound: f32,
}

// ── SystemInvariantChecker ───────────────────────────────────────────────────

/// Stateful checker that validates system invariants across rounds.
///
/// Maintains previous-round state for monotonicity properties (I3, I6).
/// Call [`check_round()`] once per round.
pub struct SystemInvariantChecker {
    /// All violations detected across the session.
    violations: Vec<InvariantViolation>,
    /// Previous SLA mode ordinal (for I3 monotonicity).
    prev_sla_ordinal: Option<u8>,
    /// Previous round number (for I6 monotonicity).
    prev_round: Option<usize>,
}

impl SystemInvariantChecker {
    /// Create a new checker with empty state.
    pub fn new() -> Self {
        Self {
            violations: Vec::new(),
            prev_sla_ordinal: None,
            prev_round: None,
        }
    }

    /// Validate all invariants against the current snapshot.
    ///
    /// Returns the number of violations detected in this round.
    pub fn check_round(&mut self, snap: &InvariantSnapshot) -> usize {
        let before = self.violations.len();

        self.check_ebs_consistency(snap);
        self.check_evidence_before_synthesis(snap);
        self.check_sla_monotonicity(snap);
        self.check_single_complexity_upgrade(snap);
        self.check_replan_budget(snap);
        self.check_round_monotonicity(snap);
        self.check_utility_bounded(snap);
        self.check_cost_non_negative(snap);
        self.check_fsm_terminal_consistency(snap);
        self.check_drift_bounded(snap);

        // Update stateful tracking for next round.
        self.prev_sla_ordinal = Some(snap.sla_mode_ordinal);
        self.prev_round = Some(snap.round);

        let new_violations = self.violations.len() - before;
        if new_violations > 0 {
            for v in &self.violations[before..] {
                match v.severity {
                    ViolationSeverity::Critical | ViolationSeverity::Error => {
                        tracing::error!(
                            invariant = v.id.label(),
                            round = v.round,
                            severity = ?v.severity,
                            "{}", v.message,
                        );
                    }
                    ViolationSeverity::Warning => {
                        tracing::warn!(
                            invariant = v.id.label(),
                            round = v.round,
                            "{}", v.message,
                        );
                    }
                }
            }
        }
        new_violations
    }

    /// All violations accumulated across the session.
    pub fn violations(&self) -> &[InvariantViolation] {
        &self.violations
    }

    /// Whether any critical violation was detected.
    pub fn has_critical(&self) -> bool {
        self.violations.iter().any(|v| v.severity == ViolationSeverity::Critical)
    }

    /// Count of violations at or above a given severity.
    pub fn count_at_severity(&self, min_severity: ViolationSeverity) -> usize {
        self.violations.iter().filter(|v| v.severity >= min_severity).count()
    }

    /// Reset the checker (useful for testing).
    pub fn reset(&mut self) {
        self.violations.clear();
        self.prev_sla_ordinal = None;
        self.prev_round = None;
    }

    // ── Individual invariant checks ──────────────────────────────────────

    /// I1: EBS Consistency — `synthesis_blocked ⇒ origin == SupervisorFailure`
    fn check_ebs_consistency(&mut self, snap: &InvariantSnapshot) {
        if snap.synthesis_blocked && !snap.synthesis_origin_is_supervisor {
            self.violations.push(InvariantViolation {
                id: InvariantId::EbsConsistency,
                message: format!(
                    "synthesis_blocked=true but origin is not SupervisorFailure (round {})",
                    snap.round,
                ),
                round: snap.round,
                severity: ViolationSeverity::Critical,
            });
        }
    }

    /// I2: Evidence Before Synthesis — non-failure synthesis requires evidence ≥ threshold.
    ///
    /// Only checks when: (a) synthesis round, (b) not a supervisor-failure origin,
    /// (c) content-reading tools were actually used.
    fn check_evidence_before_synthesis(&mut self, snap: &InvariantSnapshot) {
        if snap.is_synthesis_round
            && !snap.synthesis_origin_is_supervisor
            && snap.content_read_attempts > 0
            && snap.evidence_bytes < snap.min_evidence_bytes
        {
            self.violations.push(InvariantViolation {
                id: InvariantId::EvidenceBeforeSynthesis,
                message: format!(
                    "synthesis round with insufficient evidence: {} bytes < {} threshold (round {})",
                    snap.evidence_bytes, snap.min_evidence_bytes, snap.round,
                ),
                round: snap.round,
                severity: ViolationSeverity::Error,
            });
        }
    }

    /// I3: SLA Upgrade Monotonicity — SLA mode ordinal never decreases.
    fn check_sla_monotonicity(&mut self, snap: &InvariantSnapshot) {
        if let Some(prev) = self.prev_sla_ordinal {
            if snap.sla_mode_ordinal < prev {
                self.violations.push(InvariantViolation {
                    id: InvariantId::SlaUpgradeMonotonicity,
                    message: format!(
                        "SLA mode downgraded: {} → {} (round {})",
                        prev, snap.sla_mode_ordinal, snap.round,
                    ),
                    round: snap.round,
                    severity: ViolationSeverity::Error,
                });
            }
        }
    }

    /// I4: Single Complexity Upgrade — at most 1 upgrade per session.
    fn check_single_complexity_upgrade(&mut self, snap: &InvariantSnapshot) {
        if snap.complexity_upgrade_count > 1 {
            self.violations.push(InvariantViolation {
                id: InvariantId::SingleComplexityUpgrade,
                message: format!(
                    "multiple complexity upgrades detected: {} (round {})",
                    snap.complexity_upgrade_count, snap.round,
                ),
                round: snap.round,
                severity: ViolationSeverity::Error,
            });
        }
    }

    /// I5: Replan Budget Bounded — `replan_attempts ≤ max_replan_attempts + 1`.
    ///
    /// Tolerance of +1 for the check-then-increment pattern in convergence_phase.
    fn check_replan_budget(&mut self, snap: &InvariantSnapshot) {
        let budget_with_tolerance = snap.max_replan_attempts.saturating_add(1);
        if snap.replan_attempts > budget_with_tolerance {
            self.violations.push(InvariantViolation {
                id: InvariantId::ReplanBudgetBounded,
                message: format!(
                    "replan_attempts {} exceeds budget {} + 1 tolerance (round {})",
                    snap.replan_attempts, snap.max_replan_attempts, snap.round,
                ),
                round: snap.round,
                severity: ViolationSeverity::Warning,
            });
        }
    }

    /// I6: Round Counter Monotonicity — round never decreases.
    fn check_round_monotonicity(&mut self, snap: &InvariantSnapshot) {
        if let Some(prev) = self.prev_round {
            if snap.round < prev {
                self.violations.push(InvariantViolation {
                    id: InvariantId::RoundMonotonicity,
                    message: format!(
                        "round counter decreased: {} → {} (round {})",
                        prev, snap.round, snap.round,
                    ),
                    round: snap.round,
                    severity: ViolationSeverity::Critical,
                });
            }
        }
    }

    /// I7: Utility Score Bounded — `0.0 ≤ utility ≤ 1.5`.
    ///
    /// Upper bound of 1.5 allows for slight overshoot from weighted sums.
    fn check_utility_bounded(&mut self, snap: &InvariantSnapshot) {
        if snap.utility_score.is_nan() || snap.utility_score.is_infinite() {
            self.violations.push(InvariantViolation {
                id: InvariantId::UtilityScoreBounded,
                message: format!(
                    "utility_score is NaN/Inf: {} (round {})",
                    snap.utility_score, snap.round,
                ),
                round: snap.round,
                severity: ViolationSeverity::Critical,
            });
        } else if snap.utility_score < 0.0 || snap.utility_score > 1.5 {
            self.violations.push(InvariantViolation {
                id: InvariantId::UtilityScoreBounded,
                message: format!(
                    "utility_score out of range [0.0, 1.5]: {} (round {})",
                    snap.utility_score, snap.round,
                ),
                round: snap.round,
                severity: ViolationSeverity::Warning,
            });
        }
    }

    /// I8: Cost Non-Negative — accumulated cost must be ≥ 0.0.
    fn check_cost_non_negative(&mut self, snap: &InvariantSnapshot) {
        if snap.accumulated_cost < 0.0 || snap.accumulated_cost.is_nan() {
            self.violations.push(InvariantViolation {
                id: InvariantId::CostNonNegative,
                message: format!(
                    "accumulated_cost invalid: {} (round {})",
                    snap.accumulated_cost, snap.round,
                ),
                round: snap.round,
                severity: ViolationSeverity::Error,
            });
        }
    }

    /// I9: FSM Terminal Consistency — completed/halted phases should not have active tool suppression.
    ///
    /// When the FSM is in a terminal state, tool suppression signals are meaningless
    /// and indicate a state management bug.
    fn check_fsm_terminal_consistency(&mut self, snap: &InvariantSnapshot) {
        let is_terminal = snap.fsm_phase == "completed" || snap.fsm_phase == "halted";
        if is_terminal && snap.tool_suppression_active {
            self.violations.push(InvariantViolation {
                id: InvariantId::FsmTerminalConsistency,
                message: format!(
                    "terminal FSM phase '{}' has active tool suppression (round {})",
                    snap.fsm_phase, snap.round,
                ),
                round: snap.round,
                severity: ViolationSeverity::Warning,
            });
        }
    }

    /// I10: Drift Bounded — cumulative drift should not exceed configured maximum.
    fn check_drift_bounded(&mut self, snap: &InvariantSnapshot) {
        if snap.cumulative_drift > snap.max_drift_bound {
            self.violations.push(InvariantViolation {
                id: InvariantId::DriftBounded,
                message: format!(
                    "cumulative_drift {:.3} exceeds bound {:.3} (round {})",
                    snap.cumulative_drift, snap.max_drift_bound, snap.round,
                ),
                round: snap.round,
                severity: ViolationSeverity::Warning,
            });
        }
    }
}

impl Default for SystemInvariantChecker {
    fn default() -> Self {
        Self::new()
    }
}

// ── Convenience constructors ─────────────────────────────────────────────────

impl InvariantSnapshot {
    /// Create a healthy snapshot with all invariants satisfied.
    /// Useful as a baseline for tests.
    #[cfg(test)]
    fn healthy(round: usize) -> Self {
        Self {
            synthesis_blocked: false,
            synthesis_origin_is_supervisor: false,
            evidence_bytes: 100,
            min_evidence_bytes: 30,
            content_read_attempts: 1,
            is_synthesis_round: false,
            forced_synthesis_detected: false,
            sla_mode_ordinal: 1, // Balanced
            complexity_upgrade_count: 0,
            replan_attempts: 0,
            max_replan_attempts: 2,
            round,
            utility_score: 0.5,
            accumulated_cost: 0.001,
            fsm_phase: "executing",
            tool_suppression_active: false,
            cumulative_drift: 0.3,
            max_drift_bound: 5.0,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Healthy baseline ─────────────────────────────────────────────────

    #[test]
    fn phase4_invariants_healthy_snapshot_no_violations() {
        let mut checker = SystemInvariantChecker::new();
        let snap = InvariantSnapshot::healthy(0);
        let count = checker.check_round(&snap);
        assert_eq!(count, 0, "healthy snapshot must produce 0 violations");
        assert!(!checker.has_critical());
    }

    #[test]
    fn phase4_invariants_multiple_healthy_rounds_no_violations() {
        let mut checker = SystemInvariantChecker::new();
        for r in 0..5 {
            let snap = InvariantSnapshot::healthy(r);
            let count = checker.check_round(&snap);
            assert_eq!(count, 0, "round {r} should have 0 violations");
        }
        assert!(checker.violations().is_empty());
    }

    // ── I1: EBS Consistency ──────────────────────────────────────────────

    #[test]
    fn phase4_i1_ebs_blocked_without_supervisor_origin() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(1);
        snap.synthesis_blocked = true;
        snap.synthesis_origin_is_supervisor = false;
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].id, InvariantId::EbsConsistency);
        assert_eq!(checker.violations()[0].severity, ViolationSeverity::Critical);
    }

    #[test]
    fn phase4_i1_ebs_blocked_with_supervisor_origin_ok() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(1);
        snap.synthesis_blocked = true;
        snap.synthesis_origin_is_supervisor = true;
        let count = checker.check_round(&snap);
        assert_eq!(count, 0, "synthesis_blocked + supervisor origin should be valid");
    }

    // ── I2: Evidence Before Synthesis ────────────────────────────────────

    #[test]
    fn phase4_i2_synthesis_without_evidence() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(2);
        snap.is_synthesis_round = true;
        snap.content_read_attempts = 3;
        snap.evidence_bytes = 10; // below 30 threshold
        snap.synthesis_origin_is_supervisor = false;
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].id, InvariantId::EvidenceBeforeSynthesis);
        assert_eq!(checker.violations()[0].severity, ViolationSeverity::Error);
    }

    #[test]
    fn phase4_i2_synthesis_with_sufficient_evidence_ok() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(2);
        snap.is_synthesis_round = true;
        snap.content_read_attempts = 3;
        snap.evidence_bytes = 500;
        snap.synthesis_origin_is_supervisor = false;
        let count = checker.check_round(&snap);
        assert_eq!(count, 0);
    }

    #[test]
    fn phase4_i2_synthesis_supervisor_failure_bypasses_check() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(2);
        snap.is_synthesis_round = true;
        snap.content_read_attempts = 3;
        snap.evidence_bytes = 0; // no evidence
        snap.synthesis_origin_is_supervisor = true; // supervisor failure → bypass
        let count = checker.check_round(&snap);
        assert_eq!(count, 0, "supervisor failure origin should bypass evidence check");
    }

    // ── I3: SLA Upgrade Monotonicity ─────────────────────────────────────

    #[test]
    fn phase4_i3_sla_downgrade_detected() {
        let mut checker = SystemInvariantChecker::new();
        // Round 0: Balanced (1)
        let mut snap = InvariantSnapshot::healthy(0);
        snap.sla_mode_ordinal = 1;
        checker.check_round(&snap);
        // Round 1: Fast (0) — downgrade!
        let mut snap = InvariantSnapshot::healthy(1);
        snap.sla_mode_ordinal = 0;
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].id, InvariantId::SlaUpgradeMonotonicity);
    }

    #[test]
    fn phase4_i3_sla_upgrade_ok() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(0);
        snap.sla_mode_ordinal = 0; // Fast
        checker.check_round(&snap);
        let mut snap = InvariantSnapshot::healthy(1);
        snap.sla_mode_ordinal = 2; // Deep — valid upgrade
        let count = checker.check_round(&snap);
        assert_eq!(count, 0);
    }

    #[test]
    fn phase4_i3_sla_same_mode_ok() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(0);
        snap.sla_mode_ordinal = 1;
        checker.check_round(&snap);
        let mut snap = InvariantSnapshot::healthy(1);
        snap.sla_mode_ordinal = 1; // same — ok
        let count = checker.check_round(&snap);
        assert_eq!(count, 0);
    }

    // ── I4: Single Complexity Upgrade ────────────────────────────────────

    #[test]
    fn phase4_i4_multiple_complexity_upgrades_detected() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(3);
        snap.complexity_upgrade_count = 2;
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].id, InvariantId::SingleComplexityUpgrade);
    }

    #[test]
    fn phase4_i4_single_complexity_upgrade_ok() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(3);
        snap.complexity_upgrade_count = 1;
        let count = checker.check_round(&snap);
        assert_eq!(count, 0);
    }

    // ── I5: Replan Budget Bounded ────────────────────────────────────────

    #[test]
    fn phase4_i5_replan_exceeds_budget() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(5);
        snap.replan_attempts = 4;
        snap.max_replan_attempts = 2; // budget + 1 tolerance = 3, 4 > 3
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].id, InvariantId::ReplanBudgetBounded);
        assert_eq!(checker.violations()[0].severity, ViolationSeverity::Warning);
    }

    #[test]
    fn phase4_i5_replan_within_tolerance_ok() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(5);
        snap.replan_attempts = 3;
        snap.max_replan_attempts = 2; // budget + 1 = 3, 3 ≤ 3
        let count = checker.check_round(&snap);
        assert_eq!(count, 0);
    }

    // ── I6: Round Counter Monotonicity ───────────────────────────────────

    #[test]
    fn phase4_i6_round_decrease_detected() {
        let mut checker = SystemInvariantChecker::new();
        checker.check_round(&InvariantSnapshot::healthy(5));
        let snap = InvariantSnapshot::healthy(3); // decrease!
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].id, InvariantId::RoundMonotonicity);
        assert_eq!(checker.violations()[0].severity, ViolationSeverity::Critical);
    }

    #[test]
    fn phase4_i6_round_same_value_ok() {
        let mut checker = SystemInvariantChecker::new();
        checker.check_round(&InvariantSnapshot::healthy(3));
        let count = checker.check_round(&InvariantSnapshot::healthy(3));
        // Same round is valid (can happen on re-checks within same round).
        assert_eq!(count, 0);
    }

    // ── I7: Utility Score Bounded ────────────────────────────────────────

    #[test]
    fn phase4_i7_utility_nan_critical() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(0);
        snap.utility_score = f64::NAN;
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].id, InvariantId::UtilityScoreBounded);
        assert_eq!(checker.violations()[0].severity, ViolationSeverity::Critical);
    }

    #[test]
    fn phase4_i7_utility_negative_warning() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(0);
        snap.utility_score = -0.1;
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].severity, ViolationSeverity::Warning);
    }

    #[test]
    fn phase4_i7_utility_above_1_5_warning() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(0);
        snap.utility_score = 1.6;
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].severity, ViolationSeverity::Warning);
    }

    #[test]
    fn phase4_i7_utility_at_boundary_ok() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(0);
        snap.utility_score = 0.0;
        assert_eq!(checker.check_round(&snap), 0);
        checker.reset();
        snap.utility_score = 1.5;
        assert_eq!(checker.check_round(&snap), 0);
    }

    // ── I8: Cost Non-Negative ────────────────────────────────────────────

    #[test]
    fn phase4_i8_negative_cost_detected() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(0);
        snap.accumulated_cost = -0.001;
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].id, InvariantId::CostNonNegative);
    }

    #[test]
    fn phase4_i8_nan_cost_detected() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(0);
        snap.accumulated_cost = f64::NAN;
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].id, InvariantId::CostNonNegative);
    }

    // ── I9: FSM Terminal Consistency ─────────────────────────────────────

    #[test]
    fn phase4_i9_completed_with_tool_suppression() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(10);
        snap.fsm_phase = "completed";
        snap.tool_suppression_active = true;
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].id, InvariantId::FsmTerminalConsistency);
    }

    #[test]
    fn phase4_i9_halted_with_tool_suppression() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(10);
        snap.fsm_phase = "halted";
        snap.tool_suppression_active = true;
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
    }

    #[test]
    fn phase4_i9_executing_with_tool_suppression_ok() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(3);
        snap.fsm_phase = "executing";
        snap.tool_suppression_active = true;
        // Not a terminal phase → no violation
        let count = checker.check_round(&snap);
        assert_eq!(count, 0);
    }

    // ── I10: Drift Bounded ───────────────────────────────────────────────

    #[test]
    fn phase4_i10_drift_exceeds_bound() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(5);
        snap.cumulative_drift = 6.0;
        snap.max_drift_bound = 5.0;
        let count = checker.check_round(&snap);
        assert_eq!(count, 1);
        assert_eq!(checker.violations()[0].id, InvariantId::DriftBounded);
    }

    #[test]
    fn phase4_i10_drift_at_bound_ok() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(5);
        snap.cumulative_drift = 5.0;
        snap.max_drift_bound = 5.0;
        let count = checker.check_round(&snap);
        assert_eq!(count, 0);
    }

    // ── Multi-violation detection ────────────────────────────────────────

    #[test]
    fn phase4_invariants_multiple_violations_in_one_round() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(2);
        // Break I1: synthesis_blocked without supervisor origin
        snap.synthesis_blocked = true;
        snap.synthesis_origin_is_supervisor = false;
        // Break I7: NaN utility
        snap.utility_score = f64::NAN;
        // Break I8: negative cost
        snap.accumulated_cost = -1.0;
        let count = checker.check_round(&snap);
        assert_eq!(count, 3, "should detect 3 violations in one round");
        assert!(checker.has_critical());
    }

    // ── Severity queries ─────────────────────────────────────────────────

    #[test]
    fn phase4_invariants_count_at_severity() {
        let mut checker = SystemInvariantChecker::new();
        // Critical: NaN utility
        let mut snap = InvariantSnapshot::healthy(0);
        snap.utility_score = f64::NAN;
        checker.check_round(&snap);
        // Warning: drift exceeded
        let mut snap = InvariantSnapshot::healthy(1);
        snap.cumulative_drift = 10.0;
        snap.max_drift_bound = 5.0;
        checker.check_round(&snap);

        assert_eq!(checker.count_at_severity(ViolationSeverity::Critical), 1);
        assert_eq!(checker.count_at_severity(ViolationSeverity::Warning), 2); // both
        assert_eq!(checker.count_at_severity(ViolationSeverity::Error), 1); // critical only
    }

    // ── Stateful tracking ────────────────────────────────────────────────

    #[test]
    fn phase4_invariants_reset_clears_state() {
        let mut checker = SystemInvariantChecker::new();
        let mut snap = InvariantSnapshot::healthy(0);
        snap.utility_score = f64::NAN;
        checker.check_round(&snap);
        assert_eq!(checker.violations().len(), 1);

        checker.reset();
        assert!(checker.violations().is_empty());
        assert!(!checker.has_critical());
        // prev_round and prev_sla_ordinal should also be reset
        // (verified indirectly: round 0 after reset should not trigger I6)
        let snap = InvariantSnapshot::healthy(0);
        let count = checker.check_round(&snap);
        assert_eq!(count, 0);
    }

    // ── Label coverage ───────────────────────────────────────────────────

    #[test]
    fn phase4_invariants_all_labels_unique() {
        let ids = [
            InvariantId::EbsConsistency,
            InvariantId::EvidenceBeforeSynthesis,
            InvariantId::SlaUpgradeMonotonicity,
            InvariantId::SingleComplexityUpgrade,
            InvariantId::ReplanBudgetBounded,
            InvariantId::RoundMonotonicity,
            InvariantId::UtilityScoreBounded,
            InvariantId::CostNonNegative,
            InvariantId::FsmTerminalConsistency,
            InvariantId::DriftBounded,
        ];
        let labels: Vec<&str> = ids.iter().map(|id| id.label()).collect();
        let unique: std::collections::HashSet<&str> = labels.iter().copied().collect();
        assert_eq!(labels.len(), unique.len(), "all invariant labels must be unique");
        assert_eq!(labels.len(), 10, "must have exactly 10 invariants");
    }

    #[test]
    fn phase4_invariants_severity_ordering() {
        assert!(ViolationSeverity::Warning < ViolationSeverity::Error);
        assert!(ViolationSeverity::Error < ViolationSeverity::Critical);
    }
}
