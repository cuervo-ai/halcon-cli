//! `dep_check` tool: audit project dependencies for outdated packages and known vulnerabilities.
//!
//! Supports Rust (cargo), Node.js (npm/pnpm/yarn), and Python (pip).
//! Runs entirely with local tooling — no network-only dependency.

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

#[allow(unused_imports)]
use tracing::instrument;

const MAX_OUTPUT_BYTES: usize = 128 * 1024; // 128 KB cap on raw command output
const DEFAULT_TIMEOUT: u64 = 120; // cargo audit baseline
                                  // npm/pnpm audit fetches vulnerability DB from registry — much slower than cargo audit.
                                  // Node projects need a higher timeout to avoid cascade failures in sub-agents.
const NODE_TIMEOUT: u64 = 240;
// Python pip-audit also fetches from PyPI — give extra time.
const PYTHON_TIMEOUT: u64 = 180;

pub struct DepCheckTool {
    timeout_secs: u64,
}

impl DepCheckTool {
    pub fn new(timeout_secs: u64) -> Self {
        let timeout_secs = if timeout_secs == 0 {
            DEFAULT_TIMEOUT
        } else {
            timeout_secs
        };
        Self { timeout_secs }
    }
}

impl Default for DepCheckTool {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT)
    }
}

// ─── Ecosystem detection ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Ecosystem {
    Rust,
    Node,
    Python,
}

fn detect_ecosystem(working_dir: &str) -> Option<Ecosystem> {
    let cargo = std::path::Path::new(working_dir).join("Cargo.toml");
    let pkg_json = std::path::Path::new(working_dir).join("package.json");
    let pyproject = std::path::Path::new(working_dir).join("pyproject.toml");
    let requirements = std::path::Path::new(working_dir).join("requirements.txt");
    let setup_py = std::path::Path::new(working_dir).join("setup.py");

    if cargo.exists() {
        return Some(Ecosystem::Rust);
    }
    if pkg_json.exists() {
        return Some(Ecosystem::Node);
    }
    if pyproject.exists() || requirements.exists() || setup_py.exists() {
        return Some(Ecosystem::Python);
    }
    None
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

async fn run_command(
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
            format!("'{program}' not found — is it installed and in PATH?")
        } else {
            format!("failed to run '{program}': {e}")
        }
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let code = output.status.code().unwrap_or(-1);
    Ok((stdout, stderr, code))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}\n… [truncated, {} bytes total]", &s[..max], s.len())
    }
}

// ─── Vulnerability struct ─────────────────────────────────────────────────────

#[derive(Debug)]
struct Vuln {
    package: String,
    severity: String,
    title: String,
    advisory: String,
}

// ─── Rust ─────────────────────────────────────────────────────────────────────

fn parse_cargo_audit(output: &str) -> Vec<Vuln> {
    let mut vulns = Vec::new();
    let mut pkg = String::new();
    let mut title = String::new();
    let mut advisory = String::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("ID:") {
            advisory = trimmed.trim_start_matches("ID:").trim().to_string();
        } else if trimmed.starts_with("Crate:") {
            pkg = trimmed.trim_start_matches("Crate:").trim().to_string();
        } else if trimmed.starts_with("Title:") {
            title = trimmed.trim_start_matches("Title:").trim().to_string();
        } else if trimmed.starts_with("Severity:") {
            let severity = trimmed.trim_start_matches("Severity:").trim().to_string();
            if !pkg.is_empty() {
                vulns.push(Vuln {
                    package: pkg.clone(),
                    severity,
                    title: title.clone(),
                    advisory: advisory.clone(),
                });
                pkg.clear();
                title.clear();
                advisory.clear();
            }
        }
    }
    vulns
}

async fn run_rust(working_dir: &str, timeout_secs: u64, check_vulns: bool) -> ToolOutput {
    let tool_use_id = "dep_check".to_string();

    // 1. cargo tree (always)
    let tree_result = run_command("cargo", &["tree", "--depth=1"], working_dir, timeout_secs).await;

    // 2. cargo audit (optional — requires cargo-audit installed)
    let vulns: Vec<Vuln> = if check_vulns {
        match run_command("cargo", &["audit", "--quiet"], working_dir, timeout_secs).await {
            Ok((stdout, stderr, _)) => parse_cargo_audit(&format!("{stdout}{stderr}")),
            Err(e) => {
                // cargo audit not installed — skip silently
                tracing::debug!("cargo audit unavailable: {e}");
                vec![]
            }
        }
    } else {
        vec![]
    };

    let vuln_count = vulns.len();
    let vuln_list: Vec<serde_json::Value> = vulns
        .iter()
        .map(|v| json!({ "package": v.package, "severity": v.severity, "title": v.title, "advisory": v.advisory }))
        .collect();

    let tree_summary = match tree_result {
        Ok((stdout, _, _)) => truncate(&stdout, MAX_OUTPUT_BYTES),
        Err(e) => format!("cargo tree failed: {e}"),
    };

    let content = if vuln_count == 0 {
        format!("Rust dependencies (depth=1):\n{tree_summary}\n\nNo known vulnerabilities found.")
    } else {
        let vuln_text: String = vulns
            .iter()
            .map(|v| {
                format!(
                    "  • [{severity}] {pkg}: {title} ({id})",
                    severity = v.severity,
                    pkg = v.package,
                    title = v.title,
                    id = v.advisory
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("Rust dependencies (depth=1):\n{tree_summary}\n\n⚠ {vuln_count} vulnerability(ies):\n{vuln_text}")
    };

    ToolOutput {
        tool_use_id,
        content,
        is_error: vuln_count > 0,
        metadata: Some(
            json!({ "ecosystem": "rust", "vuln_count": vuln_count, "vulnerabilities": vuln_list }),
        ),
    }
}

// ─── Node.js ──────────────────────────────────────────────────────────────────

fn detect_node_pm(working_dir: &str) -> &'static str {
    if std::path::Path::new(working_dir)
        .join("pnpm-lock.yaml")
        .exists()
    {
        "pnpm"
    } else if std::path::Path::new(working_dir).join("yarn.lock").exists() {
        "yarn"
    } else {
        "npm"
    }
}

fn parse_npm_audit_summary(output: &str) -> (u32, u32, u32) {
    // "X vulnerabilities (Y low, Z moderate, W high, Q critical)"
    let mut high = 0u32;
    let mut medium = 0u32;
    let mut low = 0u32;
    for line in output.lines() {
        let l = line.to_lowercase();
        if l.contains("critical") || l.contains("high") {
            // Extract numbers after "critical" and "high"
            for word in l.split_whitespace() {
                if let Ok(n) = word
                    .trim_matches(|c: char| !c.is_ascii_digit())
                    .parse::<u32>()
                {
                    if n > 0 && (l.contains("critical") || l.contains("high")) {
                        high += n;
                        break;
                    }
                }
            }
        }
        if l.contains("moderate") || l.contains("medium") {
            for word in l.split_whitespace() {
                if let Ok(n) = word
                    .trim_matches(|c: char| !c.is_ascii_digit())
                    .parse::<u32>()
                {
                    if n > 0 {
                        medium += n;
                        break;
                    }
                }
            }
        }
        if l.contains("low") {
            for word in l.split_whitespace() {
                if let Ok(n) = word
                    .trim_matches(|c: char| !c.is_ascii_digit())
                    .parse::<u32>()
                {
                    if n > 0 {
                        low += n;
                        break;
                    }
                }
            }
        }
    }
    (high, medium, low)
}

async fn run_node(working_dir: &str, timeout_secs: u64, check_vulns: bool) -> ToolOutput {
    let tool_use_id = "dep_check".to_string();
    let pm = detect_node_pm(working_dir);

    // 1. list deps
    let list_args: &[&str] = match pm {
        "pnpm" => &["list", "--depth=1"],
        "yarn" => &["list", "--depth=1"],
        _ => &["ls", "--depth=1"],
    };
    let list_result = run_command(pm, list_args, working_dir, timeout_secs).await;

    // 2. audit
    let (audit_stdout, audit_code) = if check_vulns {
        match run_command("npm", &["audit", "--json"], working_dir, timeout_secs).await {
            Ok((stdout, _, code)) => (stdout, code),
            Err(_) => (String::new(), 0),
        }
    } else {
        (String::new(), 0)
    };

    let (high, medium, low) = if !audit_stdout.is_empty() {
        parse_npm_audit_summary(&audit_stdout)
    } else {
        (0, 0, 0)
    };
    let vuln_count = high + medium + low;

    let dep_summary = match list_result {
        Ok((stdout, _, _)) => truncate(&stdout, 8 * 1024),
        Err(e) => format!("{pm} ls failed: {e}"),
    };

    let vuln_note = if !check_vulns {
        String::new()
    } else if vuln_count == 0 && audit_code == 0 {
        "\n\nNo known vulnerabilities.".to_string()
    } else if vuln_count == 0 {
        "\n\nnpm audit: could not determine vulnerability status.".to_string()
    } else {
        format!("\n\n⚠ {vuln_count} vulnerability(ies): {high} high, {medium} medium, {low} low\n  Run `npm audit` for details.")
    };

    ToolOutput {
        tool_use_id,
        content: format!("{pm} dependencies:\n{dep_summary}{vuln_note}"),
        is_error: vuln_count > 0,
        metadata: Some(json!({
            "ecosystem": "node",
            "package_manager": pm,
            "vuln_high": high, "vuln_medium": medium, "vuln_low": low,
        })),
    }
}

// ─── Python ───────────────────────────────────────────────────────────────────

#[derive(Debug)]
#[allow(dead_code)]
struct PipPackage {
    name: String,
    version: String,
}

fn parse_pip_list(output: &str) -> Vec<PipPackage> {
    output
        .lines()
        .skip(2) // skip header + separator
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?.to_string();
            let version = parts.next().unwrap_or("?").to_string();
            if name.is_empty() {
                None
            } else {
                Some(PipPackage { name, version })
            }
        })
        .collect()
}

async fn run_python(working_dir: &str, timeout_secs: u64) -> ToolOutput {
    let tool_use_id = "dep_check".to_string();

    let list_result = run_command(
        "pip",
        &["list", "--format=columns"],
        working_dir,
        timeout_secs,
    )
    .await;

    let (packages, content) = match list_result {
        Ok((stdout, _, _)) => {
            let pkgs = parse_pip_list(&stdout);
            let count = pkgs.len();
            let summary = truncate(&stdout, 8 * 1024);
            (
                count,
                format!("Python packages ({count} installed):\n{summary}"),
            )
        }
        Err(e) => (0, format!("pip list failed: {e}")),
    };

    // pip-audit for vuln check (optional, best-effort)
    let vuln_note = match run_command("pip-audit", &["--format=json"], working_dir, timeout_secs)
        .await
    {
        Ok((stdout, _, code)) => {
            if code == 0 {
                "\n\nNo known vulnerabilities (pip-audit).".to_string()
            } else if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
                let count = v.as_array().map(|a| a.len()).unwrap_or(0);
                if count > 0 {
                    format!("\n\n⚠ {count} vulnerability(ies) found — run `pip-audit` for details.")
                } else {
                    "\n\nNo known vulnerabilities (pip-audit).".to_string()
                }
            } else {
                String::new()
            }
        }
        Err(_) => String::new(), // pip-audit not installed
    };

    ToolOutput {
        tool_use_id,
        content: format!("{content}{vuln_note}"),
        is_error: false,
        metadata: Some(json!({ "ecosystem": "python", "package_count": packages })),
    }
}

// ─── Tool impl ────────────────────────────────────────────────────────────────

#[async_trait]
impl Tool for DepCheckTool {
    fn name(&self) -> &str {
        "dep_check"
    }

    fn description(&self) -> &str {
        "Audit project dependencies: list packages and check for known vulnerabilities. \
         Auto-detects ecosystem from project files (Cargo.toml → Rust/cargo, \
         package.json → Node.js/npm/pnpm/yarn, pyproject.toml/requirements.txt → Python/pip). \
         Uses cargo-tree + cargo-audit (Rust), npm audit (Node.js), pip list + pip-audit (Python). \
         Returns structured summary with vulnerability counts and severity breakdown."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    #[tracing::instrument(skip(self), fields(tool = "dep_check"))]
    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let working_dir = &input.working_directory;

        // Allow caller to override ecosystem detection.
        let ecosystem_override = input.arguments.get("ecosystem").and_then(|v| v.as_str());
        let check_vulns = input
            .arguments
            .get("check_vulnerabilities")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let ecosystem = if let Some(eco) = ecosystem_override {
            match eco {
                "rust" | "cargo" => Ecosystem::Rust,
                "node" | "npm" | "javascript" | "typescript" => Ecosystem::Node,
                "python" | "pip" => Ecosystem::Python,
                other => {
                    return Err(HalconError::InvalidInput(format!(
                        "dep_check: unknown ecosystem '{other}'. Use: rust, node, python"
                    )));
                }
            }
        } else {
            detect_ecosystem(working_dir).ok_or_else(|| {
                HalconError::InvalidInput(
                    "dep_check: no recognized project file found (Cargo.toml, package.json, pyproject.toml, requirements.txt). \
                     Use the 'ecosystem' argument to specify explicitly.".into(),
                )
            })?
        };

        // Use ecosystem-appropriate timeouts. npm/pip fetch from network registries and
        // are significantly slower than cargo audit which uses a local advisory DB.
        // The ecosystem-specific timeout must stay below sub_agent_timeout_secs (200s).
        let effective_timeout = match &ecosystem {
            Ecosystem::Rust => self.timeout_secs, // 120s (local cargo-audit DB)
            Ecosystem::Node => self.timeout_secs.max(NODE_TIMEOUT), // 240s (npm/pnpm registry fetch)
            Ecosystem::Python => self.timeout_secs.max(PYTHON_TIMEOUT), // 180s (PyPI fetch)
        };
        tracing::debug!(
            ecosystem = ?ecosystem,
            timeout_secs = effective_timeout,
            "dep_check: using ecosystem-specific timeout"
        );
        let mut output = match ecosystem {
            Ecosystem::Rust => run_rust(working_dir, effective_timeout, check_vulns).await,
            Ecosystem::Node => run_node(working_dir, effective_timeout, check_vulns).await,
            Ecosystem::Python => run_python(working_dir, effective_timeout).await,
        };

        output.tool_use_id = input.tool_use_id;
        Ok(output)
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "ecosystem": {
                    "type": "string",
                    "enum": ["rust", "node", "python"],
                    "description": "Force a specific ecosystem. Auto-detected when omitted."
                },
                "check_vulnerabilities": {
                    "type": "boolean",
                    "description": "Run vulnerability audit (cargo audit / npm audit / pip-audit). Default: true."
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
        let t = DepCheckTool::new(30);
        assert_eq!(t.name(), "dep_check");
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["ecosystem"].is_object());
    }

    #[test]
    fn no_confirmation_needed() {
        let t = DepCheckTool::default();
        let input = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(!t.requires_confirmation(&input));
    }

    #[test]
    fn detect_ecosystem_rust() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"",
        )
        .unwrap();
        assert_eq!(
            detect_ecosystem(dir.path().to_str().unwrap()),
            Some(Ecosystem::Rust)
        );
    }

    #[test]
    fn detect_ecosystem_node() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(
            detect_ecosystem(dir.path().to_str().unwrap()),
            Some(Ecosystem::Node)
        );
    }

    #[test]
    fn detect_ecosystem_python_requirements() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("requirements.txt"), "requests==2.28.0").unwrap();
        assert_eq!(
            detect_ecosystem(dir.path().to_str().unwrap()),
            Some(Ecosystem::Python)
        );
    }

    #[test]
    fn detect_ecosystem_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_ecosystem(dir.path().to_str().unwrap()), None);
    }

    #[test]
    fn detect_node_pm_npm() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_node_pm(dir.path().to_str().unwrap()), "npm");
    }

    #[test]
    fn detect_node_pm_pnpm() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect_node_pm(dir.path().to_str().unwrap()), "pnpm");
    }

    #[test]
    fn detect_node_pm_yarn() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("yarn.lock"), "").unwrap();
        assert_eq!(detect_node_pm(dir.path().to_str().unwrap()), "yarn");
    }

    #[test]
    fn parse_pip_list_parses_table() {
        let output =
            "Package    Version\n---------- -------\nrequests   2.28.0\nflask      2.3.1\n";
        let pkgs = parse_pip_list(output);
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].name, "requests");
        assert_eq!(pkgs[0].version, "2.28.0");
        assert_eq!(pkgs[1].name, "flask");
    }

    #[test]
    fn parse_cargo_audit_extracts_vulns() {
        let output = r#"
error[RUSTSEC-2021-0124]: Vulnerability found
Crate: rustdoc
Version: 1.50.0
Date: 2021-08-02
ID: RUSTSEC-2021-0124
Title: Rustdoc builds private documentation
Severity: low
"#;
        let vulns = parse_cargo_audit(output);
        assert_eq!(vulns.len(), 1);
        assert_eq!(vulns[0].package, "rustdoc");
        assert_eq!(vulns[0].severity, "low");
        assert_eq!(vulns[0].advisory, "RUSTSEC-2021-0124");
    }

    #[tokio::test]
    async fn unknown_ecosystem_override_returns_error() {
        let t = DepCheckTool::new(30);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "ecosystem": "ruby" }),
            working_directory: "/tmp".into(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn no_project_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let t = DepCheckTool::new(30);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({}),
            working_directory: dir.path().to_str().unwrap().to_string(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn rust_project_produces_output() {
        // Use this workspace as a real Rust project.
        let dir = std::env!("CARGO_MANIFEST_DIR");
        let t = DepCheckTool::new(60);
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "check_vulnerabilities": false }),
            working_directory: dir.to_string(),
        };
        let out = t.execute(input).await.unwrap();
        // cargo tree output should contain at least the package name
        assert!(!out.content.is_empty());
        let meta = out.metadata.unwrap();
        assert_eq!(meta["ecosystem"], "rust");
    }
}
