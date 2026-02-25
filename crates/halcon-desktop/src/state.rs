use halcon_api::types::agent::AgentInfo;
use halcon_api::types::config::RuntimeConfigResponse;
use halcon_api::types::observability::{LogEntry, MetricsSnapshot};
use halcon_api::types::system::SystemStatus;
use halcon_api::types::task::TaskExecution;
use halcon_api::types::tool::ToolInfo;
use halcon_api::types::ws::WsServerEvent;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use crate::widgets::metric_chart::MetricChart;
use crate::workers::FileDirEntry;

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
    Chat,
}

/// Connection state to the runtime API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

// ── Domain sub-states ─────────────────────────────────────────────────────────

/// Number of messages shown per "page" in the chat view.
pub const CHAT_PAGE_SIZE: usize = 50;

/// All chat-related state: sessions, streaming, permissions, error recovery.
pub struct ChatState {
    pub sessions: Vec<halcon_api::types::chat::ChatSession>,
    pub active_session: Option<uuid::Uuid>,
    pub input: String,
    pub messages: VecDeque<ChatDisplayMessage>,
    pub streaming_token: String,
    pub streaming_token_count: usize,
    pub is_streaming: bool,
    pub error: Option<String>,
    pub permission_modal: Option<ChatPermissionModal>,
    /// When the current turn started — used by ThinkingBubble for elapsed time.
    pub turn_started_at: Option<std::time::Instant>,
    // New session dialog
    pub new_session_model: String,
    pub new_session_provider: String,
    pub show_new_session_dialog: bool,
    // Sub-agent tracking (most recent N agents for the active session)
    pub sub_agents: Vec<SubAgentEntry>,
    // Inline session rename state
    pub rename_session_id: Option<uuid::Uuid>,
    pub rename_buffer: String,
    // Error recovery
    pub error_recoverable: bool,
    pub retry_count: u32,
    /// The last user message — (content, orchestrate) — cached for retry.
    pub last_message: Option<(String, bool)>,
    /// Persistent markdown render cache shared across all chat messages.
    pub md_cache: egui_commonmark::CommonMarkCache,
    /// Media attachments queued for the next message send.
    pub pending_attachments: Vec<DesktopAttachment>,
    /// True while an async file-read / attachment encode is in progress.
    pub is_uploading_attachment: bool,
    /// How many of the most-recent messages to render (client-side window).
    pub messages_visible_count: usize,
    /// True while a LoadChatMessages command is in flight (session switch).
    pub messages_loading: bool,
    /// Tracks the highest sequence number seen in this turn.  Used to drop
    /// duplicate tokens that may re-arrive after a WS reconnect.
    pub last_sequence_num: Option<u64>,
    /// Whether the next message should be sent with orchestrate=true (sub-agents).
    pub orchestrate: bool,
    /// Whether the live-activity panel is expanded in the chat view.
    pub show_activity_panel: bool,
    /// C1: Number of sequence-number gaps detected in the current session.
    /// A non-zero value indicates dropped tokens (WS overflow or reconnect).
    pub gaps_detected: u32,
    /// C4: Duration of the last completed turn (from ConversationCompleted.total_duration_ms).
    pub last_turn_duration_ms: Option<u64>,
    /// E2: In-progress media analysis: (current_index, total, filename).
    /// Set from MediaAnalysisProgress WS events, cleared on turn complete/fail.
    pub media_analysis_progress: Option<(usize, usize, String)>,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            active_session: None,
            input: String::new(),
            messages: VecDeque::new(),
            streaming_token: String::new(),
            streaming_token_count: 0,
            is_streaming: false,
            error: None,
            permission_modal: None,
            turn_started_at: None,
            new_session_model: "deepseek-chat".to_string(),
            new_session_provider: "deepseek".to_string(),
            show_new_session_dialog: false,
            sub_agents: Vec::new(),
            rename_session_id: None,
            rename_buffer: String::new(),
            error_recoverable: false,
            retry_count: 0,
            last_message: None,
            md_cache: egui_commonmark::CommonMarkCache::default(),
            pending_attachments: Vec::new(),
            is_uploading_attachment: false,
            messages_visible_count: CHAT_PAGE_SIZE,
            messages_loading: false,
            last_sequence_num: None,
            orchestrate: false,
            show_activity_panel: false,
            gaps_detected: 0,
            last_turn_duration_ms: None,
            media_analysis_progress: None,
        }
    }
}

/// File explorer state: directory tree, selection, lazy-load cache.
pub struct FileState {
    /// User-editable project root path (text input).
    pub root: String,
    /// Cache of directory listings keyed by path.  Populated lazily as dirs expand.
    pub dir_cache: HashMap<PathBuf, Vec<FileDirEntry>>,
    /// Set of directory paths whose children are currently shown in the tree.
    pub expanded: HashSet<PathBuf>,
    /// Currently selected file path.
    pub selected: Option<PathBuf>,
    /// Content of the currently selected file (truncated to 64 KB).
    pub content: Option<String>,
    /// True while a LoadDirectory or LoadFile command is in flight.
    pub loading: bool,
    /// Last IO error from the file explorer.
    pub error: Option<String>,
}

impl Default for FileState {
    fn default() -> Self {
        Self {
            root: String::new(),
            dir_cache: HashMap::new(),
            expanded: HashSet::new(),
            selected: None,
            content: None,
            loading: false,
            error: None,
        }
    }
}

/// Agent / task fire-and-forget operation inputs and last error.
pub struct OpsState {
    /// Text buffer for the inline agent invocation input.
    pub invoke_agent_input: String,
    /// Text buffer for the inline task submission input.
    pub submit_task_input: String,
    /// Error from the last InvokeAgent or SubmitTask command.
    pub error: Option<String>,
}

impl Default for OpsState {
    fn default() -> Self {
        Self {
            invoke_agent_input: String::new(),
            submit_task_input: String::new(),
            error: None,
        }
    }
}

/// Rolling metric trend charts (fed on each MetricsUpdated, rendered in Metrics view).
pub struct MetricChartsState {
    /// Events per second — last 60 samples.
    pub events_per_sec: MetricChart,
    /// Active task count — last 60 samples.
    pub active_tasks: MetricChart,
}

impl Default for MetricChartsState {
    fn default() -> Self {
        Self {
            events_per_sec: MetricChart::new("Events/s", 60),
            active_tasks: MetricChart::new("Active Tasks", 60),
        }
    }
}

// ── AppState ──────────────────────────────────────────────────────────────────

/// All application state for the desktop UI.
pub struct AppState {
    // Navigation
    pub active_view: ActiveView,

    // Connection
    pub connection: ConnectionState,

    // Backend data
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

    // UI prefs
    pub log_search: String,
    pub log_level_filter: String,
    pub selected_agent: Option<uuid::Uuid>,
    pub selected_task: Option<uuid::Uuid>,
    pub show_connect_dialog: bool,

    // Limits
    pub max_log_entries: usize,
    pub max_events: usize,

    /// When set, the app will attempt to reconnect at this instant.
    pub reconnect_after: Option<std::time::Instant>,

    // ── Domain sub-states ─────────────────────────────────────────────────────
    pub chat: ChatState,
    pub files: FileState,
    pub ops: OpsState,
    pub charts: MetricChartsState,
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
            reconnect_after: None,
            chat: ChatState::default(),
            files: FileState::default(),
            ops: OpsState::default(),
            charts: MetricChartsState::default(),
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

// ── Supporting types ──────────────────────────────────────────────────────────

/// A message to display in the chat view.
#[derive(Debug, Clone)]
pub struct ChatDisplayMessage {
    /// Unique identifier — used as egui widget id_salt for stable layout.
    pub id: uuid::Uuid,
    pub role: ChatDisplayRole,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Role indicator for chat display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatDisplayRole {
    User,
    Assistant,
    System,
}

/// A pending permission modal for the chat view.
#[derive(Debug, Clone)]
pub struct ChatPermissionModal {
    pub request_id: uuid::Uuid,
    pub tool_name: String,
    pub risk_level: String,
    pub description: String,
    pub deadline_secs: u64,
    pub created_at: std::time::Instant,
}

/// A media attachment pending to be sent with the next chat message.
#[derive(Debug, Clone)]
pub struct DesktopAttachment {
    /// Unique identifier (local, not server-side).
    pub id: uuid::Uuid,
    /// Original file name shown in the UI chip.
    pub name: String,
    /// Absolute path on disk (used for drag-and-drop display).
    pub path: std::path::PathBuf,
    /// MIME type detected from magic bytes or file extension.
    pub content_type: String,
    /// Size of the raw file in bytes.
    pub size_bytes: usize,
    /// Base64-encoded content (ready to send).
    pub data_base64: String,
}

impl DesktopAttachment {
    /// Primary modality label for display.
    pub fn modality_label(&self) -> &'static str {
        if self.content_type.starts_with("image/") {
            "image"
        } else if self.content_type.starts_with("audio/") {
            "audio"
        } else if self.content_type.starts_with("video/") {
            "video"
        } else {
            "text"
        }
    }

    /// Emoji icon for the modality chip.
    pub fn icon(&self) -> &'static str {
        match self.modality_label() {
            "image" => "🖼",
            "audio" => "🔊",
            "video" => "🎬",
            _       => "📄",
        }
    }

    /// Human-readable file size.
    pub fn size_label(&self) -> String {
        if self.size_bytes >= 1_048_576 {
            format!("{:.1}MB", self.size_bytes as f64 / 1_048_576.0)
        } else if self.size_bytes >= 1_024 {
            format!("{:.0}KB", self.size_bytes as f64 / 1_024.0)
        } else {
            format!("{}B", self.size_bytes)
        }
    }
}

/// A sub-agent execution entry tracked in the chat view.
#[derive(Debug, Clone)]
pub struct SubAgentEntry {
    pub sub_agent_id: String,
    pub description: String,
    pub wave: usize,
    pub allowed_tools: Vec<String>,
    /// None = still running, Some(true) = success, Some(false) = failed.
    pub success: Option<bool>,
    pub summary: Option<String>,
    pub duration_ms: Option<u64>,
    /// C3: Tools actually called by this sub-agent (from SubAgentCompleted.tools_used).
    pub tools_used: Vec<String>,
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_api::types::observability::{LogEntry, LogLevel};

    fn make_log() -> LogEntry {
        LogEntry {
            level: LogLevel::Info,
            target: "test".to_string(),
            message: "test message".to_string(),
            timestamp: chrono::Utc::now(),
            fields: Default::default(),
            span: None,
        }
    }

    #[test]
    fn push_log_respects_max() {
        let mut state = AppState::default();
        state.max_log_entries = 3;
        for _ in 0..5 {
            state.push_log(make_log());
        }
        assert_eq!(state.logs.len(), 3);
    }

    #[test]
    fn push_event_respects_max() {
        let mut state = AppState::default();
        state.max_events = 2;
        use halcon_api::types::ws::WsServerEvent;
        for _ in 0..4 {
            state.push_event(WsServerEvent::Pong);
        }
        assert_eq!(state.events.len(), 2);
    }

    #[test]
    fn chat_state_defaults() {
        let cs = ChatState::default();
        assert_eq!(cs.messages_visible_count, CHAT_PAGE_SIZE);
        assert!(!cs.messages_loading);
        assert!(cs.last_sequence_num.is_none());
        assert!(cs.messages.is_empty());
        assert!(!cs.is_streaming);
        assert_eq!(cs.retry_count, 0);
        assert!(!cs.orchestrate);
        assert!(!cs.show_activity_panel);
    }

    #[test]
    fn file_state_defaults() {
        let fs = FileState::default();
        assert!(fs.root.is_empty());
        assert!(!fs.loading);
        assert!(fs.error.is_none());
        assert!(fs.content.is_none());
    }

    #[test]
    fn ops_state_defaults() {
        let ops = OpsState::default();
        assert!(ops.invoke_agent_input.is_empty());
        assert!(ops.submit_task_input.is_empty());
        assert!(ops.error.is_none());
    }

    #[test]
    fn chat_page_size_constant() {
        assert!(CHAT_PAGE_SIZE > 0);
    }
}
