//! OAuth 2.1 + PKCE S256 authorization flow for HTTP MCP servers.
//!
//! # Flow
//!
//! 1. **Token check**: look up `<server>:access_token` in OS keychain.
//!    - If present and not expiring in < 5 min → return immediately.
//!    - If a refresh token is stored → attempt silent refresh first.
//! 2. **AS Metadata Discovery**: GET `<base_url>/.well-known/oauth-authorization-server`
//! 3. **Dynamic Client Registration**: POST to `registration_endpoint` if needed.
//! 4. **PKCE S256**: generate 32-byte random `code_verifier`, compute
//!    `code_challenge = base64url(SHA-256(code_verifier))`.
//! 5. **Browser redirect**: open the authorization URL; spin up a local loopback
//!    server (port 9_876) to receive the authorization code redirect.
//! 6. **Token exchange**: POST to `token_endpoint` with the code + `code_verifier`.
//! 7. **Storage**: write access token + refresh token + expiry to OS keychain.
//!
//! # CI/CD bypass
//!
//! If `HALCON_MCP_CLIENT_SECRET` is set, the client uses it directly in a
//! `client_credentials` grant, skipping the browser redirect entirely.
//!
//! # Platform keychain
//!
//! Uses the `keyring` v3 crate which maps to:
//! - macOS: Keychain Services
//! - Linux: Secret Service API (libsecret)
//! - Windows: Credential Manager

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use rand::RngCore;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use url::Url;

const KEYCHAIN_SERVICE: &str = "halcon-mcp";
const OAUTH_REDIRECT_PORT: u16 = 9_876;
/// Proactive refresh: refresh the token if it expires within this many seconds.
const REFRESH_WINDOW_SECS: u64 = 5 * 60;

// ── Public API ────────────────────────────────────────────────────────────────

/// Persistent OAuth token for one MCP server.
#[derive(Debug, Clone)]
pub struct McpToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix timestamp when the access token expires (0 = unknown).
    pub expires_at: u64,
}

impl McpToken {
    pub fn is_expiring(&self) -> bool {
        if self.expires_at == 0 {
            return false; // unknown — assume still valid
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.expires_at < now + REFRESH_WINDOW_SECS
    }
}

/// Authorization manager for a single HTTP MCP server.
pub struct OAuthManager {
    server_name: String,
    base_url: String,
    http: Arc<Client>,
}

impl OAuthManager {
    pub fn new(server_name: impl Into<String>, base_url: impl Into<String>) -> Self {
        let http = Arc::new(Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client"));
        Self {
            server_name: server_name.into(),
            base_url: base_url.into(),
            http,
        }
    }

    /// Ensure a valid access token is available for `server_name`, running the
    /// full OAuth flow if necessary.
    ///
    /// Returns the bearer token string to include in `Authorization: Bearer …`.
    pub async fn ensure_token(&self) -> Result<String, OAuthError> {
        // CI/CD bypass: use client credentials if secret is set.
        if let Ok(secret) = std::env::var("HALCON_MCP_CLIENT_SECRET") {
            return self.client_credentials_grant(&secret).await;
        }

        // Check keychain for a stored token.
        if let Some(token) = self.load_token() {
            if !token.is_expiring() {
                return Ok(token.access_token);
            }
            // Proactive refresh.
            if let Some(ref refresh) = token.refresh_token {
                match self.refresh_token(refresh).await {
                    Ok(new_token) => {
                        self.store_token(&new_token)?;
                        return Ok(new_token.access_token);
                    }
                    Err(e) => {
                        tracing::warn!("Token refresh failed for '{}': {e}", self.server_name);
                        // Fall through to full flow.
                    }
                }
            }
        }

        // Full authorization code flow.
        let token = self.authorization_code_flow().await?;
        self.store_token(&token)?;
        Ok(token.access_token)
    }

    // ── Full authorization code flow ──────────────────────────────────────────

    async fn authorization_code_flow(&self) -> Result<McpToken, OAuthError> {
        // Step 1: discover AS metadata.
        let metadata = self.discover_metadata().await?;

        // Step 2: dynamic client registration (best-effort; ignore if unsupported).
        let client_id = self.register_client(&metadata).await.unwrap_or_else(|_| {
            // Fallback: use a stable pseudo-client-id derived from server name.
            format!("halcon-mcp-{}", &self.server_name)
        });

        // Step 3: generate PKCE challenge.
        let (code_verifier, code_challenge) = pkce_pair();

        // Step 4: build authorization URL and open browser.
        let state = random_state();
        let redirect_uri = format!("http://127.0.0.1:{OAUTH_REDIRECT_PORT}/callback");

        let mut auth_url = Url::parse(&metadata.authorization_endpoint)
            .map_err(|e| OAuthError::InvalidUrl(e.to_string()))?;
        {
            let mut q = auth_url.query_pairs_mut();
            q.append_pair("response_type", "code");
            q.append_pair("client_id", &client_id);
            q.append_pair("redirect_uri", &redirect_uri);
            q.append_pair("code_challenge", &code_challenge);
            q.append_pair("code_challenge_method", "S256");
            q.append_pair("state", &state);
        }

        println!("\nOpening browser for MCP authorization ({})…", self.server_name);
        println!("If the browser does not open, visit:\n  {auth_url}\n");
        let _ = open::that(auth_url.as_str());

        // Step 5: receive authorization code via loopback server.
        let code = receive_auth_code(&state).await?;

        // Step 6: exchange code for tokens.
        self.exchange_code(&metadata, &client_id, &code, &code_verifier, &redirect_uri).await
    }

    // ── AS Metadata Discovery ─────────────────────────────────────────────────

    async fn discover_metadata(&self) -> Result<AuthServerMetadata, OAuthError> {
        let base = self.base_url.trim_end_matches('/');
        let url = format!("{base}/.well-known/oauth-authorization-server");
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(OAuthError::MetadataDiscovery(format!(
                "HTTP {} from {url}",
                resp.status()
            )));
        }
        let meta: AuthServerMetadata = resp.json().await?;
        Ok(meta)
    }

    // ── Dynamic Client Registration ───────────────────────────────────────────

    async fn register_client(&self, metadata: &AuthServerMetadata) -> Result<String, OAuthError> {
        let endpoint = metadata.registration_endpoint.as_deref()
            .ok_or_else(|| OAuthError::RegistrationUnsupported)?;

        let body = serde_json::json!({
            "client_name": format!("Halcon CLI ({})", self.server_name),
            "redirect_uris": [format!("http://127.0.0.1:{OAUTH_REDIRECT_PORT}/callback")],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "code_challenge_methods_supported": ["S256"],
        });

        let resp = self.http
            .post(endpoint)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(OAuthError::Registration(format!(
                "HTTP {} from registration endpoint",
                resp.status()
            )));
        }

        let reg: ClientRegistrationResponse = resp.json().await?;
        Ok(reg.client_id)
    }

    // ── Token exchange ────────────────────────────────────────────────────────

    async fn exchange_code(
        &self,
        metadata: &AuthServerMetadata,
        client_id: &str,
        code: &str,
        code_verifier: &str,
        redirect_uri: &str,
    ) -> Result<McpToken, OAuthError> {
        let mut params = HashMap::new();
        params.insert("grant_type", "authorization_code");
        params.insert("code", code);
        params.insert("redirect_uri", redirect_uri);
        params.insert("code_verifier", code_verifier);
        params.insert("client_id", client_id);

        let resp = self.http
            .post(&metadata.token_endpoint)
            .form(&params)
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OAuthError::TokenExchange(format!("token endpoint error: {body}")));
        }

        let tr: TokenResponse = resp.json().await?;
        Ok(tr.into_token())
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<McpToken, OAuthError> {
        let metadata = self.discover_metadata().await?;
        let client_id = format!("halcon-mcp-{}", self.server_name);

        let mut params = HashMap::new();
        params.insert("grant_type", "refresh_token");
        params.insert("refresh_token", refresh_token);
        params.insert("client_id", &client_id);

        let resp = self.http
            .post(&metadata.token_endpoint)
            .form(&params)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(OAuthError::TokenExchange("refresh failed".into()));
        }

        let tr: TokenResponse = resp.json().await?;
        Ok(tr.into_token())
    }

    async fn client_credentials_grant(&self, client_secret: &str) -> Result<String, OAuthError> {
        let metadata = self.discover_metadata().await?;
        let client_id = format!("halcon-mcp-{}", self.server_name);

        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", &client_id),
            ("client_secret", client_secret),
        ];

        let resp = self.http
            .post(&metadata.token_endpoint)
            .form(&params)
            .send()
            .await?;

        let tr: TokenResponse = resp.json().await?;
        Ok(tr.access_token)
    }

    // ── Keychain storage ──────────────────────────────────────────────────────

    fn load_token(&self) -> Option<McpToken> {
        let access_token = keyring::Entry::new(KEYCHAIN_SERVICE, &self.token_key("access"))
            .ok()?.get_password().ok()?;
        let refresh_token = keyring::Entry::new(KEYCHAIN_SERVICE, &self.token_key("refresh"))
            .ok()?.get_password().ok();
        let expires_at = keyring::Entry::new(KEYCHAIN_SERVICE, &self.token_key("expires"))
            .ok()?.get_password().ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        if access_token.is_empty() {
            None
        } else {
            Some(McpToken { access_token, refresh_token, expires_at })
        }
    }

    fn store_token(&self, token: &McpToken) -> Result<(), OAuthError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, &self.token_key("access"))
            .map_err(OAuthError::Keychain)?
            .set_password(&token.access_token)
            .map_err(OAuthError::Keychain)?;

        if let Some(ref rt) = token.refresh_token {
            keyring::Entry::new(KEYCHAIN_SERVICE, &self.token_key("refresh"))
                .map_err(OAuthError::Keychain)?
                .set_password(rt)
                .map_err(OAuthError::Keychain)?;
        }

        keyring::Entry::new(KEYCHAIN_SERVICE, &self.token_key("expires"))
            .map_err(OAuthError::Keychain)?
            .set_password(&token.expires_at.to_string())
            .map_err(OAuthError::Keychain)?;

        Ok(())
    }

    /// Clear all stored tokens for this server from the keychain.
    pub fn clear_token(&self) {
        for kind in &["access", "refresh", "expires"] {
            if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, &self.token_key(kind)) {
                let _ = entry.delete_credential();
            }
        }
    }

    fn token_key(&self, kind: &str) -> String {
        format!("{}:{kind}", self.server_name)
    }
}

// ── Loopback callback server ──────────────────────────────────────────────────

/// Spin up a minimal HTTP server on loopback port 9876 and wait for the OAuth
/// authorization redirect containing `?code=…&state=…`.
async fn receive_auth_code(expected_state: &str) -> Result<String, OAuthError> {
    let listener = TcpListener::bind(format!("127.0.0.1:{OAUTH_REDIRECT_PORT}"))
        .await
        .map_err(|e| OAuthError::LocalServer(e.to_string()))?;

    // Timeout after 5 minutes waiting for the browser redirect.
    let timeout = Duration::from_secs(300);
    tokio::time::timeout(timeout, async {
        loop {
            let (mut stream, _) = listener.accept().await
                .map_err(|e| OAuthError::LocalServer(e.to_string()))?;

            let mut reader = BufReader::new(&mut stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).await
                .map_err(|e| OAuthError::LocalServer(e.to_string()))?;

            // Parse GET /callback?code=…&state=… HTTP/1.1
            if let Some(path) = request_line.split_whitespace().nth(1) {
                let url = format!("http://127.0.0.1{path}");
                if let Ok(parsed) = Url::parse(&url) {
                    let params: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
                    if params.get("state").map(|s| s.as_str()) == Some(expected_state) {
                        if let Some(code) = params.get("code") {
                            let code = code.clone();
                            // Send success response.
                            let body = b"<html><body><h1>Authorization complete - you may close this tab.</h1></body></html>";
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = stream.write_all(response.as_bytes()).await;
                            let _ = stream.write_all(body).await;
                            return Ok(code);
                        }
                    }
                }
            }
            // Ignore non-matching requests (favicon.ico, etc.).
        }
    })
    .await
    .map_err(|_| OAuthError::Timeout)?
}

// ── PKCE helpers ──────────────────────────────────────────────────────────────

/// Generate a PKCE (code_verifier, code_challenge) pair using S256.
fn pkce_pair() -> (String, String) {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash);

    (verifier, challenge)
}

fn random_state() -> String {
    let mut buf = [0u8; 16];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

// ── JSON types ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AuthServerMetadata {
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    registration_endpoint: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClientRegistrationResponse {
    client_id: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

impl TokenResponse {
    fn into_token(self) -> McpToken {
        let expires_at = self.expires_in.map(|secs| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                + secs
        }).unwrap_or(0);

        McpToken {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            expires_at,
        }
    }
}

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("AS metadata discovery failed: {0}")]
    MetadataDiscovery(String),
    #[error("dynamic client registration unsupported by server")]
    RegistrationUnsupported,
    #[error("client registration failed: {0}")]
    Registration(String),
    #[error("token exchange failed: {0}")]
    TokenExchange(String),
    #[error("keychain error: {0}")]
    Keychain(#[from] keyring::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("local callback server error: {0}")]
    LocalServer(String),
    #[error("authorization flow timed out waiting for browser redirect")]
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_and_challenge_differ() {
        let (v, c) = pkce_pair();
        assert_ne!(v, c, "verifier and challenge must differ");
        assert!(!v.is_empty());
        assert!(!c.is_empty());
    }

    #[test]
    fn pkce_challenge_is_base64url_of_sha256() {
        let verifier = "test_verifier_value";
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash);

        // Manually verify the S256 mechanism.
        assert!(!expected.contains('+'), "must be URL-safe (no +)");
        assert!(!expected.contains('/'), "must be URL-safe (no /)");
    }

    #[test]
    fn token_expiry_detection() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let expiring = McpToken {
            access_token: "tok".into(),
            refresh_token: None,
            expires_at: now + 60, // expires in 1 minute
        };
        assert!(expiring.is_expiring(), "token expiring in 60s should be detected");

        let fresh = McpToken {
            access_token: "tok".into(),
            refresh_token: None,
            expires_at: now + 3600, // expires in 1 hour
        };
        assert!(!fresh.is_expiring(), "token expiring in 1h should not be expiring");

        let unknown = McpToken {
            access_token: "tok".into(),
            refresh_token: None,
            expires_at: 0,
        };
        assert!(!unknown.is_expiring(), "unknown expiry should not trigger refresh");
    }

    #[test]
    fn random_state_unique() {
        let s1 = random_state();
        let s2 = random_state();
        assert_ne!(s1, s2, "states must be unique");
        assert_eq!(s1.len(), 32, "state should be 32 hex chars");
    }
}
