//! Tool execution pipeline — single-tool lifecycle from resolution to recording.
//!
//! Extracted from `mod.rs` to isolate the execution pipeline from the public API
//! surface. Each stage is a composable function (`check_*`, `record_*`).
//!
//! ## Pipeline Stages
//!
//! 1. resolve   → `check_tool_known()` — find tool in registry/session tools
//! 2. plugin    → `check_plugin_gate()` — plugin circuit breaker + capability check
//! 3. dry-run   → `check_dry_run()` — skip execution in dry-run mode
//! 4. cache     → `check_idempotency()` — return cached result if duplicate
//! 5. validate  → `validate_tool_args()` + `pre_validate_path_args()` — schema + path checks
//! 6. hook      → `fire_pre_tool_hook()` — lifecycle hook integration
//! 7. execute   → `run_with_retry()` — actual tool execution with retry
//! 8. post-hook → `fire_post_tool_hook()` — post-execution hook
//! 9. record    → `record_idempotency()` + `record_plugin_metrics()` — persist results

use std::time::Duration;

use halcon_core::types::ToolRetryConfig;
use halcon_core::types::{ContentBlock, PermissionLevel};
use halcon_tools::ToolRegistry;

use super::hooks;
use super::retry::run_with_retry;
use super::validation::{pre_validate_path_args, validate_tool_args};
use super::{
    canonicalize_name, resolve_tool_from_registry, synthetic_dry_run_result, CompletedToolUse,
    ToolExecResult,
};
use crate::render::sink::RenderSink;
use crate::repl::security::idempotency::DryRunMode;

// ─────────────────────────────────────────────────────────────────────────────
// Helper functions — each <50 LOC, independently testable
// ─────────────────────────────────────────────────────────────────────────────

/// Build a ToolExecResult with is_error=true and zero duration.
#[inline]
pub(crate) fn make_error_result(tool_call: &CompletedToolUse, content: String) -> ToolExecResult {
    ToolExecResult {
        tool_use_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        content_block: ContentBlock::ToolResult {
            tool_use_id: tool_call.id.clone(),
            content,
            is_error: true,
        },
        duration_ms: 0,
        was_parallel: false,
    }
}

/// Return an error result if the tool is not in the registry.
#[allow(clippy::result_large_err)]
pub(crate) fn check_tool_known(
    tool_call: &CompletedToolUse,
    registry: &ToolRegistry,
    session_tools: &[std::sync::Arc<dyn halcon_core::traits::Tool>],
) -> Result<std::sync::Arc<dyn halcon_core::traits::Tool>, ToolExecResult> {
    if let Some(t) = resolve_tool_from_registry(&tool_call.name, registry) {
        return Ok(t);
    }
    let canonical = canonicalize_name(&tool_call.name);
    for t in session_tools {
        if t.name() == tool_call.name || t.name() == canonical {
            return Ok(t.clone());
        }
    }
    Err(make_error_result(
        tool_call,
        format!(
            "Error: unknown tool '{}' (canonical: '{}')",
            tool_call.name, canonical
        ),
    ))
}

/// Return a dry-run result if the mode demands it, otherwise None.
pub(crate) fn check_dry_run(
    tool_call: &CompletedToolUse,
    perm_level: PermissionLevel,
    dry_run_mode: DryRunMode,
) -> Option<ToolExecResult> {
    match dry_run_mode {
        DryRunMode::Off => None,
        DryRunMode::Full => Some(synthetic_dry_run_result(tool_call)),
        DryRunMode::DestructiveOnly if perm_level >= PermissionLevel::ReadWrite => {
            Some(synthetic_dry_run_result(tool_call))
        }
        DryRunMode::DestructiveOnly => None,
    }
}

/// Return a cached result if this call was already executed, plus the execution_id.
pub(crate) fn check_idempotency(
    tool_call: &CompletedToolUse,
    idempotency: Option<&crate::repl::security::idempotency::IdempotencyRegistry>,
) -> (Option<ToolExecResult>, Option<String>) {
    let Some(reg) = idempotency else {
        return (None, None);
    };
    let id = crate::repl::security::idempotency::compute_execution_id(
        &tool_call.name,
        &tool_call.input,
        "",
    );
    if let Some(cached) = reg.lookup(&id) {
        let result = ToolExecResult {
            tool_use_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content_block: ContentBlock::ToolResult {
                tool_use_id: tool_call.id.clone(),
                content: cached.result_content,
                is_error: cached.is_error,
            },
            duration_ms: 0,
            was_parallel: false,
        };
        return (Some(result), Some(id));
    }
    (None, Some(id))
}

/// Record the execution result in the idempotency registry.
pub(crate) fn record_idempotency(
    idempotency: Option<&crate::repl::security::idempotency::IdempotencyRegistry>,
    exec_id: Option<String>,
    tool_call: &CompletedToolUse,
    result: &ToolExecResult,
) {
    let (Some(registry), Some(id)) = (idempotency, exec_id) else {
        return;
    };
    let (content, is_error) = match &result.content_block {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => (content.clone(), *is_error),
        _ => (String::new(), false),
    };
    registry.record(crate::repl::security::idempotency::ExecutionRecord {
        execution_id: id,
        tool_name: tool_call.name.clone(),
        result_content: content,
        is_error,
        executed_at: chrono::Utc::now(),
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Execution Pipeline
// ─────────────────────────────────────────────────────────────────────────────

/// Bundled context for tool execution — replaces 12 positional parameters.
pub(crate) struct ExecutionContext<'a> {
    pub registry: &'a ToolRegistry,
    pub working_dir: &'a str,
    pub tool_timeout: Duration,
    pub dry_run_mode: DryRunMode,
    pub idempotency: Option<&'a crate::repl::security::idempotency::IdempotencyRegistry>,
    pub retry_config: &'a ToolRetryConfig,
    pub render_sink: &'a dyn RenderSink,
    pub plugin_registry: Option<&'a std::sync::Mutex<crate::repl::plugins::PluginRegistry>>,
    pub hook_runner: Option<&'a crate::repl::hooks::HookRunner>,
    pub session_id_str: &'a str,
    pub session_tools: &'a [std::sync::Arc<dyn halcon_core::traits::Tool>],
}

/// Check plugin pre-invoke gate. Returns Some(denied_result) if blocked.
fn check_plugin_gate(
    tool_call: &CompletedToolUse,
    plugin_registry: Option<&std::sync::Mutex<crate::repl::plugins::PluginRegistry>>,
) -> Option<ToolExecResult> {
    let pr_mutex = plugin_registry?;
    match pr_mutex.try_lock() {
        Ok(pr) => {
            if let Some(plugin_id) = pr.plugin_id_for_tool(&tool_call.name).map(str::to_owned) {
                if let crate::repl::plugins::InvokeGateResult::Deny(reason) =
                    pr.pre_invoke_gate(&plugin_id, &tool_call.name, false)
                {
                    return Some(synthetic_plugin_denied_result(tool_call, &reason));
                }
            }
            None
        }
        Err(_) => {
            tracing::warn!(tool = %tool_call.name, "plugin gate lock contention — denying tool (fail-closed)");
            Some(synthetic_plugin_denied_result(
                tool_call,
                "plugin service temporarily unavailable",
            ))
        }
    }
}

/// Record plugin post-invoke metrics (best-effort, never blocks).
fn record_plugin_metrics(
    tool_call: &CompletedToolUse,
    result: &ToolExecResult,
    plugin_registry: Option<&std::sync::Mutex<crate::repl::plugins::PluginRegistry>>,
) {
    let Some(pr_mutex) = plugin_registry else {
        return;
    };
    match pr_mutex.try_lock() {
        Ok(mut pr) => {
            if let Some(plugin_id) = pr.plugin_id_for_tool(&tool_call.name).map(str::to_owned) {
                let is_err = matches!(
                    &result.content_block,
                    ContentBlock::ToolResult { is_error: true, .. }
                );
                pr.post_invoke(&plugin_id, &tool_call.name, 0, 0.0, !is_err, None);
            }
        }
        Err(_) => {
            tracing::warn!(tool = %tool_call.name, "plugin post-invoke metrics skipped — lock contention")
        }
    }
}

/// Execute a single tool through the execution pipeline.
///
/// Pipeline stages (each is a separate function, composable):
///   1. resolve   → check_tool_known()
///   2. plugin    → check_plugin_gate()
///   3. dry-run   → check_dry_run()
///   4. cache     → check_idempotency()
///   5. validate  → validate_tool_args() + pre_validate_path_args()
///   6. hook      → hooks::fire_pre_tool_hook()
///   7. execute   → run_with_retry()
///   8. post-hook → hooks::fire_post_tool_hook()
///   9. record    → record_idempotency() + record_plugin_metrics()
pub(crate) async fn execute_tool_pipeline(
    tool_call: &CompletedToolUse,
    ctx: &ExecutionContext<'_>,
) -> ToolExecResult {
    // Stage 1: Resolve tool.
    let tool = match check_tool_known(tool_call, ctx.registry, ctx.session_tools) {
        Ok(t) => t,
        Err(e) => return e,
    };

    // Stage 2: Plugin pre-invoke gate.
    if let Some(denied) = check_plugin_gate(tool_call, ctx.plugin_registry) {
        return denied;
    }

    // Stage 3: Dry-run shortcut.
    if let Some(r) = check_dry_run(tool_call, tool.permission_level(), ctx.dry_run_mode) {
        return r;
    }

    // Stage 4: Idempotency cache.
    let (cached, exec_id) = check_idempotency(tool_call, ctx.idempotency);
    if let Some(r) = cached {
        return r;
    }

    // Stage 5: Argument validation + path existence.
    if let Some(r) = validate_tool_args(tool_call) {
        return r;
    }
    if let Some(r) = pre_validate_path_args(tool_call, tool.permission_level(), ctx.working_dir) {
        record_idempotency(ctx.idempotency, exec_id, tool_call, &r);
        return r;
    }

    // Stage 6: PreToolUse lifecycle hook.
    if let Some(runner) = ctx.hook_runner {
        if let Some(denied) = hooks::fire_pre_tool_hook(runner, tool_call, ctx.session_id_str).await
        {
            record_idempotency(ctx.idempotency, exec_id, tool_call, &denied);
            return denied;
        }
    }

    // Stage 7: Execute with retry.
    let result = run_with_retry(
        tool_call,
        &tool,
        ctx.working_dir,
        ctx.tool_timeout,
        ctx.retry_config,
        ctx.render_sink,
    )
    .await;

    // Stage 8: PostToolUse hook (best-effort).
    if let Some(runner) = ctx.hook_runner {
        let is_error = matches!(&result.content_block,
            ContentBlock::ToolResult { is_error, .. } if *is_error);
        hooks::fire_post_tool_hook(runner, tool_call, is_error, ctx.session_id_str).await;
    }

    // Stage 9: Record idempotency + plugin metrics.
    record_idempotency(ctx.idempotency, exec_id, tool_call, &result);
    record_plugin_metrics(tool_call, &result, ctx.plugin_registry);

    result
}

/// Execute a single tool — constructs an `ExecutionContext` and delegates to the pipeline.
///
/// This is the primary entry point for parallel.rs and sequential.rs.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_one_tool(
    tool_call: &CompletedToolUse,
    registry: &ToolRegistry,
    working_dir: &str,
    tool_timeout: Duration,
    dry_run_mode: DryRunMode,
    idempotency: Option<&crate::repl::security::idempotency::IdempotencyRegistry>,
    retry_config: &ToolRetryConfig,
    render_sink: &dyn RenderSink,
    plugin_registry: Option<&std::sync::Mutex<crate::repl::plugins::PluginRegistry>>,
    hook_runner: Option<&crate::repl::hooks::HookRunner>,
    session_id_str: &str,
    session_tools: &[std::sync::Arc<dyn halcon_core::traits::Tool>],
) -> ToolExecResult {
    let ctx = ExecutionContext {
        registry,
        working_dir,
        tool_timeout,
        dry_run_mode,
        idempotency,
        retry_config,
        render_sink,
        plugin_registry,
        hook_runner,
        session_id_str,
        session_tools,
    };
    execute_tool_pipeline(tool_call, &ctx).await
}

/// Build a synthetic ToolExecResult for plugin gate denials.
fn synthetic_plugin_denied_result(tool_call: &CompletedToolUse, reason: &str) -> ToolExecResult {
    ToolExecResult {
        tool_use_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        content_block: ContentBlock::ToolResult {
            tool_use_id: tool_call.id.clone(),
            content: format!("Plugin gate denied: {reason}"),
            is_error: true,
        },
        duration_ms: 0,
        was_parallel: false,
    }
}
