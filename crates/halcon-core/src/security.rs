//! Centralized security patterns for command safety analysis.
//!
//! This module is the **single source of truth** for catastrophic bash command patterns.
//! Both the runtime blacklist (`halcon-tools/bash.rs`) and the G7 permission gate
//! (`halcon-cli/repl/command_blacklist.rs`) import from here, eliminating duplication.
//!
//! ## Dual Blacklist Architecture
//!
//! There are two independent blacklist systems in this codebase:
//!
//! 1. **Runtime guard** (`halcon-tools/bash.rs`): applied at `execute()` time, returns
//!    `HalconError::InvalidInput` — occurs AFTER permission was granted. Uses the
//!    `CATASTROPHIC_PATTERNS` from this module as its built-in set.
//!
//! 2. **G7 HARD VETO** (`halcon-cli/repl/command_blacklist.rs`): applied at
//!    `ConversationalPermissionHandler::authorize()` — occurs BEFORE execution.
//!    Uses `DANGEROUS_COMMAND_PATTERNS` (named patterns with reasons) from this module.
//!
//! Both systems compile these raw pattern strings using their own `regex::Regex` instances.
//! No `regex` dependency is needed in `halcon-core` itself.

/// Raw regex patterns for catastrophic commands that must NEVER execute.
///
/// These are the patterns used by the runtime safety guard in `halcon-tools/bash.rs`.
/// All patterns are case-insensitive (the consuming code must apply `(?i)` flags).
///
/// Pattern design principles:
/// - Only block commands that are unambiguously catastrophic (unrecoverable data loss, system crash)
/// - Do NOT block commands that are safe redirections (e.g., `2>/dev/null`)
/// - Use anchors (`^`) where possible to avoid false positives
pub const CATASTROPHIC_PATTERNS: &[&str] = &[
    r"(?i)^rm\s+(-[rfivRF]+\s+)+/\s*$",                    // rm -rf /
    r"(?i)^rm\s+(-[rfivRF]+\s+)+/\*+\s*$",                // rm -rf /*
    r"(?i)^rm\s+(-[rfivRF]+\s+)+/(bin|etc|usr|var|sys|proc|dev)\b", // rm -rf /etc
    r"(?i)^rm\s+(-[rfivRF]+\s+)+~/?\s*$",                   // rm -rf ~/
    r"(?i)^rm\s+(-[rfivRF]+\s+)+\$HOME/?\s*$",             // rm -rf $HOME
    r"(?i)^rm\s+(-[rfivRF]+\s+)+/Users/\*",               // rm -rf /Users/*
    r":\(\)\{:\|:&\};:",                                    // Fork bomb
    r"(?i)^mkfs\.",                                         // mkfs.ext4
    r"(?i)dd\s+.*\s+of=/dev/[sh]d[a-z]",                  // dd to /dev/sda
    r"(?i)dd\s+.*\s+of=/dev/nvme",                         // dd to nvme
    r"(?i)(curl|wget)\s+.*\|\s*(ba)?sh\b",                 // curl | bash
    r"(?i)(curl|wget)\s+.*\|\s*python\b",                  // curl | python
    r"(?i)^chmod\s+(-R\s+)?[0-7]{3,4}\s+/\s*$",           // chmod 777 /
    r"(?i)^chown\s+(-R\s+)?.*\s+/\s*$",                   // chown -R user /
    r"(?i)^systemctl\s+stop\s+(sshd|network|NetworkManager)\b", // Stop critical services
    r"(?i)^kill\s+-9\s+1\b",                               // kill -9 1 (init)
    r"(?i)^(rm|mod)mod\s+",                                // rmmod/modmod kernel modules
    // NOTE: bare ">/dev/null" redirect (command IS only the redirect).
    // Anchored at ^ so "cargo build 2>/dev/null" is NOT matched — only standalone redirect.
    r"(?i)^\s*>\s*/dev/(null|zero)\s*$",
];

/// Named dangerous command patterns for the G7 HARD VETO gate.
///
/// Each entry is `(name, pattern, reason)`. The consuming code (`command_blacklist.rs`)
/// compiles the pattern into a `Regex` and wraps it in `DangerousPattern`.
pub const DANGEROUS_COMMAND_PATTERNS: &[(&str, &str, &str)] = &[
    (
        "Root filesystem deletion",
        r"\brm\s+(-[a-zA-Z]*r[a-zA-Z]*f[a-zA-Z]*|--recursive.*--force)\s+(/$|/\s|/\*|/\.)",
        "Attempts to recursively delete root filesystem — unrecoverable data loss",
    ),
    (
        "Disk wipe with dd",
        r"\bdd\s+.*of=/dev/(sd[a-z]|nvme[0-9]|hd[a-z]|xvd[a-z])($|\s)",
        "Direct disk write — can destroy entire partitions or disks",
    ),
    (
        "Filesystem creation on device",
        r"\bmkfs\.[a-z0-9]+\s+/dev/(sd[a-z]|nvme[0-9]|hd[a-z]|xvd[a-z])",
        "Creates new filesystem — destroys all existing data on device",
    ),
    (
        "Fork bomb",
        r":\(\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:",
        "Fork bomb — exhausts system resources and crashes the system",
    ),
    (
        "Global world-writable permissions",
        r"\bchmod\s+(-R\s+)?777\s+(/$|/\s|/\*|/\.)",
        "Makes root filesystem world-writable — critical security vulnerability",
    ),
    (
        "Disable SELinux/AppArmor",
        r"\b(setenforce\s+0|systemctl\s+disable\s+apparmor)",
        "Disables security enforcement — removes critical security protections",
    ),
    (
        "Kernel panic trigger",
        r#"echo\s+['"]?c['"]?\s*>\s*/proc/sysrq-trigger"#,
        "Forces immediate kernel panic — crashes the system",
    ),
    (
        "Memory device overwrite",
        r"\bdd\s+.*of=/dev/(mem|kmem|null|zero|random)",
        "Writes to kernel memory devices — can corrupt system state",
    ),
    (
        "Partition table destruction",
        r"\b(fdisk|parted|gdisk)\s+/dev/(sd[a-z]|nvme[0-9]|hd[a-z])",
        "Modifies partition table — can make entire disk unreadable",
    ),
    (
        "Global chown to non-root",
        r"\bchown\s+(-R\s+)?[a-z][a-z0-9]*\s+(/$|/\s|/\*|/\.)",
        "Changes ownership of root filesystem — breaks system permissions",
    ),
    (
        "Package manager removal",
        r"\b(apt|yum|dnf)\s+(remove|purge|erase)\s+(-y\s+)?(apt|dpkg|rpm|yum)",
        "Removes package manager itself — breaks system update capability",
    ),
    (
        "Swap disable on low memory",
        r"\bswapoff\s+-a",
        "Disables all swap space — can cause out-of-memory crashes",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catastrophic_patterns_non_empty() {
        assert!(!CATASTROPHIC_PATTERNS.is_empty());
        assert_eq!(CATASTROPHIC_PATTERNS.len(), 18);
    }

    #[test]
    fn dangerous_command_patterns_non_empty() {
        assert!(!DANGEROUS_COMMAND_PATTERNS.is_empty());
        assert_eq!(DANGEROUS_COMMAND_PATTERNS.len(), 12);
    }

    #[test]
    fn all_dangerous_patterns_have_name_and_reason() {
        for (name, _pattern, reason) in DANGEROUS_COMMAND_PATTERNS {
            assert!(!name.is_empty(), "pattern name must not be empty");
            assert!(!reason.is_empty(), "pattern reason must not be empty");
        }
    }

    #[test]
    fn all_catastrophic_patterns_non_empty() {
        for pattern in CATASTROPHIC_PATTERNS {
            assert!(!pattern.is_empty(), "pattern must not be empty");
        }
    }

    #[test]
    fn catastrophic_patterns_do_not_contain_bare_double_redirect() {
        // Ensure the >/dev/null pattern is anchored — raw string must start with the anchor.
        let dev_null_patterns: Vec<&&str> = CATASTROPHIC_PATTERNS
            .iter()
            .filter(|p| p.contains("/dev/(null|zero)"))
            .collect();
        assert_eq!(dev_null_patterns.len(), 1, "should have exactly one /dev/null pattern");
        assert!(
            dev_null_patterns[0].contains(r"^\s*>"),
            "the /dev/null pattern must be anchored at start to avoid blocking 2>/dev/null"
        );
    }
}
