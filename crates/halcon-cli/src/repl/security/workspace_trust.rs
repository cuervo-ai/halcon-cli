//! Workspace Trust — directory-level trust gate (Gate 1 of Trust Chain).
//!
//! Implements the same security model as Claude Code and VS Code:
//! - First session in a directory → mandatory trust modal
//! - Restricted mode until trust is explicitly granted
//! - Trust stored per canonical path
//! - Revocable via CLI command
//!
//! # Threat mitigated
//!
//! **Malicious repo clone**: Attacker commits `.halcon/HALCON.md` with
//! instructions to exfiltrate secrets. Without workspace trust, the agent
//! would auto-load those instructions and execute tools in the untrusted
//! directory.
//!
//! # Trust chain position
//!
//! ```text
//! Gate 1: Workspace Trust ← THIS
//!   → Gate 2: Project Config Trust (HALCON.md)
//!     → Gate 3: MCP Server Approval
//!       → Gate 4: Tool Permissions (per-execution)
//! ```

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

// ── Trust Decision ───────────────────────────────────────────────────────

/// The user's explicit trust decision for a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustDecision {
    /// User explicitly trusted this workspace.
    Trusted,
    /// User explicitly denied trust — restricted mode.
    Denied,
    /// Trust not yet decided (first access).
    Unknown,
}

/// Stored trust record for a workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustRecord {
    /// Canonical path of the trusted directory.
    pub path: String,
    /// Trust decision.
    pub decision: TrustDecision,
    /// Unix timestamp when trust was granted/denied.
    pub decided_at: u64,
    /// SHA-256 of the path (for fast lookup).
    pub path_hash: String,
}

// ── Workspace Trust Store ────────────────────────────────────────────────

/// Persistent workspace trust store.
///
/// Trust decisions are stored in `~/.halcon/workspace-trust.json`.
/// This file is user-local (never committed to repos) and contains
/// the canonical paths of trusted/denied workspaces.
///
/// # Security properties
///
/// - Trust is per **canonical** path (symlinks resolved)
/// - Subdirectories inherit parent trust (like VS Code)
/// - Trust can be revoked at any time
/// - Default is **untrusted** (deny-by-default)
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkspaceTrustStore {
    version: u32,
    workspaces: Vec<TrustRecord>,
}

impl Default for WorkspaceTrustStore {
    fn default() -> Self {
        Self {
            version: 1,
            workspaces: Vec::new(),
        }
    }
}

impl WorkspaceTrustStore {
    /// Load trust store from disk, or create empty if not found.
    pub fn load() -> Self {
        let path = Self::store_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|e| {
                warn!("Corrupt workspace-trust.json, resetting: {e}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Persist trust store to disk.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::store_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&path, json)
    }

    /// Check if a directory is trusted.
    ///
    /// Returns `TrustDecision::Unknown` if the directory has never been
    /// evaluated. Checks parent directories for inherited trust.
    pub fn check(&self, dir: &Path) -> TrustDecision {
        let canonical = match dir.canonicalize() {
            Ok(p) => p,
            Err(_) => dir.to_path_buf(),
        };

        // Check exact match first
        for record in &self.workspaces {
            if let Ok(stored) = PathBuf::from(&record.path).canonicalize() {
                if stored == canonical {
                    return record.decision.clone();
                }
            }
        }

        // Check parent directories (trust inheritance)
        for record in &self.workspaces {
            if record.decision == TrustDecision::Trusted {
                if let Ok(stored) = PathBuf::from(&record.path).canonicalize() {
                    if canonical.starts_with(&stored) {
                        debug!(
                            parent = %stored.display(),
                            child = %canonical.display(),
                            "Workspace trust inherited from parent"
                        );
                        return TrustDecision::Trusted;
                    }
                }
            }
        }

        TrustDecision::Unknown
    }

    /// Record a trust decision for a directory.
    pub fn set_trust(&mut self, dir: &Path, decision: TrustDecision) {
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        let path_str = canonical.to_string_lossy().to_string();
        let path_hash = sha256_hex(&path_str);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Update existing or insert new
        if let Some(record) = self
            .workspaces
            .iter_mut()
            .find(|r| r.path_hash == path_hash)
        {
            record.decision = decision;
            record.decided_at = now;
        } else {
            self.workspaces.push(TrustRecord {
                path: path_str,
                decision,
                decided_at: now,
                path_hash,
            });
        }

        info!(dir = %canonical.display(), "Workspace trust decision recorded");
    }

    /// Revoke trust for a directory.
    pub fn revoke(&mut self, dir: &Path) {
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        let path_hash = sha256_hex(&canonical.to_string_lossy());
        self.workspaces.retain(|r| r.path_hash != path_hash);
        info!(dir = %canonical.display(), "Workspace trust revoked");
    }

    /// List all trusted workspaces.
    pub fn list_trusted(&self) -> Vec<&TrustRecord> {
        self.workspaces
            .iter()
            .filter(|r| r.decision == TrustDecision::Trusted)
            .collect()
    }

    /// Storage path: ~/.halcon/workspace-trust.json
    fn store_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".halcon")
            .join("workspace-trust.json")
    }
}

// ── Restricted Mode ──────────────────────────────────────────────────────

/// Capabilities available in restricted mode (untrusted workspace).
///
/// When a workspace is untrusted, only read-only operations are allowed.
/// No tools, no HALCON.md loading, no MCP connections.
#[derive(Debug, Clone)]
pub struct RestrictedMode {
    /// Directory that triggered restricted mode.
    pub directory: PathBuf,
    /// Why the workspace is restricted.
    pub reason: RestrictedReason,
}

#[derive(Debug, Clone)]
pub enum RestrictedReason {
    /// User has not yet decided to trust this workspace.
    FirstAccess,
    /// User explicitly denied trust.
    ExplicitlyDenied,
}

impl RestrictedMode {
    /// Tools blocked in restricted mode.
    pub fn blocked_tool_categories(&self) -> &[&str] {
        &[
            "bash",
            "file_write",
            "file_edit",
            "file_delete",
            "git_commit",
            "git_push",
            "background_start",
        ]
    }

    /// Whether HALCON.md should be loaded.
    pub fn allow_project_instructions(&self) -> bool {
        false
    }

    /// Whether MCP servers should be connected.
    pub fn allow_mcp_connections(&self) -> bool {
        false
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn unknown_directory_returns_unknown() {
        let store = WorkspaceTrustStore::default();
        let dir = PathBuf::from("/tmp/nonexistent-test-dir");
        assert_eq!(store.check(&dir), TrustDecision::Unknown);
    }

    #[test]
    fn trusted_directory_returns_trusted() {
        let tmp = TempDir::new().unwrap();
        let mut store = WorkspaceTrustStore::default();
        store.set_trust(tmp.path(), TrustDecision::Trusted);
        assert_eq!(store.check(tmp.path()), TrustDecision::Trusted);
    }

    #[test]
    fn denied_directory_returns_denied() {
        let tmp = TempDir::new().unwrap();
        let mut store = WorkspaceTrustStore::default();
        store.set_trust(tmp.path(), TrustDecision::Denied);
        assert_eq!(store.check(tmp.path()), TrustDecision::Denied);
    }

    #[test]
    fn child_inherits_parent_trust() {
        let tmp = TempDir::new().unwrap();
        let child = tmp.path().join("subdir");
        fs::create_dir_all(&child).unwrap();

        let mut store = WorkspaceTrustStore::default();
        store.set_trust(tmp.path(), TrustDecision::Trusted);

        assert_eq!(store.check(&child), TrustDecision::Trusted);
    }

    #[test]
    fn revoke_removes_trust() {
        let tmp = TempDir::new().unwrap();
        let mut store = WorkspaceTrustStore::default();
        store.set_trust(tmp.path(), TrustDecision::Trusted);
        assert_eq!(store.check(tmp.path()), TrustDecision::Trusted);

        store.revoke(tmp.path());
        assert_eq!(store.check(tmp.path()), TrustDecision::Unknown);
    }

    #[test]
    fn restricted_mode_blocks_destructive_tools() {
        let rm = RestrictedMode {
            directory: PathBuf::from("/tmp/untrusted"),
            reason: RestrictedReason::FirstAccess,
        };
        assert!(rm.blocked_tool_categories().contains(&"bash"));
        assert!(rm.blocked_tool_categories().contains(&"file_write"));
        assert!(!rm.allow_project_instructions());
        assert!(!rm.allow_mcp_connections());
    }
}
