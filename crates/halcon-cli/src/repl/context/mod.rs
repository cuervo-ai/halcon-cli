// context/ — context sources, memory retrieval, governance
// MIGRATION-2026: archivos movidos desde repl/ raíz

pub mod compaction;
pub mod compaction_budget;
pub mod compaction_summary;
// DELETED: compaction_pipeline (dead code, replaced by compaction::ContextCompactor)
pub mod consolidator;
pub mod intent_anchor;
pub mod preflight_gate;
pub mod protected_context;
pub mod tiered_compactor;
pub mod tool_result_truncator;
// Fase 2: advanced context management
pub mod compaction_eval;
pub mod episodic;
pub mod file_re_reader;
pub(crate) mod governance;
pub mod hybrid_retriever;
pub(crate) mod manager;
pub mod memory;
pub(crate) mod metrics;
pub mod reflection;
pub mod repo_map;
pub mod tool_result_evictor;
pub mod tool_result_persister;
pub mod vector_memory;

// Re-exports — preserve API surface for callers in repl/
// consolidator: free functions (consolidate, maybe_consolidate), no MemoryConsolidator struct
