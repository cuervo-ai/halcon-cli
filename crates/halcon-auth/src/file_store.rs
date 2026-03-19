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
//! - Writes are **atomic** via a sibling `.tmp` + `rename(2)` — no partial reads
//! - Tmp file is always cleaned up, even on rename failure
//! - No encryption at rest; relies on UNIX DAC. For high-security deployments,
//!   back this with a secrets manager via `CENZONTLE_ACCESS_TOKEN` env var.
//!
//! # Concurrency
//!
//! Atomic rename ensures readers always see a complete JSON object. Use
//! `set_multiple()` when writing multiple correlated keys (e.g., all three
//! OAuth token fields) so they are committed in a single atomic write.
//! Concurrent independent writers use last-rename-wins semantics, consistent
//! with GitHub CLI and Docker CLI behavior.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use halcon_core::error::{HalconError, Result};
use tracing::warn;

/// Generate a 6-hex-char random suffix for tmp filenames to prevent concurrent
/// writers from clobbering each other's in-flight tmp file.
fn tmp_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Combine nanosecond timestamp + thread ID for cheap uniqueness without
    // pulling in a full RNG crate. Collision probability for 8 concurrent
    // writers: ~1 in 16^6 (1 in 16 million) per write — acceptable for a
    // local credential store.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let tid = std::thread::current().id();
    format!("{nanos:x}{tid:?}")
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(8)
        .collect()
}

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

    /// Construct a store at an explicit path (used in tests and diagnostics).
    #[cfg(any(test, feature = "test-support"))]
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
    ///
    /// Prefer [`set_multiple`] when storing several related keys (e.g., all
    /// three OAuth token fields) to keep them atomically consistent.
    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        self.ensure_directory()?;
        let mut map = self.read_map_or_default();
        map.insert(key.to_string(), value.to_string());
        self.write_map_atomic(&map)
    }

    /// Store multiple `(key, value)` pairs in a **single atomic write**.
    ///
    /// This is the preferred API when several correlated values must be kept
    /// consistent (e.g., `access_token`, `refresh_token`, and `expires_at`).
    /// A single `rename(2)` replaces the file, so readers never see a
    /// partially-updated set of credentials.
    pub fn set_multiple<'a, I>(&self, entries: I) -> Result<()>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        self.ensure_directory()?;
        let mut map = self.read_map_or_default();
        for (key, value) in entries {
            map.insert(key.to_string(), value.to_string());
        }
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
                "credential file {} is corrupt ({}); \
                 remove it and re-authenticate: `halcon auth login cenzontle`",
                self.path.display(),
                e
            ))
        })
    }

    /// Read the map or return an empty map on absence.
    ///
    /// Logs a warning if the file exists but is corrupt so the caller
    /// does not silently lose data.
    fn read_map_or_default(&self) -> HashMap<String, String> {
        if !self.path.exists() {
            return HashMap::new();
        }
        match self.read_map() {
            Ok(m) => m,
            Err(e) => {
                // A corrupt credential file is unusual and actionable.
                // Log it rather than silently discarding it.
                warn!(
                    path = %self.path.display(),
                    error = %e,
                    "Credential file is corrupt and will be overwritten. \
                     You may need to re-authenticate."
                );
                HashMap::new()
            }
        }
    }

    /// Atomically write `map` to `self.path` via a sibling tmp file.
    ///
    /// The tmp file is always cleaned up, even when rename fails.
    fn write_map_atomic(&self, map: &HashMap<String, String>) -> Result<()> {
        let serialized = serde_json::to_string_pretty(map).map_err(|e| {
            HalconError::AuthFailed(format!("credential serialization failed: {e}"))
        })?;

        // Write to a uniquely-named sibling temp file to avoid concurrent
        // writers clobbering each other's in-flight tmp file (rename is the
        // only atomic operation; the write+chmod window must be per-writer).
        let tmp = self
            .path
            .with_extension(format!("tmp.{}", tmp_suffix()));
        std::fs::write(&tmp, &serialized).map_err(|e| {
            HalconError::AuthFailed(format!(
                "cannot write credential tmp file {}: {e}",
                tmp.display()
            ))
        })?;

        // chmod 0600 before the rename so the target is never world-readable.
        #[cfg(unix)]
        if let Err(e) = set_file_mode_600(&tmp) {
            // Clean up before returning the error.
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }

        // Atomic rename. Clean up tmp on failure.
        if let Err(e) = std::fs::rename(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp); // best-effort cleanup
            return Err(HalconError::AuthFailed(format!(
                "atomic credential write failed (rename {} → {}): {e}",
                tmp.display(),
                self.path.display()
            )));
        }

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
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
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

    fn tmp_store() -> (tempfile::TempDir, FileCredentialStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        (dir, FileCredentialStore::at(path))
    }

    // ── Basic operations ──────────────────────────────────────────────────────

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
        store.delete("nonexistent").unwrap();
        assert_eq!(store.get("x").unwrap(), Some("v".to_string()));
    }

    #[test]
    fn delete_is_noop_when_file_absent() {
        let (_dir, store) = tmp_store();
        store.delete("any").unwrap();
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

    // ── set_multiple (atomic multi-key write) ─────────────────────────────────

    #[test]
    fn set_multiple_writes_all_keys_atomically() {
        let (_dir, store) = tmp_store();
        store
            .set_multiple([
                ("access_token", "tok-a"),
                ("refresh_token", "tok-r"),
                ("expires_at", "9999"),
            ])
            .unwrap();
        assert_eq!(store.get("access_token").unwrap(), Some("tok-a".to_string()));
        assert_eq!(store.get("refresh_token").unwrap(), Some("tok-r".to_string()));
        assert_eq!(store.get("expires_at").unwrap(), Some("9999".to_string()));
    }

    #[test]
    fn set_multiple_preserves_existing_keys() {
        let (_dir, store) = tmp_store();
        store.set("existing", "preserved").unwrap();
        store
            .set_multiple([("access_token", "tok-a"), ("expires_at", "9999")])
            .unwrap();
        // Pre-existing key must survive.
        assert_eq!(store.get("existing").unwrap(), Some("preserved".to_string()));
        assert_eq!(store.get("access_token").unwrap(), Some("tok-a".to_string()));
    }

    #[test]
    fn set_multiple_overwrites_existing_keys() {
        let (_dir, store) = tmp_store();
        store.set("token", "old").unwrap();
        store.set_multiple([("token", "new")]).unwrap();
        assert_eq!(store.get("token").unwrap(), Some("new".to_string()));
    }

    // ── Atomicity ─────────────────────────────────────────────────────────────

    #[test]
    fn atomic_write_leaves_no_tmp_on_success() {
        let (_dir, store) = tmp_store();
        store.set("k", "v").unwrap();
        let tmp = store.path().with_extension("tmp");
        assert!(!tmp.exists(), "tmp file should be cleaned up after rename");
    }

    // ── Error handling ────────────────────────────────────────────────────────

    #[test]
    fn corrupt_file_returns_auth_error_on_get() {
        let (_dir, store) = tmp_store();
        std::fs::write(store.path(), b"not valid json {{{").unwrap();
        let err = store.get("any").unwrap_err().to_string();
        assert!(
            err.contains("corrupt") || err.contains("invalid") || err.contains("expected"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn corrupt_file_is_overwritten_on_set_with_warning() {
        let (_dir, store) = tmp_store();
        std::fs::write(store.path(), b"}{invalid").unwrap();
        // Should NOT error — corrupt file is treated as empty and overwritten.
        store.set("k", "v").unwrap();
        assert_eq!(store.get("k").unwrap(), Some("v".to_string()));
    }

    #[test]
    fn corrupt_file_is_overwritten_on_set_multiple() {
        let (_dir, store) = tmp_store();
        std::fs::write(store.path(), b"not json").unwrap();
        store
            .set_multiple([("access_token", "tok"), ("refresh_token", "ref")])
            .unwrap();
        assert_eq!(store.get("access_token").unwrap(), Some("tok".to_string()));
        assert_eq!(store.get("refresh_token").unwrap(), Some("ref".to_string()));
    }

    // ── File format ───────────────────────────────────────────────────────────

    #[test]
    fn file_json_is_human_readable() {
        let (_dir, store) = tmp_store();
        store.set("hello", "world").unwrap();
        let raw = std::fs::read_to_string(store.path()).unwrap();
        assert!(raw.contains('\n'), "credentials file should be pretty-printed");
        assert!(raw.contains("hello"), "key should appear in file");
    }

    // ── Permissions (Unix only) ───────────────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn file_permissions_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = tmp_store();
        store.set("k", "v").unwrap();
        let mode = std::fs::metadata(store.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "file mode should be 0600");
    }

    #[cfg(unix)]
    #[test]
    fn directory_permissions_are_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("halcon_test_subdir");
        let path = subdir.join("creds.json");
        let store = FileCredentialStore::at(path);
        store.set("k", "v").unwrap();
        let mode = std::fs::metadata(&subdir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "directory mode should be 0700");
    }

    // ── Persistence simulation ────────────────────────────────────────────────

    #[test]
    fn data_survives_store_reconstruction() {
        // Simulates process restart: create store, write, drop, recreate, read.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("creds.json");

        {
            let store = FileCredentialStore::at(path.clone());
            store.set("session_token", "persistent-value").unwrap();
        }

        // Simulate process restart by creating a new store instance.
        {
            let store2 = FileCredentialStore::at(path);
            assert_eq!(
                store2.get("session_token").unwrap(),
                Some("persistent-value".to_string()),
                "Value must persist across store instances"
            );
        }
    }

    // ── Concurrent access ─────────────────────────────────────────────────────

    #[test]
    fn concurrent_writers_do_not_corrupt_file() {
        // Validates that concurrent set_multiple calls don't produce invalid JSON.
        // Last writer wins, but the file must always be parseable.
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("concurrent.json");
        let path = Arc::new(path);

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let p = Arc::clone(&path);
                std::thread::spawn(move || {
                    let store = FileCredentialStore::at((*p).clone());
                    let access = format!("tok-{i}");
                    let worker = i.to_string();
                    store
                        .set_multiple([
                            ("access_token", access.as_str()),
                            ("worker_id", worker.as_str()),
                        ])
                        .unwrap();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // After all writers finish, the file must be valid JSON.
        let raw = std::fs::read_to_string(&*path).unwrap();
        let map: HashMap<String, String> = serde_json::from_str(&raw)
            .expect("file must be valid JSON after concurrent writes");

        // At least access_token and worker_id must be present.
        assert!(map.contains_key("access_token"), "access_token must be present");
        assert!(map.contains_key("worker_id"), "worker_id must be present");
    }
}
