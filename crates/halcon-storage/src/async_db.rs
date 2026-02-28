//! Async wrapper around `Database` for use in tokio async contexts.
//!
//! Each method clones the inner `Arc<Database>` and delegates to
//! `tokio::task::spawn_blocking`, keeping the sync rusqlite work
//! off the async runtime threads.

use std::sync::Arc;

use halcon_core::error::{HalconError, Result};
use halcon_core::types::Session;

use crate::cache::CacheEntry;
use crate::memory::MemoryEntry;
use crate::metrics::{InvocationMetric, ProviderWindowedMetrics, SystemMetrics, ToolExecutionMetric};
use crate::resilience::ResilienceEvent;
use crate::trace::TraceStep;
use crate::Database;

/// Async bridge to [`Database`] via `spawn_blocking`.
///
/// Wraps `Arc<Database>` so it can be cloned cheaply and shared
/// across async tasks. Only the 10 methods called from async
/// contexts are wrapped — sync command handlers access the
/// underlying `Database` directly via [`inner()`].
#[derive(Clone)]
pub struct AsyncDatabase {
    inner: Arc<Database>,
}

impl AsyncDatabase {
    /// Create a new async wrapper around a shared `Database`.
    pub fn new(db: Arc<Database>) -> Self {
        Self { inner: db }
    }

    /// Access the underlying synchronous `Database` for command handlers.
    pub fn inner(&self) -> &Database {
        &self.inner
    }

    // --- Trace ---

    pub async fn append_trace_step(&self, step: &TraceStep) -> Result<()> {
        let db = self.inner.clone();
        let step = step.clone();
        tokio::task::spawn_blocking(move || db.append_trace_step(&step))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn max_step_index(&self, session_id: uuid::Uuid) -> Result<Option<u32>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.max_step_index(session_id))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Metrics ---

    pub async fn insert_metric(&self, metric: &InvocationMetric) -> Result<()> {
        let db = self.inner.clone();
        let metric = metric.clone();
        tokio::task::spawn_blocking(move || db.insert_metric(&metric))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn insert_tool_metric(&self, metric: &ToolExecutionMetric) -> Result<()> {
        let db = self.inner.clone();
        let metric = metric.clone();
        tokio::task::spawn_blocking(move || db.insert_tool_metric(&metric))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn batch_insert_metrics(&self, metrics: Vec<InvocationMetric>) -> Result<()> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.batch_insert_metrics(&metrics))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn batch_insert_tool_metrics(&self, metrics: Vec<ToolExecutionMetric>) -> Result<()> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.batch_insert_tool_metrics(&metrics))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn system_metrics(&self) -> Result<SystemMetrics> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.system_metrics())
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn recent_tool_executions(
        &self,
        tool_name: String,
        limit: usize,
    ) -> Result<Vec<crate::db::ToolExecutionRow>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.recent_tool_executions(&tool_name, limit))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn events_per_second_last_60s(&self) -> Result<f64> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.events_per_second_last_60s())
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Plugin circuit state (M34) ---

    /// Persist circuit breaker states for all plugins (call post-loop).
    pub async fn save_circuit_breaker_states(
        &self,
        rows: Vec<crate::db::CircuitBreakerStateRow>,
    ) -> Result<()> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.save_circuit_breaker_states(&rows))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    /// Load persisted circuit breaker states for all plugins (call at startup).
    pub async fn load_circuit_breaker_states(
        &self,
    ) -> Result<Vec<crate::db::CircuitBreakerStateRow>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.load_circuit_breaker_states())
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Cache ---

    pub async fn lookup_cache(&self, key: &str) -> Result<Option<CacheEntry>> {
        let db = self.inner.clone();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || db.lookup_cache(&key))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn insert_cache_entry(&self, entry: &CacheEntry) -> Result<()> {
        let db = self.inner.clone();
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || db.insert_cache_entry(&entry))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Session ---

    pub async fn save_session(&self, session: &Session) -> Result<()> {
        let db = self.inner.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || db.save_session(&session))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_session(&self, id: uuid::Uuid) -> Result<Option<Session>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.load_session(id))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn list_sessions(&self, limit: u32) -> Result<Vec<Session>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.list_sessions(limit))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    /// Set a session title only when none is stored yet (no-clobber).
    pub async fn update_session_title(&self, id: uuid::Uuid, title: String) -> Result<()> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.update_session_title(id, &title))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn top_cache_entries(&self, limit: usize) -> Result<Vec<CacheEntry>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.top_cache_entries(limit))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Planning ---

    pub async fn save_plan_steps(
        &self,
        session_id: &uuid::Uuid,
        plan: &halcon_core::traits::ExecutionPlan,
    ) -> Result<()> {
        let db = self.inner.clone();
        let session_id = *session_id;
        let plan = plan.clone();
        tokio::task::spawn_blocking(move || db.save_plan_steps(&session_id, &plan))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn update_plan_step_outcome(
        &self,
        plan_id: &uuid::Uuid,
        step_index: u32,
        outcome: &str,
        detail: &str,
    ) -> Result<()> {
        let db = self.inner.clone();
        let plan_id = *plan_id;
        let outcome = outcome.to_string();
        let detail = detail.to_string();
        tokio::task::spawn_blocking(move || {
            db.update_plan_step_outcome(&plan_id, step_index, &outcome, &detail)
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Policy decisions ---

    pub async fn save_policy_decision(
        &self,
        session_id: &uuid::Uuid,
        context_id: &uuid::Uuid,
        tool_name: &str,
        decision: &str,
        reason: Option<&str>,
        arguments_hash: Option<&str>,
    ) -> Result<()> {
        let db = self.inner.clone();
        let session_id = *session_id;
        let context_id = *context_id;
        let tool_name = tool_name.to_string();
        let decision = decision.to_string();
        let reason = reason.map(|s| s.to_string());
        let arguments_hash = arguments_hash.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            db.save_policy_decision(
                &session_id,
                &context_id,
                &tool_name,
                &decision,
                reason.as_deref(),
                arguments_hash.as_deref(),
            )
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Memory ---

    pub async fn insert_memory(&self, entry: &MemoryEntry) -> Result<bool> {
        let db = self.inner.clone();
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || db.insert_memory(&entry))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn list_memories(
        &self,
        entry_type: Option<crate::memory::MemoryEntryType>,
        limit: u32,
    ) -> Result<Vec<MemoryEntry>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.list_memories(entry_type, limit))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn search_memory_fts(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let db = self.inner.clone();
        let query = query.to_string();
        tokio::task::spawn_blocking(move || db.search_memory_fts(&query, limit))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn search_memory_by_embedding(
        &self,
        query_vec: &[f32],
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let db = self.inner.clone();
        let query_vec = query_vec.to_vec();
        tokio::task::spawn_blocking(move || db.search_memory_by_embedding(&query_vec, limit))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn update_memory_relevance(
        &self,
        entry_id: uuid::Uuid,
        relevance_score: f64,
    ) -> Result<bool> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.update_memory_relevance(entry_id, relevance_score))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn search_memory_fts_by_type(
        &self,
        query: &str,
        entry_type: crate::memory::MemoryEntryType,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let db = self.inner.clone();
        let query = query.to_string();
        tokio::task::spawn_blocking(move || db.search_memory_fts_by_type(&query, entry_type, limit))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn save_episode(
        &self,
        episode: &crate::memory::MemoryEpisode,
    ) -> Result<()> {
        let db = self.inner.clone();
        let episode = episode.clone();
        tokio::task::spawn_blocking(move || db.save_episode(&episode))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn link_entry_to_episode(
        &self,
        entry_uuid: &str,
        episode_id: &str,
        position: u32,
    ) -> Result<()> {
        let db = self.inner.clone();
        let entry_uuid = entry_uuid.to_string();
        let episode_id = episode_id.to_string();
        tokio::task::spawn_blocking(move || {
            db.link_entry_to_episode(&entry_uuid, &episode_id, position)
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Agent Tasks ---

    #[allow(clippy::too_many_arguments)]
    pub async fn save_agent_task(
        &self,
        task_id: &str,
        orchestrator_id: &str,
        session_id: &str,
        agent_type: &str,
        instruction: &str,
        status: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
        latency_ms: u64,
        rounds: u32,
        error_message: Option<&str>,
        output_text: Option<&str>,
    ) -> Result<()> {
        let db = self.inner.clone();
        let task_id = task_id.to_string();
        let orchestrator_id = orchestrator_id.to_string();
        let session_id = session_id.to_string();
        let agent_type = agent_type.to_string();
        let instruction = instruction.to_string();
        let status = status.to_string();
        let error_message = error_message.map(|s| s.to_string());
        let output_text = output_text.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            db.save_agent_task(
                &task_id,
                &orchestrator_id,
                &session_id,
                &agent_type,
                &instruction,
                &status,
                input_tokens,
                output_tokens,
                cost_usd,
                latency_ms,
                rounds,
                error_message.as_deref(),
                output_text.as_deref(),
            )
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_agent_tasks(
        &self,
        orchestrator_id: &str,
    ) -> Result<Vec<crate::db::AgentTaskRow>> {
        let db = self.inner.clone();
        let orchestrator_id = orchestrator_id.to_string();
        tokio::task::spawn_blocking(move || db.load_agent_tasks(&orchestrator_id))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_agent_task_status(
        &self,
        task_id: &str,
        status: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
        latency_ms: u64,
        rounds: u32,
        error_message: Option<&str>,
        output_text: Option<&str>,
    ) -> Result<()> {
        let db = self.inner.clone();
        let task_id = task_id.to_string();
        let status = status.to_string();
        let error_message = error_message.map(|s| s.to_string());
        let output_text = output_text.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            db.update_agent_task_status(
                &task_id,
                &status,
                input_tokens,
                output_tokens,
                cost_usd,
                latency_ms,
                rounds,
                error_message.as_deref(),
                output_text.as_deref(),
            )
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Checkpoints ---

    pub async fn save_checkpoint(&self, checkpoint: &crate::db::SessionCheckpoint) -> Result<()> {
        let db = self.inner.clone();
        let checkpoint = checkpoint.clone();
        tokio::task::spawn_blocking(move || db.save_checkpoint(&checkpoint))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_checkpoint(
        &self,
        session_id: uuid::Uuid,
        round: u32,
    ) -> Result<Option<crate::db::SessionCheckpoint>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.load_checkpoint(session_id, round))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_latest_checkpoint(
        &self,
        session_id: uuid::Uuid,
    ) -> Result<Option<crate::db::SessionCheckpoint>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.load_latest_checkpoint(session_id))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Resilience ---

    pub async fn provider_metrics_windowed(
        &self,
        provider: &str,
        window_minutes: u64,
    ) -> Result<ProviderWindowedMetrics> {
        let db = self.inner.clone();
        let provider = provider.to_string();
        tokio::task::spawn_blocking(move || db.provider_metrics_windowed(&provider, window_minutes))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn insert_resilience_event(&self, event: &ResilienceEvent) -> Result<()> {
        let db = self.inner.clone();
        let event = event.clone();
        tokio::task::spawn_blocking(move || db.insert_resilience_event(&event))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Structured Tasks ---

    #[allow(clippy::too_many_arguments)]
    pub async fn save_structured_task(
        &self,
        task_id: String,
        session_id: Option<String>,
        plan_id: Option<String>,
        step_index: Option<i64>,
        title: String,
        description: String,
        status: String,
        priority: i64,
        depends_on_json: String,
        inputs_json: String,
        outputs_json: String,
        artifacts_json: String,
        provenance_json: Option<String>,
        retry_policy_json: String,
        retry_count: i64,
        tags_json: String,
        tool_name: Option<String>,
        expected_args_json: Option<String>,
        error: Option<String>,
        created_at: String,
        started_at: Option<String>,
        finished_at: Option<String>,
        duration_ms: Option<i64>,
    ) -> Result<()> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            db.save_structured_task(
                &task_id,
                session_id.as_deref(),
                plan_id.as_deref(),
                step_index,
                &title,
                &description,
                &status,
                priority,
                &depends_on_json,
                &inputs_json,
                &outputs_json,
                &artifacts_json,
                provenance_json.as_deref(),
                &retry_policy_json,
                retry_count,
                &tags_json,
                tool_name.as_deref(),
                expected_args_json.as_deref(),
                error.as_deref(),
                &created_at,
                started_at.as_deref(),
                finished_at.as_deref(),
                duration_ms,
            )
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_structured_tasks_by_session(
        &self,
        session_id: String,
    ) -> Result<Vec<crate::db::StructuredTaskRow>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.load_structured_tasks_by_session(&session_id))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_structured_tasks_by_plan(
        &self,
        plan_id: String,
    ) -> Result<Vec<crate::db::StructuredTaskRow>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.load_structured_tasks_by_plan(&plan_id))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_incomplete_structured_tasks(
        &self,
    ) -> Result<Vec<crate::db::StructuredTaskRow>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.load_incomplete_structured_tasks())
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn delete_structured_tasks_by_session(
        &self,
        session_id: String,
    ) -> Result<u64> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.delete_structured_tasks_by_session(&session_id))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Reasoning Experience ---

    pub async fn save_reasoning_experience(
        &self,
        task_type: &str,
        strategy: &str,
        score: f64,
    ) -> Result<()> {
        let db = self.inner.clone();
        let task_type = task_type.to_string();
        let strategy = strategy.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn()?;
            crate::db::reasoning::save_reasoning_experience(&conn, &task_type, &strategy, score)
                .map_err(|e| HalconError::DatabaseError(e.to_string()))
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_reasoning_experience(
        &self,
        task_type: &str,
        strategy: &str,
    ) -> Result<Option<crate::db::reasoning::ReasoningExperience>> {
        let db = self.inner.clone();
        let task_type = task_type.to_string();
        let strategy = strategy.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn()?;
            crate::db::reasoning::load_reasoning_experience(&conn, &task_type, &strategy)
                .map_err(|e| HalconError::DatabaseError(e.to_string()))
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_all_reasoning_experiences(
        &self,
    ) -> Result<Vec<crate::db::reasoning::ReasoningExperience>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn()?;
            crate::db::reasoning::load_all_reasoning_experiences(&conn)
                .map_err(|e| HalconError::DatabaseError(e.to_string()))
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Model Quality Stats (Phase 4: cross-session ModelPerformanceTracker) ---

    /// Persist accumulated model quality stats for a provider after a session message completes.
    ///
    /// Non-fatal: errors are logged but do not propagate — quality persistence is best-effort.
    pub async fn save_model_quality_stats(
        &self,
        provider: &str,
        stats: Vec<(String, u32, u32, f64)>,
    ) -> Result<()> {
        let db = self.inner.clone();
        let provider = provider.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn()?;
            crate::db::model_quality::save_all_model_quality_stats(&conn, &provider, &stats)
                .map_err(|e| HalconError::DatabaseError(e.to_string()))
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    /// Load model quality stats for a provider at session start for seeding ModelSelector.
    ///
    /// Returns Vec of (model_id, success_count, failure_count, total_reward) tuples.
    pub async fn load_model_quality_stats(
        &self,
        provider: &str,
    ) -> Result<Vec<(String, u32, u32, f64)>> {
        let db = self.inner.clone();
        let provider = provider.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn()?;
            crate::db::model_quality::load_model_quality_stats(&conn, &provider)
                .map_err(|e| HalconError::DatabaseError(e.to_string()))
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Permission Rules ---

    pub async fn save_permission_rule(&self, rule: &halcon_core::types::PermissionRule) -> Result<()> {
        let db = self.inner.clone();
        let rule = rule.clone();
        tokio::task::spawn_blocking(move || db.save_permission_rule(&rule))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn update_permission_rule(&self, rule: &halcon_core::types::PermissionRule) -> Result<()> {
        let db = self.inner.clone();
        let rule = rule.clone();
        tokio::task::spawn_blocking(move || db.update_permission_rule(&rule))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn delete_permission_rule(&self, rule_id: &str) -> Result<()> {
        let db = self.inner.clone();
        let rule_id = rule_id.to_string();
        tokio::task::spawn_blocking(move || db.delete_permission_rule(&rule_id))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_permission_rule(&self, rule_id: &str) -> Result<Option<halcon_core::types::PermissionRule>> {
        let db = self.inner.clone();
        let rule_id = rule_id.to_string();
        tokio::task::spawn_blocking(move || db.load_permission_rule(&rule_id))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn find_permission_rules_by_scope(
        &self,
        scope: halcon_core::types::RuleScope,
    ) -> Result<Vec<halcon_core::types::PermissionRule>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.find_permission_rules_by_scope(scope))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn find_permission_rules_by_tool(&self, tool_name: &str) -> Result<Vec<halcon_core::types::PermissionRule>> {
        let db = self.inner.clone();
        let tool_name = tool_name.to_string();
        tokio::task::spawn_blocking(move || db.find_permission_rules_by_tool(&tool_name))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn find_permission_rules_by_scope_value(
        &self,
        scope: halcon_core::types::RuleScope,
        scope_value: &str,
    ) -> Result<Vec<halcon_core::types::PermissionRule>> {
        let db = self.inner.clone();
        let scope_value = scope_value.to_string();
        tokio::task::spawn_blocking(move || db.find_permission_rules_by_scope_value(scope, &scope_value))
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_all_permission_rules(&self) -> Result<Vec<halcon_core::types::PermissionRule>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.load_all_permission_rules())
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn cleanup_expired_permission_rules(&self) -> Result<usize> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.cleanup_expired_permission_rules())
            .await
            .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Activity Search History (Phase 3 SRCH-004) ---

    pub async fn save_search_history(
        &self,
        query: String,
        search_mode: String,
        match_count: i32,
        session_id: Option<String>,
    ) -> rusqlite::Result<i64> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            db.save_search_history(&query, &search_mode, match_count, session_id.as_deref())
        })
        .await
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
            format!("spawn_blocking: {e}"),
        ))))?
    }

    pub async fn load_search_history(
        &self,
        limit: usize,
    ) -> rusqlite::Result<Vec<crate::db::activity_search::ActivitySearchEntry>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.load_search_history(limit))
            .await
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                format!("spawn_blocking: {e}"),
            ))))?
    }

    pub async fn get_recent_queries(&self, limit: usize) -> rusqlite::Result<Vec<String>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.get_recent_queries(limit))
            .await
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                format!("spawn_blocking: {e}"),
            ))))?
    }

    pub async fn clear_search_history(&self) -> rusqlite::Result<()> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.clear_search_history())
            .await
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                format!("spawn_blocking: {e}"),
            ))))?
    }

    // ── Media Cache (M27) ────────────────────────────────────────────────────

    /// Get a media analysis from the cache.
    pub async fn get_media_cache(
        &self,
        content_hash: &str,
    ) -> halcon_core::error::Result<Option<crate::media::MediaCacheEntry>> {
        let db = self.inner.clone();
        let hash = content_hash.to_owned();
        tokio::task::spawn_blocking(move || db.get_media_cache(&hash))
            .await
            .map_err(|e| halcon_core::error::HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    /// Store a media analysis result in the cache.
    pub async fn store_media_cache(
        &self,
        entry: &crate::media::MediaCacheEntry,
    ) -> halcon_core::error::Result<()> {
        let db = self.inner.clone();
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || db.store_media_cache(&entry))
            .await
            .map_err(|e| halcon_core::error::HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    /// Evict expired media cache entries (older than TTL seconds).
    pub async fn evict_expired_media_cache(
        &self,
        ttl_secs: u64,
    ) -> halcon_core::error::Result<usize> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.evict_expired_media_cache(ttl_secs))
            .await
            .map_err(|e| halcon_core::error::HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // ── Media Index (M28) ────────────────────────────────────────────────────

    /// Store a media embedding in the index.
    pub async fn store_media_index_entry(
        &self,
        entry: &crate::media::MediaIndexEntry,
    ) -> halcon_core::error::Result<()> {
        let db = self.inner.clone();
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || db.store_media_index_entry(&entry))
            .await
            .map_err(|e| halcon_core::error::HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    /// Search media index by cosine similarity (linear scan, returns top-k).
    pub async fn search_media_index(
        &self,
        query_embedding: Vec<f32>,
        modality: Option<&str>,
        top_k: usize,
    ) -> halcon_core::error::Result<Vec<crate::media::MediaIndexEntry>> {
        let db = self.inner.clone();
        let modality = modality.map(|s| s.to_owned());
        tokio::task::spawn_blocking(move || {
            db.search_media_index(&query_embedding, modality.as_deref(), top_k)
        })
        .await
        .map_err(|e| halcon_core::error::HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Palette Optimization History ---

    /// Persist the result of an adaptive palette optimization run.
    ///
    /// Non-fatal: errors are logged but do not propagate to the caller.
    #[allow(clippy::too_many_arguments)]
    pub async fn save_palette_optimization(
        &self,
        session_id: &str,
        base_hue: f64,
        initial_quality: f64,
        final_quality: f64,
        quality_delta: f64,
        iterations: usize,
        convergence_status: &str,
        duration_ms: u64,
        steps_json: &str,
    ) -> Result<()> {
        let db = self.inner.clone();
        let session_id = session_id.to_string();
        let convergence_status = convergence_status.to_string();
        let steps_json = steps_json.to_string();
        tokio::task::spawn_blocking(move || {
            db.with_connection(|conn| {
                crate::db::palette_optimization::save_palette_optimization(
                    conn,
                    &session_id,
                    base_hue,
                    initial_quality,
                    final_quality,
                    quality_delta,
                    iterations,
                    &convergence_status,
                    duration_ms,
                    &steps_json,
                )
            })
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
        .map_err(|e| HalconError::DatabaseError(e.to_string()))
    }

    /// Load the best prior palette optimization for a hue bucket (±`tolerance_deg`).
    ///
    /// Returns `None` if no prior run exists for that hue range.
    pub async fn load_best_palette_for_hue(
        &self,
        base_hue: f64,
        tolerance_deg: f64,
    ) -> Result<Option<crate::db::PaletteOptimizationRecord>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            db.with_connection(|conn| {
                crate::db::palette_optimization::load_best_palette_for_hue(
                    conn,
                    base_hue,
                    tolerance_deg,
                )
            })
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
        .map_err(|e| HalconError::DatabaseError(e.to_string()))
    }

    // --- Plugin System (M31) ---

    /// Persist an installed plugin manifest record.
    pub async fn save_installed_plugin(
        &self,
        plugin: crate::db::plugins::InstalledPlugin,
    ) -> Result<()> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            db.save_installed_plugin(&plugin)
                .map_err(|e| HalconError::DatabaseError(e.to_string()))
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    /// Load all installed plugin records.
    pub async fn load_installed_plugins(
        &self,
    ) -> Result<Vec<crate::db::plugins::InstalledPlugin>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            db.load_installed_plugins()
                .map_err(|e| HalconError::DatabaseError(e.to_string()))
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    /// Persist plugin UCB1 metrics (fire-and-forget from session end).
    pub async fn save_plugin_metrics(
        &self,
        metrics: Vec<crate::db::plugins::PluginMetricsRecord>,
    ) -> Result<()> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            db.save_plugin_metrics(&metrics)
                .map_err(|e| HalconError::DatabaseError(e.to_string()))
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    /// Load all plugin metrics records.
    pub async fn load_plugin_metrics(
        &self,
    ) -> Result<Vec<crate::db::plugins::PluginMetricsRecord>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            db.load_plugin_metrics()
                .map_err(|e| HalconError::DatabaseError(e.to_string()))
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Phase 1: Loop Observability ---

    /// Persist one structured loop event to `execution_loop_events` (Phase 1).
    pub async fn save_loop_event(
        &self,
        session_id: String,
        round: u32,
        event_type: String,
        event_json: String,
    ) -> Result<()> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            db.save_loop_event(&session_id, round, &event_type, &event_json)
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::trace::TraceStepType;
    use sha2::{Digest, Sha256};
    use uuid::Uuid;

    fn test_async_db() -> AsyncDatabase {
        AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()))
    }

    #[tokio::test]
    async fn async_db_trace_step() {
        let adb = test_async_db();
        let session_id = Uuid::new_v4();

        let step = TraceStep {
            session_id,
            step_index: 0,
            step_type: TraceStepType::ModelRequest,
            data_json: r#"{"round":0}"#.to_string(),
            duration_ms: 42,
            timestamp: Utc::now(),
        };
        adb.append_trace_step(&step).await.unwrap();

        // Verify via sync inner.
        let steps = adb.inner().load_trace_steps(session_id).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_index, 0);
        assert_eq!(steps[0].step_type, TraceStepType::ModelRequest);
    }

    #[tokio::test]
    async fn async_db_metric_insert() {
        let adb = test_async_db();

        let metric = InvocationMetric {
            provider: "test".to_string(),
            model: "m".to_string(),
            latency_ms: 200,
            input_tokens: 100,
            output_tokens: 50,
            estimated_cost_usd: 0.001,
            success: true,
            stop_reason: "end_turn".to_string(),
            session_id: None,
            created_at: Utc::now(),
        };
        adb.insert_metric(&metric).await.unwrap();

        let sys = adb.system_metrics().await.unwrap();
        assert_eq!(sys.total_invocations, 1);
    }

    #[tokio::test]
    async fn async_db_cache_round_trip() {
        let adb = test_async_db();

        let entry = CacheEntry {
            cache_key: "test-key".to_string(),
            model: "claude".to_string(),
            response_text: "hello world".to_string(),
            tool_calls_json: None,
            stop_reason: "end_turn".to_string(),
            usage_json: "{}".to_string(),
            created_at: Utc::now(),
            expires_at: None,
            hit_count: 0,
        };
        adb.insert_cache_entry(&entry).await.unwrap();

        let found = adb.lookup_cache("test-key").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().response_text, "hello world");
    }

    #[tokio::test]
    async fn async_db_session_save_load() {
        let adb = test_async_db();

        let session = Session::new("model".into(), "provider".into(), "/tmp".into());
        let id = session.id;
        adb.save_session(&session).await.unwrap();

        let loaded = adb.inner().load_session(id).unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().id, id);
    }

    #[tokio::test]
    async fn async_db_memory_search() {
        let adb = test_async_db();

        let content = "Rust workspace with nine crates for CLI tool";
        let hash = hex::encode(Sha256::digest(content.as_bytes()));
        let entry = MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: crate::memory::MemoryEntryType::Fact,
            content: content.to_string(),
            content_hash: hash,
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        adb.insert_memory(&entry).await.unwrap();

        let results = adb.search_memory_fts("rust", 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Rust"));
    }

    #[tokio::test]
    async fn async_db_load_session() {
        let adb = test_async_db();

        let mut session = halcon_core::types::Session::new("model".into(), "provider".into(), "/tmp".into());
        session.tool_invocations = 5;
        session.agent_rounds = 3;
        session.total_latency_ms = 1500;
        session.estimated_cost_usd = 0.042;
        let id = session.id;
        adb.save_session(&session).await.unwrap();

        let loaded = adb.load_session(id).await.unwrap().unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.tool_invocations, 5);
        assert_eq!(loaded.agent_rounds, 3);
        assert_eq!(loaded.total_latency_ms, 1500);
        assert!((loaded.estimated_cost_usd - 0.042).abs() < 0.001);
    }

    #[tokio::test]
    async fn resume_preserves_metrics() {
        let adb = test_async_db();

        let mut session = halcon_core::types::Session::new("claude".into(), "anthropic".into(), "/home".into());
        session.tool_invocations = 12;
        session.agent_rounds = 7;
        session.total_latency_ms = 5200;
        session.estimated_cost_usd = 0.15;
        session.total_usage = halcon_core::types::TokenUsage {
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        let id = session.id;
        adb.save_session(&session).await.unwrap();

        // Simulate resume: load and verify all fields roundtrip.
        let loaded = adb.load_session(id).await.unwrap().unwrap();
        assert_eq!(loaded.tool_invocations, 12);
        assert_eq!(loaded.agent_rounds, 7);
        assert_eq!(loaded.total_latency_ms, 5200);
        assert!((loaded.estimated_cost_usd - 0.15).abs() < 0.001);
        assert_eq!(loaded.total_usage.input_tokens, 1000);
        assert_eq!(loaded.total_usage.output_tokens, 500);
    }

    #[tokio::test]
    async fn backwards_compat_old_sessions() {
        // Simulate a pre-migration-007 session by inserting directly with only old columns.
        let adb = test_async_db();
        let id = Uuid::new_v4();
        let now = Utc::now();

        // Insert using old column set (the new columns have DEFAULT values).
        adb.inner()
            .save_session(&halcon_core::types::Session {
                id,
                title: None,
                model: "echo".into(),
                provider: "echo".into(),
                working_directory: "/tmp".into(),
                messages: vec![],
                total_usage: Default::default(),
                created_at: now,
                updated_at: now,
                tool_invocations: 0,
                agent_rounds: 0,
                total_latency_ms: 0,
                estimated_cost_usd: 0.0,
                execution_fingerprint: None,
                replay_source_session: None,
            })
            .unwrap();

        let loaded = adb.load_session(id).await.unwrap().unwrap();
        assert_eq!(loaded.tool_invocations, 0);
        assert_eq!(loaded.agent_rounds, 0);
        assert_eq!(loaded.total_latency_ms, 0);
        assert!((loaded.estimated_cost_usd).abs() < 0.001);
    }

    #[tokio::test]
    async fn concurrent_save_does_not_corrupt() {
        let adb = test_async_db();

        let mut session = halcon_core::types::Session::new("echo".into(), "echo".into(), "/tmp".into());
        let id = session.id;

        // Save multiple times concurrently.
        let mut handles = Vec::new();
        for i in 0..5u32 {
            let adb_clone = adb.clone();
            let mut s = session.clone();
            s.tool_invocations = i;
            handles.push(tokio::spawn(async move {
                adb_clone.save_session(&s).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        // Should load without corruption (last writer wins).
        let loaded = adb.load_session(id).await.unwrap().unwrap();
        assert_eq!(loaded.id, id);
        // tool_invocations is one of [0..4] — we don't know which write won, just that it's valid.
        assert!(loaded.tool_invocations < 5);

        // Update session with final state and re-save.
        session.tool_invocations = 99;
        adb.save_session(&session).await.unwrap();
        let final_loaded = adb.load_session(id).await.unwrap().unwrap();
        assert_eq!(final_loaded.tool_invocations, 99);
    }

    #[tokio::test]
    async fn async_db_save_plan_steps() {
        let adb = test_async_db();
        let session_id = Uuid::new_v4();

        let plan = halcon_core::traits::ExecutionPlan {
            goal: "Fix the bug".into(),
            steps: vec![
                halcon_core::traits::PlanStep {
                    step_id: Uuid::new_v4(),
                    description: "Read the file".into(),
                    tool_name: Some("read_file".into()),
                    parallel: false,
                    confidence: 0.9,
                    expected_args: None,
                    outcome: None,
                },
                halcon_core::traits::PlanStep {
                    step_id: Uuid::new_v4(),
                    description: "Edit the file".into(),
                    tool_name: Some("edit_file".into()),
                    parallel: false,
                    confidence: 0.8,
                    expected_args: None,
                    outcome: None,
                },
            ],
            requires_confirmation: false,
            plan_id: Uuid::new_v4(),
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        };

        // Should succeed without error.
        adb.save_plan_steps(&session_id, &plan).await.unwrap();

        // Save again with different plan_id — should not conflict.
        let plan2 = halcon_core::traits::ExecutionPlan {
            plan_id: Uuid::new_v4(),
            ..plan
        };
        adb.save_plan_steps(&session_id, &plan2).await.unwrap();
    }

    #[tokio::test]
    async fn async_db_update_plan_step_outcome() {
        let adb = test_async_db();
        let session_id = Uuid::new_v4();
        let plan_id = Uuid::new_v4();

        let plan = halcon_core::traits::ExecutionPlan {
            goal: "Test outcome update".into(),
            steps: vec![halcon_core::traits::PlanStep {
                step_id: Uuid::new_v4(),
                description: "Step one".into(),
                tool_name: Some("bash".into()),
                parallel: false,
                confidence: 0.95,
                expected_args: None,
                outcome: None,
            }],
            requires_confirmation: false,
            plan_id,
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        };

        adb.save_plan_steps(&session_id, &plan).await.unwrap();

        // Update outcome — should succeed without error.
        adb.update_plan_step_outcome(&plan_id, 0, "success", "Completed OK")
            .await
            .unwrap();

        // Update again to "failed" — idempotent, no error.
        adb.update_plan_step_outcome(&plan_id, 0, "failed", "Retried and failed")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn async_db_update_memory_relevance() {
        let adb = test_async_db();

        let content = "Always validate inputs before processing";
        let hash = hex::encode(Sha256::digest(content.as_bytes()));
        let entry = MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: crate::memory::MemoryEntryType::Reflection,
            content: content.to_string(),
            content_hash: hash,
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        adb.insert_memory(&entry).await.unwrap();

        // Boost relevance via async wrapper.
        let updated = adb.update_memory_relevance(entry.entry_id, 1.8).await.unwrap();
        assert!(updated);

        // Verify via sync inner.
        let loaded = adb.inner().load_memory(entry.entry_id).unwrap().unwrap();
        assert!((loaded.relevance_score - 1.8).abs() < 0.001);
    }

    #[tokio::test]
    async fn async_db_search_memory_fts_by_type() {
        let adb = test_async_db();

        let content = "Rust error handling with thiserror reflection";
        let hash = hex::encode(Sha256::digest(content.as_bytes()));
        let entry = MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: crate::memory::MemoryEntryType::Reflection,
            content: content.to_string(),
            content_hash: hash,
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        adb.insert_memory(&entry).await.unwrap();

        // Search with type filter.
        let results = adb
            .search_memory_fts_by_type("rust error", crate::memory::MemoryEntryType::Reflection, 5)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);

        // Search wrong type → empty.
        let results = adb
            .search_memory_fts_by_type("rust error", crate::memory::MemoryEntryType::Fact, 5)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn async_db_resilience_event() {
        let adb = test_async_db();

        let event = ResilienceEvent {
            provider: "test".to_string(),
            event_type: "breaker_transition".to_string(),
            from_state: Some("closed".to_string()),
            to_state: Some("open".to_string()),
            score: None,
            details: None,
            created_at: Utc::now(),
        };
        adb.insert_resilience_event(&event).await.unwrap();

        // Verify via sync inner.
        let events = adb.inner().resilience_events(Some("test"), None, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "breaker_transition");
    }

    // ─── New query method tests (FASE D) ─────────────────────────────────────

    #[tokio::test]
    async fn async_db_recent_tool_executions_empty_for_unknown_tool() {
        let adb = test_async_db();
        let rows = adb.recent_tool_executions("no_such_tool".to_string(), 10).await.unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn async_db_recent_tool_executions_returns_inserted_rows() {
        let adb = test_async_db();
        let metric = crate::metrics::ToolExecutionMetric {
            tool_name:    "bash".to_string(),
            session_id:   None,
            duration_ms:  75,
            success:      true,
            is_parallel:  false,
            input_summary: Some("echo hello".into()),
            created_at:   Utc::now(),
        };
        adb.insert_tool_metric(&metric).await.unwrap();

        let rows = adb.recent_tool_executions("bash".to_string(), 5).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tool_name, "bash");
        assert_eq!(rows[0].duration_ms, 75);
        assert!(rows[0].success);
    }

    #[tokio::test]
    async fn async_db_events_per_second_zero_when_empty() {
        let adb = test_async_db();
        let eps = adb.events_per_second_last_60s().await.unwrap();
        assert_eq!(eps, 0.0);
    }

    #[tokio::test]
    async fn async_db_events_per_second_increases_after_insert() {
        let adb = test_async_db();
        let metric = InvocationMetric {
            provider: "test".to_string(),
            model:    "m".to_string(),
            latency_ms:           50,
            input_tokens:         10,
            output_tokens:        5,
            estimated_cost_usd:   0.0,
            success:              true,
            stop_reason:          "end_turn".into(),
            session_id:           None,
            created_at:           Utc::now(),
        };
        adb.insert_metric(&metric).await.unwrap();
        let eps = adb.events_per_second_last_60s().await.unwrap();
        assert!(eps > 0.0, "eps should be > 0 after inserting a recent metric");
    }
}
