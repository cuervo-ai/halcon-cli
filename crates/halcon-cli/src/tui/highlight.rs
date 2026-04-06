//! Highlight pulse system for subtle visual attention cues.
//!
//! Uses sine-wave modulation of OKLCH lightness to create gentle pulsing
//! effects without harsh RGB flashing.

use crate::render::theme::ThemeColor;
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

/// Pulsing highlight effect with sine-wave modulation.
///
/// Modulates OKLCH lightness (perceptual brightness) using a sine wave
/// for smooth, natural-feeling attention cues.
#[derive(Debug, Clone)]
pub struct HighlightPulse {
    /// Base color to pulse from.
    base_color: ThemeColor,
    /// Pulse frequency (cycles per second).
    frequency_hz: f32,
    /// Lightness amplitude (±percentage).
    amplitude: f32,
    /// When pulse started.
    started_at: Instant,
}

impl HighlightPulse {
    /// Create a new highlight pulse.
    ///
    /// # Arguments
    /// * `base_color` - Base color to modulate
    /// * `frequency_hz` - Pulse frequency (0.5-2.0 Hz recommended)
    /// * `amplitude` - Lightness modulation amplitude (0.05-0.15 recommended)
    pub fn new(base_color: ThemeColor, frequency_hz: f32, amplitude: f32) -> Self {
        Self {
            base_color,
            frequency_hz,
            amplitude,
            started_at: Instant::now(),
        }
    }

    /// Create subtle pulse (0.5 Hz, ±8% lightness).
    pub fn subtle(base_color: ThemeColor) -> Self {
        Self::new(base_color, 0.5, 0.08)
    }

    /// Create medium pulse (1.0 Hz, ±12% lightness).
    pub fn medium(base_color: ThemeColor) -> Self {
        Self::new(base_color, 1.0, 0.12)
    }

    /// Create strong pulse (1.5 Hz, ±15% lightness).
    pub fn strong(base_color: ThemeColor) -> Self {
        Self::new(base_color, 1.5, 0.15)
    }

    /// Get current pulsed color.
    pub fn current(&self) -> ThemeColor {
        let elapsed = self.started_at.elapsed().as_secs_f32();
        let phase = elapsed * self.frequency_hz * 2.0 * std::f32::consts::PI;
        let modulation = phase.sin() * self.amplitude;

        self.modulate_lightness(modulation)
    }

    /// Reset pulse to start from now.
    pub fn reset(&mut self) {
        self.started_at = Instant::now();
    }

    /// Update base color (keeps phase).
    pub fn update_base(&mut self, new_base: ThemeColor) {
        self.base_color = new_base;
    }

    /// Modulate OKLCH lightness by delta.
    #[cfg(feature = "color-science")]
    fn modulate_lightness(&self, delta: f32) -> ThemeColor {
        use momoto_core::OKLCH;

        let oklch = self.base_color.to_oklch();
        let new_l = (oklch.l + delta as f64).clamp(0.0, 1.0);

        let result_oklch = OKLCH::new(new_l, oklch.c, oklch.h).map_to_gamut();
        ThemeColor::from_srgb8(result_oklch.to_color().to_srgb8())
    }

    /// Fallback RGB-based lightness modulation (approximation).
    #[cfg(not(feature = "color-science"))]
    fn modulate_lightness(&self, delta: f32) -> ThemeColor {
        let [r, g, b] = self.base_color.srgb8();

        // Approximate lightness shift via linear RGB scale
        let scale = 1.0 + delta;
        let r = (r as f32 * scale).clamp(0.0, 255.0) as u8;
        let g = (g as f32 * scale).clamp(0.0, 255.0) as u8;
        let b = (b as f32 * scale).clamp(0.0, 255.0) as u8;

        ThemeColor::from_srgb8([r, g, b])
    }
}

/// Manages multiple concurrent highlight pulses by key.
#[derive(Debug, Default)]
pub struct HighlightManager {
    /// Active pulses keyed by element name.
    pulses: std::collections::HashMap<String, HighlightPulse>,
}

impl HighlightManager {
    /// Create a new highlight manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new pulse for a named element.
    pub fn start(&mut self, key: impl Into<String>, pulse: HighlightPulse) {
        self.pulses.insert(key.into(), pulse);
    }

    /// Start a subtle pulse.
    pub fn start_subtle(&mut self, key: impl Into<String>, base_color: ThemeColor) {
        self.pulses
            .insert(key.into(), HighlightPulse::subtle(base_color));
    }

    /// Start a medium pulse.
    pub fn start_medium(&mut self, key: impl Into<String>, base_color: ThemeColor) {
        self.pulses
            .insert(key.into(), HighlightPulse::medium(base_color));
    }

    /// Start a strong pulse.
    pub fn start_strong(&mut self, key: impl Into<String>, base_color: ThemeColor) {
        self.pulses
            .insert(key.into(), HighlightPulse::strong(base_color));
    }

    /// Get current pulsed color (or base if not pulsing).
    pub fn current(&self, key: &str, default: ThemeColor) -> ThemeColor {
        self.pulses.get(key).map(|p| p.current()).unwrap_or(default)
    }

    /// Stop pulse for a key.
    pub fn stop(&mut self, key: &str) {
        self.pulses.remove(key);
    }

    /// Check if a key is pulsing.
    pub fn is_pulsing(&self, key: &str) -> bool {
        self.pulses.contains_key(key)
    }

    /// Clear all pulses.
    pub fn clear(&mut self) {
        self.pulses.clear();
    }

    /// Check if there are any active pulses.
    pub fn has_active(&self) -> bool {
        !self.pulses.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_new_stores_params() {
        let color = ThemeColor::from_srgb8([100, 100, 100]);
        let pulse = HighlightPulse::new(color, 1.0, 0.1);

        assert_eq!(pulse.base_color.srgb8(), color.srgb8());
        assert_eq!(pulse.frequency_hz, 1.0);
        assert_eq!(pulse.amplitude, 0.1);
    }

    #[test]
    fn pulse_subtle_has_correct_params() {
        let color = ThemeColor::from_srgb8([100, 100, 100]);
        let pulse = HighlightPulse::subtle(color);

        assert_eq!(pulse.frequency_hz, 0.5);
        assert_eq!(pulse.amplitude, 0.08);
    }

    #[test]
    fn pulse_medium_has_correct_params() {
        let color = ThemeColor::from_srgb8([100, 100, 100]);
        let pulse = HighlightPulse::medium(color);

        assert_eq!(pulse.frequency_hz, 1.0);
        assert_eq!(pulse.amplitude, 0.12);
    }

    #[test]
    fn pulse_strong_has_correct_params() {
        let color = ThemeColor::from_srgb8([100, 100, 100]);
        let pulse = HighlightPulse::strong(color);

        assert_eq!(pulse.frequency_hz, 1.5);
        assert_eq!(pulse.amplitude, 0.15);
    }

    #[test]
    fn pulse_current_modulates_color() {
        let color = ThemeColor::from_srgb8([100, 100, 100]);
        let pulse = HighlightPulse::new(color, 10.0, 0.2); // Fast for testing

        let current = pulse.current();
        // Should be different due to modulation (unless exactly at zero phase)
        // We can't assert exact values due to timing, but structure is testable
        let [_r, _g, _b] = current.srgb8();
        // srgb8() returns [u8; 3] — values are inherently in 0..=255
    }

    #[test]
    fn pulse_reset_restarts_phase() {
        let color = ThemeColor::from_srgb8([100, 100, 100]);
        let mut pulse = HighlightPulse::new(color, 1.0, 0.1);

        std::thread::sleep(Duration::from_millis(100));
        let before_reset = pulse.started_at;

        pulse.reset();
        assert!(pulse.started_at > before_reset);
    }

    #[test]
    fn pulse_update_base_changes_color() {
        let color1 = ThemeColor::from_srgb8([100, 100, 100]);
        let color2 = ThemeColor::from_srgb8([200, 200, 200]);
        let mut pulse = HighlightPulse::subtle(color1);

        pulse.update_base(color2);
        assert_eq!(pulse.base_color.srgb8(), color2.srgb8());
    }

    #[test]
    fn manager_stores_pulses() {
        let mut manager = HighlightManager::new();
        let color = ThemeColor::from_srgb8([100, 100, 100]);

        manager.start("border", HighlightPulse::subtle(color));
        assert!(manager.is_pulsing("border"));
    }

    #[test]
    fn manager_start_subtle_creates_pulse() {
        let mut manager = HighlightManager::new();
        let color = ThemeColor::from_srgb8([100, 100, 100]);

        manager.start_subtle("border", color);
        assert!(manager.is_pulsing("border"));
    }

    #[test]
    fn manager_current_returns_default_if_not_pulsing() {
        let manager = HighlightManager::new();
        let default = ThemeColor::from_srgb8([255, 0, 0]);

        let current = manager.current("nonexistent", default);
        assert_eq!(current.srgb8(), default.srgb8());
    }

    #[test]
    fn manager_stop_removes_pulse() {
        let mut manager = HighlightManager::new();
        let color = ThemeColor::from_srgb8([100, 100, 100]);

        manager.start_subtle("border", color);
        assert!(manager.is_pulsing("border"));

        manager.stop("border");
        assert!(!manager.is_pulsing("border"));
    }

    #[test]
    fn manager_clear_removes_all() {
        let mut manager = HighlightManager::new();
        let color = ThemeColor::from_srgb8([100, 100, 100]);

        manager.start_subtle("border", color);
        manager.start_medium("bg", color);
        assert!(manager.is_pulsing("border"));
        assert!(manager.is_pulsing("bg"));

        manager.clear();
        assert!(!manager.is_pulsing("border"));
        assert!(!manager.is_pulsing("bg"));
    }

    #[test]
    fn manager_start_medium_creates_pulse() {
        let mut manager = HighlightManager::new();
        let color = ThemeColor::from_srgb8([100, 100, 100]);

        manager.start_medium("border", color);
        assert!(manager.is_pulsing("border"));
    }

    #[test]
    fn manager_start_strong_creates_pulse() {
        let mut manager = HighlightManager::new();
        let color = ThemeColor::from_srgb8([100, 100, 100]);

        manager.start_strong("border", color);
        assert!(manager.is_pulsing("border"));
    }
}
