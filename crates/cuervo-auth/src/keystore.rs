use cuervo_core::error::{CuervoError, Result};

/// Secure storage for API keys and tokens.
///
/// Uses the OS keychain (macOS Keychain, Windows Credential Manager,
/// Linux Secret Service) via the `keyring` crate.
pub struct KeyStore {
    service_name: String,
}

impl KeyStore {
    pub fn new(service_name: &str) -> Self {
        Self {
            service_name: service_name.to_string(),
        }
    }

    /// Store a secret in the OS keychain.
    pub fn set_secret(&self, key: &str, value: &str) -> Result<()> {
        let entry = keyring::Entry::new(&self.service_name, key)
            .map_err(|e| CuervoError::AuthFailed(format!("keychain access: {e}")))?;
        entry
            .set_password(value)
            .map_err(|e| CuervoError::AuthFailed(format!("keychain store: {e}")))?;
        Ok(())
    }

    /// Retrieve a secret from the OS keychain.
    pub fn get_secret(&self, key: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(&self.service_name, key)
            .map_err(|e| CuervoError::AuthFailed(format!("keychain access: {e}")))?;
        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(CuervoError::AuthFailed(format!("keychain read: {e}"))),
        }
    }

    /// Delete a secret from the OS keychain.
    pub fn delete_secret(&self, key: &str) -> Result<()> {
        let entry = keyring::Entry::new(&self.service_name, key)
            .map_err(|e| CuervoError::AuthFailed(format!("keychain access: {e}")))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(CuervoError::AuthFailed(format!("keychain delete: {e}"))),
        }
    }
}
