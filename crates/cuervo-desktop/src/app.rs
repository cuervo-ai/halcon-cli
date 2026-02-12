use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::state::{ActiveView, AppState, ConnectionState};
use crate::theme::CuervoTheme;
use crate::views;
use crate::workers::{BackendMessage, UiCommand};

/// The main desktop application.
pub struct CuervoApp {
    state: AppState,
    config: AppConfig,
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    msg_rx: mpsc::UnboundedReceiver<BackendMessage>,
    theme_applied: bool,
    /// Keep the tokio runtime alive for the lifetime of the app.
    _runtime: tokio::runtime::Runtime,
}

impl CuervoApp {
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
        msg_rx: mpsc::UnboundedReceiver<BackendMessage>,
        runtime: tokio::runtime::Runtime,
    ) -> Self {
        let config = AppConfig::load();
        let mut state = AppState::default();

        // Allow overriding the initial view via env var (for testing/automation).
        if let Ok(view) = std::env::var("CUERVO_VIEW") {
            state.active_view = match view.as_str() {
                "dashboard" => ActiveView::Dashboard,
                "agents" => ActiveView::Agents,
                "tasks" => ActiveView::Tasks,
                "tools" => ActiveView::Tools,
                "logs" => ActiveView::Logs,
                "metrics" => ActiveView::Metrics,
                "protocols" => ActiveView::Protocols,
                "files" => ActiveView::Files,
                "settings" => ActiveView::Settings,
                _ => ActiveView::Dashboard,
            };
        }

        // Auto-connect if CUERVO_API_TOKEN env var is set.
        if config.has_auto_connect() {
            state.connection = ConnectionState::Connecting;
            state.show_connect_dialog = false;
            let _ = cmd_tx.send(UiCommand::Connect {
                url: config.server_url.clone(),
                token: config.auth_token.clone(),
            });
        }

        Self {
            state,
            config,
            cmd_tx,
            msg_rx,
            theme_applied: false,
            _runtime: runtime,
        }
    }

    /// Drain all pending backend messages and update state.
    fn process_backend_messages(&mut self) {
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                BackendMessage::Connected => {
                    self.state.connection = ConnectionState::Connected;
                    self.state.show_connect_dialog = false;
                    // Trigger initial data load.
                    let _ = self.cmd_tx.send(UiCommand::RefreshAgents);
                    let _ = self.cmd_tx.send(UiCommand::RefreshTasks);
                    let _ = self.cmd_tx.send(UiCommand::RefreshTools);
                    let _ = self.cmd_tx.send(UiCommand::RefreshMetrics);
                    let _ = self.cmd_tx.send(UiCommand::RefreshStatus);
                    let _ = self.cmd_tx.send(UiCommand::RefreshConfig);
                }
                BackendMessage::Disconnected(reason) => {
                    self.state.connection = ConnectionState::Error(reason);
                    self.state.runtime_config = None;
                    self.state.config_dirty = false;
                    self.state.config_error = None;
                }
                BackendMessage::ConnectionError(err) => {
                    self.state.connection = ConnectionState::Error(err);
                }
                BackendMessage::AgentsUpdated(agents) => {
                    self.state.agents = agents;
                }
                BackendMessage::TasksUpdated(tasks) => {
                    self.state.tasks = tasks;
                }
                BackendMessage::ToolsUpdated(tools) => {
                    self.state.tools = tools;
                }
                BackendMessage::MetricsUpdated(m) => {
                    self.state.metrics = Some(m);
                }
                BackendMessage::SystemStatusUpdated(s) => {
                    self.state.system_status = Some(s);
                }
                BackendMessage::ConfigLoaded(cfg) => {
                    self.state.runtime_config = Some(cfg);
                    self.state.config_dirty = false;
                    self.state.config_error = None;
                }
                BackendMessage::ConfigUpdated(cfg) => {
                    self.state.runtime_config = Some(cfg);
                    self.state.config_dirty = false;
                    self.state.config_error = None;
                }
                BackendMessage::ConfigError(err) => {
                    self.state.config_error = Some(err);
                }
                BackendMessage::Event(event) => {
                    // Also extract log entries from events.
                    if let cuervo_api::types::ws::WsServerEvent::Log(ref entry) = event {
                        self.state.push_log(entry.clone());
                    }
                    self.state.push_event(event);
                }
            }
        }
    }

    fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Cuervo")
                    .size(16.0)
                    .strong()
                    .color(CuervoTheme::ACCENT),
            );
            ui.label(
                egui::RichText::new("Control Plane")
                    .size(11.0)
                    .color(CuervoTheme::TEXT_MUTED),
            );
            ui.add_space(12.0);

            let views = [
                (ActiveView::Dashboard, "Dashboard"),
                (ActiveView::Agents, "Agents"),
                (ActiveView::Tasks, "Tasks"),
                (ActiveView::Tools, "Tools"),
            ];
            for (view, label) in &views {
                if ui
                    .selectable_label(self.state.active_view == *view, *label)
                    .clicked()
                {
                    self.state.active_view = *view;
                }
            }

            ui.separator();

            let obs_views = [
                (ActiveView::Logs, "Logs"),
                (ActiveView::Metrics, "Metrics"),
                (ActiveView::Protocols, "Protocols"),
            ];
            for (view, label) in &obs_views {
                if ui
                    .selectable_label(self.state.active_view == *view, *label)
                    .clicked()
                {
                    self.state.active_view = *view;
                }
            }

            ui.separator();

            let util_views = [
                (ActiveView::Files, "Files"),
                (ActiveView::Settings, "Settings"),
            ];
            for (view, label) in &util_views {
                if ui
                    .selectable_label(self.state.active_view == *view, *label)
                    .clicked()
                {
                    self.state.active_view = *view;
                }
            }
        });
    }

    fn render_status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Connection indicator.
            let (color, label) = match &self.state.connection {
                ConnectionState::Connected => (CuervoTheme::SUCCESS, "Connected"),
                ConnectionState::Connecting => (CuervoTheme::WARNING, "Connecting..."),
                ConnectionState::Disconnected => (CuervoTheme::TEXT_MUTED, "Disconnected"),
                ConnectionState::Error(_) => (CuervoTheme::ERROR, "Error"),
            };
            ui.colored_label(color, format!("\u{25CF} {label}"));
            ui.separator();

            ui.label(format!("{} agents", self.state.agents.len()));
            ui.separator();
            ui.label(format!("{} tools", self.state.tools.len()));
            ui.separator();

            if let Some(ref status) = self.state.system_status {
                ui.label(format!("uptime {}s", status.uptime_seconds));
                ui.separator();
                ui.label(format!("mem {}B", status.platform.memory_usage_bytes));
            }
        });
    }

    fn render_connect_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("Connect to Runtime")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.label("Server URL:");
                    ui.text_edit_singleline(&mut self.config.server_url);
                    ui.add_space(4.0);
                    ui.label("Auth Token:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.config.auth_token).password(true),
                    );
                    ui.add_space(8.0);

                    if let ConnectionState::Error(ref err) = self.state.connection {
                        ui.colored_label(CuervoTheme::ERROR, err);
                        ui.add_space(4.0);
                    }

                    ui.horizontal(|ui| {
                        if ui.button("Connect").clicked() {
                            self.state.connection = ConnectionState::Connecting;
                            let _ = self.cmd_tx.send(UiCommand::Connect {
                                url: self.config.server_url.clone(),
                                token: self.config.auth_token.clone(),
                            });
                        }
                        if ui.button("Skip").clicked() {
                            self.state.show_connect_dialog = false;
                        }
                    });
                });
            });
    }
}

impl eframe::App for CuervoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.theme_applied {
            CuervoTheme::apply(ctx);
            self.theme_applied = true;
        }

        self.process_backend_messages();

        // Keyboard shortcuts for view switching (Ctrl+1..9).
        ctx.input(|i| {
            use egui::Key;
            let ctrl = i.modifiers.ctrl || i.modifiers.mac_cmd;
            if ctrl {
                let mappings = [
                    (Key::Num1, ActiveView::Dashboard),
                    (Key::Num2, ActiveView::Agents),
                    (Key::Num3, ActiveView::Tasks),
                    (Key::Num4, ActiveView::Tools),
                    (Key::Num5, ActiveView::Logs),
                    (Key::Num6, ActiveView::Metrics),
                    (Key::Num7, ActiveView::Protocols),
                    (Key::Num8, ActiveView::Files),
                    (Key::Num9, ActiveView::Settings),
                ];
                for (key, view) in mappings {
                    if i.key_pressed(key) {
                        self.state.active_view = view;
                    }
                }
            }
        });

        // Connect dialog.
        if self.state.show_connect_dialog {
            self.render_connect_dialog(ctx);
        }

        // Top bar.
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Cuervo Control Plane")
                        .strong()
                        .color(CuervoTheme::ACCENT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (color, label) = match &self.state.connection {
                        ConnectionState::Connected => (CuervoTheme::SUCCESS, "Connected"),
                        ConnectionState::Connecting => (CuervoTheme::WARNING, "Connecting"),
                        ConnectionState::Disconnected => (CuervoTheme::TEXT_MUTED, "Disconnected"),
                        ConnectionState::Error(_) => (CuervoTheme::ERROR, "Error"),
                    };
                    ui.colored_label(color, format!("\u{25CF} {label}"));
                });
            });
        });

        // Status bar.
        egui::TopBottomPanel::bottom("status_bar")
            .max_height(24.0)
            .show(ctx, |ui| {
                self.render_status_bar(ui);
            });

        // Sidebar.
        egui::SidePanel::left("sidebar")
            .resizable(false)
            .default_width(130.0)
            .show(ctx, |ui| {
                self.render_sidebar(ui);
            });

        // Central panel — active view.
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.state.active_view {
                ActiveView::Dashboard => views::dashboard::render(ui, &self.state),
                ActiveView::Agents => {
                    views::agents::render(ui, &mut self.state, &self.cmd_tx);
                }
                ActiveView::Tasks => {
                    views::tasks::render(ui, &mut self.state, &self.cmd_tx);
                }
                ActiveView::Tools => {
                    views::tools::render(ui, &self.state, &self.cmd_tx);
                }
                ActiveView::Logs => views::logs::render(ui, &mut self.state),
                ActiveView::Metrics => views::metrics::render(ui, &self.state),
                ActiveView::Protocols => views::protocols::render(ui, &self.state),
                ActiveView::Files => views::files::render(ui, &self.state),
                ActiveView::Settings => {
                    views::settings::render(
                        ui,
                        &mut self.state,
                        &mut self.config,
                        &self.cmd_tx,
                    );
                }
            }
        });
    }
}
