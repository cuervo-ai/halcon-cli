use chrono::Utc;
use sha2::Digest;

use halcon_core::types::{ChatMessage, ContentBlock, MessageContent, Role, Session};
use halcon_storage::{AsyncDatabase, TraceStep, TraceStepType};

/// Compute a SHA-256 fingerprint of the message sequence for replay verification.
///
/// Hashes raw content directly instead of JSON-serializing each message,
/// eliminating N string allocations per call (only ToolUse inputs still serialize).
pub fn compute_fingerprint(messages: &[ChatMessage]) -> String {
    let mut hasher = sha2::Sha256::new();
    for msg in messages {
        // Role discriminant as a single byte.
        let role_byte = match msg.role {
            Role::User => b'U',
            Role::Assistant => b'A',
            Role::System => b'S',
        };
        hasher.update([role_byte]);
        // Hash content without JSON serialization.
        match &msg.content {
            MessageContent::Text(t) => {
                hasher.update(b"T");
                hasher.update(t.as_bytes());
            }
            MessageContent::Blocks(blocks) => {
                hasher.update(b"B");
                for block in blocks {
                    match block {
                        ContentBlock::Text { text } => {
                            hasher.update(b"t");
                            hasher.update(text.as_bytes());
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            hasher.update(b"u");
                            hasher.update(id.as_bytes());
                            hasher.update(name.as_bytes());
                            hasher.update(input.to_string().as_bytes());
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            hasher.update(b"r");
                            hasher.update(tool_use_id.as_bytes());
                            hasher.update(content.as_bytes());
                            hasher.update([*is_error as u8]);
                        }
                        ContentBlock::Image { source } => {
                            hasher.update(b"i");
                            if let Ok(src_json) = serde_json::to_string(source) {
                                hasher.update(src_json.as_bytes());
                            }
                        }
                        ContentBlock::AudioTranscript { text, .. } => {
                            hasher.update(b"a");
                            hasher.update(text.as_bytes());
                        }
                    }
                }
            }
        }
    }
    format!("{:x}", hasher.finalize())
}

/// Fire-and-forget trace step recording. Errors are logged but never propagate.
///
/// Uses the provided `ExecutionClock` for timestamps — deterministic in replay,
/// real-time in production.
pub(crate) fn record_trace(
    db: Option<&AsyncDatabase>,
    session_id: uuid::Uuid,
    step_index: &mut u32,
    step_type: TraceStepType,
    data_json: String,
    duration_ms: u64,
    clock: &halcon_core::types::ExecutionClock,
) {
    if let Some(db) = db {
        let step = TraceStep {
            session_id,
            step_index: *step_index,
            step_type,
            data_json,
            duration_ms,
            timestamp: clock.now(),
        };
        if let Err(e) = db.inner().append_trace_step(&step) {
            tracing::warn!("trace recording failed (step {}): {e}", *step_index);
        }
        *step_index += 1;
    }
}

/// Fire-and-forget session checkpoint (lightweight, ~2-5ms).
///
/// Persists the current conversation state so the session can be resumed
/// after a crash or unexpected termination.
pub(crate) fn auto_checkpoint(
    db: Option<&AsyncDatabase>,
    session_id: uuid::Uuid,
    rounds: usize,
    messages: &[ChatMessage],
    session: &Session,
    trace_step_index: u32,
) {
    if let Some(db) = db {
        let checkpoint = halcon_storage::SessionCheckpoint {
            session_id,
            round: rounds as u32,
            step_index: trace_step_index,
            messages_json: serde_json::to_string(messages).unwrap_or_default(),
            usage_json: serde_json::json!({
                "input_tokens": session.total_usage.input_tokens,
                "output_tokens": session.total_usage.output_tokens,
            })
            .to_string(),
            fingerprint: compute_fingerprint(messages),
            created_at: Utc::now(),
            agent_state: None,
        };
        if let Err(e) = db.inner().save_checkpoint(&checkpoint) {
            tracing::warn!("auto-checkpoint failed: {e}");
        }
    }
}

/// Classify a provider error message and return a user-friendly hint.
///
/// Uses case-insensitive matching with separate `.contains()` checks
/// (not regex patterns) for reliable matching.
pub fn classify_error_hint(error: &str) -> &'static str {
    let lower = error.to_lowercase();
    if lower.contains("credit balance")
        || lower.contains("billing")
        || lower.contains("payment")
        || lower.contains("insufficient_quota")
    {
        "Check your account balance at https://console.anthropic.com/settings/billing"
    } else if lower.contains("authentication")
        || lower.contains("invalid api key")
        || lower.contains("invalid_api_key")
        || lower.contains("unauthorized")
        || lower.contains("api key")
    {
        "Verify your API key with `halcon auth status` or set ANTHROPIC_API_KEY"
    } else if lower.contains("429")
        || lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("too many requests")
    {
        "Rate limited — wait a moment and retry, or switch to a different provider"
    } else {
        "Check your API key and network connection"
    }
}
