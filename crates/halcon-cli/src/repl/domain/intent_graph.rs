//! `IntentGraph` — declarative tool-to-intent mapping for tool selection.
//!
//! Phase 2 addition behind `feature = "intent-graph"`.
//!
//! # Why this exists
//! The existing `ToolSelector` covers only 25 of 61 registered tools via
//! keyword matching. `IntentGraph` provides a declarative graph where each
//! tool node declares which intents it serves. When enabled, `ToolSelector`
//! consults the graph first; tools not covered by the graph fall through to
//! the existing keyword logic unchanged.
//!
//! # Design
//! - No model calls required — pure data-driven lookup.
//! - Loaded from a TOML config file OR built from compiled defaults.
//! - Additive to `ToolSelector`: existing behavior is the fallback, not replaced.
//! - Phase 4 will expand the default coverage to all 61 tools.
//! - Future: intent edge weights enable UCB1-style tool routing learning.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// An intent label — a short, lowercase, underscore-separated identifier.
///
/// Examples: "file_read", "code_execution", "web_search", "git_operation".
pub type IntentLabel = String;

/// A single tool node in the intent graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolNode {
    /// The exact tool name as registered in the `ToolRegistry`.
    pub tool_name: String,
    /// Human-readable description (used in docs and diagnostics, not routing).
    pub description: String,
    /// Intent labels this tool serves.
    ///
    /// A tool may serve multiple intents (e.g., `bash` serves `code_execution`,
    /// `file_operation`, and `build`).
    pub intents: Vec<IntentLabel>,
    /// Optional priority within this intent group [0.0, 1.0].
    ///
    /// Higher-priority tools appear first in the selected tool list.
    /// Default: 0.5.
    #[serde(default = "default_priority")]
    pub priority: f32,
}

fn default_priority() -> f32 { 0.5 }

/// Query context derived from `IntentProfile`.
///
/// A simplified view of `IntentProfile` for graph queries — avoids importing
/// the full `IntentProfile` type into the graph module.
#[derive(Debug, Clone)]
pub struct GraphQuery {
    /// Primary intent labels to match (derived from TaskType).
    pub primary_intents: Vec<IntentLabel>,
    /// Secondary intent labels (derived from scope/depth, optional).
    pub secondary_intents: Vec<IntentLabel>,
    /// Maximum number of tools to return (0 = no limit).
    pub max_tools: usize,
}

impl GraphQuery {
    pub fn new(primary_intents: Vec<IntentLabel>) -> Self {
        Self { primary_intents, secondary_intents: vec![], max_tools: 0 }
    }

    pub fn with_secondary(mut self, secondary: Vec<IntentLabel>) -> Self {
        self.secondary_intents = secondary;
        self
    }

    pub fn with_max(mut self, max: usize) -> Self {
        self.max_tools = max;
        self
    }
}

/// Graph query result.
#[derive(Debug, Clone)]
pub struct GraphResult {
    /// Tools matched by primary intents, sorted by priority desc.
    pub primary_tools: Vec<String>,
    /// Additional tools matched by secondary intents (not in primary_tools).
    pub secondary_tools: Vec<String>,
}

impl GraphResult {
    /// Merge primary and secondary into a single deduplicated list.
    pub fn merged(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for t in self.primary_tools.iter().chain(self.secondary_tools.iter()) {
            if seen.insert(t.clone()) {
                out.push(t.clone());
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.primary_tools.is_empty() && self.secondary_tools.is_empty()
    }
}

/// The intent graph — maps intents to tool nodes.
///
/// # Thread safety
/// `IntentGraph` is immutable after construction and `Send + Sync`.
pub struct IntentGraph {
    /// intent_label → sorted Vec<ToolNode> (sorted by priority desc)
    index: HashMap<IntentLabel, Vec<ToolNode>>,
    /// All registered tool nodes (for diagnostics).
    nodes: Vec<ToolNode>,
}

impl IntentGraph {
    /// Build an `IntentGraph` from a list of `ToolNode` definitions.
    pub fn from_nodes(nodes: Vec<ToolNode>) -> Self {
        let mut index: HashMap<IntentLabel, Vec<ToolNode>> = HashMap::new();
        for node in &nodes {
            for intent in &node.intents {
                index.entry(intent.clone()).or_default().push(node.clone());
            }
        }
        // Sort each intent bucket by priority desc.
        for bucket in index.values_mut() {
            bucket.sort_by(|a, b| b.priority.partial_cmp(&a.priority).unwrap_or(std::cmp::Ordering::Equal));
        }
        Self { index, nodes }
    }

    /// Build the compiled default graph covering all registered tools.
    ///
    /// Covers all 63 registered halcon-tools with intent annotations.
    pub fn default_graph() -> Self {
        Self::from_nodes(default_tool_nodes())
    }

    /// Query the graph for tools matching a `GraphQuery`.
    ///
    /// Returns `GraphResult` with primary and secondary tool lists.
    /// If no tools match the primary intents, returns an empty result
    /// so the caller can fall back to the existing keyword logic.
    pub fn query(&self, q: &GraphQuery) -> GraphResult {
        let primary_tools = self.collect_tools(&q.primary_intents, q.max_tools);
        let secondary_tools = {
            let already: HashSet<&str> = primary_tools.iter().map(|s| s.as_str()).collect();
            let mut sec = self.collect_tools(&q.secondary_intents, 0);
            sec.retain(|t| !already.contains(t.as_str()));
            if q.max_tools > 0 && primary_tools.len() + sec.len() > q.max_tools {
                sec.truncate(q.max_tools.saturating_sub(primary_tools.len()));
            }
            sec
        };
        GraphResult { primary_tools, secondary_tools }
    }

    /// Return all tool names in the graph (for diagnostics).
    pub fn all_tool_names(&self) -> Vec<String> {
        self.nodes.iter().map(|n| n.tool_name.clone()).collect()
    }

    /// Coverage: fraction of `registered_tool_names` covered by this graph.
    pub fn coverage(&self, registered_tool_names: &[&str]) -> f32 {
        if registered_tool_names.is_empty() { return 0.0; }
        let graph_tools: HashSet<&str> = self.nodes.iter().map(|n| n.tool_name.as_str()).collect();
        let covered = registered_tool_names.iter().filter(|t| graph_tools.contains(*t)).count();
        covered as f32 / registered_tool_names.len() as f32
    }

    fn collect_tools(&self, intents: &[IntentLabel], max: usize) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out: Vec<(String, f32)> = Vec::new();
        for intent in intents {
            if let Some(nodes) = self.index.get(intent) {
                for node in nodes {
                    if seen.insert(node.tool_name.clone()) {
                        out.push((node.tool_name.clone(), node.priority));
                    }
                }
            }
        }
        // Sort by priority desc (nodes were pre-sorted per intent bucket,
        // but merging across intents requires a re-sort).
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let tools: Vec<String> = out.into_iter().map(|(t, _)| t).collect();
        if max > 0 { tools.into_iter().take(max).collect() } else { tools }
    }
}

/// Default tool nodes covering all 63 registered halcon-tools.
///
/// Expanded in Phase 4 from the initial 26-tool set to cover the full registry.
/// Intent labels follow the existing taxonomy; new labels are additive (string-typed).
fn default_tool_nodes() -> Vec<ToolNode> {
    vec![
        // ── File operations ───────────────────────────────────────────────
        ToolNode { tool_name: "file_read".into(),    description: "Read file contents".into(),     intents: vec!["file_operation".into(), "code_review".into()], priority: 0.9 },
        ToolNode { tool_name: "file_write".into(),   description: "Write/create a file".into(),    intents: vec!["file_operation".into(), "artifact_creation".into()], priority: 0.9 },
        ToolNode { tool_name: "file_edit".into(),    description: "Edit file in-place".into(),     intents: vec!["file_operation".into(), "code_edit".into()], priority: 0.85 },
        ToolNode { tool_name: "file_delete".into(),  description: "Delete a file".into(),          intents: vec!["file_operation".into()], priority: 0.6 },
        ToolNode { tool_name: "file_inspect".into(), description: "Inspect file metadata".into(),  intents: vec!["file_operation".into(), "discovery".into()], priority: 0.7 },
        ToolNode { tool_name: "file_diff".into(),    description: "Diff two files".into(),         intents: vec!["file_operation".into(), "code_review".into()], priority: 0.7 },
        ToolNode { tool_name: "directory_tree".into(), description: "List directory tree".into(),  intents: vec!["file_operation".into(), "discovery".into()], priority: 0.75 },
        // NOTE: registered tool name is "glob" (not "glob_tool")
        ToolNode { tool_name: "glob".into(),         description: "Match files by pattern".into(), intents: vec!["file_operation".into(), "discovery".into()], priority: 0.8 },
        ToolNode { tool_name: "archive".into(),      description: "Compress/extract archives".into(), intents: vec!["file_operation".into()], priority: 0.55 },
        ToolNode { tool_name: "checksum".into(),     description: "Compute file checksums".into(), intents: vec!["file_operation".into(), "security".into()], priority: 0.5 },
        ToolNode { tool_name: "patch_apply".into(),  description: "Apply a patch file".into(),     intents: vec!["file_operation".into(), "code_edit".into()], priority: 0.7 },
        ToolNode { tool_name: "diff_apply".into(),   description: "Apply unified diff".into(),     intents: vec!["file_operation".into(), "code_edit".into()], priority: 0.7 },

        // ── Search & retrieval ────────────────────────────────────────────
        ToolNode { tool_name: "grep".into(),          description: "Search file contents".into(),   intents: vec!["search".into(), "code_review".into()], priority: 0.9 },
        ToolNode { tool_name: "semantic_grep".into(), description: "Semantic code search".into(),   intents: vec!["search".into(), "code_review".into()], priority: 0.8 },
        ToolNode { tool_name: "symbol_search".into(), description: "Search code symbols".into(),    intents: vec!["search".into(), "code_review".into()], priority: 0.8 },
        ToolNode { tool_name: "web_search".into(),    description: "Web search".into(),             intents: vec!["web_access".into(), "search".into()], priority: 0.85 },
        ToolNode { tool_name: "web_fetch".into(),     description: "Fetch URL content".into(),      intents: vec!["web_access".into()], priority: 0.8 },
        ToolNode { tool_name: "fuzzy_find".into(),    description: "Fuzzy file search".into(),      intents: vec!["search".into(), "discovery".into()], priority: 0.75 },
        ToolNode { tool_name: "native_search".into(), description: "Local indexed search".into(),   intents: vec!["search".into(), "discovery".into()], priority: 0.75 },
        ToolNode { tool_name: "native_index_query".into(), description: "Query native index".into(), intents: vec!["search".into(), "discovery".into()], priority: 0.7 },
        ToolNode { tool_name: "native_crawl".into(),  description: "Crawl local codebase".into(),   intents: vec!["search".into(), "discovery".into()], priority: 0.65 },

        // ── Code execution & testing ──────────────────────────────────────
        ToolNode { tool_name: "bash".into(),          description: "Execute shell command".into(),  intents: vec!["code_execution".into(), "build".into(), "testing".into()], priority: 0.95 },
        ToolNode { tool_name: "execute_test".into(),  description: "Run test suite".into(),         intents: vec!["testing".into(), "code_execution".into()], priority: 0.85 },
        ToolNode { tool_name: "test_run".into(),      description: "Run tests".into(),              intents: vec!["testing".into(), "code_execution".into()], priority: 0.85 },
        ToolNode { tool_name: "lint_check".into(),    description: "Lint code".into(),              intents: vec!["code_review".into(), "testing".into()], priority: 0.7 },
        ToolNode { tool_name: "syntax_check".into(),  description: "Check syntax".into(),           intents: vec!["code_review".into(), "testing".into()], priority: 0.7 },
        ToolNode { tool_name: "code_coverage".into(), description: "Measure test coverage".into(),  intents: vec!["testing".into(), "code_review".into()], priority: 0.65 },
        ToolNode { tool_name: "make".into(),          description: "Run make targets".into(),       intents: vec!["build".into(), "code_execution".into()], priority: 0.75 },
        ToolNode { tool_name: "docker".into(),        description: "Docker container ops".into(),   intents: vec!["code_execution".into(), "build".into()], priority: 0.7 },
        ToolNode { tool_name: "regex_test".into(),    description: "Test regex patterns".into(),    intents: vec!["testing".into(), "analysis".into()], priority: 0.55 },
        ToolNode { tool_name: "test_data_gen".into(), description: "Generate test data".into(),     intents: vec!["testing".into()], priority: 0.5 },

        // ── Git operations ────────────────────────────────────────────────
        ToolNode { tool_name: "git_status".into(),    description: "Git status".into(),             intents: vec!["git_operation".into()], priority: 0.85 },
        ToolNode { tool_name: "git_diff".into(),      description: "Git diff".into(),               intents: vec!["git_operation".into(), "code_review".into()], priority: 0.85 },
        ToolNode { tool_name: "git_log".into(),       description: "Git log".into(),                intents: vec!["git_operation".into(), "discovery".into()], priority: 0.75 },
        ToolNode { tool_name: "git_commit".into(),    description: "Git commit".into(),             intents: vec!["git_operation".into()], priority: 0.7 },
        ToolNode { tool_name: "git_branch".into(),    description: "Git branch management".into(),  intents: vec!["git_operation".into()], priority: 0.65 },
        ToolNode { tool_name: "git_add".into(),       description: "Stage files".into(),            intents: vec!["git_operation".into()], priority: 0.65 },
        ToolNode { tool_name: "git_stash".into(),     description: "Git stash ops".into(),          intents: vec!["git_operation".into()], priority: 0.6 },
        ToolNode { tool_name: "git_blame".into(),     description: "Git blame history".into(),      intents: vec!["git_operation".into(), "code_review".into()], priority: 0.65 },
        ToolNode { tool_name: "changelog_gen".into(), description: "Generate changelog".into(),     intents: vec!["git_operation".into(), "artifact_creation".into()], priority: 0.6 },

        // ── HTTP / external ───────────────────────────────────────────────
        ToolNode { tool_name: "http_request".into(),  description: "HTTP request".into(),           intents: vec!["web_access".into(), "api_testing".into()], priority: 0.75 },
        ToolNode { tool_name: "http_probe".into(),    description: "Probe HTTP endpoint".into(),    intents: vec!["web_access".into(), "api_testing".into()], priority: 0.7 },
        ToolNode { tool_name: "port_check".into(),    description: "Check port availability".into(), intents: vec!["discovery".into(), "api_testing".into()], priority: 0.6 },
        ToolNode { tool_name: "url_parse".into(),     description: "Parse URL components".into(),   intents: vec!["analysis".into(), "web_access".into()], priority: 0.5 },

        // ── Analysis & metrics ────────────────────────────────────────────
        ToolNode { tool_name: "code_metrics".into(),     description: "Code complexity metrics".into(),  intents: vec!["code_review".into(), "discovery".into()], priority: 0.65 },
        ToolNode { tool_name: "dep_check".into(),        description: "Dependency audit".into(),         intents: vec!["discovery".into(), "security".into()], priority: 0.6 },
        ToolNode { tool_name: "dependency_graph".into(), description: "Visualize dependencies".into(),   intents: vec!["discovery".into(), "code_review".into()], priority: 0.6 },
        ToolNode { tool_name: "token_count".into(),      description: "Count tokens in text".into(),     intents: vec!["analysis".into()], priority: 0.5 },
        ToolNode { tool_name: "env_inspect".into(),      description: "Inspect environment vars".into(), intents: vec!["discovery".into(), "code_execution".into()], priority: 0.55 },
        ToolNode { tool_name: "perf_analyze".into(),     description: "Analyze performance data".into(), intents: vec!["analysis".into(), "code_review".into()], priority: 0.6 },
        ToolNode { tool_name: "parse_logs".into(),       description: "Parse log files".into(),          intents: vec!["analysis".into(), "discovery".into()], priority: 0.6 },
        ToolNode { tool_name: "ci_logs".into(),          description: "Fetch CI build logs".into(),      intents: vec!["discovery".into(), "testing".into()], priority: 0.65 },
        ToolNode { tool_name: "task_track".into(),       description: "Track task progress".into(),      intents: vec!["analysis".into()], priority: 0.5 },

        // ── Security & validation ─────────────────────────────────────────
        ToolNode { tool_name: "secret_scan".into(),        description: "Scan for secrets/keys".into(),    intents: vec!["security".into(), "code_review".into()], priority: 0.7 },
        ToolNode { tool_name: "openapi_validate".into(),   description: "Validate OpenAPI spec".into(),    intents: vec!["api_testing".into(), "code_review".into()], priority: 0.65 },
        ToolNode { tool_name: "json_schema_validate".into(), description: "Validate JSON schema".into(),  intents: vec!["code_review".into(), "api_testing".into()], priority: 0.6 },
        ToolNode { tool_name: "config_validate".into(),    description: "Validate config file".into(),    intents: vec!["code_review".into(), "testing".into()], priority: 0.6 },

        // ── Process management ────────────────────────────────────────────
        ToolNode { tool_name: "process_list".into(),    description: "List running processes".into(),    intents: vec!["discovery".into(), "process_management".into()], priority: 0.6 },
        ToolNode { tool_name: "process_monitor".into(), description: "Monitor process metrics".into(),   intents: vec!["discovery".into(), "process_management".into()], priority: 0.6 },
        ToolNode { tool_name: "background_start".into(), description: "Start background process".into(), intents: vec!["code_execution".into(), "process_management".into()], priority: 0.7 },
        ToolNode { tool_name: "background_output".into(), description: "Read background output".into(),  intents: vec!["code_execution".into(), "process_management".into()], priority: 0.65 },
        ToolNode { tool_name: "background_kill".into(),  description: "Kill background process".into(),  intents: vec!["code_execution".into(), "process_management".into()], priority: 0.6 },

        // ── Data transformation ───────────────────────────────────────────
        ToolNode { tool_name: "json_transform".into(),  description: "Transform JSON data".into(),       intents: vec!["analysis".into(), "data_processing".into()], priority: 0.6 },
        ToolNode { tool_name: "sql_query".into(),       description: "Execute SQL query".into(),         intents: vec!["analysis".into(), "data_processing".into()], priority: 0.65 },
        ToolNode { tool_name: "template_engine".into(), description: "Render templates".into(),          intents: vec!["artifact_creation".into()], priority: 0.55 },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_graph() -> IntentGraph {
        IntentGraph::default_graph()
    }

    #[test]
    fn query_file_operation_returns_file_tools() {
        let g = test_graph();
        let q = GraphQuery::new(vec!["file_operation".into()]);
        let result = g.query(&q);
        assert!(!result.is_empty());
        assert!(result.primary_tools.contains(&"file_read".to_string()));
        assert!(result.primary_tools.contains(&"file_write".to_string()));
    }

    #[test]
    fn query_git_operation_returns_git_tools() {
        let g = test_graph();
        let q = GraphQuery::new(vec!["git_operation".into()]);
        let result = g.query(&q);
        assert!(result.primary_tools.contains(&"git_status".to_string()));
        assert!(result.primary_tools.contains(&"git_diff".to_string()));
    }

    #[test]
    fn query_unknown_intent_returns_empty() {
        let g = test_graph();
        let q = GraphQuery::new(vec!["nonexistent_intent".into()]);
        let result = g.query(&q);
        assert!(result.is_empty());
    }

    #[test]
    fn query_with_max_tools_respected() {
        let g = test_graph();
        let q = GraphQuery::new(vec!["file_operation".into()]).with_max(3);
        let result = g.query(&q);
        assert!(result.primary_tools.len() <= 3);
    }

    #[test]
    fn secondary_tools_excluded_from_primary() {
        let g = test_graph();
        let q = GraphQuery::new(vec!["git_operation".into()])
            .with_secondary(vec!["file_operation".into()]);
        let result = g.query(&q);
        let primary_set: HashSet<&str> = result.primary_tools.iter().map(|s| s.as_str()).collect();
        for t in &result.secondary_tools {
            assert!(!primary_set.contains(t.as_str()), "secondary tool {t} also in primary");
        }
    }

    #[test]
    fn merged_is_deduplicated() {
        let g = test_graph();
        // code_review appears in both file_operation and git_operation
        let q = GraphQuery::new(vec!["file_operation".into()])
            .with_secondary(vec!["file_operation".into()]); // same intent — would duplicate
        let result = g.query(&q);
        let merged = result.merged();
        let unique: HashSet<&str> = merged.iter().map(|s| s.as_str()).collect();
        assert_eq!(merged.len(), unique.len(), "merged must not contain duplicates");
    }

    #[test]
    fn coverage_above_zero() {
        let g = test_graph();
        let registered = ["file_read", "bash", "grep", "git_status", "web_search"];
        let cov = g.coverage(&registered);
        assert!(cov > 0.0);
        assert!(cov <= 1.0);
    }

    #[test]
    fn all_tool_names_non_empty() {
        let g = test_graph();
        let names = g.all_tool_names();
        assert!(!names.is_empty());
        for n in &names {
            assert!(!n.is_empty());
        }
    }

    #[test]
    fn high_priority_tools_first() {
        let g = test_graph();
        let q = GraphQuery::new(vec!["code_execution".into()]);
        let result = g.query(&q);
        // bash has priority 0.95, should be first
        if !result.primary_tools.is_empty() {
            assert_eq!(result.primary_tools[0], "bash");
        }
    }
}
