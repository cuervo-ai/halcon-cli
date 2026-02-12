use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::agent::{AgentResult, AgentType};
use super::model::TokenUsage;
use super::tool::PermissionLevel;

/// Domain events emitted throughout the system.
///
/// Subscribers receive these via `tokio::sync::broadcast` channel.
/// Used for: logging, audit trail, UI updates, metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainEvent {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub payload: EventPayload,
}

impl DomainEvent {
    pub fn new(payload: EventPayload) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            payload,
        }
    }
}

/// The specific event that occurred.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    ModelInvoked {
        provider: String,
        model: String,
        usage: TokenUsage,
        latency_ms: u64,
    },
    ToolExecuted {
        tool: String,
        permission: PermissionLevel,
        duration_ms: u64,
        success: bool,
    },
    AgentStarted {
        agent_type: AgentType,
        task: String,
    },
    AgentCompleted {
        agent_type: AgentType,
        result: AgentResult,
    },
    PiiDetected {
        pii_type: String,
        action: PiiAction,
    },
    SessionStarted {
        session_id: Uuid,
    },
    SessionEnded {
        session_id: Uuid,
        total_usage: TokenUsage,
    },
    ConfigChanged {
        key: String,
        old_hash: String,
        new_hash: String,
    },
    PermissionRequested {
        tool: String,
        level: PermissionLevel,
    },
    PermissionGranted {
        tool: String,
        level: PermissionLevel,
    },
    PermissionDenied {
        tool: String,
        level: PermissionLevel,
    },
    CircuitBreakerTripped {
        provider: String,
        from_state: String,
        to_state: String,
    },
    HealthChanged {
        provider: String,
        old_score: u32,
        new_score: u32,
        level: String,
    },
    BackpressureSaturated {
        provider: String,
        current: u32,
        max: u32,
    },
    ProviderFallback {
        from_provider: String,
        to_provider: String,
        reason: String,
    },
    PlanGenerated {
        plan_id: Uuid,
        goal: String,
        step_count: usize,
        replan_count: u32,
    },
    PlanStepCompleted {
        plan_id: Uuid,
        step_index: usize,
        outcome: String,
    },
    GuardrailTriggered {
        guardrail: String,
        checkpoint: String,
        action: String,
    },
    PolicyDecision {
        tool: String,
        decision: String,
        context_id: uuid::Uuid,
    },
    ReflectionGenerated {
        round: usize,
        trigger: String,
    },
    EpisodeCreated {
        episode_id: String,
        title: String,
    },
    MemoryRetrieved {
        query: String,
        result_count: usize,
        top_score: f64,
    },
    SubAgentSpawned {
        orchestrator_id: Uuid,
        task_id: Uuid,
        agent_type: AgentType,
        instruction: String,
    },
    SubAgentCompleted {
        orchestrator_id: Uuid,
        task_id: Uuid,
        success: bool,
        latency_ms: u64,
        error: Option<String>,
    },
    OrchestratorCompleted {
        orchestrator_id: Uuid,
        success_count: usize,
        total_count: usize,
        total_cost_usd: f64,
    },
    AgentStateChanged {
        execution_id: Uuid,
        from: String,
        to: String,
        round: usize,
        reason: Option<String>,
    },
}

/// Action taken when PII is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiAction {
    Redacted,
    Blocked,
    Warned,
}
