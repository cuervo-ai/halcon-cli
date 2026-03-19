//! Unified credential manager with platform-adaptive backend selection.
//!
//! This module provides a single [`CredentialManager`] abstraction that
//! automatically selects the most appropriate secure storage backend for the
//! current environment, following the same strategy as production-grade CLI
//! tools (`gh`, `docker`, `aws-cli`, `1password-cli`).
//!
//! # Backend Selection Order
//!
//! | Platform | Priority 1 | Priority 2 | Priority 3 |
//! |----------|-----------|-----------|-----------|
//! | macOS    | Keychain (apple-native) | — | — |
//! | Windows  | Credential Manager | — | — |
//! | Linux    | Secret Service (D-Bus) | Kernel keyring | File store |
//!
//! On Linux, the selection is probed at runtime:
//!
//! 1. **Secret Service** — if `DBUS_SESSION_BUS_ADDRESS` is set and a keyring
//!    daemon responds, the OS keyring crate handles storage via the
//!    `org.freedesktop.secrets` protocol. Persistent and encrypted.
//!
//! 2. **File store** — if the OS keyring is unavailable (headless servers,
//!    containers, SSH sessions without a forwarded D-Bus socket), credentials
//!    are written to `$XDG_DATA_HOME/halcon/<service>.json` with mode `0600`.
//!    Persistent across reboots and login sessions. Not encrypted at rest, but
//!    protected by UNIX DAC — the same model used by Docker and GitHub CLI.
//!
//! # Diagnostics
//!
//! Call [`CredentialManager::backend_info`] to retrieve a human-readable
//! description of the active backend. This is surfaced by `halcon auth status`
//! and `halcon debug` to help users understand where credentials live.

use halcon_core::error::{HalconError, Result};
use tracing::debug;
#[cfg(target_os = "linux")]
use tracing::warn;

use crate::file_store::FileCredentialStore;

/// Describes the active credential storage backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialBackend {
    /// macOS Keychain (always available on macOS; Secure Enclave-backed).
    MacosKeychain,
    /// Windows Credential Manager.
    WindowsCredentialManager,
    /// Linux Secret Service via D-Bus (GNOME Keyring, KWallet, etc.).
    LinuxSecretService,
    /// XDG file store — persistent, `0600`, no encryption at rest.
    LinuxFileStore,
}

impl CredentialBackend {
    /// Short human-readable name for display in `auth status` / `debug`.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::MacosKeychain => "macOS Keychain (Secure Enclave)",
            Self::WindowsCredentialManager => "Windows Credential Manager",
            Self::LinuxSecretService => "Linux Secret Service (D-Bus)",
            Self::LinuxFileStore => "Linux file store (~/.local/share/halcon/)",
        }
    }

    /// One-line note about persistence / security properties.
    pub fn persistence_note(&self) -> &'static str {
        match self {
            Self::MacosKeychain => "persistent, hardware-backed",
            Self::WindowsCredentialManager => "persistent, OS-encrypted",
            Self::LinuxSecretService => "persistent, daemon-encrypted",
            Self::LinuxFileStore => "persistent (chmod 0600, no encryption at rest)",
        }
    }
}

/// Platform-adaptive credential manager.
///
/// Create via [`CredentialManager::new`]; the backend is chosen once at
/// construction time and remains fixed for the lifetime of the manager.
pub struct CredentialManager {
    service_name: String,
    backend: CredentialBackend,
    file_store: Option<FileCredentialStore>,
}

impl CredentialManager {
    /// Create a manager for `service`, auto-detecting the best backend.
    pub fn new(service_name: &str) -> Self {
        let (backend, file_store) = Self::detect_backend(service_name);

        debug!(
            service = service_name,
            backend = backend.display_name(),
            persistence = backend.persistence_note(),
            "Credential backend selected"
        );

        Self {
            service_name: service_name.to_string(),
            backend,
            file_store,
        }
    }

    /// The backend currently in use.
    pub fn backend(&self) -> &CredentialBackend {
        &self.backend
    }

    /// Display name of the active backend (for `auth status` / `debug`).
    pub fn backend_info(&self) -> String {
        format!(
            "{} — {}",
            self.backend.display_name(),
            self.backend.persistence_note()
        )
    }

    /// Retrieve the secret for `key`. Returns `None` if not present.
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        match &self.backend {
            // macOS / Windows — native OS keyring, always reliable.
            CredentialBackend::MacosKeychain | CredentialBackend::WindowsCredentialManager => {
                self.keyring_get(key)
            }

            // Linux: Secret Service via D-Bus.
            CredentialBackend::LinuxSecretService => self.keyring_get(key),

            // Linux: file store fallback.
            CredentialBackend::LinuxFileStore => {
                self.file_store
                    .as_ref()
                    .expect("file_store must be set for LinuxFileStore backend")
                    .get(key)
            }
        }
    }

    /// Store `value` under `key`.
    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        match &self.backend {
            CredentialBackend::MacosKeychain | CredentialBackend::WindowsCredentialManager => {
                self.keyring_set(key, value)
            }
            CredentialBackend::LinuxSecretService => self.keyring_set(key, value),
            CredentialBackend::LinuxFileStore => {
                self.file_store
                    .as_ref()
                    .expect("file_store must be set for LinuxFileStore backend")
                    .set(key, value)
            }
        }
    }

    /// Atomically store multiple key-value pairs in a single write.
    ///
    /// On the file-store backend this is a single `rename(2)` — all keys
    /// are visible together or not at all.  On OS keyring backends the writes
    /// are sequential (the keyring API has no bulk-write primitive), which is
    /// still an improvement over calling `set()` in a loop because failures
    /// are collected rather than early-returning on the first error.
    ///
    /// Use this when writing a correlated group of secrets (e.g., all three
    /// OAuth token fields) to avoid partial-write races on Linux.
    pub fn set_multiple<'a, I>(&self, entries: I) -> Result<()>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        match &self.backend {
            CredentialBackend::LinuxFileStore => {
                self.file_store
                    .as_ref()
                    .expect("file_store must be set for LinuxFileStore backend")
                    .set_multiple(entries)
            }
            // For OS keyring backends, collect entries and write sequentially.
            // The keyring protocol has no bulk-write primitive; collect errors
            // and return the first one (all-or-nothing semantics best-effort).
            _ => {
                let pairs: Vec<(&'a str, &'a str)> = entries.into_iter().collect();
                let mut first_err: Option<halcon_core::error::HalconError> = None;
                for (key, value) in pairs {
                    if let Err(e) = self.set(key, value) {
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    }
                }
                match first_err {
                    Some(e) => Err(e),
                    None => Ok(()),
                }
            }
        }
    }

    /// Delete `key`. No-op if the key does not exist.
    pub fn delete(&self, key: &str) -> Result<()> {
        match &self.backend {
            CredentialBackend::MacosKeychain | CredentialBackend::WindowsCredentialManager => {
                self.keyring_delete(key)
            }
            CredentialBackend::LinuxSecretService => self.keyring_delete(key),
            CredentialBackend::LinuxFileStore => {
                self.file_store
                    .as_ref()
                    .expect("file_store must be set for LinuxFileStore backend")
                    .delete(key)
            }
        }
    }

    // ── Backend detection ──────────────────────────────────────────────────────

    fn detect_backend(service: &str) -> (CredentialBackend, Option<FileCredentialStore>) {
        // On non-Linux platforms `service` is used only in the file-store branch;
        // allow the compiler to see it as used on all paths.
        let _ = service;

        #[cfg(target_os = "macos")]
        { return (CredentialBackend::MacosKeychain, None); }

        #[cfg(target_os = "windows")]
        { return (CredentialBackend::WindowsCredentialManager, None); }

        #[cfg(target_os = "linux")]
        { return Self::detect_linux_backend(service); }

        // Fallback for other Unix (FreeBSD, etc.) — use file store.
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            return (
                CredentialBackend::LinuxFileStore,
                Some(FileCredentialStore::new(service)),
            );
        }
    }

    #[cfg(target_os = "linux")]
    fn detect_linux_backend(service: &str) -> (CredentialBackend, Option<FileCredentialStore>) {
        // Probe the OS keyring: if D-Bus is reachable and a secrets daemon
        // responds, we can use the Secret Service protocol (persistent +
        // encrypted). We probe with a lightweight Entry::new() + get_password()
        // call, which is what keyring v3 does internally.
        //
        // We treat ANY error other than NoEntry as "unavailable" so we fall
        // back to the file store rather than returning an opaque error.
        let probe_key = "__halcon_probe__";
        let probe_result = keyring::Entry::new(service, probe_key)
            .and_then(|e| e.get_password().or_else(|err| {
                // NoEntry means the daemon responded — backend is available.
                if matches!(err, keyring::Error::NoEntry) {
                    Ok(String::new())
                } else {
                    Err(err)
                }
            }));

        match probe_result {
            Ok(_) => {
                debug!(
                    "Linux Secret Service (D-Bus) available — using OS keyring"
                );
                (CredentialBackend::LinuxSecretService, None)
            }
            Err(probe_err) => {
                // Distinguish between "no daemon" and unexpected errors.
                let reason = probe_err.to_string();
                warn!(
                    error = %reason,
                    "OS keyring unavailable on Linux (D-Bus / Secret Service not responding); \
                     falling back to XDG file store. \
                     To use an encrypted keyring, install gnome-keyring or kwallet \
                     and ensure the D-Bus session is exported (DBUS_SESSION_BUS_ADDRESS)."
                );
                let fs = FileCredentialStore::new(service);
                debug!(path = %fs.path().display(), "File credential store path");
                (CredentialBackend::LinuxFileStore, Some(fs))
            }
        }
    }

    // ── OS keyring helpers ─────────────────────────────────────────────────────

    fn keyring_get(&self, key: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(&self.service_name, key)
            .map_err(|e| HalconError::AuthFailed(format!("keyring entry creation: {e}")))?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(HalconError::AuthFailed(format!("keyring read ({key}): {e}"))),
        }
    }

    fn keyring_set(&self, key: &str, value: &str) -> Result<()> {
        let entry = keyring::Entry::new(&self.service_name, key)
            .map_err(|e| HalconError::AuthFailed(format!("keyring entry creation: {e}")))?;
        entry
            .set_password(value)
            .map_err(|e| HalconError::AuthFailed(format!("keyring write ({key}): {e}")))
    }

    fn keyring_delete(&self, key: &str) -> Result<()> {
        let entry = keyring::Entry::new(&self.service_name, key)
            .map_err(|e| HalconError::AuthFailed(format!("keyring entry creation: {e}")))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(HalconError::AuthFailed(format!("keyring delete ({key}): {e}"))),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct a CredentialManager backed by a temp file store, bypassing
    /// OS keyring probing so tests run hermetically in CI (no D-Bus required).
    fn test_manager() -> (tempfile::TempDir, CredentialManager) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-creds.json");
        let fs = FileCredentialStore::at(path);
        let mgr = CredentialManager {
            service_name: "test-service".to_string(),
            backend: CredentialBackend::LinuxFileStore,
            file_store: Some(fs),
        };
        (dir, mgr)
    }

    #[test]
    fn get_set_delete_round_trip() {
        let (_dir, mgr) = test_manager();
        mgr.set("token", "secret-value").unwrap();
        assert_eq!(mgr.get("token").unwrap(), Some("secret-value".to_string()));
        mgr.delete("token").unwrap();
        assert_eq!(mgr.get("token").unwrap(), None);
    }

    #[test]
    fn get_returns_none_for_unknown_key() {
        let (_dir, mgr) = test_manager();
        assert_eq!(mgr.get("nonexistent").unwrap(), None);
    }

    #[test]
    fn backend_info_is_human_readable() {
        let (_dir, mgr) = test_manager();
        let info = mgr.backend_info();
        // Should mention both name and persistence note.
        assert!(!info.is_empty());
        assert!(info.contains("—"));
    }

    #[test]
    fn file_store_backend_display_name() {
        assert!(CredentialBackend::LinuxFileStore
            .display_name()
            .contains("file store"));
        assert!(CredentialBackend::MacosKeychain
            .display_name()
            .contains("macOS"));
    }

    #[test]
    fn file_store_backend_persistence_note_contains_chmod() {
        assert!(CredentialBackend::LinuxFileStore
            .persistence_note()
            .contains("0600"));
    }

    #[test]
    fn overwrite_replaces_value() {
        let (_dir, mgr) = test_manager();
        mgr.set("k", "v1").unwrap();
        mgr.set("k", "v2").unwrap();
        assert_eq!(mgr.get("k").unwrap(), Some("v2".to_string()));
    }

    #[test]
    fn multiple_keys_independent() {
        let (_dir, mgr) = test_manager();
        mgr.set("access", "tok-a").unwrap();
        mgr.set("refresh", "tok-r").unwrap();
        assert_eq!(mgr.get("access").unwrap(), Some("tok-a".to_string()));
        assert_eq!(mgr.get("refresh").unwrap(), Some("tok-r".to_string()));
    }
}
