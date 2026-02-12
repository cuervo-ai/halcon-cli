#![allow(deprecated)] // assert_cmd::Command::cargo_bin deprecation
//! End-to-end integration tests for the `cuervo` binary.
//!
//! These tests exercise the compiled binary using `assert_cmd`,
//! isolated temp directories (no home pollution), and mock HTTP servers.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// Build a Command for the cuervo binary with an isolated environment.
///
/// Sets CUERVO_CONFIG to a temp config file so no real home files are touched.
/// Sets HOME to a temp dir so keychain/history don't pollute the real system.
fn cuervo_cmd(tmp: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("cuervo").unwrap();
    // Isolate from real user config.
    cmd.env("HOME", tmp.path());
    cmd.env("XDG_DATA_HOME", tmp.path().join("data"));
    // Prevent real ANTHROPIC_API_KEY from leaking in.
    cmd.env_remove("ANTHROPIC_API_KEY");
    cmd.env_remove("OPENAI_API_KEY");
    // Suppress log noise.
    cmd.env("CUERVO_LOG", "error");
    cmd
}

// ========================================================
// Basic CLI smoke tests
// ========================================================

#[test]
fn version_flag() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("cuervo"));
}

#[test]
fn help_flag() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("AI-powered CLI"))
        .stdout(predicate::str::contains("chat"))
        .stdout(predicate::str::contains("auth"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("--verbose"));
}

#[test]
fn help_subcommand() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .args(["help", "chat"])
        .assert()
        .success()
        .stdout(predicate::str::contains("interactive chat"));
}

#[test]
fn unknown_subcommand_fails() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .arg("nonexistent")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
}

// ========================================================
// Status command
// ========================================================

#[test]
fn status_shows_provider_info() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Provider:"))
        .stdout(predicate::str::contains("Configured providers:"));
}

// ========================================================
// Config command
// ========================================================

#[test]
fn config_show_outputs_json_like() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("default_provider"));
}

#[test]
fn config_path_shows_path() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("config.toml"));
}

// ========================================================
// Auth command
// ========================================================

#[test]
fn auth_status_shows_providers() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("anthropic:"))
        .stdout(predicate::str::contains("openai:"))
        .stdout(predicate::str::contains("ollama:"));
}

#[test]
fn auth_help() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .args(["auth", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("login"))
        .stdout(predicate::str::contains("logout"))
        .stdout(predicate::str::contains("status"));
}

// ========================================================
// Echo provider — single-shot E2E
// ========================================================

#[test]
fn echo_provider_single_shot() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .args(["-p", "echo", "-m", "echo", "chat", "hello world"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Echo:"))
        .stdout(predicate::str::contains("hello world"));
}

#[test]
fn echo_provider_markdown_formatting() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .args(["-p", "echo", "-m", "echo", "chat", "test markdown"])
        .assert()
        .success()
        .stdout(predicate::str::contains("test markdown"));
}

#[test]
fn echo_provider_empty_prompt() {
    // Even empty-ish prompts should not crash.
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .args(["-p", "echo", "-m", "echo", "chat", " "])
        .assert()
        .success();
}

#[test]
fn echo_provider_long_prompt() {
    let tmp = TempDir::new().unwrap();
    let long_prompt = "word ".repeat(200);
    cuervo_cmd(&tmp)
        .args(["-p", "echo", "-m", "echo", "chat", &long_prompt])
        .assert()
        .success()
        .stdout(predicate::str::contains("word"));
}

// ========================================================
// Provider not configured — graceful error
// ========================================================

#[test]
fn missing_provider_shows_error() {
    let tmp = TempDir::new().unwrap();
    let output = cuervo_cmd(&tmp)
        .args([
            "-p",
            "anthropic",
            "-m",
            "claude-sonnet-4-5-20250929",
            "chat",
            "hi",
        ])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}{stderr}");

    // With resilience hardening, the system either:
    // (a) falls back to a local provider (Ollama) and may succeed or fail with model error, or
    // (b) shows "no providers available" if nothing works, or
    // (c) shows the original "not configured" message.
    // In all cases, a warning about the primary provider should appear.
    assert!(
        combined.contains("not registered")
            || combined.contains("not available")
            || combined.contains("not configured")
            || combined.contains("fallback provider")
            || combined.contains("no providers available"),
        "expected provider unavailability message, got:\nstdout: {stdout}\nstderr: {stderr}"
    );
}

// ========================================================
// Config override via --config flag
// ========================================================

#[test]
fn custom_config_file_loaded() {
    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("custom.toml");
    std::fs::write(
        &config_path,
        r#"
[general]
default_provider = "echo"
default_model = "echo"
"#,
    )
    .unwrap();

    cuervo_cmd(&tmp)
        .args(["--config", config_path.to_str().unwrap(), "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("echo"));
}

// ========================================================
// Environment variable overrides
// ========================================================

#[test]
fn env_var_overrides_provider() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .env("CUERVO_PROVIDER", "echo")
        .env("CUERVO_MODEL", "echo")
        .args(["chat", "from env var"])
        .assert()
        .success()
        .stdout(predicate::str::contains("from env var"));
}

#[test]
fn env_var_overrides_model() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .env("CUERVO_PROVIDER", "echo")
        .env("CUERVO_MODEL", "echo")
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Model:"))
        .stdout(predicate::str::contains("echo"));
}

// ========================================================
// Verbose flag
// ========================================================

#[test]
fn verbose_flag_enables_debug_output() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .env_remove("CUERVO_LOG") // allow --verbose to take effect
        .args(["-v", "-p", "echo", "-m", "echo", "chat", "hi"])
        .assert()
        .success()
        // Debug logs go to stderr.
        .stderr(predicate::str::contains("DEBUG"));
}

// ========================================================
// Security: secrets not leaked in verbose output
// ========================================================

#[test]
fn verbose_output_never_contains_api_key() {
    let tmp = TempDir::new().unwrap();
    let secret_key = "sk-ant-super-secret-key-99999";

    let output = cuervo_cmd(&tmp)
        .env_remove("CUERVO_LOG")
        .env("ANTHROPIC_API_KEY", secret_key)
        .args(["-v", "-p", "echo", "-m", "echo", "chat", "security test"])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stdout.contains(secret_key),
        "stdout must never contain the API key"
    );
    assert!(
        !stderr.contains(secret_key),
        "stderr must never contain the API key"
    );
}

// ========================================================
// --trace-json flag
// ========================================================

#[test]
fn trace_json_flag_emits_json_lines() {
    let tmp = TempDir::new().unwrap();
    let output = cuervo_cmd(&tmp)
        .env_remove("CUERVO_LOG")
        .args(["--trace-json", "-p", "echo", "-m", "echo", "chat", "test"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    // JSON trace output must contain at least one JSON object line.
    let has_json = stderr.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with('{') && trimmed.ends_with('}')
    });
    assert!(
        has_json,
        "stderr should contain JSON lines when --trace-json is active, got: {stderr}"
    );
}

#[test]
fn no_trace_json_by_default() {
    let tmp = TempDir::new().unwrap();
    let output = cuervo_cmd(&tmp)
        .args(["-p", "echo", "-m", "echo", "chat", "hello"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Without --trace-json, stderr should NOT contain JSON lines (only plain text or empty).
    let has_json = stderr.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with('{') && trimmed.ends_with('}')
    });
    assert!(
        !has_json,
        "stderr should NOT contain JSON lines without --trace-json"
    );
}

// ========================================================
// Init command
// ========================================================

#[test]
fn init_creates_project_config() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .current_dir(tmp.path())
        .arg("init")
        .assert()
        .success();

    // Verify .cuervo/ directory was created.
    assert!(tmp.path().join(".cuervo").exists());
}

#[test]
fn init_force_reinitializes() {
    let tmp = TempDir::new().unwrap();
    // First init.
    cuervo_cmd(&tmp)
        .current_dir(tmp.path())
        .arg("init")
        .assert()
        .success();

    // Second init with --force should not fail.
    cuervo_cmd(&tmp)
        .current_dir(tmp.path())
        .args(["init", "--force"])
        .assert()
        .success();
}

// ========================================================
// Memory commands
// ========================================================

#[test]
fn memory_stats_on_fresh_db() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .args(["memory", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Total entries: 0"));
}

#[test]
fn memory_list_empty() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .args(["memory", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No memory entries found"));
}

#[test]
fn memory_search_empty() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .args(["memory", "search", "rust"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No results"));
}

#[test]
fn memory_prune_nothing() {
    let tmp = TempDir::new().unwrap();
    cuervo_cmd(&tmp)
        .args(["memory", "prune"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Nothing to prune"));
}
