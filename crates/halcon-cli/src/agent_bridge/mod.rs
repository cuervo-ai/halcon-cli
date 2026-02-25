//! AgentBridge — headless adapter between the halcon agent pipeline and HTTP/WS clients.
//!
//! Feature-gated behind `headless` (automatically enabled by `tui`).
//! Does NOT import ratatui or UiEvent — clean separation from presentation layer.

pub mod traits;
pub mod types;
pub mod bridge_sink;
pub mod executor;
// Phase 4: GDEM bridge — compiled only with feature = "gdem-primary"
#[cfg(feature = "gdem-primary")]
pub mod gdem_bridge;

pub use traits::{AgentExecutor, StreamEmitter, PermissionHandler};
pub use types::{
    AgentBridgeError, AgentStreamEvent, ChatTokenUsage, PermissionDecisionKind,
    PermissionRequest, TurnContext, TurnMessage, TurnResult, TurnRole,
};
pub use executor::AgentBridgeImpl;
