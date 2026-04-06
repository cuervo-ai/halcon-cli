//! `halcon remote-control` — Remote control interface for Halcon sessions.
//!
//! Provides CLI-first human-in-the-loop control over running Halcon agent sessions.
//! Connects to a running `halcon serve` instance via REST + WebSocket.
//!
//! Subcommands:
//!   start              — Start a new remote-controlled session
//!   status             — Show current session and pending permissions
//!   approve <id>       — Approve a pending permission request
//!   reject <id>        — Reject a pending permission request
//!   replan <file>      — Submit a new plan (JSON) to replace the current execution
//!   cancel <task_id>   — Cancel a running task/session
//!   attach             — Interactive mode (live event stream + inline approve/reject)

use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::PathBuf;

mod client;
mod interactive;
mod protocol;

pub use protocol::ReplanPayload;

/// Subcommands for `halcon remote-control`.
#[derive(Subcommand)]
pub enum RemoteControlAction {
    /// Start a new remote-controlled agent session
    Start {
        /// Model to use for the session
        #[arg(long)]
        model: Option<String>,

        /// Provider to use
        #[arg(long)]
        provider: Option<String>,

        /// Initial instruction for the agent
        #[arg(short, long)]
        instruction: Option<String>,

        /// Enable orchestration (multi-agent)
        #[arg(long)]
        orchestrate: bool,
    },

    /// Show status of the remote-controlled session
    Status {
        /// Session ID (uses active session if omitted)
        #[arg(short, long)]
        session: Option<String>,
    },

    /// Approve a pending permission request
    Approve {
        /// Permission request ID
        permission_id: String,

        /// Session ID (uses active session if omitted)
        #[arg(short, long)]
        session: Option<String>,
    },

    /// Reject a pending permission request
    Reject {
        /// Permission request ID
        permission_id: String,

        /// Session ID (uses active session if omitted)
        #[arg(short, long)]
        session: Option<String>,
    },

    /// Submit a new execution plan to replace the current one
    Replan {
        /// Path to plan JSON file
        plan: PathBuf,

        /// Session ID (uses active session if omitted)
        #[arg(short, long)]
        session: Option<String>,
    },

    /// Cancel a running task or session
    Cancel {
        /// Task or session ID to cancel
        task_id: String,
    },

    /// Attach to a session in interactive mode (live events + inline control)
    ///
    /// Similar to Claude Code's interactive experience: shows real-time
    /// permission requests, task progress, and allows inline approve/reject.
    Attach {
        /// Session ID to attach to (uses most recent if omitted)
        #[arg(short, long)]
        session: Option<String>,

        /// Show all event types (verbose mode)
        #[arg(short, long)]
        verbose: bool,
    },
}

/// Run the remote-control command.
pub async fn run(action: RemoteControlAction, server_url: &str, token: &str) -> Result<()> {
    let rc_client = client::RemoteControlClient::new(server_url, token)?;

    match action {
        RemoteControlAction::Start {
            model,
            provider,
            instruction,
            orchestrate,
        } => cmd_start(&rc_client, model, provider, instruction, orchestrate).await,

        RemoteControlAction::Status { session } => cmd_status(&rc_client, session).await,

        RemoteControlAction::Approve {
            permission_id,
            session,
        } => cmd_approve(&rc_client, &permission_id, session).await,

        RemoteControlAction::Reject {
            permission_id,
            session,
        } => cmd_reject(&rc_client, &permission_id, session).await,

        RemoteControlAction::Replan { plan, session } => {
            cmd_replan(&rc_client, &plan, session).await
        }

        RemoteControlAction::Cancel { task_id } => cmd_cancel(&rc_client, &task_id).await,

        RemoteControlAction::Attach { session, verbose } => {
            interactive::run_attach(&rc_client, session, verbose).await
        }
    }
}

// ── Subcommand implementations ──────────────────────────────────────────────

async fn cmd_start(
    client: &client::RemoteControlClient,
    model: Option<String>,
    provider: Option<String>,
    instruction: Option<String>,
    orchestrate: bool,
) -> Result<()> {
    let model = model.unwrap_or_else(|| "claude-sonnet-4-5-20250929".to_string());
    let provider = provider.unwrap_or_else(|| "anthropic".to_string());

    println!("Starting remote-controlled session...");
    println!("  Model:    {model}");
    println!("  Provider: {provider}");

    let session = client.create_session(&model, &provider).await?;
    println!("  Session:  {}", session.id);
    println!();

    // If an instruction was provided, submit it immediately.
    if let Some(msg) = instruction {
        println!("Submitting instruction: {}", truncate(&msg, 80));
        client
            .submit_message(&session.id, &msg, orchestrate)
            .await?;
        println!("Execution started. Use `halcon remote-control attach` to monitor.");
    } else {
        println!("Session created. Submit a message with:");
        println!("  halcon remote-control attach -s {}", session.id);
    }

    // Persist active session ID for subsequent commands.
    persist_active_session(&session.id)?;

    Ok(())
}

async fn cmd_status(
    client: &client::RemoteControlClient,
    session_id: Option<String>,
) -> Result<()> {
    let sessions = client.list_sessions().await?;

    if sessions.is_empty() {
        println!("No active sessions.");
        return Ok(());
    }

    if let Some(sid) = session_id {
        let session = sessions
            .iter()
            .find(|s| s.id == sid)
            .context("Session not found")?;
        print_session_detail(session);
    } else {
        println!("Active sessions ({}):", sessions.len());
        println!();
        for s in &sessions {
            println!(
                "  {} {} [{}] ({} msgs)",
                status_icon(&s.status),
                &s.id[..8],
                s.status,
                s.message_count
            );
        }
        println!();

        // Show active session hint
        if let Some(active) = load_active_session() {
            println!("Active session: {active}");
        }
    }

    Ok(())
}

async fn cmd_approve(
    client: &client::RemoteControlClient,
    permission_id: &str,
    session_id: Option<String>,
) -> Result<()> {
    let sid = resolve_session(session_id)?;
    let perm_id: uuid::Uuid = permission_id
        .parse()
        .context("Invalid permission ID format")?;

    println!("Approving permission {}...", &permission_id[..8]);
    let resp = client.resolve_permission(&sid, perm_id, true).await?;
    println!(
        "  Decision: approve | Tool executed: {}",
        resp.tool_executed
    );
    Ok(())
}

async fn cmd_reject(
    client: &client::RemoteControlClient,
    permission_id: &str,
    session_id: Option<String>,
) -> Result<()> {
    let sid = resolve_session(session_id)?;
    let perm_id: uuid::Uuid = permission_id
        .parse()
        .context("Invalid permission ID format")?;

    println!("Rejecting permission {}...", &permission_id[..8]);
    let _resp = client.resolve_permission(&sid, perm_id, false).await?;
    println!("  Decision: deny");
    Ok(())
}

async fn cmd_replan(
    client: &client::RemoteControlClient,
    plan_path: &PathBuf,
    session_id: Option<String>,
) -> Result<()> {
    let sid = resolve_session(session_id)?;
    let plan_json = std::fs::read_to_string(plan_path).context("Failed to read plan file")?;
    let payload: ReplanPayload = serde_json::from_str(&plan_json).context("Invalid plan JSON")?;

    println!("Submitting replan ({} steps)...", payload.steps.len());
    client.submit_replan(&sid, &payload).await?;
    println!("  Replan accepted. Execution will restart with the new plan.");
    Ok(())
}

async fn cmd_cancel(client: &client::RemoteControlClient, task_id: &str) -> Result<()> {
    println!(
        "Cancelling {}...",
        &task_id[..std::cmp::min(8, task_id.len())]
    );
    client.cancel_session(task_id).await?;
    println!("  Cancelled.");
    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn resolve_session(explicit: Option<String>) -> Result<String> {
    explicit
        .or_else(load_active_session)
        .context("No session specified. Use -s <session_id> or start a session first.")
}

fn persist_active_session(session_id: &str) -> Result<()> {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("halcon");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("active_rc_session"), session_id)?;
    Ok(())
}

fn load_active_session() -> Option<String> {
    let path = dirs::data_dir()?.join("halcon").join("active_rc_session");
    std::fs::read_to_string(path).ok().filter(|s| !s.is_empty())
}

fn status_icon(status: &str) -> &'static str {
    match status {
        "idle" => " ",
        "executing" => " ",
        "awaiting_permission" => " ",
        "error" => " ",
        "cancelled" => " ",
        _ => " ",
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max - 3])
    }
}

fn print_session_detail(s: &protocol::RemoteSessionInfo) {
    println!("Session: {}", s.id);
    println!("  Status:   {} {}", status_icon(&s.status), s.status);
    println!("  Model:    {}", s.model);
    println!("  Provider: {}", s.provider);
    println!("  Messages: {}", s.message_count);
    if let Some(ref title) = s.title {
        println!("  Title:    {title}");
    }
}
