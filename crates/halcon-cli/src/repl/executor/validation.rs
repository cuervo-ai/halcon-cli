//! Tool argument validation: parse-error rejection, risk scoring, and path existence gates.
//!
//! All functions are pure or near-pure (path existence is the only I/O) and independently testable.

use halcon_core::types::PermissionLevel;

use crate::repl::output_risk_scorer;
use super::{make_error_result, CompletedToolUse, ToolExecResult};

/// Validate tool arguments: reject poisoned parse errors and high-risk args.
/// Returns Some(error_result) if the call must be blocked, None if safe to proceed.
pub(crate) fn validate_tool_args(tool_call: &CompletedToolUse) -> Option<ToolExecResult> {
    // RC-4: Reject malformed args from streaming parse failures.
    if let Some(parse_err) = tool_call.input.get("_parse_error") {
        let err_msg = parse_err.as_str().unwrap_or("unknown parse error");
        tracing::error!(
            tool = %tool_call.name,
            tool_use_id = %tool_call.id,
            parse_error = %err_msg,
            "Rejecting tool call with malformed arguments from streaming parse failure"
        );
        return Some(make_error_result(
            tool_call,
            format!(
                "Error: tool arguments were corrupted during streaming (parse error: {err_msg}). \
                 The model's tool call was truncated or malformed. Please retry."
            ),
        ));
    }
    // G3: Pre-execution risk scoring — block high-risk args before execution.
    let risk = output_risk_scorer::score_tool_args(&tool_call.name, &tool_call.input);
    if risk.is_high_risk() {
        tracing::warn!(
            tool = %tool_call.name,
            score = risk.score,
            flags = ?risk.flags,
            "Tool args blocked by pre-execution risk scorer (score >= 50)"
        );
        return Some(make_error_result(
            tool_call,
            format!(
                "[BLOCKED] High-risk tool arguments detected (score: {}/100). \
                 Flags: {:?}. The command was rejected by pre-execution risk scoring.",
                risk.score, risk.flags
            ),
        ));
    }
    None
}

// ── FASE-2: Pre-execution path existence invariant ────────────────────────────

/// Extract resolved path strings from a tool's JSON input for pre-existence validation.
///
/// Inspects `path`, `file_path`, `source_path` (single string) and `paths` (array or string).
/// Paths containing glob characters (`*`, `?`, `[`) are skipped — they are search patterns,
/// not concrete filesystem targets. Non-absolute paths are resolved against `working_dir`.
pub(crate) fn extract_path_args(input: &serde_json::Value, working_dir: &str) -> Vec<String> {
    let mut paths = Vec::new();

    for key in &["path", "file_path", "source_path"] {
        if let Some(s) = input.get(*key).and_then(|v| v.as_str()) {
            let s = s.trim();
            if !s.is_empty() && !s.contains(['*', '?', '[']) {
                paths.push(resolve_to_absolute(s, working_dir));
            }
        }
    }

    if let Some(v) = input.get("paths") {
        if let Some(arr) = v.as_array() {
            for item in arr {
                if let Some(s) = item.as_str() {
                    let s = s.trim();
                    if !s.is_empty() && !s.contains(['*', '?', '[']) {
                        paths.push(resolve_to_absolute(s, working_dir));
                    }
                }
            }
        } else if let Some(s) = v.as_str() {
            let s = s.trim();
            if !s.is_empty() && !s.contains(['*', '?', '[']) {
                paths.push(resolve_to_absolute(s, working_dir));
            }
        }
    }

    paths
}

/// Resolve `path` to an absolute string using `working_dir` as base if not already absolute.
pub(crate) fn resolve_to_absolute(path: &str, working_dir: &str) -> String {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        path.to_string()
    } else {
        std::path::Path::new(working_dir)
            .join(path)
            .to_string_lossy()
            .into_owned()
    }
}

/// Look for a similarly-named entry in the parent directory of a missing path.
///
/// Uses case-insensitive substring matching. Returns a candidate path string or None.
pub(crate) fn suggest_similar_path(missing: &str) -> Option<String> {
    let p = std::path::Path::new(missing);
    let parent = p.parent().filter(|d| d != &std::path::Path::new(""))?;
    let stem = p.file_name()?.to_string_lossy().to_lowercase();
    let entries = std::fs::read_dir(parent).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.contains(stem.as_str()) || stem.contains(name.as_str()) {
            return Some(entry.path().to_string_lossy().into_owned());
        }
    }
    None
}

/// Pre-execution path existence invariant (FASE-2 structural gate).
///
/// For ReadOnly tools with path arguments, verify that every referenced path exists
/// on the filesystem before delegating to `run_with_retry`. If any path is missing,
/// returns a structured `is_error` result with an optional "did you mean" hint.
///
/// Only applies to `ReadOnly` tools — write/destructive tools may legitimately target
/// non-existent paths (creation intent). Glob patterns in path values are also skipped.
pub(crate) fn pre_validate_path_args(
    tool_call: &CompletedToolUse,
    perm_level: PermissionLevel,
    working_dir: &str,
) -> Option<ToolExecResult> {
    if perm_level != PermissionLevel::ReadOnly {
        return None;
    }

    let paths = extract_path_args(&tool_call.input, working_dir);
    if paths.is_empty() {
        return None;
    }

    let missing: Vec<String> = paths
        .into_iter()
        .filter(|p| !std::path::Path::new(p).exists())
        .collect();

    if missing.is_empty() {
        return None;
    }

    let lines: Vec<String> = missing
        .iter()
        .map(|p| {
            if let Some(hint) = suggest_similar_path(p) {
                format!("  • {p}\n    Did you mean: {hint}")
            } else {
                format!("  • {p}")
            }
        })
        .collect();

    let msg = format!(
        "Error: the following path(s) do not exist on the filesystem:\n{}\n\
         Working directory: {working_dir}\n\
         Use 'directory_tree' or 'glob' to discover the actual file structure before retrying.",
        lines.join("\n")
    );

    tracing::warn!(
        tool = %tool_call.name,
        missing = ?missing,
        "pre_validate_path_args: path existence check failed — blocking execution"
    );

    Some(make_error_result(tool_call, msg))
}
