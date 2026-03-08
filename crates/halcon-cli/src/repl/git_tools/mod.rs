// git_tools/ — Git, IDE, CI integration
// MIGRATION-2026: archivos movidos desde repl/ raíz

pub mod ast_symbols;
pub mod branch;
pub mod ci_detection;
pub mod ci_ingestor;
pub mod instrumentation;
pub mod commit_rewards;
pub mod edit_transaction;
pub mod context;
pub mod events;
pub mod ide_protocol;
pub mod patch;
pub mod project_inspector;
pub mod safe_edit;
pub mod sdlc_phase;
pub mod test_results;
pub mod test_runner;
pub mod traceback;
pub mod unsaved_buffer;

// Re-exports
pub use context::GitContext;
pub use events::GitEventListener;
pub use edit_transaction::EditTransaction;
pub use safe_edit::SafeEditManager;
pub use ci_detection::CiEnvironment;
pub use commit_rewards::CommitRewardTracker;
pub use ast_symbols::SymbolIndex;
pub use branch::BranchDivergenceAnalyzer;
pub use patch::PatchPreviewEngine;
pub use traceback::ParsedFailure;
pub use test_results::TestSuiteResult;
pub use test_runner::TestRunConfig;
pub use project_inspector::ProjectInspector;
pub use sdlc_phase::SdlcPhaseDetector;
pub use unsaved_buffer::UnsavedBufferTracker;
pub use instrumentation::InstrumentedCode;
