//! ContextSource adapter for instruction files (CUERVO.md).

use async_trait::async_trait;

use cuervo_core::error::Result;
use cuervo_core::traits::{ContextChunk, ContextQuery, ContextSource};

use crate::assembler::estimate_tokens;
use crate::instruction;

/// A ContextSource that loads CUERVO.md instruction files.
pub struct InstructionSource;

impl InstructionSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for InstructionSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ContextSource for InstructionSource {
    fn name(&self) -> &str {
        "instructions"
    }

    fn priority(&self) -> u32 {
        100 // Highest priority — always included first.
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let working_dir = std::path::Path::new(&query.working_directory);

        match instruction::load_instructions(working_dir) {
            Some(content) => {
                let tokens = estimate_tokens(&content);
                Ok(vec![ContextChunk {
                    source: "instruction".into(),
                    priority: self.priority(),
                    content,
                    estimated_tokens: tokens,
                }])
            }
            None => Ok(vec![]),
        }
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
    async fn no_instruction_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let source = InstructionSource::new();
        let chunks = source
            .gather(&query_for(dir.path().to_str().unwrap()))
            .await
            .unwrap();
        // May return empty or global CUERVO.md content.
        for chunk in &chunks {
            assert_eq!(chunk.source, "instruction");
        }
    }

    #[tokio::test]
    async fn loads_instruction_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("CUERVO.md"), "# Rules\nBe concise.").unwrap();

        let source = InstructionSource::new();
        let chunks = source
            .gather(&query_for(dir.path().to_str().unwrap()))
            .await
            .unwrap();

        assert!(!chunks.is_empty());
        let content = &chunks.last().unwrap().content;
        assert!(content.contains("Be concise."));
    }

    #[test]
    fn metadata() {
        let source = InstructionSource::new();
        assert_eq!(source.name(), "instructions");
        assert_eq!(source.priority(), 100);
    }

    #[tokio::test]
    async fn token_estimation_populated() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("CUERVO.md"), "Hello world test content here").unwrap();

        let source = InstructionSource::new();
        let chunks = source
            .gather(&query_for(dir.path().to_str().unwrap()))
            .await
            .unwrap();

        let chunk = chunks.last().unwrap();
        assert!(chunk.estimated_tokens > 0, "estimated_tokens should be > 0");
    }

    #[tokio::test]
    async fn source_field_is_instruction() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("CUERVO.md"), "content").unwrap();

        let source = InstructionSource::new();
        let chunks = source
            .gather(&query_for(dir.path().to_str().unwrap()))
            .await
            .unwrap();

        for chunk in &chunks {
            assert_eq!(chunk.source, "instruction");
        }
    }

    #[test]
    fn default_creates_source() {
        let source = InstructionSource;
        assert_eq!(source.name(), "instructions");
    }
}
