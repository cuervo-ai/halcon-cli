//! File-explorer command handlers: directory listing and file reading.
//!
//! Both functions are self-contained async tasks; they do **not** need a
//! [`HalconClient`] because they operate on the local filesystem via Tokio I/O.

use std::path::PathBuf;
use tokio::sync::mpsc;

use super::{BackendMessage, FileDirEntry, RepaintFn};

/// Load the direct (non-hidden) children of `path`, capped at 500 entries.
///
/// Result: [`BackendMessage::DirectoryLoaded`] on success,
///         [`BackendMessage::FileError`] on failure.
pub async fn load_directory(
    path: PathBuf,
    msg_tx: &mpsc::Sender<BackendMessage>,
    repaint: &RepaintFn,
) {
    match tokio::fs::read_dir(&path).await {
        Ok(mut rd) => {
            let mut entries: Vec<FileDirEntry> = Vec::new();
            let mut count = 0usize;
            while let Ok(Some(entry)) = rd.next_entry().await {
                // Skip hidden entries (dot-prefixed names).
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let is_dir = entry
                    .file_type()
                    .await
                    .map(|t| t.is_dir())
                    .unwrap_or(false);
                entries.push(FileDirEntry {
                    name,
                    path: entry.path(),
                    is_dir,
                });
                count += 1;
                // Cap at 500 entries to keep the UI responsive.
                if count >= 500 {
                    break;
                }
            }
            // Sort: directories first (alphabetical), then files (alphabetical).
            entries.sort_unstable_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            let _ = msg_tx.try_send(BackendMessage::DirectoryLoaded { path, entries });
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "LoadDirectory failed");
            let _ = msg_tx.try_send(BackendMessage::FileError {
                path,
                error: e.to_string(),
            });
        }
    }
    (repaint)();
}

/// Read up to 64 KB of `path` and send it as [`BackendMessage::FileLoaded`].
///
/// Binary files (no valid UTF-8 prefix) are replaced with a placeholder string.
/// Files larger than 64 KB are truncated with an informational suffix.
pub async fn load_file(
    path: PathBuf,
    msg_tx: &mpsc::Sender<BackendMessage>,
    repaint: &RepaintFn,
) {
    const MAX_BYTES: usize = 65_536; // 64 KB display limit
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let truncated = bytes.len() > MAX_BYTES;
            let slice = if truncated { &bytes[..MAX_BYTES] } else { &bytes };
            // Walk back to a valid UTF-8 char boundary.
            let mut end = slice.len();
            while end > 0 && std::str::from_utf8(&slice[..end]).is_err() {
                end -= 1;
            }
            let content = if end == 0 {
                // No valid UTF-8 prefix — binary file.
                format!("[binary file — {} bytes]", bytes.len())
            } else {
                // SAFETY: we verified the prefix with from_utf8 above.
                let text = std::str::from_utf8(&slice[..end]).unwrap();
                if truncated {
                    format!("{text}\n…[first 64 KB shown, {} bytes total]", bytes.len())
                } else {
                    text.to_owned()
                }
            };
            let _ = msg_tx.try_send(BackendMessage::FileLoaded { path, content });
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "LoadFile failed");
            let _ = msg_tx.try_send(BackendMessage::FileError {
                path,
                error: e.to_string(),
            });
        }
    }
    (repaint)();
}
