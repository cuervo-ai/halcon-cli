//! Web Fetch tool — HTTP GET with content extraction.
//!
//! Fetches a URL and returns the body as text. HTML is stripped
//! to a simplified text representation. Respects configurable
//! timeout and max response size.

use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

/// Maximum response body size (1 MB).
const MAX_BODY_BYTES: usize = 1_048_576;
/// Default request timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Fetch content from a URL.
pub struct WebFetchTool;

impl WebFetchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL. Returns the response body as text. HTML tags are stripped."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let url = input.arguments["url"]
            .as_str()
            .ok_or_else(|| CuervoError::ToolExecutionFailed {
                tool: "web_fetch".into(),
                message: "missing required 'url' argument".into(),
            })?;

        // Validate URL scheme.
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(CuervoError::ToolExecutionFailed {
                tool: "web_fetch".into(),
                message: format!("invalid URL scheme: must be http or https — got '{url}'"),
            });
        }

        let timeout_secs = input.arguments["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent("cuervo-cli/0.1")
            .build()
            .map_err(|e| CuervoError::ToolExecutionFailed {
                tool: "web_fetch".into(),
                message: format!("failed to build HTTP client: {e}"),
            })?;

        let response = client.get(url).send().await.map_err(|e| {
            CuervoError::ToolExecutionFailed {
                tool: "web_fetch".into(),
                message: format!("request failed: {e}"),
            }
        })?;

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        if !response.status().is_success() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("HTTP {status}: request to {url} failed"),
                is_error: true,
                metadata: Some(json!({
                    "status": status,
                    "url": url,
                })),
            });
        }

        // Read body with size limit.
        let bytes = response.bytes().await.map_err(|e| {
            CuervoError::ToolExecutionFailed {
                tool: "web_fetch".into(),
                message: format!("failed to read response body: {e}"),
            }
        })?;

        let truncated = bytes.len() > MAX_BODY_BYTES;
        let body_bytes = if truncated {
            &bytes[..MAX_BODY_BYTES]
        } else {
            &bytes[..]
        };

        let body_text = String::from_utf8_lossy(body_bytes).to_string();

        // If HTML, strip tags for readability.
        let content = if content_type.contains("text/html") {
            strip_html_tags(&body_text)
        } else {
            body_text
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "status": status,
                "content_type": content_type,
                "bytes": bytes.len(),
                "truncated": truncated,
                "url": url,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch (http or https)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Request timeout in seconds (default: 30)"
                }
            },
            "required": ["url"]
        })
    }
}

/// Strip HTML tags, collapsing whitespace. Keeps text content only.
///
/// Single-pass, byte-level implementation. Avoids O(n) Vec<char> allocations
/// and per-tag String creation of the naive approach.
fn strip_html_tags(html: &str) -> String {
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut result = String::with_capacity(len / 3);
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut last_was_space = false;
    let mut i = 0;

    while i < len {
        if bytes[i] == b'<' {
            // Check for script/style and block-level tags using byte slices.
            if tag_starts_with_ci(bytes, i, b"<script") {
                in_script = true;
            } else if tag_starts_with_ci(bytes, i, b"</script") {
                in_script = false;
            } else if tag_starts_with_ci(bytes, i, b"<style") {
                in_style = true;
            } else if tag_starts_with_ci(bytes, i, b"</style") {
                in_style = false;
            }
            // Newline for block elements.
            if is_block_tag(bytes, i) {
                if !result.ends_with('\n') {
                    result.push('\n');
                }
                last_was_space = true;
            }
            in_tag = true;
        } else if bytes[i] == b'>' {
            in_tag = false;
        } else if !in_tag && !in_script && !in_style {
            // Check for HTML entity (&...;) — decode inline in one pass.
            if bytes[i] == b'&' {
                if let Some((decoded, advance)) = decode_entity(bytes, i) {
                    result.push_str(decoded);
                    last_was_space = decoded == " ";
                    i += advance;
                    continue;
                }
            }
            if bytes[i].is_ascii_whitespace() {
                if !last_was_space {
                    result.push(' ');
                    last_was_space = true;
                }
            } else {
                // Safe: we only push non-whitespace ASCII or valid UTF-8.
                // For multi-byte UTF-8 chars, find the full character boundary.
                let ch_start = i;
                let c = html[ch_start..].chars().next().unwrap_or('?');
                result.push(c);
                last_was_space = false;
                i += c.len_utf8();
                continue;
            }
        }
        i += 1;
    }

    result
}

/// Case-insensitive tag prefix match on byte slice (no allocation).
#[inline]
fn tag_starts_with_ci(bytes: &[u8], pos: usize, prefix: &[u8]) -> bool {
    if pos + prefix.len() > bytes.len() {
        return false;
    }
    bytes[pos..pos + prefix.len()]
        .iter()
        .zip(prefix.iter())
        .all(|(a, b)| a.to_ascii_lowercase() == *b)
}

/// Check if the tag at `pos` is a block-level element.
#[inline]
fn is_block_tag(bytes: &[u8], pos: usize) -> bool {
    tag_starts_with_ci(bytes, pos, b"<br")
        || tag_starts_with_ci(bytes, pos, b"<p")
        || tag_starts_with_ci(bytes, pos, b"</p")
        || tag_starts_with_ci(bytes, pos, b"<div")
        || tag_starts_with_ci(bytes, pos, b"</div")
        || tag_starts_with_ci(bytes, pos, b"<h")
        || tag_starts_with_ci(bytes, pos, b"</h")
        || tag_starts_with_ci(bytes, pos, b"<li")
        || tag_starts_with_ci(bytes, pos, b"<tr")
}

/// Decode an HTML entity starting at `&` at position `pos`.
/// Returns (decoded_str, bytes_to_skip) or None if not a recognized entity.
#[inline]
fn decode_entity(bytes: &[u8], pos: usize) -> Option<(&'static str, usize)> {
    let remaining = &bytes[pos..];
    // Common entities (most frequent first).
    if remaining.starts_with(b"&amp;") {
        Some(("&", 5))
    } else if remaining.starts_with(b"&lt;") {
        Some(("<", 4))
    } else if remaining.starts_with(b"&gt;") {
        Some((">", 4))
    } else if remaining.starts_with(b"&quot;") {
        Some(("\"", 6))
    } else if remaining.starts_with(b"&#39;") {
        Some(("'", 5))
    } else if remaining.starts_with(b"&nbsp;") {
        Some((" ", 6))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_simple_html() {
        let html = "<p>Hello <b>world</b></p>";
        let text = strip_html_tags(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("world"));
        assert!(!text.contains("<p>"));
        assert!(!text.contains("<b>"));
    }

    #[test]
    fn strip_script_tags() {
        let html = "<p>Before</p><script>alert('hi')</script><p>After</p>";
        let text = strip_html_tags(html);
        assert!(text.contains("Before"));
        assert!(text.contains("After"));
        assert!(!text.contains("alert"));
    }

    #[test]
    fn strip_style_tags() {
        let html = "<style>body { color: red; }</style><p>Content</p>";
        let text = strip_html_tags(html);
        assert!(text.contains("Content"));
        assert!(!text.contains("color"));
    }

    #[test]
    fn decode_html_entities() {
        let html = "<p>A &amp; B &lt; C &gt; D</p>";
        let text = strip_html_tags(html);
        assert!(text.contains("A & B < C > D"));
    }

    #[test]
    fn collapse_whitespace() {
        let html = "<p>  lots   of    spaces  </p>";
        let text = strip_html_tags(html);
        // Should not have consecutive spaces.
        assert!(!text.contains("  "));
    }

    #[test]
    fn input_schema_valid() {
        let tool = WebFetchTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["url"].is_object());
        assert_eq!(schema["required"][0], "url");
    }

    #[tokio::test]
    async fn missing_url_argument() {
        let tool = WebFetchTool::new();
        let input = ToolInput {
            tool_use_id: "test".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        let result = tool.execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn invalid_url_scheme() {
        let tool = WebFetchTool::new();
        let input = ToolInput {
            tool_use_id: "test".into(),
            arguments: json!({"url": "ftp://example.com"}),
            working_directory: "/tmp".into(),
        };
        let result = tool.execute(input).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid URL scheme"));
    }

    #[test]
    fn tool_metadata() {
        let tool = WebFetchTool::new();
        assert_eq!(tool.name(), "web_fetch");
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
        assert!(!tool.description().is_empty());
    }
}
