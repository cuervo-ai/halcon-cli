//! UrlParseTool — parse, validate and inspect URLs.
//!
//! Extracts components (scheme, host, port, path, query params, fragment),
//! checks for common security issues, and normalizes URLs to canonical form.

use async_trait::async_trait;
use serde_json::json;
use url::Url;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

/// Decode a percent-encoded string (best-effort).
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte as char);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn parse_query_params(query: &str) -> Vec<(String, String)> {
    if query.is_empty() {
        return vec![];
    }
    query
        .split('&')
        .filter_map(|pair| {
            let mut it = pair.splitn(2, '=');
            let key = it.next()?;
            let val = it.next().unwrap_or("");
            Some((percent_decode(key), percent_decode(val)))
        })
        .collect()
}

const SENSITIVE_PARAMS: &[&str] = &[
    "password",
    "passwd",
    "token",
    "api_key",
    "apikey",
    "secret",
    "access_token",
    "auth",
    "key",
    "credentials",
    "session",
    "ssn",
];

fn check_security_issues(url: &Url) -> Vec<String> {
    let mut issues = Vec::new();

    // Sensitive query parameters
    if let Some(query) = url.query() {
        let params = parse_query_params(query);
        for (k, _) in &params {
            let kl = k.to_lowercase();
            if SENSITIVE_PARAMS.iter().any(|s| kl.contains(s)) {
                issues.push(format!("Sensitive parameter in query string: '{k}'"));
            }
        }
    }

    // Credentials embedded in URL
    if !url.username().is_empty() {
        issues.push("Username embedded in URL (credentials exposure risk)".to_string());
    }
    if url.password().is_some() {
        issues.push("Password embedded in URL (credentials exposure risk)".to_string());
    }

    // Localhost / private addresses
    if let Some(host) = url.host_str() {
        let h = host.to_lowercase();
        if h == "localhost" || h.starts_with("127.") || h == "::1" || h == "0.0.0.0" {
            issues.push(format!("URL points to localhost/loopback: {host}"));
        }
        if h.starts_with("192.168.") || h.starts_with("10.") || h.starts_with("172.") {
            issues.push(format!("URL points to private network address: {host}"));
        }
    }

    // Unencrypted HTTP
    if url.scheme() == "http" {
        issues.push("URL uses HTTP (unencrypted) — consider HTTPS".to_string());
    }

    // Open redirect patterns
    if let Some(query) = url.query() {
        let params = parse_query_params(query);
        for (k, v) in &params {
            let kl = k.to_lowercase();
            if matches!(kl.as_str(), "redirect" | "url" | "next" | "return" | "goto")
                && v.starts_with("http")
            {
                issues.push(format!("Potential open redirect via '{k}' parameter: {v}"));
            }
        }
    }

    issues
}

fn normalize_url(url: &Url) -> String {
    let mut out = format!("{}://", url.scheme());
    if let Some(host) = url.host_str() {
        out.push_str(&host.to_lowercase());
    }
    // Only include port if non-default
    let add_port = match (url.scheme(), url.port()) {
        ("http", Some(80)) | ("https", Some(443)) => false,
        (_, Some(_)) => true,
        _ => false,
    };
    if add_port {
        if let Some(port) = url.port() {
            out.push(':');
            out.push_str(&port.to_string());
        }
    }
    let path = url.path();
    if path != "/" {
        out.push_str(path.trim_end_matches('/'));
    }
    if let Some(q) = url.query() {
        out.push('?');
        out.push_str(q);
    }
    if let Some(frag) = url.fragment() {
        out.push('#');
        out.push_str(frag);
    }
    out
}

// ─── Tool struct ──────────────────────────────────────────────────────────────

pub struct UrlParseTool;

impl UrlParseTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UrlParseTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for UrlParseTool {
    fn name(&self) -> &str {
        "url_parse"
    }

    fn description(&self) -> &str {
        "Parse, validate and inspect URLs. Extracts components (scheme, host, port, path, \
         query params, fragment), checks security issues, and normalizes URLs."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["parse", "validate", "params", "security", "normalize"],
                    "description": "parse=full breakdown, validate=check if valid URL, params=query params only, security=security audit, normalize=canonical form"
                },
                "url": {
                    "type": "string",
                    "description": "The URL to process"
                }
            },
            "required": ["operation", "url"]
        })
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let args = &input.arguments;
        let operation = args["operation"]
            .as_str()
            .ok_or_else(|| HalconError::InvalidInput("operation required".into()))?;

        let raw_url = args["url"]
            .as_str()
            .ok_or_else(|| HalconError::InvalidInput("url required".into()))?;

        if raw_url.trim().is_empty() {
            return Err(HalconError::InvalidInput("url cannot be empty".into()));
        }

        let content = match operation {
            "validate" => match Url::parse(raw_url) {
                Ok(_) => format!("✓ Valid URL: {raw_url}"),
                Err(e) => format!("✗ Invalid URL: {e}"),
            },

            "parse" => {
                let url = Url::parse(raw_url)
                    .map_err(|e| HalconError::InvalidInput(format!("Invalid URL: {e}")))?;

                let params = url.query().map(parse_query_params).unwrap_or_default();
                let params_str = if params.is_empty() {
                    "(none)".to_string()
                } else {
                    params
                        .iter()
                        .map(|(k, v)| format!("    {k} = {v}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                };

                format!(
                    "URL Breakdown:\n\
                     Scheme   : {}\n\
                     Host     : {}\n\
                     Port     : {}\n\
                     Path     : {}\n\
                     Query    : {}\n\
                     Fragment : {}\n\
                     Origin   : {}\n\
                     Params:\n{}",
                    url.scheme(),
                    url.host_str().unwrap_or("(none)"),
                    url.port()
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "(default)".to_string()),
                    url.path(),
                    url.query().unwrap_or("(none)"),
                    url.fragment().unwrap_or("(none)"),
                    url.origin().ascii_serialization(),
                    params_str,
                )
            }

            "params" => {
                let url = Url::parse(raw_url)
                    .map_err(|e| HalconError::InvalidInput(format!("Invalid URL: {e}")))?;

                let params = url.query().map(parse_query_params).unwrap_or_default();
                if params.is_empty() {
                    "No query parameters found.".to_string()
                } else {
                    let mut lines = vec![format!("{} query parameter(s):", params.len())];
                    for (k, v) in &params {
                        lines.push(format!("  {k} = {v}"));
                    }
                    lines.join("\n")
                }
            }

            "security" => {
                let url = Url::parse(raw_url)
                    .map_err(|e| HalconError::InvalidInput(format!("Invalid URL: {e}")))?;

                let issues = check_security_issues(&url);
                if issues.is_empty() {
                    format!("✓ No security issues found in: {raw_url}")
                } else {
                    let mut lines = vec![format!("{} security issue(s) found:", issues.len())];
                    for (i, issue) in issues.iter().enumerate() {
                        lines.push(format!("  {}. {issue}", i + 1));
                    }
                    lines.join("\n")
                }
            }

            "normalize" => {
                let url = Url::parse(raw_url)
                    .map_err(|e| HalconError::InvalidInput(format!("Invalid URL: {e}")))?;
                let normalized = normalize_url(&url);
                format!("Original  : {raw_url}\nNormalized: {normalized}")
            }

            _ => {
                return Err(HalconError::InvalidInput(format!(
                    "Unknown operation: {operation}"
                )))
            }
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content,
            is_error: false,
            metadata: None,
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_query_params_basic() {
        let params = parse_query_params("foo=bar&baz=qux");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], ("foo".to_string(), "bar".to_string()));
        assert_eq!(params[1], ("baz".to_string(), "qux".to_string()));
    }

    #[test]
    fn parse_query_params_empty() {
        assert!(parse_query_params("").is_empty());
    }

    #[test]
    fn parse_query_params_no_value() {
        let params = parse_query_params("flag");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], ("flag".to_string(), "".to_string()));
    }

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("foo%3Dbar"), "foo=bar");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn security_check_http() {
        let url = Url::parse("http://example.com/path").unwrap();
        let issues = check_security_issues(&url);
        assert!(issues.iter().any(|i| i.contains("HTTP")));
    }

    #[test]
    fn security_check_sensitive_param() {
        let url = Url::parse("https://example.com/?api_key=secret123").unwrap();
        let issues = check_security_issues(&url);
        assert!(issues.iter().any(|i| i.contains("api_key")));
    }

    #[test]
    fn security_check_localhost() {
        let url = Url::parse("https://localhost:8080/api").unwrap();
        let issues = check_security_issues(&url);
        assert!(issues.iter().any(|i| i.contains("localhost")));
    }

    #[test]
    fn normalize_removes_default_port() {
        let url = Url::parse("https://example.com:443/path").unwrap();
        let normalized = normalize_url(&url);
        assert!(!normalized.contains(":443"));
        assert!(normalized.contains("example.com"));
    }

    #[test]
    fn tool_name_and_permission() {
        let tool = UrlParseTool::new();
        assert_eq!(tool.name(), "url_parse");
        assert!(matches!(tool.permission_level(), PermissionLevel::ReadOnly));
    }

    #[tokio::test]
    async fn execute_validate_valid_url() {
        let tool = UrlParseTool::new();
        let input = ToolInput {
            tool_use_id: "t1".into(),
            arguments: serde_json::json!({"operation": "validate", "url": "https://example.com/path?q=1"}),
            working_directory: ".".into(),
        };
        let out = tool.execute(input).await.unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains('✓'));
    }

    #[tokio::test]
    async fn execute_validate_invalid_url() {
        let tool = UrlParseTool::new();
        let input = ToolInput {
            tool_use_id: "t2".into(),
            arguments: serde_json::json!({"operation": "validate", "url": "not a url"}),
            working_directory: ".".into(),
        };
        let out = tool.execute(input).await.unwrap();
        assert!(out.content.contains('✗'));
    }

    #[tokio::test]
    async fn execute_params_operation() {
        let tool = UrlParseTool::new();
        let input = ToolInput {
            tool_use_id: "t3".into(),
            arguments: serde_json::json!({"operation": "params", "url": "https://api.example.com/search?q=rust&lang=en"}),
            working_directory: ".".into(),
        };
        let out = tool.execute(input).await.unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("q = rust"));
        assert!(out.content.contains("lang = en"));
    }
}
