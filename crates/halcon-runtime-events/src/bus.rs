//! EventBus — broadcast channel for `RuntimeEvent` distribution.
//!
//! # Design
//!
//! ```text
//! AgentEngine
//!   └─ EventBus (tokio::sync::broadcast, capacity = 1024)
//!        ├─ CliEventSink          ← terminal rendering
//!        ├─ JsonRpcEventSink      ← VS Code extension (NDJSON over stdio)
//!        ├─ WsSink (Phase 8)      ← WebSocket clients
//!        └─ SilentSink            ← unit tests / sub-agents
//! ```
//!
//! # Thread-safety
//!
//! `EventBus` is `Clone + Send + Sync`. Cloning produces a new sender that
//! shares the same broadcast channel — all clones deliver to all subscribers.
//!
//! # Back-pressure
//!
//! `tokio::sync::broadcast` drops the oldest message when the channel is full.
//! The default capacity of 1024 is sufficient for all IDE panel use cases; slow
//! consumers (e.g. a paused WS client) will miss events rather than blocking
//! the agent loop. Consumers that require guaranteed delivery (e.g. audit)
//! should use `halcon_storage::AsyncDatabase` directly, not this bus.
//!
//! # Emit model
//!
//! `EventBus::emit()` is a **synchronous** fire-and-forget call with zero
//! allocations in the hot path (the `RuntimeEvent` is already heap-allocated
//! by the caller). It must never block the agent loop. The tokio broadcast
//! `send()` is lock-free on the fast path.

#[cfg(feature = "bus")]
use tokio::sync::broadcast;

use uuid::Uuid;

use crate::event::{RuntimeEvent, RuntimeEventKind};

// ─── EventSink trait ─────────────────────────────────────────────────────────

/// Trait for consuming `RuntimeEvent`s.
///
/// Implementations must be `Send + Sync` so they can be shared across the
/// async runtime without additional locking. The `emit` method is intentionally
/// synchronous — implementations that need to do async work (e.g. writing to a
/// WebSocket) must use an internal channel and spawn a background task in their
/// constructor.
pub trait EventSink: Send + Sync {
    /// Consume a `RuntimeEvent`. Must not block or panic.
    fn emit(&self, event: &RuntimeEvent);

    /// Whether this sink is in silent mode (tests, sub-agents).
    ///
    /// When `true`, the agent loop may skip constructing expensive event payloads
    /// that are only needed for display — an optimisation hint, not a guarantee.
    fn is_silent(&self) -> bool {
        false
    }
}

// ─── EventBus ────────────────────────────────────────────────────────────────

/// Broadcast channel for `RuntimeEvent`.
///
/// Create with `EventBus::new(capacity)` then pass to `AgentEngine` and
/// subscribe sinks via `EventBus::subscribe()`.
///
/// `EventBus` itself does **not** implement `EventSink` — call `EventBus::emit()`
/// directly from the producer side. Consumers receive events via the
/// `EventReceiver` handle returned by `subscribe()`.
#[cfg(feature = "bus")]
#[derive(Clone, Debug)]
pub struct EventBus {
    sender: broadcast::Sender<RuntimeEvent>,
}

#[cfg(feature = "bus")]
impl EventBus {
    /// Create a new bus with `capacity` slots.
    ///
    /// 1024 is sufficient for typical IDE sessions. Increase for high-throughput
    /// scenarios (e.g. large multi-agent trees with many tool calls per second).
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Emit a `RuntimeEventKind`, wrapping it in a `RuntimeEvent` envelope.
    ///
    /// `session_id` is injected here so call sites don't need to build the
    /// envelope themselves. If no subscribers exist the event is silently
    /// discarded (not an error).
    #[inline]
    pub fn emit(&self, session_id: Uuid, kind: RuntimeEventKind) {
        let event = RuntimeEvent::new(session_id, kind);
        // `send` returns Err if there are no receivers — that is fine; the bus
        // may have zero subscribers during early startup or in test runs.
        if self.sender.send(event).is_err() {
            tracing::trace!("Event bus: no subscribers (dropped event)");
        }
    }

    /// Subscribe to the event stream, receiving all future events.
    ///
    /// Lagged subscribers receive `RecvError::Lagged(n)` when they fall behind
    /// by more than `capacity` messages — they should reconnect.
    pub fn subscribe(&self) -> EventReceiver {
        EventReceiver {
            inner: self.sender.subscribe(),
        }
    }

    /// Number of currently active receivers.
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

// ─── EventReceiver ────────────────────────────────────────────────────────────

/// A receiver handle for one subscriber of the `EventBus`.
#[cfg(feature = "bus")]
pub struct EventReceiver {
    inner: broadcast::Receiver<RuntimeEvent>,
}

#[cfg(feature = "bus")]
impl EventReceiver {
    /// Receive the next event, waiting asynchronously.
    pub async fn recv(&mut self) -> Result<RuntimeEvent, broadcast::error::RecvError> {
        self.inner.recv().await
    }

    /// Try to receive an event without blocking.
    pub fn try_recv(&mut self) -> Result<RuntimeEvent, broadcast::error::TryRecvError> {
        self.inner.try_recv()
    }
}

// ─── EventBusSink ────────────────────────────────────────────────────────────

/// An `EventSink` backed by an `EventBus`.
///
/// Wraps `EventBus` so it can be passed to subsystems that expect an
/// `Arc<dyn EventSink>` rather than a direct `EventBus` reference.
/// The session_id must be provided at construction time and is embedded
/// in every event emitted through this sink.
#[cfg(feature = "bus")]
pub struct EventBusSink {
    bus: EventBus,
    session_id: Uuid,
}

#[cfg(feature = "bus")]
impl EventBusSink {
    pub fn new(bus: EventBus, session_id: Uuid) -> Self {
        Self { bus, session_id }
    }
}

#[cfg(feature = "bus")]
impl EventSink for EventBusSink {
    fn emit(&self, event: &RuntimeEvent) {
        // Re-emit through the bus, preserving the original event's kind.
        // This allows EventBusSink to act as a bridge between direct EventSink
        // callers and EventBus subscribers.
        let _ = self
            .bus
            .sender
            .send(RuntimeEvent::new(self.session_id, event.kind.clone()));
    }
}

// ─── NullEventBus (no-bus builds) ────────────────────────────────────────────

/// A zero-cost event bus stub for builds without the `bus` feature.
///
/// All emit calls are no-ops. Used in WASM builds or minimal library contexts.
#[cfg(not(feature = "bus"))]
#[derive(Clone, Debug, Default)]
pub struct EventBus;

#[cfg(not(feature = "bus"))]
impl EventBus {
    #[must_use]
    pub fn new(_capacity: usize) -> Self {
        Self
    }

    #[inline(always)]
    pub fn emit(&self, _session_id: Uuid, _kind: RuntimeEventKind) {}
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "bus"))]
mod tests {
    use super::*;
    use crate::event::RuntimeEventKind;

    #[tokio::test]
    async fn emit_and_receive() {
        let session = Uuid::new_v4();
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();

        bus.emit(
            session,
            RuntimeEventKind::SessionStarted {
                query_preview: "test query".into(),
                model: "claude-sonnet-4-6".into(),
                provider: "anthropic".into(),
                max_rounds: 10,
            },
        );

        let event = rx.recv().await.expect("should receive event");
        assert_eq!(event.session_id, session);
        assert_eq!(event.type_name(), "session_started");
    }

    #[tokio::test]
    async fn no_receiver_does_not_panic() {
        let session = Uuid::new_v4();
        let bus = EventBus::new(4);
        // No subscribers — emit should not panic.
        bus.emit(
            session,
            RuntimeEventKind::RoundStarted {
                round: 1,
                model: "claude-haiku-4-5-20251001".into(),
                tools_allowed: true,
                token_budget_remaining: 8192,
            },
        );
    }

    #[tokio::test]
    async fn multiple_subscribers_all_receive() {
        let session = Uuid::new_v4();
        let bus = EventBus::new(16);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.emit(
            session,
            RuntimeEventKind::BudgetWarning {
                tokens_used: 6500,
                tokens_total: 8000,
                pct_used: 0.81,
                time_elapsed_ms: 10_000,
                time_limit_ms: 120_000,
            },
        );

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        assert_eq!(e1.type_name(), "budget_warning");
        assert_eq!(e2.type_name(), "budget_warning");
        assert_eq!(e1.event_id, e2.event_id); // same event object broadcast
    }

    #[tokio::test]
    async fn receiver_count() {
        let bus = EventBus::new(4);
        assert_eq!(bus.receiver_count(), 0);
        let _rx1 = bus.subscribe();
        assert_eq!(bus.receiver_count(), 1);
        let _rx2 = bus.subscribe();
        assert_eq!(bus.receiver_count(), 2);
    }

    #[tokio::test]
    async fn event_bus_sink_bridges_to_bus() {
        let session = Uuid::new_v4();
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let sink = EventBusSink::new(bus, session);

        let ev = RuntimeEvent::new(
            session,
            RuntimeEventKind::SessionEnded {
                rounds_completed: 4,
                stop_condition: "end_turn".into(),
                total_tokens: 12_345,
                estimated_cost_usd: 0.003,
                duration_ms: 15_000,
                fingerprint: None,
            },
        );
        sink.emit(&ev);

        let received = rx.recv().await.unwrap();
        assert_eq!(received.type_name(), "session_ended");
    }
}
