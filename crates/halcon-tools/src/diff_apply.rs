//! `diff_apply` tool: apply a unified diff patch to a file.
//!
//! Parses standard unified diff format (from `git diff`, `diff -u`, etc.)
//! and applies hunks with strict context verification.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

use crate::fs_service::FsService;

#[allow(unused_imports)]
use tracing::instrument;

pub struct DiffApplyTool {
    fs: Arc<FsService>,
}

impl DiffApplyTool {
    pub fn new(fs: Arc<FsService>) -> Self {
        Self { fs }
    }
}

// ─── Diff parsing ─────────────────────────────────────────────────────────────

#[derive(Debug)]
enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Debug)]
struct Hunk {
    orig_start: usize, // 1-based
    lines: Vec<HunkLine>,
}

/// Parse unified diff text into a list of hunks.
///
/// Skips `---`/`+++` file header lines. Each `@@ … @@` starts a new hunk.
fn parse_hunks(diff: &str) -> std::result::Result<Vec<Hunk>, String> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut current: Option<Hunk> = None;

    for line in diff.lines() {
        if line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("diff ")
            || line.starts_with("index ")
        {
            continue;
        }
        if line.starts_with("@@ ") {
            // Flush previous hunk
            if let Some(h) = current.take() {
                hunks.push(h);
            }
            // Parse: @@ -orig_start[,orig_count] +new_start[,new_count] @@
            let orig_start = parse_hunk_header(line)?;
            current = Some(Hunk {
                orig_start,
                lines: Vec::new(),
            });
        } else if let Some(ref mut h) = current {
            if let Some(stripped) = line.strip_prefix('-') {
                h.lines.push(HunkLine::Remove(stripped.to_string()));
            } else if let Some(stripped) = line.strip_prefix('+') {
                h.lines.push(HunkLine::Add(stripped.to_string()));
            } else if let Some(stripped) = line.strip_prefix(' ') {
                h.lines.push(HunkLine::Context(stripped.to_string()));
            } else if line == "\\ No newline at end of file" {
                // informational — skip
            }
        }
    }
    if let Some(h) = current {
        hunks.push(h);
    }
    Ok(hunks)
}

/// Extract the original start line from a hunk header like `@@ -12,5 +12,6 @@`.
fn parse_hunk_header(line: &str) -> std::result::Result<usize, String> {
    // Find "-N" after "@@"
    let after = line
        .split("@@")
        .nth(1)
        .ok_or_else(|| format!("Malformed hunk header: {line}"))?
        .trim();
    let orig_part = after
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("No orig range in hunk header: {line}"))?;
    // orig_part is like "-12,5" or "-12"
    let digits = orig_part
        .trim_start_matches('-')
        .split(',')
        .next()
        .unwrap_or("1");
    digits
        .parse::<usize>()
        .map_err(|_| format!("Cannot parse orig start from: {line}"))
}

/// Apply parsed hunks to the original file content.
fn apply_hunks(original: &str, hunks: &[Hunk]) -> std::result::Result<String, String> {
    if hunks.is_empty() {
        return Ok(original.to_string());
    }

    let orig_lines: Vec<&str> = original.lines().collect();
    let mut result: Vec<String> = Vec::new();
    let mut orig_pos = 0usize; // 0-indexed

    for hunk in hunks {
        let hunk_orig_start = hunk.orig_start.saturating_sub(1); // convert to 0-indexed

        // Copy unchanged lines before this hunk
        if hunk_orig_start > orig_pos {
            if hunk_orig_start > orig_lines.len() {
                return Err(format!(
                    "Hunk starts at line {} but file only has {} lines",
                    hunk.orig_start,
                    orig_lines.len()
                ));
            }
            for line in &orig_lines[orig_pos..hunk_orig_start] {
                result.push(line.to_string());
            }
            orig_pos = hunk_orig_start;
        }

        for hunk_line in &hunk.lines {
            match hunk_line {
                HunkLine::Context(expected) => {
                    let actual = orig_lines.get(orig_pos).copied().unwrap_or("");
                    if actual != expected.as_str() {
                        return Err(format!(
                            "Context mismatch at line {}: expected {:?}, got {:?}",
                            orig_pos + 1,
                            expected,
                            actual
                        ));
                    }
                    result.push(actual.to_string());
                    orig_pos += 1;
                }
                HunkLine::Remove(expected) => {
                    let actual = orig_lines.get(orig_pos).copied().unwrap_or("");
                    if actual != expected.as_str() {
                        return Err(format!(
                            "Remove mismatch at line {}: expected {:?}, got {:?}",
                            orig_pos + 1,
                            expected,
                            actual
                        ));
                    }
                    orig_pos += 1; // consume line, don't add to result
                }
                HunkLine::Add(line) => {
                    result.push(line.clone());
                }
            }
        }
    }

    // Copy any remaining lines
    for line in orig_lines.get(orig_pos..).unwrap_or(&[]) {
        result.push(line.to_string());
    }

    let mut out = result.join("\n");
    if original.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

// ─── Tool impl ────────────────────────────────────────────────────────────────

#[async_trait]
impl Tool for DiffApplyTool {
    fn name(&self) -> &str {
        "diff_apply"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to a file. \
         Supports standard unified diff format (output of `git diff` or `diff -u`). \
         The patch is applied with strict context verification — all context lines must match. \
         On success returns the number of hunks applied. On failure returns the mismatch details."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadWrite
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    #[tracing::instrument(skip(self), fields(tool = "diff_apply"))]
    async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
        let path = input.arguments["path"]
            .as_str()
            .ok_or_else(|| HalconError::InvalidInput("diff_apply requires 'path' string".into()))?;
        let diff = input.arguments["diff"]
            .as_str()
            .ok_or_else(|| HalconError::InvalidInput("diff_apply requires 'diff' string".into()))?;

        if diff.trim().is_empty() {
            return Err(HalconError::InvalidInput(
                "diff_apply: diff must not be empty".into(),
            ));
        }

        const MAX_DIFF_BYTES: usize = 10 * 1024 * 1024; // 10 MB
        if diff.len() > MAX_DIFF_BYTES {
            return Err(HalconError::InvalidInput(format!(
                "diff_apply: diff too large ({} bytes, max {MAX_DIFF_BYTES})",
                diff.len()
            )));
        }

        // Resolve path via FsService
        let resolved = self.fs.resolve_path(path, &input.working_directory)?;

        // Read current file content
        let original = self.fs.read_to_string(&resolved).await?;

        // Parse and apply the diff
        let hunks = parse_hunks(diff)
            .map_err(|e| HalconError::InvalidInput(format!("Failed to parse diff: {e}")))?;

        let hunk_count = hunks.len();
        if hunk_count == 0 {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "No hunks found in diff — file unchanged.".to_string(),
                is_error: false,
                metadata: Some(json!({ "hunks_applied": 0, "path": path })),
            });
        }

        let patched =
            apply_hunks(&original, &hunks).map_err(|e| HalconError::ToolExecutionFailed {
                tool: "diff_apply".to_string(),
                message: format!("Patch failed: {e}"),
            })?;

        // Write back atomically
        self.fs.atomic_write(&resolved, patched.as_bytes()).await?;

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: format!("Applied {hunk_count} hunk(s) to {path}"),
            is_error: false,
            metadata: Some(json!({
                "hunks_applied": hunk_count,
                "path": path,
                "bytes_written": patched.len(),
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to patch (relative to working directory or absolute)."
                },
                "diff": {
                    "type": "string",
                    "description": "Unified diff content (from git diff, diff -u, etc.). Must include @@ hunk headers."
                }
            },
            "required": ["path", "diff"]
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hunk_header_basic() {
        assert_eq!(parse_hunk_header("@@ -1,5 +1,6 @@").unwrap(), 1);
        assert_eq!(parse_hunk_header("@@ -12,3 +15,4 @@").unwrap(), 12);
        assert_eq!(
            parse_hunk_header("@@ -100 +100,2 @@ fn foo()").unwrap(),
            100
        );
    }

    #[test]
    fn apply_simple_addition() {
        let original = "line1\nline2\nline3\n";
        let diff = "@@ -1,3 +1,4 @@\n line1\n+added\n line2\n line3\n";
        let result = apply_hunks(original, &parse_hunks(diff).unwrap()).unwrap();
        assert_eq!(result, "line1\nadded\nline2\nline3\n");
    }

    #[test]
    fn apply_simple_removal() {
        let original = "line1\nline2\nline3\n";
        let diff = "@@ -1,3 +1,2 @@\n line1\n-line2\n line3\n";
        let result = apply_hunks(original, &parse_hunks(diff).unwrap()).unwrap();
        assert_eq!(result, "line1\nline3\n");
    }

    #[test]
    fn apply_replace() {
        let original = "fn old_name() {}\n";
        let diff = "@@ -1 +1 @@\n-fn old_name() {}\n+fn new_name() {}\n";
        let result = apply_hunks(original, &parse_hunks(diff).unwrap()).unwrap();
        assert_eq!(result, "fn new_name() {}\n");
    }

    #[test]
    fn context_mismatch_returns_error() {
        let original = "line1\nline2\nline3\n";
        let diff = "@@ -1,2 +1,2 @@\n wrong_context\n line2\n";
        let err = apply_hunks(original, &parse_hunks(diff).unwrap()).unwrap_err();
        assert!(err.contains("Context mismatch"), "err: {err}");
    }

    #[test]
    fn no_hunks_returns_original() {
        let original = "unchanged\n";
        let diff = "--- a/file\n+++ b/file\n";
        let result = apply_hunks(original, &parse_hunks(diff).unwrap()).unwrap();
        assert_eq!(result, "unchanged\n");
    }

    #[test]
    fn apply_mid_file_hunk() {
        let original = "a\nb\nc\nd\ne\n";
        let diff = "@@ -3,1 +3,2 @@\n-c\n+c1\n+c2\n";
        let result = apply_hunks(original, &parse_hunks(diff).unwrap()).unwrap();
        assert_eq!(result, "a\nb\nc1\nc2\nd\ne\n");
    }

    #[test]
    fn preserves_trailing_newline() {
        let original = "foo\n";
        let diff = "@@ -1 +1 @@\n-foo\n+bar\n";
        let result = apply_hunks(original, &parse_hunks(diff).unwrap()).unwrap();
        assert!(result.ends_with('\n'), "trailing newline must be preserved");
    }

    #[test]
    fn skips_diff_file_headers() {
        let diff = "diff --git a/f b/f\nindex abc..def 100644\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n";
        let hunks = parse_hunks(diff).unwrap();
        assert_eq!(hunks.len(), 1);
    }
}
