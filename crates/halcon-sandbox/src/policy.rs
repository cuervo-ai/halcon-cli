//! Sandbox policy definition and pre-execution command validation.
//!
//! The policy is checked *before* any process is spawned, ensuring that
//! dangerous commands never reach the OS even if the sandbox mechanisms fail.

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── PolicyViolationKind ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyViolationKind {
    /// Command matches a hardcoded dangerous pattern.
    DangerousCommand { pattern: String },
    /// Command attempts to escape the working directory.
    DirectoryEscape,
    /// Command uses a disallowed shell operator (`&&`, `|`, etc. when restricted).
    DisallowedOperator { operator: String },
    /// Command exceeds allowed length.
    CommandTooLong { len: usize, max: usize },
    /// Network access attempted when disabled.
    NetworkDisallowed,
    /// Privilege escalation attempt (sudo, su, doas).
    PrivilegeEscalation,
}

// ─── PolicyViolation ──────────────────────────────────────────────────────────

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
#[error("Policy violation [{kind:?}]: {message}")]
pub struct PolicyViolation {
    pub kind: PolicyViolationKind,
    pub message: String,
    pub command_snippet: String,
}

// ─── SandboxPolicy ────────────────────────────────────────────────────────────

/// Configurable security policy for the sandbox executor.
///
/// The policy is evaluated before any subprocess is created. If the command
/// violates any rule, a [`PolicyViolation`] is returned and no process starts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Allow network syscalls (curl, wget, etc.).
    pub allow_network: bool,
    /// Allow writing to files outside the working directory.
    pub allow_writes_outside_workdir: bool,
    /// Allow `sudo`, `su`, `doas` privilege escalation.
    pub allow_privilege_escalation: bool,
    /// Maximum command length in characters.
    pub max_command_len: usize,
    /// Restrict shell operators (`&&`, `||`, `|`, `;` when chaining is disabled).
    pub allow_shell_chaining: bool,
    /// Additional denylist patterns (regex strings).
    pub extra_denylist: Vec<String>,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            allow_network: false,
            allow_writes_outside_workdir: false,
            allow_privilege_escalation: false,
            max_command_len: 4096,
            allow_shell_chaining: true,
            extra_denylist: Vec::new(),
        }
    }
}

impl SandboxPolicy {
    /// Policy with all restrictions enabled (most secure).
    pub fn strict() -> Self {
        Self {
            allow_network: false,
            allow_writes_outside_workdir: false,
            allow_privilege_escalation: false,
            max_command_len: 2048,
            allow_shell_chaining: false,
            extra_denylist: Vec::new(),
        }
    }

    /// Policy with network enabled (for tools like http_probe, docker_tool).
    pub fn with_network() -> Self {
        Self {
            allow_network: true,
            ..Self::default()
        }
    }

    /// Validate a command string against this policy.
    ///
    /// Returns `Ok(())` if the command passes all checks, or a [`PolicyViolation`]
    /// describing the first violation found.
    pub fn validate(&self, command: &str) -> Result<(), PolicyViolation> {
        // Length check.
        if command.len() > self.max_command_len {
            return Err(PolicyViolation {
                kind: PolicyViolationKind::CommandTooLong {
                    len: command.len(),
                    max: self.max_command_len,
                },
                message: format!(
                    "Command length {} exceeds maximum {}",
                    command.len(),
                    self.max_command_len
                ),
                command_snippet: command[..self.max_command_len.min(80)].to_string(),
            });
        }

        let cmd_lower = command.to_lowercase();

        // Privilege escalation check.
        if !self.allow_privilege_escalation {
            for escalation_cmd in &["sudo ", "sudo\t", " su ", "\tsu\t", "doas ", "pkexec "] {
                if cmd_lower.contains(escalation_cmd)
                    || cmd_lower.starts_with(escalation_cmd.trim_start())
                {
                    return Err(PolicyViolation {
                        kind: PolicyViolationKind::PrivilegeEscalation,
                        message: format!(
                            "Privilege escalation via '{}' is not allowed",
                            escalation_cmd.trim()
                        ),
                        command_snippet: command.chars().take(80).collect(),
                    });
                }
            }
        }

        // Network commands check.
        if !self.allow_network {
            for net_cmd in &["curl ", "wget ", "nc ", "netcat ", "ssh ", "scp ", "rsync "] {
                if cmd_lower.contains(net_cmd) {
                    return Err(PolicyViolation {
                        kind: PolicyViolationKind::NetworkDisallowed,
                        message: format!(
                            "Network command '{}' is disabled by sandbox policy",
                            net_cmd.trim()
                        ),
                        command_snippet: command.chars().take(80).collect(),
                    });
                }
            }
        }

        // Shell chaining check.
        // Only check when chaining is disabled (strict mode).
        // We check for operators outside of quoted strings to reduce false positives.
        if !self.allow_shell_chaining {
            for (op, label) in &[
                (" && ", "&&"),
                (" || ", "||"),
                (" | ", "|"),
                (" ; ", ";"),
                // Also catch operators at command boundaries (start/end, after/before quotes).
                (";", ";"),
            ] {
                // For the bare semicolon, only match if it's not inside quotes.
                // For spaced operators, the spaces already reduce false positives.
                if *op == ";" {
                    // Check for semicolons that are not inside single or double quotes.
                    if contains_unquoted(command, ';') {
                        return Err(PolicyViolation {
                            kind: PolicyViolationKind::DisallowedOperator {
                                operator: label.to_string(),
                            },
                            message: format!(
                                "Shell operator '{}' is not allowed in strict mode",
                                label
                            ),
                            command_snippet: command.chars().take(80).collect(),
                        });
                    }
                } else if command.contains(op) {
                    return Err(PolicyViolation {
                        kind: PolicyViolationKind::DisallowedOperator {
                            operator: label.to_string(),
                        },
                        message: format!(
                            "Shell operator '{}' is not allowed in strict mode",
                            label
                        ),
                        command_snippet: command.chars().take(80).collect(),
                    });
                }
            }
        }

        // Dangerous command patterns — static string matching.
        let dangerous_patterns = [
            ("rm -rf /", "Recursive delete of root filesystem"),
            ("rm -rf /*", "Recursive delete of root filesystem"),
            (":(){ :|:& };:", "Fork bomb"),
            ("> /dev/sda", "Direct disk write"),
            ("dd if=", "Low-level disk operation"),
            ("mkfs.", "Filesystem formatting"),
            ("chmod -R 777 /", "Mass permission change on root"),
            ("chown -R", "Recursive ownership change"),
            ("shred ", "Secure file deletion"),
            ("wipe ", "Secure file deletion"),
            ("> /etc/passwd", "Overwrite system password file"),
            ("> /etc/shadow", "Overwrite system shadow file"),
        ];

        for (pattern, description) in &dangerous_patterns {
            if cmd_lower.contains(pattern) {
                return Err(PolicyViolation {
                    kind: PolicyViolationKind::DangerousCommand {
                        pattern: pattern.to_string(),
                    },
                    message: description.to_string(),
                    command_snippet: command.chars().take(80).collect(),
                });
            }
        }

        // Encoding bypass detection — catches attempts to evade denylist via encoding.
        // Must run BEFORE interpreter escape detection (which only checks plain text).
        if let Some(violation) = detect_encoding_bypass(command) {
            return Err(violation);
        }

        // Interpreter escape detection — normalized to catch variants.
        // Matches: python, python2, python3, python3.11, /usr/bin/python3, etc.
        // Triggers on `-c` (python/ruby/php/lua) or `-e` (perl/ruby/node).
        if let Some(violation) = detect_interpreter_escape(command) {
            return Err(violation);
        }

        // Directory escape check.
        // Detect attempts to write to sensitive system directories.
        if !self.allow_writes_outside_workdir {
            let sensitive_paths = [
                "../../",
                "/etc/",
                "/var/",
                "/root/",
                "/sys/",
                "/proc/",
                "/boot/",
                "/usr/local/bin/",
            ];
            let has_sensitive_path = sensitive_paths.iter().any(|p| command.contains(p));

            if has_sensitive_path {
                // Flag writes (both redirect and command-based).
                let write_indicators = [">", ">>", "tee ", "cp ", "mv ", "install ", "ln "];
                for indicator in &write_indicators {
                    if command.contains(indicator) {
                        return Err(PolicyViolation {
                            kind: PolicyViolationKind::DirectoryEscape,
                            message: format!(
                                "Write operation to sensitive directory detected (indicator: '{}')",
                                indicator.trim()
                            ),
                            command_snippet: command.chars().take(80).collect(),
                        });
                    }
                }
            }
        }

        // Extra denylist (user-configured patterns).
        for pattern in &self.extra_denylist {
            if cmd_lower.contains(pattern.as_str()) {
                return Err(PolicyViolation {
                    kind: PolicyViolationKind::DangerousCommand {
                        pattern: pattern.clone(),
                    },
                    message: format!("Command matches extra denylist pattern: {}", pattern),
                    command_snippet: command.chars().take(80).collect(),
                });
            }
        }

        Ok(())
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Check whether `ch` appears outside of single/double quotes in `s`.
///
/// This is a lightweight scanner (no full shell parser) that handles the common
/// case of commands like `echo "a; b"` where the semicolon is inside quotes.
fn contains_unquoted(s: &str, ch: char) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut prev_backslash = false;

    for c in s.chars() {
        if prev_backslash {
            prev_backslash = false;
            continue;
        }
        if c == '\\' {
            prev_backslash = true;
            continue;
        }
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            _ if c == ch && !in_single && !in_double => return true,
            _ => {}
        }
    }
    false
}

/// Interpreter families and their inline-execution flags.
const INTERPRETER_FAMILIES: &[(&str, &[&str])] = &[
    ("python", &["-c"]),
    ("ruby", &["-e", "-c"]),
    ("perl", &["-e"]),
    ("node", &["-e", "--eval"]),
    ("php", &["-r"]),
    ("lua", &["-e"]),
];

/// Dangerous patterns in interpreter argument strings.
const INTERPRETER_DENYLIST: &[(&str, &str)] = &[
    ("os.system", "os.system() shell escape"),
    ("subprocess", "subprocess shell escape"),
    ("import socket", "socket network escape"),
    ("exec(", "exec() code execution"),
    ("eval(", "eval() code execution"),
    ("system(", "system() shell escape"),
    ("popen(", "popen() shell escape"),
    ("io.popen", "Lua popen shell escape"),
    ("child_process", "Node child_process escape"),
    ("require('child_process')", "Node child_process escape"),
];

/// Detect interpreter-based shell escapes using normalized matching.
///
/// Handles: full paths (`/usr/bin/python3`), version suffixes (`python3.11`),
/// whitespace variations, and both single/double quote styles.
fn detect_interpreter_escape(command: &str) -> Option<PolicyViolation> {
    // Normalize: collapse whitespace runs to single space for reliable token splitting.
    let normalized: String = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized_lower = normalized.to_lowercase();
    let tokens: Vec<&str> = normalized_lower.split_whitespace().collect();

    for (i, token) in tokens.iter().enumerate() {
        // Strip path prefix to get basename: /usr/bin/python3 → python3
        let basename = token.rsplit('/').next().unwrap_or(token);

        for (family, flags) in INTERPRETER_FAMILIES {
            // Match family: "python" matches "python", "python3", "python3.11"
            if basename == *family
                || (basename.starts_with(family)
                    && basename[family.len()..]
                        .starts_with(|c: char| c.is_ascii_digit() || c == '.'))
            {
                // Check if the next token is an execution flag.
                if let Some(flag_token) = tokens.get(i + 1) {
                    if flags.contains(flag_token) {
                        // Collect everything after the flag as the "argument".
                        let arg_portion = tokens[i + 2..].join(" ");
                        // Check argument against denylist.
                        for (pattern, description) in INTERPRETER_DENYLIST {
                            if arg_portion.contains(pattern) {
                                return Some(PolicyViolation {
                                    kind: PolicyViolationKind::DangerousCommand {
                                        pattern: format!("{} {} <{}>", family, flag_token, pattern),
                                    },
                                    message: format!(
                                        "Interpreter shell escape: {} via {} {}",
                                        description, basename, flag_token
                                    ),
                                    command_snippet: command.chars().take(80).collect(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

// ─── Encoding bypass detection ──────────────────────────────────────────────

/// Detect attempts to bypass the denylist via encoding tricks:
///
/// 1. **Base64 pipe-to-shell**: `echo "..." | base64 -d | sh`
/// 2. **Hex/octal escape sequences**: `$'\x72\x6d'`, `$'\162\155'`
/// 3. **Variable indirection**: `cmd=sudo; $cmd rm -rf /`
/// 4. **Backtick/subshell evaluation**: `` `echo rm` `` or `$(echo rm)`
/// 5. **eval with string construction**: `eval "su"+"do"`
fn detect_encoding_bypass(command: &str) -> Option<PolicyViolation> {
    let cmd_lower = command.to_lowercase();

    // 1. Base64 decode piped to shell execution.
    // Catches: base64 -d | sh, base64 --decode | bash, base64 -d | /bin/sh
    if (cmd_lower.contains("base64 -d") || cmd_lower.contains("base64 --decode"))
        && (cmd_lower.contains("| sh")
            || cmd_lower.contains("| bash")
            || cmd_lower.contains("| /bin/sh")
            || cmd_lower.contains("| /bin/bash")
            || cmd_lower.contains("|sh")
            || cmd_lower.contains("|bash"))
    {
        return Some(PolicyViolation {
            kind: PolicyViolationKind::DangerousCommand {
                pattern: "base64 decode piped to shell".to_string(),
            },
            message: "Base64-encoded command piped to shell execution detected".to_string(),
            command_snippet: command.chars().take(80).collect(),
        });
    }

    // 2. Hex escape sequences in $'...' ANSI-C quoting (bash/zsh).
    // Pattern: $'\xNN' or $'\NNN' (octal) — used to construct commands character by character.
    if command.contains("$'\\x") || command.contains("$'\\0") {
        return Some(PolicyViolation {
            kind: PolicyViolationKind::DangerousCommand {
                pattern: "hex/octal escape sequence".to_string(),
            },
            message: "Hex/octal escape sequences in ANSI-C quoting can bypass command denylist"
                .to_string(),
            command_snippet: command.chars().take(80).collect(),
        });
    }

    // 3. printf-based command construction piped to shell.
    // Catches: printf '\x72\x6d' | sh, printf "%s" "rm" | bash
    if cmd_lower.contains("printf")
        && cmd_lower.contains("\\x")
        && (cmd_lower.contains("| sh")
            || cmd_lower.contains("| bash")
            || cmd_lower.contains("|sh")
            || cmd_lower.contains("|bash"))
    {
        return Some(PolicyViolation {
            kind: PolicyViolationKind::DangerousCommand {
                pattern: "printf hex to shell".to_string(),
            },
            message: "Printf with hex escapes piped to shell can bypass command denylist"
                .to_string(),
            command_snippet: command.chars().take(80).collect(),
        });
    }

    // 4. eval with string concatenation or variable expansion.
    // Catches: eval "su"+"do", eval "$var", eval $(...)
    if cmd_lower.contains("eval ") {
        // eval is inherently dangerous — any use in a sandbox is suspect.
        return Some(PolicyViolation {
            kind: PolicyViolationKind::DangerousCommand {
                pattern: "eval".to_string(),
            },
            message:
                "Use of 'eval' is blocked in sandbox — it can bypass all command denylist checks"
                    .to_string(),
            command_snippet: command.chars().take(80).collect(),
        });
    }

    // 5. xxd reverse (binary → text) piped to shell.
    if cmd_lower.contains("xxd -r") && (cmd_lower.contains("| sh") || cmd_lower.contains("| bash"))
    {
        return Some(PolicyViolation {
            kind: PolicyViolationKind::DangerousCommand {
                pattern: "xxd reverse to shell".to_string(),
            },
            message: "xxd -r piped to shell can bypass command denylist".to_string(),
            command_snippet: command.chars().take(80).collect(),
        });
    }

    None
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> SandboxPolicy {
        SandboxPolicy::default()
    }

    #[test]
    fn safe_command_passes() {
        assert!(policy().validate("ls -la").is_ok());
        assert!(policy().validate("cargo build").is_ok());
        assert!(policy().validate("grep -r 'TODO' src/").is_ok());
    }

    #[test]
    fn rm_rf_root_blocked() {
        let result = policy().validate("rm -rf /");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            PolicyViolationKind::DangerousCommand { .. }
        ));
    }

    #[test]
    fn sudo_blocked() {
        let result = policy().validate("sudo apt-get install vim");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            PolicyViolationKind::PrivilegeEscalation
        ));
    }

    #[test]
    fn network_blocked_by_default() {
        let result = policy().validate("curl https://example.com");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            PolicyViolationKind::NetworkDisallowed
        ));
    }

    #[test]
    fn network_allowed_when_policy_permits() {
        let p = SandboxPolicy::with_network();
        assert!(p.validate("curl https://example.com").is_ok());
    }

    #[test]
    fn command_too_long_blocked() {
        let long_cmd = "a".repeat(5000);
        let result = policy().validate(&long_cmd);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            PolicyViolationKind::CommandTooLong { .. }
        ));
    }

    #[test]
    fn fork_bomb_blocked() {
        let result = policy().validate(":(){ :|:& };:");
        assert!(result.is_err());
    }

    #[test]
    fn extra_denylist_works() {
        let mut p = SandboxPolicy::default();
        p.extra_denylist = vec!["forbidden_tool".into()];
        assert!(p.validate("forbidden_tool --run").is_err());
        assert!(p.validate("allowed_tool --run").is_ok());
    }

    #[test]
    fn strict_policy_blocks_network() {
        let p = SandboxPolicy::strict();
        assert!(p.validate("curl http://x.com").is_err());
    }

    // ── Shell chaining tests ─────────────────────────────────────────────

    #[test]
    fn default_policy_allows_shell_chaining() {
        // Default has allow_shell_chaining: true
        let p = SandboxPolicy::default();
        assert!(p.validate("ls && echo done").is_ok());
        assert!(p.validate("cat foo | grep bar").is_ok());
        assert!(p.validate("cmd1 ; cmd2").is_ok());
    }

    #[test]
    fn strict_policy_blocks_and_operator() {
        let p = SandboxPolicy::strict();
        let r = p.validate("ls && echo pwned");
        assert!(r.is_err());
        assert!(matches!(
            r.unwrap_err().kind,
            PolicyViolationKind::DisallowedOperator { .. }
        ));
    }

    #[test]
    fn strict_policy_blocks_or_operator() {
        let p = SandboxPolicy::strict();
        assert!(p.validate("false || echo fallback").is_err());
    }

    #[test]
    fn strict_policy_blocks_pipe_operator() {
        let p = SandboxPolicy::strict();
        assert!(p.validate("cat file | grep secret").is_err());
    }

    #[test]
    fn strict_policy_blocks_semicolon() {
        let p = SandboxPolicy::strict();
        assert!(p.validate("ls; rm -rf /tmp").is_err());
    }

    #[test]
    fn strict_policy_allows_semicolon_in_quotes() {
        let p = SandboxPolicy::strict();
        // Semicolons inside quotes should not trigger (common in echo/printf).
        assert!(p.validate("echo \"hello; world\"").is_ok());
        assert!(p.validate("echo 'a; b; c'").is_ok());
    }

    #[test]
    fn strict_policy_allows_escaped_semicolon() {
        let p = SandboxPolicy::strict();
        assert!(p.validate("echo hello\\; world").is_ok());
    }

    // ── Unquoted scanner tests ───────────────────────────────────────────

    #[test]
    fn contains_unquoted_basic() {
        assert!(super::contains_unquoted("a;b", ';'));
        assert!(!super::contains_unquoted("'a;b'", ';'));
        assert!(!super::contains_unquoted("\"a;b\"", ';'));
        assert!(!super::contains_unquoted("a\\;b", ';'));
    }

    // ── Directory escape tests ─────────────────────────────────────────

    #[test]
    fn directory_escape_blocks_append_to_etc() {
        let p = policy();
        let r = p.validate("echo 'bad' >> /etc/hosts");
        assert!(r.is_err());
        assert!(matches!(
            r.unwrap_err().kind,
            PolicyViolationKind::DirectoryEscape
        ));
    }

    #[test]
    fn directory_escape_blocks_write_to_proc() {
        let p = policy();
        assert!(p
            .validate("echo 1 > /proc/sys/net/ipv4/ip_forward")
            .is_err());
    }

    #[test]
    fn directory_escape_blocks_cp_to_root() {
        let p = policy();
        assert!(p.validate("cp malware /root/.bashrc").is_err());
    }

    #[test]
    fn directory_escape_blocks_symlink_to_sys() {
        let p = policy();
        assert!(p.validate("ln -s payload /sys/firmware/efi").is_err());
    }

    #[test]
    fn directory_escape_allows_reads() {
        // Reading sensitive dirs should be fine — only writes are blocked.
        let p = policy();
        assert!(p.validate("cat /etc/hosts").is_ok());
        assert!(p.validate("ls /var/log/").is_ok());
    }

    // ── Interpreter escape tests ───────────────────────────────────────

    #[test]
    fn python_os_system_double_quotes() {
        let p = policy();
        assert!(p
            .validate(r#"python -c "import os; os.system('id')" "#)
            .is_err());
    }

    #[test]
    fn python_os_system_single_quotes() {
        let p = policy();
        assert!(p
            .validate("python -c 'import os; os.system(\"id\")'")
            .is_err());
    }

    #[test]
    fn python3_version_specific() {
        let p = policy();
        assert!(p
            .validate("python3.11 -c 'import os; os.system(\"ls\")'")
            .is_err());
    }

    #[test]
    fn python_full_path() {
        let p = policy();
        assert!(p
            .validate("/usr/bin/python3 -c 'import subprocess; subprocess.run([])'")
            .is_err());
    }

    #[test]
    fn python_extra_whitespace() {
        let p = policy();
        assert!(p
            .validate("python3   -c   'import os; os.system(\"id\")'")
            .is_err());
    }

    #[test]
    fn perl_system_escape() {
        let p = policy();
        assert!(p.validate("perl -e 'system(\"ls\")'").is_err());
    }

    #[test]
    fn node_child_process() {
        let p = policy();
        assert!(p
            .validate("node -e \"require('child_process').exec('ls')\"")
            .is_err());
    }

    #[test]
    fn ruby_system_escape() {
        let p = policy();
        assert!(p.validate("ruby -e 'system(\"whoami\")'").is_err());
    }

    #[test]
    fn safe_python_allowed() {
        // Normal Python usage without dangerous patterns should pass.
        let p = policy();
        assert!(p.validate("python3 -c 'print(42)'").is_ok());
        assert!(p.validate("python3 script.py").is_ok());
    }

    #[test]
    fn contains_unquoted_mixed_quotes() {
        // Semicolon inside double quotes nested in command.
        assert!(!super::contains_unquoted("echo \"hello; world\" done", ';'));
        // Semicolon outside quotes.
        assert!(super::contains_unquoted("echo \"hello\" ; echo world", ';'));
    }

    // ── Encoding bypass tests ───────────────────────────────────────────

    #[test]
    fn base64_decode_to_shell_blocked() {
        let p = policy();
        assert!(p.validate("echo 'cm0gLXJmIC8=' | base64 -d | sh").is_err());
        assert!(p.validate("echo payload | base64 --decode | bash").is_err());
        assert!(p.validate("echo payload | base64 -d | /bin/sh").is_err());
    }

    #[test]
    fn base64_decode_without_shell_allowed() {
        let p = SandboxPolicy::with_network();
        // base64 decode without piping to shell is fine (e.g., decoding a file).
        assert!(p
            .validate("echo 'dGVzdA==' | base64 -d > output.txt")
            .is_ok());
    }

    #[test]
    fn hex_escape_sequences_blocked() {
        let p = policy();
        assert!(p.validate("$'\\x72\\x6d' -rf /").is_err());
        assert!(p.validate("$'\\0162\\0155' -rf /").is_err());
    }

    #[test]
    fn printf_hex_to_shell_blocked() {
        let p = policy();
        assert!(p.validate("printf '\\x72\\x6d' | sh").is_err());
    }

    #[test]
    fn eval_blocked() {
        let p = policy();
        assert!(p.validate("eval \"rm -rf /\"").is_err());
        assert!(p.validate("eval $DANGEROUS_CMD").is_err());
    }

    #[test]
    fn xxd_reverse_to_shell_blocked() {
        let p = policy();
        assert!(p
            .validate("echo '726d202d7266202f' | xxd -r -p | sh")
            .is_err());
    }

    #[test]
    fn safe_echo_not_blocked() {
        // Normal echo commands should still work fine.
        let p = policy();
        assert!(p.validate("echo hello world").is_ok());
        assert!(p.validate("echo 'test string'").is_ok());
    }
}
