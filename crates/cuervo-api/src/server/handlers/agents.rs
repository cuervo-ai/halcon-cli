use axum::{
    extract::{Path, State},
    Json,
};
use uuid::Uuid;

use crate::error::ApiError;
use crate::server::state::AppState;
use crate::types::agent::*;

/// GET /api/v1/agents — list all agents.
pub async fn list_agents(
    State(state): State<AppState>,
) -> Result<Json<Vec<AgentInfo>>, ApiError> {
    let descriptors = state.runtime.all_agents().await;
    let health_report = state.runtime.health_report().await;

    let agents: Vec<AgentInfo> = descriptors
        .into_iter()
        .map(|desc| {
            let health = health_report
                .get(&desc.id)
                .map(convert_health)
                .unwrap_or(HealthStatus::Unknown);

            AgentInfo {
                id: desc.id,
                name: desc.name.clone(),
                kind: convert_agent_kind(&desc.agent_kind),
                capabilities: desc
                    .capabilities
                    .iter()
                    .map(|c| format!("{c:?}"))
                    .collect(),
                protocols: desc
                    .protocols
                    .iter()
                    .map(|p| format!("{p:?}"))
                    .collect(),
                health,
                registered_at: chrono::Utc::now(), // TODO: track in registry
                last_invoked: None,
                invocation_count: 0,
                max_concurrency: desc.max_concurrency,
                metadata: desc.metadata.clone(),
            }
        })
        .collect();

    Ok(Json(agents))
}

/// GET /api/v1/agents/:id — get agent by ID.
pub async fn get_agent(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<AgentInfo>, ApiError> {
    let descriptors = state.runtime.all_agents().await;
    let desc = descriptors
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| ApiError::not_found(format!("agent {id} not found")))?;

    let health_report = state.runtime.health_report().await;
    let health = health_report
        .get(&id)
        .map(convert_health)
        .unwrap_or(HealthStatus::Unknown);

    Ok(Json(AgentInfo {
        id: desc.id,
        name: desc.name.clone(),
        kind: convert_agent_kind(&desc.agent_kind),
        capabilities: desc
            .capabilities
            .iter()
            .map(|c| format!("{c:?}"))
            .collect(),
        protocols: desc
            .protocols
            .iter()
            .map(|p| format!("{p:?}"))
            .collect(),
        health,
        registered_at: chrono::Utc::now(),
        last_invoked: None,
        invocation_count: 0,
        max_concurrency: desc.max_concurrency,
        metadata: desc.metadata.clone(),
    }))
}

/// DELETE /api/v1/agents/:id — deregister an agent.
pub async fn stop_agent(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.runtime.deregister_agent(&id).await;
    state.broadcast(crate::types::ws::WsServerEvent::AgentDeregistered { id });
    Ok(Json(serde_json::json!({ "stopped": true, "id": id.to_string() })))
}

/// POST /api/v1/agents/:id/invoke — invoke an agent.
pub async fn invoke_agent(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<InvokeAgentRequest>,
) -> Result<Json<InvokeAgentResponse>, ApiError> {
    use cuervo_runtime::{AgentBudget, AgentRequest};

    let budget = req.budget.map(|b| AgentBudget {
        max_tokens: b.max_tokens,
        max_cost_usd: b.max_cost_usd,
        max_duration: std::time::Duration::from_millis(b.max_duration_ms),
    });

    let timeout = req
        .timeout_ms
        .map(std::time::Duration::from_millis);

    let agent_req = AgentRequest {
        request_id: uuid::Uuid::new_v4(),
        instruction: req.instruction,
        context: req.context,
        allowed_capabilities: None,
        budget,
        timeout,
    };

    let request_id = agent_req.request_id;

    state.broadcast(crate::types::ws::WsServerEvent::AgentInvoked {
        id,
        request_id,
    });

    let response = state
        .runtime
        .invoke_agent(&id, agent_req)
        .await
        .map_err(|e| ApiError::runtime(e.to_string()))?;

    let usage = UsageInfo {
        input_tokens: response.usage.input_tokens,
        output_tokens: response.usage.output_tokens,
        cost_usd: response.usage.cost_usd,
        latency_ms: response.usage.latency_ms,
        rounds: response.usage.rounds,
    };

    state.broadcast(crate::types::ws::WsServerEvent::AgentCompleted {
        id,
        request_id,
        success: response.success,
        usage: usage.clone(),
    });

    Ok(Json(InvokeAgentResponse {
        request_id: response.request_id,
        success: response.success,
        output: response.output,
        artifacts: response
            .artifacts
            .into_iter()
            .map(|a| ArtifactInfo {
                kind: format!("{:?}", a.kind),
                name: a.path.unwrap_or_default(),
                content: a.content,
            })
            .collect(),
        usage,
    }))
}

/// GET /api/v1/agents/:id/health — get agent health detail.
pub async fn agent_health(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<AgentHealthDetail>, ApiError> {
    let descriptors = state.runtime.all_agents().await;
    let desc = descriptors
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| ApiError::not_found(format!("agent {id} not found")))?;

    let health_report = state.runtime.health_report().await;
    let health = health_report
        .get(&id)
        .map(convert_health)
        .unwrap_or(HealthStatus::Unknown);

    Ok(Json(AgentHealthDetail {
        id,
        name: desc.name.clone(),
        health,
        failure_count: 0,
        success_count: 0,
        last_check: None,
    }))
}

// --- Conversion helpers ---

fn convert_agent_kind(kind: &cuervo_runtime::AgentKind) -> AgentKind {
    match kind {
        cuervo_runtime::AgentKind::Llm => AgentKind::Llm,
        cuervo_runtime::AgentKind::Mcp => AgentKind::Mcp,
        cuervo_runtime::AgentKind::CliProcess => AgentKind::CliProcess,
        cuervo_runtime::AgentKind::HttpEndpoint => AgentKind::HttpEndpoint,
        cuervo_runtime::AgentKind::CuervoRemote => AgentKind::CuervoRemote,
        cuervo_runtime::AgentKind::Plugin => AgentKind::Plugin,
    }
}

fn convert_health(health: &cuervo_runtime::AgentHealth) -> HealthStatus {
    match health {
        cuervo_runtime::AgentHealth::Healthy => HealthStatus::Healthy,
        cuervo_runtime::AgentHealth::Degraded { reason } => HealthStatus::Degraded {
            reason: reason.clone(),
        },
        cuervo_runtime::AgentHealth::Unavailable { reason } => {
            HealthStatus::Unavailable {
                reason: reason.clone(),
            }
        }
    }
}
