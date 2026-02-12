use axum::{extract::State, Json};

use crate::error::ApiError;
use crate::server::state::AppState;
use crate::types::observability::*;

/// GET /api/v1/metrics — get current metrics snapshot.
pub async fn get_metrics(
    State(state): State<AppState>,
) -> Result<Json<MetricsSnapshot>, ApiError> {
    let agents = state.runtime.all_agents().await;
    let health_report = state.runtime.health_report().await;
    let tool_states = state.tool_states.read().await;
    let task_executions = state.task_executions.read().await;

    let active_tasks = task_executions
        .values()
        .filter(|t| {
            t.status == crate::types::task::TaskStatus::Running
                || t.status == crate::types::task::TaskStatus::Pending
        })
        .count();
    let completed_tasks = task_executions
        .values()
        .filter(|t| t.status == crate::types::task::TaskStatus::Completed)
        .count();
    let failed_tasks = task_executions
        .values()
        .filter(|t| t.status == crate::types::task::TaskStatus::Failed)
        .count();

    let total_tool_executions: u64 = tool_states.values().map(|ts| ts.execution_count).sum();

    let agent_metrics: Vec<AgentMetricSummary> = agents
        .iter()
        .map(|desc| {
            let _health = health_report.get(&desc.id);
            AgentMetricSummary {
                agent_id: desc.id,
                agent_name: desc.name.clone(),
                invocation_count: 0, // TODO: track per-agent
                avg_latency_ms: 0.0,
                total_tokens: 0,
                total_cost_usd: 0.0,
                error_rate: 0.0,
            }
        })
        .collect();

    Ok(Json(MetricsSnapshot {
        timestamp: chrono::Utc::now(),
        agent_count: agents.len(),
        tool_count: tool_states.len(),
        total_invocations: 0, // TODO: aggregate from agent metrics
        total_tool_executions,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cost_usd: 0.0,
        uptime_seconds: state.uptime_seconds(),
        active_tasks,
        completed_tasks,
        failed_tasks,
        events_per_second: 0.0,
        agent_metrics,
    }))
}
