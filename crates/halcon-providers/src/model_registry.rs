//! Unified Model Registry — dynamic discovery with layered fallback.
//!
//! # Architecture (Frontier 2026)
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │              Unified Model Registry                  │
//! │                                                     │
//! │  Source 1: Gateway API (Cenzontle /v1/llm/models)   │
//! │           → ALL cloud models for user's plan        │
//! │           → tier, capabilities, permissions          │
//! │                                                     │
//! │  Source 2: Local providers (Ollama /api/tags)        │
//! │           → ALL local models on this machine         │
//! │                                                     │
//! │  Source 3: Direct provider APIs (/v1/models)         │
//! │           → Bypass mode (user's own API keys)        │
//! │                                                     │
//! │  Source 4: Static fallback (compiled-in)             │
//! │           → Offline / degraded / network failure     │
//! └─────────────────────────────────────────────────────┘
//! ```
//!
//! The registry queries sources in order and merges results. When a source
//! fails, it falls through to the next. The static fallback guarantees the
//! CLI always has a model list, even fully offline.
//!
//! # Why not hardcode?
//!
//! Frontier AI providers release new models monthly. Hardcoded lists become
//! stale within weeks. LiteLLM maintains 2000+ model entries; Aider caches
//! a remote JSON with 24h TTL. Our approach: let Cenzontle (the gateway)
//! be the source of truth, with static as safety net.

use std::collections::HashMap;
use std::time::Duration;

use tracing::{debug, info};

use halcon_core::types::ModelInfo;

// ── Static Fallback Registry ─────────────────────────────────────────────
//
// These are compiled into the binary and used ONLY when all live sources fail.
// They represent the minimum viable model set for each provider.
// Last updated: 2026-03-22.

/// Static fallback models for when live discovery fails.
///
/// Organized by provider. Each entry is the minimum needed for routing to work:
/// at least one Fast + one Balanced + one Deep (if available).
pub fn static_fallback_models(provider: &str) -> Vec<ModelInfo> {
    match provider {
        "anthropic" => vec![
            model(
                "claude-haiku-4-5-20251001",
                "Claude Haiku 4.5",
                "anthropic",
                200_000,
                8_192,
                true,
                true,
                true,
                false,
                0.80,
                4.0,
            ),
            model(
                "claude-sonnet-4-6",
                "Claude Sonnet 4.6",
                "anthropic",
                200_000,
                16_000,
                true,
                true,
                true,
                false,
                3.0,
                15.0,
            ),
            model(
                "claude-opus-4-6",
                "Claude Opus 4.6",
                "anthropic",
                200_000,
                32_000,
                true,
                true,
                true,
                true,
                15.0,
                75.0,
            ),
        ],
        "openai" => vec![
            model(
                "gpt-4o-mini",
                "GPT-4o Mini",
                "openai",
                128_000,
                16_384,
                true,
                true,
                true,
                false,
                0.15,
                0.60,
            ),
            model(
                "gpt-4o", "GPT-4o", "openai", 128_000, 16_384, true, true, true, false, 2.50, 10.0,
            ),
            model(
                "o3-mini", "o3-mini", "openai", 200_000, 100_000, true, true, false, true, 1.10,
                4.40,
            ),
        ],
        "deepseek" => vec![
            model(
                "deepseek-chat",
                "DeepSeek Chat",
                "deepseek",
                64_000,
                8_192,
                true,
                true,
                false,
                false,
                0.14,
                0.28,
            ),
            model(
                "deepseek-reasoner",
                "DeepSeek Reasoner",
                "deepseek",
                64_000,
                32_768,
                true,
                false,
                false,
                true,
                0.55,
                2.19,
            ),
        ],
        "gemini" => vec![
            model(
                "gemini-2.0-flash",
                "Gemini 2.0 Flash",
                "gemini",
                1_048_576,
                8_192,
                true,
                true,
                true,
                false,
                0.10,
                0.40,
            ),
            model(
                "gemini-2.5-pro",
                "Gemini 2.5 Pro",
                "gemini",
                1_048_576,
                65_536,
                true,
                true,
                true,
                true,
                1.25,
                10.0,
            ),
        ],
        _ => vec![],
    }
}

// ── Live Model Discovery ─────────────────────────────────────────────────

/// Fetch models from an OpenAI-compatible `/v1/models` endpoint.
///
/// Most providers (OpenAI, DeepSeek, Gemini, and any OpenAI-compatible server)
/// implement this endpoint. Returns a list of model IDs.
///
/// The caller is responsible for enriching the raw IDs with capabilities
/// (context window, tool support, etc.) via `enrich_from_static_or_infer`.
pub async fn fetch_openai_models(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .bearer_auth(api_key)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| format!("GET /v1/models failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("/v1/models returned HTTP {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse /v1/models response: {e}"))?;

    let models = body["data"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|m| m["id"].as_str().map(String::from))
        .collect();

    Ok(models)
}

/// Enrich raw model IDs with capabilities from static registry or inference.
///
/// For each model ID from a live `/v1/models` response:
/// 1. Look up in static fallback (exact match) → use those capabilities
/// 2. If not found → infer from model ID patterns (frontier approach from LiteLLM)
///
/// This means NEW models released by a provider are automatically discovered
/// (via /v1/models) and get reasonable default capabilities (via inference),
/// without requiring a Halcon release.
pub fn enrich_model_ids(provider: &str, model_ids: &[String], base_url: &str) -> Vec<ModelInfo> {
    let static_models = static_fallback_models(provider);
    let static_map: HashMap<&str, &ModelInfo> =
        static_models.iter().map(|m| (m.id.as_str(), m)).collect();

    model_ids
        .iter()
        .map(|id| {
            if let Some(known) = static_map.get(id.as_str()) {
                // Known model — use curated capabilities
                (*known).clone()
            } else {
                // Unknown model — infer capabilities from ID patterns
                infer_model_info(id, provider, base_url)
            }
        })
        .collect()
}

/// Discover models for a direct provider (not Cenzontle gateway).
///
/// Strategy: live `/v1/models` API → static fallback.
/// If live discovery succeeds, models are enriched with known capabilities.
/// If it fails, static fallback is returned.
pub async fn discover_provider_models(
    client: &reqwest::Client,
    provider: &str,
    base_url: &str,
    api_key: &str,
) -> Vec<ModelInfo> {
    let timeout = Duration::from_secs(5);

    match fetch_openai_models(client, base_url, api_key, timeout).await {
        Ok(ids) if !ids.is_empty() => {
            info!(
                provider,
                count = ids.len(),
                "Live model discovery succeeded"
            );
            enrich_model_ids(provider, &ids, base_url)
        }
        Ok(_) => {
            debug!(
                provider,
                "Live /v1/models returned empty list, using static fallback"
            );
            static_fallback_models(provider)
        }
        Err(e) => {
            debug!(provider, error = %e, "Live /v1/models failed, using static fallback");
            static_fallback_models(provider)
        }
    }
}

// ── Model ID Inference Engine ────────────────────────────────────────────
//
// When a model ID is returned by /v1/models but isn't in our static registry,
// we infer capabilities from naming patterns. This is the same approach used
// by LiteLLM (2000+ model regex patterns) and Continue.dev (regex matching).

fn infer_model_info(id: &str, provider: &str, _base_url: &str) -> ModelInfo {
    let lower = id.to_ascii_lowercase();

    let supports_reasoning = infer_reasoning(&lower);
    let supports_vision = infer_vision(&lower);
    let supports_tools = !supports_reasoning || lower.contains("o3") || lower.contains("o4");
    let (context_window, max_output) = infer_context(&lower);
    let (cost_in, cost_out) = infer_cost(&lower);

    ModelInfo {
        id: id.to_string(),
        name: id.to_string(),
        provider: provider.to_string(),
        context_window,
        max_output_tokens: max_output,
        supports_streaming: true,
        supports_tools,
        supports_vision,
        supports_reasoning,
        cost_per_input_token: cost_in,
        cost_per_output_token: cost_out,
    }
}

fn infer_reasoning(id: &str) -> bool {
    id.contains("o1")
        || id.contains("o3")
        || id.contains("o4")
        || id.contains("reasoner")
        || id.contains("r1")
        || id.contains("opus")
        || id.contains("thinking")
        || id.contains("think")
}

fn infer_vision(id: &str) -> bool {
    id.contains("vision")
        || id.contains("4o")
        || id.contains("gpt-4")
        || id.contains("claude")
        || id.contains("gemini")
        || id.contains("llava")
        || id.contains("pixtral")
}

fn infer_context(id: &str) -> (u32, u32) {
    if id.contains("gemini") {
        (1_048_576, 65_536)
    } else if id.contains("claude") || id.contains("o1") || id.contains("o3") {
        (200_000, 16_000)
    } else if id.contains("gpt-4") {
        (128_000, 16_384)
    } else if id.contains("deepseek") {
        (64_000, 8_192)
    } else if id.contains("mini") || id.contains("flash") {
        (128_000, 8_192)
    } else {
        (128_000, 4_096) // conservative default
    }
}

fn infer_cost(id: &str) -> (f64, f64) {
    // Synthetic relative costs for router tier sorting.
    // IMPORTANT: check reasoning/premium patterns FIRST because models like
    // "o3-mini" contain both "mini" and "o3".
    if id.contains("reasoner")
        || id.contains("o1-")
        || id.contains("o3")
        || id.contains("o4")
        || id.contains("opus")
    {
        (15.0 / 1_000_000.0, 60.0 / 1_000_000.0) // premium/reasoning
    } else if id.contains("mini") || id.contains("flash") || id.contains("haiku") {
        (0.15 / 1_000_000.0, 0.60 / 1_000_000.0) // economy
    } else if id.contains("pro") || id.contains("sonnet") || id.contains("4o") {
        (3.0 / 1_000_000.0, 15.0 / 1_000_000.0) // balanced
    } else {
        (1.0 / 1_000_000.0, 4.0 / 1_000_000.0) // default mid-range
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn model(
    id: &str,
    name: &str,
    provider: &str,
    ctx: u32,
    max_out: u32,
    streaming: bool,
    tools: bool,
    vision: bool,
    reasoning: bool,
    cost_in_per_m: f64,
    cost_out_per_m: f64,
) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        name: name.to_string(),
        provider: provider.to_string(),
        context_window: ctx,
        max_output_tokens: max_out,
        supports_streaming: streaming,
        supports_tools: tools,
        supports_vision: vision,
        supports_reasoning: reasoning,
        cost_per_input_token: cost_in_per_m / 1_000_000.0,
        cost_per_output_token: cost_out_per_m / 1_000_000.0,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_fallback_has_all_major_providers() {
        for provider in &["anthropic", "openai", "deepseek", "gemini"] {
            let models = static_fallback_models(provider);
            assert!(!models.is_empty(), "{provider} should have static models");
        }
    }

    #[test]
    fn static_fallback_unknown_provider_returns_empty() {
        assert!(static_fallback_models("nonexistent").is_empty());
    }

    #[test]
    fn static_anthropic_has_fast_balanced_deep() {
        let models = static_fallback_models("anthropic");
        assert!(
            models.iter().any(|m| m.id.contains("haiku")),
            "should have fast (haiku)"
        );
        assert!(
            models.iter().any(|m| m.id.contains("sonnet")),
            "should have balanced (sonnet)"
        );
        assert!(
            models.iter().any(|m| m.id.contains("opus")),
            "should have deep (opus)"
        );
    }

    #[test]
    fn enrich_known_model_uses_static() {
        let ids = vec!["claude-sonnet-4-6".to_string()];
        let models = enrich_model_ids("anthropic", &ids, "https://api.anthropic.com");
        assert_eq!(models[0].context_window, 200_000);
        assert_eq!(models[0].name, "Claude Sonnet 4.6");
    }

    #[test]
    fn enrich_unknown_model_infers() {
        let ids = vec!["claude-sonnet-5-0-20260601".to_string()];
        let models = enrich_model_ids("anthropic", &ids, "https://api.anthropic.com");
        assert_eq!(models[0].context_window, 200_000); // inferred from "claude"
        assert!(models[0].supports_vision); // inferred from "claude"
        assert!(!models[0].supports_reasoning); // sonnet != reasoning
    }

    #[test]
    fn infer_reasoning_models() {
        assert!(infer_reasoning("o1-preview"));
        assert!(infer_reasoning("o3-mini"));
        assert!(infer_reasoning("deepseek-reasoner"));
        assert!(infer_reasoning("claude-opus-4-6"));
        assert!(!infer_reasoning("gpt-4o"));
        assert!(!infer_reasoning("claude-sonnet-4-6"));
    }

    #[test]
    fn infer_cost_tiers_ordered() {
        let (_, economy) = infer_cost("gpt-4o-mini");
        let (_, balanced) = infer_cost("gpt-4o");
        let (_, premium) = infer_cost("o3-mini");
        assert!(economy < balanced, "economy < balanced");
        assert!(balanced < premium, "balanced < premium");
    }

    #[test]
    fn new_provider_model_discovered_and_inferred() {
        // Simulates a new model that doesn't exist in static registry
        let ids = vec!["gpt-5-turbo-2026-06-01".to_string()];
        let models = enrich_model_ids("openai", &ids, "https://api.openai.com");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5-turbo-2026-06-01");
        assert!(models[0].supports_streaming);
        // Should get reasonable defaults even though it's unknown
        assert!(models[0].context_window >= 128_000);
    }
}
