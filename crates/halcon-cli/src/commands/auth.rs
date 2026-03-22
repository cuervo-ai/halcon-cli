use std::io::{BufRead, BufReader};

use anyhow::Result;
use halcon_auth::KeyStore;
use serde_json;

const SERVICE_NAME: &str = "halcon-cli";

/// Known provider key names for the keychain.
fn provider_key(provider: &str) -> String {
    format!("{provider}_api_key")
}

/// Run `halcon auth login <provider>` — prompt for API key and store in keychain.
///
/// For `claude_code`, launches the Claude Code CLI OAuth browser flow instead of
/// prompting for an API key.
pub fn login(provider: &str) -> Result<()> {
    if provider == "claude_code" {
        return login_claude_code_oauth();
    }
    login_api_key(provider)
}

/// OAuth login for the `claude_code` provider — OpenCode-style browser flow.
///
/// Strategy:
/// 1. Check if already authenticated (fast path).
/// 2. Generate the authorization URL ourselves (same endpoint claude uses)
///    and open the default browser BEFORE spawning the CLI — avoids TTY-pipe
///    buffering issues where piped stdout suppresses browser launch.
/// 3. Run `claude auth login` with fully inherited stdio (TTY mode) so the
///    user sees all progress and can interact naturally.
/// 4. Confirm the result.
fn login_claude_code_oauth() -> Result<()> {
    let claude_bin = find_claude_binary();

    println!();
    println!("  Claude Code — OAuth Login");
    println!("  ─────────────────────────────────────────────────────");
    println!();

    // ── 1. Already logged in? ────────────────────────────────────────────────
    if let Some((method, sub, email)) = check_claude_auth_status(&claude_bin) {
        let email_display = if email.is_empty() {
            "—".to_string()
        } else {
            email
        };

        // Validate that the token is still accepted by the API (not just stored).
        print!("  Verificando validez del token...");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let token_ok = validate_claude_token(&claude_bin);

        if token_ok {
            println!("  ✓");
            println!("  ✓  Ya autenticado (token válido)");
            println!("     Cuenta  {email_display}");
            println!("     Método  {method}");
            println!("     Plan    {sub}");
            println!();
            print_usage_hint();
            return Ok(());
        } else {
            println!("  ✗  Token expirado");
            println!();
            println!("  ⚠  Tu sesión de Claude Code ha expirado.");
            println!("     Cuenta  {email_display}");
            println!("     Iniciando renovación de sesión...");
            println!();
            // Fall through to re-authenticate below.
        }
    }

    // ── 2. Show instructions + open browser ──────────────────────────────────
    // Capture the authorization URL by running the command once with piped
    // stdout (fast — claude prints the URL immediately before waiting).
    // Then re-run with inherited stdio so the user gets a real TTY experience.
    println!("  Iniciando sesión con tu cuenta de claude.ai...");
    println!();

    let url = capture_auth_url(&claude_bin);

    match &url {
        Some(u) => {
            // Print the URL first — user can copy it if browser fails.
            println!("  URL de autorización:");
            println!();
            println!("    {u}");
            println!();

            // Try to open the browser.
            match open_browser(u) {
                Ok(_) => {
                    println!("  ✓ Navegador abierto — inicia sesión en la ventana que acaba de aparecer.");
                }
                Err(_) => {
                    println!("  ⚠  El navegador no se abrió automáticamente.");
                    println!("     Copia la URL de arriba y pégala en tu navegador.");
                }
            }
        }
        None => {
            // Couldn't capture URL — just let the CLI handle everything.
            println!("  Abriendo el navegador para iniciar sesión...");
        }
    }

    println!();
    println!("  Esperando que completes el login en el navegador...");
    println!("  (puedes cerrar esta pantalla si el navegador se abrió correctamente)");
    println!();
    println!("  ─────────────────────────────────────────────────────");
    println!();

    // ── 3. Run the real login with full TTY — inherit all stdio ─────────────
    let exit_status = std::process::Command::new(&claude_bin)
        .args(["auth", "login"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| anyhow::anyhow!("No se pudo ejecutar '{claude_bin} auth login': {e}"))?;

    if !exit_status.success() {
        println!();
        println!("  ✗  El proceso de login terminó con error (código {exit_status}).");
        println!("     Intenta ejecutar directamente:");
        println!("       {claude_bin} auth login");
        println!();
        return Ok(());
    }

    // ── 4. Confirm ───────────────────────────────────────────────────────────
    println!();
    if let Some((method, sub, email)) = check_claude_auth_status(&claude_bin) {
        let email_display = if email.is_empty() {
            "—".to_string()
        } else {
            email
        };
        println!("  ✓  Login exitoso");
        println!("     Cuenta  {email_display}");
        println!("     Método  {method}");
        println!("     Plan    {sub}");
        println!();
        print_usage_hint();
    } else {
        println!("  ✓  Proceso completado — verifica con `halcon auth status`");
        println!();
    }
    Ok(())
}

/// Spawn `claude auth login` briefly with piped stdout to capture the
/// authorization URL before the process blocks waiting for the callback.
/// We kill the process immediately after reading the URL.
fn capture_auth_url(claude_bin: &str) -> Option<String> {
    let mut child = std::process::Command::new(claude_bin)
        .args(["auth", "login"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    let reader = BufReader::new(stdout);

    let mut found_url: Option<String> = None;

    // Read lines until we find the URL or give up after a few lines.
    for (i, line) in reader.lines().enumerate() {
        if i > 10 {
            break;
        }
        let line = line.ok()?;
        if let Some(url) = extract_https_url(&line) {
            found_url = Some(url);
            break;
        }
    }

    // Kill the probe process — the real login will be a fresh call.
    let _ = child.kill();
    let _ = child.wait();

    found_url
}

fn print_usage_hint() {
    println!("  Para usar Claude Code como proveedor:");
    println!();
    println!("    halcon -p claude_code chat \"tu pregunta\"");
    println!();
    println!("  Para verificar el estado:");
    println!();
    println!("    halcon auth status");
    println!();
}

/// Run `claude auth status --json` and return (method, subscriptionType, email)
/// if logged in, or None otherwise.
fn check_claude_auth_status(claude_bin: &str) -> Option<(String, String, String)> {
    let out = std::process::Command::new(claude_bin)
        .args(["auth", "status", "--json"])
        .output()
        .ok()?;

    let json_str = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&json_str).ok()?;

    if v["loggedIn"].as_bool() != Some(true) {
        return None;
    }

    let method = v["authMethod"].as_str().unwrap_or("unknown").to_string();
    let sub = v["subscriptionType"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let email = v["email"].as_str().unwrap_or("").to_string();
    Some((method, sub, email))
}

/// Validate the Claude OAuth token by making a minimal `claude api messages` call.
///
/// Returns `true` when the token is valid (or when we cannot determine validity —
/// e.g. `claude api messages` is not available in the installed version).
/// Returns `false` only when the API explicitly returns an `authentication_error`.
///
/// Uses a 10-second timeout via a background thread + channel so we never block
/// the login flow indefinitely.
fn validate_claude_token(claude_bin: &str) -> bool {
    use std::io::Write as _;
    use std::sync::mpsc;
    use std::time::Duration;

    // A minimal Anthropic messages request — 1 output token, no tools.
    let req = concat!(
        r#"{"model":"claude-haiku-4-5-20251001","max_tokens":1,"messages":"#,
        r#"[{"role":"user","content":"hi"}]}"#
    );

    let mut child = match std::process::Command::new(claude_bin)
        .args(["api", "messages"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        // `claude api messages` might not exist in older versions — skip validation.
        Err(_) => return true,
    };

    // Write the request body to stdin, then drop to signal EOF.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(req.as_bytes());
        // stdin auto-dropped here → EOF → claude reads the full request
    }

    // Read stdout in a background thread so we can apply a timeout.
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return true,
    };

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        use std::io::Read as _;
        let mut buf = String::new();
        let mut reader = std::io::BufReader::new(stdout);
        let _ = reader.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    // Wait up to 10 s for the probe result.
    let output = rx.recv_timeout(Duration::from_secs(10)).unwrap_or_default();

    // Clean up the process regardless of outcome.
    let _ = child.kill();
    let _ = child.wait();

    // An authentication_error in the response means the token is expired/invalid.
    if output.contains("authentication_error") {
        return false;
    }

    // Any other response (including network errors, empty body from timeout) is
    // treated as "valid" to avoid false positives that force re-login.
    true
}

/// Extract the first `https://` URL from a line of text.
fn extract_https_url(line: &str) -> Option<String> {
    // Find "https://" in the line and extract until whitespace or end.
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ')')
        .unwrap_or(rest.len());
    let url = &rest[..end];
    if url.len() > 8 {
        Some(url.to_string())
    } else {
        None
    }
}

/// Open the default browser for the given URL.
///
/// - macOS: `open <url>`
/// - Linux: `xdg-open <url>`
/// - Fallback: silently fail (user sees the printed URL)
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("open failed: {e}"))?;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("xdg-open failed: {e}"))?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err(anyhow::anyhow!(
        "browser open not supported on this platform"
    ))
}

/// Locate the `claude` binary: prefer the native install location, then PATH.
fn find_claude_binary() -> String {
    // Native install location (installed via `claude` installer script).
    if let Ok(home) = std::env::var("HOME") {
        let native = format!("{home}/.local/bin/claude");
        if std::path::Path::new(&native).exists() {
            return native;
        }
    }
    // Fall back to whatever is in PATH.
    "claude".to_string()
}

/// Manual API key entry — prompt and store in keychain.
fn login_api_key(provider: &str) -> Result<()> {
    let keystore = KeyStore::new(SERVICE_NAME);
    let key_name = provider_key(provider);

    // Read API key from stdin (hidden input).
    eprint!("Enter API key for {provider}: ");
    let api_key = read_hidden_line()?;
    let api_key = api_key.trim();

    if api_key.is_empty() {
        eprintln!("No key entered, aborting.");
        return Ok(());
    }

    keystore
        .set_secret(&key_name, api_key)
        .map_err(|e| anyhow::anyhow!("Failed to store API key: {e}"))?;

    println!("API key for {provider} stored in OS keychain.");
    Ok(())
}

/// Run `halcon auth logout <provider>` — remove API key from keychain.
pub fn logout(provider: &str) -> Result<()> {
    let keystore = KeyStore::new(SERVICE_NAME);
    let key_name = provider_key(provider);

    keystore
        .delete_secret(&key_name)
        .map_err(|e| anyhow::anyhow!("Failed to remove API key: {e}"))?;

    println!("API key for {provider} removed from OS keychain.");
    Ok(())
}

/// All known providers that may have API keys.
const KNOWN_PROVIDERS: &[&str] = &["anthropic", "openai", "deepseek", "gemini", "ollama"];

/// Run `halcon auth status` — show which providers have keys stored.
pub fn status() -> Result<()> {
    let keystore = KeyStore::new(SERVICE_NAME);

    println!("API key status:");

    // Claude Code uses OAuth via claude.ai — check via `claude auth status`.
    let claude_bin = find_claude_binary();
    let claude_status_str = std::process::Command::new(&claude_bin)
        .args(["auth", "status", "--json"])
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).to_string();
            if s.trim().is_empty() {
                None
            } else {
                Some(s)
            }
        });

    let claude_code_status = match &claude_status_str {
        Some(json_str) => {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) {
                let logged_in = v["loggedIn"].as_bool().unwrap_or(false);
                if logged_in {
                    let method = v["authMethod"].as_str().unwrap_or("unknown");
                    let sub = v["subscriptionType"].as_str().unwrap_or("unknown");
                    format!("logged in (OAuth · {method} · {sub})  -> halcon -p claude_code chat")
                } else {
                    "not logged in  -> run `halcon auth login claude_code`".into()
                }
            } else {
                "unknown (run `claude auth status`)".into()
            }
        }
        None => "not installed or not found".into(),
    };
    println!("  claude_code: {claude_code_status}");

    // Cenzontle uses SSO tokens, not API keys — check its dedicated keychain entries.
    // Unlike simple API keys, JWT tokens expire. We validate by hitting /v1/auth/me.
    let cenzontle_token = keystore.get_secret("cenzontle:access_token").ok().flatten();
    let cenzontle_env = std::env::var("CENZONTLE_ACCESS_TOKEN")
        .ok()
        .filter(|v| !v.is_empty());

    let cenzontle_status: String = match (&cenzontle_token, &cenzontle_env) {
        (Some(token), _) | (_, Some(token)) => {
            let source = if cenzontle_token.is_some() && cenzontle_env.is_some() {
                "SSO keychain + $CENZONTLE_ACCESS_TOKEN"
            } else if cenzontle_token.is_some() {
                "SSO keychain"
            } else {
                "$CENZONTLE_ACCESS_TOKEN"
            };
            // Validate token is still accepted by the backend
            if validate_cenzontle_token(token) {
                format!("logged in ({source})  -> halcon -p cenzontle chat")
            } else {
                format!("token expired ({source})  -> run `halcon auth login cenzontle`")
            }
        }
        (None, None) => "not logged in  -> run `halcon auth login cenzontle`".into(),
    };
    println!("  cenzontle: {cenzontle_status}");

    for provider in KNOWN_PROVIDERS {
        let key_name = provider_key(provider);

        // Check keychain.
        let in_keychain = keystore.get_secret(&key_name).ok().flatten().is_some();

        // Check env var.
        let env_var = format!("{}_API_KEY", provider.to_uppercase());
        let in_env = std::env::var(&env_var)
            .map(|v| !v.is_empty())
            .unwrap_or(false);

        let status = match (in_keychain, in_env) {
            (true, true) => format!("set (keychain + ${env_var})"),
            (true, false) => "set (keychain)".into(),
            (false, true) => format!("set (${env_var})"),
            (false, false) => "not set".into(),
        };

        println!("  {provider}: {status}");
    }
    Ok(())
}

/// Validate a Cenzontle JWT token by hitting GET /v1/auth/me.
///
/// Returns `true` if the token is accepted (HTTP 200).
/// Returns `false` on 401/403 (expired/invalid) or network error.
/// Uses a 5-second timeout to avoid blocking the status command.
fn validate_cenzontle_token(token: &str) -> bool {
    use std::time::Duration;

    let base_url = std::env::var("CENZONTLE_BASE_URL").unwrap_or_else(|_| {
        "https://ca-cenzontle-backend.graypond-e35bfdd8.eastus2.azurecontainerapps.io".to_string()
    });
    let url = format!("{base_url}/v1/auth/me");

    // Synchronous HTTP call (auth status is a CLI command, not async runtime)
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(_) => return false, // can't build client → treat as invalid
    };

    match client.get(&url).bearer_auth(token).send() {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false, // network error → treat as expired
    }
}

/// Resolve the API key for a provider, checking keychain then env var.
pub fn resolve_api_key(provider: &str, env_var: Option<&str>) -> Option<String> {
    // 1. Check env var first (takes precedence).
    if let Some(var) = env_var {
        if let Ok(key) = std::env::var(var) {
            if !key.is_empty() {
                return Some(key);
            }
        }
    }

    // 2. Fall back to OS keychain.
    let keystore = KeyStore::new(SERVICE_NAME);
    let key_name = provider_key(provider);
    keystore.get_secret(&key_name).ok().flatten()
}

/// Read a line from stdin with echo disabled (for API key input).
fn read_hidden_line() -> Result<String> {
    // Try crossterm raw mode for hidden input.
    use std::io::{self, Read};
    crossterm::terminal::enable_raw_mode()
        .map_err(|e| anyhow::anyhow!("Failed to enable raw mode: {e}"))?;

    let stdin = io::stdin();
    let mut line = String::new();
    // Read bytes until newline.
    for byte_result in stdin.lock().bytes() {
        match byte_result {
            Ok(b'\n') | Ok(b'\r') => break,
            Ok(3) => {
                // Ctrl+C
                crossterm::terminal::disable_raw_mode().ok();
                eprintln!();
                return Ok(String::new());
            }
            Ok(127) | Ok(8) => {
                // Backspace
                line.pop();
            }
            Ok(b) if b >= 32 => {
                line.push(b as char);
            }
            Ok(_) => {}
            Err(e) => {
                crossterm::terminal::disable_raw_mode().ok();
                return Err(anyhow::anyhow!("Read error: {e}"));
            }
        }
    }
    crossterm::terminal::disable_raw_mode().ok();
    eprintln!(); // newline after hidden input
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_key_format() {
        assert_eq!(provider_key("anthropic"), "anthropic_api_key");
        assert_eq!(provider_key("openai"), "openai_api_key");
    }

    #[test]
    fn resolve_api_key_from_env() {
        let var_name = "HALCON_TEST_KEY_12345";
        std::env::set_var(var_name, "sk-test-12345");
        let result = resolve_api_key("test", Some(var_name));
        assert_eq!(result, Some("sk-test-12345".into()));
        std::env::remove_var(var_name);
    }

    #[test]
    fn resolve_api_key_empty_env_returns_none() {
        let var_name = "HALCON_TEST_KEY_EMPTY";
        std::env::set_var(var_name, "");
        // Empty env var should fall through (keychain likely has nothing for "test").
        let result = resolve_api_key("test_nonexistent", Some(var_name));
        // Result is None since there's no keychain entry either.
        assert!(result.is_none());
        std::env::remove_var(var_name);
    }

    #[test]
    fn resolve_api_key_no_env_var() {
        // No env var set, no keychain entry.
        let result = resolve_api_key("nonexistent_provider_xyz", None);
        assert!(result.is_none());
    }
}
