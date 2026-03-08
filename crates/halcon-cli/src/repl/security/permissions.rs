use halcon_core::types::{AuthzDecision, PermissionDecision, PermissionLevel, TaskContext, ToolInput};

use super::authorization::AuthorizationMiddleware;

/// Checks tool permissions and prompts the user for confirmation when needed.
///
/// Extended with TBAC (Task-Based Authorization Control) support:
/// when `tbac_enabled` is true and a task context is active,
/// `check_tbac()` gates tool access to the context's allowlist
/// and parameter constraints.
///
/// Internally delegates permission decisions to [`AuthorizationMiddleware`]
/// which evaluates a policy chain (NonInteractive → PermissionLevel → SessionMemory)
/// and falls back to an interactive prompt with configurable timeout.
///
/// In TUI mode, a dedicated approval channel replaces the stdin interactive
/// prompt. Call [`set_tui_channel`] to wire the TUI permission response path.
pub struct PermissionChecker {
    /// Authorization middleware with policy chain and session state.
    middleware: AuthorizationMiddleware,
    /// Active task context stack (innermost = most restrictive).
    task_contexts: Vec<TaskContext>,
    /// Whether TBAC is enabled.
    tbac_enabled: bool,
    /// TUI permission approval channel. When set, `authorize()` waits on this
    /// channel instead of falling through to the middleware's stdin prompt.
    /// The TUI sends PermissionDecision values for advanced 8-option modal support.
    #[cfg(feature = "tui")]
    tui_approve_rx: Option<std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<halcon_core::types::PermissionDecision>>>>,
    /// Sudo password channel (TUI → executor).
    ///
    /// After a sudo command is approved, the TUI sends the OS password here.
    /// `None` means the user cancelled the sudo password entry.
    /// Kept separate from `perm_tx` because `PermissionDecision` is `Copy` and
    /// cannot carry a `String` payload.
    #[cfg(feature = "tui")]
    sudo_pw_rx: Option<std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<Option<String>>>>>,
    /// TUI notification channel for permission events (e.g. timeout denial).
    ///
    /// When set, `authorize()` sends a `UiEvent::Warning` to the TUI activity
    /// panel when the permission timeout fires, so the user can see why the
    /// tool was denied even if they missed the modal.
    #[cfg(feature = "tui")]
    notification_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::tui::events::UiEvent>>,
    /// Kept for non-breaking constructor compatibility (`with_config()`).
    ///
    /// No longer used in the TUI path — the TUI now owns the deadline via the
    /// countdown bar (`OverlayState::permission_deadline`). The backend's
    /// `authorize()` waits indefinitely; the TUI sends `Denied` when the timer
    /// expires, so there is no `tokio::time::timeout` in the backend.
    #[cfg(feature = "tui")]
    #[allow(dead_code)]
    tui_timeout_secs: u64,
}

impl PermissionChecker {
    #[allow(dead_code)] // Used by tests in permissions.rs + agent.rs + executor.rs.
    pub fn new(confirm_destructive: bool) -> Self {
        Self::with_config(confirm_destructive, false, true, 30)
    }

    /// Create a new PermissionChecker with TBAC support (legacy constructor).
    pub fn with_tbac(confirm_destructive: bool, tbac_enabled: bool) -> Self {
        Self::with_config(confirm_destructive, tbac_enabled, true, 30)
    }

    /// Create a new PermissionChecker with full configuration.
    pub fn with_config(
        confirm_destructive: bool,
        tbac_enabled: bool,
        auto_approve_in_ci: bool,
        prompt_timeout_secs: u64,
    ) -> Self {
        Self {
            middleware: AuthorizationMiddleware::new(confirm_destructive, auto_approve_in_ci, prompt_timeout_secs),
            task_contexts: Vec::new(),
            tbac_enabled,
            #[cfg(feature = "tui")]
            tui_approve_rx: None,
            #[cfg(feature = "tui")]
            sudo_pw_rx: None,
            #[cfg(feature = "tui")]
            notification_tx: None,
            #[cfg(feature = "tui")]
            tui_timeout_secs: prompt_timeout_secs.max(30),
        }
    }

    /// Disable interactive prompts (for non-interactive single-shot mode).
    ///
    /// When there is no TTY for user input, tools that would normally require
    /// confirmation are auto-approved instead of hanging on stdin.
    pub fn set_non_interactive(&mut self) {
        self.middleware.set_non_interactive();
    }

    /// Set the TUI permission approval channel.
    ///
    /// When this channel is set, `authorize()` waits for the TUI user's
    /// permission decision instead of falling through to the middleware's stdin
    /// prompt. This is essential in TUI mode where stdin is in raw mode.
    /// Now supports advanced 8-option modal with PermissionDecision values.
    #[cfg(feature = "tui")]
    pub fn set_tui_channel(&mut self, rx: tokio::sync::mpsc::UnboundedReceiver<halcon_core::types::PermissionDecision>) {
        self.tui_approve_rx = Some(std::sync::Arc::new(tokio::sync::Mutex::new(rx)));
    }

    /// Set the sudo password channel (for sudo elevation after permission approval).
    ///
    /// The TUI sends `Some(password)` when the user enters a password, or `None`
    /// when they cancel the sudo modal. The executor awaits this channel (30s
    /// timeout) after the normal permission check returns `Allowed`.
    #[cfg(feature = "tui")]
    pub fn set_sudo_channel(&mut self, rx: tokio::sync::mpsc::UnboundedReceiver<Option<String>>) {
        self.sudo_pw_rx = Some(std::sync::Arc::new(tokio::sync::Mutex::new(rx)));
    }

    /// Set the TUI notification channel for permission events.
    ///
    /// When set, `authorize()` sends a `UiEvent::Warning` to the TUI activity panel
    /// whenever the permission timeout fires. This ensures the user sees a visible
    /// explanation for why the tool was denied even if they missed the modal.
    ///
    /// Wire this with the same `ui_tx` used by `TuiSink` in TUI mode.
    #[cfg(feature = "tui")]
    pub fn set_notification_tx(
        &mut self,
        tx: tokio::sync::mpsc::UnboundedSender<crate::tui::events::UiEvent>,
    ) {
        self.notification_tx = Some(tx);
    }

    /// Await the sudo password from the TUI modal (up to `timeout_secs`).
    ///
    /// Returns:
    /// - `Some(password)` — user entered a password (may be empty string on error)
    /// - `None` — user cancelled, timed out, or sudo channel is not set
    ///
    /// Called by the executor AFTER `authorize()` returns `Allowed` for a bash
    /// command that starts with `sudo`.
    #[cfg(feature = "tui")]
    pub async fn get_sudo_password(&self, timeout_secs: u64) -> Option<String> {
        use tokio::time::{timeout, Duration};
        let Some(ref rx_arc) = self.sudo_pw_rx else {
            return None;
        };
        let mut rx = rx_arc.lock().await;
        match timeout(Duration::from_secs(timeout_secs), rx.recv()).await {
            Ok(Some(pw)) => pw, // Some(password) or None (user cancelled)
            Ok(None) => None,   // Channel closed
            Err(_) => {
                tracing::warn!(timeout_secs, "Sudo password entry timed out");
                None
            }
        }
    }

    /// Returns true if a sudo password has been cached (for display in the modal).
    ///
    /// Currently always returns false — the TUI manages cache state on its side.
    /// This method exists so `executor.rs` can pass `has_cached` to `sudo_password_request()`.
    #[cfg(feature = "tui")]
    pub fn has_cached_sudo_password(&self) -> bool {
        // Cache lifetime is managed by TuiApp (5-minute in-process cache).
        // The executor doesn't have visibility into it; TUI signals via has_cached field.
        false
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

    /// Async authorization: evaluates TBAC first, then delegates to the middleware
    /// policy chain. Falls back to interactive prompt with timeout.
    ///
    /// In TUI mode (when `set_tui_channel()` has been called), if the tool
    /// requires an interactive prompt, the method waits on the TUI approval
    /// channel instead of spawning a stdin reader. This ensures the TUI's
    /// permission overlay is the sole decision source.
    pub async fn authorize(
        &mut self,
        tool_name: &str,
        perm_level: PermissionLevel,
        input: &ToolInput,
    ) -> PermissionDecision {
        // TUI mode: intercept before middleware's stdin prompt.
        // Backend waits indefinitely — TUI owns the deadline via the countdown bar.
        // When the countdown reaches 0, the TUI sends Denied and closes the modal.
        // If the channel closes unexpectedly (TUI crash/exit), we fail-closed.
        #[cfg(feature = "tui")]
        if let Some(ref tui_rx) = self.tui_approve_rx {
            if self.needs_prompt(tool_name, perm_level) {
                let mut rx = tui_rx.lock().await;
                // Drain any stale approvals from previous permission requests.
                while rx.try_recv().is_ok() {}
                // Wait indefinitely — TUI sends a decision (or Denied on countdown expiry).
                match rx.recv().await {
                    Some(decision) => return decision,
                    None => {
                        // Channel closed — TUI exited or crashed: fail-closed.
                        tracing::warn!(tool = %tool_name, "TUI permission channel closed — denying (TUI exited?)");
                        if let Some(ref tx) = self.notification_tx {
                            let _ = tx.send(crate::tui::events::UiEvent::Warning {
                                message: format!(
                                    "Permission channel closed — '{}' denied (TUI exited?)",
                                    tool_name
                                ),
                                hint: None,
                            });
                        }
                        return PermissionDecision::Denied;
                    }
                }
            }
        }

        self.middleware.authorize(tool_name, perm_level, input).await
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
        self.middleware.needs_prompt(tool_name, permission_level)
    }

    /// Decide without prompting (for auto-allowed tools).
    pub fn auto_decide(
        &self,
        tool_name: &str,
        permission_level: PermissionLevel,
    ) -> PermissionDecision {
        self.middleware.auto_decide(tool_name, permission_level)
    }

    /// Format the permission prompt string.
    #[cfg(test)]
    pub fn format_prompt(tool_name: &str, input: &ToolInput) -> String {
        AuthorizationMiddleware::format_prompt(tool_name, input)
    }

    /// Apply a user answer and update internal state.
    #[cfg(test)]
    pub fn apply_answer(&mut self, tool_name: &str, answer: &str) -> PermissionDecision {
        self.middleware.apply_answer(tool_name, answer)
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
        // Updated: now includes [d]eny always
        assert!(prompt.contains("[y]es"));
        assert!(prompt.contains("[n]o"));
        assert!(prompt.contains("[a]lways"));
        assert!(prompt.contains("[d]eny always"));
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

    // --- New: deny-always tests ---

    #[test]
    fn apply_answer_deny_always() {
        let mut checker = PermissionChecker::new(true);
        let result = checker.apply_answer("bash", "d");
        assert_eq!(result, PermissionDecision::Denied);

        // After "deny always", needs_prompt should return false (SessionMemoryPolicy handles it).
        assert!(!checker.needs_prompt("bash", PermissionLevel::Destructive));
        // And auto_decide should return Denied.
        assert_eq!(
            checker.auto_decide("bash", PermissionLevel::Destructive),
            PermissionDecision::Denied
        );
    }

    #[test]
    fn destructive_user_says_deny_always() {
        let mut checker = PermissionChecker::new(true);
        let input = dummy_input(serde_json::json!({ "command": "rm -rf /" }));

        // First call — user says "d" (deny always)
        let mut reader = io::Cursor::new(b"d\n");
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

        // Second call — should not prompt (auto-denied).
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
        assert_eq!(result2, PermissionDecision::Denied);
        assert!(writer2.is_empty());
    }

    #[test]
    fn with_config_prompt_timeout() {
        let checker = PermissionChecker::with_config(true, false, false, 60);
        assert!(checker.needs_prompt("bash", PermissionLevel::Destructive));
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
        use halcon_core::types::auth::ParameterConstraint;

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
        let config = halcon_core::types::SecurityConfig::default();
        assert!(!config.tbac_enabled);
    }
}
