//! OAuth 2.0 Authorization Code + PKCE flow for browser-based login.
//!
//! Builds the authorization URL, opens the browser, and exchanges the
//! authorization code for an access token.

use crate::pkce::PkceChallenge;
use cuervo_core::error::{CuervoError, Result};
use cuervo_core::types::OAuthConfig;
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

/// Result of building the authorization URL.
#[derive(Debug, Clone)]
pub struct AuthorizeRequest {
    /// The full authorization URL to open in the browser.
    pub url: String,
    /// The PKCE code verifier (needed later for token exchange).
    pub code_verifier: String,
    /// The state parameter for CSRF protection.
    pub state: String,
}

/// Token response from the OAuth token endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub token_type: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

/// OAuth error response from the token endpoint.
#[derive(Debug, Deserialize)]
struct OAuthErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Orchestrates the OAuth 2.0 Authorization Code + PKCE flow.
pub struct OAuthFlow {
    config: OAuthConfig,
    client: reqwest::Client,
}

impl OAuthFlow {
    /// Create a new OAuth flow with the given configuration.
    pub fn new(config: OAuthConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    /// Create a new OAuth flow with a custom HTTP client (for testing).
    pub fn with_client(config: OAuthConfig, client: reqwest::Client) -> Self {
        Self { config, client }
    }

    /// Build the authorization URL with PKCE challenge and state parameter.
    pub fn build_authorize_url(&self) -> Result<AuthorizeRequest> {
        let pkce = PkceChallenge::generate();
        let state = Uuid::new_v4().to_string();

        let mut url = Url::parse(&self.config.authorize_url)
            .map_err(|e| CuervoError::AuthFailed(format!("invalid authorize_url: {e}")))?;

        {
            let mut params = url.query_pairs_mut();
            params.append_pair("response_type", "code");
            params.append_pair("client_id", &self.config.client_id);
            params.append_pair("redirect_uri", &self.config.redirect_uri);
            params.append_pair("code_challenge", &pkce.code_challenge);
            params.append_pair("code_challenge_method", "S256");
            params.append_pair("state", &state);

            if !self.config.scopes.is_empty() {
                params.append_pair("scope", &self.config.scopes);
            }
        }

        Ok(AuthorizeRequest {
            url: url.to_string(),
            code_verifier: pkce.code_verifier,
            state,
        })
    }

    /// Exchange an authorization code for an access token.
    pub async fn exchange_code(
        &self,
        code: &str,
        code_verifier: &str,
        state: &str,
    ) -> Result<TokenResponse> {
        let mut body = serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": self.config.redirect_uri,
            "client_id": self.config.client_id,
            "code_verifier": code_verifier,
            "state": state,
        });

        if !self.config.scopes.is_empty() {
            body["scope"] = serde_json::Value::String(self.config.scopes.clone());
        }

        let response = self
            .client
            .post(&self.config.token_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| CuervoError::AuthFailed(format!("token exchange request failed: {e}")))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| CuervoError::AuthFailed(format!("failed to read token response: {e}")))?;

        if !status.is_success() {
            if let Ok(err) = serde_json::from_str::<OAuthErrorResponse>(&body) {
                let desc = err.error_description.unwrap_or_default();
                return Err(CuervoError::AuthFailed(format!(
                    "token exchange error: {} {}",
                    err.error, desc
                )));
            }
            return Err(CuervoError::AuthFailed(format!(
                "token exchange failed (HTTP {}): {}",
                status, body
            )));
        }

        let token: TokenResponse = serde_json::from_str(&body)
            .map_err(|e| CuervoError::AuthFailed(format!("invalid token response: {e}")))?;

        Ok(token)
    }

    /// Exchange an OAuth access token for an API key via the provider's API key endpoint.
    ///
    /// This is the final step for providers (like Anthropic) that require converting
    /// an OAuth token into a usable API key.
    pub async fn create_api_key(&self, access_token: &str) -> Result<String> {
        let api_key_url = self
            .config
            .api_key_url
            .as_deref()
            .ok_or_else(|| CuervoError::AuthFailed("no api_key_url configured".to_string()))?;

        let response = self
            .client
            .post(api_key_url)
            .header("Authorization", format!("Bearer {access_token}"))
            .send()
            .await
            .map_err(|e| {
                CuervoError::AuthFailed(format!("API key creation request failed: {e}"))
            })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            CuervoError::AuthFailed(format!("failed to read API key response: {e}"))
        })?;

        if !status.is_success() {
            return Err(CuervoError::AuthFailed(format!(
                "API key creation failed (HTTP {status}): {body}"
            )));
        }

        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| CuervoError::AuthFailed(format!("invalid API key response: {e}")))?;

        parsed["raw_key"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                CuervoError::AuthFailed("API key response missing 'raw_key' field".to_string())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> OAuthConfig {
        OAuthConfig {
            client_id: "test-client-id".to_string(),
            authorize_url: "https://auth.example.com/authorize".to_string(),
            token_url: "https://auth.example.com/token".to_string(),
            redirect_uri: "http://localhost:9876/callback".to_string(),
            api_key_url: None,
            scopes: "read write".to_string(),
        }
    }

    #[test]
    fn authorize_url_contains_required_params() {
        let flow = OAuthFlow::new(test_config());
        let req = flow.build_authorize_url().unwrap();

        let url = Url::parse(&req.url).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().collect();

        assert_eq!(params.get("response_type").unwrap(), "code");
        assert_eq!(params.get("client_id").unwrap(), "test-client-id");
        assert_eq!(
            params.get("redirect_uri").unwrap(),
            "http://localhost:9876/callback"
        );
        assert_eq!(params.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(params.get("scope").unwrap(), "read write");
        assert!(params.contains_key("code_challenge"));
        assert!(params.contains_key("state"));
    }

    #[test]
    fn authorize_url_omits_scope_when_empty() {
        let mut config = test_config();
        config.scopes = String::new();

        let flow = OAuthFlow::new(config);
        let req = flow.build_authorize_url().unwrap();

        let url = Url::parse(&req.url).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().collect();

        assert!(!params.contains_key("scope"));
    }

    #[test]
    fn authorize_url_preserves_existing_query_params() {
        let mut config = test_config();
        config.authorize_url = "https://auth.example.com/authorize?code=true".to_string();

        let flow = OAuthFlow::new(config);
        let req = flow.build_authorize_url().unwrap();

        let url = Url::parse(&req.url).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().collect();

        assert_eq!(params.get("code").unwrap(), "true");
        assert_eq!(params.get("response_type").unwrap(), "code");
        assert!(params.contains_key("client_id"));
    }

    #[test]
    fn state_is_uuid_v4_format() {
        let flow = OAuthFlow::new(test_config());
        let req = flow.build_authorize_url().unwrap();

        // UUID v4 format: 8-4-4-4-12 hex chars
        let uuid = Uuid::parse_str(&req.state);
        assert!(uuid.is_ok(), "state should be a valid UUID: {}", req.state);
    }

    #[tokio::test]
    async fn exchange_code_success() {
        let server = wiremock::MockServer::start().await;

        let config = OAuthConfig {
            client_id: "test-client".to_string(),
            authorize_url: "https://auth.example.com/authorize".to_string(),
            token_url: format!("{}/token", server.uri()),
            redirect_uri: "http://localhost:9876/callback".to_string(),
            api_key_url: None,
            scopes: String::new(),
        };

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/token"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "access_token": "sk-test-token-abc123",
                    "token_type": "Bearer",
                    "expires_in": 3600
                })),
            )
            .mount(&server)
            .await;

        let flow = OAuthFlow::new(config);
        let token = flow
            .exchange_code("auth-code-xyz", "verifier123", "test-state")
            .await
            .unwrap();

        assert_eq!(token.access_token, "sk-test-token-abc123");
        assert_eq!(token.token_type, "Bearer");
        assert_eq!(token.expires_in, Some(3600));
        assert!(token.refresh_token.is_none());
    }

    #[tokio::test]
    async fn exchange_code_error_response() {
        let server = wiremock::MockServer::start().await;

        let config = OAuthConfig {
            client_id: "test-client".to_string(),
            authorize_url: "https://auth.example.com/authorize".to_string(),
            token_url: format!("{}/token", server.uri()),
            redirect_uri: "http://localhost:9876/callback".to_string(),
            api_key_url: None,
            scopes: String::new(),
        };

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/token"))
            .respond_with(
                wiremock::ResponseTemplate::new(400).set_body_json(serde_json::json!({
                    "error": "invalid_grant",
                    "error_description": "Authorization code expired"
                })),
            )
            .mount(&server)
            .await;

        let flow = OAuthFlow::new(config);
        let result = flow
            .exchange_code("expired-code", "verifier123", "test-state")
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("invalid_grant"),
            "Error should contain error code: {err}"
        );
    }

    #[tokio::test]
    async fn exchange_code_network_error() {
        // Use a port that's not listening to trigger a connection error.
        let config = OAuthConfig {
            client_id: "test-client".to_string(),
            authorize_url: "https://auth.example.com/authorize".to_string(),
            token_url: "http://127.0.0.1:1/token".to_string(),
            redirect_uri: "http://localhost:9876/callback".to_string(),
            api_key_url: None,
            scopes: String::new(),
        };

        let flow = OAuthFlow::new(config);
        let result = flow.exchange_code("code", "verifier", "test-state").await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("token exchange request failed"),
            "Error should indicate network failure: {err}"
        );
    }
}
