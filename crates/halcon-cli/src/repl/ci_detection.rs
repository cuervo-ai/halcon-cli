//! CI environment auto-detection for auto-approving destructive tools.
//!
//! When running in CI/CD environments (GitHub Actions, GitLab CI, etc.),
//! we auto-approve destructive tool calls to prevent 30-second timeout hangs
//! on permission prompts.

use crate::repl::authorization::{AuthorizationPolicy, AuthorizationState};
use halcon_core::types::{PermissionDecision, PermissionLevel, ToolInput};
use std::env;

/// Result of CI environment detection.
#[derive(Debug, Clone)]
pub struct CiEnvironment {
    /// Whether a CI environment was detected.
    pub is_ci: bool,
    /// The specific CI variable that triggered detection (for logging).
    pub detected_via: Option<String>,
}

/// Detect the current CI environment.
///
/// Checks both generic `CI=true` and platform-specific variables.
/// Returns a `CiEnvironment` with `is_ci = true` if any CI indicator is found.
///
/// This is the standalone detect function for session initialization, separate
/// from the `CIDetectionPolicy` (which is used in the authorization chain).
pub fn detect() -> CiEnvironment {
    // Generic CI indicator
    if env::var("CI").is_ok_and(|v| v == "true" || v == "1") {
        return CiEnvironment { is_ci: true, detected_via: Some("CI".to_string()) };
    }

    // Platform-specific CI variables
    const CI_VARS: &[&str] = &[
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "CIRCLECI",
        "JENKINS_HOME",
        "TRAVIS",
        "BUILDKITE",
        "DRONE",
        "TEAMCITY_VERSION",
        "CIRRUS_CI",
        "SEMAPHORE",
        "CODEBUILD_BUILD_ID",
    ];

    for &var in CI_VARS {
        if env::var(var).is_ok() {
            return CiEnvironment { is_ci: true, detected_via: Some(var.to_string()) };
        }
    }

    CiEnvironment { is_ci: false, detected_via: None }
}

/// Auto-approves all tools when running in a CI environment.
///
/// Detects CI by checking standard environment variables:
/// - CI=true (generic)
/// - GITHUB_ACTIONS, GITLAB_CI, CIRCLECI, JENKINS_HOME, etc. (platform-specific)
///
/// When enabled and CI is detected, this policy returns `Allowed` for all tools,
/// bypassing permission prompts that would otherwise timeout.
pub struct CIDetectionPolicy {
    enabled: bool,
}

impl CIDetectionPolicy {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Returns true if running in a CI environment.
    ///
    /// Checks both generic CI=true and platform-specific variables.
    fn is_ci_environment() -> bool {
        // Generic CI indicator
        if env::var("CI").is_ok_and(|v| v == "true" || v == "1") {
            tracing::info!("CI environment detected: CI=true");
            return true;
        }

        // Platform-specific CI variables
        const CI_VARS: &[&str] = &[
            "GITHUB_ACTIONS",   // GitHub Actions
            "GITLAB_CI",        // GitLab CI
            "CIRCLECI",         // CircleCI
            "JENKINS_HOME",     // Jenkins
            "TRAVIS",           // Travis CI
            "BUILDKITE",        // Buildkite
            "DRONE",            // Drone CI
            "TEAMCITY_VERSION", // TeamCity
            "CIRRUS_CI",        // Cirrus CI
            "SEMAPHORE",        // Semaphore CI
            "CODEBUILD_BUILD_ID", // AWS CodeBuild
        ];

        for &var in CI_VARS {
            if env::var(var).is_ok() {
                tracing::info!("CI environment detected: {}=true", var);
                return true;
            }
        }

        false
    }
}

impl AuthorizationPolicy for CIDetectionPolicy {
    fn evaluate(
        &self,
        tool_name: &str,
        _perm_level: PermissionLevel,
        _input: &ToolInput,
        state: &AuthorizationState,
    ) -> Option<PermissionDecision> {
        if !self.enabled {
            return None; // Abstain when disabled
        }

        if !Self::is_ci_environment() {
            return None; // Not CI, let other policies decide
        }

        // C7 FIX: deny-always decisions must take priority over CI auto-approval.
        //
        // A tool in `always_denied` was explicitly rejected by the user via the
        // "deny always" prompt option. That decision must be honoured even in CI
        // environments — otherwise an attacker who injects a CI variable can bypass
        // user-set denials for dangerous tools (e.g. `file_delete`, `bash`).
        //
        // We abstain here so `SessionMemoryPolicy` (later in the chain) can return
        // `Some(Denied)` as the authoritative decision.
        if state.always_denied.contains(tool_name) {
            tracing::info!(
                "Tool '{}' is in always_denied — CI auto-approval skipped (deny-always has priority)",
                tool_name
            );
            return None;
        }

        tracing::info!("Auto-approving tool '{}' in CI environment", tool_name);
        Some(PermissionDecision::Allowed)
    }

    fn name(&self) -> &str {
        "CIDetectionPolicy"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn detect_returns_is_ci_true_for_github_actions() {
        std::env::set_var("GITHUB_ACTIONS", "true");
        let env = detect();
        assert!(env.is_ci);
        assert_eq!(env.detected_via.as_deref(), Some("GITHUB_ACTIONS"));
        std::env::remove_var("GITHUB_ACTIONS");
    }

    #[test]
    #[serial]
    fn detect_returns_is_ci_false_when_no_ci_vars() {
        for var in &["CI", "GITHUB_ACTIONS", "GITLAB_CI", "CIRCLECI", "JENKINS_HOME"] {
            std::env::remove_var(var);
        }
        let env = detect();
        assert!(!env.is_ci);
        assert!(env.detected_via.is_none());
    }

    #[test]
    #[serial]
    fn detect_returns_generic_ci_var() {
        std::env::set_var("CI", "true");
        let env = detect();
        assert!(env.is_ci);
        assert_eq!(env.detected_via.as_deref(), Some("CI"));
        std::env::remove_var("CI");
    }

    fn dummy_input(args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[test]
    #[serial]
    fn ci_detection_github_actions() {
        std::env::set_var("GITHUB_ACTIONS", "true");
        assert!(CIDetectionPolicy::is_ci_environment());
        std::env::remove_var("GITHUB_ACTIONS");
    }

    #[test]
    #[serial]
    fn ci_detection_gitlab_ci() {
        std::env::set_var("GITLAB_CI", "true");
        assert!(CIDetectionPolicy::is_ci_environment());
        std::env::remove_var("GITLAB_CI");
    }

    #[test]
    #[serial]
    fn ci_detection_generic() {
        std::env::set_var("CI", "true");
        assert!(CIDetectionPolicy::is_ci_environment());
        std::env::remove_var("CI");
    }

    #[test]
    #[serial]
    fn ci_detection_generic_one() {
        std::env::set_var("CI", "1");
        assert!(CIDetectionPolicy::is_ci_environment());
        std::env::remove_var("CI");
    }

    #[test]
    #[serial]
    fn not_ci_environment() {
        // Clear all potential CI vars
        for var in &["CI", "GITHUB_ACTIONS", "GITLAB_CI", "CIRCLECI", "JENKINS_HOME"] {
            std::env::remove_var(var);
        }
        assert!(!CIDetectionPolicy::is_ci_environment());
    }

    #[test]
    #[serial]
    fn auto_approves_in_ci() {
        std::env::set_var("CI", "true");
        let policy = CIDetectionPolicy::new(true);
        let state = AuthorizationState::new(true);
        let input = dummy_input(serde_json::json!({"command": "rm -rf /tmp/test"}));

        let decision = policy.evaluate("file_delete", PermissionLevel::Destructive, &input, &state);
        assert_eq!(decision, Some(PermissionDecision::Allowed));

        std::env::remove_var("CI");
    }

    #[test]
    #[serial]
    fn abstains_when_not_ci() {
        std::env::remove_var("CI");
        std::env::remove_var("GITHUB_ACTIONS");

        let policy = CIDetectionPolicy::new(true);
        let state = AuthorizationState::new(true);
        let input = dummy_input(serde_json::json!({}));

        let decision = policy.evaluate("bash", PermissionLevel::Destructive, &input, &state);
        assert_eq!(decision, None);
    }

    #[test]
    #[serial]
    fn abstains_when_disabled() {
        std::env::set_var("CI", "true");
        let policy = CIDetectionPolicy::new(false); // Disabled
        let state = AuthorizationState::new(true);
        let input = dummy_input(serde_json::json!({}));

        let decision = policy.evaluate("bash", PermissionLevel::Destructive, &input, &state);
        assert_eq!(decision, None); // Should abstain even though CI detected

        std::env::remove_var("CI");
    }

    // --- C7: deny-always must override CI auto-approval ---

    #[test]
    #[serial]
    fn ci_does_not_override_deny_always() {
        std::env::set_var("CI", "true");
        let policy = CIDetectionPolicy::new(true);
        let mut state = AuthorizationState::new(true);
        // User explicitly denied this tool for the session.
        state.always_denied.insert("file_delete".to_string());

        let input = dummy_input(serde_json::json!({"path": "/tmp/important.txt"}));
        let decision = policy.evaluate("file_delete", PermissionLevel::Destructive, &input, &state);

        // Must abstain (not approve) so SessionMemoryPolicy can deny.
        assert_eq!(
            decision, None,
            "CI policy must NOT override deny-always — returned {:?}",
            decision
        );

        std::env::remove_var("CI");
    }

    #[test]
    #[serial]
    fn ci_still_approves_non_denied_tool() {
        std::env::set_var("CI", "true");
        let policy = CIDetectionPolicy::new(true);
        let mut state = AuthorizationState::new(true);
        // Only file_delete is denied; bash is not.
        state.always_denied.insert("file_delete".to_string());

        let input = dummy_input(serde_json::json!({"command": "echo ok"}));
        let decision = policy.evaluate("bash", PermissionLevel::Destructive, &input, &state);

        assert_eq!(
            decision,
            Some(PermissionDecision::Allowed),
            "CI policy must still approve tools not in deny-always"
        );

        std::env::remove_var("CI");
    }
}
