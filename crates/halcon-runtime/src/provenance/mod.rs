//! Session-level provenance tracker for multi-agent artifact lineage.
//!
//! Records the complete execution lineage of every artifact produced in a
//! session: which agent created it, which tool was used, at what time, and
//! which prior artifacts were consumed as inputs.
//!
//! ## Design
//!
//! - One `ArtifactProvenance` record per artifact (keyed by `artifact_id`).
//! - `dependency_chain()` reconstructs the full lineage graph recursively.
//! - Thread-safe: callers wrap in `Arc<tokio::sync::RwLock<_>>`.
//!
//! ## Relation to task-level tracker
//!
//! `halcon-cli::repl::bridges::provenance_tracker::ProvenanceTracker` tracks
//! per-task execution stats (tokens, model, delegation). This module tracks
//! per-artifact lineage for multi-agent audit and reproducibility.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use halcon_core::types::AgentRole;

// ── ArtifactProvenance ────────────────────────────────────────────────────────

/// Complete provenance record for a single session artifact.
///
/// Every artifact stored in `SessionArtifactStore` should have a corresponding
/// `ArtifactProvenance` entry recording how it was produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactProvenance {
    /// The artifact this record describes.
    pub artifact_id: Uuid,
    /// UUID of the session that produced the artifact.
    pub session_id: Uuid,
    /// UUID of the specific agent that produced the artifact.
    pub created_by_agent: Uuid,
    /// Functional role the producing agent held at creation time.
    pub agent_role: AgentRole,
    /// Name of the tool invoked to produce the artifact (e.g., `"bash"`,
    /// `"file_write"`, `"model_synthesis"`, `"search"`).
    pub tool_invoked: String,
    /// Artifact IDs that were consumed as inputs to produce this artifact.
    /// Forms the dependency edges of the lineage DAG.
    pub input_artifacts: Vec<Uuid>,
    /// UTC timestamp when the artifact was created.
    pub created_at: DateTime<Utc>,
    /// Optional human-readable description of the step.
    pub description: Option<String>,
}

impl ArtifactProvenance {
    /// Create a new provenance record.
    pub fn new(
        artifact_id: Uuid,
        session_id: Uuid,
        created_by_agent: Uuid,
        agent_role: AgentRole,
        tool_invoked: impl Into<String>,
        input_artifacts: Vec<Uuid>,
    ) -> Self {
        Self {
            artifact_id,
            session_id,
            created_by_agent,
            agent_role,
            tool_invoked: tool_invoked.into(),
            input_artifacts,
            created_at: Utc::now(),
            description: None,
        }
    }

    /// Attach a human-readable description to the record.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

// ── SessionProvenanceTracker ──────────────────────────────────────────────────

/// Tracks artifact lineage for all agents in a session.
///
/// Wrap in `Arc<tokio::sync::RwLock<SessionProvenanceTracker>>` to share
/// across concurrent agent tasks.
#[derive(Debug)]
pub struct SessionProvenanceTracker {
    /// Session this tracker is scoped to.
    pub session_id: Uuid,
    /// artifact_id → provenance record.
    records: HashMap<Uuid, ArtifactProvenance>,
    /// agent_id → list of artifact_ids it produced.
    by_agent: HashMap<Uuid, Vec<Uuid>>,
}

impl SessionProvenanceTracker {
    /// Create an empty tracker for the given session.
    pub fn new(session_id: Uuid) -> Self {
        Self {
            session_id,
            records: HashMap::new(),
            by_agent: HashMap::new(),
        }
    }

    /// Record provenance for an artifact.
    ///
    /// Overwrites any prior record for the same `artifact_id` (allows
    /// provenance correction during the same session turn).
    pub fn record(&mut self, prov: ArtifactProvenance) {
        self.by_agent
            .entry(prov.created_by_agent)
            .or_default()
            .push(prov.artifact_id);
        self.records.insert(prov.artifact_id, prov);
    }

    /// Get the provenance record for a specific artifact.
    pub fn get(&self, artifact_id: Uuid) -> Option<&ArtifactProvenance> {
        self.records.get(&artifact_id)
    }

    /// List all artifacts produced by a specific agent.
    pub fn artifacts_by_agent(&self, agent_id: Uuid) -> Vec<&ArtifactProvenance> {
        self.by_agent
            .get(&agent_id)
            .map(|ids| ids.iter().filter_map(|id| self.records.get(id)).collect())
            .unwrap_or_default()
    }

    /// Reconstruct the full dependency chain for an artifact.
    ///
    /// Returns provenance records in topological order (direct inputs first,
    /// then transitive). Handles cycles safely (visited set prevents loops).
    /// Returns an empty `Vec` if the artifact has no provenance record.
    pub fn dependency_chain(&self, artifact_id: Uuid) -> Vec<&ArtifactProvenance> {
        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        self.collect_chain(artifact_id, &mut visited, &mut result);
        result
    }

    fn collect_chain<'a>(
        &'a self,
        id: Uuid,
        visited: &mut std::collections::HashSet<Uuid>,
        out: &mut Vec<&'a ArtifactProvenance>,
    ) {
        if !visited.insert(id) {
            return; // already visited — cycle guard
        }
        if let Some(prov) = self.records.get(&id) {
            // Recurse into inputs first (depth-first).
            for &input_id in &prov.input_artifacts {
                self.collect_chain(input_id, visited, out);
            }
            out.push(prov);
        }
    }

    /// Total number of provenance records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether any provenance has been recorded.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Return all records in an unspecified order (for auditing/export).
    pub fn all_records(&self) -> Vec<&ArtifactProvenance> {
        self.records.values().collect()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tracker() -> SessionProvenanceTracker {
        SessionProvenanceTracker::new(Uuid::new_v4())
    }

    fn prov(
        session_id: Uuid,
        artifact_id: Uuid,
        agent_id: Uuid,
        role: AgentRole,
        tool: &str,
        inputs: Vec<Uuid>,
    ) -> ArtifactProvenance {
        ArtifactProvenance::new(artifact_id, session_id, agent_id, role, tool, inputs)
    }

    #[test]
    fn record_and_get() {
        let mut t = make_tracker();
        let session = t.session_id;
        let art = Uuid::new_v4();
        let agent = Uuid::new_v4();

        let p = prov(session, art, agent, AgentRole::Coder, "file_write", vec![]);
        t.record(p);

        let got = t.get(art).unwrap();
        assert_eq!(got.artifact_id, art);
        assert_eq!(got.agent_role, AgentRole::Coder);
        assert_eq!(got.tool_invoked, "file_write");
    }

    #[test]
    fn artifacts_by_agent() {
        let mut t = make_tracker();
        let session = t.session_id;
        let a1 = Uuid::new_v4();
        let a2 = Uuid::new_v4();

        t.record(prov(session, Uuid::new_v4(), a1, AgentRole::Coder, "bash", vec![]));
        t.record(prov(session, Uuid::new_v4(), a1, AgentRole::Coder, "file_write", vec![]));
        t.record(prov(session, Uuid::new_v4(), a2, AgentRole::Analyzer, "grep", vec![]));

        assert_eq!(t.artifacts_by_agent(a1).len(), 2);
        assert_eq!(t.artifacts_by_agent(a2).len(), 1);
        assert!(t.artifacts_by_agent(Uuid::new_v4()).is_empty());
    }

    #[test]
    fn dependency_chain_simple() {
        let mut t = make_tracker();
        let session = t.session_id;
        let agent = Uuid::new_v4();

        let src = Uuid::new_v4();
        let mid = Uuid::new_v4();
        let fin = Uuid::new_v4();

        // src → mid → fin
        t.record(prov(session, src, agent, AgentRole::Analyzer, "search", vec![]));
        t.record(prov(session, mid, agent, AgentRole::Analyzer, "grep", vec![src]));
        t.record(prov(session, fin, agent, AgentRole::Coder, "file_write", vec![mid]));

        let chain = t.dependency_chain(fin);
        // Expect: src, mid, fin (depth-first, leaves first).
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].artifact_id, src);
        assert_eq!(chain[1].artifact_id, mid);
        assert_eq!(chain[2].artifact_id, fin);
    }

    #[test]
    fn dependency_chain_no_provenance_returns_empty() {
        let t = make_tracker();
        assert!(t.dependency_chain(Uuid::new_v4()).is_empty());
    }

    #[test]
    fn dependency_chain_cycle_guard() {
        // Provenance shouldn't form cycles in practice but must not panic.
        let mut t = make_tracker();
        let session = t.session_id;
        let agent = Uuid::new_v4();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        // Artificial cycle: a → b → a
        t.record(prov(session, a, agent, AgentRole::Coder, "tool_a", vec![b]));
        t.record(prov(session, b, agent, AgentRole::Coder, "tool_b", vec![a]));

        // Must terminate without stack overflow.
        let chain = t.dependency_chain(a);
        assert!(!chain.is_empty()); // contains a and b exactly once each
        assert!(chain.len() <= 2);
    }

    #[test]
    fn with_description() {
        let prov = ArtifactProvenance::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            AgentRole::Reviewer,
            "model_synthesis",
            vec![],
        )
        .with_description("Final review summary");

        assert_eq!(prov.description.as_deref(), Some("Final review summary"));
    }

    #[test]
    fn len_and_is_empty() {
        let mut t = make_tracker();
        assert!(t.is_empty());
        assert_eq!(t.len(), 0);

        let session = t.session_id;
        t.record(prov(session, Uuid::new_v4(), Uuid::new_v4(), AgentRole::Coder, "bash", vec![]));

        assert!(!t.is_empty());
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn all_records_returns_all() {
        let mut t = make_tracker();
        let session = t.session_id;
        let agent = Uuid::new_v4();

        for tool in ["bash", "grep", "file_write"] {
            t.record(prov(session, Uuid::new_v4(), agent, AgentRole::Coder, tool, vec![]));
        }

        assert_eq!(t.all_records().len(), 3);
    }
}
