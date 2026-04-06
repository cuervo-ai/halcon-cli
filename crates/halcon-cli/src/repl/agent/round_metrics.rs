//! RoundMetrics — Per-round accumulated statistics that reset between rounds.
//!
//! Phase 2.2: This struct groups fields that are cleared/reset at the start
//! of each agent loop round. Transient state for a single round.

use super::loop_state::TokenAccounting;

/// Per-round metrics that reset at the start of each round.
///
/// Maximum 15 fields (Phase 2 constraint). Focused on transient state that
/// accumulates during a round and is cleared before the next round begins.
#[derive(Debug, Default)]
pub(super) struct RoundMetrics {
    /// Accumulated text output from the model in this round.
    pub full_text: String,

    /// Token usage tracking for this round (existing sub-struct).
    pub tokens: TokenAccounting,

    /// Trace step cursor (incremented after each trace write).
    pub trace_step_index: u32,

    /// Tools successfully executed in this round (tool names).
    pub tools_executed: Vec<String>,

    /// Model name used in the last round (for comparison).
    pub last_round_model_name: String,

    /// Counter for NextRound restarts (phase outcome tracking).
    pub next_round_restarts: usize,

    /// Dynamic tool trust scorer (updated per-tool execution).
    pub tool_trust: super::super::tool_trust::ToolTrustScorer,
}

impl RoundMetrics {
    /// Reset transient fields at the start of a new round.
    ///
    /// Called by the agent loop before each round begins. Preserves
    /// `last_round_model_name` for comparison. Clears accumulated state.
    pub(super) fn reset(&mut self) {
        self.full_text.clear();
        self.trace_step_index = 0;
        self.tools_executed.clear();
        // last_round_model_name is preserved for cross-round comparison
        self.next_round_restarts = 0;
        // tool_trust is NOT reset — scores accumulate across rounds
    }

    /// Update the last round model name (called after round completion).
    pub(super) fn set_last_model(&mut self, model_name: String) {
        self.last_round_model_name = model_name;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_metrics_default() {
        let metrics = RoundMetrics::default();
        assert!(metrics.full_text.is_empty());
        assert_eq!(metrics.trace_step_index, 0);
        assert!(metrics.tools_executed.is_empty());
        assert_eq!(metrics.next_round_restarts, 0);
    }

    #[test]
    fn round_metrics_reset_clears_transient_state() {
        let mut metrics = RoundMetrics::default();

        // Simulate round accumulation
        metrics.full_text = "Some output".to_string();
        metrics.trace_step_index = 5;
        metrics.tools_executed.push("bash".to_string());
        metrics.tools_executed.push("file_read".to_string());
        metrics.next_round_restarts = 2;
        metrics.last_round_model_name = "gpt-4".to_string();

        // Reset for next round
        metrics.reset();

        // Transient state cleared
        assert!(metrics.full_text.is_empty());
        assert_eq!(metrics.trace_step_index, 0);
        assert!(metrics.tools_executed.is_empty());
        assert_eq!(metrics.next_round_restarts, 0);

        // last_round_model_name preserved
        assert_eq!(metrics.last_round_model_name, "gpt-4");
    }

    #[test]
    fn round_metrics_set_last_model() {
        let mut metrics = RoundMetrics::default();
        assert!(metrics.last_round_model_name.is_empty());

        metrics.set_last_model("claude-3-opus".to_string());
        assert_eq!(metrics.last_round_model_name, "claude-3-opus");
    }
}
