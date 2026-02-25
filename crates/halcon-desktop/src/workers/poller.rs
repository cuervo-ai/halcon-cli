use std::time::Duration;
use tokio::sync::mpsc;

use super::UiCommand;

/// Background worker that periodically sends refresh commands.
pub async fn run_poller(
    cmd_tx: mpsc::Sender<UiCommand>,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    // Skip the first immediate tick.
    ticker.tick().await;

    loop {
        ticker.tick().await;

        // Send all refresh commands. If channel is closed, stop.
        // .await provides backpressure: if the UI is busy the poller waits here.
        if cmd_tx.send(UiCommand::RefreshAgents).await.is_err() {
            break;
        }
        if cmd_tx.send(UiCommand::RefreshTasks).await.is_err() {
            break;
        }
        if cmd_tx.send(UiCommand::RefreshTools).await.is_err() {
            break;
        }
        if cmd_tx.send(UiCommand::RefreshMetrics).await.is_err() {
            break;
        }
        if cmd_tx.send(UiCommand::RefreshStatus).await.is_err() {
            break;
        }
    }
}
