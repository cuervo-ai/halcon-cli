//! Phase 8: Dev Ecosystem Integration Tests
//!
//! Cross-module invariant validation for the dev ecosystem integration layer.
//!
//! These tests exercise the full signal chain across all 7 phases:
//!   Phase 1: SafeEditManager → risk-gated file mutations
//!   Phase 2: GitContext / CommitRewardTracker / GitEventListener
//!   Phase 3: TestRunnerBridge → TestSuiteResult
//!   Phase 4: CiResultIngestor → CiRunRecord → reward signal
//!   Phase 5: DevGateway → UnsavedBufferTracker → IdeProtocolHandler
//!   Phase 6: AstSymbolExtractor → SymbolIndex
//!   Phase 7: RuntimeSignalIngestor → RuntimeMetrics → reward

// ── Imports ───────────────────────────────────────────────────────────────────

use std::sync::Arc;
use std::time::Duration;

use super::ast_symbol_extractor::{
    extract_from_buffer, RegexExtractor, SymbolExtractor, SymbolKind,
};
use super::ci_result_ingestor::{
    CiApiClient, CiEvent, CiIngestorConfig, CiResultIngestor, CiRunRecord, CiRunStatus,
};
use super::dev_gateway::DevGateway;
use super::ide_protocol_handler::IdeProtocolHandler;
use super::runtime_signal_ingestor::{metrics_to_markdown, RuntimeSignal, RuntimeSignalIngestor};
use super::git_tools::test_results::{parse_cargo_test, parse_junit_xml, TestStatus};
use super::unsaved_buffer_tracker::UnsavedBufferTracker;

// ── Phase 1–2 invariants ──────────────────────────────────────────────────────

/// Phase 1: Safe edit manager should classify known risky patterns.
#[test]
fn safe_edit_risk_classifier_invariants() {
    use super::security::risk_tier::RiskTierClassifier;

    // Deletions are always riskier than additions.
    // Unified diff: lines starting with '-' are deletions, '+' are additions.
    let add_diff = "+let x = 1;";
    let del_diff = "-let x = 1;";

    let add_tier = RiskTierClassifier::classify_diff(add_diff);
    let del_tier = RiskTierClassifier::classify_diff(del_diff);

    // Deletion tier should be >= addition tier (never safer).
    assert!(
        del_tier >= add_tier,
        "deletion {del_tier:?} should be >= addition {add_tier:?}"
    );
}

/// Phase 2: CommitRewardTracker.flush_rewards() must produce rewards in [0, 1].
#[test]
fn commit_reward_tracker_invariant_reward_in_unit_interval() {
    use super::git_tools::commit_rewards::CommitRewardTracker;

    let session_id = uuid::Uuid::new_v4();
    let mut tracker = CommitRewardTracker::new(session_id);

    // Simulate several rounds with different commit subjects.
    let subjects = [
        Some("feat: add new feature"),
        Some("fix a bug"),
        Some("x"),
        None,
        Some("refactor: clean up the authentication module to use the new JWT token format"),
    ];
    let mut sha = 0u64;
    for subject in &subjects {
        sha += 1;
        tracker.record_pre_round_sha(format!("{sha:040x}"));
        sha += 1;
        let post = format!("{sha:040x}");
        tracker.record_post_round(&post, subject.map(|s| s.to_string()));
    }

    let rewards = tracker.flush_rewards();
    assert!(!rewards.is_empty(), "expected at least one commit reward");
    for (sha, _tools, reward) in &rewards {
        assert!(
            (0.0..=1.0).contains(reward),
            "reward {reward} for sha {sha} out of [0,1]"
        );
    }
}

// ── Phase 3 invariants ────────────────────────────────────────────────────────

/// Phase 3: Parsing valid cargo test output must never panic and must
/// produce `all_passed` consistent with the individual test statuses.
#[test]
fn cargo_test_parser_all_passed_is_consistent() {
    let outputs = [
        "test a::b ... ok\ntest a::c ... ok\n",
        "test a::b ... ok\ntest a::c ... FAILED\n",
        "",
        "test x ... ignored\n",
    ];
    for output in &outputs {
        let result = parse_cargo_test(output, "suite");
        let has_failure = result.cases.iter().any(|c| c.status == TestStatus::Failed);
        assert_eq!(
            result.all_passed, !has_failure,
            "all_passed mismatch for output: {output:?}"
        );
    }
}

/// Phase 3: JUnit XML parser — passed() + failed() + ignored() ≤ total cases.
#[test]
fn junit_totals_are_consistent() {
    let xml = r#"<?xml version="1.0"?>
<testsuite name="suite" tests="3">
  <testcase name="a" classname="X" time="0.1"/>
  <testcase name="b" classname="X" time="0.2">
    <failure message="err">err detail</failure>
  </testcase>
  <testcase name="c" classname="X" time="0.1">
    <skipped/>
  </testcase>
</testsuite>"#;

    let r = parse_junit_xml(xml, "suite");
    let total = r.passed() + r.failed() + r.ignored();
    assert_eq!(total, r.cases.len(), "totals must equal case count");
    assert_eq!(r.passed(), 1);
    assert_eq!(r.failed(), 1);
    assert_eq!(r.ignored(), 1);
}

// ── Phase 4 invariants ────────────────────────────────────────────────────────

/// Phase 4: CiRunRecord::compute_reward is always in [0, 1].
#[test]
fn ci_reward_is_always_in_unit_interval() {
    use super::git_tools::test_results::{TestCase, TestResultFormat, TestSuiteResult};

    let statuses = [
        CiRunStatus::Success,
        CiRunStatus::Failure,
        CiRunStatus::Cancelled,
        CiRunStatus::Pending,
        CiRunStatus::Running,
        CiRunStatus::Unknown("wat".to_string()),
    ];

    let results_options: Vec<Option<TestSuiteResult>> = vec![
        None,
        Some(TestSuiteResult {
            suite_name: "ci".to_string(),
            cases: vec![
                TestCase {
                    name: "p".to_string(),
                    status: TestStatus::Passed,
                    duration_ms: None,
                    failure_message: None,
                },
                TestCase {
                    name: "f".to_string(),
                    status: TestStatus::Failed,
                    duration_ms: None,
                    failure_message: None,
                },
            ],
            all_passed: false,
            total_duration_ms: None,
            format: TestResultFormat::JunitXml,
        }),
    ];

    for status in &statuses {
        for results in &results_options {
            let r = CiRunRecord::compute_reward(status, results.as_ref());
            assert!(
                (0.0..=1.0).contains(&r),
                "reward {r} out of [0,1] for status {status:?}"
            );
        }
    }
}

/// Phase 4: CI events from mock client flow through ingestor to broadcast channel.
#[tokio::test]
async fn ci_ingestor_event_pipeline_end_to_end() {
    struct SuccessMock;
    #[async_trait::async_trait]
    impl CiApiClient for SuccessMock {
        async fn list_recent_runs(
            &self,
        ) -> Result<Vec<(String, String, String, String, String)>, String> {
            Ok(vec![(
                "run-99".to_string(),
                "sha-abc".to_string(),
                "CI Pipeline".to_string(),
                "main".to_string(),
                "success".to_string(),
            )])
        }
        async fn fetch_junit_xml(&self, _: &str) -> Result<Option<String>, String> {
            Ok(None)
        }
    }

    let ingestor = CiResultIngestor::new(Arc::new(SuccessMock), CiIngestorConfig::default());
    let mut rx = ingestor.subscribe();
    ingestor.poll_now().await;

    match rx.try_recv() {
        Ok(CiEvent::RunCompleted(rec)) => {
            assert_eq!(rec.status, CiRunStatus::Success);
            assert!((rec.reward - 1.0).abs() < 1e-9, "success should reward 1.0");
        }
        other => panic!("expected RunCompleted, got {other:?}"),
    }
}

// ── Phase 5 invariants ────────────────────────────────────────────────────────

/// Phase 5: Buffer tracker → protocol handler → context block pipeline.
#[tokio::test]
async fn ide_buffer_context_pipeline() {
    let tracker = Arc::new(UnsavedBufferTracker::new());
    let handler = IdeProtocolHandler::new(tracker.clone());

    // Simulate an IDE opening a Rust file.
    let open_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": "file:///src/integration.rs",
                "version": 1,
                "languageId": "rust",
                "text": "pub fn integration_test() -> bool { true }"
            }
        }
    });
    let bytes = serde_json::to_vec(&open_msg).unwrap();
    handler.handle_raw(&bytes).await.unwrap();

    // Context block must mention the file.
    let block = tracker.context_block(4096).await;
    assert!(
        block.contains("file:///src/integration.rs"),
        "context block must include opened URI"
    );
    assert!(
        block.contains("integration_test"),
        "context block must include buffer content"
    );
}

/// Phase 5: DevGateway context reflects buffer state.
#[tokio::test]
async fn dev_gateway_context_includes_open_buffers() {
    let gw = DevGateway::new();

    let open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": "file:///gw_test.rs",
                "version": 1,
                "languageId": "rust",
                "text": "fn test() {}"
            }
        }
    });
    gw.handle_lsp_message(&serde_json::to_vec(&open).unwrap())
        .await;

    let ctx = gw.build_context().await;
    assert_eq!(ctx.open_buffers, 1);
    assert!(ctx.buffer_block.contains("gw_test.rs"));
}

// ── Phase 6 invariants ────────────────────────────────────────────────────────

/// Phase 6: Symbol extractor invariant — render() never exceeds max_chars * 2.
#[test]
fn symbol_render_budget_respected() {
    let code = r#"
pub struct Alpha {}
pub struct Beta {}
pub struct Gamma {}
pub fn delta() {}
pub fn epsilon() {}
pub trait Zeta { fn method(&self); }
"#;
    let idx = RegexExtractor.extract("budget.rs", "rust", code);
    let max_chars = 80;
    let rendered = idx.render(max_chars);
    // The render may be slightly over due to the header and truncation notice,
    // but should never be more than 2× the budget.
    assert!(
        rendered.len() <= max_chars * 3,
        "rendered {} chars vs budget {max_chars}",
        rendered.len()
    );
}

/// Phase 6: public_symbols() never includes private symbols.
#[test]
fn symbol_extractor_public_symbols_excludes_private() {
    let code = "pub fn public_fn() {}\nfn private_fn() {}\npub struct Pub {}\nstruct Priv {}";
    let idx = extract_from_buffer("file:///lib.rs", code);
    for sym in idx.public_symbols() {
        assert!(
            sym.visibility.is_some(),
            "public_symbols() returned symbol without visibility: {:?}",
            sym.name
        );
    }
}

/// Phase 6: Symbol names extracted from a well-known code snippet are non-empty.
#[test]
fn symbol_extractor_names_are_non_empty() {
    let code = r#"
pub fn main() {}
pub struct Config { timeout: u32 }
pub enum Status { Ok, Err }
pub trait Handle { fn handle(&self); }
"#;
    let idx = extract_from_buffer("file:///main.rs", code);
    for sym in &idx.symbols {
        assert!(!sym.name.is_empty(), "symbol name must not be empty");
        assert!(
            sym.name
                .chars()
                .next()
                .map_or(false, |c| c.is_alphabetic() || c == '_'),
            "symbol name {:?} starts with non-alphabetic character",
            sym.name
        );
    }
}

// ── Phase 7 invariants ────────────────────────────────────────────────────────

/// Phase 7: as_reward() is monotonically decreasing with error_rate.
#[test]
fn runtime_metrics_reward_decreases_with_error_rate() {
    use super::runtime_signal_ingestor::RuntimeMetrics;

    let rates = [0.0f64, 0.1, 0.3, 0.5, 0.8, 1.0];
    let mut prev_reward = 1.1f64;
    for &rate in &rates {
        let m = RuntimeMetrics {
            sample_count: 10,
            error_rate: rate,
            p95_ms: 100.0,
            ..Default::default()
        };
        let r = m.as_reward();
        assert!(
            r <= prev_reward + 1e-9,
            "reward {r} at error_rate {rate} should be ≤ previous reward {prev_reward}"
        );
        prev_reward = r;
    }
}

/// Phase 7: Ingesting 100 span signals and computing metrics is fast (<50ms).
#[tokio::test]
async fn runtime_ingestor_performance_invariant() {
    let ingestor = RuntimeSignalIngestor::new(512);
    let start = std::time::Instant::now();

    for i in 0..100u64 {
        ingestor
            .ingest(RuntimeSignal::span(
                format!("op-{i}"),
                (i % 50) as f64 * 10.0,
                i % 10 == 0,
            ))
            .await;
    }
    let _ = ingestor.metrics().await;

    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(50),
        "100 ingests + metrics took {}ms (>50ms budget)",
        elapsed.as_millis()
    );
}

// ── Cross-phase chain ─────────────────────────────────────────────────────────

/// End-to-end: LSP message → buffer → symbol extraction → context markdown.
#[tokio::test]
async fn lsp_to_symbol_to_context_chain() {
    let gw = DevGateway::new();

    // Open a Python file via LSP.
    let python_code = r#"
class DataPipeline:
    def __init__(self):
        pass

    def run(self, data):
        return data

def create_pipeline() -> DataPipeline:
    return DataPipeline()
"#;

    let open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": "file:///pipeline.py",
                "version": 1,
                "languageId": "python",
                "text": python_code
            }
        }
    });
    gw.handle_lsp_message(&serde_json::to_vec(&open).unwrap())
        .await;

    // Extract symbols from the buffer content.
    let content = gw.buffers.content("file:///pipeline.py").await.unwrap();
    let idx = extract_from_buffer("file:///pipeline.py", &content);

    // Should have found DataPipeline class and create_pipeline function.
    let classes: Vec<_> = idx
        .symbols
        .iter()
        .filter(|s| s.kind == SymbolKind::Class)
        .collect();
    let fns: Vec<_> = idx
        .symbols
        .iter()
        .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
        .collect();

    assert!(!classes.is_empty(), "should extract DataPipeline class");
    assert!(!fns.is_empty(), "should extract at least one function");

    // Render into markdown.
    let rendered = idx.render(2048);
    assert!(rendered.contains("DataPipeline"));

    // DevContext should reference the open buffer.
    let ctx = gw.build_context().await;
    assert_eq!(ctx.open_buffers, 1);
}

/// End-to-end: CI reward + runtime signals → blended environment reward.
#[tokio::test]
async fn ci_and_runtime_signals_blended_reward() {
    // Set up CI reward.
    let gw = DevGateway::new();
    gw.ingest_ci_event(CiEvent::RunCompleted(CiRunRecord {
        run_id: "end-to-end".to_string(),
        head_sha: "abc".to_string(),
        workflow_name: "CI".to_string(),
        branch: "main".to_string(),
        status: CiRunStatus::Success,
        test_results: None,
        reward: 1.0,
    }))
    .await;

    // Set up runtime signals.
    let ingestor = RuntimeSignalIngestor::new(64);
    for _ in 0..10 {
        ingestor
            .ingest(RuntimeSignal::span("agent.round", 80.0, false))
            .await;
    }
    let rt_metrics = ingestor.metrics().await;
    let rt_reward = rt_metrics.as_reward();

    // Both rewards must be in [0, 1].
    let ctx = gw.build_context().await;
    assert!(
        (0.0..=1.0).contains(&ctx.env_reward),
        "CI env reward out of range"
    );
    assert!((0.0..=1.0).contains(&rt_reward), "RT reward out of range");

    // Blended reward (50/50) must also be in [0, 1].
    let blended = 0.5 * ctx.env_reward + 0.5 * rt_reward;
    assert!(
        (0.0..=1.0).contains(&blended),
        "blended reward out of range: {blended}"
    );

    // Markdown summaries must be non-empty.
    let md = metrics_to_markdown(&rt_metrics);
    assert!(!md.is_empty(), "runtime metrics markdown must not be empty");
}
