//! Phase 3c: SessionMode — route between Interactive and Structured execution.
//!
//! Interactive mode uses the simplified FeedbackArbiter-based loop (Phase 3b).
//! Structured mode uses GDEM with goal verification (CI, code review, compliance).
//!
//! The default remains the legacy `run_agent_loop()` unless the `simplified-loop`
//! feature is enabled.

/// Execution mode for an agent session.
///
/// Determines which agent loop implementation handles the session.
#[derive(Debug, Clone)]
pub enum SessionMode {
    /// Simplified loop with FeedbackArbiter (Phase 3b).
    /// Used for interactive REPL sessions where the LLM decides when to stop.
    Interactive,

    /// GDEM-based structured execution with goal verification.
    /// Used for CI, code review, compliance — tasks with verifiable criteria.
    #[cfg(feature = "gdem-primary")]
    Structured(halcon_agent_core::loop_driver::GdemConfig),
}

impl Default for SessionMode {
    fn default() -> Self {
        Self::Interactive
    }
}

impl SessionMode {
    /// Whether this session uses the simplified interactive loop.
    pub fn is_interactive(&self) -> bool {
        matches!(self, Self::Interactive)
    }

    /// Whether this session uses structured GDEM execution.
    #[cfg(feature = "gdem-primary")]
    pub fn is_structured(&self) -> bool {
        matches!(self, Self::Structured(_))
    }
}

impl std::fmt::Display for SessionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interactive => write!(f, "interactive"),
            #[cfg(feature = "gdem-primary")]
            Self::Structured(_) => write!(f, "structured"),
        }
    }
}
