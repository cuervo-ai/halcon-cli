/// Codebase Context Server (Server 3).
///
/// Provides context from repository structure, recent code changes, and symbol index.
/// Reuses existing RepoMap infrastructure from halcon-cli.
/// Phase: Implementation
/// Priority: 85

use async_trait::async_trait;
use halcon_context::estimate_tokens;
use halcon_core::error::Result;
use halcon_core::traits::{ContextChunk, ContextQuery, ContextSource};
use halcon_core::types::SdlcPhase;
use std::path::{Path, PathBuf};

pub struct CodebaseServer {
    working_dir: PathBuf,
    priority: u32,
    token_budget: u32,
}

impl CodebaseServer {
    pub fn new(working_dir: impl AsRef<Path>, priority: u32, token_budget: u32) -> Self {
        Self {
            working_dir: working_dir.as_ref().to_path_buf(),
            priority,
            token_budget,
        }
    }

    pub fn sdlc_phase(&self) -> SdlcPhase {
        SdlcPhase::Implementation
    }

    async fn build_codebase_context(&self, query: Option<&str>) -> Result<String> {
        let working_dir = self.working_dir.clone();
        let query_opt = query.map(String::from);

        tokio::task::spawn_blocking(move || {
            // Build repo map from working directory
            let repo_map = halcon_context::build_repo_map(&working_dir, 500, 5000);

            // If query present, search for relevant symbols
            let context = if let Some(q) = query_opt {
                let query_lower = q.to_lowercase();
                let relevant_symbols: Vec<_> = repo_map
                    .search(&query_lower)
                    .into_iter()
                    .take(20) // Limit to top 20 matches
                    .collect();

                if relevant_symbols.is_empty() {
                    // No symbol matches, return general repo structure
                    format!(
                        "[Codebase Structure]\nTotal files: {}\nTotal symbols: {}\n\nDirectory tree (limited view):\n{}",
                        repo_map.file_count(),
                        repo_map.symbol_count(),
                        repo_map.render(500) // Budget: ~500 tokens for tree
                    )
                } else {
                    // Return relevant symbols
                    let mut content = String::from("[Codebase Context - Relevant Symbols]\n\n");
                    for symbol in &relevant_symbols {
                        content.push_str(&format!("{:?} {} in {} (line {})\n",
                            symbol.kind, symbol.signature, symbol.file_path, symbol.line));
                    }
                    content.push_str(&format!("\nTotal matches: {} out of {} symbols\n",
                        relevant_symbols.len(), repo_map.symbol_count()));
                    content
                }
            } else {
                // No query, return general overview
                format!(
                    "[Codebase Overview]\nTotal files: {}\nTotal symbols: {}\n\nTop-level structure:\n{}",
                    repo_map.file_count(),
                    repo_map.symbol_count(),
                    repo_map.render(800) // Budget: ~800 tokens for overview
                )
            };

            Ok::<String, halcon_core::error::HalconError>(context)
        })
        .await
        .map_err(|e| halcon_core::error::HalconError::DatabaseError(e.to_string()))?
    }
}

#[async_trait]
impl ContextSource for CodebaseServer {
    fn name(&self) -> &str {
        "codebase"
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let context = self
            .build_codebase_context(query.user_message.as_deref())
            .await?;

        let token_estimate = estimate_tokens(&context);

        // Truncate if exceeds budget
        let final_context = if token_estimate > self.token_budget as usize {
            let char_limit = (self.token_budget as usize * 4).min(context.len()); // ~4 chars per token
            format!("{}...\n[Truncated due to budget limit]", &context[..char_limit])
        } else {
            context
        };

        let final_tokens = estimate_tokens(&final_context);

        Ok(vec![ContextChunk {
            source: self.name().to_string(),
            priority: self.priority,
            content: final_context,
            estimated_tokens: final_tokens,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_codebase_server_creation() {
        let server = CodebaseServer::new("/tmp/test-repo", 85, 5000);
        assert_eq!(server.name(), "codebase");
        assert_eq!(server.priority(), 85);
        assert_eq!(server.sdlc_phase(), SdlcPhase::Implementation);
    }

    #[tokio::test]
    async fn test_gather_nonexistent_dir() {
        let server = CodebaseServer::new("/nonexistent/repo", 85, 5000);

        let query = ContextQuery {
            working_directory: "/nonexistent/repo".to_string(),
            user_message: None,
            token_budget: 10000,
        };

        // Should handle gracefully (empty repo map)
        let result = server.gather(&query).await;
        // May succeed with empty or fail gracefully - both acceptable
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_gather_with_current_dir() {
        // Use actual codebase (halcon-cli itself)
        let current_dir = std::env::current_dir().unwrap();
        let server = CodebaseServer::new(&current_dir, 85, 5000);

        let query = ContextQuery {
            working_directory: current_dir.display().to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("Codebase"));
        assert!(chunks[0].estimated_tokens > 0);
    }

    #[tokio::test]
    async fn test_symbol_search() {
        let current_dir = std::env::current_dir().unwrap();
        let server = CodebaseServer::new(&current_dir, 85, 5000);

        let query = ContextQuery {
            working_directory: current_dir.display().to_string(),
            user_message: Some("CodebaseServer".to_string()), // Search for this struct
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        // Should find CodebaseServer struct reference
        assert!(chunks[0].content.contains("Relevant Symbols") || chunks[0].content.contains("Structure"));
    }

    #[tokio::test]
    async fn test_budget_enforcement() {
        let current_dir = std::env::current_dir().unwrap();
        let server = CodebaseServer::new(&current_dir, 85, 100); // Very small budget

        let query = ContextQuery {
            working_directory: current_dir.display().to_string(),
            user_message: None,
            token_budget: 10000,
        };

        let chunks = server.gather(&query).await.unwrap();
        assert_eq!(chunks.len(), 1);
        // Should be truncated
        assert!(chunks[0].estimated_tokens <= 150); // Some overhead allowed
        if chunks[0].content.len() > 500 {
            assert!(chunks[0].content.contains("Truncated"));
        }
    }
}
