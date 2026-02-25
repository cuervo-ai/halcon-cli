use egui::{RichText, Ui};

use crate::state::AppState;
use crate::theme::HalconTheme;

// MetricChart::render takes &self so no mut borrow of state needed here.

pub fn render(ui: &mut Ui, state: &AppState) {
    ui.heading("Metrics");
    ui.separator();

    match &state.metrics {
        None => {
            ui.label(RichText::new("No metrics available").color(HalconTheme::TEXT_MUTED));
        }
        Some(m) => {
            // Overview cards.
            ui.horizontal(|ui| {
                metric_card(ui, "Uptime", &format!("{}s", m.uptime_seconds));
                metric_card(ui, "Agents", &m.agent_count.to_string());
                metric_card(ui, "Tools", &m.tool_count.to_string());
                metric_card(ui, "Events/s", &format!("{:.1}", m.events_per_second));
            });

            ui.add_space(8.0);

            // Token usage.
            ui.group(|ui| {
                ui.label(RichText::new("Token Usage").strong());
                ui.horizontal(|ui| {
                    ui.label(format!("Input: {}", m.total_input_tokens));
                    ui.separator();
                    ui.label(format!("Output: {}", m.total_output_tokens));
                    ui.separator();
                    ui.label(format!(
                        "Total: {}",
                        m.total_input_tokens + m.total_output_tokens
                    ));
                });
            });

            // Cost.
            ui.group(|ui| {
                ui.label(RichText::new("Cost").strong());
                ui.label(format!("Total: ${:.4}", m.total_cost_usd));
            });

            // Task counts.
            ui.group(|ui| {
                ui.label(RichText::new("Tasks").strong());
                ui.horizontal(|ui| {
                    ui.colored_label(
                        HalconTheme::ACCENT,
                        format!("Active: {}", m.active_tasks),
                    );
                    ui.separator();
                    ui.colored_label(
                        HalconTheme::SUCCESS,
                        format!("Completed: {}", m.completed_tasks),
                    );
                    ui.separator();
                    ui.colored_label(
                        HalconTheme::ERROR,
                        format!("Failed: {}", m.failed_tasks),
                    );
                });
            });

            // Per-agent metrics.
            if !m.agent_metrics.is_empty() {
                ui.add_space(8.0);
                ui.group(|ui| {
                    ui.label(RichText::new("Per-Agent Metrics").strong());
                    egui::Grid::new("agent_metrics_table")
                        .num_columns(5)
                        .striped(true)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new("Agent").strong());
                            ui.label(RichText::new("Invocations").strong());
                            ui.label(RichText::new("Avg Latency").strong());
                            ui.label(RichText::new("Tokens").strong());
                            ui.label(RichText::new("Error Rate").strong());
                            ui.end_row();

                            for am in &m.agent_metrics {
                                ui.label(&am.agent_name);
                                ui.label(am.invocation_count.to_string());
                                ui.label(format!("{:.0}ms", am.avg_latency_ms));
                                ui.label(am.total_tokens.to_string());

                                let err_color = if am.error_rate > 0.1 {
                                    HalconTheme::ERROR
                                } else if am.error_rate > 0.01 {
                                    HalconTheme::WARNING
                                } else {
                                    HalconTheme::SUCCESS
                                };
                                ui.colored_label(
                                    err_color,
                                    format!("{:.1}%", am.error_rate * 100.0),
                                );

                                ui.end_row();
                            }
                        });
                });
            }

            // ── Trend charts ─────────────────────────────────────────────────────────
            // Only meaningful once at least two samples have been collected.
            if state.charts.events_per_sec.values.len() >= 2
                || state.charts.active_tasks.values.len() >= 2
            {
                ui.add_space(8.0);
                ui.group(|ui| {
                    ui.label(RichText::new("Trends").strong());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        state
                            .charts.events_per_sec
                            .render(ui, egui::Vec2::new(220.0, 80.0));
                        ui.add_space(8.0);
                        state
                            .charts.active_tasks
                            .render(ui, egui::Vec2::new(220.0, 80.0));
                    });
                });
            }
        }
    }
}

fn metric_card(ui: &mut Ui, label: &str, value: &str) {
    ui.group(|ui| {
        ui.vertical(|ui| {
            ui.label(
                RichText::new(value)
                    .size(20.0)
                    .color(HalconTheme::ACCENT),
            );
            ui.label(
                RichText::new(label)
                    .size(11.0)
                    .color(HalconTheme::TEXT_MUTED),
            );
        });
    });
}
