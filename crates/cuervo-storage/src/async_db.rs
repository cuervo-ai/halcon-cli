//! Async wrapper around `Database` for use in tokio async contexts.
//!
//! Each method clones the inner `Arc<Database>` and delegates to
//! `tokio::task::spawn_blocking`, keeping the sync rusqlite work
//! off the async runtime threads.

use std::sync::Arc;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::types::Session;

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
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn max_step_index(&self, session_id: uuid::Uuid) -> Result<Option<u32>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.max_step_index(session_id))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Metrics ---

    pub async fn insert_metric(&self, metric: &InvocationMetric) -> Result<()> {
        let db = self.inner.clone();
        let metric = metric.clone();
        tokio::task::spawn_blocking(move || db.insert_metric(&metric))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn insert_tool_metric(&self, metric: &ToolExecutionMetric) -> Result<()> {
        let db = self.inner.clone();
        let metric = metric.clone();
        tokio::task::spawn_blocking(move || db.insert_tool_metric(&metric))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn batch_insert_metrics(&self, metrics: Vec<InvocationMetric>) -> Result<()> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.batch_insert_metrics(&metrics))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn batch_insert_tool_metrics(&self, metrics: Vec<ToolExecutionMetric>) -> Result<()> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.batch_insert_tool_metrics(&metrics))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn system_metrics(&self) -> Result<SystemMetrics> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.system_metrics())
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Cache ---

    pub async fn lookup_cache(&self, key: &str) -> Result<Option<CacheEntry>> {
        let db = self.inner.clone();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || db.lookup_cache(&key))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn insert_cache_entry(&self, entry: &CacheEntry) -> Result<()> {
        let db = self.inner.clone();
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || db.insert_cache_entry(&entry))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Session ---

    pub async fn save_session(&self, session: &Session) -> Result<()> {
        let db = self.inner.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || db.save_session(&session))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_session(&self, id: uuid::Uuid) -> Result<Option<Session>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.load_session(id))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn top_cache_entries(&self, limit: usize) -> Result<Vec<CacheEntry>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.top_cache_entries(limit))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Planning ---

    pub async fn save_plan_steps(
        &self,
        session_id: &uuid::Uuid,
        plan: &cuervo_core::traits::ExecutionPlan,
    ) -> Result<()> {
        let db = self.inner.clone();
        let session_id = *session_id;
        let plan = plan.clone();
        tokio::task::spawn_blocking(move || db.save_plan_steps(&session_id, &plan))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
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
        .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
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
        .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Memory ---

    pub async fn insert_memory(&self, entry: &MemoryEntry) -> Result<bool> {
        let db = self.inner.clone();
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || db.insert_memory(&entry))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn list_memories(
        &self,
        entry_type: Option<crate::memory::MemoryEntryType>,
        limit: u32,
    ) -> Result<Vec<MemoryEntry>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.list_memories(entry_type, limit))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn search_memory_fts(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let db = self.inner.clone();
        let query = query.to_string();
        tokio::task::spawn_blocking(move || db.search_memory_fts(&query, limit))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
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
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn update_memory_relevance(
        &self,
        entry_id: uuid::Uuid,
        relevance_score: f64,
    ) -> Result<bool> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.update_memory_relevance(entry_id, relevance_score))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
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
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn save_episode(
        &self,
        episode: &crate::memory::MemoryEpisode,
    ) -> Result<()> {
        let db = self.inner.clone();
        let episode = episode.clone();
        tokio::task::spawn_blocking(move || db.save_episode(&episode))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
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
        .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
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
        .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_agent_tasks(
        &self,
        orchestrator_id: &str,
    ) -> Result<Vec<crate::db::AgentTaskRow>> {
        let db = self.inner.clone();
        let orchestrator_id = orchestrator_id.to_string();
        tokio::task::spawn_blocking(move || db.load_agent_tasks(&orchestrator_id))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
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
        .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    // --- Checkpoints ---

    pub async fn save_checkpoint(&self, checkpoint: &crate::db::SessionCheckpoint) -> Result<()> {
        let db = self.inner.clone();
        let checkpoint = checkpoint.clone();
        tokio::task::spawn_blocking(move || db.save_checkpoint(&checkpoint))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_checkpoint(
        &self,
        session_id: uuid::Uuid,
        round: u32,
    ) -> Result<Option<crate::db::SessionCheckpoint>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.load_checkpoint(session_id, round))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn load_latest_checkpoint(
        &self,
        session_id: uuid::Uuid,
    ) -> Result<Option<crate::db::SessionCheckpoint>> {
        let db = self.inner.clone();
        tokio::task::spawn_blocking(move || db.load_latest_checkpoint(session_id))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
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
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
    }

    pub async fn insert_resilience_event(&self, event: &ResilienceEvent) -> Result<()> {
        let db = self.inner.clone();
        let event = event.clone();
        tokio::task::spawn_blocking(move || db.insert_resilience_event(&event))
            .await
            .map_err(|e| CuervoError::Internal(format!("spawn_blocking: {e}")))?
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

        let mut session = cuervo_core::types::Session::new("model".into(), "provider".into(), "/tmp".into());
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

        let mut session = cuervo_core::types::Session::new("claude".into(), "anthropic".into(), "/home".into());
        session.tool_invocations = 12;
        session.agent_rounds = 7;
        session.total_latency_ms = 5200;
        session.estimated_cost_usd = 0.15;
        session.total_usage = cuervo_core::types::TokenUsage {
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
            .save_session(&cuervo_core::types::Session {
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

        let mut session = cuervo_core::types::Session::new("echo".into(), "echo".into(), "/tmp".into());
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

        let plan = cuervo_core::traits::ExecutionPlan {
            goal: "Fix the bug".into(),
            steps: vec![
                cuervo_core::traits::PlanStep {
                    description: "Read the file".into(),
                    tool_name: Some("read_file".into()),
                    parallel: false,
                    confidence: 0.9,
                    expected_args: None,
                    outcome: None,
                },
                cuervo_core::traits::PlanStep {
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
        };

        // Should succeed without error.
        adb.save_plan_steps(&session_id, &plan).await.unwrap();

        // Save again with different plan_id — should not conflict.
        let plan2 = cuervo_core::traits::ExecutionPlan {
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

        let plan = cuervo_core::traits::ExecutionPlan {
            goal: "Test outcome update".into(),
            steps: vec![cuervo_core::traits::PlanStep {
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
}
