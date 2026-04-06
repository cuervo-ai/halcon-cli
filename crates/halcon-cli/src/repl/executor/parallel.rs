//! Parallel batch execution for ReadOnly/concurrent tools.
//!
//! Uses `buffer_unordered` to limit the number of concurrent tool executions.
//! Includes PASO 5 safety guard: blocks Destructive tools from parallel path.

use std::time::Duration;

use chrono::Utc;
use futures::stream::StreamExt as _;

use halcon_core::types::{ContentBlock, DomainEvent, EventPayload, PermissionLevel};
use halcon_core::EventSender;
use halcon_storage::{AsyncDatabase, ToolExecutionMetric, TraceStep, TraceStepType};
use halcon_tools::ToolRegistry;

use super::{
    canonicalize_name, execute_one_tool, make_error_result, resolve_tool_from_registry,
    CompletedToolUse, ToolExecResult, ToolExecutionConfig,
};
use crate::render::sink::RenderSink;

/// Execute the parallel batch concurrently with a concurrency cap.
#[allow(clippy::too_many_arguments)]
pub async fn execute_parallel_batch(
    batch: &[CompletedToolUse],
    registry: &ToolRegistry,
    working_dir: &str,
    tool_timeout: Duration,
    event_tx: &EventSender,
    trace_db: Option<&AsyncDatabase>,
    session_id: uuid::Uuid,
    trace_step_index: &mut u32,
    max_parallel_tools: usize,
    exec_config: &ToolExecutionConfig<'_>,
    render_sink: &dyn RenderSink,
    plugin_registry: Option<&std::sync::Mutex<crate::repl::plugins::PluginRegistry>>,
) -> Vec<ToolExecResult> {
    if batch.is_empty() {
        return Vec::new();
    }

    tracing::info!(count = batch.len(), "Executing parallel tool batch");

    // Record parallel batch trace step.
    if let Some(db) = trace_db {
        let tool_ids: Vec<&str> = batch.iter().map(|t| t.id.as_str()).collect();
        let step = TraceStep {
            session_id,
            step_index: *trace_step_index,
            step_type: TraceStepType::ToolCall,
            data_json: serde_json::json!({
                "parallel_batch": true,
                "tool_count": batch.len(),
                "tool_ids": tool_ids,
                "tool_names": batch.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            })
            .to_string(),
            duration_ms: 0,
            timestamp: Utc::now(),
        };
        if let Err(e) = db.inner().append_trace_step(&step) {
            tracing::warn!("trace recording failed (step {}): {e}", *trace_step_index);
        }
        *trace_step_index += 1;
    }

    // PASO 5 guard: Destructive tools must NEVER run in the parallel batch.
    let mut early_errors: Vec<ToolExecResult> = Vec::new();
    let safe_batch: Vec<&CompletedToolUse> = batch
        .iter()
        .filter(|tool_call| {
            let perm = resolve_tool_from_registry(&tool_call.name, registry)
                .map(|t| t.permission_level())
                .unwrap_or(PermissionLevel::ReadOnly);
            if perm >= PermissionLevel::ReadWrite {
                tracing::error!(
                    tool = %tool_call.name,
                    perm = ?perm,
                    "Destructive tool reached parallel batch — blocked by safety guard (PASO 5)"
                );
                early_errors.push(make_error_result(
                    tool_call,
                    format!(
                        "Error: tool '{}' is {:?} and cannot run in the parallel (no-permission) batch. \
                        This is a routing bug — Destructive tools must use the sequential executor.",
                        tool_call.name, perm
                    ),
                ));
                false
            } else {
                true
            }
        })
        .collect();

    // Launch all safe tools concurrently.
    let dry_run_mode = exec_config.dry_run_mode;
    let futures: Vec<_> = safe_batch
        .iter()
        .map(|tool_call| {
            let name = tool_call.name.clone();
            let input = tool_call.input.clone();
            render_sink.tool_start(&name, &input);
            execute_one_tool(
                tool_call,
                registry,
                working_dir,
                tool_timeout,
                dry_run_mode,
                exec_config.idempotency,
                &exec_config.retry,
                render_sink,
                plugin_registry,
                exec_config.hook_runner.as_deref(),
                &exec_config.session_id_str,
                &exec_config.session_tools,
            )
        })
        .collect();

    let max_concurrent = max_parallel_tools.max(1);
    let mut results: Vec<ToolExecResult> = futures::stream::iter(futures)
        .buffer_unordered(max_concurrent)
        .collect()
        .await;

    // Mark all as parallel and emit events.
    for result in &mut results {
        result.was_parallel = true;

        let perm_level = registry
            .get(&result.tool_name)
            .map(|t| t.permission_level())
            .unwrap_or(PermissionLevel::ReadOnly);

        let is_error = matches!(&result.content_block,
            ContentBlock::ToolResult { is_error, .. } if *is_error);

        let _ = event_tx.send(DomainEvent::new(EventPayload::ToolExecuted {
            tool: result.tool_name.clone(),
            permission: perm_level,
            duration_ms: result.duration_ms,
            success: !is_error,
        }));

        // Individual trace step per tool result.
        if let Some(db) = trace_db {
            let content = match &result.content_block {
                ContentBlock::ToolResult { content, .. } => content.as_str(),
                _ => "",
            };
            let step = TraceStep {
                session_id,
                step_index: *trace_step_index,
                step_type: TraceStepType::ToolResult,
                data_json: serde_json::json!({
                    "tool_use_id": &result.tool_use_id,
                    "tool_name": &result.tool_name,
                    "content": content,
                    "is_error": is_error,
                    "duration_ms": result.duration_ms,
                    "parallel": true,
                })
                .to_string(),
                duration_ms: result.duration_ms,
                timestamp: Utc::now(),
            };
            if let Err(e) = db.inner().append_trace_step(&step) {
                tracing::warn!("trace recording failed (step {}): {e}", *trace_step_index);
            }
            *trace_step_index += 1;
        }
    }

    // Persist tool execution metrics to M11.
    if let Some(db) = trace_db {
        let metrics: Vec<ToolExecutionMetric> = results
            .iter()
            .map(|r| {
                let is_error = matches!(
                    &r.content_block,
                    ContentBlock::ToolResult { is_error, .. } if *is_error
                );
                ToolExecutionMetric {
                    tool_name: r.tool_name.clone(),
                    session_id: Some(session_id.to_string()),
                    duration_ms: r.duration_ms,
                    success: !is_error,
                    is_parallel: true,
                    input_summary: None,
                    created_at: Utc::now(),
                }
            })
            .collect();
        if !metrics.is_empty() {
            if let Err(e) = db.inner().batch_insert_tool_metrics(&metrics) {
                tracing::warn!("tool_execution_metrics batch insert failed: {e}");
            }
        }
    }

    // Merge PASO 5 early errors.
    results.extend(early_errors);

    // Sort by tool_use_id for deterministic ordering.
    results.sort_by(|a, b| a.tool_use_id.cmp(&b.tool_use_id));
    results
}
