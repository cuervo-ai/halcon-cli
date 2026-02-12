use cuervo_api::types::ws::{WsChannel, WsClientMessage, WsServerEvent};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::config::ClientConfig;
use crate::error::ClientError;

/// A WebSocket event stream connection to the control plane.
pub struct EventStream {
    /// Channel receiving server events.
    pub rx: mpsc::UnboundedReceiver<WsServerEvent>,
    /// Handle to the background connection task.
    _task: tokio::task::JoinHandle<()>,
    /// Channel for sending commands to the WebSocket.
    cmd_tx: mpsc::UnboundedSender<WsClientMessage>,
}

impl EventStream {
    /// Connect to the WebSocket event stream.
    pub async fn connect(config: &ClientConfig) -> Result<Self, ClientError> {
        let ws_url = config.ws_url();
        let (ws_stream, _response) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| ClientError::WebSocket(e.to_string()))?;

        let (mut ws_sink, mut ws_source) = ws_stream.split();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<WsClientMessage>();

        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Forward incoming WebSocket messages to the event channel.
                    msg = ws_source.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                if let Ok(event) = serde_json::from_str::<WsServerEvent>(&text) {
                                    if event_tx.send(event).is_err() {
                                        break;
                                    }
                                }
                            }
                            Some(Ok(Message::Close(_))) | None => break,
                            _ => {}
                        }
                    }
                    // Forward outgoing commands to the WebSocket.
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(msg) => {
                                if let Ok(json) = serde_json::to_string(&msg) {
                                    if ws_sink.send(Message::Text(json)).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        Ok(Self {
            rx: event_rx,
            _task: task,
            cmd_tx,
        })
    }

    /// Subscribe to specific event channels.
    pub fn subscribe(&self, channels: Vec<WsChannel>) -> Result<(), ClientError> {
        self.cmd_tx
            .send(WsClientMessage::Subscribe { channels })
            .map_err(|_| ClientError::NotConnected)
    }

    /// Unsubscribe from event channels.
    pub fn unsubscribe(&self, channels: Vec<WsChannel>) -> Result<(), ClientError> {
        self.cmd_tx
            .send(WsClientMessage::Unsubscribe { channels })
            .map_err(|_| ClientError::NotConnected)
    }

    /// Send a ping to keep the connection alive.
    pub fn ping(&self) -> Result<(), ClientError> {
        self.cmd_tx
            .send(WsClientMessage::Ping)
            .map_err(|_| ClientError::NotConnected)
    }

    /// Receive the next event (async).
    pub async fn next_event(&mut self) -> Option<WsServerEvent> {
        self.rx.recv().await
    }
}
