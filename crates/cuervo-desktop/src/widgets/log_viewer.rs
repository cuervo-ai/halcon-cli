use egui::{RichText, Ui};
use std::collections::VecDeque;

use crate::theme::CuervoTheme;
use cuervo_api::types::observability::LogEntry;

/// Render a compact log viewer widget.
pub fn render_log_viewer(ui: &mut Ui, logs: &VecDeque<LogEntry>, max_lines: usize) {
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .stick_to_bottom(true)
        .max_height(300.0)
        .show(ui, |ui| {
            let start = if logs.len() > max_lines {
                logs.len() - max_lines
            } else {
                0
            };

            for entry in logs.iter().skip(start) {
                let level_color = match entry.level {
                    cuervo_api::types::observability::LogLevel::Error => CuervoTheme::ERROR,
                    cuervo_api::types::observability::LogLevel::Warn => CuervoTheme::WARNING,
                    cuervo_api::types::observability::LogLevel::Info => CuervoTheme::INFO,
                    cuervo_api::types::observability::LogLevel::Debug => CuervoTheme::TEXT_SECONDARY,
                    cuervo_api::types::observability::LogLevel::Trace => CuervoTheme::TEXT_MUTED,
                };

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(entry.timestamp.format("%H:%M:%S").to_string())
                            .monospace()
                            .size(10.0)
                            .color(CuervoTheme::TEXT_MUTED),
                    );
                    ui.colored_label(
                        level_color,
                        RichText::new(format!("{:5?}", entry.level))
                            .monospace()
                            .size(10.0),
                    );
                    ui.label(
                        RichText::new(&entry.message)
                            .monospace()
                            .size(10.0),
                    );
                });
            }
        });
}
