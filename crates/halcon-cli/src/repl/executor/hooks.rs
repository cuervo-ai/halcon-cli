//! Pre/post tool lifecycle hook integration (Feature 2).
//!
//! Fires `PreToolUse` hooks before execution (can deny) and
//! `PostToolUse` / `PostToolUseFailure` hooks after execution (observability only).

use halcon_core::types::ContentBlock;

// Import hook types at module level — no super::super:: inline references.
use crate::repl::hooks::{HookEventName, HookOutcome, HookRunner, tool_event};

use super::{CompletedToolUse, ToolExecResult};

/// Fire PreToolUse hook. Returns Some(denied_result) if the hook denies execution.
pub(crate) async fn fire_pre_tool_hook(
    runner: &HookRunner,
    tool_call: &CompletedToolUse,
    session_id_str: &str,
) -> Option<ToolExecResult> {
    if !runner.has_hooks_for(HookEventName::PreToolUse) {
        return None;
    }
    let hook_event = tool_event(
        HookEventName::PreToolUse,
        &tool_call.name,
        &tool_call.input,
        session_id_str,
    );
    if let HookOutcome::Deny(reason) = runner.fire(&hook_event).await {
        return Some(ToolExecResult {
            tool_use_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content_block: ContentBlock::ToolResult {
                tool_use_id: tool_call.id.clone(),
                content: format!(
                    "Error: PreToolUse hook denied '{}': {reason}. \
                     Do NOT retry this tool without resolving the policy conflict.",
                    tool_call.name
                ),
                is_error: true,
            },
            duration_ms: 0,
            was_parallel: false,
        });
    }
    None
}

/// Fire PostToolUse or PostToolUseFailure hook (best-effort, never blocks).
pub(crate) async fn fire_post_tool_hook(
    runner: &HookRunner,
    tool_call: &CompletedToolUse,
    is_error: bool,
    session_id_str: &str,
) {
    let post_event_name = if is_error {
        HookEventName::PostToolUseFailure
    } else {
        HookEventName::PostToolUse
    };
    if runner.has_hooks_for(post_event_name) {
        let hook_event = tool_event(
            post_event_name,
            &tool_call.name,
            &tool_call.input,
            session_id_str,
        );
        // Best-effort: ignore outcome (post-hooks never block).
        let _ = runner.fire(&hook_event).await;
    }
}
