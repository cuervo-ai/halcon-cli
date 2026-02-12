use egui::{RichText, Ui};
use tokio::sync::mpsc;

use crate::state::AppState;
use crate::theme::CuervoTheme;
use crate::workers::UiCommand;

pub fn render(ui: &mut Ui, state: &mut AppState, cmd_tx: &mpsc::UnboundedSender<UiCommand>) {
    ui.horizontal(|ui| {
        ui.heading("Agents");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Refresh").clicked() {
                let _ = cmd_tx.send(UiCommand::RefreshAgents);
            }
        });
    });
    ui.separator();

    if state.agents.is_empty() {
        ui.label(RichText::new("No agents registered").color(CuervoTheme::TEXT_MUTED));
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
                        cuervo_api::types::agent::HealthStatus::Healthy => "Healthy",
                        cuervo_api::types::agent::HealthStatus::Degraded { .. } => "Degraded",
                        cuervo_api::types::agent::HealthStatus::Unavailable { .. } => "Unavailable",
                        cuervo_api::types::agent::HealthStatus::Unknown => "Unknown",
                    };
                    let color = CuervoTheme::health_color(&health_str.to_lowercase());
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
                            .color(CuervoTheme::TEXT_SECONDARY),
                    );

                    // Invocations.
                    ui.label(agent.invocation_count.to_string());

                    // Concurrency.
                    ui.label(agent.max_concurrency.to_string());

                    // Actions.
                    ui.horizontal(|ui| {
                        if ui
                            .small_button(RichText::new("Stop").color(CuervoTheme::ERROR))
                            .clicked()
                        {
                            let _ = cmd_tx.send(UiCommand::StopAgent(agent.id));
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
            });
        }
    }
}
