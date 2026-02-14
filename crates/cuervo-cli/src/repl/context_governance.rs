// Context governance: wired into context assembly (Phase 42) and exposed via /inspect context.
//! Context governance: per-source token limits and provenance tracking.
//!
//! Enforces configurable constraints on how much context each source can
//! contribute, and records provenance metadata for debugging and observability.

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use cuervo_core::traits::ContextChunk;
use cuervo_context::assembler::estimate_tokens;

/// Provenance metadata for a single context contribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ContextProvenance {
    pub source_name: String,
    pub token_count: u32,
    pub priority: u32,
    pub timestamp: DateTime<Utc>,
}

/// Per-source governance configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SourceGovernance {
    /// Maximum tokens this source can contribute. 0 = unlimited.
    pub max_tokens: u32,
    /// Whether this source is enabled.
    pub enabled: bool,
}

impl Default for SourceGovernance {
    fn default() -> Self {
        Self {
            max_tokens: 0,
            enabled: true,
        }
    }
}

/// Governance engine that enforces limits on context contributions.
pub(crate) struct ContextGovernance {
    /// Per-source-name limits.
    source_limits: HashMap<String, SourceGovernance>,
    /// Default limits for unconfigured sources.
    default_limits: SourceGovernance,
    /// Provenance log (capped at last N assemblies).
    provenance_history: VecDeque<Vec<ContextProvenance>>,
    /// Maximum number of assembly records to keep.
    max_history: usize,
}

impl ContextGovernance {
    /// Create a new governance engine with per-source limits.
    pub fn new(source_limits: HashMap<String, SourceGovernance>) -> Self {
        Self {
            source_limits,
            default_limits: SourceGovernance::default(),
            max_history: 20,
            provenance_history: VecDeque::new(),
        }
    }

    /// Create a governance engine with a default max-tokens limit.
    pub fn with_default_max_tokens(default_max: u32) -> Self {
        Self {
            source_limits: HashMap::new(),
            default_limits: SourceGovernance {
                max_tokens: default_max,
                enabled: true,
            },
            max_history: 20,
            provenance_history: VecDeque::new(),
        }
    }

    /// Apply governance rules to assembled chunks.
    ///
    /// Returns filtered/truncated chunks and provenance records.
    /// Chunks from disabled sources are excluded.
    /// Chunks exceeding their source's max_tokens are truncated.
    pub fn apply(
        &mut self,
        chunks: Vec<ContextChunk>,
    ) -> (Vec<ContextChunk>, Vec<ContextProvenance>) {
        let now = Utc::now();
        let mut result = Vec::with_capacity(chunks.len());
        let mut provenance = Vec::with_capacity(chunks.len());

        for chunk in chunks {
            let limits = self
                .source_limits
                .get(&chunk.source)
                .unwrap_or(&self.default_limits);

            // Skip disabled sources.
            if !limits.enabled {
                continue;
            }

            // Apply token limit.
            let mut chunk = chunk;
            if limits.max_tokens > 0 && chunk.estimated_tokens > limits.max_tokens as usize {
                // Truncate content to approximate the token limit.
                let target_chars = (limits.max_tokens as usize) * 4; // inverse of estimate_tokens
                if chunk.content.len() > target_chars {
                    chunk.content.truncate(target_chars);
                    chunk.estimated_tokens = estimate_tokens(&chunk.content);
                }
            }

            provenance.push(ContextProvenance {
                source_name: chunk.source.clone(),
                token_count: chunk.estimated_tokens as u32,
                priority: chunk.priority,
                timestamp: now,
            });
            result.push(chunk);
        }

        // Record provenance history.
        if self.provenance_history.len() >= self.max_history {
            self.provenance_history.pop_front();
        }
        self.provenance_history.push_back(provenance.clone());

        (result, provenance)
    }

    /// Get the last N assembly provenance records.
    pub fn recent_provenance(&self) -> &VecDeque<Vec<ContextProvenance>> {
        &self.provenance_history
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(source: &str, priority: u32, content: &str) -> ContextChunk {
        ContextChunk {
            source: source.into(),
            priority,
            content: content.into(),
            estimated_tokens: estimate_tokens(content),
        }
    }

    #[test]
    fn apply_no_limits_passes_through() {
        let mut gov = ContextGovernance::new(HashMap::new());
        let chunks = vec![chunk("a", 10, "hello world")];
        let (result, prov) = gov.apply(chunks);
        assert_eq!(result.len(), 1);
        assert_eq!(prov.len(), 1);
        assert_eq!(prov[0].source_name, "a");
    }

    #[test]
    fn disabled_source_excluded() {
        let mut limits = HashMap::new();
        limits.insert(
            "disabled_src".to_string(),
            SourceGovernance {
                max_tokens: 0,
                enabled: false,
            },
        );
        let mut gov = ContextGovernance::new(limits);
        let chunks = vec![
            chunk("disabled_src", 10, "should be excluded"),
            chunk("enabled_src", 10, "should remain"),
        ];
        let (result, prov) = gov.apply(chunks);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source, "enabled_src");
        assert_eq!(prov.len(), 1);
    }

    #[test]
    fn max_tokens_truncation() {
        let mut limits = HashMap::new();
        limits.insert(
            "limited".to_string(),
            SourceGovernance {
                max_tokens: 5, // Very small limit
                enabled: true,
            },
        );
        let mut gov = ContextGovernance::new(limits);
        // This content is ~25 tokens (100 chars / 4)
        let long_content = "a".repeat(100);
        let chunks = vec![chunk("limited", 10, &long_content)];
        let (result, _) = gov.apply(chunks);
        assert_eq!(result.len(), 1);
        // After truncation, content should be at most 5*4=20 chars
        assert!(result[0].content.len() <= 20);
    }

    #[test]
    fn default_limits_fallback() {
        let mut gov = ContextGovernance::with_default_max_tokens(10);
        let long_content = "a".repeat(200);
        let chunks = vec![chunk("any_source", 10, &long_content)];
        let (result, _) = gov.apply(chunks);
        // Default limit of 10 tokens → 40 chars max
        assert!(result[0].content.len() <= 40);
    }

    #[test]
    fn provenance_recorded() {
        let mut gov = ContextGovernance::new(HashMap::new());
        let chunks = vec![chunk("src1", 10, "data"), chunk("src2", 20, "more data")];
        let (_, prov) = gov.apply(chunks);
        assert_eq!(prov.len(), 2);
        assert_eq!(prov[0].source_name, "src1");
        assert_eq!(prov[1].source_name, "src2");
        assert_eq!(prov[1].priority, 20);
    }

    #[test]
    fn history_capped() {
        let mut gov = ContextGovernance::new(HashMap::new());
        gov.max_history = 3;
        for _ in 0..5 {
            gov.apply(vec![chunk("s", 1, "x")]);
        }
        assert_eq!(gov.recent_provenance().len(), 3);
    }

    #[test]
    fn empty_chunks_pass_through() {
        let mut gov = ContextGovernance::new(HashMap::new());
        let (result, prov) = gov.apply(vec![]);
        assert!(result.is_empty());
        assert!(prov.is_empty());
        // But provenance history should have one empty record
        assert_eq!(gov.recent_provenance().len(), 1);
    }

    #[test]
    fn small_chunk_not_truncated() {
        let mut limits = HashMap::new();
        limits.insert(
            "src".to_string(),
            SourceGovernance {
                max_tokens: 100,
                enabled: true,
            },
        );
        let mut gov = ContextGovernance::new(limits);
        let chunks = vec![chunk("src", 10, "small")];
        let (result, _) = gov.apply(chunks);
        assert_eq!(result[0].content, "small");
    }

    #[test]
    fn provenance_has_timestamps() {
        let before = Utc::now();
        let mut gov = ContextGovernance::new(HashMap::new());
        let (_, prov) = gov.apply(vec![chunk("s", 1, "data")]);
        let after = Utc::now();
        assert!(prov[0].timestamp >= before);
        assert!(prov[0].timestamp <= after);
    }

    #[test]
    fn governance_passthrough_no_limits() {
        let mut gov = ContextGovernance::new(HashMap::new());
        let original = "This is a test context chunk with several words";
        let chunks = vec![chunk("instructions", 100, original)];
        let (result, prov) = gov.apply(chunks);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, original, "should not truncate when no limits set");
        assert_eq!(prov.len(), 1);
        assert_eq!(prov[0].source_name, "instructions");
    }

    #[test]
    fn governance_truncation_applied_when_limit_set() {
        let mut limits = HashMap::new();
        limits.insert(
            "big_source".to_string(),
            SourceGovernance {
                max_tokens: 8,
                enabled: true,
            },
        );
        let mut gov = ContextGovernance::new(limits);
        // 200 chars ≈ 50 tokens, limit is 8 tokens → truncate to 32 chars
        let long_content = "x".repeat(200);
        let chunks = vec![chunk("big_source", 10, &long_content)];
        let (result, _) = gov.apply(chunks);
        assert_eq!(result.len(), 1);
        assert!(
            result[0].content.len() <= 32,
            "content should be truncated to ~8*4=32 chars, got {}",
            result[0].content.len()
        );
    }
}
