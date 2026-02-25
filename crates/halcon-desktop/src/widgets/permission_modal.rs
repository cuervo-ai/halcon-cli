//! Permission modal widget with animated countdown progress bar.
//!
//! Renders a centered egui Window that asks the user to approve or deny a
//! tool-execution request from the agent. The modal auto-denies once the
//! countdown reaches zero — the parent is responsible for calling the deny
//! handler at that point (the widget just signals it via the return value).

use egui::{Color32, RichText, Vec2};

use crate::state::ChatPermissionModal;
use crate::theme::HalconTheme;

/// Return value from [`show`]: what the user (or timeout) decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// User clicked Approve.
    Approved,
    /// User clicked Deny or the countdown expired.
    Denied,
    /// Modal is still open; no decision yet.
    Pending,
}

/// Show the permission modal window.
///
/// Returns the user's decision. Call every frame while the modal is visible.
/// The caller must clear `state.chat.permission_modal` when the result is not
/// `PermissionOutcome::Pending`.
pub fn show(ctx: &egui::Context, modal: &ChatPermissionModal) -> PermissionOutcome {
    let elapsed = modal.created_at.elapsed().as_secs();
    let remaining = modal.deadline_secs.saturating_sub(elapsed);
    let fraction = if modal.deadline_secs > 0 {
        remaining as f32 / modal.deadline_secs as f32
    } else {
        0.0
    };

    // Auto-deny once the countdown reaches zero.
    if remaining == 0 {
        return PermissionOutcome::Denied;
    }

    let risk_color = risk_level_color(&modal.risk_level);

    let mut outcome = PermissionOutcome::Pending;

    egui::Window::new("Permission Required")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .min_width(340.0)
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                // Tool name header.
                ui.horizontal(|ui| {
                    ui.colored_label(
                        risk_color,
                        RichText::new("\u{26A0}").size(18.0),
                    );
                    ui.add_space(4.0);
                    ui.colored_label(
                        risk_color,
                        RichText::new(&modal.tool_name).strong().size(15.0),
                    );
                });

                ui.add_space(2.0);

                // Risk badge.
                let risk_text = format!("Risk level: {}", modal.risk_level.to_uppercase());
                ui.colored_label(risk_color, RichText::new(risk_text).size(11.0));

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                // Description.
                ui.label(&modal.description);

                ui.add_space(10.0);

                // Countdown progress bar.
                let bar_color = if fraction > 0.5 {
                    HalconTheme::SUCCESS
                } else if fraction > 0.25 {
                    HalconTheme::WARNING
                } else {
                    HalconTheme::ERROR
                };

                let (rect, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 6.0),
                    egui::Sense::hover(),
                );
                let filled_rect = egui::Rect::from_min_size(
                    rect.min,
                    Vec2::new(rect.width() * fraction, rect.height()),
                );
                ui.painter().rect_filled(rect, 2.0, Color32::from_gray(45));
                ui.painter().rect_filled(filled_rect, 2.0, bar_color);

                ui.add_space(4.0);
                ui.colored_label(
                    HalconTheme::TEXT_MUTED,
                    RichText::new(format!("Auto-deny in {}s", remaining)).size(10.0),
                );

                ui.add_space(12.0);

                // Buttons.
                ui.horizontal(|ui| {
                    let approve_btn = egui::Button::new(
                        RichText::new("Approve").color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(30, 100, 50))
                    .min_size(Vec2::new(90.0, 28.0));

                    let deny_btn = egui::Button::new(
                        RichText::new("Deny").color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(120, 30, 30))
                    .min_size(Vec2::new(90.0, 28.0));

                    if ui.add(approve_btn).clicked() {
                        outcome = PermissionOutcome::Approved;
                    }
                    ui.add_space(8.0);
                    if ui.add(deny_btn).clicked() {
                        outcome = PermissionOutcome::Denied;
                    }
                });
            });
        });

    outcome
}

fn risk_level_color(risk: &str) -> Color32 {
    match risk.to_lowercase().as_str() {
        "critical" => Color32::from_rgb(220, 50, 50),
        "high" => Color32::from_rgb(200, 80, 30),
        "medium" => HalconTheme::WARNING,
        "low" => HalconTheme::TEXT_SECONDARY,
        _ => HalconTheme::TEXT_MUTED,
    }
}
