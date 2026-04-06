//! File re-read after compaction (Fase 2).
//!
//! When compaction removes tool results that contained file contents (Read tool),
//! the agent loses access to the actual file data. FileReReader tracks which files
//! were read during the session and re-injects their current content into the
//! conversation after compaction, respecting a strict token budget.
//!
//! Design principles:
//!   - Best-effort: disk errors are logged and skipped, never fatal.
//!   - Budget-bounded: total re-read tokens capped at a fraction of pipeline budget.
//!   - Recency-weighted: most recently read files are re-injected first.
//!   - Skip-if-present: files still in the keep window are not re-read.
//!   - Per-file cap: no single file consumes more than its budget share.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use halcon_core::types::{ChatMessage, ContentBlock, MessageContent, Role};

// ── Config defaults ─────────────────────────────────────────────────────────

/// Fraction of pipeline_budget allocated to file re-reads.
const DEFAULT_REREAD_BUDGET_FRACTION: f64 = 0.05;

/// Maximum tokens per individual file re-read.
const DEFAULT_PER_FILE_CAP_TOKENS: usize = 2000;

/// Maximum number of files to re-read per compaction event.
const DEFAULT_MAX_FILES: usize = 5;

// ── FileReadRecord ──────────────────────────────────────────────────────────

/// Record of a file read during the session.
#[derive(Debug, Clone)]
struct FileReadRecord {
    /// Absolute path to the file.
    path: PathBuf,
    /// Last time this file was read by a tool.
    last_read: Instant,
    /// Number of times this file was read.
    read_count: u32,
}

// ── FileReReader ────────────────────────────────────────────────────────────

/// Tracks file reads and re-injects content after compaction.
pub struct FileReReader {
    /// Files read during this session, keyed by canonical path string.
    reads: HashMap<String, FileReadRecord>,
    /// Token budget for re-reads (fraction of pipeline budget).
    budget_fraction: f64,
    /// Maximum tokens per file.
    per_file_cap: usize,
    /// Maximum files to re-read.
    max_files: usize,
}

/// Result of a re-read operation.
#[derive(Debug)]
pub struct ReReadResult {
    /// Number of files re-read.
    pub files_reread: u32,
    /// Total tokens injected.
    pub tokens_injected: usize,
    /// Files that were skipped (already in keep window, budget exceeded, etc.).
    pub files_skipped: u32,
}

impl FileReReader {
    pub fn new() -> Self {
        Self {
            reads: HashMap::new(),
            budget_fraction: DEFAULT_REREAD_BUDGET_FRACTION,
            per_file_cap: DEFAULT_PER_FILE_CAP_TOKENS,
            max_files: DEFAULT_MAX_FILES,
        }
    }

    /// Create with explicit config overrides.
    pub fn with_config(budget_fraction: f64, per_file_cap: usize, max_files: usize) -> Self {
        Self {
            reads: HashMap::new(),
            budget_fraction: budget_fraction.clamp(0.0, 0.20),
            per_file_cap,
            max_files,
        }
    }

    /// Record that a file was read by a tool.
    ///
    /// Called from the tool execution path when a Read/file_read tool succeeds.
    pub fn record_read(&mut self, path: &str) {
        let canonical = path.to_string();
        let entry = self.reads.entry(canonical).or_insert(FileReadRecord {
            path: PathBuf::from(path),
            last_read: Instant::now(),
            read_count: 0,
        });
        entry.last_read = Instant::now();
        entry.read_count += 1;
    }

    /// Record multiple file reads from tool execution results.
    ///
    /// Scans tool_use blocks for Read/file_read tool names and extracts file paths.
    pub fn record_reads_from_tools(
        &mut self,
        tool_names: &[String],
        tool_inputs: &[serde_json::Value],
    ) {
        for (name, input) in tool_names.iter().zip(tool_inputs.iter()) {
            if name == "Read" || name == "file_read" || name == "read" {
                if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                    self.record_read(path);
                }
            }
        }
    }

    /// Re-inject recently read files after compaction.
    ///
    /// Appends a single User message with file contents to `messages`.
    /// Only called when compaction actually occurred (compact_count incremented).
    ///
    /// `pipeline_budget`: total token budget for the context pipeline.
    /// `messages`: the post-compaction message list.
    ///
    /// Returns metrics about the re-read operation.
    pub fn reread_after_compaction(
        &self,
        messages: &mut Vec<ChatMessage>,
        pipeline_budget: usize,
    ) -> ReReadResult {
        if self.reads.is_empty() {
            return ReReadResult {
                files_reread: 0,
                tokens_injected: 0,
                files_skipped: 0,
            };
        }

        let total_budget = (pipeline_budget as f64 * self.budget_fraction) as usize;
        if total_budget == 0 {
            return ReReadResult {
                files_reread: 0,
                tokens_injected: 0,
                files_skipped: self.reads.len() as u32,
            };
        }

        // Sort by recency (most recent first)
        let mut candidates: Vec<&FileReadRecord> = self.reads.values().collect();
        candidates.sort_by(|a, b| b.last_read.cmp(&a.last_read));

        // Check which files are already mentioned in the keep window
        let existing_paths = extract_file_paths_from_messages(messages);

        let mut injected_parts: Vec<String> = Vec::new();
        let mut tokens_used: usize = 0;
        let mut files_reread: u32 = 0;
        let mut files_skipped: u32 = 0;

        for record in candidates.iter().take(self.max_files + 5) {
            // Budget check
            if tokens_used >= total_budget || files_reread >= self.max_files as u32 {
                files_skipped += 1;
                continue;
            }

            // Skip if file is already mentioned in keep window
            let path_str = record.path.to_string_lossy();
            if existing_paths.iter().any(|p| p == path_str.as_ref()) {
                files_skipped += 1;
                continue;
            }

            // Try to read the file
            match std::fs::read_to_string(&record.path) {
                Ok(content) => {
                    let file_tokens = content.len() / 4;
                    let remaining_budget = total_budget.saturating_sub(tokens_used);
                    let capped = file_tokens.min(self.per_file_cap).min(remaining_budget);
                    let char_limit = capped * 4;

                    if capped == 0 {
                        files_skipped += 1;
                        continue;
                    }

                    let truncated = if content.len() > char_limit {
                        format!(
                            "{}...[truncated to ~{capped} tokens]",
                            &content[..char_limit]
                        )
                    } else {
                        content
                    };

                    injected_parts.push(format!("[Re-read: {}]\n{}", path_str, truncated));
                    tokens_used += capped;
                    files_reread += 1;
                }
                Err(e) => {
                    tracing::debug!(
                        path = %path_str,
                        error = %e,
                        "file_reread_skipped"
                    );
                    files_skipped += 1;
                }
            }
        }

        if !injected_parts.is_empty() {
            let combined = format!(
                "[File context restored after compaction]\n\n{}",
                injected_parts.join("\n\n")
            );
            messages.push(ChatMessage {
                role: Role::User,
                content: MessageContent::Text(combined),
            });
            tracing::info!(
                files_reread,
                tokens_injected = tokens_used,
                files_skipped,
                "file_reread_post_compaction"
            );
        }

        ReReadResult {
            files_reread,
            tokens_injected: tokens_used,
            files_skipped,
        }
    }

    /// Number of unique files tracked.
    pub fn tracked_count(&self) -> usize {
        self.reads.len()
    }
}

/// Extract file paths mentioned in tool results (heuristic).
fn extract_file_paths_from_messages(messages: &[ChatMessage]) -> Vec<String> {
    let mut paths = Vec::new();
    for msg in messages {
        if let MessageContent::Blocks(blocks) = &msg.content {
            for block in blocks {
                if let ContentBlock::ToolUse { name, input, .. } = block {
                    if name == "Read"
                        || name == "file_read"
                        || name == "read"
                        || name == "Edit"
                        || name == "Write"
                    {
                        if let Some(p) = input.get("file_path").and_then(|v| v.as_str()) {
                            paths.push(p.to_string());
                        }
                    }
                }
            }
        }
    }
    paths
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn text_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
        }
    }

    #[test]
    fn record_and_count() {
        let mut rr = FileReReader::new();
        rr.record_read("/tmp/foo.rs");
        rr.record_read("/tmp/bar.rs");
        rr.record_read("/tmp/foo.rs"); // duplicate
        assert_eq!(rr.tracked_count(), 2);
    }

    #[test]
    fn reread_injects_file_content() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.rs");
        std::fs::write(&file_path, "fn main() {}").unwrap();

        let mut rr = FileReReader::with_config(0.10, 5000, 5);
        rr.record_read(file_path.to_str().unwrap());

        let mut msgs = vec![text_msg("compacted summary")];
        let result = rr.reread_after_compaction(&mut msgs, 100_000);

        assert_eq!(result.files_reread, 1);
        assert!(result.tokens_injected > 0);
        assert_eq!(msgs.len(), 2); // original + re-read
        let injected = msgs[1].content.as_text().unwrap();
        assert!(injected.contains("fn main()"));
        assert!(injected.contains("[Re-read:"));
    }

    #[test]
    fn reread_respects_budget() {
        let tmp = TempDir::new().unwrap();
        let f1 = tmp.path().join("big.rs");
        std::fs::write(&f1, "x".repeat(40_000)).unwrap(); // ~10K tokens

        let mut rr = FileReReader::with_config(0.01, 5000, 5); // 1% of 100K = 1000 tokens
        rr.record_read(f1.to_str().unwrap());

        let mut msgs = vec![text_msg("summary")];
        let result = rr.reread_after_compaction(&mut msgs, 100_000);

        // Should inject but truncated to budget
        assert_eq!(result.files_reread, 1);
        assert!(result.tokens_injected <= 1000);
    }

    #[test]
    fn reread_skips_missing_files() {
        let mut rr = FileReReader::new();
        rr.record_read("/nonexistent/path/foo.rs");

        let mut msgs = vec![text_msg("summary")];
        let result = rr.reread_after_compaction(&mut msgs, 100_000);

        assert_eq!(result.files_reread, 0);
        assert_eq!(result.files_skipped, 1);
        assert_eq!(msgs.len(), 1); // no injection
    }

    #[test]
    fn empty_reader_is_noop() {
        let rr = FileReReader::new();
        let mut msgs = vec![text_msg("summary")];
        let result = rr.reread_after_compaction(&mut msgs, 100_000);
        assert_eq!(result.files_reread, 0);
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn reread_max_files_limit() {
        let tmp = TempDir::new().unwrap();
        let mut rr = FileReReader::with_config(0.10, 5000, 2); // max 2 files

        for i in 0..5 {
            let f = tmp.path().join(format!("file{i}.rs"));
            std::fs::write(&f, format!("content {i}")).unwrap();
            rr.record_read(f.to_str().unwrap());
        }

        let mut msgs = vec![text_msg("summary")];
        let result = rr.reread_after_compaction(&mut msgs, 100_000);
        assert_eq!(result.files_reread, 2);
    }
}
