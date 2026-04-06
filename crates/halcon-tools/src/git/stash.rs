//! `git_stash` tool: save, list, apply, pop, and drop git stashes.

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

use super::helpers::{is_git_repo, run_git_command};

#[allow(unused_imports)]
use tracing::instrument;

pub struct GitStashTool;

impl GitStashTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GitStashTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Validate stash index is a non-negative integer string or empty.
fn parse_stash_ref(index: Option<&str>) -> Result<String> {
    match index {
        None | Some("") => Ok("stash@{0}".to_string()),
        Some(s) => {
            // Accept "stash@{N}" directly or just a number.
            if s.starts_with("stash@{") {
                Ok(s.to_string())
            } else {
                let n: u32 = s.parse().map_err(|_| {
                    HalconError::InvalidInput(format!(
                        "git_stash: invalid stash index '{s}' — use a number or 'stash@{{N}}'"
                    ))
                })?;
                Ok(format!("stash@{{{n}}}"))
            }
        }
    }
}

#[async_trait]
impl Tool for GitStashTool {
    fn name(&self) -> &str {
        "git_stash"
    }

    fn description(&self) -> &str {
        "Manage git stashes: save uncommitted changes, list stashes, apply or pop a stash, \
         drop a stash, or show stash diff. \
         Operations: 'save' (stash working tree + index), 'list' (show all stashes), \
         'pop' (apply latest + remove), 'apply' (apply without removing), \
         'drop' (delete stash), 'show' (diff of stash contents)."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadWrite
    }

    fn requires_confirmation(&self, input: &ToolInput) -> bool {
        // drop requires confirmation
        input
            .arguments
            .get("operation")
            .and_then(|v| v.as_str())
            .map(|op| op == "drop")
            .unwrap_or(false)
    }

    #[tracing::instrument(skip(self), fields(tool = "git_stash"))]
    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let op = input
            .arguments
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("list");

        let working_dir = &input.working_directory;

        if !is_git_repo(working_dir).await {
            return Err(HalconError::InvalidInput(
                "git_stash: not a git repository".into(),
            ));
        }

        match op {
            "save" => self.save(working_dir, &input).await,
            "list" => self.list(working_dir, &input).await,
            "pop" => self.pop(working_dir, &input).await,
            "apply" => self.apply(working_dir, &input).await,
            "drop" => self.drop(working_dir, &input).await,
            "show" => self.show(working_dir, &input).await,
            other => Err(HalconError::InvalidInput(format!(
                "git_stash: unknown operation '{other}'. Use: save, list, pop, apply, drop, show"
            ))),
        }
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["save", "list", "pop", "apply", "drop", "show"],
                    "description": "Stash operation (default: list)."
                },
                "message": {
                    "type": "string",
                    "description": "Description for the stash (used with save)."
                },
                "index": {
                    "type": "string",
                    "description": "Stash index: a number (0, 1, 2) or 'stash@{N}'. Defaults to most recent (stash@{0})."
                },
                "include_untracked": {
                    "type": "boolean",
                    "description": "Include untracked files in save (default false)."
                }
            },
            "required": []
        })
    }
}

impl GitStashTool {
    async fn save(&self, working_dir: &str, input: &ToolInput) -> Result<ToolOutput> {
        let message = input
            .arguments
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let include_untracked = input
            .arguments
            .get("include_untracked")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut args = vec!["stash", "push"];
        if include_untracked {
            args.push("--include-untracked");
        }
        if !message.is_empty() {
            args.push("-m");
            args.push(message);
        }

        let out = run_git_command(working_dir, &args, None).await?;

        if out.exit_code != 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id.clone(),
                content: format!("Failed to stash: {}", out.stderr.trim()),
                is_error: true,
                metadata: Some(json!({ "exit_code": out.exit_code })),
            });
        }

        let content = if out.stdout.trim() == "No local changes to save" {
            "No local changes to save — working tree is clean.".to_string()
        } else {
            format!("Stashed changes.\n{}", out.stdout.trim())
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content,
            is_error: false,
            metadata: Some(json!({
                "operation": "save",
                "message": message,
                "include_untracked": include_untracked,
            })),
        })
    }

    async fn list(&self, working_dir: &str, input: &ToolInput) -> Result<ToolOutput> {
        let out = run_git_command(working_dir, &["stash", "list"], None).await?;

        if out.stdout.trim().is_empty() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id.clone(),
                content: "No stashes found.".to_string(),
                is_error: false,
                metadata: Some(json!({ "stash_count": 0, "stashes": [] })),
            });
        }

        let stashes: Vec<serde_json::Value> = out
            .stdout
            .lines()
            .enumerate()
            .map(|(i, line)| {
                json!({
                    "index": i,
                    "ref": format!("stash@{{{i}}}"),
                    "description": line.trim(),
                })
            })
            .collect();

        let count = stashes.len();
        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content: format!("{} stash(es):\n{}", count, out.stdout.trim()),
            is_error: false,
            metadata: Some(json!({ "stash_count": count, "stashes": stashes })),
        })
    }

    async fn pop(&self, working_dir: &str, input: &ToolInput) -> Result<ToolOutput> {
        let stash_ref = parse_stash_ref(input.arguments.get("index").and_then(|v| v.as_str()))?;

        let out = run_git_command(working_dir, &["stash", "pop", &stash_ref], None).await?;

        if out.exit_code != 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id.clone(),
                content: format!("Failed to pop stash '{stash_ref}': {}", out.stderr.trim()),
                is_error: true,
                metadata: Some(json!({ "exit_code": out.exit_code })),
            });
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content: format!("Applied and dropped '{stash_ref}'.\n{}", out.stdout.trim()),
            is_error: false,
            metadata: Some(json!({ "stash_ref": stash_ref, "operation": "pop" })),
        })
    }

    async fn apply(&self, working_dir: &str, input: &ToolInput) -> Result<ToolOutput> {
        let stash_ref = parse_stash_ref(input.arguments.get("index").and_then(|v| v.as_str()))?;

        let out = run_git_command(working_dir, &["stash", "apply", &stash_ref], None).await?;

        if out.exit_code != 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id.clone(),
                content: format!("Failed to apply stash '{stash_ref}': {}", out.stderr.trim()),
                is_error: true,
                metadata: Some(json!({ "exit_code": out.exit_code })),
            });
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content: format!(
                "Applied '{stash_ref}' (stash preserved).\n{}",
                out.stdout.trim()
            ),
            is_error: false,
            metadata: Some(json!({ "stash_ref": stash_ref, "operation": "apply" })),
        })
    }

    async fn drop(&self, working_dir: &str, input: &ToolInput) -> Result<ToolOutput> {
        let stash_ref = parse_stash_ref(input.arguments.get("index").and_then(|v| v.as_str()))?;

        let out = run_git_command(working_dir, &["stash", "drop", &stash_ref], None).await?;

        if out.exit_code != 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id.clone(),
                content: format!("Failed to drop stash '{stash_ref}': {}", out.stderr.trim()),
                is_error: true,
                metadata: Some(json!({ "exit_code": out.exit_code })),
            });
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content: format!("Dropped '{stash_ref}'.\n{}", out.stdout.trim()),
            is_error: false,
            metadata: Some(json!({ "stash_ref": stash_ref, "operation": "drop" })),
        })
    }

    async fn show(&self, working_dir: &str, input: &ToolInput) -> Result<ToolOutput> {
        let stash_ref = parse_stash_ref(input.arguments.get("index").and_then(|v| v.as_str()))?;

        let out = run_git_command(working_dir, &["stash", "show", "-p", &stash_ref], None).await?;

        if out.exit_code != 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id.clone(),
                content: format!("Failed to show stash '{stash_ref}': {}", out.stderr.trim()),
                is_error: true,
                metadata: Some(json!({ "exit_code": out.exit_code })),
            });
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content: format!("Diff of '{stash_ref}':\n{}", out.stdout.trim()),
            is_error: false,
            metadata: Some(json!({ "stash_ref": stash_ref, "operation": "show" })),
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stash_ref_defaults_to_zero() {
        assert_eq!(parse_stash_ref(None).unwrap(), "stash@{0}");
        assert_eq!(parse_stash_ref(Some("")).unwrap(), "stash@{0}");
    }

    #[test]
    fn parse_stash_ref_numeric() {
        assert_eq!(parse_stash_ref(Some("2")).unwrap(), "stash@{2}");
        assert_eq!(parse_stash_ref(Some("0")).unwrap(), "stash@{0}");
    }

    #[test]
    fn parse_stash_ref_passthrough() {
        assert_eq!(parse_stash_ref(Some("stash@{5}")).unwrap(), "stash@{5}");
    }

    #[test]
    fn parse_stash_ref_rejects_non_numeric() {
        assert!(parse_stash_ref(Some("bad")).is_err());
        assert!(parse_stash_ref(Some("-1")).is_err());
    }

    #[test]
    fn tool_meta() {
        let t = GitStashTool::new();
        assert_eq!(t.name(), "git_stash");
        assert_eq!(t.permission_level(), PermissionLevel::ReadWrite);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["operation"].is_object());
    }

    #[test]
    fn drop_requires_confirmation() {
        let t = GitStashTool::new();
        let drop_input = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({ "operation": "drop" }),
            working_directory: "/tmp".into(),
        };
        assert!(t.requires_confirmation(&drop_input));
        let list_input = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({ "operation": "list" }),
            working_directory: "/tmp".into(),
        };
        assert!(!t.requires_confirmation(&list_input));
    }

    #[tokio::test]
    async fn list_on_non_repo_returns_error() {
        let t = GitStashTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "operation": "list" }),
            working_directory: "/tmp".into(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn list_on_clean_repo_shows_no_stashes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();

        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.email", "t@t.com"])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        tokio::fs::write(format!("{path}/f.txt"), b"hi")
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(path)
            .output()
            .await
            .unwrap();

        let t = GitStashTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "operation": "list" }),
            working_directory: path.to_string(),
        };
        let out = t.execute(input).await.unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("No stashes"));
        assert_eq!(out.metadata.unwrap()["stash_count"], 0);
    }

    #[tokio::test]
    async fn save_and_list_stash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();

        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.email", "t@t.com"])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        tokio::fs::write(format!("{path}/f.txt"), b"v1")
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(path)
            .output()
            .await
            .unwrap();

        // Dirty the working tree.
        tokio::fs::write(format!("{path}/f.txt"), b"v2")
            .await
            .unwrap();

        let t = GitStashTool::new();

        // Save stash.
        let save_in = ToolInput {
            tool_use_id: "s".into(),
            arguments: json!({ "operation": "save", "message": "my-stash" }),
            working_directory: path.to_string(),
        };
        let save_out = t.execute(save_in).await.unwrap();
        assert!(!save_out.is_error, "save failed: {}", save_out.content);

        // List should now have 1 stash.
        let list_in = ToolInput {
            tool_use_id: "l".into(),
            arguments: json!({ "operation": "list" }),
            working_directory: path.to_string(),
        };
        let list_out = t.execute(list_in).await.unwrap();
        assert!(!list_out.is_error);
        let meta = list_out.metadata.unwrap();
        assert_eq!(meta["stash_count"], 1);
    }
}
