//! PKCE (Proof Key for Code Exchange) implementation per RFC 7636.
//!
//! Generates a `code_verifier` and `code_challenge` (S256 only) for
//! OAuth 2.0 Authorization Code flows with public clients.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};

/// A PKCE challenge pair: `code_verifier` + `code_challenge` (S256).
#[derive(Debug, Clone)]
pub struct PkceChallenge {
    /// The random code verifier (128 unreserved ASCII characters).
    pub code_verifier: String,
    /// SHA256(code_verifier) encoded as base64url-no-pad.
    pub code_challenge: String,
}

/// Characters allowed in the code verifier (RFC 7636 §4.1: unreserved characters).
const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

impl PkceChallenge {
    /// Generate a new PKCE challenge with a 128-character random code verifier.
    ///
    /// The code challenge is computed as `BASE64URL(SHA256(code_verifier))` (S256 method).
    pub fn generate() -> Self {
        let mut rng = rand::rng();
        let code_verifier: String = (0..128)
            .map(|_| {
                let idx = rng.random_range(0..UNRESERVED.len());
                UNRESERVED[idx] as char
            })
            .collect();

        let code_challenge = Self::compute_challenge(&code_verifier);

        Self {
            code_verifier,
            code_challenge,
        }
    }

    /// Compute the S256 code challenge for a given verifier.
    pub fn compute_challenge(verifier: &str) -> String {
        let digest = Sha256::digest(verifier.as_bytes());
        URL_SAFE_NO_PAD.encode(digest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_verifier_is_128_chars() {
        let pkce = PkceChallenge::generate();
        assert_eq!(pkce.code_verifier.len(), 128);
    }

    #[test]
    fn verifier_uses_only_unreserved_chars() {
        let pkce = PkceChallenge::generate();
        let allowed: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
        for ch in pkce.code_verifier.bytes() {
            assert!(
                allowed.contains(&ch),
                "Invalid character in verifier: {:?}",
                ch as char
            );
        }
    }

    #[test]
    fn challenge_is_valid_base64url_no_pad() {
        let pkce = PkceChallenge::generate();
        // SHA256 → 32 bytes → base64url-no-pad = 43 characters
        assert_eq!(pkce.code_challenge.len(), 43);
        assert!(!pkce.code_challenge.contains('='));
        assert!(!pkce.code_challenge.contains('+'));
        assert!(!pkce.code_challenge.contains('/'));
    }

    #[test]
    fn challenge_matches_verifier() {
        let pkce = PkceChallenge::generate();
        let recomputed = PkceChallenge::compute_challenge(&pkce.code_verifier);
        assert_eq!(pkce.code_challenge, recomputed);
    }

    #[test]
    fn rfc7636_appendix_b_known_vector() {
        // RFC 7636 Appendix B test vector:
        // code_verifier  = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        // code_challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected_challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        let challenge = PkceChallenge::compute_challenge(verifier);
        assert_eq!(challenge, expected_challenge);
    }
}
