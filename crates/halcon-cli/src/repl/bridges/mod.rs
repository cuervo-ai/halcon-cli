// bridges/ — bridges, runtimes, MCP, comms
// MIGRATION-2026: archivos movidos desde repl/ raíz

pub mod agent_comm;
pub mod dev_gateway;
pub(crate) mod mcp_manager;
pub mod replay_executor;
pub mod replay_runner;
pub(crate) mod runtime;
pub mod search;
pub(crate) mod task;

// Re-exports
pub use agent_comm::{SharedContextStore, AgentCommHub, AgentCommSender};
pub use dev_gateway::DevGateway;
pub(crate) use mcp_manager::McpResourceManager;
pub(crate) use runtime::CliToolRuntime;
pub(crate) use task::TaskBridge;
