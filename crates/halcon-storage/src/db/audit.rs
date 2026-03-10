use hmac::{Hmac, Mac};
use sha2::Sha256;

use halcon_core::error::{HalconError, Result};
use halcon_core::types::{DomainEvent, EventPayload};

use super::Database;

/// HMAC-SHA256: keyed hash for tamper-evident audit chain.
///
/// Unlike bare SHA-256, an attacker who modifies the SQLite rows cannot
/// recompute valid hashes without knowing the per-database HMAC key
/// (stored in `audit_hmac_key`, never embedded in the audit log itself).
type HmacSha256 = Hmac<Sha256>;

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
            .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

        let payload_json = serde_json::to_string(&event.payload)
            .map_err(|e| HalconError::DatabaseError(format!("serialize event: {e}")))?;

        // CircuitBreakerTripped is deprecated for new code but must remain here to map
        // historical rows that were written before the variant was split into specific types.
        #[allow(deprecated)]
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
            EventPayload::CircuitBreakerOpened { .. } => "circuit_breaker_opened",
            EventPayload::CircuitBreakerRecovered { .. } => "circuit_breaker_recovered",
            EventPayload::CircuitBreakerHalfOpen { .. } => "circuit_breaker_half_open",
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
            EventPayload::OrchestratorStarted { .. } => "orchestrator_started",
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
            EventPayload::SdlcPhaseChanged { .. } => "sdlc_phase_changed",
            EventPayload::ContextServerActivated { .. } => "context_server_activated",
            EventPayload::ContextServerHealthChanged { .. } => "context_server_health_changed",
            EventPayload::SearchStarted { .. } => "search_started",
            EventPayload::SearchCompleted { .. } => "search_completed",
            EventPayload::DocumentIndexed { .. } => "document_indexed",
            EventPayload::IndexStatsUpdated { .. } => "index_stats_updated",
            EventPayload::CrawlStarted { .. } => "crawl_started",
            EventPayload::CrawlCompleted { .. } => "crawl_completed",
            EventPayload::IntegrationConnected { .. } => "integration_connected",
            EventPayload::IntegrationDisconnected { .. } => "integration_disconnected",
            EventPayload::IntegrationHealthChanged { .. } => "integration_health_changed",
            EventPayload::IntegrationMessageReceived { .. } => "integration_message_received",
            EventPayload::IntegrationMessageSent { .. } => "integration_message_sent",
        };

        // Get previous hash from in-memory cache (eliminates 1 SELECT per insert).
        let previous_hash = self
            .last_audit_hash
            .lock()
            .map_err(|e| HalconError::DatabaseError(e.to_string()))?
            .clone();

        // Compute HMAC-SHA256(key, previous_hash ‖ event_id ‖ timestamp ‖ payload).
        // Keyed MAC: without the per-database key the hash cannot be forged even if
        // the raw DB file is modified (attacker cannot recompute the chain).
        let mut mac = HmacSha256::new_from_slice(&self.audit_hmac_key)
            .expect("HMAC accepts keys of any length");
        mac.update(previous_hash.as_bytes());
        mac.update(event.id.to_string().as_bytes());
        mac.update(event.timestamp.to_rfc3339().as_bytes());
        mac.update(payload_json.as_bytes());
        let hash = hex::encode(mac.finalize().into_bytes());

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
        .map_err(|e| HalconError::DatabaseError(format!("append audit: {e}")))?;

        // Update in-memory cache with new hash.
        if let Ok(mut cached) = self.last_audit_hash.lock() {
            *cached = hash;
        }

        Ok(())
    }
}
