//! Sequential tool execution: authorization → execution → recording.
//!
//! Three-phase pipeline for destructive/readwrite tools:
//!   Phase 1 (Authorization): resolve, dry-run, permission pipeline, sudo injection
//!   Phase 2 (Execution): delegate to execute_one_tool via pipeline
//!   Phase 3 (Recording): trace steps, metrics, events, render output

use std::time::Duration;

use chrono::Utc;

use halcon_core::types::{ContentBlock, DomainEvent, EventPayload, PermissionLevel, ToolInput};
use halcon_core::EventSender;
use halcon_storage::{AsyncDatabase, ToolExecutionMetric, TraceStep, TraceStepType};
use halcon_tools::ToolRegistry;

use super::{
    canonicalize_name, execute_one_tool, generate_file_edit_preview, resolve_tool_from_registry,
    synthetic_dry_run_result, CompletedToolUse, ToolExecResult, ToolExecutionConfig,
};
use crate::render::sink::RenderSink;
use crate::repl::adaptive_prompt::RiskLevel as AdaptiveRiskLevel;
use crate::repl::conversational_permission::ConversationalPermissionHandler;
use crate::repl::security::idempotency::DryRunMode;

/// Execute a single tool sequentially (with permission check).
#[allow(clippy::too_many_arguments)]
pub async fn execute_sequential_tool(
    tool_call: &CompletedToolUse,
    registry: &ToolRegistry,
    permissions: &mut ConversationalPermissionHandler,
    permission_pipeline: Option<
        &mut crate::repl::security::permission_pipeline::PermissionPipeline,
    >,
    working_dir: &str,
    tool_timeout: Duration,
    event_tx: &EventSender,
    trace_db: Option<&AsyncDatabase>,
    session_id: uuid::Uuid,
    trace_step_index: &mut u32,
    exec_config: &ToolExecutionConfig<'_>,
    render_sink: &dyn RenderSink,
    plugin_registry: Option<&std::sync::Mutex<crate::repl::plugins::PluginRegistry>>,
) -> ToolExecResult {
    let canonical_name = canonicalize_name(&tool_call.name);
    let Some(tool) = resolve_tool_from_registry(&tool_call.name, registry) else {
        return ToolExecResult {
            tool_use_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content_block: ContentBlock::ToolResult {
                tool_use_id: tool_call.id.clone(),
                content: format!(
                    "Error: unknown tool '{}' (canonical: '{}')",
                    tool_call.name, canonical_name
                ),
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

    #[allow(unused_mut)]
    let mut tool_input = ToolInput {
        tool_use_id: tool_call.id.clone(),
        arguments: tool_call.input.clone(),
        working_directory: working_dir.to_string(),
    };

    // ── Permission authorization via unified pipeline ──────────────────────
    // Pre-authorization UI: risk assessment + diff preview for destructive tools.
    if perm_level >= PermissionLevel::Destructive {
        halcon_core::emit_event(
            event_tx,
            DomainEvent::new(EventPayload::PermissionRequested {
                tool: tool_call.name.clone(),
                level: perm_level,
            }),
        );
    }
    if perm_level == PermissionLevel::Destructive {
        let handler = ConversationalPermissionHandler::new(true);
        let risk = handler.assess_risk_level(&tool_call.name, perm_level, &tool_call.input);
        let risk_level = match risk {
            AdaptiveRiskLevel::Low => "Low",
            AdaptiveRiskLevel::Medium => "Medium",
            AdaptiveRiskLevel::High => "High",
            AdaptiveRiskLevel::Critical => "Critical",
        };
        render_sink.permission_awaiting(&tool_call.name, &tool_call.input, risk_level);
        render_sink.agent_state_transition("executing", "tool_wait", "awaiting permission");
        if tool_call.name == "file_edit" {
            let _ = generate_file_edit_preview(&tool_call.input);
        }
    }

    // Unified permission pipeline: TBAC → Blacklist → Safety → Denial → Conversational.
    // CR-2 fix: Wrap permission check with configurable timeout to prevent unbounded blocking.
    use crate::repl::security::permission_pipeline::{self, PermissionContext, PipelineDecision};
    let perm_timeout_secs = exec_config.permission_timeout_secs.unwrap_or(0);
    let pipeline_future = async {
        if let Some(pipeline) = permission_pipeline {
            let ctx = PermissionContext {
                tool_name: &tool_call.name,
                perm_level,
                input: &tool_input,
            };
            pipeline.check(&ctx, permissions).await
        } else {
            permission_pipeline::authorize_tool(
                &tool_call.name,
                perm_level,
                &tool_input,
                permissions,
                None,
            )
            .await
        }
    };
    let pipeline_result = if perm_timeout_secs > 0 {
        match tokio::time::timeout(Duration::from_secs(perm_timeout_secs), pipeline_future).await {
            Ok(decision) => decision,
            Err(_elapsed) => {
                tracing::warn!(
                    tool = %tool_call.name,
                    timeout_secs = perm_timeout_secs,
                    "Permission prompt timed out — auto-denying tool"
                );
                PipelineDecision::Deny {
                    reason: format!(
                        "Permission prompt timed out after {perm_timeout_secs}s. \
                         Configure security.permission_timeout_secs to adjust."
                    ),
                    gate: "timeout",
                }
            }
        }
    } else {
        // Legacy behavior: no timeout (perm_timeout_secs = 0)
        pipeline_future.await
    };

    // Post-authorization UI: state transition.
    if perm_level == PermissionLevel::Destructive {
        render_sink.agent_state_transition("tool_wait", "executing", "permission decided");
    }

    // Handle pipeline decision.
    match pipeline_result {
        PipelineDecision::Deny { reason, gate } => {
            tracing::info!(tool = %tool_call.name, gate = gate, "Permission denied: {reason}");
            halcon_core::emit_event(
                event_tx,
                DomainEvent::new(EventPayload::PermissionDenied {
                    tool: tool_call.name.clone(),
                    level: perm_level,
                }),
            );
            if let Some(db) = trace_db {
                let context_id = uuid::Uuid::new_v4();
                let _ = db
                    .save_policy_decision(
                        &session_id,
                        &context_id,
                        &tool_call.name,
                        "denied",
                        None,
                        None,
                    )
                    .await;
            }
            render_sink.tool_denied(&tool_call.name);
            return ToolExecResult {
                tool_use_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                content_block: ContentBlock::ToolResult {
                    tool_use_id: tool_call.id.clone(),
                    content: format!(
                        "Error: permission denied for '{}' (gate: {gate}): {reason}. \
                         Do NOT retry this tool. Acknowledge the denial and adjust your plan.",
                        tool_call.name
                    ),
                    is_error: true,
                },
                duration_ms: 0,
                was_parallel: false,
            };
        }
        PipelineDecision::Allow(_authorized_input) => {
            // Permission granted — emit event and persist.
            if perm_level >= PermissionLevel::Destructive {
                halcon_core::emit_event(
                    event_tx,
                    DomainEvent::new(EventPayload::PermissionGranted {
                        tool: tool_call.name.clone(),
                        level: perm_level,
                    }),
                );
                if let Some(db) = trace_db {
                    let context_id = uuid::Uuid::new_v4();
                    let _ = db
                        .save_policy_decision(
                            &session_id,
                            &context_id,
                            &tool_call.name,
                            "granted",
                            None,
                            None,
                        )
                        .await;
                }
            }
        }
        PipelineDecision::Ask { .. } => {
            // Not currently used — conversational handler handles interactive prompts internally.
        }
    }

    // --- Sudo Password Injection (TUI mode only) ---
    #[cfg(feature = "tui")]
    {
        let is_sudo_bash = tool_call.name == "bash" && {
            tool_call
                .input
                .get("command")
                .and_then(|v| v.as_str())
                .map(|cmd| {
                    let t = cmd.trim();
                    t.starts_with("sudo ") || t == "sudo"
                })
                .unwrap_or(false)
        };

        if is_sudo_bash {
            let cmd_str = tool_call
                .input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let has_cached = permissions.has_cached_sudo_password();
            render_sink.sudo_password_request(&tool_call.name, &cmd_str, has_cached);
            tracing::debug!(command = %cmd_str, "Sudo password requested from TUI modal");

            if let Some(pw) = permissions.get_sudo_password(30).await {
                let cmd_without_sudo = cmd_str
                    .trim()
                    .strip_prefix("sudo ")
                    .unwrap_or(cmd_str.trim());
                let pw_escaped = pw.replace('\'', "'\\''");
                let new_cmd = format!(
                    "printf '%s\\n' '{}' | sudo -S -- {}",
                    pw_escaped, cmd_without_sudo
                );
                tool_input.arguments["command"] = serde_json::json!(new_cmd);
                tracing::debug!("Sudo command rewritten for password injection (password hidden)");
            } else {
                tracing::info!("Sudo password not provided (cancelled or timed out) — executing without injection");
            }
        }
    }

    // ── Phase 2: Execution ──────────────────────────────────────────────────
    record_trace_call(trace_db, session_id, trace_step_index, tool_call);
    render_sink.tool_start(&tool_call.name, &tool_call.input);

    let result = execute_one_tool(
        tool_call,
        registry,
        working_dir,
        tool_timeout,
        exec_config.dry_run_mode,
        exec_config.idempotency,
        &exec_config.retry,
        render_sink,
        plugin_registry,
        exec_config.hook_runner.as_deref(),
        &exec_config.session_id_str,
        &exec_config.session_tools,
    )
    .await;

    // ── Phase 3: Recording ────────────────────────────────────────────────
    let is_error = matches!(&result.content_block,
        ContentBlock::ToolResult { is_error, .. } if *is_error);

    record_tool_metric(trace_db, &result, session_id, is_error);

    halcon_core::emit_event(
        event_tx,
        DomainEvent::new(EventPayload::ToolExecuted {
            tool: tool_call.name.clone(),
            permission: perm_level,
            duration_ms: result.duration_ms,
            success: !is_error,
        }),
    );

    render_sink.tool_output(&result.content_block, result.duration_ms);
    record_trace_result(
        trace_db,
        session_id,
        trace_step_index,
        tool_call,
        &result,
        is_error,
    );

    result
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 3 helpers: trace + metrics recording (extracted to reduce main fn size)
// ─────────────────────────────────────────────────────────────────────────────

fn record_trace_call(
    trace_db: Option<&AsyncDatabase>,
    session_id: uuid::Uuid,
    trace_step_index: &mut u32,
    tool_call: &CompletedToolUse,
) {
    let Some(db) = trace_db else { return };
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

fn record_tool_metric(
    trace_db: Option<&AsyncDatabase>,
    result: &ToolExecResult,
    session_id: uuid::Uuid,
    is_error: bool,
) {
    let Some(db) = trace_db else { return };
    let tool_metric = ToolExecutionMetric {
        tool_name: result.tool_name.clone(),
        session_id: Some(session_id.to_string()),
        duration_ms: result.duration_ms,
        success: !is_error,
        is_parallel: false,
        input_summary: None,
        created_at: Utc::now(),
    };
    if let Err(e) = db.inner().insert_tool_metric(&tool_metric) {
        tracing::warn!("tool_execution_metrics insert failed: {e}");
    }
}

fn record_trace_result(
    trace_db: Option<&AsyncDatabase>,
    session_id: uuid::Uuid,
    trace_step_index: &mut u32,
    tool_call: &CompletedToolUse,
    result: &ToolExecResult,
    is_error: bool,
) {
    let Some(db) = trace_db else { return };
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
