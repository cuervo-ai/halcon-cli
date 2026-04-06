//! Unified tracing subsystem for the REPL.
//!
//! Provides a fire-and-forget `TraceRecorder` that consolidates all trace
//! recording from executor, agent loop, and execution tracker into a
//! single source of truth.

pub mod recorder;

pub use recorder::TraceRecorder;
