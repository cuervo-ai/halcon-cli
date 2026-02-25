//! Activity panel widget — scrollable real-time event feed.
//!
//! Renders the recent `WsServerEvent` buffer as a compact, color-coded list.
//! Designed to be embedded in a side panel or collapsible section of any view.

use egui::{RichText, ScrollArea};
use halcon_api::types::ws::WsServerEvent;

use crate::theme::HalconTheme;

/// Maximum events to display in the activity panel at once.
const MAX_VISIBLE_EVENTS: usize = 200;

/// Render the activity panel inside the given `ui`.
///
/// `events` — the most-recent events buffer (front = oldest, back = newest).
/// `title` — optional section header (pass `None` to omit).
pub fn show(
    ui: &mut egui::Ui,
    events: &std::collections::VecDeque<WsServerEvent>,
    title: Option<&str>,
) {
    if let Some(t) = title {
        ui.label(RichText::new(t).strong().color(HalconTheme::ACCENT).size(12.0));
        ui.add_space(4.0);
    }

    if events.is_empty() {
        ui.colored_label(HalconTheme::TEXT_MUTED, "No events yet.");
        return;
    }

    let skip = events.len().saturating_sub(MAX_VISIBLE_EVENTS);

    ScrollArea::vertical()
        .id_salt("activity_panel_scroll")
        .auto_shrink([false, true])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for event in events.iter().skip(skip) {
                render_event_row(ui, event);
            }
        });
}

fn render_event_row(ui: &mut egui::Ui, event: &WsServerEvent) {
    let (icon, label, color) = describe_event(event);

    ui.horizontal(|ui| {
        ui.colored_label(color, RichText::new(icon).size(10.0));
        ui.add_space(2.0);
        ui.colored_label(
            HalconTheme::TEXT_SECONDARY,
            RichText::new(label).size(11.0),
        );
    });
}

fn describe_event(event: &WsServerEvent) -> (&'static str, String, egui::Color32) {
    match event {
        WsServerEvent::AgentRegistered { agent } => (
            "●",
            format!("Agent registered: {}", agent.name),
            HalconTheme::SUCCESS,
        ),
        WsServerEvent::AgentDeregistered { id } => (
            "○",
            format!("Agent deregistered: {}", &id.to_string()[..8]),
            HalconTheme::TEXT_MUTED,
        ),
        WsServerEvent::AgentHealthChanged { id, .. } => (
            "♥",
            format!("Health changed: {}", &id.to_string()[..8]),
            HalconTheme::WARNING,
        ),
        WsServerEvent::AgentInvoked { id, .. } => (
            "▶",
            format!("Agent invoked: {}", &id.to_string()[..8]),
            HalconTheme::ACCENT,
        ),
        WsServerEvent::AgentCompleted { id, success, .. } => {
            let icon = if *success { "✓" } else { "✗" };
            let color = if *success { HalconTheme::SUCCESS } else { HalconTheme::ERROR };
            (icon, format!("Agent completed: {}", &id.to_string()[..8]), color)
        }
        WsServerEvent::TaskSubmitted { execution_id, node_count } => (
            "▶",
            format!("Task submitted: {} ({} nodes)", &execution_id.to_string()[..8], node_count),
            HalconTheme::ACCENT,
        ),
        WsServerEvent::TaskProgress(_) => (
            "~",
            "Task progress update".to_string(),
            HalconTheme::TEXT_MUTED,
        ),
        WsServerEvent::TaskCompleted { execution_id, success, .. } => {
            let icon = if *success { "✓" } else { "✗" };
            let color = if *success { HalconTheme::SUCCESS } else { HalconTheme::ERROR };
            (icon, format!("Task completed: {}", &execution_id.to_string()[..8]), color)
        }
        WsServerEvent::ToolExecuted { name, success, duration_ms, .. } => {
            let icon = if *success { "⚙" } else { "⚠" };
            let color = if *success { HalconTheme::TEXT_SECONDARY } else { HalconTheme::WARNING };
            (icon, format!("Tool {}: {}ms", name, duration_ms), color)
        }
        WsServerEvent::Log(entry) => {
            use halcon_api::types::observability::LogLevel;
            let color = match entry.level {
                LogLevel::Error => HalconTheme::ERROR,
                LogLevel::Warn => HalconTheme::WARNING,
                LogLevel::Debug | LogLevel::Trace => HalconTheme::TEXT_MUTED,
                LogLevel::Info => HalconTheme::TEXT_SECONDARY,
            };
            ("▪", format!("[{:?}] {}", entry.level, entry.message), color)
        }
        WsServerEvent::Metric(_) => (
            "~",
            "Metric point".to_string(),
            HalconTheme::TEXT_MUTED,
        ),
        WsServerEvent::Protocol(_) => (
            "◈",
            "Protocol message".to_string(),
            HalconTheme::TEXT_MUTED,
        ),
        WsServerEvent::ConfigChanged { section } => (
            "⚙",
            format!("Config changed: {}", section),
            HalconTheme::ACCENT,
        ),
        WsServerEvent::SystemHealthChanged { health } => (
            "i",
            format!("System health: {}", health),
            HalconTheme::WARNING,
        ),
        WsServerEvent::Error { code, message } => (
            "✗",
            format!("Error {}: {}", code, message),
            HalconTheme::ERROR,
        ),
        WsServerEvent::Connected { server_version } => (
            "●",
            format!("Connected (server {})", server_version),
            HalconTheme::SUCCESS,
        ),
        WsServerEvent::Pong => ("·", "Pong".to_string(), HalconTheme::TEXT_MUTED),

        // Chat events
        WsServerEvent::ChatStreamToken { session_id, .. } => (
            "»",
            format!("Token stream: {}", &session_id.to_string()[..8]),
            HalconTheme::TEXT_MUTED,
        ),
        WsServerEvent::ThinkingProgress { session_id, chars_so_far, .. } => (
            "…",
            format!("Thinking: {}c ({})", chars_so_far, &session_id.to_string()[..8]),
            HalconTheme::TEXT_MUTED,
        ),
        WsServerEvent::PermissionRequired { tool_name, risk_level, .. } => (
            "⚠",
            format!("Permission required: {} [{}]", tool_name, risk_level),
            HalconTheme::WARNING,
        ),
        WsServerEvent::PermissionResolved { decision, .. } => (
            "✓",
            format!("Permission resolved: {}", decision),
            HalconTheme::TEXT_SECONDARY,
        ),
        WsServerEvent::PermissionExpired { request_id, .. } => (
            "⊘",
            format!("Permission expired: {}", &request_id.to_string()[..8]),
            HalconTheme::WARNING,
        ),
        WsServerEvent::SubAgentStarted { sub_agent_id, task_description, .. } => (
            "▷",
            format!("Sub-agent {}: {}", &sub_agent_id[..8.min(sub_agent_id.len())], task_description),
            HalconTheme::ACCENT,
        ),
        WsServerEvent::SubAgentCompleted { sub_agent_id, success, duration_ms, .. } => {
            let icon = if *success { "▶" } else { "▷" };
            let color = if *success { HalconTheme::SUCCESS } else { HalconTheme::ERROR };
            (icon, format!("Sub-agent {} done: {}ms", &sub_agent_id[..8.min(sub_agent_id.len())], duration_ms), color)
        }
        WsServerEvent::ExecutionFailed { error_code, recoverable, .. } => (
            "✗",
            format!("Execution failed: {} (recoverable: {})", error_code, recoverable),
            HalconTheme::ERROR,
        ),
        WsServerEvent::ConversationCompleted { session_id, stop_reason, .. } => (
            "■",
            format!("Turn done: {} ({})", &session_id.to_string()[..8], stop_reason),
            HalconTheme::SUCCESS,
        ),
        WsServerEvent::ChatSessionCreated { session_id, model, .. } => (
            "+",
            format!("Session created: {} ({})", &session_id.to_string()[..8], model),
            HalconTheme::SUCCESS,
        ),
        WsServerEvent::ChatSessionDeleted { session_id } => (
            "-",
            format!("Session deleted: {}", &session_id.to_string()[..8]),
            HalconTheme::TEXT_MUTED,
        ),
        WsServerEvent::MediaAnalysisStarted { session_id, file_count } => (
            "◈",
            format!("Media analysis: {} file(s) — {}", file_count, &session_id.to_string()[..8]),
            HalconTheme::ACCENT,
        ),
        WsServerEvent::MediaAnalysisProgress { session_id, index, total, .. } => (
            "◈",
            format!("Media {}/{}: {}", index + 1, total, &session_id.to_string()[..8]),
            HalconTheme::TEXT_MUTED,
        ),
        WsServerEvent::MediaAnalysisCompleted { session_id, processed, .. } => (
            "◈",
            format!("Media done: {} processed — {}", processed, &session_id.to_string()[..8]),
            HalconTheme::SUCCESS,
        ),
    }
}
