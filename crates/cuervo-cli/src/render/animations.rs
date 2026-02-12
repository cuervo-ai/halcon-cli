//! Animation frames and capability detection for terminal spinners.

use super::color;

/// Braille-pattern spinner frames for neon-themed animation.
pub const NEON_SPINNER_FRAMES: &[&str] = &[
    "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏",
];

/// ASCII fallback spinner frames for non-Unicode terminals.
pub const ASCII_SPINNER_FRAMES: &[&str] = &["|", "/", "-", "\\"];

/// Select the appropriate spinner frame set based on terminal capabilities.
pub fn spinner_frames() -> &'static [&'static str] {
    if color::unicode_enabled() {
        NEON_SPINNER_FRAMES
    } else {
        ASCII_SPINNER_FRAMES
    }
}

/// Returns true if animations should be rendered (TTY, not CI, not opt-out).
pub fn should_animate() -> bool {
    color::animations_enabled()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neon_frames_non_empty() {
        assert!(!NEON_SPINNER_FRAMES.is_empty());
        for frame in NEON_SPINNER_FRAMES {
            assert!(!frame.is_empty());
        }
    }

    #[test]
    fn ascii_frames_non_empty() {
        assert!(!ASCII_SPINNER_FRAMES.is_empty());
        for frame in ASCII_SPINNER_FRAMES {
            assert!(!frame.is_empty());
        }
    }

    #[test]
    fn spinner_frames_returns_valid_set() {
        let frames = spinner_frames();
        assert!(frames.len() >= 4);
    }

    #[test]
    fn should_animate_consistent() {
        let a1 = should_animate();
        let a2 = should_animate();
        assert_eq!(a1, a2);
    }
}
