//! Progress-aware execution tracker (Frontier AAA).
//!
//! Replaces token-based diminishing returns detection with actual task progress
//! measurement. Tracks unique successful tool executions and stagnation patterns.
//!
//! Design rationale (papers):
//!   - Renze 2024: "Self-reflection most effective when accuracy is low" — only
//!     intervene when stagnation is detected via progress signals, not token volume.
//!   - Plan-and-Act (ICML 2025): Progress should be measured by plan step completion,
//!     not by output token deltas.
//!   - Rabanser 2026: Reliability requires measuring consistency and predictability
//!     separately from raw capability.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use halcon_core::types::ContentBlock;

// ── Progress signals ────────────────────────────────────────────────────────

/// Per-round progress signal for the feedback arbiter.
#[derive(Debug, Clone)]
pub struct ProgressSignal {
    /// Number of tool calls not seen in prior rounds.
    pub new_tools: u32,
    /// Number of tool results with is_error = false.
    pub successful_results: u32,
    /// Number of tool results with is_error = true.
    pub failed_results: u32,
    /// Consecutive rounds with no new successful unique tool calls.
    pub rounds_stalled: u32,
}

// ── Reflection triggers ─────────────────────────────────────────────────────

/// Typed reason for injecting a reflection prompt.
///
/// Conditional reflection avoids "degeneration of thought" (MAR 2025):
/// only reflect when there's a concrete signal, never reflexively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectionTrigger {
    /// 2+ tool errors in a single round.
    RepeatedToolFailure,
    /// ProgressTracker.is_stagnating() returned true.
    Stagnation,
    /// StagnationTracker detected identical tool hash patterns.
    LoopDetected,
}

// ── ProgressTracker ─────────────────────────────────────────────────────────

/// Tracks real task progress across agent loop rounds.
///
/// Progress is defined as executing new, unique, successful tool calls.
/// Two consecutive rounds producing < 500 output tokens is NOT stagnation
/// if new tools are being called successfully (e.g., short confirmations).
///
/// Invariant: `is_stagnating()` only returns true after 3+ rounds with
/// zero new successful unique tool executions.
pub struct ProgressTracker {
    /// Set of (tool_name, args_hash) pairs that succeeded.
    successful_calls: HashSet<u64>,
    /// Consecutive rounds with no new successful unique tool call.
    no_progress_rounds: u32,
    /// Last round number where progress was observed.
    last_progress_round: u32,
    /// Total successful tool results across all rounds.
    total_successes: u32,
    /// Total failed tool results across all rounds.
    total_failures: u32,
}

impl ProgressTracker {
    pub fn new() -> Self {
        Self {
            successful_calls: HashSet::new(),
            no_progress_rounds: 0,
            last_progress_round: 0,
            total_successes: 0,
            total_failures: 0,
        }
    }

    /// Observe a round's tool names and results.
    ///
    /// Returns a `ProgressSignal` summarizing the round's contribution.
    /// A round makes progress if at least one tool call is both new AND successful.
    pub fn observe_round(
        &mut self,
        tool_names: &[String],
        tool_results: &[ContentBlock],
        round: u32,
    ) -> ProgressSignal {
        let mut new_tools: u32 = 0;
        let mut successful: u32 = 0;
        let mut failed: u32 = 0;

        // Count successes and failures from results
        for result in tool_results {
            if let ContentBlock::ToolResult { is_error, .. } = result {
                if *is_error {
                    failed += 1;
                } else {
                    successful += 1;
                }
            }
        }

        // Check for new unique tool calls
        for name in tool_names {
            let hash = Self::tool_hash(name);
            if self.successful_calls.insert(hash) {
                new_tools += 1;
            }
        }

        self.total_successes += successful;
        self.total_failures += failed;

        // Progress = new unique tools OR successful results from novel calls
        let made_progress = new_tools > 0 && successful > 0;
        if made_progress {
            self.no_progress_rounds = 0;
            self.last_progress_round = round;
        } else {
            self.no_progress_rounds += 1;
        }

        ProgressSignal {
            new_tools,
            successful_results: successful,
            failed_results: failed,
            rounds_stalled: self.no_progress_rounds,
        }
    }

    /// True when 3+ consecutive rounds produced no new successful unique tool results.
    ///
    /// This replaces token-based diminishing returns detection.
    /// Short responses that ARE making progress (new tools, successful results)
    /// will NOT trigger stagnation.
    pub fn is_stagnating(&self) -> bool {
        self.no_progress_rounds >= 3
    }

    /// Current stagnation depth (for arbiter signals).
    pub fn stagnation_depth(&self) -> u32 {
        self.no_progress_rounds
    }

    /// Total successful tool executions across all rounds.
    pub fn total_successes(&self) -> u32 {
        self.total_successes
    }

    /// Total failed tool executions across all rounds.
    pub fn total_failures(&self) -> u32 {
        self.total_failures
    }

    fn tool_hash(name: &str) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut h);
        h.finish()
    }
}

// ── Conditional reflection ──────────────────────────────────────────────────

/// Determine whether to inject a reflection prompt this round.
///
/// Only reflects when there are concrete signals — never reflexively.
/// This avoids the "degeneration of thought" problem identified in MAR (2025).
pub fn should_reflect(
    progress: &ProgressSignal,
    stagnation_stalls: u32,
) -> Option<ReflectionTrigger> {
    // 1. Multiple tool errors → reflect on approach
    if progress.failed_results >= 2 {
        return Some(ReflectionTrigger::RepeatedToolFailure);
    }
    // 2. Progress tracker says we're stagnating
    if progress.rounds_stalled >= 3 {
        return Some(ReflectionTrigger::Stagnation);
    }
    // 3. StagnationTracker detected repeated tool patterns
    if stagnation_stalls >= 2 {
        return Some(ReflectionTrigger::LoopDetected);
    }
    None
}

/// Generate a focused reflection injection message.
///
/// Each trigger type gets a specific prompt designed to break the failure mode:
/// - RepeatedToolFailure → diagnose tool input/permission issues
/// - Stagnation → high-level strategy review
/// - LoopDetected → force completely different approach
pub fn reflection_message(trigger: ReflectionTrigger) -> &'static str {
    match trigger {
        ReflectionTrigger::RepeatedToolFailure => {
            "REFLECT: Multiple tool calls failed this round. Before retrying, consider: \
             (1) Is the approach correct? (2) Are there permission or path issues? \
             (3) Should you try a completely different tool or strategy?"
        }
        ReflectionTrigger::Stagnation => {
            "REFLECT: No meaningful progress in recent rounds. Briefly assess: \
             What have you accomplished so far? What is blocking you? \
             Propose ONE concrete different strategy and execute it."
        }
        ReflectionTrigger::LoopDetected => {
            "REFLECT: You are repeating the same tool calls. This is a loop. \
             STOP the current approach entirely. Analyze WHY it's failing, then \
             try a fundamentally different strategy."
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_result(id: &str, is_error: bool) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: "result".to_string(),
            is_error,
        }
    }

    #[test]
    fn new_tools_count_as_progress() {
        let mut tracker = ProgressTracker::new();
        let sig = tracker.observe_round(
            &["read".into(), "write".into()],
            &[tool_result("1", false), tool_result("2", false)],
            1,
        );
        assert_eq!(sig.new_tools, 2);
        assert_eq!(sig.successful_results, 2);
        assert!(!tracker.is_stagnating());
    }

    #[test]
    fn repeated_tools_no_progress() {
        let mut tracker = ProgressTracker::new();
        // Round 1: new tools
        tracker.observe_round(&["read".into()], &[tool_result("1", false)], 1);
        // Round 2: same tool, still has new hash? No — "read" was already seen
        let sig = tracker.observe_round(&["read".into()], &[tool_result("2", false)], 2);
        assert_eq!(sig.new_tools, 0);
        assert_eq!(tracker.no_progress_rounds, 1);
    }

    #[test]
    fn stagnation_after_three_rounds() {
        let mut tracker = ProgressTracker::new();
        // Establish baseline
        tracker.observe_round(&["tool_a".into()], &[tool_result("1", false)], 1);
        assert!(!tracker.is_stagnating());

        // 3 rounds of no new tools
        for r in 2..=4 {
            tracker.observe_round(&["tool_a".into()], &[tool_result(&r.to_string(), false)], r);
        }
        assert!(tracker.is_stagnating());
    }

    #[test]
    fn failed_tools_dont_count_as_progress() {
        let mut tracker = ProgressTracker::new();
        let sig = tracker.observe_round(&["bash".into()], &[tool_result("1", true)], 1);
        assert_eq!(sig.new_tools, 1); // Tool is new
        assert_eq!(sig.successful_results, 0); // But failed
                                               // new_tools > 0 BUT successful == 0 → no progress
        assert_eq!(tracker.no_progress_rounds, 1);
    }

    #[test]
    fn progress_resets_stagnation() {
        let mut tracker = ProgressTracker::new();
        // 2 rounds no progress
        tracker.observe_round(&["a".into()], &[tool_result("1", false)], 1);
        tracker.observe_round(&["a".into()], &[tool_result("2", false)], 2);
        tracker.observe_round(&["a".into()], &[tool_result("3", false)], 3);
        assert_eq!(tracker.no_progress_rounds, 2);

        // New tool → progress
        tracker.observe_round(&["b".into()], &[tool_result("4", false)], 4);
        assert_eq!(tracker.no_progress_rounds, 0);
        assert!(!tracker.is_stagnating());
    }

    #[test]
    fn reflection_on_repeated_failure() {
        let sig = ProgressSignal {
            new_tools: 0,
            successful_results: 0,
            failed_results: 3,
            rounds_stalled: 0,
        };
        assert_eq!(
            should_reflect(&sig, 0),
            Some(ReflectionTrigger::RepeatedToolFailure)
        );
    }

    #[test]
    fn reflection_on_stagnation() {
        let sig = ProgressSignal {
            new_tools: 0,
            successful_results: 1,
            failed_results: 0,
            rounds_stalled: 3,
        };
        assert_eq!(should_reflect(&sig, 0), Some(ReflectionTrigger::Stagnation));
    }

    #[test]
    fn reflection_on_loop_detected() {
        let sig = ProgressSignal {
            new_tools: 0,
            successful_results: 1,
            failed_results: 0,
            rounds_stalled: 0,
        };
        assert_eq!(
            should_reflect(&sig, 2),
            Some(ReflectionTrigger::LoopDetected)
        );
    }

    #[test]
    fn no_reflection_when_progressing() {
        let sig = ProgressSignal {
            new_tools: 2,
            successful_results: 2,
            failed_results: 0,
            rounds_stalled: 0,
        };
        assert_eq!(should_reflect(&sig, 0), None);
    }

    #[test]
    fn total_counters_accumulate() {
        let mut tracker = ProgressTracker::new();
        tracker.observe_round(&["a".into()], &[tool_result("1", false)], 1);
        tracker.observe_round(&["b".into()], &[tool_result("2", true)], 2);
        tracker.observe_round(
            &["c".into()],
            &[tool_result("3", false), tool_result("4", true)],
            3,
        );
        assert_eq!(tracker.total_successes(), 2);
        assert_eq!(tracker.total_failures(), 2);
    }
}
