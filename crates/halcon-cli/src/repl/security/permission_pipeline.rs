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

use super::adaptive_prompt::RiskLevel;
use super::blacklist;
use super::conversational::ConversationalPermissionHandler;
use super::denial_tracker::DenialTracker;

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
    ".git/",
    ".claude/",
    ".halcon/",
    ".bashrc",
    ".zshrc",
    ".profile",
    ".ssh/",
    ".env",
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
    let command = input
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("");

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
    let current_input = input.clone();

    // ── Gate 1: TBAC ─────────────────────────────────────────────────────
    match permissions.check_tbac(tool_name, &current_input.arguments) {
        AuthzDecision::Allowed { .. } | AuthzDecision::NoContext => {}
        AuthzDecision::ToolNotAllowed { ref tool, .. }
        | AuthzDecision::ParamViolation { ref tool, .. } => {
            if let Some(tracker) = denial_tracker {
                tracker.record_denial(tool_name);
            }
            return PipelineDecision::Deny {
                reason: format!("tool '{}' denied by task context policy", tool),
                gate: "tbac",
            };
        }
        AuthzDecision::ContextInvalid { reason, .. } => {
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
    // Extracted from conversational handler into explicit pipeline gate.
    // Blacklisted commands are ALWAYS denied, no exceptions.
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
    }

    // ── Gate 3: Safety-sensitive path detection (bypass-immune) ───────────
    // Xiyo steps 1e/1f/1g: certain writes ALWAYS require confirmation.
    if is_safety_sensitive(tool_name, &current_input.arguments) {
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
    // If this tool has been denied too many times, surface it as info.
    // The actual escalation behavior is handled by the caller — this gate
    // only records the signal. The conversational handler will still make
    // the final decision.
    if let Some(tracker) = denial_tracker.as_ref() {
        if tracker.should_escalate(tool_name) {
            tracing::info!(
                tool = %tool_name,
                "Tool has been denied multiple times — consider adjusting approach"
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
            if let Some(tracker) = denial_tracker {
                tracker.record_success(tool_name);
            }
        }
    }

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
}

impl PermissionPipeline {
    /// Create a new pipeline with default denial threshold (3).
    pub fn new() -> Self {
        Self {
            denial_tracker: DenialTracker::default(),
        }
    }

    /// Create a pipeline with a custom denial escalation threshold.
    pub fn with_threshold(threshold: u32) -> Self {
        Self {
            denial_tracker: DenialTracker::new(threshold),
        }
    }

    /// The single entry point for ALL permission decisions.
    ///
    /// Delegates to `authorize_tool()` with the owned `DenialTracker`.
    pub async fn check(
        &mut self,
        ctx: &PermissionContext<'_>,
        permissions: &mut ConversationalPermissionHandler,
    ) -> PipelineDecision {
        authorize_tool(
            ctx.tool_name,
            ctx.perm_level,
            ctx.input,
            permissions,
            Some(&mut self.denial_tracker),
        )
        .await
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
