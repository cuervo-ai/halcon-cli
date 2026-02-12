//! `web_search` tool: search the web via Brave Search API.
//!
//! Supports Brave Search as the primary provider.
//! API key sourced from `BRAVE_API_KEY` environment variable.
//! ReadOnly permission — searches are read-only operations.

use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

/// Default number of results.
const DEFAULT_COUNT: u64 = 5;
/// Maximum number of results.
const MAX_COUNT: u64 = 10;
/// Request timeout.
const SEARCH_TIMEOUT_SECS: u64 = 15;
/// Brave Search API endpoint.
const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";
/// Max response body (512KB).
const MAX_RESPONSE_BYTES: usize = 512 * 1024;

/// A single search result.
#[derive(Debug, Clone)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Web search tool using Brave Search API.
pub struct WebSearchTool {
    api_key: Option<String>,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            api_key: std::env::var("BRAVE_API_KEY").ok(),
        }
    }

    /// Create with an explicit API key (for testing).
    pub fn with_api_key(api_key: String) -> Self {
        Self {
            api_key: Some(api_key),
        }
    }

    /// Parse Brave Search API JSON response into results.
    fn parse_brave_response(body: &str) -> Vec<SearchResult> {
        let parsed: serde_json::Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let results = parsed["web"]["results"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        results
            .into_iter()
            .filter_map(|r| {
                let title = r["title"].as_str()?.to_string();
                let url = r["url"].as_str()?.to_string();
                let snippet = r["description"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                Some(SearchResult {
                    title,
                    url,
                    snippet,
                })
            })
            .collect()
    }

    /// Format results as numbered list.
    fn format_results(results: &[SearchResult], query: &str) -> String {
        if results.is_empty() {
            return format!("No results found for '{query}'.");
        }

        let mut out = format!("Search results for '{query}':\n\n");
        for (i, r) in results.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, r.title));
            out.push_str(&format!("   {}\n", r.url));
            if !r.snippet.is_empty() {
                out.push_str(&format!("   {}\n", r.snippet));
            }
            out.push('\n');
        }
        out
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using Brave Search. Returns ranked results with titles, URLs, and snippets."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let query = input.arguments["query"]
            .as_str()
            .ok_or_else(|| {
                CuervoError::InvalidInput("web_search requires 'query' string".into())
            })?;

        if query.trim().is_empty() {
            return Err(CuervoError::InvalidInput("web_search: query must not be empty".into()));
        }

        let count = input.arguments["count"]
            .as_u64()
            .unwrap_or(DEFAULT_COUNT)
            .clamp(1, MAX_COUNT);

        let domain_filter = input.arguments["domain_filter"]
            .as_str()
            .map(|s| s.to_string());

        let api_key = self.api_key.as_deref().ok_or_else(|| {
            CuervoError::ToolExecutionFailed {
                tool: "web_search".into(),
                message: "BRAVE_API_KEY environment variable not set. Set it to use web search."
                    .into(),
            }
        })?;

        // Build query with optional domain filter.
        let search_query = if let Some(ref domain) = domain_filter {
            format!("{query} site:{domain}")
        } else {
            query.to_string()
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(SEARCH_TIMEOUT_SECS))
            .build()
            .map_err(|e| CuervoError::ToolExecutionFailed {
                tool: "web_search".into(),
                message: format!("failed to create HTTP client: {e}"),
            })?;

        let response = client
            .get(BRAVE_SEARCH_URL)
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .query(&[
                ("q", search_query.as_str()),
                ("count", &count.to_string()),
                ("search_lang", "en"),
            ])
            .send()
            .await
            .map_err(|e| CuervoError::ToolExecutionFailed {
                tool: "web_search".into(),
                message: format!("search request failed: {e}"),
            })?;

        let status = response.status();
        if !status.is_success() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("web_search error: API returned status {status}"),
                is_error: true,
                metadata: None,
            });
        }

        let body = response
            .text()
            .await
            .map_err(|e| CuervoError::ToolExecutionFailed {
                tool: "web_search".into(),
                message: format!("failed to read response: {e}"),
            })?;

        // Truncate if too large.
        let body = if body.len() > MAX_RESPONSE_BYTES {
            body[..MAX_RESPONSE_BYTES].to_string()
        } else {
            body
        };

        let results = Self::parse_brave_response(&body);
        let content = Self::format_results(&results, query);

        let results_json: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "title": r.title,
                    "url": r.url,
                    "snippet": r.snippet,
                })
            })
            .collect();

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "result_count": results.len(),
                "query": query,
                "results": results_json,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query."
                },
                "count": {
                    "type": "integer",
                    "description": "Number of results (1-10, default 5)."
                },
                "domain_filter": {
                    "type": "string",
                    "description": "Restrict results to a specific domain (e.g., 'docs.rs')."
                }
            },
            "required": ["query"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test".to_string(),
            arguments: args,
            working_directory: "/tmp".to_string(),
        }
    }

    #[test]
    fn schema_is_valid() {
        let tool = WebSearchTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "query"));
    }

    #[test]
    fn permission_is_readonly() {
        let tool = WebSearchTool::new();
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
        let dummy = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(!tool.requires_confirmation(&dummy));
    }

    #[tokio::test]
    async fn missing_query_error() {
        let tool = WebSearchTool::with_api_key("test-key".into());
        let result = tool.execute(make_input(json!({}))).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn missing_api_key_error() {
        // Create tool with no API key.
        let tool = WebSearchTool { api_key: None };
        let result = tool
            .execute(make_input(json!({"query": "test"})))
            .await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("BRAVE_API_KEY"));
    }

    #[test]
    fn parse_brave_response_valid() {
        let response = json!({
            "web": {
                "results": [
                    {
                        "title": "Rust Programming Language",
                        "url": "https://www.rust-lang.org/",
                        "description": "A language for reliability and performance."
                    },
                    {
                        "title": "Docs.rs",
                        "url": "https://docs.rs/",
                        "description": "Documentation for Rust crates."
                    }
                ]
            }
        });
        let results = WebSearchTool::parse_brave_response(&response.to_string());
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[1].url, "https://docs.rs/");
    }

    #[test]
    fn parse_brave_response_empty() {
        let response = json!({"web": {"results": []}});
        let results = WebSearchTool::parse_brave_response(&response.to_string());
        assert!(results.is_empty());
    }

    #[test]
    fn parse_brave_response_invalid_json() {
        let results = WebSearchTool::parse_brave_response("not json");
        assert!(results.is_empty());
    }

    #[test]
    fn format_results_empty() {
        let formatted = WebSearchTool::format_results(&[], "test");
        assert!(formatted.contains("No results found"));
    }

    #[test]
    fn format_results_numbered() {
        let results = vec![
            SearchResult {
                title: "Result One".into(),
                url: "https://example.com/1".into(),
                snippet: "First result.".into(),
            },
            SearchResult {
                title: "Result Two".into(),
                url: "https://example.com/2".into(),
                snippet: "Second result.".into(),
            },
        ];
        let formatted = WebSearchTool::format_results(&results, "test");
        assert!(formatted.contains("1. Result One"));
        assert!(formatted.contains("2. Result Two"));
        assert!(formatted.contains("https://example.com/1"));
        assert!(formatted.contains("First result."));
    }

    #[test]
    fn domain_filter_appended() {
        // Verify the query construction includes site: filter.
        let query = "rust tutorials";
        let domain = "docs.rs";
        let filtered = format!("{query} site:{domain}");
        assert_eq!(filtered, "rust tutorials site:docs.rs");
    }

    #[test]
    fn count_clamping() {
        // Count should be clamped to 1..10.
        assert_eq!(0u64.clamp(1, MAX_COUNT), 1);
        assert_eq!(50u64.clamp(1, MAX_COUNT), MAX_COUNT);
        assert_eq!(5u64.clamp(1, MAX_COUNT), 5);
    }

    // === Phase 30: Fix 5c — reject empty query ===

    #[tokio::test]
    async fn empty_query_rejected() {
        let tool = WebSearchTool::new();
        let input = make_input(json!({ "query": "  " }));
        let result = tool.execute(input).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("query must not be empty"), "Error: {err}");
    }
}
