//! File explorer view — local project tree with async-backed directory loading.
//!
//! Layout:
//!   ┌──────────────────┬────────────────────────────────────────┐
//!   │  Root input      │                                        │
//!   │  + Load button   │  File content viewer (read-only)       │
//!   ├──────────────────│                                        │
//!   │  Directory tree  │  (or placeholder when nothing loaded)  │
//!   │  (lazy-loaded)   │                                        │
//!   └──────────────────┴────────────────────────────────────────┘
//!
//! All filesystem IO is performed in the Tokio worker via `UiCommand::LoadDirectory`
//! and `UiCommand::LoadFile` — the egui update loop is never blocked.

use egui::{RichText, ScrollArea, Ui};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

use crate::state::AppState;
use crate::theme::HalconTheme;
use crate::workers::UiCommand;

pub fn render(ui: &mut Ui, state: &mut AppState, cmd_tx: &mpsc::Sender<UiCommand>) {
    // Left panel: root input + directory tree.
    egui::SidePanel::left("file_tree_panel")
        .default_width(220.0)
        .min_width(150.0)
        .max_width(350.0)
        .resizable(true)
        .show_inside(ui, |ui| {
            render_tree_panel(ui, state, cmd_tx);
        });

    // Right panel: file content viewer.
    egui::CentralPanel::default().show_inside(ui, |ui| {
        render_content_panel(ui, state);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Left panel — root selector + tree
// ─────────────────────────────────────────────────────────────────────────────

fn render_tree_panel(ui: &mut Ui, state: &mut AppState, cmd_tx: &mpsc::Sender<UiCommand>) {
    ui.heading("Files");
    ui.separator();

    // ── Root path input ───────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let edit = egui::TextEdit::singleline(&mut state.files.root)
            .hint_text("Project root path…")
            .desired_width(ui.available_width() - 48.0);
        let resp = ui.add(edit);

        // Load on Enter or button click.
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let load_clicked = ui.button("Go").clicked();

        if (enter || load_clicked) && !state.files.root.trim().is_empty() {
            let root = PathBuf::from(state.files.root.trim());
            load_directory(state, cmd_tx, root);
        }
    });

    // Error banner.
    if let Some(ref err) = state.files.error.clone() {
        ui.colored_label(HalconTheme::ERROR, format!("⚠  {err}"));
    }

    // Loading indicator.
    if state.files.loading {
        ui.colored_label(HalconTheme::TEXT_MUTED, "Loading…");
    }

    ui.add_space(4.0);

    // ── Directory tree ────────────────────────────────────────────────────────
    let root = PathBuf::from(state.files.root.trim());
    if state.files.dir_cache.contains_key(&root) {
        ScrollArea::vertical()
            .id_salt("file_tree_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                render_entries(ui, state, cmd_tx, &root.clone(), 0);
            });
    } else if state.files.root.trim().is_empty() {
        ui.label(
            RichText::new("Enter a project root path above")
                .color(HalconTheme::TEXT_MUTED)
                .size(11.0),
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Recursive tree renderer
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum directory nesting depth rendered.  Guards against deep symlink cycles.
const MAX_DEPTH: usize = 12;

fn render_entries(
    ui: &mut Ui,
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<UiCommand>,
    dir: &Path,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        ui.label(RichText::new("…").color(HalconTheme::TEXT_MUTED));
        return;
    }

    // Clone to avoid holding the borrow across the mutable state updates below.
    let entries = match state.files.dir_cache.get(dir) {
        Some(e) => e.clone(),
        None => return,
    };

    if entries.is_empty() {
        ui.label(RichText::new("(empty)").color(HalconTheme::TEXT_MUTED).size(10.0));
        return;
    }

    for entry in &entries {
        if entry.is_dir {
            // ── Directory row ─────────────────────────────────────────────────
            let is_expanded = state.files.expanded.contains(&entry.path);
            let icon = if is_expanded { "▼ " } else { "▶ " };
            let label = format!("{}{}/", icon, entry.name);

            if ui
                .selectable_label(
                    false,
                    RichText::new(label)
                        .size(12.0)
                        .color(HalconTheme::TEXT_SECONDARY),
                )
                .clicked()
            {
                if is_expanded {
                    state.files.expanded.remove(&entry.path);
                } else {
                    state.files.expanded.insert(entry.path.clone());
                    // Lazy-load: only fetch if not already in cache.
                    if !state.files.dir_cache.contains_key(&entry.path) {
                        load_directory(state, cmd_tx, entry.path.clone());
                    }
                }
            }

            // Render children indented when expanded.
            if state.files.expanded.contains(&entry.path) {
                let child_path = entry.path.clone();
                ui.indent(child_path.to_string_lossy().as_ref(), |ui| {
                    render_entries(ui, state, cmd_tx, &child_path, depth + 1);
                });
            }
        } else {
            // ── File row ──────────────────────────────────────────────────────
            let is_selected = state.files.selected.as_deref() == Some(entry.path.as_path());
            if ui
                .selectable_label(
                    is_selected,
                    RichText::new(&entry.name).size(12.0),
                )
                .clicked()
                && !is_selected
            {
                state.files.selected = Some(entry.path.clone());
                state.files.content = None; // clear stale content while loading
                load_file(state, cmd_tx, entry.path.clone());
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Right panel — content viewer
// ─────────────────────────────────────────────────────────────────────────────

fn render_content_panel(ui: &mut Ui, state: &AppState) {
    // Path breadcrumb.
    if let Some(ref path) = state.files.selected {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(path.to_string_lossy().as_ref())
                    .size(10.0)
                    .color(HalconTheme::TEXT_MUTED),
            );
        });
        ui.separator();
    }

    match &state.files.content {
        Some(content) => {
            ScrollArea::both()
                .id_salt("file_content_scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    // Monospace read-only label — no allocations per frame (content is
                    // borrowed directly, not cloned).
                    ui.monospace(content);
                });
        }
        None if state.files.loading => {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("Loading…")
                        .color(HalconTheme::TEXT_MUTED)
                        .size(13.0),
                );
            });
        }
        None => {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("Select a file to view its contents")
                        .color(HalconTheme::TEXT_MUTED)
                        .size(13.0),
                );
            });
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper: dispatch async commands + set loading flag
// ─────────────────────────────────────────────────────────────────────────────

fn load_directory(state: &mut AppState, cmd_tx: &mpsc::Sender<UiCommand>, path: PathBuf) {
    state.files.loading = true;
    state.files.error = None;
    let _ = cmd_tx.try_send(UiCommand::LoadDirectory { path });
}

fn load_file(state: &mut AppState, cmd_tx: &mpsc::Sender<UiCommand>, path: PathBuf) {
    state.files.loading = true;
    state.files.error = None;
    let _ = cmd_tx.try_send(UiCommand::LoadFile { path });
}
