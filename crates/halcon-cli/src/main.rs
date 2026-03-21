// When building with --no-default-features (CI), most interactive modules are
// feature-gated out, leaving many items unused. These allows prevent -D warnings
// from failing the build for dead code that is alive under default features.
#![allow(
    dead_code,
    unused_imports,
    unused_variables,
    unused_assignments,
    unexpected_cfgs,
    private_interfaces,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::should_implement_trait,
    clippy::if_same_then_else,
    clippy::manual_strip,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    clippy::manual_clamp,
    clippy::nonminimal_bool,
    unreachable_patterns,
    clippy::len_without_is_empty,
    clippy::needless_question_mark,
    clippy::manual_let_else,
    clippy::format_in_format_args,
    clippy::unwrap_or_default,
    clippy::empty_line_after_doc_comments,
    clippy::manual_unwrap_or_default,
    clippy::question_mark,
    clippy::needless_range_loop,
    clippy::ptr_arg,
    clippy::enum_variant_names,
    clippy::derivable_impls,
    clippy::unnecessary_cast,
    clippy::needless_return
)]

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;

mod audit;
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

/// Output format for machine-readable CI/CD integration.
///
/// - `human`: Colored, ANSI-decorated terminal output (default)
/// - `json`:  NDJSON to stdout — one JSON object per line, parseable with `jq`
/// - `junit`: JUnit XML to stdout (reserved for future use)
/// - `plain`: Plain text, no color codes (for simple log capture)
///
/// See US-output-format (PASO 2-A).
#[derive(Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
    Junit,
    Plain,
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

    /// Output format for machine-readable CI/CD integration.
    ///
    /// `json` emits NDJSON to stdout (one object per line) — pipe to `jq` with no
    /// extra tooling. GitHub Actions, GitLab CI, and Datadog all consume NDJSON natively.
    #[arg(
        long,
        value_enum,
        default_value = "human",
        global = true,
        env = "HALCON_OUTPUT_FORMAT"
    )]
    output_format: OutputFormat,

    /// Operating mode: "interactive" (default) or "json-rpc" (for IDE extensions)
    ///
    /// In json-rpc mode Halcon reads newline-delimited JSON requests from stdin
    /// and writes streaming JSON events to stdout. All TUI/ANSI output is suppressed.
    #[arg(long, value_name = "MODE", env = "HALCON_MODE")]
    mode: Option<String>,

    /// Maximum agent loop turns (used by json-rpc mode and chat --max-turns)
    #[arg(long, value_name = "N", env = "HALCON_MAX_TURNS")]
    max_turns: Option<u32>,

    /// Air-gap mode: only the Ollama (localhost) provider is allowed.
    ///
    /// Enforced at the provider factory layer so sub-agents and orchestrator
    /// instances also respect the constraint. Sets HALCON_AIR_GAP=1 env var.
    ///
    /// Requires a running Ollama instance at http://localhost:11434 (or
    /// OLLAMA_BASE_URL env var).
    #[arg(long, env = "HALCON_AIR_GAP")]
    air_gap: bool,

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

    /// Manage declarative sub-agent configurations (Feature 4)
    Agents {
        #[command(subcommand)]
        action: AgentsAction,
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

    /// Manage MCP (Model Context Protocol) server connections
    ///
    /// Supports HTTP (OAuth 2.1 + PKCE) and stdio transports.
    /// Configuration is stored in three scopes: local > project > user.
    Mcp {
        #[command(subcommand)]
        action: McpAction,
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

        /// Release channel: stable (default), beta, or nightly
        #[arg(long, value_name = "CHANNEL", env = "HALCON_CHANNEL")]
        channel: Option<String>,
    },

    /// Manage V3 plugins
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },

    /// Compliance and audit export (SOC 2)
    ///
    /// Query existing session data and export it as JSONL, CSV, or PDF.
    /// Verify the HMAC-SHA256 hash chain to detect tampered audit records.
    Audit {
        #[command(subcommand)]
        action: AuditAction,
    },

    /// Manage user accounts and role assignments (RBAC)
    ///
    /// Provisions users in ~/.halcon/users.toml with roles:
    /// Admin | Developer | ReadOnly | AuditViewer
    Users {
        #[command(subcommand)]
        action: UsersAction,
    },

    /// Manage cron-based scheduled agent tasks (US-scheduler — PASO 4-C)
    ///
    /// Examples:
    ///   halcon schedule add --name "security-scan" --cron "0 2 * * 1" \
    ///                       --instruction "Scan for vulnerabilities"
    ///   halcon schedule list
    ///   halcon schedule disable <id>
    ///   halcon schedule run     <id>
    Schedule {
        #[command(subcommand)]
        action: ScheduleAction,
    },

    /// Cenzontle agent orchestration — delegate tasks to the multi-agent backend
    ///
    /// Connects to a running Cenzontle instance to execute agent tasks, query
    /// MCP tools, and search the knowledge base via RAG.
    ///
    /// Requires authentication: set CENZONTLE_ACCESS_TOKEN or run `halcon login cenzontle`.
    #[cfg(feature = "cenzontle-agents")]
    Cenzontle {
        #[command(subcommand)]
        action: commands::cenzontle::CenzontleAction,
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
    /// Clear auto-memory files for a given scope
    Clear {
        /// Memory scope to clear: 'project' (.halcon/memory/) or 'user' (~/.halcon/memory/<repo>/)
        #[arg(default_value = "project")]
        scope: String,
    },
}

/// MCP server management subcommands.
#[derive(Subcommand)]
enum McpAction {
    /// Add an HTTP or stdio MCP server to a scope
    Add {
        /// Server name (used in config and CLI references)
        name: String,
        /// HTTP server URL (use this OR --command, not both)
        #[arg(long, conflicts_with = "command")]
        url: Option<String>,
        /// stdio server executable (use this OR --url, not both)
        #[arg(long, conflicts_with = "url")]
        command: Option<String>,
        /// Arguments for the stdio command
        #[arg(long, num_args = 0.., requires = "command")]
        args: Vec<String>,
        /// Environment variables for the stdio command (KEY=VALUE)
        #[arg(long = "env", num_args = 0.., requires = "command")]
        env_vars: Vec<String>,
        /// Config scope: local, project, or user (default: project)
        #[arg(long, default_value = "project")]
        scope: String,
    },
    /// Remove an MCP server from a scope
    Remove {
        /// Server name to remove
        name: String,
        /// Scope to remove from (default: project)
        #[arg(long, default_value = "project")]
        scope: String,
    },
    /// List configured MCP servers
    List {
        /// Filter by scope: all, local, project, user (default: all)
        #[arg(long, default_value = "all")]
        scope: String,
    },
    /// Show detailed config for one server
    Get {
        /// Server name to inspect
        name: String,
    },
    /// Run OAuth 2.1 + PKCE authorization flow for an HTTP server
    Auth {
        /// Server name to authorize
        name: String,
    },
    /// Expose Halcon's tools as an MCP server (Feature 9)
    Serve {
        /// Transport mode: stdio (default) or http
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// HTTP port (only used with --transport http, default: 7777)
        #[arg(long, default_value_t = 7777)]
        port: u16,
    },
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
enum AgentsAction {
    /// List all registered sub-agent definitions
    List {
        /// Show the full routing manifest injected into the system prompt
        #[arg(short, long)]
        verbose: bool,
    },
    /// Validate agent definition files and report all errors
    Validate {
        /// Specific .md files to validate (default: discover from all scopes)
        paths: Vec<std::path::PathBuf>,
    },
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
enum AuditAction {
    /// Export session audit events as JSONL, CSV, or PDF
    Export {
        /// Session UUID to export (exclusive with --since)
        #[arg(long, short = 's')]
        session: Option<String>,

        /// Export all sessions starting after this ISO-8601 timestamp (exclusive with --session)
        #[arg(long)]
        since: Option<String>,

        /// Output format: jsonl (default), csv, pdf
        #[arg(long, short = 'f', default_value = "jsonl")]
        format: String,

        /// Output file path (default: stdout for jsonl/csv; auto-named for pdf)
        #[arg(long, short = 'o')]
        output: Option<std::path::PathBuf>,

        /// Include raw tool inputs in exported payload
        #[arg(long)]
        include_tool_inputs: bool,

        /// Include raw tool outputs in exported payload
        #[arg(long)]
        include_tool_outputs: bool,

        /// Override database path (default: ~/.halcon/halcon.db)
        #[arg(long)]
        db: Option<std::path::PathBuf>,
    },

    /// List all sessions with compliance summary statistics
    List {
        /// Emit JSON instead of the default table view
        #[arg(long)]
        json: bool,

        /// Override database path (default: ~/.halcon/halcon.db)
        #[arg(long)]
        db: Option<std::path::PathBuf>,
    },

    /// Verify the HMAC-SHA256 hash chain for a session
    Verify {
        /// Session UUID to verify
        session_id: String,

        /// Override database path (default: ~/.halcon/halcon.db)
        #[arg(long)]
        db: Option<std::path::PathBuf>,
    },

    /// Generate a compliance report (SOC 2 / FedRAMP / ISO 27001)
    Compliance {
        /// Compliance framework: soc2, fedramp, or iso27001
        #[arg(long, short = 'f', default_value = "soc2")]
        format: String,

        /// Output file path (default: halcon-compliance-<format>-<date>.pdf)
        #[arg(long, short = 'o')]
        output: Option<std::path::PathBuf>,

        /// Start of reporting period (YYYY-MM-DD, default: 30 days ago)
        #[arg(long)]
        from: Option<String>,

        /// End of reporting period (YYYY-MM-DD, default: today)
        #[arg(long)]
        to: Option<String>,

        /// Override database path (default: ~/.halcon/halcon.db)
        #[arg(long)]
        db: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
enum UsersAction {
    /// Provision a new user with a role
    Add {
        /// User email address
        #[arg(long)]
        email: String,
        /// Role to assign: Admin, Developer, ReadOnly, or AuditViewer
        #[arg(long)]
        role: String,
    },
    /// List all provisioned users and their roles
    List,
    /// Revoke a user's access (soft delete, record retained for audit)
    Revoke {
        /// User email address to revoke
        #[arg(long)]
        email: String,
    },
}

/// Actions for `halcon schedule` (PASO 4-C).
#[derive(Subcommand)]
enum ScheduleAction {
    /// Add a new scheduled task
    Add {
        /// Human-readable name for this task
        #[arg(long)]
        name: String,
        /// Standard cron expression (5-field, e.g., "0 2 * * 1")
        #[arg(long)]
        cron: String,
        /// Natural-language instruction for the agent to execute
        #[arg(long)]
        instruction: String,
        /// Optional agent definition ID to use
        #[arg(long)]
        agent: Option<String>,
    },
    /// List all scheduled tasks
    List,
    /// Disable a scheduled task (stops running but keeps the record)
    Disable {
        /// Task ID
        id: String,
    },
    /// Re-enable a disabled scheduled task
    Enable {
        /// Task ID
        id: String,
    },
    /// Force-run a task immediately (ignores the cron schedule)
    Run {
        /// Task ID
        id: String,
    },
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
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        let message = info
            .payload()
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
        cli.log_level // default: "warn" (see --log-level flag default_value)
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
                .with_ansi(false) // no color codes in the log file
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

    // DECISION: In --air-gap mode we set HALCON_AIR_GAP=1 at the process level
    // so that every code path that creates a ProviderRegistry (including sub-agents
    // and the MCP server) automatically respects the constraint.
    // The env var is the single chokepoint: provider_factory::build_registry() checks
    // it and discards all non-Ollama providers at construction time.
    if cli.air_gap {
        // Ensure OLLAMA_BASE_URL defaults to localhost:11434 when not set.
        if std::env::var("OLLAMA_BASE_URL").is_err() {
            std::env::set_var("OLLAMA_BASE_URL", "http://localhost:11434");
        }
        // Propagate air-gap flag to child processes and sub-agents.
        std::env::set_var("HALCON_AIR_GAP", "1");

        // Display the air-gap banner before any further output.
        eprintln!("┌─────────────────────────────────────────────────┐");
        eprintln!("│  ⚠  MODO AIR-GAP ACTIVO — Sin conexiones externas │");
        eprintln!("│     Proveedor: Ollama (localhost:11434)           │");
        eprintln!("└─────────────────────────────────────────────────┘");
    }

    // Load configuration
    let config = config_loader::load_config(cli.config.as_deref())
        .context("Failed to load configuration")?;

    // Initialize the terminal design system theme.
    render::theme::init(&config.display.theme, config.display.brand_color.as_deref());

    // Apply CLI overrides
    let explicit_model = cli.model.is_some();
    let explicit_provider = cli.provider.is_some();
    let provider = cli
        .provider
        .unwrap_or_else(|| config.general.default_provider.clone());
    let model = cli
        .model
        .unwrap_or_else(|| config.general.default_model.clone());

    // JSON-RPC mode: activated by `halcon --mode json-rpc` (used by the VS Code extension).
    // Bypasses all subcommand handling — runs an async stdin/stdout JSON-RPC server.
    if cli.mode.as_deref() == Some("json-rpc") {
        return commands::json_rpc::run(&config, &provider, &model, cli.max_turns, explicit_model)
            .await;
    }

    // Background update check + one-line hint (non-TUI, non-CI, non-JSON mode only).
    if !is_tui_mode && !cli.no_banner && cli.output_format != OutputFormat::Json {
        commands::update::notify_if_update_available();
        commands::update::print_update_hint();
    }

    match cli.command {
        Some(Commands::Chat {
            prompt,
            resume,
            tui,
            orchestrate,
            tasks,
            reflexion,
            metrics,
            timeline,
            full,
            expert,
            trace_out,
            trace_in,
        }) => {
            commands::chat::run(
                &config,
                &provider,
                &model,
                prompt,
                resume,
                cli.no_banner,
                tui,
                explicit_model,
                explicit_provider,
                commands::chat::FeatureFlags {
                    orchestrate,
                    tasks,
                    reflexion,
                    metrics,
                    timeline,
                    full,
                    expert,
                    background_tools: false,
                    trace_out,
                    trace_in,
                },
                cli.output_format == OutputFormat::Json,
            )
            .await
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
            AuthAction::Login { provider: p } if p.eq_ignore_ascii_case("cenzontle") => {
                commands::sso::login().await
            }
            AuthAction::Login { provider: p } => commands::auth::login(&p),
            AuthAction::Logout { provider: p } if p.eq_ignore_ascii_case("cenzontle") => {
                commands::sso::logout()
            }
            AuthAction::Logout { provider: p } => commands::auth::logout(&p),
            AuthAction::Status => commands::auth::status(),
        },
        Some(Commands::Trace { action }) => match action {
            TraceAction::Export { session_id } => commands::trace::export(&session_id, None),
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
            MemoryAction::Clear { scope } => {
                let working_dir =
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let repo_name = working_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                commands::memory::clear(&scope, &working_dir, &repo_name)
            }
        },
        Some(Commands::Metrics { action }) => match action {
            MetricsAction::Show { recent } => {
                commands::metrics::show_baseline(recent).await?;
                Ok(())
            }
            MetricsAction::Export { output } => {
                commands::metrics::export_baselines(output.clone()).await?;
                Ok(())
            }
            MetricsAction::Prune { keep } => {
                commands::metrics::prune_baselines(keep).await?;
                Ok(())
            }
            MetricsAction::Decide => {
                commands::metrics::decision_report().await?;
                Ok(())
            }
        },
        Some(Commands::Agents { action }) => {
            let working_dir = std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .display()
                .to_string();
            match action {
                AgentsAction::List { verbose } => commands::agents::list(&working_dir, verbose),
                AgentsAction::Validate { paths } => {
                    commands::agents::validate(&working_dir, &paths)
                }
            }
        }
        Some(Commands::Doctor) => commands::doctor::run(&config, None),
        Some(Commands::Tools { action }) => match action {
            ToolsAction::Doctor => commands::tools::doctor(&config, None).await,
            ToolsAction::List => commands::tools::list(&config),
            ToolsAction::Validate => commands::tools::validate(&config),
            ToolsAction::Add {
                name,
                command,
                description,
                permission,
            } => commands::tools::add_tool(&name, &command, &description, &permission),
            ToolsAction::Remove { name, force } => commands::tools::remove_tool(&name, force),
        },
        Some(Commands::Lsp) => commands::lsp::run_lsp_server().await,
        Some(Commands::McpServer { working_dir }) => {
            commands::mcp_server::run(&config, working_dir.as_deref()).await
        }
        Some(Commands::Mcp { action }) => {
            let working_dir =
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            match action {
                McpAction::Add {
                    name,
                    url,
                    command,
                    args,
                    env_vars,
                    scope,
                } => {
                    if let Some(url) = url {
                        commands::mcp::add_http(
                            &name,
                            &url,
                            &scope,
                            std::collections::HashMap::new(),
                            &working_dir,
                        )
                    } else if let Some(cmd) = command {
                        commands::mcp::add_stdio(&name, &cmd, args, env_vars, &scope, &working_dir)
                    } else {
                        Err(anyhow::anyhow!("Specify --url (HTTP) or --command (stdio)"))
                    }
                }
                McpAction::Remove { name, scope } => {
                    commands::mcp::remove(&name, &scope, &working_dir)
                }
                McpAction::List { scope } => commands::mcp::list(&scope, &working_dir),
                McpAction::Get { name } => commands::mcp::get(&name, &working_dir),
                McpAction::Auth { name } => commands::mcp::auth(&name, &working_dir).await,
                McpAction::Serve { transport, port } => {
                    commands::mcp_serve::run(&config, Some(transport.as_str()), Some(port)).await
                }
            }
        }
        Some(Commands::Theme(args)) => commands::theme::run(args),
        Some(Commands::Update {
            check,
            force,
            version,
            channel,
        }) => tokio::task::spawn_blocking(move || {
            commands::update::run(commands::update::UpdateArgs {
                check,
                force,
                version,
                channel,
            })
        })
        .await
        .context("update task panicked")?,
        Some(Commands::Plugin { action }) => match action {
            PluginAction::List => commands::plugin::list(&config),
            PluginAction::Install { source } => commands::plugin::install(&config, &source),
            PluginAction::Remove { id, force } => commands::plugin::remove(&config, &id, force),
            PluginAction::Status => commands::plugin::status(&config),
        },
        Some(Commands::Serve { host, port, token }) => {
            commands::serve::run(&host, port, token).await
        }
        Some(Commands::Audit { action }) => match action {
            AuditAction::Export {
                session,
                since,
                format,
                output,
                include_tool_inputs,
                include_tool_outputs,
                db,
            } => commands::audit::export(
                session,
                since,
                &format,
                output,
                include_tool_inputs,
                include_tool_outputs,
                db,
            ),
            AuditAction::List { json, db } => commands::audit::list(db, json),
            AuditAction::Verify { session_id, db } => commands::audit::verify(&session_id, db),
            AuditAction::Compliance {
                format,
                output,
                from,
                to,
                db,
            } => commands::audit::compliance(&format, output, from, to, db),
        },
        Some(Commands::Users { action }) => match action {
            UsersAction::Add { email, role } => commands::users::add(&email, &role),
            UsersAction::List => commands::users::list(),
            UsersAction::Revoke { email } => commands::users::revoke(&email),
        },
        // PASO 4-C: cron-based scheduled agent tasks (US-scheduler)
        Some(Commands::Schedule { action }) => {
            let db_path = config.storage.database_path.clone().unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join(".halcon")
                    .join("halcon.db")
            });
            let db = halcon_storage::Database::open(&db_path)
                .context("failed to open database for schedule command")?;
            match action {
                ScheduleAction::Add {
                    name,
                    cron,
                    instruction,
                    agent,
                } => commands::schedule::add(&db, &name, &cron, &instruction, agent.as_deref()),
                ScheduleAction::List => commands::schedule::list(&db),
                ScheduleAction::Disable { id } => commands::schedule::disable(&db, &id),
                ScheduleAction::Enable { id } => commands::schedule::enable(&db, &id),
                ScheduleAction::Run { id } => commands::schedule::run_now(&db, &id),
            }
        }
        #[cfg(feature = "cenzontle-agents")]
        Some(Commands::Cenzontle { action }) => commands::cenzontle::run(action).await,
        None => {
            // Default: start interactive chat
            commands::chat::run(
                &config,
                &provider,
                &model,
                None,
                None,
                cli.no_banner,
                false,
                explicit_model,
                explicit_provider,
                commands::chat::FeatureFlags::default(),
                cli.output_format == OutputFormat::Json,
            )
            .await
        }
    }
}
