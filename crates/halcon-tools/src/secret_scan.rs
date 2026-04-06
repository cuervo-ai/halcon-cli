//! SecretScanTool — scan files/directories for leaked secrets and credentials.
//!
//! Uses pattern matching to detect common secret patterns (API keys, tokens, passwords,
//! private keys, connection strings, etc.) without executing any external process.
//! Completely read-only and safe to run on any codebase.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use regex::Regex;
use serde_json::{json, Value};

/// Compiled set of secret detection patterns.
static SECRET_PATTERNS: LazyLock<Vec<SecretPattern>> = LazyLock::new(|| {
    vec![
        SecretPattern::new(
            "aws_access_key",
            r"(?i)(aws_access_key_id|AKIA)[A-Z0-9]{16,20}",
            "AWS Access Key ID",
        ),
        SecretPattern::new(
            "aws_secret_key",
            r"(?i)aws_secret_access_key\s*[=:]\s*[A-Za-z0-9/+=]{40}",
            "AWS Secret Access Key",
        ),
        SecretPattern::new(
            "github_token",
            r"(?i)(ghp_|gho_|ghu_|ghs_|ghr_)[A-Za-z0-9]{36,255}",
            "GitHub Personal Access Token",
        ),
        SecretPattern::new(
            "github_classic",
            r"(?i)github[_\-\s.]*token[_\-\s.]*[=:][_\-\s.]*[A-Za-z0-9]{40}",
            "GitHub Classic Token",
        ),
        SecretPattern::new(
            "anthropic_key",
            r"sk-ant-api[0-9]+-[A-Za-z0-9_\-]{95,}",
            "Anthropic API Key",
        ),
        SecretPattern::new("openai_key", r"sk-[A-Za-z0-9]{48}", "OpenAI API Key"),
        SecretPattern::new(
            "google_api_key",
            r"AIza[0-9A-Za-z\-_]{35}",
            "Google API Key",
        ),
        SecretPattern::new(
            "stripe_key",
            r"(?:sk|pk)_(?:live|test)_[0-9a-zA-Z]{24,}",
            "Stripe API Key",
        ),
        SecretPattern::new(
            "slack_token",
            r"xox[baprs]-[0-9A-Za-z]{10,48}",
            "Slack Token",
        ),
        SecretPattern::new(
            "jwt_token",
            r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
            "JWT Token",
        ),
        SecretPattern::new(
            "private_key",
            r"-----BEGIN (RSA |EC |OPENSSH |DSA )?PRIVATE KEY( BLOCK)?-----",
            "Private Key",
        ),
        SecretPattern::new(
            "password_assign",
            r#"(?i)(password|passwd|pwd)\s*[=:]\s*["'][^"']{8,}["']"#,
            "Hardcoded Password",
        ),
        SecretPattern::new(
            "secret_assign",
            r#"(?i)(secret|api_secret|client_secret)\s*[=:]\s*["'][^"']{8,}["']"#,
            "Hardcoded Secret",
        ),
        SecretPattern::new(
            "db_connection",
            r"(?i)(postgres|mysql|mongodb|redis):\/\/[^:]+:[^@]+@",
            "Database Connection String with Credentials",
        ),
        SecretPattern::new(
            "generic_token",
            r#"(?i)(access_token|auth_token|bearer_token)\s*[=:]\s*["'][A-Za-z0-9._\-]{20,}["']"#,
            "Generic Access Token",
        ),
        SecretPattern::new(
            "heroku_key",
            r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            "UUID-format Key (potential Heroku/service key)",
        ),
        SecretPattern::new(
            "ssh_key_header",
            r"(ssh-rsa|ssh-ed25519|ecdsa-sha2-nistp256) AAAA[A-Za-z0-9+/]{40,}",
            "SSH Public Key",
        ),
        SecretPattern::new(
            "dotenv_secret",
            r#"(?m)^[A-Z_]+_(KEY|SECRET|TOKEN|PASSWORD|PASS|PWD|API)\s*=\s*[^\s#]{8,}"#,
            ".env Secret Variable",
        ),
    ]
});

/// Skip-list for directories and files that should never be scanned.
static SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "vendor",
    "dist",
    "build",
    ".gradle",
    ".cargo",
];

static SKIP_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "ico", "svg", "woff", "woff2", "ttf", "otf", "mp3", "mp4", "wav",
    "pdf", "zip", "tar", "gz", "bz2", "xz", "7z", "exe", "dll", "so", "dylib", "a", "o",
    "lock", // Cargo.lock, package-lock.json etc. contain hashes, not secrets
];

struct SecretPattern {
    id: &'static str,
    regex: Regex,
    description: &'static str,
}

impl SecretPattern {
    fn new(id: &'static str, pattern: &str, description: &'static str) -> Self {
        Self {
            id,
            regex: Regex::new(pattern).expect("invalid secret pattern"),
            description,
        }
    }
}

#[derive(Debug)]
struct Finding {
    file: PathBuf,
    line_number: usize,
    pattern_id: String,
    description: String,
    /// Redacted snippet (never stores the actual secret value)
    context: String,
}

/// Scan files and directories for leaked secrets and credential patterns.
pub struct SecretScanTool;

impl SecretScanTool {
    pub fn new() -> Self {
        Self
    }

    fn scan_content(path: &Path, content: &str) -> Vec<Finding> {
        let mut findings = Vec::new();
        for (line_idx, line) in content.lines().enumerate() {
            let line_number = line_idx + 1;
            for pattern in SECRET_PATTERNS.iter() {
                if pattern.regex.is_match(line) {
                    // Redact the match: show only first 4 chars of any matched value
                    let redacted = redact_line(line, &pattern.regex);
                    findings.push(Finding {
                        file: path.to_path_buf(),
                        line_number,
                        pattern_id: pattern.id.to_string(),
                        description: pattern.description.to_string(),
                        context: redacted,
                    });
                }
            }
        }
        findings
    }

    fn should_skip_path(path: &Path) -> bool {
        // Skip hidden files/dirs (except .env files)
        for component in path.components() {
            let name = component.as_os_str().to_string_lossy();
            if name.starts_with('.')
                && name != ".env"
                && !name.starts_with(".env.")
                && SKIP_DIRS.iter().any(|&s| s == name.as_ref())
            {
                return true;
            }
            if SKIP_DIRS.iter().any(|&s| s == name.as_ref()) {
                return true;
            }
        }
        // Skip binary/asset extensions
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            if SKIP_EXTENSIONS.contains(&ext_str.as_str()) {
                return true;
            }
        }
        false
    }

    fn collect_files(root: &Path, max_files: usize) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if files.len() >= max_files {
                break;
            }
            let rd = match std::fs::read_dir(&dir) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for entry in rd.flatten() {
                let path = entry.path();
                if Self::should_skip_path(&path) {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else if path.is_file() {
                    files.push(path);
                    if files.len() >= max_files {
                        break;
                    }
                }
            }
        }
        files
    }
}

impl Default for SecretScanTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Redact matched secret values in a line — keep leading context, mask the match.
fn redact_line(line: &str, pattern: &Regex) -> String {
    let mut result = line.to_string();
    // Replace each match with a redacted version
    for m in pattern.find_iter(line) {
        let matched = m.as_str();
        let visible = matched.chars().take(8).collect::<String>();
        let redacted = format!("{}***[REDACTED]", visible);
        result = result.replacen(matched, &redacted, 1);
    }
    // Truncate to 120 chars
    if result.len() > 120 {
        result.truncate(120);
        result.push_str("...");
    }
    result
}

#[async_trait]
impl Tool for SecretScanTool {
    fn name(&self) -> &str {
        "secret_scan"
    }

    fn description(&self) -> &str {
        "Scan files or directories for leaked secrets, API keys, tokens, passwords, and credentials. \
         Uses pattern matching to detect common secret formats without executing any external process. \
         Never outputs actual secret values — matches are redacted. Use this before committing code \
         or sharing repositories."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File or directory to scan. Defaults to the working directory."
                },
                "patterns": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of pattern IDs to check (e.g. [\"aws_access_key\", \"github_token\"]). If omitted, all patterns are used."
                },
                "max_files": {
                    "type": "integer",
                    "description": "Maximum number of files to scan (default: 500, max: 5000)."
                },
                "include_low_confidence": {
                    "type": "boolean",
                    "description": "Include lower-confidence patterns like UUID keys and generic tokens (default: false)."
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
        let working_dir = PathBuf::from(&input.working_directory);

        let scan_path = args["path"]
            .as_str()
            .map(|p| {
                let p = Path::new(p);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    working_dir.join(p)
                }
            })
            .unwrap_or_else(|| working_dir.clone());

        let max_files = args["max_files"]
            .as_u64()
            .map(|n| (n as usize).min(5000))
            .unwrap_or(500);

        let include_low_confidence = args["include_low_confidence"].as_bool().unwrap_or(false);

        // Filter pattern IDs if specified
        let pattern_filter: Option<Vec<String>> = args["patterns"].as_array().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });

        // Low-confidence patterns to skip unless explicitly requested
        let low_confidence = ["heroku_key", "ssh_key_header"];

        let files = if scan_path.is_file() {
            vec![scan_path.clone()]
        } else if scan_path.is_dir() {
            Self::collect_files(&scan_path, max_files)
        } else {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("Path not found: {}", scan_path.display()),
                is_error: true,
                metadata: None,
            });
        };

        let files_scanned = files.len();
        let mut all_findings: Vec<Finding> = Vec::new();
        let mut files_with_issues: HashMap<String, usize> = HashMap::new();
        let errors: Vec<String> = Vec::new();

        for file in &files {
            let content = match std::fs::read_to_string(file) {
                Ok(c) => c,
                Err(_) => continue, // Binary or unreadable file
            };
            let mut findings = Self::scan_content(file, &content);

            // Apply pattern filter
            findings.retain(|f| {
                // Skip low-confidence unless requested
                if !include_low_confidence && low_confidence.contains(&f.pattern_id.as_str()) {
                    return false;
                }
                // Apply user pattern filter
                if let Some(ref filter) = pattern_filter {
                    return filter.contains(&f.pattern_id);
                }
                true
            });

            if !findings.is_empty() {
                let path_str = file
                    .strip_prefix(&working_dir)
                    .unwrap_or(file)
                    .to_string_lossy()
                    .to_string();
                *files_with_issues.entry(path_str).or_insert(0) += findings.len();
                all_findings.extend(findings);
            }
        }

        if !errors.is_empty() {
            tracing::warn!(
                count = errors.len(),
                "secret_scan: some files could not be read"
            );
        }

        let total = all_findings.len();
        if total == 0 {
            let summary = format!(
                "✅ No secrets found\n\nScanned {} file(s) — clean.\nPatterns checked: {}",
                files_scanned,
                SECRET_PATTERNS.len()
            );
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: summary,
                is_error: false,
                metadata: Some(json!({ "files_scanned": files_scanned, "findings": 0 })),
            });
        }

        // Build output
        let mut output = format!(
            "⚠️  {} potential secret(s) found in {} file(s) — scanned {} file(s)\n\n",
            total,
            files_with_issues.len(),
            files_scanned
        );

        // Group by file
        let mut by_file: HashMap<String, Vec<&Finding>> = HashMap::new();
        for f in &all_findings {
            let key = f
                .file
                .strip_prefix(&working_dir)
                .unwrap_or(&f.file)
                .to_string_lossy()
                .to_string();
            by_file.entry(key).or_default().push(f);
        }

        let mut sorted_files: Vec<&String> = by_file.keys().collect();
        sorted_files.sort();

        for file_path in sorted_files {
            let findings = &by_file[file_path];
            output.push_str(&format!("📄 {}\n", file_path));
            for finding in findings.iter().take(10) {
                output.push_str(&format!(
                    "  L{:4} [{:20}] {}\n       {}\n",
                    finding.line_number, finding.pattern_id, finding.description, finding.context
                ));
            }
            if findings.len() > 10 {
                output.push_str(&format!("  ... and {} more\n", findings.len() - 10));
            }
            output.push('\n');
        }

        output.push_str("Action: Review findings and rotate any exposed credentials immediately.");

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: output,
            is_error: false,
            metadata: Some(json!({
                "files_scanned": files_scanned,
                "findings": total,
                "files_with_issues": files_with_issues.len()
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::ToolInput;
    use tempfile::TempDir;

    fn make_input(args: Value, dir: &str) -> ToolInput {
        ToolInput {
            tool_use_id: "t1".into(),
            arguments: args,
            working_directory: dir.to_string(),
        }
    }

    #[test]
    fn redact_line_hides_secret() {
        let pattern = Regex::new(r"sk-[A-Za-z0-9]{48}").unwrap();
        let line = "key = sk-aAbBcCdDeEfFgGhHiIjJkKlLmMnNoOpPqQrRsStTuUvVwWxX";
        let redacted = redact_line(line, &pattern);
        assert!(redacted.contains("[REDACTED]"), "should redact");
        assert!(
            !redacted.contains("aAbBcCdDeEfFgGhHiIjJkKlLmMnNoOpPqQrRsStTuUvVwWxX"),
            "full secret should be removed"
        );
    }

    #[test]
    fn scan_content_detects_github_token() {
        let content = "token = ghp_1234567890abcdefghijklmnopqrstuvwxyz";
        let path = Path::new("test.env");
        let findings = SecretScanTool::scan_content(path, content);
        assert!(!findings.is_empty(), "should detect github token");
        assert!(findings.iter().any(|f| f.pattern_id == "github_token"));
    }

    #[test]
    fn scan_content_detects_private_key() {
        let content =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA...\n-----END RSA PRIVATE KEY-----";
        let path = Path::new("id_rsa");
        let findings = SecretScanTool::scan_content(path, content);
        assert!(
            findings.iter().any(|f| f.pattern_id == "private_key"),
            "should detect private key"
        );
    }

    #[test]
    fn scan_content_detects_db_connection() {
        let content = r#"DATABASE_URL=postgres://user:supersecret@localhost/mydb"#;
        let path = Path::new(".env");
        let findings = SecretScanTool::scan_content(path, content);
        assert!(
            findings.iter().any(|f| f.pattern_id == "db_connection"),
            "should detect db conn"
        );
    }

    #[test]
    fn scan_content_clean_returns_empty() {
        let content = "fn main() { println!(\"Hello, world!\"); }";
        let path = Path::new("main.rs");
        let findings = SecretScanTool::scan_content(path, content);
        assert!(findings.is_empty(), "clean code should have no findings");
    }

    #[test]
    fn should_skip_git_dir() {
        assert!(SecretScanTool::should_skip_path(Path::new(
            ".git/COMMIT_EDITMSG"
        )));
        assert!(SecretScanTool::should_skip_path(Path::new(
            "node_modules/pkg/index.js"
        )));
        assert!(!SecretScanTool::should_skip_path(Path::new("src/main.rs")));
    }

    #[test]
    fn should_skip_binary_extensions() {
        assert!(SecretScanTool::should_skip_path(Path::new("image.png")));
        assert!(SecretScanTool::should_skip_path(Path::new(
            "archive.tar.gz"
        )));
        assert!(!SecretScanTool::should_skip_path(Path::new("config.toml")));
    }

    #[tokio::test]
    async fn execute_clean_directory_reports_ok() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

        let tool = SecretScanTool::new();
        let out = tool
            .execute(make_input(json!({}), dir.path().to_str().unwrap()))
            .await
            .unwrap();

        assert!(!out.is_error);
        assert!(out.content.contains("No secrets found") || out.content.contains("clean"));
    }

    #[tokio::test]
    async fn execute_detects_secret_in_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUV\n",
        )
        .unwrap();

        let tool = SecretScanTool::new();
        let out = tool
            .execute(make_input(
                json!({"path": dir.path().to_str().unwrap()}),
                dir.path().to_str().unwrap(),
            ))
            .await
            .unwrap();

        assert!(!out.is_error);
        assert!(
            out.content.contains("REDACTED") || out.content.contains("potential secret"),
            "should report finding: {}",
            out.content
        );
    }

    #[test]
    fn tool_metadata() {
        let t = SecretScanTool::new();
        assert_eq!(t.name(), "secret_scan");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
    }
}
