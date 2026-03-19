//! Context engine for Halcon CLI.
//!
//! Provides instruction file loading (HALCON.md), repository mapping,
//! context assembly with token budgeting, and the multi-tiered context
//! pipeline (L0 hot buffer, L1 sliding window, token accounting, elision).

pub mod assembler;
pub mod instruction;
pub mod instruction_source;

// Context Engine v2 modules
pub mod accountant;
pub mod cold_archive;
pub mod cold_store;
pub mod compression;
pub mod elider;
pub mod hot_buffer;
pub mod instruction_cache;
pub mod pipeline;
pub mod segment;
pub mod repo_map;
pub mod embedding;
pub mod semantic_store;
pub mod sliding_window;
pub mod vector_store;

pub use assembler::{assemble_context, chunks_to_system_prompt, estimate_tokens};
pub use instruction::{find_instruction_files, load_instructions};
pub use instruction_source::InstructionSource;

// v2 public API
pub use accountant::{estimate_message_tokens, BudgetResult, Tier, TokenAccountant};
pub use cold_archive::ColdArchive;
pub use cold_store::ColdStore;
pub use compression::{compress, decompress, delta_decode, delta_encode, CompressedBlock, DeltaEncoded, DeltaOp};
pub use elider::ToolOutputElider;
pub use hot_buffer::HotBuffer;
pub use instruction_cache::InstructionCache;
pub use pipeline::{ContextPipeline, ContextPipelineConfig};
pub use segment::{extract_segment_from_message, ContextSegment};
pub use repo_map::{RepoMap, build_repo_map};
pub use embedding::{
    cosine_sim, EmbeddingEngine, EmbeddingEngineFactory, OllamaEmbeddingEngine,
    TfIdfHashEngine, DIMS, OLLAMA_DEFAULT_ENDPOINT, OLLAMA_DEFAULT_MODEL,
};

/// Obtain the best available embedding engine, respecting env-var overrides.
///
/// Resolution order: `HALCON_EMBEDDING_ENDPOINT` > `OLLAMA_HOST` > default localhost.
/// All subsystems without access to PolicyConfig should call this instead of
/// constructing engines directly. Policy-level callers should use
/// `EmbeddingEngineFactory::with_config(&policy.embedding_endpoint, &policy.embedding_model)`.
pub fn embedding_engine() -> Box<dyn EmbeddingEngine> {
    embedding::EmbeddingEngineFactory::from_env()
}
pub mod semantic_cache;
pub use semantic_cache::{
    CacheOutcome, CacheResult, SemanticCache,
    TTL_CODE_GEN, TTL_CONVERSATION, TTL_DEFAULT, TTL_RESEARCH, TTL_SUMMARIZATION,
};
pub use semantic_store::SemanticStore;
pub use vector_store::{MemoryEntry, SearchResult, VectorMemoryStore};
pub use sliding_window::SlidingWindow;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_test() {
        assert!(!version().is_empty());
    }
}
