use std::sync::Arc;

use anyhow::Result;
use halcon_core::traits::ModelProvider;
use halcon_core::types::{AppConfig, HttpConfig, McpConfig, ModelInfo};
use halcon_providers::{
    AnthropicProvider, ClaudeCodeProvider, DeepSeekProvider, EchoProvider, GeminiProvider,
    OllamaProvider, OpenAICompatibleProvider, OpenAIProvider, ProviderRegistry,
};
use halcon_tools::ToolRegistry;
use serde::Deserialize;

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

    // Register ClaudeCode if enabled and the claude binary is available.
    if let Some(provider_cfg) = config.models.providers.get("claude_code") {
        if provider_cfg.enabled {
            use halcon_providers::claude_code::ClaudeCodeConfig;

            let cc_config = ClaudeCodeConfig::from_provider_extra(&provider_cfg.extra);
            // Use file-existence check instead of subprocess invocation:
            // - avoids nested-session / sudo errors when run inside Claude Code
            // - avoids PATH lookup issues for absolute-path binaries
            let available = std::path::Path::new(&cc_config.command).exists()
                || std::process::Command::new(&cc_config.command)
                    .arg("--version")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);

            if available {
                let cmd = cc_config.command.clone();
                registry.register(Arc::new(ClaudeCodeProvider::new(cc_config)));
                tracing::info!(command = %cmd, "Registered claude_code provider");
            } else {
                tracing::warn!(
                    command = %cc_config.command,
                    "claude_code enabled but binary not found — skipping registration"
                );
            }
        }
    }

    // DECISION: Bedrock is activated when CLAUDE_CODE_USE_BEDROCK=1 is set,
    // matching the Claude Code SDK convention for Bedrock usage.
    // This allows users to switch providers by changing a single env var.
    // See US-bedrock (PASO 2-B).
    #[cfg(feature = "bedrock")]
    if std::env::var("CLAUDE_CODE_USE_BEDROCK").is_ok() {
        if let Some(provider) = halcon_providers::BedrockProvider::from_env() {
            registry.register(provider);
            tracing::info!("Registered Bedrock provider (CLAUDE_CODE_USE_BEDROCK is set)");
        } else {
            tracing::warn!("CLAUDE_CODE_USE_BEDROCK is set but AWS credentials are missing (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY)");
        }
    }

    // DECISION: Vertex is activated when CLAUDE_CODE_USE_VERTEX=1 is set,
    // matching the Claude Code SDK convention for Vertex usage.
    // See US-vertex (PASO 2-C).
    #[cfg(feature = "vertex")]
    if std::env::var("CLAUDE_CODE_USE_VERTEX").is_ok() {
        if let Some(provider) = halcon_providers::VertexProvider::from_env() {
            registry.register(Arc::new(provider));
            tracing::info!("Registered Vertex AI provider (CLAUDE_CODE_USE_VERTEX is set)");
        } else {
            tracing::warn!("CLAUDE_CODE_USE_VERTEX is set but ANTHROPIC_VERTEX_PROJECT_ID is missing");
        }
    }

    // DECISION: Azure is activated when CLAUDE_CODE_USE_AZURE=1 is set.
    // See US-foundry (PASO 2-D).
    if std::env::var("CLAUDE_CODE_USE_AZURE").is_ok() {
        if let Some(provider) = halcon_providers::AzureFoundryProvider::from_env() {
            registry.register(Arc::new(provider));
            tracing::info!("Registered Azure AI Foundry provider (CLAUDE_CODE_USE_AZURE is set)");
        } else {
            tracing::warn!("CLAUDE_CODE_USE_AZURE is set but AZURE_AI_ENDPOINT is missing");
        }
    }

    // P0.3: Load dynamic providers from ~/.halcon/providers.d/*.toml
    if let Some(providers_dir) = dirs::home_dir()
        .map(|h| h.join(".halcon").join("providers.d"))
    {
        load_dynamic_providers(&providers_dir, &mut registry);
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
///
/// `explicit_model`: true when the user passed `-m <model>` explicitly.
/// When false (model came from global config default_model), model mismatches
/// are resolved silently — the provider's best model is used without a warning.
pub async fn precheck_providers(
    registry: &ProviderRegistry,
    primary: &str,
    model: &str,
) -> Result<(String, String)> {
    precheck_providers_with_explicit(registry, primary, model, false).await
}

/// Like `precheck_providers` but with explicit-model flag.
pub async fn precheck_providers_explicit(
    registry: &ProviderRegistry,
    primary: &str,
    model: &str,
    explicit_model: bool,
) -> Result<(String, String)> {
    precheck_providers_with_explicit(registry, primary, model, explicit_model).await
}

async fn precheck_providers_with_explicit(
    registry: &ProviderRegistry,
    primary: &str,
    model: &str,
    explicit_model: bool,
) -> Result<(String, String)> {
    // Check if primary provider is in the registry and available.
    if let Some(p) = registry.get(primary) {
        if p.is_available().await {
            // Fix: validate the requested model against the provider's supported list.
            // If the model is not valid (e.g. "claude-sonnet" on deepseek), select the
            // provider's best model automatically rather than silently propagating a model
            // that will fail deep in the agent loop.
            let resolved_model = if p.validate_model(model).is_ok() {
                model.to_string()
            } else {
                // Find the FIRST model with the highest context_window that supports tools.
                // Using max_by_key alone returns the LAST model on equal keys (Rust iterator
                // semantics), which can pick a low-priority fallback (e.g. command-path alias)
                // when all models have the same context window.
                let supported = p.supported_models();
                let max_ctx = supported
                    .iter()
                    .filter(|m| m.supports_tools)
                    .map(|m| m.context_window)
                    .max()
                    .unwrap_or(0);
                let best = supported
                    .iter()
                    .filter(|m| m.supports_tools && m.context_window >= max_ctx)
                    .next() // first model wins on ties (order = priority in supported_models())
                    .map(|m| m.id.clone())
                    .unwrap_or_else(|| {
                        supported
                            .first()
                            .map(|m| m.id.clone())
                            .unwrap_or_else(|| model.to_string())
                    });
                // Only warn when the user explicitly passed -m <model>.
                // When model came from the global config default_model (explicit_model=false),
                // the mismatch is expected (e.g. default_model="claude-sonnet-4-6" on openai)
                // — silently select the provider's best model instead.
                if explicit_model {
                    feedback::user_warning(
                        &format!(
                            "model '{model}' is not available on provider '{primary}', \
                             using '{best}' instead"
                        ),
                        Some("Use -m to explicitly specify a model for this provider"),
                    );
                } else {
                    tracing::debug!(
                        global_default = model,
                        resolved = %best,
                        provider = primary,
                        "model not valid for provider; using provider default (no warning — model came from global config)"
                    );
                }
                best
            };
            return Ok((primary.to_string(), resolved_model));
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
) -> Vec<Arc<tokio::sync::Mutex<halcon_mcp::McpHost>>> {
    let mut hosts = Vec::new();

    for (name, server_config) in &mcp_config.servers {
        tracing::debug!(server = name, command = %server_config.command, "Starting MCP server");

        let mut host = match halcon_mcp::McpHost::new(
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
                    "ReadOnly" => Some(halcon_core::types::PermissionLevel::ReadOnly),
                    "Destructive" => Some(halcon_core::types::PermissionLevel::Destructive),
                    _ => None,
                });
            let bridge =
                halcon_mcp::McpToolBridge::new(tool_def, Arc::clone(&host_arc), perm_override);
            tool_registry.register(Arc::new(bridge));
        }

        tracing::info!(server = name, tools = tool_count, "MCP server connected");
        hosts.push(host_arc);
    }

    hosts
}

// ---------------------------------------------------------------------------
// P0.3 — Dynamic provider manifest loader
// ---------------------------------------------------------------------------

/// TOML schema for a model entry in a dynamic provider manifest.
///
/// Example:
/// ```toml
/// [[models]]
/// id = "my-model-v1"
/// name = "My Model V1"
/// context_window = 128000
/// max_output_tokens = 4096
/// supports_streaming = true
/// supports_tools = true
/// supports_vision = false
/// cost_per_input_token = 0.0
/// cost_per_output_token = 0.0
/// ```
#[derive(Debug, Deserialize)]
struct DynamicModelDef {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "default_context_window")]
    context_window: u32,
    #[serde(default = "default_max_output")]
    max_output_tokens: u32,
    #[serde(default = "default_true")]
    supports_streaming: bool,
    #[serde(default = "default_true")]
    supports_tools: bool,
    #[serde(default)]
    supports_vision: bool,
    #[serde(default)]
    supports_reasoning: bool,
    #[serde(default)]
    cost_per_input_token: f64,
    #[serde(default)]
    cost_per_output_token: f64,
}

/// TOML schema for a dynamic provider manifest file.
///
/// Place at `~/.halcon/providers.d/<name>.toml`. Each file registers one
/// OpenAI-compatible provider without recompilation.
///
/// Example (`~/.halcon/providers.d/openrouter.toml`):
/// ```toml
/// [provider]
/// name = "openrouter"
/// base_url = "https://openrouter.ai/api/v1"
/// api_key_env = "OPENROUTER_API_KEY"
///
/// [[models]]
/// id = "anthropic/claude-3-haiku"
/// name = "Claude 3 Haiku via OpenRouter"
/// context_window = 200000
/// max_output_tokens = 4096
/// supports_tools = true
/// ```
#[derive(Debug, Deserialize)]
struct DynamicProviderManifest {
    provider: DynamicProviderSection,
    #[serde(default)]
    models: Vec<DynamicModelDef>,
    #[serde(default)]
    http: Option<HttpConfig>,
}

#[derive(Debug, Deserialize)]
struct DynamicProviderSection {
    /// Display name used as provider ID (e.g. "openrouter", "lmstudio").
    name: String,
    /// Base URL for the OpenAI-compatible Chat Completions endpoint.
    base_url: String,
    /// Environment variable that holds the API key.
    /// If the variable is absent, a warning is logged and the provider is skipped.
    api_key_env: Option<String>,
}

fn default_context_window() -> u32 { 128_000 }
fn default_max_output() -> u32 { 4_096 }
fn default_true() -> bool { true }

/// Load dynamic provider manifests from `dir/*.toml` and register each as an
/// `OpenAICompatibleProvider` in `registry`.
///
/// * Missing directory → silent no-op (common on fresh installs).
/// * Missing api_key_env variable → warning + skip.
/// * Parse errors → warning + skip (other manifests still loaded).
/// * No models defined → single placeholder model using provider name.
pub fn load_dynamic_providers(dir: &std::path::Path, registry: &mut ProviderRegistry) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return, // directory doesn't exist — silent
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "Failed to read dynamic provider manifest");
                continue;
            }
        };

        let manifest: DynamicProviderManifest = match toml::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "Failed to parse dynamic provider manifest");
                continue;
            }
        };

        let name = manifest.provider.name.clone();

        // Resolve API key (optional — some providers like LM Studio need none).
        let api_key = if let Some(ref env_var) = manifest.provider.api_key_env {
            match std::env::var(env_var) {
                Ok(k) if !k.is_empty() => k,
                _ => {
                    tracing::warn!(
                        provider = %name,
                        env_var = %env_var,
                        "Dynamic provider api_key_env not set — skipping"
                    );
                    continue;
                }
            }
        } else {
            // No key required (e.g., local LM Studio).
            String::new()
        };

        // Build ModelInfo list.
        let models: Vec<ModelInfo> = if manifest.models.is_empty() {
            // Default: one model whose id equals the provider name.
            vec![ModelInfo {
                id: name.clone(),
                name: name.clone(),
                provider: name.clone(),
                context_window: default_context_window(),
                max_output_tokens: default_max_output(),
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                cost_per_input_token: 0.0,
                cost_per_output_token: 0.0,
            }]
        } else {
            manifest
                .models
                .into_iter()
                .map(|m| ModelInfo {
                    id: m.id.clone(),
                    name: m.name.unwrap_or_else(|| m.id.clone()),
                    provider: name.clone(),
                    context_window: m.context_window,
                    max_output_tokens: m.max_output_tokens,
                    supports_streaming: m.supports_streaming,
                    supports_tools: m.supports_tools,
                    supports_vision: m.supports_vision,
                    supports_reasoning: m.supports_reasoning,
                    cost_per_input_token: m.cost_per_input_token,
                    cost_per_output_token: m.cost_per_output_token,
                })
                .collect()
        };

        let http_config = manifest.http.unwrap_or_default();

        let provider = OpenAICompatibleProvider::new(
            name.clone(),
            api_key,
            manifest.provider.base_url,
            models,
            http_config,
        );

        registry.register(Arc::new(provider));
        tracing::info!(provider = %name, "Registered dynamic provider from providers.d");
    }
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

    // --- P0.3 dynamic provider loader tests ---

    #[test]
    fn load_dynamic_providers_missing_dir_is_noop() {
        let dir = std::path::Path::new("/tmp/halcon_test_nonexistent_providers_d_xyz");
        let mut registry = ProviderRegistry::new();
        load_dynamic_providers(dir, &mut registry);
        assert!(registry.list().is_empty());
    }

    #[test]
    fn load_dynamic_providers_empty_dir_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let mut registry = ProviderRegistry::new();
        load_dynamic_providers(tmp.path(), &mut registry);
        assert!(registry.list().is_empty());
    }

    #[test]
    fn load_dynamic_providers_ignores_non_toml_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("provider.json"), "{}").unwrap();
        std::fs::write(tmp.path().join("provider.yaml"), "name: x").unwrap();
        let mut registry = ProviderRegistry::new();
        load_dynamic_providers(tmp.path(), &mut registry);
        assert!(registry.list().is_empty());
    }

    #[test]
    fn load_dynamic_providers_skips_missing_api_key_env() {
        let tmp = tempfile::tempdir().unwrap();
        let toml = r#"
[provider]
name = "testprovider_nokey"
base_url = "https://api.example.com/v1"
api_key_env = "HALCON_TEST_KEY_DEFINITELY_NOT_SET_XYZ"

[[models]]
id = "test-model"
"#;
        std::fs::write(tmp.path().join("test.toml"), toml).unwrap();
        // Ensure the env var is NOT set.
        std::env::remove_var("HALCON_TEST_KEY_DEFINITELY_NOT_SET_XYZ");
        let mut registry = ProviderRegistry::new();
        load_dynamic_providers(tmp.path(), &mut registry);
        assert!(registry.get("testprovider_nokey").is_none());
    }

    #[test]
    fn load_dynamic_providers_loads_provider_with_api_key() {
        let tmp = tempfile::tempdir().unwrap();
        let toml = r#"
[provider]
name = "myprovider"
base_url = "https://api.example.com/v1"
api_key_env = "HALCON_TEST_DYNAMIC_KEY_P03"

[[models]]
id = "my-model-v1"
name = "My Model V1"
context_window = 128000
max_output_tokens = 4096
supports_streaming = true
supports_tools = true
"#;
        std::fs::write(tmp.path().join("myprovider.toml"), toml).unwrap();
        std::env::set_var("HALCON_TEST_DYNAMIC_KEY_P03", "test-key-value");
        let mut registry = ProviderRegistry::new();
        load_dynamic_providers(tmp.path(), &mut registry);
        assert!(registry.get("myprovider").is_some());
        std::env::remove_var("HALCON_TEST_DYNAMIC_KEY_P03");
    }

    #[test]
    fn load_dynamic_providers_default_model_when_no_models_section() {
        let tmp = tempfile::tempdir().unwrap();
        let toml = r#"
[provider]
name = "lmstudio"
base_url = "http://localhost:1234/v1"
"#;
        std::fs::write(tmp.path().join("lmstudio.toml"), toml).unwrap();
        let mut registry = ProviderRegistry::new();
        load_dynamic_providers(tmp.path(), &mut registry);
        // No api_key_env means no skip — should load with empty key.
        assert!(registry.get("lmstudio").is_some());
    }

    #[test]
    fn load_dynamic_providers_skips_invalid_toml() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("bad.toml"), "this is not valid toml :::").unwrap();
        // Write a valid provider too.
        let valid_toml = r#"
[provider]
name = "valid_provider_p03"
base_url = "http://localhost:1234/v1"
"#;
        std::fs::write(tmp.path().join("valid.toml"), valid_toml).unwrap();
        let mut registry = ProviderRegistry::new();
        load_dynamic_providers(tmp.path(), &mut registry);
        // Bad file skipped, valid one loaded.
        assert!(registry.get("valid_provider_p03").is_some());
    }

    #[test]
    fn load_dynamic_providers_model_defaults_applied() {
        let tmp = tempfile::tempdir().unwrap();
        let toml = r#"
[provider]
name = "minimalmodel"
base_url = "http://localhost:1234/v1"

[[models]]
id = "minimal-model"
"#;
        std::fs::write(tmp.path().join("minimal.toml"), toml).unwrap();
        let mut registry = ProviderRegistry::new();
        load_dynamic_providers(tmp.path(), &mut registry);
        let provider = registry.get("minimalmodel").unwrap();
        let models = provider.supported_models();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "minimal-model");
        assert_eq!(models[0].context_window, 128_000);
        assert_eq!(models[0].max_output_tokens, 4_096);
        assert!(models[0].supports_streaming);
        assert!(models[0].supports_tools);
    }
}
