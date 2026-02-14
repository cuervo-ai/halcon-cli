/// What action the tool loop guard recommends after a tool round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopAction {
    /// Normal — proceed to next round.
    Continue,
    /// Consecutive tool rounds >= synthesis_threshold → inject synthesis directive.
    InjectSynthesis,
    /// Consecutive tool rounds >= force_threshold → remove tools from request.
    ForceNoTools,
    /// Oscillation detected or plan complete → stop now.
    Break,
}

/// Intelligent multi-layered tool loop termination guard.
///
/// Replaces the blunt `consecutive_tool_rounds >= 5` counter with pattern
/// detection: oscillation (A→B→A→B), read saturation (3+ rounds of only
/// ReadOnly tools), deduplication, and graduated escalation (synthesis
/// directive → forced tool withdrawal → break).
pub(crate) struct ToolLoopGuard {
    /// Per-round tool call log: Vec<Vec<(tool_name, args_hash)>>.
    history: Vec<Vec<(String, u64)>>,
    /// Consecutive tool-use rounds.
    consecutive_rounds: usize,
    /// Threshold for synthesis directive injection (default 3).
    synthesis_threshold: usize,
    /// Threshold for forced tool withdrawal (default 4).
    force_threshold: usize,
    /// Whether plan completion has been signaled.
    plan_complete: bool,
}

/// Known read-only tool names. Tools in this set gather information but don't
/// modify state — sustained use signals the model is exploring without converging.
const READ_ONLY_TOOLS: &[&str] = &[
    "file_read",
    "glob",
    "grep",
    "directory_tree",
    "git_status",
    "git_diff",
    "git_log",
    "fuzzy_find",
    "symbol_search",
    "file_inspect",
    "web_search",
    "web_fetch",
];

impl ToolLoopGuard {
    pub(crate) fn new() -> Self {
        Self {
            history: Vec::new(),
            consecutive_rounds: 0,
            synthesis_threshold: 3,
            force_threshold: 4,
            plan_complete: false,
        }
    }

    /// Record a completed tool round and return the recommended action.
    pub(crate) fn record_round(&mut self, tools: &[(String, u64)]) -> LoopAction {
        self.history.push(tools.to_vec());
        self.consecutive_rounds += 1;

        // Plan complete → force synthesis immediately.
        if self.plan_complete {
            return LoopAction::Break;
        }

        // Oscillation detection takes priority — stop immediately.
        if self.detect_oscillation() {
            return LoopAction::Break;
        }

        // Graduated escalation based on consecutive rounds.
        if self.consecutive_rounds >= self.force_threshold {
            return LoopAction::ForceNoTools;
        }
        if self.consecutive_rounds >= self.synthesis_threshold {
            // Read saturation amplifies urgency — but still InjectSynthesis at this stage.
            return LoopAction::InjectSynthesis;
        }

        LoopAction::Continue
    }

    /// Detect oscillation patterns: A→B→A→B or A→A→A (3+ identical rounds).
    pub(crate) fn detect_oscillation(&self) -> bool {
        let len = self.history.len();
        if len < 3 {
            return false;
        }

        // Check A→A→A: 3 consecutive identical tool sets.
        let last = &self.history[len - 1];
        let prev1 = &self.history[len - 2];
        let prev2 = &self.history[len - 3];
        if last == prev1 && prev1 == prev2 {
            return true;
        }

        // Check A→B→A→B: alternating pattern over 4 rounds.
        if len >= 4 {
            let prev3 = &self.history[len - 4];
            if last == prev2 && prev1 == prev3 && last != prev1 {
                return true;
            }
        }

        false
    }

    /// Detect read saturation: 3+ consecutive rounds using only read-only tools.
    pub(crate) fn detect_read_saturation(&self) -> bool {
        if self.history.len() < 3 {
            return false;
        }
        let recent = &self.history[self.history.len().saturating_sub(3)..];
        recent.iter().all(|round| {
            !round.is_empty()
                && round
                    .iter()
                    .all(|(name, _)| READ_ONLY_TOOLS.contains(&name.as_str()))
        })
    }

    /// Check if this exact (tool_name, args_hash) was already executed in any prior round.
    pub(crate) fn is_duplicate(&self, tool_name: &str, args_hash: u64) -> bool {
        // Check all rounds except the current one being built (last element, if any).
        // At the point of dedup checking, the current round hasn't been recorded yet,
        // so we check all of self.history.
        self.history.iter().any(|round| {
            round
                .iter()
                .any(|(name, hash)| name == tool_name && *hash == args_hash)
        })
    }

    /// Signal that all plan steps have been completed.
    pub(crate) fn force_synthesis(&mut self) {
        self.plan_complete = true;
    }

    /// Get the current consecutive tool round count.
    pub(crate) fn consecutive_rounds(&self) -> usize {
        self.consecutive_rounds
    }

    /// Whether plan_complete was signaled.
    pub(crate) fn plan_complete(&self) -> bool {
        self.plan_complete
    }
}

/// Compute a deterministic hash of a serde_json::Value for dedup purposes.
pub(crate) fn hash_tool_args(value: &serde_json::Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Canonical JSON string for deterministic hashing.
    let canonical = value.to_string();
    canonical.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
pub(crate) use tests::READ_ONLY_TOOLS_LIST;

#[cfg(test)]
mod tests {
    use super::*;

    /// Expose READ_ONLY_TOOLS for test assertions in other modules.
    pub(crate) const READ_ONLY_TOOLS_LIST: &[&str] = READ_ONLY_TOOLS;
}
