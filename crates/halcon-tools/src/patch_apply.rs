//! PatchApplyTool — apply unified diff patches to files or directories.
//!
//! Supports:
//! - Standard unified diff format (`diff -u` / `git diff` output)
//! - Dry-run mode (show what would change without writing)
//! - Context validation (verify surrounding lines match)
//! - Multiple hunks per file, multiple files per patch
//! - Reverse patch application (undo a patch)

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct PatchApplyTool;

impl PatchApplyTool {
    pub fn new() -> Self {
        Self
    }

    /// Parse a unified diff patch into file patches.
    fn parse_patch(patch: &str) -> Vec<FilePatch> {
        let mut file_patches = vec![];
        let mut current: Option<FilePatch> = None;
        let mut current_hunk: Option<Hunk> = None;

        for line in patch.lines() {
            if line.starts_with("--- ") {
                // Start of a new file patch
                if let Some(mut fp) = current.take() {
                    if let Some(h) = current_hunk.take() {
                        fp.hunks.push(h);
                    }
                    file_patches.push(fp);
                }
                let path = line
                    .trim_start_matches("--- ")
                    .trim_start_matches("a/")
                    .to_string();
                let path = path.split('\t').next().unwrap_or(&path).to_string();
                current = Some(FilePatch {
                    from: path,
                    to: String::new(),
                    hunks: vec![],
                });
            } else if line.starts_with("+++ ") {
                if let Some(ref mut fp) = current {
                    let path = line
                        .trim_start_matches("+++ ")
                        .trim_start_matches("b/")
                        .to_string();
                    fp.to = path.split('\t').next().unwrap_or(&path).to_string();
                }
            } else if line.starts_with("@@ ") {
                // New hunk
                if let Some(ref mut fp) = current {
                    if let Some(h) = current_hunk.take() {
                        fp.hunks.push(h);
                    }
                    // Parse @@ -old_start,old_count +new_start,new_count @@
                    let (old_start, old_count, new_start, new_count) =
                        Self::parse_hunk_header(line);
                    current_hunk = Some(Hunk {
                        old_start,
                        old_count,
                        new_start,
                        new_count,
                        lines: vec![],
                    });
                }
            } else if let Some(ref mut hunk) = current_hunk {
                if let Some(stripped) = line.strip_prefix('-') {
                    hunk.lines.push(HunkLine::Remove(stripped.to_string()));
                } else if let Some(stripped) = line.strip_prefix('+') {
                    hunk.lines.push(HunkLine::Add(stripped.to_string()));
                } else if line.starts_with(' ') || line.is_empty() {
                    hunk.lines
                        .push(HunkLine::Context(line.get(1..).unwrap_or("").to_string()));
                }
            }
        }

        // Flush the last file/hunk
        if let Some(mut fp) = current {
            if let Some(h) = current_hunk {
                fp.hunks.push(h);
            }
            file_patches.push(fp);
        }

        file_patches
    }

    fn parse_hunk_header(header: &str) -> (usize, usize, usize, usize) {
        // @@ -old_start,old_count +new_start,new_count @@ [context]
        let after_at = header.trim_start_matches('@').trim_start_matches(' ');
        let parts: Vec<&str> = after_at.split_whitespace().take(2).collect();

        let parse_range = |s: &str| -> (usize, usize) {
            let s = s.trim_start_matches('-').trim_start_matches('+');
            if let Some((start, count)) = s.split_once(',') {
                (start.parse().unwrap_or(1), count.parse().unwrap_or(1))
            } else {
                (s.parse().unwrap_or(1), 1)
            }
        };

        let (old_start, old_count) = parts.first().map(|s| parse_range(s)).unwrap_or((1, 0));
        let (new_start, new_count) = parts.get(1).map(|s| parse_range(s)).unwrap_or((1, 0));
        (old_start, old_count, new_start, new_count)
    }

    /// Apply a single file patch to file content.
    fn apply_hunk(
        content_lines: &[String],
        hunk: &Hunk,
        reverse: bool,
    ) -> Result<Vec<String>, String> {
        let mut result = vec![];
        let start = if hunk.old_start > 0 {
            hunk.old_start - 1
        } else {
            0
        };

        // Lines before the hunk
        for line in content_lines.iter().take(start.min(content_lines.len())) {
            result.push(line.clone());
        }

        let mut content_idx = start;
        for hl in &hunk.lines {
            match hl {
                HunkLine::Context(l) => {
                    // Verify context matches
                    if let Some(existing) = content_lines.get(content_idx) {
                        if existing != l {
                            return Err(format!(
                                "Context mismatch at line {}: expected {:?}, found {:?}",
                                content_idx + 1,
                                l,
                                existing
                            ));
                        }
                        result.push(existing.clone());
                        content_idx += 1;
                    }
                }
                HunkLine::Remove(l) => {
                    if reverse {
                        // Reverse: treat removes as adds
                        result.push(l.clone());
                    } else {
                        // Normal: skip this line (remove it)
                        if let Some(existing) = content_lines.get(content_idx) {
                            if existing != l {
                                return Err(format!(
                                    "Remove mismatch at line {}: expected {:?}, found {:?}",
                                    content_idx + 1,
                                    l,
                                    existing
                                ));
                            }
                            content_idx += 1; // skip/consume the line
                        }
                    }
                }
                HunkLine::Add(l) => {
                    if reverse {
                        // Reverse: treat adds as removes (consume without writing)
                        if let Some(existing) = content_lines.get(content_idx) {
                            if existing != l {
                                return Err(format!(
                                    "Reverse add mismatch at line {}: expected {:?}, found {:?}",
                                    content_idx + 1,
                                    l,
                                    existing
                                ));
                            }
                            content_idx += 1;
                        }
                    } else {
                        // Normal: insert this new line
                        result.push(l.clone());
                    }
                }
            }
        }

        // Lines after the hunk
        for line in content_lines.iter().skip(content_idx) {
            result.push(line.clone());
        }

        Ok(result)
    }

    fn apply_file_patch(content: &str, fp: &FilePatch, reverse: bool) -> Result<String, String> {
        let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        let had_trailing_newline = content.ends_with('\n');

        for hunk in &fp.hunks {
            lines = Self::apply_hunk(&lines, hunk, reverse)?;
        }

        let mut result = lines.join("\n");
        if had_trailing_newline || !result.is_empty() {
            result.push('\n');
        }
        Ok(result)
    }

    fn diff_stats(fp: &FilePatch) -> (usize, usize) {
        let mut adds = 0;
        let mut removes = 0;
        for hunk in &fp.hunks {
            for hl in &hunk.lines {
                match hl {
                    HunkLine::Add(_) => adds += 1,
                    HunkLine::Remove(_) => removes += 1,
                    _ => {}
                }
            }
        }
        (adds, removes)
    }
}

struct FilePatch {
    from: String,
    to: String,
    hunks: Vec<Hunk>,
}

#[allow(dead_code)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<HunkLine>,
}

enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

impl Default for PatchApplyTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for PatchApplyTool {
    fn name(&self) -> &str {
        "patch_apply"
    }

    fn description(&self) -> &str {
        "Apply unified diff patches to files. Supports standard `diff -u` and `git diff` format. \
         Validates context lines before applying. Supports dry-run mode to preview changes, \
         reverse mode to undo patches, and multiple files per patch."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Unified diff patch content to apply."
                },
                "patch_file": {
                    "type": "string",
                    "description": "Path to a .patch or .diff file."
                },
                "base_path": {
                    "type": "string",
                    "description": "Base directory for resolving file paths in the patch (default: current)."
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "If true, show what would change without modifying files (default: false)."
                },
                "reverse": {
                    "type": "boolean",
                    "description": "Apply the patch in reverse (undo a patch). Default: false."
                },
                "strip": {
                    "type": "integer",
                    "description": "Number of leading path components to strip (like patch -p). Default: 1."
                }
            },
            "required": []
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadWrite
    }

    fn requires_confirmation(&self, input: &halcon_core::types::ToolInput) -> bool {
        !input.arguments["dry_run"].as_bool().unwrap_or(false)
    }

    async fn execute_inner(
        &self,
        input: ToolInput,
    ) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let args = &input.arguments;
        let dry_run = args["dry_run"].as_bool().unwrap_or(false);
        let reverse = args["reverse"].as_bool().unwrap_or(false);
        let base_path = args["base_path"]
            .as_str()
            .unwrap_or(&input.working_directory);
        let strip = args["strip"].as_u64().unwrap_or(1) as usize;

        // Get patch content
        let patch_content = if let Some(p) = args["patch"].as_str() {
            p.to_string()
        } else if let Some(pf) = args["patch_file"].as_str() {
            match tokio::fs::read_to_string(pf).await {
                Ok(c) => c,
                Err(e) => {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: format!("Failed to read patch file '{pf}': {e}"),
                        is_error: true,
                        metadata: None,
                    });
                }
            }
        } else {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "Provide 'patch' (inline content) or 'patch_file' (path to .patch file)."
                    .into(),
                is_error: true,
                metadata: None,
            });
        };

        let file_patches = Self::parse_patch(&patch_content);

        if file_patches.is_empty() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "No file patches found in the provided patch content.".into(),
                is_error: false,
                metadata: None,
            });
        }

        let mut report = String::new();
        let mut applied_count = 0;
        let mut error_count = 0;
        let mut results: HashMap<String, Value> = HashMap::new();

        for fp in &file_patches {
            // Determine target file path
            let target_name = if reverse { &fp.from } else { &fp.to };
            let stripped = if strip > 0 {
                target_name
                    .splitn(strip + 1, '/')
                    .last()
                    .unwrap_or(target_name)
            } else {
                target_name
            };
            let full_path = format!("{}/{}", base_path.trim_end_matches('/'), stripped);

            let (adds, removes) = Self::diff_stats(fp);
            report.push_str(&format!("## {}\n", stripped));
            report.push_str(&format!(
                "  {} hunks, +{} -{} lines\n",
                fp.hunks.len(),
                adds,
                removes
            ));

            // Read source file
            let content = match tokio::fs::read_to_string(&full_path).await {
                Ok(c) => c,
                Err(e) => {
                    let msg = format!("  ❌ Cannot read '{}': {}\n", full_path, e);
                    report.push_str(&msg);
                    error_count += 1;
                    results.insert(stripped.to_string(), json!({ "error": e.to_string() }));
                    continue;
                }
            };

            // Apply
            match Self::apply_file_patch(&content, fp, reverse) {
                Ok(new_content) => {
                    if dry_run {
                        report.push_str("  ✅ Would apply (dry-run)\n");
                        // Show a brief diff preview
                        let orig_lines = content.lines().count();
                        let new_lines = new_content.lines().count();
                        let diff = new_lines as i64 - orig_lines as i64;
                        report.push_str(&format!(
                            "  Lines: {} → {} ({:+})\n",
                            orig_lines, new_lines, diff
                        ));
                    } else {
                        // Write new content
                        match tokio::fs::write(&full_path, &new_content).await {
                            Ok(_) => {
                                report.push_str("  ✅ Applied\n");
                                applied_count += 1;
                            }
                            Err(e) => {
                                report.push_str(&format!("  ❌ Write failed: {}\n", e));
                                error_count += 1;
                            }
                        }
                    }
                    if dry_run {
                        applied_count += 1;
                    }
                    results.insert(
                        stripped.to_string(),
                        json!({ "status": "ok", "adds": adds, "removes": removes }),
                    );
                }
                Err(e) => {
                    report.push_str(&format!("  ❌ Patch failed: {}\n", e));
                    error_count += 1;
                    results.insert(stripped.to_string(), json!({ "error": e }));
                }
            }
            report.push('\n');
        }

        let mode = if dry_run { " (dry-run)" } else { "" };
        let reverse_note = if reverse { " [reverse]" } else { "" };
        let header = format!(
            "# Patch Application{}{}\n\nFiles: {} total, {} applied, {} failed\n\n",
            mode,
            reverse_note,
            file_patches.len(),
            applied_count,
            error_count
        );

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: format!("{}{}", header, report),
            is_error: error_count > 0 && applied_count == 0,
            metadata: Some(json!({
                "files": file_patches.len(),
                "applied": applied_count,
                "errors": error_count,
                "dry_run": dry_run,
                "reverse": reverse
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let t = PatchApplyTool::new();
        assert_eq!(t.name(), "patch_apply");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadWrite);
    }

    #[test]
    fn requires_confirmation_when_not_dry_run() {
        let t = PatchApplyTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(t.requires_confirmation(&input));
    }

    #[test]
    fn no_confirmation_for_dry_run() {
        let t = PatchApplyTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "dry_run": true }),
            working_directory: "/tmp".into(),
        };
        assert!(!t.requires_confirmation(&input));
    }

    #[test]
    fn parse_empty_patch() {
        let fps = PatchApplyTool::parse_patch("");
        assert!(fps.is_empty());
    }

    #[test]
    fn parse_hunk_header_standard() {
        let (os, oc, ns, nc) = PatchApplyTool::parse_hunk_header("@@ -1,4 +1,5 @@ fn main() {");
        assert_eq!(os, 1);
        assert_eq!(oc, 4);
        assert_eq!(ns, 1);
        assert_eq!(nc, 5);
    }

    #[test]
    fn apply_hunk_add_line() {
        let content = vec!["line1".to_string(), "line3".to_string()];
        let hunk = Hunk {
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 2,
            lines: vec![
                HunkLine::Context("line1".to_string()),
                HunkLine::Add("line2".to_string()),
            ],
        };
        let result = PatchApplyTool::apply_hunk(&content, &hunk, false).unwrap();
        assert_eq!(result, vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn apply_hunk_remove_line() {
        let content = vec![
            "line1".to_string(),
            "line_to_remove".to_string(),
            "line3".to_string(),
        ];
        let hunk = Hunk {
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 1,
            lines: vec![
                HunkLine::Context("line1".to_string()),
                HunkLine::Remove("line_to_remove".to_string()),
            ],
        };
        let result = PatchApplyTool::apply_hunk(&content, &hunk, false).unwrap();
        assert_eq!(result, vec!["line1", "line3"]);
    }

    #[test]
    fn apply_hunk_context_mismatch_error() {
        let content = vec!["actual_line".to_string()];
        let hunk = Hunk {
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            lines: vec![HunkLine::Context("expected_different".to_string())],
        };
        let result = PatchApplyTool::apply_hunk(&content, &hunk, false);
        assert!(result.is_err());
    }

    #[test]
    fn full_patch_parse_and_apply() {
        let patch = "--- a/test.txt\n+++ b/test.txt\n@@ -1,2 +1,3 @@\n hello\n+world\n end\n";
        let fps = PatchApplyTool::parse_patch(patch);
        assert_eq!(fps.len(), 1);
        assert_eq!(fps[0].hunks.len(), 1);

        let content = "hello\nend\n";
        let result = PatchApplyTool::apply_file_patch(content, &fps[0], false).unwrap();
        assert!(result.contains("world"));
        assert!(result.contains("hello"));
        assert!(result.contains("end"));
    }

    #[tokio::test]
    async fn execute_no_patch_returns_error() {
        let tool = PatchApplyTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({}),
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn execute_dry_run_nonexistent_file() {
        let patch = "--- a/nonexistent_file_test_9999.txt\n+++ b/nonexistent_file_test_9999.txt\n@@ -1 +1,2 @@\n line1\n+line2\n";
        let tool = PatchApplyTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({ "patch": patch, "dry_run": true }),
                working_directory: "/tmp".into(),
            })
            .await
            .unwrap();
        // Should report an error for the file but not crash
        assert!(
            out.content.contains("Cannot read")
                || out.content.contains("failed")
                || out.content.contains("error")
        );
    }
}
