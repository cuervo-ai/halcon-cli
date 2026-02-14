//! Professional 3-zone TUI for Cuervo CLI.
//!
//! Feature-gated behind `tui` — only compiled when `--features tui` is active.
//! The TUI provides:
//! - **Prompt zone**: multiline editor (tui-textarea), Enter=newline, Ctrl+Enter=submit
//! - **Activity zone**: scrollable agent output (streaming text, tool results, warnings)
//! - **Status zone**: provider/model info, token counts, cost, round indicator

pub mod events;

#[cfg(feature = "tui")]
pub mod app;
#[cfg(feature = "tui")]
pub mod constants;
#[cfg(feature = "tui")]
pub mod input;
#[cfg(feature = "tui")]
pub mod layout;
#[cfg(feature = "tui")]
pub mod state;
#[cfg(feature = "tui")]
pub mod overlay;
#[cfg(feature = "tui")]
pub mod theme_bridge;
#[cfg(feature = "tui")]
pub mod widgets;
