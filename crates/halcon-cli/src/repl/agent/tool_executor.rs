//! Tool execution with permission-aware partitioning (Xiyo-aligned).
//!
//! Execution model matches Xiyo's toolOrchestration.ts:
//!   1. Partition tool_uses into CONSECUTIVE batches by concurrency safety
//!   2. Execute each batch IN SEQUENCE:
//!      - Safe batch → parallel (Semaphore-bounded)
//!      - Unsafe batch → serial (with permission check per tool)
//!   3. Results naturally arrive in causal order — no reordering needed
//!   4. Cancellation checked between serial tools + synthetic results for orphans

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use halcon_core::types::{ContentBlock, PermissionLevel, ToolInput};
use halcon_tools::ToolRegistry;

use super::accumulator::CompletedToolUse;
use crate::render::sink::RenderSink;
use crate::repl::conversational_permission::ConversationalPermissionHandler;
use crate::repl::security::permission_pipeline::{self, PipelineDecision};

// ── Batch type ───────────────────────────────────────────────────────────────

struct Batch<'a> {
    tools: Vec<&'a CompletedToolUse>,
    is_concurrent: bool,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Execute tools with Xiyo-aligned consecutive-batch partitioning.
///
/// Causal ordering preserved: batches execute in sequence, each batch is either
/// all-parallel (safe) or all-serial (unsafe). A safe tool at index 3 will NOT
/// execute before an unsafe tool at index 2.
///
/// `is_cancelled` is called between serial tools and between batches. If
/// cancellation is detected, synthetic error results are generated for all
/// remaining tools (maintaining the tool_use ↔ tool_result 1:1 invariant).
pub async fn execute_tools_partitioned(
    tool_uses: &[CompletedToolUse],
    registry: &ToolRegistry,
    permissions: &mut ConversationalPermissionHandler,
    working_dir: &str,
    tool_timeout: Duration,
    max_parallel: usize,
    render_sink: &dyn RenderSink,
    is_cancelled: impl Fn() -> bool,
) -> Vec<ContentBlock> {
    let batches = partition_into_batches(tool_uses, registry, working_dir);
    let mut results: Vec<ContentBlock> = Vec::with_capacity(tool_uses.len());
    let mut executed_count = 0;

    for batch in &batches {
        // Cancel check between batches
        if is_cancelled() {
            // Generate synthetic error results for ALL remaining tools
            for b in batches.iter().skip(executed_count) {
                for tu in &b.tools {
                    if !results.iter().any(|r| match r {
                        ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id == &tu.id,
                        _ => false,
                    }) {
                        results.push(synthetic_cancel_result(tu));
                    }
                }
            }
            return results;
        }

        if batch.is_concurrent {
            let batch_results = execute_concurrent_batch(
                &batch.tools,
                registry,
                working_dir,
                tool_timeout,
                max_parallel,
                render_sink,
            )
            .await;
            results.extend(batch_results);
        } else {
            let batch_results = execute_serial_batch(
                &batch.tools,
                registry,
                permissions,
                working_dir,
                tool_timeout,
                render_sink,
                &is_cancelled,
            )
            .await;
            results.extend(batch_results);
        }

        executed_count += 1;
    }

    results
}

// ── Batch partitioning (Xiyo pattern: consecutive same-type groups) ──────────

/// Partition tool_uses into consecutive batches where each batch contains tools
/// of the same concurrency type. Preserves causal ordering.
///
/// Example: [read, read, bash, read, write] →
///   Batch { [read, read], concurrent: true }
///   Batch { [bash],       concurrent: false }
///   Batch { [read],       concurrent: true }
///   Batch { [write],      concurrent: false }
fn partition_into_batches<'a>(
    tool_uses: &'a [CompletedToolUse],
    registry: &ToolRegistry,
    working_dir: &str,
) -> Vec<Batch<'a>> {
    let mut batches: Vec<Batch<'a>> = Vec::new();

    for tu in tool_uses {
        let is_safe = registry
            .get(&tu.name)
            .map(|tool| {
                let input = ToolInput {
                    tool_use_id: tu.id.clone(),
                    arguments: tu.input.clone(),
                    working_directory: working_dir.to_owned(),
                };
                tool.is_concurrency_safe(&input)
            })
            .unwrap_or(false);

        // Append to current batch if same type, otherwise start new batch
        if let Some(last) = batches.last_mut() {
            if last.is_concurrent == is_safe {
                last.tools.push(tu);
                continue;
            }
        }
        batches.push(Batch {
            tools: vec![tu],
            is_concurrent: is_safe,
        });
    }

    batches
}

// ── Concurrent batch execution (safe tools, Semaphore-bounded) ───────────────

async fn execute_concurrent_batch(
    tools: &[&CompletedToolUse],
    registry: &ToolRegistry,
    working_dir: &str,
    tool_timeout: Duration,
    max_parallel: usize,
    render_sink: &dyn RenderSink,
) -> Vec<ContentBlock> {
    let semaphore = Arc::new(Semaphore::new(max_parallel));
    // Phase 4: Sibling abort — shared cancellation token per batch.
    // If any tool in this batch fails, cancel all siblings.
    let batch_cancel = CancellationToken::new();

    let futures: Vec<_> = tools
        .iter()
        .map(|tu| {
            let sem = semaphore.clone();
            let cancel = batch_cancel.clone();
            async move {
                let _permit = sem.acquire().await.expect("semaphore closed");
                // Check if batch was cancelled before starting
                if cancel.is_cancelled() {
                    return synthetic_cancel_result(tu);
                }
                let result = execute_single(tu, registry, working_dir, tool_timeout, render_sink).await;
                // If this tool failed, cancel all siblings
                if let ContentBlock::ToolResult { is_error: true, .. } = &result {
                    cancel.cancel();
                }
                result
            }
        })
        .collect();

    futures::future::join_all(futures).await
}

// ── Serial batch execution (unsafe tools, permission-gated, cancel-aware) ────

async fn execute_serial_batch(
    tools: &[&CompletedToolUse],
    registry: &ToolRegistry,
    permissions: &mut ConversationalPermissionHandler,
    working_dir: &str,
    tool_timeout: Duration,
    render_sink: &dyn RenderSink,
    is_cancelled: &impl Fn() -> bool,
) -> Vec<ContentBlock> {
    let mut results: Vec<ContentBlock> = Vec::with_capacity(tools.len());

    for (i, tu) in tools.iter().enumerate() {
        // Cancel check between serial tools
        if is_cancelled() {
            // Synthetic results for remaining tools in this batch
            for remaining in &tools[i..] {
                results.push(synthetic_cancel_result(remaining));
            }
            break;
        }

        let result =
            execute_with_permission(tu, registry, permissions, working_dir, tool_timeout, render_sink)
                .await;
        results.push(result);
    }

    results
}

// ── Single tool execution (no permission check — for safe tools) ─────────────

async fn execute_single(
    tool_call: &CompletedToolUse,
    registry: &ToolRegistry,
    working_dir: &str,
    tool_timeout: Duration,
    render_sink: &dyn RenderSink,
) -> ContentBlock {
    let Some(tool) = registry.get(&tool_call.name) else {
        return ContentBlock::ToolResult {
            tool_use_id: tool_call.id.clone(),
            content: format!("Tool '{}' not found", tool_call.name),
            is_error: true,
        };
    };

    let input = ToolInput {
        tool_use_id: tool_call.id.clone(),
        arguments: tool_call.input.clone(),
        working_directory: working_dir.to_owned(),
    };

    render_sink.tool_start(&tool_call.name, &tool_call.input);

    let (content, is_error) =
        match tokio::time::timeout(tool_timeout, tool.execute(input)).await {
            Ok(Ok(output)) => (output.content, output.is_error),
            Ok(Err(e)) => (format!("Tool error: {e}"), true),
            Err(_) => (
                format!("Tool '{}' timed out after {tool_timeout:?}", tool_call.name),
                true,
            ),
        };

    ContentBlock::ToolResult {
        tool_use_id: tool_call.id.clone(),
        content,
        is_error,
    }
}

// ── Single tool execution with permission (for unsafe tools) ─────────────────

async fn execute_with_permission(
    tool_call: &CompletedToolUse,
    registry: &ToolRegistry,
    permissions: &mut ConversationalPermissionHandler,
    working_dir: &str,
    tool_timeout: Duration,
    render_sink: &dyn RenderSink,
) -> ContentBlock {
    let Some(tool) = registry.get(&tool_call.name) else {
        return ContentBlock::ToolResult {
            tool_use_id: tool_call.id.clone(),
            content: format!("Tool '{}' not found", tool_call.name),
            is_error: true,
        };
    };

    let input = ToolInput {
        tool_use_id: tool_call.id.clone(),
        arguments: tool_call.input.clone(),
        working_directory: working_dir.to_owned(),
    };

    // Permission gate via unified pipeline (Phase 1 — single authority).
    // All permission decisions route through authorize_tool(): TBAC → Blacklist →
    // Safety → Denial tracking → Conversational. No direct calls to
    // permissions.authorize() outside this pipeline.
    let level = tool.permission_level();
    if level >= PermissionLevel::ReadWrite {
        let pipeline_result = permission_pipeline::authorize_tool(
            &tool_call.name,
            level,
            &input,
            permissions,
            None, // DenialTracker: wired when executor owns tracker instance
        )
        .await;

        match pipeline_result {
            PipelineDecision::Deny { reason, gate } => {
                tracing::info!(tool = %tool_call.name, gate = gate, "Permission denied: {reason}");
                render_sink.tool_denied(&tool_call.name);
                return ContentBlock::ToolResult {
                    tool_use_id: tool_call.id.clone(),
                    content: format!(
                        "Permission denied for '{}' (gate: {gate}): {reason}. \
                         Do NOT retry this tool. Acknowledge the denial and adjust your plan.",
                        tool_call.name
                    ),
                    is_error: true,
                };
            }
            PipelineDecision::Allow(_) => {
                // Permission granted — proceed to execution
            }
            PipelineDecision::Ask { .. } => {
                // Interactive prompt handled inside pipeline's conversational gate
            }
        }
    }

    render_sink.tool_start(&tool_call.name, &tool_call.input);

    let (content, is_error) =
        match tokio::time::timeout(tool_timeout, tool.execute(input)).await {
            Ok(Ok(output)) => (output.content, output.is_error),
            Ok(Err(e)) => (format!("Tool error: {e}"), true),
            Err(_) => (
                format!("Tool '{}' timed out after {tool_timeout:?}", tool_call.name),
                true,
            ),
        };

    ContentBlock::ToolResult {
        tool_use_id: tool_call.id.clone(),
        content,
        is_error,
    }
}

// ── Synthetic cancel result ──────────────────────────────────────────────────

fn synthetic_cancel_result(tu: &CompletedToolUse) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: tu.id.clone(),
        content: "Interrupted by user".to_owned(),
        is_error: true,
    }
}
