use std::collections::HashMap;

use cuervo_core::types::ModelChunk;

/// Accumulates streaming tool use chunks into complete tool calls.
///
/// The Anthropic API sends tool_use in three parts:
/// 1. `content_block_start` with `type: tool_use`, `id`, `name`  → `ToolUseStart`
/// 2. One or more `content_block_delta` with `input_json_delta`   → `ToolUseDelta`
/// 3. `content_block_stop`                                         → finalized
///
/// This accumulator collects the partial JSON from deltas and produces
/// `CompletedToolUse` values when `finalize()` is called.
pub struct ToolUseAccumulator {
    pending: HashMap<u32, PendingToolUse>,
}

struct PendingToolUse {
    id: String,
    name: String,
    json_buf: String,
}

/// A fully assembled tool use call, ready for execution.
#[derive(Clone)]
pub struct CompletedToolUse {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

impl ToolUseAccumulator {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    /// Process a model chunk. Returns `true` if this chunk was tool-related.
    pub fn process(&mut self, chunk: &ModelChunk) -> bool {
        match chunk {
            ModelChunk::ToolUseStart { index, id, name } => {
                self.pending.insert(
                    *index,
                    PendingToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        json_buf: String::new(),
                    },
                );
                true
            }
            ModelChunk::ToolUseDelta {
                index,
                partial_json,
            } => {
                if let Some(pending) = self.pending.get_mut(index) {
                    pending.json_buf.push_str(partial_json);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Finalize all pending tool uses, parsing their accumulated JSON buffers.
    ///
    /// Returns completed tool uses and clears the accumulator.
    pub fn finalize(&mut self) -> Vec<CompletedToolUse> {
        let mut result: Vec<CompletedToolUse> = self
            .pending
            .drain()
            .map(|(_, pending)| {
                let input = if pending.json_buf.is_empty() {
                    serde_json::Value::Object(serde_json::Map::new())
                } else {
                    serde_json::from_str(&pending.json_buf).unwrap_or_else(|e| {
                        tracing::warn!(
                            tool = %pending.name,
                            error = %e,
                            json = %pending.json_buf,
                            "failed to parse tool input JSON"
                        );
                        serde_json::json!({ "_parse_error": e.to_string() })
                    })
                };
                CompletedToolUse {
                    id: pending.id,
                    name: pending.name,
                    input,
                }
            })
            .collect();
        // Sort by id for deterministic ordering.
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }

    /// Whether there are any pending (started but not finalized) tool uses.
    #[allow(dead_code)]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

impl Default for ToolUseAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulate_single_tool_use() {
        let mut acc = ToolUseAccumulator::new();

        assert!(acc.process(&ModelChunk::ToolUseStart {
            index: 0,
            id: "toolu_1".into(),
            name: "file_read".into(),
        }));
        assert!(acc.has_pending());

        assert!(acc.process(&ModelChunk::ToolUseDelta {
            index: 0,
            partial_json: "{\"path\":".into(),
        }));
        assert!(acc.process(&ModelChunk::ToolUseDelta {
            index: 0,
            partial_json: "\"test.rs\"}".into(),
        }));

        let completed = acc.finalize();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].id, "toolu_1");
        assert_eq!(completed[0].name, "file_read");
        assert_eq!(completed[0].input["path"], "test.rs");
        assert!(!acc.has_pending());
    }

    #[test]
    fn accumulate_multiple_tool_uses() {
        let mut acc = ToolUseAccumulator::new();

        acc.process(&ModelChunk::ToolUseStart {
            index: 0,
            id: "toolu_a".into(),
            name: "file_read".into(),
        });
        acc.process(&ModelChunk::ToolUseStart {
            index: 1,
            id: "toolu_b".into(),
            name: "bash".into(),
        });

        acc.process(&ModelChunk::ToolUseDelta {
            index: 0,
            partial_json: "{\"path\":\"a.rs\"}".into(),
        });
        acc.process(&ModelChunk::ToolUseDelta {
            index: 1,
            partial_json: "{\"command\":\"ls\"}".into(),
        });

        let completed = acc.finalize();
        assert_eq!(completed.len(), 2);
        // Sorted by id.
        assert_eq!(completed[0].id, "toolu_a");
        assert_eq!(completed[1].id, "toolu_b");
    }

    #[test]
    fn non_tool_chunks_return_false() {
        let mut acc = ToolUseAccumulator::new();

        assert!(!acc.process(&ModelChunk::TextDelta("hello".into())));
        assert!(!acc.process(&ModelChunk::Usage(cuervo_core::types::TokenUsage::default())));
        assert!(!acc.process(&ModelChunk::Done(cuervo_core::types::StopReason::EndTurn)));
        assert!(!acc.has_pending());
    }

    #[test]
    fn empty_json_becomes_empty_object() {
        let mut acc = ToolUseAccumulator::new();

        acc.process(&ModelChunk::ToolUseStart {
            index: 0,
            id: "toolu_1".into(),
            name: "bash".into(),
        });

        let completed = acc.finalize();
        assert_eq!(completed.len(), 1);
        assert!(completed[0].input.is_object());
        assert!(completed[0].input.as_object().unwrap().is_empty());
    }

    #[test]
    fn invalid_json_produces_parse_error() {
        let mut acc = ToolUseAccumulator::new();

        acc.process(&ModelChunk::ToolUseStart {
            index: 0,
            id: "toolu_1".into(),
            name: "test".into(),
        });
        acc.process(&ModelChunk::ToolUseDelta {
            index: 0,
            partial_json: "{invalid json".into(),
        });

        let completed = acc.finalize();
        assert_eq!(completed.len(), 1);
        assert!(completed[0].input["_parse_error"].is_string());
    }

    #[test]
    fn delta_for_unknown_index_returns_false() {
        let mut acc = ToolUseAccumulator::new();

        assert!(!acc.process(&ModelChunk::ToolUseDelta {
            index: 99,
            partial_json: "{}".into(),
        }));
    }

    #[test]
    fn finalize_clears_state() {
        let mut acc = ToolUseAccumulator::new();

        acc.process(&ModelChunk::ToolUseStart {
            index: 0,
            id: "toolu_1".into(),
            name: "test".into(),
        });
        acc.process(&ModelChunk::ToolUseDelta {
            index: 0,
            partial_json: "{}".into(),
        });

        let first = acc.finalize();
        assert_eq!(first.len(), 1);

        // Second finalize should be empty.
        let second = acc.finalize();
        assert!(second.is_empty());
    }

    #[test]
    fn incremental_json_assembly() {
        let mut acc = ToolUseAccumulator::new();

        acc.process(&ModelChunk::ToolUseStart {
            index: 0,
            id: "toolu_1".into(),
            name: "file_edit".into(),
        });

        // Simulate many small deltas.
        let parts = [
            "{",
            "\"path\"",
            ":",
            "\"test.rs\"",
            ",",
            "\"old",
            "_string\"",
            ":",
            "\"foo\"",
            ",",
            "\"new",
            "_string\"",
            ":",
            "\"bar\"",
            "}",
        ];
        for part in &parts {
            acc.process(&ModelChunk::ToolUseDelta {
                index: 0,
                partial_json: part.to_string(),
            });
        }

        let completed = acc.finalize();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].input["path"], "test.rs");
        assert_eq!(completed[0].input["old_string"], "foo");
        assert_eq!(completed[0].input["new_string"], "bar");
    }

    // === Stress tests for accumulator robustness (RC-4) ===

    #[test]
    fn truncated_json_mid_string_produces_parse_error() {
        let mut acc = ToolUseAccumulator::new();
        acc.process(&ModelChunk::ToolUseStart {
            index: 0,
            id: "t1".into(),
            name: "file_read".into(),
        });
        // Simulate stream cutoff mid-string value
        acc.process(&ModelChunk::ToolUseDelta {
            index: 0,
            partial_json: "{\"path\": \"/tmp/some_very_long_pa".into(),
        });

        let completed = acc.finalize();
        assert_eq!(completed.len(), 1);
        assert!(completed[0].input.get("_parse_error").is_some());
    }

    #[test]
    fn truncated_json_mid_key_produces_parse_error() {
        let mut acc = ToolUseAccumulator::new();
        acc.process(&ModelChunk::ToolUseStart {
            index: 0,
            id: "t1".into(),
            name: "bash".into(),
        });
        acc.process(&ModelChunk::ToolUseDelta {
            index: 0,
            partial_json: "{\"comm".into(),
        });

        let completed = acc.finalize();
        assert_eq!(completed.len(), 1);
        assert!(completed[0].input.get("_parse_error").is_some());
    }

    #[test]
    fn parse_error_contains_useful_message() {
        let mut acc = ToolUseAccumulator::new();
        acc.process(&ModelChunk::ToolUseStart {
            index: 0,
            id: "t1".into(),
            name: "test".into(),
        });
        acc.process(&ModelChunk::ToolUseDelta {
            index: 0,
            partial_json: "{\"key\": [1, 2, ".into(),
        });

        let completed = acc.finalize();
        let err_msg = completed[0].input["_parse_error"].as_str().unwrap();
        assert!(
            err_msg.contains("EOF") || err_msg.contains("expected"),
            "Error message should indicate parse failure: {err_msg}"
        );
    }

    #[test]
    fn multiple_tools_one_truncated() {
        let mut acc = ToolUseAccumulator::new();

        // Tool A: valid JSON
        acc.process(&ModelChunk::ToolUseStart {
            index: 0,
            id: "t_a".into(),
            name: "file_read".into(),
        });
        acc.process(&ModelChunk::ToolUseDelta {
            index: 0,
            partial_json: "{\"path\": \"a.rs\"}".into(),
        });

        // Tool B: truncated JSON
        acc.process(&ModelChunk::ToolUseStart {
            index: 1,
            id: "t_b".into(),
            name: "bash".into(),
        });
        acc.process(&ModelChunk::ToolUseDelta {
            index: 1,
            partial_json: "{\"command\": \"ls -la /tmp/".into(),
        });

        let completed = acc.finalize();
        assert_eq!(completed.len(), 2);

        // Tool A should be valid
        let tool_a = completed.iter().find(|t| t.id == "t_a").unwrap();
        assert_eq!(tool_a.input["path"], "a.rs");
        assert!(tool_a.input.get("_parse_error").is_none());

        // Tool B should have parse error
        let tool_b = completed.iter().find(|t| t.id == "t_b").unwrap();
        assert!(tool_b.input.get("_parse_error").is_some());
    }

    #[test]
    fn unicode_in_json_parses_correctly() {
        let mut acc = ToolUseAccumulator::new();
        acc.process(&ModelChunk::ToolUseStart {
            index: 0,
            id: "t1".into(),
            name: "file_write".into(),
        });
        acc.process(&ModelChunk::ToolUseDelta {
            index: 0,
            partial_json: "{\"path\": \"/tmp/日本語.txt\", \"content\": \"こんにちは\"}".into(),
        });

        let completed = acc.finalize();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].input["path"], "/tmp/日本語.txt");
        assert!(completed[0].input.get("_parse_error").is_none());
    }

    #[test]
    fn deeply_nested_json_parses() {
        let mut acc = ToolUseAccumulator::new();
        acc.process(&ModelChunk::ToolUseStart {
            index: 0,
            id: "t1".into(),
            name: "test".into(),
        });
        acc.process(&ModelChunk::ToolUseDelta {
            index: 0,
            partial_json: r#"{"a":{"b":{"c":{"d":"deep"}}}}"#.into(),
        });

        let completed = acc.finalize();
        assert_eq!(completed[0].input["a"]["b"]["c"]["d"], "deep");
    }
}
