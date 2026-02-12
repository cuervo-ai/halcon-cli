//! Response cache types for the semantic response cache.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A cached model response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// SHA-256 hex hash of the cache key components.
    pub cache_key: String,
    /// Model that generated this response.
    pub model: String,
    /// The response text.
    pub response_text: String,
    /// Serialized tool calls (if the response included tool_use).
    pub tool_calls_json: Option<String>,
    /// Stop reason (e.g., "end_turn", "tool_use").
    pub stop_reason: String,
    /// Token usage JSON.
    pub usage_json: String,
    /// When this entry was created.
    pub created_at: DateTime<Utc>,
    /// When this entry expires (None = never).
    pub expires_at: Option<DateTime<Utc>>,
    /// Number of times this entry has been returned as a cache hit.
    pub hit_count: u32,
}

/// Summary statistics for the response cache.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheStats {
    pub total_entries: u32,
    pub total_hits: u64,
    pub oldest_entry: Option<DateTime<Utc>>,
    pub newest_entry: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_entry_serde_roundtrip() {
        let entry = CacheEntry {
            cache_key: "abc123".into(),
            model: "claude-sonnet".into(),
            response_text: "Hello world".into(),
            tool_calls_json: None,
            stop_reason: "end_turn".into(),
            usage_json: r#"{"input_tokens":10,"output_tokens":5}"#.into(),
            created_at: Utc::now(),
            expires_at: None,
            hit_count: 0,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: CacheEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.cache_key, "abc123");
        assert_eq!(parsed.model, "claude-sonnet");
        assert_eq!(parsed.response_text, "Hello world");
    }

    #[test]
    fn cache_stats_default() {
        let stats = CacheStats::default();
        assert_eq!(stats.total_entries, 0);
        assert_eq!(stats.total_hits, 0);
        assert!(stats.oldest_entry.is_none());
    }
}
