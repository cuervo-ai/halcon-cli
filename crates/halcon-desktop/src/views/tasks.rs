use egui::{RichText, Ui};
use tokio::sync::mpsc;

use crate::state::AppState;
use crate::theme::HalconTheme;
use crate::workers::UiCommand;

pub fn render(ui: &mut Ui, state: &mut AppState, cmd_tx: &mpsc::Sender<UiCommand>) {
    ui.horizontal(|ui| {
        ui.heading("Tasks");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Refresh").clicked() {
                let _ = cmd_tx.try_send(UiCommand::RefreshTasks);
            }
        });
    });
    ui.separator();

    // ── Submit task form (always visible) ────────────────────────────────────
    ui.collapsing("Submit New Task", |ui| {
        ui.label(
            RichText::new("Instruction:")
                .size(11.0)
                .color(HalconTheme::TEXT_SECONDARY),
        );
        let edit = egui::TextEdit::singleline(&mut state.ops.submit_task_input)
            .hint_text("Describe the task for the agent…")
            .desired_width(ui.available_width() - 72.0);
        let resp = ui.add(edit);
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        ui.horizontal(|ui| {
            let can_submit = !state.ops.submit_task_input.trim().is_empty();
            if (ui.add_enabled(can_submit, egui::Button::new("Submit")).clicked() || enter)
                && can_submit
            {
                let _ = cmd_tx.try_send(UiCommand::SubmitTask {
                    instruction: state.ops.submit_task_input.trim().to_string(),
                    agent_id: None, // server picks best agent
                });
                state.ops.submit_task_input.clear();
                state.ops.error = None;
            }
        });

        if let Some(ref err) = state.ops.error {
            ui.add_space(4.0);
            ui.colored_label(HalconTheme::ERROR, format!("⚠  {err}"));
        }
    });

    ui.add_space(4.0);
    ui.separator();

    if state.tasks.is_empty() {
        ui.label(RichText::new("No task executions").color(HalconTheme::TEXT_MUTED));
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
                    let color = HalconTheme::task_status_color(&status_str);
                    ui.colored_label(color, &status_str);

                    ui.label(task.wave_count.to_string());
                    ui.label(task.node_results.len().to_string());
                    ui.label(task.submitted_at.format("%H:%M:%S").to_string());

                    ui.horizontal(|ui| {
                        let is_active = task.status
                            == halcon_api::types::task::TaskStatus::Running
                            || task.status == halcon_api::types::task::TaskStatus::Pending;
                        if is_active
                            && ui
                                .small_button(
                                    RichText::new("Cancel").color(HalconTheme::WARNING),
                                )
                                .clicked()
                        {
                            let _ = cmd_tx.try_send(UiCommand::CancelTask(task.id));
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
                                let color = HalconTheme::task_status_color(
                                    &format!("{:?}", nr.status).to_lowercase(),
                                );
                                ui.colored_label(
                                    color,
                                    format!("{}: {:?}", &nr.task_id.to_string()[..8], nr.status),
                                );
                                if let Some(ref err) = nr.error {
                                    ui.colored_label(HalconTheme::ERROR, err);
                                }
                            });
                        }
                    });
                }

                // DAG visualization — only shown when there are multiple nodes
                // so the graph is non-trivial. Single-node tasks are not graphs.
                if task.node_results.len() > 1 {
                    ui.collapsing("Task Graph", |ui| {
                        let nodes = crate::widgets::dag_viewer::nodes_from_task(task);
                        // ScrollArea::both allows horizontal scrolling for wide graphs.
                        egui::ScrollArea::both()
                            .id_salt("task_dag_scroll")
                            .max_height(200.0)
                            .show(ui, |ui| {
                                crate::widgets::dag_viewer::render_dag(ui, &nodes);
                            });
                    });
                }
            });
        }
    }
}
