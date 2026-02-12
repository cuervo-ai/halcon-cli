use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashSet;

use super::state::AppState;
use crate::types::ws::{WsChannel, WsClientMessage, WsServerEvent};

/// WebSocket upgrade handler.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_connection(socket, state))
}

async fn handle_ws_connection(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // Send connected event.
    let connected = WsServerEvent::Connected {
        server_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    if let Ok(json) = serde_json::to_string(&connected) {
        let _ = sender.send(Message::Text(json)).await;
    }

    // Subscribe to broadcast channel.
    let mut event_rx = state.event_tx.subscribe();
    let mut subscribed_channels: HashSet<WsChannel> = HashSet::new();
    // Default: subscribe to all.
    subscribed_channels.insert(WsChannel::All);

    loop {
        tokio::select! {
            // Incoming messages from client.
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(client_msg) = serde_json::from_str::<WsClientMessage>(&text) {
                            match client_msg {
                                WsClientMessage::Subscribe { channels } => {
                                    for ch in channels {
                                        subscribed_channels.insert(ch);
                                    }
                                }
                                WsClientMessage::Unsubscribe { channels } => {
                                    for ch in &channels {
                                        subscribed_channels.remove(ch);
                                    }
                                }
                                WsClientMessage::Ping => {
                                    let pong = serde_json::to_string(&WsServerEvent::Pong)
                                        .unwrap_or_default();
                                    if sender.send(Message::Text(pong)).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            // Outgoing events from server.
            event = event_rx.recv() => {
                match event {
                    Ok(ev) if should_forward(&ev, &subscribed_channels) => {
                        if let Ok(json) = serde_json::to_string(&ev) {
                            if sender.send(Message::Text(json)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "ws client lagged, dropped events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        }
    }

    tracing::debug!("ws connection closed");
}

/// Determine if an event should be forwarded based on subscriptions.
fn should_forward(event: &WsServerEvent, channels: &HashSet<WsChannel>) -> bool {
    if channels.contains(&WsChannel::All) {
        return true;
    }
    match event {
        WsServerEvent::AgentRegistered { .. }
        | WsServerEvent::AgentDeregistered { .. }
        | WsServerEvent::AgentHealthChanged { .. }
        | WsServerEvent::AgentInvoked { .. }
        | WsServerEvent::AgentCompleted { .. } => channels.contains(&WsChannel::Agents),

        WsServerEvent::TaskSubmitted { .. }
        | WsServerEvent::TaskProgress(_)
        | WsServerEvent::TaskCompleted { .. } => channels.contains(&WsChannel::Tasks),

        WsServerEvent::ToolExecuted { .. } => channels.contains(&WsChannel::Tools),

        WsServerEvent::Log(_) => channels.contains(&WsChannel::Logs),
        WsServerEvent::Metric(_) => channels.contains(&WsChannel::Metrics),
        WsServerEvent::Protocol(_) => channels.contains(&WsChannel::Protocols),

        WsServerEvent::ConfigChanged { .. }
        | WsServerEvent::SystemHealthChanged { .. } => channels.contains(&WsChannel::System),

        WsServerEvent::Error { .. }
        | WsServerEvent::Pong
        | WsServerEvent::Connected { .. } => true,
    }
}
