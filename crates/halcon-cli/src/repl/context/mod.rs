// context/ — context sources, memory retrieval, governance
// MIGRATION-2026: archivos movidos desde repl/ raíz

pub(crate) mod governance;
pub(crate) mod manager;
pub(crate) mod metrics;
pub mod episodic;
pub mod hybrid_retriever;
pub mod consolidator;
pub mod memory;
pub mod reflection;
pub mod repo_map;
pub mod vector_memory;

// Re-exports — preserve API surface for callers in repl/
pub(crate) use manager::{ContextManager, AssembledContext, SubAgentContext};
pub(crate) use governance::{ContextGovernance, ContextProvenance};
pub(crate) use metrics::ContextMetrics;
pub use memory::MemorySource;
pub use episodic::EpisodicSource;
pub use vector_memory::{VectorMemorySource, SharedVectorStore};
pub use hybrid_retriever::HybridRetriever;
pub use reflection::ReflectionSource;
pub use repo_map::RepoMapSource;
pub use consolidator::MemoryConsolidator;
