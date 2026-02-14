use std::sync::Arc;

use anyhow::Result;
use cuervo_core::traits::ModelProvider;
use cuervo_core::types::{AppConfig, HttpConfig, McpConfig};
use cuervo_providers::{
    AnthropicProvider, DeepSeekProvider, EchoProvider, GeminiProvider, OllamaProvider,
    OpenAIProvider, ProviderRegistry,
};
use cuervo_tools::ToolRegistry;

use crate::render::feedback;

/// Build a ProviderRegistry from configuration.
///
/// Registers providers whose API keys are available.
pub fn build_registry(config: &AppConfig) -> ProviderRegistry {
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
pub async fn ensure_local_fallback(registry: &mut ProviderRegistry) {
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
pub async fn precheck_providers(
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
pub async fn connect_mcp_servers(
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
            let perm_override = server_config
                .tool_permissions
                .get(&tool_def.name)
                .and_then(|s| match s.as_str() {
                    "ReadOnly" => Some(cuervo_core::types::PermissionLevel::ReadOnly),
                    "Destructive" => Some(cuervo_core::types::PermissionLevel::Destructive),
                    _ => None,
                });
            let bridge =
                cuervo_mcp::McpToolBridge::new(tool_def, Arc::clone(&host_arc), perm_override);
            tool_registry.register(Arc::new(bridge));
        }

        tracing::info!(server = name, tools = tool_count, "MCP server connected");
        hosts.push(host_arc);
    }

    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_registry_always_has_echo() {
        let config = AppConfig::default();
        let registry = build_registry(&config);
        assert!(registry.get("echo").is_some());
    }

    #[test]
    fn build_registry_ollama_default_enabled() {
        let config = AppConfig::default();
        let registry = build_registry(&config);
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
}
