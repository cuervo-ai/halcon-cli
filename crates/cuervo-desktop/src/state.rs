use cuervo_api::types::agent::AgentInfo;
use cuervo_api::types::config::RuntimeConfigResponse;
use cuervo_api::types::observability::{LogEntry, MetricsSnapshot};
use cuervo_api::types::system::SystemStatus;
use cuervo_api::types::task::TaskExecution;
use cuervo_api::types::tool::ToolInfo;
use cuervo_api::types::ws::WsServerEvent;
use std::collections::VecDeque;

/// Which view is currently active in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    Dashboard,
    Agents,
    Tasks,
    Tools,
    Logs,
    Metrics,
    Protocols,
    Files,
    Settings,
}

/// Connection state to the runtime API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// All application state for the desktop UI.
pub struct AppState {
    // Navigation
    pub active_view: ActiveView,

    // Connection
    pub connection: ConnectionState,

    // Data
    pub agents: Vec<AgentInfo>,
    pub tasks: Vec<TaskExecution>,
    pub tools: Vec<ToolInfo>,
    pub logs: VecDeque<LogEntry>,
    pub events: VecDeque<WsServerEvent>,
    pub metrics: Option<MetricsSnapshot>,
    pub system_status: Option<SystemStatus>,

    // Runtime config (from server)
    pub runtime_config: Option<RuntimeConfigResponse>,
    pub config_dirty: bool,
    pub config_error: Option<String>,

    // UI state
    pub log_search: String,
    pub log_level_filter: String,
    pub selected_agent: Option<uuid::Uuid>,
    pub selected_task: Option<uuid::Uuid>,
    pub show_connect_dialog: bool,

    // Limits
    pub max_log_entries: usize,
    pub max_events: usize,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            active_view: ActiveView::Dashboard,
            connection: ConnectionState::Disconnected,
            agents: Vec::new(),
            tasks: Vec::new(),
            tools: Vec::new(),
            logs: VecDeque::new(),
            events: VecDeque::new(),
            metrics: None,
            system_status: None,
            runtime_config: None,
            config_dirty: false,
            config_error: None,
            log_search: String::new(),
            log_level_filter: String::new(),
            selected_agent: None,
            selected_task: None,
            show_connect_dialog: true,
            max_log_entries: 10_000,
            max_events: 5_000,
        }
    }
}

impl AppState {
    /// Push an event, enforcing the max buffer size.
    pub fn push_event(&mut self, event: WsServerEvent) {
        if self.events.len() >= self.max_events {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    /// Push a log entry, enforcing the max buffer size.
    pub fn push_log(&mut self, entry: LogEntry) {
        if self.logs.len() >= self.max_log_entries {
            self.logs.pop_front();
        }
        self.logs.push_back(entry);
    }
}
