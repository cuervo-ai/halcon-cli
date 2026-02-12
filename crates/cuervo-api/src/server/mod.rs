pub mod auth;
pub mod handlers;
pub mod router;
pub mod state;
pub mod ws;

use cuervo_runtime::runtime::CuervoRuntime;
use std::net::SocketAddr;
use std::sync::Arc;

use auth::generate_token;
use router::build_router;
use state::AppState;

/// Configuration for the control plane API server.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: String,
    pub port: u16,
    pub auth_token: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: crate::DEFAULT_BIND.to_string(),
            port: crate::DEFAULT_PORT,
            auth_token: None,
        }
    }
}

/// Start the control plane API server.
///
/// Returns the generated auth token and the socket address the server is bound to.
pub async fn start_server(
    runtime: Arc<CuervoRuntime>,
    config: ServerConfig,
) -> Result<(String, SocketAddr), Box<dyn std::error::Error + Send + Sync>> {
    start_server_with_tools(runtime, config, &[]).await
}

/// Start the control plane API server with pre-registered tool names.
///
/// Returns the generated auth token and the socket address the server is bound to.
pub async fn start_server_with_tools(
    runtime: Arc<CuervoRuntime>,
    config: ServerConfig,
    tool_names: &[&str],
) -> Result<(String, SocketAddr), Box<dyn std::error::Error + Send + Sync>> {
    let token = config.auth_token.unwrap_or_else(generate_token);
    let state = AppState::new(runtime, token.clone());

    // Pre-register tools so they appear in the desktop.
    for name in tool_names {
        handlers::tools::register_tool_state(&state, name).await;
    }

    let router = build_router(state);
    let addr: SocketAddr = format!("{}:{}", config.bind_addr, config.port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;

    tracing::info!(addr = %bound_addr, "control plane API server starting");
    eprintln!("Control Plane API: http://{bound_addr}");
    eprintln!("Auth Token: {token}");

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            tracing::error!(error = %e, "API server error");
        }
    });

    Ok((token, bound_addr))
}
