//! Centralised tool name aliasing — canonical name resolution for the entire pipeline.
//!
//! LLMs and MCP servers use diverse names for the same operation:
//! `file_read` ↔ `read_file`, `file_write` ↔ `write_file`, `bash` ↔ `run_command`, etc.
//!
//! This module provides a single source of truth for alias resolution, used by:
//! - `evidence_pipeline.rs` — recognising content-read tools regardless of name variant
//! - `tool_policy.rs` — classifying execution vs read-only tools
//! - `delegation.rs` — mapping step tool_name to capability
//! - `execution_tracker.rs` — fuzzy matching plan steps to executed tools

/// Known equivalences between native tool names and MCP/external tool variants.
///
/// Each entry is `(canonical_name, [alias1, alias2, ...])`. The canonical name is
/// the halcon-native form. All aliases resolve to the same canonical name.
static TOOL_ALIASES: &[(&str, &[&str])] = &[
    // ── File operations ──
    // file_read and read_multiple_files are the same capability (read file content into context).
    // Merging them into one canonical entry ensures plan-step matching works regardless of
    // whether the model uses the single-file or multi-file variant for a given step.
    ("file_read",       &[
        "read_text_file", "read_file", "readfile", "get_file_contents", "get_file", "read_content",
        // read_multiple_files family — functionally equivalent for plan-step matching
        "read_multiple_files", "read_multiple_files_content", "read_files", "batch_read", "read_files_batch",
    ]),
    ("file_write",      &["write_file", "write_text_file", "create_file", "save_file", "put_file"]),
    ("file_edit",       &["edit_file", "update_file", "modify_file", "patch_file", "replace_in_file"]),
    ("file_delete",     &["delete_file", "remove_file", "rm_file"]),
    // ── Directory listing ──
    ("directory_tree",  &[
        "list_directory", "list_dir", "list_files", "read_dir", "ls", "show_directory",
        // DeepSeek-generated non-standard variants seen in the wild:
        "list_directory_with_sizes", "list_directory_tree", "directory_listing",
        "show_directory_tree", "explore_directory", "browse_directory",
    ]),
    ("glob",            &["find_files", "search_files", "glob_pattern", "list_glob", "match_files"]),
    // ── File inspection (token-budget read, fallback for large files) ──
    ("file_inspect",        &["inspect_file", "read_with_budget", "file_view"]),
    // ── Search ──
    ("grep",            &["search_text", "grep_search", "search_in_file", "search_file", "find_in_files", "semantic_grep", "code_search"]),
    // ── Shell execution ──
    ("bash",            &["run_bash", "execute_bash", "shell", "run_command", "execute_command", "run_shell", "exec"]),
    // ── Git operations ──
    ("git_status",      &["get_git_status", "git_state", "show_git_status"]),
    ("git_diff",        &["get_git_diff", "show_diff", "diff"]),
    ("git_log",         &["get_git_log", "show_log", "git_history", "log"]),
    ("git_commit",      &["do_git_commit", "commit_changes", "commit"]),
    ("git_add",         &["stage_changes", "git_stage", "add_to_staging"]),
    ("git_branch",      &["list_branches", "show_branch", "current_branch"]),
    // ── Web ──
    ("web_search",      &["search_web", "search_internet", "web_query", "internet_search"]),
    ("web_fetch",       &["fetch_url", "get_url", "http_get", "fetch_page", "fetch"]),
    // ── Native search (local index) ──
    ("native_search",   &["local_search", "index_search", "document_search", "search_index", "search_local"]),
    ("native_crawl",    &["crawl_url", "index_url", "crawl_page", "crawl_website"]),
    ("native_index_query", &["index_stats", "search_stats", "index_query"]),
    // ── Tasks ──
    ("task_track",      &["track_task", "create_task", "add_task", "new_task"]),
];

/// Resolve a tool name to its canonical (halcon-native) form.
///
/// Returns the canonical name if the input is an alias, or the input itself if
/// it is already canonical or not found in the alias table.
///
/// ```ignore
/// assert_eq!(canonicalize("read_file"), "file_read");
/// assert_eq!(canonicalize("file_read"), "file_read");
/// assert_eq!(canonicalize("unknown_tool"), "unknown_tool");
/// ```
pub(crate) fn canonicalize(name: &str) -> &str {
    for (canonical, aliases) in TOOL_ALIASES {
        if name == *canonical {
            return canonical;
        }
        if aliases.contains(&name) {
            return canonical;
        }
    }
    name
}

/// Returns true if `a` and `b` name the same tool operation.
///
/// Checks exact equality first, then resolves both to canonical form.
pub(crate) fn are_equivalent(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    canonicalize(a) == canonicalize(b)
}

/// Returns true if `name` (or any alias of it) is a content-reading tool.
///
/// Used by the evidence pipeline to track content-read attempts.
/// Both `file_read` and `read_multiple_files` (and all their aliases) canonicalize
/// to `"file_read"` — so a single check covers the full family.
pub(crate) fn is_content_read_tool(name: &str) -> bool {
    canonicalize(name) == "file_read"
}

/// Returns true if `name` satisfies the "file read" capability —
/// i.e., it reads file content into context regardless of API differences.
///
/// Used by Gate 2 of PostBatchSupervisor to accept `file_inspect` as a
/// valid substitute for `file_read` / `read_multiple_files` when those
/// tools have been circuit-broken. All three tools read file content;
/// `file_inspect` additionally accepts a `token_budget` to limit output size.
pub(crate) fn is_file_read_capable(name: &str) -> bool {
    matches!(canonicalize(name), "file_read" | "file_inspect")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_names_resolve_to_themselves() {
        assert_eq!(canonicalize("file_read"), "file_read");
        assert_eq!(canonicalize("file_write"), "file_write");
        assert_eq!(canonicalize("bash"), "bash");
        assert_eq!(canonicalize("grep"), "grep");
        assert_eq!(canonicalize("git_commit"), "git_commit");
    }

    #[test]
    fn aliases_resolve_to_canonical() {
        assert_eq!(canonicalize("read_file"), "file_read");
        assert_eq!(canonicalize("write_file"), "file_write");
        assert_eq!(canonicalize("edit_file"), "file_edit");
        assert_eq!(canonicalize("delete_file"), "file_delete");
        assert_eq!(canonicalize("run_command"), "bash");
        assert_eq!(canonicalize("search_text"), "grep");
        assert_eq!(canonicalize("semantic_grep"), "grep");
        assert_eq!(canonicalize("code_search"), "grep");
        assert_eq!(canonicalize("list_directory"), "directory_tree");
        assert_eq!(canonicalize("find_files"), "glob");
        assert_eq!(canonicalize("commit_changes"), "git_commit");
        assert_eq!(canonicalize("fetch_url"), "web_fetch");
    }

    #[test]
    fn unknown_tools_pass_through() {
        assert_eq!(canonicalize("some_future_tool"), "some_future_tool");
        assert_eq!(canonicalize("custom_mcp_tool"), "custom_mcp_tool");
    }

    #[test]
    fn equivalence_checks() {
        assert!(are_equivalent("file_read", "read_file"));
        assert!(are_equivalent("read_file", "file_read"));
        assert!(are_equivalent("file_write", "write_file"));
        assert!(are_equivalent("bash", "run_command"));
        assert!(are_equivalent("bash", "bash"));
        // Not equivalent
        assert!(!are_equivalent("file_read", "file_write"));
        assert!(!are_equivalent("bash", "grep"));
    }

    #[test]
    fn file_read_and_read_multiple_files_are_equivalent() {
        // Regression: plan step matching must succeed when the model uses file_read
        // but the plan specified read_multiple_files (or vice versa). Both are the
        // same capability (read file content into context).
        assert!(are_equivalent("file_read", "read_multiple_files"));
        assert!(are_equivalent("read_multiple_files", "file_read"));
        assert!(are_equivalent("read_files", "file_read"));
        assert!(are_equivalent("read_multiple_files_content", "read_file"));
    }

    #[test]
    fn content_read_tool_detection() {
        assert!(is_content_read_tool("file_read"));
        assert!(is_content_read_tool("read_file"));
        assert!(is_content_read_tool("read_multiple_files"));
        assert!(is_content_read_tool("read_multiple_files_content"));
        assert!(is_content_read_tool("read_text_file"));
        // Not read tools
        assert!(!is_content_read_tool("file_write"));
        assert!(!is_content_read_tool("bash"));
        assert!(!is_content_read_tool("grep"));
    }

    #[test]
    fn mcp_filesystem_aliases_resolve() {
        // MCP @modelcontextprotocol/server-filesystem tool names
        assert_eq!(canonicalize("read_file"), "file_read");
        assert_eq!(canonicalize("write_file"), "file_write");
        assert_eq!(canonicalize("search_files"), "glob");
        assert_eq!(canonicalize("list_directory"), "directory_tree");
    }

    #[test]
    fn native_search_aliases_resolve() {
        assert_eq!(canonicalize("local_search"), "native_search");
        assert_eq!(canonicalize("index_search"), "native_search");
        assert_eq!(canonicalize("document_search"), "native_search");
        assert_eq!(canonicalize("search_index"), "native_search");
        assert_eq!(canonicalize("search_local"), "native_search");
        assert_eq!(canonicalize("crawl_url"), "native_crawl");
        assert_eq!(canonicalize("index_url"), "native_crawl");
        assert_eq!(canonicalize("crawl_page"), "native_crawl");
        assert_eq!(canonicalize("index_stats"), "native_index_query");
        assert_eq!(canonicalize("search_stats"), "native_index_query");
        // Canonical names still resolve to themselves
        assert_eq!(canonicalize("native_search"), "native_search");
        assert_eq!(canonicalize("native_crawl"), "native_crawl");
        assert_eq!(canonicalize("native_index_query"), "native_index_query");
    }

    #[test]
    fn all_canonical_names_are_unique() {
        let canonicals: Vec<&str> = TOOL_ALIASES.iter().map(|(c, _)| *c).collect();
        let mut seen = std::collections::HashSet::new();
        for c in &canonicals {
            assert!(seen.insert(c), "duplicate canonical name: {c}");
        }
    }

    #[test]
    fn no_alias_appears_in_multiple_rows() {
        let mut alias_to_canonical = std::collections::HashMap::new();
        for (canonical, aliases) in TOOL_ALIASES {
            for alias in *aliases {
                if let Some(prev) = alias_to_canonical.insert(*alias, *canonical) {
                    panic!("alias '{alias}' appears in both '{prev}' and '{canonical}'");
                }
            }
        }
    }
}
