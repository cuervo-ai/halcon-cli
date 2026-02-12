use async_trait::async_trait;

use crate::error::Result;

/// Describes an action a connector can perform.
#[derive(Debug, Clone)]
pub struct ConnectorAction {
    /// Action identifier (e.g., "list_repos", "create_issue").
    pub name: String,
    /// Human-readable description.
    pub description: String,
}

/// Trait for external service connectors (GitHub, Jira, Slack, etc.).
///
/// This trait is defined now for interface stability. Implementations
/// are deferred to Sprint 8.
#[async_trait]
pub trait Connector: Send + Sync {
    /// Unique connector identifier (e.g., "github", "jira").
    fn id(&self) -> &str;

    /// Human-readable name (e.g., "GitHub", "Jira Cloud").
    fn display_name(&self) -> &str;

    /// Authenticate with the external service.
    async fn authenticate(&mut self) -> Result<()>;

    /// Check if the connector is currently authenticated.
    async fn is_authenticated(&self) -> bool;

    /// Execute an action on the external service.
    async fn execute(&self, action: &str, params: serde_json::Value) -> Result<serde_json::Value>;

    /// List available actions for this connector.
    fn available_actions(&self) -> Vec<ConnectorAction>;
}
