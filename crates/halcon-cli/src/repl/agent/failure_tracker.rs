/// Circuit breaker for repeated tool failures.
///
/// Tracks (canonical_tool_name, error_pattern) pairs and trips when a threshold is reached.
/// Tool names are canonicalized via `tool_aliases::canonicalize` before tracking so that
/// aliased names (e.g. `read_file` and `file_read`) trip the same circuit.
/// Once tripped, the agent loop can:
/// 1. Inject a strong "stop retrying" directive to the model
/// 2. Skip replanning for deterministic failures
/// 3. Classify which tools the model should avoid
pub(crate) struct ToolFailureTracker {
    /// Map of (tool_name, error_pattern) → occurrence count.
    failures: std::collections::HashMap<(String, String), u32>,
    /// Number of identical failures before tripping the circuit.
    threshold: u32,
}

impl ToolFailureTracker {
    pub(crate) fn new(threshold: u32) -> Self {
        Self {
            failures: std::collections::HashMap::new(),
            threshold,
        }
    }

    /// Normalize error message into a classification pattern.
    /// Groups similar errors (e.g., different file paths but same "not found" error type).
    pub(crate) fn error_pattern(error: &str) -> String {
        let lower = error.to_lowercase();
        if lower.contains("no such file or directory")
            || lower.contains("not found")
            || lower.contains("do not exist")   // FASE-2 path-existence gate errors
            || lower.contains("does not exist")
        {
            "not_found".to_string()
        } else if lower.contains("permission denied") {
            "permission_denied".to_string()
        } else if lower.contains("is a directory") || lower.contains("not a directory") {
            "path_type_error".to_string()
        } else if lower.contains("path traversal") || lower.contains("blocked by security") {
            "security_blocked".to_string()
        } else if lower.contains("unknown tool") {
            "unknown_tool".to_string()
        } else if lower.contains("denied by task context") {
            "tbac_denied".to_string()
        } else if lower.contains("mcp pool call failed")
            || lower.contains("failed to call")
            || lower.contains("mcp server is not initialized")
            || (lower.contains("mcp") && lower.contains("not initialized"))
            || lower.contains("process start")
        {
            // MCP environment errors share a single pattern so the circuit breaker
            // trips after 3 MCP failures regardless of which server method fails.
            // NOTE: "not initialized" alone is intentionally NOT matched here — it is
            // too broad and would misclassify non-MCP errors such as
            // "Configuration not initialized" or "Database not initialized".
            "mcp_unavailable".to_string()
        } else {
            // Use first 80 chars as a generic key for unclassified errors.
            lower.chars().take(80).collect()
        }
    }

    /// Record a tool failure. Returns `true` if the circuit has tripped
    /// (i.e., this failure pattern has reached the threshold).
    /// Tool name is canonicalized so aliases (`read_file` ↔ `file_read`) share the same counter.
    pub(crate) fn record(&mut self, tool_name: &str, error: &str) -> bool {
        let canonical = super::tool_aliases::canonicalize(tool_name).to_string();
        let pattern = Self::error_pattern(error);
        let key = (canonical, pattern);
        let count = self.failures.entry(key).or_insert(0);
        *count += 1;
        *count >= self.threshold
    }

    /// Check if a specific tool+error combination has already tripped.
    /// Tool name is canonicalized for consistent lookup across aliases.
    pub(crate) fn is_tripped(&self, tool_name: &str, error: &str) -> bool {
        let canonical = super::tool_aliases::canonicalize(tool_name).to_string();
        let pattern = Self::error_pattern(error);
        let key = (canonical, pattern);
        self.failures.get(&key).copied().unwrap_or(0) >= self.threshold
    }

    /// Get the failure count for a specific (tool, error_pattern) key.
    /// Used for testing to inspect internal state.
    #[cfg(test)]
    pub(crate) fn failure_count(&self, tool_name: &str, error: &str) -> u32 {
        let canonical = super::tool_aliases::canonicalize(tool_name).to_string();
        let pattern = Self::error_pattern(error);
        let key = (canonical, pattern);
        self.failures.get(&key).copied().unwrap_or(0)
    }

    /// Reset the circuit breaker for a specific tool (all error patterns).
    ///
    /// Called when the root cause of repeated failures is confirmed resolved — e.g.,
    /// the operator granted missing permissions, a file was created, or an MCP server
    /// was restarted.  Without this, a once-tripped tool stays tripped for the entire
    /// session even if the environment recovers.
    pub(crate) fn reset_tool(&mut self, tool_name: &str) {
        let canonical = super::tool_aliases::canonicalize(tool_name);
        self.failures.retain(|(tool, _), _| tool != canonical);
    }

    /// Reset all circuit breakers (wipes every tool's failure history).
    ///
    /// Called at the start of a fresh retry attempt or when the overall environment
    /// is confirmed stable (e.g., all MCP servers reconnected successfully).
    pub(crate) fn reset_all(&mut self) {
        self.failures.clear();
    }

    /// Get all tripped tool names for directive injection.
    pub(crate) fn tripped_tools(&self) -> Vec<String> {
        let mut tools: Vec<String> = self
            .failures
            .iter()
            .filter(|(_, count)| **count >= self.threshold)
            .map(|((tool, _), _)| tool.clone())
            .collect();
        tools.sort();
        tools.dedup();
        tools
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Basic circuit breaker ──────────────────────────────────────────────

    #[test]
    fn trips_after_threshold() {
        let mut t = ToolFailureTracker::new(3);
        assert!(!t.record("bash", "permission denied"));
        assert!(!t.record("bash", "permission denied"));
        assert!(t.record("bash", "permission denied")); // 3rd — trips
        assert!(t.is_tripped("bash", "permission denied"));
    }

    #[test]
    fn does_not_trip_below_threshold() {
        let mut t = ToolFailureTracker::new(3);
        t.record("bash", "permission denied");
        t.record("bash", "permission denied");
        assert!(!t.is_tripped("bash", "permission denied"));
    }

    // ── MCP pattern classification ─────────────────────────────────────────

    #[test]
    fn mcp_pool_call_failed_classified_as_mcp_unavailable() {
        assert_eq!(
            ToolFailureTracker::error_pattern("MCP pool call failed: connection refused"),
            "mcp_unavailable"
        );
    }

    #[test]
    fn mcp_server_is_not_initialized_classified_correctly() {
        assert_eq!(
            ToolFailureTracker::error_pattern("mcp server is not initialized"),
            "mcp_unavailable"
        );
    }

    #[test]
    fn mcp_prefix_plus_not_initialized_classified_correctly() {
        assert_eq!(
            ToolFailureTracker::error_pattern("MCP transport not initialized"),
            "mcp_unavailable"
        );
    }

    #[test]
    fn non_mcp_not_initialized_does_not_classify_as_mcp() {
        // Regression: "Configuration not initialized" must NOT be classified as MCP.
        let pat = ToolFailureTracker::error_pattern("Configuration not initialized");
        assert_ne!(
            pat, "mcp_unavailable",
            "non-MCP 'not initialized' must not map to mcp_unavailable"
        );
    }

    #[test]
    fn database_not_initialized_does_not_classify_as_mcp() {
        let pat = ToolFailureTracker::error_pattern("Database not initialized");
        assert_ne!(pat, "mcp_unavailable");
    }

    // ── reset_tool ─────────────────────────────────────────────────────────

    #[test]
    fn reset_tool_clears_all_patterns_for_that_tool() {
        let mut t = ToolFailureTracker::new(2);
        t.record("bash", "permission denied");
        t.record("bash", "permission denied"); // tripped
        t.record("bash", "not found");
        assert!(t.is_tripped("bash", "permission denied"));

        t.reset_tool("bash");

        assert!(!t.is_tripped("bash", "permission denied"), "should be reset");
        assert_eq!(t.failure_count("bash", "not found"), 0, "all patterns cleared");
    }

    #[test]
    fn reset_tool_does_not_affect_other_tools() {
        let mut t = ToolFailureTracker::new(2);
        t.record("bash", "permission denied");
        t.record("bash", "permission denied"); // tripped
        t.record("grep", "permission denied");
        t.record("grep", "permission denied"); // also tripped

        t.reset_tool("bash");

        assert!(!t.is_tripped("bash", "permission denied"), "bash reset");
        assert!(t.is_tripped("grep", "permission denied"), "grep unchanged");
    }

    #[test]
    fn reset_tool_on_unknown_tool_is_noop() {
        let mut t = ToolFailureTracker::new(2);
        t.record("bash", "permission denied");
        t.reset_tool("unknown_tool"); // must not panic or corrupt state
        assert_eq!(t.failure_count("bash", "permission denied"), 1);
    }

    // ── reset_all ──────────────────────────────────────────────────────────

    #[test]
    fn reset_all_clears_every_tool() {
        let mut t = ToolFailureTracker::new(2);
        t.record("bash", "permission denied");
        t.record("bash", "permission denied");
        t.record("grep", "not found");
        t.record("grep", "not found");
        assert!(t.is_tripped("bash", "permission denied"));
        assert!(t.is_tripped("grep", "not found"));

        t.reset_all();

        assert!(!t.is_tripped("bash", "permission denied"), "bash reset");
        assert!(!t.is_tripped("grep", "not found"), "grep reset");
        assert!(t.tripped_tools().is_empty(), "no tripped tools after reset_all");
    }

    #[test]
    fn tripped_tools_empty_after_reset_all() {
        let mut t = ToolFailureTracker::new(1);
        t.record("tool_a", "error");
        t.record("tool_b", "error");
        assert_eq!(t.tripped_tools().len(), 2);

        t.reset_all();
        assert!(t.tripped_tools().is_empty());
    }
}
