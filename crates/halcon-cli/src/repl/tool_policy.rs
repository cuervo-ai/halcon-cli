//! Post-delegation tool retention policy for the coordinator.
//!
//! After sub-agents execute tools, the coordinator must NOT re-call mutating tools
//! (file_write, bash, git_commit, etc.) that were already completed. However, the
//! coordinator MUST retain **read-only** tools so it can verify sub-agent results
//! before synthesising. Stripping ALL delegated tools (the previous behavior) left
//! the coordinator with `tool_count: 0` and forced it into text-only synthesis —
//! causing speculative shell-script generation instead of evidence-based analysis.
//!
//! # Categories
//!
//! | Category    | Examples                              | Post-delegation |
//! |-------------|---------------------------------------|-----------------|
//! | `ReadOnly`  | file_read, directory_tree, grep, glob | **Retained**    |
//! | `Execution` | file_write, bash, git_commit          | **Removed** if delegated |
//! | `Analysis`  | code_metrics, dependency_graph        | **Retained**    |
//! | `External`  | web_fetch, http_request               | **Retained**    |

use std::collections::HashSet;

/// Category that determines whether a tool survives post-delegation stripping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolCategory {
    /// Read-only introspection: never mutates state. Always retained.
    ReadOnly,
    /// Mutating execution: writes files, runs processes, modifies git.
    /// Removed from coordinator after successful delegation.
    Execution,
    /// Analysis tools: compute metrics, scan, validate. Read-only semantics.
    Analysis,
    /// External access: web fetch, HTTP probes. Retained.
    External,
}

/// Tools that the coordinator always keeps after delegation.
/// Any tool NOT in the EXECUTION category is retained.
const EXECUTION_TOOLS: &[&str] = &[
    // File mutation
    "file_write",
    "file_edit",
    "file_delete",
    "diff_apply",
    "patch_apply",
    // Process execution
    "bash",
    "background_start",
    "background_kill",
    // Git mutation
    "git_add",
    "git_commit",
    "git_branch",
    "git_stash",
    // Infrastructure mutation
    "docker",
    "make",
    // Test execution (side-effects: creates files, runs processes)
    "test_run",
    "execute_test",
    // Data mutation
    "sql_query",
    "template_engine",
    "test_data_gen",
    // Archive creation
    "archive",
    "changelog_gen",
];

/// Classify a tool name into its retention category.
///
/// Resolves aliases via `tool_aliases::canonicalize()` before classification,
/// so `write_file` → `file_write` → `Execution` works correctly.
pub(crate) fn classify(tool_name: &str) -> ToolCategory {
    let canonical = super::tool_aliases::canonicalize(tool_name);
    if EXECUTION_TOOLS.contains(&canonical) {
        ToolCategory::Execution
    } else {
        // Default: retain. This covers read-only, analysis, and external tools.
        // New tools added to the registry are retained by default (safe-by-default).
        ToolCategory::ReadOnly
    }
}

/// Given a set of successfully delegated tool names, return the set that should
/// be removed from the coordinator's tool list. Only `Execution` tools are removed.
pub(crate) fn tools_to_remove(delegated_ok_tools: &[String]) -> HashSet<String> {
    delegated_ok_tools
        .iter()
        .filter(|name| classify(name) == ToolCategory::Execution)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_tools_classified_correctly() {
        assert_eq!(classify("file_read"), ToolCategory::ReadOnly);
        assert_eq!(classify("directory_tree"), ToolCategory::ReadOnly);
        assert_eq!(classify("grep"), ToolCategory::ReadOnly);
        assert_eq!(classify("glob"), ToolCategory::ReadOnly);
        assert_eq!(classify("read_multiple_files"), ToolCategory::ReadOnly);
        assert_eq!(classify("file_inspect"), ToolCategory::ReadOnly);
        assert_eq!(classify("fuzzy_find"), ToolCategory::ReadOnly);
        assert_eq!(classify("symbol_search"), ToolCategory::ReadOnly);
        assert_eq!(classify("git_status"), ToolCategory::ReadOnly);
        assert_eq!(classify("git_diff"), ToolCategory::ReadOnly);
        assert_eq!(classify("git_log"), ToolCategory::ReadOnly);
        assert_eq!(classify("git_blame"), ToolCategory::ReadOnly);
    }

    #[test]
    fn execution_tools_classified_correctly() {
        assert_eq!(classify("file_write"), ToolCategory::Execution);
        assert_eq!(classify("file_edit"), ToolCategory::Execution);
        assert_eq!(classify("file_delete"), ToolCategory::Execution);
        assert_eq!(classify("bash"), ToolCategory::Execution);
        assert_eq!(classify("git_commit"), ToolCategory::Execution);
        assert_eq!(classify("test_run"), ToolCategory::Execution);
    }

    #[test]
    fn analysis_tools_retained() {
        assert_eq!(classify("code_metrics"), ToolCategory::ReadOnly);
        assert_eq!(classify("dependency_graph"), ToolCategory::ReadOnly);
        assert_eq!(classify("dep_check"), ToolCategory::ReadOnly);
        assert_eq!(classify("secret_scan"), ToolCategory::ReadOnly);
        assert_eq!(classify("lint_check"), ToolCategory::ReadOnly);
    }

    #[test]
    fn external_tools_retained() {
        assert_eq!(classify("web_fetch"), ToolCategory::ReadOnly);
        assert_eq!(classify("http_request"), ToolCategory::ReadOnly);
        assert_eq!(classify("web_search"), ToolCategory::ReadOnly);
    }

    #[test]
    fn unknown_tool_retained_by_default() {
        assert_eq!(classify("some_future_tool"), ToolCategory::ReadOnly);
    }

    #[test]
    fn tools_to_remove_filters_execution_only() {
        let delegated = vec![
            "file_read".to_string(),
            "file_write".to_string(),
            "grep".to_string(),
            "bash".to_string(),
            "directory_tree".to_string(),
        ];
        let removed = tools_to_remove(&delegated);
        assert!(removed.contains("file_write"));
        assert!(removed.contains("bash"));
        assert!(!removed.contains("file_read"));
        assert!(!removed.contains("grep"));
        assert!(!removed.contains("directory_tree"));
        assert_eq!(removed.len(), 2);
    }

    #[test]
    fn empty_delegation_removes_nothing() {
        let removed = tools_to_remove(&[]);
        assert!(removed.is_empty());
    }

    #[test]
    fn all_read_only_delegation_removes_nothing() {
        let delegated = vec![
            "file_read".to_string(),
            "directory_tree".to_string(),
            "grep".to_string(),
        ];
        let removed = tools_to_remove(&delegated);
        assert!(removed.is_empty());
    }

    // ── Alias resolution tests (FASE 4) ──

    #[test]
    fn aliases_classified_as_execution() {
        // MCP aliases for execution tools must be classified correctly
        assert_eq!(classify("write_file"), ToolCategory::Execution);
        assert_eq!(classify("edit_file"), ToolCategory::Execution);
        assert_eq!(classify("delete_file"), ToolCategory::Execution);
        assert_eq!(classify("run_command"), ToolCategory::Execution);
        assert_eq!(classify("execute_bash"), ToolCategory::Execution);
        assert_eq!(classify("commit_changes"), ToolCategory::Execution);
    }

    #[test]
    fn aliases_classified_as_read_only() {
        // MCP aliases for read-only tools must be retained
        assert_eq!(classify("read_file"), ToolCategory::ReadOnly);
        assert_eq!(classify("list_directory"), ToolCategory::ReadOnly);
        assert_eq!(classify("search_text"), ToolCategory::ReadOnly);
        assert_eq!(classify("find_files"), ToolCategory::ReadOnly);
        assert_eq!(classify("fetch_url"), ToolCategory::ReadOnly);
    }

    #[test]
    fn tools_to_remove_handles_aliases() {
        let delegated = vec![
            "read_file".to_string(),   // alias for file_read → retained
            "write_file".to_string(),  // alias for file_write → removed
            "run_command".to_string(), // alias for bash → removed
            "search_text".to_string(), // alias for grep → retained
        ];
        let removed = tools_to_remove(&delegated);
        assert!(removed.contains("write_file"));
        assert!(removed.contains("run_command"));
        assert!(!removed.contains("read_file"));
        assert!(!removed.contains("search_text"));
        assert_eq!(removed.len(), 2);
    }
}
