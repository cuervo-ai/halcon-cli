//! Secure credential storage abstraction.
//!
//! [`KeyStore`] is the public API used throughout halcon to store and retrieve
//! secrets (API keys, OAuth tokens, etc.). It delegates to
//! [`CredentialManager`], which automatically selects the most appropriate
//! backend for the current platform and environment:
//!
//! | Platform | Backend |
//! |----------|---------|
//! | macOS | Keychain (Secure Enclave) |
//! | Windows | Windows Credential Manager |
//! | Linux + D-Bus | Secret Service (GNOME Keyring / KWallet) |
//! | Linux headless | XDG file store (`~/.local/share/halcon/<svc>.json`, `0600`) |
//!
//! All operations are logged at `DEBUG` level so that `RUST_LOG=debug` gives a
//! clear picture of what backend is in use during diagnostics.

use halcon_core::error::Result;

use crate::credential_manager::{CredentialBackend, CredentialManager};

/// Secure storage for API keys and tokens.
///
/// Uses the best available OS credential store via [`CredentialManager`], with
/// an automatic fallback to a permission-locked XDG file store on Linux.
pub struct KeyStore {
    manager: CredentialManager,
}

impl KeyStore {
    /// Create a key store for the given `service_name`.
    ///
    /// The backend is selected once at construction time.  Subsequent calls to
    /// `get_secret`, `set_secret`, and `delete_secret` always use the same
    /// backend.
    pub fn new(service_name: &str) -> Self {
        Self {
            manager: CredentialManager::new(service_name),
        }
    }

    /// The active backend — useful for surfacing in `auth status` / `debug`.
    pub fn backend(&self) -> &CredentialBackend {
        self.manager.backend()
    }

    /// A human-readable description of the active backend and its persistence
    /// properties (e.g. `"Linux file store — persistent (chmod 0600)"`).
    pub fn backend_info(&self) -> String {
        self.manager.backend_info()
    }

    /// Retrieve a secret from the credential store.
    ///
    /// Returns:
    /// - `Ok(Some(value))` — secret found.
    /// - `Ok(None)` — secret not present (no entry).
    /// - `Err(_)` — I/O or credential-store error (always propagated; never
    ///   silently swallowed).
    pub fn get_secret(&self, key: &str) -> Result<Option<String>> {
        self.manager.get(key)
    }

    /// Store a secret in the credential store.
    ///
    /// On Linux with the file-store backend, the write is atomic (`O_TMPFILE` +
    /// `rename(2)`) and the file is created with mode `0600`.
    pub fn set_secret(&self, key: &str, value: &str) -> Result<()> {
        self.manager.set(key, value)
    }

    /// Remove a secret from the credential store.
    ///
    /// No-op if the key does not exist (not an error).
    pub fn delete_secret(&self, key: &str) -> Result<()> {
        self.manager.delete(key)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // KeyStore wraps CredentialManager; the thorough backend tests live in
    // credential_manager.rs and file_store.rs.  These tests verify the public
    // API surface compiles and delegates correctly.

    #[test]
    fn backend_info_is_non_empty() {
        let ks = KeyStore::new("halcon-test");
        assert!(!ks.backend_info().is_empty());
    }

    #[test]
    fn backend_returns_a_variant() {
        let ks = KeyStore::new("halcon-test");
        // Just verify it compiles and returns something — the specific variant
        // depends on the test environment.
        let _b = ks.backend();
    }

    // Integration round-trip — only runs when the test environment has a working
    // credential backend (skips gracefully otherwise via the file-store fallback).
    #[test]
    fn set_get_delete_round_trip() {
        let ks = KeyStore::new("halcon-test-keystore");
        let key = "__test_round_trip__";
        let value = "halcon-test-value-abc123";

        // Set
        ks.set_secret(key, value).expect("set_secret should not fail");

        // Get
        let got = ks.get_secret(key).expect("get_secret should not fail");
        assert_eq!(got, Some(value.to_string()));

        // Delete
        ks.delete_secret(key).expect("delete_secret should not fail");

        // Gone
        let gone = ks.get_secret(key).expect("get_secret after delete should not fail");
        assert_eq!(gone, None);
    }

    #[test]
    fn get_returns_none_for_missing_key() {
        let ks = KeyStore::new("halcon-test-keystore");
        let result = ks.get_secret("__definitely_does_not_exist_xyz__");
        assert!(result.is_ok(), "get_secret should not error on missing key");
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn delete_is_noop_for_missing_key() {
        let ks = KeyStore::new("halcon-test-keystore");
        let result = ks.delete_secret("__also_does_not_exist_xyz__");
        assert!(result.is_ok(), "delete_secret should not error on missing key");
    }
}
