//! Tool idempotency registry and dry-run mode for safe orchestration.
//!
//! - IdempotencyRegistry: deduplicates identical tool calls within a session.
//! - DryRunMode: skips tool execution and returns synthetic results.
//! - RollbackHint: provides undo guidance for destructive operations.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Compute a deterministic execution ID from tool name, args, and salt.
///
/// Returns `"exec_{sha256_prefix_16}"`.
pub fn compute_execution_id(tool_name: &str, args: &serde_json::Value, salt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tool_name.as_bytes());
    hasher.update(serde_json::to_string(args).unwrap_or_default().as_bytes());
    hasher.update(salt.as_bytes());
    let hash = hex::encode(hasher.finalize());
    format!("exec_{}", &hash[..16])
}

/// A recorded tool execution result for idempotency deduplication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// Execution ID (deterministic hash).
    pub execution_id: String,
    /// Tool that was executed.
    pub tool_name: String,
    /// Tool result content.
    pub result_content: String,
    /// Whether the result was an error.
    pub is_error: bool,
    /// When the execution occurred.
    pub executed_at: DateTime<Utc>,
}

/// In-memory registry for deduplicating identical tool calls.
pub struct IdempotencyRegistry {
    records: Mutex<HashMap<String, ExecutionRecord>>,
}

impl IdempotencyRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
        }
    }

    /// Look up a previous execution by its ID.
    pub fn lookup(&self, id: &str) -> Option<ExecutionRecord> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    /// Record an execution result.
    pub fn record(&self, rec: ExecutionRecord) {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(rec.execution_id.clone(), rec);
    }

    /// Number of recorded executions.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Whether the registry is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for IdempotencyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Re-export DryRunMode from halcon-core (canonical definition in phase14.rs).
pub use halcon_core::types::DryRunMode;

#[cfg(test)]
mod tests {
    use super::*;

    /// Guidance for undoing a destructive tool operation (test-only — not yet integrated into production).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct RollbackHint {
        tool_name: String,
        description: String,
        undo_command: Option<String>,
        affected_paths: Vec<String>,
    }

    /// Generate a rollback hint for a tool call (best-effort, test-only).
    fn generate_rollback_hint(
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Option<RollbackHint> {
        match tool_name {
            "file_write" | "write_file" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<unknown>");
                Some(RollbackHint {
                    tool_name: tool_name.to_string(),
                    description: format!("Would write to file: {path}"),
                    undo_command: Some(format!("rm {path}")),
                    affected_paths: vec![path.to_string()],
                })
            }
            "file_edit" | "edit_file" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<unknown>");
                Some(RollbackHint {
                    tool_name: tool_name.to_string(),
                    description: format!("Would edit file: {path}"),
                    undo_command: Some(format!("git checkout -- {path}")),
                    affected_paths: vec![path.to_string()],
                })
            }
            "bash" => {
                let cmd = args
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<unknown>");
                Some(RollbackHint {
                    tool_name: tool_name.to_string(),
                    description: format!("Would execute: {cmd}"),
                    undo_command: None,
                    affected_paths: vec![],
                })
            }
            _ => None,
        }
    }

    #[test]
    fn compute_execution_id_deterministic() {
        let id1 = compute_execution_id("bash", &serde_json::json!({"cmd": "ls"}), "salt");
        let id2 = compute_execution_id("bash", &serde_json::json!({"cmd": "ls"}), "salt");
        assert_eq!(id1, id2);
        assert!(id1.starts_with("exec_"));
    }

    #[test]
    fn compute_execution_id_different_args() {
        let id1 = compute_execution_id("bash", &serde_json::json!({"cmd": "ls"}), "s");
        let id2 = compute_execution_id("bash", &serde_json::json!({"cmd": "pwd"}), "s");
        assert_ne!(id1, id2);
    }

    #[test]
    fn compute_execution_id_different_tool() {
        let id1 = compute_execution_id("bash", &serde_json::json!({}), "s");
        let id2 = compute_execution_id("file_read", &serde_json::json!({}), "s");
        assert_ne!(id1, id2);
    }

    #[test]
    fn idempotency_registry_empty() {
        let reg = IdempotencyRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn idempotency_registry_record_and_lookup() {
        let reg = IdempotencyRegistry::new();
        let rec = ExecutionRecord {
            execution_id: "exec_abc".to_string(),
            tool_name: "bash".to_string(),
            result_content: "output".to_string(),
            is_error: false,
            executed_at: Utc::now(),
        };
        reg.record(rec);
        assert_eq!(reg.len(), 1);

        let found = reg.lookup("exec_abc").unwrap();
        assert_eq!(found.tool_name, "bash");
        assert_eq!(found.result_content, "output");
    }

    #[test]
    fn idempotency_registry_miss() {
        let reg = IdempotencyRegistry::new();
        assert!(reg.lookup("nonexistent").is_none());
    }

    #[test]
    fn idempotency_registry_len() {
        let reg = IdempotencyRegistry::new();
        for i in 0..5 {
            reg.record(ExecutionRecord {
                execution_id: format!("exec_{i}"),
                tool_name: "t".to_string(),
                result_content: String::new(),
                is_error: false,
                executed_at: Utc::now(),
            });
        }
        assert_eq!(reg.len(), 5);
    }

    #[test]
    fn dry_run_mode_default_off() {
        assert_eq!(DryRunMode::default(), DryRunMode::Off);
    }

    #[test]
    fn dry_run_mode_serde() {
        let json = serde_json::to_string(&DryRunMode::DestructiveOnly).unwrap();
        assert_eq!(json, r#""destructive_only""#);
        let parsed: DryRunMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, DryRunMode::DestructiveOnly);
    }

    #[test]
    fn rollback_hint_file_write() {
        let hint =
            generate_rollback_hint("file_write", &serde_json::json!({"path": "/tmp/foo.txt"}));
        let hint = hint.unwrap();
        assert!(hint.description.contains("/tmp/foo.txt"));
        assert!(hint.undo_command.is_some());
        assert_eq!(hint.affected_paths, vec!["/tmp/foo.txt"]);
    }

    #[test]
    fn rollback_hint_file_edit() {
        let hint =
            generate_rollback_hint("file_edit", &serde_json::json!({"path": "src/main.rs"}));
        let hint = hint.unwrap();
        assert!(hint.undo_command.unwrap().contains("git checkout"));
    }

    #[test]
    fn rollback_hint_bash() {
        let hint = generate_rollback_hint("bash", &serde_json::json!({"command": "rm -rf /tmp"}));
        let hint = hint.unwrap();
        assert!(hint.description.contains("rm -rf"));
        assert!(hint.undo_command.is_none());
    }

    #[test]
    fn rollback_hint_unknown_tool_none() {
        let hint = generate_rollback_hint("unknown_tool", &serde_json::json!({}));
        assert!(hint.is_none());
    }

    #[test]
    fn rollback_hint_serde_roundtrip() {
        let hint = RollbackHint {
            tool_name: "bash".to_string(),
            description: "test".to_string(),
            undo_command: Some("undo".to_string()),
            affected_paths: vec!["/tmp".to_string()],
        };
        let json = serde_json::to_string(&hint).unwrap();
        let parsed: RollbackHint = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tool_name, "bash");
        assert_eq!(parsed.affected_paths.len(), 1);
    }
}
