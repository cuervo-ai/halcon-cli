use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::model::{ChatMessage, TokenUsage};

/// A conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub title: Option<String>,
    pub model: String,
    pub provider: String,
    pub working_directory: String,
    pub messages: Vec<ChatMessage>,
    pub total_usage: TokenUsage,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Number of tool invocations in this session.
    #[serde(default)]
    pub tool_invocations: u32,
    /// Number of agent loop rounds (model re-invocations after tool use).
    #[serde(default)]
    pub agent_rounds: u32,
    /// Total model invocation latency in milliseconds.
    #[serde(default)]
    pub total_latency_ms: u64,
    /// Estimated cumulative cost in USD.
    #[serde(default)]
    pub estimated_cost_usd: f64,
    /// SHA-256 fingerprint of the session's message sequence (for replay verification).
    #[serde(default)]
    pub execution_fingerprint: Option<String>,
    /// If this session was produced by a replay, the ID of the original session.
    #[serde(default)]
    pub replay_source_session: Option<String>,
}

impl Session {
    pub fn new(model: String, provider: String, working_directory: String) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            title: None,
            model,
            provider,
            working_directory,
            messages: Vec::new(),
            total_usage: TokenUsage::default(),
            created_at: now,
            updated_at: now,
            tool_invocations: 0,
            agent_rounds: 0,
            total_latency_ms: 0,
            estimated_cost_usd: 0.0,
            execution_fingerprint: None,
            replay_source_session: None,
        }
    }

    pub fn add_message(&mut self, message: ChatMessage) {
        self.messages.push(message);
        self.updated_at = Utc::now();
    }

    pub fn accumulate_usage(&mut self, usage: &TokenUsage) {
        self.total_usage.input_tokens += usage.input_tokens;
        self.total_usage.output_tokens += usage.output_tokens;
        self.updated_at = Utc::now();
    }
}
