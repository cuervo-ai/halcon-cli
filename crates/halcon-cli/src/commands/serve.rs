//! `halcon serve` command: starts the control plane API server.
//!
//! Boots the HalconRuntime, launches the axum HTTP + WebSocket server,
//! and blocks until Ctrl-C / SIGTERM.

use std::sync::Arc;

use anyhow::Result;
use halcon_api::server::{start_server_with_executor, ServerConfig};
use halcon_core::types::ToolsConfig;
use halcon_runtime::bridges::tool_agent::LocalToolAgent;
use halcon_runtime::runtime::{HalconRuntime, RuntimeConfig};
use halcon_tools::background::ProcessRegistry;

#[cfg(feature = "headless")]
use crate::agent_bridge::AgentBridgeImpl;

/// All tool names from the halcon-tools registry.
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
    let runtime = Arc::new(HalconRuntime::new(rt_config));

    // Build tool registry and register each tool as a RuntimeAgent.
    let tools_config = ToolsConfig::default();
    let proc_reg = Arc::new(ProcessRegistry::new(5));
    let tool_registry = halcon_tools::full_registry(&tools_config, Some(proc_reg), None, None);
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

    // Persist chat sessions to ~/.halcon/chat_sessions.json across restarts.
    let sessions_file = std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".halcon").join("chat_sessions.json"));

    let server_config = ServerConfig {
        bind_addr: host.to_string(),
        port,
        auth_token: token,
        sessions_file,
    };

    // Build executor when headless feature is enabled.
    // Inject the provider registry so AgentBridgeImpl can resolve providers by name.
    #[cfg(feature = "headless")]
    let executor: Option<Arc<dyn halcon_core::traits::ChatExecutor>> = {
        let config = crate::config_loader::load_config(None)
            .unwrap_or_default();
        let provider_registry = Arc::new(crate::commands::provider_factory::build_registry(&config));
        let bridge_tools = {
            let proc_reg2 = Arc::new(ProcessRegistry::new(5));
            Arc::new(halcon_tools::full_registry(&tools_config, Some(proc_reg2), None, None))
        };
        tracing::info!("registering AgentBridgeImpl as ChatExecutor");
        Some(Arc::new(AgentBridgeImpl::with_registries(provider_registry, bridge_tools)))
    };
    #[cfg(not(feature = "headless"))]
    let executor: Option<Arc<dyn halcon_core::traits::ChatExecutor>> = None;

    let (_token, addr) = start_server_with_executor(runtime, server_config, TOOL_NAMES, executor)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    eprintln!("Press Ctrl+C to stop the server.");
    eprintln!("Server listening on http://{addr}");

    // Block until shutdown signal.
    tokio::signal::ctrl_c().await?;
    eprintln!("\nShutting down...");

    Ok(())
}
