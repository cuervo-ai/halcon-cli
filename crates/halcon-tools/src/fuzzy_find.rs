//! `fuzzy_find` tool: fuzzy file search by partial path or filename.
//!
//! Walks directory tree, scores each path by subsequence match against query,
//! returns top-N results sorted by score. Skips hidden dirs, node_modules,
//! target, .git. ReadOnly permission.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

/// Directories to skip during traversal.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "__pycache__",
    ".hg",
    ".svn",
    "dist",
    "build",
    ".next",
];

const DEFAULT_MAX_RESULTS: usize = 20;
const ABSOLUTE_MAX_RESULTS: usize = 50;
/// Maximum files to scan before stopping (prevent unbounded traversal).
const MAX_FILES_SCANNED: usize = 50_000;

/// Compute a fuzzy match score for `query` against `path_str`.
///
/// Returns `None` if the query is not a subsequence of the path.
/// Higher score = better match.
fn fuzzy_score(query: &str, path_str: &str) -> Option<f64> {
    let query_lower: Vec<char> = query.to_lowercase().chars().collect();
    let path_lower: Vec<char> = path_str.to_lowercase().chars().collect();

    if query_lower.is_empty() {
        return Some(0.0);
    }

    let mut qi = 0;
    let mut score = 0.0;
    let mut last_match: Option<usize> = None;

    for (pi, &pc) in path_lower.iter().enumerate() {
        if qi < query_lower.len() && pc == query_lower[qi] {
            // Base point for each matched character.
            score += 1.0;

            // Consecutive match bonus.
            if let Some(prev) = last_match {
                if pi == prev + 1 {
                    score += 1.5;
                }
            }

            // Word boundary bonus (after /, \, -, _, or at start).
            if pi == 0
                || matches!(
                    path_lower.get(pi.wrapping_sub(1)),
                    Some('/' | '\\' | '-' | '_' | '.')
                )
            {
                score += 1.0;
            }

            // Filename bonus: matches in the last path component score higher.
            if let Some(last_sep) = path_str.rfind('/') {
                if pi > last_sep {
                    score += 0.5;
                }
            }

            last_match = Some(pi);
            qi += 1;
        }
    }

    if qi == query_lower.len() {
        // Normalize: shorter paths are preferred (less noise).
        let length_penalty = 1.0 / (1.0 + path_lower.len() as f64 * 0.01);
        Some(score * length_penalty)
    } else {
        None
    }
}

/// Recursively walk a directory, collecting file paths.
fn walk_dir(base: &Path, relative_prefix: &Path, files: &mut Vec<PathBuf>, limit: usize) {
    if files.len() >= limit {
        return;
    }

    let entries = match std::fs::read_dir(base.join(relative_prefix)) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut dirs = Vec::new();

    for entry in entries.flatten() {
        if files.len() >= limit {
            return;
        }

        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if file_type.is_dir() {
            // Skip hidden directories and known large directories.
            if name_str.starts_with('.') || SKIP_DIRS.contains(&name_str.as_ref()) {
                continue;
            }
            dirs.push(relative_prefix.join(&*name_str));
        } else if file_type.is_file() {
            // Skip hidden files.
            if name_str.starts_with('.') {
                continue;
            }
            files.push(relative_prefix.join(&*name_str));
        }
    }

    for dir in dirs {
        walk_dir(base, &dir, files, limit);
    }
}

/// Fuzzy file finder.
pub struct FuzzyFindTool;

impl Default for FuzzyFindTool {
    fn default() -> Self {
        Self
    }
}

impl FuzzyFindTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for FuzzyFindTool {
    fn name(&self) -> &str {
        "fuzzy_find"
    }

    fn description(&self) -> &str {
        "Fuzzy search for files by partial path or filename. Returns ranked results."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let query = input.arguments["query"].as_str().ok_or_else(|| {
            HalconError::InvalidInput("fuzzy_find requires 'query' string".into())
        })?;

        let base_path = input.arguments["path"]
            .as_str()
            .unwrap_or(&input.working_directory);

        let max_results = input.arguments["max_results"]
            .as_u64()
            .map(|n| (n as usize).clamp(1, ABSOLUTE_MAX_RESULTS))
            .unwrap_or(DEFAULT_MAX_RESULTS);

        let base = PathBuf::from(base_path);
        if !base.is_dir() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("fuzzy_find error: '{}' is not a directory", base.display()),
                is_error: true,
                metadata: None,
            });
        }

        // Walk directory in a blocking task (filesystem I/O).
        let base_clone = base.clone();
        let query_owned = query.to_string();
        let results = tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();
            walk_dir(&base_clone, Path::new(""), &mut files, MAX_FILES_SCANNED);

            // Score and rank.
            let mut scored: Vec<(PathBuf, f64)> = files
                .into_iter()
                .filter_map(|path| {
                    let path_str = path.to_string_lossy();
                    fuzzy_score(&query_owned, &path_str).map(|s| (path, s))
                })
                .collect();

            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(max_results);
            scored
        })
        .await
        .map_err(|e| HalconError::ToolExecutionFailed {
            tool: "fuzzy_find".into(),
            message: format!("directory walk task failed: {e}"),
        })?;

        let total_matches = results.len();
        let truncated = total_matches == max_results;

        if results.is_empty() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("No files matching '{query}' found."),
                is_error: false,
                metadata: Some(json!({ "match_count": 0, "truncated": false })),
            });
        }

        let mut content = format!("Found {total_matches} match(es) for '{query}':\n");
        for (path, score) in &results {
            content.push_str(&format!("  {:<60} (score: {score:.1})\n", path.display()));
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "match_count": total_matches,
                "truncated": truncated,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Partial path or filename to search for."
                },
                "path": {
                    "type": "string",
                    "description": "Base directory to search in (default: working directory)."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum results to return (1-50, default 20)."
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

    /// Create a temp directory with a known file structure.
    fn setup_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();

        // Create structure:
        // src/main.rs
        // src/lib.rs
        // src/utils/helpers.rs
        // docs/readme.txt
        // .hidden/secret.txt (should be skipped)
        // node_modules/pkg/index.js (should be skipped)
        std::fs::create_dir_all(base.join("src/utils")).unwrap();
        std::fs::create_dir_all(base.join("docs")).unwrap();
        std::fs::create_dir_all(base.join(".hidden")).unwrap();
        std::fs::create_dir_all(base.join("node_modules/pkg")).unwrap();

        std::fs::write(base.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(base.join("src/lib.rs"), "pub mod utils;").unwrap();
        std::fs::write(base.join("src/utils/helpers.rs"), "pub fn help() {}").unwrap();
        std::fs::write(base.join("docs/readme.txt"), "readme").unwrap();
        std::fs::write(base.join(".hidden/secret.txt"), "secret").unwrap();
        std::fs::write(
            base.join("node_modules/pkg/index.js"),
            "module.exports = {}",
        )
        .unwrap();

        dir
    }

    #[tokio::test]
    async fn exact_filename_match() {
        let dir = setup_dir();
        let tool = FuzzyFindTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "main.rs"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("main.rs"));
    }

    #[tokio::test]
    async fn subsequence_match() {
        let dir = setup_dir();
        let tool = FuzzyFindTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "hlp"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        // "hlp" should match "helpers.rs" (h-l-p subsequence).
        assert!(out.content.contains("helpers.rs"));
    }

    #[tokio::test]
    async fn no_match() {
        let dir = setup_dir();
        let tool = FuzzyFindTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "zzzzz"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("No files matching"));
        let meta = out.metadata.unwrap();
        assert_eq!(meta["match_count"], 0);
    }

    #[tokio::test]
    async fn hidden_files_skipped() {
        let dir = setup_dir();
        let tool = FuzzyFindTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "secret"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        // .hidden/secret.txt should not appear.
        assert!(out.content.contains("No files matching"));
    }

    #[tokio::test]
    async fn node_modules_skipped() {
        let dir = setup_dir();
        let tool = FuzzyFindTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "index.js"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        // node_modules/pkg/index.js should not appear.
        assert!(out.content.contains("No files matching"));
    }

    #[tokio::test]
    async fn max_results_respected() {
        let dir = setup_dir();
        let tool = FuzzyFindTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "rs", "max_results": 2}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        let meta = out.metadata.unwrap();
        // We have 3 .rs files but max_results=2.
        assert_eq!(meta["match_count"], 2);
        assert_eq!(meta["truncated"], true);
    }

    #[tokio::test]
    async fn nested_directories() {
        let dir = setup_dir();
        let tool = FuzzyFindTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "helpers"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("helpers.rs"));
        assert!(out.content.contains("utils"));
    }

    #[tokio::test]
    async fn explicit_base_path() {
        let dir = setup_dir();
        let tool = FuzzyFindTool::new();
        let out = tool
            .execute(make_input(json!({
                "query": "main",
                "path": dir.path().to_str().unwrap()
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("main.rs"));
    }

    #[tokio::test]
    async fn score_ordering() {
        let dir = setup_dir();
        let tool = FuzzyFindTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "lib"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        // lib.rs should rank high — exact filename component match.
        let lines: Vec<&str> = out.content.lines().collect();
        // First result (line 1 after header) should be lib.rs.
        assert!(lines[1].contains("lib.rs"));
    }

    #[tokio::test]
    async fn invalid_directory() {
        let tool = FuzzyFindTool::new();
        let out = tool
            .execute(make_input(json!({
                "query": "test",
                "path": "/nonexistent_dir_12345"
            })))
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("not a directory"));
    }

    #[test]
    fn schema_is_valid() {
        let tool = FuzzyFindTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "query"));
    }

    #[test]
    fn permission_is_readonly() {
        let tool = FuzzyFindTool::new();
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
    }

    // Unit test for the scoring function.
    #[test]
    fn fuzzy_score_basics() {
        // Exact subsequence match.
        assert!(fuzzy_score("main", "src/main.rs").is_some());
        // Non-match.
        assert!(fuzzy_score("xyz", "src/main.rs").is_none());
        // Case insensitive.
        assert!(fuzzy_score("MAIN", "src/main.rs").is_some());
        // Empty query matches everything.
        assert!(fuzzy_score("", "anything").is_some());
        // Consecutive chars score higher.
        let consecutive = fuzzy_score("main", "src/main.rs").unwrap();
        let scattered = fuzzy_score("mars", "src/main.rs").unwrap();
        assert!(consecutive > scattered);
    }
}
