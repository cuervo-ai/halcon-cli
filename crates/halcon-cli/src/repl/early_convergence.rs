//! Early Convergence Detector — triggers synthesis before token budget exhaustion.
//!
//! ## Problem
//!
//! The current agent loop explores until one of these fires:
//! - Token budget exhausted (→ truncation, bad UX)
//! - Max rounds reached (→ MaxRounds, incomplete answer)
//! - Loop guard oscillation detection (→ ForcedSynthesis)
//!
//! By the time these fire, substantial tokens have been wasted on exploration
//! that added marginal value.
//!
//! ## Solution
//!
//! This module implements **early convergence**: proactive synthesis when the
//! system has gathered enough evidence to answer well, *before* budget exhaustion.
//!
//! Two complementary signals drive early convergence:
//!
//! ### Signal 1: Evidence Threshold (80% Plan Completion)
//! When ≥80% of planned tool steps complete successfully, the remaining 20% is
//! unlikely to change the answer materially. Synthesise immediately.
//!
//! ### Signal 2: Token Headroom Check
//! If remaining token budget < estimated synthesis cost, synthesise now — even
//! if plan is not yet complete — to avoid truncated output.
//!
//! ### Signal 3: Diminishing Returns (trajectory scoring)
//! When the per-round progress delta has been below `MIN_PROGRESS_DELTA` for
//! `DIMINISHING_WINDOW` consecutive rounds, further exploration is unlikely to
//! yield new information.
//!
//! ## Tuning
//!
//! All thresholds are constants and can be adjusted based on empirical data:
//! - `EVIDENCE_THRESHOLD`: 0.80 (80% plan completion → synthesise)
//! - `MIN_SYNTHESIS_HEADROOM`: 4000 tokens (absolute floor for headroom)
//! - `MIN_PROGRESS_DELTA`: 0.05 (minimum meaningful progress per round)
//! - `DIMINISHING_WINDOW`: 2 (rounds of sub-threshold progress → give up)
//!
//! Use [`ConvergenceDetector::with_context_window`] to calibrate headroom to the
//! provider's actual context window (8% of window, clamped to [4000, 20000]).

// ── Constants ──────────────────────────────────────────────────────────────

/// Fraction of plan steps that must complete before early synthesis fires.
/// 0.80 = 80% — leaves 20% tolerance for optional or retry-able steps.
pub const EVIDENCE_THRESHOLD: f32 = 0.80;

/// Minimum token headroom required to produce a complete synthesis.
/// This is the absolute floor — the actual threshold used at runtime is
/// either this constant (via `new()`) or a context-window-scaled value
/// (via `with_context_window()`).
/// Conservative estimate: synthesis averages ~800-1200 tokens.
pub const MIN_SYNTHESIS_HEADROOM: u64 = 4_000;

/// Maximum token headroom — caps `with_context_window` scaling at this value.
/// Prevents absurdly large headroom on 200K+ windows.
pub const MAX_SYNTHESIS_HEADROOM: u64 = 20_000;

/// Minimum meaningful per-round progress delta.
/// Rounds below this are considered stagnant for diminishing-returns detection.
pub const MIN_PROGRESS_DELTA: f32 = 0.05;

/// Number of consecutive sub-threshold rounds before diminishing returns fires.
pub const DIMINISHING_WINDOW: usize = 2;

// ── Convergence Signal ─────────────────────────────────────────────────────

/// The reason early convergence was triggered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvergenceReason {
    /// ≥80% of plan steps completed — evidence threshold met.
    EvidenceThreshold,
    /// Token budget too low to continue safely — synthesis now.
    TokenHeadroom,
    /// Per-round progress has been below MIN_PROGRESS_DELTA for DIMINISHING_WINDOW rounds.
    DiminishingReturns,
}

impl ConvergenceReason {
    /// Human-readable explanation for logging / user display.
    pub fn description(&self) -> &'static str {
        match self {
            Self::EvidenceThreshold => "sufficient evidence gathered (≥80% plan complete)",
            Self::TokenHeadroom => "token budget low — synthesising to avoid truncation",
            Self::DiminishingReturns => "diminishing returns — no meaningful progress in recent rounds",
        }
    }
}

// ── ConvergenceDetector ────────────────────────────────────────────────────

/// Stateful detector that tracks per-round signals across the agent loop.
///
/// Create once per agent loop invocation, then call `check()` after each round.
///
/// # Constructors
///
/// - [`new()`](Self::new): Uses `MIN_SYNTHESIS_HEADROOM` (4000 tokens) as headroom floor.
/// - [`with_context_window()`](Self::with_context_window): Calibrates headroom to 8% of the
///   provider's actual context window, clamped to [4000, 20000]. Preferred in production.
#[derive(Debug)]
pub struct ConvergenceDetector {
    /// Number of consecutive rounds where progress_delta < MIN_PROGRESS_DELTA.
    consecutive_stagnant: usize,
    /// Most recent plan completion ratio observed.
    last_completion_ratio: f32,
    /// Whether convergence has already been triggered (prevents repeated firing).
    fired: bool,
    /// Token headroom threshold for Signal 2 (TokenHeadroom).
    /// Set to MIN_SYNTHESIS_HEADROOM by `new()` or computed from context_window
    /// by `with_context_window()`. Never less than MIN_SYNTHESIS_HEADROOM.
    synthesis_headroom: u64,
    /// Plan completion fraction required for early convergence (default: EVIDENCE_THRESHOLD = 0.80).
    /// Overridden by PolicyConfig.early_convergence_threshold via `with_policy_context_window()`.
    evidence_threshold: f32,
}

impl ConvergenceDetector {
    /// Create a new detector with default headroom (MIN_SYNTHESIS_HEADROOM = 4000 tokens).
    ///
    /// Prefer [`with_context_window`](Self::with_context_window) in production so the
    /// headroom scales with the provider's actual context window.
    pub fn new() -> Self {
        Self {
            consecutive_stagnant: 0,
            last_completion_ratio: 0.0,
            fired: false,
            synthesis_headroom: MIN_SYNTHESIS_HEADROOM,
            evidence_threshold: EVIDENCE_THRESHOLD,
        }
    }

    /// Create a detector calibrated to the provider's context window.
    ///
    /// Computes `headroom = (context_window * 0.08).clamp(MIN_SYNTHESIS_HEADROOM, MAX_SYNTHESIS_HEADROOM)`:
    /// - 32K window → 2560 → clamped to 4000
    /// - 64K window → 5120
    /// - 128K window → 10240
    /// - 200K window → 16000
    ///
    /// This prevents both false positives (headroom too large → fires too early on large windows)
    /// and false negatives (headroom too small → allows truncation on large-context providers).
    pub fn with_context_window(context_window: u64) -> Self {
        let headroom = ((context_window as f64 * 0.08) as u64)
            .clamp(MIN_SYNTHESIS_HEADROOM, MAX_SYNTHESIS_HEADROOM);
        Self {
            consecutive_stagnant: 0,
            last_completion_ratio: 0.0,
            fired: false,
            synthesis_headroom: headroom,
            evidence_threshold: EVIDENCE_THRESHOLD,
        }
    }

    /// Create a detector calibrated to the context window with policy-driven evidence threshold.
    pub fn with_policy_context_window(
        context_window: u64,
        policy: &halcon_core::types::PolicyConfig,
    ) -> Self {
        let mut det = Self::with_context_window(context_window);
        det.evidence_threshold = policy.early_convergence_threshold;
        det
    }

    /// Returns the effective synthesis headroom in use.
    pub fn synthesis_headroom(&self) -> u64 {
        self.synthesis_headroom
    }

    /// Check convergence after a round completes.
    ///
    /// # Arguments
    ///
    /// - `plan_completion_ratio`: Fraction of plan steps in a terminal state [0, 1].
    ///   Pass `0.0` when no plan is active.
    /// - `tokens_remaining`: Estimated remaining token budget for this session.
    /// - `progress_delta`: Change in plan completion this round (can be 0.0 or negative).
    ///   Pass `0.0` on the first round.
    ///
    /// # Returns
    ///
    /// `Some(reason)` if early convergence should be triggered, `None` to continue.
    ///
    /// Once triggered, subsequent calls always return `None` (prevent double-fire).
    pub fn check(
        &mut self,
        plan_completion_ratio: f32,
        tokens_remaining: u64,
        progress_delta: f32,
    ) -> Option<ConvergenceReason> {
        if self.fired {
            return None;
        }

        // Update stagnation counter.
        if progress_delta.abs() < MIN_PROGRESS_DELTA {
            self.consecutive_stagnant += 1;
        } else {
            self.consecutive_stagnant = 0;
        }
        self.last_completion_ratio = plan_completion_ratio;

        // Signal 2: Token headroom (highest priority — prevents truncation).
        // Uses self.synthesis_headroom (calibrated to context window) not the bare constant.
        if tokens_remaining > 0 && tokens_remaining < self.synthesis_headroom {
            self.fired = true;
            return Some(ConvergenceReason::TokenHeadroom);
        }

        // Signal 1: Evidence threshold (from PolicyConfig.early_convergence_threshold).
        if plan_completion_ratio > 0.0 && plan_completion_ratio >= self.evidence_threshold {
            self.fired = true;
            return Some(ConvergenceReason::EvidenceThreshold);
        }

        // Signal 3: Diminishing returns.
        // BUG-H3 FIX: Guard on plan_completion_ratio > 0.0 — without an active plan,
        // stagnation simply means the model is in conversational mode (no exploration
        // cycle). Triggering DiminishingReturns without a plan forces premature synthesis
        // on legitimate multi-turn dialogues that have no tool phases.
        if plan_completion_ratio > 0.0 && self.consecutive_stagnant >= DIMINISHING_WINDOW {
            self.fired = true;
            return Some(ConvergenceReason::DiminishingReturns);
        }

        None
    }

    /// Returns true if convergence has already been triggered.
    pub fn has_fired(&self) -> bool {
        self.fired
    }

    /// Returns the last observed plan completion ratio.
    pub fn last_completion_ratio(&self) -> f32 {
        self.last_completion_ratio
    }

    /// Returns the current consecutive stagnant round count.
    pub fn consecutive_stagnant(&self) -> usize {
        self.consecutive_stagnant
    }

    /// Reset the detector (e.g. after a successful replan resets progress).
    pub fn reset_stagnation(&mut self) {
        self.consecutive_stagnant = 0;
    }

    /// Like [`check`](Self::check) but uses `estimate_synthesis_cost(current_input_tokens)`
    /// to compute a dynamic headroom floor for Signal 2 (TokenHeadroom).
    ///
    /// The effective headroom is `max(self.synthesis_headroom, estimate_synthesis_cost(input_tokens))`.
    /// This means the TokenHeadroom signal never fires earlier than the cost of producing
    /// one more synthesis response — avoiding over-eager truncation protection.
    ///
    /// # BUG-L2 FIX
    /// `estimate_synthesis_cost()` was previously public but orphaned (never called).
    /// This method wires it into the check pipeline, making cost awareness opt-in
    /// for callers that have accurate token-usage data.
    pub fn check_with_cost(
        &mut self,
        plan_completion_ratio: f32,
        tokens_remaining: u64,
        progress_delta: f32,
        current_input_tokens: u64,
    ) -> Option<ConvergenceReason> {
        // Compute cost-aware headroom (never less than the constructor-set value).
        let cost_estimate = estimate_synthesis_cost(current_input_tokens);
        let effective_headroom = self.synthesis_headroom.max(cost_estimate);

        // Temporarily apply cost-aware headroom for this check.
        let saved = self.synthesis_headroom;
        self.synthesis_headroom = effective_headroom;
        let result = self.check(plan_completion_ratio, tokens_remaining, progress_delta);
        // Restore original headroom (only headroom changes; `fired` correctly reflects the outcome).
        self.synthesis_headroom = saved;

        result
    }
}

impl Default for ConvergenceDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ── Token Budget Estimator ─────────────────────────────────────────────────

/// Estimate tokens remaining given current usage and context window.
///
/// This is a coarse estimate used for the headroom check. Precision is not
/// critical — we just need to know if we're dangerously low.
pub fn estimate_remaining_tokens(
    context_window: u64,
    input_tokens_used: u64,
    output_tokens_used: u64,
    reserved_output: u64,
) -> u64 {
    let total_used = input_tokens_used + output_tokens_used;
    let available = context_window.saturating_sub(reserved_output);
    available.saturating_sub(total_used)
}

/// Estimate the minimum tokens needed for a synthesis response.
///
/// Synthesis = model reads context + produces answer. Context size dominates.
/// Conservative estimate: 1.5× current input tokens for synthesis context +
/// 800 tokens for the answer itself.
pub fn estimate_synthesis_cost(current_input_tokens: u64) -> u64 {
    // Synthesis needs the accumulated conversation as context plus answer tokens.
    // We use a conservative multiplier to account for context growth.
    let context_cost = (current_input_tokens as f64 * 0.3) as u64; // 30% overhead for context
    let answer_cost: u64 = 800; // typical synthesis answer
    context_cost + answer_cost
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_threshold_fires_at_80_percent() {
        let mut det = ConvergenceDetector::new();
        let result = det.check(0.80, 50_000, 0.10);
        assert_eq!(result, Some(ConvergenceReason::EvidenceThreshold));
    }

    #[test]
    fn evidence_threshold_does_not_fire_at_79_percent() {
        let mut det = ConvergenceDetector::new();
        let result = det.check(0.79, 50_000, 0.10);
        assert_eq!(result, None);
    }

    #[test]
    fn evidence_threshold_does_not_fire_without_plan() {
        // plan_completion_ratio = 0.0 means no plan → threshold should not fire.
        let mut det = ConvergenceDetector::new();
        let result = det.check(0.0, 50_000, 0.10);
        assert_eq!(result, None);
    }

    #[test]
    fn token_headroom_fires_below_threshold() {
        let mut det = ConvergenceDetector::new();
        let result = det.check(0.50, MIN_SYNTHESIS_HEADROOM - 1, 0.10);
        assert_eq!(result, Some(ConvergenceReason::TokenHeadroom));
    }

    #[test]
    fn token_headroom_does_not_fire_at_threshold() {
        let mut det = ConvergenceDetector::new();
        let result = det.check(0.50, MIN_SYNTHESIS_HEADROOM, 0.10);
        assert_eq!(result, None);
    }

    #[test]
    fn token_headroom_takes_priority_over_evidence() {
        // Both signals fire — token headroom is checked first.
        let mut det = ConvergenceDetector::new();
        let result = det.check(0.90, MIN_SYNTHESIS_HEADROOM - 1, 0.10);
        assert_eq!(result, Some(ConvergenceReason::TokenHeadroom));
    }

    #[test]
    fn diminishing_returns_fires_after_window() {
        let mut det = ConvergenceDetector::new();
        // Round 0: stagnant (0.01 < MIN_PROGRESS_DELTA=0.05) → consecutive=1, not yet ≥ WINDOW=2.
        assert_eq!(det.check(0.10, 50_000, 0.01), None);
        // Round 1: stagnant → consecutive=2 ≥ WINDOW=2 → fires.
        assert_eq!(
            det.check(0.10, 50_000, 0.01),
            Some(ConvergenceReason::DiminishingReturns)
        );
    }

    #[test]
    fn diminishing_returns_resets_on_progress() {
        let mut det = ConvergenceDetector::new();
        // One stagnant round.
        assert_eq!(det.check(0.10, 50_000, 0.01), None);
        // Progress round — resets counter.
        assert_eq!(det.check(0.30, 50_000, 0.20), None); // 0.20 > MIN_PROGRESS_DELTA
        // One stagnant round again (counter was reset, so need WINDOW more).
        assert_eq!(det.check(0.30, 50_000, 0.01), None);
        // Second stagnant round — fires.
        assert_eq!(
            det.check(0.30, 50_000, 0.01),
            Some(ConvergenceReason::DiminishingReturns)
        );
    }

    #[test]
    fn fired_flag_prevents_double_trigger() {
        let mut det = ConvergenceDetector::new();
        // First fire.
        det.check(0.85, 50_000, 0.10);
        assert!(det.has_fired());
        // Subsequent calls return None even with triggering conditions.
        let result = det.check(1.0, 100, 0.0);
        assert_eq!(result, None);
    }

    #[test]
    fn no_fire_for_healthy_in_progress_session() {
        let mut det = ConvergenceDetector::new();
        // Simulating rounds with steady progress and healthy budget.
        assert_eq!(det.check(0.20, 40_000, 0.20), None);
        assert_eq!(det.check(0.40, 38_000, 0.20), None);
        assert_eq!(det.check(0.60, 35_000, 0.20), None);
        assert!(!det.has_fired());
    }

    #[test]
    fn zero_tokens_remaining_doesnt_fire_headroom() {
        // tokens_remaining = 0 means no budget tracking → skip headroom check.
        let mut det = ConvergenceDetector::new();
        let result = det.check(0.50, 0, 0.10); // 0 = no tracking
        assert_eq!(result, None);
    }

    #[test]
    fn reset_stagnation_clears_counter() {
        let mut det = ConvergenceDetector::new();
        det.check(0.10, 50_000, 0.01); // stagnant
        assert_eq!(det.consecutive_stagnant(), 1);
        det.reset_stagnation();
        assert_eq!(det.consecutive_stagnant(), 0);
    }

    #[test]
    fn convergence_reason_descriptions_non_empty() {
        assert!(!ConvergenceReason::EvidenceThreshold.description().is_empty());
        assert!(!ConvergenceReason::TokenHeadroom.description().is_empty());
        assert!(!ConvergenceReason::DiminishingReturns.description().is_empty());
    }

    #[test]
    fn estimate_remaining_tokens_basic() {
        let remaining = estimate_remaining_tokens(64_000, 20_000, 5_000, 4_000);
        // available = 64000 - 4000 = 60000; used = 25000; remaining = 35000
        assert_eq!(remaining, 35_000);
    }

    #[test]
    fn estimate_remaining_tokens_saturates_at_zero() {
        let remaining = estimate_remaining_tokens(10_000, 8_000, 5_000, 1_000);
        // used = 13000 > available = 9000 → saturate to 0
        assert_eq!(remaining, 0);
    }

    #[test]
    fn estimate_synthesis_cost_reasonable() {
        let cost = estimate_synthesis_cost(10_000);
        assert!(cost >= 800, "Should include answer cost");
        assert!(cost <= 10_000, "Should not exceed input tokens");
    }

    // ── BUG-H3 regression: DiminishingReturns must NOT fire without an active plan ──

    #[test]
    fn diminishing_returns_does_not_fire_without_active_plan() {
        // BUG-H3 regression: plan_completion_ratio=0.0 means no plan is active.
        // Stagnant rounds during a conversational exchange must NOT trigger DiminishingReturns.
        let mut det = ConvergenceDetector::new();
        // 3 rounds of stagnation, but no plan active (ratio=0.0).
        assert_eq!(det.check(0.0, 50_000, 0.01), None);
        assert_eq!(det.check(0.0, 50_000, 0.01), None);
        assert_eq!(det.check(0.0, 50_000, 0.01), None);
        assert!(!det.has_fired(), "Should not fire without an active plan");
    }

    // ── BUG-H4: with_context_window calibrates headroom ───────────────────

    #[test]
    fn with_context_window_scales_headroom_for_64k_window() {
        // 64K * 0.08 = 5120 — above MIN, below MAX
        let det = ConvergenceDetector::with_context_window(64_000);
        assert_eq!(det.synthesis_headroom(), 5_120);
    }

    #[test]
    fn with_context_window_clamps_headroom_at_minimum_for_small_window() {
        // 32K * 0.08 = 2560 — below MIN_SYNTHESIS_HEADROOM=4000, must clamp up
        let det = ConvergenceDetector::with_context_window(32_000);
        assert_eq!(det.synthesis_headroom(), MIN_SYNTHESIS_HEADROOM);
    }

    #[test]
    fn with_context_window_clamps_headroom_at_maximum_for_huge_window() {
        // 300K * 0.08 = 24000 — above MAX_SYNTHESIS_HEADROOM=20000, must clamp down
        let det = ConvergenceDetector::with_context_window(300_000);
        assert_eq!(det.synthesis_headroom(), MAX_SYNTHESIS_HEADROOM);
    }

    #[test]
    fn with_context_window_fires_at_calibrated_headroom() {
        // For a 64K window, headroom = 5120.
        // check() must fire TokenHeadroom when tokens_remaining < 5120.
        let mut det = ConvergenceDetector::with_context_window(64_000);
        // Just above calibrated headroom → no fire.
        assert_eq!(det.check(0.5, 5_120, 0.10), None);
        // Just below calibrated headroom → fires.
        // (Need a fresh detector since we used plan_completion_ratio=0.5 which
        // is below EVIDENCE_THRESHOLD and doesn't fire EvidenceThreshold.)
        let mut det2 = ConvergenceDetector::with_context_window(64_000);
        let result = det2.check(0.5, 5_119, 0.10);
        assert_eq!(result, Some(ConvergenceReason::TokenHeadroom));
    }

    // ── BUG-L2: check_with_cost wires estimate_synthesis_cost ─────────────

    #[test]
    fn check_with_cost_uses_cost_aware_headroom() {
        // With current_input_tokens=10_000:
        // cost = (10_000 * 0.3) as u64 + 800 = 3000 + 800 = 3800.
        // MIN_SYNTHESIS_HEADROOM = 4000 > 3800 → effective_headroom = max(4000, 3800) = 4000.
        // So check_with_cost should behave like check() with headroom=4000 in this case.
        let mut det = ConvergenceDetector::new();
        let result = det.check_with_cost(0.5, MIN_SYNTHESIS_HEADROOM - 1, 0.10, 10_000);
        assert_eq!(result, Some(ConvergenceReason::TokenHeadroom));
    }

    #[test]
    fn check_with_cost_raises_headroom_when_cost_exceeds_floor() {
        // With current_input_tokens=50_000:
        // cost = (50_000 * 0.3) as u64 + 800 = 15_000 + 800 = 15_800.
        // effective_headroom = max(MIN=4000, 15_800) = 15_800.
        // tokens_remaining=10_000 < 15_800 → TokenHeadroom fires.
        let mut det = ConvergenceDetector::new();
        let result = det.check_with_cost(0.5, 10_000, 0.10, 50_000);
        assert_eq!(result, Some(ConvergenceReason::TokenHeadroom));
    }

    #[test]
    fn check_with_cost_does_not_modify_synthesis_headroom_permanently() {
        // synthesis_headroom must be restored after check_with_cost().
        let mut det = ConvergenceDetector::new();
        assert_eq!(det.synthesis_headroom(), MIN_SYNTHESIS_HEADROOM);
        // Call check_with_cost with large input (would raise headroom temporarily).
        let _ = det.check_with_cost(0.5, 50_000, 0.10, 100_000);
        // Headroom must be back to MIN_SYNTHESIS_HEADROOM after the call.
        assert_eq!(det.synthesis_headroom(), MIN_SYNTHESIS_HEADROOM);
    }
}
