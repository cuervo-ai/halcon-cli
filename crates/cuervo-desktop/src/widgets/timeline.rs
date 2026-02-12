use egui::{Pos2, RichText, Ui, Vec2};

use crate::theme::CuervoTheme;

/// A single entry on the timeline.
pub struct TimelineEntry {
    pub timestamp: String,
    pub label: String,
    pub color: egui::Color32,
    pub detail: Option<String>,
}

/// Render a vertical timeline.
pub fn render_timeline(ui: &mut Ui, entries: &[TimelineEntry]) {
    if entries.is_empty() {
        ui.label(RichText::new("No events").color(CuervoTheme::TEXT_MUTED));
        return;
    }

    let row_height = 28.0;
    let dot_radius = 4.0;
    let line_x = 80.0;

    let total_height = entries.len() as f32 * row_height;
    let (response, painter) =
        ui.allocate_painter(Vec2::new(ui.available_width(), total_height), egui::Sense::hover());
    let origin = response.rect.min;

    for (i, entry) in entries.iter().enumerate() {
        let y = origin.y + i as f32 * row_height + row_height / 2.0;

        // Timestamp.
        painter.text(
            Pos2::new(origin.x + 4.0, y),
            egui::Align2::LEFT_CENTER,
            &entry.timestamp,
            egui::FontId::monospace(10.0),
            CuervoTheme::TEXT_MUTED,
        );

        // Vertical line.
        if i > 0 {
            painter.line_segment(
                [
                    Pos2::new(origin.x + line_x, y - row_height),
                    Pos2::new(origin.x + line_x, y),
                ],
                egui::Stroke::new(1.0, CuervoTheme::BORDER),
            );
        }

        // Dot.
        painter.circle_filled(Pos2::new(origin.x + line_x, y), dot_radius, entry.color);

        // Label.
        painter.text(
            Pos2::new(origin.x + line_x + 12.0, y),
            egui::Align2::LEFT_CENTER,
            &entry.label,
            egui::FontId::proportional(12.0),
            CuervoTheme::TEXT_PRIMARY,
        );
    }
}
