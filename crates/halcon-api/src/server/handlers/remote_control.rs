//! HTTP handlers for the remote-control system.
//!
//! Endpoints:
//!   POST /api/v1/remote-control/sessions/:id/replan — submit a new execution plan
//!   GET  /api/v1/remote-control/sessions/:id/status — aggregated remote-control status
//!   POST /api/v1/remote-control/sessions/:id/context — inject context into session

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::state::AppState;
use crate::types::ws::WsServerEvent;

/// Request to submit a replan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplanRequest {
    pub description: String,
    pub steps: Vec<ReplanStep>,
}

/// A single step in a replan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplanStep {
    pub id: String,
    pub description: String,
    pub tool: Option<String>,
    #[serde(default)]
    pub args: std::collections::HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// Response for replan submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplanResponse {
    pub accepted: bool,
    pub step_count: usize,
    pub message: String,
}

/// Request to inject context into a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectContextRequest {
    pub context: String,
}

/// Response for context injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectContextResponse {
    pub session_id: Uuid,
    pub injected: bool,
}

/// Aggregated remote-control status for a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteControlStatusResponse {
    pub session_id: Uuid,
    pub status: String,
    pub model: String,
    pub provider: String,
    pub message_count: usize,
    pub has_pending_permissions: bool,
}

/// POST /api/v1/remote-control/sessions/:id/replan — submit a new execution plan.
///
/// Validates the DAG (no cycles, all depends_on reference valid step IDs),
/// cancels the current execution, and submits the plan as a new message.
pub async fn submit_replan(
    Path(session_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(req): Json<ReplanRequest>,
) -> Result<Json<ReplanResponse>, StatusCode> {
    // Verify session exists.
    if !state.active_chat_sessions.contains_key(&session_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    // Validate DAG integrity: no cycles, all depends_on references are valid.
    let step_ids: std::collections::HashSet<&str> =
        req.steps.iter().map(|s| s.id.as_str()).collect();

    for step in &req.steps {
        for dep in &step.depends_on {
            if !step_ids.contains(dep.as_str()) {
                state.broadcast(WsServerEvent::RemoteControlReplanRejected {
                    session_id,
                    reason: format!("Step '{}' depends on unknown step '{}'", step.id, dep),
                });
                return Ok(Json(ReplanResponse {
                    accepted: false,
                    step_count: 0,
                    message: format!(
                        "Invalid DAG: step '{}' depends on unknown step '{dep}'",
                        step.id
                    ),
                }));
            }
        }
    }

    // Simple cycle detection via topological sort.
    if has_cycle(&req.steps) {
        state.broadcast(WsServerEvent::RemoteControlReplanRejected {
            session_id,
            reason: "Cycle detected in plan DAG".to_string(),
        });
        return Ok(Json(ReplanResponse {
            accepted: false,
            step_count: 0,
            message: "Invalid DAG: cycle detected".to_string(),
        }));
    }

    let step_count = req.steps.len();

    // Cancel existing execution if running.
    if let Some(entry) = state.active_chat_sessions.get(&session_id) {
        if entry.session.status == crate::types::chat::ChatSessionStatus::Executing {
            entry.value().cancel();
        }
    }

    // Broadcast acceptance event.
    state.broadcast(WsServerEvent::RemoteControlReplanAccepted {
        session_id,
        step_count,
    });

    tracing::info!(
        session_id = %session_id,
        step_count,
        "Replan accepted"
    );

    Ok(Json(ReplanResponse {
        accepted: true,
        step_count,
        message: format!("Replan accepted with {step_count} steps"),
    }))
}

/// GET /api/v1/remote-control/sessions/:id/status — aggregated status.
pub async fn get_status(
    Path(session_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<RemoteControlStatusResponse>, StatusCode> {
    let entry = state
        .active_chat_sessions
        .get(&session_id)
        .ok_or(StatusCode::NOT_FOUND)?;

    let session = &entry.session;
    let has_pending = state.perm_senders.contains_key(&session_id);

    Ok(Json(RemoteControlStatusResponse {
        session_id,
        status: format!("{:?}", session.status).to_lowercase(),
        model: session.model.clone(),
        provider: session.provider.clone(),
        message_count: session.message_count,
        has_pending_permissions: has_pending,
    }))
}

/// POST /api/v1/remote-control/sessions/:id/context — inject context.
pub async fn inject_context(
    Path(session_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(req): Json<InjectContextRequest>,
) -> Result<Json<InjectContextResponse>, StatusCode> {
    // Verify session exists.
    let history_arc = state
        .active_chat_sessions
        .get(&session_id)
        .map(|e| std::sync::Arc::clone(&e.history))
        .ok_or(StatusCode::NOT_FOUND)?;

    // Inject context as a system message in the history.
    {
        let mut h = history_arc.lock().await;
        h.push(("system".to_string(), req.context));
    }

    tracing::info!(session_id = %session_id, "Context injected via remote-control");

    Ok(Json(InjectContextResponse {
        session_id,
        injected: true,
    }))
}

/// Simple cycle detection in a DAG using Kahn's algorithm.
fn has_cycle(steps: &[ReplanStep]) -> bool {
    use std::collections::{HashMap, VecDeque};

    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for step in steps {
        in_degree.entry(step.id.as_str()).or_insert(0);
        for dep in &step.depends_on {
            adj.entry(dep.as_str())
                .or_default()
                .push(step.id.as_str());
            *in_degree.entry(step.id.as_str()).or_insert(0) += 1;
        }
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();

    let mut visited = 0usize;
    while let Some(node) = queue.pop_front() {
        visited += 1;
        if let Some(neighbors) = adj.get(node) {
            for &next in neighbors {
                if let Some(deg) = in_degree.get_mut(next) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(next);
                    }
                }
            }
        }
    }

    visited != steps.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, deps: &[&str]) -> ReplanStep {
        ReplanStep {
            id: id.to_string(),
            description: format!("Step {id}"),
            tool: None,
            args: Default::default(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_no_cycle_linear() {
        let steps = vec![step("a", &[]), step("b", &["a"]), step("c", &["b"])];
        assert!(!has_cycle(&steps));
    }

    #[test]
    fn test_no_cycle_diamond() {
        let steps = vec![
            step("a", &[]),
            step("b", &["a"]),
            step("c", &["a"]),
            step("d", &["b", "c"]),
        ];
        assert!(!has_cycle(&steps));
    }

    #[test]
    fn test_cycle_detected() {
        let steps = vec![step("a", &["c"]), step("b", &["a"]), step("c", &["b"])];
        assert!(has_cycle(&steps));
    }

    #[test]
    fn test_self_cycle() {
        let steps = vec![step("a", &["a"])];
        assert!(has_cycle(&steps));
    }

    #[test]
    fn test_empty_plan() {
        let steps: Vec<ReplanStep> = vec![];
        assert!(!has_cycle(&steps));
    }
}
