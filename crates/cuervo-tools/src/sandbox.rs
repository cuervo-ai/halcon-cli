//! Process sandboxing: output truncation and Unix rlimits.
//!
//! Provides configurable resource limits for child processes
//! and smart output truncation that preserves head + tail.

use cuervo_core::types::SandboxConfig;

/// Truncate tool output preserving head and tail context.
///
/// If `text` exceeds `max_bytes`, keeps the first ~60% and last ~30%
/// with a truncation marker in between.
pub fn truncate_output(text: &str, max_bytes: usize) -> String {
    if max_bytes == 0 || text.len() <= max_bytes {
        return text.to_string();
    }

    // Keep 60% head, 30% tail, 10% for the marker.
    let head_bytes = max_bytes * 60 / 100;
    let tail_bytes = max_bytes * 30 / 100;

    // Find safe char boundaries.
    let head_end = safe_truncate_pos(text, head_bytes);
    let tail_start = safe_truncate_pos_rev(text, tail_bytes);

    let omitted = text.len() - head_end - (text.len() - tail_start);
    let mut result = String::with_capacity(max_bytes + 80);
    result.push_str(&text[..head_end]);
    result.push_str(&format!(
        "\n\n... [{omitted} bytes truncated] ...\n\n"
    ));
    result.push_str(&text[tail_start..]);
    result
}

/// Find the last valid char boundary at or before `pos`.
fn safe_truncate_pos(text: &str, pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    let mut p = pos;
    while p > 0 && !text.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Find the first valid char boundary at or after `text.len() - bytes_from_end`.
fn safe_truncate_pos_rev(text: &str, bytes_from_end: usize) -> usize {
    if bytes_from_end >= text.len() {
        return 0;
    }
    let mut p = text.len() - bytes_from_end;
    while p < text.len() && !text.is_char_boundary(p) {
        p += 1;
    }
    p
}

/// Apply rlimits to the current process (Unix only).
///
/// This is designed to be called inside a `pre_exec` closure
/// on a child process before `exec`. It sets:
/// - RLIMIT_CPU: CPU time
/// - RLIMIT_FSIZE: file size
///
/// Individual rlimit failures are silently ignored (best-effort)
/// since different platforms support different limits.
///
/// Returns Ok(()) always — never fails the child spawn.
#[cfg(unix)]
pub fn apply_rlimits(config: &SandboxConfig) -> std::io::Result<()> {
    use libc::{rlimit, setrlimit, RLIMIT_CPU, RLIMIT_FSIZE};

    if !config.enabled {
        return Ok(());
    }

    // CPU time limit (RLIMIT_CPU) — well-supported on all Unix.
    if config.max_cpu_secs > 0 {
        let limit = rlimit {
            rlim_cur: config.max_cpu_secs,
            rlim_max: config.max_cpu_secs,
        };
        // Best-effort: ignore failures (platform may not support).
        unsafe { setrlimit(RLIMIT_CPU, &limit) };
    }

    // File size limit (RLIMIT_FSIZE) — well-supported on all Unix.
    if config.max_file_size_bytes > 0 {
        let limit = rlimit {
            rlim_cur: config.max_file_size_bytes,
            rlim_max: config.max_file_size_bytes,
        };
        unsafe { setrlimit(RLIMIT_FSIZE, &limit) };
    }

    // Note: RLIMIT_AS (address space) is intentionally omitted.
    // On macOS it may reject values that conflict with the system hard limit,
    // and on Linux containers it may already be capped by cgroups.
    // Memory limits are better enforced via max_output_bytes + timeout.

    Ok(())
}

/// No-op on non-Unix platforms.
#[cfg(not(unix))]
pub fn apply_rlimits(_config: &SandboxConfig) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_output_no_op_when_within_limit() {
        let text = "hello world";
        assert_eq!(truncate_output(text, 100), "hello world");
    }

    #[test]
    fn truncate_output_no_op_when_zero_limit() {
        let text = "hello world";
        assert_eq!(truncate_output(text, 0), "hello world");
    }

    #[test]
    fn truncate_output_preserves_head_and_tail() {
        let text = "A".repeat(200);
        let result = truncate_output(&text, 100);
        assert!(result.len() < 200 + 50); // within bounds + marker
        assert!(result.contains("truncated"));
        assert!(result.starts_with("AAAA"));
        assert!(result.ends_with("AAAA"));
    }

    #[test]
    fn truncate_output_handles_utf8() {
        // 4-byte UTF-8 chars
        let text = "🦀".repeat(100);
        let result = truncate_output(&text, 200);
        assert!(result.contains("truncated"));
        // Should not panic or produce invalid UTF-8
        assert!(result.is_char_boundary(0));
    }

    #[test]
    fn truncate_output_exact_boundary() {
        let text = "abc";
        assert_eq!(truncate_output(text, 3), "abc");
    }

    #[test]
    fn sandbox_config_defaults() {
        let config = SandboxConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_output_bytes, 100_000);
        assert_eq!(config.max_memory_mb, 512);
        assert_eq!(config.max_cpu_secs, 60);
        assert_eq!(config.max_file_size_bytes, 50_000_000);
    }

    #[test]
    fn sandbox_config_default_values() {
        // ToolsConfig.sandbox uses serde(default) so SandboxConfig::default() is used
        // when the [tools.sandbox] section is missing from TOML.
        let config = SandboxConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_output_bytes, 100_000);
        assert_eq!(config.max_memory_mb, 512);
    }

    #[cfg(unix)]
    #[test]
    fn apply_rlimits_disabled_is_noop() {
        let config = SandboxConfig {
            enabled: false,
            ..SandboxConfig::default()
        };
        // Should not modify this process's limits.
        assert!(apply_rlimits(&config).is_ok());
    }
}
