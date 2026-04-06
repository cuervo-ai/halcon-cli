//! `git_branch` tool: create, list, delete, switch, and rename git branches.

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

use super::helpers::{is_git_repo, run_git_command};

#[allow(unused_imports)]
use tracing::instrument;

pub struct GitBranchTool;

impl GitBranchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GitBranchTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Validate a branch name — must be non-empty and not contain dangerous chars.
fn validate_branch_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(HalconError::InvalidInput(
            "git_branch: branch name must not be empty".into(),
        ));
    }
    // Git prohibits these chars/patterns; we enforce a strict subset.
    let bad_chars = [' ', '~', '^', ':', '?', '*', '[', '\\', '\x7f'];
    for ch in bad_chars {
        if name.contains(ch) {
            return Err(HalconError::InvalidInput(format!(
                "git_branch: invalid character '{ch}' in branch name '{name}'"
            )));
        }
    }
    if name.starts_with('-') {
        return Err(HalconError::InvalidInput(format!(
            "git_branch: branch name must not start with '-': '{name}'"
        )));
    }
    if name.contains("..") {
        return Err(HalconError::InvalidInput(format!(
            "git_branch: branch name must not contain '..': '{name}'"
        )));
    }
    Ok(())
}

#[async_trait]
impl Tool for GitBranchTool {
    fn name(&self) -> &str {
        "git_branch"
    }

    fn description(&self) -> &str {
        "Manage git branches: list, create, delete, switch, or rename. \
         Operations: 'list' (all local+remote), 'create' (new branch), \
         'delete' (remove branch), 'switch' (checkout branch), \
         'rename' (rename current branch). \
         Returns current branch, list of branches, and ahead/behind info."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadWrite
    }

    fn requires_confirmation(&self, input: &ToolInput) -> bool {
        // delete requires confirmation
        input
            .arguments
            .get("operation")
            .and_then(|v| v.as_str())
            .map(|op| op == "delete")
            .unwrap_or(false)
    }

    #[tracing::instrument(skip(self), fields(tool = "git_branch"))]
    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let op = input
            .arguments
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("list");

        let working_dir = &input.working_directory;

        if !is_git_repo(working_dir).await {
            return Err(HalconError::InvalidInput(
                "git_branch: not a git repository".into(),
            ));
        }

        match op {
            "list" => self.list(working_dir, &input).await,
            "create" => self.create(working_dir, &input).await,
            "delete" => self.delete(working_dir, &input).await,
            "switch" => self.switch(working_dir, &input).await,
            "rename" => self.rename(working_dir, &input).await,
            other => Err(HalconError::InvalidInput(format!(
                "git_branch: unknown operation '{other}'. Use: list, create, delete, switch, rename"
            ))),
        }
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["list", "create", "delete", "switch", "rename"],
                    "description": "Branch operation to perform (default: list)."
                },
                "name": {
                    "type": "string",
                    "description": "Branch name (required for create, delete, switch, rename)."
                },
                "new_name": {
                    "type": "string",
                    "description": "New branch name (required for rename)."
                },
                "from": {
                    "type": "string",
                    "description": "Starting point for create (branch, tag, or commit SHA). Defaults to HEAD."
                },
                "force": {
                    "type": "boolean",
                    "description": "Force delete even if not merged (default false)."
                },
                "remote": {
                    "type": "boolean",
                    "description": "Include remote branches in list (default true)."
                }
            },
            "required": []
        })
    }
}

impl GitBranchTool {
    async fn list(&self, working_dir: &str, input: &ToolInput) -> Result<ToolOutput> {
        let include_remote = input
            .arguments
            .get("remote")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        // Get all branches with verbose info.
        let args = if include_remote {
            vec!["branch", "--all", "-vv"]
        } else {
            vec!["branch", "-vv"]
        };

        let out = run_git_command(working_dir, &args, None).await?;

        // Parse branch listing into structured data.
        let mut branches = Vec::new();
        let mut current = String::new();

        for line in out.stdout.lines() {
            let is_current = line.starts_with('*');
            let trimmed = line.trim_start_matches(['*', ' ']);

            // Skip HEAD pointer lines (remote tracking HEAD)
            if trimmed.contains("HEAD ->") || trimmed.contains("HEAD detached") {
                if is_current {
                    // Detached HEAD state
                    let sha = trimmed.split_whitespace().next().unwrap_or("").to_string();
                    current = format!("(HEAD detached at {sha})");
                }
                continue;
            }

            let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
            if parts.is_empty() {
                continue;
            }

            let name = parts[0].trim_start_matches("remotes/").to_string();
            let sha = parts
                .get(1)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let desc = parts
                .get(2)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            if is_current {
                current = name.clone();
            }

            // Parse ahead/behind from description like "[origin/main: ahead 2, behind 1]"
            let (ahead, behind) = parse_ahead_behind(&desc);

            branches.push(json!({
                "name": name,
                "sha": sha,
                "current": is_current,
                "remote": name.contains('/'),
                "ahead": ahead,
                "behind": behind,
                "description": desc,
            }));
        }

        let content = format!(
            "Current branch: {}\n{} branches total\n\n{}",
            if current.is_empty() {
                "(unknown)"
            } else {
                &current
            },
            branches.len(),
            out.stdout.trim()
        );

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content,
            is_error: false,
            metadata: Some(json!({
                "current_branch": current,
                "branch_count": branches.len(),
                "branches": branches,
            })),
        })
    }

    async fn create(&self, working_dir: &str, input: &ToolInput) -> Result<ToolOutput> {
        let name = input
            .arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                HalconError::InvalidInput("git_branch create: 'name' is required".into())
            })?;

        validate_branch_name(name)?;

        let from = input
            .arguments
            .get("from")
            .and_then(|v| v.as_str())
            .unwrap_or("HEAD");

        let out = run_git_command(working_dir, &["checkout", "-b", name, from], None).await?;

        if out.exit_code != 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id.clone(),
                content: format!("Failed to create branch '{name}': {}", out.stderr.trim()),
                is_error: true,
                metadata: Some(json!({ "exit_code": out.exit_code, "stderr": out.stderr })),
            });
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content: format!(
                "Created and switched to branch '{name}' from '{from}'.\n{}",
                out.stdout.trim()
            ),
            is_error: false,
            metadata: Some(json!({
                "branch": name,
                "from": from,
                "operation": "create",
            })),
        })
    }

    async fn delete(&self, working_dir: &str, input: &ToolInput) -> Result<ToolOutput> {
        let name = input
            .arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                HalconError::InvalidInput("git_branch delete: 'name' is required".into())
            })?;

        validate_branch_name(name)?;

        let force = input
            .arguments
            .get("force")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let flag = if force { "-D" } else { "-d" };
        let out = run_git_command(working_dir, &["branch", flag, name], None).await?;

        if out.exit_code != 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id.clone(),
                content: format!(
                    "Failed to delete branch '{name}': {}\n\
                     Hint: use force=true to delete unmerged branches.",
                    out.stderr.trim()
                ),
                is_error: true,
                metadata: Some(json!({ "exit_code": out.exit_code, "stderr": out.stderr })),
            });
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content: format!("Deleted branch '{name}'.\n{}", out.stdout.trim()),
            is_error: false,
            metadata: Some(json!({ "branch": name, "operation": "delete", "force": force })),
        })
    }

    async fn switch(&self, working_dir: &str, input: &ToolInput) -> Result<ToolOutput> {
        let name = input
            .arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                HalconError::InvalidInput("git_branch switch: 'name' is required".into())
            })?;

        validate_branch_name(name)?;

        let out = run_git_command(working_dir, &["checkout", name], None).await?;

        if out.exit_code != 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id.clone(),
                content: format!("Failed to switch to branch '{name}': {}", out.stderr.trim()),
                is_error: true,
                metadata: Some(json!({ "exit_code": out.exit_code, "stderr": out.stderr })),
            });
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content: format!(
                "Switched to branch '{name}'.\n{}",
                if out.stdout.trim().is_empty() {
                    out.stderr.trim()
                } else {
                    out.stdout.trim()
                }
            ),
            is_error: false,
            metadata: Some(json!({ "branch": name, "operation": "switch" })),
        })
    }

    async fn rename(&self, working_dir: &str, input: &ToolInput) -> Result<ToolOutput> {
        let new_name = input
            .arguments
            .get("new_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                HalconError::InvalidInput("git_branch rename: 'new_name' is required".into())
            })?;

        validate_branch_name(new_name)?;

        let out = run_git_command(working_dir, &["branch", "-m", new_name], None).await?;

        if out.exit_code != 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id.clone(),
                content: format!("Failed to rename branch: {}", out.stderr.trim()),
                is_error: true,
                metadata: Some(json!({ "exit_code": out.exit_code })),
            });
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id.clone(),
            content: format!("Renamed current branch to '{new_name}'."),
            is_error: false,
            metadata: Some(json!({ "new_name": new_name, "operation": "rename" })),
        })
    }
}

fn parse_ahead_behind(desc: &str) -> (u32, u32) {
    // Format: "[origin/main: ahead 3, behind 1]" or "[gone]" or "[origin/main]"
    let mut ahead = 0u32;
    let mut behind = 0u32;
    if let Some(start) = desc.find('[') {
        if let Some(end) = desc.rfind(']') {
            let inner = &desc[start + 1..end];
            // Skip the remote ref name (everything up to ": ").
            let status_part = inner.find(": ").map(|i| &inner[i + 2..]).unwrap_or(inner);
            for part in status_part.split(',') {
                let p = part.trim();
                if let Some(rest) = p.strip_prefix("ahead ") {
                    ahead = rest.trim().parse().unwrap_or(0);
                } else if let Some(rest) = p.strip_prefix("behind ") {
                    behind = rest.trim().parse().unwrap_or(0);
                }
            }
        }
    }
    (ahead, behind)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_branch_name_rejects_spaces() {
        assert!(validate_branch_name("my branch").is_err());
    }

    #[test]
    fn validate_branch_name_rejects_leading_dash() {
        assert!(validate_branch_name("-bad").is_err());
    }

    #[test]
    fn validate_branch_name_rejects_double_dot() {
        assert!(validate_branch_name("feat..main").is_err());
    }

    #[test]
    fn validate_branch_name_accepts_valid() {
        assert!(validate_branch_name("feature/my-branch").is_ok());
        assert!(validate_branch_name("fix/TICKET-123").is_ok());
        assert!(validate_branch_name("main").is_ok());
        assert!(validate_branch_name("v1.2.3").is_ok());
    }

    #[test]
    fn validate_branch_name_rejects_empty() {
        assert!(validate_branch_name("").is_err());
    }

    #[test]
    fn parse_ahead_behind_extracts_values() {
        assert_eq!(
            parse_ahead_behind("[origin/main: ahead 3, behind 1]"),
            (3, 1)
        );
        assert_eq!(parse_ahead_behind("[origin/main: ahead 5]"), (5, 0));
        assert_eq!(parse_ahead_behind("[origin/main: behind 2]"), (0, 2));
        assert_eq!(parse_ahead_behind(""), (0, 0));
    }

    #[test]
    fn tool_meta() {
        let t = GitBranchTool::new();
        assert_eq!(t.name(), "git_branch");
        assert_eq!(t.permission_level(), PermissionLevel::ReadWrite);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn delete_requires_confirmation() {
        let t = GitBranchTool::new();
        let del_input = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({ "operation": "delete", "name": "feat" }),
            working_directory: "/tmp".into(),
        };
        assert!(t.requires_confirmation(&del_input));
        let list_input = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({ "operation": "list" }),
            working_directory: "/tmp".into(),
        };
        assert!(!t.requires_confirmation(&list_input));
    }

    #[tokio::test]
    async fn list_on_non_repo_returns_error() {
        let t = GitBranchTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "operation": "list" }),
            working_directory: "/tmp".into(),
        };
        let result = t.execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_on_real_repo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();

        // Init repo with a commit so branches work.
        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
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

        let t = GitBranchTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "operation": "create", "name": "test-branch" }),
            working_directory: path.to_string(),
        };
        let out = t.execute(input).await.unwrap();
        assert!(!out.is_error, "create failed: {}", out.content);
        assert!(out.content.contains("test-branch"));
    }

    #[tokio::test]
    async fn invalid_operation_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .await
            .unwrap();

        let t = GitBranchTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "operation": "frobnicate" }),
            working_directory: path.to_string(),
        };
        assert!(t.execute(input).await.is_err());
    }
}
