//! Session-scoped ArtifactStore for multi-agent execution.
//!
//! Promotes artifact storage to the runtime level so that multiple concurrent
//! agents in the same session can share a single content-addressed store.
//!
//! ## Design
//!
//! - Keyed by `session_id` (fixed at construction).
//! - Content-addressed: same bytes → same `artifact_id` (SHA-256 dedup).
//! - Thread-safe by design: callers wrap in `Arc<tokio::sync::RwLock<_>>`.
//! - Agents identify writes via `agent_id` (UUID).
//!
//! ## Relation to task-level store
//!
//! `halcon-cli::repl::bridges::artifact_store::ArtifactStore` is a private,
//! task-scoped store used within a single agent turn. This module provides
//! the session-scoped, runtime-exported counterpart for multi-agent sessions.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

// ── SessionArtifactKind ───────────────────────────────────────────────────────

/// Semantic classification of a session-level artifact.
///
/// Distinct from `halcon_core::types::ArtifactType` (task-level).
/// This enum operates at the runtime/session coordination layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionArtifactKind {
    /// A file produced or modified on disk (`location` records the path).
    File,
    /// Raw output of a tool invocation (JSON blob or plain text).
    ToolOutput,
    /// Synthesized text produced by a model completion turn.
    ModelResponse,
    /// Summary or analysis document for human consumption.
    Report,
    /// Intermediate reasoning artifact (plan step, chain-of-thought, etc.).
    Reasoning,
    /// Structured search result set.
    SearchResult,
    /// Caller-supplied label for domain-specific artifacts.
    Custom(String),
}

impl std::fmt::Display for SessionArtifactKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::File => "file",
            Self::ToolOutput => "tool_output",
            Self::ModelResponse => "model_response",
            Self::Report => "report",
            Self::Reasoning => "reasoning",
            Self::SearchResult => "search_result",
            Self::Custom(s) => s.as_str(),
        };
        f.write_str(s)
    }
}

// ── SessionArtifact ───────────────────────────────────────────────────────────

/// A single artifact stored in `SessionArtifactStore`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionArtifact {
    /// Stable unique ID — content-addressed: same bytes → same `artifact_id`.
    pub artifact_id: Uuid,
    /// Human-readable name (e.g., `"analysis.md"`, `"grep_output"`).
    pub name: String,
    /// Semantic classification.
    pub kind: SessionArtifactKind,
    /// SHA-256 hex digest — used for deduplication and integrity checks.
    pub content_hash: String,
    /// Size of the artifact content in bytes.
    pub size_bytes: u64,
    /// Optional filesystem path when the artifact was written to disk.
    pub location: Option<String>,
    /// UUID of the agent that produced this artifact.
    pub produced_by: Uuid,
    /// UTC timestamp of first creation.
    pub created_at: DateTime<Utc>,
}

// ── SessionArtifactStore ──────────────────────────────────────────────────────

/// Shared, session-scoped content-addressed artifact store.
///
/// All agents participating in a session read from and write to the same
/// instance. Wrap in `Arc<tokio::sync::RwLock<SessionArtifactStore>>` for
/// concurrent multi-agent access:
///
/// ```rust,ignore
/// let store = Arc::new(tokio::sync::RwLock::new(
///     SessionArtifactStore::new(session_id)
/// ));
/// // Writer (agent A):
/// store.write().await.store_artifact(agent_a, "out.md", Report, b"...", None);
/// // Reader (agent B):
/// let all = store.read().await.list_artifacts();
/// ```
#[derive(Debug)]
pub struct SessionArtifactStore {
    /// Session this store is scoped to.
    pub session_id: Uuid,
    /// Primary index: content hash → artifact.
    by_hash: HashMap<String, SessionArtifact>,
    /// Secondary index: agent_id → content hashes produced by that agent.
    by_agent: HashMap<Uuid, Vec<String>>,
    /// Insertion-ordered list of hashes (for deterministic `list_artifacts()`).
    insertion_order: Vec<String>,
}

impl SessionArtifactStore {
    /// Create an empty store bound to `session_id`.
    pub fn new(session_id: Uuid) -> Self {
        Self {
            session_id,
            by_hash: HashMap::new(),
            by_agent: HashMap::new(),
            insertion_order: Vec::new(),
        }
    }

    /// Store an artifact produced by `agent_id`.
    ///
    /// Computes SHA-256 over `content`. If the same hash is already present
    /// (content deduplication), the existing record is returned without
    /// duplication; the agent is still registered as a producer in the index.
    ///
    /// # Arguments
    /// - `agent_id` — UUID of the agent writing the artifact.
    /// - `name` — Human-readable name.
    /// - `kind` — Semantic classification.
    /// - `content` — Raw bytes (used for hash computation and size).
    /// - `location` — Optional filesystem path.
    pub fn store_artifact(
        &mut self,
        agent_id: Uuid,
        name: impl Into<String>,
        kind: SessionArtifactKind,
        content: &[u8],
        location: Option<String>,
    ) -> SessionArtifact {
        let hash = hex::encode(Sha256::digest(content));

        // Dedup: identical content is stored once; agent is added to the index.
        if let Some(existing) = self.by_hash.get(&hash) {
            self.by_agent.entry(agent_id).or_default().push(hash);
            return existing.clone();
        }

        let artifact = SessionArtifact {
            artifact_id: Uuid::new_v4(),
            name: name.into(),
            kind,
            content_hash: hash.clone(),
            size_bytes: content.len() as u64,
            location,
            produced_by: agent_id,
            created_at: Utc::now(),
        };

        self.by_hash.insert(hash.clone(), artifact.clone());
        self.by_agent.entry(agent_id).or_default().push(hash.clone());
        self.insertion_order.push(hash);
        artifact
    }

    /// Retrieve an artifact by its SHA-256 content hash.
    pub fn get_artifact(&self, hash: &str) -> Option<&SessionArtifact> {
        self.by_hash.get(hash)
    }

    /// Retrieve an artifact by its UUID.
    pub fn get_by_id(&self, id: Uuid) -> Option<&SessionArtifact> {
        self.by_hash.values().find(|a| a.artifact_id == id)
    }

    /// List all artifacts in insertion order.
    pub fn list_artifacts(&self) -> Vec<&SessionArtifact> {
        self.insertion_order
            .iter()
            .filter_map(|h| self.by_hash.get(h))
            .collect()
    }

    /// List artifacts produced by a specific agent, in insertion order.
    pub fn artifacts_by_agent(&self, agent_id: Uuid) -> Vec<&SessionArtifact> {
        self.by_agent
            .get(&agent_id)
            .map(|hashes| {
                hashes
                    .iter()
                    .filter_map(|h| self.by_hash.get(h))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Number of unique artifacts stored (deduplicated).
    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    /// Whether the store contains no artifacts.
    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }

    /// Total bytes across all unique artifacts (counted once per dedup group).
    pub fn total_size_bytes(&self) -> u64 {
        self.by_hash.values().map(|a| a.size_bytes).sum()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> SessionArtifactStore {
        SessionArtifactStore::new(Uuid::new_v4())
    }

    #[test]
    fn store_and_retrieve_by_hash() {
        let mut s = make_store();
        let agent = Uuid::new_v4();
        let art = s.store_artifact(agent, "out.md", SessionArtifactKind::Report, b"hello", None);
        assert_eq!(s.get_artifact(&art.content_hash).unwrap().name, "out.md");
    }

    #[test]
    fn store_and_retrieve_by_id() {
        let mut s = make_store();
        let agent = Uuid::new_v4();
        let art = s.store_artifact(
            agent,
            "result.json",
            SessionArtifactKind::ToolOutput,
            b"{}",
            None,
        );
        assert_eq!(s.get_by_id(art.artifact_id).unwrap().name, "result.json");
    }

    #[test]
    fn content_deduplication() {
        let mut s = make_store();
        let a1 = Uuid::new_v4();
        let a2 = Uuid::new_v4();
        let content = b"shared content";

        let r1 = s.store_artifact(a1, "file_a", SessionArtifactKind::File, content, None);
        let r2 = s.store_artifact(a2, "file_b", SessionArtifactKind::File, content, None);

        // Same content → same artifact_id, store has 1 unique artifact.
        assert_eq!(r1.artifact_id, r2.artifact_id);
        assert_eq!(s.len(), 1);
        // Both agents indexed.
        assert_eq!(s.artifacts_by_agent(a1).len(), 1);
        assert_eq!(s.artifacts_by_agent(a2).len(), 1);
    }

    #[test]
    fn list_artifacts_insertion_order() {
        let mut s = make_store();
        let agent = Uuid::new_v4();

        s.store_artifact(agent, "first", SessionArtifactKind::File, b"aaa", None);
        s.store_artifact(agent, "second", SessionArtifactKind::File, b"bbb", None);
        s.store_artifact(agent, "third", SessionArtifactKind::File, b"ccc", None);

        let names: Vec<&str> = s.list_artifacts().iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["first", "second", "third"]);
    }

    #[test]
    fn artifacts_by_agent_isolation() {
        let mut s = make_store();
        let a1 = Uuid::new_v4();
        let a2 = Uuid::new_v4();

        s.store_artifact(a1, "a", SessionArtifactKind::File, b"111", None);
        s.store_artifact(a1, "b", SessionArtifactKind::File, b"222", None);
        s.store_artifact(a2, "c", SessionArtifactKind::File, b"333", None);

        assert_eq!(s.artifacts_by_agent(a1).len(), 2);
        assert_eq!(s.artifacts_by_agent(a2).len(), 1);
        assert!(s.artifacts_by_agent(Uuid::new_v4()).is_empty());
    }

    #[test]
    fn total_size_dedup() {
        let mut s = make_store();
        let agent = Uuid::new_v4();
        let content = b"12345"; // 5 bytes

        s.store_artifact(agent, "x", SessionArtifactKind::File, content, None);
        s.store_artifact(agent, "y", SessionArtifactKind::File, content, None); // dedup
        s.store_artifact(agent, "z", SessionArtifactKind::File, b"67890", None); // 5 bytes

        // 2 unique artifacts × 5 bytes = 10 (not 15).
        assert_eq!(s.total_size_bytes(), 10);
    }

    #[test]
    fn empty_store_invariants() {
        let s = make_store();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert!(s.list_artifacts().is_empty());
        assert!(s.get_artifact("nonexistent").is_none());
        assert!(s.get_by_id(Uuid::new_v4()).is_none());
    }

    #[test]
    fn location_preserved() {
        let mut s = make_store();
        let agent = Uuid::new_v4();
        let art = s.store_artifact(
            agent,
            "patch.diff",
            SessionArtifactKind::File,
            b"--- a\n+++ b\n",
            Some("/tmp/patch.diff".into()),
        );
        assert_eq!(art.location.as_deref(), Some("/tmp/patch.diff"));
    }

    #[test]
    fn produced_by_recorded() {
        let mut s = make_store();
        let agent = Uuid::new_v4();
        let art = s.store_artifact(agent, "r", SessionArtifactKind::Reasoning, b"thoughts", None);
        assert_eq!(art.produced_by, agent);
    }
}
