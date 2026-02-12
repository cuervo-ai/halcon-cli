use egui::{RichText, Ui};

use crate::state::AppState;
use crate::theme::CuervoTheme;

pub fn render(ui: &mut Ui, state: &AppState) {
    ui.heading("Protocol Inspector");
    ui.separator();

    // Filter protocol messages from events.
    let protocol_events: Vec<_> = state
        .events
        .iter()
        .filter_map(|e| match e {
            cuervo_api::types::ws::WsServerEvent::Protocol(msg) => Some(msg),
            _ => None,
        })
        .collect();

    if protocol_events.is_empty() {
        ui.label(
            RichText::new("No protocol messages captured yet")
                .color(CuervoTheme::TEXT_MUTED),
        );
        ui.add_space(8.0);
        ui.label("Protocol messages (MCP, A2A, Federation) will appear here as they flow through the runtime.");
        return;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            for msg in protocol_events.iter().rev().take(100) {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(msg.timestamp.format("%H:%M:%S%.3f").to_string())
                                .monospace()
                                .size(11.0)
                                .color(CuervoTheme::TEXT_MUTED),
                        );
                        ui.colored_label(CuervoTheme::ACCENT, format!("{:?}", msg.protocol));
                        ui.colored_label(
                            CuervoTheme::TEXT_SECONDARY,
                            format!("{:?}", msg.direction),
                        );
                        ui.label(&msg.message_type);
                        ui.label(format!("{}B", msg.payload_size_bytes));
                        if let Some(latency) = msg.latency_ms {
                            ui.label(format!("{latency}ms"));
                        }
                    });
                    ui.collapsing("Payload", |ui| {
                        let json = serde_json::to_string_pretty(&msg.payload)
                            .unwrap_or_else(|_| msg.payload.to_string());
                        ui.label(
                            RichText::new(json)
                                .monospace()
                                .size(11.0)
                                .color(CuervoTheme::TEXT_SECONDARY),
                        );
                    });
                });
            }
        });
}
