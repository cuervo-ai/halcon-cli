//! Thinking bubble widget — shows an animated "AI is thinking" indicator.
//!
//! Renders a compact inline indicator with a rotating braille-spinner, elapsed
//! time, and current token count. The elapsed time is passed in by the caller
//! (stored in `AppState::chat_turn_started_at`) so the widget is stateless and
//! can be called freely without resetting any internal timer.

use egui::RichText;

use crate::theme::HalconTheme;

/// Braille spinner frames (8 positions, one per rotation step at 8fps).
const SPINNER_FRAMES: &[char] = &['⣾', '⣽', '⣻', '⢿', '⡿', '⣟', '⣯', '⣷'];

/// Render the thinking bubble inline.
///
/// - `elapsed_secs` — seconds since the current turn began (from `AppState::chat_turn_started_at`)
/// - `token_count` — streaming tokens received so far (0 while pre-first-token)
pub fn show(ui: &mut egui::Ui, elapsed_secs: f32, token_count: usize) {
    // Rotate spinner at ~8fps using elapsed time.
    let frame_idx = ((elapsed_secs * 8.0) as usize) % SPINNER_FRAMES.len();
    let spinner = SPINNER_FRAMES[frame_idx];

    ui.horizontal(|ui| {
        // Spinner glyph.
        ui.colored_label(
            HalconTheme::ACCENT,
            RichText::new(spinner.to_string()).size(13.0),
        );

        // Label.
        let label = if token_count == 0 {
            format!("Thinking... ({:.1}s)", elapsed_secs)
        } else {
            format!("Generating... {} tokens ({:.1}s)", token_count, elapsed_secs)
        };
        ui.colored_label(HalconTheme::TEXT_MUTED, RichText::new(label).size(12.0));
    });
}
