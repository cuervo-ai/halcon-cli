//! Halcon CLI library — exposes modules for testing and benchmarking.
//!
//! This library interface allows benchmarks and integration tests to access
//! internal modules like render, tui, and repl without duplicating code.

// Crate-level lint policy.
//
// `dead_code` + `private_interfaces`: structural — lib.rs exposes modules for
// testing/benchmarking but the real consumer is the binary target (main.rs).
// Functions used only from main.rs appear "dead" when checking the lib target.
//
// `unexpected_cfgs`: custom feature flags (tui, headless, cenzontle-agents, etc.).
//
// All other lints (unused_imports, unused_variables, clippy) are NOT suppressed
// here — they must be fixed at the source or suppressed with targeted attributes.
#![allow(dead_code, private_interfaces, unexpected_cfgs)]
// Clippy style conventions accepted project-wide:
#![allow(
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items
)]

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
