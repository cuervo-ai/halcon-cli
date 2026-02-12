use cuervo_core::types::AppConfig;
use cuervo_runtime::runtime::CuervoRuntime;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

use crate::types::ws::WsServerEvent;

/// Shared application state for the API server.
#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<CuervoRuntime>,
    pub auth_token: Arc<String>,
    pub started_at: Instant,
    pub event_tx: broadcast::Sender<WsServerEvent>,
    pub tool_states: Arc<RwLock<HashMap<String, ToolState>>>,
    pub task_executions: Arc<RwLock<HashMap<Uuid, crate::types::task::TaskExecution>>>,
    pub config: Arc<RwLock<AppConfig>>,
}

/// Tracked state for a tool (enable/disable, execution count).
#[derive(Debug, Clone)]
pub struct ToolState {
    pub enabled: bool,
    pub execution_count: u64,
    pub last_executed: Option<chrono::DateTime<chrono::Utc>>,
}

impl AppState {
    /// Create new server state wrapping the given runtime.
    pub fn new(runtime: Arc<CuervoRuntime>, auth_token: String) -> Self {
        let (event_tx, _) = broadcast::channel(4096);
        Self {
            runtime,
            auth_token: Arc::new(auth_token),
            started_at: Instant::now(),
            event_tx,
            tool_states: Arc::new(RwLock::new(HashMap::new())),
            task_executions: Arc::new(RwLock::new(HashMap::new())),
            config: Arc::new(RwLock::new(AppConfig::default())),
        }
    }

    /// Broadcast a WebSocket event to all connected clients.
    pub fn broadcast(&self, event: WsServerEvent) {
        // Ignore send error (no subscribers).
        let _ = self.event_tx.send(event);
    }

    /// Get uptime in seconds.
    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}
