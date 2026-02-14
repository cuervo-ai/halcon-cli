//! Directory tree tool — async recursive listing with depth limit.
//!
//! Generates a tree-style view of a directory structure, similar to
//! the Unix `tree` command. Useful for understanding project layout.
//!
//! Uses `FsService::read_dir_async()` for non-blocking directory reads.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

use crate::fs_service::FsService;

/// Maximum depth to prevent runaway recursion.
const MAX_DEPTH: u32 = 10;
/// Maximum entries to prevent enormous output.
const MAX_ENTRIES: usize = 2000;

/// Display a directory's contents as a tree.
pub struct DirectoryTreeTool {
    fs: Arc<FsService>,
}

impl DirectoryTreeTool {
    pub fn new(fs: Arc<FsService>) -> Self {
        Self { fs }
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
            PathBuf::from(path_str)
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
        let mut count = 0usize;
        let mut truncated = false;

        output.push_str(&format!("{}/\n", root.display()));

        // Iterative stack-based async traversal.
        // Each entry: (dir_path, prefix, depth).
        let mut stack: Vec<(PathBuf, String, u32)> = vec![(root.clone(), String::new(), 0)];

        while let Some((dir, prefix, depth)) = stack.pop() {
            if depth >= max_depth || truncated {
                continue;
            }

            let entries = match self.fs.read_dir_async(&dir).await {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Filter hidden and noise directories.
            let entries: Vec<_> = entries
                .into_iter()
                .filter(|e| {
                    !e.name.starts_with('.')
                        && e.name != "node_modules"
                        && e.name != "target"
                        && e.name != "__pycache__"
                })
                .collect();

            // Collect child dirs to push onto stack (in reverse order so first is processed first).
            let mut child_dirs: Vec<(PathBuf, String, u32)> = Vec::new();

            let total = entries.len();
            for (i, entry) in entries.iter().enumerate() {
                if count >= MAX_ENTRIES {
                    truncated = true;
                    break;
                }

                let is_last = i == total - 1;
                let connector = if is_last { "└── " } else { "├── " };
                let child_prefix = if is_last { "    " } else { "│   " };

                if entry.is_dir {
                    output.push_str(&format!("{prefix}{connector}{}/\n", entry.name));
                } else {
                    output.push_str(&format!("{prefix}{connector}{}\n", entry.name));
                }
                count += 1;

                if entry.is_dir {
                    let new_prefix = format!("{prefix}{child_prefix}");
                    child_dirs.push((entry.path.clone(), new_prefix, depth + 1));
                }
            }

            // Push child dirs in reverse so they're processed in order.
            for child in child_dirs.into_iter().rev() {
                stack.push(child);
            }
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_fs() -> Arc<FsService> {
        Arc::new(FsService::new(vec![], vec![]))
    }

    #[test]
    fn tool_metadata() {
        let tool = DirectoryTreeTool::new(test_fs());
        assert_eq!(tool.name(), "directory_tree");
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn input_schema_valid() {
        let tool = DirectoryTreeTool::new(test_fs());
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["path"].is_object());
        assert_eq!(schema["required"][0], "path");
    }

    #[tokio::test]
    async fn missing_path_argument() {
        let tool = DirectoryTreeTool::new(test_fs());
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
        let tool = DirectoryTreeTool::new(test_fs());
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
        let tool = DirectoryTreeTool::new(test_fs());
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
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "content").unwrap();
        std::fs::write(dir.path().join("src").join("main.rs"), "fn main(){}").unwrap();

        let tool = DirectoryTreeTool::new(test_fs());
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

        let tool = DirectoryTreeTool::new(test_fs());
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

        let tool = DirectoryTreeTool::new(test_fs());
        let input = ToolInput {
            tool_use_id: "test".into(),
            arguments: json!({"path": dir.path().to_str().unwrap()}),
            working_directory: "/tmp".into(),
        };

        let result = tool.execute(input).await.unwrap();
        assert!(result.content.contains("visible/"));
        assert!(!result.content.contains(".hidden"));
    }

    #[tokio::test]
    async fn tree_connectors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("aaa.txt"), "").unwrap();
        std::fs::write(dir.path().join("zzz.txt"), "").unwrap();

        let tool = DirectoryTreeTool::new(test_fs());
        let input = ToolInput {
            tool_use_id: "test".into(),
            arguments: json!({"path": dir.path().to_str().unwrap(), "depth": 3}),
            working_directory: "/tmp".into(),
        };

        let result = tool.execute(input).await.unwrap();
        assert!(result.content.contains("├──"));
        assert!(result.content.contains("└──"));
        let meta = result.metadata.unwrap();
        assert_eq!(meta["entries"], 2);
    }
}
