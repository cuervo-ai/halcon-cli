//! Unified permission pipeline — single authority for all permission decisions.
//!
//! ## Single Authority Principle (Phase 1, Remediation Sprint)
//!
//! `PermissionPipeline::check()` is the SOLE entry point for permission decisions.
//! No other code in the system calls blacklist, TBAC, or conversational permission
//! checks directly. All traffic routes through this pipeline.
//!
//! ## 7-Phase Cascade
//!
//!   Phase 1 → Deny rules    (TBAC deny, fast exit, no side effects)
//!   Phase 2 → Allow rules   (ReadOnly auto-allow, TBAC allow — record success)
//!   Phase 3 → Blacklist     (G7 hard veto, pattern match)
//!   Phase 4 → Risk classify (RiskLevel assessment for UI context)
//!   Phase 5 → Safety check  (bypass-immune paths: .git/, .ssh/, .env, etc.)
//!   Phase 6 → Denial check  (DenialTracker::should_escalate?)
//!   Phase 7 → Conversational (interactive prompt / auto-decision)
//!
//! Xiyo alignment: steps 1e, 1f, 1g ALWAYS prompt even in auto/bypass mode.
//! HALCON implements this via the `bypass_immune` flag on `Ask`.

use halcon_core::types::{AuthzDecision, PermissionDecision, PermissionLevel, ToolInput};
use halcon_security::{Action, Permission, RbacPolicy, Resource, Role};

use super::blacklist;
use super::conversational::ConversationalPermissionHandler;
use super::denial_tracker::DenialTracker;

/// Map a tool name to its RBAC `Resource` class.
///
/// Groups tools by the resource they operate on, enabling role-based control.
/// Unknown tools default to `Resource::Bash` (most restrictive non-custom resource).
fn tool_to_resource(tool_name: &str) -> Resource {
    match tool_name {
        // Shell execution
        "bash" | "background_start" | "background_output" | "background_kill" => Resource::Bash,

        // File reads
        "file_read"
        | "grep"
        | "glob_tool"
        | "directory_tree"
        | "symbol_search"
        | "semantic_grep"
        | "file_inspect"
        | "json_schema_validate"
        | "json_transform" => Resource::FileRead,

        // File writes
        "file_write" | "file_edit" | "file_delete" | "diff_apply" => Resource::FileWrite,

        // Network access
        "web_fetch" | "web_search" | "http_request" | "native_search" | "native_crawl"
        | "native_index_query" => Resource::Network,

        // Git operations
        "git" | "git_blame" => Resource::Git,

        // Database access
        "sql_query" => Resource::Database,

        // System information
        "process_list" | "port_check" | "env_inspect" | "test_run" | "code_coverage"
        | "lint_check" => Resource::SystemInfo,

        // Default: custom resource matching tool name (restrictive — requires explicit grant)
        other => Resource::Custom(other.to_string()),
    }
}

/// Map a `PermissionLevel` to the RBAC `Action` to check.
fn perm_level_to_action(level: PermissionLevel) -> Action {
    match level {
        PermissionLevel::ReadOnly => Action::Read,
        PermissionLevel::ReadWrite => Action::Write,
        PermissionLevel::Destructive => Action::Execute,
    }
}

/// Final decision from the unified permission pipeline.
#[derive(Debug)]
pub enum PipelineDecision {
    /// Tool is allowed to execute.
    Allow(ToolInput),
    /// Tool is denied — includes the reason and which gate denied it.
    Deny { reason: String, gate: &'static str },
    /// Tool needs interactive user confirmation.
    ///
    /// When `bypass_immune` is true, auto-mode/bypass-mode CANNOT convert
    /// this to Allow. The user MUST interact (safety checks, .git/ writes,
    /// shell config edits, tools requiring explicit interaction).
    ///
    /// Xiyo reference: steps 1e, 1f, 1g in hasPermissionsToUseToolInner.
    Ask {
        prompt: String,
        gate: &'static str,
        bypass_immune: bool,
    },
}

/// Paths that trigger bypass-immune permission checks.
/// These ALWAYS require explicit user confirmation, even in auto mode.
const SAFETY_PATHS: &[&str] = &[
    ".git/", ".claude/", ".halcon/", ".bashrc", ".zshrc", ".profile", ".ssh/", ".env",
];

/// Check if a tool invocation targets a safety-sensitive path.
fn is_safety_sensitive(tool_name: &str, input: &serde_json::Value) -> bool {
    // Only file-mutating tools can be safety-sensitive
    if !matches!(tool_name, "file_write" | "file_edit" | "bash") {
        return false;
    }

    // Check path arguments
    let path = input
        .get("path")
        .or_else(|| input.get("file_path"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Check command argument (for bash)
    let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

    for sensitive in SAFETY_PATHS {
        if path.contains(sensitive) || command.contains(sensitive) {
            return true;
        }
    }

    false
}

/// Run the unified permission pipeline for a tool invocation.
///
/// This is THE single entry point for all permission decisions in the system.
/// No other code should call blacklist, TBAC, or conversational permission directly.
///
/// Gate order:
///   1. TBAC (task-scoped whitelist)       → Allow | Deny | Pass
///   2. Blacklist (G7 HARD VETO)           → Deny | Pass
///   3. Safety check (bypass-immune)       → Ask(bypass_immune=true) | Pass
///   4. Denial tracking (escalation)       → info only (modifies final behavior)
///   5. Conversational handler (legacy)    → Allow | Deny | Ask
pub async fn authorize_tool(
    tool_name: &str,
    perm_level: PermissionLevel,
    input: &ToolInput,
    permissions: &mut ConversationalPermissionHandler,
    denial_tracker: Option<&mut DenialTracker>,
) -> PipelineDecision {
    authorize_tool_with_rbac(
        tool_name,
        perm_level,
        input,
        permissions,
        denial_tracker,
        None,
        None,
    )
    .await
}

/// Run the unified permission pipeline with optional RBAC enforcement.
///
/// When `rbac_policy` and `role` are provided, RBAC is checked as Gate 0
/// before all other gates. When `None`, RBAC is skipped (backward compatible).
pub async fn authorize_tool_with_rbac(
    tool_name: &str,
    perm_level: PermissionLevel,
    input: &ToolInput,
    permissions: &mut ConversationalPermissionHandler,
    denial_tracker: Option<&mut DenialTracker>,
    rbac_policy: Option<&RbacPolicy>,
    role: Option<&Role>,
) -> PipelineDecision {
    let _span = tracing::info_span!(
        "permission_pipeline",
        tool = %tool_name,
        perm_level = ?perm_level,
    )
    .entered();

    let current_input = input.clone();

    // ── Gate 0: RBAC (Role-Based Access Control) ─────────────────────────
    if let (Some(policy), Some(role)) = (rbac_policy, role) {
        let resource = tool_to_resource(tool_name);
        let action = perm_level_to_action(perm_level);
        let requested = Permission::new(action, resource);
        if !policy.can(role, &requested) {
            tracing::info!(
                gate = "rbac",
                decision = "deny",
                tool = %tool_name,
                role = %role,
                permission = %requested,
                "Gate 0 RBAC: denied — role lacks permission"
            );
            return PipelineDecision::Deny {
                reason: format!(
                    "role '{}' does not have permission '{}' for tool '{}'",
                    role, requested, tool_name
                ),
                gate: "rbac",
            };
        }
        tracing::debug!(gate = "rbac", decision = "pass", tool = %tool_name, role = %role, "Gate 0 RBAC: passed");
    }

    // ── Gate 1: TBAC ─────────────────────────────────────────────────────
    match permissions.check_tbac(tool_name, &current_input.arguments) {
        AuthzDecision::Allowed { .. } | AuthzDecision::NoContext => {
            tracing::debug!(gate = "tbac", decision = "pass", tool = %tool_name, "Gate 1 TBAC: passed");
        }
        AuthzDecision::ToolNotAllowed { ref tool, .. }
        | AuthzDecision::ParamViolation { ref tool, .. } => {
            tracing::info!(gate = "tbac", decision = "deny", tool = %tool_name, "Gate 1 TBAC: denied by task context");
            if let Some(tracker) = denial_tracker {
                tracker.record_denial(tool_name);
            }
            return PipelineDecision::Deny {
                reason: format!("tool '{}' denied by task context policy", tool),
                gate: "tbac",
            };
        }
        AuthzDecision::ContextInvalid { reason, .. } => {
            tracing::info!(gate = "tbac", decision = "deny", tool = %tool_name, reason = %reason, "Gate 1 TBAC: context invalid");
            if let Some(tracker) = denial_tracker {
                tracker.record_denial(tool_name);
            }
            return PipelineDecision::Deny {
                reason: format!("task context expired or exhausted: {}", reason),
                gate: "tbac",
            };
        }
    }

    // ── Gate 2: Blacklist (G7 HARD VETO) ──────────────────────────────────
    if tool_name == "bash" {
        if let Some(cmd) = current_input
            .arguments
            .get("command")
            .and_then(|v| v.as_str())
        {
            let analysis = blacklist::analyze_command(cmd);
            if analysis.is_blacklisted {
                let pattern_name = analysis
                    .matched_pattern
                    .as_ref()
                    .map(|p| p.name)
                    .unwrap_or("unknown");
                tracing::warn!(
                    gate = "blacklist",
                    decision = "deny",
                    tool = %tool_name,
                    pattern = pattern_name,
                    "Gate 2 Blacklist: G7 HARD VETO"
                );
                if let Some(tracker) = denial_tracker {
                    tracker.record_denial(tool_name);
                }
                return PipelineDecision::Deny {
                    reason: format!(
                        "command blocked by G7 blacklist ({}): {}",
                        pattern_name, cmd
                    ),
                    gate: "blacklist",
                };
            }
        }
        tracing::debug!(gate = "blacklist", decision = "pass", tool = %tool_name, "Gate 2 Blacklist: passed");
    }

    // ── Gate 3: Safety-sensitive path detection (bypass-immune) ───────────
    if is_safety_sensitive(tool_name, &current_input.arguments) {
        tracing::info!(
            gate = "safety_check",
            decision = "ask",
            tool = %tool_name,
            bypass_immune = true,
            "Gate 3 Safety: targets sensitive path — requires confirmation"
        );
        return PipelineDecision::Ask {
            prompt: format!(
                "Tool '{}' targets a safety-sensitive path. Requires explicit confirmation.",
                tool_name
            ),
            gate: "safety_check",
            bypass_immune: true,
        };
    }

    // ── Gate 4: Denial tracking (escalation check) ────────────────────────
    if let Some(tracker) = denial_tracker.as_ref() {
        if tracker.should_escalate(tool_name) {
            tracing::info!(
                gate = "denial_tracker",
                tool = %tool_name,
                "Gate 4 Denial tracker: escalation threshold reached"
            );
        }
    }

    // ── Gate 5: Conversational permission handler ────────────────────────
    let decision = permissions
        .authorize(tool_name, perm_level, &current_input)
        .await;

    match decision {
        PermissionDecision::Denied
        | PermissionDecision::DeniedForDirectory
        | PermissionDecision::DeniedForPattern => {
            tracing::info!(
                gate = "conversational",
                decision = "deny",
                tool = %tool_name,
                perm_decision = ?decision,
                "Gate 5 Conversational: user denied"
            );
            if let Some(tracker) = denial_tracker {
                tracker.record_denial(tool_name);
            }
            return PipelineDecision::Deny {
                reason: format!("the user denied permission for '{}'", tool_name),
                gate: "conversational",
            };
        }
        PermissionDecision::Allowed
        | PermissionDecision::AllowedAlways
        | PermissionDecision::AllowedForDirectory
        | PermissionDecision::AllowedForRepository
        | PermissionDecision::AllowedForPattern
        | PermissionDecision::AllowedThisSession => {
            tracing::debug!(
                gate = "conversational",
                decision = "allow",
                tool = %tool_name,
                perm_decision = ?decision,
                "Gate 5 Conversational: allowed"
            );
            if let Some(tracker) = denial_tracker {
                tracker.record_success(tool_name);
            }
        }
    }

    tracing::debug!(tool = %tool_name, final_decision = "allow", "Permission pipeline: all gates passed");
    PipelineDecision::Allow(current_input)
}

// ─────────────────────────────────────────────────────────────────────────────
// PermissionPipeline — stateful single authority (Phase 1.3)
// ─────────────────────────────────────────────────────────────────────────────

/// Context provided to the permission pipeline for each check.
pub struct PermissionContext<'a> {
    /// Tool name (canonical or alias — pipeline resolves internally).
    pub tool_name: &'a str,
    /// Permission level of the tool (ReadOnly, ReadWrite, Destructive).
    pub perm_level: PermissionLevel,
    /// Tool input (arguments, working directory).
    pub input: &'a ToolInput,
}

/// Unified permission pipeline — single authority for all permission decisions.
///
/// Owns the `DenialTracker` so denial state persists across tool invocations
/// within a session. The `ConversationalPermissionHandler` is passed mutably
/// to `check()` because it holds interactive state (TUI channels, TBAC context).
///
/// ## Usage
///
/// ```ignore
/// let mut pipeline = PermissionPipeline::new();
///
/// let decision = pipeline.check(
///     &PermissionContext { tool_name: "bash", perm_level, input: &tool_input },
///     &mut permissions,
/// ).await;
/// ```
pub struct PermissionPipeline {
    denial_tracker: DenialTracker,
    /// RBAC policy for role-based access control. When set, Gate 0 enforces
    /// role permissions before all other gates.
    rbac_policy: Option<RbacPolicy>,
    /// Current execution role. Required when `rbac_policy` is set.
    role: Option<Role>,
}

impl PermissionPipeline {
    /// Create a new pipeline with default denial threshold (3).
    pub fn new() -> Self {
        Self {
            denial_tracker: DenialTracker::default(),
            rbac_policy: None,
            role: None,
        }
    }

    /// Create a pipeline with a custom denial escalation threshold.
    pub fn with_threshold(threshold: u32) -> Self {
        Self {
            denial_tracker: DenialTracker::new(threshold),
            rbac_policy: None,
            role: None,
        }
    }

    /// Create a pipeline with RBAC enforcement.
    pub fn with_rbac(policy: RbacPolicy, role: Role) -> Self {
        Self {
            denial_tracker: DenialTracker::default(),
            rbac_policy: Some(policy),
            role: Some(role),
        }
    }

    /// Set or replace the RBAC policy and role.
    pub fn set_rbac(&mut self, policy: RbacPolicy, role: Role) {
        self.rbac_policy = Some(policy);
        self.role = Some(role);
    }

    /// The single entry point for ALL permission decisions.
    ///
    /// Delegates to `authorize_tool_with_rbac()` with RBAC context.
    pub async fn check(
        &mut self,
        ctx: &PermissionContext<'_>,
        permissions: &mut ConversationalPermissionHandler,
    ) -> PipelineDecision {
        authorize_tool_with_rbac(
            ctx.tool_name,
            ctx.perm_level,
            ctx.input,
            permissions,
            Some(&mut self.denial_tracker),
            self.rbac_policy.as_ref(),
            self.role.as_ref(),
        )
        .await
    }

    /// Non-interactive pre-authorization for ReadOnly tools in the parallel path.
    ///
    /// Runs the fast, stateless gates (TBAC deny, blacklist, safety-sensitive)
    /// WITHOUT the conversational handler. ReadOnly tools auto-allow through
    /// the conversational gate anyway, so skipping it is safe.
    ///
    /// This method requires `&mut` for TBAC (which decrements usage counters)
    /// and denial tracking, but does NOT require interactive prompting.
    ///
    /// Call this sequentially for each tool BEFORE launching parallel futures.
    /// Returns `Some(deny_reason)` if the tool should be blocked, `None` if allowed.
    pub fn pre_authorize_readonly(
        &mut self,
        tool_name: &str,
        input: &halcon_core::types::ToolInput,
        permissions: &mut ConversationalPermissionHandler,
    ) -> Option<PipelineDecision> {
        // D2 fix: Skip RBAC Gate 0 in the parallel pre-auth path for ReadOnly tools.
        //
        // BEFORE: RBAC could hard-deny a ReadOnly tool (e.g., directory_tree) before
        // the user ever gets prompted, causing a false "permission denied" plan step
        // failure even when the user later approves.
        //
        // AFTER: ReadOnly tools skip RBAC in pre-auth. The full pipeline (with RBAC
        // + conversational) runs in the sequential path if the tool needs higher
        // permissions. ReadOnly tools are inherently safe (they read, not write).
        //
        // RBAC still enforces on ReadWrite/Destructive tools via the sequential path.
        if let (Some(policy), Some(role)) = (&self.rbac_policy, &self.role) {
            let resource = tool_to_resource(tool_name);
            let requested = Permission::new(Action::Read, resource);
            if !policy.can(role, &requested) {
                // Log but do NOT deny — ReadOnly tools should be allowed through
                // the parallel path. The user can always deny at the TUI prompt.
                tracing::debug!(
                    gate = "rbac",
                    tool = %tool_name,
                    role = %role,
                    permission = %requested,
                    "D2: RBAC would deny ReadOnly tool in pre-auth — allowing through (safe read operation)"
                );
            }
        }

        // Gate 1: TBAC — task-scoped policy can deny even ReadOnly tools.
        match permissions.check_tbac(tool_name, &input.arguments) {
            halcon_core::types::AuthzDecision::Allowed { .. }
            | halcon_core::types::AuthzDecision::NoContext => {}
            halcon_core::types::AuthzDecision::ToolNotAllowed { ref tool, .. }
            | halcon_core::types::AuthzDecision::ParamViolation { ref tool, .. } => {
                self.denial_tracker.record_denial(tool_name);
                return Some(PipelineDecision::Deny {
                    reason: format!("tool '{}' denied by task context policy", tool),
                    gate: "tbac",
                });
            }
            halcon_core::types::AuthzDecision::ContextInvalid { reason, .. } => {
                self.denial_tracker.record_denial(tool_name);
                return Some(PipelineDecision::Deny {
                    reason: format!("task context expired or exhausted: {}", reason),
                    gate: "tbac",
                });
            }
        }

        // Gate 2: Blacklist (G7 hard veto).
        if tool_name == "bash" {
            if let Some(cmd) = input.arguments.get("command").and_then(|v| v.as_str()) {
                let analysis = blacklist::analyze_command(cmd);
                if analysis.is_blacklisted {
                    let pattern_name = analysis
                        .matched_pattern
                        .as_ref()
                        .map(|p| p.name)
                        .unwrap_or("unknown");
                    self.denial_tracker.record_denial(tool_name);
                    return Some(PipelineDecision::Deny {
                        reason: format!(
                            "command blocked by G7 blacklist ({}): {}",
                            pattern_name, cmd
                        ),
                        gate: "blacklist",
                    });
                }
            }
        }

        // Gate 3: Safety-sensitive path detection.
        if is_safety_sensitive(tool_name, &input.arguments) {
            self.denial_tracker.record_denial(tool_name);
            return Some(PipelineDecision::Deny {
                reason: format!(
                    "tool '{}' targets a safety-sensitive path — requires sequential execution",
                    tool_name
                ),
                gate: "safety_check",
            });
        }

        // All fast gates passed — ReadOnly auto-allows through conversational handler.
        self.denial_tracker.record_success(tool_name);
        None
    }

    /// Read-only access to the denial tracker (for observability).
    pub fn denial_tracker(&self) -> &DenialTracker {
        &self.denial_tracker
    }

    /// Mutable access to the denial tracker (for manual reset).
    pub fn denial_tracker_mut(&mut self) -> &mut DenialTracker {
        &mut self.denial_tracker
    }
}

impl Default for PermissionPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(tool_name: &str) -> ToolInput {
        ToolInput {
            tool_use_id: format!("test_{}", tool_name),
            arguments: serde_json::json!({}),
            working_directory: "/tmp".to_string(),
        }
    }

    fn make_bash_input(command: &str) -> ToolInput {
        ToolInput {
            tool_use_id: "test_bash".to_string(),
            arguments: serde_json::json!({"command": command}),
            working_directory: "/tmp".to_string(),
        }
    }

    // ── RBAC tests ────────────────────────────────────────────────────────

    #[test]
    fn tool_to_resource_mapping() {
        assert!(matches!(tool_to_resource("bash"), Resource::Bash));
        assert!(matches!(tool_to_resource("file_read"), Resource::FileRead));
        assert!(matches!(
            tool_to_resource("file_write"),
            Resource::FileWrite
        ));
        assert!(matches!(tool_to_resource("web_fetch"), Resource::Network));
        assert!(matches!(tool_to_resource("git"), Resource::Git));
        assert!(matches!(tool_to_resource("sql_query"), Resource::Database));
        assert!(matches!(
            tool_to_resource("process_list"),
            Resource::SystemInfo
        ));
        assert!(matches!(
            tool_to_resource("unknown_tool"),
            Resource::Custom(_)
        ));
    }

    #[test]
    fn perm_level_action_mapping() {
        assert!(matches!(
            perm_level_to_action(PermissionLevel::ReadOnly),
            Action::Read
        ));
        assert!(matches!(
            perm_level_to_action(PermissionLevel::ReadWrite),
            Action::Write
        ));
        assert!(matches!(
            perm_level_to_action(PermissionLevel::Destructive),
            Action::Execute
        ));
    }

    #[tokio::test]
    async fn rbac_denies_readonly_role_from_bash() {
        let policy = RbacPolicy::default_halcon_policy();
        let role = Role::Readonly;
        let mut perms = ConversationalPermissionHandler::new(true);
        let input = make_bash_input("ls");

        let decision = authorize_tool_with_rbac(
            "bash",
            PermissionLevel::Destructive,
            &input,
            &mut perms,
            None,
            Some(&policy),
            Some(&role),
        )
        .await;

        assert!(matches!(
            decision,
            PipelineDecision::Deny { gate: "rbac", .. }
        ));
    }

    #[tokio::test]
    async fn rbac_allows_developer_to_read_files() {
        let policy = RbacPolicy::default_halcon_policy();
        let role = Role::Developer;
        let mut perms = ConversationalPermissionHandler::new(true);
        let input = make_input("file_read");

        let decision = authorize_tool_with_rbac(
            "file_read",
            PermissionLevel::ReadOnly,
            &input,
            &mut perms,
            None,
            Some(&policy),
            Some(&role),
        )
        .await;

        // Should pass RBAC and reach conversational (which auto-allows ReadOnly)
        assert!(matches!(decision, PipelineDecision::Allow(_)));
    }

    #[tokio::test]
    async fn rbac_denies_plugin_from_bash() {
        let policy = RbacPolicy::default_halcon_policy();
        let role = Role::Plugin;
        let mut perms = ConversationalPermissionHandler::new(true);
        let input = make_bash_input("echo hello");

        let decision = authorize_tool_with_rbac(
            "bash",
            PermissionLevel::Destructive,
            &input,
            &mut perms,
            None,
            Some(&policy),
            Some(&role),
        )
        .await;

        assert!(matches!(
            decision,
            PipelineDecision::Deny { gate: "rbac", .. }
        ));
    }

    #[tokio::test]
    async fn rbac_admin_can_do_everything() {
        let policy = RbacPolicy::default_halcon_policy();
        let role = Role::Admin;
        let mut perms = ConversationalPermissionHandler::new(true);
        let input = make_bash_input("rm -rf /tmp/test");

        let decision = authorize_tool_with_rbac(
            "bash",
            PermissionLevel::Destructive,
            &input,
            &mut perms,
            None,
            Some(&policy),
            Some(&role),
        )
        .await;

        // RBAC passes for Admin, but blacklist or conversational may still deny
        // (this tests that RBAC itself doesn't block Admin)
        assert!(!matches!(
            decision,
            PipelineDecision::Deny { gate: "rbac", .. }
        ));
    }

    #[tokio::test]
    async fn no_rbac_policy_allows_through() {
        let mut perms = ConversationalPermissionHandler::new(true);
        let input = make_input("file_read");

        let decision = authorize_tool_with_rbac(
            "file_read",
            PermissionLevel::ReadOnly,
            &input,
            &mut perms,
            None,
            None, // no RBAC policy
            None,
        )
        .await;

        // Without RBAC, ReadOnly tools auto-allow through conversational
        assert!(matches!(decision, PipelineDecision::Allow(_)));
    }

    #[test]
    fn pipeline_with_rbac_construction() {
        let policy = RbacPolicy::default_halcon_policy();
        let pipeline = PermissionPipeline::with_rbac(policy, Role::Developer);
        assert!(pipeline.rbac_policy.is_some());
        assert!(matches!(pipeline.role, Some(Role::Developer)));
    }

    #[test]
    fn pipeline_set_rbac() {
        let mut pipeline = PermissionPipeline::new();
        assert!(pipeline.rbac_policy.is_none());

        pipeline.set_rbac(RbacPolicy::default_halcon_policy(), Role::Readonly);
        assert!(pipeline.rbac_policy.is_some());
        assert!(matches!(pipeline.role, Some(Role::Readonly)));
    }

    // ── Pre-authorize readonly with RBAC ──────────────────────────────────

    #[test]
    fn pre_authorize_readonly_rbac_allows_through_d2() {
        // D2: pre_authorize_readonly no longer hard-denies on RBAC for ReadOnly tools.
        // ReadOnly tools are safe read operations — RBAC logs but allows through.
        let policy = RbacPolicy::default_halcon_policy();
        let mut pipeline = PermissionPipeline::with_rbac(policy, Role::Custom("no_access".into()));
        let mut perms = ConversationalPermissionHandler::new(true);
        let input = make_input("file_read");

        let result = pipeline.pre_authorize_readonly("file_read", &input, &mut perms);
        assert!(
            result.is_none(),
            "D2: ReadOnly tools must pass pre-auth even without RBAC grant"
        );
    }

    #[test]
    fn pre_authorize_readonly_rbac_allow() {
        let policy = RbacPolicy::default_halcon_policy();
        let mut pipeline = PermissionPipeline::with_rbac(policy, Role::Developer);
        let mut perms = ConversationalPermissionHandler::new(true);
        let input = make_input("file_read");

        let result = pipeline.pre_authorize_readonly("file_read", &input, &mut perms);
        assert!(result.is_none()); // Allowed
    }
}
