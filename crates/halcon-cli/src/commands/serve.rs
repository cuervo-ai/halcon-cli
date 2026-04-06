//! `halcon serve` command: starts the control plane API server.
//!
//! Boots the HalconRuntime, launches the axum HTTP + WebSocket server,
//! and blocks until Ctrl-C / SIGTERM.

use std::sync::Arc;

use anyhow::{Context, Result};
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
    use halcon_storage::PersistentEventBuffer;

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

    // Load Cenzontle token from keychain or env.
    // Service name MUST be "halcon-cli" to match sso.rs store_tokens().
    let cenzontle_token = std::env::var("CENZONTLE_ACCESS_TOKEN")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            let keystore = halcon_auth::KeyStore::new("halcon-cli");
            keystore.get_secret("cenzontle:access_token").ok().flatten()
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No active Cenzontle session found.\n\n\
                 Run:\n  halcon auth login cenzontle\n\n\
                 Or set the CENZONTLE_ACCESS_TOKEN environment variable.\n\
                 Check status with: halcon auth status"
            )
        })?;

    // Compute machine ID (simple hash of env vars — no external crate needed)
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("NAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    let username = std::env::var("USER").unwrap_or_default();
    let machine_id = {
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

    // Build tool registry for task delegation
    let tools_config = ToolsConfig::default();
    let proc_reg = Arc::new(ProcessRegistry::new(5));
    let tool_registry = Arc::new(halcon_tools::full_registry(
        &tools_config,
        Some(proc_reg),
        None,
        None,
    ));
    let working_dir = std::env::current_dir()
        .unwrap_or_else(|_| "/tmp".into())
        .to_string_lossy()
        .to_string();

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

    // Open persistent event buffer
    let halcon_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".halcon");

    // Ensure directory exists
    let _ = std::fs::create_dir_all(&halcon_dir);

    let event_buffer_path = halcon_dir.join("bridge_event_buffer.db");
    let mut event_buffer = PersistentEventBuffer::open(&event_buffer_path)
        .context("Failed to open persistent event buffer")?;

    // Open Dead Letter Queue for failed task tracking
    let dlq_path = halcon_dir.join("dlq.db");
    let dlq = Arc::new(tokio::sync::Mutex::new(
        halcon_storage::DeadLetterQueue::open(&dlq_path)
            .map_err(|e| anyhow::anyhow!("Failed to open DLQ: {}", e))?,
    ));

    // Recover last sequence from buffer
    let mut last_acked_seq: u64 = event_buffer.last_seq()?.unwrap_or(0);

    eprintln!("📊 Event buffer ready at {:?}", event_buffer_path);
    let stats = event_buffer.stats()?;
    eprintln!(
        "   Pending: {}, Sent: {}, Acked: {}",
        stats.pending, stats.sent, stats.acked
    );

    // Bridge relay loop with reconnection
    let mut backoff_secs: u64 = 1;
    let mut current_seq: u64 = last_acked_seq;

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
                backoff_secs = 1;
                eprintln!("🌉 Bridge connected to Cenzontle!");

                let (mut write, mut read) = ws_stream.split();

                // Retransmit unsent/unacked events from persistent buffer
                let unsent = event_buffer.recover_unsent().unwrap_or_default();
                if !unsent.is_empty() {
                    eprintln!("🔄 Retransmitting {} buffered events...", unsent.len());
                    for evt in unsent {
                        if let Err(e) = write.send(Message::Text(evt.payload.clone())).await {
                            eprintln!("⚠️  Failed to retransmit seq {}: {}", evt.seq, e);
                            break;
                        }
                        // Mark as sent (awaiting ACK)
                        let _ = event_buffer.mark_sent(evt.seq);
                    }
                }

                // Channel for task results → WebSocket upstream
                let (upstream_tx, mut upstream_rx) = tokio::sync::mpsc::channel::<String>(256);

                // Heartbeat ticker
                let mut hb_interval = tokio::time::interval(std::time::Duration::from_secs(30));
                let mut hb_deadline =
                    tokio::time::Instant::now() + std::time::Duration::from_secs(40);

                loop {
                    tokio::select! {
                        // Remote message from Cenzontle → dispatch
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Text(text))) => {
                                    hb_deadline = tokio::time::Instant::now()
                                        + std::time::Duration::from_secs(40);

                                    if text.contains("\"t\":\"hb\"") {
                                        // Heartbeat — ignore
                                    } else if text.contains("\"t\":\"ack\"") {
                                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                                            if let Some(seq) = v["seq"].as_u64() {
                                                last_acked_seq = seq;
                                                // Mark events as acked in persistent buffer
                                                match event_buffer.mark_acked(seq) {
                                                    Ok(n) if n > 0 => {
                                                        eprintln!("✅ ACK received: seq {} ({} events confirmed)", seq, n);
                                                    }
                                                    Err(e) => {
                                                        eprintln!("⚠️  Failed to mark acked: {}", e);
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }
                                    } else if text.contains("\"t\":\"ctx\"") {
                                        // Context injection — check for task_delegation
                                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                                            let key = v["d"]["key"].as_str().unwrap_or("");
                                            if key == "task_delegation" {
                                                let value_str = v["d"]["value"].as_str().unwrap_or("{}");
                                                if let Ok(task) = serde_json::from_str::<serde_json::Value>(value_str) {
                                                    let task_id = task["taskId"].as_str().unwrap_or("unknown").to_string();
                                                    let instructions = task["instructions"].as_str().unwrap_or("").to_string();
                                                    let timeout_ms = task["timeout"].as_u64().unwrap_or(60_000);

                                                    eprintln!("📥 Task delegation: {task_id}");
                                                    eprintln!("   Instructions: {}", &instructions[..instructions.len().min(120)]);

                                                    // Spawn task execution in background
                                                    let tx = upstream_tx.clone();
                                                    let registry = tool_registry.clone();
                                                    let wd = working_dir.clone();
                                                    let dlq_clone = dlq.clone();
                                                    tokio::spawn(async move {
                                                        execute_delegated_task(
                                                            &task_id,
                                                            &instructions,
                                                            timeout_ms,
                                                            registry,
                                                            &wd,
                                                            tx,
                                                            dlq_clone,
                                                        )
                                                        .await;
                                                    });
                                                }
                                            } else {
                                                eprintln!("📥 Context: key={key}");
                                            }
                                        }
                                    } else if text.contains("\"t\":\"msg\"") || text.contains("\"t\":\"presol\"") || text.contains("\"t\":\"cancel\"") {
                                        eprintln!("📥 Remote command: {}", &text[..text.len().min(80)]);
                                    }
                                }
                                Some(Ok(Message::Close(_))) | None => {
                                    eprintln!("🌉 Bridge disconnected");
                                    break;
                                }
                                _ => {}
                            }
                        }

                        // Task results → send upstream to Cenzontle
                        Some(result_json) = upstream_rx.recv() => {
                            current_seq += 1;
                            // Persist BEFORE sending (guarantees zero data loss)
                            if let Err(e) = event_buffer.push(current_seq, result_json.clone()) {
                                eprintln!("⚠️  Failed to persist event seq {}: {}", current_seq, e);
                            }

                            match write.send(Message::Text(result_json)).await {
                                Ok(_) => {
                                    // Mark as sent (awaiting ACK)
                                    let _ = event_buffer.mark_sent(current_seq);
                                }
                                Err(e) => {
                                    eprintln!("⚠️  Send failed, event buffered (seq {}): {}", current_seq, e);
                                    // Event stays in 'pending' status, will be retransmitted on reconnect
                                    break;
                                }
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
        let delay = backoff_secs as f64;
        eprintln!("🔄 Reconnecting in {delay:.1}s...");
        tokio::time::sleep(std::time::Duration::from_secs_f64(delay)).await;
        backoff_secs = (backoff_secs * 2).min(60);
    }
}

// ─── Task Delegation Executor ────────────────────────────────────────────────

/// Execute a delegated task from Cenzontle by running tools locally and
/// streaming results back via the upstream channel.
///
/// Protocol:
/// - For each tool call: sends `{"t":"tresult","d":{name, input, output, ok}}`
/// - When done: sends `{"t":"done","d":{"taskId":"..."}}`
async fn execute_delegated_task(
    task_id: &str,
    instructions: &str,
    timeout_ms: u64,
    tool_registry: Arc<halcon_tools::ToolRegistry>,
    working_dir: &str,
    upstream: tokio::sync::mpsc::Sender<String>,
    dlq: Arc<tokio::sync::Mutex<halcon_storage::DeadLetterQueue>>,
) {
    use tokio::time::{timeout, Duration};

    let deadline = Duration::from_millis(timeout_ms);

    // Wrap ENTIRE execution in global timeout
    let result = timeout(deadline, async {
        // Parse instructions to determine which tools to run
        let tool_calls = parse_instructions_to_tool_calls(instructions, working_dir);

        if tool_calls.is_empty() {
            // Fallback: run as a single bash command
            let fallback = vec![ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": instructions}),
            }];
            run_tool_calls(task_id, &fallback, &tool_registry, working_dir, &upstream).await
        } else {
            run_tool_calls(task_id, &tool_calls, &tool_registry, working_dir, &upstream).await
        }
    })
    .await;

    match result {
        Ok(Ok(_)) => {
            // Success
            let done_msg = serde_json::json!({
                "t": "done",
                "d": { "taskId": task_id }
            });
            let _ = upstream.send(done_msg.to_string()).await;
            eprintln!("✅ Task {task_id} completed");
        }
        Ok(Err(e)) => {
            // Tool execution error
            let error = format!("Task failed: {}", e);
            eprintln!("❌ {}", error);

            // Add to DLQ
            let payload = serde_json::json!({
                "taskId": task_id,
                "instructions": instructions,
                "timeout": timeout_ms
            })
            .to_string();

            let mut dlq_guard = dlq.lock().await;
            let _ = dlq_guard.add_failure(task_id, payload, error, 3);

            // Send done (with failure)
            let done_msg = serde_json::json!({
                "t": "done",
                "d": { "taskId": task_id }
            });
            let _ = upstream.send(done_msg.to_string()).await;
        }
        Err(_) => {
            // Global timeout exceeded
            let error = format!("Task timed out after {}ms", timeout_ms);
            eprintln!("⏰ {}", error);

            // Send failure result
            let timeout_result = serde_json::json!({
                "t": "tresult",
                "d": {
                    "id": format!("{task_id}-timeout"),
                    "name": "global_timeout",
                    "output": error.clone(),
                    "ok": false,
                }
            });
            let _ = upstream.send(timeout_result.to_string()).await;

            // Add to DLQ
            let payload = serde_json::json!({
                "taskId": task_id,
                "instructions": instructions,
                "timeout": timeout_ms
            })
            .to_string();

            let mut dlq_guard = dlq.lock().await;
            let _ = dlq_guard.add_failure(task_id, payload, error, 3);

            // Send done (with failure)
            let done_msg = serde_json::json!({
                "t": "done",
                "d": { "taskId": task_id }
            });
            let _ = upstream.send(done_msg.to_string()).await;
        }
    }
}

struct ToolCall {
    name: String,
    args: serde_json::Value,
}

/// Run a sequence of tool calls and send results upstream.
async fn run_tool_calls(
    task_id: &str,
    calls: &[ToolCall],
    registry: &halcon_tools::ToolRegistry,
    working_dir: &str,
    upstream: &tokio::sync::mpsc::Sender<String>,
) -> Result<()> {
    use halcon_core::types::ToolInput;

    for (i, call) in calls.iter().enumerate() {
        let tool = match registry.get(&call.name) {
            Some(t) => t,
            None => {
                let error = format!("Unknown tool: {}", call.name);
                eprintln!("⚠️  {}", error);
                let err_result = serde_json::json!({
                    "t": "tresult",
                    "d": {
                        "id": format!("{task_id}-{i}"),
                        "name": &call.name,
                        "input": call.args.to_string(),
                        "error": error.clone(),
                        "ok": false,
                    }
                });
                upstream
                    .send(err_result.to_string())
                    .await
                    .map_err(|e| anyhow::anyhow!("Upstream send error: {}", e))?;

                // Return error to propagate to execute_delegated_task
                return Err(anyhow::anyhow!(error));
            }
        };

        let tool_input = ToolInput {
            tool_use_id: format!("{task_id}-{i}"),
            arguments: call.args.clone(),
            working_directory: working_dir.to_string(),
        };

        eprintln!("🔧 [{}/{}] {} ...", i + 1, calls.len(), call.name);

        let result = match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            tool.execute(tool_input),
        )
        .await
        {
            Ok(Ok(output)) => {
                // Truncate large outputs for the wire protocol
                let content = if output.content.len() > 8192 {
                    format!(
                        "{}...\n[truncated {} bytes]",
                        &output.content[..8192],
                        output.content.len()
                    )
                } else {
                    output.content
                };
                serde_json::json!({
                    "t": "tresult",
                    "d": {
                        "id": format!("{task_id}-{i}"),
                        "name": &call.name,
                        "input": call.args.to_string(),
                        "output": content,
                        "ok": !output.is_error,
                    }
                })
            }
            Ok(Err(e)) => {
                serde_json::json!({
                    "t": "tresult",
                    "d": {
                        "id": format!("{task_id}-{i}"),
                        "name": &call.name,
                        "input": call.args.to_string(),
                        "error": format!("{e}"),
                        "ok": false,
                    }
                })
            }
            Err(_) => {
                serde_json::json!({
                    "t": "tresult",
                    "d": {
                        "id": format!("{task_id}-{i}"),
                        "name": &call.name,
                        "input": call.args.to_string(),
                        "error": "Tool execution timed out (30s)",
                        "ok": false,
                    }
                })
            }
        };

        upstream
            .send(result.to_string())
            .await
            .map_err(|e| anyhow::anyhow!("Upstream send error: {}", e))?;
    }

    Ok(())
}

/// Parse LLM-generated instructions into a sequence of tool calls.
///
/// Recognizes patterns like:
/// - "Read file /path/to/file" → file_read
/// - "Run: ls -la" or "Execute: npm test" → bash
/// - "Search for 'pattern' in src/" → grep
/// - "Find files matching *.rs" → glob
/// - "List files" → bash(ls)
/// - "Check git status" → git_status
fn parse_instructions_to_tool_calls(instructions: &str, working_dir: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();

    for line in instructions.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }

        // Pattern: "Read <path>" or "Read file <path>"
        if let Some(path) = line
            .strip_prefix("Read file ")
            .or_else(|| line.strip_prefix("Read "))
        {
            let path = path.trim().trim_matches('"').trim_matches('`');
            calls.push(ToolCall {
                name: "file_read".to_string(),
                args: serde_json::json!({"path": path}),
            });
            continue;
        }

        // Pattern: "Run: <cmd>" or "Execute: <cmd>" or "$ <cmd>" or "```\n<cmd>\n```"
        if let Some(cmd) = line
            .strip_prefix("Run: ")
            .or_else(|| line.strip_prefix("Execute: "))
            .or_else(|| line.strip_prefix("run: "))
            .or_else(|| line.strip_prefix("$ "))
        {
            let cmd = cmd.trim().trim_matches('`');
            if !cmd.is_empty() {
                calls.push(ToolCall {
                    name: "bash".to_string(),
                    args: serde_json::json!({"command": cmd}),
                });
            }
            continue;
        }

        // Pattern: "Search for 'pattern'" or "Grep <pattern>"
        if let Some(rest) = line
            .strip_prefix("Search for ")
            .or_else(|| line.strip_prefix("Grep "))
            .or_else(|| line.strip_prefix("grep "))
        {
            let pattern = rest.trim().trim_matches('\'').trim_matches('"');
            calls.push(ToolCall {
                name: "grep".to_string(),
                args: serde_json::json!({"pattern": pattern, "path": working_dir}),
            });
            continue;
        }

        // Pattern: "Find files matching <glob>" or "Glob <pattern>"
        if let Some(rest) = line
            .strip_prefix("Find files matching ")
            .or_else(|| line.strip_prefix("Glob "))
            .or_else(|| line.strip_prefix("glob "))
        {
            let pattern = rest.trim().trim_matches('\'').trim_matches('"');
            calls.push(ToolCall {
                name: "glob".to_string(),
                args: serde_json::json!({"pattern": pattern}),
            });
            continue;
        }

        // Pattern: "git status", "git diff", "git log"
        if line.starts_with("git ") {
            let parts: Vec<&str> = line.splitn(2, ' ').collect();
            if parts.len() == 2 {
                let git_cmd = parts[1].split_whitespace().next().unwrap_or("");
                match git_cmd {
                    "status" => calls.push(ToolCall {
                        name: "git_status".to_string(),
                        args: serde_json::json!({}),
                    }),
                    "diff" => calls.push(ToolCall {
                        name: "git_diff".to_string(),
                        args: serde_json::json!({}),
                    }),
                    "log" => calls.push(ToolCall {
                        name: "git_log".to_string(),
                        args: serde_json::json!({"max_count": 10}),
                    }),
                    _ => calls.push(ToolCall {
                        name: "bash".to_string(),
                        args: serde_json::json!({"command": line}),
                    }),
                }
            }
            continue;
        }

        // Fallback: treat as bash command if it looks executable
        if line.starts_with("ls")
            || line.starts_with("cat ")
            || line.starts_with("find ")
            || line.starts_with("npm ")
            || line.starts_with("cargo ")
            || line.starts_with("python")
            || line.starts_with("node ")
            || line.starts_with("make")
            || line.starts_with("echo ")
            || line.starts_with("cd ")
            || line.starts_with("pwd")
            || line.starts_with("tree")
            || line.starts_with("wc ")
            || line.starts_with("head ")
            || line.starts_with("tail ")
            || line.starts_with("sort ")
            || line.starts_with("du ")
            || line.starts_with("df ")
            || line.contains('|')
        // piped commands
        {
            calls.push(ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": line}),
            });
            continue;
        }

        // If nothing matched but there's content, try as bash
        if !line.is_empty() && !line.starts_with('-') && !line.starts_with('*') {
            calls.push(ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": line}),
            });
        }
    }

    calls
}
