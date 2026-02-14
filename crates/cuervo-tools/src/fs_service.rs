//! Centralized filesystem service shared across all file-operating tools.
//!
//! Encapsulates path security validation, atomic writes, async I/O,
//! metrics, and optional read caching. All file tools receive an
//! `Arc<FsService>` at construction time instead of duplicating
//! `allowed_dirs` / `blocked_patterns` fields.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

use cuervo_core::error::{CuervoError, Result};

use crate::path_security::{self, CompiledPatterns};

/// Maximum allowed content size for writes (10 MB).
pub const MAX_WRITE_SIZE: usize = 10 * 1024 * 1024;

/// Information about a directory entry returned by [`FsService::read_dir_async`].
#[derive(Debug, Clone)]
pub struct DirEntryInfo {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
}

/// Atomic counters for filesystem operation metrics.
pub struct FsMetrics {
    pub reads: AtomicU64,
    pub writes: AtomicU64,
    pub deletes: AtomicU64,
    pub read_bytes: AtomicU64,
    pub write_bytes: AtomicU64,
    pub errors: AtomicU64,
}

impl FsMetrics {
    fn new() -> Self {
        Self {
            reads: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            deletes: AtomicU64::new(0),
            read_bytes: AtomicU64::new(0),
            write_bytes: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }
}

impl Default for FsMetrics {
    fn default() -> Self {
        Self::new()
    }
}

struct CacheEntry {
    content: String,
    mtime: SystemTime,
    size: u64,
}

/// Optional mtime-validated read cache for small files.
struct FsCache {
    entries: Mutex<HashMap<PathBuf, CacheEntry>>,
    max_entries: usize,
    max_entry_size: usize,
}

impl FsCache {
    fn new(max_entries: usize, max_entry_size: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            max_entries,
            max_entry_size,
        }
    }

    fn get(&self, path: &Path, current_mtime: SystemTime, current_size: u64) -> Option<String> {
        let entries = self.entries.lock().ok()?;
        let entry = entries.get(path)?;
        if entry.mtime == current_mtime && entry.size == current_size {
            Some(entry.content.clone())
        } else {
            None
        }
    }

    fn put(&self, path: PathBuf, content: String, mtime: SystemTime, size: u64) {
        if content.len() > self.max_entry_size {
            return;
        }
        if let Ok(mut entries) = self.entries.lock() {
            // Evict oldest if at capacity (simple: just clear half).
            if entries.len() >= self.max_entries {
                let keys: Vec<PathBuf> = entries.keys().take(self.max_entries / 2).cloned().collect();
                for k in keys {
                    entries.remove(&k);
                }
            }
            entries.insert(path, CacheEntry { content, mtime, size });
        }
    }

    fn invalidate(&self, path: &Path) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(path);
        }
    }
}

/// Centralized filesystem service shared across all file-operating tools.
///
/// Encapsulates security validation, atomic writes, and I/O metrics.
pub struct FsService {
    allowed_dirs: Vec<PathBuf>,
    compiled_patterns: CompiledPatterns,
    metrics: FsMetrics,
    cache: Option<FsCache>,
}

impl FsService {
    /// Create a new `FsService` with the given allowed directories and blocked patterns.
    pub fn new(allowed_dirs: Vec<PathBuf>, blocked_patterns: Vec<String>) -> Self {
        Self {
            compiled_patterns: CompiledPatterns::new(&blocked_patterns),
            allowed_dirs,
            metrics: FsMetrics::new(),
            cache: None,
        }
    }

    /// Create a new `FsService` with read caching enabled.
    pub fn new_with_cache(
        allowed_dirs: Vec<PathBuf>,
        blocked_patterns: Vec<String>,
        max_entries: usize,
        max_entry_size: usize,
    ) -> Self {
        Self {
            compiled_patterns: CompiledPatterns::new(&blocked_patterns),
            allowed_dirs,
            metrics: FsMetrics::new(),
            cache: Some(FsCache::new(max_entries, max_entry_size)),
        }
    }

    /// Expose metrics for observability.
    pub fn metrics(&self) -> &FsMetrics {
        &self.metrics
    }

    // --- Path Security ---

    /// Resolve a path relative to working_dir and validate against security policies.
    ///
    /// Uses pre-compiled blocked patterns for zero per-call compilation.
    #[tracing::instrument(skip(self), fields(path = %path, working_dir = %working_dir))]
    pub fn resolve_path(&self, path: &str, working_dir: &str) -> Result<PathBuf> {
        path_security::resolve_and_validate_compiled(
            path,
            working_dir,
            &self.allowed_dirs,
            &self.compiled_patterns,
        )
    }

    /// Check that a path is not a symlink. Returns error if it is.
    #[tracing::instrument(skip(self), fields(path = %resolved.display()))]
    pub async fn check_not_symlink(&self, resolved: &Path) -> Result<()> {
        match tokio::fs::symlink_metadata(resolved).await {
            Ok(meta) if meta.is_symlink() => Err(CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("refusing to operate on symlink: {}", resolved.display()),
            }),
            _ => Ok(()),
        }
    }

    // --- Read Operations ---

    /// Read file contents to string (async, non-blocking).
    ///
    /// Uses cache if enabled and file mtime matches.
    #[tracing::instrument(skip(self), fields(path = %resolved.display()))]
    pub async fn read_to_string(&self, resolved: &Path) -> Result<String> {
        self.metrics.reads.fetch_add(1, Ordering::Relaxed);

        // Try cache first.
        if let Some(ref cache) = self.cache {
            if let Ok(meta) = tokio::fs::metadata(resolved).await {
                if let Ok(mtime) = meta.modified() {
                    if let Some(cached) = cache.get(resolved, mtime, meta.len()) {
                        self.metrics
                            .read_bytes
                            .fetch_add(cached.len() as u64, Ordering::Relaxed);
                        return Ok(cached);
                    }
                }
            }
        }

        let content = tokio::fs::read_to_string(resolved).await.map_err(|e| {
            self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("failed to read {}: {e}", resolved.display()),
            }
        })?;

        self.metrics
            .read_bytes
            .fetch_add(content.len() as u64, Ordering::Relaxed);

        // Populate cache.
        if let Some(ref cache) = self.cache {
            if let Ok(meta) = tokio::fs::metadata(resolved).await {
                if let Ok(mtime) = meta.modified() {
                    cache.put(
                        resolved.to_path_buf(),
                        content.clone(),
                        mtime,
                        meta.len(),
                    );
                }
            }
        }

        Ok(content)
    }

    /// Read specific line range without loading entire file into memory.
    ///
    /// Returns `(numbered_content, total_lines)`.
    #[tracing::instrument(skip(self), fields(path = %resolved.display(), offset, limit))]
    pub async fn read_lines(
        &self,
        resolved: &Path,
        offset: usize,
        limit: usize,
    ) -> Result<(String, usize)> {
        use tokio::io::{AsyncBufReadExt, BufReader};

        self.metrics.reads.fetch_add(1, Ordering::Relaxed);

        let file = tokio::fs::File::open(resolved).await.map_err(|e| {
            self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("failed to open {}: {e}", resolved.display()),
            }
        })?;

        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        let mut total = 0usize;
        let mut numbered = String::new();
        let mut collected = 0usize;

        while let Some(line) = lines.next_line().await.map_err(|e| {
            CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("failed to read line from {}: {e}", resolved.display()),
            }
        })? {
            if total >= offset && (limit == 0 || collected < limit) {
                if collected > 0 {
                    numbered.push('\n');
                }
                use std::fmt::Write;
                let _ = write!(numbered, "{:>6}\t{}", total + 1, line);
                collected += 1;
                self.metrics
                    .read_bytes
                    .fetch_add(line.len() as u64, Ordering::Relaxed);
            }
            total += 1;
        }

        Ok((numbered, total))
    }

    // --- Write Operations ---

    /// Atomic write: temp file + fsync + rename.
    ///
    /// Returns bytes written. Creates parent directories as needed.
    #[tracing::instrument(skip(self, content), fields(path = %resolved.display(), content_len = content.len()))]
    pub async fn atomic_write(&self, resolved: &Path, content: &[u8]) -> Result<u64> {
        self.metrics.writes.fetch_add(1, Ordering::Relaxed);

        // Create parent directories if needed.
        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                self.metrics.errors.fetch_add(1, Ordering::Relaxed);
                CuervoError::ToolExecutionFailed {
                    tool: "fs_service".into(),
                    message: format!("failed to create directories: {e}"),
                }
            })?;
        }

        let parent_dir = resolved.parent().unwrap_or(Path::new("."));
        let temp_path = parent_dir.join(format!(
            ".cuervo_tmp_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));

        // Write to temp file.
        tokio::fs::write(&temp_path, content).await.map_err(|e| {
            self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("failed to write temp file {}: {e}", temp_path.display()),
            }
        })?;

        // Fsync for durability.
        let temp_for_sync = temp_path.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let f = std::fs::File::open(&temp_for_sync)?;
            f.sync_all()?;
            Ok(())
        })
        .await
        .map_err(|e| CuervoError::ToolExecutionFailed {
            tool: "fs_service".into(),
            message: format!("fsync task failed: {e}"),
        })? {
            let _ = tokio::fs::remove_file(&temp_path).await;
            self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            return Err(CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("failed to fsync: {e}"),
            });
        }

        // Atomic rename.
        if let Err(e) = tokio::fs::rename(&temp_path, resolved).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            return Err(CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("failed to write {}: {e}", resolved.display()),
            });
        }

        let bytes = content.len() as u64;
        self.metrics
            .write_bytes
            .fetch_add(bytes, Ordering::Relaxed);

        // Invalidate cache for this path.
        if let Some(ref cache) = self.cache {
            cache.invalidate(resolved);
        }

        Ok(bytes)
    }

    // --- Delete Operations ---

    /// Delete a file after symlink and directory checks.
    ///
    /// Returns the file size before deletion.
    #[tracing::instrument(skip(self), fields(path = %resolved.display()))]
    pub async fn delete_file(&self, resolved: &Path) -> Result<u64> {
        self.metrics.deletes.fetch_add(1, Ordering::Relaxed);

        // Use symlink_metadata (lstat) — does NOT follow symlinks.
        let metadata = tokio::fs::symlink_metadata(resolved).await.map_err(|e| {
            self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("cannot stat '{}': {e}", resolved.display()),
            }
        })?;

        if metadata.is_symlink() {
            self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            return Err(CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!(
                    "refusing to delete symlink: {}",
                    resolved.display()
                ),
            });
        }

        if metadata.is_dir() {
            self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            return Err(CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!(
                    "'{}' is a directory (recursive delete not supported)",
                    resolved.display()
                ),
            });
        }

        let file_size = metadata.len();

        tokio::fs::remove_file(resolved).await.map_err(|e| {
            self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("failed to delete '{}': {e}", resolved.display()),
            }
        })?;

        // Invalidate cache.
        if let Some(ref cache) = self.cache {
            cache.invalidate(resolved);
        }

        Ok(file_size)
    }

    // --- Directory Operations ---

    /// Async directory listing. Returns entries sorted: directories first, then alphabetical.
    #[tracing::instrument(skip(self), fields(path = %path.display()))]
    pub async fn read_dir_async(&self, path: &Path) -> Result<Vec<DirEntryInfo>> {
        let mut rd = tokio::fs::read_dir(path).await.map_err(|e| {
            self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("failed to read directory {}: {e}", path.display()),
            }
        })?;

        let mut entries = Vec::new();
        while let Some(entry) = rd.next_entry().await.map_err(|e| {
            CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("failed to read dir entry: {e}"),
            }
        })? {
            let ft = entry.file_type().await.unwrap_or_else(|_| {
                // Fallback: treat as file
                std::fs::metadata(entry.path())
                    .map(|m| m.file_type())
                    .unwrap_or_else(|_| std::fs::metadata("/dev/null").unwrap().file_type())
            });
            let name = entry.file_name().to_string_lossy().into_owned();
            entries.push(DirEntryInfo {
                name,
                path: entry.path(),
                is_dir: ft.is_dir(),
                is_file: ft.is_file(),
                is_symlink: ft.is_symlink(),
            });
        }

        // Sort: directories first, then alphabetical.
        entries.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.cmp(&b.name),
            }
        });

        Ok(entries)
    }

    /// Get symlink metadata (lstat) for a path.
    #[tracing::instrument(skip(self), fields(path = %resolved.display()))]
    pub async fn symlink_metadata(&self, resolved: &Path) -> Result<std::fs::Metadata> {
        tokio::fs::symlink_metadata(resolved).await.map_err(|e| {
            CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("failed to stat {}: {e}", resolved.display()),
            }
        })
    }

    /// Create directories recursively.
    #[tracing::instrument(skip(self), fields(path = %path.display()))]
    pub async fn create_dir_all(&self, path: &Path) -> Result<()> {
        tokio::fs::create_dir_all(path).await.map_err(|e| {
            CuervoError::ToolExecutionFailed {
                tool: "fs_service".into(),
                message: format!("failed to create directories {}: {e}", path.display()),
            }
        })
    }

    // --- Batch Operations ---

    /// Read multiple files concurrently, returning results keyed by path.
    pub async fn batch_read(&self, paths: &[PathBuf]) -> Vec<(PathBuf, Result<String>)> {
        let futs: Vec<_> = paths
            .iter()
            .map(|p| {
                let p = p.clone();
                async move {
                    let result = self.read_to_string(&p).await;
                    (p, result)
                }
            })
            .collect();
        futures::future::join_all(futs).await
    }

    /// Get metadata for multiple files concurrently.
    pub async fn batch_metadata(
        &self,
        paths: &[PathBuf],
    ) -> Vec<(PathBuf, Result<std::fs::Metadata>)> {
        let futs: Vec<_> = paths
            .iter()
            .map(|p| {
                let p = p.clone();
                async move {
                    let result = self.symlink_metadata(&p).await;
                    (p, result)
                }
            })
            .collect();
        futures::future::join_all(futs).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_fs() -> FsService {
        FsService::new(vec![], vec![])
    }

    fn test_fs_with_blocked(blocked: Vec<String>) -> FsService {
        FsService::new(vec![], blocked)
    }

    fn test_fs_with_cache() -> FsService {
        FsService::new_with_cache(vec![], vec![], 100, 512 * 1024)
    }

    // --- resolve_path tests ---

    #[test]
    fn resolve_relative_path() {
        let fs = test_fs();
        let result = fs.resolve_path("src/main.rs", "/project").unwrap();
        assert_eq!(result, PathBuf::from("/project/src/main.rs"));
    }

    #[test]
    fn resolve_absolute_path() {
        let fs = test_fs();
        let result = fs.resolve_path("/project/src/lib.rs", "/project").unwrap();
        assert_eq!(result, PathBuf::from("/project/src/lib.rs"));
    }

    #[test]
    fn resolve_rejects_traversal() {
        let fs = test_fs();
        let result = fs.resolve_path("../../etc/passwd", "/project/src");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_rejects_blocked() {
        let fs = test_fs_with_blocked(vec![".env".into(), "*.pem".into()]);
        assert!(fs.resolve_path(".env", "/project").is_err());
        assert!(fs.resolve_path("certs/server.pem", "/project").is_err());
    }

    // --- read_to_string tests ---

    #[tokio::test]
    async fn read_to_string_success() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world").unwrap();

        let fs = test_fs();
        let content = fs.read_to_string(&file).await.unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn read_to_string_missing() {
        let dir = TempDir::new().unwrap();
        let fs = test_fs();
        let result = fs.read_to_string(&dir.path().join("nonexistent.txt")).await;
        assert!(result.is_err());
    }

    // --- read_lines tests ---

    #[tokio::test]
    async fn read_lines_with_offset_limit() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("lines.txt");
        std::fs::write(&file, "a\nb\nc\nd\ne\n").unwrap();

        let fs = test_fs();
        let (content, total) = fs.read_lines(&file, 1, 2).await.unwrap();
        assert_eq!(total, 5);
        assert!(content.contains("b"));
        assert!(content.contains("c"));
        assert!(!content.contains("\ta\n"));
        assert!(!content.contains("\td\n"));
    }

    #[tokio::test]
    async fn read_lines_all() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("all.txt");
        std::fs::write(&file, "line1\nline2\nline3\n").unwrap();

        let fs = test_fs();
        let (content, total) = fs.read_lines(&file, 0, 0).await.unwrap();
        assert_eq!(total, 3);
        assert!(content.contains("line1"));
        assert!(content.contains("line2"));
        assert!(content.contains("line3"));
    }

    // --- atomic_write tests ---

    #[tokio::test]
    async fn atomic_write_new_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("new.txt");

        let fs = test_fs();
        let bytes = fs.atomic_write(&file, b"hello world").await.unwrap();
        assert_eq!(bytes, 11);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn atomic_write_overwrite() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("exist.txt");
        std::fs::write(&file, "old content").unwrap();

        let fs = test_fs();
        fs.atomic_write(&file, b"new content").await.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new content");
    }

    #[tokio::test]
    async fn atomic_write_no_temp_leftover() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("clean.txt");

        let fs = test_fs();
        fs.atomic_write(&file, b"data").await.unwrap();

        // No .cuervo_tmp_ files should remain.
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with(".cuervo_tmp_"),
                "temp file left over: {name}"
            );
        }
    }

    #[tokio::test]
    async fn atomic_write_creates_dirs() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("a/b/c/deep.txt");

        let fs = test_fs();
        fs.atomic_write(&file, b"nested").await.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "nested");
    }

    // --- delete_file tests ---

    #[tokio::test]
    async fn delete_file_success() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("to_delete.txt");
        std::fs::write(&file, "delete me").unwrap();

        let fs = test_fs();
        let size = fs.delete_file(&file).await.unwrap();
        assert_eq!(size, 9);
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn delete_file_rejects_symlink() {
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real.txt");
        let link = dir.path().join("link.txt");
        std::fs::write(&real, "real content").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(not(unix))]
        return; // Skip on non-unix

        let fs = test_fs();
        let result = fs.delete_file(&link).await;
        assert!(result.is_err());
        assert!(real.exists(), "real file should still exist");
    }

    #[tokio::test]
    async fn delete_file_rejects_dir() {
        let dir = TempDir::new().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();

        let fs = test_fs();
        let result = fs.delete_file(&subdir).await;
        assert!(result.is_err());
    }

    // --- read_dir_async tests ---

    #[tokio::test]
    async fn read_dir_async_sorted() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("zdir")).unwrap();
        std::fs::create_dir(dir.path().join("adir")).unwrap();
        std::fs::write(dir.path().join("bfile.txt"), "").unwrap();
        std::fs::write(dir.path().join("afile.txt"), "").unwrap();

        let fs = test_fs();
        let entries = fs.read_dir_async(dir.path()).await.unwrap();

        // Dirs first (alphabetical), then files (alphabetical).
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].name, "adir");
        assert!(entries[1].is_dir);
        assert_eq!(entries[1].name, "zdir");
        assert!(entries[2].is_file);
        assert_eq!(entries[2].name, "afile.txt");
        assert!(entries[3].is_file);
        assert_eq!(entries[3].name, "bfile.txt");
    }

    #[tokio::test]
    async fn read_dir_async_nonexistent() {
        let fs = test_fs();
        let result = fs.read_dir_async(Path::new("/nonexistent_dir_xyz")).await;
        assert!(result.is_err());
    }

    // --- check_not_symlink tests ---

    #[tokio::test]
    async fn check_not_symlink_rejects() {
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real.txt");
        let link = dir.path().join("link.txt");
        std::fs::write(&real, "content").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(not(unix))]
        return;

        let fs = test_fs();
        let result = fs.check_not_symlink(&link).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn check_not_symlink_allows_regular() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("regular.txt");
        std::fs::write(&file, "content").unwrap();

        let fs = test_fs();
        assert!(fs.check_not_symlink(&file).await.is_ok());
    }

    // --- metrics tests ---

    #[tokio::test]
    async fn metrics_incremented() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("metrics.txt");

        let fs = test_fs();
        fs.atomic_write(&file, b"hello").await.unwrap();
        assert_eq!(fs.metrics().writes.load(Ordering::Relaxed), 1);
        assert_eq!(fs.metrics().write_bytes.load(Ordering::Relaxed), 5);

        fs.read_to_string(&file).await.unwrap();
        assert_eq!(fs.metrics().reads.load(Ordering::Relaxed), 1);
        assert_eq!(fs.metrics().read_bytes.load(Ordering::Relaxed), 5);

        fs.delete_file(&file).await.unwrap();
        assert_eq!(fs.metrics().deletes.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn metrics_errors_counted() {
        let fs = test_fs();
        let _ = fs.read_to_string(Path::new("/nonexistent_xyz")).await;
        assert!(fs.metrics().errors.load(Ordering::Relaxed) >= 1);
    }

    // --- cache tests ---

    #[tokio::test]
    async fn cache_hit_same_mtime() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("cached.txt");
        std::fs::write(&file, "cached content").unwrap();

        let fs = test_fs_with_cache();

        // First read populates cache.
        let content1 = fs.read_to_string(&file).await.unwrap();
        // Second read should use cache.
        let content2 = fs.read_to_string(&file).await.unwrap();
        assert_eq!(content1, content2);
        assert_eq!(content1, "cached content");
        // Two reads recorded.
        assert_eq!(fs.metrics().reads.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn cache_miss_changed_mtime() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("changing.txt");
        std::fs::write(&file, "version1").unwrap();

        let fs = test_fs_with_cache();
        let content1 = fs.read_to_string(&file).await.unwrap();
        assert_eq!(content1, "version1");

        // Wait for mtime to change.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&file, "version2").unwrap();

        let content2 = fs.read_to_string(&file).await.unwrap();
        assert_eq!(content2, "version2");
    }

    #[tokio::test]
    async fn cache_skip_large_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("large.txt");
        // max_entry_size is 512KB, write 600KB.
        std::fs::write(&file, "x".repeat(600 * 1024)).unwrap();

        let fs = test_fs_with_cache();
        let _ = fs.read_to_string(&file).await.unwrap();

        // Cache should not contain this entry (too large).
        // Verify by checking that a second read still works.
        let content = fs.read_to_string(&file).await.unwrap();
        assert_eq!(content.len(), 600 * 1024);
    }

    #[tokio::test]
    async fn cache_invalidated_after_write() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("invalidate.txt");
        std::fs::write(&file, "original").unwrap();

        let fs = test_fs_with_cache();
        let _ = fs.read_to_string(&file).await.unwrap();

        // Write new content via atomic_write.
        fs.atomic_write(&file, b"updated").await.unwrap();

        let content = fs.read_to_string(&file).await.unwrap();
        assert_eq!(content, "updated");
    }

    // --- batch operations tests ---

    #[tokio::test]
    async fn batch_read_multiple_files() {
        let dir = TempDir::new().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        let f3 = dir.path().join("missing.txt");
        std::fs::write(&f1, "aaa").unwrap();
        std::fs::write(&f2, "bbb").unwrap();

        let fs = test_fs();
        let results = fs.batch_read(&[f1.clone(), f2.clone(), f3.clone()]).await;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].1.as_ref().unwrap(), "aaa");
        assert_eq!(results[1].1.as_ref().unwrap(), "bbb");
        assert!(results[2].1.is_err());
    }

    #[tokio::test]
    async fn batch_metadata_multiple_files() {
        let dir = TempDir::new().unwrap();
        let f1 = dir.path().join("exists.txt");
        let f2 = dir.path().join("gone.txt");
        std::fs::write(&f1, "content").unwrap();

        let fs = test_fs();
        let results = fs.batch_metadata(&[f1.clone(), f2.clone()]).await;
        assert_eq!(results.len(), 2);
        assert!(results[0].1.is_ok());
        assert!(results[1].1.is_err());
    }

    // --- create_dir_all test ---

    #[tokio::test]
    async fn create_dir_all_nested() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("x/y/z");

        let fs = test_fs();
        fs.create_dir_all(&nested).await.unwrap();
        assert!(nested.is_dir());
    }

    // --- symlink_metadata test ---

    #[tokio::test]
    async fn symlink_metadata_returns_info() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("meta.txt");
        std::fs::write(&file, "12345").unwrap();

        let fs = test_fs();
        let meta = fs.symlink_metadata(&file).await.unwrap();
        assert!(meta.is_file());
        assert_eq!(meta.len(), 5);
    }
}
