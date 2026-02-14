#![allow(dead_code)] // Infrastructure module: wired via /inspect mcp, not all methods called yet
//! MCP Resource Manager: wraps the McpPool with lazy initialization,
//! health monitoring, and tool discovery.
//!
//! Activates the existing McpPool infrastructure (previously dead code)
//! by providing a higher-level manager that handles lifecycle concerns.

use std::collections::HashMap;
use std::sync::Arc;

use cuervo_core::types::McpConfig;
use cuervo_mcp::pool::{McpConnectionHealth, McpPool, McpServerDef};
use cuervo_tools::ToolRegistry;

/// Manages MCP server connections with lazy initialization and health tracking.
///
/// Instead of eagerly connecting to all MCP servers at startup (as the old
/// `connect_mcp_servers()` did), this manager defers connection until the
/// first time tools are needed.
pub(crate) struct McpResourceManager {
    pool: Arc<McpPool>,
    /// Tools discovered from MCP servers: (server_name, tool_name) pairs.
    discovered_tools: Vec<(String, String)>,
    /// Whether initial discovery has happened.
    initialized: bool,
    /// Whether any servers are configured.
    has_servers: bool,
}

impl McpResourceManager {
    /// Create from MCP config. Does NOT connect yet — connections are lazy.
    pub fn new(mcp_config: &McpConfig) -> Self {
        let server_defs: HashMap<String, McpServerDef> = mcp_config
            .servers
            .iter()
            .map(|(name, cfg)| {
                (
                    name.clone(),
                    McpServerDef {
                        command: cfg.command.clone(),
                        args: cfg.args.clone(),
                        env: cfg.env.clone(),
                        enabled: true,
                    },
                )
            })
            .collect();

        let has_servers = !server_defs.is_empty();
        let pool = Arc::new(McpPool::new(server_defs, mcp_config.max_reconnect_attempts));

        Self {
            pool,
            discovered_tools: Vec::new(),
            initialized: false,
            has_servers,
        }
    }

    /// Create an empty manager (no MCP servers configured).
    pub fn empty() -> Self {
        Self {
            pool: Arc::new(McpPool::new(HashMap::new(), 0)),
            discovered_tools: Vec::new(),
            initialized: true,
            has_servers: false,
        }
    }

    /// Lazy initialization: connect all servers + discover tools.
    ///
    /// Safe to call multiple times — subsequent calls are no-ops.
    /// Failed servers are logged but don't block other servers.
    pub async fn ensure_initialized(
        &mut self,
        _tool_registry: &mut ToolRegistry,
    ) -> Vec<(String, Result<(), String>)> {
        if self.initialized {
            return Vec::new();
        }
        self.initialized = true;

        if !self.has_servers {
            return Vec::new();
        }

        // Connect all servers.
        let connect_results = self.pool.initialize_all().await;
        let mut results = Vec::new();

        for (name, result) in &connect_results {
            match result {
                Ok(()) => {
                    tracing::info!(server = %name, "MCP server connected via pool");
                    results.push((name.clone(), Ok(())));
                }
                Err(e) => {
                    tracing::warn!(server = %name, error = %e, "MCP server failed to connect");
                    results.push((name.clone(), Err(e.to_string())));
                }
            }
        }

        // Discover tools from connected servers.
        let all_tools = self.pool.all_tools().await;
        for (server_name, tools) in all_tools {
            for tool_def in tools {
                let tool_name = tool_def.name.clone();
                // Create a bridge tool backed by the pool's host.
                // Note: McpToolBridge expects an Arc<Mutex<McpHost>>, but we're using
                // the pool for connection management. For now, register discovered
                // tool names for tracking — the actual bridge registration happens
                // through the existing mechanism since McpPool manages hosts internally.
                self.discovered_tools
                    .push((server_name.clone(), tool_name));
            }
            tracing::info!(
                server = %server_name,
                tool_count = self.discovered_tools.iter().filter(|(s, _)| s == &server_name).count(),
                "Discovered MCP tools"
            );
        }

        results
    }

    /// Health check all connections. Returns map of server→health.
    pub async fn health_check(&self) -> HashMap<String, McpConnectionHealth> {
        self.pool.health_check_all().await
    }

    /// Get the pool reference for direct tool calls.
    pub fn pool(&self) -> &Arc<McpPool> {
        &self.pool
    }

    /// Check if any MCP servers are configured.
    pub fn has_servers(&self) -> bool {
        self.has_servers
    }

    /// Check if initialization has been performed.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Get discovered tool names: (server, tool) pairs.
    pub fn discovered_tools(&self) -> &[(String, String)] {
        &self.discovered_tools
    }

    /// Shut down all MCP connections.
    pub async fn shutdown(&self) {
        self.pool.shutdown_all().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::McpServerConfig;

    #[test]
    fn new_empty_config() {
        let config = McpConfig::default();
        let mgr = McpResourceManager::new(&config);
        assert!(!mgr.has_servers());
        assert!(!mgr.is_initialized());
    }

    #[test]
    fn has_servers_with_config() {
        let mut config = McpConfig::default();
        config.servers.insert(
            "test".to_string(),
            McpServerConfig {
                command: "echo".to_string(),
                args: vec![],
                env: HashMap::new(),
                tool_permissions: HashMap::new(),
            },
        );
        let mgr = McpResourceManager::new(&config);
        assert!(mgr.has_servers());
    }

    #[test]
    fn empty_manager_is_initialized() {
        let mgr = McpResourceManager::empty();
        assert!(mgr.is_initialized());
        assert!(!mgr.has_servers());
    }

    #[tokio::test]
    async fn ensure_initialized_idempotent() {
        let config = McpConfig::default();
        let mut mgr = McpResourceManager::new(&config);
        let mut reg = ToolRegistry::new();
        // First call initializes.
        let r1 = mgr.ensure_initialized(&mut reg).await;
        assert!(r1.is_empty());
        assert!(mgr.is_initialized());
        // Second call is a no-op.
        let r2 = mgr.ensure_initialized(&mut reg).await;
        assert!(r2.is_empty());
    }

    #[tokio::test]
    async fn health_check_empty() {
        let mgr = McpResourceManager::empty();
        let health = mgr.health_check().await;
        assert!(health.is_empty());
    }

    #[test]
    fn pool_access() {
        let config = McpConfig::default();
        let mgr = McpResourceManager::new(&config);
        let _pool = mgr.pool();
    }
}
