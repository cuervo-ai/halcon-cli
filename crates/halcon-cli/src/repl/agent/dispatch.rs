//! Dispatch layer: Routes AgentContext to simplified_loop (canonical runtime).
//!
//! This module exists to maintain API compatibility during the transition.
//! Eventually, call sites should construct SimplifiedLoopConfig directly.

use std::sync::Arc;

use anyhow::Result;

use super::super::agent_types::AgentLoopResult;
use super::simplified_loop::{run_simplified_loop, SimplifiedLoopConfig};
use super::AgentContext;

/// Dispatch AgentContext to the canonical simplified_loop runtime.
///
/// Extracts the minimal set of fields needed by simplified_loop from the
/// larger AgentContext. Fields not used by simplified_loop are ignored:
/// - session (messages come from request.messages)
/// - event_tx, trace_db, response_cache (observability - not in canonical loop)
/// - resilience, fallback_providers, routing_config (recovery - handled by FeedbackArbiter)
/// - planner, planning_config (planning - not in canonical loop)
/// - guardrails, reflector, replay_tool_executor (optional features)
/// - phase14, model_selector, registry, episode_id (advanced features)
/// - orchestrator_config, tool_selection_enabled, task_bridge (delegation)
/// - context_metrics, context_manager (context assembly)
/// - speculator, security_config, strategy_context (advanced)
/// - critic_provider, critic_model, plugin_registry (plugins)
/// - is_sub_agent, requested_provider, policy (metadata)
///
/// The canonical loop focuses on core execution: provider, tools, permissions,
/// limits, compaction, hooks, cost tracking, cancellation.
pub async fn dispatch_to_simplified_loop(ctx: AgentContext<'_>) -> Result<AgentLoopResult> {
    // Extract hook_runner if available (currently None - will be wired in Phase 5)
    let hook_runner = None;

    // Extract max cost from limits or use unlimited
    let max_cost_usd = if ctx.limits.max_cost_usd > 0.0 {
        ctx.limits.max_cost_usd
    } else {
        0.0 // unlimited
    };

    let config = SimplifiedLoopConfig {
        provider: ctx.provider,
        request: ctx.request,
        tool_registry: ctx.tool_registry,
        limits: ctx.limits,
        render_sink: ctx.render_sink,
        working_dir: ctx.working_dir,
        compactor: ctx.compactor,
        ctrl_rx: ctx.ctrl_rx,
        hook_runner,
        permissions: ctx.permissions,
        max_cost_usd,
        cancel_token: None, // TODO: wire cancellation token
    };

    run_simplified_loop(config).await
}
