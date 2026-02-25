//! Formal contracts for the AgentBridge layer.
//!
//! These traits define the ports of the hexagonal architecture:
//!   - AgentExecutor: primary port (what API clients call)
//!   - StreamEmitter: secondary port (how the pipeline emits events)
//!   - PermissionHandler: secondary port (how permissions are resolved)

use std::sync::Arc;

use async_trait::async_trait;
use halcon_core::types::PermissionDecision;
use tokio_util::sync::CancellationToken;

use super::types::{AgentBridgeError, AgentStreamEvent, PermissionRequest, TurnContext, TurnResult};

/// Primary port — clients (halcon-api) invoke this to execute a conversation turn.
///
/// Invariant I-AE-1: Exactly one AgentExecution active per ChatSession.
/// Invariant I-AE-2: completed_at >= started_at always.
///
/// Note: `?Send` because `run_agent_loop` holds tracing `EnteredSpan` across awaits.
/// Use `std::thread::spawn` + `LocalSet::block_on` when calling from a Send context.
#[async_trait(?Send)]
pub trait AgentExecutor {
    /// Execute a full conversation turn.
    ///
    /// Contract:
    /// - MUST emit `TurnCompleted` or `TurnFailed` via `emitter` before returning.
    /// - If `cancellation.is_cancelled()` is detected -> return `Err(CancelledByUser)`.
    /// - `emitter.emit()` is non-blocking; the agent never pauses waiting for UI.
    async fn execute_turn(
        &self,
        context: TurnContext,
        emitter: Arc<dyn StreamEmitter>,
        permission_handler: Arc<dyn PermissionHandler>,
        cancellation: CancellationToken,
    ) -> Result<TurnResult, AgentBridgeError>;
}

/// Secondary port — how the pipeline emits streaming events to clients.
///
/// Invariant: `emit()` MUST be non-blocking.
/// If the internal channel is full, the event is dropped with a warning.
/// The agent pipeline NEVER pauses waiting for a slow consumer.
pub trait StreamEmitter: Send + Sync {
    /// Emit a streaming event. Non-blocking.
    fn emit(&self, event: AgentStreamEvent);

    /// Returns `true` if the downstream client is still connected.
    fn is_connected(&self) -> bool;
}

/// Secondary port — resolves permission requests from the agent pipeline.
///
/// Invariant I-PR-3: A request is resolved exactly once.
/// Invariant: If the channel is closed, returns `Denied` (fail-closed).
#[async_trait]
pub trait PermissionHandler: Send + Sync {
    /// Block until the user approves or rejects the permission request.
    ///
    /// Always returns a decision. Never panics.
    async fn request_permission(&self, request: PermissionRequest) -> PermissionDecision;
}
