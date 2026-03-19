//! Authentication module for Halcon CLI.
//!
//! Implements:
//! - OAuth 2.0 Authorization Code + PKCE (browser login)
//! - Device Authorization Flow (RFC 8628) for SSO
//! - Platform-adaptive credential storage:
//!   - macOS: Keychain (Secure Enclave-backed)
//!   - Windows: Windows Credential Manager
//!   - Linux + D-Bus: Secret Service (GNOME Keyring / KWallet)
//!   - Linux headless: XDG file store (chmod 0600, atomic writes)
//! - JWT validation for halcon-auth-service tokens
//! - API key management (Anthropic, OpenAI, etc.)

pub mod credential_manager;
pub mod file_store;
pub mod keystore;
pub mod oauth;
pub mod pkce;
pub mod rbac;

pub use credential_manager::{CredentialBackend, CredentialManager};
pub use file_store::FileCredentialStore;
pub use keystore::KeyStore;
pub use oauth::{AuthorizeRequest, OAuthFlow, TokenResponse};
pub use pkce::PkceChallenge;
pub use rbac::Role;
