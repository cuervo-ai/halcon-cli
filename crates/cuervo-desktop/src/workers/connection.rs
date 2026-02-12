use cuervo_client::{ClientConfig, CuervoClient};
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{BackendMessage, UiCommand};

/// Background worker that manages the client connection and processes commands.
pub async fn run_connection_worker(
    mut cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
    msg_tx: mpsc::UnboundedSender<BackendMessage>,
    repaint: Arc<dyn Fn() + Send + Sync>,
) {
    let mut client: Option<CuervoClient> = None;

    tracing::info!("connection worker started");

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            UiCommand::Connect { url, token } => {
                tracing::info!(url = %url, "connecting");
                let config = ClientConfig::new(&url, &token);
                match CuervoClient::new(config) {
                    Ok(c) => match c.health_check().await {
                        Ok(true) => {
                            tracing::info!("connected");
                            let _ = msg_tx.send(BackendMessage::Connected);
                            client = Some(c);
                            (repaint)();
                        }
                        Ok(false) => {
                            let _ = msg_tx.send(BackendMessage::ConnectionError(
                                "health check failed".into(),
                            ));
                            (repaint)();
                        }
                        Err(e) => {
                            let _ = msg_tx.send(BackendMessage::ConnectionError(format!(
                                "connection failed: {e}"
                            )));
                            (repaint)();
                        }
                    },
                    Err(e) => {
                        let _ = msg_tx.send(BackendMessage::ConnectionError(e.to_string()));
                        (repaint)();
                    }
                }
            }
            UiCommand::Disconnect => {
                client = None;
                let _ = msg_tx.send(BackendMessage::Disconnected("user disconnected".into()));
                (repaint)();
            }
            UiCommand::RefreshAgents => {
                if let Some(ref c) = client {
                    match c.list_agents().await {
                        Ok(agents) => {
                            let _ = msg_tx.send(BackendMessage::AgentsUpdated(agents));
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to refresh agents");
                        }
                    }
                    (repaint)();
                }
            }
            UiCommand::RefreshTasks => {
                if let Some(ref c) = client {
                    match c.list_tasks().await {
                        Ok(tasks) => {
                            let _ = msg_tx.send(BackendMessage::TasksUpdated(tasks));
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to refresh tasks");
                        }
                    }
                    (repaint)();
                }
            }
            UiCommand::RefreshTools => {
                if let Some(ref c) = client {
                    match c.list_tools().await {
                        Ok(tools) => {
                            let _ = msg_tx.send(BackendMessage::ToolsUpdated(tools));
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to refresh tools");
                        }
                    }
                    (repaint)();
                }
            }
            UiCommand::RefreshMetrics => {
                if let Some(ref c) = client {
                    match c.metrics().await {
                        Ok(m) => {
                            let _ = msg_tx.send(BackendMessage::MetricsUpdated(m));
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to refresh metrics");
                        }
                    }
                    (repaint)();
                }
            }
            UiCommand::RefreshStatus => {
                if let Some(ref c) = client {
                    match c.system_status().await {
                        Ok(s) => {
                            let _ = msg_tx.send(BackendMessage::SystemStatusUpdated(s));
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to refresh status");
                        }
                    }
                    (repaint)();
                }
            }
            UiCommand::RefreshConfig => {
                if let Some(ref c) = client {
                    match c.get_config().await {
                        Ok(cfg) => {
                            let _ = msg_tx.send(BackendMessage::ConfigLoaded(cfg));
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to refresh config");
                            let _ =
                                msg_tx.send(BackendMessage::ConfigError(e.to_string()));
                        }
                    }
                    (repaint)();
                }
            }
            UiCommand::UpdateConfig(update) => {
                if let Some(ref c) = client {
                    match c.update_config(*update).await {
                        Ok(cfg) => {
                            let _ = msg_tx.send(BackendMessage::ConfigUpdated(cfg));
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to update config");
                            let _ =
                                msg_tx.send(BackendMessage::ConfigError(e.to_string()));
                        }
                    }
                    (repaint)();
                }
            }
            UiCommand::StopAgent(id) => {
                if let Some(ref c) = client {
                    if let Err(e) = c.stop_agent(id).await {
                        tracing::warn!(error = %e, agent = %id, "failed to stop agent");
                    }
                    (repaint)();
                }
            }
            UiCommand::CancelTask(id) => {
                if let Some(ref c) = client {
                    if let Err(e) = c.cancel_task(id).await {
                        tracing::warn!(error = %e, task = %id, "failed to cancel task");
                    }
                    (repaint)();
                }
            }
            UiCommand::ToggleTool { name, enabled } => {
                if let Some(ref c) = client {
                    if let Err(e) = c.toggle_tool(&name, enabled).await {
                        tracing::warn!(error = %e, tool = %name, "failed to toggle tool");
                    }
                    (repaint)();
                }
            }
            UiCommand::Shutdown { graceful } => {
                if let Some(ref c) = client {
                    if let Err(e) = c.shutdown(graceful, None).await {
                        tracing::warn!(error = %e, "failed to shutdown");
                    }
                    (repaint)();
                }
            }
        }
    }
}
