use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod commands;
mod config_loader;
mod render;
mod repl;
mod tui;

#[cfg(feature = "headless")]
mod agent_bridge;

/// Build version string with git hash and build date (leaked to 'static).
fn build_version() -> &'static str {
    let s = format!(
        "{} ({} {}, {})",
        env!("CARGO_PKG_VERSION"),
        env!("HALCON_GIT_HASH"),
        env!("HALCON_BUILD_DATE"),
        env!("HALCON_TARGET"),
    );
    Box::leak(s.into_boxed_str())
}

/// Halcon — AI-powered CLI for software development.
///
/// Multi-model, self-hosted, open source.
#[derive(Parser)]
#[command(name = "halcon", version = build_version(), about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    /// Model to use (e.g., "claude-sonnet-4-5-20250929", "llama3.2")
    #[arg(short, long, env = "HALCON_MODEL")]
    model: Option<String>,

    /// Provider to use (e.g., "anthropic", "ollama")
    #[arg(short, long, env = "HALCON_PROVIDER")]
    provider: Option<String>,

    /// Enable verbose output (sets log level to debug)
    #[arg(short, long)]
    verbose: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "warn", env = "HALCON_LOG")]
    log_level: String,

    /// Emit traces as JSON lines to stderr (for offline debugging / piping)
    #[arg(long)]
    trace_json: bool,

    /// Configuration file path
    #[arg(long, env = "HALCON_CONFIG")]
    config: Option<String>,

    /// Suppress the startup banner
    #[arg(long, env = "HALCON_NO_BANNER")]
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
        #[arg(long, env = "HALCON_TUI")]
        tui: bool,

        /// Enable multi-agent orchestration
        #[arg(long, env = "HALCON_ORCHESTRATE")]
        orchestrate: bool,

        /// Enable structured task framework
        #[arg(long, env = "HALCON_TASKS")]
        tasks: bool,

        /// Enable self-improvement reflexion loop
        #[arg(long, env = "HALCON_REFLEXION")]
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
        #[arg(long, env = "HALCON_EXPERT")]
        expert: bool,

        /// Write execution timeline as JSONL to this file (P0.2 — human-editable trace)
        #[arg(long, value_name = "PATH")]
        trace_out: Option<std::path::PathBuf>,

        /// Import and display a JSONL trace file before starting the session (P2.1)
        #[arg(long, value_name = "PATH")]
        trace_in: Option<std::path::PathBuf>,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Initialize Halcon in the current project
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

    /// Metrics and baseline analysis
    Metrics {
        #[command(subcommand)]
        action: MetricsAction,
    },

    /// Run runtime health diagnostics
    Doctor,

    /// Tool diagnostics and health checks
    Tools {
        #[command(subcommand)]
        action: ToolsAction,
    },

    /// Start a Language Server Protocol (LSP) server over stdio
    ///
    /// IDE extensions launch this process and communicate via the standard
    /// Content-Length framing protocol. Supports textDocument/* notifications
    /// and custom $/halcon/* methods for context injection.
    Lsp,

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
        #[arg(long, env = "HALCON_API_TOKEN")]
        token: Option<String>,
    },

    /// Theme generation and optimization
    Theme(commands::theme::ThemeArgs),

    /// Update Halcon CLI to the latest version
    Update {
        /// Only check if an update is available; don't download
        #[arg(long, short = 'c')]
        check: bool,

        /// Force update even if already on the latest version
        #[arg(long, short = 'f')]
        force: bool,

        /// Install a specific version (e.g., "v0.3.0")
        #[arg(long, short = 'V', value_name = "VERSION")]
        version: Option<String>,
    },

    /// Manage V3 plugins
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
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
enum MetricsAction {
    /// Show metrics baseline report
    Show {
        /// Number of recent baselines to include (default: all)
        #[arg(short, long)]
        recent: Option<usize>,
    },
    /// Export baselines to JSON file
    Export {
        /// Output file path
        output: String,
    },
    /// Prune old baseline data
    Prune {
        /// Number of recent baselines to keep
        #[arg(default_value = "100")]
        keep: usize,
    },
    /// Generate integration decision based on baselines
    Decide,
}

#[derive(Subcommand)]
enum ToolsAction {
    /// Run tool health diagnostics (schema validation, probes, metrics)
    Doctor,
    /// List all available tools with permission levels
    List,
    /// Validate all tool schemas in detail
    Validate,
    /// Add a custom tool manifest to ~/.halcon/tools/
    Add {
        /// Tool name (no spaces; becomes the manifest filename)
        name: String,
        /// Shell command template (use {{arg_name}} for argument substitution)
        #[arg(short, long)]
        command: String,
        /// Tool description shown to the agent
        #[arg(short, long)]
        description: String,
        /// Permission level: "ReadOnly" or "Destructive"
        #[arg(short, long, default_value = "Destructive")]
        permission: String,
    },
    /// Remove a custom tool manifest from ~/.halcon/tools/
    Remove {
        /// Tool name to remove
        name: String,
        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum PluginAction {
    /// List all installed plugins in ~/.halcon/plugins/
    List,
    /// Install a plugin from a .plugin.toml manifest file
    Install {
        /// Path to the .plugin.toml manifest to install
        source: String,
    },
    /// Remove an installed plugin by ID
    Remove {
        /// Plugin ID (e.g. "git-enhanced")
        id: String,
        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },
    /// Show plugin system status (directory, manifest count)
    Status,
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

/// Install a panic hook that restores the terminal before printing the panic message.
///
/// In TUI mode (raw mode + alternate screen) a panic without cleanup leaves the terminal
/// in a broken state: raw mode enabled, alternate screen active, lingering ANSI escape codes.
/// Since release builds use `panic = "abort"`, the `Drop` impl on `TuiApp` never executes.
/// This hook fires before the process aborts and performs the minimal teardown.
#[cfg(feature = "tui")]
fn install_panic_hook() {
    use crossterm::{
        event::{DisableMouseCapture, PopKeyboardEnhancementFlags},
        execute,
        terminal::{self, LeaveAlternateScreen},
    };
    use std::io::Write;

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Best-effort terminal restore — ignore errors.
        let _ = terminal::disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen);
        let _ = execute!(stdout, DisableMouseCapture);
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        // Flush stdout so the escape sequences are sent before the error.
        let _ = stdout.flush();

        // Print a clean human-readable error to stderr (no ANSI leakage).
        let location = info.location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        let message = info.payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str()))
            .unwrap_or("unknown panic");
        eprintln!("\nhalcon crashed at {location}:\n  {message}");
        eprintln!("Log: ~/.local/share/halcon/halcon.log");
        eprintln!("Report: https://github.com/cuervo-ai/halcon-cli/issues\n");

        // Delegate to the default hook for full backtrace when RUST_BACKTRACE is set.
        default_hook(info);
    }));
}

#[cfg(not(feature = "tui"))]
fn install_panic_hook() {}

#[tokio::main]
async fn main() -> Result<()> {
    // Install panic hook early so any subsequent panic restores the terminal cleanly.
    install_panic_hook();

    // Migrate ~/.cuervo/ → ~/.halcon/ for existing users upgrading from cuervo.
    config_loader::migrate_legacy_dir();

    let cli = Cli::parse();

    // Detect TUI mode early — must happen BEFORE logging init so we can redirect output.
    // When TUI is active, ratatui owns the terminal in raw mode; any bytes written to
    // stderr bleed directly into the rendered layout, corrupting the display.
    let is_tui_mode = matches!(
        &cli.command,
        Some(Commands::Chat { tui: true, .. }) | Some(Commands::Chat { full: true, .. })
    ) && !cli.trace_json;

    // Initialize logging. --verbose overrides --log-level. --trace-json enables JSON output.
    // In TUI mode (without --verbose): redirect ALL output to a log file so nothing
    // bleeds into the terminal. --verbose bypasses this to keep debug output visible.
    let log_level = if cli.verbose || cli.trace_json {
        "debug".to_string()
    } else {
        cli.log_level  // default: "warn" (see --log-level flag default_value)
    };

    if is_tui_mode && !cli.verbose {
        // ── TUI mode: route tracing to log file, never to the terminal ────────
        // Only errors are logged (warnings are informational, not actionable).
        // Log file: ~/.local/share/halcon/halcon.log — inspect for debugging.
        let log_dir = dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("halcon");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("halcon.log");

        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            tracing_subscriber::fmt()
                .with_env_filter(EnvFilter::new("error"))
                .with_target(false)
                .with_ansi(false)   // no color codes in the log file
                .with_writer(std::sync::Mutex::new(file))
                .init();
        } else {
            // Fallback: stderr but error-only — at least minimize TUI corruption.
            tracing_subscriber::fmt()
                .with_env_filter(EnvFilter::new("error"))
                .with_target(false)
                .with_writer(std::io::stderr)
                .init();
        }
    } else {
        // ── Non-TUI mode: write to stderr as before ────────────────────────────
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
        Some(Commands::Chat { prompt, resume, tui, orchestrate, tasks, reflexion, metrics, timeline, full, expert, trace_out, trace_in }) => {
            commands::chat::run(
                &config, &provider, &model, prompt, resume, cli.no_banner, tui, explicit_model,
                commands::chat::FeatureFlags { orchestrate, tasks, reflexion, metrics, timeline, full, expert, background_tools: false, trace_out, trace_in },
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
        Some(Commands::Metrics { action }) => match action {
            MetricsAction::Show { recent } => {
                commands::metrics::show_baseline(recent.clone()).await?;
                Ok(())
            }
            MetricsAction::Export { output } => {
                commands::metrics::export_baselines(output.clone()).await?;
                Ok(())
            }
            MetricsAction::Prune { keep } => {
                commands::metrics::prune_baselines(keep.clone()).await?;
                Ok(())
            }
            MetricsAction::Decide => {
                commands::metrics::decision_report().await?;
                Ok(())
            }
        },
        Some(Commands::Doctor) => commands::doctor::run(&config, None),
        Some(Commands::Tools { action }) => match action {
            ToolsAction::Doctor => commands::tools::doctor(&config, None).await,
            ToolsAction::List => commands::tools::list(&config),
            ToolsAction::Validate => commands::tools::validate(&config),
            ToolsAction::Add { name, command, description, permission } => {
                commands::tools::add_tool(&name, &command, &description, &permission)
            }
            ToolsAction::Remove { name, force } => {
                commands::tools::remove_tool(&name, force)
            }
        },
        Some(Commands::Lsp) => {
            commands::lsp::run_lsp_server().await
        }
        Some(Commands::McpServer { working_dir }) => {
            commands::mcp_server::run(&config, working_dir.as_deref()).await
        }
        Some(Commands::Theme(args)) => {
            commands::theme::run(args)
        }
        Some(Commands::Update { check, force, version }) => {
            commands::update::run(commands::update::UpdateArgs { check, force, version })
        }
        Some(Commands::Plugin { action }) => match action {
            PluginAction::List => commands::plugin::list(&config),
            PluginAction::Install { source } => commands::plugin::install(&config, &source),
            PluginAction::Remove { id, force } => commands::plugin::remove(&config, &id, force),
            PluginAction::Status => commands::plugin::status(&config),
        },
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
