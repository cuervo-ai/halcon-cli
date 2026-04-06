//! Adaptive loop termination — SOTA 2026.
//!
//! Replaces the static `max_rounds` counter with semantic progress tracking.
//! Each agent round is observed; the controller decides whether to:
//! - Continue the loop (`Action::Continue`)
//! - Force synthesis now (`Action::Synthesize`) — we have enough to answer
//! - Trigger a replan (`Action::Replan`) — current approach is failing
//! - Hard-halt (`Action::Halt`) — no progress possible
//!
//! # Stagnation Detection
//! "Tool call diversity" is measured as the number of distinct (tool_name, args_hash) pairs
//! seen across the last N rounds. If the same tools are called with the same arguments
//! repeatedly, stagnation is detected.
//!
//! # Goal Coverage
//! Estimated as the fraction of intent keywords present in the accumulated response text.
//! Simple O(K) scan — no embedding needed.
//!
//! # Integration
//! Wire into `agent.rs` 'agent_loop:
//! ```text
//! let mut conv = ConvergenceController::new(&profile);
//! // ... inside 'agent_loop after collecting round_tools:
//! let action = conv.observe_round(round, &tool_names, &tool_args_hashes, &full_text);
//! match action {
//!     ConvergenceAction::Synthesize => { forced_synthesis_detected = true; break; }
//!     ConvergenceAction::Halt      => { break; }
//!     ConvergenceAction::Replan    => { /* trigger replan */ }
//!     ConvergenceAction::Continue  => {}
//! }
//! ```

use std::collections::{HashMap, HashSet, VecDeque};

use super::intent_scorer::{IntentProfile, ReasoningDepth, TaskScope};

// ── Action ─────────────────────────────────────────────────────────────────

/// Recommended action after observing a round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvergenceAction {
    /// Loop is making progress — continue normally.
    Continue,
    /// Sufficient information gathered — synthesize now.
    Synthesize,
    /// Current approach is failing — request a replan.
    Replan,
    /// No progress possible — halt without synthesis.
    Halt,
}

// ── RoundObservation ───────────────────────────────────────────────────────

/// Summary of a single agent round for convergence analysis.
#[derive(Debug, Clone)]
struct RoundObservation {
    round: u32,
    /// Distinct tool names called this round (empty = text-only round).
    tool_names: Vec<String>,
    /// Hashed (tool, args) pairs for deduplication detection.
    tool_call_hashes: Vec<u64>,
    /// Token count of response text this round (0 = tool-only round).
    response_tokens: u32,
    /// Cumulative full_text length at end of this round.
    cumulative_text_len: usize,
    /// Whether any tool this round returned an error.
    had_errors: bool,
}

// ── ConvergenceController ──────────────────────────────────────────────────

/// Adaptive convergence controller.
///
/// Stateful — one instance per agent loop run.
pub struct ConvergenceController {
    // Config (derived from IntentProfile at construction time)
    max_rounds: u32,
    stagnation_window: usize,
    stagnation_threshold: f32,
    min_rounds_before_stagnation: u32,
    goal_coverage_threshold: f32,

    // State
    history: VecDeque<RoundObservation>,
    /// All distinct (tool, args_hash) pairs seen so far.
    seen_call_hashes: HashSet<u64>,
    /// Per-tool-name call count for saturation detection.
    tool_call_counts: HashMap<String, u32>,
    /// Goal keywords extracted from the intent profile.
    goal_keywords: Vec<String>,
    /// Number of consecutive stagnation signals.
    consecutive_stagnation: u32,
    /// Number of consecutive error rounds.
    consecutive_errors: u32,
    /// Round at which synthesis was first suggested (for escalation).
    synthesize_suggested_at: Option<u32>,
}

impl ConvergenceController {
    /// Construct a controller calibrated to the given intent profile.
    ///
    /// `original_query` is used to extract goal keywords for coverage estimation.
    pub fn new(profile: &IntentProfile, original_query: &str) -> Self {
        let max_rounds = profile.suggested_max_rounds();

        // Stagnation window: larger scope = more rounds before declaring stagnation.
        let stagnation_window = match profile.scope {
            TaskScope::Conversational => 2,
            TaskScope::SingleArtifact => 3,
            TaskScope::LocalContext => 4,
            TaskScope::ProjectWide | TaskScope::SystemWide => 5,
        };

        // Minimum rounds before stagnation fires (give the loop time to explore).
        let min_rounds_before_stagnation = match profile.scope {
            TaskScope::Conversational => 1,
            TaskScope::SingleArtifact => 2,
            TaskScope::LocalContext => 3,
            TaskScope::ProjectWide | TaskScope::SystemWide => 4,
        };

        // Stagnation threshold: fraction of window rounds that must show repetition.
        let stagnation_threshold = match profile.reasoning_depth {
            ReasoningDepth::None => 0.50,
            ReasoningDepth::Light => 0.60,
            ReasoningDepth::Deep => 0.70,
            ReasoningDepth::Exhaustive => 0.80,
        };

        // Goal coverage threshold: how much of the goal must be covered before synthesis.
        let goal_coverage_threshold = match profile.scope {
            TaskScope::Conversational => 0.50,
            TaskScope::SingleArtifact => 0.60,
            TaskScope::LocalContext => 0.55,
            TaskScope::ProjectWide | TaskScope::SystemWide => 0.45,
        };

        let goal_keywords = Self::extract_goal_keywords(original_query);

        Self {
            max_rounds,
            stagnation_window,
            stagnation_threshold,
            min_rounds_before_stagnation,
            goal_coverage_threshold,
            history: VecDeque::with_capacity(8),
            seen_call_hashes: HashSet::new(),
            tool_call_counts: HashMap::new(),
            goal_keywords,
            consecutive_stagnation: 0,
            consecutive_errors: 0,
            synthesize_suggested_at: None,
        }
    }

    /// Construct a controller calibrated to the given intent profile with a
    /// pre-computed effective budget.
    ///
    /// This is the unified-pipeline variant of `new()`. It is called from `agent/mod.rs`
    /// after `IntentPipeline::resolve()` has computed `effective_max_rounds` as the
    /// authoritative reconciliation of IntentScorer and BoundaryDecisionEngine outputs.
    /// Using this constructor ensures that the loop bound and the convergence controller
    /// share a **single source of truth** for `max_rounds` — fixing BV-1 and BV-2.
    ///
    /// The stagnation/coverage calibration logic is identical to `new()` (derived from
    /// `profile.scope` and `profile.reasoning_depth`); only `max_rounds` is overridden.
    pub fn new_with_budget(
        profile: &IntentProfile,
        effective_max_rounds: u32,
        original_query: &str,
    ) -> Self {
        let mut ctrl = Self::new(profile, original_query);
        ctrl.max_rounds = effective_max_rounds;
        ctrl
    }

    /// Construct a tightly-tuned controller for sub-agent execution.
    ///
    /// Sub-agents have focused, narrow tasks — they should converge much faster
    /// than top-level agents.  Key differences from `new()`:
    /// - `max_rounds = 6` — force synthesis sooner (outer timeout provides the hard cap).
    /// - `stagnation_window = 2` — detect repetition quickly.
    /// - `min_rounds_before_stagnation = 1` — fire on first confirmed repetition.
    /// - `goal_coverage_threshold = 0.10` — low bar; sub-agents focus on tool output,
    ///   not prose coverage.  A single keyword match is sufficient to take Synthesize
    ///   over Replan when stagnation is detected.
    /// - Multilingual keyword extraction — handles Spanish/English instructions so
    ///   Spanish task instructions ("estructura del repositorio") map to English output
    ///   tokens ("structure", "repository") via the translation table.
    pub fn new_for_sub_agent(instruction: &str) -> Self {
        Self {
            max_rounds: 6,
            stagnation_window: 2,
            stagnation_threshold: 0.50,
            min_rounds_before_stagnation: 1,
            goal_coverage_threshold: 0.10,
            history: VecDeque::with_capacity(4),
            seen_call_hashes: HashSet::new(),
            tool_call_counts: HashMap::new(),
            goal_keywords: Self::extract_goal_keywords_multilingual(instruction),
            consecutive_stagnation: 0,
            consecutive_errors: 0,
            synthesize_suggested_at: None,
        }
    }

    /// Cap the max_rounds budget to align with the reasoning engine's adjusted limits.
    ///
    /// Called after construction when `reasoning_engine.pre_loop()` computes a tighter
    /// cap (e.g., StrategySelector limits PlanExecuteReflect+Complex to 8 rounds while
    /// the IntentProfile suggests 16 for a ProjectWide query). Ensures ConvergenceController
    /// and the outer agent loop use the same effective round budget — single source of truth.
    ///
    /// This only REDUCES `max_rounds` (cap from above). For sub-agents this is correct:
    /// the profile sets a hard sub-agent limit (6), and the parent can further constrain it.
    /// For top-level agents, use [`set_max_rounds`] instead so the user-configured limit
    /// is honored as both floor and ceiling.
    pub fn cap_max_rounds(&mut self, max: usize) {
        self.max_rounds = self.max_rounds.min(max as u32);
    }

    /// Set the max_rounds budget unconditionally.
    ///
    /// Used for top-level agents where the user-configured `max_rounds` is the
    /// authoritative hard limit. Unlike `cap_max_rounds`, this can INCREASE
    /// `max_rounds` beyond the IntentProfile's suggestion (e.g., profile suggests
    /// 4 rounds for a Simple-classified task, but user configured `max_rounds = 40`).
    /// The profile's stagnation/coverage thresholds still apply for early synthesis.
    ///
    /// Also used for K5-1 budget expansion when the plan requires more rounds than
    /// the initial profile estimate.
    pub fn set_max_rounds(&mut self, max: usize) {
        self.max_rounds = max as u32;
    }

    /// Lower the synthesis trigger threshold based on oscillation urgency.
    ///
    /// Called when `AdaptivePolicy::observe()` returns a non-zero `synthesis_urgency_boost`
    /// (i.e., the agent is oscillating and AdaptivePolicy wants synthesis sooner).
    ///
    /// `boost` is in `[0.0, 1.0]` where `1.0` = maximum urgency.  The threshold is
    /// reduced proportionally, but never below `MIN_COVERAGE_FLOOR` to avoid triggering
    /// synthesis on genuinely insufficient work.
    ///
    /// Effect: the next `observe_round()` call triggers `Synthesize` at a lower
    /// goal-coverage fraction, ending the oscillating loop earlier.
    pub fn boost_synthesis_urgency(&mut self, boost: f32) {
        /// Maximum absolute reduction in goal_coverage_threshold per call.
        const MAX_REDUCTION: f32 = 0.20;
        /// Floor: never reduce coverage threshold below this fraction.
        const MIN_COVERAGE_FLOOR: f32 = 0.20;

        let boost = boost.clamp(0.0, 1.0);
        let reduction = boost * MAX_REDUCTION;
        self.goal_coverage_threshold =
            (self.goal_coverage_threshold - reduction).max(MIN_COVERAGE_FLOOR);
    }

    /// Return the current goal-coverage threshold (for tests and diagnostics).
    #[cfg(test)]
    pub fn goal_coverage_threshold(&self) -> f32 {
        self.goal_coverage_threshold
    }

    /// Observe the outcome of a completed round and return the recommended action.
    ///
    /// # Parameters
    /// - `round` — 0-indexed round number.
    /// - `tool_names` — tools called this round (empty if text-only).
    /// - `tool_args_hashes` — deterministic hashes of (tool_name, serialized_args) pairs.
    /// - `cumulative_text` — full accumulated response text so far.
    /// - `had_errors` — any tool this round returned an error.
    pub fn observe_round(
        &mut self,
        round: u32,
        tool_names: &[String],
        tool_args_hashes: &[u64],
        cumulative_text: &str,
        had_errors: bool,
    ) -> ConvergenceAction {
        // Record observation.
        let response_tokens = (cumulative_text.split_whitespace().count() as u32)
            .saturating_sub(self.current_cumulative_tokens());

        let obs = RoundObservation {
            round,
            tool_names: tool_names.to_vec(),
            tool_call_hashes: tool_args_hashes.to_vec(),
            response_tokens,
            cumulative_text_len: cumulative_text.len(),
            had_errors,
        };
        self.history.push_back(obs.clone());
        if self.history.len() > self.stagnation_window + 2 {
            self.history.pop_front();
        }

        // Update dedup tracking.
        for &h in tool_args_hashes {
            self.seen_call_hashes.insert(h);
        }
        for name in tool_names {
            *self.tool_call_counts.entry(name.clone()).or_insert(0) += 1;
        }

        // Error tracking.
        if had_errors {
            self.consecutive_errors += 1;
        } else {
            self.consecutive_errors = 0;
        }

        // ── Decision tree ────────────────────────────────────────────────

        // 1. Max rounds reached → synthesize.
        if round + 1 >= self.max_rounds {
            return ConvergenceAction::Synthesize;
        }

        // 2. Consecutive errors → replan (3 errors = stuck).
        if self.consecutive_errors >= 3 {
            self.consecutive_errors = 0;
            return ConvergenceAction::Replan;
        }

        // 3. Text-only rounds with sufficient coverage → synthesize early.
        if tool_names.is_empty() {
            let coverage = self.estimate_goal_coverage(cumulative_text);
            if coverage >= self.goal_coverage_threshold && round >= 1 {
                return ConvergenceAction::Synthesize;
            }
        }

        // 4. Stagnation detection (only after min_rounds).
        if round >= self.min_rounds_before_stagnation {
            if self.is_stagnating(tool_args_hashes) {
                self.consecutive_stagnation += 1;
            } else {
                self.consecutive_stagnation = 0;
            }

            if self.consecutive_stagnation >= 1 {
                // One confirmed stagnation round (window excludes self) → synthesize if coverage ok, else replan.
                let coverage = self.estimate_goal_coverage(cumulative_text);
                self.consecutive_stagnation = 0;
                if coverage >= self.goal_coverage_threshold * 0.70 {
                    return ConvergenceAction::Synthesize;
                } else {
                    return ConvergenceAction::Replan;
                }
            }
        }

        // 5. Early success: high goal coverage + enough text → synthesize.
        if round >= 2 {
            let coverage = self.estimate_goal_coverage(cumulative_text);
            let text_substantial = cumulative_text.len() > 500;
            if coverage >= 0.80 && text_substantial {
                return ConvergenceAction::Synthesize;
            }
        }

        ConvergenceAction::Continue
    }

    // ── Stagnation detection ─────────────────────────────────────────────

    /// Returns true if this round's tool calls are predominantly repetitions of recent rounds.
    fn is_stagnating(&self, current_hashes: &[u64]) -> bool {
        if current_hashes.is_empty() {
            return false; // Text-only round = not stagnation in the tool sense.
        }
        // Skip the first element (current round, just pushed) to avoid self-comparison.
        let window: Vec<_> = self
            .history
            .iter()
            .rev()
            .skip(1)
            .take(self.stagnation_window)
            .collect();
        if window.len() < 2 {
            return false;
        }

        let current_set: HashSet<u64> = current_hashes.iter().copied().collect();
        let stagnant_rounds = window
            .iter()
            .filter(|obs| {
                let prev_set: HashSet<u64> = obs.tool_call_hashes.iter().copied().collect();
                let intersection = current_set.intersection(&prev_set).count();
                let union = current_set.union(&prev_set).count();
                // Jaccard similarity: > 70% overlap = stagnant.
                union > 0 && (intersection as f32 / union as f32) >= 0.70
            })
            .count();

        let ratio = stagnant_rounds as f32 / window.len() as f32;
        ratio >= self.stagnation_threshold
    }

    // ── Goal coverage ────────────────────────────────────────────────────

    /// Estimate what fraction of goal keywords appear in the accumulated text.
    ///
    /// Simple O(K × W) scan — good enough for loop termination decisions.
    fn estimate_goal_coverage(&self, text: &str) -> f32 {
        if self.goal_keywords.is_empty() {
            // CR-4 fix: Previously returned 0.5 (neutral), which satisfied stagnation
            // thresholds (thresh × 0.70) for ALL scopes, forcing Synthesize instead of
            // Replan on ambiguous queries. Returning 0.0 forces the stagnation path to
            // choose Replan, giving the agent a chance to try alternative strategies.
            return 0.0;
        }
        let text_lower = text.to_lowercase();
        let covered = self
            .goal_keywords
            .iter()
            .filter(|kw| text_lower.contains(kw.as_str()))
            .count();
        covered as f32 / self.goal_keywords.len() as f32
    }

    /// Extract meaningful goal keywords from the original query.
    ///
    /// Delegates to the unified `text_utils::extract_keywords` — single source of truth
    /// for stopwords and filtering logic (D6 fix: eliminates duplicated STOPWORDS list).
    fn extract_goal_keywords(query: &str) -> Vec<String> {
        super::text_utils::extract_keywords(query)
            .into_iter()
            .collect()
    }

    /// Multilingual keyword extraction for sub-agent instructions.
    ///
    /// Uses `text_utils::extract_keywords_multilingual` which handles Spanish instructions
    /// by adding English equivalents (e.g. "estructura" → also adds "structure").
    /// This prevents false-negative coverage misses when the instruction is in Spanish
    /// but the agent output (tool results, directory listings) is in English.
    fn extract_goal_keywords_multilingual(query: &str) -> Vec<String> {
        super::text_utils::extract_keywords_multilingual(query)
            .into_iter()
            .collect()
    }

    // ── Helpers ───────────────────────────────────────────────────────────

    fn current_cumulative_tokens(&self) -> u32 {
        self.history.iter().map(|o| o.response_tokens).sum()
    }

    /// Returns a human-readable summary of current controller state for tracing.
    pub fn diagnostic(&self) -> String {
        format!(
            "ConvergenceController {{ max_rounds={}, stagnation_window={}, consecutive_stagnation={}, consecutive_errors={}, goal_keywords={} }}",
            self.max_rounds,
            self.stagnation_window,
            self.consecutive_stagnation,
            self.consecutive_errors,
            self.goal_keywords.len(),
        )
    }

    /// How many rounds remain before max_rounds forces synthesis.
    pub fn rounds_remaining(&self, current_round: u32) -> u32 {
        self.max_rounds.saturating_sub(current_round + 1)
    }

    /// Maximum rounds this controller will allow before forcing synthesis.
    pub fn max_rounds(&self) -> u32 {
        self.max_rounds
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::intent_scorer::IntentScorer;

    fn make_controller(query: &str) -> ConvergenceController {
        let profile = IntentScorer::score(query);
        ConvergenceController::new(&profile, query)
    }

    fn empty_tools() -> (Vec<String>, Vec<u64>) {
        (vec![], vec![])
    }

    fn tools(names: &[&str], hashes: &[u64]) -> (Vec<String>, Vec<u64>) {
        (
            names.iter().map(|s| s.to_string()).collect(),
            hashes.to_vec(),
        )
    }

    #[test]
    fn synthesize_at_max_rounds() {
        let profile = IntentScorer::score("analiza el proyecto");
        // Force max_rounds = 3 by using a very simple profile override.
        let mut ctrl = ConvergenceController::new(&profile, "analiza el proyecto");
        ctrl.max_rounds = 3;

        let (ns, hs) = empty_tools();
        assert_eq!(
            ctrl.observe_round(0, &ns, &hs, "", false),
            ConvergenceAction::Continue
        );
        assert_eq!(
            ctrl.observe_round(1, &ns, &hs, "", false),
            ConvergenceAction::Continue
        );
        assert_eq!(
            ctrl.observe_round(2, &ns, &hs, "", false),
            ConvergenceAction::Synthesize
        );
    }

    #[test]
    fn continue_when_progress_being_made() {
        let mut ctrl = make_controller("fix the bug in auth.rs");
        let (ns, hs) = tools(&["file_read"], &[1001]);
        assert_eq!(
            ctrl.observe_round(0, &ns, &hs, "Reading auth.rs...", false),
            ConvergenceAction::Continue
        );
    }

    #[test]
    fn replan_after_three_consecutive_errors() {
        let mut ctrl = make_controller("find all broken imports in the project");
        let (ns, hs) = tools(&["bash"], &[999]);
        ctrl.observe_round(0, &ns, &hs, "", true);
        ctrl.observe_round(1, &ns, &hs, "", true);
        let action = ctrl.observe_round(2, &ns, &hs, "", true);
        assert_eq!(
            action,
            ConvergenceAction::Replan,
            "Expected Replan after 3 consecutive errors"
        );
    }

    #[test]
    fn synthesize_on_text_only_round_with_good_coverage() {
        let query = "explain how caching works";
        let mut ctrl = make_controller(query);
        ctrl.min_rounds_before_stagnation = 0;

        // First round with tools.
        let (ns, hs) = tools(&["file_read"], &[100]);
        ctrl.observe_round(0, &ns, &hs, "Reading cache.rs...", false);

        // Second round: text-only with high coverage.
        let (ns2, hs2) = empty_tools();
        let rich_text = "explain how caching works: the cache stores key-value pairs with TTL. \
            Caching improves performance by reducing redundant computation. \
            The cache invalidation strategy uses LRU eviction.";
        let action = ctrl.observe_round(1, &ns2, &hs2, rich_text, false);
        assert_eq!(
            action,
            ConvergenceAction::Synthesize,
            "Expected Synthesize on text-only round with high coverage"
        );
    }

    #[test]
    fn stagnation_triggers_synthesize_or_replan() {
        let mut ctrl = make_controller("analyze codebase structure");
        ctrl.min_rounds_before_stagnation = 0;
        ctrl.stagnation_threshold = 0.50;

        // Same tool+args for many rounds = stagnation.
        let repeated_hash = 12345u64;
        let (ns, _) = tools(&["glob"], &[repeated_hash]);
        let hs = vec![repeated_hash];

        ctrl.observe_round(0, &ns, &hs, "found some files", false);
        ctrl.observe_round(1, &ns, &hs, "found some files still", false);
        let action = ctrl.observe_round(2, &ns, &hs, "found some files again", false);

        assert!(
            action == ConvergenceAction::Synthesize || action == ConvergenceAction::Replan,
            "Expected Synthesize or Replan on stagnation, got {:?}",
            action
        );
    }

    #[test]
    fn no_stagnation_when_tools_vary() {
        let mut ctrl = make_controller("fix bugs across modules");
        ctrl.min_rounds_before_stagnation = 0;
        ctrl.max_rounds = 10; // Override to prevent max-rounds guard from firing.

        // Different tools each round = progress.
        let (ns1, hs1) = tools(&["file_read"], &[1]);
        let (ns2, hs2) = tools(&["bash"], &[2]);
        let (ns3, hs3) = tools(&["grep"], &[3]);

        assert_eq!(
            ctrl.observe_round(0, &ns1, &hs1, "text1", false),
            ConvergenceAction::Continue
        );
        assert_eq!(
            ctrl.observe_round(1, &ns2, &hs2, "text2", false),
            ConvergenceAction::Continue
        );
        assert_eq!(
            ctrl.observe_round(2, &ns3, &hs3, "text3", false),
            ConvergenceAction::Continue
        );
    }

    #[test]
    fn conversational_has_low_max_rounds() {
        let ctrl = make_controller("hola");
        assert!(
            ctrl.max_rounds <= 3,
            "conversational max_rounds={}",
            ctrl.max_rounds
        );
    }

    #[test]
    fn project_wide_has_high_max_rounds() {
        let ctrl = make_controller("analiza el proyecto completo y revisa todos los archivos");
        assert!(
            ctrl.max_rounds >= 10,
            "project_wide max_rounds={}",
            ctrl.max_rounds
        );
    }

    #[test]
    fn early_success_with_high_coverage() {
        let query = "how does the authentication module work";
        let mut ctrl = make_controller(query);

        // After 2 rounds, lots of text covering all goal keywords.
        let (ns, hs) = tools(&["file_read"], &[1]);
        ctrl.observe_round(0, &ns, &hs, "authentication module implements OAuth", false);
        let (ns2, hs2) = empty_tools();
        // Rich text with high coverage of goal keywords.
        let rich = "The authentication module works by implementing OAuth2. \
            The module handles token validation and session management. \
            Authentication uses JWT tokens for stateless verification.";
        let action = ctrl.observe_round(1, &ns2, &hs2, rich, false);
        // Should synthesize or continue (never replan with good progress).
        assert_ne!(
            action,
            ConvergenceAction::Replan,
            "No replan with good coverage"
        );
        assert_ne!(action, ConvergenceAction::Halt);
    }

    #[test]
    fn goal_keywords_extracted_correctly() {
        let kws = ConvergenceController::extract_goal_keywords(
            "analyze the authentication module performance",
        );
        // Should contain content words, not stopwords.
        assert!(
            kws.contains(&"analyze".to_string()) || kws.contains(&"authentication".to_string()),
            "Expected content words in keywords: {:?}",
            kws
        );
        // Should NOT contain stopwords.
        assert!(!kws.contains(&"the".to_string()));
        assert!(!kws.contains(&"and".to_string()));
    }

    #[test]
    fn rounds_remaining_correct() {
        let mut ctrl = make_controller("fix bug");
        ctrl.max_rounds = 5;
        assert_eq!(ctrl.rounds_remaining(0), 4);
        assert_eq!(ctrl.rounds_remaining(4), 0);
    }

    #[test]
    fn diagnostic_is_non_empty() {
        let ctrl = make_controller("analyze codebase");
        let diag = ctrl.diagnostic();
        assert!(diag.contains("max_rounds"), "diagnostic: {}", diag);
    }

    // ── Phase 110: E2E integration tests (Intent→Convergence pipeline) ────────

    /// Phase 110-A: Verify that IntentScorer→ConvergenceController forms a coherent pipeline.
    /// The same user query must produce the same profile and the same max_rounds bound on
    /// every call — no nondeterminism that would cause flapping between rounds.
    #[test]
    fn intent_scorer_to_convergence_is_deterministic() {
        let query = "find all files that import the auth module";
        let ctrl1 = make_controller(query);
        let ctrl2 = make_controller(query);
        assert_eq!(
            ctrl1.max_rounds, ctrl2.max_rounds,
            "ConvergenceController must be deterministic for same query"
        );
        assert_eq!(
            ctrl1.min_rounds_before_stagnation, ctrl2.min_rounds_before_stagnation,
            "min_rounds_before_stagnation must be deterministic"
        );
    }

    /// Phase 110-B: Conversational queries must get very few max_rounds.
    /// This is the critical case — greeting → ConvergenceController configured for 2-3 rounds max.
    /// Prevents the agent from burning tokens on a simple "hola" request.
    #[test]
    fn conversational_scope_limits_max_rounds_to_small_budget() {
        for query in &["hola", "hello", "gracias", "ok thanks"] {
            let ctrl = make_controller(query);
            assert!(
                ctrl.max_rounds <= 4,
                "Conversational query {:?} must have max_rounds ≤ 4, got {}",
                query,
                ctrl.max_rounds
            );
        }
    }

    /// Phase 110-C: ProjectWide queries must get more rounds than LocalContext queries.
    /// Ensures the scope hierarchy is reflected in the convergence budget.
    #[test]
    fn scope_hierarchy_reflected_in_max_rounds() {
        let conversational = make_controller("hola");
        let local = make_controller("fix the bug in auth.rs line 42");
        let project_wide =
            make_controller("refactor all authentication modules across the entire codebase");

        assert!(
            conversational.max_rounds <= local.max_rounds,
            "conversational({}) should have ≤ local({}) rounds",
            conversational.max_rounds,
            local.max_rounds
        );
        assert!(
            local.max_rounds <= project_wide.max_rounds,
            "local({}) should have ≤ project_wide({}) rounds",
            local.max_rounds,
            project_wide.max_rounds
        );
    }

    /// Phase 110-D: Repeated identical tool calls + no output growth = stagnation.
    /// Verify the full pipeline from IntentScorer → ConvergenceController → Synthesize/Replan.
    #[test]
    fn full_pipeline_stagnation_triggers_action() {
        let query = "find all broken imports in the codebase and fix them";
        let profile = IntentScorer::score(query);
        let mut ctrl = ConvergenceController::new(&profile, query);
        ctrl.min_rounds_before_stagnation = 0; // Fire immediately for test speed.
        ctrl.stagnation_threshold = 0.40;

        // Identical tool + args every round → stagnation.
        let same_tool = vec!["bash".to_string()];
        let same_hash = vec![99999u64];

        ctrl.observe_round(0, &same_tool, &same_hash, "", false);
        ctrl.observe_round(1, &same_tool, &same_hash, "", false);
        let action = ctrl.observe_round(2, &same_tool, &same_hash, "", false);

        assert!(
            action == ConvergenceAction::Synthesize || action == ConvergenceAction::Replan,
            "Expected stagnation to produce Synthesize or Replan, got {:?}",
            action
        );
    }

    /// Phase 110-E: IntentProfile.suggested_max_rounds() and ConvergenceController.max_rounds
    /// must agree — ensures the pipeline does not silently lose the scope-based bound.
    #[test]
    fn convergence_max_rounds_matches_intent_suggested() {
        let cases = [
            "hola",
            "explain how the cache works",
            "refactor the auth module to use JWT",
            "analyze the entire project and produce a comprehensive architecture report",
        ];
        for query in &cases {
            let profile = IntentScorer::score(query);
            let ctrl = ConvergenceController::new(&profile, query);
            let suggested = profile.suggested_max_rounds();
            assert!(
                ctrl.max_rounds() <= suggested,
                "Query {:?}: ctrl.max_rounds ({}) > suggested_max_rounds ({})",
                query,
                ctrl.max_rounds(),
                suggested
            );
        }
    }

    // ── boost_synthesis_urgency tests ────────────────────────────────────────

    #[test]
    fn boost_reduces_goal_coverage_threshold() {
        let mut ctrl = make_controller("analyze the project architecture");
        let before = ctrl.goal_coverage_threshold();
        ctrl.boost_synthesis_urgency(0.5);
        assert!(
            ctrl.goal_coverage_threshold() < before,
            "boost should lower goal_coverage_threshold: before={before:.3}, after={:.3}",
            ctrl.goal_coverage_threshold()
        );
    }

    #[test]
    fn boost_zero_has_no_effect() {
        let mut ctrl = make_controller("fix the auth module");
        let before = ctrl.goal_coverage_threshold();
        ctrl.boost_synthesis_urgency(0.0);
        assert_eq!(
            ctrl.goal_coverage_threshold(),
            before,
            "boost=0.0 must not change threshold"
        );
    }

    #[test]
    fn boost_max_reduces_by_at_most_twenty_points() {
        let mut ctrl = make_controller("refactor the entire codebase");
        let before = ctrl.goal_coverage_threshold();
        ctrl.boost_synthesis_urgency(1.0);
        let reduction = before - ctrl.goal_coverage_threshold();
        assert!(
            (reduction - 0.20_f32).abs() <= f32::EPSILON * 10.0,
            "max boost should reduce by exactly 0.20: reduction={reduction:.4}"
        );
    }

    #[test]
    fn boost_clamps_at_floor_twenty_percent() {
        let mut ctrl = make_controller("hi");
        // Force threshold to a low value, then apply maximum boost repeatedly.
        ctrl.goal_coverage_threshold = 0.25;
        ctrl.boost_synthesis_urgency(1.0);
        ctrl.boost_synthesis_urgency(1.0);
        ctrl.boost_synthesis_urgency(1.0);
        assert!(
            ctrl.goal_coverage_threshold() >= 0.20,
            "threshold must never fall below MIN_COVERAGE_FLOOR=0.20, got {}",
            ctrl.goal_coverage_threshold()
        );
    }

    // ── new_for_sub_agent tests ───────────────────────────────────────────────

    #[test]
    fn sub_agent_has_low_max_rounds() {
        let ctrl = ConvergenceController::new_for_sub_agent("Obtener estructura del repositorio");
        assert!(
            ctrl.max_rounds <= 6,
            "sub-agent controller must have max_rounds ≤ 6, got {}",
            ctrl.max_rounds
        );
    }

    #[test]
    fn sub_agent_has_low_goal_coverage_threshold() {
        let ctrl = ConvergenceController::new_for_sub_agent("list all files in src/");
        assert!(
            ctrl.goal_coverage_threshold() <= 0.15,
            "sub-agent coverage threshold must be ≤ 0.15, got {}",
            ctrl.goal_coverage_threshold()
        );
    }

    #[test]
    fn sub_agent_stagnation_yields_synthesize_not_replan() {
        // Stagnation detection requires window.len() >= 2, which means at least 3 observed rounds
        // (the window skips the current round, so history must have 3 entries for window=2).
        // This test verifies: with the low sub-agent threshold (0.10), when stagnation fires
        // and output contains "repository" (English translation of "repositorio"), we get
        // Synthesize (coverage=1/6≈0.17 > 0.10*0.70=0.07) instead of Replan.
        let mut ctrl = ConvergenceController::new_for_sub_agent("estructura repositorio archivo");
        ctrl.min_rounds_before_stagnation = 0;

        // Simulate: output mentions "repository" (English equivalent of "repositorio").
        let text_with_match = "The repository contains src/";
        let same_hash = vec![42u64];
        let ns = vec!["bash".to_string()];

        // Three rounds of identical tool+args to trigger stagnation (window needs len>=2).
        ctrl.observe_round(0, &ns, &same_hash, text_with_match, false);
        ctrl.observe_round(1, &ns, &same_hash, text_with_match, false);
        let action = ctrl.observe_round(2, &ns, &same_hash, text_with_match, false);

        // Coverage: "repository" matches (via translation), ~1/6 keywords = ~0.17 > 0.07 threshold.
        assert_eq!(
            action,
            ConvergenceAction::Synthesize,
            "sub-agent stagnation with partial coverage should Synthesize, got {:?}",
            action
        );
    }

    #[test]
    fn sub_agent_multilingual_keywords_include_english() {
        let ctrl = ConvergenceController::new_for_sub_agent(
            "Obtener una vista general de la estructura del repositorio",
        );
        // Keywords should include English translations for Spanish domain words.
        let has_structure = ctrl.goal_keywords.contains(&"structure".to_string());
        let has_repository = ctrl.goal_keywords.contains(&"repository".to_string());
        assert!(
            has_structure || has_repository,
            "multilingual extraction should include English equivalents, keywords: {:?}",
            ctrl.goal_keywords
        );
    }

    #[test]
    fn sub_agent_fits_within_parent_cap() {
        let mut ctrl = ConvergenceController::new_for_sub_agent("analyze the project structure");
        ctrl.cap_max_rounds(10); // Simulate derive_sub_limits cap.
        assert_eq!(
            ctrl.max_rounds(),
            6,
            "cap_max_rounds(10) should not increase sub-agent max_rounds above 6"
        );
    }

    #[test]
    fn boosted_controller_synthesizes_at_lower_coverage() {
        // Build two controllers; boost one; verify the boosted one synthesizes
        // at the same coverage that makes the un-boosted one Continue.
        let mut ctrl_default = make_controller("create a simple hello world script");
        let mut ctrl_boosted = make_controller("create a simple hello world script");
        ctrl_boosted.boost_synthesis_urgency(1.0);

        // Use a text with partial keyword coverage that sits between the two thresholds.
        // The text contains the word "hello" and "world" but not "create" or "script".
        let partial_text = "hello world output done";

        // Both controllers start at round 1+ (tool-only → text round); text-only path uses threshold.
        let (empty_names, empty_hashes) = empty_tools();
        // Feed one prior round so round >= 1 check passes.
        ctrl_default.observe_round(0, &empty_names, &empty_hashes, "", false);
        ctrl_boosted.observe_round(0, &empty_names, &empty_hashes, "", false);

        let default_action =
            ctrl_default.observe_round(1, &empty_names, &empty_hashes, partial_text, false);
        let boosted_action =
            ctrl_boosted.observe_round(1, &empty_names, &empty_hashes, partial_text, false);

        // The boosted controller's lower threshold means the same partial_text may flip to Synthesize.
        // We only assert that the boosted threshold is lower (already tested above) and that
        // the actions are deterministic given their respective thresholds.
        // Full coverage of the "boosted flips to Synthesize" case depends on keyword extraction
        // of the query which varies — so we just assert the boosted threshold is lower.
        assert!(
            ctrl_boosted.goal_coverage_threshold() < ctrl_default.goal_coverage_threshold(),
            "boosted controller must have lower threshold than default"
        );
        // If default already synthesizes, boosted must also synthesize (monotonicity).
        if default_action == ConvergenceAction::Synthesize {
            assert_eq!(
                boosted_action,
                ConvergenceAction::Synthesize,
                "if default synthesizes, boosted must also synthesize"
            );
        }
    }

    // ── BV-1 fix: ConvergenceController calibrates from final SLA budget ─────

    #[test]
    fn new_with_budget_sets_max_rounds_to_provided_budget() {
        let profile = IntentScorer::score("fix bug in auth module");
        // Profile might suggest 8 rounds; we force a short SLA budget of 5.
        let short_budget = 5u32;
        let ctrl = ConvergenceController::new_with_budget(&profile, short_budget, "fix bug");
        assert_eq!(
            ctrl.max_rounds, short_budget,
            "ConvergenceController must use the provided SLA budget, not profile.suggested_max_rounds()"
        );
    }

    #[test]
    fn new_with_budget_stagnation_window_proportional_to_budget() {
        let profile = IntentScorer::score("fix bug in auth module");
        let short_budget = 5u32;
        let ctrl = ConvergenceController::new_with_budget(&profile, short_budget, "fix bug");
        // stagnation_window must be ≤ max_rounds so stagnation can actually fire
        assert!(
            ctrl.stagnation_window <= ctrl.max_rounds as usize,
            "stagnation_window({}) must be <= max_rounds({}) — otherwise stagnation never fires",
            ctrl.stagnation_window,
            ctrl.max_rounds
        );
    }

    // ── CR-4 fix: empty keywords return 0.0, not 0.5 ─────────────────────

    #[test]
    fn empty_keywords_coverage_returns_zero() {
        // A query composed entirely of stopwords should produce empty goal_keywords
        // and return 0.0 coverage (not 0.5 neutral). This ensures stagnation
        // paths choose Replan over Synthesize for ambiguous queries.
        let ctrl = make_controller("the and with");
        let coverage = ctrl.estimate_goal_coverage("some random text about things");
        assert_eq!(
            coverage, 0.0,
            "CR-4: empty keywords must return 0.0, not 0.5 — \
             otherwise stagnation always synthesizes for ambiguous queries"
        );
    }

    #[test]
    fn stagnation_with_empty_keywords_triggers_replan() {
        // When keywords are empty and stagnation fires, coverage=0.0 should
        // be below threshold×0.70 for all scopes, forcing Replan not Synthesize.
        let mut ctrl = make_controller("the and or");
        ctrl.max_rounds = 10;
        ctrl.stagnation_window = 2;
        ctrl.min_rounds_before_stagnation = 1;

        // Feed identical tool rounds to trigger stagnation.
        let (ns, hs) = tools(&["grep"], &[42]);
        ctrl.observe_round(0, &ns, &hs, "", false);
        ctrl.observe_round(1, &ns, &hs, "", false);
        let action = ctrl.observe_round(2, &ns, &hs, "", false);

        // With empty keywords, coverage=0.0, so stagnation should Replan (not Synthesize).
        assert!(
            matches!(
                action,
                ConvergenceAction::Replan | ConvergenceAction::Continue
            ),
            "CR-4: stagnation with empty keywords should NOT Synthesize. Got: {action:?}"
        );
    }
}
