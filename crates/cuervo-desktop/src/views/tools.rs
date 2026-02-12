use egui::{RichText, Ui};
use tokio::sync::mpsc;

use crate::state::AppState;
use crate::theme::CuervoTheme;
use crate::workers::UiCommand;

pub fn render(ui: &mut Ui, state: &AppState, cmd_tx: &mpsc::UnboundedSender<UiCommand>) {
    ui.horizontal(|ui| {
        ui.heading("Tools");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Refresh").clicked() {
                let _ = cmd_tx.send(UiCommand::RefreshTools);
            }
        });
    });
    ui.separator();

    if state.tools.is_empty() {
        ui.label(RichText::new("No tools registered").color(CuervoTheme::TEXT_MUTED));
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("tools_table")
            .num_columns(6)
            .striped(true)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Permission").strong());
                ui.label(RichText::new("Enabled").strong());
                ui.label(RichText::new("Executions").strong());
                ui.label(RichText::new("Last Run").strong());
                ui.label(RichText::new("Actions").strong());
                ui.end_row();

                for tool in &state.tools {
                    ui.label(&tool.name);

                    let perm_color = match tool.permission_level {
                        cuervo_api::types::tool::PermissionLevel::ReadOnly => CuervoTheme::SUCCESS,
                        cuervo_api::types::tool::PermissionLevel::ReadWrite => CuervoTheme::WARNING,
                        cuervo_api::types::tool::PermissionLevel::Destructive => CuervoTheme::ERROR,
                    };
                    ui.colored_label(perm_color, format!("{:?}", tool.permission_level));

                    if tool.enabled {
                        ui.colored_label(CuervoTheme::SUCCESS, "Yes");
                    } else {
                        ui.colored_label(CuervoTheme::ERROR, "No");
                    }

                    ui.label(tool.execution_count.to_string());

                    ui.label(
                        tool.last_executed
                            .map(|t| t.format("%H:%M:%S").to_string())
                            .unwrap_or_else(|| "-".to_string()),
                    );

                    ui.horizontal(|ui| {
                        let label = if tool.enabled { "Disable" } else { "Enable" };
                        if ui.small_button(label).clicked() {
                            let _ = cmd_tx.send(UiCommand::ToggleTool {
                                name: tool.name.clone(),
                                enabled: !tool.enabled,
                            });
                        }
                    });

                    ui.end_row();
                }
            });
    });
}
