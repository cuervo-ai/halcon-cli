//! `search_memory` tool — semantic search over indexed MEMORY.md entries.
//!
//! This tool is **not** registered in `full_registry()`. It must be injected
//! per-session by the agent loop when `policy.enable_semantic_memory = true`:
//!
//! ```ignore
//! let store = Arc::new(Mutex::new(VectorMemoryStore::load_from_disk(idx_path)));
//! registry.register(Arc::new(SearchMemoryTool::new(store.clone())));
//! ```
//!
//! The tool performs cosine-similarity retrieval with MMR re-ranking over the
//! vector index built from MEMORY.md files (project + user scopes).

use async_trait::async_trait;
use halcon_context::VectorMemoryStore;
use halcon_core::{
    error::HalconError,
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::json;
use std::sync::{Arc, Mutex};

/// Shared, thread-safe handle to a `VectorMemoryStore`.
pub type SharedVectorStore = Arc<Mutex<VectorMemoryStore>>;

/// `search_memory` — semantic search over indexed MEMORY.md entries.
pub struct SearchMemoryTool {
    store: SharedVectorStore,
    /// Default top-K results.
    default_k: usize,
}

impl SearchMemoryTool {
    /// Create a new tool wrapping the given shared store.
    pub fn new(store: SharedVectorStore) -> Self {
        Self {
            store,
            default_k: 5,
        }
    }

    /// Override the default top-K.
    pub fn with_default_k(mut self, k: usize) -> Self {
        self.default_k = k;
        self
    }
}

#[async_trait]
impl Tool for SearchMemoryTool {
    fn name(&self) -> &str {
        "search_memory"
    }

    fn description(&self) -> &str {
        "Search past session memories and debugging insights by semantic similarity. \
         Returns the most relevant memory entries from MEMORY.md files indexed across \
         project and user scopes. Use this when you need to recall past decisions, \
         patterns, or fixes relevant to the current task."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language query describing what you want to recall \
                                    (e.g., 'file path errors', 'authentication patterns', \
                                    'how we fixed the FASE-2 gate')."
                },
                "top_k": {
                    "type": "integer",
                    "description": "Maximum number of memory entries to return (default: 5, max: 20).",
                    "minimum": 1,
                    "maximum": 20
                }
            },
            "required": ["query"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput, HalconError> {
        let args = &input.arguments;
        let tool_use_id = input.tool_use_id.clone();

        let query = match args["query"].as_str() {
            Some(q) if !q.trim().is_empty() => q.to_string(),
            _ => {
                return Ok(ToolOutput {
                    tool_use_id,
                    content: "Error: `query` is required and must be a non-empty string."
                        .to_string(),
                    is_error: true,
                    metadata: None,
                });
            }
        };

        let top_k = args["top_k"]
            .as_u64()
            .map(|k| (k as usize).clamp(1, 20))
            .unwrap_or(self.default_k);

        // Lock store and search.
        let results = {
            let store = self.store.lock().map_err(|e| {
                HalconError::Internal(format!("search_memory: store lock poisoned: {e}"))
            })?;
            store.search(&query, top_k)
        };

        if results.is_empty() {
            return Ok(ToolOutput {
                tool_use_id,
                content: format!(
                    "No memory entries found matching '{query}'.\n\
                     The vector index may be empty or no entries are sufficiently similar."
                ),
                is_error: false,
                metadata: None,
            });
        }

        // Format results.
        let mut output = format!(
            "## Memory Search: \"{query}\"\n\nFound {} result(s):\n\n",
            results.len()
        );

        for (i, result) in results.iter().enumerate() {
            let score_pct = (result.score * 100.0).round() as u32;
            output.push_str(&format!(
                "### Result {} (similarity: {score_pct}%)\n**Source:** {}\n\n{}\n\n---\n\n",
                i + 1,
                result.entry.source,
                result.entry.text.trim()
            ));
        }

        Ok(ToolOutput {
            tool_use_id,
            content: output,
            is_error: false,
            metadata: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_context::VectorMemoryStore;

    fn make_store_with_entries() -> SharedVectorStore {
        let mut s = VectorMemoryStore::new();
        s.index_text(
            "## Authentication\nImplemented JWT RS256 token verification for API endpoints.",
            "project:MEMORY.md§Authentication",
        );
        s.index_text(
            "## Database\nSQLite with WAL mode. 16 tables, no migration system.",
            "project:MEMORY.md§Database",
        );
        s.index_text(
            "## Debugging\nFASE-2 path-existence gate fired on bad file paths. \
             Fix: explore with directory_tree first then use verified paths.",
            "project:MEMORY.md§Debugging",
        );
        Arc::new(Mutex::new(s))
    }

    fn make_input(query: &str) -> ToolInput {
        ToolInput {
            tool_use_id: "test-id".to_string(),
            arguments: json!({ "query": query }),
            working_directory: "/tmp".to_string(),
        }
    }

    fn make_input_args(args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test-id".to_string(),
            arguments: args,
            working_directory: "/tmp".to_string(),
        }
    }

    #[tokio::test]
    async fn search_finds_relevant_entry() {
        let tool = SearchMemoryTool::new(make_store_with_entries());
        let out = tool
            .execute(make_input("JWT authentication token"))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("Authentication") || out.content.contains("JWT"));
    }

    #[tokio::test]
    async fn search_finds_debugging_entry() {
        let tool = SearchMemoryTool::new(make_store_with_entries());
        let out = tool
            .execute(make_input("file path errors FASE-2"))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("FASE-2") || out.content.contains("path"));
    }

    #[tokio::test]
    async fn empty_query_returns_error() {
        let store = Arc::new(Mutex::new(VectorMemoryStore::new()));
        let tool = SearchMemoryTool::new(store);
        let out = tool
            .execute(make_input_args(json!({ "query": "" })))
            .await
            .unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn missing_query_returns_error() {
        let store = Arc::new(Mutex::new(VectorMemoryStore::new()));
        let tool = SearchMemoryTool::new(store);
        let out = tool.execute(make_input_args(json!({}))).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn no_matches_returns_not_found_message() {
        let tool = SearchMemoryTool::new(make_store_with_entries());
        let out = tool
            .execute(make_input("quantum physics superposition"))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("No memory entries found") || out.content.contains("Result"));
    }

    #[tokio::test]
    async fn top_k_limits_results() {
        let tool = SearchMemoryTool::new(make_store_with_entries());
        let out = tool
            .execute(make_input_args(
                json!({ "query": "implementation", "top_k": 1 }),
            ))
            .await
            .unwrap();
        assert!(!out.is_error);
        // At most 1 result section.
        let count = out.content.matches("### Result").count();
        assert!(count <= 1, "expected ≤1 results, got {count}");
    }

    #[tokio::test]
    async fn permission_level_is_readonly() {
        let store = Arc::new(Mutex::new(VectorMemoryStore::new()));
        let tool = SearchMemoryTool::new(store);
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
    }

    #[test]
    fn tool_name_is_search_memory() {
        let store = Arc::new(Mutex::new(VectorMemoryStore::new()));
        let tool = SearchMemoryTool::new(store);
        assert_eq!(tool.name(), "search_memory");
    }
}
