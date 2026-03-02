//! Evidence Graph — Structured evidence tracking with provenance.
//!
//! Each tool call produces an `EvidenceNode` capturing:
//! - Source (tool name, arguments)
//! - Result quality (byte count, binary flag, error flag)
//! - Timestamps
//! - Dependencies between nodes (which evidence led to which follow-up)
//!
//! At synthesis time, the graph enforces that any claim in the final output
//! must trace back to at least one evidence node. This prevents hallucination
//! in multi-step investigations where intermediate results are forgotten.
//!
//! ## Design
//! - Nodes are append-only (immutable once created).
//! - Edges represent causal "led-to" relationships (node A's output was used to
//!   decide to call node B).
//! - `synthesis_coverage()` returns the fraction of nodes referenced by synthesis.
//! - Works alongside `EvidenceBundle` (aggregate metrics) — this module adds
//!   per-node granularity.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use super::evidence_pipeline::MIN_EVIDENCE_BYTES;

/// Unique identifier for an evidence node.
pub(crate) type NodeId = u32;

/// Quality classification of a tool result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvidenceQuality {
    /// ≥ 30 bytes of readable text.
    Good,
    /// < 30 bytes but non-empty.
    Partial,
    /// Empty or error result.
    Empty,
    /// Binary content detected.
    Binary,
}

/// A single evidence node — one tool call's contribution.
#[derive(Debug, Clone)]
pub(crate) struct EvidenceNode {
    pub id: NodeId,
    pub tool_name: String,
    pub tool_args_summary: String,
    pub quality: EvidenceQuality,
    pub byte_count: usize,
    pub timestamp: Instant,
    pub round: u32,
    /// Error message if the tool call failed.
    pub error: Option<String>,
}

/// Directed edge: source node's output caused target node's invocation.
#[derive(Debug, Clone)]
struct EvidenceEdge {
    from: NodeId,
    to: NodeId,
}

/// Evidence graph tracking all tool-produced evidence in a session.
#[derive(Debug, Clone)]
pub(crate) struct EvidenceGraph {
    nodes: HashMap<NodeId, EvidenceNode>,
    edges: Vec<EvidenceEdge>,
    next_id: NodeId,
    /// Nodes referenced during synthesis.
    referenced: HashSet<NodeId>,
}

impl EvidenceGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            next_id: 0,
            referenced: HashSet::new(),
        }
    }

    /// Add an evidence node. Returns its ID for later reference.
    pub fn add_node(
        &mut self,
        tool_name: &str,
        tool_args_summary: &str,
        byte_count: usize,
        is_binary: bool,
        error: Option<&str>,
        round: u32,
    ) -> NodeId {
        let quality = if error.is_some() || byte_count == 0 {
            EvidenceQuality::Empty
        } else if is_binary {
            EvidenceQuality::Binary
        } else if byte_count < MIN_EVIDENCE_BYTES {
            EvidenceQuality::Partial
        } else {
            EvidenceQuality::Good
        };

        let id = self.next_id;
        self.next_id += 1;

        self.nodes.insert(id, EvidenceNode {
            id,
            tool_name: tool_name.to_string(),
            tool_args_summary: tool_args_summary.to_string(),
            quality,
            byte_count,
            timestamp: Instant::now(),
            round,
            error: error.map(|e| e.to_string()),
        });

        id
    }

    /// Record a causal edge: `from` node's output led to `to` node's invocation.
    pub fn add_edge(&mut self, from: NodeId, to: NodeId) {
        if self.nodes.contains_key(&from) && self.nodes.contains_key(&to) {
            self.edges.push(EvidenceEdge { from, to });
        }
    }

    /// Mark a node as referenced by synthesis.
    pub fn mark_referenced(&mut self, id: NodeId) {
        if self.nodes.contains_key(&id) {
            self.referenced.insert(id);
        }
    }

    /// Mark multiple nodes as referenced.
    pub fn mark_referenced_batch(&mut self, ids: &[NodeId]) {
        for &id in ids {
            self.mark_referenced(id);
        }
    }

    /// Total number of evidence nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of nodes with Good quality.
    pub fn good_node_count(&self) -> usize {
        self.nodes.values().filter(|n| n.quality == EvidenceQuality::Good).count()
    }

    /// Number of nodes referenced by synthesis.
    pub fn referenced_count(&self) -> usize {
        self.referenced.len()
    }

    /// Fraction of Good nodes that were referenced. 1.0 = full coverage.
    /// Returns 1.0 if there are no Good nodes (nothing to cover).
    pub fn synthesis_coverage(&self) -> f64 {
        let good: HashSet<NodeId> = self.nodes.iter()
            .filter(|(_, n)| n.quality == EvidenceQuality::Good)
            .map(|(&id, _)| id)
            .collect();

        if good.is_empty() {
            return 1.0;
        }

        let covered = good.intersection(&self.referenced).count();
        covered as f64 / good.len() as f64
    }

    /// Get all Good-quality nodes that were NOT referenced by synthesis.
    /// These represent potentially missed evidence.
    pub fn unreferenced_evidence(&self) -> Vec<&EvidenceNode> {
        self.nodes.values()
            .filter(|n| n.quality == EvidenceQuality::Good && !self.referenced.contains(&n.id))
            .collect()
    }

    /// Get a node by ID.
    pub fn get_node(&self, id: NodeId) -> Option<&EvidenceNode> {
        self.nodes.get(&id)
    }

    /// Get all nodes for a specific round.
    pub fn nodes_for_round(&self, round: u32) -> Vec<&EvidenceNode> {
        self.nodes.values().filter(|n| n.round == round).collect()
    }

    /// Total readable bytes across all Good and Partial nodes.
    pub fn total_evidence_bytes(&self) -> usize {
        self.nodes.values()
            .filter(|n| matches!(n.quality, EvidenceQuality::Good | EvidenceQuality::Partial))
            .map(|n| n.byte_count)
            .sum()
    }

    /// Summary string for observability/debugging.
    pub fn summary(&self) -> String {
        let good = self.good_node_count();
        let total = self.node_count();
        let coverage = self.synthesis_coverage();
        format!(
            "EvidenceGraph: {total} nodes ({good} good), {:.0}% coverage, {} edges",
            coverage * 100.0,
            self.edges.len(),
        )
    }
}

// ── Trait implementation ──────────────────────────────────────────────────────

impl halcon_core::traits::EvidenceTracker for EvidenceGraph {
    fn add_node(
        &mut self,
        tool_name: &str,
        args_summary: &str,
        byte_count: usize,
        is_binary: bool,
        error: Option<&str>,
        round: u32,
    ) -> u32 {
        self.add_node(tool_name, args_summary, byte_count, is_binary, error, round)
    }

    fn add_edge(&mut self, from: u32, to: u32) {
        self.add_edge(from, to)
    }

    fn mark_referenced(&mut self, id: u32) {
        self.mark_referenced(id)
    }

    fn mark_referenced_batch(&mut self, ids: &[u32]) {
        self.mark_referenced_batch(ids)
    }

    fn synthesis_coverage(&self) -> f64 {
        self.synthesis_coverage()
    }

    fn unreferenced_summaries(&self) -> Vec<String> {
        self.unreferenced_evidence()
            .into_iter()
            .map(|node| format!("{}: {}", node.tool_name, node.tool_args_summary))
            .collect()
    }

    fn summary(&self) -> String {
        self.summary()
    }

    fn node_count(&self) -> usize {
        self.node_count()
    }

    fn total_evidence_bytes(&self) -> usize {
        self.total_evidence_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_graph_full_coverage() {
        let g = EvidenceGraph::new();
        assert_eq!(g.node_count(), 0);
        assert_eq!(g.synthesis_coverage(), 1.0);
    }

    #[test]
    fn add_good_node() {
        let mut g = EvidenceGraph::new();
        let id = g.add_node("file_read", "path=/src/main.rs", 500, false, None, 1);
        assert_eq!(id, 0);
        assert_eq!(g.node_count(), 1);
        assert_eq!(g.good_node_count(), 1);
        let node = g.get_node(id).unwrap();
        assert_eq!(node.quality, EvidenceQuality::Good);
    }

    #[test]
    fn binary_detection() {
        let mut g = EvidenceGraph::new();
        let id = g.add_node("file_read", "path=report.pdf", 2048, true, None, 1);
        let node = g.get_node(id).unwrap();
        assert_eq!(node.quality, EvidenceQuality::Binary);
    }

    #[test]
    fn error_detection() {
        let mut g = EvidenceGraph::new();
        let id = g.add_node("bash", "ls /nonexistent", 0, false, Some("not found"), 1);
        let node = g.get_node(id).unwrap();
        assert_eq!(node.quality, EvidenceQuality::Empty);
    }

    #[test]
    fn partial_quality() {
        let mut g = EvidenceGraph::new();
        let id = g.add_node("file_read", "path=tiny.txt", 15, false, None, 1);
        let node = g.get_node(id).unwrap();
        assert_eq!(node.quality, EvidenceQuality::Partial);
    }

    #[test]
    fn synthesis_coverage_tracking() {
        let mut g = EvidenceGraph::new();
        let id1 = g.add_node("file_read", "a.rs", 100, false, None, 1);
        let id2 = g.add_node("file_read", "b.rs", 200, false, None, 2);
        let _id3 = g.add_node("file_read", "c.rs", 300, false, None, 3);

        assert_eq!(g.synthesis_coverage(), 0.0); // 0/3 referenced

        g.mark_referenced(id1);
        assert!((g.synthesis_coverage() - 1.0 / 3.0).abs() < 0.01);

        g.mark_referenced(id2);
        assert!((g.synthesis_coverage() - 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn unreferenced_evidence() {
        let mut g = EvidenceGraph::new();
        let id1 = g.add_node("file_read", "a.rs", 100, false, None, 1);
        let _id2 = g.add_node("file_read", "b.rs", 200, false, None, 2);

        g.mark_referenced(id1);
        let unreferenced = g.unreferenced_evidence();
        assert_eq!(unreferenced.len(), 1);
        assert_eq!(unreferenced[0].tool_args_summary, "b.rs");
    }

    #[test]
    fn edges_require_valid_nodes() {
        let mut g = EvidenceGraph::new();
        let id1 = g.add_node("file_read", "a.rs", 100, false, None, 1);
        g.add_edge(id1, 999); // 999 doesn't exist
        assert_eq!(g.edges.len(), 0);

        let id2 = g.add_node("bash", "grep foo a.rs", 50, false, None, 2);
        g.add_edge(id1, id2);
        assert_eq!(g.edges.len(), 1);
    }

    #[test]
    fn nodes_for_round() {
        let mut g = EvidenceGraph::new();
        g.add_node("file_read", "a.rs", 100, false, None, 1);
        g.add_node("bash", "ls", 50, false, None, 1);
        g.add_node("file_read", "b.rs", 200, false, None, 2);

        assert_eq!(g.nodes_for_round(1).len(), 2);
        assert_eq!(g.nodes_for_round(2).len(), 1);
        assert_eq!(g.nodes_for_round(3).len(), 0);
    }

    #[test]
    fn total_evidence_bytes() {
        let mut g = EvidenceGraph::new();
        g.add_node("file_read", "a.rs", 100, false, None, 1);
        g.add_node("file_read", "b.rs", 200, false, None, 2);
        g.add_node("file_read", "c.pdf", 5000, true, None, 3); // Binary, excluded
        g.add_node("bash", "fail", 0, false, Some("error"), 4); // Error, excluded

        assert_eq!(g.total_evidence_bytes(), 300);
    }

    #[test]
    fn summary_format() {
        let mut g = EvidenceGraph::new();
        g.add_node("file_read", "a.rs", 100, false, None, 1);
        g.add_node("bash", "ls", 0, false, Some("err"), 1);
        let summary = g.summary();
        assert!(summary.contains("2 nodes"));
        assert!(summary.contains("1 good"));
    }

    #[test]
    fn mark_referenced_batch() {
        let mut g = EvidenceGraph::new();
        let id1 = g.add_node("file_read", "a.rs", 100, false, None, 1);
        let id2 = g.add_node("file_read", "b.rs", 200, false, None, 2);
        g.mark_referenced_batch(&[id1, id2]);
        assert_eq!(g.referenced_count(), 2);
        assert_eq!(g.synthesis_coverage(), 1.0);
    }

    #[test]
    fn coverage_ignores_non_good_nodes() {
        let mut g = EvidenceGraph::new();
        g.add_node("file_read", "a.rs", 100, false, None, 1); // Good
        g.add_node("file_read", "b.pdf", 5000, true, None, 2); // Binary
        g.add_node("bash", "fail", 0, false, Some("err"), 3);  // Empty

        // Only 1 Good node, 0 referenced → 0% coverage
        assert_eq!(g.synthesis_coverage(), 0.0);

        g.mark_referenced(0); // Mark the Good node
        assert_eq!(g.synthesis_coverage(), 1.0);
    }

    #[test]
    fn uses_shared_min_evidence_bytes() {
        use super::MIN_EVIDENCE_BYTES;
        let mut g = EvidenceGraph::new();
        let partial = g.add_node("f", "a", MIN_EVIDENCE_BYTES - 1, false, None, 1);
        let good = g.add_node("f", "b", MIN_EVIDENCE_BYTES, false, None, 1);
        assert_eq!(g.get_node(partial).unwrap().quality, EvidenceQuality::Partial);
        assert_eq!(g.get_node(good).unwrap().quality, EvidenceQuality::Good);
    }
}
