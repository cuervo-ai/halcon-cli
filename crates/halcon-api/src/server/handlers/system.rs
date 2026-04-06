use axum::{extract::State, Json};

use crate::error::ApiError;
use crate::server::state::AppState;
use crate::types::system::*;

/// Get current process memory usage in bytes (RSS).
///
/// Uses /proc/self/statm on Linux. Falls back to 0 on other platforms
/// where libc types aren't available in this crate.
fn get_process_memory_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(rss_pages) = statm.split_whitespace().nth(1) {
                if let Ok(pages) = rss_pages.parse::<u64>() {
                    return pages * 4096;
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Parse `ps -o rss= -p <pid>` output (RSS in KB).
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
        {
            if let Ok(rss_kb) = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u64>()
            {
                return rss_kb * 1024;
            }
        }
    }
    0
}

/// GET /api/v1/system/status — get system status.
pub async fn get_status(State(state): State<AppState>) -> Result<Json<SystemStatus>, ApiError> {
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
    let any_unhealthy = health_report
        .values()
        .any(|h| matches!(h, halcon_runtime::AgentHealth::Unavailable { .. }));
    let any_degraded = health_report
        .values()
        .any(|h| matches!(h, halcon_runtime::AgentHealth::Degraded { .. }));

    let health = if any_unhealthy {
        SystemHealth::Unhealthy
    } else if any_degraded {
        SystemHealth::Degraded
    } else {
        SystemHealth::Healthy
    };

    Ok(Json(SystemStatus {
        version: env!("CARGO_PKG_VERSION").to_string(),
        started_at: chrono::Utc::now() - chrono::Duration::seconds(state.uptime_seconds() as i64),
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
            memory_usage_bytes: get_process_memory_bytes(),
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
            if req.graceful {
                "graceful"
            } else {
                "immediate"
            }
        ),
    }))
}
