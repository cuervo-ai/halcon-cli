//! Unified retry logic: transient/deterministic classification, exponential backoff,
//! adaptive argument mutation, and environment self-repair.

use std::time::{Duration, Instant};

use halcon_core::types::{ContentBlock, ToolInput, ToolRetryConfig};

use super::{CompletedToolUse, ToolExecResult};
use crate::render::sink::RenderSink;

/// Check if an error message indicates a transient failure that can be retried.
///
/// Transient errors are temporary conditions — a brief wait or a single retry may
/// succeed. The agent loop uses this to decide whether to suppress the
/// `EnvironmentError` halt path and allow one more round.
///
/// IMPORTANT: MCP *connection* failures (pool call failed, connection reset) are
/// classified as transient — the MCP server can recover within the same session.
/// MCP *initialization* failures (server not started, process fail) are deterministic.
pub fn is_transient_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("429")
        // HTTP 5xx transient server errors.
        || lower.contains("http 500")
        || lower.contains("http 502")
        || lower.contains("http 503")
        || lower.contains("http 504")
        || lower.contains("http 529")
        || lower.contains("500 internal")
        || lower.contains("502 bad gateway")
        || lower.contains("503 service unavailable")
        || lower.contains("504 gateway timeout")
        // Anthropic-specific patterns
        || lower.contains("overloaded")
        || lower.contains("retryable error:")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("broken pipe")
        || lower.contains("temporary")
        || lower.contains("network error")
        // MCP pool/transport errors
        || lower.contains("mcp pool call failed")
        || lower.contains("failed to call")
        || lower.contains("transport error")
        || lower.contains("channel closed")
        // Cargo build lock contention
        || lower.contains(".cargo-lock")
        || lower.contains("cargo-lock")
        || lower.contains("could not acquire package cache lock")
        || (lower.contains("file lock") && (lower.contains("build") || lower.contains("cargo")))
        // Generic filesystem lock contention (EAGAIN, EWOULDBLOCK)
        || lower.contains("resource temporarily unavailable")
        || lower.contains("eagain")
}

/// Check if an error is deterministic (will never succeed on retry/replan).
///
/// These errors indicate permanent conditions: missing files, bad permissions,
/// invalid schemas, billing/auth failures, tool not registered, etc.
/// Retrying or replanning will produce the same result — abort rather than loop.
///
/// NOTE: MCP *connection* failures are NOT in this list (moved to is_transient_error).
/// Only MCP *initialization* failures (server not started, process crash) are here.
pub fn is_deterministic_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("no such file or directory")
        || lower.contains("not found")
        || lower.contains("permission denied")
        || lower.contains("invalid path")
        || lower.contains("is a directory")
        || lower.contains("not a directory")
        || lower.contains("path traversal")
        || lower.contains("blocked by security")
        || lower.contains("unknown tool")
        || lower.contains("denied by task context")
        || lower.contains("schema")
        || lower.contains("missing required")
        // Auth/billing errors — retrying will never fix these.
        || lower.contains("credit balance")
        || lower.contains("invalid_api_key")
        || lower.contains("authentication")
        || lower.contains("unauthorized")
        || lower.contains("insufficient_quota")
        // MCP initialization errors
        || lower.contains("mcp server is not initialized")
        || lower.contains("not initialized")
        || lower.contains("process start")
        || lower.contains("process failed")
}

/// Classify an error using the typed `ToolFailureKind` enum.
///
/// This is the recommended entry point for new code. Existing code continues
/// to use `is_transient_error()` / `is_deterministic_error()` until migration completes.
///
/// # Example
/// ```ignore
/// let kind = classify_error("Rate limit exceeded (429)");
/// if kind.is_transient() { /* retry */ }
/// ```
pub fn classify_error(error: &str) -> crate::repl::failure_handler::typed_errors::ToolFailureKind {
    crate::repl::failure_handler::typed_errors::ToolFailureKind::classify(error)
}

/// IMP-1 Adaptive Retry — mutate tool arguments for the second retry attempt.
///
/// Applies conservative, reversible modifications that make transient failures
/// more likely to succeed:
/// - `lint_check` / `bash` running clippy: append `--no-deps` to args to avoid
///   fetching network deps, which can time out or trigger lock contention.
/// - All other tools: return `None` (no mutation — same args as attempt 0).
///
/// Returns `Some(new_input)` when a mutation is applicable, `None` otherwise.
pub(crate) fn mutate_args_for_retry(
    tool_name: &str,
    input: &serde_json::Value,
) -> Option<serde_json::Value> {
    match tool_name {
        "lint_check" => {
            let mut mutated = input.clone();
            if let Some(args) = mutated.get_mut("args").and_then(|a| a.as_array_mut()) {
                if !args.iter().any(|v| v.as_str() == Some("--no-deps")) {
                    args.push(serde_json::Value::String("--no-deps".to_string()));
                    tracing::debug!(
                        tool = "lint_check",
                        "IMP-1: adaptive retry — injected --no-deps"
                    );
                    return Some(mutated);
                }
            }
            None
        }
        "bash" => {
            let mut mutated = input.clone();
            if let Some(cmd) = mutated.get_mut("command").and_then(|c| c.as_str()) {
                let cmd_str = cmd.to_string();
                if cmd_str.contains("cargo clippy") && !cmd_str.contains("--no-deps") {
                    let new_cmd = cmd_str.replacen("cargo clippy", "cargo clippy --no-deps", 1);
                    mutated["command"] = serde_json::Value::String(new_cmd);
                    tracing::debug!(
                        tool = "bash",
                        "IMP-1: adaptive retry — added --no-deps to cargo clippy"
                    );
                    return Some(mutated);
                }
            }
            None
        }
        _ => None,
    }
}

/// Apply ±20% jitter to a delay to prevent thundering herd.
pub(crate) fn jittered_delay(delay_ms: u64) -> u64 {
    use rand::Rng;
    let jitter_factor = 0.8 + rand::rng().random_range(0.0..0.4);
    (delay_ms as f64 * jitter_factor) as u64
}

/// Execute the tool with exponential-backoff retries for transient failures.
pub(crate) async fn run_with_retry(
    tool_call: &CompletedToolUse,
    tool: &std::sync::Arc<dyn halcon_core::traits::Tool>,
    working_dir: &str,
    tool_timeout: Duration,
    retry_config: &ToolRetryConfig,
    render_sink: &dyn RenderSink,
) -> ToolExecResult {
    let start = Instant::now();
    let max_attempts = retry_config.max_retries + 1;

    for attempt in 0..max_attempts {
        // IMP-1 Adaptive Retry: on attempt 2+ apply conservative arg mutations
        let effective_args = if attempt >= 2 {
            mutate_args_for_retry(&tool_call.name, &tool_call.input)
                .unwrap_or_else(|| tool_call.input.clone())
        } else {
            tool_call.input.clone()
        };
        let tool_input = ToolInput {
            tool_use_id: tool_call.id.clone(),
            arguments: effective_args,
            working_directory: working_dir.to_string(),
        };

        match tokio::time::timeout(tool_timeout, tool.execute(tool_input)).await {
            Ok(Ok(output)) => {
                // S1: Unified is_error retry contract.
                if output.is_error
                    && attempt + 1 < max_attempts
                    && is_transient_error(&output.content)
                {
                    if let Some(repairs) = crate::repl::env_repair::run_repairs(
                        &output.content,
                        &tool_call.name,
                        working_dir,
                    ) {
                        for repair in &repairs {
                            tracing::info!(
                                tool = %tool_call.name,
                                attempt = attempt + 1,
                                env_repair.action = %repair.action,
                                env_repair.repaired = repair.repaired,
                                "env-repair (is_error path): {}",
                                repair.description
                            );
                        }
                    }
                    let delay = jittered_delay(std::cmp::min(
                        retry_config.base_delay_ms * 2u64.pow(std::cmp::min(attempt, 5)),
                        retry_config.max_delay_ms,
                    ));
                    tracing::info!(
                        tool = %tool_call.name,
                        attempt = attempt + 1,
                        delay_ms = delay,
                        "Retrying transient is_error=true output: {}",
                        output.content
                    );
                    render_sink.tool_retrying(
                        &tool_call.name,
                        (attempt + 1) as usize,
                        max_attempts as usize,
                        delay,
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    continue;
                }
                return ToolExecResult {
                    tool_use_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    content_block: ContentBlock::ToolResult {
                        tool_use_id: tool_call.id.clone(),
                        content: output.content,
                        is_error: output.is_error,
                    },
                    duration_ms: start.elapsed().as_millis() as u64,
                    was_parallel: false,
                };
            }
            Ok(Err(e)) => {
                let err_str = format!("{e}");
                if attempt + 1 < max_attempts && is_transient_error(&err_str) {
                    // IMP-3: attempt environment self-repair before sleeping.
                    if let Some(repairs) =
                        crate::repl::env_repair::run_repairs(&err_str, &tool_call.name, working_dir)
                    {
                        for repair in &repairs {
                            tracing::info!(
                                tool = %tool_call.name,
                                attempt = attempt + 1,
                                env_repair.action = %repair.action,
                                env_repair.repaired = repair.repaired,
                                "env-repair: {}",
                                repair.description
                            );
                        }
                    }
                    let delay = jittered_delay(std::cmp::min(
                        retry_config.base_delay_ms * 2u64.pow(std::cmp::min(attempt, 5)),
                        retry_config.max_delay_ms,
                    ));
                    tracing::info!(tool = %tool_call.name, attempt = attempt + 1, delay_ms = delay, "Retrying transient tool error: {err_str}");
                    render_sink.tool_retrying(
                        &tool_call.name,
                        (attempt + 1) as usize,
                        max_attempts as usize,
                        delay,
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    continue;
                }
                return ToolExecResult {
                    tool_use_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    content_block: ContentBlock::ToolResult {
                        tool_use_id: tool_call.id.clone(),
                        content: format!("Error: {e}"),
                        is_error: true,
                    },
                    duration_ms: start.elapsed().as_millis() as u64,
                    was_parallel: false,
                };
            }
            Err(_elapsed) => {
                let err_str = format!(
                    "Error: tool '{}' timed out after {}s",
                    tool_call.name,
                    tool_timeout.as_secs()
                );
                if attempt + 1 < max_attempts {
                    let delay = std::cmp::min(
                        retry_config.base_delay_ms * 2u64.pow(std::cmp::min(attempt, 5)),
                        retry_config.max_delay_ms,
                    );
                    tracing::info!(tool = %tool_call.name, attempt = attempt + 1, delay_ms = delay, "Retrying timed out tool");
                    render_sink.tool_retrying(
                        &tool_call.name,
                        (attempt + 1) as usize,
                        max_attempts as usize,
                        delay,
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    continue;
                }
                return ToolExecResult {
                    tool_use_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    content_block: ContentBlock::ToolResult {
                        tool_use_id: tool_call.id.clone(),
                        content: err_str,
                        is_error: true,
                    },
                    duration_ms: start.elapsed().as_millis() as u64,
                    was_parallel: false,
                };
            }
        }
    }
    unreachable!("loop always returns on final attempt")
}
