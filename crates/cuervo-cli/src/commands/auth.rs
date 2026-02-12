use anyhow::Result;
use cuervo_auth::KeyStore;

const SERVICE_NAME: &str = "cuervo-cli";

/// Known provider key names for the keychain.
fn provider_key(provider: &str) -> String {
    format!("{provider}_api_key")
}

/// Run `cuervo auth login <provider>` — prompt for API key and store in keychain.
pub fn login(provider: &str) -> Result<()> {
    login_api_key(provider)
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

/// Run `cuervo auth logout <provider>` — remove API key from keychain.
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

/// Run `cuervo auth status` — show which providers have keys stored.
pub fn status() -> Result<()> {
    let keystore = KeyStore::new(SERVICE_NAME);

    println!("API key status:");
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
        let var_name = "CUERVO_TEST_KEY_12345";
        std::env::set_var(var_name, "sk-test-12345");
        let result = resolve_api_key("test", Some(var_name));
        assert_eq!(result, Some("sk-test-12345".into()));
        std::env::remove_var(var_name);
    }

    #[test]
    fn resolve_api_key_empty_env_returns_none() {
        let var_name = "CUERVO_TEST_KEY_EMPTY";
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
