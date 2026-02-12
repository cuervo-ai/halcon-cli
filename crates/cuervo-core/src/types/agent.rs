use serde::{Deserialize, Serialize};

/// Type of agent executing a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    /// Interactive chat agent (REPL).
    Chat,
    /// Autonomous coding agent.
    Coder,
    /// Code review agent.
    Reviewer,
    /// Multi-agent orchestrator.
    Orchestrator,
}

impl std::fmt::Display for AgentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentType::Chat => write!(f, "chat"),
            AgentType::Coder => write!(f, "coder"),
            AgentType::Reviewer => write!(f, "reviewer"),
            AgentType::Orchestrator => write!(f, "orchestrator"),
        }
    }
}

/// Result of an agent's task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub success: bool,
    pub summary: String,
    pub files_modified: Vec<String>,
    pub tools_used: Vec<String>,
}
