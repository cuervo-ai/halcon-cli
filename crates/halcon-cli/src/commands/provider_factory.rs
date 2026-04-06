use std::sync::Arc;

use anyhow::Result;
use halcon_core::traits::ModelProvider;
use halcon_core::types::{AppConfig, HttpConfig, McpConfig, ModelInfo};
use halcon_providers::{
    AnthropicProvider, CenzontleProvider, ClaudeCodeProvider, DeepSeekProvider, EchoProvider,
    GeminiProvider, OllamaProvider, OpenAICompatibleProvider, OpenAIProvider, ProviderRegistry,
};
use halcon_tools::ToolRegistry;
use serde::Deserialize;

use crate::render::feedback;

// ── Typo detection ────────────────────────────────────────────────────────────

/// Compute the Levenshtein edit distance between two strings.
/// Pure function — no allocations beyond the DP matrix.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (la, lb) = (a.len(), b.len());
    if la == 0 {
        return lb;
    }
    if lb == 0 {
        return la;
    }
    let mut prev: Vec<usize> = (0..=lb).collect();
    let mut curr = vec![0usize; lb + 1];
    for i in 1..=la {
        curr[0] = i;
        for j in 1..=lb {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (curr[j - 1] + 1).min(prev[j] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[lb]
}

/// Return the closest registered provider name to `typo` if within edit distance ≤ 2.
fn suggest_provider(typo: &str, registered: &[String]) -> Option<String> {
    let typo_lc = typo.to_lowercase();
    registered
        .iter()
        .filter(|name| name.as_str() != "echo") // echo is internal
        .min_by_key(|name| levenshtein(&typo_lc, &name.to_lowercase()))
        .and_then(|best| {
            if levenshtein(&typo_lc, &best.to_lowercase()) <= 2 {
                Some(best.clone())
            } else {
                None
            }
        })
}

/// Build a ProviderRegistry from configuration.
///
/// Registers providers whose API keys are available.
///
/// DECISION: In --air-gap mode (HALCON_AIR_GAP=1) only the Ollama provider
/// is registered, regardless of what is configured. This is enforced at the
/// factory level (not the CLI level) so that sub-agents spawned by
/// orchestrator.rs also respect the constraint.
/// The factory is the single chokepoint for all provider creation — matching
/// how security boundaries work in privilege separation (enforce at the lowest
/// common layer, not the entry point).
pub fn build_registry(config: &AppConfig) -> ProviderRegistry {
    let air_gap = std::env::var("HALCON_AIR_GAP")
        .map(|v| v == "1")
        .unwrap_or(false);

    if air_gap {
        // Air-gap mode: register only Ollama.
        // OLLAMA_BASE_URL is guaranteed to be set (defaults to localhost:11434)
        // by the air-gap enforcement code in main.rs.
        let mut registry = ProviderRegistry::new();
        let base_url = std::env::var("OLLAMA_BASE_URL").ok();
        let provider = OllamaProvider::with_default_model(
            base_url,
            halcon_core::types::HttpConfig::default(),
            None,
        );
        registry.register(Arc::new(provider));
        tracing::info!("Air-gap mode: only Ollama provider registered");
        return registry;
    }

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
            tracing::warn!(
                "CLAUDE_CODE_USE_VERTEX is set but ANTHROPIC_VERTEX_PROJECT_ID is missing"
            );
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

    // Register Cenzontle if an SSO access token is available.
    //
    // Enabled when:
    //   a) config.models.providers["cenzontle"].enabled == true, OR
    //   b) CENZONTLE_ACCESS_TOKEN env var is set (env var overrides config for CI/CD)
    //
    // Token priority:
    // 1. CENZONTLE_ACCESS_TOKEN env var (CI/CD — also forces registration)
    // 2. Credential store (written by `halcon auth login cenzontle`)
    //    Backend is auto-selected by CredentialManager:
    //      macOS    → Keychain
    //      Linux+dbus → Secret Service
    //      Linux headless → XDG file store (~/.local/share/halcon/halcon-cli.json)
    //
    // IMPORTANT: credential store errors are logged explicitly (not silenced with
    // .ok().flatten()) so that Linux users get actionable diagnostics instead of
    // a mysterious "provider not registered" error.
    //
    // Model discovery is deferred to ensure_cenzontle_models() which runs after
    // build_registry() in an async context.
    let cenzontle_env_token = std::env::var("CENZONTLE_ACCESS_TOKEN")
        .ok()
        .filter(|v| !v.is_empty());

    // Auto-detect cenzontle: enabled if (a) env var set, (b) config opt-in, or
    // (c) a token already exists in the keystore from a prior `halcon auth login cenzontle`.
    // Case (c) ensures that logging in is sufficient — no manual config edit required.
    let cenzontle_enabled = cenzontle_env_token.is_some()
        || config
            .models
            .providers
            .get("cenzontle")
            .map(|c| c.enabled)
            .unwrap_or(false)
        || {
            let ks = halcon_auth::KeyStore::new("halcon-cli");
            matches!(ks.get_secret("cenzontle:access_token"), Ok(Some(_)))
        };

    if cenzontle_enabled {
        let cenzontle_token: Option<String> = cenzontle_env_token.or_else(|| {
            let keystore = halcon_auth::KeyStore::new("halcon-cli");
            tracing::debug!(
                backend = keystore.backend_info(),
                "Cenzontle: reading access token from credential store"
            );
            match keystore.get_secret("cenzontle:access_token") {
                Ok(token) => token,
                Err(e) => {
                    // Surface the real error rather than silencing it.
                    // On Linux headless this typically means D-Bus is absent and the
                    // file store fallback also failed — surface actionable guidance.
                    tracing::warn!(
                        error = %e,
                        backend = keystore.backend_info(),
                        "Cenzontle: credential store read failed. \
                         Workaround: set CENZONTLE_ACCESS_TOKEN env var, or run \
                         `halcon auth login cenzontle` to re-authenticate."
                    );
                    None
                }
            }
        });

        // Resolve base URL: env var > config api_base > built-in default
        let base_url = std::env::var("CENZONTLE_BASE_URL").ok().or_else(|| {
            config
                .models
                .providers
                .get("cenzontle")
                .and_then(|c| c.api_base.clone())
                .filter(|s| !s.is_empty())
        });

        if let Some(token) = cenzontle_token {
            // Build with empty model list — `ensure_cenzontle_models()` populates async.
            let provider = CenzontleProvider::new(token, base_url, Vec::new());
            registry.register(Arc::new(provider));
            tracing::debug!("Registered Cenzontle provider (token found, enabled=true)");
        } else {
            tracing::warn!(
                "Cenzontle is enabled but no access token found. \
                 Run `halcon auth login cenzontle` or set CENZONTLE_ACCESS_TOKEN."
            );
        }
    }

    // P0.3: Load dynamic providers from ~/.halcon/providers.d/*.toml
    if let Some(providers_dir) = dirs::home_dir().map(|h| h.join(".halcon").join("providers.d")) {
        load_dynamic_providers(&providers_dir, &mut registry);
    }

    registry
}

/// Populate Cenzontle models by calling the API.
///
/// Call this after `build_registry()` when an async context is available.
/// If the Cenzontle provider is not registered (no token) this is a no-op.
/// TTL for the Cenzontle model list disk cache (1 hour).
const CENZONTLE_MODEL_CACHE_TTL_SECS: u64 = 3600;

/// Path to the Cenzontle model cache file.
fn cenzontle_cache_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".halcon").join("cenzontle-models.json"))
}

/// Grace period after TTL expiry: serve stale cache for up to 24h to prevent
/// cold-start failures when the API is temporarily unreachable.
/// Inspired by Xiyo's stale-while-error keychain pattern.
const CENZONTLE_MODEL_CACHE_STALE_GRACE_SECS: u64 = 86_400;

/// Load the Cenzontle model list from the disk cache.
///
/// **Cache freshness tiers:**
/// 1. **Fresh** (age < TTL=1h): returned immediately, no API call needed.
/// 2. **Stale** (TTL < age < 24h): returned if `allow_stale` is true. The caller
///    should still attempt an API refresh but can use this as a fallback.
/// 3. **Expired** (age > 24h) or absent/corrupt: returns `None`.
///
/// The `allow_stale` parameter implements Xiyo's "stale-while-error" pattern:
/// the parallel startup path passes `allow_stale=false` (it will fetch from API),
/// while the fallback recovery path passes `allow_stale=true`.
///
/// **Integrity checks:**
/// - JSON must parse as `Vec<ModelInfo>` (schema validation).
/// - Model list must be non-empty.
/// - Each model must have a non-empty `id` field.
/// - Corrupt files are self-healing: deleted and rebuilt on next startup.
///
/// **Fallback-aware**: checks both `~/.halcon/` and XDG data dir.
fn load_cenzontle_model_cache() -> Option<Vec<halcon_core::types::ModelInfo>> {
    load_cenzontle_model_cache_inner(false)
}

/// Load with stale-while-error support.
fn load_cenzontle_model_cache_stale() -> Option<Vec<halcon_core::types::ModelInfo>> {
    load_cenzontle_model_cache_inner(true)
}

fn load_cenzontle_model_cache_inner(
    allow_stale: bool,
) -> Option<Vec<halcon_core::types::ModelInfo>> {
    let cache_path = cenzontle_cache_path()?;

    // Try reading from primary path or XDG fallback (for self-healing scenario).
    let json = crate::config_loader::safe_read_file(&cache_path)
        .and_then(|bytes| String::from_utf8(bytes).ok())?;

    // Check age: try primary path, then XDG fallback.
    let age_secs = std::fs::metadata(&cache_path)
        .ok()
        .or_else(|| {
            let fb = crate::config_loader::xdg_fallback_path("cenzontle-models.json")?;
            std::fs::metadata(&fb).ok()
        })
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs())
        .unwrap_or(u64::MAX);

    let max_age = if allow_stale {
        CENZONTLE_MODEL_CACHE_STALE_GRACE_SECS
    } else {
        CENZONTLE_MODEL_CACHE_TTL_SECS
    };

    if age_secs > max_age {
        tracing::debug!(
            age_secs = age_secs,
            max_age = max_age,
            allow_stale = allow_stale,
            "Cenzontle model cache expired"
        );
        return None;
    }

    let models: Vec<halcon_core::types::ModelInfo> = match serde_json::from_str(&json) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %cache_path.display(),
                "Cenzontle model cache is corrupt (invalid JSON); removing it"
            );
            let _ = std::fs::remove_file(&cache_path);
            return None;
        }
    };

    if models.is_empty() {
        tracing::debug!("Cenzontle model cache is empty, treating as miss");
        return None;
    }

    if models.iter().any(|m| m.id.is_empty()) {
        tracing::warn!(
            path = %cache_path.display(),
            "Cenzontle model cache contains model(s) with empty id; removing cache"
        );
        let _ = std::fs::remove_file(&cache_path);
        return None;
    }

    let is_stale = age_secs > CENZONTLE_MODEL_CACHE_TTL_SECS;
    tracing::debug!(
        count = models.len(),
        age_secs = age_secs,
        stale = is_stale,
        "Cenzontle: model list loaded from disk cache"
    );
    Some(models)
}

/// Save the Cenzontle model list to the disk cache using atomic write.
///
/// Uses `safe_write_file` for:
/// - Atomic tmp+rename (no partial reads).
/// - Ownership pre-check (catches root-owned files early).
/// - 0600 permissions on the cache file.
///
/// Failures are logged at WARN with actionable fix hints.
fn save_cenzontle_model_cache(models: &[halcon_core::types::ModelInfo]) {
    let Some(cache_path) = cenzontle_cache_path() else {
        return;
    };

    // Don't cache empty model lists — would just cause a cache miss on next load.
    if models.is_empty() {
        tracing::debug!("Cenzontle: skipping cache write for empty model list");
        return;
    }

    let json = match serde_json::to_string(models) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(error = %e, "Cenzontle: failed to serialize model list for cache");
            return;
        }
    };

    let result = crate::config_loader::safe_write_file(&cache_path, json.as_bytes());
    if result.is_ok() {
        tracing::debug!(
            count = models.len(),
            path = %cache_path.display(),
            "Cenzontle: model list saved to disk cache"
        );
    } else {
        result.log_on_failure("save_cenzontle_model_cache");
    }
}

pub async fn ensure_cenzontle_models(registry: &mut ProviderRegistry) {
    // If cenzontle isn't registered, skip.
    if registry.get("cenzontle").is_none() {
        return;
    }

    let token = std::env::var("CENZONTLE_ACCESS_TOKEN")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            let keystore = halcon_auth::KeyStore::new("halcon-cli");
            match keystore.get_secret("cenzontle:access_token") {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Cenzontle: credential store read failed during model refresh"
                    );
                    None
                }
            }
        });

    let Some(token) = token else { return };
    // Resolve base URL: env var > config api_base > built-in default
    let base_url = std::env::var("CENZONTLE_BASE_URL").ok().or_else(|| {
        crate::config_loader::load_config(None)
            .ok()
            .and_then(|cfg| cfg.models.providers.get("cenzontle").cloned())
            .and_then(|c| c.api_base)
            .filter(|s| !s.is_empty())
    });

    // Fast path: use disk cache if still fresh (< 1 hour old).
    // This avoids a GET /v1/llm/models API call on every startup — saves 2-10s
    // especially when the Azure Container Apps backend is cold or slow.
    if let Some(cached_models) = load_cenzontle_model_cache() {
        let provider = CenzontleProvider::new(token, base_url, cached_models);
        registry.register(Arc::new(provider));
        tracing::debug!("Cenzontle provider: re-registered with cached model list");
        return;
    }

    // Cache miss — fetch from API.
    if let Some(provider) = CenzontleProvider::from_token(token, base_url).await {
        // Persist the model list so the next startup uses the cache.
        save_cenzontle_model_cache(provider.supported_models());
        // Re-register with populated model list.
        registry.register(Arc::new(provider));
        tracing::debug!("Cenzontle provider models populated from API");
    }
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

/// Run the Ollama probe and Cenzontle model fetch **in parallel** to minimize startup latency.
///
/// Previously these were sequential: Ollama probe (~2s) + Cenzontle model fetch (~2-10s) = 4-12s.
/// By running in parallel, startup cost is `max(ollama_latency, cenzontle_latency)` ≈ 2-10s.
/// With the disk cache active, the Cenzontle step resolves in <1ms so total = Ollama probe only.
pub async fn ensure_startup_providers(registry: &mut ProviderRegistry) {
    let needs_ollama = registry.get("ollama").is_none();
    let needs_cenzontle = registry.get("cenzontle").is_some();

    if !needs_ollama && !needs_cenzontle {
        return; // Nothing to do
    }

    // Extract cenzontle token + base_url before entering async block (registry not movable).
    let cenzontle_token: Option<String> = if needs_cenzontle {
        std::env::var("CENZONTLE_ACCESS_TOKEN")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| {
                let keystore = halcon_auth::KeyStore::new("halcon-cli");
                match keystore.get_secret("cenzontle:access_token") {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(error = %e, "Cenzontle: credential read failed (parallel probe)");
                        None
                    }
                }
            })
    } else {
        None
    };
    let cenzontle_base = std::env::var("CENZONTLE_BASE_URL").ok();

    // ── Run Ollama and Cenzontle in parallel ──────────────────────────────────
    let ollama_fut = async {
        if !needs_ollama {
            return None;
        }
        let provider = OllamaProvider::new(None, HttpConfig::default());
        if provider.is_available().await {
            Some(provider)
        } else {
            None
        }
    };

    let cenzontle_fut = async {
        let Some(token) = cenzontle_token else {
            return None;
        };

        // Fast path: use the disk cache if still fresh (< 1h old).
        // Cache freshness proves a recent successful API call, so mark
        // connection_verified=true to skip the /v1/auth/me probe.
        if let Some(cached_models) = load_cenzontle_model_cache() {
            let has_models = !cached_models.is_empty();
            let mut p = CenzontleProvider::new(token, cenzontle_base, cached_models);
            if has_models {
                p.set_connection_verified(true);
            }
            return Some(p);
        }

        // Cache miss or expired — fetch from the API.
        if let Some(provider) =
            CenzontleProvider::from_token(token.clone(), cenzontle_base.clone()).await
        {
            save_cenzontle_model_cache(provider.supported_models());
            return Some(provider);
        }

        // API fetch failed — Xiyo-inspired stale-while-error: serve the expired
        // cache (up to 24h old) rather than leaving the provider unregistered.
        // The user gets a working session while the API recovers.
        if let Some(stale_models) = load_cenzontle_model_cache_stale() {
            tracing::warn!(
                count = stale_models.len(),
                "Cenzontle: API unreachable, using stale model cache (stale-while-error)"
            );
            let p = CenzontleProvider::new(token, cenzontle_base, stale_models);
            // Don't set connection_verified — let is_available() probe the API
            // with its own retry + stale-while-error logic.
            return Some(p);
        }

        None
    };

    let (ollama_result, cenzontle_result) = tokio::join!(ollama_fut, cenzontle_fut);

    // Register results (no I/O here — fast sequential operations).
    if let Some(p) = ollama_result {
        registry.register(Arc::new(p));
        tracing::info!("Auto-detected local Ollama — registered as fallback provider");
    }
    if let Some(p) = cenzontle_result {
        registry.register(Arc::new(p));
        tracing::debug!("Cenzontle provider ready (parallel startup)");
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
                let model_info = p.supported_models().iter().find(|m| m.id == model);
                tracing::info!(
                    provider = primary,
                    model = model,
                    supports_tools = model_info.map(|m| m.supports_tools).unwrap_or(false),
                    context_window = model_info.map(|m| m.context_window).unwrap_or(0),
                    source = if explicit_model {
                        "explicit -m flag"
                    } else {
                        "config default"
                    },
                    "Model selection: validated"
                );
                model.to_string()
            } else {
                // Model not found on this provider. Resolution order:
                // 1. Provider-specific default_model from config (most intentional)
                // 2. First supported model with tool support (for agent loop)
                // 3. First supported model regardless (at least something works)
                let supported = p.supported_models();

                // Step 1: Check provider-specific default_model from config
                let provider_default = crate::config_loader::load_config(None)
                    .ok()
                    .and_then(|cfg| cfg.models.providers.get(primary).cloned())
                    .and_then(|pc| pc.default_model);
                let best = if let Some(ref pd) = provider_default {
                    if supported.iter().any(|m| m.id == *pd) {
                        pd.clone()
                    } else {
                        String::new() // provider default not in list, fall through
                    }
                } else {
                    String::new()
                };

                let best = if best.is_empty() {
                    // Step 2: First model with tool support (needed for agent loop)
                    supported
                        .iter()
                        .find(|m| m.supports_tools)
                        .map(|m| m.id.clone())
                        // Step 3: Absolute fallback — first model in list
                        .unwrap_or_else(|| {
                            supported
                                .first()
                                .map(|m| m.id.clone())
                                .unwrap_or_else(|| model.to_string())
                        })
                } else {
                    best
                };
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
        // Provider registered but is_available() returned false.
        // Provide provider-specific diagnostics.
        if primary == "cenzontle" {
            // Check if the model cache is writable — a common cause of degraded state.
            let cache_writable = cenzontle_cache_path()
                .map(|p| crate::config_loader::is_writable(&p))
                .unwrap_or(true);

            let hint = if !cache_writable {
                "Model cache is not writable (root-owned file?). \
                 Run: sudo chown -R $(whoami) ~/.halcon"
            } else {
                "SSO token expired or API unreachable — run `halcon auth login cenzontle`"
            };

            tracing::warn!(
                provider = "cenzontle",
                cache_writable = cache_writable,
                "Primary provider unavailable: is_available() returned false"
            );
            feedback::user_warning(
                &format!("primary provider '{primary}' is not available"),
                Some(hint),
            );
        } else {
            tracing::warn!(
                provider = primary,
                "Primary provider unavailable: is_available() returned false"
            );
            feedback::user_warning(
                &format!("primary provider '{primary}' is not available"),
                Some("Checking fallback providers..."),
            );
        }
    } else {
        // Provider not in the registry at all.
        let registered = registry
            .list()
            .into_iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let suggestion = suggest_provider(primary, &registered);
        let hint = match &suggestion {
            Some(s) => format!("Did you mean: \"{s}\"? (use -p {s})"),
            None => "Checking fallback providers...".to_string(),
        };
        tracing::warn!(
            provider = primary,
            registered = ?registered,
            suggestion = ?suggestion,
            "Provider not registered"
        );
        feedback::user_warning(
            &format!("provider '{primary}' is not registered (missing API key?)"),
            Some(hint.as_str()),
        );
    }

    // ── Fallback selection ─────────────────────────────────────────────────
    // Try all other registered providers (excluding echo).
    // IMPORTANT: Every fallback is logged at WARN so users can see exactly
    // what happened in `--verbose` mode. No silent provider switching.
    let mut tried: Vec<&str> = Vec::new();
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
                tracing::warn!(
                    primary = primary,
                    fallback = name,
                    model = %fallback_model,
                    tried_unavailable = ?tried,
                    "Falling back from primary provider"
                );
                feedback::user_warning(
                    &format!("using fallback provider '{name}' with model '{fallback_model}'"),
                    None,
                );
                return Ok((name.to_string(), fallback_model));
            }
            tried.push(name);
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
    use crate::repl::security::config_trust::{McpTrustDecision, McpTrustStore};
    use std::io::IsTerminal;

    let mut hosts = Vec::new();
    let workspace = std::env::current_dir().unwrap_or_default();
    let is_interactive = std::io::stdin().is_terminal();
    let mut mcp_store = McpTrustStore::load();

    for (name, server_config) in &mcp_config.servers {
        // ── MCP Trust Gate (Gate 3) ────────────────────────────────────
        // Serialize the full server config to compute a hash. If ANY field
        // changes (command, args, env), the hash changes and re-approval
        // is required — mitigates CVE-2025-54136 (MCPoison).
        let config_json = serde_json::to_string(&serde_json::json!({
            "command": server_config.command,
            "args": server_config.args,
            "env": server_config.env,
        }))
        .unwrap_or_default();

        let trust_decision = mcp_store.check(&workspace, name, &config_json);

        match trust_decision {
            McpTrustDecision::Allowed => {
                tracing::debug!(server = name, "MCP server approved (config hash match)");
            }
            McpTrustDecision::Denied => {
                tracing::info!(server = name, "MCP server denied — skipping");
                continue;
            }
            McpTrustDecision::Changed => {
                if is_interactive {
                    eprintln!();
                    eprintln!("  MCP Server Config Changed: {name}");
                    eprintln!(
                        "  Command: {} {}",
                        server_config.command,
                        server_config.args.join(" ")
                    );
                    eprint!("  The config for this MCP server has changed. Allow? [y/N] ");
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                    let mut input = String::new();
                    let approved = std::io::stdin().read_line(&mut input).is_ok()
                        && matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                    let decision = if approved {
                        McpTrustDecision::Allowed
                    } else {
                        McpTrustDecision::Denied
                    };
                    mcp_store.set_trust(
                        &workspace,
                        name,
                        &config_json,
                        &format!("{} {}", server_config.command, server_config.args.join(" ")),
                        decision.clone(),
                    );
                    let _ = mcp_store.save();
                    if decision == McpTrustDecision::Denied {
                        continue;
                    }
                } else {
                    tracing::warn!(
                        server = name,
                        "MCP server config changed — skipping in non-interactive mode"
                    );
                    continue;
                }
            }
            McpTrustDecision::Unknown => {
                if is_interactive {
                    eprintln!();
                    eprintln!("  New MCP Server: {name}");
                    eprintln!(
                        "  Command: {} {}",
                        server_config.command,
                        server_config.args.join(" ")
                    );
                    eprint!("  Allow this MCP server to connect? [y/N] ");
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                    let mut input = String::new();
                    let approved = std::io::stdin().read_line(&mut input).is_ok()
                        && matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                    let decision = if approved {
                        McpTrustDecision::Allowed
                    } else {
                        McpTrustDecision::Denied
                    };
                    mcp_store.set_trust(
                        &workspace,
                        name,
                        &config_json,
                        &format!("{} {}", server_config.command, server_config.args.join(" ")),
                        decision.clone(),
                    );
                    let _ = mcp_store.save();
                    if decision == McpTrustDecision::Denied {
                        continue;
                    }
                } else {
                    // Non-interactive: skip unknown MCP servers (fail-closed)
                    tracing::debug!(
                        server = name,
                        "Unknown MCP server in non-interactive mode — skipping"
                    );
                    continue;
                }
            }
        }

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

fn default_context_window() -> u32 {
    128_000
}
fn default_max_output() -> u32 {
    4_096
}
fn default_true() -> bool {
    true
}

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

/// Promote cenzontle to default provider in the on-disk config, at most once per process.
/// Called from chat::run() when cenzontle is auto-detected at runtime but the config still
/// points to a different default_provider (e.g. users who logged in before v0.3.8).
pub fn activate_cenzontle_in_config_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(crate::commands::sso::activate_cenzontle_in_config);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[serial_test::serial(provider_factory_env)]
    #[test]
    fn build_registry_air_gap_only_registers_ollama() {
        // DECISION: air-gap mode is tested by directly setting the env var.
        // We must clean it up to avoid leaking into other tests.
        std::env::set_var("HALCON_AIR_GAP", "1");
        std::env::set_var("OLLAMA_BASE_URL", "http://localhost:11434");

        let config = AppConfig::default();
        let registry = build_registry(&config);

        // Only Ollama should be registered in air-gap mode.
        assert!(
            registry.get("ollama").is_some(),
            "ollama must be registered in air-gap mode"
        );
        assert!(
            registry.get("echo").is_none(),
            "echo must NOT be registered in air-gap mode"
        );
        assert!(
            registry.get("anthropic").is_none(),
            "anthropic must NOT be registered in air-gap mode"
        );

        // Cleanup.
        std::env::remove_var("HALCON_AIR_GAP");
        std::env::remove_var("OLLAMA_BASE_URL");
    }

    #[serial_test::serial(provider_factory_env)]
    #[test]
    fn build_registry_always_has_echo() {
        // Ensure HALCON_AIR_GAP is not set for this test.
        std::env::remove_var("HALCON_AIR_GAP");

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

    // ── Typo detection tests ──────────────────────────────────────────────────

    #[test]
    fn levenshtein_exact_match() {
        assert_eq!(levenshtein("anthropic", "anthropic"), 0);
    }

    #[test]
    fn levenshtein_single_transposition() {
        // "antropic" → "anthropic": insert 'h' = distance 1
        assert_eq!(levenshtein("antropic", "anthropic"), 1);
    }

    #[test]
    fn levenshtein_two_edits() {
        // "anthrpc" → "anthropic": insert 'o' + insert 'i' = distance 2
        assert_eq!(levenshtein("anthrpc", "anthropic"), 2);
    }

    #[test]
    fn suggest_provider_catches_antropic_typo() {
        let registered = vec![
            "anthropic".to_string(),
            "openai".to_string(),
            "ollama".to_string(),
            "deepseek".to_string(),
            "echo".to_string(),
        ];
        let suggestion = suggest_provider("antropic", &registered);
        assert_eq!(suggestion.as_deref(), Some("anthropic"));
    }

    #[test]
    fn suggest_provider_catches_opnai_typo() {
        let registered = vec![
            "anthropic".to_string(),
            "openai".to_string(),
            "ollama".to_string(),
            "echo".to_string(),
        ];
        let suggestion = suggest_provider("opnai", &registered);
        assert_eq!(suggestion.as_deref(), Some("openai"));
    }

    #[test]
    fn suggest_provider_no_suggestion_for_nonsense() {
        let registered = vec![
            "anthropic".to_string(),
            "openai".to_string(),
            "echo".to_string(),
        ];
        // "xyz" is edit distance >2 from everything → no suggestion
        let suggestion = suggest_provider("xyz", &registered);
        assert!(suggestion.is_none());
    }

    #[test]
    fn suggest_provider_excludes_echo() {
        let registered = vec!["echo".to_string()];
        // Only "echo" registered; typo "ech" is distance 1 but echo is excluded
        let suggestion = suggest_provider("ech", &registered);
        assert!(suggestion.is_none());
    }

    // ── Model cache integrity tests ──────────────────────────────────────────

    #[test]
    fn save_cenzontle_model_cache_skips_empty_list() {
        // Saving empty models should be a no-op (no file created).
        // This test verifies the guard clause works.
        save_cenzontle_model_cache(&[]);
        // If we got here without panic, the guard worked.
    }

    #[test]
    fn cenzontle_cache_path_returns_some() {
        // Verify the path helper returns a path (not None) when HOME is set.
        let path = cenzontle_cache_path();
        assert!(path.is_some(), "cache path should resolve when HOME is set");
        assert!(
            path.unwrap()
                .to_str()
                .unwrap()
                .contains("cenzontle-models.json"),
            "path should end with cenzontle-models.json"
        );
    }
}
