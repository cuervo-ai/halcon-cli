use std::path::{Path, PathBuf};

use cuervo_core::error::{CuervoError, Result};

/// Pre-compiled blocked patterns for efficient repeated validation.
///
/// Compile once at tool initialization, reuse for every file operation.
pub struct CompiledPatterns {
    patterns: Vec<glob::Pattern>,
}

impl CompiledPatterns {
    /// Compile blocked patterns from string representations.
    pub fn new(blocked_patterns: &[String]) -> Self {
        let patterns = blocked_patterns
            .iter()
            .filter_map(|p| glob::Pattern::new(p).ok())
            .collect();
        Self { patterns }
    }

    /// Check if a path matches any compiled blocked pattern.
    pub fn is_blocked(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        for pattern in &self.patterns {
            if pattern.matches(&path_str) {
                return true;
            }
            if let Some(name) = path.file_name() {
                if pattern.matches(&name.to_string_lossy()) {
                    return true;
                }
            }
        }
        false
    }
}

/// Resolve a path relative to the working directory and validate it against security policies.
///
/// Returns the canonicalized absolute path if validation passes.
pub fn resolve_and_validate(
    path: &str,
    working_dir: &str,
    allowed_dirs: &[PathBuf],
    blocked_patterns: &[String],
) -> Result<PathBuf> {
    // Pre-normalize working_dir once (reused for join + allowed check).
    let wd = normalize_path(Path::new(working_dir));

    let raw = Path::new(path);
    let absolute = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        wd.join(raw)
    };

    // Normalize the path (resolve `.` and `..` without requiring the file to exist).
    let normalized = normalize_path(&absolute);

    // Check blocked patterns first.
    if is_blocked(&normalized, blocked_patterns) {
        return Err(CuervoError::ToolExecutionFailed {
            tool: "path_security".into(),
            message: format!("path matches blocked pattern: {}", normalized.display()),
        });
    }

    // Check allowed directories (working_dir already normalized).
    if !is_within_allowed(&normalized, &wd, allowed_dirs) {
        return Err(CuervoError::ToolExecutionFailed {
            tool: "path_security".into(),
            message: format!(
                "path {} is outside allowed directories",
                normalized.display()
            ),
        });
    }

    Ok(normalized)
}

/// Resolve and validate using pre-compiled blocked patterns (zero per-call compilation).
pub fn resolve_and_validate_compiled(
    path: &str,
    working_dir: &str,
    allowed_dirs: &[PathBuf],
    compiled: &CompiledPatterns,
) -> Result<PathBuf> {
    // Pre-normalize working_dir once (reused for join + allowed check).
    let wd = normalize_path(Path::new(working_dir));

    let raw = Path::new(path);
    let absolute = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        wd.join(raw)
    };

    let normalized = normalize_path(&absolute);

    if compiled.is_blocked(&normalized) {
        return Err(CuervoError::ToolExecutionFailed {
            tool: "path_security".into(),
            message: format!("path matches blocked pattern: {}", normalized.display()),
        });
    }

    if !is_within_allowed(&normalized, &wd, allowed_dirs) {
        return Err(CuervoError::ToolExecutionFailed {
            tool: "path_security".into(),
            message: format!(
                "path {} is outside allowed directories",
                normalized.display()
            ),
        });
    }

    Ok(normalized)
}

/// Check if a path matches any blocked pattern.
pub fn is_blocked(path: &Path, blocked_patterns: &[String]) -> bool {
    let path_str = path.to_string_lossy();
    for pattern in blocked_patterns {
        // Use glob matching against the filename and full path.
        if let Ok(compiled) = glob::Pattern::new(pattern) {
            // Check against the full path.
            if compiled.matches(&path_str) {
                return true;
            }
            // Check against just the filename.
            if let Some(name) = path.file_name() {
                if compiled.matches(&name.to_string_lossy()) {
                    return true;
                }
            }
        }
    }
    false
}

/// Check if a path is within the working directory or one of the allowed directories.
/// Takes an already-normalized working_dir to avoid redundant normalization.
fn is_within_allowed(path: &Path, normalized_wd: &Path, allowed_dirs: &[PathBuf]) -> bool {
    if path.starts_with(normalized_wd) {
        return true;
    }
    for dir in allowed_dirs {
        let normalized = normalize_path(dir);
        if path.starts_with(&normalized) {
            return true;
        }
    }
    false
}

/// Normalize a path by resolving `.` and `..` components without filesystem access.
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                // Only pop if we have a real directory to go back from.
                if components
                    .last()
                    .is_some_and(|c| !matches!(c, std::path::Component::RootDir))
                {
                    components.pop();
                }
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_relative_path() {
        let result = resolve_and_validate("src/main.rs", "/project", &[], &[]).unwrap();
        assert_eq!(result, PathBuf::from("/project/src/main.rs"));
    }

    #[test]
    fn resolve_absolute_path_within_working_dir() {
        let result = resolve_and_validate("/project/src/lib.rs", "/project", &[], &[]).unwrap();
        assert_eq!(result, PathBuf::from("/project/src/lib.rs"));
    }

    #[test]
    fn reject_path_outside_working_dir() {
        let result = resolve_and_validate("/etc/passwd", "/project", &[], &[]);
        assert!(result.is_err());
    }

    #[test]
    fn allow_path_in_allowed_dir() {
        let allowed = vec![PathBuf::from("/home/user/data")];
        let result =
            resolve_and_validate("/home/user/data/file.txt", "/project", &allowed, &[]).unwrap();
        assert_eq!(result, PathBuf::from("/home/user/data/file.txt"));
    }

    #[test]
    fn reject_traversal_attack() {
        let result = resolve_and_validate("../../etc/passwd", "/project/src", &[], &[]);
        assert!(result.is_err());
    }

    #[test]
    fn block_env_files() {
        let blocked = vec![".env".to_string(), ".env.*".to_string()];
        let result = resolve_and_validate("src/.env", "/project", &[], &blocked);
        assert!(result.is_err());
    }

    #[test]
    fn block_pem_files() {
        let blocked = vec!["*.pem".to_string()];
        let result = resolve_and_validate("certs/server.pem", "/project", &[], &blocked);
        assert!(result.is_err());
    }

    #[test]
    fn allow_normal_files() {
        let blocked = vec![".env".to_string(), "*.pem".to_string()];
        let result = resolve_and_validate("src/main.rs", "/project", &[], &blocked).unwrap();
        assert_eq!(result, PathBuf::from("/project/src/main.rs"));
    }

    #[test]
    fn is_blocked_matches_filename() {
        let blocked = vec!["credentials.json".to_string()];
        assert!(is_blocked(
            Path::new("/project/config/credentials.json"),
            &blocked
        ));
    }

    #[test]
    fn normalize_resolves_parent() {
        let p = normalize_path(Path::new("/a/b/../c"));
        assert_eq!(p, PathBuf::from("/a/c"));
    }

    #[test]
    fn normalize_resolves_current() {
        let p = normalize_path(Path::new("/a/./b/./c"));
        assert_eq!(p, PathBuf::from("/a/b/c"));
    }

    #[test]
    fn deeply_nested_traversal_blocked() {
        let result = resolve_and_validate(
            "../../../../../../../../etc/passwd",
            "/project/src/deep/nested",
            &[],
            &[],
        );
        assert!(result.is_err(), "deeply nested traversal should be blocked");
    }

    #[test]
    fn empty_path_resolves_to_working_dir() {
        let result = resolve_and_validate("", "/project", &[], &[]);
        // Empty path resolves to working_dir itself, which is within working_dir.
        assert!(result.is_ok());
    }

    #[test]
    fn block_key_files() {
        let blocked = vec!["*.key".to_string()];
        let result = resolve_and_validate("secrets/api.key", "/project", &[], &blocked);
        assert!(result.is_err());
    }

    #[test]
    fn normalize_at_root_boundary() {
        // Can't go above root.
        let p = normalize_path(Path::new("/../../etc/passwd"));
        assert_eq!(p, PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn multiple_blocked_patterns() {
        let blocked = vec![
            ".env".to_string(),
            "*.pem".to_string(),
            "*.key".to_string(),
            "credentials.json".to_string(),
        ];
        assert!(resolve_and_validate(".env", "/p", &[], &blocked).is_err());
        assert!(resolve_and_validate("x.pem", "/p", &[], &blocked).is_err());
        assert!(resolve_and_validate("x.key", "/p", &[], &blocked).is_err());
        assert!(resolve_and_validate("credentials.json", "/p", &[], &blocked).is_err());
        assert!(resolve_and_validate("safe.txt", "/p", &[], &blocked).is_ok());
    }

    // --- CompiledPatterns tests ---

    #[test]
    fn compiled_patterns_blocks_matching() {
        let compiled = CompiledPatterns::new(&[
            ".env".to_string(),
            "*.pem".to_string(),
        ]);
        assert!(compiled.is_blocked(Path::new("/project/.env")));
        assert!(compiled.is_blocked(Path::new("/certs/server.pem")));
        assert!(!compiled.is_blocked(Path::new("/project/main.rs")));
    }

    #[test]
    fn compiled_patterns_matches_filename_only() {
        let compiled = CompiledPatterns::new(&["credentials.json".to_string()]);
        assert!(compiled.is_blocked(Path::new("/deep/nested/credentials.json")));
    }

    #[test]
    fn compiled_patterns_empty_passes_all() {
        let compiled = CompiledPatterns::new(&[]);
        assert!(!compiled.is_blocked(Path::new("/any/path")));
    }

    #[test]
    fn resolve_with_compiled_patterns() {
        let compiled = CompiledPatterns::new(&["*.pem".to_string()]);
        let result = resolve_and_validate_compiled("certs/server.pem", "/project", &[], &compiled);
        assert!(result.is_err());
        let result = resolve_and_validate_compiled("src/main.rs", "/project", &[], &compiled);
        assert!(result.is_ok());
    }
}
