//! Message Pipeline — Decomposition of `handle_message_with_sink()`.
//!
//! # Architecture
//!
//! Replaces the 1,584-LOC monolith with 5 typed pipeline stages:
//!
//! ```text
//! UserInput
//!     │
//!     ▼
//! ┌──────────┐   ┌─────────┐   ┌──────────┐   ┌───────────┐   ┌─────────────┐
//! │ 1.INTAKE │──▶│2.CONTEXT│──▶│3.RESOLVE │──▶│ 4.AGENT   │──▶│5.POST-PROC  │
//! │ Guards   │   │ Prompts │   │ Provider │   │   LOOP    │   │ Quality/UCB │
//! │ Plugins  │   │ Media   │   │ Planner  │   │ (existing)│   │ Playbook    │
//! │ Record   │   │ MCP     │   │ Selector │   │           │   │ Memory      │
//! └──────────┘   └─────────┘   └──────────┘   └───────────┘   └─────────────┘
//! ```
//!
//! Each stage:
//! - Declares typed Input → Output contracts
//! - Isolates side effects (DB, network) from pure logic
//! - Is independently testable with mock inputs
//!
//! # Xiyo Comparison
//!
//! Xiyo's `query.ts` uses a single `while(true)` generator with 7 `continue` sites
//! and a flat 10-field `State`. Halcon's pipeline approach provides:
//! - **Stronger typing**: Each stage transition is compiler-enforced
//! - **Better observability**: Stage boundaries are natural tracing span boundaries
//! - **Easier testing**: Each stage can be tested in isolation
//! - **Clearer data flow**: No hidden mutation between stages

pub mod context;
pub mod intake;
pub mod post_process;
pub mod resolve;

// Re-export stage types for the thin orchestrator in mod.rs
pub use intake::IntakeStage;
pub use post_process::PostProcessStage;
pub use resolve::ResolveStage;
