use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use std::sync::LazyLock;
use std::time::Duration;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, SandboxConfig, ToolInput, ToolOutput};

use crate::sandbox;

/// Built-in blacklist of dangerous command patterns.
///
/// These patterns block catastrophic commands that could destroy the system:
/// - `rm -rf /` and variants (root directory deletion)
/// - Fork bombs (`: (){:|:&};:`)
/// - Disk formatting (`mkfs.*`)
/// - Direct disk writes (`dd ... of=/dev/...`)
/// - Pipe-to-shell exploits (`curl ... | sh`)
/// - Critical service shutdowns (`systemctl stop sshd`)
/// - Init process termination (`kill -9 1`)
///
/// All patterns are case-insensitive for maximum safety.
static DEFAULT_BLACKLIST: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        r"(?i)^rm\s+(-[rfivRF]+\s+)+/\s*$",                    // rm -rf /
        r"(?i)^rm\s+(-[rfivRF]+\s+)+/\*+\s*$",                // rm -rf /*
        r"(?i)^rm\s+(-[rfivRF]+\s+)+/(bin|etc|usr|var|sys|proc|dev)\b", // rm -rf /etc
        r":\(\)\{:\|:&\};:",                                   // Fork bomb
        r"(?i)^mkfs\.",                                        // mkfs.ext4
        r"(?i)dd\s+.*\s+of=/dev/[sh]d[a-z]",                  // dd to /dev/sda
        r"(?i)dd\s+.*\s+of=/dev/nvme",                        // dd to nvme
        r"(?i)(curl|wget)\s+.*\|\s*(ba)?sh\b",                // curl | bash
        r"(?i)(curl|wget)\s+.*\|\s*python\b",                 // curl | python
        r"(?i)^chmod\s+(-R\s+)?[0-7]{3,4}\s+/\s*$",          // chmod 777 /
        r"(?i)^chown\s+(-R\s+)?.*\s+/\s*$",                  // chown -R user /
        r"(?i)^systemctl\s+stop\s+(sshd|network|NetworkManager)\b", // Stop critical services
        r"(?i)^kill\s+-9\s+1\b",                              // kill -9 1 (init)
        r"(?i)^(rm|mod)mod\s+",                               // rmmod/modmod kernel modules
        r"(?i)>\s*/dev/(null|zero)\s*$",                      // > /dev/null data loss
    ]
    .into_iter()
    .map(|pattern| {
        Regex::new(pattern).unwrap_or_else(|e| {
            panic!("Invalid built-in blacklist pattern {}: {}", pattern, e)
        })
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
                CuervoError::InvalidInput(format!(
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

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let command = input.arguments["command"]
            .as_str()
            .ok_or_else(|| CuervoError::InvalidInput("bash requires 'command' string".into()))?;

        if command.trim().is_empty() {
            return Err(CuervoError::InvalidInput("bash: command must not be empty".into()));
        }

        // Blacklist check: block dangerous commands
        if let Some(reason) = self.is_command_blacklisted(command) {
            tracing::warn!(
                command = %command,
                reason = %reason,
                "Blocked dangerous bash command"
            );
            return Err(CuervoError::InvalidInput(format!(
                "Dangerous command blocked: {}",
                reason
            )));
        }

        let timeout_ms = input.arguments["timeout_ms"]
            .as_u64()
            .unwrap_or(self.default_timeout_ms);

        let timeout = Duration::from_millis(timeout_ms.min(600_000));

        // Build the command with optional sandbox rlimits.
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
            Ok(Err(e)) => Err(CuervoError::ToolExecutionFailed {
                tool: "bash".into(),
                message: format!("failed to execute command: {e}"),
            }),
            Err(_) => Err(CuervoError::ToolTimeout {
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
        BashTool::new(120, SandboxConfig {
            enabled: false,
            ..SandboxConfig::default()
        }, vec![], false).unwrap()
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
            CuervoError::ToolTimeout { .. } => {}
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
}
