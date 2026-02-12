use std::sync::Arc;

use anyhow::Result;
use cuervo_core::traits::ModelProvider;
use cuervo_core::types::{AppConfig, HttpConfig, McpConfig};
use cuervo_providers::{
    AnthropicProvider, DeepSeekProvider, EchoProvider, GeminiProvider, OllamaProvider,
    OpenAIProvider, ProviderRegistry,
};
use cuervo_storage::Database;
use cuervo_tools::ToolRegistry;
use uuid::Uuid;

use crate::config_loader::default_db_path;
use crate::render::feedback;
use crate::repl::Repl;

/// Build a ProviderRegistry from configuration.
///
/// Registers providers whose API keys are available.
fn build_registry(config: &AppConfig) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();

    // Always register echo for testing.
    registry.register(Arc::new(EchoProvider::new()));

    // Register Anthropic if API key is available (env var or keychain).
    if let Some(provider_cfg) = config.models.providers.get("anthropic") {
        if provider_cfg.enabled {
            let api_key =
                super::auth::resolve_api_key("anthropic", provider_cfg.api_key_env.as_deref());

            if let Some(key) = api_key {
                let provider = AnthropicProvider::with_config(
                    key,
                    provider_cfg.api_base.clone(),
                    provider_cfg.http.clone(),
                );
                registry.register(Arc::new(provider));
                tracing::debug!("Registered Anthropic provider");
            } else {
                tracing::debug!("Anthropic provider enabled but no API key found");
            }
        }
    }

    // Register Ollama if enabled (no API key required).
    if let Some(provider_cfg) = config.models.providers.get("ollama") {
        if provider_cfg.enabled {
            let provider = OllamaProvider::with_default_model(
                provider_cfg.api_base.clone(),
                provider_cfg.http.clone(),
                provider_cfg.default_model.clone(),
            );
            registry.register(Arc::new(provider));
            tracing::debug!("Registered Ollama provider");
        }
    }

    // Register OpenAI if enabled and API key available.
    if let Some(provider_cfg) = config.models.providers.get("openai") {
        if provider_cfg.enabled {
            let api_key =
                super::auth::resolve_api_key("openai", provider_cfg.api_key_env.as_deref());

            if let Some(key) = api_key {
                let provider = OpenAIProvider::new(
                    key,
                    provider_cfg.api_base.clone(),
                    provider_cfg.http.clone(),
                );
                registry.register(Arc::new(provider));
                tracing::debug!("Registered OpenAI provider");
            } else {
                tracing::debug!("OpenAI provider enabled but no API key found");
            }
        }
    }

    // Register DeepSeek if enabled and API key available.
    if let Some(provider_cfg) = config.models.providers.get("deepseek") {
        if provider_cfg.enabled {
            let api_key =
                super::auth::resolve_api_key("deepseek", provider_cfg.api_key_env.as_deref());

            if let Some(key) = api_key {
                let provider = DeepSeekProvider::new(
                    key,
                    provider_cfg.api_base.clone(),
                    provider_cfg.http.clone(),
                );
                registry.register(Arc::new(provider));
                tracing::debug!("Registered DeepSeek provider");
            } else {
                tracing::debug!("DeepSeek provider enabled but no API key found");
            }
        }
    }

    // Register Gemini if enabled and API key available.
    if let Some(provider_cfg) = config.models.providers.get("gemini") {
        if provider_cfg.enabled {
            let api_key =
                super::auth::resolve_api_key("gemini", provider_cfg.api_key_env.as_deref());

            if let Some(key) = api_key {
                let provider = GeminiProvider::new(
                    key,
                    provider_cfg.api_base.clone(),
                    provider_cfg.http.clone(),
                );
                registry.register(Arc::new(provider));
                tracing::debug!("Registered Gemini provider");
            } else {
                tracing::debug!("Gemini provider enabled but no API key found");
            }
        }
    }

    registry
}

/// Ensure Ollama is in the registry as a last-resort local fallback.
///
/// If Ollama is not already registered and is reachable at localhost:11434,
/// it is added to the registry. This runs once at startup (~0-2s).
async fn ensure_local_fallback(registry: &mut ProviderRegistry) {
    // Skip if Ollama is already registered.
    if registry.get("ollama").is_some() {
        return;
    }

    // Create a temporary provider to probe availability (uses its own HTTP client
    // with connect_timeout, so the probe is bounded).
    let provider = OllamaProvider::new(None, HttpConfig::default());
    if provider.is_available().await {
        registry.register(Arc::new(provider));
        tracing::info!("Auto-detected local Ollama — registered as fallback provider");
    } else {
        tracing::debug!("Ollama not reachable at localhost:11434, skipping local fallback");
    }
}

/// Precheck that the requested provider is available; fall back if not.
///
/// Returns the (provider_name, model) to use. If the primary is unavailable,
/// tries other registered providers. Shows clear errors if nothing works.
async fn precheck_providers(
    registry: &ProviderRegistry,
    primary: &str,
    model: &str,
) -> Result<(String, String)> {
    // Check if primary provider is in the registry and available.
    if let Some(p) = registry.get(primary) {
        if p.is_available().await {
            return Ok((primary.to_string(), model.to_string()));
        }
        feedback::user_warning(
            &format!("primary provider '{primary}' is not available"),
            Some("Checking fallback providers..."),
        );
    } else {
        feedback::user_warning(
            &format!("provider '{primary}' is not registered (missing API key?)"),
            Some("Checking fallback providers..."),
        );
    }

    // Try all other registered providers (excluding echo).
    for name in registry.list() {
        if name == primary || name == "echo" {
            continue;
        }
        if let Some(p) = registry.get(name) {
            if p.is_available().await {
                let fallback_model = p
                    .supported_models()
                    .first()
                    .map(|m| m.id.clone())
                    .unwrap_or_else(|| model.to_string());
                feedback::user_warning(
                    &format!("using fallback provider '{name}' with model '{fallback_model}'"),
                    None,
                );
                return Ok((name.to_string(), fallback_model));
            }
        }
    }

    // No providers available.
    feedback::user_error(
        "no providers available",
        Some("Set ANTHROPIC_API_KEY or start Ollama (`ollama serve`) and retry"),
    );
    anyhow::bail!(
        "No providers available. Set an API key (e.g., ANTHROPIC_API_KEY) or start a local Ollama instance."
    );
}

/// Connect to configured MCP servers and register their tools.
///
/// Each server is spawned via stdio, initialized, and its tools are
/// registered in the tool registry as McpToolBridge instances.
#[tracing::instrument(skip_all, fields(server_count = mcp_config.servers.len()))]
async fn connect_mcp_servers(
    mcp_config: &McpConfig,
    tool_registry: &mut ToolRegistry,
) -> Vec<Arc<tokio::sync::Mutex<cuervo_mcp::McpHost>>> {
    let mut hosts = Vec::new();

    for (name, server_config) in &mcp_config.servers {
        tracing::debug!(server = name, command = %server_config.command, "Starting MCP server");

        let mut host = match cuervo_mcp::McpHost::new(
            name,
            &server_config.command,
            &server_config.args,
            &server_config.env,
        ) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(server = name, error = %e, "Failed to start MCP server");
                feedback::user_warning(
                    &format!("MCP server '{name}' failed to start — {e}"),
                    Some("Check the server command and arguments in your config"),
                );
                continue;
            }
        };

        // Initialize the MCP connection.
        if let Err(e) = host.initialize().await {
            tracing::warn!(server = name, error = %e, "MCP server initialization failed");
            feedback::user_warning(
                &format!("MCP server '{name}' initialization failed — {e}"),
                Some("Ensure the server supports MCP protocol"),
            );
            let _ = host.shutdown().await;
            continue;
        }

        // Discover tools.
        let tools = match host.list_tools().await {
            Ok(tools) => tools.to_vec(),
            Err(e) => {
                tracing::warn!(server = name, error = %e, "MCP server tool discovery failed");
                feedback::user_warning(
                    &format!("MCP server '{name}' tool discovery failed — {e}"),
                    None,
                );
                let _ = host.shutdown().await;
                continue;
            }
        };

        let tool_count = tools.len();
        let host_arc = Arc::new(tokio::sync::Mutex::new(host));

        // Register each MCP tool in the tool registry.
        for tool_def in tools {
            let bridge = cuervo_mcp::McpToolBridge::new(tool_def, Arc::clone(&host_arc));
            tool_registry.register(Arc::new(bridge));
        }

        tracing::info!(server = name, tools = tool_count, "MCP server connected");
        hosts.push(host_arc);
    }

    hosts
}

/// Run the chat command: interactive REPL or single prompt.
#[tracing::instrument(skip_all, fields(provider, model))]
pub async fn run(
    config: &AppConfig,
    provider: &str,
    model: &str,
    prompt: Option<String>,
    resume: Option<String>,
    no_banner: bool,
    tui: bool,
    explicit_model: bool,
) -> Result<()> {
    // Validate configuration before proceeding.
    let issues = cuervo_core::types::validate_config(config);
    let has_errors = issues
        .iter()
        .any(|i| i.level == cuervo_core::types::IssueLevel::Error);
    for issue in &issues {
        let msg = format!("[{}]: {}", issue.field, issue.message);
        match issue.level {
            cuervo_core::types::IssueLevel::Error => {
                feedback::user_error(&msg, issue.suggestion.as_deref());
            }
            cuervo_core::types::IssueLevel::Warning => {
                feedback::user_warning(&msg, issue.suggestion.as_deref());
            }
        }
    }
    if has_errors {
        anyhow::bail!("Configuration has errors. Fix them and retry.");
    }

    let mut registry = build_registry(config);

    // Ensure Ollama is available as a last-resort local fallback.
    ensure_local_fallback(&mut registry).await;

    // Precheck that the selected provider is available, falling back if needed.
    let (provider, model) = precheck_providers(&registry, provider, model).await?;
    let provider = provider.as_str();
    let model = model.as_str();

    let mut tool_registry = cuervo_tools::default_registry(&config.tools);

    // Connect to MCP servers and register their tools.
    let _mcp_hosts = connect_mcp_servers(&config.mcp, &mut tool_registry).await;

    // Instantiate the domain event bus for observability.
    let (event_tx, _event_rx) = cuervo_core::event_bus(256);

    // Open database (non-fatal if it fails).
    let db_path = config
        .storage
        .database_path
        .clone()
        .unwrap_or_else(default_db_path);
    let db = match Database::open(&db_path) {
        Ok(db) => {
            tracing::debug!("Database opened at {}", db_path.display());
            Some(Arc::new(db))
        }
        Err(e) => {
            tracing::warn!("Could not open database: {e}");
            None
        }
    };

    // Resume session if requested.
    let resume_session = if let Some(ref id_str) = resume {
        let id = Uuid::parse_str(id_str)
            .map_err(|e| anyhow::anyhow!("Invalid session ID: {e}"))?;
        match db.as_ref().and_then(|d| d.load_session(id).ok().flatten()) {
            Some(session) => {
                println!("Resuming session {}", &id_str[..8.min(id_str.len())]);
                Some(session)
            }
            None => {
                eprintln!("Session {id_str} not found, starting new session.");
                None
            }
        }
    } else {
        None
    };

    let mut repl = Repl::new(
        config,
        provider.to_string(),
        model.to_string(),
        db,
        resume_session,
        registry,
        tool_registry,
        event_tx,
        no_banner,
        explicit_model,
    )?;

    match prompt {
        Some(p) => {
            // Single prompt with full agent loop (tools, context, resilience).
            repl.run_single_prompt(&p).await?;
        }
        None => {
            #[cfg(feature = "tui")]
            if tui {
                repl.run_tui().await?;
                return Ok(());
            }

            #[cfg(not(feature = "tui"))]
            if tui {
                feedback::user_warning(
                    "TUI mode requires --features tui at compile time",
                    Some("Falling back to classic REPL"),
                );
            }

            repl.run().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_registry_always_has_echo() {
        let config = AppConfig::default();
        let registry = build_registry(&config);
        // Echo is always registered.
        assert!(registry.get("echo").is_some());
    }

    #[test]
    fn build_registry_ollama_default_enabled() {
        let config = AppConfig::default();
        let registry = build_registry(&config);
        // Ollama is enabled by default (no API key needed).
        assert!(registry.get("ollama").is_some());
    }

    #[test]
    fn build_registry_ollama_disabled() {
        let mut config = AppConfig::default();
        if let Some(p) = config.models.providers.get_mut("ollama") {
            p.enabled = false;
        }
        let registry = build_registry(&config);
        assert!(registry.get("ollama").is_none());
    }

    #[test]
    fn config_validation_empty_config_no_errors() {
        let config = AppConfig::default();
        let issues = cuervo_core::types::validate_config(&config);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.level == cuervo_core::types::IssueLevel::Error)
            .collect();
        assert!(errors.is_empty(), "default config should have no errors");
    }
}
