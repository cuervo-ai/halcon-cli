//! Parallel tool executor: partitions tools by permission level and executes
//! ReadOnly tools concurrently via `futures::join_all`, while Destructive/ReadWrite
//! tools requiring permission run sequentially.

use std::time::{Duration, Instant};

use futures::stream::StreamExt as _;

use chrono::Utc;

use cuervo_core::types::{
    ContentBlock, DomainEvent, EventPayload, PermissionDecision, PermissionLevel, ToolInput,
};
use cuervo_core::EventSender;
use cuervo_storage::{AsyncDatabase, TraceStep, TraceStepType};
use cuervo_tools::ToolRegistry;

use cuervo_core::types::ToolRetryConfig;

use super::accumulator::CompletedToolUse;
use super::idempotency::DryRunMode;
use super::permissions::PermissionChecker;
use crate::render::sink::RenderSink;

/// Configuration for tool execution (dry-run + idempotency).
///
/// Introduced in Phase 16 to avoid cascading parameter changes.
/// Pass `&ToolExecutionConfig::default()` for normal execution.
pub struct ToolExecutionConfig<'a> {
    /// Dry-run mode controls which tools are actually executed.
    pub dry_run_mode: DryRunMode,
    /// Optional idempotency registry for deduplicating identical tool calls.
    /// Wired in Sub-Phase 16.1.
    pub idempotency: Option<&'a super::idempotency::IdempotencyRegistry>,
    /// Tool retry configuration for transient failures.
    pub retry: ToolRetryConfig,
}

impl Default for ToolExecutionConfig<'_> {
    fn default() -> Self {
        Self {
            dry_run_mode: DryRunMode::Off,
            idempotency: None,
            retry: ToolRetryConfig::default(),
        }
    }
}

/// Result of executing one tool.
pub struct ToolExecResult {
    pub tool_use_id: String,
    pub tool_name: String,
    pub content_block: ContentBlock,
    pub duration_ms: u64,
    pub was_parallel: bool,
}

/// Plan for executing a batch of tools.
pub struct ToolExecutionPlan {
    /// ReadOnly tools that can be executed concurrently.
    pub parallel_batch: Vec<CompletedToolUse>,
    /// Tools that require sequential execution (permission prompt or destructive).
    pub sequential_batch: Vec<CompletedToolUse>,
}

/// Partition completed tool uses into parallel and sequential batches.
pub fn plan_execution(
    tools: Vec<CompletedToolUse>,
    registry: &ToolRegistry,
) -> ToolExecutionPlan {
    let mut parallel = Vec::new();
    let mut sequential = Vec::new();

    for tool_call in tools {
        let can_parallel = if let Some(tool) = registry.get(&tool_call.name) {
            let level = tool.permission_level();
            // ReadOnly tools are always auto-allowed, safe to parallelize.
            // ReadWrite tools are auto-allowed too, but they mutate state — keep sequential.
            level == PermissionLevel::ReadOnly
        } else {
            // Unknown tools go sequential (will produce error anyway).
            false
        };

        if can_parallel {
            parallel.push(tool_call);
        } else {
            sequential.push(tool_call);
        }
    }

    ToolExecutionPlan {
        parallel_batch: parallel,
        sequential_batch: sequential,
    }
}

/// Build a synthetic dry-run result for a tool that was skipped.
fn synthetic_dry_run_result(tool_call: &CompletedToolUse) -> ToolExecResult {
    ToolExecResult {
        tool_use_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        content_block: ContentBlock::ToolResult {
            tool_use_id: tool_call.id.clone(),
            content: format!("[dry-run] Tool '{}' skipped (would execute with: {})",
                tool_call.name,
                serde_json::to_string(&tool_call.input).unwrap_or_default(),
            ),
            is_error: false,
        },
        duration_ms: 0,
        was_parallel: false,
    }
}

/// Check if an error message indicates a transient failure that can be retried.
fn is_transient_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("429")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("broken pipe")
        || lower.contains("temporary")
}

/// Check if an error is deterministic (will never succeed on retry/replan).
///
/// These errors indicate permanent conditions: missing files, bad permissions,
/// invalid schemas, billing/auth failures, etc. Retrying will produce the same result.
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
}

/// Apply ±20% jitter to a delay to prevent thundering herd.
fn jittered_delay(delay_ms: u64) -> u64 {
    use rand::Rng;
    let jitter_factor = 0.8 + rand::rng().random_range(0.0..0.4);
    (delay_ms as f64 * jitter_factor) as u64
}

/// Execute a single tool (used for both parallel and sequential paths).
async fn execute_one_tool(
    tool_call: &CompletedToolUse,
    registry: &ToolRegistry,
    working_dir: &str,
    tool_timeout: Duration,
    dry_run_mode: DryRunMode,
    idempotency: Option<&super::idempotency::IdempotencyRegistry>,
    retry_config: &ToolRetryConfig,
    render_sink: &dyn RenderSink,
) -> ToolExecResult {
    let Some(tool) = registry.get(&tool_call.name) else {
        return ToolExecResult {
            tool_use_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content_block: ContentBlock::ToolResult {
                tool_use_id: tool_call.id.clone(),
                content: format!("Error: unknown tool '{}'", tool_call.name),
                is_error: true,
            },
            duration_ms: 0,
            was_parallel: false,
        };
    };

    // Dry-run mode: skip execution based on mode + permission level.
    match dry_run_mode {
        DryRunMode::Off => { /* Normal execution, fall through. */ }
        DryRunMode::Full => {
            return synthetic_dry_run_result(tool_call);
        }
        DryRunMode::DestructiveOnly => {
            let perm_level = tool.permission_level();
            if perm_level >= PermissionLevel::ReadWrite {
                return synthetic_dry_run_result(tool_call);
            }
        }
    }

    // Idempotency check: return cached result if this exact call was already executed.
    let exec_id = if let Some(reg) = idempotency {
        let id = super::idempotency::compute_execution_id(&tool_call.name, &tool_call.input, "");
        if let Some(cached) = reg.lookup(&id) {
            return ToolExecResult {
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
        }
        Some(id)
    } else {
        None
    };

    // Reject poisoned tool arguments from accumulator parse failures (RC-4).
    if let Some(parse_err) = tool_call.input.get("_parse_error") {
        let err_msg = parse_err.as_str().unwrap_or("unknown parse error");
        tracing::error!(
            tool = %tool_call.name,
            tool_use_id = %tool_call.id,
            parse_error = %err_msg,
            "Rejecting tool call with malformed arguments from streaming parse failure"
        );
        return ToolExecResult {
            tool_use_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content_block: ContentBlock::ToolResult {
                tool_use_id: tool_call.id.clone(),
                content: format!(
                    "Error: tool arguments were corrupted during streaming (parse error: {err_msg}). \
                     The model's tool call was truncated or malformed. Please retry."
                ),
                is_error: true,
            },
            duration_ms: 0,
            was_parallel: false,
        };
    }

    let start = Instant::now();
    let max_attempts = retry_config.max_retries + 1; // 1 initial + N retries

    let mut exec_result = None;
    for attempt in 0..max_attempts {
        let tool_input = ToolInput {
            tool_use_id: tool_call.id.clone(),
            arguments: tool_call.input.clone(),
            working_directory: working_dir.to_string(),
        };

        let result = tokio::time::timeout(tool_timeout, tool.execute(tool_input)).await;

        match result {
            Ok(Ok(output)) => {
                exec_result = Some(ToolExecResult {
                    tool_use_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    content_block: ContentBlock::ToolResult {
                        tool_use_id: tool_call.id.clone(),
                        content: output.content,
                        is_error: output.is_error,
                    },
                    duration_ms: start.elapsed().as_millis() as u64,
                    was_parallel: false,
                });
                break;
            }
            Ok(Err(e)) => {
                let err_str = format!("{e}");
                if attempt + 1 < max_attempts && is_transient_error(&err_str) {
                    let base = std::cmp::min(
                        retry_config.base_delay_ms * 2u64.pow(std::cmp::min(attempt, 5)),
                        retry_config.max_delay_ms,
                    );
                    let delay = jittered_delay(base);
                    tracing::info!(
                        tool = %tool_call.name,
                        attempt = attempt + 1,
                        delay_ms = delay,
                        "Retrying transient tool error: {err_str}"
                    );
                    render_sink.tool_retrying(&tool_call.name, (attempt + 1) as usize, max_attempts as usize, delay);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    continue;
                }
                exec_result = Some(ToolExecResult {
                    tool_use_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    content_block: ContentBlock::ToolResult {
                        tool_use_id: tool_call.id.clone(),
                        content: format!("Error: {e}"),
                        is_error: true,
                    },
                    duration_ms: start.elapsed().as_millis() as u64,
                    was_parallel: false,
                });
                break;
            }
            Err(_elapsed) => {
                let err_str = format!(
                    "Error: tool '{}' timed out after {}s",
                    tool_call.name,
                    tool_timeout.as_secs()
                );
                if attempt + 1 < max_attempts && is_transient_error(&err_str) {
                    let delay = std::cmp::min(
                        retry_config.base_delay_ms * 2u64.pow(std::cmp::min(attempt, 5)),
                        retry_config.max_delay_ms,
                    );
                    tracing::info!(
                        tool = %tool_call.name,
                        attempt = attempt + 1,
                        delay_ms = delay,
                        "Retrying timed out tool"
                    );
                    render_sink.tool_retrying(&tool_call.name, (attempt + 1) as usize, max_attempts as usize, delay);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    continue;
                }
                exec_result = Some(ToolExecResult {
                    tool_use_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    content_block: ContentBlock::ToolResult {
                        tool_use_id: tool_call.id.clone(),
                        content: err_str,
                        is_error: true,
                    },
                    duration_ms: start.elapsed().as_millis() as u64,
                    was_parallel: false,
                });
                break;
            }
        }
    }

    // Unwrap is safe: the loop always sets exec_result before break.
    let exec_result = exec_result.unwrap();

    // Record result in idempotency registry for future deduplication.
    if let (Some(registry), Some(id)) = (idempotency, exec_id) {
        let (content, is_error) = match &exec_result.content_block {
            ContentBlock::ToolResult { content, is_error, .. } => (content.clone(), *is_error),
            _ => (String::new(), false),
        };
        registry.record(super::idempotency::ExecutionRecord {
            execution_id: id,
            tool_name: tool_call.name.clone(),
            result_content: content,
            is_error,
            executed_at: chrono::Utc::now(),
        });
    }

    exec_result
}

/// Execute the parallel batch concurrently with a concurrency cap.
///
/// Uses `buffer_unordered` to limit the number of concurrent tool executions.
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

    // Launch all tools concurrently.
    let dry_run_mode = exec_config.dry_run_mode;
    let futures: Vec<_> = batch
        .iter()
        .map(|tool_call| {
            let name = tool_call.name.clone();
            let input = tool_call.input.clone();
            render_sink.tool_start(&name, &input);
            execute_one_tool(tool_call, registry, working_dir, tool_timeout, dry_run_mode, exec_config.idempotency, &exec_config.retry, render_sink)
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

    // Sort by tool_use_id for deterministic ordering.
    results.sort_by(|a, b| a.tool_use_id.cmp(&b.tool_use_id));
    results
}

/// Execute a single tool sequentially (with permission check).
#[allow(clippy::too_many_arguments)]
pub async fn execute_sequential_tool(
    tool_call: &CompletedToolUse,
    registry: &ToolRegistry,
    permissions: &mut PermissionChecker,
    working_dir: &str,
    tool_timeout: Duration,
    event_tx: &EventSender,
    trace_db: Option<&AsyncDatabase>,
    session_id: uuid::Uuid,
    trace_step_index: &mut u32,
    exec_config: &ToolExecutionConfig<'_>,
    render_sink: &dyn RenderSink,
) -> ToolExecResult {
    let Some(tool) = registry.get(&tool_call.name) else {
        return ToolExecResult {
            tool_use_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content_block: ContentBlock::ToolResult {
                tool_use_id: tool_call.id.clone(),
                content: format!("Error: unknown tool '{}'", tool_call.name),
                is_error: true,
            },
            duration_ms: 0,
            was_parallel: false,
        };
    };

    let perm_level = tool.permission_level();

    // Dry-run bypass: skip permission flow for tools that would be dry-run skipped.
    match exec_config.dry_run_mode {
        DryRunMode::Off => { /* Normal execution, fall through. */ }
        DryRunMode::Full => {
            return synthetic_dry_run_result(tool_call);
        }
        DryRunMode::DestructiveOnly => {
            if perm_level >= PermissionLevel::ReadWrite {
                return synthetic_dry_run_result(tool_call);
            }
        }
    }

    let tool_input = ToolInput {
        tool_use_id: tool_call.id.clone(),
        arguments: tool_call.input.clone(),
        working_directory: working_dir.to_string(),
    };

    // TBAC check (before legacy permission check).
    {
        use cuervo_core::types::AuthzDecision;
        match permissions.check_tbac(&tool_call.name, &tool_call.input) {
            AuthzDecision::Allowed { .. } => {
                // TBAC allowed — continue to legacy permission check.
            }
            AuthzDecision::NoContext => {
                // No TBAC context — fall through to legacy.
            }
            AuthzDecision::ToolNotAllowed { ref tool, .. }
            | AuthzDecision::ParamViolation { ref tool, .. } => {
                tracing::info!(tool = %tool, "TBAC denied");
                return ToolExecResult {
                    tool_use_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    content_block: ContentBlock::ToolResult {
                        tool_use_id: tool_call.id.clone(),
                        content: format!(
                            "Error: tool '{}' denied by task context policy",
                            tool_call.name
                        ),
                        is_error: true,
                    },
                    duration_ms: 0,
                    was_parallel: false,
                };
            }
            AuthzDecision::ContextInvalid { reason, .. } => {
                tracing::info!(tool = %tool_call.name, reason = %reason, "TBAC context invalid");
                return ToolExecResult {
                    tool_use_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    content_block: ContentBlock::ToolResult {
                        tool_use_id: tool_call.id.clone(),
                        content: format!(
                            "Error: task context expired or exhausted for tool '{}'",
                            tool_call.name
                        ),
                        is_error: true,
                    },
                    duration_ms: 0,
                    was_parallel: false,
                };
            }
        }
    }

    // Emit permission event for destructive tools.
    if perm_level >= PermissionLevel::Destructive {
        let _ = event_tx.send(DomainEvent::new(EventPayload::PermissionRequested {
            tool: tool_call.name.clone(),
            level: perm_level,
        }));
    }

    // Emit permission-awaiting event for destructive tools.
    if perm_level == cuervo_core::types::PermissionLevel::Destructive {
        render_sink.permission_awaiting(&tool_call.name);
        // Phase E5: Transition to ToolWait while awaiting permission.
        render_sink.agent_state_transition("executing", "tool_wait", "awaiting permission");
    }

    let decision = permissions
        .authorize(&tool_call.name, perm_level, &tool_input)
        .await;

    // Phase E5: Transition back from ToolWait after permission decision.
    if perm_level == cuervo_core::types::PermissionLevel::Destructive {
        render_sink.agent_state_transition("tool_wait", "executing", "permission decided");
    }

    if decision == PermissionDecision::Denied {
        let _ = event_tx.send(DomainEvent::new(EventPayload::PermissionDenied {
            tool: tool_call.name.clone(),
            level: perm_level,
        }));
        render_sink.tool_denied(&tool_call.name);
        return ToolExecResult {
            tool_use_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content_block: ContentBlock::ToolResult {
                tool_use_id: tool_call.id.clone(),
                content: "Error: user denied permission".into(),
                is_error: true,
            },
            duration_ms: 0,
            was_parallel: false,
        };
    }

    if perm_level >= PermissionLevel::Destructive {
        let _ = event_tx.send(DomainEvent::new(EventPayload::PermissionGranted {
            tool: tool_call.name.clone(),
            level: perm_level,
        }));
    }

    // Trace: record tool call.
    if let Some(db) = trace_db {
        let step = TraceStep {
            session_id,
            step_index: *trace_step_index,
            step_type: TraceStepType::ToolCall,
            data_json: serde_json::json!({
                "tool_use_id": &tool_call.id,
                "tool_name": &tool_call.name,
                "input": &tool_call.input,
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

    render_sink.tool_start(&tool_call.name, &tool_call.input);

    let result = execute_one_tool(tool_call, registry, working_dir, tool_timeout, exec_config.dry_run_mode, exec_config.idempotency, &exec_config.retry, render_sink).await;
    let is_error = matches!(&result.content_block,
        ContentBlock::ToolResult { is_error, .. } if *is_error);

    let _ = event_tx.send(DomainEvent::new(EventPayload::ToolExecuted {
        tool: tool_call.name.clone(),
        permission: perm_level,
        duration_ms: result.duration_ms,
        success: !is_error,
    }));

    render_sink.tool_output(&result.content_block, result.duration_ms);

    // Trace: record tool result.
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
                "tool_use_id": &tool_call.id,
                "tool_name": &tool_call.name,
                "content": content,
                "is_error": is_error,
                "duration_ms": result.duration_ms,
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

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::sink::SilentSink;

    static TEST_SINK: std::sync::LazyLock<SilentSink> =
        std::sync::LazyLock::new(SilentSink::new);

    fn make_completed(id: &str, name: &str) -> CompletedToolUse {
        CompletedToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: serde_json::json!({}),
        }
    }

    #[test]
    fn plan_partitions_readonly_vs_destructive() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let tools = vec![
            make_completed("t1", "file_read"),
            make_completed("t2", "bash"),
            make_completed("t3", "grep"),
            make_completed("t4", "file_write"),
        ];

        let plan = plan_execution(tools, &registry);

        // file_read and grep are ReadOnly -> parallel
        let par_names: Vec<&str> = plan.parallel_batch.iter().map(|t| t.name.as_str()).collect();
        assert!(par_names.contains(&"file_read"));
        assert!(par_names.contains(&"grep"));

        // bash is Destructive, file_write is Destructive -> sequential
        let seq_names: Vec<&str> = plan.sequential_batch.iter().map(|t| t.name.as_str()).collect();
        assert!(seq_names.contains(&"bash"));
        assert!(seq_names.contains(&"file_write"));
    }

    #[test]
    fn plan_unknown_tool_goes_sequential() {
        let registry = ToolRegistry::new();


        let tools = vec![make_completed("t1", "nonexistent_tool")];
        let plan = plan_execution(tools, &registry);

        assert!(plan.parallel_batch.is_empty());
        assert_eq!(plan.sequential_batch.len(), 1);
    }

    #[test]
    fn plan_all_readonly_all_parallel() {
        let registry = cuervo_tools::default_registry(&Default::default());


        let tools = vec![
            make_completed("t1", "file_read"),
            make_completed("t2", "glob"),
            make_completed("t3", "grep"),
        ];

        let plan = plan_execution(tools, &registry);
        assert_eq!(plan.parallel_batch.len(), 3);
        assert!(plan.sequential_batch.is_empty());
    }

    #[test]
    fn plan_all_destructive_all_sequential() {
        let registry = cuervo_tools::default_registry(&Default::default());


        let tools = vec![
            make_completed("t1", "bash"),
            make_completed("t2", "file_write"),
            make_completed("t3", "file_edit"),
        ];

        let plan = plan_execution(tools, &registry);
        assert!(plan.parallel_batch.is_empty());
        assert_eq!(plan.sequential_batch.len(), 3);
    }

    #[test]
    fn plan_empty_input() {
        let registry = ToolRegistry::new();


        let plan = plan_execution(vec![], &registry);
        assert!(plan.parallel_batch.is_empty());
        assert!(plan.sequential_batch.is_empty());
    }

    #[tokio::test]
    async fn execute_parallel_batch_empty_returns_empty() {
        let registry = ToolRegistry::new();
        let (event_tx, _rx) = cuervo_core::event_bus(16);
        let mut idx = 0u32;

        let results = execute_parallel_batch(
            &[],
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
        )
        .await;

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn execute_parallel_batch_unknown_tool() {
        let registry = ToolRegistry::new();
        let (event_tx, _rx) = cuervo_core::event_bus(16);
        let mut idx = 0u32;

        let batch = vec![make_completed("t1", "nonexistent")];
        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
        )
        .await;

        assert_eq!(results.len(), 1);
        match &results[0].content_block {
            ContentBlock::ToolResult { is_error, content, .. } => {
                assert!(is_error);
                assert!(content.contains("unknown tool"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn parallel_results_sorted_by_id() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let (event_tx, _rx) = cuervo_core::event_bus(16);
        let mut idx = 0u32;

        let batch = vec![
            make_completed("z_last", "file_read"),
            make_completed("a_first", "file_read"),
            make_completed("m_middle", "file_read"),
        ];

        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
        )
        .await;

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].tool_use_id, "a_first");
        assert_eq!(results[1].tool_use_id, "m_middle");
        assert_eq!(results[2].tool_use_id, "z_last");
    }

    #[tokio::test]
    async fn parallel_results_marked_as_parallel() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let (event_tx, _rx) = cuervo_core::event_bus(16);
        let mut idx = 0u32;

        let batch = vec![make_completed("t1", "file_read")];
        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
        )
        .await;

        assert!(results[0].was_parallel);
    }

    #[tokio::test]
    async fn parallel_batch_with_trace_recording() {
        use std::sync::Arc;
        use cuervo_storage::Database;

        let registry = cuervo_tools::default_registry(&Default::default());
        let (event_tx, _rx) = cuervo_core::event_bus(16);
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
        let session_id = uuid::Uuid::new_v4();
        let mut idx = 0u32;

        let batch = vec![
            make_completed("t1", "file_read"),
            make_completed("t2", "grep"),
        ];

        execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            Some(&db),
            session_id,
            &mut idx,
            10,
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
        )
        .await;

        // Should have recorded: 1 batch step + 2 individual result steps = 3.
        let steps = db.inner().load_trace_steps(session_id).unwrap();
        assert_eq!(steps.len(), 3);
        // First step is the batch metadata.
        assert!(steps[0].data_json.contains("parallel_batch"));
    }

    #[tokio::test]
    async fn parallel_concurrency_limit_enforced() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let (event_tx, _rx) = cuervo_core::event_bus(16);
        let mut idx = 0u32;

        // Create a large batch (20 tools) with concurrency cap of 10.
        let batch: Vec<_> = (0..20)
            .map(|i| make_completed(&format!("t{}", i), "file_read"))
            .collect();

        let start = std::time::Instant::now();
        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(30),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10, // Concurrency cap of 10
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
        )
        .await;

        // All 20 tools should complete.
        assert_eq!(results.len(), 20);
        // All should have tool_use_ids and results.
        assert!(results.iter().all(|r| !r.tool_use_id.is_empty()));
        assert!(results.iter().all(|r| r.tool_name == "file_read"));
        // Execution should complete (buffer_unordered prevents stall).
        assert!(start.elapsed().as_secs() < 25);
    }

    #[tokio::test]
    async fn parallel_concurrency_cap_zero_defaults_to_one() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let (event_tx, _rx) = cuervo_core::event_bus(16);
        let mut idx = 0u32;

        let batch = vec![
            make_completed("t1", "file_read"),
            make_completed("t2", "file_read"),
        ];

        // max_parallel_tools=0 should default to 1 (no panic, still completes).
        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            0, // 0 defaults to 1
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
        )
        .await;

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.was_parallel));
    }

    // --- Sub-Phase 16.0: Dry-run mode tests ---

    #[tokio::test]
    async fn dry_run_off_executes_normally() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let tool = make_completed("t1", "file_read");
        let result = execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, None, &ToolRetryConfig::default(), &*TEST_SINK).await;
        // file_read on non-existent path produces an error, but it DID execute (not a dry-run skip).
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(!content.contains("[dry-run]"), "Off mode should execute normally");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn dry_run_full_skips_all_tools() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let tool = make_completed("t1", "file_read");
        let result = execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Full, None, &ToolRetryConfig::default(), &*TEST_SINK).await;
        match &result.content_block {
            ContentBlock::ToolResult { content, is_error, .. } => {
                assert!(content.contains("[dry-run]"));
                assert!(content.contains("file_read"));
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn dry_run_full_returns_synthetic_result() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let tool = make_completed("t1", "bash");
        let result = execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Full, None, &ToolRetryConfig::default(), &*TEST_SINK).await;
        assert_eq!(result.duration_ms, 0);
        assert_eq!(result.tool_name, "bash");
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("[dry-run]"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn dry_run_destructive_only_skips_bash() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let tool = make_completed("t1", "bash");
        let result = execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::DestructiveOnly, None, &ToolRetryConfig::default(), &*TEST_SINK).await;
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("[dry-run]"), "bash should be skipped in DestructiveOnly mode");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn dry_run_destructive_only_allows_read_file() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let tool = make_completed("t1", "file_read");
        let result = execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::DestructiveOnly, None, &ToolRetryConfig::default(), &*TEST_SINK).await;
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(!content.contains("[dry-run]"), "file_read should execute in DestructiveOnly mode");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn synthetic_result_contains_tool_name() {
        let tool = make_completed("t1", "file_write");
        let result = synthetic_dry_run_result(&tool);
        assert_eq!(result.tool_name, "file_write");
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("file_write"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn synthetic_result_is_not_error() {
        let tool = make_completed("t1", "bash");
        let result = synthetic_dry_run_result(&tool);
        match &result.content_block {
            ContentBlock::ToolResult { is_error, .. } => {
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn tool_execution_config_default_is_off() {
        let config = ToolExecutionConfig::default();
        assert_eq!(config.dry_run_mode, DryRunMode::Off);
        assert!(config.idempotency.is_none());
    }

    #[tokio::test]
    async fn execute_parallel_with_dry_run_full() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let (event_tx, _rx) = cuervo_core::event_bus(16);
        let mut idx = 0u32;

        let batch = vec![
            make_completed("t1", "file_read"),
            make_completed("t2", "grep"),
        ];

        let config = ToolExecutionConfig {
            dry_run_mode: DryRunMode::Full,
            idempotency: None,
            ..Default::default()
        };

        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &config,
            &*TEST_SINK,
        )
        .await;

        assert_eq!(results.len(), 2);
        for result in &results {
            match &result.content_block {
                ContentBlock::ToolResult { content, .. } => {
                    assert!(content.contains("[dry-run]"));
                }
                _ => panic!("expected ToolResult"),
            }
        }
    }

    #[tokio::test]
    async fn execute_sequential_with_dry_run_full() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let (event_tx, _rx) = cuervo_core::event_bus(16);
        let mut idx = 0u32;
        let mut perms = PermissionChecker::new(true);

        let tool = make_completed("t1", "bash");
        let config = ToolExecutionConfig {
            dry_run_mode: DryRunMode::Full,
            idempotency: None,
            ..Default::default()
        };

        let result = execute_sequential_tool(
            &tool,
            &registry,
            &mut perms,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            &config,
            &*TEST_SINK,
        )
        .await;

        match &result.content_block {
            ContentBlock::ToolResult { content, is_error, .. } => {
                assert!(content.contains("[dry-run]"));
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn execute_sequential_with_dry_run_destructive_only() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let (event_tx, _rx) = cuervo_core::event_bus(16);
        let mut idx = 0u32;
        let mut perms = PermissionChecker::new(true);

        // file_write is Destructive — should be skipped in DestructiveOnly mode.
        let tool = make_completed("t1", "file_write");
        let config = ToolExecutionConfig {
            dry_run_mode: DryRunMode::DestructiveOnly,
            idempotency: None,
            ..Default::default()
        };

        let result = execute_sequential_tool(
            &tool,
            &registry,
            &mut perms,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            &config,
            &*TEST_SINK,
        )
        .await;

        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("[dry-run]"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn tool_execution_config_with_idempotency_none() {
        let config = ToolExecutionConfig {
            dry_run_mode: DryRunMode::Off,
            idempotency: None,
            ..Default::default()
        };
        assert_eq!(config.dry_run_mode, DryRunMode::Off);
        assert!(config.idempotency.is_none());
    }

    // --- Sub-Phase 16.1: Idempotency tests ---

    #[tokio::test]
    async fn idempotency_deduplicates_identical_tool_call() {
        use crate::repl::idempotency::IdempotencyRegistry;

        let registry = cuervo_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        let tool = CompletedToolUse {
            id: "t1".to_string(),
            name: "file_read".to_string(),
            input: serde_json::json!({"path": "/tmp/nonexistent_test_file_abc123"}),
        };

        // First call: executes and records.
        let r1 = execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, Some(&idem), &ToolRetryConfig::default(), &*TEST_SINK).await;
        assert_eq!(idem.len(), 1);

        // Second call with same args: returns cached result.
        let r2 = execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, Some(&idem), &ToolRetryConfig::default(), &*TEST_SINK).await;
        assert_eq!(idem.len(), 1); // No new record.

        // Both should have the same content.
        let c1 = match &r1.content_block { ContentBlock::ToolResult { content, .. } => content.clone(), _ => String::new() };
        let c2 = match &r2.content_block { ContentBlock::ToolResult { content, .. } => content.clone(), _ => String::new() };
        assert_eq!(c1, c2);
    }

    #[tokio::test]
    async fn idempotency_different_args_not_cached() {
        use crate::repl::idempotency::IdempotencyRegistry;

        let registry = cuervo_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        let tool1 = CompletedToolUse {
            id: "t1".to_string(),
            name: "file_read".to_string(),
            input: serde_json::json!({"path": "/tmp/aaa"}),
        };
        let tool2 = CompletedToolUse {
            id: "t2".to_string(),
            name: "file_read".to_string(),
            input: serde_json::json!({"path": "/tmp/bbb"}),
        };

        execute_one_tool(&tool1, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, Some(&idem), &ToolRetryConfig::default(), &*TEST_SINK).await;
        execute_one_tool(&tool2, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, Some(&idem), &ToolRetryConfig::default(), &*TEST_SINK).await;
        assert_eq!(idem.len(), 2); // Two distinct entries.
    }

    #[tokio::test]
    async fn idempotency_records_after_execution() {
        use crate::repl::idempotency::{IdempotencyRegistry, compute_execution_id};

        let registry = cuervo_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        let tool = make_completed("t1", "file_read");
        let exec_id = compute_execution_id("file_read", &serde_json::json!({}), "");

        assert!(idem.lookup(&exec_id).is_none());
        execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, Some(&idem), &ToolRetryConfig::default(), &*TEST_SINK).await;
        assert!(idem.lookup(&exec_id).is_some());
    }

    #[tokio::test]
    async fn idempotency_returns_cached_content() {
        use crate::repl::idempotency::{IdempotencyRegistry, ExecutionRecord};

        let registry = cuervo_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        // Pre-seed the registry with a fake cached result.
        let exec_id = crate::repl::idempotency::compute_execution_id("file_read", &serde_json::json!({}), "");
        idem.record(ExecutionRecord {
            execution_id: exec_id,
            tool_name: "file_read".to_string(),
            result_content: "cached output".to_string(),
            is_error: false,
            executed_at: chrono::Utc::now(),
        });

        let tool = make_completed("t1", "file_read");
        let result = execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, Some(&idem), &ToolRetryConfig::default(), &*TEST_SINK).await;
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content, "cached output");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn idempotency_none_executes_normally() {
        let registry = cuervo_tools::default_registry(&Default::default());
        let tool = make_completed("t1", "file_read");
        // No idempotency (None) — should execute normally.
        let result = execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, None, &ToolRetryConfig::default(), &*TEST_SINK).await;
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(!content.contains("cached output"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn idempotency_registry_survives_multiple_rounds() {
        use crate::repl::idempotency::IdempotencyRegistry;

        let registry = cuervo_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        let tool = make_completed("t1", "file_read");
        // Round 1.
        execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, Some(&idem), &ToolRetryConfig::default(), &*TEST_SINK).await;
        // Round 2 (same tool).
        execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, Some(&idem), &ToolRetryConfig::default(), &*TEST_SINK).await;
        // Round 3 (same tool).
        execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, Some(&idem), &ToolRetryConfig::default(), &*TEST_SINK).await;
        assert_eq!(idem.len(), 1); // Still just 1 entry.
    }

    #[tokio::test]
    async fn idempotency_with_dry_run_no_record() {
        use crate::repl::idempotency::IdempotencyRegistry;

        let registry = cuervo_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        let tool = make_completed("t1", "file_read");
        // Dry-run full: should NOT record to idempotency (tool didn't execute).
        execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Full, Some(&idem), &ToolRetryConfig::default(), &*TEST_SINK).await;
        assert!(idem.is_empty(), "dry-run should not record to idempotency registry");
    }

    #[tokio::test]
    async fn idempotency_error_result_also_cached() {
        use crate::repl::idempotency::IdempotencyRegistry;

        let registry = cuervo_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        // file_read on non-existent path → error result.
        let tool = CompletedToolUse {
            id: "t1".to_string(),
            name: "file_read".to_string(),
            input: serde_json::json!({"path": "/tmp/nonexistent_xyz_987654"}),
        };
        let r1 = execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, Some(&idem), &ToolRetryConfig::default(), &*TEST_SINK).await;
        assert_eq!(idem.len(), 1);

        // Second call returns cached error.
        let r2 = execute_one_tool(&tool, &registry, "/tmp", Duration::from_secs(10), DryRunMode::Off, Some(&idem), &ToolRetryConfig::default(), &*TEST_SINK).await;
        let e1 = matches!(&r1.content_block, ContentBlock::ToolResult { is_error, .. } if *is_error);
        let e2 = matches!(&r2.content_block, ContentBlock::ToolResult { is_error, .. } if *is_error);
        assert_eq!(e1, e2);
    }

    #[test]
    fn compute_execution_id_in_executor_matches() {
        use crate::repl::idempotency::compute_execution_id;
        let id1 = compute_execution_id("bash", &serde_json::json!({"cmd": "ls"}), "");
        let id2 = compute_execution_id("bash", &serde_json::json!({"cmd": "ls"}), "");
        assert_eq!(id1, id2);
    }

    #[tokio::test]
    async fn idempotency_parallel_batch_dedup() {
        use crate::repl::idempotency::IdempotencyRegistry;

        let registry = cuervo_tools::default_registry(&Default::default());
        let (event_tx, _rx) = cuervo_core::event_bus(16);
        let mut idx = 0u32;
        let idem = IdempotencyRegistry::new();

        // Two identical file_read calls in a parallel batch.
        let batch = vec![
            CompletedToolUse { id: "t1".to_string(), name: "file_read".to_string(), input: serde_json::json!({"path": "/tmp"}) },
            CompletedToolUse { id: "t2".to_string(), name: "file_read".to_string(), input: serde_json::json!({"path": "/tmp"}) },
        ];

        let config = ToolExecutionConfig {
            dry_run_mode: DryRunMode::Off,
            idempotency: Some(&idem),
            ..Default::default()
        };

        let results = execute_parallel_batch(
            &batch, &registry, "/tmp", Duration::from_secs(10),
            &event_tx, None, uuid::Uuid::new_v4(), &mut idx, 10, &config, &*TEST_SINK,
        ).await;

        assert_eq!(results.len(), 2);
        // Only 1 entry in idempotency (deduped by same args).
        assert_eq!(idem.len(), 1);
    }

    // --- Phase 18: Tool retry tests ---

    #[test]
    fn transient_error_detection() {
        assert!(is_transient_error("connection timed out after 30s"));
        assert!(is_transient_error("rate_limit_exceeded"));
        assert!(is_transient_error("429 Too Many Requests"));
        assert!(is_transient_error("Connection reset by peer"));
        assert!(!is_transient_error("file not found: /tmp/missing.rs"));
        assert!(!is_transient_error("permission denied"));
    }

    #[test]
    fn tool_retry_config_defaults() {
        let config = ToolRetryConfig::default();
        assert_eq!(config.max_retries, 2);
        assert_eq!(config.base_delay_ms, 500);
        assert_eq!(config.max_delay_ms, 5000);
    }

    #[test]
    fn tool_retry_config_serde_roundtrip() {
        let config = ToolRetryConfig {
            max_retries: 5,
            base_delay_ms: 1000,
            max_delay_ms: 10000,
        };
        let json = serde_json::to_string(&config).unwrap();
        let rt: ToolRetryConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.max_retries, 5);
        assert_eq!(rt.base_delay_ms, 1000);
        assert_eq!(rt.max_delay_ms, 10000);
    }

    #[test]
    fn permanent_error_not_retried() {
        // Permanent errors should not be classified as transient.
        assert!(!is_transient_error("Error: unknown tool 'foo'"));
        assert!(!is_transient_error("Error: invalid JSON input"));
    }

    #[tokio::test]
    async fn max_retries_zero_executes_once() {
        // With max_retries=0, tool should execute exactly once (no retries).
        // Use an unknown tool to get a deterministic error.
        let registry = ToolRegistry::new();
        let tool = make_completed("t1", "nonexistent_tool");

        let no_retry = ToolRetryConfig {
            max_retries: 0,
            base_delay_ms: 10,
            max_delay_ms: 100,
        };

        let result = execute_one_tool(
            &tool, &registry, "/tmp", Duration::from_secs(10),
            DryRunMode::Off, None, &no_retry, &*TEST_SINK,
        ).await;

        // Unknown tool should return error (no retries attempted).
        match &result.content_block {
            ContentBlock::ToolResult { content, is_error, .. } => {
                assert!(*is_error);
                assert!(content.contains("unknown tool"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn backoff_increases_exponentially() {
        let config = ToolRetryConfig {
            max_retries: 5,
            base_delay_ms: 100,
            max_delay_ms: 5000,
        };
        // Base delays before jitter:
        // attempt 0: 100 * 2^0 = 100
        // attempt 1: 100 * 2^1 = 200
        // attempt 2: 100 * 2^2 = 400
        // attempt 3: 100 * 2^3 = 800
        let delays: Vec<u64> = (0..4)
            .map(|attempt| {
                std::cmp::min(
                    config.base_delay_ms * 2u64.pow(std::cmp::min(attempt, 5)),
                    config.max_delay_ms,
                )
            })
            .collect();
        assert_eq!(delays, vec![100, 200, 400, 800]);
    }

    #[test]
    fn jittered_delay_stays_within_bounds() {
        // ±20% jitter means 80% to 120% of base.
        for _ in 0..100 {
            let d = jittered_delay(1000);
            assert!(d >= 800 && d <= 1200, "jittered delay out of range: {d}");
        }
    }

    // ============================================================
    //  Phase 3: Tool Integration Audit Tests
    //  Tests the executor pipeline: tool_call → execute → result chain
    // ============================================================

    mod integration_audit {
        use super::*;

        fn make_tool_call(id: &str, name: &str, args: serde_json::Value) -> CompletedToolUse {
            CompletedToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: args,
            }
        }

        // --- tool_use_id chain integrity ---

        #[tokio::test]
        async fn tool_use_id_preserved_through_execution() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("test.txt");
            std::fs::write(&f, "hello integration").unwrap();

            let registry = cuervo_tools::default_registry(&Default::default());
            let unique_id = "toolu_integration_abc123";
            let tool_call = make_tool_call(
                unique_id,
                "file_read",
                serde_json::json!({"path": f.to_str().unwrap()}),
            );

            let result = execute_one_tool(
                &tool_call, &registry, dir.path().to_str().unwrap(),
                Duration::from_secs(10), DryRunMode::Off, None,
                &ToolRetryConfig::default(), &*TEST_SINK,
            ).await;

            assert_eq!(result.tool_use_id, unique_id);
            match &result.content_block {
                ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                    assert_eq!(tool_use_id, unique_id);
                    assert!(!is_error);
                    assert!(content.contains("hello integration"));
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn tool_use_id_preserved_on_error() {
            let registry = cuervo_tools::default_registry(&Default::default());
            let unique_id = "toolu_error_xyz789";
            let tool_call = make_tool_call(
                unique_id,
                "file_read",
                serde_json::json!({"path": "/nonexistent/path/file.txt"}),
            );

            let result = execute_one_tool(
                &tool_call, &registry, "/tmp",
                Duration::from_secs(10), DryRunMode::Off, None,
                &ToolRetryConfig::default(), &*TEST_SINK,
            ).await;

            assert_eq!(result.tool_use_id, unique_id);
            match &result.content_block {
                ContentBlock::ToolResult { tool_use_id, is_error, .. } => {
                    assert_eq!(tool_use_id, unique_id);
                    assert!(is_error);
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn tool_use_id_preserved_for_unknown_tool() {
            let registry = cuervo_tools::default_registry(&Default::default());
            let unique_id = "toolu_unknown_456";
            let tool_call = make_tool_call(unique_id, "nonexistent_tool", serde_json::json!({}));

            let result = execute_one_tool(
                &tool_call, &registry, "/tmp",
                Duration::from_secs(10), DryRunMode::Off, None,
                &ToolRetryConfig::default(), &*TEST_SINK,
            ).await;

            assert_eq!(result.tool_use_id, unique_id);
            match &result.content_block {
                ContentBlock::ToolResult { tool_use_id, is_error, content } => {
                    assert_eq!(tool_use_id, unique_id);
                    assert!(is_error);
                    assert!(content.contains("unknown tool"));
                }
                _ => panic!("expected ToolResult"),
            }
        }

        // --- Poisoned arg rejection (RC-4) ---

        #[tokio::test]
        async fn poisoned_args_rejected_immediately() {
            let registry = cuervo_tools::default_registry(&Default::default());
            let tool_call = CompletedToolUse {
                id: "toolu_poisoned".to_string(),
                name: "file_read".to_string(),
                input: serde_json::json!({"_parse_error": "truncated JSON at position 42"}),
            };

            let result = execute_one_tool(
                &tool_call, &registry, "/tmp",
                Duration::from_secs(10), DryRunMode::Off, None,
                &ToolRetryConfig::default(), &*TEST_SINK,
            ).await;

            assert_eq!(result.tool_use_id, "toolu_poisoned");
            match &result.content_block {
                ContentBlock::ToolResult { tool_use_id, is_error, content } => {
                    assert_eq!(tool_use_id, "toolu_poisoned");
                    assert!(is_error);
                    assert!(content.contains("corrupted"));
                    assert!(content.contains("parse error"));
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn poisoned_args_never_reach_tool() {
            let registry = cuervo_tools::default_registry(&Default::default());
            // If _parse_error is present, the tool should NOT execute.
            // We verify by using bash with a command that would succeed.
            let tool_call = CompletedToolUse {
                id: "toolu_no_exec".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({
                    "command": "echo this_should_never_run",
                    "_parse_error": "incomplete JSON"
                }),
            };

            let result = execute_one_tool(
                &tool_call, &registry, "/tmp",
                Duration::from_secs(10), DryRunMode::Off, None,
                &ToolRetryConfig::default(), &*TEST_SINK,
            ).await;

            match &result.content_block {
                ContentBlock::ToolResult { content, is_error, .. } => {
                    assert!(is_error);
                    assert!(!content.contains("this_should_never_run"));
                    assert!(content.contains("corrupted"));
                }
                _ => panic!("expected ToolResult"),
            }
        }

        // --- Parallel batch: real tool execution ---

        #[tokio::test]
        async fn parallel_batch_real_file_read() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("a.txt"), "content_a").unwrap();
            std::fs::write(dir.path().join("b.txt"), "content_b").unwrap();

            let registry = cuervo_tools::default_registry(&Default::default());
            let (event_tx, _rx) = cuervo_core::event_bus(16);
            let mut idx = 0u32;

            let batch = vec![
                make_tool_call("t1", "file_read", serde_json::json!({"path": dir.path().join("a.txt").to_str().unwrap()})),
                make_tool_call("t2", "file_read", serde_json::json!({"path": dir.path().join("b.txt").to_str().unwrap()})),
            ];

            let results = execute_parallel_batch(
                &batch, &registry, dir.path().to_str().unwrap(),
                Duration::from_secs(10), &event_tx, None,
                uuid::Uuid::new_v4(), &mut idx, 10,
                &ToolExecutionConfig::default(), &*TEST_SINK,
            ).await;

            assert_eq!(results.len(), 2);
            // Results sorted by id.
            assert_eq!(results[0].tool_use_id, "t1");
            assert_eq!(results[1].tool_use_id, "t2");

            // Both should have actual file content.
            for result in &results {
                match &result.content_block {
                    ContentBlock::ToolResult { is_error, content, .. } => {
                        assert!(!is_error, "tool_use_id={}: {content}", result.tool_use_id);
                        assert!(content.contains("content_"));
                    }
                    _ => panic!("expected ToolResult"),
                }
            }
        }

        #[tokio::test]
        async fn parallel_batch_mixed_success_and_error() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("exists.txt"), "ok").unwrap();

            let registry = cuervo_tools::default_registry(&Default::default());
            let (event_tx, _rx) = cuervo_core::event_bus(16);
            let mut idx = 0u32;

            let batch = vec![
                make_tool_call("success", "file_read", serde_json::json!({"path": dir.path().join("exists.txt").to_str().unwrap()})),
                make_tool_call("fail", "file_read", serde_json::json!({"path": "/nonexistent/file.txt"})),
            ];

            let results = execute_parallel_batch(
                &batch, &registry, dir.path().to_str().unwrap(),
                Duration::from_secs(10), &event_tx, None,
                uuid::Uuid::new_v4(), &mut idx, 10,
                &ToolExecutionConfig::default(), &*TEST_SINK,
            ).await;

            assert_eq!(results.len(), 2);

            // Find each by tool_use_id.
            let success_result = results.iter().find(|r| r.tool_use_id == "success").unwrap();
            let fail_result = results.iter().find(|r| r.tool_use_id == "fail").unwrap();

            match &success_result.content_block {
                ContentBlock::ToolResult { is_error, content, .. } => {
                    assert!(!is_error);
                    assert!(content.contains("ok"));
                }
                _ => panic!("expected ToolResult"),
            }

            match &fail_result.content_block {
                ContentBlock::ToolResult { is_error, .. } => {
                    assert!(is_error);
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn parallel_batch_with_unknown_tool_mixed() {
            let registry = cuervo_tools::default_registry(&Default::default());
            let (event_tx, _rx) = cuervo_core::event_bus(16);
            let mut idx = 0u32;

            // file_read is ReadOnly → parallel. But nonexistent goes sequential.
            // In a real plan, unknown would go sequential. Here we test parallel batch directly.
            let batch = vec![
                make_tool_call("valid", "glob", serde_json::json!({"pattern": "*.nonexistent_ext"})),
            ];

            let results = execute_parallel_batch(
                &batch, &registry, "/tmp",
                Duration::from_secs(10), &event_tx, None,
                uuid::Uuid::new_v4(), &mut idx, 10,
                &ToolExecutionConfig::default(), &*TEST_SINK,
            ).await;

            assert_eq!(results.len(), 1);
            assert_eq!(results[0].tool_use_id, "valid");
        }

        // --- Real tool execution through pipeline ---

        #[tokio::test]
        async fn real_bash_execution_through_executor() {
            let registry = cuervo_tools::default_registry(&Default::default());
            let tool_call = make_tool_call(
                "bash-exec-1",
                "bash",
                serde_json::json!({"command": "echo integration_test_output"}),
            );

            let result = execute_one_tool(
                &tool_call, &registry, "/tmp",
                Duration::from_secs(10), DryRunMode::Off, None,
                &ToolRetryConfig::default(), &*TEST_SINK,
            ).await;

            assert_eq!(result.tool_name, "bash");
            match &result.content_block {
                ContentBlock::ToolResult { content, is_error, .. } => {
                    assert!(!is_error);
                    assert!(content.contains("integration_test_output"));
                }
                _ => panic!("expected ToolResult"),
            }
            assert!(result.duration_ms > 0, "real execution should have non-zero duration");
        }

        #[tokio::test]
        async fn real_grep_execution_through_executor() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("search_target.txt"), "needle in haystack\nhaystack only\n").unwrap();

            let registry = cuervo_tools::default_registry(&Default::default());
            let tool_call = make_tool_call(
                "grep-exec-1",
                "grep",
                serde_json::json!({"pattern": "needle", "path": dir.path().to_str().unwrap()}),
            );

            let result = execute_one_tool(
                &tool_call, &registry, dir.path().to_str().unwrap(),
                Duration::from_secs(10), DryRunMode::Off, None,
                &ToolRetryConfig::default(), &*TEST_SINK,
            ).await;

            match &result.content_block {
                ContentBlock::ToolResult { content, is_error, .. } => {
                    assert!(!is_error);
                    assert!(content.contains("needle"));
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn real_file_write_then_read_roundtrip() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("roundtrip.txt");

            let registry = cuervo_tools::default_registry(&Default::default());

            // Write.
            let write_call = make_tool_call(
                "write-1",
                "file_write",
                serde_json::json!({"path": path.to_str().unwrap(), "content": "roundtrip_data"}),
            );
            let write_result = execute_one_tool(
                &write_call, &registry, dir.path().to_str().unwrap(),
                Duration::from_secs(10), DryRunMode::Off, None,
                &ToolRetryConfig::default(), &*TEST_SINK,
            ).await;

            match &write_result.content_block {
                ContentBlock::ToolResult { is_error, .. } => assert!(!is_error),
                _ => panic!("expected ToolResult"),
            }

            // Read back.
            let read_call = make_tool_call(
                "read-1",
                "file_read",
                serde_json::json!({"path": path.to_str().unwrap()}),
            );
            let read_result = execute_one_tool(
                &read_call, &registry, dir.path().to_str().unwrap(),
                Duration::from_secs(10), DryRunMode::Off, None,
                &ToolRetryConfig::default(), &*TEST_SINK,
            ).await;

            match &read_result.content_block {
                ContentBlock::ToolResult { content, is_error, .. } => {
                    assert!(!is_error);
                    assert!(content.contains("roundtrip_data"));
                }
                _ => panic!("expected ToolResult"),
            }
        }

        // --- Protocol correctness: every result is ToolResult ---

        #[tokio::test]
        async fn all_results_are_tool_result_variant() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("f.txt"), "data").unwrap();

            let registry = cuervo_tools::default_registry(&Default::default());
            let (event_tx, _rx) = cuervo_core::event_bus(16);
            let mut idx = 0u32;

            let batch = vec![
                make_tool_call("r1", "file_read", serde_json::json!({"path": dir.path().join("f.txt").to_str().unwrap()})),
                make_tool_call("r2", "glob", serde_json::json!({"pattern": "*.txt", "path": dir.path().to_str().unwrap()})),
                make_tool_call("r3", "directory_tree", serde_json::json!({"path": dir.path().to_str().unwrap()})),
            ];

            let results = execute_parallel_batch(
                &batch, &registry, dir.path().to_str().unwrap(),
                Duration::from_secs(10), &event_tx, None,
                uuid::Uuid::new_v4(), &mut idx, 10,
                &ToolExecutionConfig::default(), &*TEST_SINK,
            ).await;

            assert_eq!(results.len(), 3);
            for result in &results {
                match &result.content_block {
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        // Every result must have a tool_use_id that matches one of the inputs.
                        assert!(
                            ["r1", "r2", "r3"].contains(&tool_use_id.as_str()),
                            "unexpected tool_use_id: {tool_use_id}"
                        );
                    }
                    other => panic!("expected ToolResult, got {other:?}"),
                }
            }
        }

        // --- No orphan results ---

        #[tokio::test]
        async fn no_orphan_results_every_id_matches_input() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("a.txt"), "a").unwrap();

            let registry = cuervo_tools::default_registry(&Default::default());
            let (event_tx, _rx) = cuervo_core::event_bus(16);
            let mut idx = 0u32;

            let input_ids = vec!["id_alpha", "id_beta", "id_gamma"];
            let batch: Vec<CompletedToolUse> = input_ids.iter().map(|id| {
                make_tool_call(id, "file_read", serde_json::json!({"path": dir.path().join("a.txt").to_str().unwrap()}))
            }).collect();

            let results = execute_parallel_batch(
                &batch, &registry, dir.path().to_str().unwrap(),
                Duration::from_secs(10), &event_tx, None,
                uuid::Uuid::new_v4(), &mut idx, 10,
                &ToolExecutionConfig::default(), &*TEST_SINK,
            ).await;

            // Every result ID must match an input ID.
            let result_ids: Vec<&str> = results.iter().map(|r| r.tool_use_id.as_str()).collect();
            for id in &input_ids {
                assert!(result_ids.contains(id), "missing result for input id: {id}");
            }
            // No extra results.
            assert_eq!(results.len(), input_ids.len());
        }

        // --- Event emission ---

        #[tokio::test]
        async fn events_emitted_during_parallel_execution() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("f.txt"), "event_test").unwrap();

            let registry = cuervo_tools::default_registry(&Default::default());
            let (event_tx, mut rx) = cuervo_core::event_bus(64);
            let mut idx = 0u32;

            let batch = vec![
                make_tool_call("ev1", "file_read", serde_json::json!({"path": dir.path().join("f.txt").to_str().unwrap()})),
            ];

            execute_parallel_batch(
                &batch, &registry, dir.path().to_str().unwrap(),
                Duration::from_secs(10), &event_tx, None,
                uuid::Uuid::new_v4(), &mut idx, 10,
                &ToolExecutionConfig::default(), &*TEST_SINK,
            ).await;

            // Should have received at least one event.
            let mut event_count = 0;
            while rx.try_recv().is_ok() {
                event_count += 1;
            }
            assert!(event_count >= 1, "expected at least 1 event, got {event_count}");
        }

        // === Phase 27 (RC-3 fix): is_deterministic_error tests ===

        #[test]
        fn deterministic_file_not_found() {
            assert!(is_deterministic_error("No such file or directory: /tmp/missing.rs"));
            assert!(is_deterministic_error("Error: File not found at /foo/bar.txt"));
            assert!(is_deterministic_error("NOT FOUND"));
        }

        #[test]
        fn deterministic_permission_denied() {
            assert!(is_deterministic_error("Permission denied: /etc/shadow"));
            assert!(is_deterministic_error("PERMISSION DENIED for user"));
        }

        #[test]
        fn deterministic_path_type_errors() {
            assert!(is_deterministic_error("Error: /tmp is a directory, expected a file"));
            assert!(is_deterministic_error("not a directory: /tmp/file.txt/sub"));
        }

        #[test]
        fn deterministic_security_errors() {
            assert!(is_deterministic_error("path traversal detected in ../../etc/passwd"));
            assert!(is_deterministic_error("Operation blocked by security policy"));
            assert!(is_deterministic_error("unknown tool: foo_bar"));
            assert!(is_deterministic_error("Action denied by task context access control"));
        }

        #[test]
        fn deterministic_schema_errors() {
            assert!(is_deterministic_error("schema validation failed: invalid type"));
            assert!(is_deterministic_error("missing required field 'path'"));
        }

        #[test]
        fn non_deterministic_transient_errors() {
            // These should NOT be classified as deterministic
            assert!(!is_deterministic_error("connection timed out"));
            assert!(!is_deterministic_error("rate limit exceeded"));
            assert!(!is_deterministic_error("internal server error"));
            assert!(!is_deterministic_error("process killed by signal"));
            assert!(!is_deterministic_error("command exited with code 1"));
        }

        #[test]
        fn deterministic_empty_error() {
            assert!(!is_deterministic_error(""));
        }

        #[test]
        fn deterministic_case_insensitive() {
            assert!(is_deterministic_error("NO SUCH FILE OR DIRECTORY"));
            assert!(is_deterministic_error("Permission Denied"));
            assert!(is_deterministic_error("Is A Directory"));
            assert!(is_deterministic_error("Path Traversal"));
        }

        // === Phase 27 Stress Tests ===

        #[test]
        fn stress_deterministic_1000_calls_consistent() {
            // Verify is_deterministic_error is deterministic across 1000 iterations
            let test_cases = vec![
                ("No such file or directory: /a/b/c.rs", true),
                ("permission denied", true),
                ("connection timed out", false),
                ("rate limit exceeded", false),
                ("unknown tool: xyz", true),
                ("path traversal attempt", true),
                ("blocked by security policy", true),
                ("command exited with code 137", false),
                ("", false),
            ];

            for _ in 0..1000 {
                for (error, expected) in &test_cases {
                    assert_eq!(
                        is_deterministic_error(error),
                        *expected,
                        "Inconsistent result for error: {error}"
                    );
                }
            }
        }

        #[test]
        fn stress_deterministic_with_varying_paths() {
            // 100 different file paths — all should be deterministic (contains "not found")
            for i in 0..100 {
                let err = format!("Error: File not found at /project/src/module_{i}/file_{i}.rs");
                assert!(
                    is_deterministic_error(&err),
                    "Expected deterministic for: {err}"
                );
            }
        }

        #[test]
        fn stress_non_deterministic_diverse_errors() {
            // 50 diverse non-deterministic errors
            let transient_patterns = [
                "connection refused",
                "connection reset by peer",
                "broken pipe",
                "timed out after 30s",
                "rate limit exceeded",
                "server returned 500",
                "server returned 502",
                "server returned 503",
                "process killed by OOM",
                "disk full",
            ];

            for (i, pattern) in transient_patterns.iter().enumerate() {
                for j in 0..5 {
                    let err = format!("{pattern} (attempt {i}.{j})");
                    assert!(
                        !is_deterministic_error(&err),
                        "Should NOT be deterministic: {err}"
                    );
                }
            }
        }
    }
}
