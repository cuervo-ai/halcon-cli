/// Circuit breaker for repeated tool failures.
///
/// Tracks (tool_name, error_pattern) pairs and trips when a threshold is reached.
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
        if lower.contains("no such file or directory") || lower.contains("not found") {
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
        } else {
            // Use first 80 chars as a generic key for unclassified errors.
            lower.chars().take(80).collect()
        }
    }

    /// Record a tool failure. Returns `true` if the circuit has tripped
    /// (i.e., this failure pattern has reached the threshold).
    pub(crate) fn record(&mut self, tool_name: &str, error: &str) -> bool {
        let pattern = Self::error_pattern(error);
        let key = (tool_name.to_string(), pattern);
        let count = self.failures.entry(key).or_insert(0);
        *count += 1;
        *count >= self.threshold
    }

    /// Check if a specific tool+error combination has already tripped.
    pub(crate) fn is_tripped(&self, tool_name: &str, error: &str) -> bool {
        let pattern = Self::error_pattern(error);
        let key = (tool_name.to_string(), pattern);
        self.failures.get(&key).copied().unwrap_or(0) >= self.threshold
    }

    /// Get the failure count for a specific (tool, error_pattern) key.
    /// Used for testing to inspect internal state.
    #[cfg(test)]
    pub(crate) fn failure_count(&self, tool_name: &str, error: &str) -> u32 {
        let pattern = Self::error_pattern(error);
        let key = (tool_name.to_string(), pattern);
        self.failures.get(&key).copied().unwrap_or(0)
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
