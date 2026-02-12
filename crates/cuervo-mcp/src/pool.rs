//! MCP connection pool with health tracking and auto-reconnect.
//!
//! Manages multiple MCP server connections with configurable
//! reconnection limits and health monitoring.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{McpError, McpResult};
use crate::host::McpHost;
use crate::types::{CallToolResult, McpToolDefinition};

/// Health status of an MCP connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpConnectionHealth {
    /// Connected and responsive.
    Healthy,
    /// Connected but experiencing issues.
    Degraded,
    /// Connection lost or server crashed.
    Failed,
    /// Not yet connected.
    Uninitialized,
}

/// Configuration for a single MCP server in the pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerDef {
    /// Command to launch the MCP server process.
    pub command: String,
    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables to set for the process.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Whether this server is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl Default for McpServerDef {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            enabled: true,
        }
    }
}

/// Internal state for a managed MCP connection.
struct ManagedConnection {
    host: Option<McpHost>,
    health: McpConnectionHealth,
    reconnect_count: u32,
    config: McpServerDef,
}

/// Pool of MCP server connections with health tracking.
pub struct McpPool {
    connections: Arc<RwLock<HashMap<String, ManagedConnection>>>,
    max_reconnect: u32,
}

impl McpPool {
    /// Create a new pool from server definitions.
    pub fn new(configs: HashMap<String, McpServerDef>, max_reconnect: u32) -> Self {
        let mut connections = HashMap::new();
        for (name, config) in configs {
            if config.enabled {
                connections.insert(
                    name,
                    ManagedConnection {
                        host: None,
                        health: McpConnectionHealth::Uninitialized,
                        reconnect_count: 0,
                        config,
                    },
                );
            }
        }
        Self {
            connections: Arc::new(RwLock::new(connections)),
            max_reconnect,
        }
    }

    /// Initialize all configured server connections.
    pub async fn initialize_all(&self) -> Vec<(String, McpResult<()>)> {
        let mut results = Vec::new();
        let mut conns = self.connections.write().await;
        let names: Vec<String> = conns.keys().cloned().collect();

        for name in names {
            let result = Self::connect_inner(&mut conns, &name);
            results.push((name, result));
        }
        results
    }

    /// Connect (or reconnect) a specific server by name.
    pub async fn connect(&self, name: &str) -> McpResult<()> {
        let mut conns = self.connections.write().await;
        Self::connect_inner(&mut conns, name)
    }

    fn connect_inner(
        conns: &mut HashMap<String, ManagedConnection>,
        name: &str,
    ) -> McpResult<()> {
        let managed = conns
            .get_mut(name)
            .ok_or_else(|| McpError::Protocol(format!("unknown server: {name}")))?;

        if managed.reconnect_count >= 100 {
            // Prevent runaway reconnection (capped separately from max_reconnect).
            return Err(McpError::Protocol(format!(
                "server '{name}' exceeded reconnection limit"
            )));
        }

        let host = McpHost::new(
            name,
            &managed.config.command,
            &managed.config.args,
            &managed.config.env,
        )?;
        managed.host = Some(host);
        managed.health = McpConnectionHealth::Healthy;
        managed.reconnect_count += 1;
        Ok(())
    }

    /// Call a tool on a specific server with auto-reconnect on failure.
    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> McpResult<CallToolResult> {
        let mut conns = self.connections.write().await;
        let managed = conns
            .get_mut(server)
            .ok_or_else(|| McpError::Protocol(format!("unknown server: {server}")))?;

        // Try the call, reconnect on failure up to max_reconnect times.
        let max_attempts = self.max_reconnect.min(5); // cap retry attempts
        for attempt in 0..=max_attempts {
            if let Some(ref host) = managed.host {
                match host.call_tool(tool, args.clone()).await {
                    Ok(result) => {
                        managed.health = McpConnectionHealth::Healthy;
                        return Ok(result);
                    }
                    Err(e) => {
                        tracing::warn!("MCP call to '{server}/{tool}' failed (attempt {attempt}): {e}");
                        managed.health = McpConnectionHealth::Failed;
                        managed.host = None;
                    }
                }
            }

            // Try reconnect.
            if attempt < max_attempts {
                match McpHost::new(
                    server,
                    &managed.config.command,
                    &managed.config.args,
                    &managed.config.env,
                ) {
                    Ok(host) => {
                        managed.host = Some(host);
                        managed.reconnect_count += 1;
                        managed.health = McpConnectionHealth::Degraded;
                    }
                    Err(e) => {
                        tracing::warn!("MCP reconnect to '{server}' failed: {e}");
                    }
                }
            }
        }

        Err(McpError::Protocol(format!(
            "failed to call '{server}/{tool}' after {max_attempts} reconnect attempts"
        )))
    }

    /// Check health of all connections.
    pub async fn health_check_all(&self) -> HashMap<String, McpConnectionHealth> {
        let conns = self.connections.read().await;
        conns
            .iter()
            .map(|(name, managed)| (name.clone(), managed.health))
            .collect()
    }

    /// Shut down all connections.
    pub async fn shutdown_all(&self) {
        let mut conns = self.connections.write().await;
        for managed in conns.values_mut() {
            managed.host = None;
            managed.health = McpConnectionHealth::Failed;
        }
    }

    /// Get all tools from all connected servers.
    pub async fn all_tools(&self) -> Vec<(String, Vec<McpToolDefinition>)> {
        let conns = self.connections.read().await;
        let mut result = Vec::new();
        for (name, managed) in conns.iter() {
            if let Some(ref host) = managed.host {
                result.push((name.clone(), host.tools().to_vec()));
            }
        }
        result
    }

    /// Get server names in this pool.
    pub async fn server_names(&self) -> Vec<String> {
        let conns = self.connections.read().await;
        conns.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_pool_creation() {
        let mut configs = HashMap::new();
        configs.insert(
            "test".to_string(),
            McpServerDef {
                command: "echo".to_string(),
                args: vec!["hello".to_string()],
                env: HashMap::new(),
                enabled: true,
            },
        );
        let pool = McpPool::new(configs, 3);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let names = rt.block_on(pool.server_names());
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn mcp_pool_empty_configs() {
        let pool = McpPool::new(HashMap::new(), 3);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let names = rt.block_on(pool.server_names());
        assert!(names.is_empty());
    }

    #[test]
    fn mcp_server_def_serde() {
        let def = McpServerDef {
            command: "npx".to_string(),
            args: vec!["server".to_string()],
            env: HashMap::from([("KEY".to_string(), "VAL".to_string())]),
            enabled: true,
        };
        let json = serde_json::to_string(&def).unwrap();
        let parsed: McpServerDef = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.command, "npx");
        assert!(parsed.enabled);
    }

    #[test]
    fn mcp_connection_health_enum() {
        let health = McpConnectionHealth::Healthy;
        let json = serde_json::to_string(&health).unwrap();
        assert_eq!(json, r#""healthy""#);
        let parsed: McpConnectionHealth = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, McpConnectionHealth::Healthy);
    }

    #[tokio::test]
    async fn mcp_pool_health_check_empty() {
        let pool = McpPool::new(HashMap::new(), 3);
        let health = pool.health_check_all().await;
        assert!(health.is_empty());
    }

    #[test]
    fn mcp_server_def_default_enabled() {
        let def = McpServerDef::default();
        assert!(def.enabled);
        assert!(def.command.is_empty());
    }

    #[tokio::test]
    async fn mcp_pool_all_tools_empty() {
        let pool = McpPool::new(HashMap::new(), 3);
        let tools = pool.all_tools().await;
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn mcp_pool_disabled_server_excluded() {
        let mut configs = HashMap::new();
        configs.insert(
            "disabled".to_string(),
            McpServerDef {
                command: "echo".to_string(),
                enabled: false,
                ..Default::default()
            },
        );
        configs.insert(
            "enabled".to_string(),
            McpServerDef {
                command: "echo".to_string(),
                enabled: true,
                ..Default::default()
            },
        );
        let pool = McpPool::new(configs, 3);
        let names = pool.server_names().await;
        assert_eq!(names.len(), 1);
        assert!(names.contains(&"enabled".to_string()));
    }

    #[tokio::test]
    async fn mcp_pool_health_uninitialized() {
        let mut configs = HashMap::new();
        configs.insert(
            "test".to_string(),
            McpServerDef {
                command: "echo".to_string(),
                ..Default::default()
            },
        );
        let pool = McpPool::new(configs, 3);
        let health = pool.health_check_all().await;
        assert_eq!(
            health.get("test"),
            Some(&McpConnectionHealth::Uninitialized)
        );
    }

    #[tokio::test]
    async fn mcp_pool_shutdown() {
        let mut configs = HashMap::new();
        configs.insert(
            "test".to_string(),
            McpServerDef {
                command: "echo".to_string(),
                ..Default::default()
            },
        );
        let pool = McpPool::new(configs, 3);
        pool.shutdown_all().await;
        let health = pool.health_check_all().await;
        assert_eq!(health.get("test"), Some(&McpConnectionHealth::Failed));
    }

    #[test]
    fn mcp_server_def_serde_backward_compat() {
        // Old config without 'enabled' field should default to true.
        let json = r#"{"command": "npx", "args": ["server"]}"#;
        let parsed: McpServerDef = serde_json::from_str(json).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.command, "npx");
    }
}
