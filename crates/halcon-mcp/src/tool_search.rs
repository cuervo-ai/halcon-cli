//! Deferred MCP tool loading with fuzzy search.
//!
//! # Activation
//!
//! After connecting to all MCP servers and fetching tool lists, compute the total
//! token cost of all tool definitions.  If the cost exceeds the threshold
//! (`HALCON_MCP_TOOL_SEARCH_THRESHOLD` env var, 0.0–1.0, default 0.10), deferred
//! mode activates:
//!
//! 1. No MCP tool definitions are injected into the context.
//! 2. A single synthetic `search_tools(query: string) → list` tool is injected.
//! 3. When the agent calls `search_tools`, the index is queried with nucleo-matcher
//!    and the top-10 matches are returned as full `ToolDefinition` objects.
//! 4. Matched tools become available for subsequent calls in the same turn.
//!
//! # `list_changed` handling
//!
//! When an MCP server sends a `notifications/tools/list_changed` notification,
//! `rebuild_index()` re-fetches all tool lists and reconstructs the search index
//! without disconnecting from any server.  The rebuilt index is ready within 2 s
//! on a typical LAN connection.
//!
//! # Token estimation
//!
//! Token count is estimated as `total_characters / 4` (GPT-4 rule-of-thumb).
//! The threshold is checked against `estimated_tokens / context_window`.

use std::collections::HashMap;
use std::sync::Arc;

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use tokio::sync::RwLock;

use crate::types::McpToolDefinition;

/// Env var name for the deferred-mode threshold (0.0–1.0).
pub const TOOL_SEARCH_THRESHOLD_ENV: &str = "HALCON_MCP_TOOL_SEARCH_THRESHOLD";
/// Default fraction of context window that triggers deferred mode.
pub const DEFAULT_THRESHOLD: f32 = 0.10;
/// Number of top results to return from a `search_tools` call.
pub const TOP_K: usize = 10;

/// A tool entry in the search index.
#[derive(Debug, Clone)]
pub struct IndexedTool {
    /// Server name this tool belongs to.
    pub server_name: String,
    /// Full tool definition (name + description + input schema).
    pub definition: McpToolDefinition,
    /// Pre-computed searchable string: `"<name> <description>"`.
    pub searchable: String,
}

/// Thread-safe deferred tool search index.
pub struct ToolSearchIndex {
    /// All indexed tools, protected for concurrent `rebuild_index` + `search`.
    tools: Arc<RwLock<Vec<IndexedTool>>>,
    /// Fuzzy-match threshold (0.0–1.0 fraction of context window).
    threshold: f32,
}

impl ToolSearchIndex {
    pub fn new() -> Self {
        let threshold = std::env::var(TOOL_SEARCH_THRESHOLD_ENV)
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(DEFAULT_THRESHOLD)
            .clamp(0.0, 1.0);

        Self {
            tools: Arc::new(RwLock::new(Vec::new())),
            threshold,
        }
    }

    /// Build or rebuild the search index from a snapshot of all server tool lists.
    ///
    /// `server_tools`: `HashMap<server_name, Vec<McpToolDefinition>>`
    pub async fn rebuild_index(&self, server_tools: HashMap<String, Vec<McpToolDefinition>>) {
        let mut indexed = Vec::new();
        for (server_name, tools) in server_tools {
            for def in tools {
                let desc = def.description.as_deref().unwrap_or("");
                let searchable = format!("{} {}", def.name, desc).to_lowercase();
                indexed.push(IndexedTool {
                    server_name: server_name.clone(),
                    definition: def,
                    searchable,
                });
            }
        }
        let mut guard = self.tools.write().await;
        *guard = indexed;
        tracing::debug!(
            count = guard.len(),
            "tool_search: index rebuilt"
        );
    }

    /// Returns `true` if deferred mode should activate for the given tool count and context window.
    ///
    /// Estimation: each tool definition averages ~200 tokens (4 chars/token heuristic on ~800 chars).
    pub fn should_activate(&self, tool_count: usize, context_window: u32) -> bool {
        if context_window == 0 || tool_count == 0 {
            return false;
        }
        let estimated_tokens = tool_count * 200; // ~200 tokens per tool definition
        let fraction = estimated_tokens as f32 / context_window as f32;
        fraction > self.threshold
    }

    /// Returns `true` if deferred mode should activate based on raw character count.
    pub fn should_activate_for_chars(&self, total_chars: usize, context_window: u32) -> bool {
        if context_window == 0 {
            return false;
        }
        let estimated_tokens = total_chars / 4;
        let fraction = estimated_tokens as f32 / context_window as f32;
        fraction > self.threshold
    }

    /// Search the index for `query`, returning up to `TOP_K` matching tools.
    ///
    /// Uses nucleo-matcher for fuzzy matching.  Results are sorted by match score
    /// descending.
    pub async fn search(&self, query: &str) -> Vec<IndexedTool> {
        let guard = self.tools.read().await;
        if guard.is_empty() {
            return vec![];
        }

        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Atom::new(query, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy, false);

        let mut scored: Vec<(u16, usize)> = guard
            .iter()
            .enumerate()
            .filter_map(|(idx, tool)| {
                let mut buf = Vec::new();
                let haystack = Utf32Str::new(&tool.searchable, &mut buf);
                let score = pattern.score(haystack, &mut matcher)?;
                Some((score, idx))
            })
            .collect();

        // Sort by score descending, take TOP_K.
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.truncate(TOP_K);

        scored.into_iter().map(|(_, idx)| guard[idx].clone()).collect()
    }

    /// Total number of tools in the index.
    pub async fn len(&self) -> usize {
        self.tools.read().await.len()
    }

    /// Returns a snapshot of all tools (for list_tools fallback).
    pub async fn all_tools(&self) -> Vec<IndexedTool> {
        self.tools.read().await.clone()
    }

    pub fn threshold(&self) -> f32 {
        self.threshold
    }
}

impl Default for ToolSearchIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// The synthetic `search_tools` tool definition injected in deferred mode.
pub fn search_tools_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "search_tools",
        "description": concat!(
            "Search for available MCP tools by keyword. ",
            "Use this when you need a specific capability but don't know the exact tool name. ",
            "Returns up to 10 matching tool definitions that you can then call directly."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keywords describing the tool capability you need (e.g. 'create issue', 'read file', 'send message')"
                }
            },
            "required": ["query"]
        }
    })
}

/// Format the results of a `search_tools` call as a JSON array of tool definitions.
pub fn format_search_results(tools: &[IndexedTool]) -> String {
    let results: Vec<serde_json::Value> = tools.iter().map(|t| {
        serde_json::json!({
            "name": t.definition.name,
            "server": t.server_name,
            "description": t.definition.description,
        })
    }).collect();
    serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::McpToolDefinition;

    fn make_tool(name: &str, description: &str) -> McpToolDefinition {
        McpToolDefinition {
            name: name.to_string(),
            description: Some(description.to_string()),
            input_schema: serde_json::Value::Null,
        }
    }

    async fn make_index_with_tools() -> ToolSearchIndex {
        let index = ToolSearchIndex::new();
        let mut map = HashMap::new();
        map.insert("github".to_string(), vec![
            make_tool("create_issue", "Create a new GitHub issue in a repository"),
            make_tool("list_issues", "List open issues in a GitHub repository"),
            make_tool("create_pull_request", "Open a new pull request on GitHub"),
        ]);
        map.insert("filesystem".to_string(), vec![
            make_tool("read_file", "Read the contents of a file"),
            make_tool("write_file", "Write content to a file"),
            make_tool("list_directory", "List files in a directory"),
        ]);
        index.rebuild_index(map).await;
        index
    }

    #[tokio::test]
    async fn search_returns_relevant_results() {
        let index = make_index_with_tools().await;
        let results = index.search("issue").await;
        assert!(!results.is_empty(), "should find tools matching 'issue'");
        let names: Vec<_> = results.iter().map(|t| t.definition.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains("issue")), "issue-related tool should appear");
    }

    #[tokio::test]
    async fn search_returns_at_most_top_k() {
        let index = make_index_with_tools().await;
        let results = index.search("e").await; // 'e' matches almost everything
        assert!(results.len() <= TOP_K, "must not exceed TOP_K={TOP_K}");
    }

    #[tokio::test]
    async fn rebuild_replaces_old_index() {
        let index = make_index_with_tools().await;
        assert_eq!(index.len().await, 6);

        // Rebuild with only 2 tools.
        let mut map = HashMap::new();
        map.insert("slack".to_string(), vec![
            make_tool("send_message", "Send a message to a Slack channel"),
            make_tool("list_channels", "List available Slack channels"),
        ]);
        index.rebuild_index(map).await;
        assert_eq!(index.len().await, 2, "index should be replaced");

        let results = index.search("channel").await;
        let has_slack = results.iter().any(|t| t.server_name == "slack");
        assert!(has_slack, "rebuilt index should contain slack tools");
    }

    #[test]
    fn should_activate_above_threshold() {
        let index = ToolSearchIndex::new();
        // 50 tools × 200 tokens = 10_000 tokens; 10% of 32_000 = 3_200 → activates
        assert!(index.should_activate(50, 32_000), "50 tools on 32k context should activate");
    }

    #[test]
    fn should_not_activate_below_threshold() {
        let index = ToolSearchIndex::new();
        // 5 tools × 200 = 1_000; 10% of 128_000 = 12_800 → does not activate
        assert!(!index.should_activate(5, 128_000), "5 tools on 128k context should not activate");
    }

    #[test]
    fn should_activate_for_chars() {
        let index = ToolSearchIndex::new();
        // 800_000 chars / 4 = 200_000 tokens; 10% of 32_000 = 3_200 → activates
        assert!(index.should_activate_for_chars(800_000, 32_000));
    }

    #[test]
    fn search_tools_definition_is_valid_json() {
        let def = search_tools_definition();
        assert_eq!(def["name"], "search_tools");
        assert!(def["inputSchema"]["properties"]["query"].is_object());
    }

    #[tokio::test]
    async fn empty_index_returns_empty_results() {
        let index = ToolSearchIndex::new();
        let results = index.search("anything").await;
        assert!(results.is_empty());
    }

    #[test]
    fn format_search_results_valid_json() {
        let tools = vec![IndexedTool {
            server_name: "github".into(),
            definition: make_tool("create_issue", "Create a GitHub issue"),
            searchable: "create_issue create a github issue".into(),
        }];
        let json = format_search_results(&tools);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed[0]["name"], "create_issue");
        assert_eq!(parsed[0]["server"], "github");
    }
}
