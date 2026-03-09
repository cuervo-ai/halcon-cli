//! Environment Self-Repair — IMP-3
//!
//! Detects tool failures caused by recoverable environment state and applies
//! targeted repair actions before the next retry attempt.
//!
//! Design principles:
//! - **Minimal blast radius**: each repair is scoped to the specific error; no
//!   broad cleanup commands.
//! - **Idempotent**: every repair is safe to run multiple times.
//! - **Auditable**: every action emits a `tracing::info!` event tagged with
//!   `env_repair.*` fields for downstream observability.
//! - **Non-blocking**: synchronous by design; called from `run_with_retry`
//!   between attempts without async overhead.

/// The outcome of an environment repair attempt.
#[derive(Debug, Clone)]
pub struct EnvRepairResult {
    /// Whether the repair was successfully applied.
    pub repaired: bool,
    /// Human-readable description of what was done (or why it failed).
    pub description: String,
    /// The repair action that was attempted.
    pub action: EnvRepairAction,
}

/// The specific repair strategy to apply.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvRepairAction {
    /// Remove a Cargo build lock file (`.cargo-lock`).
    RemoveCargoLock { path: String },
    /// Create a missing directory (and all parents).
    CreateMissingDirectory { path: String },
    /// Remove a generic filesystem lock file.
    RemoveFileLock { path: String },
    /// Wait a short interval for a transient lock to clear (no filesystem ops).
    WaitForLockRelease { description: String },
}

impl std::fmt::Display for EnvRepairAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RemoveCargoLock { path } => write!(f, "remove cargo lock: {path}"),
            Self::CreateMissingDirectory { path } => write!(f, "create directory: {path}"),
            Self::RemoveFileLock { path } => write!(f, "remove file lock: {path}"),
            Self::WaitForLockRelease { description } => write!(f, "wait for lock: {description}"),
        }
    }
}

/// Analyse a tool error message and return applicable repair actions.
///
/// Called by `run_with_retry` in `executor.rs` when a tool fails and the
/// error matches an environment-level condition.  Returns an empty vec
/// when the error has no known repair strategy (the caller falls back to
/// plain exponential-backoff retry).
pub fn detect_repair_actions(
    error_message: &str,
    tool_name: &str,
    working_dir: &str,
) -> Vec<EnvRepairAction> {
    let lower = error_message.to_lowercase();
    let mut actions = Vec::new();

    // ── Cargo lock file ───────────────────────────────────────────────────────
    // Pattern: "failed to open: .../target/debug/.cargo-lock"
    //          "could not acquire package cache lock"
    //          "waiting for file lock on build directory"
    if lower.contains(".cargo-lock")
        || lower.contains("cargo-lock")
        || lower.contains("could not acquire package cache lock")
        || (lower.contains("file lock") && (lower.contains("build") || lower.contains("cargo")))
    {
        // Try to extract the exact path from the error message.
        let cargo_lock_path = extract_path_from_error(error_message, ".cargo-lock")
            .unwrap_or_else(|| format!("{working_dir}/target/debug/.cargo-lock"));
        actions.push(EnvRepairAction::RemoveCargoLock {
            path: cargo_lock_path,
        });
    }

    // ── Missing directory ─────────────────────────────────────────────────────
    // Pattern: "no such file or directory" when tool is mkdir/create_dir or
    //          the error path ends in a directory component.
    if lower.contains("no such file or directory")
        && (tool_name == "bash" || tool_name == "file_write" || tool_name == "patch_apply")
    {
        // Extract path from "No such file or directory (os error 2) for path 'X'"
        if let Some(dir) = extract_missing_dir(error_message) {
            actions.push(EnvRepairAction::CreateMissingDirectory { path: dir });
        }
    }

    // ── Generic file locks ────────────────────────────────────────────────────
    // Pattern: "Resource temporarily unavailable" / "EAGAIN" / ".lock" files
    // Skip if the cargo-lock rule already fired — avoid emitting two actions for the same lock.
    let cargo_lock_already_detected = actions.iter().any(|a| matches!(a, EnvRepairAction::RemoveCargoLock { .. }));
    if !cargo_lock_already_detected
        && (lower.contains("resource temporarily unavailable")
            || lower.contains("eagain")
            || (lower.contains("lock") && lower.contains("failed to open")))
    {
        if let Some(lock_path) = extract_path_from_error(error_message, ".lock") {
            actions.push(EnvRepairAction::RemoveFileLock { path: lock_path });
        } else {
            actions.push(EnvRepairAction::WaitForLockRelease {
                description: format!("{tool_name}: lock contention"),
            });
        }
    }

    actions
}

/// Execute a single repair action and return the result.
///
/// All file operations are best-effort: if they fail, `repaired` is false
/// and the description explains why, so the caller can log and continue.
pub fn execute_repair(action: &EnvRepairAction) -> EnvRepairResult {
    match action {
        EnvRepairAction::RemoveCargoLock { path } => {
            let path = std::path::Path::new(path);
            if !path.exists() {
                return EnvRepairResult {
                    repaired: true, // Already gone — repair "succeeded"
                    description: format!("cargo lock already absent: {}", path.display()),
                    action: action.clone(),
                };
            }
            match std::fs::remove_file(path) {
                Ok(()) => {
                    tracing::info!(
                        env_repair.action = "remove_cargo_lock",
                        env_repair.path = %path.display(),
                        "env-repair: removed .cargo-lock — tool will be retried"
                    );
                    EnvRepairResult {
                        repaired: true,
                        description: format!("removed cargo lock: {}", path.display()),
                        action: action.clone(),
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        env_repair.action = "remove_cargo_lock",
                        env_repair.path = %path.display(),
                        env_repair.error = %e,
                        "env-repair: failed to remove .cargo-lock"
                    );
                    EnvRepairResult {
                        repaired: false,
                        description: format!("failed to remove {}: {e}", path.display()),
                        action: action.clone(),
                    }
                }
            }
        }

        EnvRepairAction::CreateMissingDirectory { path } => {
            let path = std::path::Path::new(path);
            if path.exists() {
                return EnvRepairResult {
                    repaired: true,
                    description: format!("directory already exists: {}", path.display()),
                    action: action.clone(),
                };
            }
            match std::fs::create_dir_all(path) {
                Ok(()) => {
                    tracing::info!(
                        env_repair.action = "create_directory",
                        env_repair.path = %path.display(),
                        "env-repair: created missing directory — tool will be retried"
                    );
                    EnvRepairResult {
                        repaired: true,
                        description: format!("created directory: {}", path.display()),
                        action: action.clone(),
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        env_repair.action = "create_directory",
                        env_repair.path = %path.display(),
                        env_repair.error = %e,
                        "env-repair: failed to create directory"
                    );
                    EnvRepairResult {
                        repaired: false,
                        description: format!("failed to create {}: {e}", path.display()),
                        action: action.clone(),
                    }
                }
            }
        }

        EnvRepairAction::RemoveFileLock { path } => {
            let path = std::path::Path::new(path);
            if !path.exists() {
                return EnvRepairResult {
                    repaired: true,
                    description: format!("lock already absent: {}", path.display()),
                    action: action.clone(),
                };
            }
            match std::fs::remove_file(path) {
                Ok(()) => {
                    tracing::info!(
                        env_repair.action = "remove_file_lock",
                        env_repair.path = %path.display(),
                        "env-repair: removed file lock — tool will be retried"
                    );
                    EnvRepairResult {
                        repaired: true,
                        description: format!("removed lock: {}", path.display()),
                        action: action.clone(),
                    }
                }
                Err(e) => EnvRepairResult {
                    repaired: false,
                    description: format!("failed to remove lock {}: {e}", path.display()),
                    action: action.clone(),
                },
            }
        }

        EnvRepairAction::WaitForLockRelease { description } => {
            // Pure wait — no filesystem operation.  The backoff in run_with_retry
            // already waits, so this is just a marker for observability.
            tracing::info!(
                env_repair.action = "wait_for_lock",
                env_repair.description = %description,
                "env-repair: lock contention — relying on retry backoff"
            );
            EnvRepairResult {
                repaired: true, // "repaired" in the sense that we acknowledged it
                description: format!("waiting for lock to clear: {description}"),
                action: action.clone(),
            }
        }
    }
}

/// Execute all detected repair actions for a failed tool call.
///
/// Returns a combined result. If any repair succeeded, `repaired` is true.
pub fn run_repairs(
    error_message: &str,
    tool_name: &str,
    working_dir: &str,
) -> Option<Vec<EnvRepairResult>> {
    let actions = detect_repair_actions(error_message, tool_name, working_dir);
    if actions.is_empty() {
        return None;
    }
    let results: Vec<EnvRepairResult> = actions.iter().map(execute_repair).collect();
    Some(results)
}

// ── Path extraction helpers ───────────────────────────────────────────────────

/// Try to extract a path ending in `suffix` from an error message.
///
/// Looks for a path-like token (starts with `/` or `./` or contains `/`)
/// that ends with the given suffix.
fn extract_path_from_error(error: &str, suffix: &str) -> Option<String> {
    // Walk tokens split by whitespace and common delimiters.
    for token in error.split_whitespace() {
        // Strip common punctuation that surrounds paths.
        let tok = token.trim_matches(|c| matches!(c, '\'' | '"' | ',' | '(' | ')' | ':'));
        if tok.ends_with(suffix) && (tok.starts_with('/') || tok.contains('/')) {
            return Some(tok.to_string());
        }
    }
    None
}

/// Try to extract a directory path from a "No such file or directory" error.
///
/// Extracts the parent directory of the path that was not found.
fn extract_missing_dir(error: &str) -> Option<String> {
    // Common patterns:
    //   "No such file or directory (os error 2) for path '/foo/bar/baz.txt'"
    //   "error opening '/foo/bar/baz': No such file or directory"
    for token in error.split_whitespace() {
        let tok = token.trim_matches(|c| matches!(c, '\'' | '"' | ',' | '(' | ')' | ':'));
        if tok.starts_with('/') && tok.contains('/') {
            let path = std::path::Path::new(tok);
            // Only create the parent, not the file itself.
            if let Some(parent) = path.parent() {
                if parent != std::path::Path::new("") {
                    return Some(parent.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cargo_lock_from_error_message() {
        let err = "error: failed to open: /path/to/project/target/debug/.cargo-lock";
        let actions = detect_repair_actions(err, "lint_check", "/path/to/project");
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], EnvRepairAction::RemoveCargoLock { path } if path.contains(".cargo-lock")));
    }

    #[test]
    fn detects_cargo_lock_via_package_cache_message() {
        let err = "could not acquire package cache lock";
        let actions = detect_repair_actions(err, "bash", "/workspace");
        assert!(actions.iter().any(|a| matches!(a, EnvRepairAction::RemoveCargoLock { .. })));
    }

    #[test]
    fn detects_missing_directory_for_file_write() {
        let err = "No such file or directory (os error 2): '/workspace/src/new/subdir/file.rs'";
        let actions = detect_repair_actions(err, "file_write", "/workspace");
        assert!(actions.iter().any(|a| matches!(a, EnvRepairAction::CreateMissingDirectory { path } if path.contains("subdir"))));
    }

    #[test]
    fn no_actions_for_deterministic_error() {
        // Auth failures should NOT produce repair actions.
        let err = "Error: unauthorized — invalid API key";
        let actions = detect_repair_actions(err, "bash", "/workspace");
        assert!(actions.is_empty(), "Auth errors must not trigger env repair");
    }

    #[test]
    fn run_repairs_returns_none_when_no_actions() {
        let result = run_repairs("unknown tool 'xyz'", "bash", "/tmp");
        assert!(result.is_none());
    }

    #[test]
    fn execute_repair_cargo_lock_absent_reports_repaired() {
        let action = EnvRepairAction::RemoveCargoLock {
            path: "/nonexistent/path/.cargo-lock".into(),
        };
        let result = execute_repair(&action);
        // Non-existent lock → treat as "already gone" = repaired
        assert!(result.repaired);
    }

    #[test]
    fn execute_repair_creates_directory() {
        let tmp = std::env::temp_dir().join(format!("halcon_repair_test_{}", std::process::id()));
        let action = EnvRepairAction::CreateMissingDirectory {
            path: tmp.to_string_lossy().into_owned(),
        };
        let result = execute_repair(&action);
        assert!(result.repaired);
        assert!(tmp.exists());
        let _ = std::fs::remove_dir(tmp);
    }

    #[test]
    fn extract_path_finds_cargo_lock_in_error() {
        let err = "failed to open: /some/project/target/debug/.cargo-lock";
        let path = extract_path_from_error(err, ".cargo-lock");
        assert_eq!(path.as_deref(), Some("/some/project/target/debug/.cargo-lock"));
    }

    #[test]
    fn extract_missing_dir_finds_parent() {
        let err = "No such file or directory: '/workspace/src/new/file.rs'";
        let dir = extract_missing_dir(err);
        assert!(dir.is_some());
        assert!(dir.unwrap().contains("src/new"));
    }

    #[test]
    fn wait_for_lock_release_always_reports_repaired() {
        let action = EnvRepairAction::WaitForLockRelease {
            description: "test lock".into(),
        };
        let result = execute_repair(&action);
        assert!(result.repaired);
    }
}
