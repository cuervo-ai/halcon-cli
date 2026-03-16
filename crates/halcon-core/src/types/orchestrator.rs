use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::agent::{AgentResult, AgentType};
use super::config::AgentLimits;

// DECISION: AgentRole is a separate enum (not part of SubAgentTask permission flags)
// because roles affect BEHAVIOR (timeout multiplier, tool access, context scope)
// while permissions affect CAPABILITY (which tools are callable).
// Mixing them in a single flags field would make the semantics ambiguous.
// See US-agent-roles (PASO 4-B).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentRole {
    /// Full access, can read all teammate states, can cancel teammates.
    Lead,
    /// Receives initial context from lead, stricter limits, timeout × 0.6.
    Teammate,
    /// Invoked on demand by lead, domain-scoped (e.g. "security specialist").
    Specialist,
    /// Audit-only: no tool execution, records all events.
    Observer,
}

impl AgentRole {
    /// Timeout multiplier applied to the base session timeout.
    pub fn timeout_multiplier(&self) -> f64 {
        match self {
            AgentRole::Lead => 1.0,
            AgentRole::Teammate => 0.6,
            AgentRole::Specialist => 0.8,
            AgentRole::Observer => 0.1, // short — just observe
        }
    }

    /// Maximum rounds multiplier.
    pub fn max_rounds_multiplier(&self) -> f64 {
        match self {
            AgentRole::Lead => 1.0,
            AgentRole::Teammate => 0.7,
            AgentRole::Specialist => 0.5,
            AgentRole::Observer => 0.0, // never runs rounds
        }
    }

    /// Whether this role may execute tools.
    pub fn can_execute_tools(&self) -> bool {
        !matches!(self, AgentRole::Observer)
    }

    /// Whether this role may cancel other agents in the same team.
    pub fn can_cancel_teammates(&self) -> bool {
        matches!(self, AgentRole::Lead)
    }
}

impl Default for AgentRole {
    fn default() -> Self {
        AgentRole::Lead
    }
}

/// A sub-agent task to be executed by the orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentTask {
    /// Unique identifier for this task.
    pub task_id: Uuid,
    /// Natural-language instruction for the sub-agent.
    pub instruction: String,
    /// Type of agent to use for this task.
    pub agent_type: AgentType,
    /// Override model for this sub-agent (None = inherit parent).
    #[serde(default)]
    pub model: Option<String>,
    /// Override provider for this sub-agent (None = inherit parent).
    #[serde(default)]
    pub provider: Option<String>,
    /// Restrict tool access for this sub-agent (empty = inherit all).
    #[serde(default)]
    pub allowed_tools: HashSet<String>,
    /// Override execution limits (None = derive from parent).
    #[serde(default)]
    pub limits_override: Option<AgentLimits>,
    /// Task IDs that must complete before this task can start.
    #[serde(default)]
    pub depends_on: Vec<Uuid>,
    /// Priority within a wave (higher = run first). Default 0.
    #[serde(default)]
    pub priority: u32,
    /// Optional system prompt prefix injected by the agent registry (Feature 4).
    /// Contains combined skill bodies + agent body from the .md definition file.
    #[serde(default)]
    pub system_prompt_prefix: Option<String>,
    /// Role of this agent within its team (Lead / Teammate / Specialist / Observer).
    /// Affects timeout multiplier, max_rounds cap, and tool execution eligibility.
    #[serde(default)]
    pub role: AgentRole,
    /// Team this agent belongs to (used to scope mailbox messages and shared context).
    #[serde(default)]
    pub team_id: Option<Uuid>,
    /// Mailbox instance ID for this agent's message queue.
    #[serde(default)]
    pub mailbox_id: Option<Uuid>,
    /// Planner-estimated token cost for this task (0 = unknown).
    ///
    /// When > 0 and `limits_override` is None, the orchestrator will construct a
    /// `limits_override` that caps `max_total_tokens` to this estimate.  This prevents
    /// small write-file tasks from consuming the same budget as large analysis tasks.
    ///
    /// The planner is responsible for setting this field based on instruction length,
    /// task type, and complexity score.  See `estimate_task_tokens()` in planning logic.
    #[serde(default)]
    pub estimated_tokens: u32,
}

impl Default for SubAgentTask {
    fn default() -> Self {
        Self {
            task_id: Uuid::new_v4(),
            instruction: String::new(),
            agent_type: AgentType::Chat,
            model: None,
            provider: None,
            allowed_tools: std::collections::HashSet::new(),
            limits_override: None,
            depends_on: Vec::new(),
            priority: 0,
            system_prompt_prefix: None,
            role: AgentRole::default(),
            team_id: None,
            mailbox_id: None,
            estimated_tokens: 0,
        }
    }
}

/// Result of a single sub-agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentResult {
    /// Task ID that produced this result.
    pub task_id: Uuid,
    /// Whether the sub-agent completed successfully.
    pub success: bool,
    /// All text generated by the sub-agent.
    pub output_text: String,
    /// Structured agent result (summary, files modified, tools used).
    pub agent_result: AgentResult,
    /// Total input tokens consumed.
    pub input_tokens: u64,
    /// Total output tokens consumed.
    pub output_tokens: u64,
    /// Estimated cost in USD.
    pub cost_usd: f64,
    /// Wall-clock latency in milliseconds.
    pub latency_ms: u64,
    /// Number of agent loop rounds.
    pub rounds: usize,
    /// Error message if the sub-agent failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Whether the sub-agent produced verified evidence (evidence gate did not fire).
    #[serde(default)]
    pub evidence_verified: bool,
    /// Number of content-read tool attempts made by the sub-agent.
    #[serde(default)]
    pub content_read_attempts: usize,
    /// Whether the sub-agent had at least one tool definition available.
    /// False for synthetic/skipped results and empty-registry test scenarios.
    /// Guards the zero-tool-drift detector so it does not fire on tool-less tasks.
    #[serde(default = "default_had_tools_true")]
    pub had_tools_available: bool,
}

fn default_had_tools_true() -> bool { true }

/// Aggregated result from a complete orchestrator run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorResult {
    /// Unique identifier for this orchestrator run.
    pub orchestrator_id: Uuid,
    /// Results from each sub-agent.
    pub sub_results: Vec<SubAgentResult>,
    /// Total input tokens across all sub-agents.
    pub total_input_tokens: u64,
    /// Total output tokens across all sub-agents.
    pub total_output_tokens: u64,
    /// Total estimated cost across all sub-agents.
    pub total_cost_usd: f64,
    /// Total wall-clock latency (elapsed, not sum of sub-agents).
    pub total_latency_ms: u64,
    /// Number of sub-agents that succeeded.
    pub success_count: usize,
    /// Total number of sub-agents (includes skipped).
    pub total_count: usize,
    /// True when the shared token budget was exhausted before all tasks completed.
    /// When true, `skipped_count` indicates how many tasks were not started.
    #[serde(default)]
    pub budget_exceeded: bool,
    /// Number of tasks that were dropped due to budget exhaustion (intra-wave or
    /// pre-wave). Does not include dependency-cascade skips.
    #[serde(default)]
    pub skipped_count: usize,
}

impl OrchestratorResult {
    /// Build an aggregated result from sub-agent results.
    pub fn from_results(
        orchestrator_id: Uuid,
        sub_results: Vec<SubAgentResult>,
        total_latency_ms: u64,
        budget_exceeded: bool,
        skipped_count: usize,
    ) -> Self {
        let success_count = sub_results.iter().filter(|r| r.success).count();
        let total_count = sub_results.len() + skipped_count;
        let total_input_tokens = sub_results.iter().map(|r| r.input_tokens).sum();
        let total_output_tokens = sub_results.iter().map(|r| r.output_tokens).sum();
        let total_cost_usd = sub_results.iter().map(|r| r.cost_usd).sum();

        Self {
            orchestrator_id,
            sub_results,
            total_input_tokens,
            total_output_tokens,
            total_cost_usd,
            total_latency_ms,
            success_count,
            total_count,
            budget_exceeded,
            skipped_count,
        }
    }
}

/// Orchestrator configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// Enable the multi-agent orchestrator. Default: true.
    #[serde(default = "default_enabled_true")]
    pub enabled: bool,
    /// Maximum number of sub-agents running concurrently within a wave.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_agents: usize,
    /// Timeout in seconds for each sub-agent. 0 = inherit from parent limits.
    #[serde(default)]
    pub sub_agent_timeout_secs: u64,
    /// Share the parent's token budget across all sub-agents.
    #[serde(default = "default_shared_budget")]
    pub shared_budget: bool,
    /// Enable inter-agent communication channels.
    #[serde(default)]
    pub enable_communication: bool,
    /// Minimum step confidence to consider for delegation. Default: 0.7.
    #[serde(default = "default_min_delegation_confidence")]
    pub min_delegation_confidence: f64,
}

fn default_enabled_true() -> bool {
    true
}

fn default_max_concurrent() -> usize {
    3
}

fn default_shared_budget() -> bool {
    true
}

fn default_min_delegation_confidence() -> f64 {
    0.7
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_concurrent_agents: default_max_concurrent(),
            sub_agent_timeout_secs: 0,
            shared_budget: default_shared_budget(),
            enable_communication: false,
            min_delegation_confidence: default_min_delegation_confidence(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn orchestrator_config_defaults() {
        let config = OrchestratorConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_concurrent_agents, 3);
        assert_eq!(config.sub_agent_timeout_secs, 0);
        assert!(config.shared_budget);
    }

    #[test]
    fn orchestrator_config_serde_round_trip() {
        let config = OrchestratorConfig {
            enabled: true,
            max_concurrent_agents: 5,
            sub_agent_timeout_secs: 120,
            shared_budget: false,
            enable_communication: false,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: OrchestratorConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.max_concurrent_agents, 5);
        assert_eq!(parsed.sub_agent_timeout_secs, 120);
        assert!(!parsed.shared_budget);
    }

    #[test]
    fn orchestrator_config_serde_empty_defaults() {
        // When deserialized from empty JSON, `enabled` uses default_enabled_true() = true.
        let json = "{}";
        let config: OrchestratorConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.max_concurrent_agents, 3);
        assert_eq!(config.sub_agent_timeout_secs, 0);
        assert!(config.shared_budget);
    }

    #[test]
    fn sub_agent_task_construction() {
        let task = SubAgentTask {
            task_id: Uuid::new_v4(),
            instruction: "List files".to_string(),
            agent_type: AgentType::Coder,
            model: None,
            provider: None,
            allowed_tools: HashSet::from(["bash".to_string()]),
            limits_override: None,
            depends_on: vec![],
            priority: 10,
            system_prompt_prefix: None,
            role: AgentRole::default(),
            team_id: None,
            mailbox_id: None,
            estimated_tokens: 0,
        };
        assert_eq!(task.instruction, "List files");
        assert_eq!(task.agent_type, AgentType::Coder);
        assert!(task.allowed_tools.contains("bash"));
        assert_eq!(task.priority, 10);
    }

    #[test]
    fn sub_agent_task_serde_round_trip() {
        let task = SubAgentTask {
            task_id: Uuid::new_v4(),
            instruction: "Fix the bug".to_string(),
            agent_type: AgentType::Chat,
            model: Some("claude-sonnet".to_string()),
            provider: Some("anthropic".to_string()),
            allowed_tools: HashSet::new(),
            limits_override: None,
            depends_on: vec![Uuid::new_v4()],
            priority: 5,
            system_prompt_prefix: None,
            role: AgentRole::Teammate,
            team_id: Some(Uuid::new_v4()),
            mailbox_id: None,
            estimated_tokens: 0,
        };
        let json = serde_json::to_string(&task).unwrap();
        let parsed: SubAgentTask = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task_id, task.task_id);
        assert_eq!(parsed.instruction, "Fix the bug");
        assert_eq!(parsed.depends_on.len(), 1);
    }

    #[test]
    fn sub_agent_result_serde_round_trip() {
        let result = SubAgentResult {
            task_id: Uuid::new_v4(),
            success: true,
            output_text: "Done".to_string(),
            agent_result: AgentResult {
                success: true,
                summary: "Completed task".to_string(),
                files_modified: vec![],
                tools_used: vec!["bash".to_string()],
            },
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.001,
            latency_ms: 500,
            rounds: 2,
            error: None,
            evidence_verified: true,
            content_read_attempts: 1,
            had_tools_available: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: SubAgentResult = serde_json::from_str(&json).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.output_text, "Done");
        assert_eq!(parsed.rounds, 2);
        assert!(parsed.evidence_verified);
        assert_eq!(parsed.content_read_attempts, 1);
    }

    #[test]
    fn sub_agent_result_evidence_fields_default_to_false_zero() {
        // Backward compatibility: old JSON without evidence fields deserializes with defaults.
        let old_json = r#"{
            "task_id": "00000000-0000-0000-0000-000000000001",
            "success": true,
            "output_text": "done",
            "agent_result": {"success": true, "summary": "ok", "files_modified": [], "tools_used": []},
            "input_tokens": 0,
            "output_tokens": 0,
            "cost_usd": 0.0,
            "latency_ms": 0,
            "rounds": 0
        }"#;
        let parsed: SubAgentResult = serde_json::from_str(old_json).unwrap();
        assert!(!parsed.evidence_verified, "default evidence_verified must be false");
        assert_eq!(parsed.content_read_attempts, 0, "default content_read_attempts must be 0");
    }

    #[test]
    fn orchestrator_result_aggregation() {
        let orch_id = Uuid::new_v4();
        let results = vec![
            SubAgentResult {
                task_id: Uuid::new_v4(),
                success: true,
                output_text: "A".to_string(),
                agent_result: AgentResult {
                    success: true,
                    summary: "A".to_string(),
                    files_modified: vec![],
                    tools_used: vec![],
                },
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: 0.001,
                latency_ms: 500,
                rounds: 1,
                error: None,
                evidence_verified: true,
                content_read_attempts: 2,
                had_tools_available: true,
            },
            SubAgentResult {
                task_id: Uuid::new_v4(),
                success: false,
                output_text: "B".to_string(),
                agent_result: AgentResult {
                    success: false,
                    summary: "Failed".to_string(),
                    files_modified: vec![],
                    tools_used: vec![],
                },
                input_tokens: 200,
                output_tokens: 100,
                cost_usd: 0.002,
                latency_ms: 1000,
                rounds: 3,
                error: Some("timeout".to_string()),
                evidence_verified: false,
                content_read_attempts: 0,
                had_tools_available: true,
            },
        ];

        let agg = OrchestratorResult::from_results(orch_id, results, 1200, false, 0);
        assert_eq!(agg.orchestrator_id, orch_id);
        assert_eq!(agg.success_count, 1);
        assert_eq!(agg.total_count, 2);
        assert!(!agg.budget_exceeded);
        assert_eq!(agg.skipped_count, 0);
        assert_eq!(agg.total_input_tokens, 300);
        assert_eq!(agg.total_output_tokens, 150);
        assert!((agg.total_cost_usd - 0.003).abs() < 0.0001);
        assert_eq!(agg.total_latency_ms, 1200);
    }

    #[test]
    fn orchestrator_result_serde_round_trip() {
        let result = OrchestratorResult {
            orchestrator_id: Uuid::new_v4(),
            sub_results: vec![],
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost_usd: 0.0,
            total_latency_ms: 0,
            success_count: 0,
            total_count: 0,
            budget_exceeded: false,
            skipped_count: 0,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: OrchestratorResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.orchestrator_id, result.orchestrator_id);
    }

    // --- AgentRole tests (PASO 4-B) ---

    #[test]
    fn agent_role_default_is_lead() {
        assert_eq!(AgentRole::default(), AgentRole::Lead);
    }

    #[test]
    fn agent_role_timeout_multipliers() {
        assert!((AgentRole::Lead.timeout_multiplier() - 1.0).abs() < f64::EPSILON);
        assert!((AgentRole::Teammate.timeout_multiplier() - 0.6).abs() < f64::EPSILON);
        assert!((AgentRole::Specialist.timeout_multiplier() - 0.8).abs() < f64::EPSILON);
        assert!((AgentRole::Observer.timeout_multiplier() - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn agent_role_max_rounds_multipliers() {
        assert!((AgentRole::Lead.max_rounds_multiplier() - 1.0).abs() < f64::EPSILON);
        assert!((AgentRole::Teammate.max_rounds_multiplier() - 0.7).abs() < f64::EPSILON);
        assert!((AgentRole::Specialist.max_rounds_multiplier() - 0.5).abs() < f64::EPSILON);
        assert!((AgentRole::Observer.max_rounds_multiplier() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn agent_role_capabilities() {
        assert!(AgentRole::Lead.can_execute_tools());
        assert!(AgentRole::Teammate.can_execute_tools());
        assert!(AgentRole::Specialist.can_execute_tools());
        assert!(!AgentRole::Observer.can_execute_tools());

        assert!(AgentRole::Lead.can_cancel_teammates());
        assert!(!AgentRole::Teammate.can_cancel_teammates());
        assert!(!AgentRole::Specialist.can_cancel_teammates());
        assert!(!AgentRole::Observer.can_cancel_teammates());
    }

    #[test]
    fn agent_role_serde_round_trip() {
        for role in [AgentRole::Lead, AgentRole::Teammate, AgentRole::Specialist, AgentRole::Observer] {
            let json = serde_json::to_string(&role).unwrap();
            let parsed: AgentRole = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, role);
        }
    }

    #[test]
    fn sub_agent_task_role_defaults_to_lead() {
        // Old JSON (before role field existed) should deserialize with role=Lead.
        let json = r#"{
            "task_id": "00000000-0000-0000-0000-000000000001",
            "instruction": "do something",
            "agent_type": "chat"
        }"#;
        let task: SubAgentTask = serde_json::from_str(json).unwrap();
        assert_eq!(task.role, AgentRole::Lead);
        assert!(task.team_id.is_none());
        assert!(task.mailbox_id.is_none());
    }
}
