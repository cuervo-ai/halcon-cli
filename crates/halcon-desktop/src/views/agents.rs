use egui::{RichText, Ui};
use tokio::sync::mpsc;

use crate::state::AppState;
use crate::theme::HalconTheme;
use crate::workers::UiCommand;

pub fn render(ui: &mut Ui, state: &mut AppState, cmd_tx: &mpsc::Sender<UiCommand>) {
    ui.horizontal(|ui| {
        ui.heading("Agents");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Refresh").clicked() {
                let _ = cmd_tx.try_send(UiCommand::RefreshAgents);
            }
        });
    });
    ui.separator();

    if state.agents.is_empty() {
        ui.label(RichText::new("No agents registered").color(HalconTheme::TEXT_MUTED));
        return;
    }

    // Agent table.
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("agents_table")
            .num_columns(7)
            .striped(true)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                // Header.
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Kind").strong());
                ui.label(RichText::new("Health").strong());
                ui.label(RichText::new("Capabilities").strong());
                ui.label(RichText::new("Invocations").strong());
                ui.label(RichText::new("Concurrency").strong());
                ui.label(RichText::new("Actions").strong());
                ui.end_row();

                for agent in &state.agents {
                    // Name.
                    let selected = state.selected_agent == Some(agent.id);
                    if ui
                        .selectable_label(selected, &agent.name)
                        .clicked()
                    {
                        state.selected_agent = if selected { None } else { Some(agent.id) };
                    }

                    // Kind.
                    ui.label(format!("{:?}", agent.kind));

                    // Health.
                    let health_str = match &agent.health {
                        halcon_api::types::agent::HealthStatus::Healthy => "Healthy",
                        halcon_api::types::agent::HealthStatus::Degraded { .. } => "Degraded",
                        halcon_api::types::agent::HealthStatus::Unavailable { .. } => "Unavailable",
                        halcon_api::types::agent::HealthStatus::Unknown => "Unknown",
                    };
                    let color = HalconTheme::health_color(&health_str.to_lowercase());
                    ui.colored_label(color, health_str);

                    // Capabilities.
                    let caps = agent
                        .capabilities
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    ui.label(
                        RichText::new(caps)
                            .size(11.0)
                            .color(HalconTheme::TEXT_SECONDARY),
                    );

                    // Invocations.
                    ui.label(agent.invocation_count.to_string());

                    // Concurrency.
                    ui.label(agent.max_concurrency.to_string());

                    // Actions.
                    ui.horizontal(|ui| {
                        if ui
                            .small_button(RichText::new("Stop").color(HalconTheme::ERROR))
                            .clicked()
                        {
                            let _ = cmd_tx.try_send(UiCommand::StopAgent(agent.id));
                        }
                    });

                    ui.end_row();
                }
            });
    });

    // Detail panel for selected agent.
    if let Some(selected_id) = state.selected_agent {
        if let Some(agent) = state.agents.iter().find(|a| a.id == selected_id) {
            ui.add_space(12.0);
            ui.separator();
            ui.group(|ui| {
                ui.label(RichText::new(format!("Agent: {}", agent.name)).strong());
                ui.label(format!("ID: {}", agent.id));
                ui.label(format!("Kind: {:?}", agent.kind));
                ui.label(format!("Health: {:?}", agent.health));
                ui.label(format!("Protocols: {:?}", agent.protocols));
                ui.label(format!(
                    "Capabilities: {}",
                    agent.capabilities.join(", ")
                ));
                if !agent.metadata.is_empty() {
                    ui.collapsing("Metadata", |ui| {
                        for (k, v) in &agent.metadata {
                            ui.label(format!("{k}: {v}"));
                        }
                    });
                }

                // ── Invoke panel ─────────────────────────────────────────────
                ui.add_space(8.0);
                ui.collapsing("Invoke Agent", |ui| {
                    ui.label(
                        RichText::new("Instruction:")
                            .size(11.0)
                            .color(HalconTheme::TEXT_SECONDARY),
                    );
                    let edit = egui::TextEdit::singleline(&mut state.ops.invoke_agent_input)
                        .hint_text("Enter instruction for this agent…")
                        .desired_width(ui.available_width() - 72.0);
                    let resp = ui.add(edit);
                    let enter = resp.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));

                    ui.horizontal(|ui| {
                        let can_invoke = !state.ops.invoke_agent_input.trim().is_empty();
                        if (ui.add_enabled(can_invoke, egui::Button::new("Invoke")).clicked()
                            || enter)
                            && can_invoke
                        {
                            let _ = cmd_tx.try_send(UiCommand::InvokeAgent {
                                agent_id: selected_id,
                                instruction: state.ops.invoke_agent_input.trim().to_string(),
                            });
                            state.ops.invoke_agent_input.clear();
                            // Clear any previous operation error on new attempt.
                            state.ops.error = None;
                        }
                    });

                    if let Some(ref err) = state.ops.error {
                        ui.add_space(4.0);
                        ui.colored_label(HalconTheme::ERROR, format!("⚠  {err}"));
                    }
                });
            });
        }
    }
}
