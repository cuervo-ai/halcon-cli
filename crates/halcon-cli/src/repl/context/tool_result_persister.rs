//! Tool result persistence to disk (Fase 2).
//!
//! Persists large tool results to a session-scoped cache directory before they
//! are truncated or evicted. The agent can recover the full output via a
//! `RecoveryHandle` injected into the truncated message.
//!
//! Design:
//!   - Runs BEFORE truncation: captures the full result, then truncation proceeds normally.
//!   - Recovery handle is a stable identifier the agent can reference if it needs the full output.
//!   - Fallback to truncation-only if disk write fails (no catastrophic failure).
//!   - Session-scoped directory cleaned up on session end.
//!
//! Integration point: called from simplified_loop BEFORE tool_result_truncator.

use std::collections::HashMap;
use std::io;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use halcon_core::types::{ChatMessage, ContentBlock, MessageContent};

// ── Config ──────────────────────────────────────────────────────────────────

/// Minimum token estimate for a tool result to be persisted.
/// Results below this threshold are kept inline (no disk write).
const DEFAULT_PERSIST_THRESHOLD_TOKENS: usize = 4000;

/// Maximum total bytes on disk per session.
const DEFAULT_MAX_CACHE_BYTES: u64 = 50 * 1024 * 1024; // 50 MB

// ── RecoveryHandle ──────────────────────────────────────────────────────────

/// Stable identifier for a persisted tool result.
///
/// Injected into the truncated message so the agent (or file-reread logic)
/// can recover the full output without re-executing the tool.
#[derive(Debug, Clone)]
pub struct RecoveryHandle {
    /// tool_use_id from the original ContentBlock::ToolResult.
    pub tool_use_id: String,
    /// Disk path where the full result is stored.
    pub path: PathBuf,
    /// Original token estimate of the full result.
    pub original_tokens: usize,
    /// Unix timestamp (seconds) when persisted.
    pub persisted_at: u64,
}

// ── ToolResultPersister ─────────────────────────────────────────────────────

/// Persists large tool results to disk before truncation.
///
/// Owns the session-scoped cache directory and the index of recovery handles.
/// All operations are synchronous (tool results are in-memory strings; disk I/O
/// is fast for the sizes involved).
pub struct ToolResultPersister {
    /// Session-scoped cache directory (e.g., $DATA_DIR/halcon/tool_cache/$session_id/).
    cache_dir: PathBuf,
    /// Index: tool_use_id → RecoveryHandle.
    handles: HashMap<String, RecoveryHandle>,
    /// Cumulative bytes written this session.
    total_bytes: u64,
    /// Maximum total bytes allowed.
    max_bytes: u64,
    /// Minimum token estimate to trigger persistence.
    persist_threshold: usize,
    /// Whether the cache directory was successfully created.
    dir_ready: bool,
}

impl ToolResultPersister {
    /// Create a new persister for the given session.
    ///
    /// `session_id` scopes the cache directory. The directory is created lazily
    /// on first persist to avoid unnecessary I/O for short sessions.
    pub fn new(session_id: &str) -> Self {
        let cache_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("halcon")
            .join("tool_cache")
            .join(session_id);
        Self {
            cache_dir,
            handles: HashMap::new(),
            total_bytes: 0,
            max_bytes: DEFAULT_MAX_CACHE_BYTES,
            persist_threshold: DEFAULT_PERSIST_THRESHOLD_TOKENS,
            dir_ready: false,
        }
    }

    /// Create with explicit config overrides (for testing or config-driven sessions).
    pub fn with_config(cache_dir: PathBuf, persist_threshold: usize, max_bytes: u64) -> Self {
        Self {
            cache_dir,
            handles: HashMap::new(),
            total_bytes: 0,
            max_bytes,
            persist_threshold,
            dir_ready: false,
        }
    }

    /// Scan messages and persist large tool results to disk.
    ///
    /// Returns the number of results persisted. Skips the last `skip_recent`
    /// messages (current turn). Results already persisted (by tool_use_id) are
    /// not re-written.
    ///
    /// This should be called BEFORE `tool_result_truncator::truncate_large_tool_results`.
    pub fn persist_large_results(&mut self, messages: &[ChatMessage], skip_recent: usize) -> u32 {
        if messages.len() <= skip_recent {
            return 0;
        }

        let mut persisted = 0u32;
        let scan_end = messages.len() - skip_recent;

        for msg in &messages[..scan_end] {
            let MessageContent::Blocks(blocks) = &msg.content else {
                continue;
            };
            for block in blocks {
                let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                else {
                    continue;
                };

                // Skip if already persisted
                if self.handles.contains_key(tool_use_id) {
                    continue;
                }

                // Check token estimate
                let est_tokens = content.len() / 4;
                if est_tokens < self.persist_threshold {
                    continue;
                }

                // Check budget
                let content_bytes = content.len() as u64;
                if self.total_bytes + content_bytes > self.max_bytes {
                    tracing::debug!(
                        tool_use_id,
                        content_bytes,
                        total_bytes = self.total_bytes,
                        max_bytes = self.max_bytes,
                        "tool_result_persister: cache budget exhausted, skipping"
                    );
                    continue;
                }

                // Write to disk
                match self.write_to_disk(tool_use_id, content) {
                    Ok(path) => {
                        let handle = RecoveryHandle {
                            tool_use_id: tool_use_id.clone(),
                            path,
                            original_tokens: est_tokens,
                            persisted_at: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs(),
                        };
                        self.total_bytes += content_bytes;
                        self.handles.insert(tool_use_id.clone(), handle);
                        persisted += 1;
                        tracing::debug!(tool_use_id, est_tokens, "tool_result_persisted");
                    }
                    Err(e) => {
                        // Non-fatal: truncation will handle it
                        tracing::warn!(
                            tool_use_id,
                            error = %e,
                            "tool_result_persist_failed"
                        );
                    }
                }
            }
        }

        persisted
    }

    /// Recover a full tool result from disk.
    ///
    /// Returns `None` if the handle doesn't exist or the file is missing/corrupt.
    pub fn recover(&self, tool_use_id: &str) -> Option<String> {
        let handle = self.handles.get(tool_use_id)?;
        match std::fs::read_to_string(&handle.path) {
            Ok(content) => Some(content),
            Err(e) => {
                tracing::warn!(
                    tool_use_id,
                    path = %handle.path.display(),
                    error = %e,
                    "tool_result_recovery_failed"
                );
                None
            }
        }
    }

    /// Get a recovery handle by tool_use_id.
    pub fn get_handle(&self, tool_use_id: &str) -> Option<&RecoveryHandle> {
        self.handles.get(tool_use_id)
    }

    /// All currently tracked recovery handles.
    pub fn handles(&self) -> &HashMap<String, RecoveryHandle> {
        &self.handles
    }

    /// Number of results currently persisted.
    pub fn count(&self) -> usize {
        self.handles.len()
    }

    /// Total bytes written to disk this session.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Clean up the session cache directory.
    ///
    /// Called on session end. Best-effort: logs but does not fail on errors.
    pub fn cleanup(&self) {
        if self.dir_ready {
            if let Err(e) = std::fs::remove_dir_all(&self.cache_dir) {
                tracing::debug!(
                    path = %self.cache_dir.display(),
                    error = %e,
                    "tool_result_cache_cleanup_failed"
                );
            }
        }
    }

    // ── Private helpers ─────────────────────────────────────────────────

    fn ensure_dir(&mut self) -> io::Result<()> {
        if !self.dir_ready {
            std::fs::create_dir_all(&self.cache_dir)?;
            self.dir_ready = true;
        }
        Ok(())
    }

    fn write_to_disk(&mut self, tool_use_id: &str, content: &str) -> io::Result<PathBuf> {
        self.ensure_dir()?;
        // Sanitize tool_use_id for filename (replace non-alphanumeric with _)
        let safe_id: String = tool_use_id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = self.cache_dir.join(format!("{safe_id}.txt"));
        std::fs::write(&path, content)?;
        Ok(path)
    }
}

impl Drop for ToolResultPersister {
    fn drop(&mut self) {
        // Cleanup on drop (best-effort)
        self.cleanup();
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::{ChatMessage, ContentBlock, MessageContent, Role};
    use tempfile::TempDir;

    fn make_persister(dir: &Path) -> ToolResultPersister {
        ToolResultPersister::with_config(dir.to_path_buf(), 100, 10 * 1024 * 1024)
    }

    fn tool_result_msg(id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error: false,
            }]),
        }
    }

    fn text_msg(role: Role, text: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: MessageContent::Text(text.to_string()),
        }
    }

    #[test]
    fn persist_large_result() {
        let tmp = TempDir::new().unwrap();
        let mut p = make_persister(tmp.path());
        let large = "x".repeat(2000); // 500 tokens > threshold of 100
        let msgs = vec![
            tool_result_msg("t1", &large),
            text_msg(Role::User, "recent"),
            text_msg(Role::Assistant, "response"),
        ];
        let count = p.persist_large_results(&msgs, 2);
        assert_eq!(count, 1);
        assert_eq!(p.count(), 1);
        assert!(p.get_handle("t1").is_some());
    }

    #[test]
    fn skip_small_results() {
        let tmp = TempDir::new().unwrap();
        let mut p = make_persister(tmp.path());
        let small = "x".repeat(100); // 25 tokens < threshold
        let msgs = vec![tool_result_msg("t1", &small)];
        let count = p.persist_large_results(&msgs, 0);
        assert_eq!(count, 0);
    }

    #[test]
    fn recover_persisted_result() {
        let tmp = TempDir::new().unwrap();
        let mut p = make_persister(tmp.path());
        let content = "x".repeat(2000);
        let msgs = vec![tool_result_msg("t1", &content)];
        p.persist_large_results(&msgs, 0);

        let recovered = p.recover("t1");
        assert_eq!(recovered.as_deref(), Some(content.as_str()));
    }

    #[test]
    fn recover_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let p = make_persister(tmp.path());
        assert!(p.recover("nonexistent").is_none());
    }

    #[test]
    fn skip_already_persisted() {
        let tmp = TempDir::new().unwrap();
        let mut p = make_persister(tmp.path());
        let large = "x".repeat(2000);
        let msgs = vec![tool_result_msg("t1", &large)];
        p.persist_large_results(&msgs, 0);
        // Second call should skip
        let count = p.persist_large_results(&msgs, 0);
        assert_eq!(count, 0);
        assert_eq!(p.count(), 1);
    }

    #[test]
    fn respects_max_bytes_budget() {
        let tmp = TempDir::new().unwrap();
        let mut p = ToolResultPersister::with_config(tmp.path().to_path_buf(), 100, 1000);
        let large = "x".repeat(800);
        let msgs = vec![tool_result_msg("t1", &large), tool_result_msg("t2", &large)];
        let count = p.persist_large_results(&msgs, 0);
        assert_eq!(count, 1); // Only first fits in 1000-byte budget
    }

    #[test]
    fn skip_recent_messages() {
        let tmp = TempDir::new().unwrap();
        let mut p = make_persister(tmp.path());
        let large = "x".repeat(2000);
        let msgs = vec![
            tool_result_msg("t1", &large),
            tool_result_msg("t2", &large), // skip_recent=1 means this is skipped
        ];
        let count = p.persist_large_results(&msgs, 1);
        assert_eq!(count, 1);
        assert!(p.get_handle("t1").is_some());
        assert!(p.get_handle("t2").is_none());
    }

    #[test]
    fn cleanup_removes_directory() {
        let tmp = TempDir::new().unwrap();
        let cache_dir = tmp.path().join("test_session");
        let mut p = ToolResultPersister::with_config(cache_dir.clone(), 100, 10_000_000);
        let large = "x".repeat(2000);
        let msgs = vec![tool_result_msg("t1", &large)];
        p.persist_large_results(&msgs, 0);
        assert!(cache_dir.exists());
        p.cleanup();
        assert!(!cache_dir.exists());
    }
}
