//! Instruction file cache with mtime-based invalidation.
//!
//! Caches the content of CUERVO.md instruction files to avoid repeated
//! disk I/O on every context assembly. Validates freshness by checking
//! file modification time (stat syscall, ~10μs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use crate::assembler::estimate_tokens;

/// Cached instruction file entry.
struct CachedInstruction {
    content: String,
    token_estimate: u32,
    mtime: SystemTime,
    #[allow(dead_code)]
    loaded_at: Instant,
}

/// Cache for instruction files with mtime-based invalidation.
pub struct InstructionCache {
    entries: HashMap<PathBuf, CachedInstruction>,
    total_tokens: u32,
}

impl InstructionCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            total_tokens: 0,
        }
    }

    /// Get the content of an instruction file, loading from disk if not cached or stale.
    ///
    /// Returns `None` if the file doesn't exist or can't be read.
    pub fn get_or_load(&mut self, path: &Path) -> Option<&str> {
        // Check if cached entry is still valid
        if let Some(cached) = self.entries.get(path) {
            if let Ok(meta) = path.metadata() {
                if let Ok(mtime) = meta.modified() {
                    if mtime == cached.mtime {
                        return Some(&self.entries[path].content);
                    }
                }
            }
            // Stale or can't stat — fall through to reload
        }

        // Cache miss or stale: reload from disk
        let content = std::fs::read_to_string(path).ok()?;
        if content.trim().is_empty() {
            return None;
        }
        let tokens = estimate_tokens(&content) as u32;
        let mtime = path
            .metadata()
            .ok()?
            .modified()
            .ok()?;

        self.entries.insert(
            path.to_owned(),
            CachedInstruction {
                content,
                token_estimate: tokens,
                mtime,
                loaded_at: Instant::now(),
            },
        );
        self.recalculate_total();
        Some(&self.entries[path].content)
    }

    /// Invalidate a specific cached entry.
    pub fn invalidate(&mut self, path: &Path) {
        self.entries.remove(path);
        self.recalculate_total();
    }

    /// Invalidate all cached entries.
    pub fn invalidate_all(&mut self) {
        self.entries.clear();
        self.total_tokens = 0;
    }

    /// Total estimated tokens across all cached instruction files.
    pub fn total_tokens(&self) -> u32 {
        self.total_tokens
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get all cached paths.
    pub fn cached_paths(&self) -> Vec<&Path> {
        self.entries.keys().map(|p| p.as_path()).collect()
    }

    /// Compute a deterministic hash of all cached instruction content.
    ///
    /// Used for efficient change detection: if the hash is the same after
    /// `get_or_load()` calls, no content has changed.
    pub fn content_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        let mut paths: Vec<_> = self.entries.keys().collect();
        paths.sort();
        for path in paths {
            path.hash(&mut hasher);
            self.entries[path].content.hash(&mut hasher);
        }
        hasher.finish()
    }

    fn recalculate_total(&mut self) {
        self.total_tokens = self.entries.values().map(|c| c.token_estimate).sum();
    }
}

impl Default for InstructionCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn empty_cache() {
        let cache = InstructionCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.total_tokens(), 0);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn load_file_into_cache() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CUERVO.md");
        std::fs::write(&path, "# Instructions\nUse Rust.").unwrap();

        let mut cache = InstructionCache::new();
        let content = cache.get_or_load(&path);
        assert!(content.is_some());
        assert!(content.unwrap().contains("Use Rust"));
        assert_eq!(cache.len(), 1);
        assert!(cache.total_tokens() > 0);
    }

    #[test]
    fn cache_hit_returns_same_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CUERVO.md");
        std::fs::write(&path, "cached content").unwrap();

        let mut cache = InstructionCache::new();
        let first = cache.get_or_load(&path).unwrap().to_string();
        let second = cache.get_or_load(&path).unwrap().to_string();
        assert_eq!(first, second);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn stale_file_reloads() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CUERVO.md");
        std::fs::write(&path, "version 1").unwrap();

        let mut cache = InstructionCache::new();
        let v1 = cache.get_or_load(&path).unwrap().to_string();
        assert!(v1.contains("version 1"));

        // Modify the file (need a small delay for mtime to change)
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&path, "version 2").unwrap();

        let v2 = cache.get_or_load(&path).unwrap().to_string();
        assert!(v2.contains("version 2"));
    }

    #[test]
    fn missing_file_returns_none() {
        let mut cache = InstructionCache::new();
        let result = cache.get_or_load(Path::new("/tmp/nonexistent_cuervo_test_file.md"));
        assert!(result.is_none());
        assert!(cache.is_empty());
    }

    #[test]
    fn empty_file_returns_none() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CUERVO.md");
        std::fs::write(&path, "   \n  ").unwrap();

        let mut cache = InstructionCache::new();
        let result = cache.get_or_load(&path);
        assert!(result.is_none());
    }

    #[test]
    fn invalidate_removes_entry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CUERVO.md");
        std::fs::write(&path, "content").unwrap();

        let mut cache = InstructionCache::new();
        cache.get_or_load(&path);
        assert_eq!(cache.len(), 1);

        cache.invalidate(&path);
        assert!(cache.is_empty());
        assert_eq!(cache.total_tokens(), 0);
    }

    #[test]
    fn invalidate_all_clears_cache() {
        let dir = TempDir::new().unwrap();
        let p1 = dir.path().join("a.md");
        let p2 = dir.path().join("b.md");
        std::fs::write(&p1, "file a").unwrap();
        std::fs::write(&p2, "file b").unwrap();

        let mut cache = InstructionCache::new();
        cache.get_or_load(&p1);
        cache.get_or_load(&p2);
        assert_eq!(cache.len(), 2);

        cache.invalidate_all();
        assert!(cache.is_empty());
    }

    #[test]
    fn total_tokens_accumulates() {
        let dir = TempDir::new().unwrap();
        let p1 = dir.path().join("a.md");
        let p2 = dir.path().join("b.md");
        std::fs::write(&p1, "short").unwrap();
        std::fs::write(&p2, "also short").unwrap();

        let mut cache = InstructionCache::new();
        cache.get_or_load(&p1);
        let t1 = cache.total_tokens();
        cache.get_or_load(&p2);
        let t2 = cache.total_tokens();
        assert!(t2 > t1);
    }

    #[test]
    fn cached_paths_lists_all() {
        let dir = TempDir::new().unwrap();
        let p1 = dir.path().join("a.md");
        let p2 = dir.path().join("b.md");
        std::fs::write(&p1, "content a").unwrap();
        std::fs::write(&p2, "content b").unwrap();

        let mut cache = InstructionCache::new();
        cache.get_or_load(&p1);
        cache.get_or_load(&p2);

        let paths = cache.cached_paths();
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn content_hash_empty_cache() {
        let cache = InstructionCache::new();
        let h = cache.content_hash();
        // Empty cache should produce a consistent hash.
        assert_eq!(h, InstructionCache::new().content_hash());
    }

    #[test]
    fn content_hash_changes_on_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CUERVO.md");
        std::fs::write(&path, "instructions v1").unwrap();

        let mut cache = InstructionCache::new();
        let h_empty = cache.content_hash();
        cache.get_or_load(&path);
        let h_loaded = cache.content_hash();
        assert_ne!(h_empty, h_loaded);
    }

    #[test]
    fn content_hash_changes_on_file_update() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CUERVO.md");
        std::fs::write(&path, "version 1").unwrap();

        let mut cache = InstructionCache::new();
        cache.get_or_load(&path);
        let h1 = cache.content_hash();

        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&path, "version 2").unwrap();
        cache.get_or_load(&path);
        let h2 = cache.content_hash();
        assert_ne!(h1, h2);
    }

    #[test]
    fn content_hash_stable_without_changes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CUERVO.md");
        std::fs::write(&path, "stable content").unwrap();

        let mut cache = InstructionCache::new();
        cache.get_or_load(&path);
        let h1 = cache.content_hash();
        // Second load without changes — hash should be identical.
        cache.get_or_load(&path);
        let h2 = cache.content_hash();
        assert_eq!(h1, h2);
    }
}
