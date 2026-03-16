//! `archive` tool: create, extract, and inspect zip and tar archives.
//!
//! Delegates to system tools (`zip`, `unzip`, `tar`) so the binary stays
//! dependency-free.  The tool validates paths before executing any command to
//! prevent path-traversal attacks.
//!
//! Supported operations:
//! - `create`  — bundle files into a new archive (zip or tar/tar.gz/tar.bz2/tar.xz)
//! - `extract` — extract an archive into a destination directory
//! - `list`    — list contents without extracting

use async_trait::async_trait;
use serde_json::json;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

#[allow(unused_imports)]
use tracing::instrument;

const MAX_OUTPUT_BYTES: usize = 64 * 1024; // 64 KB for listing output

pub struct ArchiveTool;

impl ArchiveTool {
    pub fn new() -> Self { Self }
}

impl Default for ArchiveTool {
    fn default() -> Self { Self::new() }
}

// ─── Format detection ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum ArchiveFormat {
    Zip,
    TarGz,
    TarBz2,
    TarXz,
    Tar,
}

fn detect_format(path: &str) -> Option<ArchiveFormat> {
    let p = path.to_lowercase();
    if p.ends_with(".zip") { return Some(ArchiveFormat::Zip); }
    if p.ends_with(".tar.gz") || p.ends_with(".tgz") { return Some(ArchiveFormat::TarGz); }
    if p.ends_with(".tar.bz2") || p.ends_with(".tbz2") { return Some(ArchiveFormat::TarBz2); }
    if p.ends_with(".tar.xz") || p.ends_with(".txz") { return Some(ArchiveFormat::TarXz); }
    if p.ends_with(".tar") { return Some(ArchiveFormat::Tar); }
    None
}

fn infer_format(archive_path: &str, format_arg: Option<&str>) -> Result<ArchiveFormat> {
    if let Some(f) = format_arg {
        return match f {
            "zip" => Ok(ArchiveFormat::Zip),
            "tar.gz" | "tgz" => Ok(ArchiveFormat::TarGz),
            "tar.bz2" | "tbz2" => Ok(ArchiveFormat::TarBz2),
            "tar.xz" | "txz" => Ok(ArchiveFormat::TarXz),
            "tar" => Ok(ArchiveFormat::Tar),
            other => Err(HalconError::InvalidInput(format!(
                "archive: unknown format '{other}'. Use: zip, tar.gz, tar.bz2, tar.xz, tar"
            ))),
        };
    }
    detect_format(archive_path).ok_or_else(|| {
        HalconError::InvalidInput(format!(
            "archive: cannot determine format from '{archive_path}' — \
             add extension (.zip/.tar.gz/.tar.bz2/.tar.xz/.tar) or pass 'format'."
        ))
    })
}

// ─── Path validation ──────────────────────────────────────────────────────────

fn validate_path(path: &str, label: &str) -> Result<()> {
    if path.contains("..") {
        return Err(HalconError::InvalidInput(format!(
            "archive: {label} path '{path}' contains '..' — path traversal rejected"
        )));
    }
    Ok(())
}

// ─── Command runner ───────────────────────────────────────────────────────────

async fn run_cmd(
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
            format!("'{program}' not found — is it installed?")
        } else {
            format!("failed to run '{program}': {e}")
        }
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((stdout, stderr, output.status.code().unwrap_or(-1)))
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

// ─── Operations ───────────────────────────────────────────────────────────────

async fn op_create(
    archive_path: &str,
    sources: &[&str],
    format: &ArchiveFormat,
    working_dir: &str,
    timeout_secs: u64,
) -> ToolOutput {
    if let Err(e) = validate_path(archive_path, "archive") {
        return ToolOutput {
            tool_use_id: "archive".into(),
            content: e.to_string(),
            is_error: true,
            metadata: None,
        };
    }

    let result: std::result::Result<(String, String, i32), String> = match format {
        ArchiveFormat::Zip => {
            let mut args = vec![archive_path];
            args.extend_from_slice(sources);
            run_cmd("zip", &args, working_dir, timeout_secs).await
        }
        ArchiveFormat::TarGz => {
            let mut args = vec!["-czf", archive_path];
            args.extend_from_slice(sources);
            run_cmd("tar", &args, working_dir, timeout_secs).await
        }
        ArchiveFormat::TarBz2 => {
            let mut args = vec!["-cjf", archive_path];
            args.extend_from_slice(sources);
            run_cmd("tar", &args, working_dir, timeout_secs).await
        }
        ArchiveFormat::TarXz => {
            let mut args = vec!["-cJf", archive_path];
            args.extend_from_slice(sources);
            run_cmd("tar", &args, working_dir, timeout_secs).await
        }
        ArchiveFormat::Tar => {
            let mut args = vec!["-cf", archive_path];
            args.extend_from_slice(sources);
            run_cmd("tar", &args, working_dir, timeout_secs).await
        }
    };

    match result {
        Ok((_, stderr, code)) if code == 0 => ToolOutput {
            tool_use_id: "archive".into(),
            content: format!("Created '{archive_path}' successfully."),
            is_error: false,
            metadata: Some(json!({ "operation": "create", "archive": archive_path })),
        },
        Ok((_, stderr, code)) => ToolOutput {
            tool_use_id: "archive".into(),
            content: format!("Failed to create archive (exit {code}):\n{}", stderr.trim()),
            is_error: true,
            metadata: Some(json!({ "exit_code": code })),
        },
        Err(e) => ToolOutput {
            tool_use_id: "archive".into(),
            content: format!("Error: {e}"),
            is_error: true,
            metadata: None,
        },
    }
}

async fn op_extract(
    archive_path: &str,
    dest: &str,
    format: &ArchiveFormat,
    working_dir: &str,
    timeout_secs: u64,
) -> ToolOutput {
    if let Err(e) = validate_path(archive_path, "archive") {
        return ToolOutput {
            tool_use_id: "archive".into(),
            content: e.to_string(),
            is_error: true,
            metadata: None,
        };
    }
    if let Err(e) = validate_path(dest, "destination") {
        return ToolOutput {
            tool_use_id: "archive".into(),
            content: e.to_string(),
            is_error: true,
            metadata: None,
        };
    }

    // Create destination directory if needed.
    let _ = tokio::fs::create_dir_all(dest).await;

    let result: std::result::Result<(String, String, i32), String> = match format {
        ArchiveFormat::Zip => {
            run_cmd("unzip", &["-o", archive_path, "-d", dest], working_dir, timeout_secs).await
        }
        ArchiveFormat::TarGz => {
            run_cmd("tar", &["-xzf", archive_path, "-C", dest], working_dir, timeout_secs).await
        }
        ArchiveFormat::TarBz2 => {
            run_cmd("tar", &["-xjf", archive_path, "-C", dest], working_dir, timeout_secs).await
        }
        ArchiveFormat::TarXz => {
            run_cmd("tar", &["-xJf", archive_path, "-C", dest], working_dir, timeout_secs).await
        }
        ArchiveFormat::Tar => {
            run_cmd("tar", &["-xf", archive_path, "-C", dest], working_dir, timeout_secs).await
        }
    };

    match result {
        Ok((_, stderr, code)) if code == 0 => ToolOutput {
            tool_use_id: "archive".into(),
            content: format!("Extracted '{archive_path}' → '{dest}'."),
            is_error: false,
            metadata: Some(json!({ "operation": "extract", "archive": archive_path, "destination": dest })),
        },
        Ok((_, stderr, code)) => ToolOutput {
            tool_use_id: "archive".into(),
            content: format!("Extraction failed (exit {code}):\n{}", stderr.trim()),
            is_error: true,
            metadata: Some(json!({ "exit_code": code })),
        },
        Err(e) => ToolOutput {
            tool_use_id: "archive".into(),
            content: format!("Error: {e}"),
            is_error: true,
            metadata: None,
        },
    }
}

async fn op_list(
    archive_path: &str,
    format: &ArchiveFormat,
    working_dir: &str,
    timeout_secs: u64,
) -> ToolOutput {
    if let Err(e) = validate_path(archive_path, "archive") {
        return ToolOutput {
            tool_use_id: "archive".into(),
            content: e.to_string(),
            is_error: true,
            metadata: None,
        };
    }

    let result: std::result::Result<(String, String, i32), String> = match format {
        ArchiveFormat::Zip => {
            run_cmd("unzip", &["-l", archive_path], working_dir, timeout_secs).await
        }
        _ => {
            run_cmd("tar", &["-tvf", archive_path], working_dir, timeout_secs).await
        }
    };

    match result {
        Ok((stdout, stderr, code)) if code == 0 => {
            let listing = truncate(&stdout, MAX_OUTPUT_BYTES);
            let lines: usize = listing.lines().count();
            ToolOutput {
                tool_use_id: "archive".into(),
                content: format!("Contents of '{archive_path}' ({lines} entries):\n{listing}"),
                is_error: false,
                metadata: Some(json!({ "operation": "list", "archive": archive_path, "entry_count": lines })),
            }
        }
        Ok((_, stderr, code)) => ToolOutput {
            tool_use_id: "archive".into(),
            content: format!("List failed (exit {code}):\n{}", stderr.trim()),
            is_error: true,
            metadata: Some(json!({ "exit_code": code })),
        },
        Err(e) => ToolOutput {
            tool_use_id: "archive".into(),
            content: format!("Error: {e}"),
            is_error: true,
            metadata: None,
        },
    }
}

// ─── Tool impl ────────────────────────────────────────────────────────────────

#[async_trait]
impl Tool for ArchiveTool {
    fn name(&self) -> &str { "archive" }

    fn description(&self) -> &str {
        "Create, extract, or list archive files (zip, tar, tar.gz, tar.bz2, tar.xz). \
         Uses system tools (zip/unzip/tar). \
         Operations: 'create' (bundle files/directories), 'extract' (unpack to destination), \
         'list' (inspect contents without extracting). \
         Format is auto-detected from the archive path extension."
    }

    fn permission_level(&self) -> PermissionLevel { PermissionLevel::ReadWrite }

    fn requires_confirmation(&self, input: &ToolInput) -> bool {
        // Creating archives in non-temp locations or extracting over existing files
        // is considered non-destructive enough to skip confirmation.
        // Only require confirmation when overwriting is explicit (not implemented yet).
        false
    }

    #[tracing::instrument(skip(self), fields(tool = "archive"))]
    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let op = input
            .arguments
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("list");

        let archive_path = input
            .arguments
            .get("archive")
            .and_then(|v| v.as_str())
            .ok_or_else(|| HalconError::InvalidInput("archive: 'archive' path is required".into()))?;

        validate_path(archive_path, "archive")?;

        let format_arg = input.arguments.get("format").and_then(|v| v.as_str());
        let format = infer_format(archive_path, format_arg)?;

        let working_dir = &input.working_directory;
        let timeout_secs = 60u64;

        let mut out = match op {
            "list" => {
                op_list(archive_path, &format, working_dir, timeout_secs).await
            }
            "extract" => {
                let dest = input
                    .arguments
                    .get("destination")
                    .and_then(|v| v.as_str())
                    .unwrap_or(".");
                validate_path(dest, "destination")?;
                op_extract(archive_path, dest, &format, working_dir, timeout_secs).await
            }
            "create" => {
                let sources: Vec<&str> = input
                    .arguments
                    .get("sources")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                if sources.is_empty() {
                    return Err(HalconError::InvalidInput(
                        "archive: 'sources' array is required for 'create' operation".into(),
                    ));
                }

                for src in &sources {
                    validate_path(src, "source")?;
                }

                op_create(archive_path, &sources, &format, working_dir, timeout_secs).await
            }
            other => {
                return Err(HalconError::InvalidInput(format!(
                    "archive: unknown operation '{other}'. Use: create, extract, list"
                )));
            }
        };

        out.tool_use_id = input.tool_use_id;
        Ok(out)
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["create", "extract", "list"],
                    "description": "Archive operation (default: list)."
                },
                "archive": {
                    "type": "string",
                    "description": "Path to the archive file."
                },
                "sources": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files/directories to include (create only)."
                },
                "destination": {
                    "type": "string",
                    "description": "Extraction target directory (extract only, default: '.')."
                },
                "format": {
                    "type": "string",
                    "enum": ["zip", "tar.gz", "tar.bz2", "tar.xz", "tar"],
                    "description": "Override format detection (optional — inferred from extension by default)."
                }
            },
            "required": ["archive"]
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_meta() {
        let t = ArchiveTool::new();
        assert_eq!(t.name(), "archive");
        assert_eq!(t.permission_level(), PermissionLevel::ReadWrite);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["operation"].is_object());
        assert!(schema["required"].as_array().unwrap().contains(&json!("archive")));
    }

    #[test]
    fn detect_format_zip() {
        assert_eq!(detect_format("out.zip"), Some(ArchiveFormat::Zip));
    }

    #[test]
    fn detect_format_tar_gz() {
        assert_eq!(detect_format("out.tar.gz"), Some(ArchiveFormat::TarGz));
        assert_eq!(detect_format("out.tgz"), Some(ArchiveFormat::TarGz));
    }

    #[test]
    fn detect_format_tar_bz2() {
        assert_eq!(detect_format("out.tar.bz2"), Some(ArchiveFormat::TarBz2));
    }

    #[test]
    fn detect_format_tar_xz() {
        assert_eq!(detect_format("out.tar.xz"), Some(ArchiveFormat::TarXz));
    }

    #[test]
    fn detect_format_plain_tar() {
        assert_eq!(detect_format("out.tar"), Some(ArchiveFormat::Tar));
    }

    #[test]
    fn detect_format_unknown() {
        assert_eq!(detect_format("out.gz"), None);
        assert_eq!(detect_format("out.rar"), None);
    }

    #[test]
    fn infer_format_from_arg() {
        assert_eq!(infer_format("any.xyz", Some("zip")).unwrap(), ArchiveFormat::Zip);
        assert_eq!(infer_format("any.xyz", Some("tgz")).unwrap(), ArchiveFormat::TarGz);
        assert!(infer_format("any.xyz", Some("rar")).is_err());
    }

    #[test]
    fn validate_path_rejects_traversal() {
        assert!(validate_path("../evil.txt", "test").is_err());
        assert!(validate_path("a/../../evil", "test").is_err());
        assert!(validate_path("/safe/path.zip", "test").is_ok());
    }

    #[tokio::test]
    async fn missing_archive_arg_returns_error() {
        let t = ArchiveTool::new();
        let input = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({ "operation": "list" }),
            working_directory: "/tmp".into(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn unknown_operation_returns_error() {
        let t = ArchiveTool::new();
        let input = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({ "operation": "delete", "archive": "f.zip" }),
            working_directory: "/tmp".into(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn unknown_extension_without_format_returns_error() {
        let t = ArchiveTool::new();
        let input = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({ "operation": "list", "archive": "f.rar" }),
            working_directory: "/tmp".into(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn create_requires_sources() {
        let t = ArchiveTool::new();
        let input = ToolInput {
            tool_use_id: "x".into(),
            arguments: json!({ "operation": "create", "archive": "/tmp/out.tar.gz", "sources": [] }),
            working_directory: "/tmp".into(),
        };
        assert!(t.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn create_and_list_tar_gz() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();

        // Write a test file.
        tokio::fs::write(format!("{path}/hello.txt"), b"hello").await.unwrap();

        let archive = format!("{path}/test.tar.gz");
        let t = ArchiveTool::new();

        // Create.
        let create_in = ToolInput {
            tool_use_id: "c".into(),
            arguments: json!({ "operation": "create", "archive": &archive, "sources": ["hello.txt"] }),
            working_directory: path.to_string(),
        };
        let out = t.execute(create_in).await.unwrap();
        assert!(!out.is_error, "create failed: {}", out.content);
        assert!(std::path::Path::new(&archive).exists());

        // List.
        let list_in = ToolInput {
            tool_use_id: "l".into(),
            arguments: json!({ "operation": "list", "archive": &archive }),
            working_directory: path.to_string(),
        };
        let out = t.execute(list_in).await.unwrap();
        assert!(!out.is_error, "list failed: {}", out.content);
        assert!(out.content.contains("hello.txt"), "expected file in listing");
    }

    #[tokio::test]
    async fn extract_tar_gz() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();

        tokio::fs::write(format!("{path}/data.txt"), b"content").await.unwrap();

        let archive = format!("{path}/data.tar.gz");
        let dest = format!("{path}/extracted");

        let t = ArchiveTool::new();

        // Create.
        let _ = t.execute(ToolInput {
            tool_use_id: "c".into(),
            arguments: json!({ "operation": "create", "archive": &archive, "sources": ["data.txt"] }),
            working_directory: path.to_string(),
        }).await.unwrap();

        // Extract.
        let out = t.execute(ToolInput {
            tool_use_id: "e".into(),
            arguments: json!({ "operation": "extract", "archive": &archive, "destination": &dest }),
            working_directory: path.to_string(),
        }).await.unwrap();
        assert!(!out.is_error, "extract failed: {}", out.content);
        assert!(std::path::Path::new(&dest).join("data.txt").exists());
    }
}
