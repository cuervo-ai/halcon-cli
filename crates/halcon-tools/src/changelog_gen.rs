//! ChangelogGenTool — generate a changelog from git commit history.
//!
//! Reads the git log and produces a structured changelog in Markdown.
//! Supports:
//! - Conventional Commits categorization (feat/fix/docs/chore/refactor/perf/test/ci/build/style)
//! - Date-grouped or tag-grouped output
//! - Filtering by tag range (e.g., v1.0.0..v1.1.0)
//! - Filtering by path
//! - Breaking change detection (BREAKING CHANGE, feat!)
//!
//! Output is Markdown suitable for CHANGELOG.md.

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Command;

pub struct ChangelogGenTool;

impl ChangelogGenTool {
    pub fn new() -> Self {
        Self
    }

    fn run_git(args: &[&str], dir: &str) -> Result<String, String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .map_err(|e| format!("git exec error: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).to_string())
        }
    }

    fn get_tags(dir: &str) -> Vec<String> {
        Self::run_git(&["tag", "--sort=-version:refname"], dir)
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    fn get_commits(range: &str, path_filter: Option<&str>, dir: &str) -> Vec<Commit> {
        let format = "--pretty=format:%H|||%as|||%s|||%b|||%an";
        let mut args = vec!["log", "--no-merges", format, range];
        let path_owned;
        if let Some(p) = path_filter {
            args.push("--");
            path_owned = p.to_string();
            args.push(&path_owned);
        }

        // `git log` with a range arg
        let log_args: Vec<&str> = {
            let mut v = vec!["log", "--no-merges", format];
            if range != "HEAD" && !range.is_empty() {
                v.push(range);
            }
            if let Some(p) = path_filter {
                v.push("--");
                v.push(p);
            }
            v
        };

        let raw = match Self::run_git(&log_args, dir) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        raw.lines()
            .filter(|l| l.contains("|||"))
            .map(|line| {
                let parts: Vec<&str> = line.splitn(5, "|||").collect();
                let hash = parts.first().copied().unwrap_or("").trim().to_string();
                let date = parts.get(1).copied().unwrap_or("").trim().to_string();
                let subject = parts.get(2).copied().unwrap_or("").trim().to_string();
                let body = parts.get(3).copied().unwrap_or("").trim().to_string();
                let author = parts.get(4).copied().unwrap_or("").trim().to_string();
                Self::parse_commit(hash, date, subject, body, author)
            })
            .collect()
    }

    fn parse_commit(
        hash: String,
        date: String,
        subject: String,
        body: String,
        author: String,
    ) -> Commit {
        let breaking = subject.contains("BREAKING CHANGE")
            || subject.contains("!")
            || body.contains("BREAKING CHANGE");

        // Parse conventional commit: type(scope): description
        let (commit_type, scope, description) = if let Some(colon_pos) = subject.find(": ") {
            let prefix = &subject[..colon_pos];
            let desc = subject[colon_pos + 2..].to_string();
            let (t, s) = if let Some(paren) = prefix.find('(') {
                let t = prefix[..paren].trim_end_matches('!').to_lowercase();
                let s = prefix[paren + 1..]
                    .trim_end_matches(')')
                    .trim_end_matches('!')
                    .to_string();
                (t, Some(s))
            } else {
                (prefix.trim_end_matches('!').to_lowercase(), None)
            };
            (t, s, desc)
        } else {
            ("other".to_string(), None, subject.clone())
        };

        Commit {
            hash,
            date,
            commit_type,
            scope,
            description,
            _body: body,
            _author: author,
            breaking,
        }
    }

    fn category_label(commit_type: &str) -> &'static str {
        match commit_type {
            "feat" => "Features",
            "fix" => "Bug Fixes",
            "docs" => "Documentation",
            "style" => "Style",
            "refactor" => "Refactoring",
            "perf" => "Performance",
            "test" => "Tests",
            "build" => "Build",
            "ci" => "CI/CD",
            "chore" => "Chores",
            "revert" => "Reverts",
            _ => "Other Changes",
        }
    }

    fn format_commit_line(c: &Commit, show_hash: bool) -> String {
        let scope_part = c
            .scope
            .as_deref()
            .map(|s| format!("**{s}**: "))
            .unwrap_or_default();
        let breaking = if c.breaking { " ⚠️ BREAKING" } else { "" };
        let hash_part = if show_hash {
            format!(" (`{}`)", &c.hash[..7.min(c.hash.len())])
        } else {
            String::new()
        };
        format!("- {scope_part}{}{breaking}{hash_part}", c.description)
    }

    fn render_section(commits: &[Commit], show_hash: bool) -> String {
        // Group by category
        let mut by_cat: HashMap<&str, Vec<&Commit>> = HashMap::new();
        let mut breaking: Vec<&Commit> = vec![];

        for c in commits {
            if c.breaking {
                breaking.push(c);
            }
            let cat = Self::category_label(&c.commit_type);
            by_cat.entry(cat).or_default().push(c);
        }

        let category_order = [
            "Features",
            "Bug Fixes",
            "Performance",
            "Refactoring",
            "Documentation",
            "Tests",
            "Build",
            "CI/CD",
            "Style",
            "Chores",
            "Reverts",
            "Other Changes",
        ];

        let mut out = String::new();

        if !breaking.is_empty() {
            out.push_str("### ⚠️ Breaking Changes\n\n");
            for c in &breaking {
                out.push_str(&format!("{}\n", Self::format_commit_line(c, show_hash)));
            }
            out.push('\n');
        }

        for cat in &category_order {
            if let Some(items) = by_cat.get(cat) {
                out.push_str(&format!("### {cat}\n\n"));
                for c in items.iter() {
                    out.push_str(&format!("{}\n", Self::format_commit_line(c, show_hash)));
                }
                out.push('\n');
            }
        }

        out
    }

    fn generate_by_tags(
        tags: &[String],
        path_filter: Option<&str>,
        max_versions: usize,
        show_hash: bool,
        dir: &str,
    ) -> String {
        let mut out = String::from("# Changelog\n\n");

        let pairs: Vec<(String, String)> = if tags.len() >= 2 {
            tags.windows(2)
                .map(|w| (w[0].clone(), w[1].clone()))
                .take(max_versions)
                .collect()
        } else if tags.len() == 1 {
            vec![("HEAD".to_string(), tags[0].clone())]
        } else {
            vec![("HEAD".to_string(), String::new())]
        };

        for (newer, older) in &pairs {
            let range = if older.is_empty() {
                newer.clone()
            } else {
                format!("{older}..{newer}")
            };
            let commits = Self::get_commits(&range, path_filter, dir);
            if commits.is_empty() {
                continue;
            }
            let date = commits
                .first()
                .map(|c| c.date.as_str())
                .unwrap_or("unknown");
            out.push_str(&format!("## {newer} ({date})\n\n"));
            out.push_str(&Self::render_section(&commits, show_hash));
        }

        // Unreleased (newer than latest tag) if we have tags
        if let Some(latest) = tags.first() {
            let commits = Self::get_commits(&format!("{latest}..HEAD"), path_filter, dir);
            if !commits.is_empty() {
                out = format!(
                    "# Changelog\n\n## [Unreleased]\n\n{}{out}",
                    Self::render_section(&commits, show_hash)
                );
            }
        }

        out
    }

    fn generate_flat(range: &str, path_filter: Option<&str>, show_hash: bool, dir: &str) -> String {
        let commits = Self::get_commits(range, path_filter, dir);
        if commits.is_empty() {
            return "No commits found in the specified range.\n".to_string();
        }
        let mut out = String::from("# Changelog\n\n");
        out.push_str(&Self::render_section(&commits, show_hash));
        out
    }
}

struct Commit {
    hash: String,
    date: String,
    commit_type: String,
    scope: Option<String>,
    description: String,
    _body: String,
    _author: String,
    breaking: bool,
}

impl Default for ChangelogGenTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ChangelogGenTool {
    fn name(&self) -> &str {
        "changelog_gen"
    }

    fn description(&self) -> &str {
        "Generate a structured Markdown changelog from git commit history. \
         Supports Conventional Commits (feat/fix/docs/refactor/perf/chore/ci/test/build). \
         Can group by git tags (versions) or produce a flat changelog for a range. \
         Detects breaking changes (BREAKING CHANGE footer or '!' prefix). \
         Output is Markdown suitable for CHANGELOG.md."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Working directory (git repo root). Defaults to current directory."
                },
                "mode": {
                    "type": "string",
                    "enum": ["tags", "range", "unreleased"],
                    "description": "Generation mode: 'tags' (group by version tags), 'range' (specific commit range), 'unreleased' (HEAD..latest-tag). Default: 'tags'."
                },
                "range": {
                    "type": "string",
                    "description": "Git range for 'range' mode, e.g. 'v1.0.0..HEAD' or 'main..HEAD'."
                },
                "filter_path": {
                    "type": "string",
                    "description": "Only include commits touching this path (e.g. 'src/api')."
                },
                "max_versions": {
                    "type": "integer",
                    "description": "Maximum number of versions to include (default: 10)."
                },
                "show_hash": {
                    "type": "boolean",
                    "description": "Include commit hash in each entry (default: false)."
                }
            },
            "required": []
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(
        &self,
        input: ToolInput,
    ) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let args = &input.arguments;
        let dir = args["path"].as_str().unwrap_or(&input.working_directory);
        let mode = args["mode"].as_str().unwrap_or("tags");
        let filter_path = args["filter_path"].as_str();
        let max_versions = args["max_versions"].as_u64().unwrap_or(10) as usize;
        let show_hash = args["show_hash"].as_bool().unwrap_or(false);

        // Verify it's a git repo
        if let Err(e) = Self::run_git(&["rev-parse", "--git-dir"], dir) {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("Not a git repository or git not found: {e}"),
                is_error: true,
                metadata: None,
            });
        }

        let content = tokio::task::spawn_blocking({
            let dir = dir.to_string();
            let mode = mode.to_string();
            let filter_path = filter_path.map(|s| s.to_string());
            let range_arg = args["range"].as_str().map(|s| s.to_string());

            move || match mode.as_str() {
                "range" => {
                    let range = range_arg.as_deref().unwrap_or("HEAD~20..HEAD");
                    Self::generate_flat(range, filter_path.as_deref(), show_hash, &dir)
                }
                "unreleased" => {
                    let tags = Self::get_tags(&dir);
                    let range = tags
                        .first()
                        .map(|t| format!("{t}..HEAD"))
                        .unwrap_or_else(|| "HEAD".to_string());
                    Self::generate_flat(&range, filter_path.as_deref(), show_hash, &dir)
                }
                _ => {
                    // "tags" mode
                    let tags = Self::get_tags(&dir);
                    Self::generate_by_tags(
                        &tags,
                        filter_path.as_deref(),
                        max_versions,
                        show_hash,
                        &dir,
                    )
                }
            }
        })
        .await
        .unwrap_or_else(|e| format!("Task error: {e}"));

        let line_count = content.lines().count();
        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "mode": mode,
                "lines": line_count
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let t = ChangelogGenTool::default();
        assert_eq!(t.name(), "changelog_gen");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"].as_array().unwrap().is_empty());
    }

    #[test]
    fn parse_conventional_commit_feat() {
        let c = ChangelogGenTool::parse_commit(
            "abc1234".into(),
            "2026-01-01".into(),
            "feat(auth): add OAuth2 login".into(),
            "".into(),
            "Alice".into(),
        );
        assert_eq!(c.commit_type, "feat");
        assert_eq!(c.scope.as_deref(), Some("auth"));
        assert_eq!(c.description, "add OAuth2 login");
        assert!(!c.breaking);
    }

    #[test]
    fn parse_conventional_commit_fix_no_scope() {
        let c = ChangelogGenTool::parse_commit(
            "def5678".into(),
            "2026-01-02".into(),
            "fix: handle nil pointer in parser".into(),
            "".into(),
            "Bob".into(),
        );
        assert_eq!(c.commit_type, "fix");
        assert!(c.scope.is_none());
        assert_eq!(c.description, "handle nil pointer in parser");
    }

    #[test]
    fn parse_breaking_change_via_body() {
        let c = ChangelogGenTool::parse_commit(
            "ghi".into(),
            "2026-01-03".into(),
            "feat: new API".into(),
            "BREAKING CHANGE: old API removed".into(),
            "Carol".into(),
        );
        assert!(c.breaking);
    }

    #[test]
    fn parse_breaking_change_via_exclamation() {
        let c = ChangelogGenTool::parse_commit(
            "jkl".into(),
            "2026-01-04".into(),
            "feat!: redesign auth flow".into(),
            "".into(),
            "Dan".into(),
        );
        assert!(c.breaking);
    }

    #[test]
    fn parse_non_conventional_commit() {
        let c = ChangelogGenTool::parse_commit(
            "mno".into(),
            "2026-01-05".into(),
            "Update readme".into(),
            "".into(),
            "Eve".into(),
        );
        assert_eq!(c.commit_type, "other");
        assert_eq!(c.description, "Update readme");
    }

    #[test]
    fn category_label_mapping() {
        assert_eq!(ChangelogGenTool::category_label("feat"), "Features");
        assert_eq!(ChangelogGenTool::category_label("fix"), "Bug Fixes");
        assert_eq!(ChangelogGenTool::category_label("docs"), "Documentation");
        assert_eq!(ChangelogGenTool::category_label("ci"), "CI/CD");
        assert_eq!(ChangelogGenTool::category_label("perf"), "Performance");
        assert_eq!(ChangelogGenTool::category_label("unknown"), "Other Changes");
    }

    #[test]
    fn format_commit_line_with_scope() {
        let c = Commit {
            hash: "abcdef1234".into(),
            date: "2026-01-01".into(),
            commit_type: "feat".into(),
            scope: Some("api".into()),
            description: "add new endpoint".into(),
            _body: "".into(),
            _author: "Alice".into(),
            breaking: false,
        };
        let line = ChangelogGenTool::format_commit_line(&c, false);
        assert!(line.contains("api"));
        assert!(line.contains("add new endpoint"));
        assert!(!line.contains("abcdef"));
    }

    #[test]
    fn format_commit_line_with_hash() {
        let c = Commit {
            hash: "abcdef1234".into(),
            date: "2026-01-01".into(),
            commit_type: "fix".into(),
            scope: None,
            description: "resolve crash".into(),
            _body: "".into(),
            _author: "Bob".into(),
            breaking: false,
        };
        let line = ChangelogGenTool::format_commit_line(&c, true);
        assert!(line.contains("abcdef1"));
        assert!(line.contains("resolve crash"));
    }

    #[test]
    fn render_section_groups_by_category() {
        let commits = vec![
            Commit {
                hash: "a1".into(),
                date: "2026-01-01".into(),
                commit_type: "feat".into(),
                scope: None,
                description: "feature one".into(),
                _body: "".into(),
                _author: "X".into(),
                breaking: false,
            },
            Commit {
                hash: "b2".into(),
                date: "2026-01-01".into(),
                commit_type: "fix".into(),
                scope: None,
                description: "bug fix".into(),
                _body: "".into(),
                _author: "Y".into(),
                breaking: false,
            },
        ];
        let rendered = ChangelogGenTool::render_section(&commits, false);
        assert!(rendered.contains("### Features"));
        assert!(rendered.contains("### Bug Fixes"));
        assert!(rendered.contains("feature one"));
        assert!(rendered.contains("bug fix"));
    }

    #[tokio::test]
    async fn execute_on_current_repo() {
        let tool = ChangelogGenTool::new();
        // This test runs on the actual repo — should produce some output
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({ "mode": "range", "range": "HEAD~5..HEAD" }),
                working_directory: "/Users/oscarvalois/Documents/Github/cuervo-cli".into(),
            })
            .await
            .unwrap();
        // Should succeed (git repo exists)
        assert!(!out.is_error, "error: {}", out.content);
        assert!(out.content.contains("Changelog") || out.content.contains("No commits"));
    }
}
