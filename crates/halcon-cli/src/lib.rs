//! Halcon CLI library — exposes modules for testing and benchmarking.
//!
//! This library interface allows benchmarks and integration tests to access
//! internal modules like render, tui, and repl without duplicating code.

// Module declarations (same as main.rs)
#[path = "audit/mod.rs"]
pub mod audit;

// commands must be accessible from repl/ and tui/ (e.g., update::UpdateInfo)
#[path = "commands/mod.rs"]
pub(crate) mod commands;

#[path = "config_loader.rs"]
pub(crate) mod config_loader;

#[path = "render/mod.rs"]
pub mod render;

#[cfg(feature = "tui")]
#[path = "tui/mod.rs"]
pub mod tui;

#[path = "repl/mod.rs"]
pub mod repl;

// Re-export commonly used types for convenience
pub use render::theme;

#[cfg(feature = "headless")]
#[path = "agent_bridge/mod.rs"]
pub mod agent_bridge;
