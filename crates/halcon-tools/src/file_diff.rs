//! FileDiffTool — compare two files or directory listings for differences.
//!
//! Features:
//! - Line-by-line unified diff between two files
//! - Side-by-side comparison mode
//! - Directory diff: compare file lists between two directories
//! - Stats: additions, deletions, unchanged lines
//! - Context lines control (default 3)
//! - Binary file detection

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};
use std::collections::HashSet;

pub struct FileDiffTool;

impl FileDiffTool {
    pub fn new() -> Self {
        Self
    }

    /// Compute a simple line-level diff between two texts, returning hunks.
    fn compute_diff<'a>(old: &'a str, new: &'a str, context: usize) -> Vec<DiffHunk> {
        let old_lines: Vec<&str> = old.lines().collect();
        let new_lines: Vec<&str> = new.lines().collect();

        // Myers-like LCS: simple forward DP
        let m = old_lines.len();
        let n = new_lines.len();

        // Build LCS table
        let mut dp = vec![vec![0usize; n + 1]; m + 1];
        for i in (0..m).rev() {
            for j in (0..n).rev() {
                if old_lines[i] == new_lines[j] {
                    dp[i][j] = 1 + dp[i + 1][j + 1];
                } else {
                    dp[i][j] = dp[i + 1][j].max(dp[i][j + 1]);
                }
            }
        }

        // Traceback to get edit operations
        let mut ops: Vec<DiffOp> = vec![];
        let mut i = 0;
        let mut j = 0;
        while i < m || j < n {
            if i < m && j < n && old_lines[i] == new_lines[j] {
                ops.push(DiffOp::Equal(i, ()));
                i += 1;
                j += 1;
            } else if j < n && (i >= m || dp[i + 1][j] <= dp[i][j + 1]) {
                ops.push(DiffOp::Insert(j));
                j += 1;
            } else {
                ops.push(DiffOp::Remove(i));
                i += 1;
            }
        }

        // Group ops into hunks with context
        Self::group_into_hunks(ops, &old_lines, &new_lines, context)
    }

    fn group_into_hunks(ops: Vec<DiffOp>, old: &[&str], new: &[&str], ctx: usize) -> Vec<DiffHunk> {
        if ops.is_empty() {
            return vec![];
        }

        // Find change positions
        let mut change_positions: Vec<usize> = vec![];
        for (pos, op) in ops.iter().enumerate() {
            if !matches!(op, DiffOp::Equal(..)) {
                change_positions.push(pos);
            }
        }

        if change_positions.is_empty() {
            return vec![];
        }

        // Build hunk ranges
        let mut ranges: Vec<(usize, usize)> = vec![];
        let mut start = change_positions[0].saturating_sub(ctx);
        let mut end = (change_positions[0] + ctx + 1).min(ops.len());

        for &pos in &change_positions[1..] {
            let new_start = pos.saturating_sub(ctx);
            if new_start <= end {
                end = (pos + ctx + 1).min(ops.len());
            } else {
                ranges.push((start, end));
                start = new_start;
                end = (pos + ctx + 1).min(ops.len());
            }
        }
        ranges.push((start, end));

        // Build hunks
        let mut hunks = vec![];

        // Pre-compute line numbers for each op
        let mut op_old = vec![0usize; ops.len()];
        let mut op_new = vec![0usize; ops.len()];
        let mut ol = 1usize;
        let mut nl = 1usize;
        for (i, op) in ops.iter().enumerate() {
            op_old[i] = ol;
            op_new[i] = nl;
            match op {
                DiffOp::Equal(..) => {
                    ol += 1;
                    nl += 1;
                }
                DiffOp::Remove(..) => {
                    ol += 1;
                }
                DiffOp::Insert(..) => {
                    nl += 1;
                }
            }
        }

        for (range_start, range_end) in ranges {
            let mut lines = vec![];
            let old_start = op_old[range_start];
            let new_start = op_new[range_start];
            let mut adds = 0usize;
            let mut dels = 0usize;

            for op in &ops[range_start..range_end] {
                match op {
                    DiffOp::Equal(oi, _) => {
                        lines.push(HunkLine::Context(old[*oi].to_string()));
                    }
                    DiffOp::Remove(oi) => {
                        lines.push(HunkLine::Remove(old[*oi].to_string()));
                        dels += 1;
                    }
                    DiffOp::Insert(ni) => {
                        lines.push(HunkLine::Add(new[*ni].to_string()));
                        adds += 1;
                    }
                }
            }

            let old_count = lines
                .iter()
                .filter(|l| !matches!(l, HunkLine::Add(_)))
                .count();
            let new_count = lines
                .iter()
                .filter(|l| !matches!(l, HunkLine::Remove(_)))
                .count();

            hunks.push(DiffHunk {
                old_start,
                old_count,
                new_start,
                new_count,
                lines,
                adds,
                dels,
            });
        }
        hunks
    }

    fn format_unified(a_path: &str, b_path: &str, hunks: &[DiffHunk]) -> String {
        if hunks.is_empty() {
            return format!("Files '{}' and '{}' are identical.\n", a_path, b_path);
        }
        let mut out = format!("--- {a_path}\n+++ {b_path}\n");
        for h in hunks {
            out.push_str(&format!(
                "@@ -{},{} +{},{} @@\n",
                h.old_start, h.old_count, h.new_start, h.new_count
            ));
            for line in &h.lines {
                match line {
                    HunkLine::Context(s) => out.push_str(&format!(" {s}\n")),
                    HunkLine::Remove(s) => out.push_str(&format!("-{s}\n")),
                    HunkLine::Add(s) => out.push_str(&format!("+{s}\n")),
                }
            }
        }
        out
    }

    fn diff_stats(hunks: &[DiffHunk]) -> (usize, usize) {
        let adds: usize = hunks.iter().map(|h| h.adds).sum();
        let dels: usize = hunks.iter().map(|h| h.dels).sum();
        (adds, dels)
    }

    fn is_binary(bytes: &[u8]) -> bool {
        bytes.iter().take(8192).any(|&b| b == 0)
    }
}

impl Default for FileDiffTool {
    fn default() -> Self {
        Self::new()
    }
}

enum DiffOp {
    Equal(usize, ()),
    Remove(usize),
    Insert(usize),
}

enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

struct DiffHunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<HunkLine>,
    adds: usize,
    dels: usize,
}

#[async_trait]
impl Tool for FileDiffTool {
    fn name(&self) -> &str {
        "file_diff"
    }

    fn description(&self) -> &str {
        "Compare two files or directory listings for differences. Produces unified diff output \
         with addition/deletion counts. Supports context line control, directory comparison \
         (list of added/removed/common files), and binary file detection."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_a": {
                    "type": "string",
                    "description": "Path to the first file (original)."
                },
                "file_b": {
                    "type": "string",
                    "description": "Path to the second file (modified)."
                },
                "content_a": {
                    "type": "string",
                    "description": "Content of file A (use instead of file_a)."
                },
                "content_b": {
                    "type": "string",
                    "description": "Content of file B (use instead of file_b)."
                },
                "mode": {
                    "type": "string",
                    "enum": ["unified", "stats", "dirs"],
                    "description": "Mode: 'unified' (default, full diff), 'stats' (counts only), 'dirs' (compare directory file lists)."
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Number of context lines around changes (default: 3)."
                },
                "output": {
                    "type": "string",
                    "enum": ["text", "json"],
                    "description": "Output format (default: text)."
                }
            },
            "required": []
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(
        &self,
        input: ToolInput,
    ) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let args = &input.arguments;
        let mode = args["mode"].as_str().unwrap_or("unified");
        let context = args["context_lines"].as_u64().unwrap_or(3) as usize;
        let output_fmt = args["output"].as_str().unwrap_or("text");

        // Directory diff mode
        if mode == "dirs" {
            let dir_a = match args["file_a"].as_str() {
                Some(p) => p,
                None => {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: "Provide 'file_a' (directory A path) for dirs mode.".into(),
                        is_error: true,
                        metadata: None,
                    })
                }
            };
            let dir_b = match args["file_b"].as_str() {
                Some(p) => p,
                None => {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: "Provide 'file_b' (directory B path) for dirs mode.".into(),
                        is_error: true,
                        metadata: None,
                    })
                }
            };

            let read_dir = |path: &str| -> std::io::Result<HashSet<String>> {
                let mut files = HashSet::new();
                for entry in std::fs::read_dir(path)? {
                    let e = entry?;
                    if let Some(name) = e.file_name().to_str() {
                        files.insert(name.to_string());
                    }
                }
                Ok(files)
            };

            let set_a = match read_dir(dir_a) {
                Ok(s) => s,
                Err(e) => {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: format!("Cannot read directory '{}': {}", dir_a, e),
                        is_error: true,
                        metadata: None,
                    })
                }
            };
            let set_b = match read_dir(dir_b) {
                Ok(s) => s,
                Err(e) => {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: format!("Cannot read directory '{}': {}", dir_b, e),
                        is_error: true,
                        metadata: None,
                    })
                }
            };

            let mut only_a: Vec<&str> = set_a
                .iter()
                .filter(|f| !set_b.contains(*f))
                .map(|f| f.as_str())
                .collect();
            let mut only_b: Vec<&str> = set_b
                .iter()
                .filter(|f| !set_a.contains(*f))
                .map(|f| f.as_str())
                .collect();
            let mut common: Vec<&str> = set_a
                .iter()
                .filter(|f| set_b.contains(*f))
                .map(|f| f.as_str())
                .collect();
            only_a.sort_unstable();
            only_b.sort_unstable();
            common.sort_unstable();

            let content = if output_fmt == "json" {
                serde_json::to_string_pretty(&json!({
                    "dir_a": dir_a, "dir_b": dir_b,
                    "only_in_a": only_a, "only_in_b": only_b,
                    "common": common
                }))
                .unwrap_or_default()
            } else {
                let mut out = format!("## Directory Diff\n\n`{}` vs `{}`\n\n", dir_a, dir_b);
                if !only_a.is_empty() {
                    out.push_str(&format!("**Only in A** ({}):\n", only_a.len()));
                    for f in &only_a {
                        out.push_str(&format!("  - {f}\n"));
                    }
                    out.push('\n');
                }
                if !only_b.is_empty() {
                    out.push_str(&format!("**Only in B** ({}):\n", only_b.len()));
                    for f in &only_b {
                        out.push_str(&format!("  + {f}\n"));
                    }
                    out.push('\n');
                }
                out.push_str(&format!("**Common**: {} files\n", common.len()));
                out
            };

            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content,
                is_error: false,
                metadata: Some(
                    json!({ "only_a": only_a.len(), "only_b": only_b.len(), "common": common.len() }),
                ),
            });
        }

        // File content diff
        let (content_a, label_a) = if let Some(c) = args["content_a"].as_str() {
            (c.to_string(), "a".to_string())
        } else if let Some(p) = args["file_a"].as_str() {
            match tokio::fs::read(p).await {
                Ok(bytes) => {
                    if Self::is_binary(&bytes) {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!("'{}' appears to be a binary file.", p),
                            is_error: true,
                            metadata: None,
                        });
                    }
                    (String::from_utf8_lossy(&bytes).to_string(), p.to_string())
                }
                Err(e) => {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: format!("Cannot read '{}': {}", p, e),
                        is_error: true,
                        metadata: None,
                    })
                }
            }
        } else {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "Provide 'file_a' or 'content_a'.".into(),
                is_error: true,
                metadata: None,
            });
        };

        let (content_b, label_b) = if let Some(c) = args["content_b"].as_str() {
            (c.to_string(), "b".to_string())
        } else if let Some(p) = args["file_b"].as_str() {
            match tokio::fs::read(p).await {
                Ok(bytes) => {
                    if Self::is_binary(&bytes) {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!("'{}' appears to be a binary file.", p),
                            is_error: true,
                            metadata: None,
                        });
                    }
                    (String::from_utf8_lossy(&bytes).to_string(), p.to_string())
                }
                Err(e) => {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: format!("Cannot read '{}': {}", p, e),
                        is_error: true,
                        metadata: None,
                    })
                }
            }
        } else {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "Provide 'file_b' or 'content_b'.".into(),
                is_error: true,
                metadata: None,
            });
        };

        let hunks = Self::compute_diff(&content_a, &content_b, context);
        let (adds, dels) = Self::diff_stats(&hunks);
        let identical = hunks.is_empty();

        let content = if mode == "stats" {
            if output_fmt == "json" {
                serde_json::to_string_pretty(&json!({
                    "identical": identical,
                    "additions": adds,
                    "deletions": dels,
                    "hunks": hunks.len()
                }))
                .unwrap_or_default()
            } else if identical {
                format!("Files '{}' and '{}' are identical.\n", label_a, label_b)
            } else {
                format!(
                    "## Diff Stats: {} vs {}\n\n+{} additions, -{} deletions, {} hunks\n",
                    label_a,
                    label_b,
                    adds,
                    dels,
                    hunks.len()
                )
            }
        } else if output_fmt == "json" {
            let hunks_json: Vec<Value> = hunks.iter().map(|h| {
                let lines_json: Vec<Value> = h.lines.iter().map(|l| match l {
                    HunkLine::Context(s) => json!({ "type": "context", "text": s }),
                    HunkLine::Remove(s) => json!({ "type": "remove", "text": s }),
                    HunkLine::Add(s) => json!({ "type": "add", "text": s }),
                }).collect();
                json!({ "old_start": h.old_start, "new_start": h.new_start, "lines": lines_json })
            }).collect();
            serde_json::to_string_pretty(&json!({
                "identical": identical,
                "additions": adds,
                "deletions": dels,
                "hunks": hunks_json
            }))
            .unwrap_or_default()
        } else {
            let diff_text = Self::format_unified(&label_a, &label_b, &hunks);
            if identical {
                diff_text
            } else {
                format!(
                    "{}\n**Stats**: +{} -{} in {} hunk(s)\n",
                    diff_text,
                    adds,
                    dels,
                    hunks.len()
                )
            }
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "identical": identical,
                "additions": adds,
                "deletions": dels,
                "hunks": hunks.len()
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::ToolInput;

    fn ti(args: Value) -> ToolInput {
        ToolInput {
            tool_use_id: "t".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[test]
    fn tool_metadata() {
        let t = FileDiffTool::new();
        assert_eq!(t.name(), "file_diff");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let s = t.input_schema();
        assert_eq!(s["type"], "object");
        assert!(s["required"].is_array());
    }

    #[tokio::test]
    async fn identical_files() {
        let t = FileDiffTool::new();
        let out = t
            .execute(ti(json!({
                "content_a": "line1\nline2\nline3",
                "content_b": "line1\nline2\nline3",
                "mode": "unified"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert_eq!(
            out.metadata.as_ref().and_then(|m| m["identical"].as_bool()),
            Some(true)
        );
    }

    #[tokio::test]
    async fn single_line_added() {
        let t = FileDiffTool::new();
        let out = t
            .execute(ti(json!({
                "content_a": "line1\nline2",
                "content_b": "line1\nline2\nline3",
                "mode": "stats"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert_eq!(
            out.metadata.as_ref().and_then(|m| m["additions"].as_u64()),
            Some(1)
        );
        assert_eq!(
            out.metadata.as_ref().and_then(|m| m["deletions"].as_u64()),
            Some(0)
        );
    }

    #[tokio::test]
    async fn single_line_removed() {
        let t = FileDiffTool::new();
        let out = t
            .execute(ti(json!({
                "content_a": "line1\nline2\nline3",
                "content_b": "line1\nline3",
                "mode": "stats"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert_eq!(
            out.metadata.as_ref().and_then(|m| m["deletions"].as_u64()),
            Some(1)
        );
    }

    #[tokio::test]
    async fn unified_diff_output() {
        let t = FileDiffTool::new();
        let out = t
            .execute(ti(json!({
                "content_a": "a\nb\nc",
                "content_b": "a\nB\nc",
                "mode": "unified"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("-b") || out.content.contains("+B"));
    }

    #[tokio::test]
    async fn json_output() {
        let t = FileDiffTool::new();
        let out = t
            .execute(ti(json!({
                "content_a": "old",
                "content_b": "new",
                "output": "json"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        let v: Value = serde_json::from_str(&out.content).unwrap();
        assert!(v["additions"].as_u64().is_some());
    }

    #[tokio::test]
    async fn missing_inputs_error() {
        let t = FileDiffTool::new();
        let out = t.execute(ti(json!({ "mode": "unified" }))).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn multiple_changes() {
        let t = FileDiffTool::new();
        let a = "line1\nline2\nline3\nline4\nline5";
        let b = "line1\nLINE2\nline3\nLINE4\nline5";
        let out = t
            .execute(ti(json!({
                "content_a": a,
                "content_b": b,
                "mode": "stats"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert_eq!(
            out.metadata.as_ref().and_then(|m| m["additions"].as_u64()),
            Some(2)
        );
        assert_eq!(
            out.metadata.as_ref().and_then(|m| m["deletions"].as_u64()),
            Some(2)
        );
    }
}
