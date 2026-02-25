pub mod chat_handlers;
pub mod connection;
pub mod file_handlers;
pub mod media_handlers;
pub mod poller;
pub mod ws_loop;
pub mod ws_translator;

use std::sync::Arc;

/// Type-erased repaint callback.  Calling it tells egui to redraw the next
/// frame.  Passed from `HalconApp` to all background workers and sub-modules.
pub type RepaintFn = Arc<dyn Fn() + Send + Sync>;

use halcon_api::types::agent::AgentInfo;
use halcon_api::types::config::RuntimeConfigResponse;
use halcon_api::types::observability::MetricsSnapshot;
use halcon_api::types::system::SystemStatus;
use halcon_api::types::task::TaskExecution;
use halcon_api::types::tool::ToolInfo;
use halcon_api::types::ws::WsServerEvent;
use halcon_client::UpdateConfigRequest;

/// A single entry in a directory listing (used by the file explorer).
#[derive(Debug, Clone)]
pub struct FileDirEntry {
    pub name: String,
    pub path: std::path::PathBuf,
    pub is_dir: bool,
}

/// Messages sent from background workers to the UI thread.
// All session-scoped messages include `session_id` so `app.rs` can route them
// to the correct ChatState regardless of which session is currently active.
#[derive(Debug)]
#[allow(dead_code)]
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

    // Chat
    ChatSessionCreated(halcon_api::types::chat::ChatSession),
    ChatMessageReceived {
        session_id: uuid::Uuid,
        token: String,
        is_thinking: bool,
        sequence_num: u64,
    },
    ChatTurnCompleted {
        session_id: uuid::Uuid,
        assistant_text: String,
        stop_reason: String,
        /// C4: Server-measured end-to-end turn duration (from ConversationCompleted).
        total_duration_ms: u64,
    },
    ChatTurnFailed {
        session_id: uuid::Uuid,
        error: String,
        recoverable: bool,
    },
    ChatPermissionRequired {
        session_id: uuid::Uuid,
        request_id: uuid::Uuid,
        tool_name: String,
        risk_level: String,
        description: String,
        deadline_secs: u64,
    },
    /// B1: A permission request timed out — the tool was automatically denied.
    /// The UI must dismiss any pending permission modal for this request_id.
    ChatPermissionExpired {
        session_id: uuid::Uuid,
        request_id: uuid::Uuid,
    },
    ChatSessionsLoaded(Vec<halcon_api::types::chat::ChatSession>),
    ChatMessagesLoaded {
        session_id: uuid::Uuid,
        messages: Vec<halcon_api::types::chat::ChatMessageEntry>,
    },
    // Sub-agent lifecycle (translated from WS events for the active session)
    SubAgentStarted {
        session_id: uuid::Uuid,
        sub_agent_id: String,
        description: String,
        wave: usize,
        allowed_tools: Vec<String>,
    },
    SubAgentCompleted {
        session_id: uuid::Uuid,
        sub_agent_id: String,
        success: bool,
        summary: String,
        duration_ms: u64,
        /// C3: Tools actually invoked during this sub-agent's execution.
        tools_used: Vec<String>,
    },
    // Session metadata updated
    ChatSessionRenamed {
        session_id: uuid::Uuid,
        title: String,
    },
    // Session metadata updated (already above, keeping Event at the bottom)
    // Streaming events forwarded as-is for the generic event buffer.
    #[allow(dead_code)]
    Event(WsServerEvent),

    // File explorer
    DirectoryLoaded {
        path: std::path::PathBuf,
        /// Entries sorted: directories first (alphabetical), then files (alphabetical).
        entries: Vec<FileDirEntry>,
    },
    FileLoaded {
        path: std::path::PathBuf,
        content: String,
    },
    FileError {
        path: std::path::PathBuf,
        error: String,
    },

    /// Error from a fire-and-forget operation (InvokeAgent, SubmitTask).
    OperationError(String),

    // Multimodal attachments
    /// A file was successfully read + encoded as base64, ready to attach.
    AttachmentReady(crate::state::DesktopAttachment),
    /// A file could not be read or encoded.
    AttachmentError {
        path: std::path::PathBuf,
        error: String,
    },
    /// Server reported media analysis progress for the active session.
    MediaAnalysisProgress {
        session_id: uuid::Uuid,
        index: usize,
        total: usize,
        filename: String,
    },
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
    // Chat commands
    CreateChatSession {
        model: String,
        provider: String,
        title: Option<String>,
    },
    LoadChatSessions,
    LoadChatMessages { session_id: uuid::Uuid },
    SendChatMessage {
        session_id: uuid::Uuid,
        content: String,
        orchestrate: bool,
        /// Inline media attachments to forward with this message.
        attachments: Vec<halcon_api::types::chat::MediaAttachmentInline>,
    },
    CancelChatExecution {
        session_id: uuid::Uuid,
    },
    ResolvePermission {
        session_id: uuid::Uuid,
        request_id: uuid::Uuid,
        approve: bool,
    },
    DeleteChatSession {
        session_id: uuid::Uuid,
    },
    RenameChatSession {
        session_id: uuid::Uuid,
        title: String,
    },

    // Multimodal attachments
    /// Read the file at `path`, detect MIME type, encode as base64.
    /// Result: `BackendMessage::AttachmentReady` or `BackendMessage::AttachmentError`.
    AttachFile { path: std::path::PathBuf },
    /// Remove the attachment at the given index from `ChatState::pending_attachments`.
    /// Handled entirely in-process (no backend call needed).
    RemoveAttachment { index: usize },
    /// Clear all pending attachments for the active session.
    ClearAttachments,

    // File explorer — async IO offloaded to the tokio worker
    /// Load the direct children of `path`.  Result: `BackendMessage::DirectoryLoaded`.
    LoadDirectory { path: std::path::PathBuf },
    /// Read a file.  Result: `BackendMessage::FileLoaded` (truncated to 64 KB).
    LoadFile { path: std::path::PathBuf },

    // Agent / task operations
    /// Invoke a registered agent with a free-text instruction.
    InvokeAgent {
        agent_id: uuid::Uuid,
        instruction: String,
    },
    /// Submit a single-node task DAG.  `agent_id = None` lets the server choose.
    SubmitTask {
        instruction: String,
        agent_id: Option<uuid::Uuid>,
    },
}
