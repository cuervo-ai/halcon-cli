use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A task context that scopes tool authorization.
///
/// Each task context defines what tools are allowed, with what parameters,
/// and for how long. Contexts can be nested (a sub-task inherits from parent
/// but may have narrower scope).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    /// Unique ID for this task context.
    pub context_id: Uuid,
    /// Human-readable description of the task.
    pub task_description: String,
    /// Parent context ID (for sub-task scoping). None = root.
    pub parent_id: Option<Uuid>,
    /// Tools allowed in this context.
    pub allowed_tools: HashSet<String>,
    /// Parameter constraints per tool (tool_name → constraint).
    pub parameter_constraints: HashMap<String, ParameterConstraint>,
    /// When this context was created.
    pub created_at: DateTime<Utc>,
    /// When this context expires. None = session-scoped.
    pub expires_at: Option<DateTime<Utc>>,
    /// Maximum number of tool invocations under this context.
    pub max_invocations: Option<u32>,
    /// Number of invocations consumed.
    pub invocations_used: u32,
}

impl TaskContext {
    pub fn new(task_description: String, allowed_tools: HashSet<String>) -> Self {
        Self {
            context_id: Uuid::new_v4(),
            task_description,
            parent_id: None,
            allowed_tools,
            parameter_constraints: HashMap::new(),
            created_at: Utc::now(),
            expires_at: None,
            max_invocations: None,
            invocations_used: 0,
        }
    }

    /// Create a child context with a narrower scope.
    pub fn child(&self, task_description: String, allowed_tools: HashSet<String>) -> Self {
        // Child can only restrict, not expand — intersect with parent.
        let effective_tools: HashSet<String> = allowed_tools
            .intersection(&self.allowed_tools)
            .cloned()
            .collect();

        Self {
            context_id: Uuid::new_v4(),
            task_description,
            parent_id: Some(self.context_id),
            allowed_tools: effective_tools,
            parameter_constraints: self.parameter_constraints.clone(),
            created_at: Utc::now(),
            expires_at: self.expires_at,
            max_invocations: self.max_invocations,
            invocations_used: 0,
        }
    }

    /// Check if this context is still valid (not expired, not exhausted).
    pub fn is_valid(&self) -> bool {
        if let Some(expires) = self.expires_at {
            if Utc::now() > expires {
                return false;
            }
        }
        if let Some(max) = self.max_invocations {
            if self.invocations_used >= max {
                return false;
            }
        }
        true
    }

    /// Check if a tool is allowed under this context.
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        self.is_valid() && self.allowed_tools.contains(tool_name)
    }

    /// Check parameter constraints for a specific tool.
    pub fn check_params(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        if let Some(constraint) = self.parameter_constraints.get(tool_name) {
            constraint.check(args)
        } else {
            true // No constraints = all params allowed.
        }
    }

    /// Consume one invocation slot. Returns false if exhausted.
    pub fn consume_invocation(&mut self) -> bool {
        if let Some(max) = self.max_invocations {
            if self.invocations_used >= max {
                return false;
            }
        }
        self.invocations_used += 1;
        true
    }
}

/// Constraints on tool parameters (path restrictions, command allowlists).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParameterConstraint {
    /// Path must be within these directories.
    PathRestriction { allowed_dirs: Vec<String> },
    /// Command must match one of these glob patterns.
    CommandAllowlist { patterns: Vec<String> },
    /// Argument value must be one of these.
    ValueAllowlist {
        field: String,
        allowed: Vec<serde_json::Value>,
    },
}

impl ParameterConstraint {
    pub fn check(&self, args: &serde_json::Value) -> bool {
        match self {
            ParameterConstraint::PathRestriction { allowed_dirs } => {
                if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                    allowed_dirs.iter().any(|dir| path.starts_with(dir))
                } else {
                    true // No path param = no restriction.
                }
            }
            ParameterConstraint::CommandAllowlist { patterns } => {
                if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                    patterns
                        .iter()
                        .any(|p| glob::Pattern::new(p).is_ok_and(|g| g.matches(cmd)))
                } else {
                    true
                }
            }
            ParameterConstraint::ValueAllowlist { field, allowed } => {
                if let Some(val) = args.get(field) {
                    allowed.contains(val)
                } else {
                    true
                }
            }
        }
    }
}

/// Result of a TBAC authorization check.
#[derive(Debug, Clone)]
pub enum AuthzDecision {
    /// Allowed by active task context.
    Allowed { context_id: Uuid },
    /// Tool not in context allowlist.
    ToolNotAllowed { tool: String, context_id: Uuid },
    /// Parameter constraint violated.
    ParamViolation { tool: String, constraint: String },
    /// Context expired or exhausted.
    ContextInvalid { context_id: Uuid, reason: String },
    /// No active task context — fall back to legacy permission check.
    NoContext,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_context_allows_listed_tools() {
        let tools: HashSet<String> = ["bash", "file_read"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let ctx = TaskContext::new("Test task".into(), tools);

        assert!(ctx.is_tool_allowed("bash"));
        assert!(ctx.is_tool_allowed("file_read"));
    }

    #[test]
    fn task_context_denies_unlisted_tools() {
        let tools: HashSet<String> = ["bash"].iter().map(|s| s.to_string()).collect();
        let ctx = TaskContext::new("Test task".into(), tools);

        assert!(!ctx.is_tool_allowed("file_write"));
        assert!(!ctx.is_tool_allowed(""));
    }

    #[test]
    fn task_context_expiry() {
        let tools: HashSet<String> = ["bash"].iter().map(|s| s.to_string()).collect();
        let mut ctx = TaskContext::new("Expiring task".into(), tools);
        // Set expiry to 1 second ago.
        ctx.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));

        assert!(!ctx.is_valid());
        assert!(!ctx.is_tool_allowed("bash"));
    }

    #[test]
    fn task_context_invocation_limit() {
        let tools: HashSet<String> = ["bash"].iter().map(|s| s.to_string()).collect();
        let mut ctx = TaskContext::new("Limited task".into(), tools);
        ctx.max_invocations = Some(2);

        assert!(ctx.consume_invocation()); // 1
        assert!(ctx.consume_invocation()); // 2
        assert!(!ctx.consume_invocation()); // exhausted
        assert!(!ctx.is_valid());
    }

    #[test]
    fn child_context_intersects() {
        let parent_tools: HashSet<String> = ["bash", "file_read", "grep"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let parent = TaskContext::new("Parent".into(), parent_tools);

        let child_tools: HashSet<String> = ["bash", "file_write"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let child = parent.child("Child".into(), child_tools);

        // Only "bash" is in both parent and child.
        assert!(child.is_tool_allowed("bash"));
        assert!(!child.is_tool_allowed("file_read")); // parent-only
        assert!(!child.is_tool_allowed("file_write")); // child-only (not in parent)
        assert_eq!(child.parent_id, Some(parent.context_id));
    }

    #[test]
    fn path_constraint_restricts() {
        let constraint = ParameterConstraint::PathRestriction {
            allowed_dirs: vec!["/home/user/project".into(), "/tmp".into()],
        };

        // Allowed paths.
        assert!(constraint.check(&serde_json::json!({"path": "/home/user/project/src/main.rs"})));
        assert!(constraint.check(&serde_json::json!({"path": "/tmp/test.txt"})));

        // Blocked path.
        assert!(!constraint.check(&serde_json::json!({"path": "/etc/passwd"})));

        // No path param = no restriction.
        assert!(constraint.check(&serde_json::json!({"content": "hello"})));
    }

    #[test]
    fn command_allowlist_filters() {
        let constraint = ParameterConstraint::CommandAllowlist {
            patterns: vec!["cargo *".into(), "git status".into()],
        };

        assert!(constraint.check(&serde_json::json!({"command": "cargo test"})));
        assert!(constraint.check(&serde_json::json!({"command": "cargo build --release"})));
        assert!(constraint.check(&serde_json::json!({"command": "git status"})));

        assert!(!constraint.check(&serde_json::json!({"command": "rm -rf /"})));
        assert!(!constraint.check(&serde_json::json!({"command": "git push"})));

        // No command param = no restriction.
        assert!(constraint.check(&serde_json::json!({"path": "/tmp"})));
    }

    #[test]
    fn value_allowlist_checks() {
        let constraint = ParameterConstraint::ValueAllowlist {
            field: "language".into(),
            allowed: vec![
                serde_json::json!("rust"),
                serde_json::json!("python"),
            ],
        };

        assert!(constraint.check(&serde_json::json!({"language": "rust"})));
        assert!(constraint.check(&serde_json::json!({"language": "python"})));
        assert!(!constraint.check(&serde_json::json!({"language": "javascript"})));

        // Missing field = allowed.
        assert!(constraint.check(&serde_json::json!({"other": "value"})));
    }
}
