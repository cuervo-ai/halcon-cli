use egui::{RichText, Ui};

use crate::state::AppState;
use crate::theme::CuervoTheme;

pub fn render(ui: &mut Ui, state: &mut AppState) {
    ui.heading("Logs");
    ui.separator();

    // Filters.
    ui.horizontal(|ui| {
        ui.label("Search:");
        ui.text_edit_singleline(&mut state.log_search);
        ui.separator();
        ui.label("Level:");
        egui::ComboBox::from_id_salt("log_level_filter")
            .selected_text(if state.log_level_filter.is_empty() {
                "All"
            } else {
                &state.log_level_filter
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.log_level_filter, String::new(), "All");
                ui.selectable_value(&mut state.log_level_filter, "error".into(), "Error");
                ui.selectable_value(&mut state.log_level_filter, "warn".into(), "Warn");
                ui.selectable_value(&mut state.log_level_filter, "info".into(), "Info");
                ui.selectable_value(&mut state.log_level_filter, "debug".into(), "Debug");
                ui.selectable_value(&mut state.log_level_filter, "trace".into(), "Trace");
            });
        ui.separator();
        ui.label(format!("{} entries", state.logs.len()));
    });

    ui.add_space(4.0);

    // Log entries.
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for entry in state.logs.iter() {
                // Apply filters.
                if !state.log_level_filter.is_empty() {
                    let level_str = format!("{:?}", entry.level).to_lowercase();
                    if level_str != state.log_level_filter {
                        continue;
                    }
                }
                if !state.log_search.is_empty()
                    && !entry
                        .message
                        .to_lowercase()
                        .contains(&state.log_search.to_lowercase())
                {
                    continue;
                }

                let level_color = match entry.level {
                    cuervo_api::types::observability::LogLevel::Error => CuervoTheme::ERROR,
                    cuervo_api::types::observability::LogLevel::Warn => CuervoTheme::WARNING,
                    cuervo_api::types::observability::LogLevel::Info => CuervoTheme::INFO,
                    cuervo_api::types::observability::LogLevel::Debug => CuervoTheme::TEXT_SECONDARY,
                    cuervo_api::types::observability::LogLevel::Trace => CuervoTheme::TEXT_MUTED,
                };

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(entry.timestamp.format("%H:%M:%S%.3f").to_string())
                            .monospace()
                            .size(11.0)
                            .color(CuervoTheme::TEXT_MUTED),
                    );
                    ui.colored_label(
                        level_color,
                        RichText::new(format!("{:5?}", entry.level))
                            .monospace()
                            .size(11.0),
                    );
                    ui.label(
                        RichText::new(&entry.target)
                            .monospace()
                            .size(11.0)
                            .color(CuervoTheme::TEXT_SECONDARY),
                    );
                    ui.label(
                        RichText::new(&entry.message)
                            .monospace()
                            .size(11.0),
                    );
                });
            }
        });
}
