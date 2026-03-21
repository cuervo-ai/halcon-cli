//! Halcon CLI library — exposes modules for testing and benchmarking.
//!
//! This library interface allows benchmarks and integration tests to access
//! internal modules like render, tui, and repl without duplicating code.

// When building with --no-default-features, many modules become unreachable
// because the `tui` feature gates most of the interactive UI code.
#![allow(
    dead_code,
    unused_imports,
    unused_variables,
    unused_assignments,
    unexpected_cfgs,
    private_interfaces,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::should_implement_trait,
    clippy::if_same_then_else,
    clippy::manual_strip,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    clippy::manual_clamp,
    clippy::nonminimal_bool,
    unreachable_patterns,
    clippy::len_without_is_empty,
    clippy::needless_question_mark,
    clippy::manual_let_else,
    clippy::format_in_format_args,
    clippy::unwrap_or_default,
    clippy::empty_line_after_doc_comments,
    clippy::manual_unwrap_or_default,
    clippy::question_mark,
    clippy::needless_range_loop,
    clippy::ptr_arg,
    clippy::enum_variant_names,
    clippy::derivable_impls,
    clippy::unnecessary_cast,
    clippy::needless_return
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
