/// Phase 94: Project Onboarding — Fast startup check.
///
/// Does this project have a HALCON.md? Sync, file-existence-only, <1ms.

use super::super::project_inspector::ProjectInspector;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingStatus {
    Configured { path: std::path::PathBuf },
    NotConfigured { root: std::path::PathBuf, project_type: String },
    Unknown,
}

pub struct OnboardingCheck;

impl OnboardingCheck {
    /// Sync check — file existence only, no subprocess. Fast (<1ms).
    pub fn run(cwd: &std::path::Path) -> OnboardingStatus {
        let root = ProjectInspector::find_project_root(cwd);

        let global_halcon = dirs::home_dir()
            .map(|h| h.join(".halcon"))
            .unwrap_or_default();

        // Check for project-level HALCON.md (excluding global ~/.halcon/)
        for candidate in &[
            root.join(".halcon").join("HALCON.md"),
            root.join("HALCON.md"),
        ] {
            if candidate.exists() && !candidate.starts_with(&global_halcon) {
                return OnboardingStatus::Configured {
                    path: candidate.clone(),
                };
            }
        }

        // Quick type detection (file existence only, no subprocess)
        let project_type = if root.join("Cargo.toml").exists() {
            "rust"
        } else if root.join("package.json").exists() {
            "node"
        } else if root.join("pyproject.toml").exists() || root.join("setup.py").exists() {
            "python"
        } else if root.join("go.mod").exists() {
            "go"
        } else {
            "unknown"
        };

        OnboardingStatus::NotConfigured {
            root,
            project_type: project_type.to_string(),
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
            "halcon_ob_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn configured_when_project_halcon_md_exists() {
        let dir = tempdir();
        let halcon_dir = dir.join(".halcon");
        fs::create_dir_all(&halcon_dir).unwrap();
        fs::write(halcon_dir.join("HALCON.md"), "# My Project\n").unwrap();

        let status = OnboardingCheck::run(&dir);
        assert!(
            matches!(status, OnboardingStatus::Configured { .. }),
            "should be Configured when .halcon/HALCON.md exists: {status:?}"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn not_configured_when_no_project_halcon_md() {
        let dir = tempdir();
        // No HALCON.md in the temp dir (global ~/.halcon is excluded by design)
        let status = OnboardingCheck::run(&dir);
        assert!(
            matches!(status, OnboardingStatus::NotConfigured { .. }),
            "should be NotConfigured when no project HALCON.md: {status:?}"
        );

        fs::remove_dir_all(&dir).unwrap();
    }
}
