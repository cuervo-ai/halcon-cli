//! Core agent abstraction layer.
//!
//! Defines the universal `RuntimeAgent` trait that abstracts over LLM agents,
//! MCP servers, CLI tools, HTTP endpoints, and remote runtimes.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;

/// Universal trait for any agent in the runtime.
///
/// Implementations exist for LLM agents, MCP servers, CLI processes,
/// HTTP endpoints, and remote Cuervo instances.
#[async_trait]
pub trait RuntimeAgent: Send + Sync {
    /// Static descriptor for this agent (identity, capabilities, protocols).
    fn descriptor(&self) -> &AgentDescriptor;

    /// Invoke this agent with a request.
    async fn invoke(&self, request: AgentRequest) -> Result<AgentResponse>;

    /// Check the health of this agent.
    async fn health(&self) -> AgentHealth;

    /// Gracefully shut down this agent.
    async fn shutdown(&self) -> Result<()>;
}

/// Static metadata describing an agent's identity and capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDescriptor {
    /// Unique identifier for this agent instance.
    pub id: Uuid,
    /// Human-readable name.
    pub name: String,
    /// What kind of agent this is.
    pub agent_kind: AgentKind,
    /// Capabilities this agent provides.
    pub capabilities: Vec<AgentCapability>,
    /// Communication protocols this agent supports.
    pub protocols: Vec<ProtocolSupport>,
    /// Arbitrary metadata (version, provider info, etc.).
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Maximum concurrent invocations this agent supports.
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
}

fn default_max_concurrency() -> usize {
    1
}

/// The kind of system backing this agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    /// LLM-based agent (uses model provider + tool loop).
    Llm,
    /// MCP server (tool provider via Model Context Protocol).
    Mcp,
    /// CLI process (command-line tool wrapped as agent).
    CliProcess,
    /// HTTP/REST API endpoint.
    HttpEndpoint,
    /// Remote Cuervo instance.
    CuervoRemote,
    /// Dynamically loaded plugin.
    Plugin,
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentKind::Llm => write!(f, "llm"),
            AgentKind::Mcp => write!(f, "mcp"),
            AgentKind::CliProcess => write!(f, "cli_process"),
            AgentKind::HttpEndpoint => write!(f, "http_endpoint"),
            AgentKind::CuervoRemote => write!(f, "cuervo_remote"),
            AgentKind::Plugin => write!(f, "plugin"),
        }
    }
}

impl FromStr for AgentKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "llm" => Ok(AgentKind::Llm),
            "mcp" => Ok(AgentKind::Mcp),
            "cli_process" => Ok(AgentKind::CliProcess),
            "http_endpoint" => Ok(AgentKind::HttpEndpoint),
            "cuervo_remote" => Ok(AgentKind::CuervoRemote),
            "plugin" => Ok(AgentKind::Plugin),
            _ => Err(format!("unknown agent kind: {s}")),
        }
    }
}

/// A capability that an agent can provide.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    CodeGeneration,
    CodeReview,
    FileOperations,
    WebSearch,
    ShellExecution,
    Planning,
    Research,
    Testing,
    Custom(String),
}

impl fmt::Display for AgentCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentCapability::CodeGeneration => write!(f, "code_generation"),
            AgentCapability::CodeReview => write!(f, "code_review"),
            AgentCapability::FileOperations => write!(f, "file_operations"),
            AgentCapability::WebSearch => write!(f, "web_search"),
            AgentCapability::ShellExecution => write!(f, "shell_execution"),
            AgentCapability::Planning => write!(f, "planning"),
            AgentCapability::Research => write!(f, "research"),
            AgentCapability::Testing => write!(f, "testing"),
            AgentCapability::Custom(s) => write!(f, "custom:{s}"),
        }
    }
}

impl FromStr for AgentCapability {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "code_generation" => Ok(AgentCapability::CodeGeneration),
            "code_review" => Ok(AgentCapability::CodeReview),
            "file_operations" => Ok(AgentCapability::FileOperations),
            "web_search" => Ok(AgentCapability::WebSearch),
            "shell_execution" => Ok(AgentCapability::ShellExecution),
            "planning" => Ok(AgentCapability::Planning),
            "research" => Ok(AgentCapability::Research),
            "testing" => Ok(AgentCapability::Testing),
            other => {
                if let Some(name) = other.strip_prefix("custom:") {
                    Ok(AgentCapability::Custom(name.to_string()))
                } else {
                    Err(format!("unknown capability: {other}"))
                }
            }
        }
    }
}

/// Communication protocol an agent supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolSupport {
    McpStdio,
    McpHttp,
    JsonRpc,
    Rest,
    Grpc,
    Native,
}

impl fmt::Display for ProtocolSupport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolSupport::McpStdio => write!(f, "mcp_stdio"),
            ProtocolSupport::McpHttp => write!(f, "mcp_http"),
            ProtocolSupport::JsonRpc => write!(f, "json_rpc"),
            ProtocolSupport::Rest => write!(f, "rest"),
            ProtocolSupport::Grpc => write!(f, "grpc"),
            ProtocolSupport::Native => write!(f, "native"),
        }
    }
}

impl FromStr for ProtocolSupport {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "mcp_stdio" => Ok(ProtocolSupport::McpStdio),
            "mcp_http" => Ok(ProtocolSupport::McpHttp),
            "json_rpc" => Ok(ProtocolSupport::JsonRpc),
            "rest" => Ok(ProtocolSupport::Rest),
            "grpc" => Ok(ProtocolSupport::Grpc),
            "native" => Ok(ProtocolSupport::Native),
            _ => Err(format!("unknown protocol: {s}")),
        }
    }
}

/// A request to an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    /// Unique identifier for this request.
    pub request_id: Uuid,
    /// Natural-language instruction or task description.
    pub instruction: String,
    /// Contextual data (previous results, shared state, etc.).
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
    /// Restrict which capabilities the agent may use.
    #[serde(default)]
    pub allowed_capabilities: Option<Vec<AgentCapability>>,
    /// Budget constraints for this invocation.
    #[serde(default)]
    pub budget: Option<AgentBudget>,
    /// Maximum wall-clock time for this invocation.
    #[serde(default)]
    pub timeout: Option<Duration>,
}

impl AgentRequest {
    /// Create a simple request with just an instruction.
    pub fn new(instruction: impl Into<String>) -> Self {
        Self {
            request_id: Uuid::new_v4(),
            instruction: instruction.into(),
            context: HashMap::new(),
            allowed_capabilities: None,
            budget: None,
            timeout: None,
        }
    }
}

/// The response from an agent invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// Echoes the request_id for correlation.
    pub request_id: Uuid,
    /// Whether the invocation was successful.
    pub success: bool,
    /// Primary text output from the agent.
    pub output: String,
    /// Structured artifacts produced (files, diffs, logs, etc.).
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    /// Resource usage for this invocation.
    pub usage: AgentUsage,
    /// Additional metadata from the agent.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// A structured artifact produced by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// What kind of artifact this is.
    pub kind: ArtifactKind,
    /// Optional file path associated with this artifact.
    #[serde(default)]
    pub path: Option<String>,
    /// The artifact content.
    pub content: String,
}

/// The kind of artifact produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    File,
    Diff,
    Log,
    Report,
    Custom(String),
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArtifactKind::File => write!(f, "file"),
            ArtifactKind::Diff => write!(f, "diff"),
            ArtifactKind::Log => write!(f, "log"),
            ArtifactKind::Report => write!(f, "report"),
            ArtifactKind::Custom(s) => write!(f, "custom:{s}"),
        }
    }
}

/// Resource usage for an agent invocation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub latency_ms: u64,
    pub rounds: usize,
}

impl AgentUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Merge another usage into this one (additive).
    pub fn merge(&mut self, other: &AgentUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cost_usd += other.cost_usd;
        self.latency_ms += other.latency_ms;
        self.rounds += other.rounds;
    }
}

/// Budget constraints for an agent invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBudget {
    /// Maximum total tokens (input + output). 0 = unlimited.
    pub max_tokens: u64,
    /// Maximum cost in USD. 0.0 = unlimited.
    pub max_cost_usd: f64,
    /// Maximum wall-clock duration.
    pub max_duration: Duration,
}

impl AgentBudget {
    /// Check if a given usage exceeds this budget.
    pub fn is_exceeded(&self, usage: &AgentUsage) -> bool {
        if self.max_tokens > 0 && usage.total_tokens() > self.max_tokens {
            return true;
        }
        if self.max_cost_usd > 0.0 && usage.cost_usd > self.max_cost_usd {
            return true;
        }
        false
    }
}

impl Default for AgentBudget {
    fn default() -> Self {
        Self {
            max_tokens: 0,
            max_cost_usd: 0.0,
            max_duration: Duration::from_secs(300),
        }
    }
}

/// Health status of an agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum AgentHealth {
    Healthy,
    Degraded { reason: String },
    Unavailable { reason: String },
}

impl AgentHealth {
    pub fn is_healthy(&self) -> bool {
        matches!(self, AgentHealth::Healthy)
    }

    pub fn is_available(&self) -> bool {
        !matches!(self, AgentHealth::Unavailable { .. })
    }
}

impl fmt::Display for AgentHealth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentHealth::Healthy => write!(f, "healthy"),
            AgentHealth::Degraded { reason } => write!(f, "degraded: {reason}"),
            AgentHealth::Unavailable { reason } => write!(f, "unavailable: {reason}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_descriptor() -> AgentDescriptor {
        AgentDescriptor {
            id: Uuid::new_v4(),
            name: "test-agent".to_string(),
            agent_kind: AgentKind::Llm,
            capabilities: vec![AgentCapability::CodeGeneration, AgentCapability::FileOperations],
            protocols: vec![ProtocolSupport::Native],
            metadata: HashMap::new(),
            max_concurrency: 3,
        }
    }

    // --- AgentDescriptor tests ---

    #[test]
    fn descriptor_construction() {
        let desc = test_descriptor();
        assert_eq!(desc.name, "test-agent");
        assert_eq!(desc.agent_kind, AgentKind::Llm);
        assert_eq!(desc.capabilities.len(), 2);
        assert_eq!(desc.max_concurrency, 3);
    }

    #[test]
    fn descriptor_serde_roundtrip() {
        let desc = test_descriptor();
        let json = serde_json::to_string(&desc).unwrap();
        let parsed: AgentDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, desc.name);
        assert_eq!(parsed.agent_kind, desc.agent_kind);
        assert_eq!(parsed.capabilities.len(), desc.capabilities.len());
    }

    #[test]
    fn descriptor_with_metadata() {
        let mut desc = test_descriptor();
        desc.metadata
            .insert("version".to_string(), serde_json::json!("1.0.0"));
        let json = serde_json::to_string(&desc).unwrap();
        let parsed: AgentDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.metadata["version"], "1.0.0");
    }

    #[test]
    fn descriptor_default_max_concurrency() {
        let json = r#"{"id":"00000000-0000-0000-0000-000000000001","name":"t","agent_kind":"llm","capabilities":[],"protocols":[]}"#;
        let desc: AgentDescriptor = serde_json::from_str(json).unwrap();
        assert_eq!(desc.max_concurrency, 1);
    }

    #[test]
    fn descriptor_custom_capability_serde() {
        let desc = AgentDescriptor {
            id: Uuid::new_v4(),
            name: "custom".to_string(),
            agent_kind: AgentKind::Plugin,
            capabilities: vec![AgentCapability::Custom("image_gen".to_string())],
            protocols: vec![],
            metadata: HashMap::new(),
            max_concurrency: 1,
        };
        let json = serde_json::to_string(&desc).unwrap();
        let parsed: AgentDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.capabilities[0],
            AgentCapability::Custom("image_gen".to_string())
        );
    }

    // --- AgentRequest/Response tests ---

    #[test]
    fn request_new_simple() {
        let req = AgentRequest::new("write tests");
        assert_eq!(req.instruction, "write tests");
        assert!(req.context.is_empty());
        assert!(req.budget.is_none());
        assert!(req.timeout.is_none());
    }

    #[test]
    fn request_serde_roundtrip() {
        let mut req = AgentRequest::new("fix bug");
        req.context
            .insert("file".to_string(), serde_json::json!("main.rs"));
        req.budget = Some(AgentBudget {
            max_tokens: 10000,
            max_cost_usd: 1.0,
            max_duration: Duration::from_secs(60),
        });
        let json = serde_json::to_string(&req).unwrap();
        let parsed: AgentRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.instruction, "fix bug");
        assert_eq!(parsed.context["file"], "main.rs");
        assert!(parsed.budget.is_some());
    }

    #[test]
    fn response_serde_roundtrip() {
        let resp = AgentResponse {
            request_id: Uuid::new_v4(),
            success: true,
            output: "Done".to_string(),
            artifacts: vec![Artifact {
                kind: ArtifactKind::File,
                path: Some("src/main.rs".to_string()),
                content: "fn main() {}".to_string(),
            }],
            usage: AgentUsage {
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: 0.001,
                latency_ms: 500,
                rounds: 2,
            },
            metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: AgentResponse = serde_json::from_str(&json).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.output, "Done");
        assert_eq!(parsed.artifacts.len(), 1);
        assert_eq!(parsed.usage.rounds, 2);
    }

    #[test]
    fn response_with_multiple_artifacts() {
        let resp = AgentResponse {
            request_id: Uuid::new_v4(),
            success: true,
            output: "Generated files".to_string(),
            artifacts: vec![
                Artifact {
                    kind: ArtifactKind::File,
                    path: Some("a.rs".to_string()),
                    content: "// a".to_string(),
                },
                Artifact {
                    kind: ArtifactKind::Diff,
                    path: Some("b.rs".to_string()),
                    content: "+line".to_string(),
                },
                Artifact {
                    kind: ArtifactKind::Log,
                    path: None,
                    content: "log entry".to_string(),
                },
            ],
            usage: AgentUsage::default(),
            metadata: HashMap::new(),
        };
        assert_eq!(resp.artifacts.len(), 3);
        assert_eq!(resp.artifacts[0].kind, ArtifactKind::File);
        assert_eq!(resp.artifacts[1].kind, ArtifactKind::Diff);
        assert_eq!(resp.artifacts[2].kind, ArtifactKind::Log);
    }

    // --- AgentHealth tests ---

    #[test]
    fn health_variants() {
        assert!(AgentHealth::Healthy.is_healthy());
        assert!(AgentHealth::Healthy.is_available());

        let degraded = AgentHealth::Degraded {
            reason: "slow".to_string(),
        };
        assert!(!degraded.is_healthy());
        assert!(degraded.is_available());

        let unavailable = AgentHealth::Unavailable {
            reason: "down".to_string(),
        };
        assert!(!unavailable.is_healthy());
        assert!(!unavailable.is_available());
    }

    #[test]
    fn health_display() {
        assert_eq!(AgentHealth::Healthy.to_string(), "healthy");
        assert_eq!(
            AgentHealth::Degraded {
                reason: "slow".into()
            }
            .to_string(),
            "degraded: slow"
        );
        assert_eq!(
            AgentHealth::Unavailable {
                reason: "down".into()
            }
            .to_string(),
            "unavailable: down"
        );
    }

    #[test]
    fn health_serde_roundtrip() {
        let values = vec![
            AgentHealth::Healthy,
            AgentHealth::Degraded {
                reason: "slow".into(),
            },
            AgentHealth::Unavailable {
                reason: "down".into(),
            },
        ];
        for v in values {
            let json = serde_json::to_string(&v).unwrap();
            let parsed: AgentHealth = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, v);
        }
    }

    // --- AgentBudget tests ---

    #[test]
    fn budget_default() {
        let budget = AgentBudget::default();
        assert_eq!(budget.max_tokens, 0);
        assert_eq!(budget.max_cost_usd, 0.0);
        assert_eq!(budget.max_duration, Duration::from_secs(300));
    }

    #[test]
    fn budget_not_exceeded() {
        let budget = AgentBudget {
            max_tokens: 1000,
            max_cost_usd: 1.0,
            max_duration: Duration::from_secs(60),
        };
        let usage = AgentUsage {
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.01,
            ..Default::default()
        };
        assert!(!budget.is_exceeded(&usage));
    }

    #[test]
    fn budget_exceeded_by_tokens() {
        let budget = AgentBudget {
            max_tokens: 100,
            max_cost_usd: 0.0,
            max_duration: Duration::from_secs(60),
        };
        let usage = AgentUsage {
            input_tokens: 80,
            output_tokens: 30,
            ..Default::default()
        };
        assert!(budget.is_exceeded(&usage));
    }

    #[test]
    fn budget_exceeded_by_cost() {
        let budget = AgentBudget {
            max_tokens: 0,
            max_cost_usd: 0.5,
            max_duration: Duration::from_secs(60),
        };
        let usage = AgentUsage {
            cost_usd: 0.6,
            ..Default::default()
        };
        assert!(budget.is_exceeded(&usage));
    }

    #[test]
    fn budget_unlimited_never_exceeded() {
        let budget = AgentBudget::default(); // all zeros = unlimited
        let usage = AgentUsage {
            input_tokens: 999999,
            output_tokens: 999999,
            cost_usd: 999.0,
            ..Default::default()
        };
        assert!(!budget.is_exceeded(&usage));
    }

    // --- AgentUsage tests ---

    #[test]
    fn usage_total_tokens() {
        let usage = AgentUsage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        };
        assert_eq!(usage.total_tokens(), 150);
    }

    #[test]
    fn usage_merge() {
        let mut a = AgentUsage {
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.01,
            latency_ms: 500,
            rounds: 2,
        };
        let b = AgentUsage {
            input_tokens: 200,
            output_tokens: 100,
            cost_usd: 0.02,
            latency_ms: 300,
            rounds: 3,
        };
        a.merge(&b);
        assert_eq!(a.input_tokens, 300);
        assert_eq!(a.output_tokens, 150);
        assert!((a.cost_usd - 0.03).abs() < 0.0001);
        assert_eq!(a.latency_ms, 800);
        assert_eq!(a.rounds, 5);
    }

    // --- AgentKind Display + FromStr ---

    #[test]
    fn agent_kind_display() {
        assert_eq!(AgentKind::Llm.to_string(), "llm");
        assert_eq!(AgentKind::Mcp.to_string(), "mcp");
        assert_eq!(AgentKind::CliProcess.to_string(), "cli_process");
        assert_eq!(AgentKind::HttpEndpoint.to_string(), "http_endpoint");
        assert_eq!(AgentKind::CuervoRemote.to_string(), "cuervo_remote");
        assert_eq!(AgentKind::Plugin.to_string(), "plugin");
    }

    #[test]
    fn agent_kind_from_str() {
        assert_eq!(AgentKind::from_str("llm").unwrap(), AgentKind::Llm);
        assert_eq!(AgentKind::from_str("mcp").unwrap(), AgentKind::Mcp);
        assert_eq!(
            AgentKind::from_str("cli_process").unwrap(),
            AgentKind::CliProcess
        );
        assert!(AgentKind::from_str("unknown").is_err());
    }

    // --- ProtocolSupport Display + FromStr ---

    #[test]
    fn protocol_display() {
        assert_eq!(ProtocolSupport::McpStdio.to_string(), "mcp_stdio");
        assert_eq!(ProtocolSupport::Rest.to_string(), "rest");
        assert_eq!(ProtocolSupport::Native.to_string(), "native");
    }

    #[test]
    fn protocol_from_str() {
        assert_eq!(
            ProtocolSupport::from_str("mcp_stdio").unwrap(),
            ProtocolSupport::McpStdio
        );
        assert_eq!(
            ProtocolSupport::from_str("rest").unwrap(),
            ProtocolSupport::Rest
        );
        assert!(ProtocolSupport::from_str("unknown").is_err());
    }

    // --- AgentCapability Display + FromStr ---

    #[test]
    fn capability_display() {
        assert_eq!(AgentCapability::CodeGeneration.to_string(), "code_generation");
        assert_eq!(AgentCapability::WebSearch.to_string(), "web_search");
        assert_eq!(
            AgentCapability::Custom("foo".into()).to_string(),
            "custom:foo"
        );
    }

    #[test]
    fn capability_from_str() {
        assert_eq!(
            AgentCapability::from_str("code_generation").unwrap(),
            AgentCapability::CodeGeneration
        );
        assert_eq!(
            AgentCapability::from_str("custom:image_gen").unwrap(),
            AgentCapability::Custom("image_gen".to_string())
        );
        assert!(AgentCapability::from_str("bogus").is_err());
    }

    // --- Artifact tests ---

    #[test]
    fn artifact_construction() {
        let a = Artifact {
            kind: ArtifactKind::File,
            path: Some("test.rs".to_string()),
            content: "hello".to_string(),
        };
        assert_eq!(a.kind, ArtifactKind::File);
        assert_eq!(a.path.as_deref(), Some("test.rs"));
    }

    #[test]
    fn artifact_kind_display() {
        assert_eq!(ArtifactKind::File.to_string(), "file");
        assert_eq!(ArtifactKind::Diff.to_string(), "diff");
        assert_eq!(ArtifactKind::Report.to_string(), "report");
        assert_eq!(
            ArtifactKind::Custom("chart".into()).to_string(),
            "custom:chart"
        );
    }

    #[test]
    fn artifact_serde_roundtrip() {
        let a = Artifact {
            kind: ArtifactKind::Custom("diagram".to_string()),
            path: None,
            content: "graph TD".to_string(),
        };
        let json = serde_json::to_string(&a).unwrap();
        let parsed: Artifact = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.kind, ArtifactKind::Custom("diagram".to_string()));
        assert!(parsed.path.is_none());
    }

    // --- Mock RuntimeAgent ---

    struct MockAgent {
        descriptor: AgentDescriptor,
        healthy: bool,
    }

    #[async_trait]
    impl RuntimeAgent for MockAgent {
        fn descriptor(&self) -> &AgentDescriptor {
            &self.descriptor
        }

        async fn invoke(&self, request: AgentRequest) -> Result<AgentResponse> {
            Ok(AgentResponse {
                request_id: request.request_id,
                success: true,
                output: format!("Mock processed: {}", request.instruction),
                artifacts: vec![],
                usage: AgentUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cost_usd: 0.0001,
                    latency_ms: 1,
                    rounds: 1,
                },
                metadata: HashMap::new(),
            })
        }

        async fn health(&self) -> AgentHealth {
            if self.healthy {
                AgentHealth::Healthy
            } else {
                AgentHealth::Unavailable {
                    reason: "mock down".to_string(),
                }
            }
        }

        async fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn mock_agent_invoke() {
        let agent = MockAgent {
            descriptor: test_descriptor(),
            healthy: true,
        };
        let req = AgentRequest::new("hello");
        let resp = agent.invoke(req).await.unwrap();
        assert!(resp.success);
        assert!(resp.output.contains("Mock processed: hello"));
    }

    #[tokio::test]
    async fn mock_agent_health_healthy() {
        let agent = MockAgent {
            descriptor: test_descriptor(),
            healthy: true,
        };
        assert_eq!(agent.health().await, AgentHealth::Healthy);
    }

    #[tokio::test]
    async fn mock_agent_health_unavailable() {
        let agent = MockAgent {
            descriptor: test_descriptor(),
            healthy: false,
        };
        assert!(!agent.health().await.is_available());
    }

    #[tokio::test]
    async fn mock_agent_shutdown() {
        let agent = MockAgent {
            descriptor: test_descriptor(),
            healthy: true,
        };
        assert!(agent.shutdown().await.is_ok());
    }

    #[tokio::test]
    async fn mock_agent_descriptor() {
        let agent = MockAgent {
            descriptor: test_descriptor(),
            healthy: true,
        };
        assert_eq!(agent.descriptor().name, "test-agent");
        assert_eq!(agent.descriptor().agent_kind, AgentKind::Llm);
    }

    // --- Edge cases ---

    #[test]
    fn empty_capabilities_descriptor() {
        let desc = AgentDescriptor {
            id: Uuid::new_v4(),
            name: "empty".to_string(),
            agent_kind: AgentKind::Plugin,
            capabilities: vec![],
            protocols: vec![],
            metadata: HashMap::new(),
            max_concurrency: 1,
        };
        assert!(desc.capabilities.is_empty());
        let json = serde_json::to_string(&desc).unwrap();
        let parsed: AgentDescriptor = serde_json::from_str(&json).unwrap();
        assert!(parsed.capabilities.is_empty());
    }

    #[test]
    fn request_with_all_fields() {
        let req = AgentRequest {
            request_id: Uuid::new_v4(),
            instruction: "complex task".to_string(),
            context: {
                let mut m = HashMap::new();
                m.insert("key".to_string(), serde_json::json!(42));
                m
            },
            allowed_capabilities: Some(vec![AgentCapability::CodeGeneration]),
            budget: Some(AgentBudget {
                max_tokens: 5000,
                max_cost_usd: 0.5,
                max_duration: Duration::from_secs(30),
            }),
            timeout: Some(Duration::from_secs(120)),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: AgentRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.instruction, "complex task");
        assert!(parsed.allowed_capabilities.is_some());
        assert!(parsed.budget.is_some());
    }
}
