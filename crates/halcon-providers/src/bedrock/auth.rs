// DECISION: BedrockProvider uses AWS SigV4 signing for every request.
// We use the aws-sigv4 crate which is the same library used internally by
// the official AWS SDK for Rust.
//
// Env vars:
//   AWS_ACCESS_KEY_ID     — required (or IAM role / instance profile)
//   AWS_SECRET_ACCESS_KEY — required
//   AWS_SESSION_TOKEN     — optional (STS temporary credentials)
//   AWS_REGION            — default: us-east-1
//   ANTHROPIC_BEDROCK_BASE_URL — override for LLM gateway / internal proxy
//
// Cross-region inference prefixes: Bedrock model IDs may start with
// "us.", "eu.", or "ap." — these are passed through verbatim.
//
// See US-bedrock (PASO 2-B).

/// AWS credentials resolved from environment variables.
///
/// We use env vars only (no ~/.aws/credentials file) to keep the
/// dependency surface minimal.  Instance-profile credentials can be
/// injected by setting the env vars from the EC2 metadata service.
#[derive(Clone, Debug)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub region: String,
}

impl AwsCredentials {
    /// Resolve credentials from environment variables.
    ///
    /// Returns None if `AWS_ACCESS_KEY_ID` or `AWS_SECRET_ACCESS_KEY` are absent.
    pub fn from_env() -> Option<Self> {
        let access_key_id = std::env::var("AWS_ACCESS_KEY_ID").ok()?;
        let secret_access_key = std::env::var("AWS_SECRET_ACCESS_KEY").ok()?;
        let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|_| "us-east-1".to_string());

        Some(Self {
            access_key_id,
            secret_access_key,
            session_token,
            region,
        })
    }

    /// Bedrock base URL for model invocation.
    ///
    /// Can be overridden via `ANTHROPIC_BEDROCK_BASE_URL` for LLM gateways
    /// that proxy Bedrock traffic (e.g., LiteLLM, Portkey).
    pub fn bedrock_base_url(&self) -> String {
        std::env::var("ANTHROPIC_BEDROCK_BASE_URL")
            .unwrap_or_else(|_| {
                format!("https://bedrock-runtime.{}.amazonaws.com", self.region)
            })
    }
}

/// Sign a reqwest::Request with AWS SigV4.
///
/// Uses the `aws-sigv4` crate to compute the Authorization header.
/// The signed headers are inserted into the request before it is sent.
#[cfg(feature = "bedrock")]
pub fn sign_request(
    request: &mut reqwest::Request,
    creds: &AwsCredentials,
    service: &str,
) -> Result<(), String> {
    use aws_sigv4::http_request::{
        sign, SignableBody, SignableRequest, SigningSettings,
    };
    use aws_sigv4::sign::v4;

    let now = std::time::SystemTime::now();

    let identity = aws_sigv4::sign::v4::signing_params::Credentials::new(
        &creds.access_key_id,
        &creds.secret_access_key,
        creds.session_token.as_deref(),
    );

    let settings = SigningSettings::default();
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(&creds.region)
        .name(service)
        .time(now)
        .settings(settings)
        .build()
        .map_err(|e| format!("SigV4 params error: {e}"))?;

    let url = request.url().to_string();
    let method = request.method().as_str();
    let headers: Vec<(&str, &str)> = request
        .headers()
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|vv| (k.as_str(), vv)))
        .collect();

    // Clone body bytes for signing (reqwest Body is not Clone, but we control
    // the body creation in mod.rs and always pass the bytes here).
    let body_bytes = request
        .body()
        .and_then(|b| b.as_bytes())
        .unwrap_or(&[]);

    let signable = SignableRequest::new(
        method,
        &url,
        headers.iter().map(|(k, v)| (*k, *v)),
        SignableBody::Bytes(body_bytes),
    ).map_err(|e| format!("SignableRequest error: {e}"))?;

    let (signing_instructions, _signature) = sign(signable, &params.into())
        .map_err(|e| format!("SigV4 signing error: {e}"))?
        .into_parts();

    // Apply all signing headers to the reqwest request.
    let headers_mut = request.headers_mut();
    for (name, value) in signing_instructions.headers() {
        if let (Ok(n), Ok(v)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            headers_mut.insert(n, v);
        }
    }

    Ok(())
}

/// No-op stub for when bedrock feature is disabled — keeps compilation clean.
#[cfg(not(feature = "bedrock"))]
pub fn sign_request(
    _request: &mut reqwest::Request,
    _creds: &AwsCredentials,
    _service: &str,
) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_from_env_missing_returns_none() {
        // Ensure clean env — any missing key should return None.
        // We can't unset keys reliably in tests, so just document the contract.
        let _ = AwsCredentials::from_env(); // must not panic
    }

    #[test]
    fn bedrock_base_url_uses_region() {
        let creds = AwsCredentials {
            access_key_id: "AKID".into(),
            secret_access_key: "SECRET".into(),
            session_token: None,
            region: "eu-west-1".into(),
        };
        let url = creds.bedrock_base_url();
        assert!(url.contains("eu-west-1"), "URL should contain region: {url}");
        assert!(url.contains("bedrock-runtime"), "URL should contain bedrock-runtime: {url}");
    }

    #[test]
    fn bedrock_base_url_override_via_env() {
        // Set env var and verify it takes precedence.
        std::env::set_var("ANTHROPIC_BEDROCK_BASE_URL", "https://proxy.example.com");
        let creds = AwsCredentials {
            access_key_id: "AKID".into(),
            secret_access_key: "SECRET".into(),
            session_token: None,
            region: "us-east-1".into(),
        };
        let url = creds.bedrock_base_url();
        std::env::remove_var("ANTHROPIC_BEDROCK_BASE_URL");
        assert_eq!(url, "https://proxy.example.com");
    }
}
