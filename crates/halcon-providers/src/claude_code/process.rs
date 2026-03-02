//! Subprocess lifecycle for the Claude Code CLI.
//!
//! `ProcessTransport` wraps a persistent `claude` subprocess and implements
//! `CliTransport`, making it swappable with `MockTransport` in tests.
//!
//! Key improvements over Goose's `CliProcess`:
//! - Implements `CliTransport` trait → fully mockable.
//! - Passes `--system-prompt`, `--model`, `--verbose`, `--include-partial-messages`
//!   at spawn time (Goose uses all of these; original halcon did not).
//! - Drain / request-collection logic lives in `ManagedProcess`, not here.

use std::path::PathBuf;

use async_trait::async_trait;
#[cfg(unix)]
use libc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tracing::{debug, warn};

use halcon_core::error::{HalconError, Result};

use super::transport::CliTransport;

// ─────────────────────────────────────────────────────────────────────────────
// Config
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters needed to spawn (or re-spawn) the `claude` CLI subprocess.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// Path / name of the `claude` binary (default: `"claude"`).
    pub command: String,
    /// Permission mode flags passed to the CLI.
    pub mode: SpawnMode,
    /// Optional path to an MCP config file.
    pub mcp_config: Option<PathBuf>,
    /// Pass `--strict-mcp-config` when `mcp_config` is set.
    pub mcp_strict: bool,
    /// Initial model passed as `--model <name>` at spawn time.
    /// Can be switched mid-session via `control_request` without a re-spawn.
    /// `None` or `"default"` → CLI uses its configured default.
    pub model: Option<String>,
    /// System prompt passed as `--system-prompt <text>` at spawn time.
    /// Fixed for the lifetime of the subprocess; changing it requires a re-spawn.
    /// `None` → CLI uses its default system prompt.
    pub system_prompt: Option<String>,
}

/// Permission mode forwarded from `ClaudeCodeMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnMode {
    /// `--dangerously-skip-permissions` — fully autonomous.
    Auto,
    /// `--permission-mode acceptEdits` — approve edits only.
    SmartApprove,
    /// No extra flags — interactive / chat mode.
    Chat,
}

// ─────────────────────────────────────────────────────────────────────────────
// ProcessTransport
// ─────────────────────────────────────────────────────────────────────────────

/// A live `claude` CLI subprocess exposing bidirectional NDJSON I/O.
///
/// Implements `CliTransport` so `ManagedProcess` can swap in `MockTransport`
/// for tests without any subprocess.
pub struct ProcessTransport {
    stdin: BufWriter<ChildStdin>,
    stdout: Lines<BufReader<ChildStdout>>,
    child: Child,
}

impl ProcessTransport {
    /// Spawn a new `claude` subprocess with the given configuration.
    pub async fn spawn(config: &SpawnConfig) -> Result<Self> {
        let mut cmd = build_command(config);

        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| HalconError::ConnectionError {
                provider: "claude_code".into(),
                message: format!("failed to spawn '{}': {e}", config.command),
            })?;

        let stdin = child
            .stdin
            .take()
            .map(BufWriter::new)
            .ok_or_else(|| HalconError::Internal("claude-code: stdin unavailable".into()))?;

        let stdout = child
            .stdout
            .take()
            .map(|s| BufReader::new(s).lines())
            .ok_or_else(|| HalconError::Internal("claude-code: stdout unavailable".into()))?;

        debug!(command = %config.command, "claude-code: subprocess spawned");
        Ok(Self { stdin, stdout, child })
    }
}

#[async_trait]
impl CliTransport for ProcessTransport {
    async fn send_line(&mut self, line: &str) -> Result<()> {
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| HalconError::StreamError(format!("write to claude-code stdin: {e}")))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|e| HalconError::StreamError(format!("write newline: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| HalconError::StreamError(format!("flush stdin: {e}")))?;
        Ok(())
    }

    async fn recv_line(&mut self) -> Result<Option<String>> {
        self.stdout
            .next_line()
            .await
            .map_err(|e| HalconError::StreamError(format!("read claude-code stdout: {e}")))
    }

    fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                debug!(status = ?status, "claude-code: subprocess exited");
                false
            }
            Err(e) => {
                tracing::warn!(error = %e, "claude-code: try_wait error");
                false
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Command builder
// ─────────────────────────────────────────────────────────────────────────────

/// Build the `tokio::process::Command` for spawning Claude Code.
///
/// Flags applied (in order):
/// 1. `--input-format stream-json --output-format stream-json` (always)
/// 2. `--verbose` (always — enables system/model events)
/// 3. `--include-partial-messages` (always — enables streaming deltas)
/// 4. Permission flags based on `mode`
/// 5. `--model <name>` if `config.model` is Some and not `"default"`
/// 6. `--system-prompt <text>` if `config.system_prompt` is Some and non-empty
/// 7. `--mcp-config <path>` and optionally `--strict-mcp-config`
pub fn build_command(config: &SpawnConfig) -> Command {
    let mut cmd = Command::new(&config.command);

    // ── Nested-session guard ─────────────────────────────────────────────────
    // Claude Code sets these env vars in every process it launches.  Without
    // removing them, the child claude CLI prints:
    //   "Claude Code cannot be launched inside another Claude Code session."
    // We are intentionally spawning a standalone subprocess, so we clear them.
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
    cmd.env_remove("CLAUDE_CODE_SESSION_ID");
    cmd.env_remove("CLAUDE_CODE_CONVERSATION_ID");

    // ── Sudo privilege guard ─────────────────────────────────────────────────
    // `--dangerously-skip-permissions` is blocked when claude detects sudo/root
    // context via SUDO_COMMAND / SUDO_USER.  Remove these so the subprocess
    // runs with user-level permissions even if the parent shell had sudo context.
    cmd.env_remove("SUDO_COMMAND");
    cmd.env_remove("SUDO_USER");
    cmd.env_remove("SUDO_UID");
    cmd.env_remove("SUDO_GID");

    // ── Working directory ────────────────────────────────────────────────────
    // Run the subprocess from the user's home directory (not the caller's CWD).
    //
    // Without this, when halcon runs inside a project directory (e.g. a Node.js
    // or Rust project), the claude subprocess detects the project context and
    // autonomously executes Bash / file-read tools to explore it.  Those tools
    // produce `tool_use` events on stdout and then wait for `tool_result` events
    // on stdin — which halcon never sends — causing a 30-second timeout → EOF.
    //
    // Setting current_dir to $HOME avoids project-context detection while still
    // allowing MCP configs (which use absolute paths) to work correctly.
    if let Ok(home) = std::env::var("HOME") {
        cmd.current_dir(home);
    }

    // ── Core NDJSON protocol flags ──────────────────────────────────────────
    // `--print` (`-p`) enables non-interactive / pipe mode.
    // Without it, the CLI opens an interactive TUI and ignores stdin/stdout.
    // `--input-format stream-json` + `--output-format stream-json` only work
    // with `--print` (see `claude --help`).
    cmd.arg("--print");
    cmd.args(["--input-format", "stream-json", "--output-format", "stream-json"]);
    // verbose: enables {"type":"system","subtype":"init"} and model resolution events
    cmd.arg("--verbose");
    // include-partial-messages: enables streaming text deltas (letter-by-letter)
    cmd.arg("--include-partial-messages");

    // ── Permission mode ─────────────────────────────────────────────────────
    // `--dangerously-skip-permissions` is blocked when claude detects uid==0.
    // Automatically downgrade Auto→Chat when running as root so the subprocess
    // doesn't exit immediately with a privilege error.
    let effective_mode = if config.mode == SpawnMode::Auto {
        #[cfg(unix)]
        let is_root = unsafe { libc::getuid() } == 0;
        #[cfg(not(unix))]
        let is_root = false;
        if is_root {
            warn!(
                "claude-code: running as root — downgrading Auto mode to Chat \
                 (--dangerously-skip-permissions is blocked for uid 0)"
            );
            SpawnMode::Chat
        } else {
            SpawnMode::Auto
        }
    } else {
        config.mode
    };
    match effective_mode {
        SpawnMode::Auto => {
            cmd.arg("--dangerously-skip-permissions");
        }
        SpawnMode::SmartApprove => {
            cmd.args(["--permission-mode", "acceptEdits"]);
        }
        SpawnMode::Chat => {
            // No extra flags — interactive approval mode.
        }
    }

    // ── Initial model ────────────────────────────────────────────────────────
    // Can be switched mid-session via set_model control_request (no re-spawn needed).
    // Skip if the model string is a file-system path (contains '/') — that means
    // the caller passed a command-path alias, not an actual Claude model ID.
    if let Some(ref model) = config.model {
        if !model.is_empty() && model != "default" && !model.contains('/') {
            cmd.args(["--model", model]);
        }
    }

    // ── System prompt (fixed for lifetime of subprocess) ────────────────────
    // Changing this requires a re-spawn; ManagedProcess detects and handles this.
    if let Some(ref sys) = config.system_prompt {
        if !sys.is_empty() {
            cmd.args(["--system-prompt", sys]);
        }
    }

    // ── MCP configuration ────────────────────────────────────────────────────
    if let Some(ref mcp_path) = config.mcp_config {
        if let Some(path_str) = mcp_path.to_str() {
            cmd.args(["--mcp-config", path_str]);
        }
        if config.mcp_strict {
            cmd.arg("--strict-mcp-config");
        }
    }

    cmd
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn std_args(config: &SpawnConfig) -> Vec<String> {
        let mut cmd = std::process::Command::new(&config.command);
        cmd.arg("--print");
        cmd.args(["--input-format", "stream-json", "--output-format", "stream-json"]);
        cmd.arg("--verbose");
        cmd.arg("--include-partial-messages");
        let effective_mode = if config.mode == SpawnMode::Auto {
            #[cfg(unix)]
            let is_root = unsafe { libc::getuid() } == 0;
            #[cfg(not(unix))]
            let is_root = false;
            if is_root { SpawnMode::Chat } else { SpawnMode::Auto }
        } else {
            config.mode
        };
        match effective_mode {
            SpawnMode::Auto => {
                cmd.arg("--dangerously-skip-permissions");
            }
            SpawnMode::SmartApprove => {
                cmd.args(["--permission-mode", "acceptEdits"]);
            }
            SpawnMode::Chat => {}
        }
        if let Some(ref model) = config.model {
            if !model.is_empty() && model != "default" && !model.contains('/') {
                cmd.args(["--model", model]);
            }
        }
        if let Some(ref sys) = config.system_prompt {
            if !sys.is_empty() {
                cmd.args(["--system-prompt", sys]);
            }
        }
        if let Some(ref mcp_path) = config.mcp_config {
            if let Some(p) = mcp_path.to_str() {
                cmd.args(["--mcp-config", p]);
            }
            if config.mcp_strict {
                cmd.arg("--strict-mcp-config");
            }
        }
        cmd.get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect()
    }

    fn chat_cfg() -> SpawnConfig {
        SpawnConfig {
            command: "claude".into(),
            mode: SpawnMode::Chat,
            mcp_config: None,
            mcp_strict: false,
            model: None,
            system_prompt: None,
        }
    }

    #[test]
    fn always_has_ndjson_verbose_partial_flags() {
        let args = std_args(&chat_cfg());
        assert!(args.contains(&"--print".into()));
        assert!(args.contains(&"--input-format".into()));
        assert!(args.contains(&"stream-json".into()));
        assert!(args.contains(&"--output-format".into()));
        assert!(args.contains(&"--verbose".into()));
        assert!(args.contains(&"--include-partial-messages".into()));
    }

    #[test]
    fn auto_mode_adds_skip_permissions() {
        let cfg = SpawnConfig { mode: SpawnMode::Auto, ..chat_cfg() };
        let args = std_args(&cfg);
        // When running as root (uid==0), Auto mode is downgraded to Chat to
        // avoid the "cannot use --dangerously-skip-permissions as root" error.
        #[cfg(unix)]
        let is_root = unsafe { libc::getuid() } == 0;
        #[cfg(not(unix))]
        let is_root = false;
        if is_root {
            assert!(!args.contains(&"--dangerously-skip-permissions".into()));
        } else {
            assert!(args.contains(&"--dangerously-skip-permissions".into()));
        }
        assert!(!args.contains(&"--permission-mode".into()));
    }

    #[test]
    fn smart_approve_mode() {
        let cfg = SpawnConfig { mode: SpawnMode::SmartApprove, ..chat_cfg() };
        let args = std_args(&cfg);
        assert!(args.contains(&"--permission-mode".into()));
        assert!(args.contains(&"acceptEdits".into()));
        assert!(!args.contains(&"--dangerously-skip-permissions".into()));
    }

    #[test]
    fn chat_mode_no_permission_flags() {
        let args = std_args(&chat_cfg());
        assert!(!args.contains(&"--dangerously-skip-permissions".into()));
        assert!(!args.contains(&"--permission-mode".into()));
    }

    #[test]
    fn model_flag_included() {
        let cfg = SpawnConfig {
            model: Some("claude-opus-4-6".into()),
            ..chat_cfg()
        };
        let args = std_args(&cfg);
        assert!(args.contains(&"--model".into()));
        assert!(args.contains(&"claude-opus-4-6".into()));
    }

    #[test]
    fn model_default_not_passed() {
        let cfg = SpawnConfig {
            model: Some("default".into()),
            ..chat_cfg()
        };
        let args = std_args(&cfg);
        assert!(!args.contains(&"--model".into()));
    }

    #[test]
    fn model_path_not_passed() {
        // When the model ID is actually a file path (command alias fallback),
        // --model must NOT be passed to avoid an "unknown model" error.
        let cfg = SpawnConfig {
            model: Some("/usr/local/bin/claude".into()),
            ..chat_cfg()
        };
        let args = std_args(&cfg);
        assert!(!args.contains(&"--model".into()));
    }

    #[test]
    fn system_prompt_flag_included() {
        let cfg = SpawnConfig {
            system_prompt: Some("You are a Rust expert.".into()),
            ..chat_cfg()
        };
        let args = std_args(&cfg);
        assert!(args.contains(&"--system-prompt".into()));
        assert!(args.contains(&"You are a Rust expert.".into()));
    }

    #[test]
    fn empty_system_prompt_not_passed() {
        let cfg = SpawnConfig {
            system_prompt: Some(String::new()),
            ..chat_cfg()
        };
        let args = std_args(&cfg);
        assert!(!args.contains(&"--system-prompt".into()));
    }

    #[test]
    fn mcp_config_flags() {
        let cfg = SpawnConfig {
            mcp_config: Some(PathBuf::from("/tmp/mcp.json")),
            mcp_strict: true,
            ..chat_cfg()
        };
        let args = std_args(&cfg);
        assert!(args.contains(&"--mcp-config".into()));
        assert!(args.contains(&"/tmp/mcp.json".into()));
        assert!(args.contains(&"--strict-mcp-config".into()));
    }

    #[test]
    fn no_mcp_no_mcp_flags() {
        let args = std_args(&chat_cfg());
        assert!(!args.contains(&"--mcp-config".into()));
        assert!(!args.contains(&"--strict-mcp-config".into()));
    }
}
