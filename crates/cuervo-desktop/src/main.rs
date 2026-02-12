mod app;
mod config;
mod state;
mod theme;
mod views;
mod widgets;
mod workers;

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn main() -> eframe::Result<()> {
    // Initialize tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Create channels for UI <-> backend communication.
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (msg_tx, msg_rx) = mpsc::unbounded_channel();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 600.0])
            .with_title("Cuervo Control Plane"),
        ..Default::default()
    };

    // Spawn the tokio runtime for background workers.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to create tokio runtime");

    let cmd_tx_clone = cmd_tx.clone();

    eframe::run_native(
        "Cuervo Control Plane",
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let repaint = Arc::new(move || ctx.request_repaint());

            // Start background connection worker.
            rt.spawn(workers::connection::run_connection_worker(
                cmd_rx,
                msg_tx,
                repaint,
            ));

            // Start periodic poller.
            rt.spawn(workers::poller::run_poller(
                cmd_tx_clone,
                Duration::from_secs(5),
            ));

            // Pass the runtime to the app so it stays alive for the app's lifetime.
            Ok(Box::new(app::CuervoApp::new(cc, cmd_tx, msg_rx, rt)))
        }),
    )
}
