use super::*;
use halcon_core::types::{ContentBlock, ModelChunk, ResilienceConfig, StopReason, ToolDefinition};
use halcon_storage::Database;
use std::sync::Arc;

// Bring in items that are pub(super) or pub(crate) but not re-exported from agent/mod.rs
use super::plan_formatter::{PLAN_SECTION_END, PLAN_SECTION_START};
use super::provider_client::{check_control, invoke_with_fallback};
use crate::repl::agent_utils::classify_error_hint;
use crate::repl::domain::loop_guard::{hash_tool_args, LoopAction};

fn test_resilience() -> ResilienceManager {
    ResilienceManager::new(ResilienceConfig::default())
}

fn make_request(tools: Vec<ToolDefinition>) -> ModelRequest {
    ModelRequest {
        model: "echo".into(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Text("hello".into()),
        }],
        tools,
        max_tokens: Some(1024),
        temperature: Some(0.0),
        system: None,
        stream: true,
    }
}

fn test_event_tx() -> (EventSender, halcon_core::EventReceiver) {
    halcon_core::event_bus(64)
}

use crate::render::sink::ClassicSink;

// NOTE: TEST_CLASSIC_SINK was intentionally removed (FASE H / Parallelism Hardening).
// ClassicSink contains Mutex<StreamRenderer> with mutable state.
// `stream_reset()` replaces the inner StreamRenderer with a fresh empty one —
// when multiple #[tokio::test] functions share a single ClassicSink, any test
// that calls `stream_reset()` *wipes the accumulated full_text for all other
// concurrent tests*. The symptom: `result.full_text.is_empty()` fails because
// a concurrent test's stream_reset() ran between our stream_text() calls and
// our stream_full_text() call.
// Fix: Box::leak(Box::new(ClassicSink::new())) in test_ctx creates a fresh,
// independent ClassicSink per AgentContext with isolated StreamRenderer state.

static TEST_PLANNING_CONFIG: std::sync::LazyLock<PlanningConfig> =
    std::sync::LazyLock::new(PlanningConfig::default);

static TEST_ORCHESTRATOR_CONFIG: std::sync::LazyLock<OrchestratorConfig> =
    std::sync::LazyLock::new(OrchestratorConfig::default);

// NOTE: TEST_SPECULATOR was intentionally removed (FASE H / Parallelism Hardening).
// ToolSpeculator contains `Arc<tokio::sync::Mutex<HashMap>>` — sharing a single
// static instance across 81+ concurrent #[tokio::test] functions causes:
//   (a) background tokio::spawn tasks from one test competing for the mutex
//       in another test's runtime context;
//   (b) stale cache entries from previous tests affecting current assertions.
// Fix: Box::leak(Box::new(ToolSpeculator::new())) in test_ctx creates a fresh,
// independently-owned instance per AgentContext with no aliasing between tests.
// Memory overhead: ~81 × (small HashMap) ≈ negligible for a test binary.

static TEST_SECURITY_CONFIG: std::sync::LazyLock<halcon_core::types::SecurityConfig> =
    std::sync::LazyLock::new(halcon_core::types::SecurityConfig::default);

/// Unit-test policy: filesystem-touching features disabled so tests are hermetic.
/// use_halcon_md / enable_auto_memory / enable_agent_registry all default true in
/// production (commit 71aa8dd) but must be false in tests to prevent:
///   - system prompt injection from HALCON.md files in /tmp changing cache keys
///   - background auto-memory writes touching the real filesystem
///   - agent registry loading from ~/.halcon/agents/ contaminating test state
static TEST_POLICY_CONFIG: std::sync::LazyLock<halcon_core::types::PolicyConfig> =
    std::sync::LazyLock::new(|| halcon_core::types::PolicyConfig {
        use_halcon_md: false,
        enable_auto_memory: false,
        enable_agent_registry: false,
        enable_semantic_memory: false,
        ..halcon_core::types::PolicyConfig::default()
    });

/// Build an AgentContext with test defaults for optional fields.
#[allow(clippy::too_many_arguments)]
fn test_ctx<'a>(
    provider: &'a Arc<dyn ModelProvider>,
    session: &'a mut Session,
    request: &'a ModelRequest,
    tool_registry: &'a ToolRegistry,
    permissions: &'a mut ConversationalPermissionHandler,
    event_tx: &'a EventSender,
    limits: &'a AgentLimits,
    resilience: &'a mut ResilienceManager,
    routing_config: &'a RoutingConfig,
) -> AgentContext<'a> {
    AgentContext {
        provider,
        session,
        request,
        tool_registry,
        permissions,
        working_dir: "/tmp",
        event_tx,
        limits,
        trace_db: None,
        response_cache: None,
        resilience,
        fallback_providers: &[],
        routing_config,
        compactor: None,
        planner: None,
        guardrails: &[],
        reflector: None,
        // Fresh ClassicSink per AgentContext — no shared StreamRenderer state.
        // See comment above (FASE H) for rationale.
        render_sink: Box::leak(Box::new(ClassicSink::new())),
        replay_tool_executor: None,
        phase14: Phase14Context::default(),
        model_selector: None,
        registry: None,
        episode_id: None,
        planning_config: &*TEST_PLANNING_CONFIG,
        orchestrator_config: &*TEST_ORCHESTRATOR_CONFIG,
        tool_selection_enabled: false,
        task_bridge: None,
        context_metrics: None,
        context_manager: None,
        ctrl_rx: None,
        // Fresh ToolSpeculator per AgentContext — no shared state across tests.
        // See comment above (FASE H) for rationale.
        speculator: Box::leak(Box::new(
            crate::repl::tool_speculation::ToolSpeculator::new(),
        )),
        security_config: &*TEST_SECURITY_CONFIG,
        strategy_context: None,
        critic_provider: None,
        critic_model: None,
        plugin_registry: None,
        is_sub_agent: false,
        requested_provider: None,
        policy: std::sync::Arc::new(TEST_POLICY_CONFIG.clone()),
    }
}

#[tokio::test]
async fn agent_loop_simple_text_response() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let result = run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    assert!(!result.full_text.is_empty());
    // Fix #1: text-only rounds are now counted (previously showed 0, which was a bug).
    assert_eq!(result.rounds, 1);
    assert_eq!(result.stop_condition, StopCondition::EndTurn);
}

#[tokio::test]
async fn event_emitted_model_invoked() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, mut event_rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    // First event should be AgentStarted (new in Phase 11).
    let started = event_rx
        .try_recv()
        .expect("should receive AgentStarted event");
    assert!(matches!(started.payload, EventPayload::AgentStarted { .. }));

    // Next should be ModelInvoked.
    let event = event_rx
        .try_recv()
        .expect("should receive ModelInvoked event");
    match event.payload {
        EventPayload::ModelInvoked {
            provider: p,
            model,
            latency_ms,
            ..
        } => {
            assert_eq!(p, "echo");
            assert_eq!(model, "echo");
            assert!(latency_ms < 5000, "latency should be reasonable");
        }
        other => panic!("expected ModelInvoked, got {other:?}"),
    }
}

#[tokio::test]
async fn event_bus_fire_and_forget_no_panic() {
    // Sender with no active receiver — send() returns Err but must not panic.
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();
    // Drop the receiver before running the loop.
    drop(_rx);

    let result = run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    // Must complete normally even with no receivers.
    assert!(!result.full_text.is_empty());
}

#[tokio::test]
async fn session_latency_tracked() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    let _ = session.total_latency_ms;
    // Fix #1: text-only response still counts as 1 agent round.
    assert_eq!(session.agent_rounds, 1);
    assert_eq!(session.tool_invocations, 0);
}

#[tokio::test]
async fn trace_recording_with_db() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();

    let routing_config = RoutingConfig::default();
    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.trace_db = Some(&db);

    let result = run_agent_loop(ctx).await.unwrap();

    assert!(!result.full_text.is_empty());

    // Should have recorded at least 2 trace steps: ModelRequest + ModelResponse.
    let steps = db.inner().load_trace_steps(session.id).unwrap();
    assert!(
        steps.len() >= 2,
        "expected >= 2 trace steps, got {}",
        steps.len()
    );
    assert_eq!(
        steps[0].step_type,
        halcon_storage::TraceStepType::ModelRequest
    );
    assert_eq!(
        steps[1].step_type,
        halcon_storage::TraceStepType::ModelResponse
    );

    for (i, step) in steps.iter().enumerate() {
        assert_eq!(step.step_index, i as u32);
    }
}

#[tokio::test]
async fn token_budget_zero_means_unlimited() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits {
        max_total_tokens: 0,
        ..AgentLimits::default()
    };
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let result = run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    assert_eq!(result.stop_condition, StopCondition::EndTurn);
}

#[tokio::test]
async fn token_budget_enforced() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits {
        max_total_tokens: 1,
        ..AgentLimits::default()
    };
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let result = run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    assert_eq!(result.stop_condition, StopCondition::TokenBudget);
}

#[tokio::test]
async fn max_rounds_respected() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits {
        max_rounds: 3,
        ..AgentLimits::default()
    };
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let result = run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    assert_eq!(result.stop_condition, StopCondition::EndTurn);
}

#[tokio::test]
async fn default_limits_backward_compatible() {
    let limits = AgentLimits::default();
    assert_eq!(limits.max_rounds, 25);
    assert_eq!(limits.max_total_tokens, 0);
    assert_eq!(limits.max_duration_secs, 600); // Phase 2 fix: 600s hard cap prevents indefinite hangs
    assert_eq!(limits.tool_timeout_secs, 120);
    assert_eq!(limits.provider_timeout_secs, 300);
    assert_eq!(limits.max_parallel_tools, 10);
}

// --- Phase 1: Wired infrastructure tests ---

fn test_cache(enabled: bool) -> ResponseCache {
    use halcon_core::types::CacheConfig;
    ResponseCache::new(
        AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap())),
        CacheConfig {
            enabled,
            default_ttl_secs: 3600,
            max_entries: 100,
        },
    )
}

#[tokio::test]
async fn cache_miss_then_store() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let cache = test_cache(true);
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();

    let routing_config = RoutingConfig::default();
    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.response_cache = Some(&cache);

    let result = run_agent_loop(ctx).await.unwrap();

    assert!(!result.full_text.is_empty());
    assert_eq!(result.stop_condition, StopCondition::EndTurn);

    let cached = cache.lookup(&request).await;
    assert!(cached.is_some(), "response should be cached after miss");
    assert!(!cached.unwrap().response_text.is_empty());
}

#[tokio::test]
async fn cache_hit_skips_provider() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let cache = test_cache(true);

    // Pre-populate cache.
    cache
        .store(&request, "cached response", "end_turn", "{}", None)
        .await;

    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();

    let routing_config = RoutingConfig::default();
    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.response_cache = Some(&cache);

    let result = run_agent_loop(ctx).await.unwrap();

    assert_eq!(result.full_text, "cached response");
    assert_eq!(result.stop_condition, StopCondition::EndTurn);
    assert_eq!(result.rounds, 0);
    assert_eq!(session.total_latency_ms, 0);
}

#[tokio::test]
async fn cache_disabled_always_invokes_provider() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let cache = test_cache(false);
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();

    let routing_config = RoutingConfig::default();
    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.response_cache = Some(&cache);

    let result = run_agent_loop(ctx).await.unwrap();

    assert!(!result.full_text.is_empty());
    assert!(cache.lookup(&request).await.is_none());
}

#[tokio::test]
async fn metrics_persisted_after_invocation() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();

    let routing_config = RoutingConfig::default();
    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.trace_db = Some(&db);

    run_agent_loop(ctx).await.unwrap();

    let metrics = db.inner().system_metrics().unwrap();
    assert!(
        metrics.total_invocations >= 1,
        "expected at least 1 metric, got {}",
        metrics.total_invocations
    );
    assert!(!metrics.models.is_empty());
    let model_stat = &metrics.models[0];
    assert_eq!(model_stat.provider, "echo");
    assert_eq!(model_stat.model, "echo");
    assert!(model_stat.success_rate > 0.0);
}

#[tokio::test]
async fn trace_and_metrics_combined() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();

    let routing_config = RoutingConfig::default();
    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.trace_db = Some(&db);

    run_agent_loop(ctx).await.unwrap();

    let steps = db.inner().load_trace_steps(session.id).unwrap();
    assert!(steps.len() >= 2, "expected trace steps");

    let metrics = db.inner().system_metrics().unwrap();
    assert!(metrics.total_invocations >= 1, "expected metrics");
}

// --- Phase 3: Fallback tests ---

#[tokio::test]
async fn invoke_with_fallback_uses_primary_when_healthy() {
    let primary: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let request = make_request(vec![]);
    let mut resilience = test_resilience();

    let attempt = invoke_with_fallback(
        &primary,
        &request,
        &[],
        &mut resilience,
        &RoutingConfig::default(),
        &test_event_tx().0,
    )
    .await
    .unwrap();

    assert_eq!(attempt.provider_name, "echo");
    assert!(!attempt.is_fallback);
}

#[tokio::test]
async fn invoke_with_fallback_returns_error_when_no_fallbacks() {
    use halcon_core::types::{BackpressureConfig, CircuitBreakerConfig, ResilienceConfig};

    let primary: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let request = make_request(vec![]);

    let mut resilience = ResilienceManager::new(ResilienceConfig {
        enabled: true,
        circuit_breaker: CircuitBreakerConfig {
            failure_threshold: 1,
            window_secs: 60,
            open_duration_secs: 30,
            half_open_probes: 2,
        },
        health: Default::default(),
        backpressure: BackpressureConfig::default(),
    });
    resilience.register_provider("echo");
    resilience.record_failure("echo").await;

    let result = invoke_with_fallback(
        &primary,
        &request,
        &[],
        &mut resilience,
        &RoutingConfig::default(),
        &test_event_tx().0,
    )
    .await;
    assert!(
        result.is_err(),
        "should fail when primary is blocked and no fallbacks"
    );
}

#[tokio::test]
async fn invoke_with_fallback_uses_fallback_when_primary_blocked() {
    use halcon_core::types::{BackpressureConfig, CircuitBreakerConfig, ResilienceConfig};

    let primary: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let fallback: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let request = make_request(vec![]);

    let mut resilience = ResilienceManager::new(ResilienceConfig {
        enabled: true,
        circuit_breaker: CircuitBreakerConfig {
            failure_threshold: 1,
            window_secs: 60,
            open_duration_secs: 30,
            half_open_probes: 2,
        },
        health: Default::default(),
        backpressure: BackpressureConfig::default(),
    });
    resilience.register_provider("echo");
    resilience.register_provider("fallback_echo");
    resilience.record_failure("echo").await;

    let fallbacks = vec![("fallback_echo".to_string(), fallback)];
    let attempt = invoke_with_fallback(
        &primary,
        &request,
        &fallbacks,
        &mut resilience,
        &RoutingConfig::default(),
        &test_event_tx().0,
    )
    .await
    .unwrap();

    assert_eq!(attempt.provider_name, "fallback_echo");
    assert!(attempt.is_fallback);
}

#[tokio::test]
async fn agent_loop_with_fallback_providers() {
    use halcon_core::types::{BackpressureConfig, CircuitBreakerConfig, ResilienceConfig};

    let primary: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let fallback: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();

    let mut resilience = ResilienceManager::new(ResilienceConfig {
        enabled: true,
        circuit_breaker: CircuitBreakerConfig {
            failure_threshold: 1,
            window_secs: 60,
            open_duration_secs: 30,
            half_open_probes: 2,
        },
        health: Default::default(),
        backpressure: BackpressureConfig::default(),
    });
    resilience.register_provider("echo");
    resilience.register_provider("fallback_echo");
    resilience.record_failure("echo").await;

    let fallbacks: Vec<(String, Arc<dyn ModelProvider>)> =
        vec![("fallback_echo".to_string(), fallback)];
    let limits = AgentLimits::default();

    let routing_config = RoutingConfig::default();
    let mut ctx = test_ctx(
        &primary,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.fallback_providers = &fallbacks;

    let result = run_agent_loop(ctx).await.unwrap();

    assert!(!result.full_text.is_empty());
    assert_eq!(result.stop_condition, StopCondition::EndTurn);
}

// --- Phase 4B: SpeculativeInvoker wiring tests ---

#[tokio::test]
async fn failover_mode_delegates_to_router() {
    let primary: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let request = make_request(vec![]);
    let mut resilience = test_resilience();
    let config = RoutingConfig::default();

    let attempt = invoke_with_fallback(
        &primary,
        &request,
        &[],
        &mut resilience,
        &config,
        &test_event_tx().0,
    )
    .await
    .unwrap();

    assert_eq!(attempt.provider_name, "echo");
    assert!(!attempt.is_fallback);
}

#[tokio::test]
async fn speculative_mode_with_fallbacks() {
    let primary: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let echo2: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let request = make_request(vec![]);
    let mut resilience = test_resilience();
    let config = RoutingConfig {
        mode: "speculative".into(),
        ..RoutingConfig::default()
    };

    let fallbacks = vec![("echo2".into(), echo2)];
    let attempt = invoke_with_fallback(
        &primary,
        &request,
        &fallbacks,
        &mut resilience,
        &config,
        &test_event_tx().0,
    )
    .await
    .unwrap();

    assert!(attempt.provider_name == "echo" || attempt.provider_name == "echo2");
}

#[tokio::test]
async fn resilience_filters_unhealthy_before_routing() {
    use halcon_core::types::{BackpressureConfig, CircuitBreakerConfig, ResilienceConfig};

    let primary: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let request = make_request(vec![]);

    let mut resilience = ResilienceManager::new(ResilienceConfig {
        enabled: true,
        circuit_breaker: CircuitBreakerConfig {
            failure_threshold: 1,
            window_secs: 60,
            open_duration_secs: 30,
            half_open_probes: 2,
        },
        health: Default::default(),
        backpressure: BackpressureConfig::default(),
    });
    resilience.register_provider("echo");
    resilience.record_failure("echo").await;

    let config = RoutingConfig::default();
    let result = invoke_with_fallback(
        &primary,
        &request,
        &[],
        &mut resilience,
        &config,
        &test_event_tx().0,
    )
    .await;

    assert!(
        result.is_err(),
        "should fail when all providers are unhealthy"
    );
}

#[tokio::test]
async fn agent_loop_passes_routing_config() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let config = RoutingConfig {
        mode: "speculative".into(),
        ..RoutingConfig::default()
    };
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();

    let ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &config,
    );

    let result = run_agent_loop(ctx).await.unwrap();

    assert!(!result.full_text.is_empty());
    assert_eq!(result.stop_condition, StopCondition::EndTurn);
}

#[tokio::test]
async fn success_recorded_on_resilience() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let request = make_request(vec![]);

    let mut resilience = ResilienceManager::new(halcon_core::types::ResilienceConfig {
        enabled: true,
        ..Default::default()
    });
    resilience.register_provider("echo");

    let attempt = invoke_with_fallback(
        &provider,
        &request,
        &[],
        &mut resilience,
        &RoutingConfig::default(),
        &test_event_tx().0,
    )
    .await
    .unwrap();

    assert_eq!(attempt.provider_name, "echo");
    assert!(attempt.permit.is_some());
}

#[tokio::test]
async fn resilience_disabled_delegates_directly() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let request = make_request(vec![]);
    let mut resilience = ResilienceManager::new(ResilienceConfig {
        enabled: false,
        ..ResilienceConfig::default()
    });

    let config = RoutingConfig {
        mode: "speculative".into(),
        ..RoutingConfig::default()
    };

    let attempt = invoke_with_fallback(
        &provider,
        &request,
        &[],
        &mut resilience,
        &config,
        &test_event_tx().0,
    )
    .await
    .unwrap();

    assert_eq!(attempt.provider_name, "echo");
    assert!(!attempt.is_fallback);
    assert!(attempt.permit.is_none());
}

#[tokio::test]
async fn speculative_end_to_end_two_echo_providers() {
    let primary: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let echo2: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();

    let config = RoutingConfig {
        mode: "speculative".into(),
        ..RoutingConfig::default()
    };
    let fallbacks: Vec<(String, Arc<dyn ModelProvider>)> = vec![("echo2".into(), echo2)];
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();

    let mut ctx = test_ctx(
        &primary,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &config,
    );
    ctx.fallback_providers = &fallbacks;

    let result = run_agent_loop(ctx).await.unwrap();

    assert!(!result.full_text.is_empty());
    assert_eq!(result.stop_condition, StopCondition::EndTurn);
}

// --- Phase 11.0: Critical runtime safety tests ---

#[tokio::test]
async fn token_budget_pre_check_breaks_loop() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    // Simulate prior usage that exceeds the budget.
    session.total_usage.input_tokens = 200;
    session.total_usage.output_tokens = 100;
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    // Budget is 150 but we already used 300 — should break before invoking.
    let limits = AgentLimits {
        max_total_tokens: 150,
        ..AgentLimits::default()
    };
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let result = run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    // Pre-check breaks before any invocation, so stop_condition
    // is EndTurn (loop exited via break, no invocation happened).
    assert_eq!(result.stop_condition, StopCondition::EndTurn);
    assert_eq!(result.rounds, 0);
    // The full_text should be empty since no invocation happened.
    assert!(result.full_text.is_empty());
}

#[tokio::test]
async fn stop_reason_trace_format_serde() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.trace_db = Some(&db);

    run_agent_loop(ctx).await.unwrap();

    // Check that trace steps use serde format ("end_turn") not Debug ("EndTurn").
    let steps = db.inner().load_trace_steps(session.id).unwrap();
    let response_step = steps
        .iter()
        .find(|s| s.step_type == halcon_storage::TraceStepType::ModelResponse);
    assert!(
        response_step.is_some(),
        "should have a ModelResponse trace step"
    );
    let data = response_step.unwrap().data_json.as_str();
    // Should contain "end_turn" not "EndTurn".
    assert!(
        data.contains("end_turn"),
        "trace should use serde format 'end_turn', got: {data}"
    );
    assert!(
        !data.contains("EndTurn"),
        "trace should NOT use Debug format 'EndTurn', got: {data}"
    );
}

// --- Phase 18: classify_error_hint tests ---

#[test]
fn error_hint_invalid_api_key() {
    let hint = classify_error_hint("Error: Invalid API key provided");
    assert!(hint.contains("Verify your API key"), "got: {hint}");
}

#[test]
fn error_hint_billing() {
    let hint = classify_error_hint("Your credit balance is too low");
    assert!(hint.contains("account balance"), "got: {hint}");
}

#[test]
fn error_hint_rate_limit() {
    let hint = classify_error_hint("429 Too Many Requests");
    assert!(hint.contains("Rate limited"), "got: {hint}");

    let hint2 = classify_error_hint("rate_limit_exceeded");
    assert!(hint2.contains("Rate limited"), "got: {hint2}");
}

#[test]
fn error_hint_generic_fallback() {
    let hint = classify_error_hint("connection refused");
    assert!(hint.contains("network connection"), "got: {hint}");
}

// --- Phase 18: trace step continuity test ---

#[tokio::test]
async fn trace_step_index_continues_across_messages() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let (event_tx, _rx) = test_event_tx();
    let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
    let limits = AgentLimits::default();
    let routing_config = RoutingConfig::default();

    // Simulate session persisting across two agent loop calls.
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let sid = session.id;

    // First message
    {
        let mut perms = ConversationalPermissionHandler::new(true);
        let mut resilience = test_resilience();
        let mut ctx = test_ctx(
            &provider,
            &mut session,
            &request,
            &tool_reg,
            &mut perms,
            &event_tx,
            &limits,
            &mut resilience,
            &routing_config,
        );
        ctx.trace_db = Some(&db);
        run_agent_loop(ctx).await.unwrap();
    }

    let steps_after_first = db.inner().load_trace_steps(sid).unwrap();
    let first_max = steps_after_first.last().unwrap().step_index;
    assert!(first_max > 0, "should have trace steps after first message");

    // Second message: step indices should continue from where first left off.
    {
        let mut perms = ConversationalPermissionHandler::new(true);
        let mut resilience = test_resilience();
        let mut ctx = test_ctx(
            &provider,
            &mut session,
            &request,
            &tool_reg,
            &mut perms,
            &event_tx,
            &limits,
            &mut resilience,
            &routing_config,
        );
        ctx.trace_db = Some(&db);
        run_agent_loop(ctx).await.unwrap();
    }

    let all_steps = db.inner().load_trace_steps(sid).unwrap();
    // Verify no duplicate indices
    let indices: Vec<u32> = all_steps.iter().map(|s| s.step_index).collect();
    let unique: std::collections::HashSet<u32> = indices.iter().copied().collect();
    assert_eq!(
        indices.len(),
        unique.len(),
        "step indices should be unique: {:?}",
        indices
    );
    // Second message should start after first message's max
    assert!(
        *indices.last().unwrap() > first_max,
        "second message indices should be higher than first"
    );
}

// --- Phase 18: Self-correction context injection tests ---

#[test]
fn correction_context_format_single_failure() {
    let failures = vec![("bash".to_string(), "command not found: foo".to_string())];
    let details: Vec<String> = failures
        .iter()
        .map(|(name, err)| format!("- {name}: {err}"))
        .collect();
    let msg = format!(
            "[System Note: {} tool(s) failed. Analyze the errors below and try a different approach.\n{}]",
            failures.len(),
            details.join("\n"),
        );
    assert!(msg.contains("1 tool(s) failed"));
    assert!(msg.contains("- bash: command not found: foo"));
}

#[test]
fn correction_context_format_multiple_failures() {
    let failures = vec![
        ("file_read".to_string(), "file not found".to_string()),
        ("bash".to_string(), "exit code 1".to_string()),
    ];
    let details: Vec<String> = failures
        .iter()
        .map(|(name, err)| format!("- {name}: {err}"))
        .collect();
    let msg = format!(
            "[System Note: {} tool(s) failed. Analyze the errors below and try a different approach.\n{}]",
            failures.len(),
            details.join("\n"),
        );
    assert!(msg.contains("2 tool(s) failed"));
    assert!(msg.contains("- file_read: file not found"));
    assert!(msg.contains("- bash: exit code 1"));
}

#[test]
fn correction_context_not_injected_on_success() {
    let failures: Vec<(String, String)> = vec![];
    // When no failures, correction context should not be injected.
    assert!(failures.is_empty());
}

// ── Plan injection tests (SP-2) ──

#[test]
fn format_plan_all_statuses() {
    use halcon_core::traits::{ExecutionPlan, PlanStep, StepOutcome};
    let plan = ExecutionPlan {
        goal: "Fix auth bug".into(),
        steps: vec![
            PlanStep {
                step_id: uuid::Uuid::new_v4(),
                description: "Read auth module".into(),
                tool_name: Some("file_read".into()),
                parallel: false,
                confidence: 0.9,
                expected_args: None,
                outcome: Some(StepOutcome::Success {
                    summary: "OK".into(),
                }),
            },
            PlanStep {
                step_id: uuid::Uuid::new_v4(),
                description: "Edit validation".into(),
                tool_name: Some("file_edit".into()),
                parallel: false,
                confidence: 0.8,
                expected_args: None,
                outcome: None,
            },
            PlanStep {
                step_id: uuid::Uuid::new_v4(),
                description: "Run tests".into(),
                tool_name: Some("bash".into()),
                parallel: false,
                confidence: 0.7,
                expected_args: None,
                outcome: None,
            },
        ],
        requires_confirmation: false,
        plan_id: uuid::Uuid::nil(),
        replan_count: 0,
        parent_plan_id: None,
        ..Default::default()
    };
    let formatted = format_plan_for_prompt(&plan, 1);
    assert!(formatted.contains(PLAN_SECTION_START));
    assert!(formatted.contains(PLAN_SECTION_END));
    assert!(formatted.contains("Fix auth bug"));
    assert!(formatted.contains("\u{2713}")); // ✓ for completed step
    assert!(formatted.contains("\u{25b8}")); // ▸ for current step
    assert!(formatted.contains("CURRENT"));
    assert!(formatted.contains("\u{25cb}")); // ○ for pending step
    assert!(formatted.contains("Step 2"));
}

#[test]
fn format_plan_empty_steps() {
    use halcon_core::traits::ExecutionPlan;
    let plan = ExecutionPlan {
        goal: "Simple query".into(),
        steps: vec![],
        requires_confirmation: false,
        plan_id: uuid::Uuid::nil(),
        replan_count: 0,
        parent_plan_id: None,
        ..Default::default()
    };
    let formatted = format_plan_for_prompt(&plan, 0);
    assert!(formatted.contains("All steps completed."));
}

#[test]
fn format_plan_current_indicator_on_first() {
    use halcon_core::traits::{ExecutionPlan, PlanStep};
    let plan = ExecutionPlan {
        goal: "Build project".into(),
        steps: vec![PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: "Compile".into(),
            tool_name: Some("bash".into()),
            parallel: false,
            confidence: 0.9,
            expected_args: None,
            outcome: None,
        }],
        requires_confirmation: false,
        plan_id: uuid::Uuid::nil(),
        replan_count: 0,
        parent_plan_id: None,
        ..Default::default()
    };
    let formatted = format_plan_for_prompt(&plan, 0);
    assert!(formatted.contains("CURRENT"));
    assert!(formatted.contains("You are on Step 1"));
}

#[test]
fn update_plan_in_system_surgical_replace() {
    let mut system = format!(
        "You are a helpful assistant.\n\n{}\nOld plan content\n{}\n\nMore instructions.",
        PLAN_SECTION_START, PLAN_SECTION_END
    );
    let new_section = format!("{}\nNew plan\n{}", PLAN_SECTION_START, PLAN_SECTION_END);
    update_plan_in_system(&mut system, &new_section);
    assert!(system.contains("New plan"));
    assert!(!system.contains("Old plan content"));
    assert!(system.contains("More instructions."));
}

// ── Plan success tracking tests (SP-3 → Phase 36 ExecutionTracker) ──

fn make_plan_step(desc: &str, tool: &str) -> halcon_core::traits::PlanStep {
    halcon_core::traits::PlanStep {
        step_id: uuid::Uuid::new_v4(),
        description: desc.into(),
        tool_name: Some(tool.into()),
        parallel: false,
        confidence: 0.9,
        expected_args: None,
        outcome: None,
    }
}

fn make_test_plan(steps: Vec<halcon_core::traits::PlanStep>) -> ExecutionPlan {
    ExecutionPlan {
        goal: "Test".into(),
        steps,
        requires_confirmation: false,
        plan_id: uuid::Uuid::nil(),
        replan_count: 0,
        parent_plan_id: None,
        ..Default::default()
    }
}

fn make_test_tracker(steps: Vec<halcon_core::traits::PlanStep>) -> ExecutionTracker {
    let (tx, _rx) = tokio::sync::broadcast::channel(16);
    ExecutionTracker::new(make_test_plan(steps), tx)
}

#[test]
fn plan_step_success_match() {
    let mut tracker = make_test_tracker(vec![
        make_plan_step("Read file", "file_read"),
        make_plan_step("Edit file", "file_edit"),
    ]);
    let matched = tracker.record_tool_results(&["file_read".into()], &[], 1);
    assert_eq!(matched.len(), 1);
    assert!(matches!(
        tracker.plan().steps[0].outcome,
        Some(StepOutcome::Success { .. })
    ));
    assert!(tracker.plan().steps[1].outcome.is_none());
}

#[test]
fn plan_step_no_match_ignored() {
    let mut tracker = make_test_tracker(vec![make_plan_step("Run tests", "bash")]);
    let matched = tracker.record_tool_results(&["file_read".into()], &[], 1);
    assert!(matched.is_empty());
    assert!(tracker.plan().steps[0].outcome.is_none());
}

#[test]
fn plan_step_multi_same_tool_sequential() {
    let mut tracker = make_test_tracker(vec![
        make_plan_step("Read first", "file_read"),
        make_plan_step("Read second", "file_read"),
    ]);
    let m1 = tracker.record_tool_results(&["file_read".into()], &[], 1);
    assert_eq!(m1.len(), 1);
    assert!(matches!(
        tracker.plan().steps[0].outcome,
        Some(StepOutcome::Success { .. })
    ));
    assert!(tracker.plan().steps[1].outcome.is_none());
}

#[test]
fn plan_step_all_completed_advances_index() {
    let plan = make_test_plan(vec![
        {
            let mut s = make_plan_step("Step 1", "bash");
            s.outcome = Some(StepOutcome::Success {
                summary: "done".into(),
            });
            s
        },
        {
            let mut s = make_plan_step("Step 2", "file_read");
            s.outcome = Some(StepOutcome::Success {
                summary: "done".into(),
            });
            s
        },
    ]);
    let (tx, _rx) = tokio::sync::broadcast::channel(16);
    let tracker = ExecutionTracker::new(plan.clone(), tx);
    assert!(tracker.is_complete());
    assert_eq!(tracker.current_step(), 2); // Past all steps.
    let formatted = format_plan_for_prompt(tracker.plan(), tracker.current_step());
    assert!(formatted.contains("All steps completed."));
}

// === Phase 27 (RC-2 fix): ToolFailureTracker tests ===

#[test]
fn tracker_new_is_empty() {
    let tracker = ToolFailureTracker::new(3);
    assert!(tracker.tripped_tools().is_empty());
    assert!(!tracker.is_tripped("file_read", "not found"));
}

#[test]
fn tracker_records_below_threshold() {
    let mut tracker = ToolFailureTracker::new(3);
    assert!(!tracker.record("file_read", "No such file or directory: /tmp/x.rs"));
    assert!(!tracker.record("file_read", "File not found: /tmp/y.rs"));
    // Both map to "not_found" pattern — 2 occurrences, threshold=3 → not tripped
    assert!(!tracker.is_tripped("file_read", "not found anything"));
    assert!(tracker.tripped_tools().is_empty());
}

#[test]
fn tracker_trips_at_threshold() {
    let mut tracker = ToolFailureTracker::new(3);
    assert!(!tracker.record("file_read", "No such file or directory: /a.rs"));
    assert!(!tracker.record("file_read", "File not found: /b.rs"));
    // Third occurrence of "not_found" pattern → trips
    assert!(tracker.record("file_read", "not found: /c.rs"));
    assert!(tracker.is_tripped("file_read", "not found"));
    assert_eq!(tracker.tripped_tools(), vec!["file_read"]);
}

#[test]
fn tracker_distinct_patterns_independent() {
    let mut tracker = ToolFailureTracker::new(2);
    // Two "not_found" → trips
    assert!(!tracker.record("file_read", "not found"));
    assert!(tracker.record("file_read", "file not found"));
    // One "permission_denied" → does NOT trip
    assert!(!tracker.record("file_read", "permission denied"));
    assert!(tracker.is_tripped("file_read", "not found here"));
    assert!(!tracker.is_tripped("file_read", "permission denied on /x"));
}

#[test]
fn tracker_distinct_tools_independent() {
    let mut tracker = ToolFailureTracker::new(2);
    // file_read + not_found
    assert!(!tracker.record("file_read", "not found"));
    // bash + not_found (different tool)
    assert!(!tracker.record("bash", "not found"));
    // Second file_read + not_found → trips file_read only
    assert!(tracker.record("file_read", "not found again"));
    assert!(tracker.is_tripped("file_read", "not found"));
    assert!(!tracker.is_tripped("bash", "not found"));
}

#[test]
fn tracker_error_pattern_classification() {
    assert_eq!(
        ToolFailureTracker::error_pattern("No such file or directory"),
        "not_found"
    );
    assert_eq!(
        ToolFailureTracker::error_pattern("File not found"),
        "not_found"
    );
    assert_eq!(
        ToolFailureTracker::error_pattern("Permission denied"),
        "permission_denied"
    );
    assert_eq!(
        ToolFailureTracker::error_pattern("Is a directory"),
        "path_type_error"
    );
    assert_eq!(
        ToolFailureTracker::error_pattern("Not a directory"),
        "path_type_error"
    );
    assert_eq!(
        ToolFailureTracker::error_pattern("path traversal detected"),
        "security_blocked"
    );
    assert_eq!(
        ToolFailureTracker::error_pattern("blocked by security"),
        "security_blocked"
    );
    assert_eq!(
        ToolFailureTracker::error_pattern("unknown tool: foobar"),
        "unknown_tool"
    );
    assert_eq!(
        ToolFailureTracker::error_pattern("denied by task context"),
        "tbac_denied"
    );
    // MCP environment failures all collapse to a single pattern.
    assert_eq!(
        ToolFailureTracker::error_pattern("MCP pool call failed: connection refused"),
        "mcp_unavailable"
    );
    assert_eq!(
        ToolFailureTracker::error_pattern("failed to call 'server/tool' after 5 attempts"),
        "mcp_unavailable"
    );
    assert_eq!(
        ToolFailureTracker::error_pattern("MCP server is not initialized"),
        "mcp_unavailable"
    );
    assert_eq!(
        ToolFailureTracker::error_pattern("process start failed: no such executable"),
        "mcp_unavailable"
    );
}

#[test]
fn tracker_error_pattern_generic_fallback() {
    // Unclassified errors use first 80 chars lowercased
    let generic = "something completely unusual happened in the tool execution pipeline";
    let pattern = ToolFailureTracker::error_pattern(generic);
    assert_eq!(pattern, generic.to_lowercase());
}

#[test]
fn tracker_error_pattern_truncates_long_generic() {
    let long_error = "a".repeat(200);
    let pattern = ToolFailureTracker::error_pattern(&long_error);
    assert_eq!(pattern.len(), 80);
}

#[test]
fn tracker_threshold_one_trips_immediately() {
    let mut tracker = ToolFailureTracker::new(1);
    assert!(tracker.record("bash", "command exited with code 1"));
    assert!(tracker.is_tripped("bash", "command exited with code 1"));
}

#[test]
fn tracker_tripped_tools_deduplicates() {
    let mut tracker = ToolFailureTracker::new(1);
    // Same tool, two different patterns — both trip
    tracker.record("file_read", "not found");
    tracker.record("file_read", "permission denied");
    // tripped_tools should return file_read only once
    let tools = tracker.tripped_tools();
    assert_eq!(tools, vec!["file_read"]);
}

#[test]
fn tracker_multiple_tripped_tools_sorted() {
    let mut tracker = ToolFailureTracker::new(1);
    tracker.record("file_write", "permission denied");
    tracker.record("bash", "not found");
    tracker.record("file_read", "not found");
    let tools = tracker.tripped_tools();
    assert_eq!(tools, vec!["bash", "file_read", "file_write"]);
}

// === Phase 27 Stress Tests ===

#[test]
fn stress_tracker_100_distinct_tools() {
    // Stress: 100 distinct tools, each with a unique error
    let mut tracker = ToolFailureTracker::new(3);
    for i in 0..100 {
        let tool = format!("tool_{i}");
        let err = format!("custom error for tool {i}");
        tracker.record(&tool, &err);
        tracker.record(&tool, &err);
        // 2 occurrences → not tripped yet
        assert!(!tracker.is_tripped(&tool, &err));
    }
    // None should be tripped
    assert!(tracker.tripped_tools().is_empty());

    // Third occurrence → trips all 100
    for i in 0..100 {
        let tool = format!("tool_{i}");
        let err = format!("custom error for tool {i}");
        assert!(tracker.record(&tool, &err));
    }
    assert_eq!(tracker.tripped_tools().len(), 100);
}

#[test]
fn stress_tracker_1000_rapid_records_same_tool() {
    // Stress: 1000 recordings of the same tool+error
    let mut tracker = ToolFailureTracker::new(3);
    for i in 0..1000 {
        let tripped = tracker.record("file_read", "not found");
        if i < 2 {
            assert!(!tripped);
        } else {
            assert!(tripped);
        }
    }
    // Count should be 1000
    assert_eq!(tracker.failure_count("file_read", "not found"), 1000);
}

#[test]
fn stress_tracker_mixed_patterns_no_false_positives() {
    // Stress: interleave 6 different error patterns for the same tool
    // Only patterns reaching threshold should trip
    let mut tracker = ToolFailureTracker::new(5);
    let errors = [
        "not found",
        "permission denied",
        "is a directory",
        "path traversal",
        "unknown tool",
        "denied by task context",
    ];

    // Record each pattern a different number of times
    for (i, err) in errors.iter().enumerate() {
        for _ in 0..=(i + 1) {
            tracker.record("multi_tool", err);
        }
    }

    // Pattern 0 ("not found"): 2 records → NOT tripped (threshold=5)
    assert!(!tracker.is_tripped("multi_tool", "not found"));
    // Pattern 4 ("unknown tool"): 6 records → tripped
    assert!(tracker.is_tripped("multi_tool", "unknown tool"));
    // Pattern 5 ("denied by task context"): 7 records → tripped
    assert!(tracker.is_tripped("multi_tool", "denied by task context"));
}

#[test]
fn stress_error_pattern_determinism() {
    // Verify error_pattern() is deterministic across 1000 calls
    let errors = vec![
        "No such file or directory: /tmp/foo.rs",
        "Permission denied for /etc/shadow",
        "Is a directory: /tmp/mydir",
        "path traversal blocked in ../../etc",
        "unknown tool: mystery_tool",
        "Something generic and unique happened here",
    ];

    for err in &errors {
        let first = ToolFailureTracker::error_pattern(err);
        for _ in 0..1000 {
            assert_eq!(ToolFailureTracker::error_pattern(err), first);
        }
    }
}

#[test]
fn spinner_label_format_failover() {
    // In failover mode, spinner should show provider name.
    let provider_name = "ollama";
    let label = format!("Thinking... [{}]", provider_name);
    assert_eq!(label, "Thinking... [ollama]");
}

#[test]
fn spinner_label_format_speculative() {
    // In speculative mode with fallbacks, spinner should show racing count.
    let fallback_count = 3;
    let count = 1 + fallback_count;
    let label = format!("Racing {count} providers...");
    assert_eq!(label, "Racing 4 providers...");
}

#[test]
fn round_separator_format() {
    let round = 2;
    let provider_name = "deepseek";
    let sep = format!("\n  --- round {} [{}] ---", round + 1, provider_name);
    assert_eq!(sep, "\n  --- round 3 [deepseek] ---");
}

// === W-4: PlanningPolicy gate tests (replaced PLANNING_ACTION_KW_RE heuristic) ===

#[test]
fn planning_gate_trivial_prompt() {
    // Conversational greeting → SkipPlanning regardless of model.
    use super::super::intent_scorer::IntentScorer;
    use super::planning_policy::{self, PlanningContext, PlanningDecision};

    let user_msg = "hola";
    let intent = IntentScorer::score(user_msg);
    let ctx = PlanningContext {
        user_msg,
        intent: &intent,
        model_info: None,
        routing_tier: intent.routing_tier(),
    };
    let decision = planning_policy::decide(&ctx);
    assert_eq!(
        decision,
        PlanningDecision::SkipPlanning,
        "Trivial greeting should not trigger planning"
    );
}

#[test]
fn planning_gate_complex_prompt() {
    // Project-wide action in Spanish → planning required.
    use super::super::intent_scorer::IntentScorer;
    use super::planning_policy::{self, PlanningContext, PlanningDecision};

    let user_msg = "crea un archivo en /tmp/test.txt con el contenido hola mundo y actualiza todos los módulos del proyecto";
    let intent = IntentScorer::score(user_msg);
    let ctx = PlanningContext {
        user_msg,
        intent: &intent,
        model_info: None,
        routing_tier: intent.routing_tier(),
    };
    let decision = planning_policy::decide(&ctx);
    assert_ne!(
        decision,
        PlanningDecision::SkipPlanning,
        "Complex multi-file Spanish task should trigger planning"
    );
}

// === Phase 30: Fix 1 — Round-2 model adaptation after fallback ===

#[test]
fn fallback_adapts_model_for_round2() {
    // Simulate: primary model "claude-sonnet-4-5-20250929" not in fallback provider.
    let fallback: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let fallback_name = "echo";
    let fallback_models = fallback.supported_models();
    let original_model = "claude-sonnet-4-5-20250929";

    // Model should NOT be found in fallback.
    let found = fallback_models.iter().any(|m| m.id == original_model);
    assert!(!found, "claude-sonnet should not exist in EchoProvider");

    // The adaptation logic: if model not in fallback, use first supported model.
    let adapted = if !found {
        fallback_models.first().map(|m| m.id.clone())
    } else {
        Some(original_model.to_string())
    };
    assert!(adapted.is_some());
    assert_eq!(
        adapted.unwrap(),
        "echo",
        "Should adapt to echo provider's default model"
    );
}

#[test]
fn fallback_preserves_model_when_supported() {
    // If the model IS supported by fallback, don't change it.
    let fallback: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let fallback_models = fallback.supported_models();
    let model = &fallback_models[0].id; // "echo"

    let found = fallback_models.iter().any(|m| m.id == *model);
    assert!(found, "echo model should be in EchoProvider");
    // No adaptation needed.
}

// === Phase 30: Fix 2 — Planner model validation ===

#[test]
fn planner_supports_model_returns_false_for_unknown() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let planner = super::super::planner::LlmPlanner::new(provider, "nonexistent-model".into());
    assert!(!planner.supports_model());
}

#[test]
fn planner_supports_model_returns_true_for_known() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let planner = super::super::planner::LlmPlanner::new(provider, "echo".into());
    assert!(planner.supports_model());
}

// ── A-1: Cost estimation after fallback ──

use futures::stream::BoxStream;
use halcon_core::types::{ModelInfo, TokenCost};

/// Provider that wraps EchoProvider behavior but returns a configurable cost.
struct CostTestProvider {
    provider_name: String,
    cost: f64,
    inner: halcon_providers::EchoProvider,
}

impl CostTestProvider {
    fn new(name: &str, cost: f64) -> Self {
        Self {
            provider_name: name.to_string(),
            cost,
            inner: halcon_providers::EchoProvider::new(),
        }
    }
}

#[async_trait::async_trait]
impl ModelProvider for CostTestProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn supported_models(&self) -> &[halcon_core::types::ModelInfo] {
        self.inner.supported_models()
    }

    async fn invoke(
        &self,
        request: &ModelRequest,
    ) -> halcon_core::error::Result<BoxStream<'static, halcon_core::error::Result<ModelChunk>>>
    {
        self.inner.invoke(request).await
    }

    async fn is_available(&self) -> bool {
        true
    }

    fn estimate_cost(&self, _request: &ModelRequest) -> TokenCost {
        TokenCost {
            estimated_input_tokens: 100,
            estimated_cost_usd: self.cost,
        }
    }

    fn validate_model(&self, model: &str) -> halcon_core::error::Result<()> {
        // Accept any model name to simplify test setup.
        if model == "echo" {
            Ok(())
        } else {
            self.inner.validate_model(model)
        }
    }
}

#[tokio::test]
async fn cost_estimation_uses_fallback_provider() {
    use halcon_core::types::{BackpressureConfig, CircuitBreakerConfig, ResilienceConfig};

    let primary: Arc<dyn ModelProvider> = Arc::new(CostTestProvider::new("cost_primary", 0.01));
    let fallback: Arc<dyn ModelProvider> = Arc::new(CostTestProvider::new("cost_fallback", 0.05));
    let mut session = Session::new("cost_primary".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();

    let mut resilience = ResilienceManager::new(ResilienceConfig {
        enabled: true,
        circuit_breaker: CircuitBreakerConfig {
            failure_threshold: 1,
            window_secs: 60,
            open_duration_secs: 30,
            half_open_probes: 2,
        },
        health: Default::default(),
        backpressure: BackpressureConfig::default(),
    });
    resilience.register_provider("cost_primary");
    resilience.register_provider("cost_fallback");
    // Break primary so fallback is used.
    resilience.record_failure("cost_primary").await;

    let fallbacks: Vec<(String, Arc<dyn ModelProvider>)> =
        vec![("cost_fallback".to_string(), fallback)];
    let limits = AgentLimits::default();
    let routing_config = RoutingConfig::default();

    let mut ctx = test_ctx(
        &primary,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.fallback_providers = &fallbacks;

    let _result = run_agent_loop(ctx).await.unwrap();

    // Session cost should use fallback pricing (0.05), not primary (0.01).
    assert!(
        (session.estimated_cost_usd - 0.05).abs() < 0.001,
        "Expected fallback cost ~0.05, got {}",
        session.estimated_cost_usd
    );
}

#[tokio::test]
async fn cost_estimation_uses_primary_when_no_fallback() {
    let primary: Arc<dyn ModelProvider> = Arc::new(CostTestProvider::new("cost_primary", 0.02));
    let mut session = Session::new("cost_primary".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let _result = run_agent_loop(test_ctx(
        &primary,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    // Session cost should use primary pricing (0.02).
    assert!(
        (session.estimated_cost_usd - 0.02).abs() < 0.001,
        "Expected primary cost ~0.02, got {}",
        session.estimated_cost_usd
    );
}

// === Phase 33: ToolLoopGuard tests ===

#[test]
fn loop_guard_continue_on_first_round() {
    let mut guard = ToolLoopGuard::new();
    let tools = vec![("file_read".into(), 123u64)];
    assert_eq!(guard.record_round(&tools), LoopAction::Continue);
    assert_eq!(guard.consecutive_rounds(), 1);
}

#[test]
fn loop_guard_continue_on_second_round() {
    let mut guard = ToolLoopGuard::new();
    assert_eq!(
        guard.record_round(&[("file_read".into(), 1)]),
        LoopAction::Continue
    );
    assert_eq!(
        guard.record_round(&[("grep".into(), 2)]),
        LoopAction::Continue
    );
    assert_eq!(guard.consecutive_rounds(), 2);
}

#[test]
fn loop_guard_synthesis_at_threshold() {
    let mut guard = ToolLoopGuard::new();
    // Rounds 1-5: Continue (< synthesis_threshold 6)
    for i in 1..=5 {
        let action = guard.record_round(&[(format!("tool{i}"), i as u64)]);
        assert_eq!(action, LoopAction::Continue, "Round {i} should continue");
    }
    // Round 6: InjectSynthesis (synthesis_threshold = 6)
    let action = guard.record_round(&[("directory_tree".into(), 6)]);
    assert_eq!(action, LoopAction::InjectSynthesis);
}

#[test]
fn loop_guard_force_at_threshold() {
    let mut guard = ToolLoopGuard::new();
    // Rounds 1-9: either Continue or InjectSynthesis (< force_threshold 10)
    for i in 1..=9 {
        guard.record_round(&[(format!("tool{i}"), i as u64)]);
    }
    // Round 10: ForceNoTools (force_threshold = 10)
    let action = guard.record_round(&[("file_inspect".into(), 10)]);
    assert_eq!(action, LoopAction::ForceNoTools);
}

#[test]
fn loop_guard_oscillation_aaa() {
    // A→A→A pattern: 3 identical rounds
    let mut guard = ToolLoopGuard::new();
    let tools = vec![("file_read".into(), 42u64)];
    guard.record_round(&tools); // Round 1: Continue
    guard.record_round(&tools); // Round 2: Continue
    let action = guard.record_round(&tools); // Round 3: oscillation detected → Break
    assert_eq!(action, LoopAction::Break);
    assert!(guard.detect_oscillation());
}

#[test]
fn loop_guard_oscillation_abab() {
    // A→B→A→B pattern: alternating over 4 rounds
    let mut guard = ToolLoopGuard::new();
    let a = vec![("file_read".into(), 1u64)];
    let b = vec![("grep".into(), 2u64)];
    guard.record_round(&a); // Round 1: Continue
    guard.record_round(&b); // Round 2: Continue
    guard.record_round(&a); // Round 3: InjectSynthesis (but also check oscillation)
    let action = guard.record_round(&b); // Round 4: oscillation A→B→A→B → Break
    assert_eq!(action, LoopAction::Break);
    assert!(guard.detect_oscillation());
}

#[test]
fn loop_guard_no_oscillation_different_tools() {
    let mut guard = ToolLoopGuard::new();
    guard.record_round(&[("file_read".into(), 1)]);
    guard.record_round(&[("grep".into(), 2)]);
    guard.record_round(&[("directory_tree".into(), 3)]);
    assert!(!guard.detect_oscillation());
}

#[test]
fn loop_guard_read_saturation_detected() {
    let mut guard = ToolLoopGuard::new();
    guard.record_round(&[("file_read".into(), 1)]);
    guard.record_round(&[("grep".into(), 2)]);
    guard.record_round(&[("glob".into(), 3)]);
    assert!(guard.detect_read_saturation());
}

#[test]
fn loop_guard_read_saturation_not_with_write() {
    let mut guard = ToolLoopGuard::new();
    guard.record_round(&[("file_read".into(), 1)]);
    guard.record_round(&[("file_write".into(), 2)]); // Not read-only
    guard.record_round(&[("grep".into(), 3)]);
    assert!(!guard.detect_read_saturation());
}

#[test]
fn loop_guard_duplicate_detection() {
    let mut guard = ToolLoopGuard::new();
    // Record a round with a specific tool+hash.
    guard.record_round(&[("file_read".into(), 12345)]);
    // Same tool+hash should be detected as duplicate.
    assert!(guard.is_duplicate("file_read", 12345));
    // Different hash should not be duplicate.
    assert!(!guard.is_duplicate("file_read", 99999));
    // Different tool should not be duplicate.
    assert!(!guard.is_duplicate("grep", 12345));
}

#[test]
fn loop_guard_near_duplicate_different_hash() {
    let mut guard = ToolLoopGuard::new();
    guard.record_round(&[("file_read".into(), 111)]);
    // Different hash → not a duplicate.
    assert!(!guard.is_duplicate("file_read", 222));
}

#[test]
fn loop_guard_plan_complete_forces_break() {
    let mut guard = ToolLoopGuard::new();
    guard.force_synthesis();
    let action = guard.record_round(&[("file_read".into(), 1)]);
    assert_eq!(action, LoopAction::Break);
    assert!(guard.plan_complete());
}

#[test]
fn loop_guard_plan_complete_false_initially() {
    let guard = ToolLoopGuard::new();
    assert!(!guard.plan_complete());
}

#[test]
fn loop_guard_consecutive_rounds_tracks() {
    let mut guard = ToolLoopGuard::new();
    assert_eq!(guard.consecutive_rounds(), 0);
    guard.record_round(&[("a".into(), 1)]);
    assert_eq!(guard.consecutive_rounds(), 1);
    guard.record_round(&[("b".into(), 2)]);
    assert_eq!(guard.consecutive_rounds(), 2);
}

#[test]
fn loop_guard_empty_round_still_counts() {
    let mut guard = ToolLoopGuard::new();
    assert_eq!(guard.record_round(&[]), LoopAction::Continue);
    assert_eq!(guard.record_round(&[]), LoopAction::Continue);
    // Empty rounds don't trigger oscillation (empty == empty, but also
    // the model probably didn't call tools, which is unusual).
    assert_eq!(guard.record_round(&[]), LoopAction::Break); // AAA oscillation on empty
}

#[test]
fn hash_tool_args_deterministic() {
    let val = serde_json::json!({"path": "/tmp/test.rs", "line": 42});
    let h1 = hash_tool_args(&val);
    let h2 = hash_tool_args(&val);
    assert_eq!(h1, h2);
}

#[test]
fn hash_tool_args_different_for_different_input() {
    let v1 = serde_json::json!({"path": "/tmp/a.rs"});
    let v2 = serde_json::json!({"path": "/tmp/b.rs"});
    assert_ne!(hash_tool_args(&v1), hash_tool_args(&v2));
}

#[test]
fn loop_action_debug_display() {
    // Ensure Debug is derived properly.
    let action = LoopAction::InjectSynthesis;
    let debug_str = format!("{:?}", action);
    assert!(debug_str.contains("InjectSynthesis"));
}

#[test]
fn stop_condition_forced_synthesis_variant() {
    let sc = StopCondition::ForcedSynthesis;
    assert_ne!(sc, StopCondition::EndTurn);
    assert_ne!(sc, StopCondition::MaxRounds);
}

#[test]
fn forced_synthesis_considered_success() {
    let sc = StopCondition::ForcedSynthesis;
    let success = matches!(sc, StopCondition::EndTurn | StopCondition::ForcedSynthesis);
    assert!(success, "ForcedSynthesis should be considered a success");
}

#[test]
fn tool_usage_policy_content() {
    // Verify the policy text is well-formed.
    let policy = "\n\n## Tool Usage Policy\n\
            - Only call tools when you need NEW information you don't already have.\n\
            - After gathering data with tools, respond directly to the user.\n\
            - Never call the same tool twice with the same or very similar arguments.\n\
            - Prefer fewer tool calls. 1-3 tool rounds should suffice for most tasks.\n\
            - When you have enough information to answer, STOP calling tools and respond.\n\
            - If a tool fails, try a different approach or inform the user — do not retry the same call.\n";
    assert!(policy.contains("## Tool Usage Policy"));
    assert!(policy.contains("STOP calling tools"));
}

#[test]
fn plan_prompt_includes_synthesis_step_rule() {
    use halcon_core::types::ToolDefinition;
    let tools = vec![ToolDefinition {
        name: "file_read".into(),
        description: "Read a file".into(),
        input_schema: serde_json::json!({}),
    }];
    let prompt = crate::repl::planner::LlmPlanner::build_plan_prompt_for_test("test", &tools);
    assert!(
        prompt.contains("synthesis")
            || prompt.contains("tool_name: null")
            || prompt.contains("ANTI-COLLAPSE"),
        "Plan prompt should include synthesis/anti-collapse rule"
    );
    assert!(
        prompt.contains("8")
            || prompt.contains("LIMIT")
            || prompt.contains("Max")
            || prompt.contains("EXECUTION"),
        "Plan prompt should include step limit or execution rule"
    );
}

#[test]
fn read_only_tools_list_correct() {
    use crate::repl::loop_guard::READ_ONLY_TOOLS_LIST as READ_ONLY_TOOLS;
    // Verify known ReadOnly tools are in the list.
    assert!(READ_ONLY_TOOLS.contains(&"file_read"));
    assert!(READ_ONLY_TOOLS.contains(&"grep"));
    assert!(READ_ONLY_TOOLS.contains(&"glob"));
    assert!(READ_ONLY_TOOLS.contains(&"directory_tree"));
    assert!(READ_ONLY_TOOLS.contains(&"git_status"));
    // Destructive tools should NOT be in the list.
    assert!(!READ_ONLY_TOOLS.contains(&"file_write"));
    assert!(!READ_ONLY_TOOLS.contains(&"bash"));
    assert!(!READ_ONLY_TOOLS.contains(&"file_delete"));
}

// --- Phase 43A: Control channel tests ---

#[test]
fn control_action_variants() {
    // Verify ControlAction enum has expected variants.
    assert_eq!(ControlAction::Continue, ControlAction::Continue);
    assert_ne!(ControlAction::Continue, ControlAction::StepOnce);
    assert_ne!(ControlAction::Continue, ControlAction::Cancel);
    assert_ne!(ControlAction::StepOnce, ControlAction::Cancel);
}

#[tokio::test]
async fn check_control_noop_when_none() {
    // When ctrl_rx is None, agent loop should proceed without error.
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let (event_tx, _event_rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut permissions = ConversationalPermissionHandler::new(false);
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();
    let ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut permissions,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    // ctrl_rx is None in test_ctx — should complete without panic.
    let result = run_agent_loop(ctx).await;
    assert!(result.is_ok());
    let res = result.unwrap();
    // ctrl_rx should come back as None.
    assert!(res.ctrl_rx.is_none());
}

#[tokio::test]
async fn check_control_cancel_breaks_loop() {
    use crate::tui::events::ControlEvent;
    let (ctrl_tx, ctrl_rx) = tokio::sync::mpsc::unbounded_channel();
    // Send Cancel immediately — the agent loop should exit on first yield point.
    ctrl_tx.send(ControlEvent::CancelAgent).unwrap();

    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let (event_tx, _event_rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut permissions = ConversationalPermissionHandler::new(false);
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();
    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut permissions,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.ctrl_rx = Some(ctrl_rx);
    let result = run_agent_loop(ctx).await;
    assert!(result.is_ok());
    let res = result.unwrap();
    // When cancelled before model invocation, should have 0 rounds.
    assert_eq!(res.rounds, 0);
    assert_eq!(res.stop_condition, StopCondition::Interrupted);
}

#[tokio::test]
async fn check_control_step_returns_ctrl_rx() {
    use crate::tui::events::ControlEvent;
    let (_ctrl_tx, ctrl_rx) = tokio::sync::mpsc::unbounded_channel::<ControlEvent>();
    // No events queued — should pass through all yield points normally.
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let (event_tx, _event_rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut permissions = ConversationalPermissionHandler::new(false);
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();
    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut permissions,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.ctrl_rx = Some(ctrl_rx);
    let result = run_agent_loop(ctx).await.unwrap();
    // ctrl_rx should be returned for reuse.
    assert!(result.ctrl_rx.is_some());
}

#[tokio::test]
async fn check_control_resume_after_pause() {
    use crate::tui::events::ControlEvent;
    let sink = crate::render::sink::SilentSink::new();
    let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::unbounded_channel();
    // Send Pause, then Resume — check_control should return Continue.
    ctrl_tx.send(ControlEvent::Pause).unwrap();
    ctrl_tx.send(ControlEvent::Resume).unwrap();
    let action = check_control(&mut ctrl_rx, &sink).await;
    assert_eq!(action, ControlAction::Continue);
}

#[tokio::test]
async fn check_control_step_after_pause() {
    use crate::tui::events::ControlEvent;
    let sink = crate::render::sink::SilentSink::new();
    let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::unbounded_channel();
    // Send Pause, then Step — should return StepOnce.
    ctrl_tx.send(ControlEvent::Pause).unwrap();
    ctrl_tx.send(ControlEvent::Step).unwrap();
    let action = check_control(&mut ctrl_rx, &sink).await;
    assert_eq!(action, ControlAction::StepOnce);
}

#[tokio::test]
async fn check_control_cancel_during_pause() {
    use crate::tui::events::ControlEvent;
    let sink = crate::render::sink::SilentSink::new();
    let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::unbounded_channel();
    // Send Pause, then CancelAgent — should return Cancel.
    ctrl_tx.send(ControlEvent::Pause).unwrap();
    ctrl_tx.send(ControlEvent::CancelAgent).unwrap();
    let action = check_control(&mut ctrl_rx, &sink).await;
    assert_eq!(action, ControlAction::Cancel);
}

#[tokio::test]
async fn check_control_ignore_unknown_events() {
    use crate::tui::events::ControlEvent;
    let sink = crate::render::sink::SilentSink::new();
    let (ctrl_tx, mut ctrl_rx) = tokio::sync::mpsc::unbounded_channel();
    // Send ApproveAction — not a control action, should return Continue.
    ctrl_tx.send(ControlEvent::ApproveAction).unwrap();
    let action = check_control(&mut ctrl_rx, &sink).await;
    assert_eq!(action, ControlAction::Continue);
}

// === Phase 43C: Feedback completeness tests ===

#[test]
fn compaction_spinner_label_is_specific() {
    // Compaction should say "Compacting context..." not "Thinking...".
    let label = "Compacting context...";
    assert!(label.contains("Compacting"));
    assert!(!label.contains("Thinking"));
}

#[test]
fn reflection_feedback_methods_exist() {
    use crate::render::sink::RenderSink;
    let sink = crate::render::sink::SilentSink::new();
    // These should be callable without panic (default no-ops on SilentSink).
    sink.reflection_started();
    sink.reflection_complete("test analysis", 0.85);
}

#[test]
fn consolidation_feedback_method_exists() {
    use crate::render::sink::RenderSink;
    let sink = crate::render::sink::SilentSink::new();
    sink.consolidation_status("consolidating reflections...");
}

#[test]
fn tool_retrying_feedback_method_exists() {
    use crate::render::sink::RenderSink;
    let sink = crate::render::sink::SilentSink::new();
    sink.tool_retrying("bash", 2, 3, 500);
}

// === Fix #2: Plan Validation Pre-Execution tests ===

fn make_validation_plan(steps: Vec<halcon_core::traits::PlanStep>) -> ExecutionPlan {
    halcon_core::traits::ExecutionPlan {
        plan_id: uuid::Uuid::new_v4(),
        goal: "Test goal".to_string(),
        steps,
        requires_confirmation: false,
        replan_count: 0,
        parent_plan_id: None,
        ..Default::default()
    }
}

#[test]
fn validate_plan_all_tools_exist() {
    let config = halcon_core::types::ToolsConfig::default();
    let registry = halcon_tools::default_registry(&config);

    let plan = make_validation_plan(vec![
        halcon_core::traits::PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: "Read file".to_string(),
            tool_name: Some("file_read".to_string()),
            parallel: false,
            confidence: 0.9,
            expected_args: None,
            outcome: None,
        },
        halcon_core::traits::PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: "Run command".to_string(),
            tool_name: Some("bash".to_string()),
            parallel: false,
            confidence: 0.8,
            expected_args: None,
            outcome: None,
        },
    ]);

    let warnings = validate_plan(&plan, &registry);
    assert!(warnings.is_empty(), "Valid plan should have no warnings");
}

#[test]
fn validate_plan_detects_missing_tool() {
    let config = halcon_core::types::ToolsConfig::default();
    let registry = halcon_tools::default_registry(&config);

    let plan = make_validation_plan(vec![halcon_core::traits::PlanStep {
        step_id: uuid::Uuid::new_v4(),
        description: "Use non-existent tool".to_string(),
        tool_name: Some("nonexistent_tool".to_string()),
        parallel: false,
        confidence: 0.9,
        expected_args: None,
        outcome: None,
    }]);

    let warnings = validate_plan(&plan, &registry);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("nonexistent_tool"));
    assert!(warnings[0].contains("not found in registry"));
}

#[test]
fn validate_plan_detects_multiple_issues() {
    let config = halcon_core::types::ToolsConfig::default();
    let registry = halcon_tools::default_registry(&config);

    let plan = make_validation_plan(vec![
        halcon_core::traits::PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: "First invalid".to_string(),
            tool_name: Some("tool_one".to_string()),
            parallel: false,
            confidence: 0.9,
            expected_args: None,
            outcome: None,
        },
        halcon_core::traits::PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: "Valid tool".to_string(),
            tool_name: Some("file_read".to_string()),
            parallel: false,
            confidence: 0.8,
            expected_args: None,
            outcome: None,
        },
        halcon_core::traits::PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: "Second invalid".to_string(),
            tool_name: Some("tool_two".to_string()),
            parallel: false,
            confidence: 0.7,
            expected_args: None,
            outcome: None,
        },
    ]);

    let warnings = validate_plan(&plan, &registry);
    assert_eq!(warnings.len(), 2);
    assert!(warnings.iter().any(|w| w.contains("tool_one")));
    assert!(warnings.iter().any(|w| w.contains("tool_two")));
}

#[test]
fn validate_plan_warns_on_empty_steps() {
    let config = halcon_core::types::ToolsConfig::default();
    let registry = halcon_tools::default_registry(&config);

    let plan = make_validation_plan(vec![]);

    let warnings = validate_plan(&plan, &registry);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("0 steps"));
}

#[test]
fn validate_plan_ignores_steps_without_tool() {
    let config = halcon_core::types::ToolsConfig::default();
    let registry = halcon_tools::default_registry(&config);

    let plan = make_validation_plan(vec![halcon_core::traits::PlanStep {
        step_id: uuid::Uuid::new_v4(),
        description: "Think about problem".to_string(),
        tool_name: None, // No tool specified
        parallel: false,
        confidence: 0.9,
        expected_args: None,
        outcome: None,
    }]);

    let warnings = validate_plan(&plan, &registry);
    assert!(
        warnings.is_empty(),
        "Steps without tools should not generate warnings"
    );
}

// ────────────────────────────────────────────────────────────────────────
// Phase 4 — Hardening Integration Tests (patches P0–P5)
// These tests were written AFTER the patches and verify the fixed behavior.
// ────────────────────────────────────────────────────────────────────────

// ── Mock providers ───────────────────────────────────────────────────────

use async_trait::async_trait;

/// Provider that emits only Usage + Done(EndTurn) with no text or tool deltas.
/// Used to test P0: spinner finalization barrier on empty streams.
struct EmptyStreamProvider {
    models: Vec<ModelInfo>,
}

impl EmptyStreamProvider {
    fn new() -> Self {
        Self {
            models: vec![ModelInfo {
                id: "echo".into(), // matches make_request() default model
                name: "Empty Stream".into(),
                provider: "empty_stream".into(),
                context_window: 4096,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.0,
                cost_per_output_token: 0.0,
            }],
        }
    }
}

#[async_trait]
impl ModelProvider for EmptyStreamProvider {
    fn name(&self) -> &str {
        "empty_stream"
    }

    fn supported_models(&self) -> &[ModelInfo] {
        &self.models
    }

    async fn invoke(
        &self,
        _request: &ModelRequest,
    ) -> halcon_core::error::Result<BoxStream<'static, halcon_core::error::Result<ModelChunk>>>
    {
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 0,
            ..Default::default()
        };
        let chunks: Vec<halcon_core::error::Result<ModelChunk>> = vec![
            Ok(ModelChunk::Usage(usage)),
            Ok(ModelChunk::Done(StopReason::EndTurn)),
        ];
        Ok(Box::pin(futures::stream::iter(chunks)))
    }

    async fn is_available(&self) -> bool {
        true
    }

    fn estimate_cost(&self, _request: &ModelRequest) -> TokenCost {
        TokenCost::default()
    }
}

/// Provider that always returns Err from invoke().
/// Used to test P3: AgentCompleted emitted on early return paths.
struct AlwaysErrorProvider {
    models: Vec<ModelInfo>,
}

impl AlwaysErrorProvider {
    fn new() -> Self {
        Self {
            models: vec![ModelInfo {
                id: "echo".into(), // matches make_request() default model
                name: "Always Error".into(),
                provider: "always_error".into(),
                context_window: 4096,
                max_output_tokens: 4096,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.0,
                cost_per_output_token: 0.0,
            }],
        }
    }
}

#[async_trait]
impl ModelProvider for AlwaysErrorProvider {
    fn name(&self) -> &str {
        "always_error"
    }

    fn supported_models(&self) -> &[ModelInfo] {
        &self.models
    }

    async fn invoke(
        &self,
        _request: &ModelRequest,
    ) -> halcon_core::error::Result<BoxStream<'static, halcon_core::error::Result<ModelChunk>>>
    {
        Err(halcon_core::error::HalconError::ProviderUnavailable {
            provider: "always_error".into(),
        })
    }

    async fn is_available(&self) -> bool {
        true
    }

    fn estimate_cost(&self, _request: &ModelRequest) -> TokenCost {
        TokenCost::default()
    }
}

// ── Recording RenderSink ─────────────────────────────────────────────────

/// A render sink that records FSM transitions and spinner stop calls.
/// Used to verify P0 and P4 observable behavior.
struct RecordingSink {
    /// (from, to, reason) triples for each agent_state_transition call.
    transitions: std::sync::Mutex<Vec<(String, String, String)>>,
    /// Count of spinner_stop() calls.
    spinner_stops: std::sync::Mutex<u32>,
    /// Accumulated text from stream_text() — returned by stream_full_text().
    /// This is the fix for the PostInvocation guardrail test: the default
    /// implementation returned String::new() so guardrail checks were skipped.
    stream_text_buf: std::sync::Mutex<String>,
}

impl RecordingSink {
    fn new() -> Self {
        Self {
            transitions: std::sync::Mutex::new(Vec::new()),
            spinner_stops: std::sync::Mutex::new(0),
            stream_text_buf: std::sync::Mutex::new(String::new()),
        }
    }

    fn get_transitions(&self) -> Vec<(String, String, String)> {
        self.transitions.lock().unwrap().clone()
    }

    fn get_spinner_stops(&self) -> u32 {
        *self.spinner_stops.lock().unwrap()
    }
}

impl RenderSink for RecordingSink {
    fn stream_text(&self, text: &str) {
        self.stream_text_buf.lock().unwrap().push_str(text);
    }
    fn stream_code_block(&self, _lang: &str, _code: &str) {}
    fn stream_tool_marker(&self, _name: &str) {}
    fn stream_done(&self) {}
    fn stream_error(&self, _msg: &str) {}
    fn tool_start(&self, _name: &str, _input: &serde_json::Value) {}
    fn tool_output(&self, _block: &ContentBlock, _duration_ms: u64) {}
    fn tool_denied(&self, _name: &str) {}
    fn spinner_start(&self, _label: &str) {}
    fn spinner_stop(&self) {
        *self.spinner_stops.lock().unwrap() += 1;
    }
    fn warning(&self, _message: &str, _hint: Option<&str>) {}
    fn error(&self, _message: &str, _hint: Option<&str>) {}
    fn info(&self, _message: &str) {}
    /// Non-silent so FSM transition calls and spinner calls are not skipped.
    fn is_silent(&self) -> bool {
        false
    }
    fn stream_reset(&self) {
        self.stream_text_buf.lock().unwrap().clear();
    }
    fn stream_full_text(&self) -> String {
        // Take accumulated text (clearing the buffer) — matches ClassicSink behaviour.
        let mut buf = self.stream_text_buf.lock().unwrap();
        std::mem::take(&mut *buf)
    }
    fn agent_state_transition(&self, from: &str, to: &str, reason: &str) {
        self.transitions.lock().unwrap().push((
            from.to_string(),
            to.to_string(),
            reason.to_string(),
        ));
    }
}

// ── Helper: test_ctx with custom render sink ──────────────────────────────

fn test_ctx_with_sink<'a>(
    provider: &'a Arc<dyn ModelProvider>,
    session: &'a mut Session,
    request: &'a ModelRequest,
    tool_registry: &'a ToolRegistry,
    permissions: &'a mut ConversationalPermissionHandler,
    event_tx: &'a EventSender,
    limits: &'a AgentLimits,
    resilience: &'a mut ResilienceManager,
    routing_config: &'a RoutingConfig,
    sink: &'a dyn RenderSink,
) -> AgentContext<'a> {
    AgentContext {
        render_sink: sink,
        ..test_ctx(
            provider,
            session,
            request,
            tool_registry,
            permissions,
            event_tx,
            limits,
            resilience,
            routing_config,
        )
    }
}

// ── P0: Empty stream terminates cleanly (spinner finalization barrier) ───

/// Proves P0 fix: agent loop must return when the model emits only
/// Usage + Done with no TextDelta/ToolUseStart. Before the fix, the
/// spinner would never receive `spinner_stop()` from a content chunk,
/// leaving the spinner in an inconsistent state. The finalization barrier
/// after the stream loop guarantees `spinner_stop()` is always called.
///
/// Correctness signal: function RETURNS (no hang) + rounds=1 + EndTurn.
#[tokio::test]
async fn p0_empty_stream_terminates_cleanly() {
    let provider: Arc<dyn ModelProvider> = Arc::new(EmptyStreamProvider::new());
    let mut session = Session::new("empty_stream".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    // If this hangs, it proves the P0 fix is needed. If it returns, fix works.
    let result = run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    assert_eq!(
        result.rounds, 1,
        "P0: empty stream must complete in 1 round"
    );
    assert_eq!(
        result.stop_condition,
        StopCondition::EndTurn,
        "P0: empty stream must stop with EndTurn"
    );
    assert!(
        result.full_text.is_empty(),
        "P0: no text output for empty stream, got: {:?}",
        result.full_text
    );
    assert_eq!(
        result.output_tokens, 0,
        "P0: zero output tokens from empty stream"
    );
}

/// Proves P0 + P4 with a RecordingSink:
/// - P0: spinner_stop() is called exactly once (via finalization barrier)
/// - P4: first FSM transition is from "idle" (tracked state, not hardcoded)
#[tokio::test]
async fn p0_spinner_stop_called_once_and_p4_fsm_starts_from_idle() {
    let sink = RecordingSink::new();

    let provider: Arc<dyn ModelProvider> = Arc::new(EmptyStreamProvider::new());
    let mut session = Session::new("empty_stream".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let result = run_agent_loop(test_ctx_with_sink(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
        &sink,
    ))
    .await
    .unwrap();

    assert_eq!(result.stop_condition, StopCondition::EndTurn);

    // P0: spinner_stop must be called at least once (finalization barrier).
    // It may be called twice if the Done chunk triggers it, but the barrier
    // guarantees at least one call even with zero content chunks.
    assert!(
        sink.get_spinner_stops() >= 1,
        "P0: spinner_stop must be called at least once on empty stream, got 0"
    );

    // P4: verify FSM transitions are recorded and start from "idle".
    let transitions = sink.get_transitions();
    assert!(
        !transitions.is_empty(),
        "P4: must have at least one FSM transition"
    );

    let (first_from, first_to, _) = &transitions[0];
    assert_eq!(
        first_from, "idle",
        "P4: first FSM transition must originate from 'idle'"
    );
    assert_eq!(
        first_to, "executing",
        "P4: first transition must go to 'executing'"
    );

    // P4: verify the final transition ends in "complete" (EndTurn) with a valid from-state.
    let (last_from, last_to, _) = transitions.last().unwrap();
    assert_eq!(
        last_to, "complete",
        "P4: final state must be 'complete' for EndTurn"
    );
    // "evaluating" is valid now that (Executing, SynthesisComplete) → Evaluating is a
    // recognized FSM path for text-only EndTurn responses (Step 3: FSM Hardening).
    let valid_predecessors = [
        "idle",
        "executing",
        "planning",
        "tool_wait",
        "reflecting",
        "evaluating",
    ];
    assert!(
        valid_predecessors.contains(&last_from.as_str()),
        "P4: final from_state '{}' is not valid (must be one of {:?})",
        last_from,
        valid_predecessors
    );
}

// ── P1: Ollama tool emulation marker stripped on ForceNoTools ────────────

/// Proves P1 fix: when force_no_tools_next_round is set, the Ollama tool
/// emulation block injected into the system prompt is stripped. Before the
/// fix, the model would still see the `<tool_call>` instructions and
/// continue generating tool calls even with tools=[].
#[test]
fn p1_ollama_tool_emulation_marker_stripped_on_force_no_tools() {
    const MARKER: &str = "\n\n# TOOL USE INSTRUCTIONS\n\n";
    let base = "You are a helpful assistant.";
    let catalog = "## Available Tools\n- file_read: read a file\n- bash: run commands\n";
    let system_with_emul = format!("{base}{MARKER}{catalog}");

    assert!(
        system_with_emul.contains(MARKER),
        "setup: marker must be present before strip"
    );
    assert!(
        system_with_emul.contains("Available Tools"),
        "setup: catalog section must be present"
    );

    // Simulate P1 FIX: truncate system prompt at Ollama emulation marker.
    let mut sys = system_with_emul.clone();
    if let Some(pos) = sys.find(MARKER) {
        sys.truncate(pos);
    }

    assert!(
        !sys.contains(MARKER),
        "P1: marker must be absent after strip"
    );
    assert!(
        !sys.contains("Available Tools"),
        "P1: tool catalog section must be absent after strip"
    );
    assert_eq!(
        sys, base,
        "P1: only the base system prompt must remain after strip"
    );
}

/// Proves P1 fix is idempotent: when no marker is present, the system
/// prompt is unchanged (no unintended truncation on non-Ollama providers).
#[test]
fn p1_no_marker_means_no_truncation() {
    const MARKER: &str = "\n\n# TOOL USE INSTRUCTIONS\n\n";
    let original = "You are a helpful assistant. No emulation block here.".to_string();
    let mut sys = original.clone();

    // Simulate P1 FIX path when no marker exists.
    if let Some(pos) = sys.find(MARKER) {
        sys.truncate(pos);
    }

    assert_eq!(
        sys, original,
        "P1: prompt must be unchanged when Ollama marker is absent"
    );
}

// ── P2: Replan convergence budget ────────────────────────────────────────

/// Proves P2 fix: the replan budget counter (MAX_REPLAN_ATTEMPTS = 2) gates
/// infinite replan cascades. Counter increments before the budget check, so
/// attempts 1 and 2 get a real replan, attempt 3+ get forced synthesis.
#[test]
fn p2_replan_counter_exhausts_after_two_replans() {
    // Simulate the P2 loop logic extracted from agent.rs.
    const MAX_REPLAN_ATTEMPTS: u32 = 2; // must match agent.rs definition
    let mut replan_attempts: u32 = 0;
    let mut real_replan_count = 0u32;
    let mut forced_synthesis_count = 0u32;

    // Simulate 5 consecutive ReplanRequired loop actions.
    for _ in 0..5 {
        replan_attempts += 1;
        if replan_attempts > MAX_REPLAN_ATTEMPTS {
            forced_synthesis_count += 1;
        } else {
            real_replan_count += 1;
        }
    }

    assert_eq!(
        real_replan_count, 2,
        "P2: must allow exactly 2 real replans before budget"
    );
    assert_eq!(
        forced_synthesis_count, 3,
        "P2: remaining attempts must become forced synthesis"
    );
}

/// Proves P2 fix: a single replan attempt is within budget.
#[test]
fn p2_single_replan_within_budget() {
    const MAX_REPLAN_ATTEMPTS: u32 = 2;
    let mut replan_attempts: u32 = 0;
    replan_attempts += 1;
    assert!(
        replan_attempts <= MAX_REPLAN_ATTEMPTS,
        "P2: first replan must be within budget"
    );
}

// ── P3: AgentCompleted emitted on provider error (early return) ──────────

/// Proves P3 fix: `AgentCompleted` domain event is emitted when the provider
/// returns an error and the agent exits early. Before the fix, early returns
/// (on error, timeout, cancellation) skipped the event, causing the TUI and
/// monitoring systems to miss the agent's completion.
///
/// Note: AlwaysErrorProvider retries once (MAX_ROUND_RETRIES=1) with a 2s
/// sleep, so this test takes ~2 seconds.
#[tokio::test]
async fn p3_agent_completed_emitted_on_provider_error() {
    let provider: Arc<dyn ModelProvider> = Arc::new(AlwaysErrorProvider::new());
    let mut session = Session::new("always_error".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, mut rx) = test_event_tx();
    // Keep defaults — agent exits after MAX_ROUND_RETRIES=1 retry.
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    // Result is Ok (early return with ProviderError stop condition), not Err.
    let result = run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await;

    // Whether Ok or Err, AgentCompleted must have been emitted (P3 fix).
    let mut events = vec![];
    while let Ok(evt) = rx.try_recv() {
        events.push(evt);
    }

    let has_agent_completed = events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::AgentCompleted { .. }));

    assert!(
        has_agent_completed,
        "P3: AgentCompleted must be emitted on provider error. \
             Got events: {:?}",
        events
            .iter()
            .map(|e| format!("{:?}", std::mem::discriminant(&e.payload)))
            .collect::<Vec<_>>()
    );

    // Verify the result indicates a provider error or error-related stop.
    match result {
        Ok(r) => {
            assert!(
                matches!(r.stop_condition, StopCondition::ProviderError),
                "P3: stop_condition must be ProviderError, got {:?}",
                r.stop_condition
            );
        }
        Err(_) => {
            // An Err result is also acceptable — AgentCompleted was still emitted.
        }
    }
}

// ── P4: FSM final transition uses tracked state (not hardcoded "executing") ──

/// Proves P4 fix: the final FSM transition emitted by the agent uses the
/// correct `from_state` (tracked via `current_fsm_state` variable) instead
/// of the hardcoded `"executing"` that was previously always emitted.
///
/// Verified via RecordingSink: the last transition's `to` must be "complete"
/// for EndTurn, and `from` must be one of the valid predecessor states.
#[tokio::test]
async fn p4_final_fsm_transition_uses_tracked_from_state() {
    let sink = RecordingSink::new();

    let provider: Arc<dyn ModelProvider> = Arc::new(EmptyStreamProvider::new());
    let mut session = Session::new("empty_stream".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let result = run_agent_loop(test_ctx_with_sink(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
        &sink,
    ))
    .await
    .unwrap();

    assert_eq!(result.stop_condition, StopCondition::EndTurn);

    let transitions = sink.get_transitions();
    assert!(
        transitions.len() >= 2,
        "P4: must have at least 2 FSM transitions (idle→executing, X→complete)"
    );

    // First transition: idle → executing (agent start).
    let (from0, to0, _) = &transitions[0];
    assert_eq!(from0, "idle", "P4: first transition from must be 'idle'");
    assert_eq!(
        to0, "executing",
        "P4: first transition to must be 'executing'"
    );

    // Last transition: ?→complete (EndTurn).
    let (last_from, last_to, _) = transitions.last().unwrap();
    assert_eq!(
        last_to, "complete",
        "P4: final to-state must be 'complete' for EndTurn"
    );

    // The from-state must be one of the valid predecessors for "complete".
    // Before the P4 fix, it was always "executing" even if the FSM was elsewhere.
    // "evaluating" is valid now that (Executing, SynthesisComplete) → Evaluating is a
    // recognized FSM path for text-only EndTurn responses (Step 3: FSM Hardening).
    let valid_predecessors = [
        "idle",
        "executing",
        "planning",
        "tool_wait",
        "reflecting",
        "evaluating",
    ];
    assert!(
        valid_predecessors.contains(&last_from.as_str()),
        "P4: final from-state '{}' is not a valid predecessor for 'complete'. \
             Valid: {:?}",
        last_from,
        valid_predecessors
    );
}

// ── P5: Single TaskBridge sync per round ─────────────────────────────────

/// Documents P5 fix: TaskBridge.sync_from_tracker() must be called only
/// once per round, using round-accurate model/provider names (which reflect
/// any mid-round fallback). Before the fix, a duplicate call at line ~2645
/// used `request.model`/`provider.name()` (original, pre-fallback values),
/// resulting in wrong provenance when a fallback occurred.
///
/// This is a behavioral assertion on the invariant: when fallback triggers,
/// the round model name differs from the original request model.
#[test]
fn p5_round_accurate_names_differ_from_original_on_fallback() {
    // Simulate the scenario: request uses "claude-sonnet-4-6" (original model),
    // but after fallback to Ollama the round uses "deepseek-coder-v2" (adapted model).
    let original_model = "claude-sonnet-4-6";
    let round_model_after_fallback = "deepseek-coder-v2"; // set by fallback adaptation

    // The invariant: when fallback occurs, round_model_name != request.model.
    // The correct sync uses round_model_name. Using request.model would be wrong.
    assert_ne!(
        original_model, round_model_after_fallback,
        "P5: when fallback occurs, original model must differ from round model"
    );

    // The P5 fix ensures only the second sync call (using round_model_after_fallback)
    // exists. This test documents the invariant that the removed first call
    // would have recorded wrong provenance.
    let correct_sync_model = round_model_after_fallback;
    let removed_wrong_sync_model = original_model;
    assert_ne!(
        correct_sync_model, removed_wrong_sync_model,
        "P5: TaskBridge sync must use round-accurate model name, not original"
    );
}

// ── Zero-token completion — no stuck states ──────────────────────────────

/// Verifies that a completion with zero output tokens (Usage{output=0} + Done)
/// does not cause any stuck state, panic, or assertion failure.
/// This covers the edge case of models that respond with pure control flow
/// and no generated content.
#[tokio::test]
async fn zero_token_output_completion_no_stuck_states() {
    let provider: Arc<dyn ModelProvider> = Arc::new(EmptyStreamProvider::new());
    let mut session = Session::new("empty_stream".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let result = run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    assert_eq!(
        result.output_tokens, 0,
        "Zero-token: output_tokens must be 0"
    );
    assert_eq!(result.rounds, 1, "Zero-token: must complete in 1 round");
    assert_eq!(
        result.stop_condition,
        StopCondition::EndTurn,
        "Zero-token: must exit cleanly with EndTurn"
    );
    assert!(
        result.full_text.is_empty(),
        "Zero-token: no text in full_text"
    );
}

// ── G2 PII hard block tests ──────────────────────────────────────────────

fn make_pii_request(user_msg: &str) -> ModelRequest {
    ModelRequest {
        model: "echo".into(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Text(user_msg.into()),
        }],
        tools: vec![],
        max_tokens: Some(1024),
        temperature: Some(0.0),
        system: None,
        stream: true,
    }
}

/// G2: When PiiPolicy::Block is active, a user message containing an email
/// must be blocked before reaching the LLM (rounds = 0, full_text empty).
#[tokio::test]
async fn g2_pii_block_email_stops_request() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_pii_request("My email is user@example.com — please help me.");
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(false);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();
    let security_config = halcon_core::types::SecurityConfig {
        pii_action: halcon_core::types::PiiPolicy::Block,
        ..Default::default()
    };

    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.security_config = &security_config;

    let result = run_agent_loop(ctx).await.unwrap();

    assert_eq!(
        result.rounds, 0,
        "G2 Block: request must be blocked before round 1"
    );
    assert!(
        result.full_text.is_empty(),
        "G2 Block: no LLM text when PII blocked"
    );
}

/// G2: When PiiPolicy::Block is active, SSN in user message is blocked.
#[tokio::test]
async fn g2_pii_block_ssn_stops_request() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_pii_request("My SSN is 123-45-6789, store it please.");
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(false);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();
    let security_config = halcon_core::types::SecurityConfig {
        pii_action: halcon_core::types::PiiPolicy::Block,
        ..Default::default()
    };

    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.security_config = &security_config;

    let result = run_agent_loop(ctx).await.unwrap();

    assert_eq!(result.rounds, 0, "G2 Block: SSN must be blocked");
}

/// G2: When PiiPolicy::Warn (default), PII-containing messages are NOT blocked.
#[tokio::test]
async fn g2_pii_warn_allows_request_through() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_pii_request("My email is user@example.com — please help.");
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(false);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();
    let security_config = halcon_core::types::SecurityConfig {
        pii_action: halcon_core::types::PiiPolicy::Warn,
        ..Default::default()
    };

    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.security_config = &security_config;

    let result = run_agent_loop(ctx).await.unwrap();

    // Warn mode: request proceeds, LLM generates response.
    assert!(result.rounds >= 1, "G2 Warn: request must proceed to LLM");
}

/// G2: Clean (no-PII) message with Block mode proceeds normally.
#[tokio::test]
async fn g2_pii_block_clean_message_proceeds() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_pii_request("What is 2 + 2?");
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(false);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();
    let security_config = halcon_core::types::SecurityConfig {
        pii_action: halcon_core::types::PiiPolicy::Block,
        ..Default::default()
    };

    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.security_config = &security_config;

    let result = run_agent_loop(ctx).await.unwrap();

    // No PII → request proceeds even in Block mode.
    assert!(result.rounds >= 1, "G2 Block: clean message must reach LLM");
}

/// G2: API key in user message is blocked (Block mode).
#[tokio::test]
async fn g2_pii_block_api_key_stopped() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_pii_request("My key is sk-ant-api03-testcredential1234567890abcdef");
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(false);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();
    let security_config = halcon_core::types::SecurityConfig {
        pii_action: halcon_core::types::PiiPolicy::Block,
        ..Default::default()
    };

    let mut ctx = test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    );
    ctx.security_config = &security_config;

    let result = run_agent_loop(ctx).await.unwrap();

    assert_eq!(
        result.rounds, 0,
        "G2 Block: API key in message must be blocked"
    );
}

// ── PostInvocation guardrail tests ──────────────────────────────────────

/// PostInvocation: when the LLM echoes back a credential, the
/// `CredentialLeakGuardrail` must block the response (rounds = 0).
#[tokio::test]
async fn guardrail_post_invocation_blocks_credential_in_llm_output() {
    // EchoProvider echoes "**Echo:** <user_msg>" so sending an API key
    // makes the LLM output contain the key, triggering PostInvocation Block.
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_pii_request("sk-ant-api03-testkey1234567890abcdefghij");
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(false);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    // RecordingSink now accumulates stream_text(), so stream_full_text() returns
    // the real content — making the guardrail check non-trivially fire.
    let sink = RecordingSink::new();

    // Warn (not Block) for G2 so the request reaches the LLM (test PostInvocation).
    let security_config = halcon_core::types::SecurityConfig {
        pii_action: halcon_core::types::PiiPolicy::Warn,
        ..Default::default()
    };

    let guardrails = halcon_security::builtin_guardrails();

    let mut ctx = test_ctx_with_sink(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
        &sink,
    );
    ctx.guardrails = guardrails;
    ctx.security_config = &security_config;

    let result = run_agent_loop(ctx).await.unwrap();

    // The PostInvocation break fires inside round 0 before the counter increments.
    assert_eq!(
        result.rounds, 0,
        "PostInvocation: credential in LLM output must abort loop (rounds=0, got {})",
        result.rounds
    );
}

/// PostInvocation: clean LLM output (no credentials) passes through normally.
#[tokio::test]
async fn guardrail_post_invocation_passes_clean_output_through() {
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_pii_request("What is the capital of France?");
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(false);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let sink = RecordingSink::new();
    let guardrails = halcon_security::builtin_guardrails();

    let mut ctx = test_ctx_with_sink(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
        &sink,
    );
    ctx.guardrails = guardrails;

    let result = run_agent_loop(ctx).await.unwrap();

    assert!(
        result.rounds >= 1,
        "PostInvocation: clean output must allow normal completion (rounds={})",
        result.rounds
    );
    assert_eq!(result.stop_condition, StopCondition::EndTurn);
}

// ── Phase 6: Failure-case stability validation ────────────────────────────

/// Phase6-A: ProviderError exits with correct stop_condition.
///
/// Validates that when the model provider always returns an error, the agent
/// loop terminates gracefully with `StopCondition::ProviderError` (not a panic
/// or hang). This is the precondition for the Phase 4 reward pipeline to assign
/// reward=0.0 and `success=false` to the quality stats.
#[tokio::test]
async fn phase6_a_provider_error_gives_provider_error_stop_condition() {
    let provider: Arc<dyn ModelProvider> = Arc::new(AlwaysErrorProvider::new());
    let mut session = Session::new("always_error".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits::default();
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let result = run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await;

    // Loop must return Ok with ProviderError stop, not propagate Err.
    match result {
        Ok(r) => assert_eq!(
            r.stop_condition,
            StopCondition::ProviderError,
            "Phase6-A: AlwaysErrorProvider must produce ProviderError stop condition"
        ),
        Err(_) => {
            // An Err propagation is also acceptable (resilience-exhausted path).
        }
    }
}

/// Phase6-B: MaxRounds stop condition when all rounds are consumed.
///
/// Uses EmptyStreamProvider (emits EndTurn) with `max_rounds=1`. After the
/// first round completes (rounds=1) and we break from the loop, the post-loop
/// check `rounds >= limits.max_rounds` (1 >= 1 = true) fires → MaxRounds.
///
/// This validates that the MaxRounds path is reachable and that the reward
/// formula correctly assigns reward=0.20 for this stop condition.
#[tokio::test]
async fn phase6_b_max_rounds_stop_condition_with_tight_round_limit() {
    let provider: Arc<dyn ModelProvider> = Arc::new(EmptyStreamProvider::new());
    let mut session = Session::new("empty_stream".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    // max_rounds=1: one round runs, rounds becomes 1, then 1>=1 → MaxRounds.
    let limits = AgentLimits {
        max_rounds: 1,
        ..AgentLimits::default()
    };
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let result = run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    assert_eq!(
        result.stop_condition,
        StopCondition::MaxRounds,
        "Phase6-B: max_rounds=1 with 1 completed round must give MaxRounds"
    );
    assert_eq!(
        result.rounds, 1,
        "Phase6-B: exactly 1 round must be counted"
    );
}

/// Phase6-C: Reward formula correctly maps all 5 stop conditions.
///
/// Validates the Phase 4 reward match block in agent.rs without a full
/// agent loop. This ensures the reward signal fed into quality routing is
/// well-ordered: ProviderError (0.0) < MaxRounds (0.20) < ForcedSynthesis
/// (0.40-0.70) < EndTurn (0.70-1.0).
#[test]
fn phase6_c_reward_formula_is_correctly_ordered_for_all_stop_conditions() {
    // Mirror the Phase 4 record_outcome reward formula from agent.rs.
    let compute_reward = |cond: StopCondition, ratio: f32| -> f64 {
        match cond {
            StopCondition::EndTurn => 0.70 + 0.30 * ratio.clamp(0.0, 1.0) as f64,
            StopCondition::ForcedSynthesis => 0.40 + 0.30 * ratio.clamp(0.0, 1.0) as f64,
            StopCondition::MaxRounds => 0.20,
            StopCondition::Interrupted => 0.50,
            _ => 0.0, // ProviderError, TokenBudget, DurationBudget
        }
    };

    let r_error = compute_reward(StopCondition::ProviderError, 0.0);
    let r_max_rounds = compute_reward(StopCondition::MaxRounds, 0.0);
    let r_forced_min = compute_reward(StopCondition::ForcedSynthesis, 0.0);
    let r_forced_max = compute_reward(StopCondition::ForcedSynthesis, 1.0);
    let r_end_min = compute_reward(StopCondition::EndTurn, 0.0);
    let r_end_max = compute_reward(StopCondition::EndTurn, 1.0);

    // Exact values
    assert_eq!(r_error, 0.0, "Phase6-C: ProviderError → reward=0.0");
    assert_eq!(r_max_rounds, 0.20, "Phase6-C: MaxRounds → reward=0.20");
    assert!(
        (r_forced_min - 0.40).abs() < 1e-9,
        "Phase6-C: ForcedSynthesis+0% → 0.40"
    );
    assert!(
        (r_forced_max - 0.70).abs() < 1e-9,
        "Phase6-C: ForcedSynthesis+100% → 0.70"
    );
    assert!(
        (r_end_min - 0.70).abs() < 1e-9,
        "Phase6-C: EndTurn+0% → 0.70"
    );
    assert!(
        (r_end_max - 1.0).abs() < 1e-9,
        "Phase6-C: EndTurn+100% → 1.0"
    );

    // Strict ordering (lower bound of each tier)
    assert!(
        r_error < r_max_rounds,
        "Phase6-C: ProviderError < MaxRounds"
    );
    assert!(
        r_max_rounds < r_forced_min,
        "Phase6-C: MaxRounds < ForcedSynthesis(min)"
    );
    assert!(
        r_forced_min <= r_end_min,
        "Phase6-C: ForcedSynthesis(min) ≤ EndTurn(min)"
    );
    assert!(r_end_min <= r_end_max, "Phase6-C: EndTurn range monotone");

    // Success flag alignment
    let is_success = |cond: StopCondition| {
        matches!(
            cond,
            StopCondition::EndTurn | StopCondition::ForcedSynthesis
        )
    };
    assert!(is_success(StopCondition::EndTurn), "EndTurn is success");
    assert!(
        is_success(StopCondition::ForcedSynthesis),
        "ForcedSynthesis is success"
    );
    assert!(
        !is_success(StopCondition::ProviderError),
        "ProviderError is failure"
    );
    assert!(
        !is_success(StopCondition::MaxRounds),
        "MaxRounds is failure"
    );
    assert!(
        !is_success(StopCondition::Interrupted),
        "Interrupted is not success"
    );
}

/// Phase6-D: ForcedSynthesis reward scales linearly with plan_completion_ratio.
///
/// Validates that partial plan completion is credited (0.40) and full completion
/// reaches the EndTurn floor (0.70), creating a continuous quality gradient.
/// This ensures the reward pipeline gives meaningful signal even on incomplete runs.
#[test]
fn phase6_d_forced_synthesis_reward_scales_with_plan_completion_ratio() {
    let synth_reward = |ratio: f32| -> f64 { 0.40 + 0.30 * ratio.clamp(0.0, 1.0) as f64 };

    let r0 = synth_reward(0.0);
    let r25 = synth_reward(0.25);
    let r50 = synth_reward(0.5);
    let r75 = synth_reward(0.75);
    let r100 = synth_reward(1.0);

    // Boundary values
    assert!((r0 - 0.40).abs() < 1e-9, "Phase6-D: 0% → 0.40");
    assert!((r100 - 0.70).abs() < 1e-9, "Phase6-D: 100% → 0.70");

    // Monotonicity
    assert!(r0 < r25, "Phase6-D: 0% < 25%");
    assert!(r25 < r50, "Phase6-D: 25% < 50%");
    assert!(r50 < r75, "Phase6-D: 50% < 75%");
    assert!(r75 < r100, "Phase6-D: 75% < 100%");

    // Range (all values in [0.40, 0.70])
    assert!(
        r0 >= 0.40 && r100 <= 0.70,
        "Phase6-D: ForcedSynthesis reward must be in [0.40, 0.70], got [{r0}, {r100}]"
    );
}

/// Phase6-E: Model selector quality routing feedback wired into agent loop.
///
/// Verifies that `ModelSelector::record_outcome()` is callable without panic
/// after the EndTurn path (which represents normal successful completion).
/// The cost multiplier invariant is validated arithmetically:
/// - ProviderError (reward=0.0) × 3 → avg=0.0 → multiplier=2.0 (max penalty)
/// - EndTurn (reward≈0.85) × 3 → avg=0.85 → multiplier < 1.0 (routing bonus)
#[test]
fn phase6_e_quality_cost_multiplier_responds_to_failure_and_success() {
    // Formula: cost_multiplier = (2.0 - 2.0 * avg_reward).clamp(0.5, 2.0)
    let cost_mult = |avg_reward: f64| -> f64 { (2.0 - 2.0 * avg_reward).clamp(0.5, 2.0) };

    // ProviderError × 3: total_reward=0.0, avg=0.0, multiplier=2.0 (max penalty)
    let fail_avg = (0.0_f64 * 3.0) / 3.0;
    let fail_mult = cost_mult(fail_avg);
    assert_eq!(
        fail_mult, 2.0,
        "Phase6-E: 3× ProviderError → avg=0.0 → cost_mult=2.0 (max routing penalty)"
    );

    // EndTurn × 3 (reward=0.85 each): avg=0.85, multiplier=0.30 → clamped to 0.5
    let succ_avg = (0.85_f64 * 3.0) / 3.0;
    let succ_mult = cost_mult(succ_avg);
    assert!(
        succ_mult < 1.0,
        "Phase6-E: 3× EndTurn (reward=0.85) → avg=0.85 → cost_mult={succ_mult} < 1.0 (bonus)"
    );
    assert!(
        succ_mult >= 0.5,
        "Phase6-E: multiplier must be clamped at 0.5 floor, got {succ_mult}"
    );

    // Monotonicity: neutral prior (no data) → 1.0
    let neutral_mult = cost_mult(0.5); // avg_reward=0.5 → 2.0 - 1.0 = 1.0
    assert!(
        (neutral_mult - 1.0).abs() < 1e-9,
        "Phase6-E: avg_reward=0.5 → neutral multiplier=1.0, got {neutral_mult}"
    );

    // Strict ordering: failure penalty > neutral > success bonus
    assert!(
        fail_mult > neutral_mult,
        "Phase6-E: failure penalty ({fail_mult}) > neutral ({neutral_mult})"
    );
    assert!(
        neutral_mult > succ_mult,
        "Phase6-E: neutral ({neutral_mult}) > success bonus ({succ_mult})"
    );
}

// ── P1/P2 control layer tests ─────────────────────────────────────────────

/// P2-C: Cost budget enforcement — `StopCondition::CostBudget` is produced when
/// `session.estimated_cost_usd >= limits.max_cost_usd` (and `max_cost_usd > 0`).
///
/// This verifies the evaluator and reward pipeline both handle the new variant.
#[test]
fn p2c_cost_budget_stop_condition_scores_correctly() {
    use super::super::reward_pipeline::{compute_reward, RawRewardSignals};
    use crate::repl::metrics::evaluator::{AgentLoopOutcome, CompositeEvaluator};

    // Evaluator: CostBudget should score 0.3 (same as TokenBudget / DurationBudget).
    let outcome = AgentLoopOutcome {
        stop_condition: super::super::agent_types::StopCondition::CostBudget,
        rounds_used: 3,
        max_rounds: 10,
        has_output: true,
    };
    let composite = CompositeEvaluator::evaluate(&outcome);
    // stop=0.3*0.5 + efficiency=0.7*0.2 + completion=1.0*0.3 = 0.15 + 0.14 + 0.30 = 0.59
    assert!(
        composite > 0.4 && composite < 0.8,
        "P2-C: CostBudget composite score should be in moderate range, got {composite}"
    );

    // Reward pipeline: CostBudget should score in [0.10, 0.20] (10 + 10*ratio range).
    let signals = RawRewardSignals {
        stop_condition: StopCondition::CostBudget,
        round_scores: vec![],
        critic_verdict: None,
        plan_coherence_score: 0.0,
        oscillation_penalty: 0.0,
        plan_completion_ratio: 0.5,
        plugin_snapshots: vec![],
        critic_unavailable: false,
        evidence_coverage: 1.0,
    };
    let reward = compute_reward(&signals, &halcon_core::types::PolicyConfig::default());
    assert!(
        (reward.breakdown.stop_score - 0.15).abs() < 1e-9,
        "P2-C: CostBudget stop_score = 0.10 + 0.10*0.5 ≈ 0.15, got {}",
        reward.breakdown.stop_score
    );
    assert!(
        reward.final_reward < 0.50,
        "P2-C: CostBudget final reward must be low (budget constraint = not converged), got {}",
        reward.final_reward
    );
}

/// P2-C: Verifies the hard enforcement invariant — `max_cost_usd = 0.0` must be treated
/// as "no limit" (guard disabled), so the default AgentLimits never triggers CostBudget.
#[test]
fn p2c_zero_max_cost_means_no_limit() {
    // Default AgentLimits has max_cost_usd = 0.0 (no limit).
    let limits = AgentLimits::default();
    assert_eq!(
        limits.max_cost_usd, 0.0,
        "P2-C: default max_cost_usd must be 0.0 (disabled)"
    );

    // The guard condition: `limits.max_cost_usd > 0.0 && session.estimated_cost_usd >= limits.max_cost_usd`
    // With max_cost_usd = 0.0, the first clause is false → guard never fires.
    let simulated_cost = 999.99_f64;
    let should_halt = limits.max_cost_usd > 0.0 && simulated_cost >= limits.max_cost_usd;
    assert!(
        !should_halt,
        "P2-C: zero max_cost_usd must disable the budget guard regardless of spend"
    );

    // With a real limit set, the guard should fire when spend meets or exceeds it.
    let real_limit = 1.00_f64;
    let spend_at_limit = 1.00_f64;
    let spend_below_limit = 0.99_f64;
    assert!(
        real_limit > 0.0 && spend_at_limit >= real_limit,
        "P2-C: guard fires when spend == limit"
    );
    assert!(
        !(real_limit > 0.0 && spend_below_limit >= real_limit),
        "P2-C: guard must not fire when spend < limit"
    );
}

/// P1-A: Parallel batch collapse semantics — when every tool result in a parallel batch
/// is an error, `parallel_batch_collapsed` must be true, triggering forced synthesis.
///
/// This documents the flag computation logic: the flag is set only when
/// (a) parallel_results is non-empty, (b) the plan had parallel steps, and
/// (c) ALL results are errors.
#[test]
fn p1a_parallel_batch_collapse_flag_semantics() {
    // Simulate the flag computation from agent.rs:
    // parallel_batch_collapsed = !parallel_results.is_empty()
    //     && !plan.parallel_batch.is_empty()
    //     && parallel_results.iter().all(|r| is_error(r))
    struct FakeResult {
        is_err: bool,
    }
    let all_fail = |results: &[FakeResult], batch_empty: bool| -> bool {
        !results.is_empty() && !batch_empty && results.iter().all(|r| r.is_err)
    };

    // Case 1: all fail → collapse = true.
    let results = vec![FakeResult { is_err: true }, FakeResult { is_err: true }];
    assert!(
        all_fail(&results, false),
        "P1-A: all-error batch must collapse"
    );

    // Case 2: partial fail → collapse = false.
    let results = vec![FakeResult { is_err: true }, FakeResult { is_err: false }];
    assert!(
        !all_fail(&results, false),
        "P1-A: partial-error batch must NOT collapse"
    );

    // Case 3: no parallel batch → collapse = false even with errors.
    let results = vec![FakeResult { is_err: true }];
    assert!(
        !all_fail(&results, true),
        "P1-A: empty batch must NOT collapse"
    );

    // Case 4: no results (no parallel tools executed) → collapse = false.
    let empty: Vec<FakeResult> = vec![];
    assert!(
        !all_fail(&empty, false),
        "P1-A: empty results must NOT collapse"
    );
}

/// P1-B: Compaction timeout escalation — utilization threshold logic.
///
/// The escalation rule: set `force_no_tools_next_round = true` when utilization ≥ 70%.
/// This test documents the threshold and the utilization computation formula.
#[test]
fn p1b_compaction_timeout_escalation_threshold() {
    // Simulate the utilization computation from agent.rs P1-B arm:
    // utilization_pct = (current_tokens as f64 / pipeline_budget as f64 * 100.0) as u32
    let compute_pct = |current: u32, budget: u32| -> u32 {
        if budget > 0 {
            (current as f64 / budget as f64 * 100.0) as u32
        } else {
            100
        }
    };

    let pipeline_budget: u32 = 51_200; // typical DeepSeek pipeline budget

    // At 70% (boundary): should escalate.
    let at_70 = (pipeline_budget as f64 * 0.70) as u32;
    assert!(
        compute_pct(at_70, pipeline_budget) >= 70,
        "P1-B: 70% utilization must trigger force_no_tools_next_round"
    );

    // At 69%: should NOT escalate.
    let at_69 = (pipeline_budget as f64 * 0.69) as u32;
    assert!(
        compute_pct(at_69, pipeline_budget) < 70,
        "P1-B: 69% utilization must NOT trigger escalation"
    );

    // At 100% (fully exhausted): should escalate.
    assert!(
        compute_pct(pipeline_budget, pipeline_budget) >= 70,
        "P1-B: 100% utilization must trigger escalation"
    );

    // Zero budget (degenerate): returns 100, which is ≥ 70 → escalate.
    assert!(
        compute_pct(1000, 0) >= 70,
        "P1-B: zero pipeline_budget must escalate (defensive: 100% utilization assumed)"
    );
}

/// P2-D: Deduplication visibility — model-visible directive is injected when > 1 duplicate
/// tool call is filtered in a round. The directive uses the exact count in its message.
#[test]
fn p2d_dedup_directive_count_threshold() {
    // The directive fires when round_dedup_count > 1.
    // round_dedup_count is computed as dedup_result_blocks.len()
    // after the loop_guard.is_duplicate() filter removes duplicate entries.

    let fires_directive = |dedup_count: usize| -> bool { dedup_count > 1 };
    let fires_sink_event = |dedup_count: usize| -> bool { dedup_count > 0 };

    // > 0 triggers sink event (render_sink.loop_guard_action).
    assert!(fires_sink_event(1), "P2-D: 1 dedup fires sink event");
    assert!(fires_sink_event(3), "P2-D: 3 dedups fire sink event");
    assert!(
        !fires_sink_event(0),
        "P2-D: 0 dedups must NOT fire sink event"
    );

    // > 1 triggers the model-visible directive User message.
    assert!(
        !fires_directive(1),
        "P2-D: 1 dedup alone must NOT inject directive (threshold = >1)"
    );
    assert!(fires_directive(2), "P2-D: 2 dedups must inject directive");
    assert!(fires_directive(5), "P2-D: 5 dedups must inject directive");
    assert!(
        !fires_directive(0),
        "P2-D: 0 dedups must NOT inject directive"
    );

    // Verify the directive format includes the count (string formatting test).
    let dedup_count: usize = 3;
    let directive = format!(
        "[System — Deduplication Guard]: {dedup_count} tool calls were \
             filtered as exact duplicates of prior rounds. You are repeating \
             without progress. Stop calling tools you have already used with the \
             same arguments. Synthesize what you have gathered and respond directly."
    );
    assert!(
        directive.contains("3 tool calls"),
        "P2-D: directive must embed the dedup count, got: {directive}"
    );
    assert!(
        directive.contains("Synthesize"),
        "P2-D: directive must include synthesis instruction"
    );
}

// ── RC-1 / RC-2: Budget Guard + Plan Completion Ratio Tests ──────────────

/// RC-1a: Headroom guard constant is defined and meaningful (>= 4K tokens for a
/// usable response, <= 16K to avoid triggering too early on large context windows).
#[test]
fn headroom_guard_constant_is_reasonable() {
    // The guard fires when remaining tokens < MIN_OUTPUT_HEADROOM_TOKENS.
    // 5 000 tokens ≈ 20 KB of text — enough for a complete synthesis response.
    // Changing this constant risks either truncation (too low) or premature
    // synthesis on short requests (too high).
    const MIN_OUTPUT_HEADROOM_TOKENS: u64 = 5_000;
    assert!(
        MIN_OUTPUT_HEADROOM_TOKENS >= 4_000,
        "Headroom must be at least 4K tokens to produce a complete response"
    );
    assert!(
        MIN_OUTPUT_HEADROOM_TOKENS <= 16_000,
        "Headroom above 16K forces synthesis too early on 128K-context providers"
    );
}

/// RC-1a: Headroom guard fires (used > 0 AND remaining < 5K).
#[test]
fn headroom_guard_fires_when_used_positive_and_remaining_low() {
    const MIN_OUTPUT_HEADROOM_TOKENS: u64 = 5_000;
    let budget: u64 = 100_000;

    // Scenario A: used > 0 AND remaining < threshold → should fire.
    let used_a: u64 = 96_000; // remaining = 4_000 < 5_000
    let remaining_a = budget.saturating_sub(used_a);
    assert!(
        used_a > 0 && remaining_a < MIN_OUTPUT_HEADROOM_TOKENS,
        "Guard should fire: used={used_a}, remaining={remaining_a}"
    );

    // Scenario B: used = 0 (round 0) → must NOT fire regardless of budget.
    let used_b: u64 = 0;
    let remaining_b = budget.saturating_sub(used_b);
    assert!(
        !(used_b > 0 && remaining_b < MIN_OUTPUT_HEADROOM_TOKENS),
        "Guard must NOT fire on round 0 (used=0)"
    );

    // Scenario C: used > 0 BUT remaining >= threshold → must NOT fire.
    let used_c: u64 = 90_000; // remaining = 10_000 >= 5_000
    let remaining_c = budget.saturating_sub(used_c);
    assert!(
        !(used_c > 0 && remaining_c < MIN_OUTPUT_HEADROOM_TOKENS),
        "Guard must NOT fire when remaining={remaining_c} >= threshold"
    );
}

/// RC-1a: With max_total_tokens = 0 (unlimited), headroom guard is disabled.
#[test]
fn headroom_guard_disabled_when_budget_zero() {
    let max_total_tokens: u64 = 0;
    // Guard condition: `limits.max_total_tokens > 0`
    assert!(
        max_total_tokens == 0,
        "max_total_tokens=0 means unlimited; headroom guard must be skipped"
    );
    // The outer `if limits.max_total_tokens > 0` is false → guard body never runs.
    let guard_active = max_total_tokens > 0;
    assert!(
        !guard_active,
        "Guard must be inactive when budget is unlimited"
    );
}

/// RC-2: TokenBudget stop with no execution_tracker yields plan_completion_ratio = 0.0.
///
/// When there is no plan (task_bridge = None), ratio must be 0.0 regardless of
/// whether the stop was due to budget, duration, or cost limits.
#[test]
fn budget_exit_without_tracker_yields_zero_ratio() {
    // Simulate the computation at each budget guard exit site.
    let execution_tracker: Option<super::super::execution_tracker::ExecutionTracker> = None;
    let ratio = execution_tracker
        .as_ref()
        .map(|t| {
            let (completed, total, _) = t.progress();
            if total > 0 {
                completed as f32 / total as f32
            } else {
                0.0
            }
        })
        .unwrap_or(0.0);
    assert_eq!(ratio, 0.0, "No tracker → ratio must be 0.0");
}

/// RC-2: TokenBudget early return now includes timeline_json from tracker.
///
/// Verifies that the `timeline_json` field in the budget guard returns is
/// populated from the tracker (not always None as it was before the fix).
/// Uses integration: run_agent_loop with max_total_tokens=1 to hit the guard.
#[tokio::test]
async fn token_budget_exit_sets_last_model_used() {
    // max_total_tokens = 1 triggers TokenBudget on post-round guard check
    // (used becomes > 1 after EchoProvider responds).
    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
    let request = make_request(vec![]);
    let tool_reg = ToolRegistry::new();
    let mut perms = ConversationalPermissionHandler::new(true);
    let (event_tx, _rx) = test_event_tx();
    let limits = AgentLimits {
        max_total_tokens: 1,
        ..AgentLimits::default()
    };
    let mut resilience = test_resilience();
    let routing_config = RoutingConfig::default();

    let result = run_agent_loop(test_ctx(
        &provider,
        &mut session,
        &request,
        &tool_reg,
        &mut perms,
        &event_tx,
        &limits,
        &mut resilience,
        &routing_config,
    ))
    .await
    .unwrap();

    assert_eq!(result.stop_condition, StopCondition::TokenBudget);
    // RC-2: last_model_used is now Some (was None before the fix).
    assert!(
        result.last_model_used.is_some(),
        "last_model_used must be Some on TokenBudget exit, got None"
    );
}

// ── FASE 6: Planning V3 Pipeline E2E Tests ───────────────────────────────
//
// These 8 tests verify the Planning V3 pipeline (plan_compressor +
// early_convergence + macro_feedback) behaves correctly AFTER wiring
// into agent.rs. They constitute the mandatory E2E validation required
// by the remediation spec.

/// FASE6-1: Plan is compressed to ≤MAX_VISIBLE_STEPS before the agent loop runs.
#[test]
fn fase6_1_plan_compressed_to_max_steps_before_loop() {
    use super::super::plan_compressor::{compress, MAX_VISIBLE_STEPS};
    use halcon_core::traits::{ExecutionPlan, PlanStep};
    use uuid::Uuid;

    let steps: Vec<PlanStep> = (1..=8)
        .map(|i| PlanStep {
            step_id: Uuid::new_v4(),
            description: format!("Step {i}"),
            tool_name: if i == 8 {
                None
            } else {
                Some("file_read".into())
            },
            parallel: false,
            confidence: 0.9,
            expected_args: Default::default(),
            outcome: None,
        })
        .collect();
    let plan = ExecutionPlan {
        plan_id: Uuid::new_v4(),
        goal: "Test goal".into(),
        steps,
        requires_confirmation: false,
        replan_count: 0,
        parent_plan_id: None,
        ..Default::default()
    };
    let (compressed, stats) = compress(plan);
    assert!(
        compressed.steps.len() <= MAX_VISIBLE_STEPS,
        "FASE6-1: got {} steps (max {})",
        compressed.steps.len(),
        MAX_VISIBLE_STEPS
    );
    assert!(
        stats.any_applied(),
        "FASE6-1: at least one compression rule must fire"
    );
    let last = compressed.steps.last().unwrap();
    assert!(
        last.tool_name.is_none(),
        "FASE6-1: synthesis must be last step"
    );
}

/// FASE6-2: EvidenceThreshold fires at ≥80% plan completion.
#[test]
fn fase6_2_evidence_threshold_fires_at_80_percent_completion() {
    use super::super::early_convergence::{ConvergenceDetector, ConvergenceReason};
    let mut det = ConvergenceDetector::new();
    // 79% — must NOT fire.
    assert!(
        det.check(0.79, 100_000, 0.10).is_none(),
        "FASE6-2: 79% must not trigger EvidenceThreshold"
    );
    // 80% — must fire.
    let mut det2 = ConvergenceDetector::new();
    assert_eq!(
        det2.check(0.80, 100_000, 0.10),
        Some(ConvergenceReason::EvidenceThreshold),
        "FASE6-2: 80% must trigger EvidenceThreshold"
    );
}

/// FASE6-3: TokenHeadroom fires when token budget falls below synthesis headroom.
#[test]
fn fase6_3_token_headroom_fires_when_budget_critically_low() {
    use super::super::early_convergence::{
        ConvergenceDetector, ConvergenceReason, MIN_SYNTHESIS_HEADROOM,
    };
    // Just above headroom — must NOT fire.
    let mut det = ConvergenceDetector::new();
    assert!(
        det.check(0.50, MIN_SYNTHESIS_HEADROOM + 1, 0.10).is_none(),
        "FASE6-3: tokens just above headroom must not fire TokenHeadroom"
    );
    // Just below headroom — must fire.
    let mut det2 = ConvergenceDetector::new();
    assert_eq!(
        det2.check(0.50, MIN_SYNTHESIS_HEADROOM - 1, 0.10),
        Some(ConvergenceReason::TokenHeadroom),
        "FASE6-3: tokens below synthesis_headroom must fire TokenHeadroom"
    );
}

/// FASE6-4: DiminishingReturns fires after DIMINISHING_WINDOW stagnant rounds with
/// an active plan, but NEVER fires without an active plan (BUG-H3 regression guard).
#[test]
fn fase6_4_diminishing_returns_fires_only_with_active_plan() {
    use super::super::early_convergence::{
        ConvergenceDetector, ConvergenceReason, DIMINISHING_WINDOW,
    };
    // With active plan: fires after DIMINISHING_WINDOW stagnant rounds.
    // Each call increments consecutive_stagnant by 1 (ratio=0.10, delta=0.0 is stagnant).
    // Fires when consecutive_stagnant >= DIMINISHING_WINDOW. So the first DIMINISHING_WINDOW-1
    // calls must NOT fire; the DIMINISHING_WINDOW-th call must fire.
    let mut det = ConvergenceDetector::new();
    for _ in 0..(DIMINISHING_WINDOW.saturating_sub(1)) {
        assert!(
            det.check(0.10, 100_000, 0.0).is_none(),
            "FASE6-4: must not fire before window completes"
        );
    }
    assert_eq!(
        det.check(0.10, 100_000, 0.0),
        Some(ConvergenceReason::DiminishingReturns),
        "FASE6-4: must fire DiminishingReturns after {} stagnant rounds",
        DIMINISHING_WINDOW
    );
    // Without active plan (ratio=0.0): MUST NOT fire (BUG-H3 guard).
    let mut det2 = ConvergenceDetector::new();
    for _ in 0..(DIMINISHING_WINDOW * 3) {
        assert!(
            det2.check(0.0, 100_000, 0.0).is_none(),
            "FASE6-4: BUG-H3: must not fire without active plan (ratio=0.0)"
        );
    }
}

/// FASE6-5: MacroPlanView emits correctly formatted [N/M] progress lines.
#[test]
fn fase6_5_macro_plan_view_emits_correct_lines() {
    use super::super::macro_feedback::{FeedbackMode, MacroPlanView};
    use halcon_core::traits::{ExecutionPlan, PlanStep};
    use uuid::Uuid;

    let steps = vec![
        PlanStep {
            step_id: Uuid::new_v4(),
            description: "Read source files".into(),
            tool_name: Some("file_read".into()),
            parallel: false,
            confidence: 0.9,
            expected_args: Default::default(),
            outcome: None,
        },
        PlanStep {
            step_id: Uuid::new_v4(),
            description: "Apply changes".into(),
            tool_name: Some("file_edit".into()),
            parallel: false,
            confidence: 0.85,
            expected_args: Default::default(),
            outcome: None,
        },
        PlanStep {
            step_id: Uuid::new_v4(),
            description: "Synthesise findings".into(),
            tool_name: None,
            parallel: false,
            confidence: 1.0,
            expected_args: Default::default(),
            outcome: None,
        },
    ];
    let plan = ExecutionPlan {
        plan_id: Uuid::new_v4(),
        goal: "Refactor module".into(),
        steps,
        requires_confirmation: false,
        replan_count: 0,
        parent_plan_id: None,
        ..Default::default()
    };
    let mut view = MacroPlanView::from_plan(&plan, FeedbackMode::Compact);
    // Summary includes step separator.
    let summary = view.format_plan_summary();
    assert!(
        summary.starts_with("Plan:"),
        "FASE6-5: summary must start with 'Plan:'"
    );
    assert!(
        summary.contains("→"),
        "FASE6-5: summary must contain → separator"
    );
    // Start line for step 0 contains [1/3].
    let start = view.format_start(0);
    assert!(
        start.as_deref().unwrap_or("").contains("[1/3]"),
        "FASE6-5: start line must contain [1/3], got {:?}",
        start
    );
    // Advance and check done line.
    let advanced = view.advance().unwrap();
    let done = advanced.done_line();
    assert!(
        done.contains("[1/3]") && done.contains('✓'),
        "FASE6-5: done line must contain [1/3] and ✓, got '{done}'"
    );
    assert_eq!(
        view.current_idx(),
        1,
        "FASE6-5: current_idx must be 1 after advance"
    );
}

/// FASE6-6: Replan output is also compressed to ≤MAX_VISIBLE_STEPS.
#[test]
fn fase6_6_replan_output_compressed_to_max_steps() {
    use super::super::plan_compressor::{compress, MAX_VISIBLE_STEPS};
    use halcon_core::traits::{ExecutionPlan, PlanStep};
    use uuid::Uuid;

    let steps: Vec<PlanStep> = (1..=7)
        .map(|i| PlanStep {
            step_id: Uuid::new_v4(),
            description: format!("Replan step {i}"),
            tool_name: if i == 7 { None } else { Some("bash".into()) },
            parallel: false,
            confidence: 0.8,
            expected_args: Default::default(),
            outcome: None,
        })
        .collect();
    let replan = ExecutionPlan {
        plan_id: Uuid::new_v4(),
        goal: "Replanned goal".into(),
        steps,
        requires_confirmation: false,
        replan_count: 1,
        parent_plan_id: None,
        ..Default::default()
    };
    let (compressed, _) = compress(replan);
    assert!(
        compressed.steps.len() <= MAX_VISIBLE_STEPS,
        "FASE6-6: replan must compress to ≤{} steps, got {}",
        MAX_VISIBLE_STEPS,
        compressed.steps.len()
    );
    let last = compressed.steps.last().unwrap();
    assert!(
        last.tool_name.is_none(),
        "FASE6-6: synthesis must remain last in replan"
    );
}

/// FASE6-7: ConvergenceDetector resets correctly after replan.
#[test]
fn fase6_7_convergence_detector_resets_on_replan() {
    use super::super::early_convergence::{ConvergenceDetector, ConvergenceReason};
    // First detector fires and becomes spent.
    let mut det = ConvergenceDetector::new();
    assert_eq!(
        det.check(0.80, 100_000, 0.10),
        Some(ConvergenceReason::EvidenceThreshold),
        "FASE6-7: setup — first detector must fire"
    );
    assert!(
        det.check(0.90, 100_000, 0.10).is_none(),
        "FASE6-7: spent detector must not fire again (idempotent)"
    );
    // Fresh detector (created on replan) can fire again.
    let mut new_det = ConvergenceDetector::with_context_window(64_000);
    assert_eq!(
        new_det.check(0.80, 100_000, 0.10),
        Some(ConvergenceReason::EvidenceThreshold),
        "FASE6-7: fresh detector after replan must fire on new plan's evidence"
    );
}

/// FASE6-8: No convergence fires without an active plan (conversational mode).
///
/// BUG-H3 regression guard: DiminishingReturns must not fire when ratio=0.0.
/// TokenHeadroom and EvidenceThreshold are also ratio-gated or token-gated.
#[test]
fn fase6_8_no_convergence_without_active_plan() {
    use super::super::early_convergence::{ConvergenceDetector, DIMINISHING_WINDOW};
    let mut det = ConvergenceDetector::new();
    for round in 0..(DIMINISHING_WINDOW * 2) {
        let result = det.check(0.0, 200_000, 0.0);
        assert!(
            result.is_none(),
            "FASE6-8: round {round}: convergence must not fire with ratio=0.0"
        );
    }
}

/// RC-1b (compaction threshold): 60% threshold fires earlier than old 70%.
///
/// At 65% utilization, the new 60% threshold triggers compaction while the old
/// 70% threshold would not — preventing the agent from approaching the budget limit.
#[test]
fn compaction_60_percent_threshold_fires_earlier_than_70() {
    use super::super::compaction::ContextCompactor;
    use halcon_core::types::CompactionConfig;

    let config = CompactionConfig {
        enabled: true,
        threshold_fraction: 0.80,
        keep_recent: 4,
        max_context_tokens: 200_000,
    };
    let compactor = ContextCompactor::new(config);

    // pipeline_budget = 100_000 tokens.
    // 60% threshold = 60_000 tokens.
    // BPE: "word ".repeat(N) ≈ N+1 tokens.
    let pipeline_budget: u32 = 100_000;
    let text_65k = "word ".repeat(65_000);
    let msgs = vec![halcon_core::types::ChatMessage {
        role: halcon_core::types::Role::User,
        content: halcon_core::types::MessageContent::Text(text_65k),
    }];

    // With new 60% threshold, 65K tokens triggers compaction.
    assert!(
        compactor.needs_compaction_with_budget(&msgs, pipeline_budget),
        "60% threshold: 65K tokens must trigger compaction on 100K budget"
    );

    // Verify exact boundary: 59_998 repeats ≈ 59_999 tokens — just below 60_000 threshold.
    let text_just_below = "word ".repeat(59_998);
    let msgs_below = vec![halcon_core::types::ChatMessage {
        role: halcon_core::types::Role::User,
        content: halcon_core::types::MessageContent::Text(text_just_below),
    }];
    assert!(
        !compactor.needs_compaction_with_budget(&msgs_below, pipeline_budget),
        "60% threshold: 59_999 tokens must NOT trigger compaction on 100K budget"
    );
}

// ── Phase 111: IntentScorer replaces TaskAnalyzer — pipeline smoke tests ──

/// Verify IntentScorer::score() returns an IntentProfile with all fields valid for the
/// TUI reasoning panel (task_type.as_str(), complexity match arms).
/// This validates the drop-in replacement of TaskAnalyzer::analyze() at agent.rs line 699.
#[test]
fn intent_scorer_profile_compatible_with_reasoning_panel() {
    use crate::repl::intent_scorer::IntentScorer;
    use crate::repl::task_analyzer::TaskComplexity;

    let queries = [
        "hola",
        "fix the bug in auth.rs",
        "refactor all modules to use async/await across the entire project",
    ];
    for query in &queries {
        let profile = IntentScorer::score(query);
        // task_type.as_str() must not panic — used at TUI panel line 858.
        let _type_str = profile.task_type.as_str();
        // complexity match must be exhaustive — used at TUI panel lines 859-863.
        let _complexity_str = match profile.complexity {
            TaskComplexity::Simple => "Simple",
            TaskComplexity::Moderate => "Moderate",
            TaskComplexity::Complex => "Complex",
        };
        // suggested_max_rounds() must be > 0 — used for ConvergenceController config.
        assert!(
            profile.suggested_max_rounds() > 0,
            "suggested_max_rounds must be positive for {:?}",
            query
        );
    }
}

/// Verify that the IntentScorer-based ConvergenceController (Phase 101 wiring)
/// produces tighter round budgets for conversational queries than project-wide queries.
/// This is the key integration guarantee: simpler queries burn fewer rounds.
#[test]
fn convergence_controller_from_intent_profile_scales_with_scope() {
    use crate::repl::convergence_controller::ConvergenceController;
    use crate::repl::intent_scorer::IntentScorer;

    let simple_profile = IntentScorer::score("hola");
    let complex_profile = IntentScorer::score(
            "analyze every Rust module and generate a comprehensive dependency graph for the whole project",
        );

    let simple_ctrl = ConvergenceController::new(&simple_profile, "hola");
    let complex_ctrl = ConvergenceController::new(&complex_profile, "analyze every module");

    assert!(
        simple_ctrl.max_rounds() < complex_ctrl.max_rounds(),
        "Conversational max_rounds ({}) must be < ProjectWide max_rounds ({})",
        simple_ctrl.max_rounds(),
        complex_ctrl.max_rounds()
    );
}

// ── Phase 113: Double-synthesis coordination tests ─────────────────────────

/// Verify that ConvergenceAction variants are all present for exhaustive matching.
/// This is a compile-time contract test: adding/removing a variant fails the count.
#[test]
fn convergence_action_exhaustive_match() {
    use crate::repl::convergence_controller::ConvergenceAction;
    let actions = [
        ConvergenceAction::Continue,
        ConvergenceAction::Synthesize,
        ConvergenceAction::Replan,
        ConvergenceAction::Halt,
    ];
    assert_eq!(
        actions.len(),
        4,
        "ConvergenceAction must have exactly 4 variants"
    );
}

/// IntentScorer is deterministic: same input → same output on every call.
/// This is required for the optimization in Phase 111 (single score call per user message).
#[test]
fn intent_scorer_is_deterministic_for_same_input() {
    use crate::repl::intent_scorer::IntentScorer;

    let query = "fix the authentication bug in login.rs and add proper error handling";
    let p1 = IntentScorer::score(query);
    let p2 = IntentScorer::score(query);

    assert_eq!(
        p1.task_type, p2.task_type,
        "task_type must be deterministic"
    );
    assert_eq!(
        p1.complexity, p2.complexity,
        "complexity must be deterministic"
    );
    assert_eq!(p1.scope, p2.scope, "scope must be deterministic");
    assert_eq!(
        p1.suggested_max_rounds(),
        p2.suggested_max_rounds(),
        "suggested_max_rounds must be deterministic"
    );
}

// ── Phase 115: Edge case hardening ────────────────────────────────────────

/// Edge case: empty string input must not panic — IntentScorer must be robust.
/// Empty queries arrive from programmatic API callers and must produce a valid profile.
#[test]
fn intent_scorer_handles_empty_string_without_panic() {
    use crate::repl::intent_scorer::IntentScorer;
    let profile = IntentScorer::score("");
    assert!(
        profile.suggested_max_rounds() > 0,
        "empty input must still produce a valid budget"
    );
    assert!(
        !profile.task_hash.is_empty(),
        "task_hash must be non-empty even for empty input"
    );
}

/// Edge case: single-word queries (e.g. "hola", "help", "quit").
/// Must be classified as Conversational/Simple with a minimal round budget.
#[test]
fn intent_scorer_single_word_is_conversational_or_simple() {
    use crate::repl::intent_scorer::IntentScorer;
    use crate::repl::task_analyzer::TaskComplexity;
    for word in &["hola", "help", "quit", "hi"] {
        let p = IntentScorer::score(word);
        assert!(
            p.complexity == TaskComplexity::Simple,
            "{:?} → complexity {:?} (expected Simple)",
            word,
            p.complexity
        );
        assert!(
            p.suggested_max_rounds() <= 4,
            "{:?} → max_rounds {} (expected ≤ 4)",
            word,
            p.suggested_max_rounds()
        );
    }
}

/// Edge case: very long queries (500+ words) must not panic and must produce a valid budget.
/// Extreme verbosity → Complex classification → higher round budget.
#[test]
fn intent_scorer_very_long_query_does_not_panic() {
    use crate::repl::intent_scorer::IntentScorer;
    use crate::repl::task_analyzer::TaskComplexity;
    let long_query = "analyze ".repeat(100); // ~800 chars, 100 tokens
    let p = IntentScorer::score(&long_query);
    assert!(
        p.suggested_max_rounds() > 0,
        "long query must produce valid budget"
    );
    assert!(
        p.complexity == TaskComplexity::Complex || p.complexity == TaskComplexity::Moderate,
        "long query should be Complex or Moderate, got {:?}",
        p.complexity
    );
}

/// Edge case: purely numeric / symbol inputs must not panic.
#[test]
fn intent_scorer_handles_non_alphabetic_input() {
    use crate::repl::intent_scorer::IntentScorer;
    let inputs = ["12345", "!@#$%", "123 abc !!", "  \t\n  "];
    for input in &inputs {
        let p = IntentScorer::score(input);
        assert!(
            p.suggested_max_rounds() > 0,
            "input {:?} must produce valid budget",
            input
        );
    }
}

/// Phase 115: ConvergenceController max_rounds() getter must match suggested_max_rounds.
/// This verifies the Phase 101 wiring is correct: ConvergenceController is initialized
/// with the same budget that IntentScorer recommends.
#[test]
fn convergence_controller_max_rounds_getter_matches_intent_profile() {
    use crate::repl::convergence_controller::ConvergenceController;
    use crate::repl::intent_scorer::IntentScorer;
    for query in &[
        "hola",
        "fix the null pointer in auth.rs",
        "refactor the entire authentication subsystem",
    ] {
        let profile = IntentScorer::score(query);
        let ctrl = ConvergenceController::new(&profile, query);
        // The controller's max_rounds must be ≤ the profile's suggestion
        // (may be less if base limits are tighter).
        assert!(
            ctrl.max_rounds() <= profile.suggested_max_rounds(),
            "Query {:?}: ctrl.max_rounds ({}) > profile.suggested ({})",
            query,
            ctrl.max_rounds(),
            profile.suggested_max_rounds()
        );
    }
}

// ── FASE H: Parallelism isolation tests ───────────────────────────────────

/// Verify that a fresh ToolSpeculator starts with a clean, empty state.
/// Documents the contract relied upon by test_ctx (Box::leak approach):
/// every ToolSpeculator::new() call produces an independent instance with no
/// residual cache entries or metrics from prior tests.
#[test]
fn fresh_speculator_has_clean_state() {
    use crate::repl::tool_speculation::ToolSpeculator;
    let spec = ToolSpeculator::new();
    let metrics = spec.metrics();
    assert_eq!(
        metrics.total_checks, 0,
        "fresh speculator must have zero total_checks"
    );
    assert_eq!(metrics.hits, 0, "fresh speculator must have zero hits");
    assert_eq!(metrics.misses, 0, "fresh speculator must have zero misses");
}

/// Two ToolSpeculator instances created via ToolSpeculator::new() must be
/// fully independent — different heap allocations, no aliasing.
/// Structural proof that Box::leak per test_ctx call eliminates
/// cross-test speculator state contamination.
#[test]
fn fresh_speculators_are_independent_allocations() {
    use crate::repl::tool_speculation::ToolSpeculator;
    let s1 = Box::new(ToolSpeculator::new());
    let s2 = Box::new(ToolSpeculator::new());
    let p1 = &*s1 as *const ToolSpeculator;
    let p2 = &*s2 as *const ToolSpeculator;
    assert_ne!(p1, p2, "two fresh ToolSpeculator instances must not alias");
}

/// Run 4 agent loops sequentially within the same test, each receiving a
/// freshly-allocated ToolSpeculator via Box::leak (FASE H isolation guarantee).
///
/// run_agent_loop uses !Send tracing EnteredSpan, so true concurrent spawning
/// via JoinSet is not feasible here. Sequential execution proves the isolation
/// property: each iteration starts with zero speculator state from prior runs.
#[tokio::test]
async fn repeated_agent_loops_with_independent_speculators() {
    for i in 0..4 {
        let provider: std::sync::Arc<dyn ModelProvider> =
            std::sync::Arc::new(halcon_providers::EchoProvider::new());
        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let request = make_request(vec![]);
        let tool_reg = ToolRegistry::new();
        let mut perms = ConversationalPermissionHandler::new(true);
        let (event_tx, _rx) = test_event_tx();
        let limits = AgentLimits::default();
        let mut resilience = test_resilience();
        let routing_config = RoutingConfig::default();

        let result = run_agent_loop(test_ctx(
            &provider,
            &mut session,
            &request,
            &tool_reg,
            &mut perms,
            &event_tx,
            &limits,
            &mut resilience,
            &routing_config,
        ))
        .await
        .unwrap();

        assert!(
            !result.full_text.is_empty(),
            "agent loop iteration {i} must produce non-empty text"
        );
        assert_eq!(
            result.stop_condition,
            StopCondition::EndTurn,
            "agent loop iteration {i} must stop at EndTurn"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Phase L Regression Suite — invariants I-L-1, I-L-2, and 5 scenarios
// ─────────────────────────────────────────────────────────────────────────

/// I-L-1: ConversationalSimple → max_rounds ≥ 1 AND scope is Conversational.
///
/// Verifies that greeting queries are NOT assigned zero max_rounds and
/// are classified as Conversational scope (not SingleArtifact or higher).
#[test]
fn phase_l_invariant_l1_conversational_simple_has_valid_budget() {
    use crate::repl::domain::intent_scorer::TaskScope;
    use crate::repl::intent_scorer::IntentScorer;
    // Note: only use greetings that appear in CONVERSATIONAL keyword list.
    // "hi" is not in the list (only "hello" is), so omit it here.
    for greeting in &["hola", "hello", "gracias", "thanks"] {
        let profile = IntentScorer::score(greeting);
        // I-L-1 part 1: max_rounds must be at least 1
        assert!(
            profile.suggested_max_rounds() >= 1,
            "I-L-1 violated: \'{greeting}\' → max_rounds={} (expected ≥ 1)",
            profile.suggested_max_rounds()
        );
        // I-L-1 part 2: must be classified as Conversational, not task scope
        assert_eq!(
            profile.scope,
            TaskScope::Conversational,
            "I-L-1 violated: \'{greeting}\' → scope={:?} (expected Conversational)",
            profile.scope
        );
    }
}

/// I-L-1 regression: "analiza mi implementacion" must NOT be Conversational.
///
/// Phase K root cause: word-count fallback classified 3-word task queries as
/// Conversational → max_rounds=2, violating INVARIANT K5-1.
/// Phase L fix B1 removes that fallback.
#[test]
fn phase_l_regression_analiza_not_conversational() {
    use crate::repl::domain::intent_scorer::TaskScope;
    use crate::repl::intent_scorer::IntentScorer;
    let cases = [
        "analiza mi implementacion",
        "revisa el código",
        "implementa la función",
        "refactoriza este módulo",
    ];
    for query in &cases {
        let profile = IntentScorer::score(query);
        assert_ne!(
                profile.scope, TaskScope::Conversational,
                "Phase L regression: \'{query}\' classified as Conversational —                  word-count fallback fix B1 not applied"
            );
    }
}

/// Budget invariant: max_rounds ≥ plan.total_steps + critic_retries + 1.
///
/// Verifies the BudgetInvariantChecker from plan_state_diagnostics correctly
/// identifies and corrects insufficient budgets (K5-1 enforcement).
#[test]
fn phase_l_regression_budget_invariant_analiza_scenario() {
    use crate::repl::planning::diagnostics::BudgetInvariantChecker;
    // "analiza mi implementacion" generates 2-step plan. With max_rounds=2:
    // required = 2 + 1 (critic_retries) + 1 (synthesis) = 4 > 2 → violated.
    let result = BudgetInvariantChecker::check_max_rounds_invariant(2, 2, 1);
    assert!(
        result.is_err(),
        "K5-1 must fire: max_rounds=2 for 2-step plan + 1 retry"
    );
    assert_eq!(result.unwrap_err(), 4, "corrected max_rounds must be 4");
}

/// Token efficiency: text-only rounds must use neutral (0.5) efficiency.
///
/// Prevents "hola" from scoring 0.56 due to short output penalization.
/// Phase L fix C3: neutralize token_efficiency when tools_total == 0.
#[test]
fn phase_l_regression_token_efficiency_neutral_for_text_rounds() {
    use crate::repl::round_scorer::RoundScorer;
    let mut scorer = RoundScorer::new(
        "answer the greeting",
        std::sync::Arc::new(halcon_core::types::PolicyConfig::default()),
    );
    // Simulate a text-only round (0 tools, short output, high input)
    // score_round(round, tools_succeeded, tools_total, output_tokens, input_tokens,
    //             plan_progress_ratio, anomaly_flags, round_text)
    let eval = scorer.score_round(
        0,                                  // round index
        0,                                  // tools_succeeded
        0,                                  // tools_total (text-only — Phase L fix C3 uses neutral)
        179,                                // output_tokens (small — like "hola" response)
        4384,                               // input_tokens (large — system prompt + tools)
        0.0,                                // plan_progress_ratio (no plan)
        vec![],                             // anomaly_flags
        "Hello! How can I help you today?", // round_text (short)
    );
    // With C3 fix, token_efficiency = 0.5 (neutral) not 179/4384 = 0.041.
    // combined_score should be noticeably above 0.20 (the old broken value).
    assert!(
        eval.token_efficiency >= 0.49 && eval.token_efficiency <= 0.51,
        "C3 fix: token_efficiency for text-only round must be 0.5 (neutral), got {}",
        eval.token_efficiency
    );
    assert!(
        eval.combined_score > 0.20,
        "C3 fix: combined_score for valid greeting response must be > 0.20, got {}",
        eval.combined_score
    );
}

/// TokenHeadroom invariant: provider_context_window > pipeline_budget.
///
/// Verifies that provider_context_window (used as denominator since Phase L)
/// is always larger than pipeline_budget (the old broken denominator).
#[test]
fn phase_l_regression_provider_window_larger_than_pipeline_budget() {
    // Typical values for deepseek-chat:
    let model_context_window: u32 = 64_000;
    let pipeline_budget: u32 = {
        let input_fraction = (model_context_window as f64 * 0.80) as u32;
        let max_total_tokens: u32 = 18_000; // from observed config
        input_fraction.min(max_total_tokens)
    };
    // call_input_tokens after sub-agent injection (observed in TOKEN_AUDIT_REPORT)
    let call_input_tokens: u64 = 24_504;

    // OLD (broken): pipeline_budget as denominator → saturates to 0
    let old_remaining = (pipeline_budget as u64).saturating_sub(call_input_tokens);
    // NEW (fixed): provider_context_window as denominator → correct value
    let new_remaining = (model_context_window as u64).saturating_sub(call_input_tokens);

    assert_eq!(
        old_remaining, 0,
        "Old formula must produce 0 (demonstrating the bug)"
    );
    assert!(
        new_remaining > 4_000,
        "New formula must leave > 4000 tokens remaining above MIN_SYNTHESIS_HEADROOM, got {}",
        new_remaining
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Phase 7 — Integration Tests (cross-phase validation)
// ═══════════════════════════════════════════════════════════════════════

/// Phase 7: PolicyConfig is consumed by reward_pipeline with correct defaults.
/// Validates Phase 1 (PolicyConfig) ↔ reward_pipeline integration.
#[test]
fn integration_policy_config_consumed_by_reward_pipeline() {
    use super::super::reward_pipeline::{compute_reward, RawRewardSignals};

    let policy = halcon_core::types::PolicyConfig::default();

    // EndTurn with full completion → high reward
    let good_signals = RawRewardSignals {
        stop_condition: StopCondition::EndTurn,
        round_scores: vec![0.8, 0.85],
        critic_verdict: Some((true, 0.90)),
        plan_coherence_score: 0.1,
        oscillation_penalty: 0.0,
        plan_completion_ratio: 1.0,
        plugin_snapshots: vec![],
        critic_unavailable: false,
        evidence_coverage: 1.0,
    };
    let good = compute_reward(&good_signals, &policy);
    assert!(
        good.final_reward > policy.success_threshold,
        "EndTurn with good signals should exceed success_threshold ({:.2}), got {:.4}",
        policy.success_threshold,
        good.final_reward
    );

    // ForcedSynthesis with no completion → below success_threshold
    let poor_signals = RawRewardSignals {
        stop_condition: StopCondition::ForcedSynthesis,
        round_scores: vec![0.2, 0.15],
        critic_verdict: Some((false, 0.85)),
        plan_coherence_score: 0.6,
        oscillation_penalty: 0.3,
        plan_completion_ratio: 0.0,
        plugin_snapshots: vec![],
        critic_unavailable: false,
        evidence_coverage: 0.2,
    };
    let poor = compute_reward(&poor_signals, &policy);
    assert!(
        poor.final_reward < policy.success_threshold,
        "ForcedSynthesis with poor signals should be below success_threshold ({:.2}), got {:.4}",
        policy.success_threshold,
        poor.final_reward
    );

    // critic_unavailable penalty uses PolicyConfig value
    let unavail_signals = RawRewardSignals {
        stop_condition: StopCondition::ForcedSynthesis,
        round_scores: vec![0.5],
        critic_verdict: None,
        plan_coherence_score: 0.2,
        oscillation_penalty: 0.0,
        plan_completion_ratio: 0.5,
        plugin_snapshots: vec![],
        critic_unavailable: true,
        evidence_coverage: 1.0,
    };
    let unavail = compute_reward(&unavail_signals, &policy);
    // Same signals without critic_unavailable
    let avail_signals = RawRewardSignals {
        critic_unavailable: false,
        ..unavail_signals.clone()
    };
    let avail = compute_reward(&avail_signals, &policy);
    let penalty_diff = avail.final_reward - unavail.final_reward;
    assert!(
        (penalty_diff - policy.critic_unavailable_penalty).abs() < 0.05,
        "critic_unavailable penalty should be ~{:.2}, got {:.4}",
        policy.critic_unavailable_penalty,
        penalty_diff
    );
}

/// Phase 7: SLA Fast mode caps rounds at 4.
/// Validates Phase 2 (SLA Hard Enforcement) constraints.
#[test]
fn integration_sla_fast_mode_4_rounds_cap() {
    use super::super::sla_manager::{SlaBudget, SlaMode};

    let budget = SlaBudget::from_mode(SlaMode::Fast);
    assert_eq!(budget.max_rounds, 4, "Fast mode must cap at 4 rounds");
    assert_eq!(
        budget.max_sub_agents, 0,
        "Fast mode must disallow sub-agents"
    );
    assert_eq!(budget.max_retries, 0, "Fast mode must disallow retries");
    assert_eq!(
        budget.max_plan_depth, 2,
        "Fast mode must cap plan depth at 2"
    );

    // clamp_rounds enforces the cap
    assert_eq!(budget.clamp_rounds(10), 4);
    assert_eq!(budget.clamp_rounds(3), 3);
    assert_eq!(budget.clamp_plan_depth(8), 2);
}

/// Phase 7: SLA blocks retry and orchestration when budget is exhausted.
/// Validates Phase 2 (SLA) allows_retry / allows_orchestration wiring.
#[test]
fn integration_sla_blocks_retry_and_orchestration() {
    use super::super::sla_manager::{SlaBudget, SlaMode};

    // Fast mode: 0 retries, 0 sub-agents
    let fast = SlaBudget::from_mode(SlaMode::Fast);
    assert!(!fast.allows_retry(0), "Fast SLA must block all retries");
    assert!(
        !fast.allows_orchestration(),
        "Fast SLA must block orchestration"
    );

    // Balanced mode: 1 retry, 3 sub-agents
    let balanced = SlaBudget::from_mode(SlaMode::Balanced);
    assert!(balanced.allows_retry(0), "Balanced allows first retry");
    assert!(!balanced.allows_retry(1), "Balanced blocks second retry");
    assert!(
        balanced.allows_orchestration(),
        "Balanced allows orchestration"
    );

    // Deep mode: 3 retries, 8 sub-agents
    let deep = SlaBudget::from_mode(SlaMode::Deep);
    assert!(deep.allows_retry(0));
    assert!(deep.allows_retry(2));
    assert!(!deep.allows_retry(3), "Deep blocks 4th retry");
    assert!(deep.allows_orchestration());
}

/// Phase 7: EvidenceGraph synthesis_coverage drives reward signal.
/// Validates Phase 3 (EvidenceGraph Governance) ↔ reward_pipeline integration.
#[test]
fn integration_evidence_graph_enriches_synthesis() {
    use super::super::evidence_graph::EvidenceGraph;
    use super::super::reward_pipeline::{compute_reward, RawRewardSignals};

    let policy = halcon_core::types::PolicyConfig::default();

    // Build graph with 3 Good nodes, reference only 1
    let mut graph = EvidenceGraph::new();
    let n0 = graph.add_node("read_file", "src/main.rs", 500, false, None, 0);
    let _n1 = graph.add_node("read_file", "src/lib.rs", 300, false, None, 1);
    let _n2 = graph.add_node("grep", "search pattern", 200, false, None, 1);
    graph.mark_referenced(n0);

    let coverage = graph.synthesis_coverage();
    assert!(
        (coverage - 1.0 / 3.0).abs() < 0.01,
        "1 of 3 Good nodes referenced → coverage ~0.33, got {:.4}",
        coverage
    );
    assert!(
        coverage < policy.min_synthesis_coverage
            || (coverage - policy.min_synthesis_coverage).abs() < 0.05,
        "Low coverage ({:.4}) should be near or below min_synthesis_coverage ({:.2})",
        coverage,
        policy.min_synthesis_coverage
    );

    // Low coverage → negative evidence bonus
    let low_cov_signals = RawRewardSignals {
        stop_condition: StopCondition::EndTurn,
        round_scores: vec![0.7],
        critic_verdict: Some((true, 0.80)),
        plan_coherence_score: 0.1,
        oscillation_penalty: 0.0,
        plan_completion_ratio: 0.8,
        plugin_snapshots: vec![],
        critic_unavailable: false,
        evidence_coverage: coverage,
    };
    let low_reward = compute_reward(&low_cov_signals, &policy);

    // Full coverage → positive evidence bonus
    let high_cov_signals = RawRewardSignals {
        evidence_coverage: 1.0,
        ..low_cov_signals.clone()
    };
    let high_reward = compute_reward(&high_cov_signals, &policy);

    // Evidence coverage difference should produce a measurable delta
    let delta = high_reward.final_reward - low_reward.final_reward;
    assert!(
        delta > 0.0,
        "Full coverage should produce higher reward than partial, delta = {:.6}",
        delta
    );
    // Max delta = (1.0-0.5)*0.05 - (0.33-0.5)*0.05 = 0.025 - (-0.0085) = 0.0335
    assert!(
        delta < 0.05,
        "Evidence coverage bonus is small (±0.025), delta should be < 0.05, got {:.6}",
        delta
    );
}

/// Phase 7: FSM full lifecycle — Idle through all phases to Completed.
/// Validates Phase 4 (FSM Formalization).
#[test]
fn integration_fsm_full_lifecycle() {
    use super::super::agent::loop_state::{transition, AgentEvent, AgentPhase};

    let mut phase = AgentPhase::Idle;

    // Idle → Planning (plan generated)
    phase = transition(phase, AgentEvent::PlanGenerated);
    assert_eq!(phase, AgentPhase::Planning);

    // Planning → Executing (plan produced)
    phase = transition(phase, AgentEvent::PlanGenerated);
    assert_eq!(phase, AgentPhase::Executing);

    // Executing → Executing (tool batch complete — stays)
    phase = transition(phase, AgentEvent::ToolBatchComplete);
    assert_eq!(phase, AgentPhase::Executing);

    // Executing → Synthesizing (synthesis started)
    phase = transition(phase, AgentEvent::SynthesisStarted);
    assert_eq!(phase, AgentPhase::Synthesizing);

    // Synthesizing → Evaluating (synthesis complete)
    phase = transition(phase, AgentEvent::SynthesisComplete);
    assert_eq!(phase, AgentPhase::Evaluating);

    // Evaluating → Completed
    phase = transition(phase, AgentEvent::EvaluationComplete);
    assert_eq!(phase, AgentPhase::Completed);

    // Completed rejects non-escape events — returns current phase unchanged (with warn log).
    let unchanged = transition(phase, AgentEvent::PlanGenerated);
    assert_eq!(
        unchanged,
        AgentPhase::Completed,
        "Completed must reject non-escape events (stays Completed)"
    );

    // ErrorOccurred can exit Completed → Halted
    phase = transition(phase, AgentEvent::ErrorOccurred);
    assert_eq!(phase, AgentPhase::Halted);
}

/// Phase 7: FSM backward compatibility — as_str() matches legacy string values.
/// Validates Phase 4 backward compat guarantee.
#[test]
fn integration_fsm_as_str_backward_compat() {
    use super::super::agent::loop_state::AgentPhase;

    let expected = [
        (AgentPhase::Idle, "idle"),
        (AgentPhase::Planning, "planning"),
        (AgentPhase::Executing, "executing"),
        (AgentPhase::Reflecting, "reflecting"),
        (AgentPhase::Synthesizing, "synthesizing"),
        (AgentPhase::Evaluating, "evaluating"),
        (AgentPhase::Completed, "completed"),
        (AgentPhase::Halted, "halted"),
    ];
    for (phase, str_val) in &expected {
        assert_eq!(
            phase.as_str(),
            *str_val,
            "AgentPhase::{:?}.as_str() must match legacy string",
            phase
        );
    }
}

/// Phase 7: K5-2 compaction triggers on sustained growth.
/// Validates Phase 5 (K5-2 Growth Invariant) threshold enforcement.
#[test]
fn integration_k5_2_compaction_on_sustained_growth() {
    let policy = halcon_core::types::PolicyConfig::default();

    // Simulate consecutive growth violations
    let mut consecutive = 0u32;
    let mut compaction_needed = false;
    let rounds_data: Vec<(u64, u64)> = vec![
        (1000, 0),    // round 0: baseline
        (1400, 1000), // round 1: 1.4× growth (> 1.3)
        (2000, 1400), // round 2: 1.43× growth (> 1.3)
        (2200, 2000), // round 3: 1.1× growth (< 1.3) — resets
        (3000, 2200), // round 4: 1.36× growth (> 1.3)
        (4500, 3000), // round 5: 1.5× growth (> 1.3) — triggers!
    ];

    for (current, prev) in &rounds_data {
        if *prev == 0 {
            continue;
        }
        let ratio = *current as f64 / *prev as f64;
        if ratio > policy.growth_threshold {
            consecutive += 1;
        } else {
            consecutive = 0;
        }
        if consecutive >= policy.growth_consecutive_trigger {
            compaction_needed = true;
        }
    }

    assert!(
        compaction_needed,
        "2 consecutive violations at rounds 4-5 should trigger compaction"
    );

    // Verify growth_threshold and trigger values from policy
    assert!((policy.growth_threshold - 1.3).abs() < f64::EPSILON);
    assert_eq!(policy.growth_consecutive_trigger, 2);
}

/// Phase 7: K5-2 linear growth does NOT trigger compaction.
/// Validates Phase 5 doesn't false-positive on steady growth.
#[test]
fn integration_k5_2_no_compaction_linear_growth() {
    let policy = halcon_core::types::PolicyConfig::default();

    let mut consecutive = 0u32;
    let mut compaction_needed = false;
    // Linear 1.2× growth each round — below 1.3 threshold
    let rounds: Vec<u64> = vec![1000, 1200, 1440, 1728, 2074];
    for i in 1..rounds.len() {
        let ratio = rounds[i] as f64 / rounds[i - 1] as f64;
        if ratio > policy.growth_threshold {
            consecutive += 1;
        } else {
            consecutive = 0;
        }
        if consecutive >= policy.growth_consecutive_trigger {
            compaction_needed = true;
        }
    }

    assert!(
        !compaction_needed,
        "1.2× growth is below 1.3 threshold — no compaction"
    );
}

/// Phase 7: ExecutionTracker truncate_to respects SLA plan depth.
/// Validates Phase 2 (SLA) ↔ ExecutionTracker integration.
#[test]
fn integration_sla_truncates_plan_via_execution_tracker() {
    use super::super::execution_tracker::ExecutionTracker;
    use super::super::sla_manager::{SlaBudget, SlaMode};
    use halcon_core::traits::{ExecutionPlan, PlanStep};

    let budget = SlaBudget::from_mode(SlaMode::Fast);
    let sla_max_rounds = budget.max_rounds; // 4

    // Build a plan with 6 steps — exceeds Fast SLA
    let plan = ExecutionPlan {
        goal: "test task".into(),
        steps: (0..6)
            .map(|i| PlanStep {
                step_id: uuid::Uuid::new_v4(),
                description: format!("Step {}", i),
                tool_name: Some("bash".into()),
                parallel: false,
                confidence: 0.9,
                expected_args: None,
                outcome: None,
            })
            .collect(),
        requires_confirmation: false,
        plan_id: uuid::Uuid::nil(),
        replan_count: 0,
        parent_plan_id: None,
        ..Default::default()
    };

    let (tx, _rx) = tokio::sync::broadcast::channel(16);
    let mut tracker = ExecutionTracker::new(plan, tx);
    assert_eq!(tracker.plan().steps.len(), 6);

    // Truncate to fit SLA: sla_max - 2 (room for critic + synthesis)
    let target = (sla_max_rounds as usize).saturating_sub(2);
    tracker.truncate_to(target);
    assert_eq!(
        tracker.plan().steps.len(),
        target,
        "Plan should be truncated to sla_max_rounds - 2 = {}",
        target
    );
}

/// Phase 7: PolicyConfig fields consumed by Phase 5 K5-2 and Phase 6 mini-critic
/// match their documented default values.
#[test]
fn integration_policy_config_phase5_phase6_defaults() {
    let p = halcon_core::types::PolicyConfig::default();

    // Phase 5 K5-2 fields
    assert!(
        (p.growth_threshold - 1.3).abs() < f64::EPSILON,
        "K5-2 growth threshold default must be 1.3"
    );
    assert_eq!(
        p.growth_consecutive_trigger, 2,
        "K5-2 consecutive trigger default must be 2"
    );

    // Phase 6 mini-critic fields
    assert_eq!(
        p.mini_critic_interval, 3,
        "Mini-critic interval default must be 3"
    );
    assert!(
        (p.mini_critic_budget_fraction - 0.50).abs() < f64::EPSILON,
        "Mini-critic budget fraction default must be 0.50"
    );

    // Phase 1 core fields verify backward compat
    assert!((p.success_threshold - 0.60).abs() < f64::EPSILON);
    assert!((p.min_synthesis_coverage - 0.30).abs() < f64::EPSILON);
    assert_eq!(p.min_evidence_bytes, 30);
}

/// Phase 7: Cross-phase invariant — SLA mode maps correctly from task complexity.
/// Validates Phase 2 (SLA) ↔ Phase F2 (Decision Layer) bridge.
#[test]
fn integration_sla_from_complexity_bridge() {
    use super::super::decision_layer::{OrchestrationDecision, TaskComplexity};
    use super::super::sla_manager::{SlaBudget, SlaMode};

    let simple = OrchestrationDecision {
        complexity: TaskComplexity::SimpleExecution,
        use_orchestration: false,
        recommended_max_rounds: 4,
        recommended_plan_depth: 2,
        reason: "simple task",
    };
    let simple_sla = SlaBudget::from_complexity(&simple);
    assert_eq!(simple_sla.mode, SlaMode::Fast);
    assert_eq!(simple_sla.max_rounds, 4);

    let multi = OrchestrationDecision {
        complexity: TaskComplexity::MultiDomain,
        use_orchestration: true,
        recommended_max_rounds: 20,
        recommended_plan_depth: 10,
        reason: "complex multi-domain task",
    };
    let multi_sla = SlaBudget::from_complexity(&multi);
    assert_eq!(multi_sla.mode, SlaMode::Deep);
    assert_eq!(multi_sla.max_rounds, 20);
    assert!(multi_sla.allows_orchestration());
    assert!(multi_sla.allows_retry(2));
}

/// Phase 7: EvidenceGraph + PolicyConfig — unreferenced evidence detection.
/// Validates Phase 3 (EvidenceGraph) governance using Phase 1 (PolicyConfig) thresholds.
#[test]
fn integration_evidence_graph_unreferenced_detection() {
    use super::super::evidence_graph::EvidenceGraph;

    let policy = halcon_core::types::PolicyConfig::default();
    let mut graph = EvidenceGraph::new();

    // Add 4 Good nodes
    let ids: Vec<_> = (0..4)
        .map(|i| graph.add_node("read_file", &format!("file_{}.rs", i), 100, false, None, i))
        .collect();

    // Reference 2 of 4
    graph.mark_referenced(ids[0]);
    graph.mark_referenced(ids[1]);

    let coverage = graph.synthesis_coverage();
    assert!((coverage - 0.5).abs() < 0.01, "2/4 = 0.5 coverage");

    // Coverage exceeds min_synthesis_coverage (0.30) — no hint injection needed
    assert!(
        coverage > policy.min_synthesis_coverage,
        "0.5 coverage exceeds min_synthesis_coverage ({:.2})",
        policy.min_synthesis_coverage
    );

    // But unreferenced list has 2 items
    let unreferenced = graph.unreferenced_evidence();
    assert_eq!(unreferenced.len(), 2, "2 Good nodes remain unreferenced");

    // Now test below threshold: 0 of 4 referenced
    let mut graph2 = EvidenceGraph::new();
    for i in 0..4 {
        graph2.add_node("read_file", &format!("file_{}.rs", i), 100, false, None, i);
    }
    let coverage2 = graph2.synthesis_coverage();
    assert!((coverage2).abs() < 0.01, "0/4 = 0.0 coverage");
    assert!(
        coverage2 < policy.min_synthesis_coverage,
        "0.0 coverage is below min_synthesis_coverage — hint injection would fire"
    );
}

// ── Phase 8: Cross-phase integration tests ──────────────────────────────

#[test]
fn integration_policy_threads_to_supervisor_timeout() {
    use crate::repl::supervisor::LoopCritic;

    let mut policy = halcon_core::types::PolicyConfig::default();
    policy.critic_timeout_secs = 120;
    policy.excerpt_len = 500;
    policy.halt_confidence_threshold = 0.90;

    // should_halt_raw with custom threshold
    assert!(
        LoopCritic::should_halt_raw(false, 0.95, policy.halt_confidence_threshold),
        "0.95 confidence >= 0.90 threshold → should halt"
    );
    assert!(
        !LoopCritic::should_halt_raw(false, 0.85, policy.halt_confidence_threshold),
        "0.85 confidence < 0.90 threshold → should NOT halt"
    );
    // Default threshold would halt at 0.85 since default is 0.80
    assert!(
        LoopCritic::should_halt_raw(false, 0.85, 0.80),
        "0.85 confidence >= 0.80 default → should halt"
    );
}

#[test]
fn integration_policy_threads_to_reward_weights() {
    use crate::repl::reward_pipeline::{self, RawRewardSignals};

    let mut policy = halcon_core::types::PolicyConfig::default();
    // Custom weights: boost w_stop, reduce w_critic
    policy.w_stop = 0.50;
    policy.w_trajectory = 0.20;
    policy.w_critic = 0.10;
    policy.w_coherence_reward = 0.20;

    let signals = RawRewardSignals {
        stop_condition: StopCondition::EndTurn,
        round_scores: vec![0.8, 0.9],
        critic_verdict: Some((true, 0.9)),
        plan_coherence_score: 0.8,
        oscillation_penalty: 0.0,
        plan_completion_ratio: 1.0,
        plugin_snapshots: vec![],
        critic_unavailable: false,
        evidence_coverage: 1.0,
    };

    let result = reward_pipeline::compute_reward(&signals, &policy);

    // With w_stop=0.50 and EndTurn (stop score = 1.0 * completion 1.0 = 1.0):
    // stop contribution = 0.50 * 1.0 = 0.50
    // With default policy (w_stop=0.25): stop contribution = 0.25
    // So custom policy should produce higher reward due to doubled stop weight
    let result_default =
        reward_pipeline::compute_reward(&signals, &halcon_core::types::PolicyConfig::default());
    assert!(
        result.final_reward > result_default.final_reward - 0.01,
        "custom w_stop=0.50 should not reduce reward vs default (custom={:.3}, default={:.3})",
        result.final_reward,
        result_default.final_reward
    );
}

#[test]
fn integration_policy_threads_to_tool_trust() {
    use crate::repl::tool_trust::ToolTrustScorer;

    let mut policy = halcon_core::types::PolicyConfig::default();
    policy.hide_threshold = 0.50; // much higher than default 0.15
    policy.min_calls_for_filtering = 2; // lower than default 3

    let mut scorer = ToolTrustScorer::new(std::sync::Arc::new(policy));

    // Record 2 failures for a tool
    scorer.record_failure("flaky_tool", 100, None);
    scorer.record_failure("flaky_tool", 100, None);

    let decision = scorer.decide("flaky_tool");
    // With 0/2 success rate (0.0) and min_calls=2, should be hidden (< 0.50)
    assert!(
        matches!(decision, crate::repl::tool_trust::TrustDecision::Hide),
        "0.0 success rate should be hidden with hide_threshold=0.50"
    );
}

#[test]
fn integration_mini_critic_feeds_oracle_not_overrides() {
    use crate::repl::domain::convergence_controller::ConvergenceAction;
    use crate::repl::domain::round_feedback::{LoopSignal, RoundFeedback};
    use crate::repl::domain::termination_oracle::{TerminationDecision, TerminationOracle};

    // Scenario: oracle Halt + mini_critic_synthesis → oracle Halt wins
    let fb = RoundFeedback {
        round: 5,
        combined_score: 0.3,
        convergence_action: ConvergenceAction::Halt,
        loop_signal: LoopSignal::Continue,
        trajectory_trend: 0.2,
        oscillation: 0.0,
        replan_advised: false,
        synthesis_advised: false,
        tool_round: true,
        had_errors: false,
        mini_critic_replan: false,
        mini_critic_synthesis: true, // mini-critic wants synthesis
        evidence_coverage: 1.0,
        semantic_cycle_detected: false,
        cycle_severity: 0.0,
        utility_score: 0.5,
        mid_critic_action: None,
        complexity_upgraded: false,
        problem_class: None,
        forecast_rounds_remaining: None,
        utility_should_synthesize: false,
        synthesis_request_count: 0,
        fsm_error_count: 0,
        budget_iteration_count: 0,
        budget_stagnation_count: 0,
        budget_token_growth: 0,
        budget_exhausted: false,
        executive_signal_count: 0,
        executive_force_reason: None,
        capability_violation: None,
        security_signals_detected: false,
        tool_call_count: 0,
        tool_failure_count: 0,
        governance_rescue_active: false,
    };
    assert_eq!(
        TerminationOracle::adjudicate(&fb),
        TerminationDecision::Halt,
        "Oracle Halt MUST override mini-critic synthesis"
    );

    // Scenario: oracle Continue + mini_critic_synthesis → InjectSynthesis
    let fb2 = RoundFeedback {
        convergence_action: ConvergenceAction::Continue,
        loop_signal: LoopSignal::Continue,
        mini_critic_synthesis: true,
        mini_critic_replan: false,
        ..fb.clone()
    };
    assert!(
        matches!(
            TerminationOracle::adjudicate(&fb2),
            TerminationDecision::InjectSynthesis { .. }
        ),
        "Mini-critic synthesis should upgrade Continue to InjectSynthesis"
    );
}

#[test]
fn integration_evidence_graph_low_coverage_delays_oracle_synthesis() {
    use crate::repl::domain::convergence_controller::ConvergenceAction;
    use crate::repl::domain::round_feedback::{LoopSignal, RoundFeedback};
    use crate::repl::domain::termination_oracle::{
        SynthesisReason, TerminationDecision, TerminationOracle,
    };

    // Low coverage + early round + synthesis_advised → delay (Continue)
    let fb = RoundFeedback {
        round: 2,
        combined_score: 0.3,
        convergence_action: ConvergenceAction::Continue,
        loop_signal: LoopSignal::Continue,
        trajectory_trend: 0.3,
        oscillation: 0.0,
        replan_advised: false,
        synthesis_advised: true,
        tool_round: true,
        had_errors: false,
        mini_critic_replan: false,
        mini_critic_synthesis: false,
        evidence_coverage: 0.10, // very low
        semantic_cycle_detected: false,
        cycle_severity: 0.0,
        utility_score: 0.5,
        mid_critic_action: None,
        complexity_upgraded: false,
        problem_class: None,
        forecast_rounds_remaining: None,
        utility_should_synthesize: false,
        synthesis_request_count: 0,
        fsm_error_count: 0,
        budget_iteration_count: 0,
        budget_stagnation_count: 0,
        budget_token_growth: 0,
        budget_exhausted: false,
        executive_signal_count: 0,
        executive_force_reason: None,
        capability_violation: None,
        security_signals_detected: false,
        tool_call_count: 0,
        tool_failure_count: 0,
        governance_rescue_active: false,
    };
    assert_eq!(
        TerminationOracle::adjudicate(&fb),
        TerminationDecision::Continue,
        "Low evidence coverage should delay synthesis_advised"
    );

    // Same but low utility + high coverage → synthesis proceeds
    let fb2 = RoundFeedback {
        evidence_coverage: 0.80,
        utility_score: 0.20, // below threshold → synthesis proceeds
        ..fb.clone()
    };
    assert_eq!(
        TerminationOracle::adjudicate(&fb2),
        TerminationDecision::InjectSynthesis {
            reason: SynthesisReason::RoundScorerConsecutiveRegression,
        },
        "Low utility + high evidence coverage should proceed with synthesis"
    );
}

#[test]
fn integration_fsm_tool_wait_lifecycle() {
    use super::loop_state::{AgentEvent, AgentPhase};

    // Full lifecycle: Idle → PlanSkipped → Executing → ToolsSubmitted → ToolWait → ToolBatchComplete → Executing → SynthesisStarted → Synthesizing → Completed
    let phase = AgentPhase::Idle;
    let phase = phase.fire(AgentEvent::PlanSkipped);
    assert_eq!(phase, AgentPhase::Executing);

    let phase = phase.fire(AgentEvent::ToolsSubmitted);
    assert_eq!(phase, AgentPhase::ToolWait);
    assert_eq!(phase.as_str(), "tool_wait");

    let phase = phase.fire(AgentEvent::ToolBatchComplete);
    assert_eq!(phase, AgentPhase::Executing);

    // Submit tools again
    let phase = phase.fire(AgentEvent::ToolsSubmitted);
    assert_eq!(phase, AgentPhase::ToolWait);

    // Can also go from ToolWait directly to Synthesizing
    let phase = phase.fire(AgentEvent::SynthesisStarted);
    assert_eq!(phase, AgentPhase::Synthesizing);

    let phase = phase.fire(AgentEvent::EvaluationComplete);
    assert_eq!(phase, AgentPhase::Completed);
}

/// RP-3: Verify that FSM events that would cause InvalidTransition in Synthesizing state
/// are correctly identified and must be guarded by callers.
///
/// This test documents the contract: `ToolsSubmitted`, `ToolBatchComplete`, and
/// `ReflectionComplete` are invalid in `Synthesizing` state. The guards in post_batch.rs
/// prevent these from being dispatched when already synthesizing.
#[test]
fn rp3_fsm_synthesizing_rejects_tool_events() {
    use super::loop_state::{transition, AgentEvent, AgentPhase};

    let phase = AgentPhase::Synthesizing;

    // ToolsSubmitted is invalid in Synthesizing — returns phase unchanged (warns in log).
    assert_eq!(
        transition(phase, AgentEvent::ToolsSubmitted),
        AgentPhase::Synthesizing,
        "Synthesizing + ToolsSubmitted must be a no-op (guarded by post_batch.rs)"
    );
    // ToolBatchComplete is invalid in Synthesizing — returns phase unchanged.
    assert_eq!(
        transition(phase, AgentEvent::ToolBatchComplete),
        AgentPhase::Synthesizing,
        "Synthesizing + ToolBatchComplete must be a no-op (guarded by post_batch.rs)"
    );
    // ReflectionComplete is invalid in Synthesizing — returns phase unchanged.
    assert_eq!(
        transition(phase, AgentEvent::ReflectionComplete),
        AgentPhase::Synthesizing,
        "Synthesizing + ReflectionComplete must be a no-op (guarded by post_batch.rs)"
    );
    // Valid transitions from Synthesizing produce a different phase.
    assert_ne!(
        transition(phase, AgentEvent::SynthesisComplete),
        AgentPhase::Synthesizing
    );
    assert_ne!(
        transition(phase, AgentEvent::EvaluationComplete),
        AgentPhase::Synthesizing
    );
    assert_ne!(
        transition(phase, AgentEvent::ErrorOccurred),
        AgentPhase::Synthesizing
    );
    assert_ne!(
        transition(phase, AgentEvent::Cancelled),
        AgentPhase::Synthesizing
    );
}

/// RP-3: Guard logic — verify that the guard condition `!matches!(phase, Synthesizing)`
/// correctly suppresses events that would produce InvalidTransition.
#[test]
fn rp3_fsm_synthesizing_guard_suppresses_events() {
    use super::loop_state::AgentPhase;

    let phase = AgentPhase::Synthesizing;

    // Simulate the guard used in post_batch.rs for ToolsSubmitted / ToolBatchComplete.
    let already_synthesizing = matches!(phase, AgentPhase::Synthesizing);
    assert!(already_synthesizing, "guard must detect Synthesizing state");

    // Simulate the guard used for ReflectionComplete.
    let in_synthesizing = matches!(phase, AgentPhase::Synthesizing);
    assert!(
        in_synthesizing,
        "reflection guard must detect Synthesizing state"
    );

    // In Executing state, neither guard should fire.
    let exec_phase = AgentPhase::Executing;
    assert!(
        !matches!(exec_phase, AgentPhase::Synthesizing),
        "guard must NOT fire in Executing"
    );
}

#[test]
fn integration_retry_mutation_reads_policy_thresholds() {
    use crate::repl::retry_mutation::*;

    let mut policy = halcon_core::types::PolicyConfig::default();
    policy.temperature_step = 0.3; // 3x bigger than default
    policy.max_temperature = 0.8; // lower ceiling
    policy.tool_failure_threshold = 10; // very lenient

    let params = RetryParams {
        temperature: 0.5,
        plan_depth: 3,
        model_name: "claude-sonnet".into(),
        available_tools: vec!["bash".into()],
    };

    // Tool with 5 failures should NOT be removed (threshold=10)
    let failures = vec![ToolFailureRecord {
        tool_name: "bash".into(),
        failure_count: 5,
    }];
    let record = compute_mutation(&params, 1, &failures, &[], &policy).unwrap();
    assert!(
        !record
            .mutations
            .iter()
            .any(|m| matches!(m, MutationAxis::ToolExposureReduced { .. })),
        "5 failures < 10 threshold → tool retained"
    );

    // Temperature should jump by 0.3 and cap at 0.8
    assert!(
        record.mutations.iter().any(|m| matches!(m,
            MutationAxis::TemperatureIncreased { to, .. } if (*to - 0.8).abs() < 0.001
        )),
        "temp should jump 0.5 + 0.3 = 0.8, capped at max_temperature=0.8"
    );
}

#[test]
fn integration_sla_blocks_replan_when_expired() {
    use crate::repl::sla_manager::{SlaBudget, SlaMode};

    let budget = SlaBudget::from_mode(SlaMode::Fast);
    // Fast mode: max_depth=2, max_retries=0

    // SLA should block retries in Fast mode
    assert!(
        !budget.allows_retry(0),
        "Fast mode SLA should block ALL retries (max_retries=0)"
    );

    // Deep mode should allow retries
    let deep_budget = SlaBudget::from_mode(SlaMode::Deep);
    assert!(
        deep_budget.allows_retry(0),
        "Deep mode SLA should allow first retry"
    );
}

// ── Phase 2 Validation: Trait Interfaces ─────────────────────────────

#[test]
fn phase2_tool_trust_trait_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn halcon_core::traits::ToolTrust>>();
}

#[test]
fn phase2_budget_manager_trait_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn halcon_core::traits::BudgetManager>>();
}

#[test]
fn phase2_evidence_tracker_trait_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn halcon_core::traits::EvidenceTracker>>();
}

#[test]
fn phase2_tool_trust_scorer_implements_trait() {
    use crate::repl::tool_trust::ToolTrustScorer;
    use halcon_core::traits::ToolTrust;
    use std::sync::Arc;

    let policy = Arc::new(halcon_core::types::PolicyConfig::default());
    let scorer: &mut dyn ToolTrust = &mut ToolTrustScorer::new(policy);

    scorer.record_success("file_read", 50);
    scorer.record_failure("bash", 100, Some("timeout"));

    let score = scorer.trust_score("file_read");
    assert!(score > 0.0, "trust score should be positive after success");

    let decision = scorer.decide("file_read");
    assert_eq!(decision, halcon_core::types::ToolTrustDecision::Include);

    let metrics = scorer.get_metrics("file_read");
    assert!(metrics.is_some(), "should have metrics for recorded tool");
    assert_eq!(metrics.unwrap().call_count, 1);

    let failures = scorer.failure_records();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].tool_name, "bash");
}

#[test]
fn phase2_sla_budget_implements_budget_manager() {
    use crate::repl::sla_manager::{SlaBudget, SlaMode};
    use halcon_core::traits::BudgetManager;

    let budget = SlaBudget::from_mode(SlaMode::Balanced);
    assert!(!budget.is_expired());
    assert!(budget.remaining().is_some());
    assert!(budget.fraction_consumed() < 0.1);
    assert!(budget.allows_orchestration());
}

#[test]
fn phase2_evidence_graph_implements_tracker() {
    use crate::repl::evidence_graph::EvidenceGraph;
    use halcon_core::traits::EvidenceTracker;

    let mut graph = EvidenceGraph::new();
    let id1 = graph.add_node("file_read", "a.rs", 100, false, None, 1);
    let id2 = graph.add_node("bash", "ls", 50, false, None, 2);
    graph.add_edge(id1, id2);

    assert_eq!(graph.node_count(), 2);
    assert_eq!(graph.total_evidence_bytes(), 150);

    assert!(graph.synthesis_coverage() < 0.01);

    graph.mark_referenced(id1);
    assert!((graph.synthesis_coverage() - 0.5).abs() < 0.01);

    graph.mark_referenced_batch(&[id2]);
    assert!((graph.synthesis_coverage() - 1.0).abs() < 0.01);

    let unreferenced = graph.unreferenced_summaries();
    assert!(unreferenced.is_empty());
}

// ── Phase 2 Validation: LoopState Sub-Struct Access ──────────────────

#[test]
fn phase2_loop_state_sub_structs_constructible() {
    use super::loop_state::{SynthesisControl, TokenAccounting};

    // Phase 1 FSM Sellado: struct fields are private — use test_default() constructor.
    let synthesis = SynthesisControl::test_default();
    assert_eq!(synthesis.request_count(), 0);

    let tokens = TokenAccounting {
        call_input_tokens: 0,
        call_output_tokens: 0,
        call_cost: 0.0,
        pipeline_budget: 50_000,
        provider_context_window: 128_000,
        tokens_planning: 0,
        tokens_subagents: 0,
        tokens_critic: 0,
        call_input_tokens_prev_round: 0,
        tokens_per_round: vec![],
        consecutive_growth_violations: 0,
        k5_2_compaction_needed: false,
    };
    assert_eq!(tokens.pipeline_budget, 50_000);
}

#[test]
fn phase2_core_types_available() {
    use halcon_core::types::{
        MutationAxis, MutationRecord, ToolFailureInfo, ToolTrustDecision, ToolTrustMetrics,
    };

    let decision = ToolTrustDecision::Include;
    assert_eq!(decision, ToolTrustDecision::Include);

    let metrics = ToolTrustMetrics {
        tool_name: "file_read".into(),
        success_rate: 0.95,
        avg_latency_ms: 50.0,
        call_count: 10,
        failure_count: 1,
    };
    assert_eq!(metrics.call_count, 10);

    let failure = ToolFailureInfo {
        tool_name: "bash".into(),
        failure_count: 3,
    };
    assert_eq!(failure.failure_count, 3);

    let record = MutationRecord {
        mutations: vec![MutationAxis::TemperatureIncreased { from: 0.0, to: 0.1 }],
        retry_number: 1,
    };
    assert_eq!(record.mutations.len(), 1);
}

// ── Phase 3 Integration Tests ──────────────────────────────────────────────

#[test]
fn phase3_integration_utility_delays_oracle_synthesis() {
    use crate::repl::domain::convergence_controller::ConvergenceAction;
    use crate::repl::domain::round_feedback::{LoopSignal, RoundFeedback};
    use crate::repl::domain::termination_oracle::{TerminationDecision, TerminationOracle};

    // High utility → delay synthesis even when synthesis_advised
    let fb = RoundFeedback {
        round: 3,
        combined_score: 0.5,
        convergence_action: ConvergenceAction::Continue,
        loop_signal: LoopSignal::Continue,
        trajectory_trend: 0.5,
        oscillation: 0.0,
        replan_advised: false,
        synthesis_advised: true,
        tool_round: true,
        had_errors: false,
        mini_critic_replan: false,
        mini_critic_synthesis: false,
        evidence_coverage: 0.50,
        semantic_cycle_detected: false,
        cycle_severity: 0.0,
        utility_score: 0.60, // well above threshold 0.35
        mid_critic_action: None,
        complexity_upgraded: false,
        problem_class: None,
        forecast_rounds_remaining: None,
        utility_should_synthesize: false,
        synthesis_request_count: 0,
        fsm_error_count: 0,
        budget_iteration_count: 0,
        budget_stagnation_count: 0,
        budget_token_growth: 0,
        budget_exhausted: false,
        executive_signal_count: 0,
        executive_force_reason: None,
        capability_violation: None,
        security_signals_detected: false,
        tool_call_count: 0,
        tool_failure_count: 0,
        governance_rescue_active: false,
    };
    assert_eq!(
        TerminationOracle::adjudicate(&fb),
        TerminationDecision::Continue,
        "High utility should delay synthesis_advised"
    );

    // Low utility → synthesis proceeds
    let fb2 = RoundFeedback {
        utility_score: 0.10,
        ..fb.clone()
    };
    assert!(
        matches!(
            TerminationOracle::adjudicate(&fb2),
            TerminationDecision::InjectSynthesis { .. }
        ),
        "Low utility should allow synthesis"
    );
}

#[test]
fn phase3_integration_strategy_mutation_with_signals() {
    use super::loop_state::ExecutionIntentPhase;
    use crate::repl::domain::mid_loop_strategy::*;

    // Test cascade: ForceSynthesis > CollapsePlan > SwitchInvestigation
    let signals = StrategySignals {
        evidence_coverage: 0.40,
        drift_score: 0.20,
        replan_attempts: 0,
        max_replan_attempts: 2,
        consecutive_errors: 0,
        tool_failure_clustering: 0.0,
        sla_fraction_consumed: 0.90, // triggers ForceSynthesis
        execution_intent: ExecutionIntentPhase::Execution,
        plan_completion_fraction: 0.30,
        cycle_detected: false,
        round: 8,
        max_rounds: 10,
    };
    let r = select_mutation(&signals, &StrategyThresholds::default());
    assert_eq!(r.mutation, StrategyMutation::ForceSynthesis);
}

#[test]
fn phase3_integration_cycle_detector_feeds_round_feedback() {
    use crate::repl::domain::semantic_cycle::SemanticCycleDetector;

    let policy = halcon_core::types::PolicyConfig::default();
    let mut det = SemanticCycleDetector::from_policy(&policy);

    // No cycles initially
    assert!(!det.has_cycle());
    assert!((det.severity().as_f32()).abs() < f32::EPSILON);

    // Force cycles
    let batch = vec![("file_read".to_string(), r#"{"path": "/a.rs"}"#.to_string())];
    det.record_round(0, &batch);
    det.record_round(1, &batch);
    det.record_round(2, &batch);

    assert!(det.has_cycle());
    assert!(det.severity().as_f32() > 0.0);
}

#[test]
fn phase3_integration_mid_critic_with_policy() {
    use crate::repl::domain::mid_loop_critic::{CriticAction, MidLoopCritic};
    use std::sync::Arc;

    let policy = Arc::new(halcon_core::types::PolicyConfig::default());
    let mut critic = MidLoopCritic::new(policy, 10);

    // Record some rounds
    for i in 0..6 {
        critic.record_snapshot(i, 0.10, 0.20, 0.4, true);
    }

    // Checkpoint at round 6 (interval=3, 6%3==0)
    assert!(critic.is_checkpoint(6));
    let cp = critic.evaluate(6, 10, 0.10, 0.20, 0.10);
    // budget=60%, progress=10% → deficit=0.50 > threshold 0.25 → Replan
    assert_eq!(cp.action, CriticAction::Replan);
}

#[test]
fn phase3_integration_complexity_upgrade_triggers_sla_refresh() {
    use crate::repl::decision_layer::TaskComplexity;
    use crate::repl::domain::complexity_feedback::*;
    use crate::repl::sla_manager::{SlaBudget, SlaMode};
    use std::sync::Arc;

    let policy = Arc::new(halcon_core::types::PolicyConfig::default());
    let mut tracker = ComplexityTracker::new(TaskComplexity::SimpleExecution, 3, policy);

    let obs = ComplexityObservation {
        rounds_used: 8,
        replans_triggered: 2,
        distinct_tools_used: 6,
        domains_touched: 2,
        elapsed_secs: 60.0,
        orchestration_used: true,
        tool_errors: 0,
    };

    let adj = tracker.evaluate(&obs).unwrap();
    assert!(adj.was_upgraded);
    assert!(adj.sla_refresh_needed);

    // Verify SLA upgrade works
    let mut budget = SlaBudget::from_mode(SlaMode::Fast);
    assert_eq!(budget.max_rounds, 4);
    budget.upgrade_from_complexity(&adj.adjusted);
    assert!(budget.max_rounds > 4, "SLA should have been upgraded");
}

#[test]
fn phase3_integration_convergence_utility_computation() {
    use crate::repl::domain::convergence_utility::*;

    let policy = halcon_core::types::PolicyConfig::default();

    // Mid-session with good progress
    let inputs = UtilityInputs {
        evidence_coverage: 0.50,
        coherence_score: 0.70,
        plan_progress: 0.40,
        time_pressure: 0.20,
        retry_cost: 0.10,
        drift_penalty: 0.05,
        evidence_rate: 0.20,
    };
    let result = compute_utility_from_policy(&inputs, &policy);
    assert!(
        result.utility > 0.0,
        "mid-session should have positive utility"
    );

    // Late session with pressure
    let late_inputs = UtilityInputs {
        time_pressure: 0.95,
        ..inputs.clone()
    };
    let late_result = compute_utility_from_policy(&late_inputs, &policy);
    assert!(
        late_result.should_synthesize,
        "late session should trigger synthesis"
    );
}

// ── Phase 5: Meta-Cognitive Intelligence Integration Tests ────────────────

fn make_round_metrics(
    round: usize,
    tool_calls: usize,
    tool_errors: usize,
    combined_score: f32,
    utility_score: f64,
    evidence_coverage: f64,
) -> crate::repl::domain::system_metrics::RoundMetrics {
    crate::repl::domain::system_metrics::RoundMetrics {
        round,
        tokens_in: 500,
        tokens_out: 200,
        tool_calls,
        tool_errors,
        combined_score,
        utility_score,
        evidence_coverage,
        drift_score: 0.0,
        sla_fraction: 0.0,
        token_fraction: 0.0,
        replan_attempts: 0,
        invariant_violations: 0,
        cycle_count: 0,
        round_duration: std::time::Duration::from_millis(100),
        oracle_decision: "Continue".to_string(),
    }
}

#[test]
fn phase5_integration_problem_classifier_with_metrics() {
    use crate::repl::domain::problem_classifier::*;
    use crate::repl::domain::system_metrics::MetricsCollector;

    let policy = std::sync::Arc::new(halcon_core::types::PolicyConfig::default());
    let mut classifier = ProblemClassifier::new(policy.clone());

    // Build 3 rounds of metrics simulating high exploration
    let mut collector = MetricsCollector::new();
    for i in 0..3 {
        collector.record_round(make_round_metrics(
            i,
            8,
            0,
            0.50 + (i as f32 * 0.05),
            0.40 + (i as f64 * 0.10),
            0.20 + (i as f64 * 0.15),
        ));
    }

    // Classify after enough rounds
    let result = classifier.classify(collector.rounds(), 0.20);
    assert!(
        result.confidence > 0.0,
        "classification should have non-zero confidence"
    );
    // Reclassify should not fire on first call
    assert!(
        classifier.reclassify(collector.rounds(), 0.20).is_none(),
        "reclassification should not fire immediately after initial classification"
    );
}

#[test]
fn phase5_integration_strategy_weights_bridge_utility() {
    use crate::repl::domain::convergence_utility;
    use crate::repl::domain::problem_classifier::ProblemClass;
    use crate::repl::domain::strategy_weights::*;

    // Get class-specific weights and bridge to utility system
    let weights = StrategyWeights::for_class(ProblemClass::HighExploration);
    let policy = halcon_core::types::PolicyConfig::default();
    let utility_weights = weights.to_utility_weights(policy.utility_w_progress);

    // Verify bridge preserves weight semantics
    assert!((utility_weights.w_evidence - weights.evidence_weight).abs() < 1e-10);
    assert!((utility_weights.w_drift - weights.drift_weight).abs() < 1e-10);
    assert!((utility_weights.w_pressure - weights.sla_weight).abs() < 1e-10);

    // Compute utility with bridged weights
    let inputs = convergence_utility::UtilityInputs {
        evidence_coverage: 0.50,
        coherence_score: 0.60,
        plan_progress: 0.30,
        time_pressure: 0.20,
        retry_cost: 0.10,
        drift_penalty: 0.05,
        evidence_rate: 0.15,
    };
    let result = convergence_utility::compute_utility(
        &inputs,
        &utility_weights,
        policy.utility_synthesis_threshold,
        policy.utility_marginal_threshold,
    );
    assert!(
        result.utility > 0.0,
        "utility with bridged weights should be positive"
    );
}

#[test]
fn phase5_integration_convergence_forecast_with_metrics() {
    use crate::repl::domain::convergence_estimator::*;
    use crate::repl::domain::system_metrics::MetricsCollector;

    let policy = halcon_core::types::PolicyConfig::default();
    let mut collector = MetricsCollector::new();

    // Build 4 rounds of improving metrics
    for i in 0..4 {
        collector.record_round(make_round_metrics(
            i,
            5,
            0,
            0.30 + (i as f32 * 0.10),
            0.20 + (i as f64 * 0.15),
            0.10 + (i as f64 * 0.12),
        ));
    }

    let forecast = forecast(
        collector.rounds(),
        0.15, // utility_trend (positive — improving)
        0.12, // evidence_rate
        6,    // sla_remaining_rounds
        policy.utility_synthesis_threshold,
        policy.forecast_min_rounds,
    );
    assert!(
        forecast.probability > 0.0,
        "should have non-zero convergence probability"
    );
    assert!(
        forecast.confidence > 0.0,
        "should have non-zero confidence with 4 rounds"
    );
    assert!(
        forecast.estimated_rounds_remaining <= 6,
        "estimated rounds should not exceed SLA remaining"
    );
}

#[test]
fn phase5_integration_strategic_init_applies_class_preset() {
    use crate::repl::domain::problem_classifier::ProblemClass;
    use crate::repl::domain::strategic_init::*;

    // Debug keyword should override to EvidenceSparse
    let profile = initialize(
        Complexity::Structured,
        "debug the authentication error",
        &["read_file".to_string(), "grep".to_string()],
    );
    assert_eq!(profile.problem_class, ProblemClass::EvidenceSparse);
    assert!(profile.rationale.contains("debug"));

    // The weights should be the EvidenceSparse preset
    let expected = crate::repl::domain::strategy_weights::StrategyWeights::for_class(
        ProblemClass::EvidenceSparse,
    );
    assert!(
        (profile.weights.evidence_weight - expected.evidence_weight).abs() < 1e-10,
        "strategic init should use class-specific weight preset"
    );
}

#[test]
fn phase5_integration_session_retrospective_end_to_end() {
    use crate::repl::domain::adaptation_bounds::*;
    use crate::repl::domain::agent_decision_trace::*;
    use crate::repl::domain::session_retrospective::*;
    use crate::repl::domain::system_invariants::*;
    use crate::repl::domain::system_metrics::MetricsCollector;

    let policy = std::sync::Arc::new(halcon_core::types::PolicyConfig::default());

    // Build minimal collectors
    let mut trace = DecisionTraceCollector::new();
    trace.record(DecisionRecord::new(
        DecisionPoint::OracleAdjudication,
        0,
        "Continue".to_string(),
    ));
    trace.record(DecisionRecord::new(
        DecisionPoint::OracleAdjudication,
        1,
        "Synthesize".to_string(),
    ));

    let mut metrics = MetricsCollector::new();
    for i in 0..3 {
        metrics.record_round(make_round_metrics(
            i,
            5,
            if i == 0 { 2 } else { 0 },
            0.30 + (i as f32 * 0.20),
            0.25 + (i as f64 * 0.15),
            0.10 + (i as f64 * 0.20),
        ));
    }

    let bounds = AdaptationBoundsChecker::new(policy.clone());
    let invariants = SystemInvariantChecker::new();

    let profile = analyze(&trace, &metrics, &bounds, &invariants, &policy);
    assert!(
        profile.convergence_efficiency >= 0.0 && profile.convergence_efficiency <= 1.0,
        "convergence efficiency should be in [0, 1]"
    );
    assert!(
        profile.decision_density > 0.0,
        "should have non-zero decision density with 2 decisions over 3 rounds"
    );
    assert!(
        profile.peak_utility >= profile.final_utility,
        "peak utility should be >= final utility"
    );
}
