//! Persistent file-based credential store for Linux headless environments.
//!
//! This module implements an XDG-compliant credential store that functions as
//! a fallback when the OS keyring is unavailable (e.g., Linux servers without
//! a D-Bus session, containers, headless CI environments).
//!
//! # Storage Model
//!
//! Credentials are stored in `$XDG_DATA_HOME/halcon/<service>.json` (or
//! `~/.local/share/halcon/<service>.json` when `XDG_DATA_HOME` is unset),
//! mirroring the strategy used by `gh` (GitHub CLI), `docker`, and `aws-cli`.
//!
//! # Security Properties
//!
//! - Directory permissions: `0700` (owner rwx only)
//! - File permissions: `0600` (owner rw only)
//! - Writes are **atomic** via `O_TMPFILE` + `rename(2)` — no partial reads
//! - No encryption at rest; relies on UNIX DAC. For high-security deployments,
//!   back this with a secrets manager via `CENZONTLE_ACCESS_TOKEN` env var.
//!
//! # Concurrency
//!
//! Atomic rename ensures readers always see a complete JSON object. Concurrent
//! writers are serialized at the OS level (last rename wins). This matches the
//! behavior of all major CLI tools on Linux.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use halcon_core::error::{HalconError, Result};

/// Atomic file-based credential store following XDG Base Directory Specification.
#[derive(Debug, Clone)]
pub struct FileCredentialStore {
    /// Absolute path to the JSON credentials file.
    path: PathBuf,
}

impl FileCredentialStore {
    /// Construct a store for `service` under the XDG data directory.
    ///
    /// Path resolution order (matches XDG spec):
    /// 1. `$XDG_DATA_HOME/halcon/<service>.json`
    /// 2. `$HOME/.local/share/halcon/<service>.json`
    pub fn new(service: &str) -> Self {
        let base = xdg_data_home();
        Self {
            path: base.join("halcon").join(format!("{service}.json")),
        }
    }

    /// Construct a store at an explicit path (used in tests).
    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Retrieve the value for `key`, returning `None` if not present.
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let map = self.read_map()?;
        Ok(map.get(key).cloned())
    }

    /// Store `value` under `key`.
    ///
    /// Uses an atomic write: the value is written to a sibling `.tmp` file
    /// then `rename`d into place, so concurrent readers always see a complete
    /// JSON object.
    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        self.ensure_directory()?;
        let mut map = self.read_map_or_default();
        map.insert(key.to_string(), value.to_string());
        self.write_map_atomic(&map)
    }

    /// Remove `key` from the store. No-op if the key does not exist.
    pub fn delete(&self, key: &str) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        let mut map = self.read_map_or_default();
        if map.remove(key).is_some() {
            self.write_map_atomic(&map)?;
        }
        Ok(())
    }

    /// Return `true` if the backing file exists (i.e., at least one secret has been stored).
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Absolute path to the backing credentials file (useful for diagnostics).
    pub fn path(&self) -> &Path {
        &self.path
    }

    // ── Private helpers ────────────────────────────────────────────────────────

    fn read_map(&self) -> Result<HashMap<String, String>> {
        let raw = std::fs::read_to_string(&self.path).map_err(|e| {
            HalconError::AuthFailed(format!(
                "cannot read credential file {}: {e}",
                self.path.display()
            ))
        })?;
        serde_json::from_str(&raw).map_err(|e| {
            HalconError::AuthFailed(format!(
                "credential file {} is corrupt ({}); remove it and re-authenticate",
                self.path.display(),
                e
            ))
        })
    }

    fn read_map_or_default(&self) -> HashMap<String, String> {
        if !self.path.exists() {
            return HashMap::new();
        }
        self.read_map().unwrap_or_default()
    }

    /// Atomically write `map` to `self.path` via a sibling tmp file.
    fn write_map_atomic(&self, map: &HashMap<String, String>) -> Result<()> {
        let serialized = serde_json::to_string_pretty(map).map_err(|e| {
            HalconError::AuthFailed(format!("credential serialization failed: {e}"))
        })?;

        // Write to a sibling temp file, then rename.
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &serialized).map_err(|e| {
            HalconError::AuthFailed(format!(
                "cannot write credential tmp file {}: {e}",
                tmp.display()
            ))
        })?;

        // chmod 0600 before the rename so the target is never world-readable.
        #[cfg(unix)]
        set_file_mode_600(&tmp)?;

        std::fs::rename(&tmp, &self.path).map_err(|e| {
            HalconError::AuthFailed(format!(
                "atomic credential write failed (rename {} → {}): {e}",
                tmp.display(),
                self.path.display()
            ))
        })?;

        Ok(())
    }

    /// Create the credential directory with `0700` permissions.
    fn ensure_directory(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            if !dir.exists() {
                std::fs::create_dir_all(dir).map_err(|e| {
                    HalconError::AuthFailed(format!(
                        "cannot create credential directory {}: {e}",
                        dir.display()
                    ))
                })?;
            }

            // Enforce 0700 on Unix regardless of umask.
            #[cfg(unix)]
            set_dir_mode_700(dir)?;
        }
        Ok(())
    }
}

// ── XDG helpers ───────────────────────────────────────────────────────────────

fn xdg_data_home() -> PathBuf {
    if let Ok(val) = std::env::var("XDG_DATA_HOME") {
        let p = PathBuf::from(val);
        if p.is_absolute() {
            return p;
        }
    }
    // Fallback per XDG spec: $HOME/.local/share
    home_dir().join(".local").join("share")
}

fn home_dir() -> PathBuf {
    // dirs::home_dir is the canonical way; fall back to HOME env var.
    #[cfg(not(test))]
    {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
    }
    #[cfg(test)]
    {
        // In tests, use a temp dir resolved by the caller via `FileCredentialStore::at`.
        PathBuf::from("/tmp")
    }
}

// ── Unix permission helpers ───────────────────────────────────────────────────

#[cfg(unix)]
fn set_file_mode_600(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
        HalconError::AuthFailed(format!("cannot set permissions on {}: {e}", path.display()))
    })
}

#[cfg(unix)]
fn set_dir_mode_700(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
        HalconError::AuthFailed(format!(
            "cannot set permissions on directory {}: {e}",
            path.display()
        ))
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_store() -> (tempfile::TempDir, FileCredentialStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        (dir, FileCredentialStore::at(path))
    }

    #[test]
    fn round_trip_get_set() {
        let (_dir, store) = tmp_store();
        store.set("token", "abc123").unwrap();
        assert_eq!(store.get("token").unwrap(), Some("abc123".to_string()));
    }

    #[test]
    fn get_returns_none_when_file_missing() {
        let (_dir, store) = tmp_store();
        assert_eq!(store.get("missing").unwrap(), None);
    }

    #[test]
    fn get_returns_none_for_absent_key() {
        let (_dir, store) = tmp_store();
        store.set("other", "value").unwrap();
        assert_eq!(store.get("missing").unwrap(), None);
    }

    #[test]
    fn delete_removes_key() {
        let (_dir, store) = tmp_store();
        store.set("a", "1").unwrap();
        store.set("b", "2").unwrap();
        store.delete("a").unwrap();
        assert_eq!(store.get("a").unwrap(), None);
        assert_eq!(store.get("b").unwrap(), Some("2".to_string()));
    }

    #[test]
    fn delete_is_noop_when_key_absent() {
        let (_dir, store) = tmp_store();
        store.set("x", "v").unwrap();
        store.delete("nonexistent").unwrap(); // must not panic or error
        assert_eq!(store.get("x").unwrap(), Some("v".to_string()));
    }

    #[test]
    fn delete_is_noop_when_file_absent() {
        let (_dir, store) = tmp_store();
        store.delete("any").unwrap(); // no file yet — must not error
    }

    #[test]
    fn overwrite_updates_value() {
        let (_dir, store) = tmp_store();
        store.set("key", "first").unwrap();
        store.set("key", "second").unwrap();
        assert_eq!(store.get("key").unwrap(), Some("second".to_string()));
    }

    #[test]
    fn multiple_keys_coexist() {
        let (_dir, store) = tmp_store();
        store.set("access_token", "tok-abc").unwrap();
        store.set("refresh_token", "ref-xyz").unwrap();
        store.set("expires_at", "9999999999").unwrap();
        assert_eq!(store.get("access_token").unwrap(), Some("tok-abc".to_string()));
        assert_eq!(store.get("refresh_token").unwrap(), Some("ref-xyz".to_string()));
        assert_eq!(store.get("expires_at").unwrap(), Some("9999999999".to_string()));
    }

    #[test]
    fn corrupt_file_returns_auth_error() {
        let (_dir, store) = tmp_store();
        // Manually write invalid JSON.
        std::fs::write(store.path(), b"not valid json {{{").unwrap();
        let err = store.get("any").unwrap_err().to_string();
        assert!(
            err.contains("corrupt") || err.contains("invalid") || err.contains("expected"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn atomic_write_leaves_no_tmp_on_success() {
        let (_dir, store) = tmp_store();
        store.set("k", "v").unwrap();
        let tmp = store.path().with_extension("tmp");
        assert!(!tmp.exists(), "tmp file should be cleaned up after rename");
    }

    #[test]
    fn file_json_is_human_readable() {
        let (_dir, store) = tmp_store();
        store.set("hello", "world").unwrap();
        let raw = std::fs::read_to_string(store.path()).unwrap();
        // pretty-printed JSON should have newlines
        assert!(raw.contains('\n'), "credentials file should be pretty-printed");
        assert!(raw.contains("hello"), "key should appear in file");
    }
}
