//! Filesystem watcher for instruction file hot-reload.
//!
//! Uses `notify::recommended_watcher` which maps to:
//! - **Linux**: inotify  (events within ~5 ms)
//! - **macOS**: FSEvents (events within ~50-100 ms)
//! - **Windows**: ReadDirectoryChangesW
//!
//! The watcher runs in a background thread managed by the `notify` crate.
//! `has_changed()` atomically reads and clears the change flag — suitable for
//! polling at agent round boundaries without interrupting an active session.
//!
//! No polling interval configuration is needed for `recommended_watcher`; the
//! `poll_interval` parameter in `start_with_interval` is accepted for API
//! compatibility with tests but is ignored (native backends have negligible
//! latency, well inside the 500 ms SLA).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use notify::{recommended_watcher, Event, EventKind, RecursiveMode, Watcher};

/// Background watcher that detects instruction file changes.
pub struct InstructionWatcher {
    /// Owned watcher — kept alive so the background thread keeps running.
    _watcher: Box<dyn Watcher + Send>,
    /// Set to `true` by the watcher callback when any watched file changes.
    changed: Arc<AtomicBool>,
}

impl InstructionWatcher {
    /// Start watching all file paths in `sources`.
    ///
    /// `sources` may include files that do not exist yet; they are silently
    /// ignored until they appear.
    ///
    /// Returns `None` if `sources` is empty or if the watcher could not be
    /// created (e.g. the platform cannot initialise inotify / FSEvents).
    pub fn start(sources: &[PathBuf]) -> Option<Self> {
        Self::start_with_interval(sources, Duration::ZERO)
    }

    /// Start with an (advisory) poll interval.
    ///
    /// For `recommended_watcher` the interval is ignored; native backends
    /// (inotify, FSEvents) deliver events as soon as the kernel signals them.
    /// The parameter exists so callers can pass a custom value for future
    /// portability (e.g. fallback to PollWatcher on unsupported platforms).
    pub fn start_with_interval(sources: &[PathBuf], _poll_interval: Duration) -> Option<Self> {
        if sources.is_empty() {
            return None;
        }

        let changed = Arc::new(AtomicBool::new(false));
        let changed_clone = changed.clone();

        let mut watcher = recommended_watcher(move |result: notify::Result<Event>| {
            if let Ok(event) = result {
                // Only mark changed for events that represent genuine file-content
                // modifications.  Ignoring Create/Access prevents spurious reloads
                // caused by:
                //   - Parallel test threads creating files in the same temp dir.
                //   - inotify queue overflow (IN_Q_OVERFLOW) delivering synthetic events.
                //   - Editor-side metadata writes (chmod, xattr) on macOS.
                let is_content_change = matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Remove(_)
                );
                if is_content_change {
                    changed_clone.store(true, Ordering::Release);
                }
            }
        })
        .ok()?;

        // Watch the parent directory of each source file (non-recursive).
        // We watch the directory rather than the file itself because many editors
        // (Vim, VS Code, some CI tools) write via a rename / atomic-replace, which
        // means the original inode is removed — a file-level watch would miss the
        // event.  Watching the parent captures all modifications to files within it.
        let mut watched_dirs: std::collections::HashSet<PathBuf> = Default::default();
        for src in sources {
            if let Some(parent) = src.parent() {
                let parent = parent.to_path_buf();
                if watched_dirs.insert(parent.clone()) {
                    let _ = watcher.watch(&parent, RecursiveMode::NonRecursive);
                }
            }
        }

        Some(Self {
            _watcher: Box::new(watcher),
            changed,
        })
    }

    /// Returns `true` **and resets the flag** if any watched file changed since
    /// the last call.  Suitable for polling at round boundaries.
    pub fn has_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
}
