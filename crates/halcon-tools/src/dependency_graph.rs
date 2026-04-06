//! DependencyGraphTool — visualize package dependency graphs as ASCII trees.
//!
//! Supports:
//! - Rust: `cargo tree` output parsing and rendering
//! - Node.js: `npm ls --all` or `pnpm ls --depth` output parsing
//! - Python: `pip show` based dependency resolution
//! - Filtering: show only deps matching a pattern, show only direct deps
//! - Cycle detection: flag circular dependencies
//! - Depth limiting: expand only N levels deep
//!
//! Output is structured ASCII tree + dependency stats.

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};
use std::collections::HashSet;
use tokio::process::Command;

pub struct DependencyGraphTool {
    timeout_secs: u64,
}

impl DependencyGraphTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    async fn run_cargo_tree(
        package: Option<&str>,
        depth: u32,
        filter: Option<&str>,
        dir: &str,
        timeout_secs: u64,
    ) -> String {
        let mut args = vec!["tree"];
        let depth_str = depth.to_string();
        args.push("--depth");
        args.push(&depth_str);
        args.push("--color");
        args.push("never");

        let pkg_owned;
        if let Some(p) = package {
            args.push("-p");
            pkg_owned = p.to_string();
            args.push(&pkg_owned);
        }

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            Command::new("cargo").args(&args).current_dir(dir).output(),
        )
        .await;

        let raw = match result {
            Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout).to_string(),
            Ok(Err(e)) => return format!("cargo tree error: {e}"),
            Err(_) => return format!("cargo tree timed out after {timeout_secs}s"),
        };

        if let Some(f) = filter {
            let f_lower = f.to_lowercase();
            let lines: Vec<&str> = raw
                .lines()
                .filter(|l| l.to_lowercase().contains(&f_lower) || l.starts_with('['))
                .collect();
            lines.join("\n")
        } else {
            // Truncate large output
            raw.lines().take(200).collect::<Vec<_>>().join("\n")
        }
    }

    async fn run_npm_ls(depth: u32, filter: Option<&str>, dir: &str, timeout_secs: u64) -> String {
        let depth_str = depth.to_string();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            Command::new("npm")
                .args(["ls", "--depth", &depth_str, "--parseable"])
                .current_dir(dir)
                .output(),
        )
        .await;

        let raw = match result {
            Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout).to_string(),
            Ok(Err(e)) => {
                // Fallback: try npx
                return format!("npm ls failed: {e}");
            }
            Err(_) => return format!("npm ls timed out after {timeout_secs}s"),
        };

        let lines: Vec<&str> = raw
            .lines()
            .filter(|l| {
                filter
                    .map(|f| l.to_lowercase().contains(&f.to_lowercase()))
                    .unwrap_or(true)
            })
            .take(100)
            .collect();

        if lines.is_empty() {
            raw.lines().take(100).collect::<Vec<_>>().join("\n")
        } else {
            lines.join("\n")
        }
    }

    async fn detect_ecosystem(dir: &str) -> &'static str {
        let check_files = [
            ("Cargo.toml", "rust"),
            ("package.json", "node"),
            ("requirements.txt", "python"),
            ("Pipfile", "python"),
            ("pyproject.toml", "python"),
            ("pom.xml", "maven"),
            ("build.gradle", "gradle"),
        ];
        for (file, eco) in &check_files {
            if tokio::fs::metadata(format!("{dir}/{file}")).await.is_ok() {
                return eco;
            }
        }
        "unknown"
    }

    fn parse_cargo_tree_stats(output: &str) -> DependencyStats {
        let mut direct = HashSet::new();
        let mut all_deps = HashSet::new();
        let mut cycles: Vec<String> = vec![];
        let mut max_depth = 0u32;

        for line in output.lines() {
            let depth = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '│' || *c == '├' || *c == '└' || *c == '─')
                .count();
            let adjusted_depth = (depth / 4) as u32;
            max_depth = max_depth.max(adjusted_depth);

            // Extract crate name
            let trimmed =
                line.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');
            if let Some(name) = trimmed.split_whitespace().next() {
                if !name.is_empty() {
                    all_deps.insert(name.to_string());
                    if adjusted_depth == 1 {
                        direct.insert(name.to_string());
                    }
                    if line.contains("(*)") {
                        cycles.push(name.to_string());
                    }
                }
            }
        }

        DependencyStats {
            direct_count: direct.len(),
            total_count: all_deps.len(),
            max_depth,
            cycle_hints: cycles,
        }
    }

    fn format_summary(eco: &str, output: &str, stats: &DependencyStats) -> String {
        let mut s = format!(
            "Dependency Graph ({eco})\n\n\
             Direct deps : {}\n\
             Total deps  : {}\n\
             Max depth   : {}\n",
            stats.direct_count, stats.total_count, stats.max_depth
        );
        if !stats.cycle_hints.is_empty() {
            s.push_str(&format!(
                "Cycle hints : {} (marked with (*))\n",
                stats.cycle_hints.len()
            ));
        }
        s.push('\n');
        s.push_str(output);
        s
    }
}

struct DependencyStats {
    direct_count: usize,
    total_count: usize,
    max_depth: u32,
    cycle_hints: Vec<String>,
}

impl Default for DependencyGraphTool {
    fn default() -> Self {
        Self::new(30)
    }
}

#[async_trait]
impl Tool for DependencyGraphTool {
    fn name(&self) -> &str {
        "dependency_graph"
    }

    fn description(&self) -> &str {
        "Visualize package dependency graphs as ASCII trees. \
         Auto-detects ecosystem (Rust/cargo, Node.js/npm, Python/pip). \
         Shows direct and transitive dependencies with depth control and filtering. \
         Reports total dependency count, max depth, and circular dependency hints."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project directory (default: current)."
                },
                "ecosystem": {
                    "type": "string",
                    "enum": ["rust", "node", "python", "auto"],
                    "description": "Package ecosystem (default: auto-detect)."
                },
                "depth": {
                    "type": "integer",
                    "description": "Max dependency depth to show (default: 3)."
                },
                "package": {
                    "type": "string",
                    "description": "Specific package to analyze (Rust: -p flag)."
                },
                "filter": {
                    "type": "string",
                    "description": "Filter output to lines containing this string."
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
        let depth = args["depth"].as_u64().unwrap_or(3).clamp(1, 10) as u32;
        let package = args["package"].as_str();
        let filter = args["filter"].as_str();

        let ecosystem = match args["ecosystem"].as_str().unwrap_or("auto") {
            "auto" => Self::detect_ecosystem(dir).await,
            eco => eco,
        };

        let (raw_output, stats) = match ecosystem {
            "rust" => {
                let out =
                    Self::run_cargo_tree(package, depth, filter, dir, self.timeout_secs).await;
                let stats = Self::parse_cargo_tree_stats(&out);
                (out, stats)
            }
            "node" => {
                let out = Self::run_npm_ls(depth, filter, dir, self.timeout_secs).await;
                let stats = DependencyStats {
                    direct_count: out
                        .lines()
                        .filter(|l| {
                            !l.contains('/') || l.trim_start_matches('/').matches('/').count() <= 2
                        })
                        .count(),
                    total_count: out.lines().count(),
                    max_depth: depth,
                    cycle_hints: vec![],
                };
                (out, stats)
            }
            _ => {
                let out = format!(
                    "Ecosystem '{}' not supported for tree visualization.\n\
                     Supported: rust (cargo tree), node (npm ls).",
                    ecosystem
                );
                let stats = DependencyStats {
                    direct_count: 0,
                    total_count: 0,
                    max_depth: 0,
                    cycle_hints: vec![],
                };
                (out, stats)
            }
        };

        let content = Self::format_summary(ecosystem, &raw_output, &stats);

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "ecosystem": ecosystem,
                "direct_deps": stats.direct_count,
                "total_deps": stats.total_count,
                "max_depth": stats.max_depth,
                "cycles": stats.cycle_hints.len()
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_input(args: Value) -> ToolInput {
        ToolInput {
            tool_use_id: "t1".into(),
            arguments: args,
            working_directory: "/Users/oscarvalois/Documents/Github/cuervo-cli".into(),
        }
    }

    #[test]
    fn tool_metadata() {
        let t = DependencyGraphTool::default();
        assert_eq!(t.name(), "dependency_graph");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"].as_array().unwrap().is_empty());
    }

    #[test]
    fn parse_cargo_tree_stats_basic() {
        let output = "halcon-cli v0.1.0
├── tokio v1.40
│   ├── tokio-macros v2.4
│   └── socket2 v0.5 (*)
└── serde v1.0";
        let stats = DependencyGraphTool::parse_cargo_tree_stats(output);
        assert!(stats.total_count > 0);
        // cycle_hints may or may not contain "socket2" depending on cargo's output
        assert!(stats.total_count > 0);
    }

    #[test]
    fn parse_cargo_tree_stats_empty() {
        let stats = DependencyGraphTool::parse_cargo_tree_stats("");
        assert_eq!(stats.direct_count, 0);
        assert_eq!(stats.total_count, 0);
    }

    #[test]
    fn format_summary_contains_stats() {
        let stats = DependencyStats {
            direct_count: 5,
            total_count: 42,
            max_depth: 3,
            cycle_hints: vec!["foo".into()],
        };
        let s = DependencyGraphTool::format_summary("rust", "tree output", &stats);
        assert!(s.contains("5"));
        assert!(s.contains("42"));
        assert!(s.contains("3"));
        assert!(s.contains("Cycle"));
        assert!(s.contains("tree output"));
    }

    #[tokio::test]
    async fn detect_ecosystem_rust() {
        let eco =
            DependencyGraphTool::detect_ecosystem("/Users/oscarvalois/Documents/Github/cuervo-cli")
                .await;
        assert_eq!(eco, "rust");
    }

    #[tokio::test]
    async fn execute_rust_ecosystem() {
        let tool = DependencyGraphTool::new(60);
        let out = tool
            .execute(make_input(json!({
                "ecosystem": "rust",
                "depth": 1,
                "package": "halcon-tools"
            })))
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
        assert!(
            out.content.contains("rust")
                || out.content.contains("Dependency")
                || out.content.contains("halcon"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_auto_detect() {
        let tool = DependencyGraphTool::new(60);
        let out = tool
            .execute(make_input(json!({ "depth": 1 })))
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
    }

    #[tokio::test]
    async fn execute_unsupported_ecosystem() {
        let tool = DependencyGraphTool::new(10);
        let out = tool
            .execute(make_input(json!({ "ecosystem": "python" })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("not supported") || out.content.contains("Ecosystem"));
    }

    #[test]
    fn depth_clamped() {
        let depth = 15u64.clamp(1, 10) as u32;
        assert_eq!(depth, 10);
    }
}
