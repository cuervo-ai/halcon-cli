//! Standardized user feedback rendering.
//!
//! All user-facing messages follow consistent formatting:
//! - Errors:   `Error: <what happened>`  +  optional `  Hint: <how to fix>`
//! - Warnings: `Warning: <what happened>` + optional `  Hint: <how to fix>`
//! - Status:   `[status message]` (bracketed, for progress/state changes)

use std::io::{self, Write};

use super::theme;

/// Print an error message to stderr with optional hint.
pub fn user_error(message: &str, hint: Option<&str>) {
    let t = theme::active();
    let r = theme::reset();
    let error_color = t.palette.error.fg();
    let hint_color = t.palette.muted.fg();

    let mut out = io::stderr().lock();
    let _ = writeln!(out, "{error_color}Error:{r} {message}");
    if let Some(h) = hint {
        let _ = writeln!(out, "  {hint_color}Hint: {h}{r}");
    }
    let _ = out.flush();
}

/// Print a warning message to stderr with optional hint.
pub fn user_warning(message: &str, hint: Option<&str>) {
    let t = theme::active();
    let r = theme::reset();
    let warn_color = t.palette.warning.fg();
    let hint_color = t.palette.muted.fg();

    let mut out = io::stderr().lock();
    let _ = writeln!(out, "{warn_color}Warning:{r} {message}");
    if let Some(h) = hint {
        let _ = writeln!(out, "  {hint_color}Hint: {h}{r}");
    }
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_error_does_not_panic() {
        user_error("something went wrong", None);
    }

    #[test]
    fn user_error_with_hint_does_not_panic() {
        user_error(
            "provider 'anthropic' not configured",
            Some("Run `cuervo auth login anthropic` to set up"),
        );
    }

    #[test]
    fn user_warning_does_not_panic() {
        user_warning("MCP server 'test' failed to start", None);
    }
}
