//! Transient cache and session state bundle.
//!
//! Phase 3.1: Groups all mutable session-scoped cache that accumulates across
//! messages. 5 fields from original Repl struct.

use std::collections::HashMap;

use halcon_core::types::DryRunMode;
use halcon_storage::TraceStep;

/// Mutable transient state that accumulates across messages.
///
/// Session-scoped cache that mutates over time but is not persisted to database.
/// Cleared on session restart.
pub struct ReplCacheState {
    /// Trace step cursor for /step forward/back navigation.
    /// Tuple: (session_id, trace_steps, current_index).
    pub trace_cursor: Option<(uuid::Uuid, Vec<TraceStep>, usize)>,

    /// Cached execution timeline JSON from the last agent loop (for --timeline flag).
    pub last_timeline: Option<String>,

    /// Phase 3: Model quality stats cache for session-level persistence.
    ///
    /// Snapshot of `ModelSelector.quality_stats` extracted after each agent loop and re-injected
    /// into the next fresh `ModelSelector` via `with_quality_seeds()`. This ensures `balanced`
    /// routing uses accumulated quality data across all messages within the session, not just
    /// the current message (previously reset to neutral every turn because ModelSelector is
    /// created fresh per message). Tuple: `(success_count, failure_count, total_reward)`.
    pub model_quality: HashMap<String, (u32, u32, f64)>,

    /// Temporary dry-run mode override for the next handle_message call.
    pub dry_run_override: Option<DryRunMode>,

    /// BRECHA-S3: Tools blocked during this session (name, reason).
    ///
    /// Accumulated from `state.blocked_tools` after each agent loop. Persists
    /// across turns so the LlmPlanner can avoid generating steps with blocked tools.
    /// Invariant: if a tool was blocked in turn N, the plan for turn N+1 excludes it.
    pub session_blocked_tools: Vec<(String, String)>,
}

impl ReplCacheState {
    /// Construct cache state with default empty values.
    pub fn new() -> Self {
        Self {
            trace_cursor: None,
            last_timeline: None,
            model_quality: HashMap::new(),
            dry_run_override: None,
            session_blocked_tools: Vec::new(),
        }
    }

    /// Check if trace navigation is available.
    pub fn has_trace_cursor(&self) -> bool {
        self.trace_cursor.is_some()
    }

    /// Check if timeline is cached.
    pub fn has_timeline(&self) -> bool {
        self.last_timeline.is_some()
    }

    /// Check if any tools are blocked.
    pub fn has_blocked_tools(&self) -> bool {
        !self.session_blocked_tools.is_empty()
    }

    /// Add a blocked tool to the session list.
    pub fn block_tool(&mut self, name: String, reason: String) {
        // Deduplicate - only add if not already blocked
        if !self.session_blocked_tools.iter().any(|(n, _)| n == &name) {
            self.session_blocked_tools.push((name, reason));
        }
    }

    /// Clear all cache state (for session reset).
    pub fn clear(&mut self) {
        self.trace_cursor = None;
        self.last_timeline = None;
        self.model_quality.clear();
        self.dry_run_override = None;
        self.session_blocked_tools.clear();
    }
}

impl Default for ReplCacheState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_state_default_is_empty() {
        let cache = ReplCacheState::default();

        assert!(!cache.has_trace_cursor());
        assert!(!cache.has_timeline());
        assert!(!cache.has_blocked_tools());
        assert!(cache.model_quality.is_empty());
    }

    #[test]
    fn cache_state_block_tool_deduplicates() {
        let mut cache = ReplCacheState::new();

        cache.block_tool("bash".to_string(), "security".to_string());
        assert_eq!(cache.session_blocked_tools.len(), 1);

        // Blocking same tool again should not duplicate
        cache.block_tool("bash".to_string(), "different reason".to_string());
        assert_eq!(cache.session_blocked_tools.len(), 1);

        // Different tool should be added
        cache.block_tool("file_write".to_string(), "permission".to_string());
        assert_eq!(cache.session_blocked_tools.len(), 2);
    }

    #[test]
    fn cache_state_clear_resets_all_fields() {
        let mut cache = ReplCacheState::new();

        // Populate cache
        cache.last_timeline = Some("timeline_json".to_string());
        cache
            .model_quality
            .insert("model1".to_string(), (5, 2, 0.8));
        cache.block_tool("bash".to_string(), "test".to_string());

        assert!(cache.has_timeline());
        assert!(!cache.model_quality.is_empty());
        assert!(cache.has_blocked_tools());

        // Clear should reset everything
        cache.clear();

        assert!(!cache.has_timeline());
        assert!(cache.model_quality.is_empty());
        assert!(!cache.has_blocked_tools());
    }
}
