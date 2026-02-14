use sha2::{Digest, Sha256};

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::types::{DomainEvent, EventPayload};

use super::Database;

impl Database {
    pub fn append_audit_event(&self, event: &DomainEvent) -> Result<()> {
        self.append_audit_event_with_session(event, None)
    }

    pub fn append_audit_event_with_session(
        &self,
        event: &DomainEvent,
        session_id: Option<&str>,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?;

        let payload_json = serde_json::to_string(&event.payload)
            .map_err(|e| CuervoError::DatabaseError(format!("serialize event: {e}")))?;

        let event_type = match &event.payload {
            EventPayload::ModelInvoked { .. } => "model_invoked",
            EventPayload::ToolExecuted { .. } => "tool_executed",
            EventPayload::AgentStarted { .. } => "agent_started",
            EventPayload::AgentCompleted { .. } => "agent_completed",
            EventPayload::PiiDetected { .. } => "pii_detected",
            EventPayload::SessionStarted { .. } => "session_started",
            EventPayload::SessionEnded { .. } => "session_ended",
            EventPayload::ConfigChanged { .. } => "config_changed",
            EventPayload::PermissionRequested { .. } => "permission_requested",
            EventPayload::PermissionGranted { .. } => "permission_granted",
            EventPayload::PermissionDenied { .. } => "permission_denied",
            EventPayload::CircuitBreakerTripped { .. } => "circuit_breaker_tripped",
            EventPayload::HealthChanged { .. } => "health_changed",
            EventPayload::BackpressureSaturated { .. } => "backpressure_saturated",
            EventPayload::ProviderFallback { .. } => "provider_fallback",
            EventPayload::PlanGenerated { .. } => "plan_generated",
            EventPayload::PlanStepCompleted { .. } => "plan_step_completed",
            EventPayload::GuardrailTriggered { .. } => "guardrail_triggered",
            EventPayload::PolicyDecision { .. } => "policy_decision",
            EventPayload::ReflectionGenerated { .. } => "reflection_generated",
            EventPayload::EpisodeCreated { .. } => "episode_created",
            EventPayload::MemoryRetrieved { .. } => "memory_retrieved",
            EventPayload::SubAgentSpawned { .. } => "sub_agent_spawned",
            EventPayload::SubAgentCompleted { .. } => "sub_agent_completed",
            EventPayload::OrchestratorCompleted { .. } => "orchestrator_completed",
            EventPayload::AgentStateChanged { .. } => "agent_state_changed",
            EventPayload::TaskCreated { .. } => "task_created",
            EventPayload::TaskStatusChanged { .. } => "task_status_changed",
            EventPayload::TaskCompleted { .. } => "task_completed",
            EventPayload::TaskFailed { .. } => "task_failed",
            EventPayload::ReasoningStarted { .. } => "reasoning_started",
            EventPayload::StrategySelected { .. } => "strategy_selected",
            EventPayload::EvaluationCompleted { .. } => "evaluation_completed",
            EventPayload::ExperienceRecorded { .. } => "experience_recorded",
            EventPayload::ReasoningRetry { .. } => "reasoning_retry",
        };

        // Get previous hash from in-memory cache (eliminates 1 SELECT per insert).
        let previous_hash = self
            .last_audit_hash
            .lock()
            .map_err(|e| CuervoError::DatabaseError(e.to_string()))?
            .clone();

        // Compute hash: SHA-256(previous_hash + event_id + timestamp + payload)
        let mut hasher = Sha256::new();
        hasher.update(previous_hash.as_bytes());
        hasher.update(event.id.to_string().as_bytes());
        hasher.update(event.timestamp.to_rfc3339().as_bytes());
        hasher.update(payload_json.as_bytes());
        let hash = hex::encode(hasher.finalize());

        conn.execute(
            "INSERT INTO audit_log (event_id, timestamp, event_type, payload_json, previous_hash, hash, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                event.id.to_string(),
                event.timestamp.to_rfc3339(),
                event_type,
                payload_json,
                previous_hash,
                hash,
                session_id,
            ],
        )
        .map_err(|e| CuervoError::DatabaseError(format!("append audit: {e}")))?;

        // Update in-memory cache with new hash.
        if let Ok(mut cached) = self.last_audit_hash.lock() {
            *cached = hash;
        }

        Ok(())
    }
}
