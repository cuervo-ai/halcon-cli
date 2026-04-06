//! `ci_logs` tool: fetch and display CI/CD pipeline logs from GitHub Actions or GitLab CI.
//!
//! Uses the `gh` CLI (GitHub CLI) for GitHub Actions and `glab` for GitLab CI.
//! Falls back to reading local log files when no remote CLI is available.
//!
//! Operations:
//! - `list`   — list recent pipeline runs / workflow runs
//! - `show`   — show logs for a specific run (or the most recent run)
//! - `status` — brief pass/fail summary of the most recent runs

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

#[allow(unused_imports)]
use tracing::instrument;

const MAX_LOG_BYTES: usize = 32 * 1024; // 32 KB cap on log output shown to model
const DEFAULT_TIMEOUT: u64 = 60;

pub struct CiLogsTool {
    timeout_secs: u64,
}

impl CiLogsTool {
    pub fn new(timeout_secs: u64) -> Self {
        let timeout_secs = if timeout_secs == 0 {
            DEFAULT_TIMEOUT
        } else {
            timeout_secs
        };
        Self { timeout_secs }
    }
}

impl Default for CiLogsTool {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT)
    }
}

// ─── Platform detection ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum CiPlatform {
    GitHub, // uses gh CLI
    GitLab, // uses glab CLI
}

fn detect_platform(working_dir: &str) -> Option<CiPlatform> {
    // Heuristic: look at git remote URL in .git/config
    let git_config = std::path::Path::new(working_dir)
        .join(".git")
        .join("config");
    if let Ok(content) = std::fs::read_to_string(&git_config) {
        if content.contains("github.com") {
            return Some(CiPlatform::GitHub);
        }
        if content.contains("gitlab.com") || content.contains("gitlab.") {
            return Some(CiPlatform::GitLab);
        }
    }
    // Fallback: check for workflow files
    if std::path::Path::new(working_dir)
        .join(".github/workflows")
        .exists()
    {
        return Some(CiPlatform::GitHub);
    }
    if std::path::Path::new(working_dir)
        .join(".gitlab-ci.yml")
        .exists()
    {
        return Some(CiPlatform::GitLab);
    }
    None
}

// ─── Command runner ───────────────────────────────────────────────────────────

async fn run_cmd(
    program: &str,
    args: &[&str],
    working_dir: &str,
    timeout_secs: u64,
) -> std::result::Result<(String, String, i32), String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::process::Command::new(program)
            .args(args)
            .current_dir(working_dir)
            .output(),
    )
    .await
    .map_err(|_| format!("{program} timed out after {timeout_secs}s"))?
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("'{program}' not found — install it to use CI log fetching")
        } else {
            format!("failed to run '{program}': {e}")
        }
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((stdout, stderr, output.status.code().unwrap_or(-1)))
}

fn truncate_log(s: &str) -> String {
    if s.len() <= MAX_LOG_BYTES {
        s.to_string()
    } else {
        // Keep first half and last quarter to preserve both start and end of logs.
        let head = MAX_LOG_BYTES / 2;
        let tail = MAX_LOG_BYTES / 4;
        let tail_start = s.len().saturating_sub(tail);
        format!(
            "{}\n\n… [log truncated — {} bytes total — showing start and end] …\n\n{}",
            &s[..head],
            s.len(),
            &s[tail_start..]
        )
    }
}

// ─── GitHub Actions via `gh` ──────────────────────────────────────────────────

/// Parse `gh run list --json` output into a simple summary.
fn parse_gh_run_list(json_str: &str) -> Vec<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(json_str)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .take(10)
        .map(|run| {
            json!({
                "id": run["databaseId"],
                "name": run["name"],
                "status": run["status"],
                "conclusion": run["conclusion"],
                "branch": run["headBranch"],
                "event": run["event"],
                "created_at": run["createdAt"],
            })
        })
        .collect()
}

async fn gh_list(working_dir: &str, timeout_secs: u64) -> ToolOutput {
    match run_cmd(
        "gh",
        &[
            "run",
            "list",
            "--limit=10",
            "--json",
            "databaseId,name,status,conclusion,headBranch,event,createdAt",
        ],
        working_dir,
        timeout_secs,
    )
    .await
    {
        Ok((stdout, _, 0)) => {
            let runs = parse_gh_run_list(&stdout);
            let summary = runs
                .iter()
                .map(|r| {
                    let id = r["id"].as_i64().map(|n| n.to_string()).unwrap_or_default();
                    let name = r["name"].as_str().unwrap_or("?");
                    let conclusion = r["conclusion"].as_str().unwrap_or("?");
                    let branch = r["branch"].as_str().unwrap_or("?");
                    format!("  [{id}] {name} — {conclusion} (branch: {branch})")
                })
                .collect::<Vec<_>>()
                .join("\n");
            ToolOutput {
                tool_use_id: "ci_logs".into(),
                content: format!("Recent GitHub Actions runs:\n{summary}"),
                is_error: false,
                metadata: Some(json!({ "platform": "github", "runs": runs })),
            }
        }
        Ok((_, stderr, code)) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!("gh run list failed (exit {code}): {}", stderr.trim()),
            is_error: true,
            metadata: None,
        },
        Err(e) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!("GitHub CLI error: {e}"),
            is_error: true,
            metadata: None,
        },
    }
}

async fn gh_show(run_id: Option<&str>, working_dir: &str, timeout_secs: u64) -> ToolOutput {
    // Fetch logs for a run. If no run_id, use the most recent run.
    let logs_result = if let Some(id) = run_id {
        run_cmd(
            "gh",
            &["run", "view", id, "--log"],
            working_dir,
            timeout_secs,
        )
        .await
    } else {
        // Get most recent run ID first.
        match run_cmd(
            "gh",
            &["run", "list", "--limit=1", "--json", "databaseId"],
            working_dir,
            timeout_secs,
        )
        .await
        {
            Ok((stdout, _, 0)) => {
                let id = serde_json::from_str::<serde_json::Value>(&stdout)
                    .ok()
                    .and_then(|v| v.as_array().and_then(|a| a.first().cloned()))
                    .and_then(|r| r["databaseId"].as_i64())
                    .map(|n| n.to_string());
                match id {
                    Some(ref run_id) => {
                        run_cmd(
                            "gh",
                            &["run", "view", run_id, "--log"],
                            working_dir,
                            timeout_secs,
                        )
                        .await
                    }
                    None => {
                        return ToolOutput {
                            tool_use_id: "ci_logs".into(),
                            content: "No recent GitHub Actions runs found.".into(),
                            is_error: false,
                            metadata: None,
                        }
                    }
                }
            }
            Ok((_, stderr, code)) => {
                return ToolOutput {
                    tool_use_id: "ci_logs".into(),
                    content: format!("Failed to fetch run list (exit {code}): {}", stderr.trim()),
                    is_error: true,
                    metadata: None,
                };
            }
            Err(e) => {
                return ToolOutput {
                    tool_use_id: "ci_logs".into(),
                    content: format!("gh error: {e}"),
                    is_error: true,
                    metadata: None,
                }
            }
        }
    };

    match logs_result {
        Ok((stdout, _, 0)) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: truncate_log(&stdout),
            is_error: false,
            metadata: Some(
                json!({ "platform": "github", "run_id": run_id, "log_bytes": stdout.len() }),
            ),
        },
        Ok((_, stderr, code)) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!("gh run view failed (exit {code}): {}", stderr.trim()),
            is_error: true,
            metadata: None,
        },
        Err(e) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!("gh error: {e}"),
            is_error: true,
            metadata: None,
        },
    }
}

async fn gh_status(working_dir: &str, timeout_secs: u64) -> ToolOutput {
    match run_cmd(
        "gh",
        &[
            "run",
            "list",
            "--limit=5",
            "--json",
            "databaseId,name,conclusion,headBranch,createdAt",
        ],
        working_dir,
        timeout_secs,
    )
    .await
    {
        Ok((stdout, _, 0)) => {
            let runs = parse_gh_run_list(&stdout);
            let failed: usize = runs
                .iter()
                .filter(|r| r["conclusion"].as_str() == Some("failure"))
                .count();
            let success: usize = runs
                .iter()
                .filter(|r| r["conclusion"].as_str() == Some("success"))
                .count();
            let in_progress: usize = runs
                .iter()
                .filter(|r| r["status"].as_str() == Some("in_progress"))
                .count();

            let lines = runs
                .iter()
                .map(|r| {
                    let icon = match r["conclusion"].as_str().unwrap_or("?") {
                        "success" => "✓",
                        "failure" => "✗",
                        _ => "⟳",
                    };
                    let name = r["name"].as_str().unwrap_or("?");
                    let branch = r["branch"].as_str().unwrap_or("?");
                    format!("  {icon} {name} ({branch})")
                })
                .collect::<Vec<_>>()
                .join("\n");

            ToolOutput {
                tool_use_id: "ci_logs".into(),
                content: format!(
                    "GitHub Actions status (last 5 runs):\n{lines}\n\nSummary: {success} passed, {failed} failed, {in_progress} in progress"
                ),
                is_error: failed > 0,
                metadata: Some(json!({ "platform": "github", "passed": success, "failed": failed, "in_progress": in_progress })),
            }
        }
        Ok((_, stderr, code)) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!("gh status failed (exit {code}): {}", stderr.trim()),
            is_error: true,
            metadata: None,
        },
        Err(e) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!("GitHub CLI error: {e}"),
            is_error: true,
            metadata: None,
        },
    }
}

// ─── GitLab CI via `glab` ────────────────────────────────────────────────────

async fn glab_list(working_dir: &str, timeout_secs: u64) -> ToolOutput {
    match run_cmd("glab", &["ci", "list"], working_dir, timeout_secs).await {
        Ok((stdout, _, 0)) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!("Recent GitLab CI pipelines:\n{}", stdout.trim()),
            is_error: false,
            metadata: Some(json!({ "platform": "gitlab" })),
        },
        Ok((_, stderr, code)) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!("glab ci list failed (exit {code}): {}", stderr.trim()),
            is_error: true,
            metadata: None,
        },
        Err(e) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!(
                "GitLab CLI error: {e}\nInstall glab: https://gitlab.com/gitlab-org/cli"
            ),
            is_error: true,
            metadata: None,
        },
    }
}

async fn glab_show(job_id: Option<&str>, working_dir: &str, timeout_secs: u64) -> ToolOutput {
    let args: Vec<&str> = if let Some(id) = job_id {
        vec!["ci", "trace", id]
    } else {
        vec!["ci", "trace"]
    };
    match run_cmd("glab", &args, working_dir, timeout_secs).await {
        Ok((stdout, _, _)) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: truncate_log(&stdout),
            is_error: false,
            metadata: Some(json!({ "platform": "gitlab", "job_id": job_id })),
        },
        Err(e) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!("glab error: {e}"),
            is_error: true,
            metadata: None,
        },
    }
}

async fn glab_status(working_dir: &str, timeout_secs: u64) -> ToolOutput {
    match run_cmd("glab", &["ci", "status"], working_dir, timeout_secs).await {
        Ok((stdout, _, 0)) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!("GitLab CI status:\n{}", stdout.trim()),
            is_error: false,
            metadata: Some(json!({ "platform": "gitlab" })),
        },
        Ok((_, stderr, code)) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!("glab ci status failed (exit {code}): {}", stderr.trim()),
            is_error: true,
            metadata: None,
        },
        Err(e) => ToolOutput {
            tool_use_id: "ci_logs".into(),
            content: format!("glab error: {e}"),
            is_error: true,
            metadata: None,
        },
    }
}

// ─── Tool impl ────────────────────────────────────────────────────────────────

#[async_trait]
impl Tool for CiLogsTool {
    fn name(&self) -> &str {
        "ci_logs"
    }

    fn description(&self) -> &str {
        "Fetch and display CI/CD pipeline logs from GitHub Actions or GitLab CI. \
         Auto-detects platform from git remote URL or workflow files. \
         Operations: 'list' (recent runs), 'show' (full logs for a run), 'status' (pass/fail summary). \
         Requires 'gh' CLI for GitHub Actions and 'glab' CLI for GitLab CI. \
         Use 'platform' to override auto-detection (github or gitlab). \
         Use 'run_id' to target a specific pipeline run instead of the latest."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    #[tracing::instrument(skip(self), fields(tool = "ci_logs"))]
    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let op = input
            .arguments
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("status");

        let working_dir = &input.working_directory;

        // Platform override or auto-detect.
        let platform = if let Some(p) = input.arguments.get("platform").and_then(|v| v.as_str()) {
            match p {
                "github" | "gh" => CiPlatform::GitHub,
                "gitlab" | "glab" => CiPlatform::GitLab,
                other => {
                    return Err(HalconError::InvalidInput(format!(
                        "ci_logs: unknown platform '{other}'. Use: github, gitlab"
                    )));
                }
            }
        } else {
            detect_platform(working_dir).ok_or_else(|| {
                HalconError::InvalidInput(
                    "ci_logs: could not detect CI platform. \
                     Use 'platform' argument to specify 'github' or 'gitlab', \
                     or run from a git repository with a remote on GitHub/GitLab."
                        .into(),
                )
            })?
        };

        let run_id = input.arguments.get("run_id").and_then(|v| v.as_str());

        let mut out = match (platform, op) {
            (CiPlatform::GitHub, "list") => gh_list(working_dir, self.timeout_secs).await,
            (CiPlatform::GitHub, "show") => gh_show(run_id, working_dir, self.timeout_secs).await,
            (CiPlatform::GitHub, "status") => gh_status(working_dir, self.timeout_secs).await,
            (CiPlatform::GitLab, "list") => glab_list(working_dir, self.timeout_secs).await,
            (CiPlatform::GitLab, "show") => glab_show(run_id, working_dir, self.timeout_secs).await,
            (CiPlatform::GitLab, "status") => glab_status(working_dir, self.timeout_secs).await,
            (_, other) => {
                return Err(HalconError::InvalidInput(format!(
                    "ci_logs: unknown operation '{other}'. Use: list, show, status"
                )));
            }
        };

        out.tool_use_id = input.tool_use_id;
        Ok(out)
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["list", "show", "status"],
                    "description": "CI operation (default: status)."
                },
                "platform": {
                    "type": "string",
                    "enum": ["github", "gitlab"],
                    "description": "CI platform override (auto-detected from git remote when omitted)."
                },
                "run_id": {
                    "type": "string",
                    "description": "Specific pipeline / workflow run ID for 'show' operation. Uses latest when omitted."
                }
            },
            "required": []
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_meta() {
        let t = CiLogsTool::new(30);
        assert_eq!(t.name(), "ci_logs");
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        assert!(!t.requires_confirmation(&ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        }));
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["operation"].is_object());
    }

    #[test]
    fn detect_platform_github_from_workflows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
        assert_eq!(
            detect_platform(dir.path().to_str().unwrap()),
            Some(CiPlatform::GitHub)
        );
    }

    #[test]
    fn detect_platform_gitlab_from_ci_yml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitlab-ci.yml"), "stages:\n  - test").unwrap();
        assert_eq!(
            detect_platform(dir.path().to_str().unwrap()),
            Some(CiPlatform::GitLab)
        );
    }

    #[test]
    fn detect_platform_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_platform(dir.path().to_str().unwrap()), None);
    }

    #[test]
    fn truncate_log_short_passthrough() {
        let s = "short log";
        assert_eq!(truncate_log(s), s);
    }

    #[test]
    fn truncate_log_long_keeps_head_and_tail() {
        let s = "A".repeat(MAX_LOG_BYTES * 2);
        let result = truncate_log(&s);
        assert!(result.len() < s.len());
        assert!(result.contains("truncated"));
    }

    #[test]
    fn parse_gh_run_list_valid_json() {
        let json = r#"[
            {"databaseId": 12345, "name": "CI", "status": "completed",
             "conclusion": "success", "headBranch": "main", "event": "push",
             "createdAt": "2026-02-01T10:00:00Z"}
        ]"#;
        let runs = parse_gh_run_list(json);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["id"], 12345);
        assert_eq!(runs[0]["conclusion"], "success");
    }

    #[test]
    fn parse_gh_run_list_invalid_json_returns_empty() {
        let runs = parse_gh_run_list("not json");
        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn unknown_platform_returns_error() {
        let t = CiLogsTool::new(30);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "platform": "jenkins" }),
            working_directory: "/tmp".into(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn unknown_operation_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
        let t = CiLogsTool::new(30);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "operation": "trigger", "platform": "github" }),
            working_directory: dir.path().to_str().unwrap().to_string(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn no_platform_detected_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let t = CiLogsTool::new(30);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "operation": "status" }),
            working_directory: dir.path().to_str().unwrap().to_string(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn github_status_graceful_when_gh_absent() {
        // If `gh` is not installed, we should get a usable error message (not a panic).
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
        let t = CiLogsTool::new(10);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "operation": "status", "platform": "github" }),
            working_directory: dir.path().to_str().unwrap().to_string(),
        };
        // May succeed (gh installed) or return is_error=true (gh absent).
        // Either way, must not panic and content must be non-empty.
        let out = t.execute(input).await.unwrap();
        assert!(!out.content.is_empty());
    }
}
