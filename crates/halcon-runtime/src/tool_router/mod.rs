//! Semantic tool routing layer for multi-agent execution.
//!
//! Instead of agents invoking tools directly, all tool calls pass through the
//! `ToolRouter`, which selects the most appropriate tools based on task intent.
//!
//! ## Design
//!
//! ```text
//! agent  →  ToolRouter::route(intent, available_tools)  →  [ToolSpec]
//!                    ↓
//!               role_filter() — strips write tools for read-only roles
//!                    ↓
//!               keyword_score() — ranks by intent-keyword overlap
//!                    ↓
//!               ranked Vec<ToolSpec> (top-K)
//! ```
//!
//! ## Extension points
//!
//! The `route()` method currently uses keyword scoring. When an embedding
//! backend is available, replace `keyword_score()` with a cosine-similarity
//! scorer over tool description embeddings.
//!
//! ## Thread safety
//!
//! `ToolRouter` is stateless after construction — safe to share as
//! `Arc<ToolRouter>` across concurrent agent tasks.

use serde::{Deserialize, Serialize};

use halcon_core::types::AgentRole;

// ── ToolSpec ──────────────────────────────────────────────────────────────────

/// Lightweight description of a tool surfaced by the router.
///
/// Carries the information an agent needs to decide whether to invoke a tool,
/// without pulling in the full `ToolDefinition` from `halcon-tools`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Canonical tool name (must match `ToolDefinition::name`).
    pub name: String,
    /// Human-readable description used for scoring and display.
    pub description: String,
    /// Whether the tool performs write-side operations.
    ///
    /// `true` → creates/modifies files, executes commands, etc.
    /// `false` → read-only (search, inspect, list, …).
    pub is_write: bool,
    /// Relevance score computed by the router (higher is better).
    /// Only meaningful within a single `route()` call.
    pub score: f32,
}

// ── RoutingContext ────────────────────────────────────────────────────────────

/// Input to `ToolRouter::route()`.
pub struct RoutingContext<'a> {
    /// Natural-language description of the task intent.
    pub intent: &'a str,
    /// Functional role of the requesting agent.
    pub agent_role: &'a AgentRole,
    /// Maximum number of tools to return (ranked by score).
    pub top_k: usize,
}

// ── ToolRouter ────────────────────────────────────────────────────────────────

/// Routes tool invocations through a semantic scoring layer.
///
/// Stateless: construct once, share via `Arc<ToolRouter>`.
pub struct ToolRouter {
    /// Write-side tool name prefixes / exact names used for access control.
    ///
    /// Any tool whose name contains one of these substrings is classified as
    /// a write tool (`ToolSpec::is_write = true`).
    write_tool_patterns: Vec<String>,
}

impl ToolRouter {
    /// Create a router with the default write-tool pattern list.
    ///
    /// Recognized write patterns (case-insensitive substring match):
    /// `bash`, `file_write`, `file_edit`, `file_delete`, `git_commit`,
    /// `git_add`, `git_stash`, `patch_apply`, `diff_apply`, `make_tool`,
    /// `docker`, `sql_query`.
    pub fn new() -> Self {
        Self {
            write_tool_patterns: vec![
                "bash".into(),
                "file_write".into(),
                "file_edit".into(),
                "file_delete".into(),
                "git_commit".into(),
                "git_add".into(),
                "git_stash".into(),
                "patch_apply".into(),
                "diff_apply".into(),
                "make_tool".into(),
                "docker".into(),
                "sql_query".into(),
            ],
        }
    }

    /// Create a router with a custom write-tool pattern list.
    pub fn with_write_patterns(patterns: Vec<String>) -> Self {
        Self {
            write_tool_patterns: patterns,
        }
    }

    /// Route a task intent to the most relevant tools.
    ///
    /// # Steps
    /// 1. Mark each tool as write/read based on name patterns.
    /// 2. Filter out write tools if the agent role does not allow writes.
    /// 3. Score each remaining tool by keyword overlap with `intent`.
    /// 4. Return the top-K tools sorted by descending score.
    ///
    /// Tools with a score of `0.0` are included only if fewer than `top_k`
    /// tools scored positively, ensuring the agent always gets *some* tools.
    pub fn route<'a>(
        &self,
        ctx: &RoutingContext<'_>,
        available: &'a [ToolSpec],
    ) -> Vec<&'a ToolSpec> {
        let allow_writes = ctx.agent_role.allows_writes();
        let intent_lower = ctx.intent.to_lowercase();
        let intent_tokens: Vec<&str> = intent_lower.split_whitespace().collect();

        // Score and filter.
        let mut scored: Vec<(&ToolSpec, f32)> = available
            .iter()
            .filter(|t| allow_writes || !t.is_write)
            .map(|t| {
                let score = keyword_score(&t.name, &t.description, &intent_tokens);
                (t, score)
            })
            .collect();

        // Sort descending by score, then alphabetically by name for stability.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.name.cmp(&b.0.name))
        });

        scored
            .into_iter()
            .take(ctx.top_k)
            .map(|(t, _)| t)
            .collect()
    }

    /// Classify a tool name as write or read-only.
    pub fn is_write_tool(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        self.write_tool_patterns
            .iter()
            .any(|pat| lower.contains(pat.as_str()))
    }

    /// Build `ToolSpec` list from raw `(name, description)` pairs.
    ///
    /// Convenience method for converting `ToolDefinition` slices:
    /// ```rust,ignore
    /// let specs = router.build_specs(
    ///     tool_defs.iter().map(|d| (d.name.as_str(), d.description.as_str()))
    /// );
    /// ```
    pub fn build_specs<'a>(
        &self,
        tools: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Vec<ToolSpec> {
        tools
            .into_iter()
            .map(|(name, desc)| ToolSpec {
                is_write: self.is_write_tool(name),
                name: name.to_string(),
                description: desc.to_string(),
                score: 0.0,
            })
            .collect()
    }
}

impl Default for ToolRouter {
    fn default() -> Self {
        Self::new()
    }
}

// ── keyword_score ─────────────────────────────────────────────────────────────

/// Score a tool by keyword overlap with the intent token list.
///
/// Combines name and description signals with a name-match bonus.
///
/// ## Scoring formula
///
/// For each intent token `t`:
/// - +2.0 if `t` appears in the tool *name* (substring match)
/// - +1.0 if `t` appears in the tool *description* (substring match)
///
/// Normalized by `intent_tokens.len()` so short and long intents compare
/// fairly. Returns `0.0` for an empty intent.
fn keyword_score(name: &str, description: &str, intent_tokens: &[&str]) -> f32 {
    if intent_tokens.is_empty() {
        return 0.0;
    }
    let name_l = name.to_lowercase();
    let desc_l = description.to_lowercase();
    let raw: f32 = intent_tokens
        .iter()
        .map(|&tok| {
            let name_hit = if name_l.contains(tok) { 2.0 } else { 0.0 };
            let desc_hit = if desc_l.contains(tok) { 1.0 } else { 0.0 };
            name_hit + desc_hit
        })
        .sum();
    raw / intent_tokens.len() as f32
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn specs(items: &[(&str, &str, bool)]) -> Vec<ToolSpec> {
        items
            .iter()
            .map(|(name, desc, is_write)| ToolSpec {
                name: name.to_string(),
                description: desc.to_string(),
                is_write: *is_write,
                score: 0.0,
            })
            .collect()
    }

    #[test]
    fn routes_by_keyword_match() {
        let router = ToolRouter::new();
        let tools = specs(&[
            ("grep", "Search file content using regex patterns", false),
            ("file_write", "Write content to a file on disk", true),
            ("bash", "Execute a shell command", true),
        ]);

        // Use "grep regex" so the tool name "grep" gets +2 for the "grep" token
        // and "regex" matches the description — making grep clearly the top scorer.
        let ctx = RoutingContext {
            intent: "grep regex pattern",
            agent_role: &AgentRole::Planner,
            top_k: 3,
        };

        let result = router.route(&ctx, &tools);
        // "grep" name matches "grep" token (+2) and desc matches "regex", "pattern".
        assert_eq!(result[0].name, "grep");
    }

    #[test]
    fn read_only_role_excludes_write_tools() {
        let router = ToolRouter::new();
        let tools = specs(&[
            ("grep", "Search content", false),
            ("bash", "Run shell command", true),
            ("file_write", "Write a file", true),
        ]);

        let ctx = RoutingContext {
            intent: "search bash file write",
            agent_role: &AgentRole::Analyzer, // allows_writes() = false
            top_k: 10,
        };

        let result = router.route(&ctx, &tools);
        assert!(result.iter().all(|t| !t.is_write));
        assert_eq!(result.len(), 1); // only grep passes
    }

    #[test]
    fn top_k_is_respected() {
        let router = ToolRouter::new();
        let tools = specs(&[
            ("file_read", "Read file content", false),
            ("grep", "Search content", false),
            ("directory_tree", "List directory tree", false),
            ("glob", "Find files by pattern", false),
        ]);

        let ctx = RoutingContext {
            intent: "file search",
            agent_role: &AgentRole::Analyzer,
            top_k: 2,
        };

        assert_eq!(router.route(&ctx, &tools).len(), 2);
    }

    #[test]
    fn write_tool_classification() {
        let router = ToolRouter::new();
        assert!(router.is_write_tool("bash"));
        assert!(router.is_write_tool("file_write"));
        assert!(router.is_write_tool("git_commit"));
        assert!(router.is_write_tool("patch_apply"));
        assert!(!router.is_write_tool("grep"));
        assert!(!router.is_write_tool("glob"));
        assert!(!router.is_write_tool("file_read"));
    }

    #[test]
    fn build_specs_classifies_correctly() {
        let router = ToolRouter::new();
        let specs = router.build_specs([
            ("bash", "Execute shell commands"),
            ("glob", "Find files by pattern"),
        ]);
        assert_eq!(specs[0].is_write, true);
        assert_eq!(specs[1].is_write, false);
    }

    #[test]
    fn empty_available_returns_empty() {
        let router = ToolRouter::new();
        let ctx = RoutingContext {
            intent: "do something",
            agent_role: &AgentRole::Coder,
            top_k: 5,
        };
        assert!(router.route(&ctx, &[]).is_empty());
    }

    #[test]
    fn empty_intent_returns_tools_with_zero_score() {
        let router = ToolRouter::new();
        let tools = specs(&[("grep", "Search", false), ("glob", "Find", false)]);
        let ctx = RoutingContext {
            intent: "",
            agent_role: &AgentRole::Analyzer,
            top_k: 5,
        };
        let result = router.route(&ctx, &tools);
        // All tools score 0.0 — both returned (order stable by name).
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn coder_role_can_use_write_tools() {
        let router = ToolRouter::new();
        let tools = specs(&[
            ("file_write", "Write files", true),
            ("bash", "Shell", true),
        ]);
        let ctx = RoutingContext {
            intent: "write and execute code",
            agent_role: &AgentRole::Coder,
            top_k: 10,
        };
        // Coder can write — both tools must be present.
        assert_eq!(router.route(&ctx, &tools).len(), 2);
    }
}
