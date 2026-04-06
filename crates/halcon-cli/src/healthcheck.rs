//! Health check endpoint for Halcon bridge relay.
//!
//! Provides `/health` endpoint for monitoring and orchestration.

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Health check state.
#[derive(Clone)]
pub struct HealthState {
    pub buffer: Arc<Mutex<halcon_storage::PersistentEventBuffer>>,
    pub dlq: Arc<Mutex<halcon_storage::DeadLetterQueue>>,
    pub connection_status: Arc<Mutex<ConnectionStatus>>,
}

#[derive(Clone, Debug)]
pub struct ConnectionStatus {
    pub connected: bool,
    pub last_heartbeat: Option<u64>,
}

/// Health check response.
#[derive(Serialize, Deserialize, Debug)]
pub struct HealthResponse {
    pub status: String,
    pub checks: HealthChecks,
    pub metadata: HealthMetadata,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct HealthChecks {
    pub event_buffer: CheckResult,
    pub dlq: CheckResult,
    pub websocket: CheckResult,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CheckResult {
    pub status: String,
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct HealthMetadata {
    pub last_acked_seq: Option<u64>,
    pub pending_events: usize,
    pub sent_events: usize,
    pub dlq_pending: usize,
    pub dlq_exhausted: usize,
    pub sequence_gaps: Vec<u64>,
    pub oldest_sent_age_secs: Option<u64>,
}

/// Health check handler.
pub async fn health_handler(
    State(state): State<HealthState>,
) -> Result<impl IntoResponse, StatusCode> {
    // Check event buffer
    let buffer_check = check_event_buffer(&state.buffer).await;

    // Check DLQ
    let dlq_check = check_dlq(&state.dlq).await;

    // Check WebSocket connection
    let ws_check = check_websocket(&state.connection_status).await;

    // Get metadata
    let metadata = get_health_metadata(&state).await;

    // Determine overall status
    let overall_status = if buffer_check.status == "healthy"
        && dlq_check.status == "healthy"
        && ws_check.status == "healthy"
    {
        "healthy"
    } else if buffer_check.status == "critical" || ws_check.status == "critical" {
        "critical"
    } else {
        "degraded"
    };

    let response = HealthResponse {
        status: overall_status.to_string(),
        checks: HealthChecks {
            event_buffer: buffer_check,
            dlq: dlq_check,
            websocket: ws_check,
        },
        metadata,
    };

    let status_code = match overall_status {
        "healthy" => StatusCode::OK,
        "degraded" => StatusCode::OK, // Still functional
        "critical" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };

    Ok((status_code, Json(response)))
}

async fn check_event_buffer(
    buffer: &Arc<Mutex<halcon_storage::PersistentEventBuffer>>,
) -> CheckResult {
    let buffer = buffer.lock().await;

    match buffer.stats() {
        Ok(stats) => {
            // Check for concerning conditions
            if stats.pending > 1000 {
                CheckResult {
                    status: "degraded".to_string(),
                    message: Some(format!("High pending events: {}", stats.pending)),
                }
            } else {
                CheckResult {
                    status: "healthy".to_string(),
                    message: None,
                }
            }
        }
        Err(e) => CheckResult {
            status: "critical".to_string(),
            message: Some(format!("Buffer stats error: {}", e)),
        },
    }
}

async fn check_dlq(dlq: &Arc<Mutex<halcon_storage::DeadLetterQueue>>) -> CheckResult {
    let dlq = dlq.lock().await;

    match dlq.stats() {
        Ok(stats) => {
            if stats.exhausted > 10 {
                CheckResult {
                    status: "degraded".to_string(),
                    message: Some(format!("High exhausted tasks: {}", stats.exhausted)),
                }
            } else if stats.pending > 100 {
                CheckResult {
                    status: "degraded".to_string(),
                    message: Some(format!("High DLQ pending: {}", stats.pending)),
                }
            } else {
                CheckResult {
                    status: "healthy".to_string(),
                    message: None,
                }
            }
        }
        Err(e) => CheckResult {
            status: "critical".to_string(),
            message: Some(format!("DLQ stats error: {}", e)),
        },
    }
}

async fn check_websocket(status: &Arc<Mutex<ConnectionStatus>>) -> CheckResult {
    let conn = status.lock().await;

    if !conn.connected {
        return CheckResult {
            status: "critical".to_string(),
            message: Some("WebSocket disconnected".to_string()),
        };
    }

    // Check last heartbeat (should be < 60s ago)
    if let Some(last_hb) = conn.last_heartbeat {
        let now = current_timestamp();
        let age = now.saturating_sub(last_hb);

        if age > 120 {
            CheckResult {
                status: "degraded".to_string(),
                message: Some(format!("No heartbeat for {}s", age)),
            }
        } else {
            CheckResult {
                status: "healthy".to_string(),
                message: None,
            }
        }
    } else {
        CheckResult {
            status: "degraded".to_string(),
            message: Some("No heartbeat received yet".to_string()),
        }
    }
}

async fn get_health_metadata(state: &HealthState) -> HealthMetadata {
    let buffer = state.buffer.lock().await;
    let dlq = state.dlq.lock().await;

    let buffer_stats = buffer.stats().ok();
    let dlq_stats = dlq.stats().ok();
    let last_seq = buffer.last_seq().ok().flatten();

    // Calculate oldest sent event age
    let oldest_sent_age = buffer
        .get_sent()
        .ok()
        .and_then(|events| {
            events.first().map(|e| {
                let now = current_timestamp();
                now.saturating_sub(e.sent_at.unwrap_or(e.created_at))
            })
        });

    // Detect sequence gaps (simplified - would need more complex logic)
    let sequence_gaps = Vec::new(); // TODO: implement gap detection

    HealthMetadata {
        last_acked_seq: last_seq,
        pending_events: buffer_stats.as_ref().map(|s| s.pending).unwrap_or(0),
        sent_events: buffer_stats.as_ref().map(|s| s.sent).unwrap_or(0),
        dlq_pending: dlq_stats.as_ref().map(|s| s.pending).unwrap_or(0),
        dlq_exhausted: dlq_stats.as_ref().map(|s| s.exhausted).unwrap_or(0),
        sequence_gaps,
        oldest_sent_age_secs: oldest_sent_age,
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
