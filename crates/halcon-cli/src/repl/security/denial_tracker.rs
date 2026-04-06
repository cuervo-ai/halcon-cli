//! Per-tool denial counter with escalation threshold.
//!
//! Adopted from Xiyo's proven denial tracking pattern (Claude Code, 2025):
//! after N consecutive denials for the same tool, the system should escalate
//! its behavior (e.g., switching from auto-deny to prompting the user,
//! or surfacing the denial pattern to the agent).
//!
//! The tracker is per-tool and per-session — it does not persist across sessions.

use std::collections::HashMap;

/// Per-tool denial counter with configurable escalation threshold.
///
/// When a tool is denied `threshold` times consecutively, `should_escalate()`
/// returns true. A successful execution resets the counter for that tool.
pub struct DenialTracker {
    counts: HashMap<String, u32>,
    threshold: u32,
}

impl DenialTracker {
    /// Create a new tracker with the given escalation threshold.
    ///
    /// A threshold of 3 means: after 3 consecutive denials for the same tool,
    /// `should_escalate()` returns true.
    pub fn new(threshold: u32) -> Self {
        Self {
            counts: HashMap::new(),
            threshold,
        }
    }

    /// Record a denial for a tool. Increments the consecutive denial counter.
    pub fn record_denial(&mut self, tool: &str) {
        *self.counts.entry(tool.to_owned()).or_insert(0) += 1;
    }

    /// Record a successful execution. Resets the denial counter for that tool.
    pub fn record_success(&mut self, tool: &str) {
        self.counts.remove(tool);
    }

    /// Check if a tool has reached the escalation threshold.
    pub fn should_escalate(&self, tool: &str) -> bool {
        self.counts
            .get(tool)
            .map_or(false, |&count| count >= self.threshold)
    }

    /// Explicitly reset the denial counter for a tool.
    pub fn reset(&mut self, tool: &str) {
        self.counts.remove(tool);
    }

    /// Get the current denial count for a tool (0 if no denials recorded).
    pub fn denial_count(&self, tool: &str) -> u32 {
        self.counts.get(tool).copied().unwrap_or(0)
    }
}

impl Default for DenialTracker {
    fn default() -> Self {
        Self::new(3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tracker_no_escalation() {
        let tracker = DenialTracker::new(3);
        assert!(!tracker.should_escalate("bash"));
        assert!(!tracker.should_escalate("file_write"));
        assert_eq!(tracker.denial_count("bash"), 0);
    }

    #[test]
    fn denial_below_threshold() {
        let mut tracker = DenialTracker::new(3);
        tracker.record_denial("bash");
        tracker.record_denial("bash");
        assert!(!tracker.should_escalate("bash"));
        assert_eq!(tracker.denial_count("bash"), 2);
    }

    #[test]
    fn denial_at_threshold_escalates() {
        let mut tracker = DenialTracker::new(3);
        tracker.record_denial("bash");
        tracker.record_denial("bash");
        tracker.record_denial("bash");
        assert!(tracker.should_escalate("bash"));
        assert_eq!(tracker.denial_count("bash"), 3);
    }

    #[test]
    fn denial_above_threshold_still_escalates() {
        let mut tracker = DenialTracker::new(2);
        tracker.record_denial("bash");
        tracker.record_denial("bash");
        tracker.record_denial("bash"); // 3 > threshold 2
        assert!(tracker.should_escalate("bash"));
    }

    #[test]
    fn success_resets_counter() {
        let mut tracker = DenialTracker::new(3);
        tracker.record_denial("bash");
        tracker.record_denial("bash");
        tracker.record_denial("bash");
        assert!(tracker.should_escalate("bash"));

        tracker.record_success("bash");
        assert!(!tracker.should_escalate("bash"));
        assert_eq!(tracker.denial_count("bash"), 0);
    }

    #[test]
    fn independent_per_tool() {
        let mut tracker = DenialTracker::new(2);
        tracker.record_denial("bash");
        tracker.record_denial("bash");
        assert!(tracker.should_escalate("bash"));

        // Other tool unaffected
        assert!(!tracker.should_escalate("file_write"));
        tracker.record_denial("file_write");
        assert!(!tracker.should_escalate("file_write"));
    }

    #[test]
    fn reset_clears_counter() {
        let mut tracker = DenialTracker::new(2);
        tracker.record_denial("bash");
        tracker.record_denial("bash");
        assert!(tracker.should_escalate("bash"));

        tracker.reset("bash");
        assert!(!tracker.should_escalate("bash"));
        assert_eq!(tracker.denial_count("bash"), 0);
    }

    #[test]
    fn reset_nonexistent_tool_is_noop() {
        let mut tracker = DenialTracker::new(3);
        tracker.reset("nonexistent"); // should not panic
        assert!(!tracker.should_escalate("nonexistent"));
    }

    #[test]
    fn threshold_of_one() {
        let mut tracker = DenialTracker::new(1);
        assert!(!tracker.should_escalate("bash"));
        tracker.record_denial("bash");
        assert!(tracker.should_escalate("bash"));
    }

    #[test]
    fn default_threshold_is_three() {
        let tracker = DenialTracker::default();
        let mut t = DenialTracker::default();
        assert!(!tracker.should_escalate("x"));
        t.record_denial("x");
        t.record_denial("x");
        assert!(!t.should_escalate("x"));
        t.record_denial("x");
        assert!(t.should_escalate("x"));
    }
}
