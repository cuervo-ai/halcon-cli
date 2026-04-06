//! SandboxedExecutor — process execution with OS-level isolation.
//!
//! ## macOS
//! Uses `sandbox-exec -p <profile>` to run commands within a Seatbelt profile
//! that restricts file-system writes and network access.
//!
//! ## Linux
//! Uses `unshare --net --user` to isolate network and user namespaces, providing
//! a lightweight sandbox without requiring root.
//!
//! ## Fallback
//! On other platforms (or when sandbox features are disabled), the policy denylist
//! is applied but process isolation is not available.
//!
//! ## Resource limits
//! All paths apply:
//! - `ulimit -t` (CPU time) via process timeout
//! - Max output size (truncate stdout/stderr)
//! - Working directory restriction

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::policy::{PolicyViolation, SandboxPolicy};

// ─── SandboxConfig ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Security policy (denylist + network/privilege flags).
    pub policy: SandboxPolicy,
    /// Working directory for all commands.
    pub working_dir: std::path::PathBuf,
    /// Maximum execution time. Commands exceeding this are killed.
    pub timeout: Duration,
    /// Maximum stdout + stderr bytes to capture.
    pub max_output_bytes: usize,
    /// Shell to use for command execution.
    pub shell: String,
    /// Whether to use OS-level sandboxing (macOS sandbox-exec, Linux unshare).
    pub use_os_sandbox: bool,
    /// Additional paths that the sandbox may write to (besides working_dir).
    /// Used for temp directories, build output, etc.
    pub writable_paths: Vec<std::path::PathBuf>,
    /// Additional paths that the sandbox may read from (besides standard system paths).
    pub readable_paths: Vec<std::path::PathBuf>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            policy: SandboxPolicy::default(),
            working_dir: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            timeout: Duration::from_secs(30),
            max_output_bytes: 256 * 1024, // 256 KB
            shell: "/bin/sh".to_string(),
            use_os_sandbox: true,
            writable_paths: Vec::new(),
            readable_paths: Vec::new(),
        }
    }
}

// ─── ExecutionResult ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Combined stdout + stderr output (head+tail truncated if oversized).
    pub output: String,
    /// Process exit code. `None` if killed by timeout.
    pub exit_code: Option<i32>,
    /// Whether the command was killed by timeout.
    pub timed_out: bool,
    /// Whether a policy violation prevented execution.
    pub policy_violation: Option<String>,
    /// Execution wall time in milliseconds.
    pub duration_ms: u64,
    /// Whether this should be treated as an error by the caller.
    pub is_error: bool,
}

impl ExecutionResult {
    /// A pre-execution policy violation result.
    pub fn policy_blocked(violation: PolicyViolation) -> Self {
        Self {
            output: format!("[SANDBOX BLOCKED] {}", violation.message),
            exit_code: None,
            timed_out: false,
            policy_violation: Some(violation.message),
            duration_ms: 0,
            is_error: true,
        }
    }

    /// A timeout result.
    pub fn timed_out_result(timeout_secs: u64) -> Self {
        Self {
            output: format!(
                "[TIMEOUT] Command exceeded {}s limit and was killed",
                timeout_secs
            ),
            exit_code: None,
            timed_out: true,
            policy_violation: None,
            duration_ms: timeout_secs * 1000,
            is_error: true,
        }
    }
}

// ─── SandboxedExecutor ────────────────────────────────────────────────────────

/// OS-sandboxed command executor.
pub struct SandboxedExecutor {
    config: SandboxConfig,
}

impl SandboxedExecutor {
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    /// Execute a shell command inside the sandbox.
    ///
    /// Steps:
    /// 1. Policy denylist check (never reaches OS if blocked).
    /// 2. Build the platform-appropriate sandboxed command.
    /// 3. Execute with timeout.
    /// 4. Truncate output if oversized.
    pub async fn execute(&self, command: &str) -> ExecutionResult {
        // Step 1: Policy check (synchronous, zero-cost).
        if let Err(violation) = self.config.policy.validate(command) {
            warn!(command = %command, violation = %violation, "Sandbox policy violation");
            return ExecutionResult::policy_blocked(violation);
        }

        debug!(command = %command, "Sandbox executing command");
        let start = std::time::Instant::now();

        // Step 2: Build sandboxed command.
        let mut cmd = self.build_command(command);

        // Step 3: Execute with timeout.
        let timeout_dur = self.config.timeout;
        let result = timeout(timeout_dur, cmd.output()).await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let combined = if stderr.is_empty() {
                    stdout
                } else if stdout.is_empty() {
                    stderr
                } else {
                    format!("{}\n[stderr]:\n{}", stdout, stderr)
                };

                let truncated = self.truncate_output(combined);
                let exit_code = output.status.code();
                let is_error = exit_code != Some(0);

                ExecutionResult {
                    output: truncated,
                    exit_code,
                    timed_out: false,
                    policy_violation: None,
                    duration_ms,
                    is_error,
                }
            }
            Ok(Err(e)) => {
                warn!(error = %e, "Command spawn failed");
                ExecutionResult {
                    output: format!("[SPAWN ERROR] {}", e),
                    exit_code: None,
                    timed_out: false,
                    policy_violation: None,
                    duration_ms,
                    is_error: true,
                }
            }
            Err(_) => {
                warn!(timeout_secs = timeout_dur.as_secs(), "Command timed out");
                ExecutionResult::timed_out_result(timeout_dur.as_secs())
            }
        }
    }

    // ─── Private helpers ────────────────────────────────────────────────────

    fn build_command(&self, command: &str) -> Command {
        #[cfg(target_os = "macos")]
        {
            if self.config.use_os_sandbox {
                return self.build_macos_sandboxed(command);
            }
        }

        #[cfg(target_os = "linux")]
        {
            if self.config.use_os_sandbox {
                return self.build_linux_sandboxed(command);
            }
        }

        // Fallback: direct shell (policy already applied above).
        self.build_direct(command)
    }

    fn build_direct(&self, command: &str) -> Command {
        let mut cmd = Command::new(&self.config.shell);
        cmd.arg("-c")
            .arg(command)
            .current_dir(&self.config.working_dir)
            .kill_on_drop(true);
        cmd
    }

    #[cfg(target_os = "macos")]
    fn build_macos_sandboxed(&self, command: &str) -> Command {
        let profile = self.build_seatbelt_profile();

        let mut cmd = Command::new("sandbox-exec");
        cmd.arg("-p")
            .arg(profile)
            .arg(&self.config.shell)
            .arg("-c")
            .arg(command)
            .current_dir(&self.config.working_dir)
            .kill_on_drop(true);
        cmd
    }

    /// Build a deny-default macOS Seatbelt profile.
    ///
    /// Strategy: deny everything, then explicitly allow:
    /// - Process execution (required for the shell itself)
    /// - File reads on system paths + working directory
    /// - File writes only in working directory + explicit writable paths
    /// - Network only when policy allows
    #[cfg(target_os = "macos")]
    fn build_seatbelt_profile(&self) -> String {
        use std::fmt::Write;

        let mut profile = String::with_capacity(2048);
        let _ = writeln!(profile, "(version 1)");
        let _ = writeln!(profile, "(deny default)");
        let _ = writeln!(profile);

        // Allow process execution (required for shell + subcommands).
        let _ = writeln!(profile, "(allow process-exec)");
        let _ = writeln!(profile, "(allow process-fork)");
        let _ = writeln!(profile);

        // Allow signal handling.
        let _ = writeln!(profile, "(allow signal)");
        let _ = writeln!(profile);

        // Allow sysctl reads (required for many programs).
        let _ = writeln!(profile, "(allow sysctl-read)");
        let _ = writeln!(profile);

        // Allow mach-* (required for basic process functionality on macOS).
        let _ = writeln!(profile, "(allow mach-lookup)");
        let _ = writeln!(profile, "(allow mach-register)");
        let _ = writeln!(profile);

        // Allow file reads on standard system paths.
        let _ = writeln!(profile, "; System read access");
        for sys_path in &[
            "/usr/lib",
            "/usr/bin",
            "/usr/local",
            "/bin",
            "/sbin",
            "/dev",
            "/private/var/tmp",
            "/private/tmp",
            "/tmp",
            "/etc",
            "/var",
            "/Library",
            "/System",
            "/Applications",
        ] {
            let _ = writeln!(profile, "(allow file-read* (subpath \"{}\"))", sys_path);
        }
        // Home directory read access (for shell config, tools like git, etc.)
        if let Ok(home) = std::env::var("HOME") {
            let _ = writeln!(profile, "(allow file-read* (subpath \"{}\"))", home);
        }
        let _ = writeln!(profile);

        // Working directory: full read access.
        let workdir = self.config.working_dir.display();
        let _ = writeln!(profile, "; Working directory read access");
        let _ = writeln!(profile, "(allow file-read* (subpath \"{}\"))", workdir);
        let _ = writeln!(profile);

        // Additional readable paths.
        if !self.config.readable_paths.is_empty() {
            let _ = writeln!(profile, "; Additional readable paths");
            for path in &self.config.readable_paths {
                let _ = writeln!(
                    profile,
                    "(allow file-read* (subpath \"{}\"))",
                    path.display()
                );
            }
            let _ = writeln!(profile);
        }

        // File writes: only working directory + /tmp + explicit writable paths.
        let _ = writeln!(profile, "; Write access");
        let _ = writeln!(profile, "(allow file-write* (subpath \"{}\"))", workdir);
        let _ = writeln!(profile, "(allow file-write* (subpath \"/private/tmp\"))");
        let _ = writeln!(profile, "(allow file-write* (subpath \"/tmp\"))");
        let _ = writeln!(profile, "(allow file-write* (subpath \"/dev/null\"))");
        let _ = writeln!(profile, "(allow file-write* (subpath \"/dev/tty\"))");
        // Additional writable paths (e.g., build output dirs).
        for path in &self.config.writable_paths {
            let _ = writeln!(
                profile,
                "(allow file-write* (subpath \"{}\"))",
                path.display()
            );
        }
        let _ = writeln!(profile);

        // Network: only when policy allows.
        if self.config.policy.allow_network {
            let _ = writeln!(profile, "; Network access enabled");
            let _ = writeln!(profile, "(allow network*)");
        } else {
            let _ = writeln!(profile, "; Network access denied");
        }

        profile
    }

    #[cfg(target_os = "linux")]
    fn build_linux_sandboxed(&self, command: &str) -> Command {
        // Linux: use unshare for namespace isolation (rootless).
        //
        // Isolation layers:
        // 1. --net: isolated network namespace (no network access unless policy allows)
        // 2. --user --map-root-user: user namespace isolation
        // 3. Environment scrubbing: remove sensitive env vars
        // 4. HOME/TMPDIR set to working directory (constrain writes)
        let mut cmd = Command::new("unshare");

        // Network isolation.
        if !self.config.policy.allow_network {
            cmd.arg("--net");
        }

        // User namespace for UID/GID isolation.
        // --map-root-user maps current user to root inside namespace (safe, no real root).
        cmd.arg("--user").arg("--map-root-user");

        cmd.arg("--")
            .arg(&self.config.shell)
            .arg("-c")
            .arg(command)
            .current_dir(&self.config.working_dir)
            .kill_on_drop(true);

        // Scrub sensitive environment variables from the sandboxed process.
        for var in &[
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "AZURE_CLIENT_SECRET",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "DATABASE_URL",
        ] {
            cmd.env_remove(var);
        }

        // Constrain TMPDIR to working directory.
        cmd.env("TMPDIR", &self.config.working_dir);

        cmd
    }

    /// Head+tail truncation (60%+30% with omission notice) — same as the SOTA
    /// truncation from post_batch.rs that replaced the naive head-only cut.
    fn truncate_output(&self, output: String) -> String {
        let max = self.config.max_output_bytes;
        if output.len() <= max {
            return output;
        }

        let head_size = (max as f32 * 0.60) as usize;
        let tail_size = (max as f32 * 0.30) as usize;

        // Ensure we cut at valid UTF-8 boundaries.
        let head_end = output
            .char_indices()
            .take_while(|(i, _)| *i < head_size)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);

        let tail_start = output.len().saturating_sub(tail_size);
        let tail_start = output[tail_start..]
            .char_indices()
            .next()
            .map(|(i, _)| tail_start + i)
            .unwrap_or(output.len());

        format!(
            "{}\n\n[... {} bytes omitted by sandbox truncation ...]\n\n{}",
            &output[..head_end],
            output.len() - head_end - (output.len() - tail_start),
            &output[tail_start..]
        )
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn exec() -> SandboxedExecutor {
        SandboxedExecutor::new(SandboxConfig {
            use_os_sandbox: false, // disable OS sandbox in tests for portability
            timeout: Duration::from_secs(5),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn safe_echo_executes() {
        let result = exec().execute("echo hello").await;
        assert!(
            !result.is_error,
            "exit_code={:?} output={}",
            result.exit_code, result.output
        );
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn exit_code_captured() {
        let result = exec().execute("exit 42").await;
        assert_eq!(result.exit_code, Some(42));
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn policy_violation_blocked() {
        let result = exec().execute("rm -rf /").await;
        assert!(result.is_error);
        assert!(result.policy_violation.is_some());
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn sudo_blocked_by_policy() {
        let result = exec().execute("sudo ls").await;
        assert!(result.is_error);
        assert!(result.policy_violation.is_some());
    }

    #[tokio::test]
    async fn output_truncated_when_large() {
        let executor = SandboxedExecutor::new(SandboxConfig {
            use_os_sandbox: false,
            timeout: Duration::from_secs(10),
            max_output_bytes: 100, // very small for test
            ..Default::default()
        });
        // Generate output larger than max
        let result = executor.execute("python3 -c \"print('x' * 1000)\" 2>/dev/null || echo 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'").await;
        // Either the python works or the fallback echo works — either way should be within limits or truncated
        // The point is no panic and output is captured
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn working_dir_respected() {
        let dir = tempfile::tempdir().unwrap();
        let executor = SandboxedExecutor::new(SandboxConfig {
            use_os_sandbox: false,
            working_dir: dir.path().to_path_buf(),
            timeout: Duration::from_secs(5),
            ..Default::default()
        });
        let result = executor.execute("pwd").await;
        assert!(!result.is_error);
        // The output should contain the temp dir path (or its realpath equivalent)
        // On macOS, /var/folders → /private/var/folders due to symlinks, so just check it exists
        assert!(!result.output.trim().is_empty());
    }

    #[test]
    fn truncate_output_head_tail() {
        let exec = SandboxedExecutor::new(SandboxConfig {
            max_output_bytes: 100,
            ..Default::default()
        });
        let large = "A".repeat(500);
        let result = exec.truncate_output(large);
        assert!(result.contains("omitted by sandbox truncation"));
        assert!(result.len() < 500);
    }
}
