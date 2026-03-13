//! `HalconAgentRuntime` — the authoritative contract for all agent session entry points.
//!
//! # Design
//!
//! Both the CLI REPL and the HTTP API bridge converge on `run_agent_loop()` in
//! `halcon-cli/src/repl/agent/mod.rs`. This trait makes that contract explicit so that:
//!
//!   1. Future implementations (GDEM, federation, replay) have a typed interface to satisfy.
//!   2. The `AgentBridgeImpl` (HTTP bridge) and `Repl::handle_message_with_sink` (CLI) can
//!      each be verified against the same contract.
//!   3. Tests can use a mock `HalconAgentRuntime` without spinning up the full REPL.
//!
//! # Invariants
//!
//!   - Every agent session produces exactly one `AgentSessionResult`.
//!   - `run_session` is the ONLY way to advance a session to the next round.
//!   - The trait is `Send + Sync` so runtime objects can be shared across threads (API server).
//!
//! # Phase 2 wiring
//!
//! - `AgentBridgeImpl::run_turn()` satisfies this contract (call graph verified 2026-03-12).
//! - `Repl::handle_message_with_sink()` satisfies this contract (call graph verified 2026-03-12).
//! - Both call `crate::repl::agent::run_agent_loop()` as the concrete implementation.

use async_trait::async_trait;
use uuid::Uuid;

/// Result returned by one agent session turn.
///
/// Produced by `run_agent_loop()` and returned through every session entry point.
#[derive(Debug, Clone)]
pub struct AgentSessionResult {
    /// The final synthesised text response to the user.
    pub response_text: String,
    /// The reason the session loop terminated.
    pub stop_reason: AgentStopReason,
    /// Total rounds (model invocations) executed.
    pub rounds: u32,
    /// Input tokens consumed across all rounds.
    pub input_tokens: u64,
    /// Output tokens consumed across all rounds.
    pub output_tokens: u64,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Session identifier (for audit/trace correlation).
    pub session_id: Uuid,
}

/// Why the agent loop terminated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStopReason {
    /// Normal: model produced a final response (end_turn).
    EndTurn,
    /// Reached configured max_rounds limit.
    MaxRounds,
    /// Token budget exhausted.
    TokenBudget,
    /// Wall-clock duration limit reached.
    DurationBudget,
    /// User or API cancelled the session.
    Interrupted,
    /// Provider returned an unrecoverable error.
    ProviderError,
    /// Loop guard forced synthesis (stagnation/oscillation detected).
    ForcedSynthesis,
    /// Goal achieved — TerminationOracle confirmed completion.
    GoalAchieved,
}

/// The authoritative contract for all HALCON agent session entry points.
///
/// ## Implementations
///
/// | Implementation | Crate | Status |
/// |----------------|-------|--------|
/// | `Repl` | halcon-cli | ✅ `repl::mod.rs` — delegates to `handle_message_with_sink → run_agent_loop` |
/// | `AgentBridgeImpl` | halcon-cli | ⏳ Pending — bridge wires directly to `run_agent_loop` |
/// | GDEM (Phase 2.4+) | halcon-agent-core | ⏳ Feature-gated (`gdem-primary` OFF) |
///
/// ## Contract
///
/// - Implementors MUST call `run_agent_loop()` (or an approved equivalent) internally.
/// - Implementors MUST NOT start parallel agent loops for the same session.
/// - `session_id()` must return a stable UUID for the lifetime of the session.
///
/// Note: `?Send` is used for the async impl because `run_agent_loop()` holds `EnteredSpan`
/// (a tracing span) across `.await` points — `EnteredSpan` is `!Send`. Use `Arc<Mutex<dyn
/// HalconAgentRuntime>>` to share a runtime object across async tasks.
#[async_trait(?Send)]
pub trait HalconAgentRuntime {
    /// Return the unique identifier for this session.
    fn session_id(&self) -> Uuid;

    /// Execute one turn of the agent loop with `user_message`.
    ///
    /// This is the single authorized way to advance an agent session.
    /// The caller must not call this concurrently on the same session instance.
    async fn run_session(&mut self, user_message: &str) -> anyhow::Result<AgentSessionResult>;

    /// Return the underlying loop implementation name (for observability/tracing).
    ///
    /// Known values: `"legacy-repl"`, `"gdem-primary"`, `"bridge-api"`, `"mock"`.
    fn runtime_name(&self) -> &'static str;
}
