pub mod connection;
pub mod poller;

use cuervo_api::types::agent::AgentInfo;
use cuervo_api::types::config::RuntimeConfigResponse;
use cuervo_api::types::observability::MetricsSnapshot;
use cuervo_api::types::system::SystemStatus;
use cuervo_api::types::task::TaskExecution;
use cuervo_api::types::tool::ToolInfo;
use cuervo_api::types::ws::WsServerEvent;
use cuervo_client::UpdateConfigRequest;

/// Messages sent from background workers to the UI thread.
#[derive(Debug)]
pub enum BackendMessage {
    // Connection lifecycle
    Connected,
    Disconnected(String),
    ConnectionError(String),

    // Data updates
    AgentsUpdated(Vec<AgentInfo>),
    TasksUpdated(Vec<TaskExecution>),
    ToolsUpdated(Vec<ToolInfo>),
    MetricsUpdated(MetricsSnapshot),
    SystemStatusUpdated(SystemStatus),

    // Config
    ConfigLoaded(RuntimeConfigResponse),
    ConfigUpdated(RuntimeConfigResponse),
    ConfigError(String),

    // Streaming events (wired in Phase 3 via WebSocket worker)
    #[allow(dead_code)]
    Event(WsServerEvent),
}

/// Commands sent from the UI thread to background workers.
#[derive(Debug)]
pub enum UiCommand {
    Connect {
        url: String,
        token: String,
    },
    Disconnect,
    RefreshAgents,
    RefreshTasks,
    RefreshTools,
    RefreshMetrics,
    RefreshStatus,
    RefreshConfig,
    UpdateConfig(Box<UpdateConfigRequest>),
    StopAgent(uuid::Uuid),
    CancelTask(uuid::Uuid),
    ToggleTool {
        name: String,
        enabled: bool,
    },
    Shutdown {
        graceful: bool,
    },
}
