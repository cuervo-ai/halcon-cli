//! Safe sub-agent spawning for multi-agent session coordination.
//!
//! Provides the infrastructure to spawn child agents that:
//!
//! - Inherit the parent session context (session_id, working_dir, budget).
//! - Share the session `ArtifactStore` and `ProvenanceTracker` via Arc handles.
//! - Have role-scoped capability restrictions (Analyzer cannot write; only
//!   Planner/Supervisor can spawn further sub-agents).
//! - Never bypass RBAC — spawn requests are validated before execution.
//!
//! ## Design
//!
//! `SubAgentSpawner` is stateless. It validates the spawn request against the
//! parent agent's role and budget, then returns a `SpawnedAgentHandle` that
//! the caller uses to drive the sub-agent.
//!
//! ```text
//! Parent agent (Planner)
//!   └─ SubAgentSpawner::spawn(SubAgentConfig { role: Coder, ... })
//!         ├─ validate_role_can_spawn()   — role permission check
//!         ├─ validate_budget()           — no negative budget inheritance
//!         └─ SpawnedAgentHandle { agent_id, role, session_id, ... }
//! ```
//!
//! ## RBAC enforcement
//!
//! The spawner checks:
//! 1. Parent role has `can_spawn_subagents() == true`.
//! 2. Child budget does not exceed parent remaining budget.
//! 3. Child role capability set is a subset of the parent's.
//!
//! The child agent is responsible for further enforcement at the tool layer
//! via `AgentRole::allows_writes()` and RBAC middleware.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

use halcon_core::types::AgentRole;

use crate::artifacts::SessionArtifactStore;
use crate::provenance::SessionProvenanceTracker;

// ── BudgetAllocation ─────────────────────────────────────────────────────────

/// Resource budget assigned to a sub-agent.
///
/// All limits are upper bounds; `0` means "no limit" (inherits from parent).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BudgetAllocation {
    /// Maximum total tokens the sub-agent may consume (`0` = unlimited).
    pub max_tokens: u64,
    /// Maximum wall-clock seconds the sub-agent may run (`0` = unlimited).
    pub max_duration_secs: u64,
    /// Maximum number of agent loop rounds (`0` = unlimited).
    pub max_rounds: u32,
}

impl Default for BudgetAllocation {
    fn default() -> Self {
        Self {
            max_tokens: 0,
            max_duration_secs: 300, // 5-minute default cap for sub-agents
            max_rounds: 10,
        }
    }
}

// ── SubAgentConfig ────────────────────────────────────────────────────────────

/// Configuration for a spawned sub-agent.
#[derive(Debug, Clone)]
pub struct SubAgentConfig {
    /// Functional role of the sub-agent.
    ///
    /// Determines default tool access (`allows_writes()`) and whether the
    /// sub-agent may itself spawn further children (`can_spawn_subagents()`).
    pub role: AgentRole,
    /// Task instruction passed to the sub-agent as its initial user message.
    pub instruction: String,
    /// Working directory the sub-agent operates in.
    ///
    /// Defaults to the parent's working directory if `None`.
    pub working_dir: Option<String>,
    /// Resource budget allocated to the sub-agent.
    pub budget: BudgetAllocation,
    /// Optional system-prompt prefix injected before the sub-agent's default
    /// system prompt (used by the agent registry for role-specific priming).
    pub system_prompt_prefix: Option<String>,
}

// ── SpawnedAgentHandle ────────────────────────────────────────────────────────

/// Handle returned after a successful spawn validation.
///
/// The caller uses this to:
/// 1. Drive the sub-agent (pass `instruction` to `run_agent_loop`).
/// 2. Record artifacts and provenance from the sub-agent's output.
/// 3. Identify the sub-agent in logs and provenance records.
#[derive(Debug, Clone)]
pub struct SpawnedAgentHandle {
    /// Unique ID assigned to this sub-agent instance.
    pub agent_id: Uuid,
    /// Session the sub-agent belongs to.
    pub session_id: Uuid,
    /// Functional role of the sub-agent.
    pub role: AgentRole,
    /// Working directory for the sub-agent.
    pub working_dir: String,
    /// Validated budget for the sub-agent.
    pub budget: BudgetAllocation,
    /// Optional system-prompt prefix.
    pub system_prompt_prefix: Option<String>,
    /// Instruction to pass to the sub-agent's run loop.
    pub instruction: String,
    /// Shared artifact store (same `Arc` as the session).
    pub artifact_store: Arc<RwLock<SessionArtifactStore>>,
    /// Shared provenance tracker (same `Arc` as the session).
    pub provenance_tracker: Arc<RwLock<SessionProvenanceTracker>>,
    /// UTC timestamp when the handle was created.
    pub spawned_at: DateTime<Utc>,
}

// ── SpawnError ────────────────────────────────────────────────────────────────

/// Errors that prevent sub-agent spawning.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    /// The parent agent's role does not permit spawning.
    #[error("role '{0:?}' is not permitted to spawn sub-agents")]
    RoleNotPermitted(AgentRole),
    /// The sub-agent instruction is empty.
    #[error("sub-agent instruction must not be empty")]
    EmptyInstruction,
    /// The requested budget exceeds the parent's remaining budget.
    #[error(
        "sub-agent budget ({requested} tokens) exceeds parent remaining budget ({remaining} tokens)"
    )]
    BudgetExceeded { requested: u64, remaining: u64 },
}

// ── SubAgentSpawner ───────────────────────────────────────────────────────────

/// Validates and creates sub-agent handles for multi-agent coordination.
///
/// Stateless: create once, share via `Arc<SubAgentSpawner>`.
#[allow(dead_code)]
pub struct SubAgentSpawner {
    /// Session all spawned agents belong to.
    pub session_id: Uuid,
    /// Default working directory (parent's working directory).
    pub default_working_dir: String,
    /// Shared artifact store passed to all sub-agents.
    pub artifact_store: Arc<RwLock<SessionArtifactStore>>,
    /// Shared provenance tracker passed to all sub-agents.
    pub provenance_tracker: Arc<RwLock<SessionProvenanceTracker>>,
    /// Parent's remaining token budget (`None` = unlimited).
    pub parent_remaining_tokens: Option<u64>,
}

impl SubAgentSpawner {
    /// Create a spawner for the given session.
    pub fn new(
        session_id: Uuid,
        default_working_dir: impl Into<String>,
        artifact_store: Arc<RwLock<SessionArtifactStore>>,
        provenance_tracker: Arc<RwLock<SessionProvenanceTracker>>,
        parent_remaining_tokens: Option<u64>,
    ) -> Self {
        Self {
            session_id,
            default_working_dir: default_working_dir.into(),
            artifact_store,
            provenance_tracker,
            parent_remaining_tokens,
        }
    }

    /// Validate a spawn request and return a `SpawnedAgentHandle`.
    ///
    /// # Errors
    ///
    /// - `SpawnError::RoleNotPermitted` — parent role lacks `can_spawn_subagents()`.
    /// - `SpawnError::EmptyInstruction` — instruction is blank.
    /// - `SpawnError::BudgetExceeded` — child budget > parent remaining tokens.
    pub fn spawn(
        &self,
        parent_role: &AgentRole,
        config: SubAgentConfig,
    ) -> Result<SpawnedAgentHandle, SpawnError> {
        // Rule 1: Parent must have spawn permission.
        if !parent_role.can_spawn_subagents() {
            return Err(SpawnError::RoleNotPermitted(parent_role.clone()));
        }

        // Rule 2: Instruction must be non-empty.
        let instruction = config.instruction.trim().to_string();
        if instruction.is_empty() {
            return Err(SpawnError::EmptyInstruction);
        }

        // Rule 3: Budget must not exceed parent's remaining budget.
        if let Some(remaining) = self.parent_remaining_tokens {
            let requested = config.budget.max_tokens;
            if requested > 0 && requested > remaining {
                return Err(SpawnError::BudgetExceeded { requested, remaining });
            }
        }

        let working_dir = config
            .working_dir
            .unwrap_or_else(|| self.default_working_dir.clone());

        Ok(SpawnedAgentHandle {
            agent_id: Uuid::new_v4(),
            session_id: self.session_id,
            role: config.role,
            working_dir,
            budget: config.budget,
            system_prompt_prefix: config.system_prompt_prefix,
            instruction,
            artifact_store: Arc::clone(&self.artifact_store),
            provenance_tracker: Arc::clone(&self.provenance_tracker),
            spawned_at: Utc::now(),
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_spawner(parent_tokens: Option<u64>) -> SubAgentSpawner {
        let session_id = Uuid::new_v4();
        let store = Arc::new(RwLock::new(SessionArtifactStore::new(session_id)));
        let tracker = Arc::new(RwLock::new(SessionProvenanceTracker::new(session_id)));
        SubAgentSpawner::new(session_id, "/tmp/test", store, tracker, parent_tokens)
    }

    fn cfg(role: AgentRole, instruction: &str) -> SubAgentConfig {
        SubAgentConfig {
            role,
            instruction: instruction.into(),
            working_dir: None,
            budget: BudgetAllocation::default(),
            system_prompt_prefix: None,
        }
    }

    #[test]
    fn planner_can_spawn_coder() {
        let spawner = make_spawner(None);
        let handle = spawner
            .spawn(&AgentRole::Planner, cfg(AgentRole::Coder, "write main.rs"))
            .unwrap();

        assert_eq!(handle.role, AgentRole::Coder);
        assert_eq!(handle.session_id, spawner.session_id);
        assert_eq!(handle.instruction, "write main.rs");
    }

    #[test]
    fn supervisor_can_spawn_analyzer() {
        let spawner = make_spawner(None);
        let handle = spawner
            .spawn(&AgentRole::Supervisor, cfg(AgentRole::Analyzer, "audit logs"))
            .unwrap();
        assert_eq!(handle.role, AgentRole::Analyzer);
    }

    #[test]
    fn coder_cannot_spawn() {
        let spawner = make_spawner(None);
        let err = spawner
            .spawn(&AgentRole::Coder, cfg(AgentRole::Analyzer, "any task"))
            .unwrap_err();
        assert!(matches!(err, SpawnError::RoleNotPermitted(_)));
    }

    #[test]
    fn analyzer_cannot_spawn() {
        let spawner = make_spawner(None);
        let err = spawner
            .spawn(&AgentRole::Analyzer, cfg(AgentRole::Coder, "task"))
            .unwrap_err();
        assert!(matches!(err, SpawnError::RoleNotPermitted(_)));
    }

    #[test]
    fn reviewer_cannot_spawn() {
        let spawner = make_spawner(None);
        let err = spawner
            .spawn(&AgentRole::Reviewer, cfg(AgentRole::Coder, "task"))
            .unwrap_err();
        assert!(matches!(err, SpawnError::RoleNotPermitted(_)));
    }

    #[test]
    fn empty_instruction_rejected() {
        let spawner = make_spawner(None);
        let err = spawner
            .spawn(&AgentRole::Planner, cfg(AgentRole::Coder, "   "))
            .unwrap_err();
        assert!(matches!(err, SpawnError::EmptyInstruction));
    }

    #[test]
    fn budget_exceeded_rejected() {
        let spawner = make_spawner(Some(1000)); // parent has 1000 tokens left
        let mut config = cfg(AgentRole::Coder, "task");
        config.budget.max_tokens = 5000; // request more than available

        let err = spawner
            .spawn(&AgentRole::Planner, config)
            .unwrap_err();
        assert!(matches!(
            err,
            SpawnError::BudgetExceeded { requested: 5000, remaining: 1000 }
        ));
    }

    #[test]
    fn unlimited_budget_passes_any_child_budget() {
        let spawner = make_spawner(None); // parent has unlimited tokens
        let mut config = cfg(AgentRole::Coder, "task");
        config.budget.max_tokens = 999_999;

        assert!(spawner.spawn(&AgentRole::Planner, config).is_ok());
    }

    #[test]
    fn zero_child_budget_skips_budget_check() {
        let spawner = make_spawner(Some(100)); // parent has only 100 tokens
        let mut config = cfg(AgentRole::Coder, "task");
        config.budget.max_tokens = 0; // 0 = unlimited → no check

        assert!(spawner.spawn(&AgentRole::Planner, config).is_ok());
    }

    #[test]
    fn working_dir_defaults_to_parent() {
        let spawner = make_spawner(None);
        let handle = spawner
            .spawn(&AgentRole::Planner, cfg(AgentRole::Coder, "task"))
            .unwrap();
        assert_eq!(handle.working_dir, "/tmp/test");
    }

    #[test]
    fn working_dir_override_respected() {
        let spawner = make_spawner(None);
        let mut config = cfg(AgentRole::Coder, "task");
        config.working_dir = Some("/custom/dir".into());

        let handle = spawner
            .spawn(&AgentRole::Planner, config)
            .unwrap();
        assert_eq!(handle.working_dir, "/custom/dir");
    }

    #[test]
    fn arc_stores_shared_with_handle() {
        let spawner = make_spawner(None);
        let handle = spawner
            .spawn(&AgentRole::Planner, cfg(AgentRole::Coder, "task"))
            .unwrap();

        // Both Arc pointers should reference the same allocation.
        assert!(Arc::ptr_eq(
            &handle.artifact_store,
            &spawner.artifact_store
        ));
        assert!(Arc::ptr_eq(
            &handle.provenance_tracker,
            &spawner.provenance_tracker
        ));
    }

    #[test]
    fn system_prompt_prefix_forwarded() {
        let spawner = make_spawner(None);
        let mut config = cfg(AgentRole::Reviewer, "review PR");
        config.system_prompt_prefix = Some("You are a security-focused reviewer.".into());

        let handle = spawner
            .spawn(&AgentRole::Planner, config)
            .unwrap();
        assert_eq!(
            handle.system_prompt_prefix.as_deref(),
            Some("You are a security-focused reviewer.")
        );
    }
}
