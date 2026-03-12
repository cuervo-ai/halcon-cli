//! Phase 31 — Real-condition integration stress tests.
//!
//! Tests the agent loop, tool execution pipeline, context management, and
//! failure tracking under realistic conditions.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use serde_json::json;

use halcon_core::error::Result as HalconResult;
use halcon_core::traits::ModelProvider;
use halcon_core::types::ResilienceConfig;
use halcon_core::types::{
    AgentLimits, ChatMessage, ContentBlock, MessageContent, ModelChunk, ModelInfo, ModelRequest,
    Role, RoutingConfig, Session, StopReason, TokenCost, TokenUsage, ToolsConfig,
};
use halcon_tools::ToolRegistry;

use super::accumulator::CompletedToolUse;
use super::agent::{run_agent_loop, AgentContext, StopCondition};
use super::compaction::ContextCompactor;
use super::executor::{self, is_deterministic_error, plan_execution, ToolExecutionConfig};
use super::security::permissions::PermissionChecker;
use super::resilience::ResilienceManager;
use crate::render::sink::{RenderSink, SilentSink};
use halcon_core::types::Phase14Context;

// ── Mock provider: emits a tool call on first invoke, text on second ──

/// Stateful provider that issues a file_read tool call on the first invocation,
/// then responds with text on subsequent invocations. Uses AtomicUsize for
/// safe concurrent access.
struct ToolCallProvider {
    call_count: AtomicUsize,
    models: Vec<ModelInfo>,
}

impl ToolCallProvider {
    fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            models: vec![ModelInfo {
                id: "tool-test".into(),
                name: "Tool Test".into(),
                provider: "tool_test".into(),
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
impl ModelProvider for ToolCallProvider {
    fn name(&self) -> &str {
        "tool_test"
    }

    fn supported_models(&self) -> &[ModelInfo] {
        &self.models
    }

    async fn invoke(
        &self,
        _request: &ModelRequest,
    ) -> HalconResult<BoxStream<'static, HalconResult<ModelChunk>>> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        };

        if count == 0 {
            // First call: emit a tool_use (file_read on a temp file).
            let chunks: Vec<HalconResult<ModelChunk>> = vec![
                Ok(ModelChunk::ToolUseStart {
                    index: 0,
                    id: "tu_stress_1".into(),
                    name: "file_read".into(),
                }),
                Ok(ModelChunk::ToolUseDelta {
                    index: 0,
                    partial_json: json!({"path": "/tmp/stress_test_31.txt"}).to_string(),
                }),
                Ok(ModelChunk::Usage(usage)),
                Ok(ModelChunk::Done(StopReason::ToolUse)),
            ];
            Ok(Box::pin(stream::iter(chunks)))
        } else {
            // Subsequent calls: respond with text.
            let chunks: Vec<HalconResult<ModelChunk>> = vec![
                Ok(ModelChunk::TextDelta("Done with tool.".into())),
                Ok(ModelChunk::Usage(usage)),
                Ok(ModelChunk::Done(StopReason::EndTurn)),
            ];
            Ok(Box::pin(stream::iter(chunks)))
        }
    }

    async fn is_available(&self) -> bool {
        true
    }

    fn estimate_cost(&self, _request: &ModelRequest) -> TokenCost {
        TokenCost::default()
    }
}

// ── Test helpers ──

fn test_event_tx() -> (halcon_core::EventSender, halcon_core::EventReceiver) {
    halcon_core::event_bus(64)
}

// NOTE: TEST_SINK removed (FASE H / Parallelism Hardening).
// SilentSink.stream_reset() calls self.text.lock().unwrap().clear() — clearing
// the shared text buffer for all concurrent tests.  Box::leak per test_ctx call.
// See agent/tests.rs comment for full rationale.

static TEST_PLANNING_CONFIG: std::sync::LazyLock<halcon_core::types::PlanningConfig> =
    std::sync::LazyLock::new(halcon_core::types::PlanningConfig::default);

static TEST_ORCH_CONFIG: std::sync::LazyLock<halcon_core::types::OrchestratorConfig> =
    std::sync::LazyLock::new(halcon_core::types::OrchestratorConfig::default);

// NOTE: TEST_SPECULATOR removed (FASE H / Parallelism Hardening).
// See agent/tests.rs comment for full rationale. test_ctx now uses
// Box::leak(Box::new(ToolSpeculator::new())) for per-call isolation.

static TEST_SECURITY_CONFIG: std::sync::LazyLock<halcon_core::types::SecurityConfig> =
    std::sync::LazyLock::new(halcon_core::types::SecurityConfig::default);

fn make_tool_registry() -> ToolRegistry {
    let config = ToolsConfig {
        allowed_directories: vec!["/tmp".into(), "/private/tmp".into()],
        ..ToolsConfig::default()
    };
    halcon_tools::default_registry(&config)
}

fn test_resilience() -> ResilienceManager {
    ResilienceManager::new(ResilienceConfig::default())
}

fn make_request(model: &str) -> ModelRequest {
    ModelRequest {
        model: model.into(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Text("hello".into()),
        }],
        tools: vec![],
        max_tokens: Some(1024),
        temperature: Some(0.0),
        system: None,
        stream: true,
    }
}

fn test_ctx<'a>(
    provider: &'a Arc<dyn ModelProvider>,
    session: &'a mut Session,
    request: &'a ModelRequest,
    tool_registry: &'a ToolRegistry,
    permissions: &'a mut super::conversational_permission::ConversationalPermissionHandler,
    event_tx: &'a halcon_core::EventSender,
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
        render_sink: Box::leak(Box::new(SilentSink::new())),
        replay_tool_executor: None,
        phase14: Phase14Context::default(),
        model_selector: None,
        registry: None,
        episode_id: None,
        planning_config: &*TEST_PLANNING_CONFIG,
        orchestrator_config: &*TEST_ORCH_CONFIG,
        tool_selection_enabled: false,
        task_bridge: None,
        context_metrics: None,
        context_manager: None,
        ctrl_rx: None,
        speculator: Box::leak(Box::new(super::tool_speculation::ToolSpeculator::new())),
        security_config: &*TEST_SECURITY_CONFIG,
        strategy_context: None,
        critic_provider: None,
        critic_model: None,
        plugin_registry: None,
        is_sub_agent: false,
        requested_provider: None,
        policy: std::sync::Arc::new(halcon_core::types::PolicyConfig::default()),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// B-1: Multi-round tool conversation
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn multi_round_tool_conversation() {
    // Create a temp file so file_read has something to read.
    // NOTE: content must be >= MIN_EVIDENCE_BYTES (30) so the EBS gate does not fire.
    // EBS-B2 fires when text_bytes_extracted < 30 AND content-read tools were attempted,
    // which would incorrectly intercept the model's legitimate EndTurn response.
    std::fs::write(
        "/tmp/stress_test_31.txt",
        "stress content for the multi-round tool conversation test",
    )
    .unwrap();

    let provider: Arc<dyn ModelProvider> = Arc::new(ToolCallProvider::new());
    let mut session = Session::new("tool_test".into(), "tool-test".into(), "/tmp".into());
    let request = make_request("tool-test");
    let tool_reg = make_tool_registry();
    let mut perms = super::conversational_permission::ConversationalPermissionHandler::new(true);
    perms.set_non_interactive();
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

    // rounds counts tool-use rounds. 1 tool-use round + final text round.
    assert!(
        result.rounds >= 1,
        "Expected at least 1 tool-use round, got {}",
        result.rounds
    );
    assert_eq!(result.stop_condition, StopCondition::EndTurn);
    assert!(
        result.full_text.contains("Done with tool"),
        "Expected final text from provider, got: {}",
        result.full_text
    );

    std::fs::remove_file("/tmp/stress_test_31.txt").ok();
}

// ═══════════════════════════════════════════════════════════════════════
// B-2: Parallel tool batch (10 concurrent file_reads)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn parallel_tool_batch_10_concurrent() {
    // Create 10 temp files.
    for i in 0..10 {
        std::fs::write(format!("/tmp/stress_par_{i}.txt"), format!("content_{i}")).unwrap();
    }

    let tool_reg = make_tool_registry();
    let (event_tx, _rx) = test_event_tx();
    let exec_config = ToolExecutionConfig::default();
    let mut trace_step = 0u32;
    let session_id = uuid::Uuid::new_v4();

    let batch: Vec<CompletedToolUse> = (0..10)
        .map(|i| CompletedToolUse {
            id: format!("tu_par_{i}"),
            name: "file_read".into(),
            input: json!({"path": format!("/tmp/stress_par_{i}.txt")}),
        })
        .collect();

    let batch_sink = SilentSink::new(); // fresh per-call sink (FASE H)
    let results = executor::execute_parallel_batch(
        &batch,
        &tool_reg,
        "/tmp",
        Duration::from_secs(10),
        &event_tx,
        None,
        session_id,
        &mut trace_step,
        10,
        &exec_config,
        &batch_sink,
        None,
    )
    .await;

    assert_eq!(results.len(), 10, "All 10 tool results returned");

    // Verify each result contains content.
    for (i, res) in results.iter().enumerate() {
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = &res.content_block
        {
            assert!(!is_error, "file_read {i} should not error");
            assert!(
                content.contains(&format!("content_{i}")) || content.contains("content_"),
                "Result {i} should contain file content"
            );
        } else {
            panic!("Expected ToolResult for result {i}");
        }
    }

    // Cleanup.
    for i in 0..10 {
        std::fs::remove_file(format!("/tmp/stress_par_{i}.txt")).ok();
    }
}

// ═══════════════════════════════════════════════════════════════════════
// B-3: Large tool output truncation
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn large_tool_output_truncation() {
    // Create a 150KB file.
    let large_content = "x".repeat(150_000);
    std::fs::write("/tmp/stress_large.txt", &large_content).unwrap();

    let tool_reg = make_tool_registry();
    let (event_tx, _rx) = test_event_tx();
    let exec_config = ToolExecutionConfig::default();
    let mut trace_step = 0u32;
    let session_id = uuid::Uuid::new_v4();

    let batch = vec![CompletedToolUse {
        id: "tu_large".into(),
        name: "file_read".into(),
        input: json!({"path": "/tmp/stress_large.txt"}),
    }];

    let batch_sink = SilentSink::new(); // fresh per-call sink (FASE H)
    let results = executor::execute_parallel_batch(
        &batch,
        &tool_reg,
        "/tmp",
        Duration::from_secs(10),
        &event_tx,
        None,
        session_id,
        &mut trace_step,
        1,
        &exec_config,
        &batch_sink,
        None,
    )
    .await;

    assert_eq!(results.len(), 1);
    if let ContentBlock::ToolResult { content, .. } = &results[0].content_block {
        // file_read itself may truncate or return the full content.
        // The important thing is it doesn't panic and returns something.
        assert!(!content.is_empty(), "Result should have content");
    }

    std::fs::remove_file("/tmp/stress_large.txt").ok();
}

// ═══════════════════════════════════════════════════════════════════════
// B-4: Context pipeline cascade with tight budget
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn context_pipeline_cascade_tight_budget() {
    use halcon_context::pipeline::{ContextPipeline, ContextPipelineConfig};

    let config = ContextPipelineConfig {
        max_context_tokens: 500,
        hot_buffer_capacity: 4,
        default_tool_output_budget: 100,
        l1_merge_threshold: 200,
        max_cold_entries: 10,
        ..Default::default()
    };

    let mut pipeline = ContextPipeline::new(&config);

    // Push 50+ messages to force L0→L1→L2 cascade.
    for i in 0..60 {
        pipeline.add_message(ChatMessage {
            role: if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
            content: MessageContent::Text(format!(
                "Message number {i} with some padding text here"
            )),
        });
    }

    // L0 should have at most hot_buffer_capacity messages.
    assert!(pipeline.l0().len() <= 4, "L0 should be bounded by capacity");
    // L1 should have some segments.
    assert!(!pipeline.l1().is_empty(), "L1 should have evicted segments");

    // build_messages should not panic and should return valid messages.
    let built = pipeline.build_messages();
    assert!(!built.is_empty(), "build_messages should return messages");

    // Verify no corruption: all messages should have valid content.
    for msg in &built {
        match &msg.content {
            MessageContent::Text(t) => assert!(!t.is_empty()),
            MessageContent::Blocks(blocks) => assert!(!blocks.is_empty()),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// B-5: Fallback provider cost tracking
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fallback_provider_cost_tracking() {
    use halcon_core::types::{BackpressureConfig, CircuitBreakerConfig};

    /// Provider with known non-zero cost for verification.
    struct CostProvider {
        provider_name: String,
        cost: f64,
        inner: halcon_providers::EchoProvider,
    }

    impl CostProvider {
        fn new(name: &str, cost: f64) -> Self {
            Self {
                provider_name: name.into(),
                cost,
                inner: halcon_providers::EchoProvider::new(),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for CostProvider {
        fn name(&self) -> &str {
            &self.provider_name
        }
        fn supported_models(&self) -> &[ModelInfo] {
            self.inner.supported_models()
        }
        async fn invoke(
            &self,
            request: &ModelRequest,
        ) -> HalconResult<BoxStream<'static, HalconResult<ModelChunk>>> {
            self.inner.invoke(request).await
        }
        async fn is_available(&self) -> bool {
            true
        }
        fn estimate_cost(&self, _request: &ModelRequest) -> TokenCost {
            TokenCost {
                estimated_input_tokens: 50,
                estimated_cost_usd: self.cost,
            }
        }
        fn validate_model(&self, model: &str) -> HalconResult<()> {
            if model == "echo" {
                Ok(())
            } else {
                self.inner.validate_model(model)
            }
        }
    }

    let primary: Arc<dyn ModelProvider> = Arc::new(CostProvider::new("fb_primary", 0.10));
    let fallback: Arc<dyn ModelProvider> = Arc::new(CostProvider::new("fb_fallback", 0.25));
    let mut session = Session::new("fb_primary".into(), "echo".into(), "/tmp".into());
    let request = make_request("echo");
    let tool_reg = ToolRegistry::new();
    let mut perms = super::conversational_permission::ConversationalPermissionHandler::new(true);
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
    resilience.register_provider("fb_primary");
    resilience.register_provider("fb_fallback");
    resilience.record_failure("fb_primary").await;

    let fallbacks = vec![("fb_fallback".to_string(), fallback)];
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

    // Cost should reflect fallback pricing (0.25), NOT primary (0.10).
    assert!(
        (session.estimated_cost_usd - 0.25).abs() < 0.01,
        "Expected fallback cost ~0.25, got {}",
        session.estimated_cost_usd
    );
}

// ═══════════════════════════════════════════════════════════════════════
// B-6: Compaction protocol integrity
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn compaction_protocol_integrity() {
    use halcon_core::types::CompactionConfig;

    let compactor = ContextCompactor::new(CompactionConfig {
        enabled: true,
        threshold_fraction: 0.8,
        keep_recent: 4,
        max_context_tokens: 200_000,
    });

    // Build 20 messages with alternating tool_use/tool_result pairs.
    let mut messages = Vec::new();
    for i in 0..10 {
        let tu_id = format!("tu_{i}");
        // User text.
        messages.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(format!("Request {i}")),
        });
        // Assistant with tool_use.
        messages.push(ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: tu_id.clone(),
                name: "file_read".into(),
                input: json!({"path": "/tmp/test.txt"}),
            }]),
        });
        // User with tool_result.
        messages.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: tu_id,
                content: "ok".into(),
                is_error: false,
            }]),
        });
        // Assistant text response.
        messages.push(ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text(format!("Response {i}")),
        });
    }

    assert_eq!(messages.len(), 40);

    // Force compaction.
    compactor.apply_compaction(&mut messages, "Summary of previous tool interactions");

    // After compaction: summary + recent messages.
    // Verify no orphaned ToolResults (each ToolResult has a matching ToolUse).
    let mut tool_use_ids = std::collections::HashSet::new();
    let mut tool_result_ids = std::collections::HashSet::new();

    for msg in &messages {
        if let MessageContent::Blocks(blocks) = &msg.content {
            for block in blocks {
                match block {
                    ContentBlock::ToolUse { id, .. } => {
                        tool_use_ids.insert(id.clone());
                    }
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        tool_result_ids.insert(tool_use_id.clone());
                    }
                    _ => {}
                }
            }
        }
    }

    // Every ToolResult should reference a ToolUse that exists.
    for result_id in &tool_result_ids {
        assert!(
            tool_use_ids.contains(result_id),
            "Orphaned ToolResult: {} has no matching ToolUse",
            result_id
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// B-7: Tool failure tracker circuit breaker
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn tool_failure_tracker_circuit_breaker() {
    use super::failure_tracker::ToolFailureTracker;

    let mut tracker = ToolFailureTracker::new(3);

    // Record 2 failures — should NOT trip.
    assert!(!tracker.record("file_read", "No such file or directory: /foo"));
    assert!(!tracker.record("file_read", "No such file or directory: /bar"));

    // 3rd failure with same pattern — should trip.
    assert!(tracker.record("file_read", "No such file or directory: /baz"));

    // Verify it's now tripped.
    assert!(tracker.is_tripped("file_read", "not found anywhere"));

    // Different tool should NOT be tripped.
    assert!(!tracker.is_tripped("bash", "not found"));
}

// ═══════════════════════════════════════════════════════════════════════
// B-8: Planning gate — unsupported model
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn planning_gate_unsupported_model() {
    use super::planner::LlmPlanner;
    use halcon_core::traits::Planner;

    let provider: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let planner = LlmPlanner::new(provider, "nonexistent-model-xyz".into());

    // supports_model should return false.
    assert!(
        !planner.supports_model(),
        "Planner should not support nonexistent model"
    );

    // With a valid model it should return true.
    let provider2: Arc<dyn ModelProvider> = Arc::new(halcon_providers::EchoProvider::new());
    let planner2 = LlmPlanner::new(provider2, "echo".into());
    assert!(
        planner2.supports_model(),
        "Planner should support 'echo' model"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// B-9: Mixed permission tools partition
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn mixed_permission_tools_partition() {
    let tool_reg = make_tool_registry();

    let tools = vec![
        // ReadOnly tools (should go parallel).
        CompletedToolUse {
            id: "t1".into(),
            name: "file_read".into(),
            input: json!({}),
        },
        CompletedToolUse {
            id: "t2".into(),
            name: "grep".into(),
            input: json!({}),
        },
        CompletedToolUse {
            id: "t3".into(),
            name: "glob".into(),
            input: json!({}),
        },
        // Destructive tools (should go sequential).
        CompletedToolUse {
            id: "t4".into(),
            name: "bash".into(),
            input: json!({}),
        },
        CompletedToolUse {
            id: "t5".into(),
            name: "file_write".into(),
            input: json!({}),
        },
    ];

    let plan = plan_execution(tools, &tool_reg);

    // ReadOnly tools should be in parallel batch.
    assert_eq!(
        plan.parallel_batch.len(),
        3,
        "3 ReadOnly tools should be parallel"
    );
    let parallel_names: Vec<&str> = plan
        .parallel_batch
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(parallel_names.contains(&"file_read"));
    assert!(parallel_names.contains(&"grep"));
    assert!(parallel_names.contains(&"glob"));

    // Destructive/ReadWrite tools should be in sequential batch.
    assert_eq!(
        plan.sequential_batch.len(),
        2,
        "2 non-ReadOnly tools should be sequential"
    );
    let seq_names: Vec<&str> = plan
        .sequential_batch
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(seq_names.contains(&"bash"));
    assert!(seq_names.contains(&"file_write"));
}

// ═══════════════════════════════════════════════════════════════════════
// B-10: Empty and malformed tool results
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn empty_malformed_tool_results() {
    let tool_reg = make_tool_registry();
    let (event_tx, _rx) = test_event_tx();
    let exec_config = ToolExecutionConfig::default();
    let mut trace_step = 0u32;
    let session_id = uuid::Uuid::new_v4();

    let batch = vec![
        // Nonexistent file.
        CompletedToolUse {
            id: "t1".into(),
            name: "file_read".into(),
            input: json!({"path": "/tmp/nonexistent_stress_31.txt"}),
        },
        // _parse_error poison args.
        CompletedToolUse {
            id: "t2".into(),
            name: "file_read".into(),
            input: json!({"_parse_error": "malformed JSON from model"}),
        },
        // Unknown tool name.
        CompletedToolUse {
            id: "t3".into(),
            name: "nonexistent_tool_xyz".into(),
            input: json!({}),
        },
        // Empty bash command (should be rejected by input validation).
        CompletedToolUse {
            id: "t4".into(),
            name: "bash".into(),
            input: json!({"command": ""}),
        },
    ];

    let batch_sink = SilentSink::new(); // fresh per-call sink (FASE H)
    let results = executor::execute_parallel_batch(
        &batch,
        &tool_reg,
        "/tmp",
        Duration::from_secs(10),
        &event_tx,
        None,
        session_id,
        &mut trace_step,
        4,
        &exec_config,
        &batch_sink,
        None,
    )
    .await;

    assert_eq!(results.len(), 4, "All 4 tool calls should return results");

    // All should be errors or contain error messages.
    for res in &results {
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = &res.content_block
        {
            // These are all error cases — content should indicate failure.
            assert!(
                *is_error
                    || content.to_lowercase().contains("error")
                    || content.contains("not found")
                    || content.contains("unknown tool")
                    || content.contains("_parse_error")
                    || content.contains("empty")
                    || content.contains("invalid"),
                "Expected error content for {}, got: {}",
                res.tool_name,
                &content[..content.len().min(200)]
            );
        }
    }

    // Verify is_deterministic_error correctly classifies these.
    assert!(is_deterministic_error("No such file or directory"));
    assert!(is_deterministic_error("unknown tool 'foo'"));
    assert!(!is_deterministic_error("connection refused")); // transient
}
