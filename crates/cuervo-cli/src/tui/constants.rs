//! TUI text constants — centralized to avoid duplication.

/// Dry-run banner label for status bar (short form).
pub const DRY_RUN_LABEL: &str = " DRY-RUN ";

/// Dry-run warning message for activity zone (detailed).
pub const DRY_RUN_WARNING: &str = "DRY-RUN MODE: Destructive tools will be skipped";

/// Dry-run hint shown alongside the warning.
pub const DRY_RUN_HINT: &str = "Disable with tools.dry_run = false in config";

/// Dry-run toast notification (brief).
pub const DRY_RUN_TOAST: &str = "DRY-RUN mode active";

// --- Event ring buffer labels (interned to reduce allocations) ---

/// Event label: stream chunk received.
pub const EVENT_STREAM_CHUNK: &str = "StreamChunk";

/// Event label: stream completed.
pub const EVENT_STREAM_DONE: &str = "StreamDone";

/// Event label: spinner animation stopped.
pub const EVENT_SPINNER_STOP: &str = "SpinnerStop";

/// Event label: status bar update.
pub const EVENT_STATUS_UPDATE: &str = "StatusUpdate";

/// Event label: agent execution completed.
pub const EVENT_AGENT_DONE: &str = "AgentDone";

/// Event label: application quit requested.
pub const EVENT_QUIT: &str = "Quit";

/// Event label: redraw UI.
pub const EVENT_REDRAW: &str = "Redraw";

/// Event label: reflection started.
pub const EVENT_REFLECTION_START: &str = "ReflectionStart";

/// Event label: reflection completed.
pub const EVENT_REFLECTION_DONE: &str = "ReflectionDone";

/// Event label: consolidation status.
pub const EVENT_CONSOLIDATION: &str = "Consolidation";

/// Event label: context tier update.
pub const EVENT_CONTEXT_UPDATE: &str = "ContextUpdate";

/// Event label: token budget update.
pub const EVENT_TOKEN_BUDGET: &str = "TokenBudget";

/// Event label: context compaction completed.
pub const EVENT_COMPACTION: &str = "Compaction";

// --- Help overlay text sections (extracted to reduce render_help() LOC) ---

/// Help section: Navigation keybindings.
pub const HELP_SECTION_NAVIGATION: &[(&str, &str)] = &[
    ("Ctrl+Enter", "Submit prompt"),
    ("Enter", "New line in prompt"),
    ("Tab", "Cycle focus (Prompt ↔ Activity)"),
    ("Ctrl+K", "Clear prompt"),
    ("Ctrl+↑/↓", "Prompt history back/forward"),
    ("Shift+↑/↓", "Scroll activity up/down"),
    ("PgUp/PgDn", "Scroll activity up/down"),
    ("End", "Scroll to bottom"),
];

/// Help section: Panels & Overlays keybindings.
pub const HELP_SECTION_PANELS: &[(&str, &str)] = &[
    ("F1", "This help overlay"),
    ("F2", "Toggle side panel"),
    ("F3", "Cycle UI mode (Minimal → Standard → Expert)"),
    ("F4", "Cycle panel section"),
    ("Ctrl+P", "Command palette"),
    ("Ctrl+F", "Search activity"),
];

/// Help section: Agent Control keybindings.
pub const HELP_SECTION_AGENT: &[(&str, &str)] = &[
    ("Space", "Pause / Resume agent"),
    ("N", "Step mode (run one turn then pause)"),
    ("Esc", "Cancel running agent"),
    ("Y", "Approve pending tool execution"),
    ("Shift+N", "Reject pending tool execution"),
];

/// Help section: General keybindings.
pub const HELP_SECTION_GENERAL: &[(&str, &str)] = &[
    ("Ctrl+C", "Quit application"),
    ("Ctrl+D", "Quit application"),
    ("Ctrl+T", "Dismiss all toasts"),
    ("/", "Open command palette"),
];

/// Help section headers.
pub const HELP_HEADER_NAVIGATION: &str = "  Navigation";
pub const HELP_HEADER_PANELS: &str = "  Panels & Overlays";
pub const HELP_HEADER_AGENT: &str = "  Agent Control (while agent is running)";
pub const HELP_HEADER_GENERAL: &str = "  General";
