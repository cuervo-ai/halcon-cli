//! Local context gathering for Cenzontle agent integration.
//!
//! Collects project-level context (git state, key files, configuration)
//! from the working directory to send alongside agent tasks. This enables
//! Cenzontle agents to understand the project without direct filesystem access.

use std::path::Path;

use tracing::debug;

/// Maximum bytes per file sent as context (4 KB).
const MAX_FILE_BYTES: usize = 4096;
/// Maximum number of key files to include.
const DEFAULT_MAX_FILES: usize = 10;

/// Gathered local project context.
#[derive(Debug)]
pub struct LocalContext {
    /// Absolute path of the working directory.
    pub cwd: String,
    /// Current git branch name (if in a git repo).
    pub git_branch: Option<String>,
    /// Git status in porcelain format (if in a git repo).
    pub git_status: Option<String>,
    /// Key project files (truncated to MAX_FILE_BYTES each).
    pub key_files: Vec<FileSnippet>,
    /// .halcon/config.toml contents (if present).
    pub halcon_config: Option<String>,
}

/// A file sent as context with content truncated for budget.
#[derive(Debug)]
pub struct FileSnippet {
    /// Relative path from cwd.
    pub path: String,
    /// File content (truncated to MAX_FILE_BYTES).
    pub content: String,
    /// Detected programming language (from extension).
    pub language: Option<String>,
}

/// Key files that provide high-value project context.
const KEY_FILE_NAMES: &[&str] = &[
    "README.md",
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "Makefile",
    "Dockerfile",
    "docker-compose.yml",
    "docker-compose.yaml",
    ".halcon/config.toml",
    "CLAUDE.md",
    "tsconfig.json",
    "build.gradle",
    "pom.xml",
    "requirements.txt",
    "Gemfile",
];

/// Gather local project context from the working directory.
///
/// Runs git commands and reads key files to build a context package
/// suitable for sending to Cenzontle agents.
pub async fn gather_local_context(cwd: &str) -> LocalContext {
    gather_local_context_with_limit(cwd, DEFAULT_MAX_FILES).await
}

/// Gather local context with a custom file limit.
pub async fn gather_local_context_with_limit(cwd: &str, max_files: usize) -> LocalContext {
    let cwd_path = Path::new(cwd);

    // Run git commands in parallel.
    let (git_branch, git_status) = tokio::join!(
        run_git_command(cwd, &["branch", "--show-current"]),
        run_git_command(cwd, &["status", "--porcelain"]),
    );

    // Read key project files.
    let key_files = read_key_files(cwd_path, max_files).await;

    // Read halcon config.
    let halcon_config = read_file_truncated(&cwd_path.join(".halcon/config.toml")).await;

    debug!(
        cwd = %cwd,
        git_branch = ?git_branch,
        key_files = key_files.len(),
        has_halcon_config = halcon_config.is_some(),
        "Gathered local context"
    );

    LocalContext {
        cwd: cwd.to_string(),
        git_branch,
        git_status,
        key_files,
        halcon_config,
    }
}

/// Convert LocalContext to Cenzontle agent_types::TaskContext.
#[cfg(feature = "cenzontle-agents")]
pub fn to_task_context(ctx: &LocalContext) -> halcon_providers::agent_types::TaskContext {
    halcon_providers::agent_types::TaskContext {
        cwd: ctx.cwd.clone(),
        git_branch: ctx.git_branch.clone(),
        git_status: ctx.git_status.clone(),
        files: ctx
            .key_files
            .iter()
            .map(|f| halcon_providers::agent_types::FileContext {
                path: f.path.clone(),
                content: f.content.clone(),
                language: f.language.clone(),
            })
            .collect(),
    }
}

async fn run_git_command(cwd: &str, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

async fn read_key_files(cwd: &Path, max_files: usize) -> Vec<FileSnippet> {
    let mut files = Vec::new();

    for name in KEY_FILE_NAMES {
        if files.len() >= max_files {
            break;
        }

        let path = cwd.join(name);
        if let Some(content) = read_file_truncated(&path).await {
            files.push(FileSnippet {
                path: name.to_string(),
                content,
                language: detect_language(name),
            });
        }
    }

    files
}

async fn read_file_truncated(path: &Path) -> Option<String> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    if content.len() > MAX_FILE_BYTES {
        // Truncate at a valid char boundary.
        let mut end = MAX_FILE_BYTES;
        while !content.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        Some(format!("{}…[truncated]", &content[..end]))
    } else {
        Some(content)
    }
}

fn detect_language(filename: &str) -> Option<String> {
    // Check extensionless filenames first.
    let basename = filename.rsplit('/').next().unwrap_or(filename);
    match basename {
        "Makefile" => return Some("makefile".into()),
        "Dockerfile" => return Some("dockerfile".into()),
        "Gemfile" => return Some("ruby".into()),
        _ => {}
    }

    // Then check by extension.
    let ext = filename.rsplit('.').next()?;
    if ext == filename {
        return None; // no extension
    }
    match ext {
        "rs" => Some("rust".into()),
        "ts" | "tsx" => Some("typescript".into()),
        "js" | "jsx" => Some("javascript".into()),
        "py" => Some("python".into()),
        "go" => Some("go".into()),
        "rb" => Some("ruby".into()),
        "java" => Some("java".into()),
        "toml" => Some("toml".into()),
        "json" => Some("json".into()),
        "yaml" | "yml" => Some("yaml".into()),
        "md" => Some("markdown".into()),
        "xml" => Some("xml".into()),
        "gradle" => Some("groovy".into()),
        "txt" => Some("text".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_language_rust() {
        assert_eq!(detect_language("Cargo.toml"), Some("toml".into()));
    }

    #[test]
    fn detect_language_makefile() {
        assert_eq!(detect_language("Makefile"), Some("makefile".into()));
    }

    #[test]
    fn detect_language_dockerfile() {
        assert_eq!(detect_language("Dockerfile"), Some("dockerfile".into()));
    }

    #[test]
    fn detect_language_unknown() {
        assert_eq!(detect_language("unknownfile"), None);
    }

    #[test]
    fn detect_language_json() {
        assert_eq!(detect_language("package.json"), Some("json".into()));
    }

    #[tokio::test]
    async fn gather_context_nonexistent_dir() {
        let ctx = gather_local_context("/nonexistent/path/12345").await;
        assert!(ctx.git_branch.is_none());
        assert!(ctx.git_status.is_none());
        assert!(ctx.key_files.is_empty());
    }
}
