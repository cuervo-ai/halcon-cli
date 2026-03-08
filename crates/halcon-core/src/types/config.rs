use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::orchestrator::OrchestratorConfig;

/// Severity of a configuration issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueLevel {
    Error,
    Warning,
}

impl fmt::Display for IssueLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IssueLevel::Error => write!(f, "error"),
            IssueLevel::Warning => write!(f, "warning"),
        }
    }
}

/// A single configuration issue found during validation.
#[derive(Debug, Clone)]
pub struct ConfigIssue {
    pub level: IssueLevel,
    pub field: String,
    pub message: String,
    pub suggestion: Option<String>,
}

/// Validate an `AppConfig` and return all issues found.
///
/// Returns an empty vec if the config is valid.
pub fn validate_config(config: &AppConfig) -> Vec<ConfigIssue> {
    let mut issues = Vec::new();

    // Temperature range.
    if config.general.temperature < 0.0 || config.general.temperature > 2.0 {
        issues.push(ConfigIssue {
            level: IssueLevel::Error,
            field: "general.temperature".into(),
            message: format!(
                "temperature {} is out of range, must be between 0.0 and 2.0",
                config.general.temperature
            ),
            suggestion: Some("Set temperature to a value between 0.0 and 2.0".into()),
        });
    }

    // Max tokens.
    if config.general.max_tokens == 0 {
        issues.push(ConfigIssue {
            level: IssueLevel::Error,
            field: "general.max_tokens".into(),
            message: "max_tokens must be greater than 0".into(),
            suggestion: Some("Set max_tokens to a positive value (e.g., 8192)".into()),
        });
    }

    // Tool timeout.
    if config.tools.timeout_secs == 0 {
        issues.push(ConfigIssue {
            level: IssueLevel::Error,
            field: "tools.timeout_secs".into(),
            message: "tool timeout must be greater than 0".into(),
            suggestion: Some("Set tools.timeout_secs to a positive value (e.g., 120)".into()),
        });
    }

    // Provider configs.
    for (name, provider_cfg) in &config.models.providers {
        if provider_cfg.enabled {
            if let Some(ref base) = provider_cfg.api_base {
                if base.is_empty() {
                    issues.push(ConfigIssue {
                        level: IssueLevel::Error,
                        field: format!("models.providers.{name}.api_base"),
                        message: "api_base is empty for enabled provider".into(),
                        suggestion: Some(format!("Set a valid API base URL for '{name}'")),
                    });
                }
            }

            // HTTP config validation.
            if provider_cfg.http.connect_timeout_secs == 0 {
                issues.push(ConfigIssue {
                    level: IssueLevel::Warning,
                    field: format!("models.providers.{name}.http.connect_timeout_secs"),
                    message: "connect timeout is 0 (no timeout)".into(),
                    suggestion: Some("Set a reasonable connect timeout (e.g., 10)".into()),
                });
            }
            if provider_cfg.http.request_timeout_secs == 0 {
                issues.push(ConfigIssue {
                    level: IssueLevel::Warning,
                    field: format!("models.providers.{name}.http.request_timeout_secs"),
                    message: "request timeout is 0 (no timeout)".into(),
                    suggestion: Some("Set a reasonable request timeout (e.g., 300)".into()),
                });
            }
        }
    }

    // Resilience enabled but no fallback models.
    if config.resilience.enabled && config.agent.routing.fallback_models.is_empty() {
        issues.push(ConfigIssue {
            level: IssueLevel::Warning,
            field: "resilience + agent.routing.fallback_models".into(),
            message: "resilience is enabled but no fallback models are configured".into(),
            suggestion: Some(
                "Add fallback_models to [agent.routing] for failover support".into(),
            ),
        });
    }

    // Cache enabled but TTL is 0.
    if config.cache.enabled && config.cache.default_ttl_secs == 0 {
        issues.push(ConfigIssue {
            level: IssueLevel::Warning,
            field: "cache.default_ttl_secs".into(),
            message: "cache is enabled but TTL is 0 (entries never expire)".into(),
            suggestion: Some("Set a TTL (e.g., 3600) or disable cache if not needed".into()),
        });
    }

    // Compaction threshold too high.
    if config.agent.compaction.enabled && config.agent.compaction.threshold_fraction > 0.95 {
        issues.push(ConfigIssue {
            level: IssueLevel::Warning,
            field: "agent.compaction.threshold_fraction".into(),
            message: format!(
                "compaction threshold {:.2} is very high — compaction may trigger too late",
                config.agent.compaction.threshold_fraction
            ),
            suggestion: Some("Set threshold_fraction to 0.80 or lower for safer compaction".into()),
        });
    }

    // Provider timeout too low.
    if config.agent.limits.provider_timeout_secs > 0
        && config.agent.limits.provider_timeout_secs < 30
    {
        issues.push(ConfigIssue {
            level: IssueLevel::Warning,
            field: "agent.limits.provider_timeout_secs".into(),
            message: format!(
                "provider timeout {}s is very low — requests may time out prematurely",
                config.agent.limits.provider_timeout_secs
            ),
            suggestion: Some("Set provider_timeout_secs to at least 30 for reliable inference".into()),
        });
    }

    // Max parallel tools of 0.
    if config.agent.limits.max_parallel_tools == 0 {
        issues.push(ConfigIssue {
            level: IssueLevel::Warning,
            field: "agent.limits.max_parallel_tools".into(),
            message: "max_parallel_tools is 0 — will default to 1 (no parallelism)".into(),
            suggestion: Some("Set max_parallel_tools to a positive value (e.g., 10)".into()),
        });
    }

    // Unbounded API spend risk: no token budget AND no duration budget.
    if config.agent.limits.max_total_tokens == 0 && config.agent.limits.max_duration_secs == 0 {
        issues.push(ConfigIssue {
            level: IssueLevel::Warning,
            field: "agent.limits".into(),
            message: "no token or duration budget set — API spend is unbounded".into(),
            suggestion: Some(
                "Set max_total_tokens or max_duration_secs for cost control".into(),
            ),
        });
    }

    // Brand color hex format.
    if let Some(ref hex) = config.display.brand_color {
        let valid = hex.starts_with('#')
            && (hex.len() == 4 || hex.len() == 7)
            && hex[1..].chars().all(|c| c.is_ascii_hexdigit());
        if !valid {
            issues.push(ConfigIssue {
                level: IssueLevel::Warning,
                field: "display.brand_color".into(),
                message: format!("invalid hex color '{hex}' — expected #RGB or #RRGGBB format"),
                suggestion: Some("Use a hex color like \"#0066cc\" or remove brand_color".into()),
            });
        }
    }

    // Terminal background hex format.
    {
        let hex = &config.display.terminal_background;
        let valid = hex.starts_with('#')
            && (hex.len() == 4 || hex.len() == 7)
            && hex[1..].chars().all(|c| c.is_ascii_hexdigit());
        if !valid {
            issues.push(ConfigIssue {
                level: IssueLevel::Warning,
                field: "display.terminal_background".into(),
                message: format!("invalid hex color '{hex}' — expected #RGB or #RRGGBB format"),
                suggestion: Some("Use a hex color like \"#1a1a1a\" for dark or \"#ffffff\" for light".into()),
            });
        }
    }

    // Orchestrator: max_concurrent_agents of 0.
    if config.orchestrator.enabled && config.orchestrator.max_concurrent_agents == 0 {
        issues.push(ConfigIssue {
            level: IssueLevel::Warning,
            field: "orchestrator.max_concurrent_agents".into(),
            message: "max_concurrent_agents is 0 — will default to 1".into(),
            suggestion: Some("Set max_concurrent_agents to a positive value (e.g., 3)".into()),
        });
    }

    // Orchestrator: sub-agent timeout exceeds parent duration.
    if config.orchestrator.enabled
        && config.orchestrator.sub_agent_timeout_secs > 0
        && config.agent.limits.max_duration_secs > 0
        && config.orchestrator.sub_agent_timeout_secs > config.agent.limits.max_duration_secs
    {
        issues.push(ConfigIssue {
            level: IssueLevel::Warning,
            field: "orchestrator.sub_agent_timeout_secs".into(),
            message: format!(
                "sub-agent timeout {}s exceeds parent max_duration_secs {}s",
                config.orchestrator.sub_agent_timeout_secs,
                config.agent.limits.max_duration_secs,
            ),
            suggestion: Some("Sub-agent timeout should be less than parent duration budget".into()),
        });
    }

    // Orchestrator: communication enabled without orchestrator enabled.
    if config.orchestrator.enable_communication && !config.orchestrator.enabled {
        issues.push(ConfigIssue {
            level: IssueLevel::Warning,
            field: "orchestrator.enable_communication".into(),
            message: "inter-agent communication is enabled but orchestrator is disabled".into(),
            suggestion: Some("Set orchestrator.enabled = true or disable enable_communication".into()),
        });
    }

    // Dry-run with confirm_destructive disabled.
    if config.tools.dry_run && !config.tools.confirm_destructive {
        issues.push(ConfigIssue {
            level: IssueLevel::Warning,
            field: "tools.dry_run + tools.confirm_destructive".into(),
            message: "dry_run is enabled but confirm_destructive is false — destructive ops have no guard".into(),
            suggestion: Some("Enable confirm_destructive for an extra safety layer".into()),
        });
    }

    // MCP reconnect attempts too high.
    if config.mcp.max_reconnect_attempts > 10 {
        issues.push(ConfigIssue {
            level: IssueLevel::Warning,
            field: "mcp.max_reconnect_attempts".into(),
            message: format!(
                "max_reconnect_attempts {} is very high — may cause long delays on failures",
                config.mcp.max_reconnect_attempts
            ),
            suggestion: Some("Set max_reconnect_attempts to 10 or lower".into()),
        });
    }

    issues
}

/// Configuration for the multimodal subsystem.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MultimodalConfig {
    /// Enable multimodal processing (default: false).
    pub enabled: bool,
    /// Routing mode: "api", "local", or "hybrid" (default: "hybrid").
    pub mode: String,
    /// Maximum file size in bytes (default: 20 MB).
    pub max_file_size_bytes: u64,
    /// Files smaller than this go to local inference when available (default: 2 MB).
    pub local_threshold_bytes: u64,
    /// Strip EXIF metadata before upload (default: true).
    pub strip_exif: bool,
    /// Reject local file paths in strict privacy mode (default: false).
    pub privacy_strict: bool,
    /// Maximum audio duration in seconds (default: 300).
    pub max_audio_duration_secs: u32,
    /// Maximum video duration in seconds (default: 60).
    pub max_video_duration_secs: u32,
    /// Video sampling rate in frames per second (default: 1).
    pub video_sample_fps: u32,
    /// Maximum number of video frames to extract (default: 10).
    pub max_video_frames: u32,
    /// Enable content-addressed caching (default: true).
    pub cache_enabled: bool,
    /// Cache TTL in seconds (default: 3600).
    pub cache_ttl_secs: u64,
    /// Directory containing ONNX model files (optional).
    pub models_dir: Option<String>,
    /// API call timeout in milliseconds (default: 30000).
    pub api_timeout_ms: u64,
    /// Maximum simultaneous media analyses (prevents DoS from bulk uploads).
    /// Default: 4. Set 0 to disable limiting.
    #[serde(default = "default_max_concurrent_analyses")]
    pub max_concurrent_analyses: u32,
}

fn default_max_concurrent_analyses() -> u32 { 4 }

impl Default for MultimodalConfig {
    fn default() -> Self {
        Self {
            enabled:                  false,
            mode:                     "hybrid".to_string(),
            max_file_size_bytes:      20 * 1024 * 1024,
            local_threshold_bytes:    2 * 1024 * 1024,
            strip_exif:               true,
            privacy_strict:           false,
            max_audio_duration_secs:  300,
            max_video_duration_secs:  60,
            video_sample_fps:         1,
            max_video_frames:         10,
            cache_enabled:            true,
            cache_ttl_secs:           3600,
            models_dir:               None,
            api_timeout_ms:           30_000,
            max_concurrent_analyses:  4,
        }
    }
}

/// Halcon-as-MCP-server configuration (Feature 9).
///
/// Configures how Halcon exposes its tools and agents to external MCP clients.
/// When `enabled = true`, start the server with `halcon mcp serve`.
/// Distinct from `McpServerConfig` (which configures client connections to external MCP servers).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServeConfig {
    /// Enable the MCP server (default: false).
    #[serde(default)]
    pub enabled: bool,
    /// Transport mode: "stdio" or "http" (default: "stdio").
    #[serde(default = "default_mcp_transport")]
    pub transport: String,
    /// HTTP port for http transport (default: 7777).
    #[serde(default = "default_mcp_port")]
    pub port: u16,
    /// Expose agent-registry agents as `agent_*` MCP tools (default: true).
    #[serde(default = "default_true")]
    pub expose_agents: bool,
    /// Require Bearer token auth in http transport (default: true).
    #[serde(default = "default_true")]
    pub require_auth: bool,
    /// Allowed MCP client IDs (empty = allow all).
    #[serde(default)]
    pub allowed_clients: Vec<String>,
    /// Session idle timeout in seconds (default: 1800 = 30 min).
    #[serde(default = "default_mcp_session_ttl")]
    pub session_ttl_secs: u64,
}

fn default_mcp_transport() -> String {
    "stdio".to_string()
}
fn default_mcp_port() -> u16 {
    7777
}
fn default_mcp_session_ttl() -> u64 {
    1800
}

impl Default for McpServeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: default_mcp_transport(),
            port: default_mcp_port(),
            expose_agents: true,
            require_auth: true,
            allowed_clients: Vec::new(),
            session_ttl_secs: default_mcp_session_ttl(),
        }
    }
}

/// Top-level application configuration.
///
/// Layered loading order: defaults → global (~/.halcon/config.toml) → project (.halcon/config.toml) → env vars.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub general: GeneralConfig,
    pub models: ModelsConfig,
    pub tools: ToolsConfig,
    pub security: SecurityConfig,
    pub storage: StorageConfig,
    pub logging: LoggingConfig,
    #[serde(default)]
    pub mcp: McpConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub planning: PlanningConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub resilience: ResilienceConfig,
    #[serde(default)]
    pub reflexion: ReflexionConfig,
    #[serde(default)]
    pub orchestrator: OrchestratorConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub task_framework: TaskFrameworkConfig,
    #[serde(default)]
    pub context_servers: ContextServersConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub reasoning: ReasoningConfig,
    /// Multimodal subsystem configuration.
    #[serde(default)]
    pub multimodal: MultimodalConfig,
    /// V3 Plugin system configuration.
    #[serde(default)]
    pub plugins: PluginsConfig,
    /// Centralized policy thresholds (reward, critic, evidence, trust, retry, growth, mini-critic).
    #[serde(default)]
    pub policy: super::policy_config::PolicyConfig,
    /// MCP Server configuration (Feature 9 — Halcon as MCP server).
    #[serde(default)]
    pub mcp_server: McpServeConfig,
}

/// V3 Plugin system configuration.
///
/// Enable via `[plugins] enabled = true` in config.toml.
/// When enabled, the plugin loader scans `~/.halcon/plugins/*.plugin.toml`
/// on the first agent-loop message and registers PluginProxyTool instances.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginsConfig {
    /// Master switch.  When false (default) no plugin discovery runs.
    #[serde(default)]
    pub enabled: bool,
    /// Override discovery path (default: ~/.halcon/plugins/).
    #[serde(default)]
    pub plugin_dir: Option<String>,
}

/// Adaptive reasoning and UCB1 strategy learning configuration.
///
/// When enabled, the ReasoningEngine wraps each agent loop:
/// - Pre-loop: analyzes task type/complexity, selects UCB1 strategy
/// - Post-loop: evaluates outcome, updates cross-session experience
///
/// Enable via `[reasoning] enabled = true` in config.toml or
/// env var `HALCON_REASONING=true`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningConfig {
    /// Master switch. Activates UCB1 learning and strategy selection.
    #[serde(default)]
    pub enabled: bool,
    /// Minimum score to consider a loop successful (0.0–1.0).
    #[serde(default = "reasoning_threshold_default")]
    pub success_threshold: f64,
    /// Max retry attempts if loop score below threshold.
    #[serde(default = "reasoning_max_retries_default")]
    pub max_retries: u32,
    /// UCB1 exploration factor (higher = more exploration).
    #[serde(default = "reasoning_exploration_default")]
    pub exploration_factor: f64,
    /// Persist experience across sessions (requires DB).
    #[serde(default = "reasoning_learning_default")]
    pub learning: bool,
    /// Phase 73 (Phase 5): enable adversarial LoopCritic evaluation after each session.
    ///
    /// When `true`, a second LLM call is made after the agent loop to adversarially
    /// evaluate whether the original goal was truly achieved. Adds ~1–3s latency.
    /// Disabled by default; enable with `enable_loop_critic = true` in config.toml.
    #[serde(default)]
    pub enable_loop_critic: bool,
    /// Optional separate model for LoopCritic (prevents self-evaluation bias).
    /// None = use executor model (backward-compatible default).
    ///
    /// Example: `critic_model = "gpt-4o-mini"` in `[reasoning]` section.
    #[serde(default)]
    pub critic_model: Option<String>,
    /// Optional separate provider name for LoopCritic.
    /// None = use executor provider.
    ///
    /// Example: `critic_provider = "openai"` in `[reasoning]` section.
    #[serde(default)]
    pub critic_provider: Option<String>,

    // ── Phase 3: AgentModelConfig — role-specific model/provider separation ──

    /// Optional model for the LlmPlanner role (plan generation / replan).
    /// None = use the session's primary execution model.
    ///
    /// Example: `planner_model = "deepseek-chat"` in `[reasoning]` section.
    #[serde(default)]
    pub planner_model: Option<String>,
    /// Optional provider for the LlmPlanner role.
    /// None = use the session's primary provider.
    ///
    /// Example: `planner_provider = "deepseek"` in `[reasoning]` section.
    #[serde(default)]
    pub planner_provider: Option<String>,

    /// Optional model for the Reflector role (post-execution reflection).
    /// None = use the session's primary execution model.
    ///
    /// Example: `reflector_model = "gpt-4o-mini"` in `[reasoning]` section.
    #[serde(default)]
    pub reflector_model: Option<String>,
    /// Optional provider for the Reflector role.
    /// None = use the session's primary provider.
    ///
    /// Example: `reflector_provider = "openai"` in `[reasoning]` section.
    #[serde(default)]
    pub reflector_provider: Option<String>,
}

fn reasoning_threshold_default() -> f64 { 0.6 }
fn reasoning_max_retries_default() -> u32 { 1 }
fn reasoning_exploration_default() -> f64 { 1.4 }
fn reasoning_learning_default() -> bool { true }

impl Default for ReasoningConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            success_threshold: reasoning_threshold_default(),
            max_retries: reasoning_max_retries_default(),
            exploration_factor: reasoning_exploration_default(),
            learning: reasoning_learning_default(),
            enable_loop_critic: false,
            critic_model: None,
            critic_provider: None,
            planner_model: None,
            planner_provider: None,
            reflector_model: None,
            reflector_provider: None,
        }
    }
}

/// Search engine configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Enable native search engine.
    #[serde(default = "search_enabled_default")]
    pub enabled: bool,
    /// Maximum documents in index (0 = unlimited).
    #[serde(default)]
    pub max_documents: usize,
    /// Enable semantic search (requires embedding model download).
    #[serde(default)]
    pub enable_semantic: bool,
    /// Enable query result caching.
    #[serde(default = "search_cache_default")]
    pub enable_cache: bool,
}

fn search_enabled_default() -> bool {
    true  // Enabled by default
}

fn search_cache_default() -> bool {
    true  // Cache enabled by default
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_documents: 0,  // Unlimited
            enable_semantic: false,  // Disabled (requires model download)
            enable_cache: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    /// Default provider to use.
    pub default_provider: String,
    /// Default model ID.
    pub default_model: String,
    /// Maximum tokens for responses.
    pub max_tokens: u32,
    /// Temperature for generation (0.0-1.0).
    pub temperature: f32,
    /// Working directory (defaults to cwd).
    pub working_directory: Option<PathBuf>,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            default_provider: "anthropic".to_string(),
            default_model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 8192,
            temperature: 0.0,
            working_directory: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    /// Provider-specific configurations.
    pub providers: HashMap<String, ProviderConfig>,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        let mut providers = HashMap::new();
        providers.insert(
            "anthropic".to_string(),
            ProviderConfig {
                enabled: true,
                api_base: Some("https://api.anthropic.com".to_string()),
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                default_model: Some("claude-sonnet-4-5-20250929".to_string()),
                http: HttpConfig::default(),
                oauth: None,
                extra: HashMap::new(),
            },
        );
        providers.insert(
            "ollama".to_string(),
            ProviderConfig {
                enabled: true,
                api_base: Some("http://localhost:11434".to_string()),
                api_key_env: None,
                default_model: Some("llama3.2".to_string()),
                http: HttpConfig::default(),
                oauth: None,
                extra: HashMap::new(),
            },
        );
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                enabled: false,
                api_base: Some("https://api.openai.com/v1".to_string()),
                api_key_env: Some("OPENAI_API_KEY".to_string()),
                default_model: Some("gpt-4o".to_string()),
                http: HttpConfig::default(),
                oauth: None,
                extra: HashMap::new(),
            },
        );
        providers.insert(
            "deepseek".to_string(),
            ProviderConfig {
                enabled: false,
                api_base: Some("https://api.deepseek.com".to_string()),
                api_key_env: Some("DEEPSEEK_API_KEY".to_string()),
                default_model: Some("deepseek-chat".to_string()),
                http: HttpConfig::default(),
                oauth: None,
                extra: HashMap::new(),
            },
        );
        providers.insert(
            "gemini".to_string(),
            ProviderConfig {
                enabled: false,
                api_base: Some("https://generativelanguage.googleapis.com".to_string()),
                api_key_env: Some("GEMINI_API_KEY".to_string()),
                default_model: Some("gemini-2.0-flash".to_string()),
                http: HttpConfig::default(),
                oauth: None,
                extra: HashMap::new(),
            },
        );
        Self { providers }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub enabled: bool,
    pub api_base: Option<String>,
    pub api_key_env: Option<String>,
    pub default_model: Option<String>,
    #[serde(default)]
    pub http: HttpConfig,
    #[serde(default)]
    pub oauth: Option<OAuthConfig>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// OAuth 2.0 Authorization Code + PKCE configuration for a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// OAuth client ID (public client, no secret).
    pub client_id: String,
    /// Authorization endpoint URL.
    pub authorize_url: String,
    /// Token exchange endpoint URL.
    pub token_url: String,
    /// Redirect URI (usually a localhost callback or OOB).
    pub redirect_uri: String,
    /// Endpoint to create an API key from the OAuth access token.
    #[serde(default)]
    pub api_key_url: Option<String>,
    /// OAuth scopes (space-separated).
    #[serde(default)]
    pub scopes: String,
}

/// HTTP client configuration for provider connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpConfig {
    /// TCP connection timeout in seconds.
    pub connect_timeout_secs: u64,
    /// Total request timeout in seconds (including streaming).
    pub request_timeout_secs: u64,
    /// Maximum number of retries for transient errors (429, 5xx).
    pub max_retries: u32,
    /// Base delay in milliseconds for exponential backoff.
    pub retry_base_delay_ms: u64,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            connect_timeout_secs: 10,
            request_timeout_secs: 300,
            max_retries: 3,
            retry_base_delay_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfig {
    /// Whether to require confirmation for destructive operations.
    pub confirm_destructive: bool,
    /// Timeout in seconds for tool execution.
    pub timeout_secs: u64,
    /// Directories that are allowed for file operations (empty = cwd only).
    pub allowed_directories: Vec<PathBuf>,
    /// File patterns to never read or modify.
    pub blocked_patterns: Vec<String>,
    /// Sandbox configuration for process isolation.
    #[serde(default)]
    pub sandbox: SandboxConfig,
    /// Enable dry-run mode (skip destructive tool execution).
    #[serde(default)]
    pub dry_run: bool,
    /// Tool retry configuration for transient failures.
    #[serde(default)]
    pub retry: ToolRetryConfig,
    /// Timeout in seconds for interactive permission prompts.
    /// 0 = no timeout (blocks indefinitely). Default: 30.
    #[serde(default = "default_prompt_timeout")]
    pub prompt_timeout_secs: u64,
    /// Auto-approve ReadOnly tools when running in CI environments (non-interactive mode).
    /// Prevents 30-second hangs on permission prompts in CI/CD pipelines.
    ///
    /// **Deprecated (Phase 72c)**: This field now controls ReadOnly tier only.
    /// Use `allow_write_in_ci` for ReadWrite tier and `allow_destructive_in_ci` for
    /// Destructive tier. Setting this to `true` no longer auto-approves Destructive tools
    /// — the `NonInteractivePolicy` now enforces per-level granular control.
    ///
    /// Default: true (ReadOnly auto-approved in CI; Destructive still requires opt-in).
    #[serde(default = "default_true")]
    pub auto_approve_in_ci: bool,
    /// Custom command blacklist patterns (regex) for bash tool.
    /// Extends built-in safety blacklist.
    #[serde(default)]
    pub command_blacklist: Vec<String>,
    /// Disable built-in bash command blacklist (dangerous commands like `rm -rf /`).
    /// Only disable if you fully trust the AI or are in a sandboxed environment.
    /// Default: false (protection enabled).
    #[serde(default)]
    pub disable_builtin_blacklist: bool,
    /// Allow ReadWrite tools (file_edit, file_write) in non-interactive/CI mode
    /// without an interactive permission prompt.
    /// Deprecated: `auto_approve_in_ci` now maps to allow_write=true only.
    /// Default: false.
    #[serde(default)]
    pub allow_write_in_ci: bool,
    /// Allow Destructive tools (bash, file_delete, etc.) in non-interactive/CI mode.
    /// Requires explicit opt-in — use `--allow-destructive` CLI flag or set this to true.
    /// Default: false.
    #[serde(default)]
    pub allow_destructive_in_ci: bool,
    /// Enforce mandatory dry-run preview before first Destructive tool in a session.
    /// When true, the first destructive tool shows a preview before executing.
    /// Default: false.
    #[serde(default)]
    pub enforce_dry_run_preview: bool,
}

/// Configuration for automatic tool retry on transient failures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRetryConfig {
    /// Maximum number of retries per tool execution.
    pub max_retries: u32,
    /// Base delay in milliseconds between retries (exponential backoff).
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds between retries.
    pub max_delay_ms: u64,
}

impl Default for ToolRetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay_ms: 500,
            max_delay_ms: 5000,
        }
    }
}

fn default_prompt_timeout() -> u64 {
    30
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            confirm_destructive: true,
            timeout_secs: 120,
            allowed_directories: Vec::new(),
            blocked_patterns: vec![
                "**/.env".to_string(),
                "**/.env.*".to_string(),
                "**/credentials.json".to_string(),
                "**/*.pem".to_string(),
                "**/*.key".to_string(),
            ],
            sandbox: SandboxConfig::default(),
            dry_run: false,
            retry: ToolRetryConfig::default(),
            prompt_timeout_secs: default_prompt_timeout(),
            auto_approve_in_ci: true,
            command_blacklist: vec![],
            disable_builtin_blacklist: false,
            allow_write_in_ci: false,
            allow_destructive_in_ci: false,
            enforce_dry_run_preview: false,
        }
    }
}

/// Sandbox configuration for tool process isolation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Enable sandbox (rlimits on Unix).
    pub enabled: bool,
    /// Maximum output bytes before truncation.
    pub max_output_bytes: usize,
    /// Maximum memory in MB for child processes (0 = unlimited).
    pub max_memory_mb: u64,
    /// Maximum CPU time in seconds for child processes (0 = unlimited).
    pub max_cpu_secs: u64,
    /// Maximum file size in bytes for child processes (0 = unlimited).
    pub max_file_size_bytes: u64,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_output_bytes: 100_000,
            max_memory_mb: 512,
            max_cpu_secs: 60,
            max_file_size_bytes: 50_000_000, // 50MB
        }
    }
}

/// Action taken when PII is detected in user input.
///
/// G2 remediation: replaces the old `pii_action: String` field with a typed enum
/// so the agent loop can branch without string matching. Deserializes from the
/// same "warn" / "block" / "redact" TOML values via `serde(rename_all = "lowercase")`.
///
/// Named `PiiPolicy` (not `PiiAction`) to avoid ambiguity with the `PiiAction` enum
/// in `event.rs` which records what action was taken (past tense: Redacted/Blocked/Warned).
/// This enum expresses what policy to apply (imperative: Warn/Block/Redact).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PiiPolicy {
    /// Log a warning and continue — PII reaches the LLM (backward-compatible default).
    Warn,
    /// Hard block: agent loop halts with an error message to the user.
    /// PII never reaches the LLM.
    Block,
    /// Best-effort redaction of credential patterns before sending to the LLM.
    /// Falls back to Warn if the pattern is not a recognizable credential.
    #[default]
    Redact,
}

impl std::fmt::Display for PiiPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PiiPolicy::Warn => write!(f, "warn"),
            PiiPolicy::Block => write!(f, "block"),
            PiiPolicy::Redact => write!(f, "redact"),
        }
    }
}

impl std::str::FromStr for PiiPolicy {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "warn" => Ok(PiiPolicy::Warn),
            "block" => Ok(PiiPolicy::Block),
            "redact" => Ok(PiiPolicy::Redact),
            _ => Ok(PiiPolicy::Redact), // safe default on unknown values
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Enable PII detection in prompts/responses.
    pub pii_detection: bool,
    /// Policy applied when PII is detected in user input.
    /// Phase 72c Phase 8.1 (G2): typed PiiPolicy enum, default "redact".
    /// Backward-compatible with existing config.toml ("warn"/"block"/"redact").
    #[serde(default)]
    pub pii_action: PiiPolicy,
    /// Enable audit trail.
    pub audit_enabled: bool,
    /// Guardrails configuration.
    #[serde(default)]
    pub guardrails: GuardrailsConfig,
    /// Enable Task-Based Authorization Control (TBAC).
    #[serde(default)]
    pub tbac_enabled: bool,
    /// Pre-execution self-critique: ask the LLM to review tool calls before executing.
    /// Expert mode only. Halts loop if model says NO. Default: false.
    #[serde(default)]
    pub pre_execution_critique: bool,
    /// TTL for session-level "always allow" grants (seconds). 0 = infinite.
    /// Phase 72c G10: prevents stale grants from being exploited. Default: 300 (5 min).
    #[serde(default = "default_session_grant_ttl")]
    pub session_grant_ttl_secs: u64,
    /// Opt-in: scan system prompts for PII patterns and emit a warning log.
    /// Default: false (system prompts are internal, not user input — low risk).
    /// When true, any PII pattern found in the cached system prompt is logged as a warning.
    /// Does NOT block execution — only observability.
    #[serde(default)]
    pub scan_system_prompts: bool,
}

/// Guardrails configuration (delegated from halcon-security for serde compat).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardrailsConfig {
    /// Enable guardrails.
    #[serde(default = "guardrails_default_true")]
    pub enabled: bool,
    /// Enable built-in guardrails (prompt injection, code injection).
    #[serde(default = "guardrails_default_true")]
    pub builtins: bool,
    /// Custom regex-based guardrail rules.
    #[serde(default)]
    pub rules: Vec<serde_json::Value>,
}

fn guardrails_default_true() -> bool {
    true
}

fn default_session_grant_ttl() -> u64 {
    300 // 5 minutes
}

impl Default for GuardrailsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            builtins: true,
            rules: Vec::new(),
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            pii_detection: true,
            // Phase 72c G2: PiiPolicy::Redact — less disruptive than Block,
            // still prevents PII from reaching the LLM unfiltered.
            pii_action: PiiPolicy::Redact,
            audit_enabled: true,
            guardrails: GuardrailsConfig::default(),
            tbac_enabled: false,
            pre_execution_critique: false,
            session_grant_ttl_secs: 300,
            scan_system_prompts: false, // opt-in: system prompts are internal
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Path to SQLite database (default: ~/.halcon/halcon.db).
    pub database_path: Option<PathBuf>,
    /// Maximum number of sessions to retain.
    pub max_sessions: u32,
    /// Maximum age of sessions in days.
    pub max_session_age_days: u32,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            database_path: None,
            max_sessions: 1000,
            max_session_age_days: 90,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level: "trace", "debug", "info", "warn", "error".
    pub level: String,
    /// Log format: "pretty" or "json".
    pub format: String,
    /// Log file path (None = stderr only).
    pub file: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: "pretty".to_string(),
            file: None,
        }
    }
}

/// MCP (Model Context Protocol) configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    /// Named MCP servers to connect to.
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
    /// Maximum reconnection attempts per server (0 = no reconnect).
    #[serde(default = "default_max_reconnect")]
    pub max_reconnect_attempts: u32,
}

fn default_max_reconnect() -> u32 {
    3
}

/// Configuration for a single MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Command to launch the MCP server process.
    pub command: String,
    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables to set for the process.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Override permission level per tool name: "ReadOnly" or "Destructive".
    #[serde(default)]
    pub tool_permissions: HashMap<String, String>,
}

/// Agent loop configuration: execution limits and model routing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub limits: AgentLimits,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub compaction: CompactionConfig,
    #[serde(default)]
    pub model_selection: ModelSelectionConfig,
}

/// Context-aware model selection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSelectionConfig {
    /// Enable automatic model selection.
    pub enabled: bool,
    /// Monthly budget cap in USD. 0 = unlimited.
    #[serde(default)]
    pub budget_cap_usd: f64,
    /// Messages shorter than this token estimate are considered "simple".
    #[serde(default = "default_complexity_threshold")]
    pub complexity_token_threshold: u32,
    /// Override model for simple tasks.
    #[serde(default)]
    pub simple_model: Option<String>,
    /// Override model for complex tasks.
    #[serde(default)]
    pub complex_model: Option<String>,
}

fn default_complexity_threshold() -> u32 {
    2000
}

impl Default for ModelSelectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            budget_cap_usd: 0.0,
            complexity_token_threshold: default_complexity_threshold(),
            simple_model: None,
            complex_model: None,
        }
    }
}

/// Context compaction configuration: rolling summarization of long conversations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Enable automatic context compaction.
    pub enabled: bool,
    /// Trigger compaction at this fraction of max context (0.0-1.0).
    pub threshold_fraction: f32,
    /// Number of recent messages to always preserve during compaction.
    pub keep_recent: usize,
    /// Max context window tokens (0 = auto-detect from model).
    pub max_context_tokens: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_fraction: 0.80,
            keep_recent: 4,
            max_context_tokens: 200_000,
        }
    }
}

/// Execution guards for the agent loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLimits {
    /// Maximum number of agent loop rounds (model re-invocations after tool use).
    pub max_rounds: usize,
    /// Maximum total tokens (input + output) before aborting. 0 = unlimited.
    pub max_total_tokens: u32,
    /// Maximum session duration in seconds. 0 = unlimited.
    pub max_duration_secs: u64,
    /// Timeout in seconds for individual tool execution.
    pub tool_timeout_secs: u64,
    /// Timeout in seconds for provider invocation (model inference). 0 = unlimited.
    #[serde(default = "default_provider_timeout")]
    pub provider_timeout_secs: u64,
    /// Maximum number of parallel tool executions. 0 = defaults to 1.
    #[serde(default = "default_max_parallel")]
    pub max_parallel_tools: usize,
    /// Maximum chars per tool output before truncation. 0 = unlimited.
    /// Prevents context explosion from large tool results (e.g., file reads).
    #[serde(default = "default_max_tool_output_chars")]
    pub max_tool_output_chars: usize,
    /// Maximum number of concurrent agent loops in TUI mode. 0 = unlimited.
    /// Phase 4: Allows multiple prompts to process concurrently without blocking TUI.
    #[serde(default = "default_max_concurrent_agents")]
    pub max_concurrent_agents: usize,
    /// Maximum USD cost for this agent session. 0.0 = unlimited.
    /// When exceeded, the agent loop stops gracefully after the current round.
    #[serde(default)]
    pub max_cost_usd: f64,
    /// Confidence threshold for the clarification gate: when a plan contains destructive
    /// steps and average confidence is below this threshold, the agent returns a clarification
    /// request instead of executing immediately. 0.0 = never ask, 1.0 = always ask.
    /// Default 0.6: ask when plan confidence < 60% and destructive tools are involved.
    #[serde(default = "default_clarification_threshold")]
    pub clarification_threshold: f64,
}

fn default_clarification_threshold() -> f64 {
    0.6
}

fn default_provider_timeout() -> u64 {
    300
}

fn default_max_parallel() -> usize {
    10
}

fn default_max_concurrent_agents() -> usize {
    3 // Default: 3 concurrent agents in TUI mode
}

fn default_max_tool_output_chars() -> usize {
    100_000 // ~25k tokens. Prevents context explosion from large file reads.
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_rounds: 25,
            max_total_tokens: 0,
            max_duration_secs: 0,
            tool_timeout_secs: 120,
            provider_timeout_secs: default_provider_timeout(),
            max_parallel_tools: default_max_parallel(),
            max_tool_output_chars: default_max_tool_output_chars(),
            max_concurrent_agents: default_max_concurrent_agents(),
            max_cost_usd: 0.0,
            clarification_threshold: default_clarification_threshold(),
        }
    }
}

/// Model routing configuration: fallback models and strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Routing strategy: "balanced", "fast", "cheap".
    pub strategy: String,
    /// Routing mode: "failover" (sequential fallback) or "speculative" (race providers).
    #[serde(default = "default_routing_mode")]
    pub mode: String,
    /// Fallback models to try if primary fails.
    #[serde(default)]
    pub fallback_models: Vec<String>,
    /// Maximum retries before switching to fallback.
    pub max_retries: u32,
    /// Provider names to race in speculative mode.
    #[serde(default)]
    pub speculation_providers: Vec<String>,
}

fn default_routing_mode() -> String {
    "failover".to_string()
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            strategy: "balanced".to_string(),
            mode: "failover".to_string(),
            fallback_models: Vec::new(),
            max_retries: 1,
            speculation_providers: Vec::new(),
        }
    }
}

/// Response cache configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Enable the response cache.
    pub enabled: bool,
    /// Default TTL in seconds for cached responses.
    pub default_ttl_secs: u64,
    /// Maximum number of cached entries before pruning.
    pub max_entries: u32,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_ttl_secs: 3600,
            max_entries: 1000,
        }
    }
}

/// Planning prompt configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningConfig {
    /// Enable the planning prompt in system context.
    pub enabled: bool,
    /// Custom planning prompt (None = use built-in default).
    #[serde(default)]
    pub custom_prompt: Option<String>,
    /// Enable LLM-based adaptive planning (generates plan before tool loop).
    #[serde(default)]
    pub adaptive: bool,
    /// Maximum replanning attempts on step failure.
    #[serde(default = "default_max_replans")]
    pub max_replans: u32,
    /// Minimum confidence threshold for auto-executing plan steps (0.0-1.0).
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f64,
    /// Timeout in seconds for plan generation (default: 30).
    #[serde(default = "default_planning_timeout")]
    pub timeout_secs: u64,
    /// Auto-save successful LLM-generated plans as YAML playbooks for future reuse.
    /// Saved to ~/.halcon/playbooks/auto-<slug>.yaml on successful EndTurn completion.
    /// Default: false (opt-in to prevent filesystem clutter).
    #[serde(default)]
    pub auto_learn_playbooks: bool,
}

fn default_max_replans() -> u32 {
    3
}

fn default_min_confidence() -> f64 {
    0.7
}

fn default_planning_timeout() -> u64 {
    30
}

impl Default for PlanningConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            custom_prompt: None,
            adaptive: true, // ✅ AUTONOMY FIX: Enable adaptive planning by default for proactive agent behavior
            max_replans: default_max_replans(),
            min_confidence: default_min_confidence(),
            timeout_secs: default_planning_timeout(),
            auto_learn_playbooks: false,
        }
    }
}

/// Semantic memory configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Enable the memory system.
    pub enabled: bool,
    /// Maximum number of memory entries before pruning.
    pub max_entries: u32,
    /// Default TTL in days for memory entries. None = no expiry.
    #[serde(default)]
    pub default_ttl_days: Option<u32>,
    /// Automatically summarize sessions on close.
    pub auto_summarize: bool,
    /// Number of memory entries to retrieve per query.
    pub retrieval_top_k: usize,
    /// Token budget for memory context (separate from instruction budget).
    pub retrieval_token_budget: usize,
    /// Enable episodic memory (groups entries into episodes with hybrid retrieval).
    #[serde(default = "default_true")]
    pub episodic: bool,
    /// Temporal decay half-life in days for relevance scoring.
    #[serde(default = "default_decay_half_life")]
    pub decay_half_life_days: f64,
    /// RRF fusion constant k (default 60, per Cormack et al.).
    #[serde(default = "default_rrf_k")]
    pub rrf_k: f64,
}

fn default_decay_half_life() -> f64 {
    30.0
}

fn default_rrf_k() -> f64 {
    60.0
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: 10000,
            default_ttl_days: None,
            auto_summarize: true,
            retrieval_top_k: 5,
            retrieval_token_budget: 2000,
            episodic: true,
            decay_half_life_days: default_decay_half_life(),
            rrf_k: default_rrf_k(),
        }
    }
}

/// Resilience layer configuration (circuit breakers, health, backpressure).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ResilienceConfig {
    /// Master switch. When false, all resilience features are bypassed.
    pub enabled: bool,
    /// Circuit breaker configuration.
    pub circuit_breaker: CircuitBreakerConfig,
    /// Health scoring configuration.
    pub health: HealthConfig,
    /// Backpressure configuration.
    pub backpressure: BackpressureConfig,
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            circuit_breaker: CircuitBreakerConfig::default(),
            health: HealthConfig::default(),
            backpressure: BackpressureConfig::default(),
        }
    }
}

/// Circuit breaker configuration per provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CircuitBreakerConfig {
    /// Number of failures within the window to trip the breaker.
    pub failure_threshold: u32,
    /// Sliding window for counting failures (seconds).
    pub window_secs: u64,
    /// Duration the breaker stays open before transitioning to half-open (seconds).
    pub open_duration_secs: u64,
    /// Number of successful probes required in half-open to close the breaker.
    pub half_open_probes: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            window_secs: 60,
            open_duration_secs: 30,
            half_open_probes: 2,
        }
    }
}

/// Health scoring configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HealthConfig {
    /// Lookback window for health score computation (minutes).
    pub window_minutes: u64,
    /// Health score at or below this is Degraded.
    pub degraded_threshold: u32,
    /// Health score at or below this is Unhealthy.
    pub unhealthy_threshold: u32,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            window_minutes: 60,
            degraded_threshold: 50,
            unhealthy_threshold: 30,
        }
    }
}

/// Backpressure configuration per provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackpressureConfig {
    /// Maximum concurrent provider invocations per provider.
    pub max_concurrent_per_provider: u32,
    /// Timeout waiting for a permit (seconds). 0 = fail immediately.
    pub queue_timeout_secs: u64,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self {
            max_concurrent_per_provider: 5,
            queue_timeout_secs: 30,
        }
    }
}

/// Reflexion self-improvement loop configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflexionConfig {
    /// Enable the reflexion self-improvement loop.
    pub enabled: bool,
    /// Number of recent reflections to inject into context.
    #[serde(default = "default_max_reflections")]
    pub max_reflections: usize,
    /// Also reflect on successful rounds (usually false).
    #[serde(default)]
    pub reflect_on_success: bool,
}

fn default_max_reflections() -> usize {
    3
}

impl Default for ReflexionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_reflections: default_max_reflections(),
            reflect_on_success: false,
        }
    }
}

/// Display and visual configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    /// Show the startup banner (can be overridden by --no-banner or HALCON_NO_BANNER).
    #[serde(default = "display_default_true")]
    pub show_banner: bool,
    /// Enable animations (spinners, etc). Auto-detected from terminal capabilities.
    #[serde(default = "display_default_true")]
    pub animations: bool,
    /// Theme name: "neon" (default), "minimal", "plain".
    #[serde(default = "default_theme_name")]
    pub theme: String,
    /// Terminal width below which compact layout is used. 0 = auto-detect.
    #[serde(default)]
    pub compact_width: u16,
    /// Optional brand color hex (e.g. "#0066cc") to generate a custom palette.
    #[serde(default)]
    pub brand_color: Option<String>,
    /// Terminal background color hex for accessibility contrast checks (default "#1a1a1a").
    #[serde(default = "default_terminal_bg")]
    pub terminal_background: String,
    /// UI mode: "minimal", "standard" (default), or "expert". Controls progressive disclosure.
    #[serde(default = "default_ui_mode")]
    pub ui_mode: String,
}

fn display_default_true() -> bool {
    true
}

fn default_true() -> bool {
    true
}

fn default_theme_name() -> String {
    "neon".to_string()
}

fn default_terminal_bg() -> String {
    "#1a1a1a".to_string()
}

fn default_ui_mode() -> String {
    "standard".to_string()
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            show_banner: true,
            animations: true,
            theme: "neon".to_string(),
            compact_width: 0,
            brand_color: None,
            terminal_background: default_terminal_bg(),
            ui_mode: default_ui_mode(),
        }
    }
}

/// Structured task framework configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFrameworkConfig {
    /// Enable the structured task framework. Default: true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Persist tasks to SQLite for cross-session resume. Default: true.
    #[serde(default = "default_true")]
    pub persist_tasks: bool,
    /// Default maximum retries for tasks. Default: 2.
    #[serde(default = "default_task_max_retries")]
    pub default_max_retries: u32,
    /// Default retry base delay in milliseconds. Default: 500.
    #[serde(default = "default_task_retry_base_ms")]
    pub default_retry_base_ms: u64,
    /// Resume incomplete tasks on startup. Default: false.
    #[serde(default)]
    pub resume_on_startup: bool,
    /// Expert mode: hard structural enforcement.
    /// When true: blocked DAG halts the loop, failure cascades halt, planner timeouts propagate.
    /// Enabled automatically by `--expert` CLI flag. Default: false.
    #[serde(default)]
    pub strict_enforcement: bool,
}

fn default_task_max_retries() -> u32 {
    2
}

fn default_task_retry_base_ms() -> u64 {
    500
}

impl Default for TaskFrameworkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            persist_tasks: true,
            default_max_retries: default_task_max_retries(),
            default_retry_base_ms: default_task_retry_base_ms(),
            resume_on_startup: false,
            strict_enforcement: false,
        }
    }
}

/// Context management configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    /// Enable dynamic tool selection based on task intent. Default: true.
    #[serde(default = "default_true")]
    pub dynamic_tool_selection: bool,
    /// Per-source governance limits.
    #[serde(default)]
    pub governance: GovernanceConfig,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            dynamic_tool_selection: true,
            governance: GovernanceConfig::default(),
        }
    }
}

/// Governance rules for context assembly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GovernanceConfig {
    /// Default max tokens per context source. 0 = unlimited.
    #[serde(default)]
    pub default_max_tokens_per_source: u32,
    /// Default TTL in seconds for context contributions. 0 = no expiry.
    #[serde(default)]
    pub default_ttl_secs: u64,
}

/// SDLC Context Servers configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextServersConfig {
    /// Enable SDLC context servers. Default: false (opt-in).
    #[serde(default)]
    pub enabled: bool,
    /// Requirements & Product server configuration.
    #[serde(default)]
    pub requirements: ContextServerDef,
    /// Architecture & Design server configuration.
    #[serde(default)]
    pub architecture: ContextServerDef,
    /// Codebase context server configuration.
    #[serde(default)]
    pub codebase: ContextServerDef,
    /// Workflow & CI/CD server configuration.
    #[serde(default)]
    pub workflow: ContextServerDef,
    /// Testing & Quality server configuration.
    #[serde(default)]
    pub testing: ContextServerDef,
    /// Runtime & Observability server configuration.
    #[serde(default)]
    pub runtime: ContextServerDef,
    /// Security & Compliance server configuration.
    #[serde(default)]
    pub security: ContextServerDef,
    /// Support & Operations server configuration.
    #[serde(default)]
    pub support: ContextServerDef,
}

impl Default for ContextServersConfig {
    fn default() -> Self {
        Self {
            // ✅ ENABLED: Los 8 servidores SDLC activos por defecto para Context Servers modal
            enabled: true,
            // Prioridades específicas por fase SDLC (mayor = más importante)
            requirements: ContextServerDef { enabled: true, priority: 100, token_budget: 2000, cache_ttl_secs: 3600 },
            architecture: ContextServerDef { enabled: true, priority: 90, token_budget: 2000, cache_ttl_secs: 3600 },
            codebase: ContextServerDef { enabled: true, priority: 80, token_budget: 2000, cache_ttl_secs: 3600 },
            workflow: ContextServerDef { enabled: true, priority: 70, token_budget: 1500, cache_ttl_secs: 3600 },
            testing: ContextServerDef { enabled: true, priority: 60, token_budget: 1500, cache_ttl_secs: 3600 },
            runtime: ContextServerDef { enabled: true, priority: 50, token_budget: 1000, cache_ttl_secs: 3600 },
            security: ContextServerDef { enabled: true, priority: 40, token_budget: 1000, cache_ttl_secs: 3600 },
            support: ContextServerDef { enabled: true, priority: 30, token_budget: 1000, cache_ttl_secs: 3600 },
        }
    }
}

/// Individual context server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextServerDef {
    /// Enable this server. Default: true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Priority for context assembly. Higher = included first.
    #[serde(default = "default_server_priority")]
    pub priority: u32,
    /// Token budget for this server.
    #[serde(default = "default_server_budget")]
    pub token_budget: u32,
    /// Cache TTL in seconds. Default: 3600 (1 hour).
    #[serde(default = "default_server_cache_ttl")]
    pub cache_ttl_secs: u64,
}

fn default_server_priority() -> u32 {
    70
}

fn default_server_budget() -> u32 {
    3000
}

fn default_server_cache_ttl() -> u64 {
    3600
}

impl Default for ContextServerDef {
    fn default() -> Self {
        Self {
            enabled: true,
            priority: default_server_priority(),
            token_budget: default_server_budget(),
            cache_ttl_secs: default_server_cache_ttl(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_config_defaults() {
        let config = DisplayConfig::default();
        assert!(config.show_banner);
        assert!(config.animations);
        assert_eq!(config.theme, "neon");
        assert_eq!(config.compact_width, 0);
        assert!(config.brand_color.is_none());
        assert_eq!(config.terminal_background, "#1a1a1a");
        assert_eq!(config.ui_mode, "standard");
    }

    #[test]
    fn ui_mode_default_is_standard() {
        let config = DisplayConfig::default();
        assert_eq!(config.ui_mode, "standard");
    }

    #[test]
    fn brand_color_invalid_hex_warns() {
        let mut config = AppConfig::default();
        config.display.brand_color = Some("not-hex".to_string());
        let issues = validate_config(&config);
        assert!(
            issues.iter().any(|i| i.field.contains("brand_color")),
            "should warn on invalid brand_color hex"
        );
    }

    #[test]
    fn brand_color_valid_hex_no_warning() {
        let mut config = AppConfig::default();
        config.display.brand_color = Some("#0066cc".to_string());
        let issues = validate_config(&config);
        assert!(
            !issues.iter().any(|i| i.field.contains("brand_color")),
            "should not warn on valid brand_color hex"
        );
    }

    #[test]
    fn terminal_background_invalid_warns() {
        let mut config = AppConfig::default();
        config.display.terminal_background = "invalid".to_string();
        let issues = validate_config(&config);
        assert!(
            issues.iter().any(|i| i.field.contains("terminal_background")),
            "should warn on invalid terminal_background hex"
        );
    }

    #[test]
    fn invalid_temperature_rejected() {
        let mut config = AppConfig::default();
        config.general.temperature = 5.0;

        let issues = validate_config(&config);
        assert!(
            issues.iter().any(|i| i.level == IssueLevel::Error
                && i.field.contains("temperature")),
            "should produce error for temperature=5.0"
        );
    }

    #[test]
    fn warning_on_resilience_no_fallback() {
        let mut config = AppConfig::default();
        config.resilience.enabled = true;
        config.agent.routing.fallback_models = vec![];

        let issues = validate_config(&config);
        assert!(
            issues.iter().any(|i| i.level == IssueLevel::Warning
                && i.field.contains("fallback")),
            "should warn when resilience enabled with no fallback models"
        );
    }

    #[test]
    fn valid_config_no_issues() {
        let config = AppConfig::default();
        let issues = validate_config(&config);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.level == IssueLevel::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "default config should have no errors: {errors:?}"
        );
    }

    #[test]
    fn warns_unbounded_budget() {
        let mut config = AppConfig::default();
        config.agent.limits.max_total_tokens = 0;
        config.agent.limits.max_duration_secs = 0;

        let issues = validate_config(&config);
        assert!(
            issues
                .iter()
                .any(|i| i.level == IssueLevel::Warning && i.field == "agent.limits"),
            "should warn when no token or duration budget is set"
        );
    }

    #[test]
    fn no_budget_warning_when_token_limit_set() {
        let mut config = AppConfig::default();
        config.agent.limits.max_total_tokens = 100_000;
        config.agent.limits.max_duration_secs = 0;

        let issues = validate_config(&config);
        assert!(
            !issues
                .iter()
                .any(|i| i.field == "agent.limits"),
            "should not warn when token budget is set"
        );
    }

    #[test]
    fn provider_timeout_config_default() {
        let limits = AgentLimits::default();
        assert_eq!(limits.provider_timeout_secs, 300);
        assert_eq!(limits.max_parallel_tools, 10);
    }

    #[test]
    fn config_provider_timeout_validation() {
        let mut config = AppConfig::default();
        config.agent.limits.provider_timeout_secs = 5;

        let issues = validate_config(&config);
        assert!(
            issues.iter().any(|i| i.level == IssueLevel::Warning
                && i.field.contains("provider_timeout")),
            "should warn when provider timeout < 30"
        );
    }

    #[test]
    fn config_max_parallel_validation() {
        let mut config = AppConfig::default();
        config.agent.limits.max_parallel_tools = 0;

        let issues = validate_config(&config);
        assert!(
            issues.iter().any(|i| i.level == IssueLevel::Warning
                && i.field.contains("max_parallel")),
            "should warn when max_parallel_tools is 0"
        );
    }

    #[test]
    fn memory_config_episodic_defaults() {
        let config = MemoryConfig::default();
        assert!(config.episodic);
        assert!((config.decay_half_life_days - 30.0).abs() < 0.001);
        assert!((config.rrf_k - 60.0).abs() < 0.001);
    }

    #[test]
    fn reflexion_config_defaults() {
        let config = ReflexionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_reflections, 3);
        assert!(!config.reflect_on_success);
    }

    #[test]
    fn orchestrator_config_defaults() {
        let config = super::OrchestratorConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_concurrent_agents, 3);
        assert_eq!(config.sub_agent_timeout_secs, 0);
        assert!(config.shared_budget);
    }

    #[test]
    fn orchestrator_zero_concurrent_warns() {
        let mut config = AppConfig::default();
        config.orchestrator.enabled = true;
        config.orchestrator.max_concurrent_agents = 0;

        let issues = validate_config(&config);
        assert!(
            issues.iter().any(|i| i.level == IssueLevel::Warning
                && i.field.contains("max_concurrent_agents")),
            "should warn when max_concurrent_agents is 0"
        );
    }

    #[test]
    fn validate_comm_without_orchestrator() {
        let mut config = AppConfig::default();
        config.orchestrator.enabled = false;
        config.orchestrator.enable_communication = true;

        let issues = validate_config(&config);
        assert!(
            issues.iter().any(|i| i.level == IssueLevel::Warning
                && i.field.contains("enable_communication")),
            "should warn when communication enabled without orchestrator"
        );
    }

    #[test]
    fn validate_dry_run_no_confirm() {
        let mut config = AppConfig::default();
        config.tools.dry_run = true;
        config.tools.confirm_destructive = false;

        let issues = validate_config(&config);
        assert!(
            issues.iter().any(|i| i.level == IssueLevel::Warning
                && i.field.contains("dry_run")),
            "should warn when dry_run enabled with confirm_destructive off"
        );
    }

    #[test]
    fn validate_mcp_reconnect_high() {
        let mut config = AppConfig::default();
        config.mcp.max_reconnect_attempts = 15;

        let issues = validate_config(&config);
        assert!(
            issues.iter().any(|i| i.level == IssueLevel::Warning
                && i.field.contains("max_reconnect_attempts")),
            "should warn when max_reconnect_attempts > 10"
        );
    }

    #[test]
    fn orchestrator_timeout_exceeds_parent() {
        let mut config = AppConfig::default();
        config.orchestrator.enabled = true;
        config.orchestrator.sub_agent_timeout_secs = 600;
        config.agent.limits.max_duration_secs = 300;

        let issues = validate_config(&config);
        assert!(
            issues.iter().any(|i| i.level == IssueLevel::Warning
                && i.field.contains("sub_agent_timeout")),
            "should warn when sub-agent timeout exceeds parent duration"
        );
    }

    #[test]
    fn default_config_has_five_providers() {
        let config = AppConfig::default();
        assert_eq!(config.models.providers.len(), 5);
        assert!(config.models.providers.contains_key("anthropic"));
        assert!(config.models.providers.contains_key("ollama"));
        assert!(config.models.providers.contains_key("openai"));
        assert!(config.models.providers.contains_key("deepseek"));
        assert!(config.models.providers.contains_key("gemini"));
    }

    #[test]
    fn new_providers_disabled_by_default() {
        let config = AppConfig::default();
        assert!(!config.models.providers["openai"].enabled);
        assert!(!config.models.providers["deepseek"].enabled);
        assert!(!config.models.providers["gemini"].enabled);
    }

    #[test]
    fn existing_providers_still_enabled() {
        let config = AppConfig::default();
        assert!(config.models.providers["anthropic"].enabled);
        assert!(config.models.providers["ollama"].enabled);
    }

    #[test]
    fn validate_config_default_still_no_errors() {
        let config = AppConfig::default();
        let issues = validate_config(&config);
        let errors: Vec<_> = issues.iter().filter(|i| i.level == IssueLevel::Error).collect();
        assert!(errors.is_empty(), "default config should have no errors: {:?}", errors);
    }

    #[test]
    fn model_info_supports_reasoning_serde_default() {
        // Deserializing without supports_reasoning should default to false
        let json = r#"{"id":"test","name":"Test","provider":"test","context_window":1000,"max_output_tokens":100,"supports_streaming":true,"supports_tools":true,"supports_vision":false,"cost_per_input_token":0.0,"cost_per_output_token":0.0}"#;
        let info: super::super::ModelInfo = serde_json::from_str(json).unwrap();
        assert!(!info.supports_reasoning);
    }

    #[test]
    fn model_info_supports_reasoning_true_roundtrip() {
        let info = super::super::ModelInfo {
            id: "test".into(),
            name: "Test".into(),
            provider: "test".into(),
            context_window: 1000,
            max_output_tokens: 100,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_reasoning: true,
            cost_per_input_token: 0.0,
            cost_per_output_token: 0.0,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"supports_reasoning\":true"));
        let roundtrip: super::super::ModelInfo = serde_json::from_str(&json).unwrap();
        assert!(roundtrip.supports_reasoning);
    }

    #[test]
    fn planning_config_timeout_secs_serde_roundtrip() {
        let config = PlanningConfig {
            timeout_secs: 60,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"timeout_secs\":60"));
        let roundtrip: PlanningConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.timeout_secs, 60);
        // Default should be 30
        let default_config = PlanningConfig::default();
        assert_eq!(default_config.timeout_secs, 30);
    }

    #[test]
    fn context_config_defaults() {
        let config = ContextConfig::default();
        assert!(config.dynamic_tool_selection);
        assert_eq!(config.governance.default_max_tokens_per_source, 0);
        assert_eq!(config.governance.default_ttl_secs, 0);
    }

    #[test]
    fn context_config_serde_roundtrip() {
        let config = ContextConfig {
            dynamic_tool_selection: true,
            governance: GovernanceConfig {
                default_max_tokens_per_source: 5000,
                default_ttl_secs: 300,
            },
        };
        let json = serde_json::to_string(&config).unwrap();
        let roundtrip: ContextConfig = serde_json::from_str(&json).unwrap();
        assert!(roundtrip.dynamic_tool_selection);
        assert_eq!(roundtrip.governance.default_max_tokens_per_source, 5000);
        assert_eq!(roundtrip.governance.default_ttl_secs, 300);
    }

    #[test]
    fn context_config_absent_defaults_correctly() {
        // Simulates loading config TOML that has no [context] section.
        let config = AppConfig::default();
        assert!(config.context.dynamic_tool_selection);
        assert_eq!(config.context.governance.default_max_tokens_per_source, 0);
    }

    #[test]
    fn task_framework_config_defaults() {
        let config = TaskFrameworkConfig::default();
        assert!(config.enabled);
        assert!(config.persist_tasks);
        assert_eq!(config.default_max_retries, 2);
        assert_eq!(config.default_retry_base_ms, 500);
        assert!(!config.resume_on_startup);
    }

    #[test]
    fn task_framework_config_serde_roundtrip() {
        let config = TaskFrameworkConfig {
            enabled: true,
            persist_tasks: false,
            default_max_retries: 5,
            default_retry_base_ms: 1000,
            resume_on_startup: true,
            strict_enforcement: true,
        };
        let json = serde_json::to_string(&config).unwrap();
        let roundtrip: TaskFrameworkConfig = serde_json::from_str(&json).unwrap();
        assert!(roundtrip.enabled);
        assert!(!roundtrip.persist_tasks);
        assert_eq!(roundtrip.default_max_retries, 5);
        assert_eq!(roundtrip.default_retry_base_ms, 1000);
        assert!(roundtrip.resume_on_startup);
        assert!(roundtrip.strict_enforcement);
    }

    #[test]
    fn task_framework_absent_defaults_correctly() {
        // Simulates loading config TOML that has no [task_framework] section.
        let config = AppConfig::default();
        assert!(config.task_framework.enabled);
        assert!(config.task_framework.persist_tasks);
        assert_eq!(config.task_framework.default_max_retries, 2);
    }

    #[test]
    fn tools_config_auto_approve_ci_default() {
        let config = ToolsConfig::default();
        assert!(config.auto_approve_in_ci);
    }

    #[test]
    fn tools_config_auto_approve_ci_serde_roundtrip() {
        let config = ToolsConfig {
            auto_approve_in_ci: false,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let roundtrip: ToolsConfig = serde_json::from_str(&json).unwrap();
        assert!(!roundtrip.auto_approve_in_ci);
    }

    #[test]
    fn tools_config_auto_approve_ci_absent_defaults_true() {
        // Simulates loading config without auto_approve_in_ci field.
        let json = r#"{
            "confirm_destructive": true,
            "timeout_secs": 120,
            "allowed_directories": [],
            "blocked_patterns": []
        }"#;
        let config: ToolsConfig = serde_json::from_str(json).unwrap();
        assert!(config.auto_approve_in_ci);
    }

    // ── Phase 3: AgentModelConfig tests ────────────────────────────────────

    #[test]
    fn reasoning_config_role_fields_default_to_none() {
        let cfg = ReasoningConfig::default();
        assert!(cfg.planner_model.is_none());
        assert!(cfg.planner_provider.is_none());
        assert!(cfg.reflector_model.is_none());
        assert!(cfg.reflector_provider.is_none());
    }

    #[test]
    fn reasoning_config_role_fields_can_be_set() {
        let cfg = ReasoningConfig {
            enabled: true,
            planner_model: Some("deepseek-chat".to_string()),
            planner_provider: Some("deepseek".to_string()),
            reflector_model: Some("gpt-4o-mini".to_string()),
            reflector_provider: Some("openai".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.planner_model.as_deref(), Some("deepseek-chat"));
        assert_eq!(cfg.planner_provider.as_deref(), Some("deepseek"));
        assert_eq!(cfg.reflector_model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(cfg.reflector_provider.as_deref(), Some("openai"));
    }

    #[test]
    fn reasoning_config_role_fields_independent_of_critic() {
        // Verify planner/reflector fields coexist with existing critic fields.
        let cfg = ReasoningConfig {
            critic_model: Some("claude-haiku-4-5".to_string()),
            critic_provider: Some("anthropic".to_string()),
            planner_model: Some("deepseek-chat".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.critic_model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(cfg.critic_provider.as_deref(), Some("anthropic"));
        assert_eq!(cfg.planner_model.as_deref(), Some("deepseek-chat"));
        assert!(cfg.planner_provider.is_none());
        assert!(cfg.reflector_model.is_none());
        assert!(cfg.reflector_provider.is_none());
    }

}
