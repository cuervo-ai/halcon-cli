use std::collections::HashSet;

use cuervo_core::types::{AuthzDecision, PermissionDecision, PermissionLevel, TaskContext, ToolInput};

/// Checks tool permissions and prompts the user for confirmation when needed.
///
/// Extended with TBAC (Task-Based Authorization Control) support:
/// when `tbac_enabled` is true and a task context is active,
/// `check_tbac()` gates tool access to the context's allowlist
/// and parameter constraints.
pub struct PermissionChecker {
    /// Tools that have been permanently allowed for this session.
    always_allowed: HashSet<String>,
    /// Whether to prompt for destructive tools.
    confirm_destructive: bool,
    /// Active task context stack (innermost = most restrictive).
    task_contexts: Vec<TaskContext>,
    /// Whether TBAC is enabled.
    tbac_enabled: bool,
}

impl PermissionChecker {
    #[allow(dead_code)] // Used by tests in permissions.rs + agent.rs.
    pub fn new(confirm_destructive: bool) -> Self {
        Self::with_tbac(confirm_destructive, false)
    }

    /// Create a new PermissionChecker with TBAC support.
    pub fn with_tbac(confirm_destructive: bool, tbac_enabled: bool) -> Self {
        Self {
            always_allowed: HashSet::new(),
            confirm_destructive,
            task_contexts: Vec::new(),
            tbac_enabled,
        }
    }

    /// Disable interactive prompts (for non-interactive single-shot mode).
    ///
    /// When there is no TTY for user input, tools that would normally require
    /// confirmation are auto-approved instead of hanging on stdin.
    pub fn set_non_interactive(&mut self) {
        self.confirm_destructive = false;
    }

    /// Push a new task context (enters a scoped authorization).
    pub fn push_context(&mut self, ctx: TaskContext) {
        self.task_contexts.push(ctx);
    }

    /// Pop the current task context (exits scoped authorization).
    pub fn pop_context(&mut self) -> Option<TaskContext> {
        self.task_contexts.pop()
    }

    /// Get the active (innermost) task context.
    pub fn active_context(&self) -> Option<&TaskContext> {
        self.task_contexts.last()
    }

    /// Check TBAC authorization. Returns NoContext if TBAC disabled or no context active.
    pub fn check_tbac(&mut self, tool_name: &str, args: &serde_json::Value) -> AuthzDecision {
        if !self.tbac_enabled {
            return AuthzDecision::NoContext;
        }

        let Some(ctx) = self.task_contexts.last_mut() else {
            return AuthzDecision::NoContext;
        };

        if !ctx.is_valid() {
            return AuthzDecision::ContextInvalid {
                context_id: ctx.context_id,
                reason: "expired or exhausted".into(),
            };
        }

        if !ctx.is_tool_allowed(tool_name) {
            return AuthzDecision::ToolNotAllowed {
                tool: tool_name.into(),
                context_id: ctx.context_id,
            };
        }

        if !ctx.check_params(tool_name, args) {
            return AuthzDecision::ParamViolation {
                tool: tool_name.into(),
                constraint: format!("{:?}", ctx.parameter_constraints.get(tool_name)),
            };
        }

        ctx.consume_invocation();

        AuthzDecision::Allowed {
            context_id: ctx.context_id,
        }
    }

    /// Check if a tool execution should be allowed (convenience for sync callers/tests).
    ///
    /// - ReadOnly tools: always allowed.
    /// - ReadWrite tools: always allowed.
    /// - Destructive tools: prompt unless already in `always_allowed` or confirmation is disabled.
    #[cfg(test)]
    pub fn check(
        &mut self,
        tool_name: &str,
        permission_level: PermissionLevel,
        input: &ToolInput,
        reader: &mut dyn std::io::BufRead,
        writer: &mut dyn std::io::Write,
    ) -> std::io::Result<PermissionDecision> {
        if !self.needs_prompt(tool_name, permission_level) {
            return Ok(self.auto_decide(tool_name, permission_level));
        }

        // Prompt the user.
        let prompt = Self::format_prompt(tool_name, input);
        write!(writer, "{}", prompt)?;
        writer.flush()?;

        let mut line = String::new();
        reader.read_line(&mut line)?;
        let answer = line.trim().to_lowercase();

        Ok(self.apply_answer(tool_name, &answer))
    }

    /// Returns true if this tool+level combination requires a user prompt.
    pub fn needs_prompt(&self, tool_name: &str, permission_level: PermissionLevel) -> bool {
        if permission_level < PermissionLevel::Destructive {
            return false;
        }
        if !self.confirm_destructive {
            return false;
        }
        if self.always_allowed.contains(tool_name) {
            return false;
        }
        true
    }

    /// Decide without prompting (for auto-allowed tools).
    pub fn auto_decide(&self, tool_name: &str, permission_level: PermissionLevel) -> PermissionDecision {
        if permission_level < PermissionLevel::Destructive || !self.confirm_destructive {
            PermissionDecision::Allowed
        } else if self.always_allowed.contains(tool_name) {
            PermissionDecision::AllowedAlways
        } else {
            // Shouldn't be called when prompt is needed, but deny as safe default.
            PermissionDecision::Denied
        }
    }

    /// Format the permission prompt string.
    pub fn format_prompt(tool_name: &str, input: &ToolInput) -> String {
        let summary = summarize_input(tool_name, input);
        format!(
            "\nAllow {tool_name} [{summary}]? [y]es [n]o [a]lways: "
        )
    }

    /// Apply a user answer and update internal state.
    pub fn apply_answer(&mut self, tool_name: &str, answer: &str) -> PermissionDecision {
        match answer {
            "y" | "yes" => PermissionDecision::Allowed,
            "a" | "always" => {
                self.always_allowed.insert(tool_name.to_string());
                PermissionDecision::AllowedAlways
            }
            _ => PermissionDecision::Denied,
        }
    }
}

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
        "file_write" | "file_edit" | "file_read" => input.arguments["path"]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn dummy_input(args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[test]
    fn readonly_auto_allowed() {
        let mut checker = PermissionChecker::new(true);
        let input = dummy_input(serde_json::json!({}));
        let mut reader = io::Cursor::new(b"");
        let mut writer = Vec::new();

        let result = checker
            .check(
                "file_read",
                PermissionLevel::ReadOnly,
                &input,
                &mut reader,
                &mut writer,
            )
            .unwrap();
        assert_eq!(result, PermissionDecision::Allowed);
        // No prompt should have been written.
        assert!(writer.is_empty());
    }

    #[test]
    fn readwrite_auto_allowed() {
        let mut checker = PermissionChecker::new(true);
        let input = dummy_input(serde_json::json!({}));
        let mut reader = io::Cursor::new(b"");
        let mut writer = Vec::new();

        // file_edit is ReadWrite — auto-allowed, no prompt
        let result = checker
            .check(
                "file_edit",
                PermissionLevel::ReadWrite,
                &input,
                &mut reader,
                &mut writer,
            )
            .unwrap();
        assert_eq!(result, PermissionDecision::Allowed);
    }

    #[test]
    fn file_write_destructive_prompts() {
        let mut checker = PermissionChecker::new(true);
        let input = dummy_input(serde_json::json!({ "path": "/tmp/test.txt" }));
        let mut reader = io::Cursor::new(b"y\n");
        let mut writer = Vec::new();

        // file_write is Destructive — requires prompt
        let result = checker
            .check(
                "file_write",
                PermissionLevel::Destructive,
                &input,
                &mut reader,
                &mut writer,
            )
            .unwrap();
        assert_eq!(result, PermissionDecision::Allowed);
        let prompt = String::from_utf8(writer).unwrap();
        assert!(prompt.contains("Allow file_write"), "Must show consent prompt");
        assert!(prompt.contains("/tmp/test.txt"), "Must show file path");
    }

    #[test]
    fn destructive_prompts_and_user_says_yes() {
        let mut checker = PermissionChecker::new(true);
        let input = dummy_input(serde_json::json!({ "command": "rm -rf /tmp/test" }));
        let mut reader = io::Cursor::new(b"y\n");
        let mut writer = Vec::new();

        let result = checker
            .check(
                "bash",
                PermissionLevel::Destructive,
                &input,
                &mut reader,
                &mut writer,
            )
            .unwrap();
        assert_eq!(result, PermissionDecision::Allowed);
        let prompt = String::from_utf8(writer).unwrap();
        assert!(prompt.contains("Allow bash"));
        assert!(prompt.contains("rm -rf"));
        assert!(prompt.contains("[y]es [n]o [a]lways"));
    }

    #[test]
    fn destructive_user_says_no() {
        let mut checker = PermissionChecker::new(true);
        let input = dummy_input(serde_json::json!({ "command": "rm -rf /" }));
        let mut reader = io::Cursor::new(b"n\n");
        let mut writer = Vec::new();

        let result = checker
            .check(
                "bash",
                PermissionLevel::Destructive,
                &input,
                &mut reader,
                &mut writer,
            )
            .unwrap();
        assert_eq!(result, PermissionDecision::Denied);
    }

    #[test]
    fn destructive_user_says_always() {
        let mut checker = PermissionChecker::new(true);
        let input = dummy_input(serde_json::json!({ "command": "ls" }));

        // First call — user says "always"
        let mut reader = io::Cursor::new(b"a\n");
        let mut writer = Vec::new();
        let result = checker
            .check(
                "bash",
                PermissionLevel::Destructive,
                &input,
                &mut reader,
                &mut writer,
            )
            .unwrap();
        assert_eq!(result, PermissionDecision::AllowedAlways);

        // Second call — should not prompt.
        let mut reader2 = io::Cursor::new(b"");
        let mut writer2 = Vec::new();
        let result2 = checker
            .check(
                "bash",
                PermissionLevel::Destructive,
                &input,
                &mut reader2,
                &mut writer2,
            )
            .unwrap();
        assert_eq!(result2, PermissionDecision::AllowedAlways);
        assert!(writer2.is_empty());
    }

    #[test]
    fn confirm_disabled_auto_allows() {
        let mut checker = PermissionChecker::new(false);
        let input = dummy_input(serde_json::json!({ "command": "rm -rf /" }));
        let mut reader = io::Cursor::new(b"");
        let mut writer = Vec::new();

        let result = checker
            .check(
                "bash",
                PermissionLevel::Destructive,
                &input,
                &mut reader,
                &mut writer,
            )
            .unwrap();
        assert_eq!(result, PermissionDecision::Allowed);
        assert!(writer.is_empty());
    }

    #[test]
    fn summarize_bash_command() {
        let input = dummy_input(serde_json::json!({ "command": "echo hello" }));
        assert_eq!(summarize_input("bash", &input), "echo hello");
    }

    #[test]
    fn summarize_file_path() {
        let input = dummy_input(serde_json::json!({ "path": "src/main.rs" }));
        assert_eq!(summarize_input("file_read", &input), "src/main.rs");
    }

    #[test]
    fn needs_prompt_returns_correct() {
        let checker = PermissionChecker::new(true);
        // ReadOnly — no prompt.
        assert!(!checker.needs_prompt("file_read", PermissionLevel::ReadOnly));
        // ReadWrite — no prompt.
        assert!(!checker.needs_prompt("file_edit", PermissionLevel::ReadWrite));
        // Destructive — prompt needed (both bash and file_write).
        assert!(checker.needs_prompt("bash", PermissionLevel::Destructive));
        assert!(checker.needs_prompt("file_write", PermissionLevel::Destructive));
    }

    #[test]
    fn apply_answer_records_always() {
        let mut checker = PermissionChecker::new(true);
        let result = checker.apply_answer("bash", "a");
        assert_eq!(result, PermissionDecision::AllowedAlways);

        // After "always", needs_prompt should return false.
        assert!(!checker.needs_prompt("bash", PermissionLevel::Destructive));
        // And auto_decide should return AllowedAlways.
        assert_eq!(
            checker.auto_decide("bash", PermissionLevel::Destructive),
            PermissionDecision::AllowedAlways
        );
    }

    #[test]
    fn summarize_truncates_long_command() {
        let long_cmd = "a".repeat(100);
        let input = dummy_input(serde_json::json!({ "command": long_cmd }));
        let summary = summarize_input("bash", &input);
        assert!(summary.len() <= 63);
        assert!(summary.ends_with("..."));
    }

    // --- TBAC tests ---

    #[test]
    fn check_tbac_no_context() {
        let mut checker = PermissionChecker::with_tbac(true, true);
        let decision = checker.check_tbac("bash", &serde_json::json!({}));
        assert!(matches!(decision, AuthzDecision::NoContext));
    }

    #[test]
    fn check_tbac_tool_denied() {
        let mut checker = PermissionChecker::with_tbac(true, true);
        let tools: std::collections::HashSet<String> =
            ["file_read"].iter().map(|s| s.to_string()).collect();
        checker.push_context(TaskContext::new("Test".into(), tools));

        let decision = checker.check_tbac("bash", &serde_json::json!({}));
        assert!(matches!(
            decision,
            AuthzDecision::ToolNotAllowed { ref tool, .. } if tool == "bash"
        ));
    }

    #[test]
    fn check_tbac_param_violation() {
        use cuervo_core::types::auth::ParameterConstraint;

        let mut checker = PermissionChecker::with_tbac(true, true);
        let tools: std::collections::HashSet<String> =
            ["bash"].iter().map(|s| s.to_string()).collect();
        let mut ctx = TaskContext::new("Test".into(), tools);
        ctx.parameter_constraints.insert(
            "bash".into(),
            ParameterConstraint::CommandAllowlist {
                patterns: vec!["cargo *".into()],
            },
        );
        checker.push_context(ctx);

        let decision = checker.check_tbac("bash", &serde_json::json!({"command": "rm -rf /"}));
        assert!(matches!(decision, AuthzDecision::ParamViolation { .. }));
    }

    #[test]
    fn check_tbac_allowed_consumes() {
        let mut checker = PermissionChecker::with_tbac(true, true);
        let tools: std::collections::HashSet<String> =
            ["bash"].iter().map(|s| s.to_string()).collect();
        let mut ctx = TaskContext::new("Test".into(), tools);
        ctx.max_invocations = Some(2);
        checker.push_context(ctx);

        let d1 = checker.check_tbac("bash", &serde_json::json!({}));
        assert!(matches!(d1, AuthzDecision::Allowed { .. }));

        let d2 = checker.check_tbac("bash", &serde_json::json!({}));
        assert!(matches!(d2, AuthzDecision::Allowed { .. }));

        // Third invocation should fail — exhausted.
        let d3 = checker.check_tbac("bash", &serde_json::json!({}));
        assert!(matches!(d3, AuthzDecision::ContextInvalid { .. }));
    }

    #[test]
    fn tbac_disabled_returns_no_context() {
        let mut checker = PermissionChecker::with_tbac(true, false);
        let tools: std::collections::HashSet<String> =
            ["bash"].iter().map(|s| s.to_string()).collect();
        checker.push_context(TaskContext::new("Test".into(), tools));

        // Even with a context pushed, TBAC disabled → NoContext.
        let decision = checker.check_tbac("bash", &serde_json::json!({}));
        assert!(matches!(decision, AuthzDecision::NoContext));
    }

    #[test]
    fn tbac_config_default_disabled() {
        let config = cuervo_core::types::SecurityConfig::default();
        assert!(!config.tbac_enabled);
    }
}
