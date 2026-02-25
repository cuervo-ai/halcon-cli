//! WebSocket event loop: subscribes to all channels, forwards events as
//! [`BackendMessage`], and sends keepalive pings to detect zombie connections.

use halcon_api::types::ws::WsChannel;
use halcon_client::EventStream;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use super::{BackendMessage, RepaintFn};
use super::ws_translator::translate_ws_event;

/// Background task: receive WebSocket events and forward them as `BackendMessage`.
///
/// Subscribes to all chat-relevant channels immediately after connecting.
/// Translates high-priority chat events into typed `BackendMessage` variants;
/// everything else is forwarded as `BackendMessage::Event`.
pub async fn run_ws_event_loop(
    mut stream: EventStream,
    msg_tx: mpsc::Sender<BackendMessage>,
    repaint: RepaintFn,
) {
    // Subscribe to each channel individually so events arrive exactly once.
    //
    // WsChannel::All was previously included alongside specific channels, which
    // caused every event to be delivered TWICE — once through its specific channel
    // and once through All. This doubled channel pressure and inflated the event
    // buffer at 2× the real event rate.
    //
    // The five channels that were previously covered only by All are now explicit:
    // Tasks, Tools, Metrics, Protocols, System.
    let channels = vec![
        WsChannel::Chat,        // ChatStreamToken, ThinkingProgress, ConversationCompleted, session lifecycle
        WsChannel::Permissions, // PermissionRequired, PermissionResolved
        WsChannel::Execution,   // ExecutionFailed
        WsChannel::SubAgents,   // SubAgentStarted, SubAgentCompleted
        WsChannel::Agents,      // AgentRegistered/Deregistered/HealthChanged/Invoked/Completed
        WsChannel::Tasks,       // TaskSubmitted, TaskProgress, TaskCompleted
        WsChannel::Tools,       // ToolExecuted
        WsChannel::Logs,        // Log(LogEntry)
        WsChannel::Metrics,     // Metric(MetricPoint)
        WsChannel::Protocols,   // Protocol(ProtocolMessageInfo)
        WsChannel::System,      // ConfigChanged, SystemHealthChanged, Error, Pong, Connected
    ];
    if let Err(e) = stream.subscribe(channels) {
        tracing::warn!(error = %e, "WS subscribe failed");
    }

    // ── Keepalive ──────────────────────────────────────────────────────────────
    // A ping is sent whenever no event arrives for KEEPALIVE_SECS (30s).  This
    // prevents NAT/firewall/proxy timeouts on idle connections.
    //
    // Two failure modes are handled:
    //   1. Ping channel closed: the underlying WS task has already exited →
    //      treat immediately as disconnect.
    //   2. Zombie connection: ping() succeeds (cmd_tx still open) but the remote
    //      end is silent — OS TCP socket is stale without a FIN reaching us.
    //      Detected via last_activity: if elapsed > ZOMBIE_THRESHOLD the loop
    //      breaks and the auto-reconnect in app.rs fires after its 5s back-off.
    //
    // tokio::time::timeout wraps next_event() rather than using tokio::select!
    // because next_event() takes &mut self while ping() takes &self; simultaneous
    // borrows inside select! arms would not compile.  timeout fires only when the
    // connection is idle, so active-traffic overhead is zero.
    const KEEPALIVE_SECS: u64 = 30;
    // 2.5× the ping interval: gives the server time to respond before we declare
    // the connection dead.  Must be strictly greater than KEEPALIVE_SECS.
    const ZOMBIE_THRESHOLD_SECS: u64 = 75;

    let mut last_activity = Instant::now();
    // C2: Track dropped BackendMessage events (channel full) and log them at warn
    // level after every DROP_LOG_INTERVAL consecutive drops to avoid spam but
    // ensure ops teams see sustained backpressure.
    const DROP_LOG_INTERVAL: u64 = 50;
    let mut dropped_total: u64 = 0;

    loop {
        match tokio::time::timeout(
            Duration::from_secs(KEEPALIVE_SECS),
            stream.next_event(),
        )
        .await
        {
            // ── Normal event — resets the zombie clock ───────────────────────
            Ok(Some(event)) => {
                last_activity = Instant::now();
                let msg = translate_ws_event(event);
                if msg_tx.try_send(msg).is_err() {
                    // Channel full — egui frame loop is behind the WS event rate.
                    dropped_total += 1;
                    if dropped_total % DROP_LOG_INTERVAL == 0 {
                        tracing::warn!(
                            dropped_total,
                            "BackendMessage channel full — {} events dropped (UI thread lagging)",
                            dropped_total
                        );
                    } else {
                        tracing::trace!(dropped_total, "BackendMessage channel full; WS event dropped");
                    }
                }
                (repaint)();
            }

            // ── Clean close from server ──────────────────────────────────────
            Ok(None) => {
                tracing::info!("WebSocket event stream closed");
                let _ = msg_tx.try_send(BackendMessage::Disconnected(
                    "WebSocket stream closed".into(),
                ));
                (repaint)();
                break;
            }

            // ── Keepalive window expired — no events for KEEPALIVE_SECS ─────
            Err(_timeout) => {
                let idle_secs = last_activity.elapsed().as_secs();

                // Zombie guard: connection has been silent beyond the threshold.
                // ping() would succeed (cmd_tx is still open) but the remote end
                // is unreachable — break now so we don't loop indefinitely.
                if idle_secs >= ZOMBIE_THRESHOLD_SECS {
                    tracing::warn!(
                        idle_secs,
                        threshold_secs = ZOMBIE_THRESHOLD_SECS,
                        "WS zombie detected — no activity; declaring disconnect"
                    );
                    let _ = msg_tx.try_send(BackendMessage::Disconnected(
                        "heartbeat timeout".into(),
                    ));
                    (repaint)();
                    break;
                }

                // Connection is just idle — send a keepalive ping.
                if let Err(e) = stream.ping() {
                    // Ping channel closed → underlying WS task has already exited.
                    tracing::warn!(
                        error = %e,
                        "WS keepalive ping failed — treating as disconnect"
                    );
                    let _ = msg_tx.try_send(BackendMessage::Disconnected(
                        "WebSocket keepalive failed".into(),
                    ));
                    (repaint)();
                    break;
                }
                tracing::trace!(secs = KEEPALIVE_SECS, idle_secs, "WS keepalive ping sent");
            }
        }
    }
}
