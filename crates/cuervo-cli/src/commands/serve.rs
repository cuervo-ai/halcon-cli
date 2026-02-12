//! `cuervo serve` command: starts the control plane API server.
//!
//! Boots the CuervoRuntime, launches the axum HTTP + WebSocket server,
//! and blocks until Ctrl-C / SIGTERM.

use std::sync::Arc;

use anyhow::Result;
use cuervo_api::server::{start_server_with_tools, ServerConfig};
use cuervo_core::types::ToolsConfig;
use cuervo_runtime::bridges::tool_agent::LocalToolAgent;
use cuervo_runtime::runtime::{CuervoRuntime, RuntimeConfig};
use cuervo_tools::background::ProcessRegistry;

/// All tool names from the cuervo-tools registry.
const TOOL_NAMES: &[&str] = &[
    "file_read",
    "file_write",
    "file_edit",
    "file_delete",
    "glob",
    "grep",
    "bash",
    "git_status",
    "git_diff",
    "git_log",
    "git_add",
    "git_commit",
    "web_search",
    "http_request",
    "task_track",
    "fuzzy_find",
    "symbol_search",
    "file_inspect",
    "background_start",
    "background_output",
    "background_kill",
];

/// Run the API server on the given host:port.
///
/// If `token` is `None`, a random token is generated and printed to stderr.
pub async fn run(host: &str, port: u16, token: Option<String>) -> Result<()> {
    // Boot a minimal runtime (no plugins by default).
    let rt_config = RuntimeConfig::default();
    let runtime = Arc::new(CuervoRuntime::new(rt_config));

    // Build tool registry and register each tool as a RuntimeAgent.
    let tools_config = ToolsConfig::default();
    let proc_reg = Arc::new(ProcessRegistry::new(5));
    let tool_registry = cuervo_tools::full_registry(&tools_config, Some(proc_reg));
    let working_dir = std::env::current_dir()
        .unwrap_or_else(|_| "/tmp".into())
        .to_string_lossy()
        .to_string();

    let mut tool_names_registered = Vec::new();
    for def in tool_registry.tool_definitions() {
        if let Some(tool) = tool_registry.get(&def.name) {
            let agent = Arc::new(LocalToolAgent::new(tool.clone(), &working_dir));
            runtime.register_agent(agent).await;
            tool_names_registered.push(def.name);
        }
    }
    eprintln!(
        "Registered {} tool agents in runtime",
        tool_names_registered.len()
    );

    let server_config = ServerConfig {
        bind_addr: host.to_string(),
        port,
        auth_token: token,
    };

    let (_token, addr) = start_server_with_tools(runtime, server_config, TOOL_NAMES)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    eprintln!("Press Ctrl+C to stop the server.");
    eprintln!("Server listening on http://{addr}");

    // Block until shutdown signal.
    tokio::signal::ctrl_c().await?;
    eprintln!("\nShutting down...");

    Ok(())
}
