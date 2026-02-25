use egui::{RichText, Ui};

use crate::state::AppState;
use crate::theme::HalconTheme;

pub fn render(ui: &mut Ui, state: &AppState) {
    ui.heading("Dashboard");
    ui.separator();

    // Summary cards row.
    ui.horizontal(|ui| {
        summary_card(ui, "Agents", state.agents.len(), HalconTheme::ACCENT);
        summary_card(
            ui,
            "Tasks",
            state.tasks.len(),
            HalconTheme::INFO,
        );
        summary_card(ui, "Tools", state.tools.len(), HalconTheme::SUCCESS);
        if let Some(ref m) = state.metrics {
            summary_card(
                ui,
                "Events/s",
                m.events_per_second as usize,
                HalconTheme::WARNING,
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
                let color = HalconTheme::health_color(&health_str.to_lowercase());
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
                t.status == halcon_api::types::task::TaskStatus::Running
                    || t.status == halcon_api::types::task::TaskStatus::Pending
            })
            .collect();

        if active.is_empty() {
            ui.label(RichText::new("No active tasks").color(HalconTheme::TEXT_MUTED));
        } else {
            for task in active.iter().take(10) {
                ui.horizontal(|ui| {
                    let color = HalconTheme::task_status_color(
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

    // Live activity feed — delegates to the reusable activity_panel widget.
    ui.group(|ui| {
        ui.set_max_height(200.0);
        crate::widgets::activity_panel::show(ui, &state.events, Some("Live Activity"));
    });

    ui.add_space(8.0);

    // Recent logs — compact embedded log feed.
    if !state.logs.is_empty() {
        ui.group(|ui| {
            ui.label(RichText::new("Recent Logs").strong());
            crate::widgets::log_viewer::render_log_viewer(ui, &state.logs, 30);
        });
        ui.add_space(8.0);
    }

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
            ui.label(RichText::new(label).size(11.0).color(HalconTheme::TEXT_MUTED));
        });
    });
}

