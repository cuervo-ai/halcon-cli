use axum::{
    extract::{Path, State},
    Json,
};
use uuid::Uuid;

use crate::error::ApiError;
use crate::server::state::AppState;
use crate::types::agent::UsageInfo;
use crate::types::task::*;

/// POST /api/v1/tasks — submit a task DAG for execution.
pub async fn submit_task(
    State(state): State<AppState>,
    Json(req): Json<SubmitTaskRequest>,
) -> Result<Json<SubmitTaskResponse>, ApiError> {
    use cuervo_runtime::executor::{AgentSelector, TaskDAG, TaskNode};

    let mut dag = TaskDAG::new();
    for node_spec in &req.nodes {
        let selector = if let Some(id) = node_spec.agent_selector.by_id {
            AgentSelector::ById(id)
        } else if let Some(ref caps) = node_spec.agent_selector.by_capability {
            let capabilities: Vec<cuervo_runtime::AgentCapability> = caps
                .iter()
                .map(|c| cuervo_runtime::AgentCapability::Custom(c.clone()))
                .collect();
            AgentSelector::ByCapability(capabilities)
        } else if let Some(ref name) = node_spec.agent_selector.by_name {
            AgentSelector::ByName(name.clone())
        } else {
            AgentSelector::ByCapability(vec![])
        };

        let budget = node_spec.budget.as_ref().map(|b| {
            cuervo_runtime::AgentBudget {
                max_tokens: b.max_tokens,
                max_cost_usd: b.max_cost_usd,
                max_duration: std::time::Duration::from_millis(b.max_duration_ms),
            }
        });

        let node = TaskNode {
            task_id: node_spec.task_id,
            instruction: node_spec.instruction.clone(),
            agent_selector: selector,
            depends_on: node_spec.depends_on.clone(),
            budget,
            context_keys: node_spec.context_keys.clone(),
        };
        dag.add_node(node);
    }

    let execution_id = Uuid::new_v4();
    let node_count = req.nodes.len();

    // Validate DAG before execution.
    dag.validate().map_err(|e| ApiError::bad_request(e.to_string()))?;
    let wave_count = dag.waves().map_err(|e| ApiError::bad_request(e.to_string()))?.len();

    state.broadcast(crate::types::ws::WsServerEvent::TaskSubmitted {
        execution_id,
        node_count,
    });

    // Store execution record.
    {
        let execution = TaskExecution {
            id: execution_id,
            status: TaskStatus::Running,
            wave_count,
            node_results: vec![],
            submitted_at: chrono::Utc::now(),
            completed_at: None,
            total_usage: UsageInfo::default(),
        };
        state
            .task_executions
            .write()
            .await
            .insert(execution_id, execution);
    }

    // Execute asynchronously.
    let state_clone = state.clone();
    let exec_id = execution_id;
    tokio::spawn(async move {
        match state_clone.runtime.execute_dag(dag).await {
            Ok(result) => {
                let total_usage = UsageInfo {
                    input_tokens: result.total_usage.input_tokens,
                    output_tokens: result.total_usage.output_tokens,
                    cost_usd: result.total_usage.cost_usd,
                    latency_ms: result.total_usage.latency_ms,
                    rounds: result.total_usage.rounds,
                };

                let node_results: Vec<TaskNodeResult> = result
                    .results
                    .iter()
                    .map(|(id, res)| match res {
                        Ok(resp) => TaskNodeResult {
                            task_id: *id,
                            agent_id: None,
                            status: if resp.success {
                                TaskStatus::Completed
                            } else {
                                TaskStatus::Failed
                            },
                            output: Some(resp.output.clone()),
                            usage: Some(UsageInfo {
                                input_tokens: resp.usage.input_tokens,
                                output_tokens: resp.usage.output_tokens,
                                cost_usd: resp.usage.cost_usd,
                                latency_ms: resp.usage.latency_ms,
                                rounds: resp.usage.rounds,
                            }),
                            error: None,
                        },
                        Err(e) => TaskNodeResult {
                            task_id: *id,
                            agent_id: None,
                            status: TaskStatus::Failed,
                            output: None,
                            usage: None,
                            error: Some(e.to_string()),
                        },
                    })
                    .collect();

                let all_success = node_results.iter().all(|r| r.status == TaskStatus::Completed);

                if let Some(exec) = state_clone.task_executions.write().await.get_mut(&exec_id) {
                    exec.status = if all_success {
                        TaskStatus::Completed
                    } else {
                        TaskStatus::Failed
                    };
                    exec.completed_at = Some(chrono::Utc::now());
                    exec.node_results = node_results;
                    exec.total_usage = total_usage.clone();
                }

                state_clone.broadcast(crate::types::ws::WsServerEvent::TaskCompleted {
                    execution_id: exec_id,
                    success: all_success,
                    usage: total_usage,
                });
            }
            Err(e) => {
                if let Some(exec) = state_clone.task_executions.write().await.get_mut(&exec_id) {
                    exec.status = TaskStatus::Failed;
                    exec.completed_at = Some(chrono::Utc::now());
                }
                tracing::error!(error = %e, execution_id = %exec_id, "task execution failed");
            }
        }
    });

    Ok(Json(SubmitTaskResponse {
        execution_id,
        node_count,
        wave_count,
    }))
}

/// GET /api/v1/tasks — list task executions.
pub async fn list_tasks(
    State(state): State<AppState>,
) -> Result<Json<Vec<TaskExecution>>, ApiError> {
    let executions = state.task_executions.read().await;
    let mut tasks: Vec<TaskExecution> = executions.values().cloned().collect();
    tasks.sort_by(|a, b| b.submitted_at.cmp(&a.submitted_at));
    Ok(Json(tasks))
}

/// GET /api/v1/tasks/:id — get task execution status.
pub async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<TaskExecution>, ApiError> {
    let executions = state.task_executions.read().await;
    executions
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("task execution {id} not found")))
}

/// DELETE /api/v1/tasks/:id — cancel a running task.
pub async fn cancel_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut executions = state.task_executions.write().await;
    if let Some(exec) = executions.get_mut(&id) {
        if exec.status == TaskStatus::Running || exec.status == TaskStatus::Pending {
            exec.status = TaskStatus::Cancelled;
            exec.completed_at = Some(chrono::Utc::now());
            Ok(Json(serde_json::json!({ "cancelled": true, "id": id.to_string() })))
        } else {
            Err(ApiError::bad_request(format!(
                "task {id} is not running (status: {:?})",
                exec.status
            )))
        }
    } else {
        Err(ApiError::not_found(format!("task execution {id} not found")))
    }
}
