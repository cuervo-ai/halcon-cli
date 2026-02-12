use egui::{Pos2, Rect, Ui, Vec2};
use std::collections::VecDeque;

use crate::theme::CuervoTheme;

/// Simple line chart for metric time series.
pub struct MetricChart {
    pub label: String,
    pub values: VecDeque<f64>,
    pub max_points: usize,
}

impl MetricChart {
    pub fn new(label: impl Into<String>, max_points: usize) -> Self {
        Self {
            label: label.into(),
            values: VecDeque::with_capacity(max_points),
            max_points,
        }
    }

    pub fn push(&mut self, value: f64) {
        if self.values.len() >= self.max_points {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    pub fn render(&self, ui: &mut Ui, size: Vec2) {
        let (response, painter) = ui.allocate_painter(size, egui::Sense::hover());
        let rect = response.rect;

        // Background.
        painter.rect_filled(rect, 4.0, CuervoTheme::BG_SECONDARY);
        painter.rect_stroke(rect, 4.0, egui::Stroke::new(1.0, CuervoTheme::BORDER));

        // Label.
        painter.text(
            rect.min + Vec2::new(6.0, 4.0),
            egui::Align2::LEFT_TOP,
            &self.label,
            egui::FontId::proportional(11.0),
            CuervoTheme::TEXT_MUTED,
        );

        if self.values.len() < 2 {
            return;
        }

        let min_val = self
            .values
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let max_val = self
            .values
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let range = (max_val - min_val).max(1.0);

        let chart_rect = Rect::from_min_max(
            rect.min + Vec2::new(4.0, 20.0),
            rect.max - Vec2::new(4.0, 4.0),
        );

        let points: Vec<Pos2> = self
            .values
            .iter()
            .enumerate()
            .map(|(i, &val)| {
                let x = chart_rect.min.x
                    + (i as f32 / (self.values.len() - 1).max(1) as f32) * chart_rect.width();
                let y = chart_rect.max.y
                    - ((val - min_val) / range) as f32 * chart_rect.height();
                Pos2::new(x, y)
            })
            .collect();

        // Draw line.
        for window in points.windows(2) {
            painter.line_segment(
                [window[0], window[1]],
                egui::Stroke::new(1.5, CuervoTheme::ACCENT),
            );
        }

        // Current value.
        if let Some(&last) = self.values.back() {
            painter.text(
                Pos2::new(chart_rect.max.x - 4.0, chart_rect.min.y),
                egui::Align2::RIGHT_TOP,
                format!("{last:.1}"),
                egui::FontId::monospace(10.0),
                CuervoTheme::ACCENT,
            );
        }
    }
}
