//! `cuervo mcp-server` command: starts an MCP server over stdio.
//!
//! Spawned as a sidecar by IDEs (e.g., Tauri `externalBin`).
//! Reads JSON-RPC from stdin, exposes cuervo tools, writes responses to stdout.

use std::sync::Arc;

use anyhow::{Context, Result};

use cuervo_core::types::AppConfig;
use cuervo_mcp::McpServer;
use cuervo_tools::default_registry;

/// Run the MCP server.
///
/// Builds the tool registry from config, then enters the JSON-RPC event loop.
/// Blocks until stdin is closed.
pub async fn run(config: &AppConfig, working_dir: Option<&str>) -> Result<()> {
    let work_dir = working_dir
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/tmp".to_string())
        });

    let registry = default_registry(&config.tools);
    let tools: Vec<Arc<dyn cuervo_core::traits::Tool>> = registry
        .tool_definitions()
        .iter()
        .filter_map(|def| registry.get(&def.name).cloned())
        .collect();

    tracing::info!(
        tool_count = tools.len(),
        working_dir = %work_dir,
        "Starting MCP server"
    );

    let server = McpServer::new(tools, work_dir);
    server
        .run()
        .await
        .context("MCP server exited with error")?;

    Ok(())
}
