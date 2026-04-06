//! MakeTool — run build system targets (make, cmake, gradle, mvn, cargo, npm, etc.).
//!
//! Auto-detects the build system from project files and provides a unified interface
//! to list targets and run builds. Supports:
//! - Makefile → `make`
//! - CMakeLists.txt → `cmake --build`
//! - build.gradle / build.gradle.kts → `./gradlew`
//! - pom.xml → `mvn`
//! - Cargo.toml → `cargo build`
//! - package.json with scripts → `npm run / pnpm run / yarn run`
//! - Justfile → `just`
//! - Taskfile.yml → `task`

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildSystem {
    Make,
    Cmake,
    Gradle,
    Maven,
    Cargo,
    Npm,
    Just,
    Task,
}

impl BuildSystem {
    fn name(&self) -> &'static str {
        match self {
            Self::Make => "make",
            Self::Cmake => "cmake",
            Self::Gradle => "gradle",
            Self::Maven => "maven",
            Self::Cargo => "cargo",
            Self::Npm => "npm/pnpm/yarn",
            Self::Just => "just",
            Self::Task => "task",
        }
    }
}

pub struct MakeTool {
    timeout_secs: u64,
}

impl MakeTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    fn detect_build_system(dir: &Path) -> Option<BuildSystem> {
        let indicators = [
            ("Justfile", BuildSystem::Just),
            ("justfile", BuildSystem::Just),
            ("Taskfile.yml", BuildSystem::Task),
            ("Taskfile.yaml", BuildSystem::Task),
            ("Makefile", BuildSystem::Make),
            ("makefile", BuildSystem::Make),
            ("GNUmakefile", BuildSystem::Make),
            ("CMakeLists.txt", BuildSystem::Cmake),
            ("build.gradle", BuildSystem::Gradle),
            ("build.gradle.kts", BuildSystem::Gradle),
            ("pom.xml", BuildSystem::Maven),
            ("Cargo.toml", BuildSystem::Cargo),
            ("package.json", BuildSystem::Npm),
        ];

        for (file, system) in &indicators {
            if dir.join(file).exists() {
                return Some(*system);
            }
        }
        None
    }

    fn detect_node_pm(dir: &Path) -> &'static str {
        if dir.join("pnpm-lock.yaml").exists() {
            "pnpm"
        } else if dir.join("yarn.lock").exists() {
            "yarn"
        } else {
            "npm"
        }
    }

    /// Extract available targets for the detected build system.
    fn list_targets_hint(system: BuildSystem, dir: &Path) -> String {
        match system {
            BuildSystem::Make => {
                // Parse Makefile targets (lines matching `target:`)
                let makefile_path = ["Makefile", "makefile", "GNUmakefile"]
                    .iter()
                    .find_map(|f| {
                        let p = dir.join(f);
                        if p.exists() {
                            Some(p)
                        } else {
                            None
                        }
                    });

                if let Some(path) = makefile_path {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let targets: Vec<&str> = content
                            .lines()
                            .filter(|l| {
                                !l.starts_with('\t')
                                    && !l.starts_with('#')
                                    && !l.starts_with('.')
                                    && l.contains(':')
                                    && !l.contains('=')
                            })
                            .filter_map(|l| l.split(':').next())
                            .map(|t| t.trim())
                            .filter(|t| !t.is_empty() && !t.contains(' '))
                            .take(20)
                            .collect();
                        if !targets.is_empty() {
                            return format!("Targets: {}", targets.join(", "));
                        }
                    }
                }
                "Common: all, build, clean, test, install".to_string()
            }
            BuildSystem::Npm => {
                if let Ok(content) = std::fs::read_to_string(dir.join("package.json")) {
                    if let Ok(v) = serde_json::from_str::<Value>(&content) {
                        if let Some(scripts) = v["scripts"].as_object() {
                            let names: Vec<&str> =
                                scripts.keys().map(|k| k.as_str()).take(15).collect();
                            return format!("Scripts: {}", names.join(", "));
                        }
                    }
                }
                "Common: build, test, dev, start, lint".to_string()
            }
            BuildSystem::Cargo => {
                "Targets: build, test, clippy, check, run, doc, bench".to_string()
            }
            BuildSystem::Maven => {
                "Phases: compile, test, package, install, clean, verify, deploy".to_string()
            }
            BuildSystem::Gradle => {
                "Tasks: build, test, clean, assemble, check, jar, run".to_string()
            }
            BuildSystem::Cmake => {
                "Targets: all, clean, install, test (after cmake configure)".to_string()
            }
            BuildSystem::Just => {
                // Run `just --list` is async — return hint only
                "Run: just --list  to see all recipes".to_string()
            }
            BuildSystem::Task => "Run: task --list  to see all tasks".to_string(),
        }
    }

    async fn run_build(
        &self,
        system: BuildSystem,
        target: &str,
        args: &[&str],
        working_dir: &Path,
    ) -> Result<(bool, String), String> {
        let (cmd, cmd_args) = match system {
            BuildSystem::Make => ("make", vec![target]),
            BuildSystem::Cmake => ("cmake", {
                let mut a = vec!["--build", "."];
                if !target.is_empty() && target != "all" {
                    a.extend(["--target", target]);
                }
                a
            }),
            BuildSystem::Gradle => {
                let gradle = if working_dir.join("gradlew").exists() {
                    "./gradlew"
                } else {
                    "gradle"
                };
                (gradle, vec![target])
            }
            BuildSystem::Maven => ("mvn", vec![target]),
            BuildSystem::Cargo => {
                let mut a = vec![target];
                a.extend(args.iter().copied());
                ("cargo", a)
            }
            BuildSystem::Npm => {
                let pm = Self::detect_node_pm(working_dir);
                let mut a = vec!["run", target];
                a.extend(args.iter().copied());
                (pm, a)
            }
            BuildSystem::Just => ("just", vec![target]),
            BuildSystem::Task => ("task", vec![target]),
        };

        let timeout = Duration::from_secs(self.timeout_secs);

        let mut command = tokio::process::Command::new(cmd);
        command
            .args(&cmd_args)
            .current_dir(working_dir)
            .env("TERM", "dumb")
            .env("NO_COLOR", "1");

        let result = tokio::time::timeout(timeout, command.output()).await;

        match result {
            Err(_) => Err(format!("Build timed out after {}s", self.timeout_secs)),
            Ok(Err(e)) => Err(format!("Failed to run {}: {}", cmd, e)),
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                let combined = format!("{}{}", stdout, stderr);
                let success = out.status.success();
                Ok((success, combined.chars().take(8000).collect()))
            }
        }
    }
}

impl Default for MakeTool {
    fn default() -> Self {
        Self::new(120)
    }
}

#[async_trait]
impl Tool for MakeTool {
    fn name(&self) -> &str {
        "make"
    }

    fn description(&self) -> &str {
        "Run build system targets (make, cmake, gradle, mvn, cargo, npm, just, task). \
         Auto-detects the build system from project files (Makefile, CMakeLists.txt, \
         Cargo.toml, package.json, etc.). Can list available targets or run a specific target. \
         Use 'list' as target to show available targets without building."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "description": "Build target or script name to run (e.g. 'build', 'test', 'clean', 'all'). Use 'list' to show available targets."
                },
                "build_system": {
                    "type": "string",
                    "enum": ["auto", "make", "cmake", "gradle", "maven", "cargo", "npm", "just", "task"],
                    "description": "Build system to use. 'auto' (default) detects from project files."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Additional arguments to pass to the build command."
                },
                "working_directory": {
                    "type": "string",
                    "description": "Directory to run the build in. Defaults to the tool's working directory."
                }
            },
            "required": ["target"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadWrite
    }

    fn requires_confirmation(&self, input: &ToolInput) -> bool {
        // Require confirmation for destructive targets
        let target = input.arguments["target"].as_str().unwrap_or("");
        matches!(
            target,
            "deploy" | "publish" | "release" | "install" | "uninstall" | "push"
        )
    }

    async fn execute_inner(
        &self,
        input: ToolInput,
    ) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let args = &input.arguments;
        let working_dir = args["working_directory"]
            .as_str()
            .map(|p| {
                let p = Path::new(p);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    PathBuf::from(&input.working_directory).join(p)
                }
            })
            .unwrap_or_else(|| PathBuf::from(&input.working_directory));

        let target = args["target"].as_str().unwrap_or("build");

        let system = if let Some(bs) = args["build_system"].as_str().filter(|&s| s != "auto") {
            match bs {
                "make" => BuildSystem::Make,
                "cmake" => BuildSystem::Cmake,
                "gradle" => BuildSystem::Gradle,
                "maven" => BuildSystem::Maven,
                "cargo" => BuildSystem::Cargo,
                "npm" => BuildSystem::Npm,
                "just" => BuildSystem::Just,
                "task" => BuildSystem::Task,
                _ => {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: format!("Unknown build system: {}", bs),
                        is_error: true,
                        metadata: None,
                    });
                }
            }
        } else {
            match Self::detect_build_system(&working_dir) {
                Some(s) => s,
                None => {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: format!(
                            "No build system detected in {}.\n\nLooked for: Makefile, CMakeLists.txt, Cargo.toml, package.json, build.gradle, pom.xml, Justfile, Taskfile.yml",
                            working_dir.display()
                        ),
                        is_error: false,
                        metadata: None,
                    });
                }
            }
        };

        if target == "list" {
            let hint = Self::list_targets_hint(system, &working_dir);
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("Build system: {}\n{}", system.name(), hint),
                is_error: false,
                metadata: Some(json!({ "build_system": system.name() })),
            });
        }

        let extra_args: Vec<&str> = args["args"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        tracing::info!(
            tool = "make",
            build_system = system.name(),
            target,
            "running build"
        );

        match self
            .run_build(system, target, &extra_args, &working_dir)
            .await
        {
            Ok((success, output)) => {
                let status = if success {
                    "✅ Build succeeded"
                } else {
                    "❌ Build failed"
                };
                let content = format!(
                    "{} — {} target '{}'\n\n{}",
                    status,
                    system.name(),
                    target,
                    output
                );
                Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content,
                    is_error: !success,
                    metadata: Some(json!({
                        "build_system": system.name(),
                        "target": target,
                        "success": success
                    })),
                })
            }
            Err(e) => Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: e,
                is_error: true,
                metadata: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn detect_makefile() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("Makefile"), "all:\n\techo hi\n").unwrap();
        assert_eq!(
            MakeTool::detect_build_system(dir.path()),
            Some(BuildSystem::Make)
        );
    }

    #[test]
    fn detect_cargo() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        assert_eq!(
            MakeTool::detect_build_system(dir.path()),
            Some(BuildSystem::Cargo)
        );
    }

    #[test]
    fn detect_npm() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"tsc"}}"#,
        )
        .unwrap();
        assert_eq!(
            MakeTool::detect_build_system(dir.path()),
            Some(BuildSystem::Npm)
        );
    }

    #[test]
    fn detect_none() {
        let dir = TempDir::new().unwrap();
        assert!(MakeTool::detect_build_system(dir.path()).is_none());
    }

    #[test]
    fn detect_node_pm_pnpm() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(MakeTool::detect_node_pm(dir.path()), "pnpm");
    }

    #[test]
    fn detect_node_pm_yarn() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("yarn.lock"), "").unwrap();
        assert_eq!(MakeTool::detect_node_pm(dir.path()), "yarn");
    }

    #[test]
    fn detect_node_pm_npm_default() {
        let dir = TempDir::new().unwrap();
        assert_eq!(MakeTool::detect_node_pm(dir.path()), "npm");
    }

    #[test]
    fn list_targets_npm() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"tsc","test":"jest","lint":"eslint ."}}"#,
        )
        .unwrap();
        let hint = MakeTool::list_targets_hint(BuildSystem::Npm, dir.path());
        assert!(hint.contains("build") || hint.contains("Scripts"));
    }

    #[test]
    fn list_targets_makefile() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Makefile"),
            "all:\n\techo all\nbuild:\n\techo build\nclean:\n\trm -f *.o\n",
        )
        .unwrap();
        let hint = MakeTool::list_targets_hint(BuildSystem::Make, dir.path());
        assert!(
            hint.contains("all") || hint.contains("build"),
            "hint={}",
            hint
        );
    }

    #[tokio::test]
    async fn execute_no_build_system_returns_helpful() {
        let dir = TempDir::new().unwrap();
        let tool = MakeTool::new(30);
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({"target": "build"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("No build system") || out.content.contains("Makefile"));
    }

    #[tokio::test]
    async fn execute_list_target_returns_info() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\nedition=\"2021\"",
        )
        .unwrap();
        let tool = MakeTool::new(30);
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({"target": "list"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("cargo") || out.content.contains("Targets"));
    }

    #[test]
    fn tool_metadata() {
        let t = MakeTool::default();
        assert_eq!(t.name(), "make");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadWrite);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("target")));
    }
}
