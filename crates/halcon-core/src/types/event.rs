use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::agent::{AgentResult, AgentType};
use super::model::TokenUsage;
use super::tool::PermissionLevel;
use crate::context::{current_session_id, current_span_id, current_trace_id, SpanId, TraceId};

/// Domain events emitted throughout the system.
///
/// Subscribers receive these via `tokio::sync::broadcast` channel.
/// Used for: logging, audit trail, UI updates, metrics.
///
/// `session_id`, `trace_id`, and `span_id` are automatically injected
/// from task-local `EXECUTION_CTX` when `DomainEvent::new()` is called
/// inside a `EXECUTION_CTX.scope(...)` block. Outside a scope they are
/// `None`/default (backward-compatible — no callsite changes required).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainEvent {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub payload: EventPayload,
    /// Session that produced this event — auto-injected from task-local EXECUTION_CTX.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    /// W3C Trace-Context trace identifier (128-bit hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<TraceId>,
    /// W3C Trace-Context span identifier (64-bit hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<SpanId>,
}

impl DomainEvent {
    pub fn new(payload: EventPayload) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            payload,
            // Auto-inject from task-local context (None when called outside a scope).
            session_id: current_session_id(),
            trace_id: current_trace_id(),
            span_id: current_span_id(),
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
    /// Generic circuit breaker transition — **deprecated**.
    ///
    /// Kept for deserialization of historical audit_log rows only.
    /// All new code must emit one of:
    /// - `CircuitBreakerOpened`   (Closed → Open)
    /// - `CircuitBreakerRecovered` (HalfOpen → Closed)
    /// - `CircuitBreakerHalfOpen`  (Open → HalfOpen)
    ///
    /// Using this variant in new code produces incorrect alerting because
    /// monitoring systems cannot distinguish a trip from a recovery.
    #[deprecated(
        since = "0.3.0",
        note = "use CircuitBreakerOpened / CircuitBreakerRecovered / CircuitBreakerHalfOpen"
    )]
    CircuitBreakerTripped {
        provider: String,
        from_state: String,
        to_state: String,
    },
    /// Closed → Open: circuit breaker has tripped due to repeated failures.
    CircuitBreakerOpened {
        provider: String,
        failure_count: u32,
    },
    /// HalfOpen → Closed: circuit breaker has fully recovered after successful probes.
    CircuitBreakerRecovered {
        provider: String,
    },
    /// Open → HalfOpen: circuit breaker is allowing a probe request.
    CircuitBreakerHalfOpen {
        provider: String,
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
    OrchestratorStarted {
        orchestrator_id: Uuid,
        task_count: usize,
        wave_count: usize,
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
    SdlcPhaseChanged {
        session_id: Uuid,
        phase: String,
        previous_phase: Option<String>,
        active_servers: Vec<String>,
    },
    ContextServerActivated {
        server_name: String,
        server_type: String,
        phase: String,
        priority: u32,
    },
    ContextServerHealthChanged {
        server_name: String,
        status: String,
        latency_ms: u64,
        consecutive_failures: u32,
    },
    SearchStarted {
        query: String,
        max_results: usize,
    },
    SearchCompleted {
        query: String,
        result_count: usize,
        cache_hit: bool,
        elapsed_ms: u64,
    },
    DocumentIndexed {
        doc_id: Uuid,
        url: String,
        tokens: usize,
    },
    IndexStatsUpdated {
        doc_count: usize,
        vocab_size: usize,
    },
    CrawlStarted {
        url: String,
        depth: usize,
    },
    CrawlCompleted {
        url: String,
        docs_indexed: usize,
    },
    IntegrationConnected {
        integration_name: String,
        protocol: String,
        endpoint: String,
    },
    IntegrationDisconnected {
        integration_name: String,
        reason: Option<String>,
    },
    IntegrationHealthChanged {
        integration_name: String,
        old_health: String,
        new_health: String,
        reason: Option<String>,
    },
    IntegrationMessageReceived {
        integration_name: String,
        source: String,
        sender: String,
        message_preview: String,
    },
    IntegrationMessageSent {
        integration_name: String,
        destination: String,
        message_preview: String,
        success: bool,
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
