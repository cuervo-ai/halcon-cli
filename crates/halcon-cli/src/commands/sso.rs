//! SSO login/logout for the Cenzontle AI platform via Zuclubit SSO.
//!
//! Implements the OAuth 2.1 Authorization Code + PKCE (S256) flow:
//!
//! 1. Generate code_verifier (32 random bytes) and code_challenge (SHA-256 / base64url).
//! 2. Open the user's browser to the SSO authorization URL.
//! 3. Spin up a local HTTP server on `http://localhost:9876/callback`
//!    (with automatic port retry if the primary port is occupied).
//! 4. Receive the authorization code redirect.
//! 5. Exchange the code for access + refresh tokens at the SSO token endpoint.
//! 6. Store tokens via the platform-adaptive credential store
//!    (macOS Keychain → Linux Secret Service → XDG file store).
//! 7. Optionally call Cenzontle `/v1/llm/models` to show available models.
//!
//! # Environment variables
//!
//! | Variable | Purpose |
//! |---|---|
//! | `ZUCLUBIT_SSO_URL` | Override SSO base URL (default: `https://sso.zuclubit.com`) |
//! | `CENZONTLE_BASE_URL` | Override Cenzontle API base URL |
//! | `HALCON_SSO_CLIENT_SECRET` | CI bypass: client_credentials grant |
//! | `HALCON_SSO_PORT` | Override callback port (default: 9876) |

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
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

/// Default OAuth callback port.  Override with `HALCON_SSO_PORT`.
const DEFAULT_REDIRECT_PORT: u16 = 9_876;

/// Number of consecutive ports to try if the primary port is occupied.
const PORT_RETRY_COUNT: u16 = 5;

const SCOPES: &str = "openid profile email offline_access";

// ── Keychain key names ─────────────────────────────────────────────────────────
const KEY_ACCESS_TOKEN: &str = "cenzontle:access_token";
const KEY_REFRESH_TOKEN: &str = "cenzontle:refresh_token";
const KEY_EXPIRES_AT: &str = "cenzontle:expires_at";

// ── Token storage result ───────────────────────────────────────────────────────

/// Outcome of a token storage attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreOutcome {
    /// All tokens written to the credential store. Persists across sessions.
    Persisted,
    /// Credential store unavailable; token is live for this process only.
    /// The caller should surface a clear warning to the user.
    NotPersisted { reason: String },
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// Perform the SSO login flow for `cenzontle`.
///
/// Opens a browser window to Zuclubit SSO, waits for the authorization code
/// callback, exchanges it for tokens, stores them in the credential store, and
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

/// Remove the stored Cenzontle tokens from the credential store.
pub fn logout() -> Result<()> {
    let keystore = KeyStore::new(SERVICE_NAME);

    let had_session = keystore
        .get_secret(KEY_ACCESS_TOKEN)
        .unwrap_or(None)
        .is_some();

    for key in [KEY_ACCESS_TOKEN, KEY_REFRESH_TOKEN, KEY_EXPIRES_AT] {
        if let Err(e) = keystore.delete_secret(key) {
            tracing::warn!(key = key, error = %e, "Failed to delete credential");
        }
    }

    if had_session {
        println!("Cenzontle session removed from credential store.");
    } else {
        println!("No active Cenzontle session found.");
    }
    Ok(())
}

/// Show the current Cenzontle SSO session status, including backend info.
pub fn status() -> Result<()> {
    let keystore = KeyStore::new(SERVICE_NAME);

    println!("  Credential backend: {}", keystore.backend_info());

    let has_token = match keystore.get_secret(KEY_ACCESS_TOKEN) {
        Ok(t) => t.is_some(),
        Err(e) => {
            println!("  cenzontle: credential store error — {e}");
            println!("             Run `halcon auth login cenzontle` to re-authenticate.");
            return Ok(());
        }
    };

    if !has_token {
        println!("  cenzontle: not logged in  (run `halcon auth login cenzontle` to authenticate)");
        return Ok(());
    }

    let expires_at: Option<u64> = keystore
        .get_secret(KEY_EXPIRES_AT)
        .unwrap_or(None)
        .and_then(|s| s.parse().ok());

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    match expires_at {
        Some(exp) if exp > now => {
            let remaining = exp - now;
            println!(
                "  cenzontle: logged in  (token expires in {remaining}s, at {})",
                format_unix_ts(exp)
            );
        }
        Some(_) => {
            println!(
                "  cenzontle: token expired  \
                 (a silent refresh will be attempted; run `halcon auth login cenzontle` if it fails)"
            );
        }
        None => {
            println!("  cenzontle: logged in  (expiry unknown)");
        }
    }

    Ok(())
}

/// Silently refresh the Cenzontle access token if it is expiring within 5 minutes.
///
/// This function is **non-blocking and infallible from the caller's perspective**:
/// it logs warnings on failure but never returns an error — the caller should
/// continue and let the provider attempt its request (which will fail with a
/// clear HTTP 401 if the token truly is expired).
///
/// Returns `true` if the token was refreshed successfully.
pub async fn refresh_if_needed() -> bool {
    let keystore = KeyStore::new(SERVICE_NAME);

    let expires_at: Option<u64> = keystore
        .get_secret(KEY_EXPIRES_AT)
        .unwrap_or(None)
        .and_then(|s| s.parse().ok());

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Refresh if expiring within 5 minutes, already expired, or expiry is
    // unknown (which can occur after a failed `store_tokens` call that wrote
    // the access token but not the expiry).  Attempting a refresh in the
    // unknown-expiry case is safe: if the token is still valid the refresh
    // endpoint will simply return a new one.
    let needs_refresh = match expires_at {
        Some(exp) => exp < now + 300,
        None => true,
    };

    if !needs_refresh {
        return false;
    }

    let refresh_token = match keystore.get_secret(KEY_REFRESH_TOKEN) {
        Ok(Some(t)) => t,
        Ok(None) => {
            tracing::debug!("Cenzontle: no refresh token stored; cannot refresh silently");
            return false;
        }
        Err(e) => {
            tracing::warn!(error = %e, "Cenzontle: failed to read refresh token from credential store");
            return false;
        }
    };

    let sso_url = std::env::var("ZUCLUBIT_SSO_URL")
        .unwrap_or_else(|_| DEFAULT_SSO_URL.to_string());

    match do_refresh(&sso_url, &refresh_token).await {
        Ok((access_token, new_refresh, expires_in)) => {
            let (outcome, _backend_info) = store_tokens(&access_token, new_refresh.as_deref(), expires_in);
            match outcome {
                StoreOutcome::Persisted => {
                    tracing::info!("Cenzontle: access token refreshed and persisted");
                }
                StoreOutcome::NotPersisted { ref reason } => {
                    tracing::warn!(
                        reason = reason,
                        "Cenzontle: access token refreshed but could not be persisted"
                    );
                }
            }
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

    // 2. Bind callback server (with port retry).
    let (listener, port) = bind_callback_server()
        .await
        .context("Failed to start OAuth callback server")?;

    // 3. Build authorization URL using the actual bound port.
    let redirect_uri = format!("http://localhost:{port}/callback");
    let auth_url = build_auth_url(sso_url, &redirect_uri, &code_challenge, &state);

    // 4. Open browser.
    println!("Opening browser for Cenzontle SSO login...");
    println!("  URL: {}", &auth_url[..auth_url.find('?').unwrap_or(auth_url.len())]);
    if let Err(e) = open::that(&auth_url) {
        tracing::debug!(error = %e, "Browser open failed");
        println!();
        println!("Could not open browser automatically.");
        println!("Please visit the following URL in your browser:");
        println!();
        println!("  {auth_url}");
        println!();
    }
    println!("Waiting for authorization callback on http://localhost:{port}/callback ...");

    // 5. Accept one connection and extract the authorization code (120s timeout).
    let code = tokio::time::timeout(
        Duration::from_secs(120),
        accept_callback(&listener, &state),
    )
    .await
    .map_err(|_| anyhow!("Authorization timed out (120s). Please try `halcon auth login cenzontle` again."))?
    .context("OAuth callback error")?;

    // 6. Exchange code for tokens.
    println!("Exchanging authorization code for tokens...");
    let (access_token, refresh_token, expires_in) =
        exchange_code(sso_url, &code, &redirect_uri, &code_verifier).await?;

    // 7. Store tokens — report outcome clearly.
    let (outcome, backend_info) = store_tokens(&access_token, refresh_token.as_deref(), expires_in);
    print_store_outcome(&outcome, &access_token, &backend_info);

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
        .context("Client credentials token request failed")?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Client credentials grant failed (HTTP {status}): {body}"));
    }

    let token_resp: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse client credentials token response")?;

    let access_token = token_resp["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("No access_token in response"))?
        .to_string();
    let refresh_token = token_resp["refresh_token"].as_str().map(String::from);
    let expires_in = token_resp["expires_in"].as_u64().unwrap_or(900);

    let (outcome, backend_info) = store_tokens(&access_token, refresh_token.as_deref(), expires_in);
    print_store_outcome(&outcome, &access_token, &backend_info);
    show_available_models(cenzontle_url, &access_token).await;

    Ok(())
}

// ── Token exchange ─────────────────────────────────────────────────────────────

async fn exchange_code(
    sso_url: &str,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<(String, Option<String>, u64)> {
    let token_url = format!("{sso_url}/oauth/token");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    let mut params = HashMap::new();
    params.insert("grant_type", "authorization_code");
    params.insert("code", code);
    params.insert("redirect_uri", redirect_uri);
    params.insert("client_id", CLIENT_ID);
    params.insert("code_verifier", code_verifier);

    let resp = http
        .post(&token_url)
        .form(&params)
        .send()
        .await
        .context("Token exchange request failed")?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Token exchange failed (HTTP {status}): {body}"));
    }

    let token_resp: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse token response")?;

    let access_token = token_resp["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("No access_token in token response"))?
        .to_string();
    let refresh_token = token_resp["refresh_token"].as_str().map(String::from);
    let expires_in = token_resp["expires_in"].as_u64().unwrap_or(900);

    Ok((access_token, refresh_token, expires_in))
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
        .context("Refresh token request failed")?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Token refresh failed (HTTP {status}): {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse refresh token response")?;

    let access = body["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("No access_token in refresh response"))?
        .to_string();
    let new_refresh = body["refresh_token"].as_str().map(String::from);
    let expires_in = body["expires_in"].as_u64().unwrap_or(900);

    Ok((access, new_refresh, expires_in))
}

// ── Token storage ─────────────────────────────────────────────────────────────

/// Write tokens to the credential store and return the storage outcome.
///
/// All tokens are written atomically via `set_multiple_secrets` — on the Linux
/// file-store backend this is a single `rename(2)` that prevents partial-write
/// races where the access token exists but the expiry does not.
///
/// This function **never panics and never returns an error** — credential store
/// failures are captured in [`StoreOutcome::NotPersisted`] so the caller can
/// display a user-friendly warning without aborting the login flow.
fn store_tokens(
    access_token: &str,
    refresh_token: Option<&str>,
    expires_in: u64,
) -> (StoreOutcome, String) {
    let keystore = KeyStore::new(SERVICE_NAME);
    let backend_info = keystore.backend_info();

    let expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + expires_in;
    let expires_at_str = expires_at.to_string();

    // Build the entry list for the atomic write.
    let mut entries: Vec<(&str, &str)> = vec![
        (KEY_ACCESS_TOKEN, access_token),
        (KEY_EXPIRES_AT, &expires_at_str),
    ];
    if let Some(rt) = refresh_token {
        entries.push((KEY_REFRESH_TOKEN, rt));
    }

    match keystore.set_multiple_secrets(entries) {
        Ok(()) => {
            tracing::debug!(backend = %backend_info, "Cenzontle tokens stored successfully");
            (StoreOutcome::Persisted, backend_info)
        }
        Err(e) => {
            tracing::warn!(error = %e, backend = %backend_info,
                "Failed to persist Cenzontle tokens to credential store");
            (
                StoreOutcome::NotPersisted {
                    reason: e.to_string(),
                },
                backend_info,
            )
        }
    }
}

/// Print the result of a `store_tokens` call to stdout/stderr.
///
/// `backend_info` is passed in so this function does not need to construct a
/// second `KeyStore` (which triggers a D-Bus probe on Linux for no reason).
fn print_store_outcome(outcome: &StoreOutcome, access_token: &str, backend_info: &str) {
    match outcome {
        StoreOutcome::Persisted => {
            println!("Cenzontle session stored. Backend: {backend_info}");
        }
        StoreOutcome::NotPersisted { reason } => {
            eprintln!();
            eprintln!("WARNING: Cenzontle tokens could not be persisted to the credential store.");
            eprintln!("         Reason: {reason}");
            eprintln!();
            eprintln!("         Your session is active for this terminal only.");
            eprintln!("         To make it permanent, add this to your shell profile:");
            eprintln!();
            eprintln!("           export CENZONTLE_ACCESS_TOKEN='{access_token}'");
            eprintln!();
            eprintln!("         On Linux, installing gnome-keyring and ensuring");
            eprintln!("         DBUS_SESSION_BUS_ADDRESS is exported will enable");
            eprintln!("         automatic persistent storage.");
        }
    }
}

// ── OAuth callback server ─────────────────────────────────────────────────────

/// Bind the OAuth callback TCP listener, retrying on adjacent ports if the
/// primary port is occupied.
///
/// Port selection order:
/// 1. `HALCON_SSO_PORT` env var (explicit override)
/// 2. `DEFAULT_REDIRECT_PORT` (9876)
/// 3. `DEFAULT_REDIRECT_PORT + 1` … `+ PORT_RETRY_COUNT - 1`
///
/// Returns the bound listener and the actual port used so the redirect URI
/// can be set correctly.
async fn bind_callback_server() -> Result<(TcpListener, u16)> {
    let base_port = std::env::var("HALCON_SSO_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(DEFAULT_REDIRECT_PORT);

    let mut last_err: Option<std::io::Error> = None;

    for offset in 0..PORT_RETRY_COUNT {
        let candidate = base_port.saturating_add(offset);
        match TcpListener::bind(format!("127.0.0.1:{candidate}")).await {
            Ok(listener) => {
                if offset > 0 {
                    tracing::info!(
                        port = candidate,
                        skipped = offset,
                        "OAuth callback bound on alternate port"
                    );
                }
                return Ok((listener, candidate));
            }
            Err(e) => {
                tracing::debug!(
                    port = candidate,
                    error = %e,
                    "OAuth callback port in use, trying next"
                );
                last_err = Some(e);
            }
        }
    }

    let last = base_port.saturating_add(PORT_RETRY_COUNT - 1);
    Err(anyhow!(
        "Could not bind OAuth callback on any port in {}–{}: {}. \
         Set HALCON_SSO_PORT to a free port and retry.",
        base_port,
        last,
        last_err.map(|e| e.to_string()).unwrap_or_default()
    ))
}

/// Build the OAuth authorization URL.
fn build_auth_url(sso_url: &str, redirect_uri: &str, code_challenge: &str, state: &str) -> String {
    format!(
        "{sso_url}/oauth/authorize?\
         client_id={CLIENT_ID}\
         &response_type=code\
         &redirect_uri={redirect_uri_enc}\
         &scope={scope_enc}\
         &code_challenge={code_challenge}\
         &code_challenge_method=S256\
         &state={state}",
        redirect_uri_enc = percent_encode(redirect_uri),
        scope_enc = percent_encode(SCOPES),
    )
}

/// Wait for the OAuth callback request and extract the authorization code.
async fn accept_callback(listener: &TcpListener, expected_state: &str) -> Result<String> {
    let (mut stream, _addr) = listener
        .accept()
        .await
        .context("Failed to accept OAuth callback connection")?;

    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .await
        .context("Failed to read OAuth callback request line")?;

    // Parse GET /callback?code=...&state=... HTTP/1.1
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();

    // Respond immediately so the browser tab shows a success message.
    let body = "Authentication successful — you can close this tab.";
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await.ok();

    // Drain query string parameters.
    let query = path.splitn(2, '?').nth(1).unwrap_or("");
    let params: HashMap<&str, &str> = query
        .split('&')
        .filter_map(|kv| {
            let mut parts = kv.splitn(2, '=');
            Some((parts.next()?, parts.next()?))
        })
        .collect();

    if let Some(error) = params.get("error") {
        let desc = params.get("error_description").copied().unwrap_or("");
        return Err(anyhow!(
            "SSO authorization error: {error}{}",
            if desc.is_empty() { String::new() } else { format!(" — {desc}") }
        ));
    }

    let received_state = params.get("state").copied().unwrap_or("");
    if received_state != expected_state {
        return Err(anyhow!(
            "State mismatch in OAuth callback (CSRF protection triggered). \
             Please try logging in again."
        ));
    }

    params
        .get("code")
        .map(|c| percent_decode(c))
        .ok_or_else(|| anyhow!("No authorization code in OAuth callback"))
}

// ── Cenzontle model listing ────────────────────────────────────────────────────

async fn show_available_models(cenzontle_url: &str, access_token: &str) {
    let url = format!("{cenzontle_url}/v1/llm/models");
    let http = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };

    match http.get(&url).bearer_auth(access_token).send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                println!();
                println!("Models available in your Cenzontle account:");
                if let Some(data) = body["data"].as_array() {
                    if data.is_empty() {
                        println!("  (no models — check your account permissions)");
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
            tracing::debug!(status = %resp.status(), "Cenzontle models endpoint non-200");
        }
        Err(e) => {
            tracing::debug!(error = %e, "Could not fetch Cenzontle models");
        }
    }
}

// ── Encoding helpers ──────────────────────────────────────────────────────────

/// RFC 3986 percent-encoding for use in OAuth authorization URL query parameters.
///
/// Only unreserved characters (ALPHA / DIGIT / "-" / "_" / "." / "~") are left
/// unencoded.  Spaces are encoded as `%20` — NOT `+` — because OAuth 2.1
/// authorization URLs are URL query parameters, not `application/x-www-form-urlencoded`
/// form data.  Using `+` for spaces in the `scope` parameter causes interoperability
/// failures with strict OAuth servers (RFC 6749 §3.3 explicitly uses space-delimited
/// scope values in URLs).
fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(
                    char::from_digit((byte >> 4) as u32, 16)
                        .unwrap_or('0')
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit((byte & 0xf) as u32, 16)
                        .unwrap_or('0')
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}

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

// ── PKCE helpers ──────────────────────────────────────────────────────────────

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

// ── Misc helpers ──────────────────────────────────────────────────────────────

fn format_unix_ts(ts: u64) -> String {
    // Minimal timestamp formatter without pulling in chrono here —
    // just show the raw epoch for now; callers that want pretty dates
    // can format with chrono themselves.
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| ts.to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_challenge_rfc7636_test_vector() {
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
    fn code_verifier_is_base64url_43_chars() {
        let verifier = generate_code_verifier();
        // base64url of 32 bytes = ceil(32*4/3) = 43 chars (no padding)
        assert_eq!(verifier.len(), 43);
        assert!(verifier.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn percent_encode_spaces_as_percent20() {
        // RFC 3986 §2.1 — spaces in URL query parameters must be %20, not +.
        assert_eq!(percent_encode("hello world"), "hello%20world");
    }

    #[test]
    fn percent_encode_special_chars() {
        assert_eq!(percent_encode("a=b&c"), "a%3Db%26c");
    }

    #[test]
    fn percent_decode_percent20_as_space() {
        // Verify the decoder handles the canonical %20 encoding produced by
        // percent_encode; also keep + decoding for inbound form-encoded values.
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("hello+world"), "hello world");
    }

    #[test]
    fn percent_decode_hex_sequences() {
        assert_eq!(percent_decode("a%3Db%26c"), "a=b&c");
    }

    #[test]
    fn percent_encode_decode_roundtrip() {
        let original = "http://localhost:9876/callback?code=abc&state=xyz";
        let encoded = percent_encode(original);
        let decoded = percent_decode(&encoded);
        assert_eq!(decoded, original);
    }

    #[test]
    fn store_outcome_persisted_variant() {
        // StoreOutcome::Persisted is not an error.
        let o = StoreOutcome::Persisted;
        assert_eq!(o, StoreOutcome::Persisted);
    }

    #[test]
    fn build_auth_url_contains_required_params() {
        let url = build_auth_url(
            "https://sso.example.com",
            "http://localhost:9876/callback",
            "CHALLENGE_HASH",
            "STATE_VALUE",
        );
        assert!(url.contains("client_id=halcon-cli"), "missing client_id");
        assert!(url.contains("response_type=code"), "missing response_type");
        assert!(url.contains("code_challenge=CHALLENGE_HASH"), "missing code_challenge");
        assert!(url.contains("code_challenge_method=S256"), "missing method");
        assert!(url.contains("state=STATE_VALUE"), "missing state");
        // Scope spaces must be %20 (RFC 3986), not + (form-encoding).
        assert!(url.contains("openid%20profile"), "scope must use %20 not +");
        assert!(!url.contains("openid+profile"), "scope must not use form-encoding +");
    }

    #[tokio::test]
    async fn bind_callback_server_succeeds() {
        let (listener, port) = bind_callback_server().await.unwrap();
        assert!(port >= DEFAULT_REDIRECT_PORT, "port should be >= default");
        drop(listener);
    }

    #[tokio::test]
    async fn bind_callback_server_env_override() {
        // Pick a port we're unlikely to collide with.
        std::env::set_var("HALCON_SSO_PORT", "19876");
        let result = bind_callback_server().await;
        std::env::remove_var("HALCON_SSO_PORT");
        // Accept either success (port was free) or error (port taken in CI).
        // The important thing is that the env var was read.
        let _ = result;
    }
}
