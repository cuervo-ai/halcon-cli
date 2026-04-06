//! `native_crawl` tool: crawl and index web pages.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

/// Shared search engine instance.
pub type SharedSearchEngine = Arc<RwLock<Option<halcon_search::SearchEngine>>>;

pub struct NativeCrawlTool {
    engine: SharedSearchEngine,
}

impl NativeCrawlTool {
    pub fn new(engine: SharedSearchEngine) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl Tool for NativeCrawlTool {
    fn name(&self) -> &str {
        "native_crawl"
    }

    fn description(&self) -> &str {
        "Crawl a website and index its pages. Fetches HTML content and adds to the local search index."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadWrite // Writes to local index
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let url = input.arguments["url"].as_str().ok_or_else(|| {
            HalconError::InvalidInput("native_crawl requires 'url' string".into())
        })?;

        // Validate URL
        let parsed_url = url::Url::parse(url)
            .map_err(|e| HalconError::InvalidInput(format!("Invalid URL: {}", e)))?;

        if !parsed_url.scheme().starts_with("http") {
            return Err(HalconError::InvalidInput(
                "URL must use http or https scheme".into(),
            ));
        }

        // Check if engine is initialized
        let engine_guard = self.engine.read().await;
        let engine = match engine_guard.as_ref() {
            Some(eng) => eng,
            None => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: "Search engine not initialized.\n\n\
                             To use native crawl, ensure the search index is configured:\n\
                             1. Check ~/.halcon/config.toml for [search] section\n\
                             2. Set search.enabled = true\n\
                             3. Restart halcon\n\n\
                             Cannot index URLs without search engine."
                        .to_string(),
                    is_error: true,
                    metadata: Some(json!({
                        "status": "not_initialized",
                        "url": url,
                    })),
                });
            }
        };

        // Fetch and index the URL
        let doc_id = match engine.index_url(parsed_url.clone()).await {
            Ok(id) => id,
            Err(e) => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!("Failed to crawl and index URL: {}\n\nError: {}", url, e),
                    is_error: true,
                    metadata: Some(json!({
                        "error": e.to_string(),
                        "url": url,
                    })),
                });
            }
        };

        // Success response
        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: format!(
                "Successfully crawled and indexed: {}\n\n\
                 Document ID: {}\n\
                 Status: Indexed and searchable via native_search tool\n\n\
                 Note: Currently indexing single page only. Recursive crawling with depth parameter coming in future update.",
                url, doc_id
            ),
            is_error: false,
            metadata: Some(json!({
                "url": url,
                "doc_id": doc_id,
                "status": "indexed",
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch and index."
                }
            },
            "required": ["url"]
        })
    }
}
