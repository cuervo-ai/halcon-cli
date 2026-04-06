//! Interactive attach mode for `halcon remote-control attach`.
//!
//! Provides a Claude Code-style experience:
//! - Live streaming of agent events (tokens, tools, sub-agents)
//! - Inline permission approve/reject prompts
//! - Human-in-the-loop chat input
//! - Real-time task progress display
//!
//! The event loop reads from two sources concurrently:
//! 1. WebSocket events from the backend
//! 2. User input from stdin (crossterm raw mode)

use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal,
};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::io::Write;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use super::client::RemoteControlClient;
use super::protocol::{PendingPermission, RemoteControlEvent};

/// State for the interactive session.
struct AttachState {
    session_id: Option<String>,
    pending_permissions: HashMap<Uuid, PendingPermission>,
    input_buffer: String,
    input_mode: InputMode,
    verbose: bool,
    accumulated_tokens: String,
    is_thinking: bool,
    tool_count: u64,
    sub_agent_count: u64,
    correlation_id: u64,
}

#[derive(PartialEq)]
enum InputMode {
    /// Normal mode — watching events.
    Watch,
    /// Chat mode — typing a message to submit.
    Chat,
    /// Permission prompt — waiting for y/n on a specific permission.
    PermissionPrompt(Uuid),
}

impl AttachState {
    fn new(session_id: Option<String>, verbose: bool) -> Self {
        Self {
            session_id,
            pending_permissions: HashMap::new(),
            input_buffer: String::new(),
            input_mode: InputMode::Watch,
            verbose,
            accumulated_tokens: String::new(),
            is_thinking: false,
            tool_count: 0,
            sub_agent_count: 0,
            correlation_id: 0,
        }
    }

    fn next_correlation(&mut self) -> u64 {
        self.correlation_id += 1;
        self.correlation_id
    }
}

/// Run the interactive attach mode.
pub async fn run_attach(
    client: &RemoteControlClient,
    session_id: Option<String>,
    verbose: bool,
) -> Result<()> {
    // If no session specified, try to find the active one.
    let session_id = if let Some(sid) = session_id {
        sid
    } else {
        let sessions = client.list_sessions().await?;
        match sessions.len() {
            0 => anyhow::bail!("No active sessions. Use `halcon remote-control start` first."),
            1 => sessions[0].id.clone(),
            _ => {
                // Pick the most recently active (last in list).
                let last = sessions.last().unwrap();
                eprintln!(
                    "Multiple sessions found. Attaching to most recent: {}",
                    &last.id[..8]
                );
                last.id.clone()
            }
        }
    };

    let mut state = AttachState::new(Some(session_id.clone()), verbose);

    // Connect WebSocket.
    let (ws_sink, ws_stream) = client.connect_ws().await?;
    let ws_sink = std::sync::Arc::new(tokio::sync::Mutex::new(ws_sink));
    let _ws_sink_clone = ws_sink.clone();

    // Print header.
    print_header(client, &session_id);

    // Enable raw mode for inline key handling.
    terminal::enable_raw_mode().context("Failed to enable raw mode")?;

    // Ensure raw mode is disabled on exit.
    let result = run_event_loop(client, &mut state, ws_stream).await;

    terminal::disable_raw_mode().ok();
    println!();

    result
}

/// Main event loop — multiplexes WebSocket events and stdin.
async fn run_event_loop(
    client: &RemoteControlClient,
    state: &mut AttachState,
    mut ws_stream: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> Result<()> {
    let mut stdout = std::io::stdout();

    loop {
        // Poll both WebSocket and stdin concurrently.
        tokio::select! {
            // WebSocket event
            ws_msg = ws_stream.next() => {
                match ws_msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(event) = RemoteControlClient::parse_event(&text) {
                            handle_event(client, state, &event, &mut stdout).await?;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        write_line(&mut stdout, "\r\n  Connection closed by server.\r\n");
                        break;
                    }
                    Some(Ok(Message::Ping(_))) => {} // tungstenite auto-pongs
                    Some(Err(e)) => {
                        write_line(&mut stdout, &format!("\r\n  WebSocket error: {e}\r\n"));
                        break;
                    }
                    _ => {}
                }
            }

            // Stdin event (crossterm)
            _ = poll_stdin() => {
                if let Ok(true) = event::poll(std::time::Duration::from_millis(0)) {
                    if let Ok(Event::Key(key)) = event::read() {
                        let should_exit = handle_key(client, state, key, &mut stdout).await?;
                        if should_exit {
                            break;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Handle a WebSocket event.
async fn handle_event(
    _client: &RemoteControlClient,
    state: &mut AttachState,
    event: &RemoteControlEvent,
    out: &mut impl Write,
) -> Result<()> {
    match event {
        RemoteControlEvent::PermissionRequired {
            request_id,
            tool_name,
            risk_level,
            description,
            deadline_secs,
            args_preview,
            ..
        } => {
            // Flush any accumulated tokens before showing the permission prompt.
            flush_tokens(state, out);

            let perm = PendingPermission {
                request_id: *request_id,
                tool_name: tool_name.clone(),
                risk_level: risk_level.clone(),
                description: description.clone(),
                deadline_secs: *deadline_secs,
                args_preview: args_preview.clone(),
            };

            let cid = state.next_correlation();
            write_line(
                out,
                &format!(
                    "\r\n\x1b[33m  [{cid}] PERMISSION REQUIRED\x1b[0m\r\n\
                     \x1b[33m    Tool:        {tool_name}\x1b[0m\r\n\
                     \x1b[33m    Risk:        {risk_level}\x1b[0m\r\n\
                     \x1b[33m    Description: {description}\x1b[0m\r\n"
                ),
            );

            // Show args preview.
            for (k, v) in args_preview {
                let truncated = if v.len() > 60 {
                    format!("{}...", &v[..57])
                } else {
                    v.clone()
                };
                write_line(out, &format!("\x1b[33m    {k}: {truncated}\x1b[0m\r\n"));
            }

            write_line(
                out,
                &format!(
                    "\x1b[33m    Deadline: {deadline_secs}s\x1b[0m\r\n\
                     \x1b[1;33m    [y] approve  [n] reject  [Enter] skip\x1b[0m\r\n\
                     \x1b[33m  > \x1b[0m"
                ),
            );

            state.pending_permissions.insert(*request_id, perm);
            state.input_mode = InputMode::PermissionPrompt(*request_id);
        }

        RemoteControlEvent::PermissionResolved {
            request_id,
            decision,
            tool_executed,
            ..
        } => {
            state.pending_permissions.remove(request_id);
            if state.input_mode == InputMode::PermissionPrompt(*request_id) {
                state.input_mode = InputMode::Watch;
            }
            let icon = if *tool_executed { " " } else { " " };
            write_line(
                out,
                &format!(
                    "\r\n  {icon} Permission {}: {decision}\r\n",
                    &request_id.to_string()[..8]
                ),
            );
        }

        RemoteControlEvent::PermissionExpired { request_id, .. } => {
            state.pending_permissions.remove(request_id);
            if state.input_mode == InputMode::PermissionPrompt(*request_id) {
                state.input_mode = InputMode::Watch;
            }
            write_line(
                out,
                &format!(
                    "\r\n\x1b[31m    Permission {} expired\x1b[0m\r\n",
                    &request_id.to_string()[..8]
                ),
            );
        }

        RemoteControlEvent::ChatStreamToken {
            token, is_thinking, ..
        } => {
            if *is_thinking {
                if !state.is_thinking {
                    state.is_thinking = true;
                    write!(out, "\r\n\x1b[2m  thinking... \x1b[0m").ok();
                    out.flush().ok();
                }
            } else {
                if state.is_thinking {
                    state.is_thinking = false;
                    write!(out, "\r\n").ok();
                }
                state.accumulated_tokens.push_str(token);
                // Flush on newlines or when buffer gets large.
                if token.contains('\n') || state.accumulated_tokens.len() > 120 {
                    flush_tokens(state, out);
                }
            }
        }

        RemoteControlEvent::ToolExecuted {
            name,
            duration_ms,
            success,
            ..
        } => {
            flush_tokens(state, out);
            state.tool_count += 1;
            let icon = if *success { " " } else { " " };
            let color = if *success { "32" } else { "31" };
            write_line(
                out,
                &format!(
                    "\r\n  \x1b[{color}m{icon} {name}\x1b[0m \x1b[2m({duration_ms}ms)\x1b[0m\r\n"
                ),
            );
        }

        RemoteControlEvent::SubAgentStarted {
            sub_agent_id,
            task_description,
            wave,
            ..
        } => {
            state.sub_agent_count += 1;
            if state.verbose {
                write_line(
                    out,
                    &format!(
                        "\r\n  \x1b[36m  Sub-agent {sub_agent_id} (wave {wave}): {task_description}\x1b[0m\r\n"
                    ),
                );
            }
        }

        RemoteControlEvent::SubAgentCompleted {
            sub_agent_id,
            success,
            summary,
            duration_ms,
            ..
        } => {
            let icon = if *success { " " } else { " " };
            write_line(
                out,
                &format!(
                    "\r\n  {icon} \x1b[36mAgent {}\x1b[0m \x1b[2m({duration_ms}ms)\x1b[0m: {summary}\r\n",
                    &sub_agent_id[..std::cmp::min(12, sub_agent_id.len())]
                ),
            );
        }

        RemoteControlEvent::ConversationCompleted {
            total_duration_ms,
            stop_reason,
            ..
        } => {
            flush_tokens(state, out);
            write_line(
                out,
                &format!(
                    "\r\n\x1b[1;32m  Completed\x1b[0m ({stop_reason}) \
                     \x1b[2m{total_duration_ms}ms | {} tools | {} sub-agents\x1b[0m\r\n\
                     \r\n  Type a message or press \x1b[1mq\x1b[0m to quit, \
                     \x1b[1mi\x1b[0m to chat\r\n",
                    state.tool_count, state.sub_agent_count
                ),
            );
            // Reset counters for next turn.
            state.tool_count = 0;
            state.sub_agent_count = 0;
        }

        RemoteControlEvent::ExecutionFailed {
            error_code,
            message,
            recoverable,
            ..
        } => {
            flush_tokens(state, out);
            let recov = if *recoverable { " (recoverable)" } else { "" };
            write_line(
                out,
                &format!(
                    "\r\n\x1b[1;31m  Execution failed: [{error_code}] {message}{recov}\x1b[0m\r\n"
                ),
            );
        }

        RemoteControlEvent::Connected { server_version } => {
            write_line(
                out,
                &format!("\r\n  Connected to halcon {server_version}\r\n"),
            );
        }

        RemoteControlEvent::ChatSessionCreated {
            session_id,
            model,
            provider,
        } => {
            if state.verbose {
                write_line(
                    out,
                    &format!(
                        "\r\n  Session created: {} ({model}/{provider})\r\n",
                        &session_id.to_string()[..8]
                    ),
                );
            }
        }

        RemoteControlEvent::Error { code, message } => {
            write_line(
                out,
                &format!("\r\n\x1b[31m  Error [{code}]: {message}\x1b[0m\r\n"),
            );
        }

        RemoteControlEvent::ReplanAccepted { step_count, .. } => {
            write_line(
                out,
                &format!("\r\n\x1b[32m  Replan accepted ({step_count} steps)\x1b[0m\r\n"),
            );
        }

        RemoteControlEvent::ReplanRejected { reason, .. } => {
            write_line(
                out,
                &format!("\r\n\x1b[31m  Replan rejected: {reason}\x1b[0m\r\n"),
            );
        }

        _ => {
            if state.verbose {
                write_line(
                    out,
                    &format!("\r\n\x1b[2m  (event: {:?})\x1b[0m\r\n", event),
                );
            }
        }
    }

    Ok(())
}

/// Handle a key press.
async fn handle_key(
    client: &RemoteControlClient,
    state: &mut AttachState,
    key: event::KeyEvent,
    out: &mut impl Write,
) -> Result<bool> {
    match &state.input_mode {
        InputMode::PermissionPrompt(request_id) => {
            let rid = *request_id;
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(sid) = &state.session_id {
                        write_line(out, "  Approving...\r\n");
                        client.resolve_permission(sid, rid, true).await.ok();
                    }
                    state.input_mode = InputMode::Watch;
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    if let Some(sid) = &state.session_id {
                        write_line(out, "  Rejecting...\r\n");
                        client.resolve_permission(sid, rid, false).await.ok();
                    }
                    state.input_mode = InputMode::Watch;
                }
                KeyCode::Enter => {
                    write_line(out, "  Skipped.\r\n");
                    state.input_mode = InputMode::Watch;
                }
                KeyCode::Char('q') => return Ok(true),
                _ => {}
            }
        }

        InputMode::Chat => match key.code {
            KeyCode::Enter => {
                let msg = std::mem::take(&mut state.input_buffer);
                if !msg.is_empty() {
                    if let Some(sid) = &state.session_id {
                        write_line(out, &format!("\r\n\x1b[1m  you:\x1b[0m {msg}\r\n"));
                        if let Err(e) = client.submit_message(sid, &msg, false).await {
                            write_line(out, &format!("\r\n\x1b[31m  Error: {e}\x1b[0m\r\n"));
                        }
                    }
                }
                state.input_mode = InputMode::Watch;
            }
            KeyCode::Esc => {
                state.input_buffer.clear();
                state.input_mode = InputMode::Watch;
                write_line(out, "\r\n  (cancelled)\r\n");
            }
            KeyCode::Backspace => {
                state.input_buffer.pop();
                // Redraw input line.
                write!(out, "\r\x1b[K  \x1b[1m>\x1b[0m {}", state.input_buffer).ok();
                out.flush().ok();
            }
            KeyCode::Char(c) => {
                state.input_buffer.push(c);
                write!(out, "{c}").ok();
                out.flush().ok();
            }
            _ => {}
        },

        InputMode::Watch => match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(true),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),
            KeyCode::Char('i') | KeyCode::Char('I') => {
                state.input_mode = InputMode::Chat;
                write!(out, "\r\n  \x1b[1m>\x1b[0m ").ok();
                out.flush().ok();
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                // Quick approve: approve the oldest pending permission.
                if let Some((&rid, _perm)) = state.pending_permissions.iter().next() {
                    if let Some(sid) = &state.session_id {
                        write_line(
                            out,
                            &format!("\r\n  Approving {}...\r\n", &rid.to_string()[..8]),
                        );
                        client.resolve_permission(sid, rid, true).await.ok();
                    }
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                // Quick reject: reject the oldest pending permission.
                if let Some((&rid, _perm)) = state.pending_permissions.iter().next() {
                    if let Some(sid) = &state.session_id {
                        write_line(
                            out,
                            &format!("\r\n  Rejecting {}...\r\n", &rid.to_string()[..8]),
                        );
                        client.resolve_permission(sid, rid, false).await.ok();
                    }
                }
            }
            KeyCode::Char('x') | KeyCode::Char('X') => {
                // Cancel the active execution.
                if let Some(sid) = &state.session_id {
                    write_line(out, "\r\n  Cancelling execution...\r\n");
                    client.cancel_session(sid).await.ok();
                }
            }
            KeyCode::Char('?') => {
                print_help(out);
            }
            _ => {}
        },
    }

    Ok(false)
}

// ── Display Helpers ─────────────────────────────────────────────────────────

fn flush_tokens(state: &mut AttachState, out: &mut impl Write) {
    if !state.accumulated_tokens.is_empty() {
        let text = std::mem::take(&mut state.accumulated_tokens);
        // Write token text with proper line handling for raw mode.
        for line in text.split('\n') {
            write!(out, "{line}\r\n").ok();
        }
        // Remove trailing \r\n from the last line (it wasn't a newline in original).
        // This is approximate but good enough for streaming.
        out.flush().ok();
    }
}

fn write_line(out: &mut impl Write, text: &str) {
    write!(out, "{text}").ok();
    out.flush().ok();
}

fn print_header(client: &RemoteControlClient, session_id: &str) {
    let short_id = &session_id[..std::cmp::min(8, session_id.len())];
    eprintln!();
    eprintln!(
        "  \x1b[1mhalcon remote-control\x1b[0m \x1b[2m(v{})\x1b[0m",
        super::protocol::PROTOCOL_VERSION
    );
    eprintln!("  Server:  {}", client.server_url());
    eprintln!("  Session: {short_id}");
    eprintln!();
    eprintln!(
        "  \x1b[2mKeys: [i] chat  [a] approve  [r] reject  [x] cancel  [q] quit  [?] help\x1b[0m"
    );
    eprintln!("  \x1b[2mWaiting for events...\x1b[0m");
    eprintln!();
}

fn print_help(out: &mut impl Write) {
    write_line(
        out,
        "\r\n\x1b[1m  Keyboard shortcuts:\x1b[0m\r\n\
         \r\n\
           \x1b[1m  i\x1b[0m  Enter chat mode (type message + Enter)\r\n\
           \x1b[1m  a\x1b[0m  Approve oldest pending permission\r\n\
           \x1b[1m  r\x1b[0m  Reject oldest pending permission\r\n\
           \x1b[1m  y/n\x1b[0m  Approve/reject when prompted\r\n\
           \x1b[1m  x\x1b[0m  Cancel active execution\r\n\
           \x1b[1m  q\x1b[0m  Quit (Ctrl+C also works)\r\n\
           \x1b[1m  ?\x1b[0m  Show this help\r\n\
         \r\n",
    );
}

/// Async-friendly stdin poll (yields to tokio when no input available).
async fn poll_stdin() {
    // Use a small delay to avoid busy-looping.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attach_state_new() {
        let state = AttachState::new(Some("test".to_string()), false);
        assert_eq!(state.session_id, Some("test".to_string()));
        assert!(!state.verbose);
        assert!(state.pending_permissions.is_empty());
    }

    #[test]
    fn test_correlation_counter() {
        let mut state = AttachState::new(None, false);
        assert_eq!(state.next_correlation(), 1);
        assert_eq!(state.next_correlation(), 2);
        assert_eq!(state.next_correlation(), 3);
    }
}
