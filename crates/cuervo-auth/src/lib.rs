//! Authentication module for Cuervo CLI.
//!
//! Implements:
//! - OAuth 2.0 Authorization Code + PKCE (browser login)
//! - Device Authorization Flow (RFC 8628) for SSO
//! - OS keychain storage via `keyring` crate
//! - JWT validation for cuervo-auth-service tokens
//! - API key management (Anthropic, OpenAI, etc.)

pub mod keystore;
pub mod oauth;
pub mod pkce;

pub use keystore::KeyStore;
pub use oauth::{AuthorizeRequest, OAuthFlow, TokenResponse};
pub use pkce::PkceChallenge;
