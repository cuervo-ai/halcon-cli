//! Directory tree tool — recursive listing with depth limit.
//!
//! Generates a tree-style view of a directory structure, similar to
//! the Unix `tree` command. Useful for understanding project layout.

use std::path::Path;

use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

/// Maximum depth to prevent runaway recursion.
const MAX_DEPTH: u32 = 10;
/// Maximum entries to prevent enormous output.
const MAX_ENTRIES: usize = 2000;

/// Display a directory's contents as a tree.
pub struct DirectoryTreeTool;

impl DirectoryTreeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DirectoryTreeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for DirectoryTreeTool {
    fn name(&self) -> &str {
        "directory_tree"
    }

    fn description(&self) -> &str {
        "List the contents of a directory as a tree structure. Supports depth limit."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let path_str = input.arguments["path"]
            .as_str()
            .ok_or_else(|| CuervoError::ToolExecutionFailed {
                tool: "directory_tree".into(),
                message: "missing required 'path' argument".into(),
            })?;

        let max_depth = input.arguments["depth"]
            .as_u64()
            .unwrap_or(3)
            .min(MAX_DEPTH as u64) as u32;

        let root = if Path::new(path_str).is_absolute() {
            std::path::PathBuf::from(path_str)
        } else {
            let wd = &input.working_directory;
            Path::new(wd).join(path_str)
        };

        if !root.exists() {
            return Err(CuervoError::ToolExecutionFailed {
                tool: "directory_tree".into(),
                message: format!("path does not exist: {}", root.display()),
            });
        }

        if !root.is_dir() {
            return Err(CuervoError::ToolExecutionFailed {
                tool: "directory_tree".into(),
                message: format!("path is not a directory: {}", root.display()),
            });
        }

        let mut output = String::new();
        let mut count = 0;
        let mut truncated = false;

        output.push_str(&format!("{}/\n", root.display()));
        build_tree(
            &root,
            "",
            0,
            max_depth,
            &mut output,
            &mut count,
            &mut truncated,
        );

        if truncated {
            output.push_str(&format!(
                "\n... truncated at {MAX_ENTRIES} entries (increase depth or narrow scope)\n"
            ));
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: output,
            is_error: false,
            metadata: Some(json!({
                "path": root.display().to_string(),
                "entries": count,
                "depth": max_depth,
                "truncated": truncated,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path to list"
                },
                "depth": {
                    "type": "integer",
                    "description": "Maximum depth (default: 3, max: 10)"
                }
            },
            "required": ["path"]
        })
    }
}

/// Recursively build a tree string.
fn build_tree(
    dir: &Path,
    prefix: &str,
    depth: u32,
    max_depth: u32,
    output: &mut String,
    count: &mut usize,
    truncated: &mut bool,
) {
    if depth >= max_depth || *truncated {
        return;
    }

    let mut entries: Vec<std::fs::DirEntry> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };

    // Sort: directories first, then alphabetically.
    entries.sort_by(|a, b| {
        let a_dir = a.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        let b_dir = b.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        match (a_dir, b_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.file_name().cmp(&b.file_name()),
        }
    });

    // Skip hidden and common noise directories.
    entries.retain(|e| {
        let name = e.file_name();
        let name_str = name.to_string_lossy();
        !name_str.starts_with('.')
            && name_str != "node_modules"
            && name_str != "target"
            && name_str != "__pycache__"
            && name_str != ".git"
    });

    let total = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        if *count >= MAX_ENTRIES {
            *truncated = true;
            return;
        }

        let is_last = i == total - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = if is_last { "    " } else { "│   " };

        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);

        if is_dir {
            output.push_str(&format!("{prefix}{connector}{name_str}/\n"));
        } else {
            output.push_str(&format!("{prefix}{connector}{name_str}\n"));
        }
        *count += 1;

        if is_dir {
            let new_prefix = format!("{prefix}{child_prefix}");
            build_tree(
                &entry.path(),
                &new_prefix,
                depth + 1,
                max_depth,
                output,
                count,
                truncated,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = DirectoryTreeTool::new();
        assert_eq!(tool.name(), "directory_tree");
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn input_schema_valid() {
        let tool = DirectoryTreeTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["path"].is_object());
        assert_eq!(schema["required"][0], "path");
    }

    #[tokio::test]
    async fn missing_path_argument() {
        let tool = DirectoryTreeTool::new();
        let input = ToolInput {
            tool_use_id: "test".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        let result = tool.execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn nonexistent_directory() {
        let tool = DirectoryTreeTool::new();
        let input = ToolInput {
            tool_use_id: "test".into(),
            arguments: json!({"path": "/nonexistent_dir_xyz"}),
            working_directory: "/tmp".into(),
        };
        let result = tool.execute(input).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn file_not_directory() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let tool = DirectoryTreeTool::new();
        let input = ToolInput {
            tool_use_id: "test".into(),
            arguments: json!({"path": tmp.path().to_str().unwrap()}),
            working_directory: "/tmp".into(),
        };
        let result = tool.execute(input).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not a directory"));
    }

    #[tokio::test]
    async fn list_tmp_directory() {
        let dir = tempfile::tempdir().unwrap();
        // Create some test files/dirs.
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "content").unwrap();
        std::fs::write(dir.path().join("src").join("main.rs"), "fn main(){}").unwrap();

        let tool = DirectoryTreeTool::new();
        let input = ToolInput {
            tool_use_id: "test".into(),
            arguments: json!({"path": dir.path().to_str().unwrap(), "depth": 3}),
            working_directory: "/tmp".into(),
        };

        let result = tool.execute(input).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("src/"));
        assert!(result.content.contains("Cargo.toml"));
        assert!(result.content.contains("main.rs"));

        let meta = result.metadata.unwrap();
        assert_eq!(meta["entries"], 3);
        assert!(!meta["truncated"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn respects_depth_limit() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a").join("b").join("c").join("d");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("deep.txt"), "deep").unwrap();

        let tool = DirectoryTreeTool::new();
        let input = ToolInput {
            tool_use_id: "test".into(),
            arguments: json!({"path": dir.path().to_str().unwrap(), "depth": 1}),
            working_directory: "/tmp".into(),
        };

        let result = tool.execute(input).await.unwrap();
        assert!(result.content.contains("a/"));
        // depth=1 should NOT recurse into a/b.
        assert!(!result.content.contains("deep.txt"));
    }

    #[tokio::test]
    async fn skips_hidden_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".hidden")).unwrap();
        std::fs::create_dir(dir.path().join("visible")).unwrap();

        let tool = DirectoryTreeTool::new();
        let input = ToolInput {
            tool_use_id: "test".into(),
            arguments: json!({"path": dir.path().to_str().unwrap()}),
            working_directory: "/tmp".into(),
        };

        let result = tool.execute(input).await.unwrap();
        assert!(result.content.contains("visible/"));
        assert!(!result.content.contains(".hidden"));
    }

    #[test]
    fn tree_connectors() {
        // Verify the tree drawing characters work.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("aaa.txt"), "").unwrap();
        std::fs::write(dir.path().join("zzz.txt"), "").unwrap();

        let mut output = String::new();
        let mut count = 0;
        let mut truncated = false;
        build_tree(
            dir.path(),
            "",
            0,
            3,
            &mut output,
            &mut count,
            &mut truncated,
        );

        assert!(output.contains("├──"));
        assert!(output.contains("└──"));
        assert_eq!(count, 2);
    }
}
