#![allow(deprecated)] // assert_cmd::Command::cargo_bin deprecation
//! FASE 6 — E2E tests for multi-provider integration.
//!
//! Uses wiremock to simulate various provider APIs (OpenAI, DeepSeek)
//! and verifies the full binary pipeline works end-to-end.

mod common;

use predicates::prelude::*;
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{halcon_cmd, openai_sse, single_provider_config};

fn write_config(tmp: &TempDir, config: &str) -> String {
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, config).unwrap();
    config_path.to_str().unwrap().to_string()
}

#[tokio::test]
async fn openai_mock_single_shot() {
    let server = MockServer::start().await;
    let body = openai_sse("Hello from mock OpenAI", "gpt-4o");

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer sk-test-openai"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(body, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config = single_provider_config("openai", &server.uri(), "OPENAI_API_KEY", "gpt-4o");
    let config_path = write_config(&tmp, &config);

    halcon_cmd(&tmp)
        .env("OPENAI_API_KEY", "sk-test-openai")
        .args(["--config", &config_path, "-p", "openai", "chat", "Say hello"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from mock OpenAI"));
}

#[tokio::test]
async fn deepseek_mock_single_shot() {
    let server = MockServer::start().await;
    let body = openai_sse("Hello from mock DeepSeek", "deepseek-chat");

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(body, "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config = single_provider_config("deepseek", &server.uri(), "DEEPSEEK_API_KEY", "deepseek-chat");
    let config_path = write_config(&tmp, &config);

    halcon_cmd(&tmp)
        .env("DEEPSEEK_API_KEY", "sk-test-deepseek")
        .args(["--config", &config_path, "-p", "deepseek", "chat", "Say hello"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from mock DeepSeek"));
}

#[tokio::test]
async fn openai_mock_tool_call() {
    let server = MockServer::start().await;

    // Build a response with text content
    let body = format!(
        "data: {{\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"I will help you. Let me think...\"}},\"finish_reason\":null}}]}}\n\n\
         data: {{\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}],\"usage\":{{\"prompt_tokens\":10,\"completion_tokens\":5}}}}\n\n\
         data: [DONE]\n\n"
    );

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(body, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config = single_provider_config("openai", &server.uri(), "OPENAI_API_KEY", "gpt-4o");
    let config_path = write_config(&tmp, &config);

    halcon_cmd(&tmp)
        .env("OPENAI_API_KEY", "sk-test-openai")
        .args(["--config", &config_path, "-p", "openai", "chat", "Help me with something"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("help you"));
}

#[tokio::test]
async fn deepseek_reasoning_content() {
    let server = MockServer::start().await;
    let body = common::deepseek_reasoning_sse(
        "Let me think about this step by step...",
        "The answer is 42."
    );

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(body, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config = single_provider_config("deepseek", &server.uri(), "DEEPSEEK_API_KEY", "deepseek-chat");
    let config_path = write_config(&tmp, &config);

    halcon_cmd(&tmp)
        .env("DEEPSEEK_API_KEY", "sk-test-ds")
        .args(["--config", &config_path, "-p", "deepseek", "chat", "What is the meaning?"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("42"));
}

#[tokio::test]
async fn provider_switch_on_failure() {
    let primary = MockServer::start().await;
    let fallback = MockServer::start().await;

    // Primary always returns 500 (retryable error → triggers fallback).
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .expect(1..) // may be retried before fallback
        .mount(&primary)
        .await;

    // Fallback returns success via Anthropic SSE.
    let body = common::anthropic_sse("Hello from fallback Anthropic");
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(body, "text/event-stream"),
        )
        .expect(1)
        .mount(&fallback)
        .await;

    let tmp = TempDir::new().unwrap();
    let config = common::fallback_config(&primary.uri(), &fallback.uri());
    let config_path = write_config(&tmp, &config);

    halcon_cmd(&tmp)
        .env("OPENAI_API_KEY", "sk-test-openai")
        .env("ANTHROPIC_API_KEY", "sk-ant-test")
        .args(["--config", &config_path, "-p", "openai", "chat", "Hello"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from fallback Anthropic"));
}

#[tokio::test]
async fn openai_auth_error_reports_authentication() {
    let server = MockServer::start().await;

    // 401 auth error — non-retryable, should fail fast with auth error message.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string(r#"{"error":{"message":"Invalid API key","type":"authentication_error"}}"#),
        )
        .expect(1..)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let config = single_provider_config("openai", &server.uri(), "OPENAI_API_KEY", "gpt-4o");
    let config_path = write_config(&tmp, &config);

    halcon_cmd(&tmp)
        .env("OPENAI_API_KEY", "sk-bad-key")
        .args(["--config", &config_path, "-p", "openai", "chat", "Test auth"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stderr(predicate::str::contains("authentication").or(predicate::str::contains("401")));
}
