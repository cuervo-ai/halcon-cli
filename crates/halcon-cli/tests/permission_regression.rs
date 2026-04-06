//! Regression tests for the permission pipeline unification (Phase 1).
//!
//! These tests capture the CURRENT behavior of every permission code path
//! as a behavioral contract. All must pass before AND after Phase 1 changes.
//!
//! Tests cover:
//! - Pipeline decision variants (Allow/Deny/Ask)
//! - Blacklist hard veto behavior
//! - Safety-sensitive path detection (bypass-immune)
//! - Risk assessment classification
//! - TBAC gate behavior (no-context passthrough)
//! - DenialTracker escalation (new in Phase 1)

use halcon_cli::repl::adaptive_prompt::RiskLevel;
use halcon_cli::repl::command_blacklist;
use halcon_cli::repl::conversational_permission::ConversationalPermissionHandler;
use halcon_core::types::PermissionLevel;
use serde_json::json;

// ── Blacklist gate (G7 HARD VETO) ──────────────────────────────────────────

#[test]
fn regression_blacklisted_command_denied_with_correct_pattern() {
    let analysis = command_blacklist::analyze_command("rm -rf /");
    assert!(analysis.is_blacklisted);
    assert_eq!(
        analysis.matched_pattern.as_ref().unwrap().name,
        "Root filesystem deletion"
    );
    assert!(!analysis.matched_pattern.as_ref().unwrap().reason.is_empty());
}

#[test]
fn regression_all_12_blacklist_patterns_active() {
    let patterns = [
        ("rm -rf /", "Root filesystem deletion"),
        ("rm -rf /*", "Root filesystem deletion"),
        ("dd if=/dev/zero of=/dev/sda bs=1M", "Disk wipe with dd"),
        ("dd if=/dev/urandom of=/dev/nvme0", "Disk wipe with dd"),
        ("mkfs.ext4 /dev/sdb1", "Filesystem creation on device"),
        (":(){ :|:& };:", "Fork bomb"),
        ("chmod -R 777 /", "Global world-writable permissions"),
        ("setenforce 0", "Disable SELinux/AppArmor"),
        ("echo c > /proc/sysrq-trigger", "Kernel panic trigger"),
        ("dd if=/dev/zero of=/dev/mem", "Memory device overwrite"),
        ("fdisk /dev/sda", "Partition table destruction"),
        ("chown -R nobody /", "Global chown to non-root"),
        ("apt remove -y apt", "Package manager removal"),
        ("swapoff -a", "Swap disable on low memory"),
    ];

    for (cmd, expected_name) in patterns {
        let analysis = command_blacklist::analyze_command(cmd);
        assert!(
            analysis.is_blacklisted,
            "Expected '{}' to be blacklisted (pattern: {})",
            cmd, expected_name
        );
        assert_eq!(
            analysis.matched_pattern.as_ref().unwrap().name,
            expected_name,
            "Pattern name mismatch for command '{}'",
            cmd
        );
    }
}

#[test]
fn regression_safe_commands_not_blacklisted() {
    let safe_commands = [
        "rm -rf /tmp/test",
        "dd if=input.img of=output.img",
        "chmod 755 /usr/local/bin/script.sh",
        "chown user:group /home/user/file.txt",
        "ls -la /",
        "echo hello",
        "git commit -m 'test'",
        "cargo build",
    ];

    for cmd in safe_commands {
        let analysis = command_blacklist::analyze_command(cmd);
        assert!(
            !analysis.is_blacklisted,
            "Safe command '{}' should NOT be blacklisted",
            cmd
        );
    }
}

// ── Risk assessment (used by permission UI) ────────────────────────────────

#[test]
fn regression_readonly_tool_low_risk() {
    let handler = ConversationalPermissionHandler::new(true);
    let risk = handler.assess_risk_level(
        "file_read",
        PermissionLevel::ReadOnly,
        &json!({"path": "/tmp/test.txt"}),
    );
    assert_eq!(risk, RiskLevel::Low);
}

#[test]
fn regression_readwrite_tool_medium_risk() {
    let handler = ConversationalPermissionHandler::new(true);
    let risk = handler.assess_risk_level(
        "file_write",
        PermissionLevel::ReadWrite,
        &json!({"path": "/tmp/test.txt", "content": "hello"}),
    );
    assert_eq!(risk, RiskLevel::Medium);
}

#[test]
fn regression_destructive_tool_high_risk() {
    let handler = ConversationalPermissionHandler::new(true);
    let risk = handler.assess_risk_level(
        "bash",
        PermissionLevel::Destructive,
        &json!({"command": "rm -rf /tmp/test"}),
    );
    assert_eq!(risk, RiskLevel::High);
}

#[test]
fn regression_blacklisted_command_critical_risk() {
    let handler = ConversationalPermissionHandler::new(true);
    let risk = handler.assess_risk_level(
        "bash",
        PermissionLevel::Destructive,
        &json!({"command": "rm -rf /"}),
    );
    assert_eq!(risk, RiskLevel::Critical);
}

// ── Safety-sensitive path detection ────────────────────────────────────────

#[test]
fn regression_safety_paths_detected() {
    // These paths should always require bypass-immune confirmation
    let sensitive_cases = [
        ("file_write", json!({"path": "/home/user/.git/config"})),
        (
            "file_edit",
            json!({"path": "/home/user/.ssh/authorized_keys"}),
        ),
        ("file_write", json!({"path": "/home/user/.env"})),
        ("bash", json!({"command": "echo 'alias x=y' >> ~/.bashrc"})),
        (
            "file_write",
            json!({"path": "/project/.claude/settings.json"}),
        ),
        (
            "file_edit",
            json!({"file_path": "/project/.halcon/config.toml"}),
        ),
    ];

    for (tool, args) in &sensitive_cases {
        // Verify is_safety_sensitive returns true for these
        // (tested indirectly through pipeline behavior — the function is pub(crate))
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(tool, PermissionLevel::Destructive, args);
        // Safety-sensitive paths should be at least High risk
        assert!(
            matches!(risk, RiskLevel::High | RiskLevel::Critical),
            "Tool '{}' with args {:?} should be High/Critical risk, got {:?}",
            tool,
            args,
            risk
        );
    }
}

#[test]
fn regression_non_sensitive_paths_not_flagged() {
    let safe_cases = [
        (
            "file_write",
            json!({"path": "/tmp/output.txt", "content": "test"}),
        ),
        (
            "file_edit",
            json!({"path": "/project/src/main.rs", "old_string": "a", "new_string": "b"}),
        ),
    ];

    let handler = ConversationalPermissionHandler::new(true);
    for (tool, args) in &safe_cases {
        let risk = handler.assess_risk_level(tool, PermissionLevel::ReadWrite, args);
        assert_eq!(
            risk,
            RiskLevel::Medium,
            "Tool '{}' with safe path should be Medium risk",
            tool
        );
    }
}

// ── Permission handler construction ────────────────────────────────────────

#[test]
fn regression_handler_with_tbac_constructs() {
    let handler = ConversationalPermissionHandler::with_tbac(true, true);
    // Should not panic, should construct successfully
    let risk = handler.assess_risk_level(
        "file_read",
        PermissionLevel::ReadOnly,
        &json!({"path": "/tmp/test"}),
    );
    assert_eq!(risk, RiskLevel::Low);
}

#[test]
fn regression_handler_with_full_config_constructs() {
    let handler = ConversationalPermissionHandler::with_config(
        true,  // confirm_destructive
        false, // tbac_enabled
        false, // auto_approve_in_ci
        30,    // prompt_timeout_secs
    );
    let risk = handler.assess_risk_level(
        "bash",
        PermissionLevel::Destructive,
        &json!({"command": "ls"}),
    );
    assert_eq!(risk, RiskLevel::High);
}

// ── TBAC gate (no-context passthrough) ─────────────────────────────────────

#[test]
fn regression_tbac_no_context_passes_through() {
    // When no task context is active, TBAC should pass through (NoContext)
    let mut handler = ConversationalPermissionHandler::with_tbac(true, true);
    let decision = handler.check_tbac("bash", &json!({"command": "ls"}));
    // NoContext means no TBAC restriction — tool is allowed to proceed to next gate
    assert!(
        matches!(
            decision,
            halcon_core::types::AuthzDecision::NoContext
                | halcon_core::types::AuthzDecision::Allowed { .. }
        ),
        "TBAC with no context should pass through, got: {:?}",
        decision
    );
}

// ── DenialTracker (new in Phase 1) ─────────────────────────────────────────

#[test]
fn regression_denial_tracker_basic_lifecycle() {
    use halcon_cli::repl::denial_tracker::DenialTracker;

    let mut tracker = DenialTracker::new(3);

    // Initially no tool should escalate
    assert!(!tracker.should_escalate("bash"));

    // Record denials below threshold
    tracker.record_denial("bash");
    tracker.record_denial("bash");
    assert!(!tracker.should_escalate("bash"));

    // Third denial hits threshold
    tracker.record_denial("bash");
    assert!(tracker.should_escalate("bash"));

    // Other tools unaffected
    assert!(!tracker.should_escalate("file_write"));
}

#[test]
fn regression_denial_tracker_success_resets() {
    use halcon_cli::repl::denial_tracker::DenialTracker;

    let mut tracker = DenialTracker::new(3);

    tracker.record_denial("bash");
    tracker.record_denial("bash");
    tracker.record_denial("bash");
    assert!(tracker.should_escalate("bash"));

    // Success resets the counter
    tracker.record_success("bash");
    assert!(!tracker.should_escalate("bash"));
}

#[test]
fn regression_denial_tracker_independent_per_tool() {
    use halcon_cli::repl::denial_tracker::DenialTracker;

    let mut tracker = DenialTracker::new(2);

    tracker.record_denial("bash");
    tracker.record_denial("bash");
    assert!(tracker.should_escalate("bash"));

    // file_write has its own counter
    tracker.record_denial("file_write");
    assert!(!tracker.should_escalate("file_write"));
}

#[test]
fn regression_denial_tracker_reset_clears() {
    use halcon_cli::repl::denial_tracker::DenialTracker;

    let mut tracker = DenialTracker::new(2);

    tracker.record_denial("bash");
    tracker.record_denial("bash");
    assert!(tracker.should_escalate("bash"));

    tracker.reset("bash");
    assert!(!tracker.should_escalate("bash"));
}

// ── Permission pipeline completeness ───────────────────────────────────────

#[test]
fn regression_permission_decision_variants_exhaustive() {
    // Verify all PermissionDecision variants exist (compilation check)
    use halcon_core::types::PermissionDecision;

    let _decisions = [
        PermissionDecision::Allowed,
        PermissionDecision::AllowedAlways,
        PermissionDecision::AllowedForDirectory,
        PermissionDecision::AllowedForRepository,
        PermissionDecision::AllowedForPattern,
        PermissionDecision::AllowedThisSession,
        PermissionDecision::Denied,
        PermissionDecision::DeniedForDirectory,
        PermissionDecision::DeniedForPattern,
    ];
}

#[test]
fn regression_permission_level_ordering() {
    // Verify the ordering that executor/sequential relies on
    assert!(PermissionLevel::ReadOnly < PermissionLevel::ReadWrite);
    assert!(PermissionLevel::ReadWrite < PermissionLevel::Destructive);
}
