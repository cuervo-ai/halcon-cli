//! Failure classification for tool errors.
//!
//! Provides a single source of truth for transient vs deterministic error
//! classification, used by both the executor (per-tool retry) and the agent
//! loop (post-batch error filtering).
//!
//! The waterfall pattern (retry → repair → fallback → surface) is implemented
//! at two separate levels:
//!   - Per-tool: `executor::retry::run_with_retry()` handles transient retries
//!   - Per-loop: `feedback_arbiter::decide()` handles recovery actions
//! These operate independently and should NOT be unified into a single function.

pub mod classifier;
pub mod typed_errors;
