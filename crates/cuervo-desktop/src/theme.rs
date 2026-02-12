use egui::{Color32, FontFamily, FontId, Rounding, Stroke, TextStyle, Visuals};

/// Dark color palette for the control plane.
pub struct CuervoTheme;

impl CuervoTheme {
    // Brand colors
    pub const BG_PRIMARY: Color32 = Color32::from_rgb(18, 18, 24);
    pub const BG_SECONDARY: Color32 = Color32::from_rgb(26, 26, 36);
    pub const BG_TERTIARY: Color32 = Color32::from_rgb(34, 34, 46);
    pub const BG_HOVER: Color32 = Color32::from_rgb(42, 42, 56);

    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(220, 220, 230);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(160, 160, 175);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(100, 100, 120);

    pub const ACCENT: Color32 = Color32::from_rgb(100, 140, 255);
    pub const ACCENT_HOVER: Color32 = Color32::from_rgb(130, 165, 255);

    pub const SUCCESS: Color32 = Color32::from_rgb(80, 200, 120);
    pub const WARNING: Color32 = Color32::from_rgb(255, 200, 60);
    pub const ERROR: Color32 = Color32::from_rgb(255, 90, 90);
    pub const INFO: Color32 = Color32::from_rgb(80, 170, 255);

    pub const BORDER: Color32 = Color32::from_rgb(50, 50, 65);
    #[allow(dead_code)]
    pub const SEPARATOR: Color32 = Color32::from_rgb(40, 40, 55);

    /// Health status color.
    pub fn health_color(status: &str) -> Color32 {
        match status {
            "healthy" => Self::SUCCESS,
            "degraded" => Self::WARNING,
            "unavailable" | "unhealthy" => Self::ERROR,
            _ => Self::TEXT_MUTED,
        }
    }

    /// Task status color.
    pub fn task_status_color(status: &str) -> Color32 {
        match status {
            "completed" => Self::SUCCESS,
            "running" => Self::ACCENT,
            "pending" => Self::TEXT_MUTED,
            "failed" => Self::ERROR,
            "cancelled" => Self::WARNING,
            _ => Self::TEXT_MUTED,
        }
    }

    /// Apply the Cuervo theme to an egui context.
    pub fn apply(ctx: &egui::Context) {
        let mut visuals = Visuals::dark();

        visuals.window_fill = Self::BG_PRIMARY;
        visuals.panel_fill = Self::BG_PRIMARY;
        visuals.faint_bg_color = Self::BG_SECONDARY;
        visuals.extreme_bg_color = Self::BG_TERTIARY;

        visuals.window_stroke = Stroke::new(1.0, Self::BORDER);
        visuals.window_rounding = Rounding::same(6.0);

        visuals.widgets.noninteractive.bg_fill = Self::BG_SECONDARY;
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Self::TEXT_SECONDARY);
        visuals.widgets.noninteractive.rounding = Rounding::same(4.0);

        visuals.widgets.inactive.bg_fill = Self::BG_TERTIARY;
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Self::TEXT_PRIMARY);
        visuals.widgets.inactive.rounding = Rounding::same(4.0);

        visuals.widgets.hovered.bg_fill = Self::BG_HOVER;
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Self::ACCENT_HOVER);
        visuals.widgets.hovered.rounding = Rounding::same(4.0);

        visuals.widgets.active.bg_fill = Self::ACCENT;
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        visuals.widgets.active.rounding = Rounding::same(4.0);

        visuals.selection.bg_fill = Self::ACCENT.linear_multiply(0.3);
        visuals.selection.stroke = Stroke::new(1.0, Self::ACCENT);

        ctx.set_visuals(visuals);

        let mut style = (*ctx.style()).clone();
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(18.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Body,
            FontId::new(13.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
        );
        style.spacing.item_spacing = egui::vec2(8.0, 4.0);
        style.spacing.window_margin = egui::Margin::same(12.0);
        ctx.set_style(style);
    }
}
