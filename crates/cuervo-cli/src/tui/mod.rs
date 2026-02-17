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
pub mod conversational_overlay;
#[cfg(feature = "tui")]
pub mod theme_bridge;
#[cfg(feature = "tui")]
pub mod input_state;
#[cfg(feature = "tui")]
pub mod permission_context;
#[cfg(feature = "tui")]
pub mod transition_engine;
#[cfg(feature = "tui")]
pub mod highlight;
#[cfg(feature = "tui")]
pub mod widgets;

// P0.1A: Core activity types (extracted from legacy activity.rs)
#[cfg(feature = "tui")]
pub mod activity_types;

// Phase A1: SOTA Activity Architecture — Modular redesign
#[cfg(feature = "tui")]
pub mod activity_model;
#[cfg(feature = "tui")]
pub mod activity_navigator;
#[cfg(feature = "tui")]
pub mod activity_controller;

// Phase A2: Virtual Scroll Optimization
#[cfg(feature = "tui")]
pub mod activity_renderer;

// Phase A3: Clipboard support
#[cfg(feature = "tui")]
pub mod clipboard;
