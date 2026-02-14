//! Human-in-the-loop authorization middleware with policy chain, timeout,
//! and deny-always support.
//!
//! Architecture:
//! ```text
//! Tool Call → AuthorizationMiddleware.authorize()
//!   → Evaluate policy chain (NonInteractive → PermissionLevel → SessionMemory)
//!   → If all abstain → interactive stdin prompt with timeout
//!   → Returns PermissionDecision
//! ```

use std::collections::HashSet;
use std::io::{self, BufRead, Write};
use std::time::Duration;

use cuervo_core::types::{PermissionDecision, PermissionLevel, ToolInput};

// ---------------------------------------------------------------------------
// Policy trait
// ---------------------------------------------------------------------------

/// A single authorization policy in the chain.
///
/// `evaluate()` returns `Some(decision)` to short-circuit, or `None` to abstain
/// and let the next policy decide.
pub trait AuthorizationPolicy: Send + Sync {
    fn evaluate(
        &self,
        tool_name: &str,
        perm_level: PermissionLevel,
        input: &ToolInput,
        state: &AuthorizationState,
    ) -> Option<PermissionDecision>;

    fn name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Shared mutable state for session-level authorization decisions.
#[derive(Debug)]
pub struct AuthorizationState {
    /// Tools permanently allowed for this session (via "always" answer).
    pub always_allowed: HashSet<String>,
    /// Tools permanently denied for this session (via "deny always" answer).
    pub always_denied: HashSet<String>,
    /// Whether the session is interactive (has a TTY).
    pub interactive: bool,
}

impl AuthorizationState {
    pub fn new(interactive: bool) -> Self {
        Self {
            always_allowed: HashSet::new(),
            always_denied: HashSet::new(),
            interactive,
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in policies
// ---------------------------------------------------------------------------

/// Auto-allows everything when not running interactively (no TTY).
pub struct NonInteractivePolicy;

impl AuthorizationPolicy for NonInteractivePolicy {
    fn evaluate(
        &self,
        _tool_name: &str,
        _perm_level: PermissionLevel,
        _input: &ToolInput,
        state: &AuthorizationState,
    ) -> Option<PermissionDecision> {
        if !state.interactive {
            Some(PermissionDecision::Allowed)
        } else {
            None
        }
    }

    fn name(&self) -> &str {
        "NonInteractivePolicy"
    }
}

/// Auto-allows ReadOnly and ReadWrite tools (only Destructive needs prompting).
pub struct PermissionLevelPolicy;

impl AuthorizationPolicy for PermissionLevelPolicy {
    fn evaluate(
        &self,
        _tool_name: &str,
        perm_level: PermissionLevel,
        _input: &ToolInput,
        _state: &AuthorizationState,
    ) -> Option<PermissionDecision> {
        if perm_level < PermissionLevel::Destructive {
            Some(PermissionDecision::Allowed)
        } else {
            None
        }
    }

    fn name(&self) -> &str {
        "PermissionLevelPolicy"
    }
}

/// Checks session-level allow-always / deny-always sets.
pub struct SessionMemoryPolicy;

impl AuthorizationPolicy for SessionMemoryPolicy {
    fn evaluate(
        &self,
        tool_name: &str,
        _perm_level: PermissionLevel,
        _input: &ToolInput,
        state: &AuthorizationState,
    ) -> Option<PermissionDecision> {
        if state.always_denied.contains(tool_name) {
            Some(PermissionDecision::Denied)
        } else if state.always_allowed.contains(tool_name) {
            Some(PermissionDecision::AllowedAlways)
        } else {
            None
        }
    }

    fn name(&self) -> &str {
        "SessionMemoryPolicy"
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Composable authorization middleware with policy chain and interactive fallback.
pub struct AuthorizationMiddleware {
    policies: Vec<Box<dyn AuthorizationPolicy>>,
    pub state: AuthorizationState,
    pub prompt_timeout: Duration,
}

impl AuthorizationMiddleware {
    /// Create middleware with the default policy chain.
    pub fn new(interactive: bool, prompt_timeout_secs: u64) -> Self {
        let timeout = if prompt_timeout_secs == 0 {
            Duration::from_secs(u64::MAX) // effectively no timeout
        } else {
            Duration::from_secs(prompt_timeout_secs)
        };

        Self {
            policies: vec![
                Box::new(NonInteractivePolicy),
                Box::new(PermissionLevelPolicy),
                Box::new(SessionMemoryPolicy),
            ],
            state: AuthorizationState::new(interactive),
            prompt_timeout: timeout,
        }
    }

    /// Create middleware with custom policies (for testing).
    #[cfg(test)]
    pub fn with_policies(
        policies: Vec<Box<dyn AuthorizationPolicy>>,
        interactive: bool,
        prompt_timeout_secs: u64,
    ) -> Self {
        let timeout = if prompt_timeout_secs == 0 {
            Duration::from_secs(u64::MAX)
        } else {
            Duration::from_secs(prompt_timeout_secs)
        };

        Self {
            policies,
            state: AuthorizationState::new(interactive),
            prompt_timeout: timeout,
        }
    }

    /// Evaluate the policy chain. If all policies abstain, falls back to
    /// an interactive prompt with timeout.
    pub async fn authorize(
        &mut self,
        tool_name: &str,
        perm_level: PermissionLevel,
        input: &ToolInput,
    ) -> PermissionDecision {
        // Evaluate policy chain (synchronous — policies are cheap).
        for policy in &self.policies {
            if let Some(decision) = policy.evaluate(tool_name, perm_level, input, &self.state) {
                return decision;
            }
        }

        // All policies abstained → interactive prompt with timeout.
        let prompt = Self::format_prompt(tool_name, input);
        let timeout = self.prompt_timeout;

        let answer = tokio::time::timeout(timeout, tokio::task::spawn_blocking(move || {
            let mut stderr = io::stderr();
            if stderr.write_all(prompt.as_bytes()).is_err() || stderr.flush().is_err() {
                return String::new();
            }
            let mut line = String::new();
            let stdin = io::stdin();
            if stdin.lock().read_line(&mut line).is_err() {
                return String::new();
            }
            line.trim().to_lowercase()
        }))
        .await;

        let answer_str = match answer {
            Ok(Ok(s)) => s,
            Ok(Err(_join_err)) => {
                tracing::warn!("Permission prompt task panicked — denying");
                return PermissionDecision::Denied;
            }
            Err(_timeout) => {
                tracing::info!(
                    tool = %tool_name,
                    timeout_secs = timeout.as_secs(),
                    "Permission prompt timed out — denying (fail-safe)"
                );
                return PermissionDecision::Denied;
            }
        };

        self.apply_answer(tool_name, &answer_str)
    }

    /// Format the permission prompt string (with deny-always option).
    pub fn format_prompt(tool_name: &str, input: &ToolInput) -> String {
        let summary = summarize_input(tool_name, input);
        format!(
            "\nAllow {tool_name} [{summary}]? [y]es [n]o [a]lways [d]eny always: "
        )
    }

    /// Apply a user answer and update session state.
    pub fn apply_answer(&mut self, tool_name: &str, answer: &str) -> PermissionDecision {
        match answer {
            "y" | "yes" => PermissionDecision::Allowed,
            "a" | "always" => {
                self.state.always_allowed.insert(tool_name.to_string());
                PermissionDecision::AllowedAlways
            }
            "d" | "deny" => {
                self.state.always_denied.insert(tool_name.to_string());
                PermissionDecision::Denied
            }
            "n" | "no" => PermissionDecision::Denied,
            _ => {
                // Invalid/empty input → deny (fail-safe).
                PermissionDecision::Denied
            }
        }
    }

    /// Check if this tool+level would need an interactive prompt
    /// (i.e., all policies would abstain).
    pub fn needs_prompt(&self, tool_name: &str, perm_level: PermissionLevel) -> bool {
        let dummy_input = ToolInput {
            tool_use_id: String::new(),
            arguments: serde_json::Value::Null,
            working_directory: String::new(),
        };
        for policy in &self.policies {
            if policy
                .evaluate(tool_name, perm_level, &dummy_input, &self.state)
                .is_some()
            {
                return false;
            }
        }
        true
    }

    /// Decide without prompting (returns the first policy decision, or Denied).
    pub fn auto_decide(&self, tool_name: &str, perm_level: PermissionLevel) -> PermissionDecision {
        let dummy_input = ToolInput {
            tool_use_id: String::new(),
            arguments: serde_json::Value::Null,
            working_directory: String::new(),
        };
        for policy in &self.policies {
            if let Some(decision) =
                policy.evaluate(tool_name, perm_level, &dummy_input, &self.state)
            {
                return decision;
            }
        }
        PermissionDecision::Denied
    }

    /// Disable interactive mode (auto-allows everything via NonInteractivePolicy).
    pub fn set_non_interactive(&mut self) {
        self.state.interactive = false;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate a brief summary of the tool input for the permission prompt.
fn summarize_input(tool_name: &str, input: &ToolInput) -> String {
    match tool_name {
        "bash" => input.arguments["command"]
            .as_str()
            .map(|c| {
                if c.len() > 60 {
                    format!("{}...", &c[..57])
                } else {
                    c.to_string()
                }
            })
            .unwrap_or_else(|| "(unknown command)".into()),
        "file_write" | "file_edit" | "file_read" | "file_delete" => input.arguments["path"]
            .as_str()
            .unwrap_or("(unknown path)")
            .to_string(),
        _ => {
            let s = serde_json::to_string(&input.arguments).unwrap_or_default();
            if s.len() > 60 {
                format!("{}...", &s[..57])
            } else {
                s
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_input(args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    // --- NonInteractivePolicy ---

    #[test]
    fn non_interactive_auto_allows() {
        let state = AuthorizationState::new(false);
        let policy = NonInteractivePolicy;
        let input = dummy_input(serde_json::json!({}));
        let result = policy.evaluate("bash", PermissionLevel::Destructive, &input, &state);
        assert_eq!(result, Some(PermissionDecision::Allowed));
    }

    #[test]
    fn interactive_abstains() {
        let state = AuthorizationState::new(true);
        let policy = NonInteractivePolicy;
        let input = dummy_input(serde_json::json!({}));
        let result = policy.evaluate("bash", PermissionLevel::Destructive, &input, &state);
        assert_eq!(result, None);
    }

    // --- PermissionLevelPolicy ---

    #[test]
    fn readonly_auto_allowed() {
        let state = AuthorizationState::new(true);
        let policy = PermissionLevelPolicy;
        let input = dummy_input(serde_json::json!({}));
        let result = policy.evaluate("file_read", PermissionLevel::ReadOnly, &input, &state);
        assert_eq!(result, Some(PermissionDecision::Allowed));
    }

    #[test]
    fn readwrite_auto_allowed() {
        let state = AuthorizationState::new(true);
        let policy = PermissionLevelPolicy;
        let input = dummy_input(serde_json::json!({}));
        let result = policy.evaluate("file_edit", PermissionLevel::ReadWrite, &input, &state);
        assert_eq!(result, Some(PermissionDecision::Allowed));
    }

    #[test]
    fn destructive_needs_prompt() {
        let state = AuthorizationState::new(true);
        let policy = PermissionLevelPolicy;
        let input = dummy_input(serde_json::json!({}));
        let result = policy.evaluate("bash", PermissionLevel::Destructive, &input, &state);
        assert_eq!(result, None);
    }

    // --- SessionMemoryPolicy ---

    #[test]
    fn session_allow_always() {
        let mut state = AuthorizationState::new(true);
        state.always_allowed.insert("bash".to_string());
        let policy = SessionMemoryPolicy;
        let input = dummy_input(serde_json::json!({}));
        let result = policy.evaluate("bash", PermissionLevel::Destructive, &input, &state);
        assert_eq!(result, Some(PermissionDecision::AllowedAlways));
    }

    #[test]
    fn session_deny_always() {
        let mut state = AuthorizationState::new(true);
        state.always_denied.insert("bash".to_string());
        let policy = SessionMemoryPolicy;
        let input = dummy_input(serde_json::json!({}));
        let result = policy.evaluate("bash", PermissionLevel::Destructive, &input, &state);
        assert_eq!(result, Some(PermissionDecision::Denied));
    }

    #[test]
    fn session_deny_takes_priority_over_allow() {
        // If both sets contain the tool, deny wins.
        let mut state = AuthorizationState::new(true);
        state.always_denied.insert("bash".to_string());
        state.always_allowed.insert("bash".to_string());
        let policy = SessionMemoryPolicy;
        let input = dummy_input(serde_json::json!({}));
        let result = policy.evaluate("bash", PermissionLevel::Destructive, &input, &state);
        assert_eq!(result, Some(PermissionDecision::Denied));
    }

    #[test]
    fn session_unknown_tool_abstains() {
        let state = AuthorizationState::new(true);
        let policy = SessionMemoryPolicy;
        let input = dummy_input(serde_json::json!({}));
        let result = policy.evaluate("bash", PermissionLevel::Destructive, &input, &state);
        assert_eq!(result, None);
    }

    // --- apply_answer ---

    #[test]
    fn apply_answer_yes() {
        let mut mw = AuthorizationMiddleware::new(true, 30);
        let result = mw.apply_answer("bash", "y");
        assert_eq!(result, PermissionDecision::Allowed);
        assert!(!mw.state.always_allowed.contains("bash"));
    }

    #[test]
    fn apply_answer_yes_full() {
        let mut mw = AuthorizationMiddleware::new(true, 30);
        let result = mw.apply_answer("bash", "yes");
        assert_eq!(result, PermissionDecision::Allowed);
    }

    #[test]
    fn apply_answer_always() {
        let mut mw = AuthorizationMiddleware::new(true, 30);
        let result = mw.apply_answer("bash", "a");
        assert_eq!(result, PermissionDecision::AllowedAlways);
        assert!(mw.state.always_allowed.contains("bash"));
    }

    #[test]
    fn apply_answer_deny_always() {
        let mut mw = AuthorizationMiddleware::new(true, 30);
        let result = mw.apply_answer("bash", "d");
        assert_eq!(result, PermissionDecision::Denied);
        assert!(mw.state.always_denied.contains("bash"));
    }

    #[test]
    fn apply_answer_deny_always_full() {
        let mut mw = AuthorizationMiddleware::new(true, 30);
        let result = mw.apply_answer("bash", "deny");
        assert_eq!(result, PermissionDecision::Denied);
        assert!(mw.state.always_denied.contains("bash"));
    }

    #[test]
    fn apply_answer_no() {
        let mut mw = AuthorizationMiddleware::new(true, 30);
        let result = mw.apply_answer("bash", "n");
        assert_eq!(result, PermissionDecision::Denied);
        // "n" is single-deny, NOT deny-always.
        assert!(!mw.state.always_denied.contains("bash"));
    }

    #[test]
    fn apply_answer_invalid_failsafe() {
        let mut mw = AuthorizationMiddleware::new(true, 30);
        // Empty input.
        assert_eq!(mw.apply_answer("bash", ""), PermissionDecision::Denied);
        // Garbage input.
        assert_eq!(mw.apply_answer("bash", "maybe"), PermissionDecision::Denied);
        // Neither should add to deny-always.
        assert!(!mw.state.always_denied.contains("bash"));
    }

    // --- Policy chain order ---

    #[test]
    fn policy_chain_order_first_wins() {
        // Custom policy that always denies.
        struct AlwaysDenyPolicy;
        impl AuthorizationPolicy for AlwaysDenyPolicy {
            fn evaluate(
                &self,
                _: &str,
                _: PermissionLevel,
                _: &ToolInput,
                _: &AuthorizationState,
            ) -> Option<PermissionDecision> {
                Some(PermissionDecision::Denied)
            }
            fn name(&self) -> &str {
                "AlwaysDeny"
            }
        }

        // AlwaysDeny first → should deny even for ReadOnly.
        let mw = AuthorizationMiddleware::with_policies(
            vec![Box::new(AlwaysDenyPolicy), Box::new(PermissionLevelPolicy)],
            true,
            30,
        );

        let decision = mw.auto_decide("file_read", PermissionLevel::ReadOnly);
        assert_eq!(decision, PermissionDecision::Denied);
    }

    #[test]
    fn policy_chain_second_wins_when_first_abstains() {
        // Custom policy that always abstains.
        struct AbstainPolicy;
        impl AuthorizationPolicy for AbstainPolicy {
            fn evaluate(
                &self,
                _: &str,
                _: PermissionLevel,
                _: &ToolInput,
                _: &AuthorizationState,
            ) -> Option<PermissionDecision> {
                None
            }
            fn name(&self) -> &str {
                "Abstain"
            }
        }

        let mw = AuthorizationMiddleware::with_policies(
            vec![Box::new(AbstainPolicy), Box::new(PermissionLevelPolicy)],
            true,
            30,
        );

        let decision = mw.auto_decide("file_read", PermissionLevel::ReadOnly);
        assert_eq!(decision, PermissionDecision::Allowed);
    }

    // --- Prompt format ---

    #[test]
    fn prompt_format_includes_deny_always() {
        let input = dummy_input(serde_json::json!({"command": "rm -rf /tmp/test"}));
        let prompt = AuthorizationMiddleware::format_prompt("bash", &input);
        assert!(prompt.contains("[d]eny always"), "prompt: {prompt}");
        assert!(prompt.contains("[y]es"));
        assert!(prompt.contains("[n]o"));
        assert!(prompt.contains("[a]lways"));
        assert!(prompt.contains("rm -rf /tmp/test"));
    }

    // --- needs_prompt / auto_decide ---

    #[test]
    fn needs_prompt_readonly_false() {
        let mw = AuthorizationMiddleware::new(true, 30);
        assert!(!mw.needs_prompt("file_read", PermissionLevel::ReadOnly));
    }

    #[test]
    fn needs_prompt_destructive_true() {
        let mw = AuthorizationMiddleware::new(true, 30);
        assert!(mw.needs_prompt("bash", PermissionLevel::Destructive));
    }

    #[test]
    fn needs_prompt_after_always_false() {
        let mut mw = AuthorizationMiddleware::new(true, 30);
        mw.state.always_allowed.insert("bash".to_string());
        assert!(!mw.needs_prompt("bash", PermissionLevel::Destructive));
    }

    #[test]
    fn needs_prompt_after_deny_always_false() {
        let mut mw = AuthorizationMiddleware::new(true, 30);
        mw.state.always_denied.insert("bash".to_string());
        assert!(!mw.needs_prompt("bash", PermissionLevel::Destructive));
    }

    #[test]
    fn needs_prompt_non_interactive_false() {
        let mw = AuthorizationMiddleware::new(false, 30);
        assert!(!mw.needs_prompt("bash", PermissionLevel::Destructive));
    }

    #[test]
    fn auto_decide_readonly_allowed() {
        let mw = AuthorizationMiddleware::new(true, 30);
        assert_eq!(
            mw.auto_decide("file_read", PermissionLevel::ReadOnly),
            PermissionDecision::Allowed
        );
    }

    #[test]
    fn auto_decide_destructive_denied_no_session_memory() {
        let mw = AuthorizationMiddleware::new(true, 30);
        // All policies abstain for Destructive interactive → Denied fallback.
        assert_eq!(
            mw.auto_decide("bash", PermissionLevel::Destructive),
            PermissionDecision::Denied
        );
    }

    #[test]
    fn set_non_interactive_auto_allows() {
        let mut mw = AuthorizationMiddleware::new(true, 30);
        assert!(mw.needs_prompt("bash", PermissionLevel::Destructive));
        mw.set_non_interactive();
        assert!(!mw.needs_prompt("bash", PermissionLevel::Destructive));
        assert_eq!(
            mw.auto_decide("bash", PermissionLevel::Destructive),
            PermissionDecision::Allowed
        );
    }

    // --- Custom policy integration ---

    #[test]
    fn custom_policy_integration() {
        struct OnlyAllowFileRead;
        impl AuthorizationPolicy for OnlyAllowFileRead {
            fn evaluate(
                &self,
                tool_name: &str,
                _: PermissionLevel,
                _: &ToolInput,
                _: &AuthorizationState,
            ) -> Option<PermissionDecision> {
                if tool_name == "file_read" {
                    Some(PermissionDecision::Allowed)
                } else {
                    Some(PermissionDecision::Denied)
                }
            }
            fn name(&self) -> &str {
                "OnlyAllowFileRead"
            }
        }

        let mw = AuthorizationMiddleware::with_policies(
            vec![Box::new(OnlyAllowFileRead)],
            true,
            30,
        );

        assert_eq!(
            mw.auto_decide("file_read", PermissionLevel::Destructive),
            PermissionDecision::Allowed
        );
        assert_eq!(
            mw.auto_decide("bash", PermissionLevel::Destructive),
            PermissionDecision::Denied
        );
    }

    // --- Timeout behavior (simulated via authorize) ---

    #[tokio::test]
    async fn timeout_returns_denied() {
        // Create middleware with 1-second timeout.
        // Since we're in a test environment without stdin, spawn_blocking will
        // block indefinitely on stdin.lock().read_line(), so timeout fires.
        let mut mw = AuthorizationMiddleware::new(true, 1);
        let input = dummy_input(serde_json::json!({"command": "test"}));
        let decision = mw
            .authorize("bash", PermissionLevel::Destructive, &input)
            .await;
        assert_eq!(decision, PermissionDecision::Denied);
    }

    // --- Summarize ---

    #[test]
    fn summarize_bash_command() {
        let input = dummy_input(serde_json::json!({"command": "echo hello"}));
        assert_eq!(summarize_input("bash", &input), "echo hello");
    }

    #[test]
    fn summarize_file_path() {
        let input = dummy_input(serde_json::json!({"path": "src/main.rs"}));
        assert_eq!(summarize_input("file_read", &input), "src/main.rs");
    }

    #[test]
    fn summarize_file_delete() {
        let input = dummy_input(serde_json::json!({"path": "/tmp/test.txt"}));
        assert_eq!(summarize_input("file_delete", &input), "/tmp/test.txt");
    }

    #[test]
    fn summarize_truncates_long_command() {
        let long_cmd = "a".repeat(100);
        let input = dummy_input(serde_json::json!({"command": long_cmd}));
        let summary = summarize_input("bash", &input);
        assert!(summary.len() <= 63);
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn zero_timeout_means_no_timeout() {
        let mw = AuthorizationMiddleware::new(true, 0);
        // u64::MAX seconds is effectively "no timeout".
        assert_eq!(mw.prompt_timeout, Duration::from_secs(u64::MAX));
    }
}
