//! Speculative read-only tool execution.
//!
//! While the model streams its response, this module predicts likely read-only
//! tool calls from conversation context and pre-executes them in background
//! tokio tasks. When the model's actual tool calls arrive, cached results are
//! served instantly for hits, avoiding redundant execution.
//!
//! **Safety**: Only ReadOnly tools are speculated (file_read, grep, glob,
//! directory_tree). No destructive operations are ever pre-executed.
//!
//! Wired into the agent loop: speculate() before model invocation,
//! get_cached() during tool execution, clear() between rounds.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::Mutex;

use halcon_core::types::{
    ChatMessage, ContentBlock, MessageContent, PermissionLevel, ToolInput, ToolOutput,
};
use halcon_tools::ToolRegistry;

/// Maximum number of speculative tool calls per round.
const MAX_SPECULATIONS: usize = 4;

/// Maximum time to wait for speculative results before giving up.
const SPECULATION_TIMEOUT: Duration = Duration::from_secs(5);

/// A predicted tool call with confidence score.
#[derive(Debug, Clone)]
pub struct PredictedToolCall {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub confidence: f64,
}

/// Cache key for speculative results.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SpeculationKey {
    pub tool_name: String,
    pub args_hash: String,
}

/// Result of a speculative execution.
#[derive(Debug, Clone)]
pub struct SpeculativeResult {
    pub output: ToolOutput,
    pub duration_ms: u64,
    #[allow(dead_code)] // Used for cache expiry in future
    pub predicted_at: Instant,
}

/// Real-time speculation metrics.
#[derive(Debug)]
pub struct SpeculationMetrics {
    /// Total number of tool calls checked against speculation cache.
    pub total_checks: AtomicU64,
    /// Number of cache hits (speculative result served).
    pub hits: AtomicU64,
    /// Number of cache misses (tool executed normally).
    pub misses: AtomicU64,
    /// Total latency saved (ms) from serving cached results.
    pub latency_saved_ms: AtomicU64,
}

impl Default for SpeculationMetrics {
    fn default() -> Self {
        Self {
            total_checks: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            latency_saved_ms: AtomicU64::new(0),
        }
    }
}

impl SpeculationMetrics {
    /// Snapshot current metrics for reporting.
    pub fn snapshot(&self) -> SpeculationMetricsSnapshot {
        let total = self.total_checks.load(Ordering::Relaxed);
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let latency_saved = self.latency_saved_ms.load(Ordering::Relaxed);

        let hit_rate = if total > 0 {
            hits as f64 / total as f64
        } else {
            0.0
        };

        SpeculationMetricsSnapshot {
            total_checks: total,
            hits,
            misses,
            hit_rate,
            latency_saved_ms: latency_saved,
        }
    }

    /// Reset all metrics to zero.
    pub fn reset(&self) {
        self.total_checks.store(0, Ordering::Relaxed);
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.latency_saved_ms.store(0, Ordering::Relaxed);
    }

    /// Record a cache hit with latency saved.
    pub fn record_hit(&self, cached_duration_ms: u64, _estimated_exec_ms: u64) {
        self.total_checks.fetch_add(1, Ordering::Relaxed);
        self.hits.fetch_add(1, Ordering::Relaxed);
        // Latency saved: we serve cached result instantly vs re-executing
        // For now, use cached_duration_ms as the baseline (what speculation took)
        self.latency_saved_ms.fetch_add(cached_duration_ms, Ordering::Relaxed);
    }

    /// Record a cache miss.
    pub fn record_miss(&self) {
        self.total_checks.fetch_add(1, Ordering::Relaxed);
        self.misses.fetch_add(1, Ordering::Relaxed);
    }
}

/// Copyable snapshot of speculation metrics.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpeculationMetricsSnapshot {
    pub total_checks: u64,
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub latency_saved_ms: u64,
}

/// Manages speculative tool pre-execution.
#[allow(dead_code)] // with_max_speculations, cache_size, stats used in tests
pub struct ToolSpeculator {
    cache: Arc<Mutex<HashMap<SpeculationKey, SpeculativeResult>>>,
    max_speculations: usize,
    metrics: Arc<SpeculationMetrics>,
}

impl ToolSpeculator {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            max_speculations: MAX_SPECULATIONS,
            metrics: Arc::new(SpeculationMetrics::default()),
        }
    }

    #[allow(dead_code)]
    pub fn with_max_speculations(mut self, max: usize) -> Self {
        self.max_speculations = max;
        self
    }

    /// Get current speculation metrics snapshot.
    pub fn metrics(&self) -> SpeculationMetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Reset speculation metrics to zero.
    pub fn reset_metrics(&self) {
        self.metrics.reset();
    }

    /// Predict and speculatively execute read-only tools based on conversation context.
    ///
    /// This should be called at the start of model invocation (before streaming begins).
    /// Results are cached for lookup during tool execution.
    pub async fn speculate(
        &self,
        messages: &[ChatMessage],
        tool_registry: &ToolRegistry,
        working_dir: &str,
    ) -> usize {
        let predictions = predict_tool_calls(messages);

        // Filter to read-only tools and limit count.
        let safe_predictions: Vec<PredictedToolCall> = predictions
            .into_iter()
            .filter(|p| {
                tool_registry
                    .get(&p.tool_name)
                    .map(|t| t.permission_level() == PermissionLevel::ReadOnly)
                    .unwrap_or(false)
            })
            .take(self.max_speculations)
            .collect();

        if safe_predictions.is_empty() {
            return 0;
        }

        let count = safe_predictions.len();
        let cache = Arc::clone(&self.cache);

        // Spawn background tasks for each prediction.
        for pred in safe_predictions {
            let cache = Arc::clone(&cache);
            let tool = match tool_registry.get(&pred.tool_name) {
                Some(t) => Arc::clone(t),
                None => continue,
            };
            let wd = working_dir.to_string();

            tokio::spawn(async move {
                let start = Instant::now();
                let input = ToolInput {
                    tool_use_id: format!("spec-{}", pred.tool_name),
                    arguments: pred.arguments.clone(),
                    working_directory: wd,
                };

                let result = tokio::time::timeout(SPECULATION_TIMEOUT, tool.execute(input)).await;

                match result {
                    Ok(Ok(output)) => {
                        let key = make_key(&pred.tool_name, &pred.arguments);
                        let spec_result = SpeculativeResult {
                            output,
                            duration_ms: start.elapsed().as_millis() as u64,
                            predicted_at: start,
                        };
                        cache.lock().await.insert(key, spec_result);
                    }
                    Ok(Err(_)) | Err(_) => {
                        // Tool error or timeout — discard silently.
                    }
                }
            });
        }

        count
    }

    /// Check if a tool call result is cached from speculation.
    ///
    /// Returns the cached result if available, or None if the tool call
    /// was not predicted or hasn't completed yet.
    ///
    /// Automatically records hit/miss metrics for telemetry.
    pub async fn get_cached(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Option<SpeculativeResult> {
        let key = make_key(tool_name, arguments);
        let cache = self.cache.lock().await;

        match cache.get(&key).cloned() {
            Some(result) => {
                // Cache hit: record metrics with latency saved.
                self.metrics.record_hit(result.duration_ms, result.duration_ms);
                Some(result)
            }
            None => {
                // Cache miss: tool will be executed normally.
                self.metrics.record_miss();
                None
            }
        }
    }

    /// Clear all cached speculations (called between rounds).
    pub async fn clear(&self) {
        self.cache.lock().await.clear();
    }

    /// Get the number of cached results.
    #[allow(dead_code)]
    pub async fn cache_size(&self) -> usize {
        self.cache.lock().await.len()
    }

    /// Get statistics about speculation cache hits/misses.
    #[allow(dead_code)]
    pub async fn stats(&self) -> SpeculationStats {
        let cache = self.cache.lock().await;
        let entries: Vec<&SpeculativeResult> = cache.values().collect();
        let total_latency: u64 = entries.iter().map(|e| e.duration_ms).sum();
        let avg_latency = if entries.is_empty() {
            0
        } else {
            total_latency / entries.len() as u64
        };
        SpeculationStats {
            cached_results: entries.len(),
            avg_latency_ms: avg_latency,
        }
    }
}

/// Statistics about speculation performance.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct SpeculationStats {
    pub cached_results: usize,
    pub avg_latency_ms: u64,
}

/// Predict likely tool calls from conversation context.
///
/// Heuristics:
/// 1. Extract file paths mentioned in recent messages → file_read
/// 2. Extract search terms from user queries → grep
/// 3. Track edited files → file_read (model often reads after editing)
pub fn predict_tool_calls(messages: &[ChatMessage]) -> Vec<PredictedToolCall> {
    let mut predictions = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut already_read: HashSet<String> = HashSet::new();

    // Analyze recent messages (last 10 for performance).
    let recent = if messages.len() > 10 {
        &messages[messages.len() - 10..]
    } else {
        messages
    };

    // First pass: track files already read.
    for msg in recent {
        for block in extract_blocks(msg) {
            if let ContentBlock::ToolUse { name, input, .. } = block {
                if name == "file_read" {
                    if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                        already_read.insert(path.to_string());
                    }
                }
            }
        }
    }

    // Second pass: extract predictions.
    for msg in recent {
        let text = extract_text(msg);

        // Extract file paths from text content.
        for path in extract_file_paths(&text) {
            if !already_read.contains(&path) && seen_paths.insert(path.clone()) {
                predictions.push(PredictedToolCall {
                    tool_name: "file_read".to_string(),
                    arguments: json!({ "path": path }),
                    confidence: 0.6,
                });
            }
        }

        // Track file_edit/file_write → predict reading the same file.
        for block in extract_blocks(msg) {
            if let ContentBlock::ToolUse { name, input, .. } = block {
                if name == "file_edit" || name == "file_write" {
                    if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                        if !already_read.contains(path) && seen_paths.insert(path.to_string()) {
                            predictions.push(PredictedToolCall {
                                tool_name: "file_read".to_string(),
                                arguments: json!({ "path": path }),
                                confidence: 0.8, // High confidence: read after edit.
                            });
                        }
                    }
                }
            }
        }
    }

    // Sort by confidence descending.
    predictions.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
    predictions
}

/// Extract visible text from a ChatMessage.
fn extract_text(msg: &ChatMessage) -> String {
    match &msg.content {
        MessageContent::Text(t) => t.clone(),
        MessageContent::Blocks(blocks) => {
            let mut text = String::new();
            for block in blocks {
                match block {
                    ContentBlock::Text { text: t } => {
                        text.push_str(t);
                        text.push(' ');
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        text.push_str(content);
                        text.push(' ');
                    }
                    _ => {}
                }
            }
            text
        }
    }
}

/// Extract ContentBlocks from a ChatMessage (returns empty for Text messages).
fn extract_blocks(msg: &ChatMessage) -> Vec<&ContentBlock> {
    match &msg.content {
        MessageContent::Blocks(blocks) => blocks.iter().collect(),
        MessageContent::Text(_) => Vec::new(),
    }
}

/// Extract file paths from text using heuristics.
///
/// Looks for patterns like:
/// - Paths with file extensions: `src/main.rs`, `config/settings.toml`
/// - Quoted paths: `"path/to/file"`
/// - Paths starting with common prefixes: `./`, `../`, `crates/`
fn extract_file_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();

    let code_extensions = [
        ".rs", ".py", ".js", ".ts", ".jsx", ".tsx", ".go", ".java", ".c", ".cpp", ".h",
        ".hpp", ".swift", ".kt", ".rb", ".lua", ".zig", ".toml", ".yaml", ".yml", ".json",
        ".md", ".txt", ".sh", ".bash", ".zsh",
    ];

    for word in text.split_whitespace() {
        // Clean up punctuation at the edges.
        let cleaned = word
            .trim_matches(['`', '\'', '"', ',', ';'])
            .trim_end_matches(['.', ':', ')', ']']);

        if cleaned.is_empty() || cleaned.len() < 3 {
            continue;
        }

        // Check if it looks like a file path.
        let has_extension = code_extensions.iter().any(|ext| cleaned.ends_with(ext));
        let has_separator = cleaned.contains('/');
        let starts_with_path = cleaned.starts_with("./")
            || cleaned.starts_with("../")
            || cleaned.starts_with("crates/")
            || cleaned.starts_with("src/")
            || cleaned.starts_with("lib/")
            || cleaned.starts_with("tests/")
            || cleaned.starts_with("benches/")
            || cleaned.starts_with("docs/")
            || cleaned.starts_with("examples/")
            || cleaned.starts_with("scripts/")
            || cleaned.starts_with("bin/")
            || cleaned.starts_with('/');  // absolute paths

        if has_extension && (has_separator || starts_with_path) {
            paths.push(cleaned.to_string());
        }
    }

    paths
}

/// Create a cache key from tool name and arguments.
fn make_key(tool_name: &str, arguments: &serde_json::Value) -> SpeculationKey {
    // Use a deterministic string representation for the key.
    let args_str = arguments.to_string();
    SpeculationKey {
        tool_name: tool_name.to_string(),
        args_hash: args_str,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::Role;

    fn text_msg(role: Role, text: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: MessageContent::Text(text.to_string()),
        }
    }

    fn blocks_msg(blocks: Vec<ContentBlock>) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(blocks),
        }
    }

    // --- Path extraction ---

    #[test]
    fn extract_paths_from_text() {
        let text = "Let me read src/main.rs and check crates/halcon-core/src/lib.rs";
        let paths = extract_file_paths(text);
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&"src/main.rs".to_string()));
        assert!(paths.contains(&"crates/halcon-core/src/lib.rs".to_string()));
    }

    #[test]
    fn extract_paths_with_backticks() {
        let text = "The file `src/config.rs` needs updating";
        let paths = extract_file_paths(text);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "src/config.rs");
    }

    #[test]
    fn extract_paths_skips_non_paths() {
        let text = "The function hello_world is defined somewhere";
        let paths = extract_file_paths(text);
        assert!(paths.is_empty());
    }

    #[test]
    fn extract_paths_with_trailing_punctuation() {
        let text = "Found in src/lib.rs, and also crates/tools/src/bash.rs.";
        let paths = extract_file_paths(text);
        assert_eq!(paths.len(), 2);
    }

    // --- Prediction ---

    #[test]
    fn predict_file_read_from_text() {
        let messages = vec![
            text_msg(Role::User, "Please check crates/halcon-cli/src/main.rs for the entry point"),
        ];
        let predictions = predict_tool_calls(&messages);
        assert!(!predictions.is_empty());
        assert_eq!(predictions[0].tool_name, "file_read");
        assert_eq!(
            predictions[0].arguments["path"],
            "crates/halcon-cli/src/main.rs"
        );
    }

    #[test]
    fn predict_skips_already_read_files() {
        let messages = vec![
            blocks_msg(vec![ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "file_read".to_string(),
                input: json!({ "path": "src/main.rs" }),
            }]),
            text_msg(Role::User, "What about src/main.rs?"),
        ];
        let predictions = predict_tool_calls(&messages);
        // src/main.rs already read — should not predict it.
        assert!(
            predictions.iter().all(|p| p.arguments["path"] != "src/main.rs"),
            "should not predict already-read file"
        );
    }

    #[test]
    fn predict_read_after_edit() {
        let messages = vec![
            blocks_msg(vec![ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "file_edit".to_string(),
                input: json!({
                    "path": "src/lib.rs",
                    "old_string": "old",
                    "new_string": "new"
                }),
            }]),
        ];
        let predictions = predict_tool_calls(&messages);
        assert!(!predictions.is_empty());
        let file_read_pred = predictions
            .iter()
            .find(|p| p.arguments["path"] == "src/lib.rs");
        assert!(file_read_pred.is_some(), "should predict reading edited file");
        assert!(
            file_read_pred.unwrap().confidence >= 0.7,
            "read-after-edit should have high confidence"
        );
    }

    #[test]
    fn predict_deduplicates_paths() {
        let messages = vec![
            text_msg(Role::User, "Check src/main.rs please"),
            text_msg(Role::Assistant, "I'll look at src/main.rs now"),
        ];
        let predictions = predict_tool_calls(&messages);
        let main_count = predictions
            .iter()
            .filter(|p| p.arguments["path"] == "src/main.rs")
            .count();
        assert_eq!(main_count, 1, "should not duplicate predictions");
    }

    #[test]
    fn predict_sorts_by_confidence() {
        let messages = vec![
            text_msg(Role::User, "Check crates/tools/src/lib.rs"),
            blocks_msg(vec![ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "file_edit".to_string(),
                input: json!({ "path": "src/edited.rs", "old_string": "a", "new_string": "b" }),
            }]),
        ];
        let predictions = predict_tool_calls(&messages);
        if predictions.len() >= 2 {
            assert!(
                predictions[0].confidence >= predictions[1].confidence,
                "should be sorted by confidence descending"
            );
        }
    }

    // --- Cache ---

    #[tokio::test]
    async fn cache_operations() {
        let speculator = ToolSpeculator::new();

        assert_eq!(speculator.cache_size().await, 0);

        // Manually insert a result.
        let key = make_key("file_read", &json!({ "path": "test.rs" }));
        let result = SpeculativeResult {
            output: ToolOutput {
                tool_use_id: "spec".to_string(),
                content: "file content".to_string(),
                is_error: false,
                metadata: None,
            },
            duration_ms: 5,
            predicted_at: Instant::now(),
        };
        speculator.cache.lock().await.insert(key, result);

        assert_eq!(speculator.cache_size().await, 1);

        // Lookup.
        let cached = speculator
            .get_cached("file_read", &json!({ "path": "test.rs" }))
            .await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().output.content, "file content");

        // Miss.
        let miss = speculator
            .get_cached("file_read", &json!({ "path": "other.rs" }))
            .await;
        assert!(miss.is_none());

        // Clear.
        speculator.clear().await;
        assert_eq!(speculator.cache_size().await, 0);
    }

    #[tokio::test]
    async fn stats_empty() {
        let speculator = ToolSpeculator::new();
        let stats = speculator.stats().await;
        assert_eq!(stats.cached_results, 0);
        assert_eq!(stats.avg_latency_ms, 0);
    }

    // --- Speculate with real tool registry ---

    #[tokio::test]
    async fn speculate_with_file_paths() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.rs");
        std::fs::write(&file_path, "fn main() {}").unwrap();

        let config = halcon_core::types::ToolsConfig {
            allowed_directories: vec![dir.path().to_path_buf()],
            ..Default::default()
        };
        let registry = halcon_tools::default_registry(&config);

        let messages = vec![text_msg(
            Role::User,
            &format!("Check {}", file_path.display()),
        )];

        let speculator = ToolSpeculator::new();
        let count = speculator
            .speculate(&messages, &registry, dir.path().to_str().unwrap())
            .await;

        // Should have spawned at least one speculation.
        assert!(count > 0, "should predict at least one tool call");

        // Give the background task time to complete.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Check if result is cached.
        let cached = speculator
            .get_cached("file_read", &json!({ "path": file_path.to_str().unwrap() }))
            .await;
        assert!(cached.is_some(), "speculated file_read should be cached");
        assert!(
            cached.unwrap().output.content.contains("fn main()"),
            "cached content should match file"
        );
    }

    #[tokio::test]
    async fn speculate_ignores_destructive_tools() {
        let config = halcon_core::types::ToolsConfig::default();
        let registry = halcon_tools::default_registry(&config);

        // Even if prediction includes a destructive tool, it should be filtered.
        let messages = vec![text_msg(
            Role::User,
            "Run `cargo test` in src/main.rs",
        )];

        let speculator = ToolSpeculator::new();
        let _count = speculator
            .speculate(&messages, &registry, "/tmp")
            .await;

        // Only file_read predictions should pass (not bash).
        // The file doesn't exist so speculation might fail, but no destructive tools were executed.
        let cache = speculator.cache.lock().await;
        for (key, _) in cache.iter() {
            assert_ne!(
                key.tool_name, "bash",
                "should never speculate on bash"
            );
        }
    }

    #[test]
    fn make_key_deterministic() {
        let k1 = make_key("file_read", &json!({ "path": "test.rs" }));
        let k2 = make_key("file_read", &json!({ "path": "test.rs" }));
        assert_eq!(k1, k2);

        let k3 = make_key("file_read", &json!({ "path": "other.rs" }));
        assert_ne!(k1, k3);
    }

    #[test]
    fn max_speculations_respected() {
        let speculator = ToolSpeculator::new().with_max_speculations(2);
        assert_eq!(speculator.max_speculations, 2);
    }
}
