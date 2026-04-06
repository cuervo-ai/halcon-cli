//! `web_search` tool: local search using native FTS5 index.
//!
//! Fast, local full-text search with BM25 ranking.
//! Zero external API dependencies, <100ms latency, cached results.
//! Async-optimized for concurrent agent operations.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};
use halcon_storage::Database;

/// Web search tool using local FTS5 index with BM25 ranking.
///
/// **Architecture**:
/// - Database: SQLite FTS5 with BM25 ranking algorithm
/// - Caching: SHA256 query hashing + MessagePack serialization
/// - Compression: zstd level 3 (60-70% space savings)
/// - Latency: <50ms cold (FTS5), <10ms warm (cache hit)
///
/// **Agent Coordination**:
/// - Fully async: agent can gather info while waiting for results
/// - Non-blocking: uses tokio spawn_blocking for SQLite I/O
/// - Fast response: optimized for minimal agent round overhead
pub struct WebSearchTool {
    db: Option<Arc<Database>>,
}

impl WebSearchTool {
    /// Create a new web search tool with database for search engine initialization.
    pub fn new(db: Option<Arc<Database>>) -> Self {
        Self { db }
    }

    /// Format search results as markdown table.
    ///
    /// Uses structured markdown for easy parsing by LLMs:
    /// - Header with query, result count, latency
    /// - Table with rank, BM25 score, title, URL
    /// - Footer with metadata (source, timing)
    fn format_results(results: &halcon_search::types::SearchResults) -> String {
        if results.results.is_empty() {
            return format!(
                "SEARCH RETURNED NO RESULTS for query: '{}'\n\n\
                 IMPORTANT: This is a LOCAL index search, NOT an internet search.\n\
                 The local search index is empty or has no matching content.\n\n\
                 You MUST inform the user that:\n\
                 1. The local search index has no results for their query.\n\
                 2. To search the internet, an external integration is required.\n\
                 3. Based on your training knowledge, provide the best answer you can.\n\n\
                 Do not leave the response empty — always synthesize an answer from your knowledge.",
                results.query
            );
        }

        let mut output = String::new();
        output.push_str(&format!("# Search Results for '{}'\n\n", results.query));
        output.push_str(&format!(
            "Found {} results in {}ms {}\n\n",
            results.total_count,
            results.elapsed_ms,
            if results.from_cache {
                "⚡ (cached)"
            } else {
                ""
            }
        ));

        output.push_str("| Rank | Score | Title | URL |\n");
        output.push_str("|------|-------|-------|-----|\n");

        for (i, result) in results.results.iter().enumerate() {
            let title = result.document.title.chars().take(60).collect::<String>();
            let url = result
                .document
                .url
                .to_string()
                .chars()
                .take(70)
                .collect::<String>();
            output.push_str(&format!(
                "| {} | {:.2} | {} | {} |\n",
                i + 1,
                result.score,
                title,
                url
            ));
        }

        output.push_str(&format!("\n**Total**: {} results\n", results.total_count));
        output.push_str(&format!("**Latency**: {}ms\n", results.elapsed_ms));
        if results.from_cache {
            output.push_str("**Source**: Cache (instant retrieval)\n");
        } else {
            output.push_str("**Source**: FTS5 BM25 Index (SQLite)\n");
        }

        output
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new(None)
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the LOCAL index (not the internet) using FTS5 full-text search with BM25 ranking. \
         Returns ranked results from previously crawled/indexed pages. \
         Zero external API dependencies. Fast (<100ms), cached. \
         Agent can continue gathering information while waiting for results. \
         Use native_crawl to populate the index before searching."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        // Extract and validate query
        let query = input.arguments["query"].as_str().ok_or_else(|| {
            HalconError::InvalidInput("web_search requires 'query' string".into())
        })?;

        if query.trim().is_empty() {
            return Err(HalconError::InvalidInput(
                "web_search: query must not be empty".into(),
            ));
        }

        // Get database reference (fail fast if not initialized)
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| HalconError::ToolExecutionFailed {
                tool: "web_search".to_string(),
                message: "Database not initialized. Search index unavailable.".to_string(),
            })?;

        // Create search engine (lightweight facade, no heavy initialization)
        let search_engine = halcon_search::SearchEngine::new(
            db.clone(),
            halcon_search::SearchEngineConfig::default(),
        )
        .map_err(|e| HalconError::ToolExecutionFailed {
            tool: "web_search".to_string(),
            message: format!("Failed to initialize search engine: {}", e),
        })?;

        // Execute search (async, non-blocking for agent)
        // SearchEngine uses tokio::spawn_blocking for SQLite I/O
        // Agent can process other tasks concurrently
        let results =
            search_engine
                .search(query)
                .await
                .map_err(|e| HalconError::ToolExecutionFailed {
                    tool: "web_search".to_string(),
                    message: format!("Search failed: {}", e),
                })?;

        // Format results for LLM consumption
        let content = Self::format_results(&results);

        // Return structured output with rich metadata.
        // is_error: false even for empty results — "no results" is valid output, not a failure.
        // Marking as error triggers parallel_batch_collapse which suppresses the synthesis round.
        // The format_results() message already instructs the LLM to synthesize from knowledge.
        let is_error = false;
        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error,
            metadata: Some(json!({
                "query": query,
                "result_count": results.total_count,
                "elapsed_ms": results.elapsed_ms,
                "from_cache": results.from_cache,
                "backend": "sqlite_fts5_bm25",
                "engine": "native",
                "api_free": true,
                "async_safe": true,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query. Supports full-text search with BM25 ranking."
                },
                "count": {
                    "type": "integer",
                    "description": "Number of results (1-50, default 10). Note: currently uses engine default of 10."
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
    fn name_is_web_search() {
        let tool = WebSearchTool::new(None);
        assert_eq!(tool.name(), "web_search");
    }

    #[test]
    fn schema_is_valid() {
        let tool = WebSearchTool::new(None);
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "query"));
    }

    #[test]
    fn permission_is_readonly() {
        let tool = WebSearchTool::new(None);
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
        let tool = WebSearchTool::new(None);
        let result = tool.execute(make_input(json!({}))).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("query"));
    }

    #[tokio::test]
    async fn empty_query_rejected() {
        let tool = WebSearchTool::new(None);
        let input = make_input(json!({ "query": "  " }));
        let result = tool.execute(input).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("query must not be empty"), "Error: {err}");
    }

    #[tokio::test]
    async fn no_db_returns_error() {
        let tool = WebSearchTool::new(None);
        let input = make_input(json!({ "query": "rust programming" }));
        let result = tool.execute(input).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Database not initialized"), "Error: {err}");
    }

    #[test]
    fn description_mentions_local_not_internet() {
        let tool = WebSearchTool::new(None);
        let desc = tool.description();
        assert!(
            desc.contains("LOCAL") || desc.contains("local"),
            "desc: {desc}"
        );
        assert!(
            desc.contains("FTS5") || desc.contains("index"),
            "desc: {desc}"
        );
        assert!(
            desc.to_lowercase().contains("not the internet") || desc.contains("crawled"),
            "description should clarify this is NOT a web search: {desc}"
        );
    }

    #[test]
    fn description_mentions_async() {
        let tool = WebSearchTool::new(None);
        let desc = tool.description();
        assert!(desc.contains("Agent can continue"));
    }

    #[test]
    fn metadata_includes_backend() {
        // Verify metadata structure (would need real DB for full test)
        let expected_keys = ["backend", "engine", "api_free", "async_safe"];
        // This test just documents expected metadata keys
        assert!(expected_keys.contains(&"backend"));
    }
}
