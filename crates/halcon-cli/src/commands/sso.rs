//! SSO login/logout for the Cenzontle AI platform via Zuclubit SSO.
//!
//! Implements the OAuth 2.1 Authorization Code + PKCE (S256) flow:
//!
//! 1. Generate code_verifier (32 random bytes) and code_challenge (SHA-256 / base64url).
//! 2. Open the user's browser to the SSO authorization URL.
//! 3. Spin up a local HTTP server on `http://localhost:9876/callback`.
//! 4. Receive the authorization code redirect.
//! 5. Exchange the code for access + refresh tokens at the SSO token endpoint.
//! 6. Store tokens in the OS keychain via `halcon-cli` service.
//! 7. Optionally call Cenzontle `/v1/llm/models` to show available models.
//!
//! # Environment variables
//!
//! - `ZUCLUBIT_SSO_URL` — override SSO base URL (default: `https://sso.zuclubit.com`)
//! - `CENZONTLE_BASE_URL` — override Cenzontle base URL (default: `https://api.cenzontle.app`)
//! - `HALCON_SSO_CLIENT_SECRET` — CI bypass: use client_credentials grant instead of browser

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use base64::Engine as _;
use halcon_auth::KeyStore;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const SERVICE_NAME: &str = "halcon-cli";
const DEFAULT_SSO_URL: &str = "https://sso.zuclubit.com";
const DEFAULT_CENZONTLE_URL: &str = "https://api.cenzontle.app";
const CLIENT_ID: &str = "halcon-cli";
const REDIRECT_PORT: u16 = 9_876;
const SCOPES: &str = "openid profile email offline_access";

// Keychain keys
const KEY_ACCESS_TOKEN: &str = "cenzontle:access_token";
const KEY_REFRESH_TOKEN: &str = "cenzontle:refresh_token";
const KEY_EXPIRES_AT: &str = "cenzontle:expires_at";

/// Perform the SSO login flow for `cenzontle`.
///
/// Opens a browser window to Zuclubit SSO, waits for the authorization code
/// callback, exchanges it for tokens, stores them in the OS keychain, and
/// prints the list of AI models available to this account.
pub async fn login() -> Result<()> {
    let sso_url = std::env::var("ZUCLUBIT_SSO_URL")
        .unwrap_or_else(|_| DEFAULT_SSO_URL.to_string());
    let cenzontle_url = std::env::var("CENZONTLE_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_CENZONTLE_URL.to_string());

    // CI bypass: client_credentials grant when a non-empty secret is available.
    if let Ok(secret) = std::env::var("HALCON_SSO_CLIENT_SECRET") {
        if !secret.is_empty() {
            return login_client_credentials(&sso_url, &cenzontle_url, &secret).await;
        }
    }

    login_pkce(&sso_url, &cenzontle_url).await
}

/// Remove the stored Cenzontle tokens from the OS keychain.
pub fn logout() -> Result<()> {
    let keystore = KeyStore::new(SERVICE_NAME);

    // Check whether a session actually exists before deleting.
    let had_session = keystore
        .get_secret(KEY_ACCESS_TOKEN)
        .ok()
        .flatten()
        .is_some();

    for key in [KEY_ACCESS_TOKEN, KEY_REFRESH_TOKEN, KEY_EXPIRES_AT] {
        let _ = keystore.delete_secret(key);
    }

    if had_session {
        println!("Cenzontle session removed from OS keychain.");
    } else {
        println!("No active Cenzontle session found.");
    }
    Ok(())
}

/// Show the current Cenzontle SSO session status.
pub fn status() -> Result<()> {
    let keystore = KeyStore::new(SERVICE_NAME);

    let has_token = keystore
        .get_secret(KEY_ACCESS_TOKEN)
        .ok()
        .flatten()
        .is_some();

    if !has_token {
        println!("cenzontle: not logged in  (run `halcon auth login cenzontle` to authenticate)");
        return Ok(());
    }

    let expires_at: Option<u64> = keystore
        .get_secret(KEY_EXPIRES_AT)
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok());

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    match expires_at {
        Some(exp) if exp > now => {
            let remaining = exp - now;
            println!("cenzontle: logged in  (token expires in {}s)", remaining);
        }
        Some(_) => {
            println!("cenzontle: token expired  (run `halcon login cenzontle` to refresh)");
        }
        None => {
            println!("cenzontle: logged in  (expiry unknown)");
        }
    }

    Ok(())
}

/// Try to silently refresh the access token using the stored refresh token.
///
/// Returns `true` if the token was refreshed successfully.
pub async fn refresh_if_needed() -> bool {
    let keystore = KeyStore::new(SERVICE_NAME);

    let expires_at: Option<u64> = keystore
        .get_secret(KEY_EXPIRES_AT)
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok());

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Only refresh if token is expiring within 5 minutes or already expired.
    let needs_refresh = match expires_at {
        Some(exp) => exp < now + 300,
        None => false,
    };

    if !needs_refresh {
        return false;
    }

    let refresh_token = match keystore.get_secret(KEY_REFRESH_TOKEN).ok().flatten() {
        Some(t) => t,
        None => return false,
    };

    let sso_url = std::env::var("ZUCLUBIT_SSO_URL")
        .unwrap_or_else(|_| DEFAULT_SSO_URL.to_string());

    match do_refresh(&sso_url, &refresh_token).await {
        Ok((access_token, new_refresh, expires_in)) => {
            store_tokens(&access_token, new_refresh.as_deref(), expires_in);
            tracing::debug!("Cenzontle: access token refreshed silently");
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "Cenzontle: silent token refresh failed");
            false
        }
    }
}

// ── PKCE Authorization Code Flow ─────────────────────────────────────────────

async fn login_pkce(sso_url: &str, cenzontle_url: &str) -> Result<()> {
    // 1. Generate PKCE pair.
    let code_verifier = generate_code_verifier();
    let code_challenge = compute_code_challenge(&code_verifier);
    let state = generate_state();

    // 2. Build authorization URL.
    let redirect_uri = format!("http://localhost:{REDIRECT_PORT}/callback");
    let auth_url = format!(
        "{sso_url}/oauth/authorize?\
         client_id={CLIENT_ID}\
         &response_type=code\
         &redirect_uri={redirect_uri_enc}\
         &scope={scope_enc}\
         &code_challenge={code_challenge}\
         &code_challenge_method=S256\
         &state={state}",
        redirect_uri_enc = percent_encode(&redirect_uri),
        scope_enc = percent_encode(SCOPES),
    );

    // 3. Start local callback server.
    let listener = TcpListener::bind(format!("127.0.0.1:{REDIRECT_PORT}"))
        .await
        .map_err(|e| anyhow!("Failed to bind callback server on port {REDIRECT_PORT}: {e}"))?;

    // 4. Open browser.
    println!("Opening browser for Cenzontle SSO login...");
    println!("  URL: {}", &auth_url[..auth_url.find('?').unwrap_or(auth_url.len())]);
    if let Err(e) = open::that(&auth_url) {
        println!("Could not open browser automatically. Please visit:");
        println!("  {auth_url}");
        tracing::debug!(error = %e, "Browser open failed");
    }
    println!("Waiting for authorization callback on http://localhost:{REDIRECT_PORT}/callback ...");

    // 5. Accept one connection and extract the authorization code.
    let code = tokio::time::timeout(
        Duration::from_secs(120),
        accept_callback(&listener, &state),
    )
    .await
    .map_err(|_| anyhow!("Authorization timed out (120s). Please try again."))?
    .map_err(|e| anyhow!("Callback error: {e}"))?;

    // 6. Exchange code for tokens.
    println!("Exchanging authorization code for tokens...");
    let token_url = format!("{sso_url}/oauth/token");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    let mut params = HashMap::new();
    params.insert("grant_type", "authorization_code");
    params.insert("code", &code);
    params.insert("redirect_uri", &redirect_uri);
    params.insert("client_id", CLIENT_ID);
    params.insert("code_verifier", &code_verifier);

    let resp = http
        .post(&token_url)
        .form(&params)
        .send()
        .await
        .map_err(|e| anyhow!("Token exchange request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Token exchange failed (HTTP {status}): {body}"));
    }

    let token_resp: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| anyhow!("Failed to parse token response: {e}"))?;

    let access_token = token_resp["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("No access_token in response"))?
        .to_string();
    let refresh_token = token_resp["refresh_token"].as_str().map(String::from);
    let expires_in = token_resp["expires_in"].as_u64().unwrap_or(900);

    // 7. Store tokens.
    store_tokens(&access_token, refresh_token.as_deref(), expires_in);
    println!("Cenzontle session stored in OS keychain.");

    // 8. Show available models.
    show_available_models(cenzontle_url, &access_token).await;

    Ok(())
}

// ── Client credentials bypass (CI/CD) ────────────────────────────────────────

async fn login_client_credentials(
    sso_url: &str,
    cenzontle_url: &str,
    client_secret: &str,
) -> Result<()> {
    println!("Authenticating via client_credentials grant (CI/CD mode)...");

    let token_url = format!("{sso_url}/oauth/token");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    let mut params = HashMap::new();
    params.insert("grant_type", "client_credentials");
    params.insert("client_id", CLIENT_ID);
    params.insert("client_secret", client_secret);
    params.insert("scope", SCOPES);

    let resp = http
        .post(&token_url)
        .form(&params)
        .send()
        .await
        .map_err(|e| anyhow!("Token request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Client credentials grant failed (HTTP {status}): {body}"));
    }

    let token_resp: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| anyhow!("Failed to parse token response: {e}"))?;

    let access_token = token_resp["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("No access_token in response"))?
        .to_string();
    let refresh_token = token_resp["refresh_token"].as_str().map(String::from);
    let expires_in = token_resp["expires_in"].as_u64().unwrap_or(900);

    store_tokens(&access_token, refresh_token.as_deref(), expires_in);
    println!("Cenzontle session stored in OS keychain.");
    show_available_models(cenzontle_url, &access_token).await;

    Ok(())
}

// ── Token refresh ─────────────────────────────────────────────────────────────

async fn do_refresh(
    sso_url: &str,
    refresh_token: &str,
) -> Result<(String, Option<String>, u64)> {
    let token_url = format!("{sso_url}/oauth/token");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    let mut params = HashMap::new();
    params.insert("grant_type", "refresh_token");
    params.insert("client_id", CLIENT_ID);
    params.insert("refresh_token", refresh_token);

    let resp = http
        .post(&token_url)
        .form(&params)
        .send()
        .await
        .map_err(|e| anyhow!("Refresh request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Token refresh failed (HTTP {status}): {body}"));
    }

    let body: serde_json::Value = resp.json().await?;
    let access = body["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("No access_token in refresh response"))?
        .to_string();
    let new_refresh = body["refresh_token"].as_str().map(String::from);
    let expires_in = body["expires_in"].as_u64().unwrap_or(900);

    Ok((access, new_refresh, expires_in))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Minimal percent-encoding for OAuth query parameters (RFC 3986 unreserved chars pass through).
fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0').to_ascii_uppercase());
                out.push(char::from_digit((byte & 0xf) as u32, 16).unwrap_or('0').to_ascii_uppercase());
            }
        }
    }
    out
}

/// Minimal percent-decoding for OAuth callback parameters.
fn percent_decode(input: &str) -> String {
    let mut out = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (
                char::from_u32(bytes[i + 1] as u32).and_then(|c| c.to_digit(16)),
                char::from_u32(bytes[i + 2] as u32).and_then(|c| c.to_digit(16)),
            ) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn generate_code_verifier() -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

fn compute_code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let digest = hasher.finalize();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn generate_state() -> String {
    let mut buf = [0u8; 16];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Wait for the OAuth callback request and extract the authorization code.
async fn accept_callback(listener: &TcpListener, expected_state: &str) -> Result<String> {
    let (mut stream, _addr) = listener
        .accept()
        .await
        .map_err(|e| anyhow!("Failed to accept callback connection: {e}"))?;

    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .await
        .map_err(|e| anyhow!("Failed to read callback request: {e}"))?;

    // Parse GET /callback?code=...&state=... HTTP/1.1
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();

    let response_body = "Authentication successful! You can close this tab.";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response_body.len(),
        response_body
    );
    stream
        .write_all(response.as_bytes())
        .await
        .ok();

    // Parse query string from path.
    let query = path.splitn(2, '?').nth(1).unwrap_or("");
    let params: HashMap<&str, &str> = query
        .split('&')
        .filter_map(|kv| {
            let mut parts = kv.splitn(2, '=');
            Some((parts.next()?, parts.next()?))
        })
        .collect();

    if let Some(error) = params.get("error") {
        return Err(anyhow!("SSO authorization error: {error}"));
    }

    let state = params.get("state").copied().unwrap_or("");
    if state != expected_state {
        return Err(anyhow!("State mismatch in OAuth callback (CSRF protection)"));
    }

    params
        .get("code")
        .map(|c| percent_decode(c))
        .ok_or_else(|| anyhow!("No authorization code in callback"))
}

fn store_tokens(access_token: &str, refresh_token: Option<&str>, expires_in: u64) {
    let keystore = KeyStore::new(SERVICE_NAME);

    if let Err(e) = keystore.set_secret(KEY_ACCESS_TOKEN, access_token) {
        tracing::warn!(error = %e, "Failed to store Cenzontle access token in keychain — token will not persist across sessions");
    }
    if let Some(rt) = refresh_token {
        if let Err(e) = keystore.set_secret(KEY_REFRESH_TOKEN, rt) {
            tracing::warn!(error = %e, "Failed to store Cenzontle refresh token in keychain");
        }
    }
    let expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + expires_in;
    if let Err(e) = keystore.set_secret(KEY_EXPIRES_AT, &expires_at.to_string()) {
        tracing::warn!(error = %e, "Failed to store Cenzontle token expiry in keychain");
    }
}

async fn show_available_models(cenzontle_url: &str, access_token: &str) {
    let url = format!("{cenzontle_url}/v1/llm/models");
    let http = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };

    match http
        .get(&url)
        .bearer_auth(access_token)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                println!("\nModels available in your Cenzontle account:");
                if let Some(data) = body["data"].as_array() {
                    if data.is_empty() {
                        println!("  (no models available — check your account permissions)");
                    } else {
                        for model in data {
                            let id = model["id"].as_str().unwrap_or("?");
                            let name = model["name"].as_str().unwrap_or(id);
                            let tier = model["tier"].as_str().unwrap_or("UNKNOWN");
                            println!("  [{tier}] {name} ({id})");
                        }
                    }
                }
            }
        }
        Ok(resp) => {
            tracing::debug!(status = %resp.status(), "Cenzontle models endpoint returned non-200");
        }
        Err(e) => {
            tracing::debug!(error = %e, "Could not fetch Cenzontle models");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_challenge_known_vector() {
        // RFC 7636 Appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = compute_code_challenge(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn state_is_hex_32_chars() {
        let state = generate_state();
        assert_eq!(state.len(), 32);
        assert!(state.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn code_verifier_is_base64url() {
        let verifier = generate_code_verifier();
        // base64url of 32 bytes = ceil(32*4/3) = 43 chars (no padding)
        assert_eq!(verifier.len(), 43);
        assert!(verifier.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'));
    }
}
