use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod commands;
mod config_loader;
mod render;
mod repl;
mod tui;

/// Build version string with git hash and build date (leaked to 'static).
fn build_version() -> &'static str {
    let s = format!(
        "{} ({} {}, {})",
        env!("CARGO_PKG_VERSION"),
        env!("CUERVO_GIT_HASH"),
        env!("CUERVO_BUILD_DATE"),
        env!("CUERVO_TARGET"),
    );
    Box::leak(s.into_boxed_str())
}

/// Cuervo — AI-powered CLI for software development.
///
/// Multi-model, self-hosted, open source.
#[derive(Parser)]
#[command(name = "cuervo", version = build_version(), about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    /// Model to use (e.g., "claude-sonnet-4-5-20250929", "llama3.2")
    #[arg(short, long, env = "CUERVO_MODEL")]
    model: Option<String>,

    /// Provider to use (e.g., "anthropic", "ollama")
    #[arg(short, long, env = "CUERVO_PROVIDER")]
    provider: Option<String>,

    /// Enable verbose output (sets log level to debug)
    #[arg(short, long)]
    verbose: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "warn", env = "CUERVO_LOG")]
    log_level: String,

    /// Emit traces as JSON lines to stderr (for offline debugging / piping)
    #[arg(long)]
    trace_json: bool,

    /// Configuration file path
    #[arg(long, env = "CUERVO_CONFIG")]
    config: Option<String>,

    /// Suppress the startup banner
    #[arg(long, env = "CUERVO_NO_BANNER")]
    no_banner: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start an interactive chat session (default)
    Chat {
        /// Initial prompt (optional; if omitted, enters REPL)
        prompt: Option<String>,

        /// Resume a previous session by ID
        #[arg(short, long)]
        resume: Option<String>,

        /// Use 3-zone TUI mode (requires --features tui)
        #[arg(long, env = "CUERVO_TUI")]
        tui: bool,

        /// Enable multi-agent orchestration
        #[arg(long, env = "CUERVO_ORCHESTRATE")]
        orchestrate: bool,

        /// Enable structured task framework
        #[arg(long, env = "CUERVO_TASKS")]
        tasks: bool,

        /// Enable self-improvement reflexion loop
        #[arg(long, env = "CUERVO_REFLEXION")]
        reflexion: bool,

        /// Show session metrics on exit
        #[arg(long)]
        metrics: bool,

        /// Export execution timeline as JSON on exit
        #[arg(long)]
        timeline: bool,

        /// Enable all advanced features
        #[arg(long)]
        full: bool,

        /// Expert mode: show full agent feedback (model selection, caching, compaction)
        #[arg(long, env = "CUERVO_EXPERT")]
        expert: bool,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Initialize Cuervo in the current project
    Init {
        /// Force re-initialization
        #[arg(short, long)]
        force: bool,
    },

    /// Show current status (provider, model, session)
    Status,

    /// Manage API keys and authentication
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },

    /// Export or inspect a session trace
    Trace {
        #[command(subcommand)]
        action: TraceAction,
    },

    /// Replay a recorded session trace
    Replay {
        /// Session ID to replay
        session_id: String,

        /// Run deterministic replay and verify execution fingerprint
        #[arg(long)]
        verify: bool,
    },

    /// Manage persistent semantic memory
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },

    /// Run runtime health diagnostics
    Doctor,

    /// Tool diagnostics and health checks
    Tools {
        #[command(subcommand)]
        action: ToolsAction,
    },

    /// Start an MCP server over stdio (for IDE sidecar integration)
    #[command(name = "mcp-server")]
    McpServer {
        /// Working directory for tool execution
        #[arg(long, short = 'w')]
        working_dir: Option<String>,
    },

    /// Start the control plane API server
    Serve {
        /// Host to bind (default: 127.0.0.1)
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Port to bind (default: 9849)
        #[arg(short, long, default_value_t = 9849)]
        port: u16,

        /// Auth token (auto-generated if omitted)
        #[arg(long, env = "CUERVO_API_TOKEN")]
        token: Option<String>,
    },
}

#[derive(Subcommand)]
enum AuthAction {
    /// Authenticate with a provider (store API key in OS keychain)
    Login {
        /// Provider name (e.g., "anthropic", "openai")
        provider: String,
    },
    /// Remove an API key from the OS keychain
    Logout {
        /// Provider name
        provider: String,
    },
    /// Show which providers have API keys configured
    Status,
}

#[derive(Subcommand)]
enum TraceAction {
    /// Export a session trace as JSON
    Export {
        /// Session ID to export
        session_id: String,
    },
}

#[derive(Subcommand)]
enum MemoryAction {
    /// List memory entries
    List {
        /// Filter by type (fact, session_summary, decision, code_snippet, project_meta)
        #[arg(short = 't', long)]
        entry_type: Option<String>,
        /// Maximum number of entries to show
        #[arg(short, long, default_value = "20")]
        limit: u32,
    },
    /// Search memory by keyword (BM25 full-text search)
    Search {
        /// Search query
        query: String,
        /// Maximum results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Prune expired and excess entries
    Prune {
        /// Skip confirmation prompt
        #[arg(long, short)]
        force: bool,
    },
    /// Show memory store statistics
    Stats,
}

#[derive(Subcommand)]
enum ToolsAction {
    /// Run tool health diagnostics (schema validation, probes, metrics)
    Doctor,
    /// List all available tools with permission levels
    List,
    /// Validate all tool schemas in detail
    Validate,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show current configuration
    Show,
    /// Get a configuration value
    Get {
        /// Configuration key (e.g., "general.default_model")
        key: String,
    },
    /// Set a configuration value
    Set {
        /// Configuration key
        key: String,
        /// Configuration value
        value: String,
    },
    /// Show configuration file path
    Path,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging. --verbose overrides --log-level. --trace-json enables JSON output.
    let log_level = if cli.verbose || cli.trace_json {
        "debug".to_string()
    } else {
        cli.log_level
    };
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&log_level));

    if cli.trace_json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_target(false)
            .with_writer(std::io::stderr)
            .init();
    }

    // Load configuration
    let config = config_loader::load_config(cli.config.as_deref())
        .context("Failed to load configuration")?;

    // Initialize the terminal design system theme.
    render::theme::init(&config.display.theme, config.display.brand_color.as_deref());

    // Apply CLI overrides
    let explicit_model = cli.model.is_some();
    let provider = cli
        .provider
        .unwrap_or_else(|| config.general.default_provider.clone());
    let model = cli
        .model
        .unwrap_or_else(|| config.general.default_model.clone());

    match cli.command {
        Some(Commands::Chat { prompt, resume, tui, orchestrate, tasks, reflexion, metrics, timeline, full, expert }) => {
            commands::chat::run(
                &config, &provider, &model, prompt, resume, cli.no_banner, tui, explicit_model,
                commands::chat::FeatureFlags { orchestrate, tasks, reflexion, metrics, timeline, full, expert },
            ).await
        }
        Some(Commands::Config { action }) => match action {
            ConfigAction::Show => commands::config::show(&config),
            ConfigAction::Get { key } => commands::config::get(&config, &key),
            ConfigAction::Set { key, value } => commands::config::set(&key, &value),
            ConfigAction::Path => commands::config::path(),
        },
        Some(Commands::Init { force }) => commands::init::run(force).await,
        Some(Commands::Status) => commands::status::run(&config, &provider, &model).await,
        Some(Commands::Auth { action }) => match action {
            AuthAction::Login { provider: p } => commands::auth::login(&p),
            AuthAction::Logout { provider: p } => commands::auth::logout(&p),
            AuthAction::Status => commands::auth::status(),
        },
        Some(Commands::Trace { action }) => match action {
            TraceAction::Export { session_id } => {
                commands::trace::export(&session_id, None)
            }
        },
        Some(Commands::Replay { session_id, verify }) => {
            commands::trace::replay(&session_id, None, verify).await
        }
        Some(Commands::Memory { action }) => match action {
            MemoryAction::List { entry_type, limit } => {
                commands::memory::list(&config, entry_type.as_deref(), limit)
            }
            MemoryAction::Search { query, limit } => {
                commands::memory::search(&config, &query, limit)
            }
            MemoryAction::Prune { force } => commands::memory::prune(&config, force),
            MemoryAction::Stats => commands::memory::stats(&config),
        },
        Some(Commands::Doctor) => commands::doctor::run(&config, None),
        Some(Commands::Tools { action }) => match action {
            ToolsAction::Doctor => commands::tools::doctor(&config, None).await,
            ToolsAction::List => commands::tools::list(&config),
            ToolsAction::Validate => commands::tools::validate(&config),
        },
        Some(Commands::McpServer { working_dir }) => {
            commands::mcp_server::run(&config, working_dir.as_deref()).await
        }
        Some(Commands::Serve { host, port, token }) => {
            commands::serve::run(&host, port, token).await
        }
        None => {
            // Default: start interactive chat
            commands::chat::run(
                &config, &provider, &model, None, None, cli.no_banner, false, explicit_model,
                commands::chat::FeatureFlags::default(),
            ).await
        }
    }
}
