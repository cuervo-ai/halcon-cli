use std::time::Duration;
use tokio::sync::mpsc;

use super::UiCommand;

/// Background worker that periodically sends refresh commands.
pub async fn run_poller(
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    // Skip the first immediate tick.
    ticker.tick().await;

    loop {
        ticker.tick().await;

        // Send all refresh commands. If channel is closed, stop.
        if cmd_tx.send(UiCommand::RefreshAgents).is_err() {
            break;
        }
        if cmd_tx.send(UiCommand::RefreshTasks).is_err() {
            break;
        }
        if cmd_tx.send(UiCommand::RefreshTools).is_err() {
            break;
        }
        if cmd_tx.send(UiCommand::RefreshMetrics).is_err() {
            break;
        }
        if cmd_tx.send(UiCommand::RefreshStatus).is_err() {
            break;
        }
    }
}
