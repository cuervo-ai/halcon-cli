use cuervo_api::error::{ApiError, ErrorCode};
use cuervo_api::types::agent::*;
use cuervo_api::types::observability::*;
use cuervo_api::types::protocol::*;
use cuervo_api::types::system::*;
use cuervo_api::types::task::*;
use cuervo_api::types::tool::*;
use cuervo_api::types::ws::*;
use std::collections::HashMap;
use uuid::Uuid;

// ── Agent type serialization ─────────────────────────

#[test]
fn agent_kind_roundtrip() {
    let kinds = [
        AgentKind::Llm,
        AgentKind::Mcp,
        AgentKind::CliProcess,
        AgentKind::HttpEndpoint,
        AgentKind::CuervoRemote,
        AgentKind::Plugin,
    ];
    for kind in &kinds {
        let json = serde_json::to_string(kind).unwrap();
        let back: AgentKind = serde_json::from_str(&json).unwrap();
        assert_eq!(*kind, back);
    }
}

#[test]
fn health_status_serialization() {
    let healthy = HealthStatus::Healthy;
    let json = serde_json::to_string(&healthy).unwrap();
    assert!(json.contains("\"status\":\"healthy\""));

    let degraded = HealthStatus::Degraded {
        reason: "high latency".into(),
    };
    let json = serde_json::to_string(&degraded).unwrap();
    assert!(json.contains("\"status\":\"degraded\""));
    assert!(json.contains("high latency"));

    let back: HealthStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(degraded, back);
}

#[test]
fn agent_info_full_roundtrip() {
    let info = AgentInfo {
        id: Uuid::new_v4(),
        name: "test-agent".into(),
        kind: AgentKind::Llm,
        capabilities: vec!["CodeGeneration".into(), "CodeReview".into()],
        protocols: vec!["Native".into()],
        health: HealthStatus::Healthy,
        registered_at: chrono::Utc::now(),
        last_invoked: None,
        invocation_count: 42,
        max_concurrency: 4,
        metadata: HashMap::new(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let back: AgentInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(info.id, back.id);
    assert_eq!(info.name, back.name);
    assert_eq!(info.invocation_count, back.invocation_count);
}

#[test]
fn invoke_agent_request_serialization() {
    let req = InvokeAgentRequest {
        instruction: "review this code".into(),
        context: HashMap::new(),
        budget: Some(BudgetSpec {
            max_tokens: 1000,
            max_cost_usd: 0.05,
            max_duration_ms: 30000,
        }),
        timeout_ms: Some(60000),
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: InvokeAgentRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.instruction, "review this code");
    assert_eq!(back.budget.unwrap().max_tokens, 1000);
}

#[test]
fn usage_info_default() {
    let usage = UsageInfo::default();
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.output_tokens, 0);
    assert_eq!(usage.cost_usd, 0.0);
    assert_eq!(usage.rounds, 0);
}

// ── Task type serialization ─────────────────────────

#[test]
fn task_status_roundtrip() {
    let statuses = [
        TaskStatus::Pending,
        TaskStatus::Running,
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
    ];
    for status in &statuses {
        let json = serde_json::to_string(status).unwrap();
        let back: TaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(*status, back);
    }
}

#[test]
fn submit_task_request_serialization() {
    let req = SubmitTaskRequest {
        nodes: vec![TaskNodeSpec {
            task_id: Uuid::new_v4(),
            instruction: "generate tests".into(),
            agent_selector: AgentSelectorSpec {
                by_id: None,
                by_capability: Some(vec!["Testing".into()]),
                by_kind: None,
                by_name: None,
            },
            depends_on: vec![],
            budget: None,
            context_keys: vec!["source_code".into()],
        }],
        context: HashMap::new(),
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("generate tests"));
    let back: SubmitTaskRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.nodes.len(), 1);
    assert_eq!(back.nodes[0].context_keys, vec!["source_code"]);
}

#[test]
fn task_progress_event_serialization() {
    let event = TaskProgressEvent::WaveStarted {
        execution_id: Uuid::new_v4(),
        wave: 0,
        node_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"event\":\"wave_started\""));
    let back: TaskProgressEvent = serde_json::from_str(&json).unwrap();
    match back {
        TaskProgressEvent::WaveStarted { wave, node_ids, .. } => {
            assert_eq!(wave, 0);
            assert_eq!(node_ids.len(), 2);
        }
        _ => panic!("wrong variant"),
    }
}

// ── Tool type serialization ─────────────────────────

#[test]
fn permission_level_roundtrip() {
    let levels = [
        PermissionLevel::ReadOnly,
        PermissionLevel::ReadWrite,
        PermissionLevel::Destructive,
    ];
    for level in &levels {
        let json = serde_json::to_string(level).unwrap();
        let back: PermissionLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(*level, back);
    }
}

#[test]
fn tool_info_serialization() {
    let tool = ToolInfo {
        name: "file_read".into(),
        description: "Read file contents".into(),
        permission_level: PermissionLevel::ReadOnly,
        enabled: true,
        requires_confirmation: false,
        execution_count: 100,
        last_executed: Some(chrono::Utc::now()),
        input_schema: serde_json::json!({"type": "object"}),
    };
    let json = serde_json::to_string(&tool).unwrap();
    let back: ToolInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(back.name, "file_read");
    assert!(back.enabled);
    assert_eq!(back.execution_count, 100);
}

// ── Observability type serialization ─────────────────

#[test]
fn log_level_ordering() {
    assert!(LogLevel::Trace < LogLevel::Debug);
    assert!(LogLevel::Debug < LogLevel::Info);
    assert!(LogLevel::Info < LogLevel::Warn);
    assert!(LogLevel::Warn < LogLevel::Error);
}

#[test]
fn log_entry_serialization() {
    let entry = LogEntry {
        timestamp: chrono::Utc::now(),
        level: LogLevel::Info,
        target: "cuervo_runtime::agent".into(),
        message: "agent invoked".into(),
        fields: HashMap::new(),
        span: Some("agent_loop".into()),
    };
    let json = serde_json::to_string(&entry).unwrap();
    let back: LogEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back.level, LogLevel::Info);
    assert_eq!(back.message, "agent invoked");
}

#[test]
fn metrics_snapshot_serialization() {
    let snapshot = MetricsSnapshot {
        timestamp: chrono::Utc::now(),
        agent_count: 5,
        tool_count: 23,
        total_invocations: 150,
        total_tool_executions: 1200,
        total_input_tokens: 500000,
        total_output_tokens: 250000,
        total_cost_usd: 2.50,
        uptime_seconds: 3600,
        active_tasks: 3,
        completed_tasks: 47,
        failed_tasks: 2,
        events_per_second: 45.5,
        agent_metrics: vec![],
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    let back: MetricsSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(back.agent_count, 5);
    assert_eq!(back.total_cost_usd, 2.50);
}

// ── Protocol type serialization ─────────────────────

#[test]
fn protocol_message_serialization() {
    let msg = ProtocolMessageInfo {
        id: Uuid::new_v4(),
        protocol: ProtocolType::Federation,
        direction: MessageDirection::Outbound,
        from_agent: Some(Uuid::new_v4()),
        to_agent: Some(Uuid::new_v4()),
        message_type: "DelegateTask".into(),
        timestamp: chrono::Utc::now(),
        payload_size_bytes: 256,
        payload: serde_json::json!({"instruction": "test"}),
        latency_ms: Some(12),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: ProtocolMessageInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(back.protocol, ProtocolType::Federation);
    assert_eq!(back.direction, MessageDirection::Outbound);
    assert_eq!(back.payload_size_bytes, 256);
}

// ── System type serialization ───────────────────────

#[test]
fn system_status_serialization() {
    let status = SystemStatus {
        version: "0.1.0".into(),
        started_at: chrono::Utc::now(),
        uptime_seconds: 7200,
        agent_count: 3,
        tool_count: 15,
        active_tasks: 1,
        health: SystemHealth::Healthy,
        platform: PlatformInfo {
            os: "macos".into(),
            arch: "aarch64".into(),
            rust_version: "1.80".into(),
            pid: 1234,
            memory_usage_bytes: 52_000_000,
        },
    };
    let json = serde_json::to_string(&status).unwrap();
    let back: SystemStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back.uptime_seconds, 7200);
    assert_eq!(back.health, SystemHealth::Healthy);
    assert_eq!(back.platform.os, "macos");
}

#[test]
fn shutdown_request_serialization() {
    let req = ShutdownRequest {
        graceful: true,
        reason: Some("maintenance".into()),
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: ShutdownRequest = serde_json::from_str(&json).unwrap();
    assert!(back.graceful);
    assert_eq!(back.reason.unwrap(), "maintenance");
}

// ── WebSocket message serialization ─────────────────

#[test]
fn ws_client_message_subscribe() {
    let msg = WsClientMessage::Subscribe {
        channels: vec![WsChannel::Agents, WsChannel::Tasks],
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"subscribe\""));
    let back: WsClientMessage = serde_json::from_str(&json).unwrap();
    match back {
        WsClientMessage::Subscribe { channels } => assert_eq!(channels.len(), 2),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn ws_server_event_agent_registered() {
    let agent = AgentInfo {
        id: Uuid::new_v4(),
        name: "test".into(),
        kind: AgentKind::Llm,
        capabilities: vec![],
        protocols: vec![],
        health: HealthStatus::Healthy,
        registered_at: chrono::Utc::now(),
        last_invoked: None,
        invocation_count: 0,
        max_concurrency: 1,
        metadata: HashMap::new(),
    };
    let event = WsServerEvent::AgentRegistered { agent };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"agent_registered\""));
}

#[test]
fn ws_server_event_tool_executed() {
    let event = WsServerEvent::ToolExecuted {
        name: "grep".into(),
        tool_use_id: "tu_123".into(),
        duration_ms: 45,
        success: true,
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: WsServerEvent = serde_json::from_str(&json).unwrap();
    match back {
        WsServerEvent::ToolExecuted {
            name, duration_ms, ..
        } => {
            assert_eq!(name, "grep");
            assert_eq!(duration_ms, 45);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn ws_channel_all_variants() {
    let channels = [
        WsChannel::Agents,
        WsChannel::Tasks,
        WsChannel::Tools,
        WsChannel::Logs,
        WsChannel::Metrics,
        WsChannel::Protocols,
        WsChannel::System,
        WsChannel::All,
    ];
    for ch in &channels {
        let json = serde_json::to_string(ch).unwrap();
        let back: WsChannel = serde_json::from_str(&json).unwrap();
        assert_eq!(*ch, back);
    }
}

// ── Error type tests ────────────────────────────────

#[test]
fn api_error_status_codes() {
    assert_eq!(ApiError::bad_request("test").status_code(), 400);
    assert_eq!(ApiError::unauthorized("test").status_code(), 401);
    assert_eq!(ApiError::not_found("test").status_code(), 404);
    assert_eq!(ApiError::internal("test").status_code(), 500);
    assert_eq!(ApiError::runtime("test").status_code(), 500);
    assert_eq!(ApiError::timeout("test").status_code(), 504);
}

#[test]
fn api_error_serialization() {
    let err = ApiError::not_found("agent xyz not found");
    let json = serde_json::to_string(&err).unwrap();
    let back: ApiError = serde_json::from_str(&json).unwrap();
    assert_eq!(back.code, ErrorCode::NotFound);
    assert_eq!(back.message, "agent xyz not found");
}

#[test]
fn api_error_display() {
    let err = ApiError::bad_request("invalid input");
    let display = format!("{err}");
    assert!(display.contains("BadRequest"));
    assert!(display.contains("invalid input"));
}

// ── Constants ───────────────────────────────────────

#[test]
fn api_constants() {
    assert_eq!(cuervo_api::API_VERSION, "v1");
    assert_eq!(cuervo_api::DEFAULT_PORT, 9849);
    assert_eq!(cuervo_api::DEFAULT_BIND, "127.0.0.1");
}
