//! Git helper functions: command execution, repo detection, output parsing.

use std::path::Path;
use std::process::Stdio;

use cuervo_core::error::{CuervoError, Result};

/// Default timeout for git commands (120 seconds, same as tool timeout).
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Maximum output size before truncation (64 KB).
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Result of running a git command.
pub struct GitCommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Run a git command with the given arguments in the specified working directory.
///
/// Uses `std::process::Command::arg()` for each argument (never shell interpolation).
/// Returns structured output with stdout, stderr, and exit code.
pub async fn run_git_command(
    working_dir: &str,
    args: &[&str],
    timeout_secs: Option<u64>,
) -> Result<GitCommandOutput> {
    let timeout = std::time::Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));

    let result = tokio::time::timeout(timeout, async {
        let mut cmd = tokio::process::Command::new("git");
        cmd.current_dir(working_dir)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        // Prevent git from using a pager or interactive prompts.
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd.env("GIT_PAGER", "cat");

        let output = cmd.output().await.map_err(|e| {
            CuervoError::ToolExecutionFailed {
                tool: "git".into(),
                message: format!("failed to execute git: {e}"),
            }
        })?;

        let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Truncate large output.
        if stdout.len() > MAX_OUTPUT_BYTES {
            stdout.truncate(MAX_OUTPUT_BYTES);
            stdout.push_str("\n... (truncated)");
        }

        let exit_code = output.status.code().unwrap_or(-1);

        Ok(GitCommandOutput {
            stdout,
            stderr,
            exit_code,
        })
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => Err(CuervoError::ToolExecutionFailed {
            tool: "git".into(),
            message: format!("git command timed out after {}s", timeout.as_secs()),
        }),
    }
}

/// Check if the working directory is inside a git repository.
pub async fn is_git_repo(working_dir: &str) -> bool {
    let output = run_git_command(working_dir, &["rev-parse", "--is-inside-work-tree"], Some(5)).await;
    matches!(output, Ok(ref o) if o.exit_code == 0 && o.stdout.trim() == "true")
}

/// Parsed git status information.
#[derive(Debug, Default)]
pub struct GitStatusInfo {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: Vec<String>,
    pub modified: Vec<String>,
    pub untracked: Vec<String>,
    pub conflicted: Vec<String>,
}

impl GitStatusInfo {
    pub fn is_clean(&self) -> bool {
        self.staged.is_empty()
            && self.modified.is_empty()
            && self.untracked.is_empty()
            && self.conflicted.is_empty()
    }
}

/// Parse `git status --porcelain=v2 --branch` output into structured data.
///
/// Porcelain v2 format:
/// - `# branch.head <name>` — current branch
/// - `# branch.upstream <name>` — upstream tracking branch
/// - `# branch.ab +N -M` — ahead/behind counts
/// - `1 <XY> ...` — ordinary changed entry (X=staged, Y=unstaged)
/// - `2 <XY> ...` — renamed/copied entry
/// - `u <XY> ...` — unmerged entry
/// - `? <path>` — untracked file
/// - `! <path>` — ignored file (not shown by default)
pub fn parse_porcelain_v2(output: &str) -> GitStatusInfo {
    let mut info = GitStatusInfo::default();

    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            info.branch = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            info.upstream = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            // Format: "+N -M"
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if let Some(ahead_str) = parts.first() {
                info.ahead = ahead_str.trim_start_matches('+').parse().unwrap_or(0);
            }
            if let Some(behind_str) = parts.get(1) {
                info.behind = behind_str.trim_start_matches('-').parse().unwrap_or(0);
            }
        } else if let Some(rest) = line.strip_prefix("1 ") {
            parse_ordinary_entry(rest, &mut info);
        } else if let Some(rest) = line.strip_prefix("2 ") {
            // Renamed/copied — treat like ordinary for categorization.
            parse_ordinary_entry(rest, &mut info);
        } else if line.starts_with("u ") {
            // Unmerged — extract path (last field).
            if let Some(path) = extract_porcelain_path(line) {
                info.conflicted.push(path);
            }
        } else if let Some(path) = line.strip_prefix("? ") {
            info.untracked.push(path.to_string());
        }
    }

    info
}

/// Parse an ordinary (type 1) or rename (type 2) entry's XY status.
fn parse_ordinary_entry(rest: &str, info: &mut GitStatusInfo) {
    // Format: "XY <sub> <mH> <mI> <mW> <hH> <hI> <path>"
    // XY: X = index status, Y = worktree status
    let xy: Vec<char> = rest.chars().take(2).collect();
    if xy.len() < 2 {
        return;
    }

    let path = extract_porcelain_path_from_rest(rest);
    let Some(path) = path else { return };

    let x = xy[0]; // Index (staged) status
    let y = xy[1]; // Worktree status

    // X != '.' means there's a staged change.
    if x != '.' {
        info.staged.push(path.clone());
    }
    // Y != '.' means there's an unstaged worktree change.
    if y != '.' {
        info.modified.push(path);
    }
}

/// Extract the file path from a porcelain v2 line.
fn extract_porcelain_path(line: &str) -> Option<String> {
    // For unmerged entries: "u XY SS SS SS SS SS SS path"
    let parts: Vec<&str> = line.splitn(11, ' ').collect();
    parts.last().map(|s| s.to_string())
}

/// Extract path from the rest of an ordinary/renamed entry (after the type prefix).
fn extract_porcelain_path_from_rest(rest: &str) -> Option<String> {
    // "XY sub mH mI mW hH hI path" (8 fields, path is the 8th)
    // For renamed: "XY sub mH mI mW hH hI Xscore path\torigPath"
    let parts: Vec<&str> = rest.splitn(9, ' ').collect();
    if parts.len() >= 8 {
        // For renames, the path might contain a tab separator.
        let path_field = parts.last().unwrap();
        // Take before tab for rename paths.
        let path = path_field.split('\t').next().unwrap_or(path_field);
        Some(path.to_string())
    } else {
        None
    }
}

/// Parsed diff stat information.
#[derive(Debug, Default)]
pub struct DiffStatInfo {
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
}

/// Parse `git diff --stat` summary line.
///
/// Example: " 3 files changed, 42 insertions(+), 15 deletions(-)"
pub fn parse_diff_stat(output: &str) -> DiffStatInfo {
    let mut info = DiffStatInfo::default();

    // The summary is typically the last non-empty line.
    let summary = output.lines().rev().find(|l| {
        let trimmed = l.trim();
        !trimmed.is_empty()
            && (trimmed.contains("changed")
                || trimmed.contains("insertion")
                || trimmed.contains("deletion"))
    });

    if let Some(line) = summary {
        for part in line.split(',') {
            let trimmed = part.trim();
            if trimmed.contains("file") && trimmed.contains("changed") {
                if let Some(num) = extract_leading_number(trimmed) {
                    info.files_changed = num;
                }
            } else if trimmed.contains("insertion") {
                if let Some(num) = extract_leading_number(trimmed) {
                    info.insertions = num;
                }
            } else if trimmed.contains("deletion") {
                if let Some(num) = extract_leading_number(trimmed) {
                    info.deletions = num;
                }
            }
        }
    }

    info
}

/// Extract the first number from a string.
fn extract_leading_number(s: &str) -> Option<u32> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Validate that a path is relative and doesn't escape the working directory.
pub fn validate_repo_path(path: &str, working_dir: &str) -> Result<()> {
    let p = Path::new(path);
    if p.is_absolute() {
        // Check it's within working_dir.
        if !path.starts_with(working_dir) {
            return Err(CuervoError::InvalidInput(format!(
                "path must be within working directory: {path}"
            )));
        }
    }
    // Check for path traversal.
    for component in p.components() {
        if let std::path::Component::ParentDir = component {
            return Err(CuervoError::InvalidInput(format!(
                "path traversal not allowed: {path}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_porcelain_v2_clean() {
        let output = "# branch.head main\n# branch.upstream origin/main\n# branch.ab +0 -0\n";
        let info = parse_porcelain_v2(output);
        assert_eq!(info.branch.as_deref(), Some("main"));
        assert_eq!(info.upstream.as_deref(), Some("origin/main"));
        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);
        assert!(info.is_clean());
    }

    #[test]
    fn parse_porcelain_v2_staged_and_modified() {
        let output = "\
# branch.head feature
1 M. N... 100644 100644 100644 abc123 def456 src/lib.rs
1 .M N... 100644 100644 100644 abc123 def456 src/main.rs
? new_file.rs
";
        let info = parse_porcelain_v2(output);
        assert_eq!(info.branch.as_deref(), Some("feature"));
        assert_eq!(info.staged, vec!["src/lib.rs"]);
        assert_eq!(info.modified, vec!["src/main.rs"]);
        assert_eq!(info.untracked, vec!["new_file.rs"]);
        assert!(!info.is_clean());
    }

    #[test]
    fn parse_porcelain_v2_ahead_behind() {
        let output = "# branch.head dev\n# branch.ab +3 -1\n";
        let info = parse_porcelain_v2(output);
        assert_eq!(info.ahead, 3);
        assert_eq!(info.behind, 1);
    }

    #[test]
    fn parse_diff_stat_standard() {
        let output = "\
 src/lib.rs | 10 ++++------
 src/main.rs | 5 ++---
 3 files changed, 42 insertions(+), 15 deletions(-)
";
        let stat = parse_diff_stat(output);
        assert_eq!(stat.files_changed, 3);
        assert_eq!(stat.insertions, 42);
        assert_eq!(stat.deletions, 15);
    }

    #[test]
    fn parse_diff_stat_empty() {
        let stat = parse_diff_stat("");
        assert_eq!(stat.files_changed, 0);
        assert_eq!(stat.insertions, 0);
        assert_eq!(stat.deletions, 0);
    }

    #[test]
    fn validate_repo_path_allows_relative() {
        assert!(validate_repo_path("src/lib.rs", "/project").is_ok());
        assert!(validate_repo_path("nested/dir/file.rs", "/project").is_ok());
    }

    #[test]
    fn validate_repo_path_rejects_traversal() {
        assert!(validate_repo_path("../etc/passwd", "/project").is_err());
        assert!(validate_repo_path("src/../../escape", "/project").is_err());
    }

    #[test]
    fn validate_repo_path_rejects_outside_absolute() {
        assert!(validate_repo_path("/etc/passwd", "/project").is_err());
    }

    #[test]
    fn validate_repo_path_allows_inside_absolute() {
        assert!(validate_repo_path("/project/src/lib.rs", "/project").is_ok());
    }

    #[tokio::test]
    async fn is_git_repo_on_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_git_repo(dir.path().to_str().unwrap()).await);
    }

    #[tokio::test]
    async fn is_git_repo_on_initialized_repo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        // Initialize a git repo.
        let _ = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .await;
        assert!(is_git_repo(path).await);
    }

    #[tokio::test]
    async fn run_git_command_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .await;

        let output = run_git_command(path, &["status"], None).await.unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(!output.stdout.is_empty());
    }

    #[tokio::test]
    async fn run_git_command_not_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let output = run_git_command(path, &["status"], None).await.unwrap();
        assert_ne!(output.exit_code, 0);
    }
}
