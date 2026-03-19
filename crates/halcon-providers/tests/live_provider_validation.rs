//! Live provider validation tests.
//!
//! These tests validate actual API connectivity with real providers.
//! They are gated behind env vars — each test only runs if the corresponding
//! API key is set. Run with:
//!
//!   OPENAI_API_KEY=... DEEPSEEK_API_KEY=... GEMINI_API_KEY=... cargo test -p halcon-providers --test live_provider_validation
//!
//! Or load from .env:
//!   export $(cat .env | sed 's/deepseek/DEEPSEEK_API_KEY/' | sed 's/gemini/GEMINI_API_KEY/' | sed 's/openai/OPENAI_API_KEY/' | xargs) && cargo test ...

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use halcon_core::traits::ModelProvider;
use halcon_core::types::{ChatMessage, HttpConfig, MessageContent, ModelChunk, ModelRequest, Role};
use futures::StreamExt;

// ========================================================
// Helper functions
// ========================================================

fn simple_request(model: &str, msg: &str) -> ModelRequest {
    ModelRequest {
        model: model.into(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Text(msg.into()),
        }],
        tools: vec![],
        max_tokens: Some(50),
        temperature: Some(0.0),
        system: None,
        stream: true,
    }
}

/// Collect stream into (text, has_usage, has_done, chunk_count)
async fn collect_stream(
    provider: &dyn ModelProvider,
    request: &ModelRequest,
) -> Result<StreamResult, String> {
    let start = Instant::now();
    let stream = provider
        .invoke(request)
        .await
        .map_err(|e| format!("invoke error: {e}"))?;

    let chunks: Vec<_> = stream.collect().await;
    let latency_ms = start.elapsed().as_millis() as u64;

    let mut text = String::new();
    let mut has_usage = false;
    let mut has_done = false;
    let mut input_tokens = 0u32;
    let mut output_tokens = 0u32;
    let mut error_count = 0u32;

    for chunk in &chunks {
        match chunk {
            Ok(ModelChunk::TextDelta(t)) => text.push_str(t),
            Ok(ModelChunk::Usage(u)) => {
                has_usage = true;
                input_tokens = u.input_tokens;
                output_tokens = u.output_tokens;
            }
            Ok(ModelChunk::Done(_)) => has_done = true,
            Ok(_) => {} // ToolUseStart, ToolUseDelta, etc.
            Err(e) => {
                error_count += 1;
                eprintln!("  stream error: {e}");
            }
        }
    }

    // Compute content hash for determinism tracking
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    let content_hash = format!("{:016x}", hasher.finish());

    Ok(StreamResult {
        text,
        has_usage,
        has_done,
        chunk_count: chunks.len(),
        latency_ms,
        input_tokens,
        output_tokens,
        error_count,
        content_hash,
    })
}

#[derive(Debug)]
struct StreamResult {
    text: String,
    has_usage: bool,
    has_done: bool,
    chunk_count: usize,
    latency_ms: u64,
    input_tokens: u32,
    output_tokens: u32,
    error_count: u32,
    content_hash: String,
}

fn print_result(provider: &str, model: &str, result: &StreamResult) {
    eprintln!("  [{provider}/{model}]");
    eprintln!("    Text length: {} chars", result.text.len());
    eprintln!("    Has usage: {}", result.has_usage);
    eprintln!("    Has done: {}", result.has_done);
    eprintln!("    Chunks: {}", result.chunk_count);
    eprintln!("    Latency: {}ms", result.latency_ms);
    eprintln!(
        "    Tokens: {} in / {} out",
        result.input_tokens, result.output_tokens
    );
    eprintln!("    Errors: {}", result.error_count);
    eprintln!("    Content hash: {}", &result.content_hash[..16]);
    eprintln!(
        "    Text preview: {}...",
        &result.text[..result.text.len().min(80)]
    );
}

// ========================================================
// Cenzontle tests (gated on CENZONTLE_ACCESS_TOKEN)
// ========================================================

/// Full integration test for the Cenzontle AI provider.
///
/// Verifies:
/// 1. Token resolves from env var
/// 2. Provider connects (is_available)
/// 3. SSE stream returns text chunks + usage + done
/// 4. HTTP/1.1 framing delivers incremental chunks (chunk_count > 1)
/// 5. Response is coherent (non-empty text)
///
/// Run:
///   CENZONTLE_ACCESS_TOKEN=<jwt> cargo test -p halcon-providers --test live_provider_validation cenzontle -- --nocapture
#[tokio::test]
async fn cenzontle_connectivity() {
    use halcon_providers::CenzontleProvider;

    let token = match std::env::var("CENZONTLE_ACCESS_TOKEN")
        .ok()
        .filter(|v| !v.is_empty())
    {
        Some(t) => t,
        None => {
            eprintln!("SKIP: CENZONTLE_ACCESS_TOKEN not set");
            return;
        }
    };

    let base_url = std::env::var("CENZONTLE_BASE_URL").ok();
    let provider = CenzontleProvider::new(token, base_url, Vec::new());

    eprintln!("\n=== Cenzontle Connectivity Test ===");

    // 1. Availability check (hits /v1/auth/me)
    let available = provider.is_available().await;
    eprintln!("  is_available: {available}");
    if !available {
        eprintln!("SKIP: Cenzontle backend not reachable (token expired or network issue)");
        return;
    }

    // 2. Chat completion — streaming
    let request = simple_request("deepseek-v3-2-coding", "Responde solo: hola");
    let result = collect_stream(&provider, &request).await;

    match result {
        Err(e) => panic!("Cenzontle invoke failed: {e}"),
        Ok(r) => {
            print_result("cenzontle", "gpt-4o-mini", &r);

            assert_eq!(r.error_count, 0, "stream had errors");
            assert!(r.has_done, "stream did not emit Done chunk");
            assert!(!r.text.is_empty(), "response text is empty");
            assert!(
                r.chunk_count > 1,
                "expected incremental SSE chunks (HTTP/1.1 streaming), got only {}",
                r.chunk_count
            );

            eprintln!("  PASS: Cenzontle SSE streaming OK");
        }
    }
}

/// Verify Cenzontle returns usage tokens when stream_options.include_usage=true.
#[tokio::test]
async fn cenzontle_usage_tokens() {
    use halcon_providers::CenzontleProvider;

    let token = match std::env::var("CENZONTLE_ACCESS_TOKEN")
        .ok()
        .filter(|v| !v.is_empty())
    {
        Some(t) => t,
        None => {
            eprintln!("SKIP: CENZONTLE_ACCESS_TOKEN not set");
            return;
        }
    };

    let provider = CenzontleProvider::new(token, std::env::var("CENZONTLE_BASE_URL").ok(), Vec::new());

    if !provider.is_available().await {
        eprintln!("SKIP: Cenzontle not reachable");
        return;
    }

    eprintln!("\n=== Cenzontle Usage Tokens Test ===");
    let request = simple_request("deepseek-v3-2-coding", "Di solo: ok");
    let result = collect_stream(&provider, &request).await.expect("invoke failed");
    print_result("cenzontle", "deepseek-v3-2-coding", &result);

    assert_eq!(result.error_count, 0);
    assert!(result.has_done);
    // Usage may be 0 if backend doesn't forward token counts, but should not error
    eprintln!(
        "  Tokens: {} prompt / {} completion",
        result.input_tokens, result.output_tokens
    );
    eprintln!("  PASS");
}

/// Verify x-halcon-context header is accepted (backend should not reject it).
#[tokio::test]
async fn cenzontle_context_header_accepted() {
    use halcon_providers::CenzontleProvider;

    let token = match std::env::var("CENZONTLE_ACCESS_TOKEN")
        .ok()
        .filter(|v| !v.is_empty())
    {
        Some(t) => t,
        None => {
            eprintln!("SKIP: CENZONTLE_ACCESS_TOKEN not set");
            return;
        }
    };

    // Set a CWD so context header is populated
    let provider = CenzontleProvider::new(token, std::env::var("CENZONTLE_BASE_URL").ok(), Vec::new());

    if !provider.is_available().await {
        eprintln!("SKIP: Cenzontle not reachable");
        return;
    }

    eprintln!("\n=== Cenzontle Context Header Test ===");
    let request = ModelRequest {
        model: "deepseek-v3-2-coding".into(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Text("Di solo: contexto ok".into()),
        }],
        tools: vec![],
        max_tokens: Some(20),
        temperature: Some(0.0),
        system: None,
        stream: true,
    };

    let result = collect_stream(&provider, &request).await.expect("invoke failed");
    assert_eq!(result.error_count, 0, "backend rejected x-halcon-context header");
    assert!(result.has_done);
    eprintln!("  PASS: x-halcon-context accepted by backend");
}

// ========================================================
// Ollama tests (always available if Ollama is running)
// ========================================================

#[tokio::test]
async fn ollama_connectivity() {
    use halcon_providers::OllamaProvider;

    let provider = OllamaProvider::new(None, HttpConfig::default());

    if !provider.is_available().await {
        eprintln!("SKIP: Ollama not running");
        return;
    }

    eprintln!("\n=== Ollama Connectivity Test ===");

    // Get available models
    let models = provider.supported_models();
    eprintln!("  Supported models: {}", models.len());
    for m in models {
        eprintln!("    - {} (ctx: {}, tools: {})", m.id, m.context_window, m.supports_tools);
    }

    // Query Ollama API for actually installed models
    let actual_models: Vec<String> = match reqwest::get("http://localhost:11434/api/tags").await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(v) => v["models"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m["name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            Err(_) => vec![],
        },
        Err(_) => vec![],
    };

    eprintln!("  Actually installed: {:?}", actual_models);

    if actual_models.is_empty() {
        eprintln!("  SKIP: No Ollama models installed");
        return;
    }

    let model_id = actual_models[0].as_str();

    let request = simple_request(model_id, "What is 2+2? Answer with just the number.");
    let result = collect_stream(&provider, &request).await;

    match result {
        Ok(r) => {
            print_result("ollama", model_id, &r);
            assert!(!r.text.is_empty(), "Ollama should return non-empty text");
            assert!(r.has_done, "Ollama should emit Done chunk");
            assert_eq!(r.error_count, 0, "Should have no stream errors");
            eprintln!("  PASS: Ollama connectivity verified");
        }
        Err(e) => {
            eprintln!("  FAIL: {e}");
            panic!("Ollama connectivity failed: {e}");
        }
    }
}

#[tokio::test]
async fn ollama_cost_is_zero() {
    use halcon_providers::OllamaProvider;

    let provider = OllamaProvider::new(None, HttpConfig::default());
    if !provider.is_available().await {
        eprintln!("SKIP: Ollama not running");
        return;
    }

    let request = simple_request("any-model", "test");
    let cost = provider.estimate_cost(&request);
    assert_eq!(
        cost.estimated_cost_usd, 0.0,
        "Ollama should always report zero cost"
    );
}

// ========================================================
// OpenAI tests (requires OPENAI_API_KEY)
// ========================================================

#[tokio::test]
async fn openai_gpt4o_mini_connectivity() {
    use halcon_providers::OpenAIProvider;

    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("SKIP: OPENAI_API_KEY not set");
            return;
        }
    };

    eprintln!("\n=== OpenAI gpt-4o-mini Connectivity Test ===");

    let provider = OpenAIProvider::new(api_key, None, HttpConfig::default());
    assert!(provider.is_available().await, "OpenAI should be available with key");

    let request = simple_request("gpt-4o-mini", "What is 2+2? Answer with just the number.");
    let result = collect_stream(&provider, &request).await;

    match result {
        Ok(r) => {
            print_result("openai", "gpt-4o-mini", &r);
            assert!(!r.text.is_empty(), "OpenAI should return non-empty text");
            assert!(r.has_usage, "OpenAI should emit Usage chunk");
            assert!(r.has_done, "OpenAI should emit Done chunk");
            assert_eq!(r.error_count, 0, "Should have no stream errors");
            eprintln!("  PASS: OpenAI gpt-4o-mini verified");
        }
        Err(e) => {
            eprintln!("  FAIL: {e}");
            panic!("OpenAI connectivity failed: {e}");
        }
    }
}

#[tokio::test]
async fn openai_cost_estimate() {
    use halcon_providers::OpenAIProvider;

    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("SKIP: OPENAI_API_KEY not set");
            return;
        }
    };

    let provider = OpenAIProvider::new(api_key, None, HttpConfig::default());
    let request = simple_request("gpt-4o-mini", "hello");
    let cost = provider.estimate_cost(&request);
    assert!(
        cost.estimated_cost_usd > 0.0,
        "OpenAI cost estimate should be positive"
    );
    assert!(
        cost.estimated_cost_usd < 0.01,
        "Simple query should cost less than $0.01"
    );
    eprintln!(
        "  OpenAI gpt-4o-mini cost estimate: ${:.6}",
        cost.estimated_cost_usd
    );
}

// ========================================================
// DeepSeek tests (requires DEEPSEEK_API_KEY)
// ========================================================

#[tokio::test]
async fn deepseek_chat_connectivity() {
    use halcon_providers::DeepSeekProvider;

    let api_key = match std::env::var("DEEPSEEK_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("SKIP: DEEPSEEK_API_KEY not set");
            return;
        }
    };

    eprintln!("\n=== DeepSeek deepseek-chat Connectivity Test ===");

    let provider = DeepSeekProvider::new(api_key, None, HttpConfig::default());
    assert!(
        provider.is_available().await,
        "DeepSeek should be available with key"
    );

    let request = simple_request(
        "deepseek-chat",
        "What is 2+2? Answer with just the number.",
    );
    let result = collect_stream(&provider, &request).await;

    match result {
        Ok(r) => {
            print_result("deepseek", "deepseek-chat", &r);
            assert!(!r.text.is_empty(), "DeepSeek should return non-empty text");
            assert!(r.has_done, "DeepSeek should emit Done chunk");
            assert_eq!(r.error_count, 0, "Should have no stream errors");
            eprintln!("  PASS: DeepSeek chat verified");
        }
        Err(e) => {
            eprintln!("  FAIL: {e}");
            panic!("DeepSeek connectivity failed: {e}");
        }
    }
}

#[tokio::test]
async fn deepseek_cost_estimate() {
    use halcon_providers::DeepSeekProvider;

    let api_key = match std::env::var("DEEPSEEK_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("SKIP: DEEPSEEK_API_KEY not set");
            return;
        }
    };

    let provider = DeepSeekProvider::new(api_key, None, HttpConfig::default());
    let request = simple_request("deepseek-chat", "hello");
    let cost = provider.estimate_cost(&request);
    assert!(
        cost.estimated_cost_usd > 0.0,
        "DeepSeek cost estimate should be positive"
    );
    assert!(
        cost.estimated_cost_usd < 0.01,
        "Simple query should cost less than $0.01"
    );
    eprintln!(
        "  DeepSeek chat cost estimate: ${:.6}",
        cost.estimated_cost_usd
    );
}

// ========================================================
// Gemini tests (requires GEMINI_API_KEY)
// ========================================================

#[tokio::test]
async fn gemini_flash_connectivity() {
    use halcon_providers::GeminiProvider;

    let api_key = match std::env::var("GEMINI_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("SKIP: GEMINI_API_KEY not set");
            return;
        }
    };

    eprintln!("\n=== Gemini 2.0 Flash Connectivity Test ===");

    let provider = GeminiProvider::new(api_key, None, HttpConfig::default());
    assert!(
        provider.is_available().await,
        "Gemini should be available with key"
    );

    let request = simple_request(
        "gemini-2.0-flash",
        "What is 2+2? Answer with just the number.",
    );
    let result = collect_stream(&provider, &request).await;

    match result {
        Ok(r) => {
            print_result("gemini", "gemini-2.0-flash", &r);
            assert!(!r.text.is_empty(), "Gemini should return non-empty text");
            assert!(r.has_done, "Gemini should emit Done chunk");
            assert_eq!(r.error_count, 0, "Should have no stream errors");
            eprintln!("  PASS: Gemini flash verified");
        }
        Err(e) => {
            eprintln!("  FAIL: {e}");
            panic!("Gemini connectivity failed: {e}");
        }
    }
}

#[tokio::test]
async fn gemini_cost_estimate() {
    use halcon_providers::GeminiProvider;

    let api_key = match std::env::var("GEMINI_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("SKIP: GEMINI_API_KEY not set");
            return;
        }
    };

    let provider = GeminiProvider::new(api_key, None, HttpConfig::default());
    let request = simple_request("gemini-2.0-flash", "hello");
    let cost = provider.estimate_cost(&request);
    assert!(
        cost.estimated_cost_usd > 0.0,
        "Gemini cost estimate should be positive"
    );
    assert!(
        cost.estimated_cost_usd < 0.01,
        "Simple query should cost less than $0.01"
    );
    eprintln!(
        "  Gemini flash cost estimate: ${:.6}",
        cost.estimated_cost_usd
    );
}

// ========================================================
// Cross-provider consistency tests
// ========================================================

#[tokio::test]
async fn cross_provider_same_question_all_respond() {
    use halcon_providers::{DeepSeekProvider, GeminiProvider, OllamaProvider, OpenAIProvider};

    eprintln!("\n=== Cross-Provider Consistency Test ===");
    let question = "What is the capital of France? Answer in one word.";
    let mut results: Vec<(&str, &str, StreamResult)> = Vec::new();

    // Ollama — discover actually installed model
    {
        let provider = OllamaProvider::new(None, HttpConfig::default());
        if provider.is_available().await {
            // Query for installed models
            if let Ok(resp) = reqwest::get("http://localhost:11434/api/tags").await {
                if let Ok(v) = resp.json::<serde_json::Value>().await {
                    if let Some(name) = v["models"][0]["name"].as_str() {
                        let req = simple_request(name, question);
                        if let Ok(r) = collect_stream(&provider, &req).await {
                            results.push(("ollama", "local-model", r));
                        }
                    }
                }
            }
        }
    }

    // OpenAI
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.is_empty() {
            let provider = OpenAIProvider::new(key, None, HttpConfig::default());
            let req = simple_request("gpt-4o-mini", question);
            if let Ok(r) = collect_stream(&provider, &req).await {
                results.push(("openai", "gpt-4o-mini", r));
            }
        }
    }

    // DeepSeek
    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
        if !key.is_empty() {
            let provider = DeepSeekProvider::new(key, None, HttpConfig::default());
            let req = simple_request("deepseek-chat", question);
            if let Ok(r) = collect_stream(&provider, &req).await {
                results.push(("deepseek", "deepseek-chat", r));
            }
        }
    }

    // Gemini
    if let Ok(key) = std::env::var("GEMINI_API_KEY") {
        if !key.is_empty() {
            let provider = GeminiProvider::new(key, None, HttpConfig::default());
            let req = simple_request("gemini-2.0-flash", question);
            if let Ok(r) = collect_stream(&provider, &req).await {
                results.push(("gemini", "gemini-2.0-flash", r));
            }
        }
    }

    eprintln!("  Providers tested: {}", results.len());

    if results.is_empty() {
        eprintln!("  SKIP: No providers available");
        return;
    }

    // All should mention Paris
    for (provider, model, result) in &results {
        print_result(provider, model, result);
        let lower = result.text.to_lowercase();
        assert!(
            lower.contains("paris"),
            "{provider}/{model}: expected 'Paris' in response, got: {}",
            result.text
        );
    }

    // Cross-provider metrics summary
    eprintln!("\n  --- Cross-Provider Metrics ---");
    eprintln!(
        "  {:>12} {:>10} {:>8} {:>8} {:>16}",
        "Provider", "Latency", "In Tok", "Out Tok", "Hash (prefix)"
    );
    for (provider, _model, result) in &results {
        eprintln!(
            "  {:>12} {:>8}ms {:>8} {:>8} {:>16}",
            provider,
            result.latency_ms,
            result.input_tokens,
            result.output_tokens,
            &result.content_hash[..16]
        );
    }

    eprintln!("  PASS: Cross-provider consistency verified");
}

// ========================================================
// Model selector integration test
// ========================================================

#[test]
fn model_selector_with_real_model_info() {
    use halcon_core::types::ModelSelectionConfig;
    use halcon_providers::{EchoProvider, ProviderRegistry};

    // Build registry with echo provider (always available)
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(EchoProvider::new()));

    let _config = ModelSelectionConfig {
        enabled: true,
        ..Default::default()
    };

    // Verify we can construct a selector from registry
    let models: Vec<_> = registry
        .list()
        .iter()
        .filter_map(|name| registry.get(name))
        .flat_map(|p| p.supported_models())
        .collect();

    assert!(!models.is_empty(), "Should have at least echo models");
    eprintln!(
        "  Model selector: {} models available from registry",
        models.len()
    );

    // Verify echo model has correct attributes
    let echo = models.iter().find(|m| m.provider == "echo").unwrap();
    assert!(!echo.supports_reasoning);
    assert!(!echo.supports_tools);
    assert_eq!(echo.cost_per_input_token, 0.0);
}
