use egui::{RichText, Ui};

use crate::state::AppState;
use crate::theme::CuervoTheme;

pub fn render(ui: &mut Ui, state: &AppState) {
    ui.heading("Dashboard");
    ui.separator();

    // Summary cards row.
    ui.horizontal(|ui| {
        summary_card(ui, "Agents", state.agents.len(), CuervoTheme::ACCENT);
        summary_card(
            ui,
            "Tasks",
            state.tasks.len(),
            CuervoTheme::INFO,
        );
        summary_card(ui, "Tools", state.tools.len(), CuervoTheme::SUCCESS);
        if let Some(ref m) = state.metrics {
            summary_card(
                ui,
                "Events/s",
                m.events_per_second as usize,
                CuervoTheme::WARNING,
            );
        }
    });

    ui.add_space(12.0);

    // System status.
    if let Some(ref status) = state.system_status {
        ui.group(|ui| {
            ui.label(RichText::new("System").strong());
            ui.horizontal(|ui| {
                let health_str = format!("{:?}", status.health);
                let color = CuervoTheme::health_color(&health_str.to_lowercase());
                ui.colored_label(color, format!("Health: {health_str}"));
                ui.separator();
                ui.label(format!("Uptime: {}s", status.uptime_seconds));
                ui.separator();
                ui.label(format!("Platform: {} {}", status.platform.os, status.platform.arch));
            });
        });
    }

    ui.add_space(8.0);

    // Active tasks.
    ui.group(|ui| {
        ui.label(RichText::new("Active Tasks").strong());
        let active: Vec<_> = state
            .tasks
            .iter()
            .filter(|t| {
                t.status == cuervo_api::types::task::TaskStatus::Running
                    || t.status == cuervo_api::types::task::TaskStatus::Pending
            })
            .collect();

        if active.is_empty() {
            ui.label(RichText::new("No active tasks").color(CuervoTheme::TEXT_MUTED));
        } else {
            for task in active.iter().take(10) {
                ui.horizontal(|ui| {
                    let color = CuervoTheme::task_status_color(
                        &format!("{:?}", task.status).to_lowercase(),
                    );
                    ui.colored_label(color, "\u{25CF}");
                    ui.monospace(&task.id.to_string()[..8]);
                    ui.label(format!("{:?}", task.status));
                    ui.label(format!("{} nodes", task.node_results.len()));
                });
            }
        }
    });

    ui.add_space(8.0);

    // Recent events.
    ui.group(|ui| {
        ui.label(RichText::new("Recent Events").strong());
        let recent: Vec<_> = state.events.iter().rev().take(20).collect();
        if recent.is_empty() {
            ui.label(RichText::new("No events yet").color(CuervoTheme::TEXT_MUTED));
        } else {
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .show(ui, |ui| {
                    for event in &recent {
                        let text = format_event(event);
                        ui.label(
                            RichText::new(text)
                                .monospace()
                                .size(11.0)
                                .color(CuervoTheme::TEXT_SECONDARY),
                        );
                    }
                });
        }
    });

    // Metrics summary.
    if let Some(ref m) = state.metrics {
        ui.add_space(8.0);
        ui.group(|ui| {
            ui.label(RichText::new("Metrics").strong());
            ui.horizontal(|ui| {
                ui.label(format!("Invocations: {}", m.total_invocations));
                ui.separator();
                ui.label(format!(
                    "Tokens: {}in / {}out",
                    m.total_input_tokens, m.total_output_tokens
                ));
                ui.separator();
                ui.label(format!("Cost: ${:.4}", m.total_cost_usd));
            });
        });
    }
}

fn summary_card(ui: &mut Ui, label: &str, value: usize, color: egui::Color32) {
    ui.group(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(value.to_string()).size(24.0).color(color));
            ui.label(RichText::new(label).size(11.0).color(CuervoTheme::TEXT_MUTED));
        });
    });
}

fn format_event(event: &cuervo_api::types::ws::WsServerEvent) -> String {
    use cuervo_api::types::ws::WsServerEvent::*;
    match event {
        AgentRegistered { agent } => format!("+ Agent registered: {}", agent.name),
        AgentDeregistered { id } => format!("- Agent deregistered: {}", &id.to_string()[..8]),
        AgentHealthChanged { id, health } => {
            format!("~ Agent {} health: {:?}", &id.to_string()[..8], health)
        }
        AgentInvoked { id, .. } => format!("> Agent {} invoked", &id.to_string()[..8]),
        AgentCompleted { id, success, .. } => {
            format!("< Agent {} completed (ok={})", &id.to_string()[..8], success)
        }
        TaskSubmitted { execution_id, node_count } => {
            format!("+ Task {} submitted ({} nodes)", &execution_id.to_string()[..8], node_count)
        }
        TaskCompleted { execution_id, success, .. } => {
            format!("< Task {} completed (ok={})", &execution_id.to_string()[..8], success)
        }
        ToolExecuted { name, duration_ms, success, .. } => {
            format!("  Tool {} ({success}) {duration_ms}ms", name)
        }
        Log(entry) => format!("[{:?}] {}", entry.level, entry.message),
        _ => format!("{:?}", std::mem::discriminant(event)),
    }
}
