//! ContextSource adapter for the repository map.
//!
//! Scans the working directory for source files, extracts symbols
//! (functions, structs, traits, etc.), and returns a compact repo map
//! as a context chunk for the assembler.

use async_trait::async_trait;
use std::path::Path;

use halcon_context::{build_repo_map, estimate_tokens};
use halcon_core::error::Result;
use halcon_core::traits::{ContextChunk, ContextQuery, ContextSource};

/// Maximum number of source files to scan.
const DEFAULT_MAX_FILES: usize = 200;

/// Default token budget for the rendered repo map.
const DEFAULT_TOKEN_BUDGET: usize = 2000;

/// A ContextSource that provides a structural map of the repository.
///
/// Scans source files in the working directory and extracts symbols
/// (functions, structs, traits, enums, etc.) to give the model
/// awareness of the codebase structure without reading every file.
pub struct RepoMapSource {
    max_files: usize,
    token_budget: usize,
}

impl RepoMapSource {
    pub fn new(max_files: usize, token_budget: usize) -> Self {
        Self {
            max_files,
            token_budget,
        }
    }
}

impl Default for RepoMapSource {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_FILES, DEFAULT_TOKEN_BUDGET)
    }
}

#[async_trait]
impl ContextSource for RepoMapSource {
    fn name(&self) -> &str {
        "repo_map"
    }

    fn priority(&self) -> u32 {
        60 // Below instructions (100) and memory (80), above reflections.
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let root = Path::new(&query.working_directory);

        if !root.is_dir() {
            return Ok(vec![]);
        }

        // build_repo_map does synchronous I/O (file scanning + reading).
        // Run in spawn_blocking to avoid blocking the async runtime.
        let root_owned = root.to_path_buf();
        let max_files = self.max_files;
        let token_budget = self.token_budget;

        let rendered = tokio::task::spawn_blocking(move || {
            let map = build_repo_map(&root_owned, max_files, token_budget);
            if map.file_count() == 0 {
                return String::new();
            }
            map.render(token_budget)
        })
        .await
        .unwrap_or_default();

        if rendered.is_empty() {
            return Ok(vec![]);
        }

        let tokens = estimate_tokens(&rendered);

        Ok(vec![ContextChunk {
            source: "repo_map".into(),
            priority: self.priority(),
            content: rendered,
            estimated_tokens: tokens,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn query_for(dir: &str) -> ContextQuery {
        ContextQuery {
            working_directory: dir.into(),
            user_message: None,
            token_budget: 10000,
        }
    }

    #[tokio::test]
    async fn empty_directory_returns_empty() {
        let dir = TempDir::new().unwrap();
        let source = RepoMapSource::default();
        let chunks = source
            .gather(&query_for(dir.path().to_str().unwrap()))
            .await
            .unwrap();
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn nonexistent_directory_returns_empty() {
        let source = RepoMapSource::default();
        let chunks = source
            .gather(&query_for("/nonexistent/path/xyz"))
            .await
            .unwrap();
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn scans_rust_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("lib.rs"),
            "pub fn hello() {}\npub struct Foo;\npub trait Bar {}",
        )
        .unwrap();

        let source = RepoMapSource::new(100, 5000);
        let chunks = source
            .gather(&query_for(dir.path().to_str().unwrap()))
            .await
            .unwrap();

        assert_eq!(chunks.len(), 1);
        let content = &chunks[0].content;
        assert!(content.contains("[Repository Map]"));
        assert!(content.contains("hello"));
        assert!(content.contains("Foo"));
        assert!(content.contains("Bar"));
    }

    #[tokio::test]
    async fn source_metadata() {
        let source = RepoMapSource::default();
        assert_eq!(source.name(), "repo_map");
        assert_eq!(source.priority(), 60);
    }

    #[tokio::test]
    async fn chunk_source_field() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

        let source = RepoMapSource::default();
        let chunks = source
            .gather(&query_for(dir.path().to_str().unwrap()))
            .await
            .unwrap();

        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].source, "repo_map");
    }

    #[tokio::test]
    async fn respects_token_budget() {
        let dir = TempDir::new().unwrap();
        // Create many files to exceed a tiny budget.
        for i in 0..20 {
            std::fs::write(
                dir.path().join(format!("mod_{i}.rs")),
                format!(
                    "pub fn func_{i}_alpha() {{}}\npub fn func_{i}_beta() {{}}\npub struct Type{i};\n"
                ),
            )
            .unwrap();
        }

        let source = RepoMapSource::new(200, 100); // Very small budget.
        let chunks = source
            .gather(&query_for(dir.path().to_str().unwrap()))
            .await
            .unwrap();

        if !chunks.is_empty() {
            // Token count should be bounded by our budget.
            assert!(chunks[0].estimated_tokens <= 200); // Some overhead beyond budget is OK.
        }
    }

    #[tokio::test]
    async fn token_estimation_nonzero() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("app.rs"), "pub fn run() {}").unwrap();

        let source = RepoMapSource::default();
        let chunks = source
            .gather(&query_for(dir.path().to_str().unwrap()))
            .await
            .unwrap();

        assert!(!chunks.is_empty());
        assert!(chunks[0].estimated_tokens > 0);
    }

    #[test]
    fn default_configuration() {
        let source = RepoMapSource::default();
        assert_eq!(source.max_files, DEFAULT_MAX_FILES);
        assert_eq!(source.token_budget, DEFAULT_TOKEN_BUDGET);
    }

    #[tokio::test]
    async fn multiple_languages() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("lib.rs"), "pub fn rust_fn() {}").unwrap();
        std::fs::write(dir.path().join("app.py"), "def python_fn():\n    pass").unwrap();
        std::fs::write(
            dir.path().join("index.js"),
            "function js_fn() {}",
        )
        .unwrap();

        let source = RepoMapSource::new(100, 5000);
        let chunks = source
            .gather(&query_for(dir.path().to_str().unwrap()))
            .await
            .unwrap();

        assert!(!chunks.is_empty());
        let content = &chunks[0].content;
        assert!(content.contains("rust_fn"));
        assert!(content.contains("python_fn"));
        assert!(content.contains("js_fn"));
    }
}
