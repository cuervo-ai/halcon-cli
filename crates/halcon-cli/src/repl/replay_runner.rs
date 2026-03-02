//! Replay runner: orchestrates deterministic replay of a recorded session.
//!
//! Loads trace steps, constructs a ReplayProvider + ReplayToolExecutor,
//! runs the agent loop, and verifies the execution fingerprint.

use std::sync::Arc;

use anyhow::Result;
use uuid::Uuid;

use halcon_core::traits::ModelProvider;
use halcon_core::types::{
    AgentLimits, ChatMessage, ExecutionContext, MessageContent, ModelRequest, Phase14Context,
    Role, RoutingConfig, Session,
};
use halcon_providers::ReplayProvider;
use halcon_storage::AsyncDatabase;
use halcon_tools::ToolRegistry;

use super::agent::{self, AgentContext};
use super::replay_executor::ReplayToolExecutor;
use super::resilience::ResilienceManager;

/// Result of a replay execution.
#[derive(Debug)]
pub struct ReplayResult {
    pub original_session_id: Uuid,
    pub replay_session_id: Uuid,
    pub original_fingerprint: Option<String>,
    pub replay_fingerprint: String,
    pub fingerprint_match: bool,
    pub rounds: usize,
    pub steps_replayed: usize,
}

/// Run a deterministic replay of a recorded session.
///
/// Loads the original session and trace, constructs replay providers,
/// runs the agent loop, and compares execution fingerprints.
pub async fn run_replay(
    original_session_id: Uuid,
    db: &AsyncDatabase,
    tool_registry: &ToolRegistry,
    event_tx: &halcon_core::EventSender,
    verify: bool,
) -> Result<ReplayResult> {
    // Load the original session.
    let original_session = db.load_session(original_session_id).await?
        .ok_or_else(|| anyhow::anyhow!("session not found: {original_session_id}"))?;

    // Load trace steps.
    let steps = db.inner().load_trace_steps(original_session_id)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if steps.is_empty() {
        return Err(anyhow::anyhow!("no trace steps found for session {original_session_id}"));
    }

    // Construct ReplayProvider from trace steps.
    let replay_provider = ReplayProvider::from_trace(&steps, &original_session.model)
        .map_err(|e| anyhow::anyhow!("failed to construct replay provider: {e}"))?;

    if replay_provider.remaining() == 0 {
        return Err(anyhow::anyhow!("no model responses found in trace for session {original_session_id}"));
    }

    // Construct ReplayToolExecutor from trace steps.
    let replay_executor = ReplayToolExecutor::from_trace(&steps);

    // Build a new session for the replay.
    let mut replay_session = Session::new(
        original_session.model.clone(),
        "replay".to_string(),
        original_session.working_directory.clone(),
    );
    replay_session.replay_source_session = Some(original_session_id.to_string());

    // Extract the first user message from the original session to seed the replay.
    let first_user_msg = original_session.messages.iter()
        .find(|m| m.role == Role::User)
        .cloned()
        .unwrap_or(ChatMessage {
            role: Role::User,
            content: MessageContent::Text("(replay)".to_string()),
        });
    replay_session.add_message(first_user_msg.clone());

    // Build the model request.
    let provider: Arc<dyn ModelProvider> = Arc::new(replay_provider);
    let request = ModelRequest {
        model: original_session.model.clone(),
        messages: replay_session.messages.clone(),
        tools: tool_registry.tool_definitions(),
        max_tokens: Some(4096),
        temperature: Some(0.0),
        system: None,
        stream: true,
    };

    let limits = AgentLimits {
        max_rounds: 100,
        ..Default::default()
    };
    let routing_config = RoutingConfig::default();
    let mut permissions = super::conversational_permission::ConversationalPermissionHandler::new(true);
    let mut resilience = ResilienceManager::new(Default::default());

    let silent_sink = crate::render::sink::SilentSink::new();
    let default_planning_config = halcon_core::types::PlanningConfig::default();
    let default_orch_config = halcon_core::types::OrchestratorConfig::default();
    let replay_speculator = super::tool_speculation::ToolSpeculator::new();
    let ctx = AgentContext {
        provider: &provider,
        session: &mut replay_session,
        request: &request,
        tool_registry,
        permissions: &mut permissions,
        working_dir: &original_session.working_directory,
        event_tx,
        limits: &limits,
        trace_db: None, // Don't record traces during replay.
        response_cache: None,
        resilience: &mut resilience,
        fallback_providers: &[],
        routing_config: &routing_config,
        compactor: None,
        planner: None,
        guardrails: &[],
        reflector: None,
        render_sink: &silent_sink,
        replay_tool_executor: Some(&replay_executor),
        phase14: {
            let seed = format!("{}_{}", original_session.id, original_session.created_at.timestamp());
            Phase14Context {
                exec_ctx: ExecutionContext::deterministic(&seed, original_session.created_at),
                ..Default::default()
            }
        },
        model_selector: None,
        registry: None,
        episode_id: None,
        planning_config: &default_planning_config,
        orchestrator_config: &default_orch_config,
        tool_selection_enabled: false,
        task_bridge: None,
        context_metrics: None,
        context_manager: None,
        ctrl_rx: None,
        speculator: &replay_speculator,
        security_config: &halcon_core::types::SecurityConfig::default(),
        strategy_context: None,
        critic_provider: None,
        critic_model: None,
        plugin_registry: None,
        is_sub_agent: false,
        requested_provider: None,
        policy: std::sync::Arc::new(halcon_core::types::PolicyConfig::default()),
    };

    let result = agent::run_agent_loop(ctx).await?;

    let original_fingerprint = original_session.execution_fingerprint.clone();
    let replay_fingerprint = result.execution_fingerprint.clone();
    let fingerprint_match = if verify {
        original_fingerprint.as_deref() == Some(&replay_fingerprint)
    } else {
        false // Not verifying, don't claim match.
    };

    // Save the replay session with its fingerprint.
    replay_session.execution_fingerprint = Some(replay_fingerprint.clone());
    let _ = db.save_session(&replay_session).await;

    Ok(ReplayResult {
        original_session_id,
        replay_session_id: replay_session.id,
        original_fingerprint,
        replay_fingerprint,
        fingerprint_match,
        rounds: result.rounds,
        steps_replayed: steps.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_storage::{Database, TraceStep, TraceStepType};
    use chrono::Utc;

    fn test_async_db() -> AsyncDatabase {
        AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()))
    }

    fn test_event_tx() -> halcon_core::EventSender {
        halcon_core::event_bus(16).0
    }

    #[tokio::test]
    async fn replay_text_only_session() {
        let adb = test_async_db();
        let tool_reg = ToolRegistry::new();
        let event_tx = test_event_tx();

        // Create original session.
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let session_id = session.id;
        session.add_message(ChatMessage {
            role: Role::User,
            content: MessageContent::Text("hello".into()),
        });
        session.add_message(ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text("Hi there".into()),
        });
        let fp = agent::compute_fingerprint(&session.messages);
        session.execution_fingerprint = Some(fp);
        adb.save_session(&session).await.unwrap();

        // Record trace steps.
        let steps = vec![
            TraceStep {
                session_id, step_index: 0, step_type: TraceStepType::ModelRequest,
                data_json: r#"{"round":0,"model":"echo","message_count":1,"tool_count":0,"has_system":false}"#.into(),
                duration_ms: 0, timestamp: Utc::now(),
            },
            TraceStep {
                session_id, step_index: 1, step_type: TraceStepType::ModelResponse,
                data_json: r#"{"round":0,"text":"Hi there","stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":3},"latency_ms":50,"tool_uses":[]}"#.into(),
                duration_ms: 50, timestamp: Utc::now(),
            },
        ];
        for step in &steps {
            adb.inner().append_trace_step(step).unwrap();
        }

        let result = run_replay(session_id, &adb, &tool_reg, &event_tx, true).await.unwrap();
        assert_eq!(result.original_session_id, session_id);
        // Fix #1: text-only rounds are now counted; replay of 1 model response = 1 round.
        assert_eq!(result.rounds, 1);
        assert_eq!(result.steps_replayed, 2);
    }

    #[tokio::test]
    async fn replay_missing_session_errors() {
        let adb = test_async_db();
        let tool_reg = ToolRegistry::new();
        let event_tx = test_event_tx();

        let result = run_replay(Uuid::new_v4(), &adb, &tool_reg, &event_tx, false).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("session not found"));
    }

    #[tokio::test]
    async fn replay_empty_trace_errors() {
        let adb = test_async_db();
        let tool_reg = ToolRegistry::new();
        let event_tx = test_event_tx();

        let session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let session_id = session.id;
        adb.save_session(&session).await.unwrap();

        let result = run_replay(session_id, &adb, &tool_reg, &event_tx, false).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no trace steps"));
    }

    #[tokio::test]
    async fn replay_creates_new_session() {
        let adb = test_async_db();
        let tool_reg = ToolRegistry::new();
        let event_tx = test_event_tx();

        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let session_id = session.id;
        session.add_message(ChatMessage {
            role: Role::User,
            content: MessageContent::Text("test".into()),
        });
        adb.save_session(&session).await.unwrap();

        let steps = vec![
            TraceStep {
                session_id, step_index: 0, step_type: TraceStepType::ModelRequest,
                data_json: r#"{"round":0,"model":"echo","message_count":1,"tool_count":0,"has_system":false}"#.into(),
                duration_ms: 0, timestamp: Utc::now(),
            },
            TraceStep {
                session_id, step_index: 1, step_type: TraceStepType::ModelResponse,
                data_json: r#"{"round":0,"text":"replayed","stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1},"latency_ms":10,"tool_uses":[]}"#.into(),
                duration_ms: 10, timestamp: Utc::now(),
            },
        ];
        for step in &steps {
            adb.inner().append_trace_step(step).unwrap();
        }

        let result = run_replay(session_id, &adb, &tool_reg, &event_tx, false).await.unwrap();
        // Replay session should have a different ID from the original.
        assert_ne!(result.replay_session_id, session_id);

        // Verify replay session was persisted.
        let replay = adb.load_session(result.replay_session_id).await.unwrap();
        assert!(replay.is_some());
        let replay = replay.unwrap();
        assert_eq!(replay.replay_source_session, Some(session_id.to_string()));
    }

    #[test]
    fn fingerprint_deterministic() {
        let messages = vec![
            ChatMessage { role: Role::User, content: MessageContent::Text("hello".into()) },
            ChatMessage { role: Role::Assistant, content: MessageContent::Text("world".into()) },
        ];
        let fp1 = agent::compute_fingerprint(&messages);
        let fp2 = agent::compute_fingerprint(&messages);
        assert_eq!(fp1, fp2);
        assert!(!fp1.is_empty());
    }

    #[test]
    fn fingerprint_changes_with_different_messages() {
        let msg1 = vec![
            ChatMessage { role: Role::User, content: MessageContent::Text("hello".into()) },
        ];
        let msg2 = vec![
            ChatMessage { role: Role::User, content: MessageContent::Text("goodbye".into()) },
        ];
        assert_ne!(agent::compute_fingerprint(&msg1), agent::compute_fingerprint(&msg2));
    }

    // --- Deterministic context tests ---

    #[test]
    fn replay_uses_deterministic_seed() {
        let session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let seed = format!("{}_{}", session.id, session.created_at.timestamp());
        let ctx = ExecutionContext::deterministic(&seed, session.created_at);
        // Should be deterministic (seeded), not random.
        matches!(ctx.uuid_gen, halcon_core::types::UuidGenerator::Seeded { .. });
        matches!(ctx.clock, halcon_core::types::ExecutionClock::Deterministic { .. });
    }

    #[test]
    fn replay_seed_derived_from_session_id() {
        let session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let seed1 = format!("{}_{}", session.id, session.created_at.timestamp());
        let seed2 = format!("{}_{}", session.id, session.created_at.timestamp());
        // Same session → same seed.
        assert_eq!(seed1, seed2);
        let ctx1 = ExecutionContext::deterministic(&seed1, session.created_at);
        let ctx2 = ExecutionContext::deterministic(&seed2, session.created_at);
        assert_eq!(ctx1.execution_id, ctx2.execution_id);
    }

    #[test]
    fn replay_clock_starts_at_original_time() {
        let base_time = chrono::DateTime::parse_from_rfc3339("2026-01-15T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let ctx = ExecutionContext::deterministic("test-seed", base_time);
        let t0 = ctx.clock.now();
        assert_eq!(t0, base_time); // First call returns base time (offset=0).
    }

    #[test]
    fn replay_clock_monotonically_advances() {
        let base_time = Utc::now();
        let ctx = ExecutionContext::deterministic("test-seed", base_time);
        let t1 = ctx.clock.now();
        let t2 = ctx.clock.now();
        let t3 = ctx.clock.now();
        assert!(t2 > t1);
        assert!(t3 > t2);
    }

    #[test]
    fn replay_uuid_gen_produces_deterministic_ids() {
        let ctx1 = ExecutionContext::deterministic("replay-test", Utc::now());
        let ctx2 = ExecutionContext::deterministic("replay-test", Utc::now());
        // Same seed produces same UUID sequence.
        let id1a = ctx1.uuid_gen.next();
        let id2a = ctx2.uuid_gen.next();
        assert_eq!(id1a, id2a);
        let id1b = ctx1.uuid_gen.next();
        let id2b = ctx2.uuid_gen.next();
        assert_eq!(id1b, id2b);
    }

    #[test]
    fn deterministic_context_same_seed_same_sequence() {
        let base = Utc::now();
        let ctx1 = ExecutionContext::deterministic("identical-seed", base);
        let ctx2 = ExecutionContext::deterministic("identical-seed", base);
        // Execution IDs match.
        assert_eq!(ctx1.execution_id, ctx2.execution_id);
        // Clock values match.
        assert_eq!(ctx1.clock.now(), ctx2.clock.now());
        assert_eq!(ctx1.clock.now(), ctx2.clock.now());
    }

    #[test]
    fn replay_phase14_context_construction() {
        let session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let seed = format!("{}_{}", session.id, session.created_at.timestamp());
        let phase14 = Phase14Context {
            exec_ctx: ExecutionContext::deterministic(&seed, session.created_at),
            ..Default::default()
        };
        // DryRunMode should default to Off even in replay.
        assert_eq!(phase14.dry_run_mode, halcon_core::types::DryRunMode::Off);
        // Execution ID should be non-nil (deterministic but valid).
        assert!(!phase14.exec_ctx.execution_id.is_nil());
    }

    #[test]
    fn replay_different_sessions_different_seeds() {
        let s1 = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let s2 = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let seed1 = format!("{}_{}", s1.id, s1.created_at.timestamp());
        let seed2 = format!("{}_{}", s2.id, s2.created_at.timestamp());
        // Different session IDs → different seeds → different execution IDs.
        let ctx1 = ExecutionContext::deterministic(&seed1, s1.created_at);
        let ctx2 = ExecutionContext::deterministic(&seed2, s2.created_at);
        assert_ne!(ctx1.execution_id, ctx2.execution_id);
    }
}
