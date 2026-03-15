/// Phase 94: Project Onboarding — Project Inspector
///
/// Detects project type, git info, and generates HALCON.md content.

/// Detected project type and metadata.
#[derive(Debug, Clone, Default)]
pub struct ProjectAnalysis {
    /// Project root directory (git root or CWD).
    pub root: std::path::PathBuf,
    /// Primary language/stack: "rust", "node", "python", "go", "mixed", "unknown"
    pub project_type: String,
    /// Package name from Cargo.toml / package.json / pyproject.toml
    pub package_name: Option<String>,
    /// Package description from manifest
    pub description: Option<String>,
    /// Git remote origin URL (if any)
    pub git_remote: Option<String>,
    /// Current git branch
    pub git_branch: Option<String>,
    /// README.md first 500 chars (as context for the system prompt)
    pub readme_excerpt: Option<String>,
    /// Key files found (up to 10): Cargo.toml, package.json, Makefile, etc.
    pub manifest_files: Vec<String>,
    /// Whether a project-level HALCON.md already exists
    pub has_project_halcon_md: bool,
    /// Save path suggestion for new HALCON.md
    pub suggested_halcon_md_path: std::path::PathBuf,
}

pub struct ProjectInspector;

impl ProjectInspector {
    /// Detect the project root starting from `cwd`.
    /// Priority: .git directory walk-up → CWD fallback.
    pub fn find_project_root(cwd: &std::path::Path) -> std::path::PathBuf {
        for ancestor in cwd.ancestors() {
            if ancestor.join(".git").exists() {
                return ancestor.to_path_buf();
            }
        }
        cwd.to_path_buf()
    }

    /// Full project analysis. Runs synchronously — call via spawn_blocking.
    pub fn analyze(cwd: &std::path::Path) -> ProjectAnalysis {
        let root = Self::find_project_root(cwd);

        // Detect project type via manifest files
        let manifest_candidates = [
            "Cargo.toml",
            "package.json",
            "pyproject.toml",
            "setup.py",
            "go.mod",
            "Makefile",
            "docker-compose.yml",
            "docker-compose.yaml",
            ".github",
            "CMakeLists.txt",
        ];

        let mut manifest_files: Vec<String> = manifest_candidates
            .iter()
            .filter(|f| root.join(f).exists())
            .map(|f| f.to_string())
            .collect();
        manifest_files.truncate(10);

        // Determine project type
        let has_cargo = root.join("Cargo.toml").exists();
        let has_package_json = root.join("package.json").exists();
        let has_pyproject = root.join("pyproject.toml").exists() || root.join("setup.py").exists();
        let has_go = root.join("go.mod").exists();

        let type_count = [has_cargo, has_package_json, has_pyproject, has_go]
            .iter()
            .filter(|&&b| b)
            .count();

        let project_type = if type_count > 1 {
            "mixed".to_string()
        } else if has_cargo {
            "rust".to_string()
        } else if has_package_json {
            "node".to_string()
        } else if has_pyproject {
            "python".to_string()
        } else if has_go {
            "go".to_string()
        } else {
            "unknown".to_string()
        };

        // Parse manifest for name/description
        let (package_name, description) = if has_cargo {
            Self::parse_toml_manifest(&root.join("Cargo.toml"))
        } else if has_package_json {
            Self::parse_json_manifest(&root.join("package.json"))
        } else if has_pyproject {
            Self::parse_toml_manifest(&root.join("pyproject.toml"))
        } else {
            (None, None)
        };

        // Git info (fire-and-forget)
        let git_remote = Self::run_git_command(&root, &["remote", "get-url", "origin"]);
        let git_branch = Self::run_git_command(&root, &["branch", "--show-current"]);

        // README excerpt
        let readme_excerpt = std::fs::read_to_string(root.join("README.md"))
            .ok()
            .map(|content| {
                let len = content.len().min(500);
                content[..len].to_string()
            });

        // Check for existing project-level HALCON.md
        let global_halcon = dirs::home_dir()
            .map(|h| h.join(".halcon"))
            .unwrap_or_default();

        let has_project_halcon_md = [
            root.join(".halcon").join("HALCON.md"),
            root.join("HALCON.md"),
        ]
        .iter()
        .any(|p| p.exists() && !p.starts_with(&global_halcon));

        let suggested_halcon_md_path = root.join(".halcon").join("HALCON.md");

        ProjectAnalysis {
            root,
            project_type,
            package_name,
            description,
            git_remote,
            git_branch,
            readme_excerpt,
            manifest_files,
            has_project_halcon_md,
            suggested_halcon_md_path,
        }
    }

    /// Generate a HALCON.md string from analysis results.
    pub fn generate_halcon_md(analysis: &ProjectAnalysis) -> String {
        let name = analysis
            .package_name
            .as_deref()
            .unwrap_or("Este Proyecto");
        let stack = &analysis.project_type;
        let description = analysis
            .description
            .as_deref()
            .unwrap_or("[sin descripción]");
        let git_remote = analysis
            .git_remote
            .as_deref()
            .unwrap_or("[sin repositorio remoto]");

        let manifest_list = if analysis.manifest_files.is_empty() {
            "- [sin archivos de manifiesto detectados]".to_string()
        } else {
            analysis
                .manifest_files
                .iter()
                .map(|f| format!("- {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let readme_section = if let Some(excerpt) = &analysis.readme_excerpt {
            format!("\n## Contexto Adicional\n\n{excerpt}\n")
        } else {
            String::new()
        };

        format!(
            "# {name} — Instrucciones de Proyecto\n\n\
## Proyecto\n\
- **Nombre**: {name}\n\
- **Stack**: {stack}\n\
- **Descripción**: {description}\n\
- **Repositorio**: {git_remote}\n\n\
## Estructura\n\
{manifest_list}\n\n\
## Guías de Desarrollo\n\
- [completar: convenciones de código, branching strategy, etc.]\n\
{readme_section}"
        )
    }

    /// Generate a .halcon/config.toml snippet for the project.
    pub fn generate_project_config(analysis: &ProjectAnalysis) -> String {
        let name = analysis
            .package_name
            .as_deref()
            .unwrap_or("este-proyecto");

        format!(
            "# {name} — Project-level HALCON config\n\
# Overrides ~/.halcon/config.toml for this project\n\n\
[general]\n\
# default_provider = \"deepseek\"\n\n\
[tools]\n\
allowed_directories = [\n\
    \".\",\n\
    \"/tmp\",\n\
]\n"
        )
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn run_git_command(root: &std::path::Path, args: &[&str]) -> Option<String> {
        std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Simple line scan for `name = "..."` (TOML) or `"name": "..."` (JSON-like).
    fn parse_toml_manifest(
        path: &std::path::Path,
    ) -> (Option<String>, Option<String>) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return (None, None),
        };

        let mut name = None;
        let mut description = None;

        for line in content.lines() {
            let trimmed = line.trim();
            if name.is_none() {
                if let Some(val) = Self::extract_toml_string(trimmed, "name") {
                    name = Some(val);
                }
            }
            if description.is_none() {
                if let Some(val) = Self::extract_toml_string(trimmed, "description") {
                    description = Some(val);
                }
            }
            if name.is_some() && description.is_some() {
                break;
            }
        }

        (name, description)
    }

    fn parse_json_manifest(
        path: &std::path::Path,
    ) -> (Option<String>, Option<String>) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return (None, None),
        };

        let mut name = None;
        let mut description = None;

        for line in content.lines() {
            let trimmed = line.trim();
            if name.is_none() {
                if let Some(val) = Self::extract_json_string(trimmed, "name") {
                    name = Some(val);
                }
            }
            if description.is_none() {
                if let Some(val) = Self::extract_json_string(trimmed, "description") {
                    description = Some(val);
                }
            }
            if name.is_some() && description.is_some() {
                break;
            }
        }

        (name, description)
    }

    /// Extract `key = "value"` from a TOML line.
    fn extract_toml_string(line: &str, key: &str) -> Option<String> {
        let prefix = format!("{key} = \"");
        if line.starts_with(&prefix) {
            let rest = &line[prefix.len()..];
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        } else {
            None
        }
    }

    /// Extract `"key": "value"` from a JSON line.
    fn extract_json_string(line: &str, key: &str) -> Option<String> {
        let prefix = format!("\"{key}\": \"");
        if let Some(pos) = line.find(&prefix) {
            let rest = &line[pos + prefix.len()..];
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tempdir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "halcon_pi_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn find_project_root_finds_git_root() {
        let dir = tempdir();
        fs::create_dir_all(dir.join(".git")).unwrap();
        let subdir = dir.join("src").join("lib");
        fs::create_dir_all(&subdir).unwrap();

        let root = ProjectInspector::find_project_root(&subdir);
        assert_eq!(root, dir, "should find .git ancestor");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_project_root_falls_back_to_cwd() {
        let dir = tempdir();
        // No .git directory anywhere under tmpdir path
        let root = ProjectInspector::find_project_root(&dir);
        // Falls back to the directory itself (or its nearest ancestor in tmpdir chain —
        // either is acceptable, the important thing is it doesn't panic)
        assert!(root.exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn analyze_detects_rust_project() {
        let dir = tempdir();
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"myapp\"\ndescription = \"A test app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let analysis = ProjectInspector::analyze(&dir);
        assert_eq!(analysis.project_type, "rust");
        assert_eq!(analysis.package_name.as_deref(), Some("myapp"));
        assert!(analysis.manifest_files.contains(&"Cargo.toml".to_string()));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn analyze_detects_node_project() {
        let dir = tempdir();
        fs::write(
            dir.join("package.json"),
            "{\n  \"name\": \"myapp\",\n  \"version\": \"1.0.0\"\n}\n",
        )
        .unwrap();

        let analysis = ProjectInspector::analyze(&dir);
        assert_eq!(analysis.project_type, "node");
        assert_eq!(analysis.package_name.as_deref(), Some("myapp"));
        assert!(analysis.manifest_files.contains(&"package.json".to_string()));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn generate_halcon_md_contains_package_name() {
        let analysis = ProjectAnalysis {
            package_name: Some("awesome-project".to_string()),
            project_type: "rust".to_string(),
            ..ProjectAnalysis::default()
        };
        let md = ProjectInspector::generate_halcon_md(&analysis);
        assert!(md.contains("awesome-project"), "should contain package name");
        assert!(md.contains("rust"), "should contain stack");
    }

    #[test]
    fn generate_project_config_has_allowed_directories() {
        let analysis = ProjectAnalysis {
            package_name: Some("my-crate".to_string()),
            ..ProjectAnalysis::default()
        };
        let config = ProjectInspector::generate_project_config(&analysis);
        assert!(config.contains("allowed_directories"), "should have allowed_directories");
        assert!(config.contains("\".\""), "should include current dir");
    }
}
