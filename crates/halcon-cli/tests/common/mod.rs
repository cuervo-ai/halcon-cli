#![allow(deprecated)] // assert_cmd::Command::cargo_bin deprecation
//! FASE 6 — Shared E2E test infrastructure for multi-provider testing.
//!
//! Provides SSE response builders and config helpers for wiremock-based E2E tests.

use assert_cmd::Command;
use tempfile::TempDir;

/// Build a halcon command isolated to a temp directory.
pub fn halcon_cmd(tmp: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("halcon").unwrap();
    cmd.env("HOME", tmp.path());
    cmd.env("XDG_DATA_HOME", tmp.path().join("data"));
    cmd.env_remove("ANTHROPIC_API_KEY");
    cmd.env_remove("OPENAI_API_KEY");
    cmd.env_remove("DEEPSEEK_API_KEY");
    cmd.env_remove("GEMINI_API_KEY");
    cmd.env("HALCON_LOG", "error");
    cmd
}

/// Build an Anthropic SSE response body.
pub fn anthropic_sse(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut body = String::new();

    body.push_str("event: message_start\n");
    body.push_str(
        r#"data: {"type":"message_start","message":{"id":"msg_test","model":"claude-sonnet-4-5-20250929","usage":{"input_tokens":10}}}"#,
    );
    body.push_str("\n\n");

    body.push_str("event: content_block_start\n");
    body.push_str(
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
    );
    body.push_str("\n\n");

    for (i, word) in words.iter().enumerate() {
        let suffix = if i < words.len() - 1 { " " } else { "" };
        body.push_str("event: content_block_delta\n");
        body.push_str(&format!(
            r#"data: {{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"{word}{suffix}"}}}}"#,
        ));
        body.push_str("\n\n");
    }

    body.push_str("event: content_block_stop\n");
    body.push_str(r#"data: {"type":"content_block_stop","index":0}"#);
    body.push_str("\n\n");

    body.push_str("event: message_delta\n");
    body.push_str(
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
    );
    body.push_str("\n\n");

    body.push_str("event: message_stop\n");
    body.push_str(r#"data: {"type":"message_stop"}"#);
    body.push_str("\n\n");

    body
}

/// Build an OpenAI-compatible SSE response body.
pub fn openai_sse(text: &str, model: &str) -> String {
    let mut body = String::new();
    let words: Vec<&str> = text.split_whitespace().collect();

    for (i, word) in words.iter().enumerate() {
        let suffix = if i < words.len() - 1 { " " } else { "" };
        body.push_str(&format!(
            "data: {{\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"model\":\"{model}\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{word}{suffix}\"}},\"finish_reason\":null}}]}}\n\n"
        ));
    }

    // Final chunk with finish_reason
    body.push_str(&format!(
        "data: {{\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"model\":\"{model}\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}],\"usage\":{{\"prompt_tokens\":10,\"completion_tokens\":5}}}}\n\n"
    ));
    body.push_str("data: [DONE]\n\n");

    body
}

/// Build a DeepSeek SSE response with reasoning_content.
pub fn deepseek_reasoning_sse(reasoning: &str, text: &str) -> String {
    let mut body = String::new();

    // Reasoning content chunk
    body.push_str(&format!(
        "data: {{\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"model\":\"deepseek-reasoner\",\"choices\":[{{\"index\":0,\"delta\":{{\"reasoning_content\":\"{reasoning}\"}},\"finish_reason\":null}}]}}\n\n"
    ));

    // Regular content chunk
    body.push_str(&format!(
        "data: {{\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"model\":\"deepseek-reasoner\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{text}\"}},\"finish_reason\":null}}]}}\n\n"
    ));

    // Final chunk
    body.push_str(
        "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"model\":\"deepseek-reasoner\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n"
    );
    body.push_str("data: [DONE]\n\n");

    body
}

/// Build a config TOML for a single provider pointing at a mock server.
///
/// Uses the real config structure: `[models.providers.<name>]` with `api_base` and `api_key_env`.
pub fn single_provider_config(provider: &str, base_url: &str, api_key_env: &str, model: &str) -> String {
    let resilience = r#"
[resilience]
enabled = false

[agent.limits]
max_total_tokens = 100000
max_duration_secs = 300
"#;

    match provider {
        "anthropic" => format!(
            r#"
[general]
default_provider = "anthropic"
default_model = "{model}"

[models.providers.anthropic]
enabled = true
api_base = "{base_url}"
api_key_env = "{api_key_env}"

[models.providers.anthropic.http]
max_retries = 0
connect_timeout_secs = 5
request_timeout_secs = 10
retry_base_delay_ms = 100
{resilience}"#
        ),
        "openai" => format!(
            r#"
[general]
default_provider = "openai"
default_model = "{model}"

[models.providers.openai]
enabled = true
api_base = "{base_url}"
api_key_env = "{api_key_env}"

[models.providers.openai.http]
max_retries = 0
connect_timeout_secs = 5
request_timeout_secs = 10
retry_base_delay_ms = 100
{resilience}"#
        ),
        "deepseek" => format!(
            r#"
[general]
default_provider = "deepseek"
default_model = "{model}"

[models.providers.deepseek]
enabled = true
api_base = "{base_url}"
api_key_env = "{api_key_env}"

[models.providers.deepseek.http]
max_retries = 0
connect_timeout_secs = 5
request_timeout_secs = 10
retry_base_delay_ms = 100
{resilience}"#
        ),
        _ => panic!("unsupported provider: {provider}"),
    }
}

/// Build a config TOML with primary OpenAI + fallback Anthropic for failover testing.
///
/// Enables resilience + routing with fallback_models so the router tries anthropic
/// when openai fails with a retryable error (500).
pub fn fallback_config(primary_url: &str, fallback_url: &str) -> String {
    format!(
        r#"
[general]
default_provider = "openai"
default_model = "gpt-4o"

[models.providers.openai]
enabled = true
api_base = "{primary_url}"
api_key_env = "OPENAI_API_KEY"

[models.providers.openai.http]
max_retries = 1
connect_timeout_secs = 5
request_timeout_secs = 10
retry_base_delay_ms = 100

[models.providers.anthropic]
enabled = true
api_base = "{fallback_url}"
api_key_env = "ANTHROPIC_API_KEY"

[models.providers.anthropic.http]
max_retries = 0
connect_timeout_secs = 5
request_timeout_secs = 10
retry_base_delay_ms = 100

[resilience]
enabled = true

[agent.routing]
mode = "failover"
fallback_models = ["anthropic"]
max_retries = 1

[agent.limits]
max_total_tokens = 100000
max_duration_secs = 300
"#
    )
}
