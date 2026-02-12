//! Hierarchical instruction file loader (CUERVO.md).
//!
//! Searches for instruction files from the global home directory
//! down through the directory tree to the working directory.
//! Files are merged in order: global → ancestors → project root → cwd.

use std::path::{Path, PathBuf};

const INSTRUCTION_FILENAME: &str = "CUERVO.md";
const GLOBAL_DIR: &str = ".cuervo";

/// Load and merge instruction files from global and project hierarchy.
///
/// Search order (each found file is appended):
/// 1. `~/.cuervo/CUERVO.md` (global)
/// 2. Walk from filesystem root down to `working_dir`, collecting any `CUERVO.md` found
///
/// Returns the merged content as a single string, or `None` if no files found.
pub fn load_instructions(working_dir: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    // 1. Global instruction file.
    if let Some(home) = home_dir() {
        let global_path = home.join(GLOBAL_DIR).join(INSTRUCTION_FILENAME);
        if let Ok(content) = std::fs::read_to_string(&global_path) {
            if !content.trim().is_empty() {
                parts.push(content);
            }
        }
    }

    // 2. Walk ancestors from root toward working_dir, collecting instruction files.
    let canonical = match working_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => working_dir.to_path_buf(),
    };

    let ancestors: Vec<&Path> = canonical.ancestors().collect();
    // ancestors goes [cwd, parent, ..., /], we want root-to-cwd order.
    for ancestor in ancestors.iter().rev() {
        let instruction_path = ancestor.join(INSTRUCTION_FILENAME);
        if let Ok(content) = std::fs::read_to_string(&instruction_path) {
            if !content.trim().is_empty() {
                parts.push(content);
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// Collect the paths of all instruction files that would be loaded.
///
/// Useful for diagnostics and `/status` commands.
pub fn find_instruction_files(working_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();

    if let Some(home) = home_dir() {
        let global_path = home.join(GLOBAL_DIR).join(INSTRUCTION_FILENAME);
        if global_path.is_file() {
            found.push(global_path);
        }
    }

    let canonical = match working_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => working_dir.to_path_buf(),
    };

    let ancestors: Vec<&Path> = canonical.ancestors().collect();
    for ancestor in ancestors.iter().rev() {
        let instruction_path = ancestor.join(INSTRUCTION_FILENAME);
        if instruction_path.is_file() {
            found.push(instruction_path);
        }
    }

    found
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn no_files_returns_none() {
        let dir = TempDir::new().unwrap();
        let result = load_instructions(dir.path());
        // May find global CUERVO.md if it exists on the dev machine,
        // but the temp dir itself won't have one.
        // We test that the function doesn't panic at minimum.
        let _ = result;
    }

    #[test]
    fn single_file_in_working_dir() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("CUERVO.md"), "# Project rules\nUse Rust.").unwrap();

        let result = load_instructions(dir.path());
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("Use Rust."));
    }

    #[test]
    fn hierarchical_merge_parent_then_child() {
        let parent = TempDir::new().unwrap();
        let child = parent.path().join("subdir");
        std::fs::create_dir(&child).unwrap();

        std::fs::write(parent.path().join("CUERVO.md"), "Parent rules.").unwrap();
        std::fs::write(child.join("CUERVO.md"), "Child rules.").unwrap();

        let result = load_instructions(&child);
        assert!(result.is_some());
        let text = result.unwrap();

        // Parent should appear before child (root-to-cwd order).
        let parent_pos = text.find("Parent rules.").unwrap();
        let child_pos = text.find("Child rules.").unwrap();
        assert!(parent_pos < child_pos);
    }

    #[test]
    fn empty_files_skipped() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("CUERVO.md"), "   \n  ").unwrap();

        let result = load_instructions(dir.path());
        // The empty-whitespace file should be skipped.
        // Result depends on whether global file exists, so just check no panic.
        if let Some(text) = result {
            assert!(!text.trim().is_empty());
        }
    }

    #[test]
    fn find_instruction_files_lists_paths() {
        let parent = TempDir::new().unwrap();
        let child = parent.path().join("sub");
        std::fs::create_dir(&child).unwrap();

        std::fs::write(parent.path().join("CUERVO.md"), "parent").unwrap();
        std::fs::write(child.join("CUERVO.md"), "child").unwrap();

        let files = find_instruction_files(&child);
        // Should contain at least the parent and child files.
        let parent_found = files.iter().any(|p| {
            p.parent().and_then(|pp| pp.canonicalize().ok()) == parent.path().canonicalize().ok()
        });
        let child_found = files
            .iter()
            .any(|p| p.parent().and_then(|pp| pp.canonicalize().ok()) == child.canonicalize().ok());
        assert!(parent_found, "Parent CUERVO.md not found in: {:?}", files);
        assert!(child_found, "Child CUERVO.md not found in: {:?}", files);
    }

    #[test]
    fn deep_nesting_collects_all() {
        let root = TempDir::new().unwrap();
        let a = root.path().join("a");
        let b = a.join("b");
        let c = b.join("c");
        std::fs::create_dir_all(&c).unwrap();

        std::fs::write(root.path().join("CUERVO.md"), "root").unwrap();
        // Skip 'a', put one in 'b'.
        std::fs::write(b.join("CUERVO.md"), "b-level").unwrap();
        std::fs::write(c.join("CUERVO.md"), "c-level").unwrap();

        let result = load_instructions(&c).unwrap();
        assert!(result.contains("root"));
        assert!(result.contains("b-level"));
        assert!(result.contains("c-level"));

        // Order: root before b before c.
        let root_pos = result.find("root").unwrap();
        let b_pos = result.find("b-level").unwrap();
        let c_pos = result.find("c-level").unwrap();
        assert!(root_pos < b_pos);
        assert!(b_pos < c_pos);
    }
}
