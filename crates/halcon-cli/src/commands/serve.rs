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
    let sessions_file = std::env::var("HOME").ok().map(|h| {
        std::path::PathBuf::from(h)
            .join(".halcon")
            .join("chat_sessions.json")
    });

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
        let config = crate::config_loader::load_config(None).unwrap_or_default();
        let provider_registry =
            Arc::new(crate::commands::provider_factory::build_registry(&config));
        let bridge_tools = {
            let proc_reg2 = Arc::new(ProcessRegistry::new(5));
            Arc::new(halcon_tools::full_registry(
                &tools_config,
                Some(proc_reg2),
                None,
                None,
            ))
        };
        tracing::info!("registering AgentBridgeImpl as ChatExecutor");
        Some(Arc::new(AgentBridgeImpl::with_registries(
            provider_registry,
            bridge_tools,
        )))
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

/// Generate a WebSocket key (base64-encoded 16 random bytes).
fn tungstenite_generate_key() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Use timestamp + pid as entropy source (good enough for WS key)
    let seed = t.as_nanos() ^ (std::process::id() as u128);
    let bytes: [u8; 16] = {
        let mut b = [0u8; 16];
        let s = seed.to_le_bytes();
        for i in 0..16 {
            b[i] = s[i % s.len()];
        }
        b
    };
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ─── Bridge Relay ────────────────────────────────────────────────────────────
// Connects outbound to Cenzontle WebSocket bridge for remote supervision.
// Forwards local control plane events to the cloud and receives commands back.

/// Run serve + bridge relay to Cenzontle.
pub async fn run_with_bridge(
    host: &str,
    port: u16,
    token: Option<String>,
    target: &str,
) -> Result<()> {
    use futures_util::{SinkExt, StreamExt};
    use std::collections::VecDeque;
    use tokio_tungstenite::tungstenite::http::Request;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    // Resolve bridge URL
    let bridge_url = match target {
        "cenzontle" => "wss://api-cenzontle.zuclubit.com/v1/bridge/connect",
        url if url.starts_with("ws") => url,
        _ => {
            eprintln!("Unknown bridge target: {target}. Use 'cenzontle' or a WSS URL.");
            return Ok(());
        }
    };

    // Load Cenzontle token from keychain or env
    let cenzontle_token = std::env::var("CENZONTLE_ACCESS_TOKEN")
        .ok()
        .or_else(|| {
            let keystore = halcon_auth::KeyStore::new("halcon");
            keystore.get_secret("cenzontle:access_token").ok().flatten()
        })
        .ok_or_else(|| {
            anyhow::anyhow!("No Cenzontle token found. Run `halcon login cenzontle` first.")
        })?;

    // Compute machine ID (simple hash of env vars — no external crate needed)
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("NAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    let username = std::env::var("USER").unwrap_or_default();
    let machine_id = {
        // Simple FNV-1a hash — not cryptographic, just a fingerprint
        let input = format!("{hostname}:{username}");
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in input.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("{hash:016x}")
    };

    eprintln!("🌉 Bridge: connecting to {bridge_url}");
    eprintln!("   Machine ID: {}", &machine_id[..16]);

    // Start local server in background
    let server_handle = {
        let h = host.to_string();
        let t = token.clone();
        tokio::spawn(async move {
            if let Err(e) = run(&h, port, t).await {
                eprintln!("Server error: {e}");
            }
        })
    };

    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Bridge relay loop with reconnection
    let mut backoff_secs: u64 = 1;
    let mut last_acked_seq: u64 = 0;
    let mut event_buffer: VecDeque<String> = VecDeque::with_capacity(10_000);

    loop {
        // Build WebSocket request with auth headers
        let request = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(bridge_url)
            .header("Authorization", format!("Bearer {}", cenzontle_token))
            .header("X-Halcon-Version", env!("CARGO_PKG_VERSION"))
            .header("X-Machine-Id", &machine_id)
            .header("X-Resume-From", last_acked_seq.to_string())
            .header("Sec-WebSocket-Key", tungstenite_generate_key())
            .header("Sec-WebSocket-Version", "13")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Host", "api-cenzontle.zuclubit.com")
            .body(())
            .map_err(|e| anyhow::anyhow!("Request build error: {e}"))?;

        match connect_async(request).await {
            Ok((ws_stream, _)) => {
                backoff_secs = 1; // Reset backoff on success
                eprintln!("🌉 Bridge connected to Cenzontle!");

                let (mut write, mut read) = ws_stream.split();

                // Retransmit buffered events
                let buffered_count = event_buffer.len();
                if buffered_count > 0 {
                    eprintln!("🔄 Retransmitting {buffered_count} buffered events...");
                    while let Some(evt) = event_buffer.pop_front() {
                        if write.send(Message::Text(evt)).await.is_err() {
                            break;
                        }
                    }
                }

                // Subscribe to local control plane events
                let local_api = format!("http://127.0.0.1:{port}");

                // Heartbeat ticker
                let mut hb_interval = tokio::time::interval(std::time::Duration::from_secs(30));
                let mut hb_deadline =
                    tokio::time::Instant::now() + std::time::Duration::from_secs(40);

                loop {
                    tokio::select! {
                        // Remote message from Cenzontle → forward to local API
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Text(text))) => {
                                    hb_deadline = tokio::time::Instant::now()
                                        + std::time::Duration::from_secs(40);

                                    // Parse and handle
                                    if text.contains("\"t\":\"hb\"") {
                                        // Heartbeat response — ignore
                                    } else if text.contains("\"t\":\"ack\"") {
                                        // ACK — update last_acked_seq
                                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                                            if let Some(seq) = v["seq"].as_u64() {
                                                last_acked_seq = seq;
                                                // Discard buffered events up to acked seq
                                                // (simplified — buffer is FIFO)
                                            }
                                        }
                                    } else if text.contains("\"t\":\"msg\"") || text.contains("\"t\":\"presol\"") || text.contains("\"t\":\"cancel\"") || text.contains("\"t\":\"ctx\"") {
                                        // Forward command to local control plane
                                        eprintln!("📥 Remote command: {}", &text[..text.len().min(80)]);
                                        // TODO: dispatch to local control plane via internal API
                                    }
                                }
                                Some(Ok(Message::Close(_))) | None => {
                                    eprintln!("🌉 Bridge disconnected");
                                    break;
                                }
                                _ => {}
                            }
                        }

                        // Heartbeat
                        _ = hb_interval.tick() => {
                            if write.send(Message::Text(r#"{"t":"hb"}"#.into())).await.is_err() {
                                break;
                            }
                        }

                        // Heartbeat timeout
                        _ = tokio::time::sleep_until(hb_deadline) => {
                            eprintln!("⚠️  Bridge heartbeat timeout, reconnecting...");
                            break;
                        }

                        // Ctrl+C
                        _ = tokio::signal::ctrl_c() => {
                            eprintln!("\n🛑 Shutting down bridge...");
                            let _ = write.send(Message::Close(None)).await;
                            server_handle.abort();
                            return Ok(());
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("🌉 Bridge connection failed: {e}");
            }
        }

        // Reconnect with exponential backoff
        let jitter = 0.0_f64; // simplified — no jitter for now
        let delay = backoff_secs as f64 + jitter;
        eprintln!("🔄 Reconnecting in {delay:.1}s...");
        tokio::time::sleep(std::time::Duration::from_secs_f64(delay)).await;
        backoff_secs = (backoff_secs * 2).min(60);
    }
}
