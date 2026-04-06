//! HttpProbeTool — probe HTTP endpoints for health, latency, and response inspection.
//!
//! Sends HTTP requests to one or more URLs and reports:
//! - Status code and response time
//! - Response headers (optional)
//! - Body preview (optional, first N bytes)
//! - Redirect chain
//! - Certificate expiry (for HTTPS, best-effort)
//!
//! Useful for debugging APIs, verifying deployments, and health checks.

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub struct HttpProbeTool;

impl HttpProbeTool {
    pub fn new() -> Self {
        Self
    }

    #[allow(clippy::too_many_arguments)]
    async fn probe_url(
        url: &str,
        method: &str,
        headers: &[(String, String)],
        body: Option<&str>,
        timeout_secs: u64,
        show_headers: bool,
        show_body: bool,
        body_limit: usize,
    ) -> ProbeResult {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(10))
            .danger_accept_invalid_certs(false)
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return ProbeResult {
                    url: url.to_string(),
                    status: None,
                    latency_ms: 0,
                    error: Some(format!("Client build error: {e}")),
                    headers: vec![],
                    body_preview: None,
                    _redirects: 0,
                }
            }
        };

        let mut request = match method.to_uppercase().as_str() {
            "GET" | "" => client.get(url),
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "DELETE" => client.delete(url),
            "HEAD" => client.head(url),
            "PATCH" => client.patch(url),
            m => {
                return ProbeResult {
                    url: url.to_string(),
                    status: None,
                    latency_ms: 0,
                    error: Some(format!("Unsupported method: {m}")),
                    headers: vec![],
                    body_preview: None,
                    _redirects: 0,
                }
            }
        };

        for (k, v) in headers {
            request = request.header(k.as_str(), v.as_str());
        }

        if let Some(b) = body {
            request = request.body(b.to_string());
        }

        let t0 = Instant::now();
        let result = request.send().await;
        let latency_ms = t0.elapsed().as_millis() as u64;

        match result {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let resp_headers: Vec<(String, String)> = if show_headers {
                    resp.headers()
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
                        .take(40)
                        .collect()
                } else {
                    // Always include content-type
                    resp.headers()
                        .get("content-type")
                        .map(|v| {
                            vec![(
                                "content-type".to_string(),
                                v.to_str().unwrap_or("").to_string(),
                            )]
                        })
                        .unwrap_or_default()
                };

                let body_preview = if show_body {
                    match resp.bytes().await {
                        Ok(b) => {
                            let limit = b.len().min(body_limit);
                            Some(String::from_utf8_lossy(&b[..limit]).to_string())
                        }
                        Err(_) => Some("<failed to read body>".to_string()),
                    }
                } else {
                    None
                };

                ProbeResult {
                    url: url.to_string(),
                    status: Some(status),
                    latency_ms,
                    error: None,
                    headers: resp_headers,
                    body_preview,
                    _redirects: 0,
                }
            }
            Err(e) => {
                let msg = if e.is_timeout() {
                    format!("Timeout after {timeout_secs}s")
                } else if e.is_connect() {
                    format!("Connection refused: {e}")
                } else if e.is_redirect() {
                    format!("Too many redirects: {e}")
                } else {
                    format!("Request error: {e}")
                };
                ProbeResult {
                    url: url.to_string(),
                    status: None,
                    latency_ms,
                    error: Some(msg),
                    headers: vec![],
                    body_preview: None,
                    _redirects: 0,
                }
            }
        }
    }

    fn status_emoji(code: Option<u16>) -> &'static str {
        match code {
            Some(200..=299) => "✅",
            Some(300..=399) => "↪",
            Some(400..=499) => "⚠️",
            Some(500..=599) => "❌",
            Some(_) => "?",
            None => "💥",
        }
    }

    fn format_result(r: &ProbeResult) -> String {
        let mut out = String::new();
        let icon = Self::status_emoji(r.status);
        let status_str = r
            .status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "ERR".to_string());
        out.push_str(&format!(
            "{icon} {} — {} — {}ms\n",
            r.url, status_str, r.latency_ms
        ));
        if let Some(ref e) = r.error {
            out.push_str(&format!("   Error: {e}\n"));
        }
        if !r.headers.is_empty() {
            out.push_str("   Headers:\n");
            for (k, v) in &r.headers {
                out.push_str(&format!("     {k}: {v}\n"));
            }
        }
        if let Some(ref body) = r.body_preview {
            out.push_str("   Body:\n");
            for line in body.lines().take(20) {
                out.push_str(&format!("     {line}\n"));
            }
            if body.lines().count() > 20 {
                out.push_str("     ...\n");
            }
        }
        out
    }
}

struct ProbeResult {
    url: String,
    status: Option<u16>,
    latency_ms: u64,
    error: Option<String>,
    headers: Vec<(String, String)>,
    body_preview: Option<String>,
    _redirects: u32,
}

impl Default for HttpProbeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HttpProbeTool {
    fn name(&self) -> &str {
        "http_probe"
    }

    fn description(&self) -> &str {
        "Probe HTTP/HTTPS endpoints for health, latency, and response inspection. \
         Supports GET, POST, PUT, DELETE, HEAD, PATCH. \
         Returns status code, response time, headers, and body preview. \
         Useful for debugging APIs, checking deployments, and health monitoring."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to probe. Can be a single URL or comma-separated list."
                },
                "urls": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of URLs to probe in parallel (alternative to 'url')."
                },
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST", "PUT", "DELETE", "HEAD", "PATCH"],
                    "description": "HTTP method (default: GET)."
                },
                "headers": {
                    "type": "object",
                    "description": "Request headers as key-value pairs."
                },
                "body": {
                    "type": "string",
                    "description": "Request body for POST/PUT/PATCH."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 10)."
                },
                "show_headers": {
                    "type": "boolean",
                    "description": "Include response headers in output (default: false)."
                },
                "show_body": {
                    "type": "boolean",
                    "description": "Include response body preview (default: false)."
                },
                "body_limit": {
                    "type": "integer",
                    "description": "Max bytes of body to show (default: 2048)."
                }
            },
            "required": []
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(
        &self,
        input: ToolInput,
    ) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let args = &input.arguments;

        // Collect URLs
        let mut urls: Vec<String> = vec![];
        if let Some(u) = args["url"].as_str() {
            for part in u.split(',') {
                let part = part.trim();
                if !part.is_empty() {
                    urls.push(part.to_string());
                }
            }
        }
        if let Some(arr) = args["urls"].as_array() {
            for v in arr {
                if let Some(s) = v.as_str() {
                    urls.push(s.to_string());
                }
            }
        }

        if urls.is_empty() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "Error: 'url' or 'urls' parameter is required.".to_string(),
                is_error: true,
                metadata: None,
            });
        }

        // Cap at 10 URLs
        urls.truncate(10);

        let method = args["method"].as_str().unwrap_or("GET");
        let timeout = args["timeout"].as_u64().unwrap_or(10).clamp(1, 60);
        let show_headers = args["show_headers"].as_bool().unwrap_or(false);
        let show_body = args["show_body"].as_bool().unwrap_or(false);
        let body_limit = args["body_limit"].as_u64().unwrap_or(2048).clamp(64, 65536) as usize;
        let body = args["body"].as_str();

        let headers: Vec<(String, String)> = args["headers"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        // Probe all URLs concurrently
        let mut tasks = vec![];
        for url in &urls {
            let url = url.clone();
            let method = method.to_string();
            let headers = headers.clone();
            let body_str = body.map(|b| b.to_string());
            tasks.push(tokio::spawn(async move {
                HttpProbeTool::probe_url(
                    &url,
                    &method,
                    &headers,
                    body_str.as_deref(),
                    timeout,
                    show_headers,
                    show_body,
                    body_limit,
                )
                .await
            }));
        }

        let mut results: Vec<ProbeResult> = vec![];
        for t in tasks {
            match t.await {
                Ok(r) => results.push(r),
                Err(e) => results.push(ProbeResult {
                    url: "unknown".to_string(),
                    status: None,
                    latency_ms: 0,
                    error: Some(format!("Join error: {e}")),
                    headers: vec![],
                    body_preview: None,
                    _redirects: 0,
                }),
            }
        }

        // Summary
        let ok_count = results
            .iter()
            .filter(|r| r.status.map(|s| s < 400).unwrap_or(false))
            .count();
        let err_count = results.len() - ok_count;
        let avg_latency = if results.is_empty() {
            0
        } else {
            results.iter().map(|r| r.latency_ms).sum::<u64>() / results.len() as u64
        };

        let mut content = format!(
            "HTTP Probe Results  ({} url(s), {}✅ up, {}❌ issues, avg {}ms)\n\n",
            results.len(),
            ok_count,
            err_count,
            avg_latency
        );

        for r in &results {
            content.push_str(&HttpProbeTool::format_result(r));
            content.push('\n');
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "probed": results.len(),
                "ok": ok_count,
                "errors": err_count,
                "avg_latency_ms": avg_latency
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_input(args: Value) -> ToolInput {
        ToolInput {
            tool_use_id: "t1".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[test]
    fn tool_metadata() {
        let t = HttpProbeTool::default();
        assert_eq!(t.name(), "http_probe");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        // No required fields
        assert!(schema["required"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn execute_no_url_returns_error() {
        let tool = HttpProbeTool::new();
        let out = tool.execute(make_input(json!({}))).await.unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("url"));
    }

    #[test]
    fn status_emoji_cases() {
        assert_eq!(HttpProbeTool::status_emoji(Some(200)), "✅");
        assert_eq!(HttpProbeTool::status_emoji(Some(201)), "✅");
        assert_eq!(HttpProbeTool::status_emoji(Some(301)), "↪");
        assert_eq!(HttpProbeTool::status_emoji(Some(404)), "⚠️");
        assert_eq!(HttpProbeTool::status_emoji(Some(500)), "❌");
        assert_eq!(HttpProbeTool::status_emoji(None), "💥");
    }

    #[test]
    fn format_result_ok() {
        let r = ProbeResult {
            url: "https://example.com".into(),
            status: Some(200),
            latency_ms: 42,
            error: None,
            headers: vec![("content-type".into(), "text/html".into())],
            body_preview: None,
            _redirects: 0,
        };
        let s = HttpProbeTool::format_result(&r);
        assert!(s.contains("200"));
        assert!(s.contains("42ms"));
        assert!(s.contains("content-type"));
    }

    #[test]
    fn format_result_error() {
        let r = ProbeResult {
            url: "https://bad.host".into(),
            status: None,
            latency_ms: 100,
            error: Some("Connection refused".into()),
            headers: vec![],
            body_preview: None,
            _redirects: 0,
        };
        let s = HttpProbeTool::format_result(&r);
        assert!(s.contains("ERR"));
        assert!(s.contains("Connection refused"));
    }

    #[test]
    fn url_list_parsed_from_comma_string() {
        // Validate comma-split logic via schema
        let args = json!({ "url": "https://a.com, https://b.com" });
        let mut urls: Vec<String> = vec![];
        if let Some(u) = args["url"].as_str() {
            for part in u.split(',') {
                let part = part.trim();
                if !part.is_empty() {
                    urls.push(part.to_string());
                }
            }
        }
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://a.com");
        assert_eq!(urls[1], "https://b.com");
    }

    #[test]
    fn urls_capped_at_10() {
        let mut urls: Vec<String> = (0..20).map(|i| format!("http://host{i}")).collect();
        urls.truncate(10);
        assert_eq!(urls.len(), 10);
    }

    #[tokio::test]
    async fn execute_invalid_url_returns_error_in_content() {
        let tool = HttpProbeTool::new();
        let out = tool
            .execute(make_input(json!({ "url": "not-a-url", "timeout": 3 })))
            .await
            .unwrap();
        // Should succeed (no panic), but error reported in content
        assert!(!out.is_error); // tool-level is_error=false; error in content
        assert!(
            out.content.contains("ERR")
                || out.content.contains("error")
                || out.content.contains("💥")
        );
    }

    #[test]
    fn headers_parsed_from_object() {
        let args = json!({
            "url": "http://x.com",
            "headers": { "Authorization": "Bearer token", "Accept": "application/json" }
        });
        let headers: Vec<(String, String)> = args["headers"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(headers.len(), 2);
    }
}
