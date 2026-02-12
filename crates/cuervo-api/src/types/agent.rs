use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Agent kind mirrors runtime AgentKind but is serialization-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Llm,
    Mcp,
    CliProcess,
    HttpEndpoint,
    CuervoRemote,
    Plugin,
}

/// Health status for an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum HealthStatus {
    Healthy,
    Degraded { reason: String },
    Unavailable { reason: String },
    Unknown,
}

/// Full agent information returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: Uuid,
    pub name: String,
    pub kind: AgentKind,
    pub capabilities: Vec<String>,
    pub protocols: Vec<String>,
    pub health: HealthStatus,
    pub registered_at: DateTime<Utc>,
    pub last_invoked: Option<DateTime<Utc>>,
    pub invocation_count: u64,
    pub max_concurrency: usize,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Request to spawn a new agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentRequest {
    pub name: String,
    pub kind: AgentKind,
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Response after spawning an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentResponse {
    pub id: Uuid,
    pub name: String,
}

/// Request to invoke an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeAgentRequest {
    pub instruction: String,
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
    pub budget: Option<BudgetSpec>,
    pub timeout_ms: Option<u64>,
}

/// Budget constraints for agent invocation or task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetSpec {
    #[serde(default)]
    pub max_tokens: u64,
    #[serde(default)]
    pub max_cost_usd: f64,
    #[serde(default)]
    pub max_duration_ms: u64,
}

/// Response from agent invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeAgentResponse {
    pub request_id: Uuid,
    pub success: bool,
    pub output: String,
    pub artifacts: Vec<ArtifactInfo>,
    pub usage: UsageInfo,
}

/// Artifact produced by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactInfo {
    pub kind: String,
    pub name: String,
    pub content: String,
}

/// Token/cost usage information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageInfo {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub latency_ms: u64,
    pub rounds: usize,
}

/// Agent health detail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHealthDetail {
    pub id: Uuid,
    pub name: String,
    pub health: HealthStatus,
    pub failure_count: u32,
    pub success_count: u32,
    pub last_check: Option<DateTime<Utc>>,
}

/// Query parameters for listing agents.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListAgentsQuery {
    pub kind: Option<AgentKind>,
    pub capability: Option<String>,
    pub health: Option<String>,
}
