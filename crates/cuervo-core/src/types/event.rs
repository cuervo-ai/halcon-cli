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
    TaskCreated {
        task_id: Uuid,
        title: String,
        plan_id: Option<Uuid>,
    },
    TaskStatusChanged {
        task_id: Uuid,
        from: String,
        to: String,
        retry_count: u32,
    },
    TaskCompleted {
        task_id: Uuid,
        duration_ms: u64,
        artifact_count: usize,
        cost_usd: f64,
    },
    TaskFailed {
        task_id: Uuid,
        error: String,
        retry_eligible: bool,
        retry_count: u32,
    },
    ReasoningStarted {
        query_hash: String,
        complexity: String,
        task_type: String,
    },
    StrategySelected {
        strategy: String,
        confidence: f64,
        task_type: String,
    },
    EvaluationCompleted {
        score: f64,
        success: bool,
        strategy: String,
    },
    ExperienceRecorded {
        task_type: String,
        strategy: String,
        score: f64,
    },
    ReasoningRetry {
        attempt: u32,
        previous_score: f64,
        new_strategy: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_created_serde_roundtrip() {
        let payload = EventPayload::TaskCreated {
            task_id: Uuid::new_v4(),
            title: "Read config file".into(),
            plan_id: Some(Uuid::new_v4()),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"type\":\"task_created\""));
        let roundtrip: EventPayload = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, EventPayload::TaskCreated { ref title, .. } if title == "Read config file"));
    }

    #[test]
    fn task_status_changed_serde_roundtrip() {
        let payload = EventPayload::TaskStatusChanged {
            task_id: Uuid::new_v4(),
            from: "Ready".into(),
            to: "Running".into(),
            retry_count: 1,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"type\":\"task_status_changed\""));
        let roundtrip: EventPayload = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, EventPayload::TaskStatusChanged { retry_count: 1, .. }));
    }

    #[test]
    fn task_completed_serde_roundtrip() {
        let payload = EventPayload::TaskCompleted {
            task_id: Uuid::new_v4(),
            duration_ms: 1234,
            artifact_count: 2,
            cost_usd: 0.005,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"type\":\"task_completed\""));
        let roundtrip: EventPayload = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, EventPayload::TaskCompleted { duration_ms: 1234, artifact_count: 2, .. }));
    }

    #[test]
    fn task_failed_serde_roundtrip() {
        let payload = EventPayload::TaskFailed {
            task_id: Uuid::new_v4(),
            error: "file not found".into(),
            retry_eligible: true,
            retry_count: 2,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"type\":\"task_failed\""));
        let roundtrip: EventPayload = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, EventPayload::TaskFailed { retry_eligible: true, retry_count: 2, .. }));
    }

    #[test]
    fn existing_event_still_deserializes() {
        // Verify backward compatibility: pre-existing variant still works.
        let payload = EventPayload::ToolExecuted {
            tool: "bash".into(),
            permission: PermissionLevel::Destructive,
            duration_ms: 500,
            success: true,
        };
        let json = serde_json::to_string(&payload).unwrap();
        let roundtrip: EventPayload = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, EventPayload::ToolExecuted { ref tool, success: true, .. } if tool == "bash"));
    }

    #[test]
    fn task_created_no_plan_id() {
        let payload = EventPayload::TaskCreated {
            task_id: Uuid::new_v4(),
            title: "Standalone task".into(),
            plan_id: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"plan_id\":null"));
        let roundtrip: EventPayload = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, EventPayload::TaskCreated { plan_id: None, .. }));
    }

    // --- Phase 40: Reasoning engine event tests ---

    #[test]
    fn reasoning_started_serde_roundtrip() {
        let payload = EventPayload::ReasoningStarted {
            query_hash: "abc123".into(),
            complexity: "Moderate".into(),
            task_type: "CodeModification".into(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"type\":\"reasoning_started\""));
        let roundtrip: EventPayload = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, EventPayload::ReasoningStarted { ref complexity, .. } if complexity == "Moderate"));
    }

    #[test]
    fn strategy_selected_serde_roundtrip() {
        let payload = EventPayload::StrategySelected {
            strategy: "PlanExecuteReflect".into(),
            confidence: 0.85,
            task_type: "CodeGeneration".into(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"type\":\"strategy_selected\""));
        let roundtrip: EventPayload = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, EventPayload::StrategySelected { confidence, .. } if (confidence - 0.85).abs() < f64::EPSILON));
    }

    #[test]
    fn evaluation_completed_serde_roundtrip() {
        let payload = EventPayload::EvaluationCompleted {
            score: 0.78,
            success: true,
            strategy: "DirectExecution".into(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"type\":\"evaluation_completed\""));
        let roundtrip: EventPayload = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, EventPayload::EvaluationCompleted { success: true, .. }));
    }

    #[test]
    fn experience_recorded_serde_roundtrip() {
        let payload = EventPayload::ExperienceRecorded {
            task_type: "Debugging".into(),
            strategy: "PlanExecuteReflect".into(),
            score: 0.92,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"type\":\"experience_recorded\""));
        let roundtrip: EventPayload = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, EventPayload::ExperienceRecorded { ref task_type, .. } if task_type == "Debugging"));
    }

    #[test]
    fn reasoning_retry_serde_roundtrip() {
        let payload = EventPayload::ReasoningRetry {
            attempt: 2,
            previous_score: 0.35,
            new_strategy: "PlanExecuteReflect".into(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"type\":\"reasoning_retry\""));
        let roundtrip: EventPayload = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, EventPayload::ReasoningRetry { attempt: 2, .. }));
    }
}
