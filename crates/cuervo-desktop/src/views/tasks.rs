use egui::{RichText, Ui};
use tokio::sync::mpsc;

use crate::state::AppState;
use crate::theme::CuervoTheme;
use crate::workers::UiCommand;

pub fn render(ui: &mut Ui, state: &mut AppState, cmd_tx: &mpsc::UnboundedSender<UiCommand>) {
    ui.horizontal(|ui| {
        ui.heading("Tasks");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Refresh").clicked() {
                let _ = cmd_tx.send(UiCommand::RefreshTasks);
            }
        });
    });
    ui.separator();

    if state.tasks.is_empty() {
        ui.label(RichText::new("No task executions").color(CuervoTheme::TEXT_MUTED));
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("tasks_table")
            .num_columns(6)
            .striped(true)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label(RichText::new("ID").strong());
                ui.label(RichText::new("Status").strong());
                ui.label(RichText::new("Waves").strong());
                ui.label(RichText::new("Nodes").strong());
                ui.label(RichText::new("Submitted").strong());
                ui.label(RichText::new("Actions").strong());
                ui.end_row();

                for task in &state.tasks {
                    let selected = state.selected_task == Some(task.id);
                    if ui
                        .selectable_label(selected, &task.id.to_string()[..8])
                        .clicked()
                    {
                        state.selected_task = if selected { None } else { Some(task.id) };
                    }

                    let status_str = format!("{:?}", task.status).to_lowercase();
                    let color = CuervoTheme::task_status_color(&status_str);
                    ui.colored_label(color, &status_str);

                    ui.label(task.wave_count.to_string());
                    ui.label(task.node_results.len().to_string());
                    ui.label(task.submitted_at.format("%H:%M:%S").to_string());

                    ui.horizontal(|ui| {
                        let is_active = task.status
                            == cuervo_api::types::task::TaskStatus::Running
                            || task.status == cuervo_api::types::task::TaskStatus::Pending;
                        if is_active
                            && ui
                                .small_button(
                                    RichText::new("Cancel").color(CuervoTheme::WARNING),
                                )
                                .clicked()
                        {
                            let _ = cmd_tx.send(UiCommand::CancelTask(task.id));
                        }
                    });

                    ui.end_row();
                }
            });
    });

    // Detail panel.
    if let Some(selected_id) = state.selected_task {
        if let Some(task) = state.tasks.iter().find(|t| t.id == selected_id) {
            ui.add_space(12.0);
            ui.separator();
            ui.group(|ui| {
                ui.label(RichText::new(format!("Task: {}", task.id)).strong());
                ui.label(format!("Status: {:?}", task.status));
                ui.label(format!("Waves: {}", task.wave_count));
                ui.label(format!(
                    "Usage: {}in / {}out tokens, ${:.4}",
                    task.total_usage.input_tokens,
                    task.total_usage.output_tokens,
                    task.total_usage.cost_usd
                ));

                if !task.node_results.is_empty() {
                    ui.collapsing("Node Results", |ui| {
                        for nr in &task.node_results {
                            ui.horizontal(|ui| {
                                let color = CuervoTheme::task_status_color(
                                    &format!("{:?}", nr.status).to_lowercase(),
                                );
                                ui.colored_label(
                                    color,
                                    format!("{}: {:?}", &nr.task_id.to_string()[..8], nr.status),
                                );
                                if let Some(ref err) = nr.error {
                                    ui.colored_label(CuervoTheme::ERROR, err);
                                }
                            });
                        }
                    });
                }
            });
        }
    }
}
