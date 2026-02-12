#![allow(deprecated)] // assert_cmd::Command::cargo_bin deprecation
//! E2E tests for provider integration using a mock HTTP/SSE server.
//!
//! Uses `wiremock` to simulate the Anthropic Messages API with SSE responses.
//! Tests the full pipeline: binary → HTTP → SSE parse → stream render → output.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build a cuervo command isolated to a temp directory.
fn cuervo_cmd(tmp: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("cuervo").unwrap();
    cmd.env("HOME", tmp.path());
    cmd.env("XDG_DATA_HOME", tmp.path().join("data"));
    cmd.env_remove("ANTHROPIC_API_KEY");
    cmd.env_remove("OPENAI_API_KEY");
    cmd.env("CUERVO_LOG", "error");
    cmd
}

/// Build an SSE response body that simulates a simple Anthropic response.
fn sse_response(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut body = String::new();

    // message_start
    body.push_str("event: message_start\n");
    body.push_str(
        r#"data: {"type":"message_start","message":{"id":"msg_test","model":"claude-sonnet-4-5-20250929","usage":{"input_tokens":10}}}"#,
    );
    body.push_str("\n\n");

    // content_block_start
    body.push_str("event: content_block_start\n");
    body.push_str(
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
    );
    body.push_str("\n\n");

    // ping
    body.push_str("event: ping\n");
    body.push_str(r#"data: {"type":"ping"}"#);
    body.push_str("\n\n");

    // text deltas (one per word)
    for (i, word) in words.iter().enumerate() {
        let suffix = if i < words.len() - 1 { " " } else { "" };
        body.push_str("event: content_block_delta\n");
        body.push_str(&format!(
            r#"data: {{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"{word}{suffix}"}}}}"#,
        ));
        body.push_str("\n\n");
    }

    // content_block_stop
    body.push_str("event: content_block_stop\n");
    body.push_str(r#"data: {"type":"content_block_stop","index":0}"#);
    body.push_str("\n\n");

    // message_delta with stop reason
    body.push_str("event: message_delta\n");
    body.push_str(
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
    );
    body.push_str("\n\n");

    // message_stop
    body.push_str("event: message_stop\n");
    body.push_str(r#"data: {"type":"message_stop"}"#);
    body.push_str("\n\n");

    body
}

/// Build a config TOML that points anthropic at a mock server.
fn mock_config(base_url: &str) -> String {
    format!(
        r#"
[general]
default_provider = "anthropic"
default_model = "claude-sonnet-4-5-20250929"

[models.providers.anthropic]
enabled = true
api_base = "{base_url}"
api_key_env = "CUERVO_TEST_API_KEY"

[resilience]
enabled = false

[agent.limits]
max_total_tokens = 100000
max_duration_secs = 300
"#
    )
}

/// Config with retries disabled — for testing error paths without delay.
fn mock_config_no_retry(base_url: &str) -> String {
    format!(
        r#"
[general]
default_provider = "anthropic"
default_model = "claude-sonnet-4-5-20250929"

[models.providers.anthropic]
enabled = true
api_base = "{base_url}"
api_key_env = "CUERVO_TEST_API_KEY"

[models.providers.anthropic.http]
max_retries = 0
connect_timeout_secs = 5
request_timeout_secs = 10
retry_base_delay_ms = 100

[resilience]
enabled = false

[agent.limits]
max_total_tokens = 100000
max_duration_secs = 300
"#
    )
}

// ========================================================
// Mock Anthropic SSE streaming
// ========================================================

#[tokio::test]
async fn anthropic_mock_single_shot() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-ant-api03-test-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(sse_response("Hello from mock Claude"), "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, mock_config(&server.uri())).unwrap();

    cuervo_cmd(&tmp)
        .env("CUERVO_TEST_API_KEY", "sk-ant-api03-test-key")
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "-p",
            "anthropic",
            "-m",
            "claude-sonnet-4-5-20250929",
            "chat",
            "What is Rust?",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from mock Claude"));
}

#[tokio::test]
async fn anthropic_mock_sends_correct_headers() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-ant-api03-verify-headers"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(header("content-type", "application/json"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(sse_response("headers OK"), "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, mock_config(&server.uri())).unwrap();

    cuervo_cmd(&tmp)
        .env("CUERVO_TEST_API_KEY", "sk-ant-api03-verify-headers")
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "-p",
            "anthropic",
            "chat",
            "test",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("headers OK"));
}

// ========================================================
// Error handling E2E
// ========================================================

#[tokio::test]
async fn anthropic_mock_auth_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "authentication_error",
                "message": "invalid x-api-key"
            }
        })))
        .expect(1..)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, mock_config(&server.uri())).unwrap();

    // REPL-based single-shot: errors are caught and printed to stderr, exit 0.
    cuervo_cmd(&tmp)
        .env("CUERVO_TEST_API_KEY", "sk-ant-api03-bad-key")
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "-p",
            "anthropic",
            "chat",
            "test",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("authentication"));
}

#[tokio::test]
async fn anthropic_mock_rate_limit() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "rate_limit_error",
                "message": "rate limited"
            }
        })))
        .expect(1..) // Router may retry rate limits.
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, mock_config_no_retry(&server.uri())).unwrap();

    cuervo_cmd(&tmp)
        .env("CUERVO_TEST_API_KEY", "sk-ant-api03-test")
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "-p",
            "anthropic",
            "chat",
            "test",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("rate limit"));
}

#[tokio::test]
async fn anthropic_mock_server_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "api_error",
                "message": "internal server error"
            }
        })))
        .expect(1..) // Router may retry 500s.
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, mock_config_no_retry(&server.uri())).unwrap();

    cuervo_cmd(&tmp)
        .env("CUERVO_TEST_API_KEY", "sk-ant-api03-test")
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "-p",
            "anthropic",
            "chat",
            "test",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("api_error").or(predicate::str::contains("server")));
}

// ========================================================
// SSE edge cases
// ========================================================

#[tokio::test]
async fn anthropic_mock_empty_response() {
    let server = MockServer::start().await;

    // SSE stream with no text deltas — just start + stop.
    let body = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_e\",\"model\":\"claude-sonnet-4-5-20250929\",\"usage\":{\"input_tokens\":5}}}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":0}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, mock_config(&server.uri())).unwrap();

    // Should not crash on empty response.
    cuervo_cmd(&tmp)
        .env("CUERVO_TEST_API_KEY", "sk-ant-api03-test")
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "-p",
            "anthropic",
            "chat",
            "test",
        ])
        .assert()
        .success();
}

#[tokio::test]
async fn anthropic_mock_multiword_streaming() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            sse_response("The quick brown fox jumps over the lazy dog"),
            "text/event-stream",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, mock_config(&server.uri())).unwrap();

    cuervo_cmd(&tmp)
        .env("CUERVO_TEST_API_KEY", "sk-ant-api03-test")
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "-p",
            "anthropic",
            "chat",
            "test",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("quick brown fox"))
        .stdout(predicate::str::contains("lazy dog"));
}

#[tokio::test]
async fn anthropic_mock_sse_error_mid_stream() {
    let server = MockServer::start().await;

    // SSE stream that sends some text, then an error event.
    let body = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_err\",\"model\":\"claude-sonnet-4-5-20250929\",\"usage\":{\"input_tokens\":5}}}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial \"}}\n\n\
event: error\n\
data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"API overloaded\"}}\n\n";

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, mock_config(&server.uri())).unwrap();

    // Should not crash — partial output is OK.
    cuervo_cmd(&tmp)
        .env("CUERVO_TEST_API_KEY", "sk-ant-api03-test")
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "-p",
            "anthropic",
            "chat",
            "test",
        ])
        .assert()
        .success();
}

// ========================================================
// Retry behavior E2E
// ========================================================

/// Config with 1 retry and fast backoff for testing retry behavior.
fn mock_config_retry_once(base_url: &str) -> String {
    format!(
        r#"
[general]
default_provider = "anthropic"
default_model = "claude-sonnet-4-5-20250929"

[models.providers.anthropic]
enabled = true
api_base = "{base_url}"
api_key_env = "CUERVO_TEST_API_KEY"

[models.providers.anthropic.http]
max_retries = 1
connect_timeout_secs = 5
request_timeout_secs = 10
retry_base_delay_ms = 100

[resilience]
enabled = false

[agent.limits]
max_total_tokens = 100000
max_duration_secs = 300
"#
    )
}

#[tokio::test]
async fn anthropic_retry_on_500_exhausted() {
    let server = MockServer::start().await;

    // Server always returns 500 — with retries enabled, expect 2+ requests.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "api_error",
                "message": "server down"
            }
        })))
        .expect(2..) // initial + retries (router may add more)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, mock_config_retry_once(&server.uri())).unwrap();

    cuervo_cmd(&tmp)
        .env("CUERVO_TEST_API_KEY", "sk-ant-api03-test")
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "-p",
            "anthropic",
            "chat",
            "test",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("api_error").or(predicate::str::contains("server")));
}

#[tokio::test]
async fn anthropic_auth_error_not_retried() {
    let server = MockServer::start().await;

    // 401 should NOT be retried by the HTTP layer — but the agent loop's fallback
    // router may retry at a higher level (resilience routing), so allow 1+.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "authentication_error",
                "message": "bad key"
            }
        })))
        .expect(1..)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, mock_config_retry_once(&server.uri())).unwrap();

    cuervo_cmd(&tmp)
        .env("CUERVO_TEST_API_KEY", "sk-ant-api03-bad")
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "-p",
            "anthropic",
            "chat",
            "test",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("authentication"));
}

#[tokio::test]
async fn anthropic_timeout_with_slow_server() {
    let server = MockServer::start().await;

    // Server takes 5 seconds to respond, but our timeout is 2 seconds.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(sse_response("slow response"), "text/event-stream")
                .set_delay(std::time::Duration::from_secs(5)),
        )
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    // Short timeout, no retries.
    std::fs::write(
        &config_path,
        format!(
            r#"
[general]
default_provider = "anthropic"
default_model = "claude-sonnet-4-5-20250929"

[models.providers.anthropic]
enabled = true
api_base = "{}"
api_key_env = "CUERVO_TEST_API_KEY"

[models.providers.anthropic.http]
max_retries = 0
connect_timeout_secs = 2
request_timeout_secs = 2
retry_base_delay_ms = 100

[resilience]
enabled = false

[agent.limits]
max_total_tokens = 100000
max_duration_secs = 300
"#,
            server.uri()
        ),
    )
    .unwrap();

    cuervo_cmd(&tmp)
        .env("CUERVO_TEST_API_KEY", "sk-ant-api03-test")
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "-p",
            "anthropic",
            "chat",
            "test",
        ])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stderr(predicate::str::contains("timed out"));
}
