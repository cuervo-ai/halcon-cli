//! `native_search` tool: semantic search over local index.
//!
//! Searches the native halcon-search index for documents matching the query.
//! Integrates BM25 ranking, PageRank, freshness scoring, and optional semantic embeddings.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

/// Shared search engine instance.
///
/// Wrapped in Arc<RwLock<Option<_>>> to allow lazy initialization.
pub type SharedSearchEngine = Arc<RwLock<Option<halcon_search::SearchEngine>>>;

pub struct NativeSearchTool {
    engine: SharedSearchEngine,
}

impl NativeSearchTool {
    pub fn new(engine: SharedSearchEngine) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl Tool for NativeSearchTool {
    fn name(&self) -> &str {
        "native_search"
    }

    fn description(&self) -> &str {
        "Search the local document index using semantic search. \
         Returns ranked results with titles, URLs, snippets, and relevance scores. \
         Integrates BM25, PageRank, and freshness signals."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let query = input.arguments["query"].as_str().ok_or_else(|| {
            HalconError::InvalidInput("native_search requires 'query' string".into())
        })?;

        if query.trim().is_empty() {
            return Err(HalconError::InvalidInput("Query cannot be empty".into()));
        }

        let limit = input
            .arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(50) as usize;

        // Check if engine is initialized
        let engine_guard = self.engine.read().await;
        let engine = match engine_guard.as_ref() {
            Some(eng) => eng,
            None => {
                // CRITICAL FIX: Return is_error=true so the agent loop correctly
                // classifies this as a deterministic error (matches "not initialized"
                // in is_deterministic_error()) and stops retrying this tool.
                // Previous is_error=false caused the agent to treat the "not initialized"
                // message as successful output and continue calling the tool repeatedly.
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: "Error: search engine not initialized.\n\n\
                             The local search index is not available in this session.\n\
                             Use file_read, bash, or grep to search the codebase directly.\n\n\
                             To enable native search in future sessions:\n\
                             1. Check ~/.halcon/config.toml for [search] section\n\
                             2. Run `halcon index init` to create the index\n\
                             3. Use native_crawl to populate documents"
                        .to_string(),
                    is_error: true,
                    metadata: Some(json!({
                        "status": "not_initialized",
                        "query": query,
                        "fallback": "use file_read, bash, grep"
                    })),
                });
            }
        };

        // Execute search
        let results = match engine.search(query).await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!(
                        "Search failed: {}\n\nTry checking index status with native_index_query.",
                        e
                    ),
                    is_error: true,
                    metadata: Some(json!({
                        "error": e.to_string(),
                        "query": query,
                    })),
                });
            }
        };

        // Format results
        let mut content = format!("Search Results for: \"{}\"\n", query);
        content.push_str(&format!(
            "Found {} results (showing top {})\n\n",
            results.total_count,
            limit.min(results.results.len())
        ));

        if results.results.is_empty() {
            content.push_str("No documents match your query.\n\n");
            content.push_str("Suggestions:\n");
            content.push_str("- Try different keywords\n");
            content.push_str("- Use native_crawl to index more content\n");
            content.push_str("- Check native_index_query stats to see indexed documents\n");
        } else {
            for (i, result) in results.results.iter().take(limit).enumerate() {
                content.push_str(&format!(
                    "{}. {} (score: {:.3})\n",
                    i + 1,
                    result.document.title,
                    result.score
                ));
                content.push_str(&format!("   URL: {}\n", result.document.url));
                if !result.snippet.is_empty() {
                    content.push_str(&format!("   {}\n", result.snippet));
                }
                content.push('\n');
            }

            // Query timing
            content.push_str(&format!(
                "Search completed in {:.2}ms\n",
                results.elapsed_ms
            ));
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "query": query,
                "total_results": results.total_count,
                "returned_count": results.results.len().min(limit),
                "duration_ms": results.elapsed_ms,
                "has_more": results.total_count > limit,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (keywords or natural language)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results to return (1-50, default 10).",
                    "minimum": 1,
                    "maximum": 50
                }
            },
            "required": ["query"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_tool() -> NativeSearchTool {
        let engine = Arc::new(RwLock::new(None));
        NativeSearchTool::new(engine)
    }

    #[test]
    fn test_name() {
        let tool = make_tool();
        assert_eq!(tool.name(), "native_search");
    }

    #[test]
    fn test_permission_level() {
        let tool = make_tool();
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
    }

    #[tokio::test]
    async fn test_execute_engine_not_initialized() {
        let tool = make_tool();

        let input = ToolInput {
            tool_use_id: "test-1".to_string(),
            arguments: json!({"query": "machine learning"}),
            working_directory: std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        };

        let output = tool.execute(input).await.unwrap();

        // is_error=true so the agent loop classifies it as deterministic error
        // and stops retrying rather than treating it as successful output.
        assert!(
            output.is_error,
            "engine_not_initialized must return is_error=true"
        );
        assert!(output.content.contains("not initialized"));
    }

    #[tokio::test]
    async fn test_execute_empty_query() {
        let tool = make_tool();

        let input = ToolInput {
            tool_use_id: "test-2".to_string(),
            arguments: json!({"query": "   "}),
            working_directory: std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        };

        let result = tool.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[tokio::test]
    async fn test_execute_missing_query() {
        let tool = make_tool();

        let input = ToolInput {
            tool_use_id: "test-3".to_string(),
            arguments: json!({}),
            working_directory: std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        };

        let result = tool.execute(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires 'query'"));
    }

    #[test]
    fn test_input_schema() {
        let tool = make_tool();
        let schema = tool.input_schema();

        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert!(schema["properties"]["limit"].is_object());
        assert_eq!(schema["required"], json!(["query"]));
    }
}
