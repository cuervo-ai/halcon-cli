use axum::{extract::State, Json};

use crate::error::ApiError;
use crate::server::state::AppState;
use crate::types::system::*;

/// GET /api/v1/system/status — get system status.
pub async fn get_status(
    State(state): State<AppState>,
) -> Result<Json<SystemStatus>, ApiError> {
    let agents = state.runtime.all_agents().await;
    let tool_states = state.tool_states.read().await;
    let task_executions = state.task_executions.read().await;

    let active_tasks = task_executions
        .values()
        .filter(|t| {
            t.status == crate::types::task::TaskStatus::Running
                || t.status == crate::types::task::TaskStatus::Pending
        })
        .count();

    let health_report = state.runtime.health_report().await;
    let any_unhealthy = health_report.values().any(|h| {
        matches!(
            h,
            cuervo_runtime::AgentHealth::Unavailable { .. }
        )
    });
    let any_degraded = health_report.values().any(|h| {
        matches!(
            h,
            cuervo_runtime::AgentHealth::Degraded { .. }
        )
    });

    let health = if any_unhealthy {
        SystemHealth::Unhealthy
    } else if any_degraded {
        SystemHealth::Degraded
    } else {
        SystemHealth::Healthy
    };

    Ok(Json(SystemStatus {
        version: env!("CARGO_PKG_VERSION").to_string(),
        started_at: chrono::Utc::now()
            - chrono::Duration::seconds(state.uptime_seconds() as i64),
        uptime_seconds: state.uptime_seconds(),
        agent_count: agents.len(),
        tool_count: tool_states.len(),
        active_tasks,
        health,
        platform: PlatformInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            rust_version: "1.80+".to_string(),
            pid: std::process::id(),
            memory_usage_bytes: 0, // TODO: platform-specific memory query
        },
    }))
}

/// POST /api/v1/system/shutdown — initiate graceful shutdown.
pub async fn shutdown(
    State(state): State<AppState>,
    Json(req): Json<ShutdownRequest>,
) -> Result<Json<ShutdownResponse>, ApiError> {
    tracing::info!(
        graceful = req.graceful,
        reason = ?req.reason,
        "shutdown requested via API"
    );

    if req.graceful {
        let state_clone = state.clone();
        tokio::spawn(async move {
            // Give time for response to be sent.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if let Err(e) = state_clone.runtime.shutdown().await {
                tracing::error!(error = %e, "runtime shutdown failed");
            }
        });
    }

    Ok(Json(ShutdownResponse {
        accepted: true,
        message: format!(
            "shutdown {} initiated",
            if req.graceful { "graceful" } else { "immediate" }
        ),
    }))
}
