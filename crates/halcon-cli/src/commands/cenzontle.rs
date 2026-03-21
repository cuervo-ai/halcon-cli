//! `halcon cenzontle` — Cenzontle agent orchestration commands.
//!
//! Provides direct CLI access to Cenzontle's agent system, MCP tools, and
//! RAG knowledge search. These commands bridge the Halcón executor with
//! Cenzontle's multi-agent brain.
//!
//! # Subcommands
//!
//! - `halcon cenzontle agent "prompt"` — Submit a task to a Cenzontle agent
//! - `halcon cenzontle tools` — List available MCP tools
//! - `halcon cenzontle search "query"` — Search knowledge base via RAG
//! - `halcon cenzontle agents` — List registered agents

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Subcommand;
use futures::StreamExt;

use halcon_providers::agent_types::*;
use halcon_providers::CenzontleAgentClient;

use super::context_gather;

/// Cenzontle integration subcommands.
#[derive(Subcommand)]
pub enum CenzontleAction {
    /// Submit a task to a Cenzontle agent and stream results
    Agent {
        /// Natural-language instruction for the agent
        prompt: String,

        /// Agent type: ORCHESTRATOR, CONVERSATIONAL, or TASK
        #[arg(long, short = 't', default_value = "ORCHESTRATOR")]
        agent_type: String,

        /// Include local project context (git status, key files)
        #[arg(long, short = 'c')]
        context: bool,

        /// Specific agent ID to use (default: auto-route)
        #[arg(long)]
        agent_id: Option<String>,
    },

    /// List available MCP tools on the Cenzontle backend
    Tools,

    /// Search the knowledge base via RAG
    Search {
        /// Search query
        query: String,

        /// Maximum number of results
        #[arg(long, short = 'k', default_value = "5")]
        top_k: usize,

        /// Bot/namespace to search within
        #[arg(long)]
        bot_id: Option<String>,
    },

    /// List registered agents on the Cenzontle backend
    Agents,
}

/// Resolve the Cenzontle access token from env var or keychain.
fn resolve_access_token() -> Result<String> {
    // 1. Environment variable (highest priority).
    if let Ok(token) = std::env::var("CENZONTLE_ACCESS_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }

    // 2. OS keychain via halcon-auth (same path as provider_factory.rs).
    {
        let keystore = halcon_auth::KeyStore::new("halcon-cli");
        match keystore.get_secret("cenzontle:access_token") {
            Ok(Some(token)) if !token.is_empty() => return Ok(token),
            _ => {}
        }
    }

    anyhow::bail!(
        "No Cenzontle access token found.\n\
         Set CENZONTLE_ACCESS_TOKEN env var or run `halcon login cenzontle`."
    )
}

/// Resolve the Cenzontle base URL.
fn resolve_base_url() -> Option<String> {
    std::env::var("CENZONTLE_BASE_URL").ok().filter(|s| !s.is_empty())
}

/// Try to resolve the access token silently (for auto-init paths).
/// Returns None if no token is available — never errors.
pub fn resolve_access_token_silent() -> Option<String> {
    resolve_access_token().ok()
}

/// Execute a cenzontle subcommand.
pub async fn run(action: CenzontleAction) -> Result<()> {
    // Attempt silent token refresh before resolving — the SSO access token
    // may have expired since the last `halcon login cenzontle`.
    super::sso::refresh_if_needed().await;

    let token = resolve_access_token()?;
    let client = Arc::new(CenzontleAgentClient::new(token, resolve_base_url()));

    match action {
        CenzontleAction::Agent {
            prompt,
            agent_type,
            context,
            agent_id,
        } => run_agent(client, prompt, agent_type, context, agent_id).await,
        CenzontleAction::Tools => run_list_tools(client).await,
        CenzontleAction::Search {
            query,
            top_k,
            bot_id,
        } => run_search(client, query, top_k, bot_id).await,
        CenzontleAction::Agents => run_list_agents(client).await,
    }
}

/// Submit a task to a Cenzontle agent and stream results to terminal.
async fn run_agent(
    client: Arc<CenzontleAgentClient>,
    prompt: String,
    agent_type: String,
    include_context: bool,
    agent_id: Option<String>,
) -> Result<()> {
    // 1. Create agent session.
    eprintln!("Creating agent session...");
    let session = client
        .create_session(&CreateSessionRequest {
            agent_id: agent_id.clone(),
            metadata: HashMap::new(),
        })
        .await
        .context("Failed to create Cenzontle agent session")?;

    eprintln!(
        "Session: {} (agents: {})",
        session.id,
        if session.agent_ids.is_empty() {
            "auto-route".to_string()
        } else {
            session.agent_ids.join(", ")
        }
    );

    // 2. Gather local context if requested.
    let task_context = if include_context {
        let cwd = std::env::current_dir()
            .ok()
            .and_then(|p| p.to_str().map(String::from))
            .unwrap_or_else(|| ".".to_string());
        eprintln!("Gathering local context from {}...", cwd);
        let local_ctx = context_gather::gather_local_context(&cwd).await;
        Some(context_gather::to_task_context(&local_ctx))
    } else {
        None
    };

    // 3. Submit task with streaming.
    eprintln!("Submitting task: {}", prompt);
    eprintln!("---");

    let req = SubmitTaskRequest {
        input: prompt,
        agent_type: Some(agent_type),
        context: task_context,
        priority: None,
    };

    let mut stream = client
        .submit_task(&session.id, &req)
        .await
        .context("Failed to submit agent task")?;

    // 4. Stream events to terminal.
    let mut total_tokens = 0u64;
    let mut streamed_content = false;
    while let Some(event) = stream.next().await {
        match event {
            Ok(TaskEvent::Started { agent_id }) => {
                if let Some(id) = agent_id {
                    eprintln!("[agent: {}]", id);
                }
            }
            Ok(TaskEvent::Thinking { content }) => {
                eprint!("\x1b[2m{}\x1b[0m", content); // dim text for thinking
            }
            Ok(TaskEvent::Content { content }) => {
                print!("{}", content);
                streamed_content = true;
            }
            Ok(TaskEvent::ToolCall { name, input }) => {
                eprintln!("\x1b[36m[tool: {} → {}]\x1b[0m", name, truncate(&input.to_string(), 80));
            }
            Ok(TaskEvent::ToolResult { name, output, is_error }) => {
                if is_error {
                    eprintln!("\x1b[31m[{} error: {}]\x1b[0m", name, truncate(&output, 120));
                } else {
                    eprintln!("\x1b[32m[{}: {}]\x1b[0m", name, truncate(&output, 120));
                }
            }
            Ok(TaskEvent::PlanStep { step, index, total }) => {
                eprintln!("\x1b[33m[step {}/{}: {}]\x1b[0m", index + 1, total, step);
            }
            Ok(TaskEvent::Completed { output, tokens_used }) => {
                // Print output if we haven't already streamed content chunks.
                if !streamed_content && !output.is_empty() {
                    println!("{}", output);
                }
                total_tokens = tokens_used.unwrap_or(0);
            }
            Ok(TaskEvent::Error { message, code }) => {
                eprintln!(
                    "\x1b[31mError{}: {}\x1b[0m",
                    code.map(|c| format!(" ({})", c)).unwrap_or_default(),
                    message
                );
            }
            Ok(TaskEvent::Unknown) => {
                // Forward-compatible: skip unknown event types.
            }
            Err(e) => {
                eprintln!("\x1b[31mStream error: {}\x1b[0m", e);
                break;
            }
        }
    }

    println!();
    if total_tokens > 0 {
        eprintln!("---\nTokens used: {}", total_tokens);
    }

    Ok(())
}

/// List available MCP tools on the Cenzontle backend.
async fn run_list_tools(client: Arc<CenzontleAgentClient>) -> Result<()> {
    let tools = client
        .list_mcp_tools()
        .await
        .context("Failed to list Cenzontle MCP tools")?;

    if tools.is_empty() {
        println!("No MCP tools available.");
        return Ok(());
    }

    println!("{:<25} {}", "TOOL", "DESCRIPTION");
    println!("{}", "-".repeat(70));
    for tool in &tools {
        println!(
            "{:<25} {}",
            tool.name,
            tool.description.as_deref().unwrap_or("-")
        );
    }
    println!("\n{} tools available", tools.len());

    Ok(())
}

/// Search the knowledge base via RAG.
async fn run_search(
    client: Arc<CenzontleAgentClient>,
    query: String,
    top_k: usize,
    bot_id: Option<String>,
) -> Result<()> {
    eprintln!("Searching: \"{}\"", query);

    let resp = client
        .knowledge_search(&KnowledgeSearchRequest {
            query,
            bot_id,
            top_k: Some(top_k),
            score_threshold: None,
        })
        .await
        .context("Failed to search Cenzontle knowledge base")?;

    if resp.chunks.is_empty() {
        println!("No results found.");
        return Ok(());
    }

    for (i, chunk) in resp.chunks.iter().enumerate() {
        println!(
            "\n\x1b[1m[{}] Score: {:.3}{}\x1b[0m",
            i + 1,
            chunk.score,
            chunk
                .source
                .as_deref()
                .map(|s| format!(" — {}", s))
                .unwrap_or_default()
        );
        println!("{}", chunk.content);
    }

    println!("\n{} results", resp.chunks.len());
    Ok(())
}

/// List registered agents on the Cenzontle backend.
async fn run_list_agents(client: Arc<CenzontleAgentClient>) -> Result<()> {
    let agents = client
        .list_agents()
        .await
        .context("Failed to list Cenzontle agents")?;

    if agents.is_empty() {
        println!("No agents registered.");
        return Ok(());
    }

    println!(
        "{:<20} {:<15} {:<10} {}",
        "NAME", "KIND", "STATUS", "CAPABILITIES"
    );
    println!("{}", "-".repeat(70));
    for agent in &agents {
        println!(
            "{:<20} {:<15} {:<10} {}",
            agent.name,
            agent.kind.as_deref().unwrap_or("-"),
            agent.status.as_deref().unwrap_or("-"),
            agent.capabilities.join(", ")
        );
    }
    println!("\n{} agents", agents.len());

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    // Walk backwards to find a valid char boundary.
    let mut end = max;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
