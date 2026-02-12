pub mod agent;
pub mod config;
pub mod observability;
pub mod protocol;
pub mod system;
pub mod task;
pub mod tool;
pub mod ws;

// Re-export key types at the types level for convenience.
pub use agent::*;
pub use config::{RuntimeConfigResponse, UpdateConfigRequest};
pub use observability::{LogEntry, LogLevel, MetricPoint, MetricsSnapshot};
pub use protocol::ProtocolMessageInfo;
pub use system::{SystemHealth, SystemStatus};
pub use task::{TaskExecution, TaskProgressEvent, TaskStatus};
pub use tool::{PermissionLevel, ToolInfo};
pub use ws::{WsChannel, WsClientMessage, WsServerEvent};
