//! Repl infrastructure dependencies — registries, database, configuration.
//!
//! Phase 3.1: Immutable infrastructure that is set once during Repl construction
//! and never changes during the session. Groups 8 fields from the original Repl struct.

use std::sync::Arc;

use halcon_core::types::AppConfig;
use halcon_core::EventSender;
use halcon_providers::ProviderRegistry;
use halcon_storage::{AsyncDatabase, Database};
use halcon_tools::ToolRegistry;

use super::bridges::dev_gateway::DevGateway;

/// Immutable infrastructure dependencies for the REPL.
///
/// Contains registries, database handles, configuration, and coordination
/// subsystems that are set once during construction and never mutate.
/// All fields are either owned or wrapped in Arc for cheap cloning.
pub struct ReplInfrastructure {
    /// Application configuration (loaded from config.toml, env vars, CLI args).
    pub config: AppConfig,

    /// Synchronous database handle for blocking operations.
    pub db: Option<Arc<Database>>,

    /// Asynchronous database handle for async operations (storage queries).
    pub async_db: Option<AsyncDatabase>,

    /// Provider registry for resolving model providers (OpenAI, Anthropic, etc.).
    pub registry: ProviderRegistry,

    /// Tool registry for all available tools (bash, file operations, etc.).
    pub tool_registry: ToolRegistry,

    /// Event emission channel for observability and telemetry.
    pub event_tx: EventSender,

    /// OS username for user context injection into system prompt (e.g. "oscarvalois").
    pub user_display_name: String,

    /// Dev ecosystem gateway: coordinates IDE buffers, git context, CI feedback.
    pub dev_gateway: DevGateway,
}

impl ReplInfrastructure {
    /// Construct infrastructure with required dependencies.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: AppConfig,
        db: Option<Arc<Database>>,
        async_db: Option<AsyncDatabase>,
        registry: ProviderRegistry,
        tool_registry: ToolRegistry,
        event_tx: EventSender,
        user_display_name: String,
        dev_gateway: DevGateway,
    ) -> Self {
        Self {
            config,
            db,
            async_db,
            registry,
            tool_registry,
            event_tx,
            user_display_name,
            dev_gateway,
        }
    }

    /// Get reference to the provider registry.
    pub fn provider_registry(&self) -> &ProviderRegistry {
        &self.registry
    }

    /// Get reference to the tool registry.
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    /// Check if database is available.
    pub fn has_database(&self) -> bool {
        self.db.is_some() || self.async_db.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infrastructure_tracks_database_availability() {
        let (event_tx, _rx) = halcon_core::event_bus(16);

        let infra_no_db = ReplInfrastructure::new(
            AppConfig::default(),
            None,
            None,
            ProviderRegistry::default(),
            ToolRegistry::new(),
            event_tx.clone(),
            "testuser".to_string(),
            Default::default(), // DevGateway::default()
        );

        assert!(!infra_no_db.has_database());

        // Note: Can't easily construct a real Database in unit tests,
        // but the pattern is clear - has_database() checks Option<>.
    }

    #[test]
    fn infrastructure_provides_registry_access() {
        let (event_tx, _rx) = halcon_core::event_bus(16);

        let registry = ProviderRegistry::default();
        let tool_registry = ToolRegistry::new();

        let infra = ReplInfrastructure::new(
            AppConfig::default(),
            None,
            None,
            registry,
            tool_registry,
            event_tx,
            "testuser".to_string(),
            Default::default(), // DevGateway::default()
        );

        // Accessing registries should not panic
        let _prov = infra.provider_registry();
        let _tools = infra.tool_registry();
    }
}
