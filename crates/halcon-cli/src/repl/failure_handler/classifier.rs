//! Error classification: transient vs deterministic (pure, no I/O).
//!
//! Delegates to the canonical implementations in `executor::retry` to maintain
//! a single source of truth for error pattern matching.

/// Classify whether an error is transient (may succeed on retry).
pub fn is_transient(error: &str) -> bool {
    crate::repl::executor::is_transient_error(error)
}

/// Classify whether an error is deterministic (will never succeed on retry).
pub fn is_deterministic(error: &str) -> bool {
    crate::repl::executor::is_deterministic_error(error)
}

/// High-level error classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Temporary condition — retry may succeed.
    Transient,
    /// Permanent condition — retry will produce the same result.
    Deterministic,
    /// Unknown — cannot classify; default to surface.
    Unknown,
}

/// Classify an error string into a high-level category.
pub fn classify(error: &str) -> ErrorKind {
    if is_transient(error) {
        ErrorKind::Transient
    } else if is_deterministic(error) {
        ErrorKind::Deterministic
    } else {
        ErrorKind::Unknown
    }
}
