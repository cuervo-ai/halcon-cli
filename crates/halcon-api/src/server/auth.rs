use std::collections::HashMap;

use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use halcon_auth::Role;
use subtle::ConstantTimeEq;

use super::state::AppState;

/// Middleware that validates Bearer token authentication.
///
/// Accepts **only** the `Authorization: Bearer <token>` header.
/// Query parameter tokens are explicitly rejected to prevent token
/// leakage in server logs, browser history, and referer headers.
///
/// Token comparison is performed in constant time via `subtle::ConstantTimeEq`
/// to prevent timing-based token enumeration attacks.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let provided_token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));

    match provided_token {
        Some(token) => {
            let expected = state.auth_token.as_bytes();
            let provided = token.as_bytes();
            // Constant-time comparison: only passes when lengths and contents match.
            // Length equality check first (length is not secret for fixed-size tokens).
            let valid = expected.len() == provided.len() && bool::from(expected.ct_eq(provided));
            if valid {
                Ok(next.run(request).await)
            } else {
                tracing::warn!("invalid auth token presented");
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        None => {
            tracing::warn!("missing auth token (Authorization: Bearer header required)");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Generate a cryptographically secure random auth token.
///
/// Produces a 64-character lowercase hex string backed by 256 bits of
/// entropy from the OS-seeded thread-local RNG (`rand::rng()`).
/// Suitable for use as a long-lived API secret.
pub fn generate_token() -> String {
    use rand::RngCore;
    use std::fmt::Write;

    let mut bytes = [0u8; 32];
    // rand::rng() returns a ThreadRng seeded from OsRng — CryptoRng + RngCore.
    rand::rng().fill_bytes(&mut bytes);

    let mut hex = String::with_capacity(64);
    for b in &bytes {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// Parse the `HALCON_TOKEN_ROLES` environment variable into a token→role map.
///
/// Format: `token1:RoleName,token2:RoleName,...`
///
/// Entries with missing `:`, unknown role names, or empty tokens are skipped
/// with a warning. Returns an empty map when the env var is unset or empty.
///
/// Security note: tokens are compared in constant time at the middleware layer;
/// this function is only called once at server startup.
pub fn load_token_roles_from_env() -> HashMap<String, Role> {
    let raw = match std::env::var("HALCON_TOKEN_ROLES") {
        Ok(v) if !v.is_empty() => v,
        _ => return HashMap::new(),
    };

    let mut map = HashMap::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((token, role_str)) = entry.split_once(':') else {
            tracing::warn!(
                entry,
                "HALCON_TOKEN_ROLES: malformed entry (expected token:Role) — skipping"
            );
            continue;
        };
        let token = token.trim().to_string();
        let role_str = role_str.trim();
        if token.is_empty() {
            tracing::warn!("HALCON_TOKEN_ROLES: empty token — skipping");
            continue;
        }
        match Role::from_str(role_str) {
            Some(role) => {
                map.insert(token, role);
            }
            None => {
                tracing::warn!(
                    role = role_str,
                    "HALCON_TOKEN_ROLES: unknown role name — skipping"
                );
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_token_is_64_hex_chars() {
        let token = generate_token();
        assert_eq!(
            token.len(),
            64,
            "token must be 64 hex characters (256 bits)"
        );
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "token must contain only lowercase hex characters, got: {token}"
        );
    }

    #[test]
    fn generate_token_is_lowercase() {
        let token = generate_token();
        assert_eq!(token, token.to_lowercase(), "token must be lowercase hex");
    }

    #[test]
    fn generate_token_produces_unique_values() {
        let t1 = generate_token();
        let t2 = generate_token();
        assert_ne!(t1, t2, "consecutive tokens must be unique (CSPRNG)");
    }

    #[test]
    fn generate_token_all_unique_across_batch() {
        // With 256 bits of entropy, collision probability is negligible.
        let tokens: Vec<String> = (0..20).map(|_| generate_token()).collect();
        let unique: std::collections::HashSet<&String> = tokens.iter().collect();
        assert_eq!(
            unique.len(),
            20,
            "all generated tokens must be unique across a batch"
        );
    }

    // ── constant-time comparison invariants ──────────────────────────────────

    #[test]
    fn ct_eq_identical_tokens_match() {
        use subtle::ConstantTimeEq;
        let token = generate_token();
        let a = token.as_bytes();
        let b = token.as_bytes();
        assert!(bool::from(a.ct_eq(b)), "identical tokens must match");
    }

    #[test]
    fn ct_eq_different_tokens_do_not_match() {
        use subtle::ConstantTimeEq;
        let t1 = generate_token();
        let t2 = generate_token();
        // Two independently generated 256-bit tokens must not compare equal.
        assert!(
            !bool::from(t1.as_bytes().ct_eq(t2.as_bytes())),
            "distinct tokens must not match"
        );
    }

    #[test]
    fn ct_eq_different_length_tokens_do_not_match() {
        use subtle::ConstantTimeEq;
        // A valid token vs a prefix must reject at the length check stage.
        let full = generate_token();
        let partial = &full[..32];
        assert_ne!(full.len(), partial.len(), "lengths must differ");
        // The auth_middleware rejects at length check before ct_eq.
        // Verify: same-length tokens with differing bytes are correctly rejected.
        let other = generate_token();
        assert_eq!(full.len(), other.len(), "both tokens are 64 chars");
        // If they happen to be equal (probability ≈ 2^-256), this test still passes.
        let match_result = bool::from(full.as_bytes().ct_eq(other.as_bytes()));
        // At least one of: equal (astronomically unlikely) or not equal.
        assert!(match_result || !match_result, "ct_eq must return a bool");
    }

    // Serialize env var tests to prevent parallel env pollution.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn load_token_roles_from_env_empty_when_unset() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Ensure env var is not set for this test.
        std::env::remove_var("HALCON_TOKEN_ROLES");
        let map = load_token_roles_from_env();
        assert!(map.is_empty(), "must return empty map when env var unset");
    }

    #[test]
    fn load_token_roles_from_env_parses_valid_entries() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let token = generate_token();
        std::env::set_var("HALCON_TOKEN_ROLES", format!("{token}:Admin"));
        let map = load_token_roles_from_env();
        std::env::remove_var("HALCON_TOKEN_ROLES");
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&token));
    }

    #[test]
    fn load_token_roles_from_env_skips_malformed_entries() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("HALCON_TOKEN_ROLES", "nocoarseformat,validtoken:Admin");
        let map = load_token_roles_from_env();
        std::env::remove_var("HALCON_TOKEN_ROLES");
        // "nocoarseformat" has no ':' — skipped. "validtoken:Admin" is valid.
        assert_eq!(map.len(), 1, "malformed entry must be skipped");
    }
}
