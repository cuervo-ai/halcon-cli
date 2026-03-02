//! Trait interface for evidence graph tracking.
//!
//! Implementations track per-tool-call evidence nodes with quality classification,
//! causal edges, and synthesis coverage metrics.

/// Evidence graph tracking — structured evidence tracking with provenance.
///
/// Implementations record evidence produced by tool calls and track which
/// evidence was referenced during synthesis, enabling coverage metrics
/// and unreferenced-evidence hints.
pub trait EvidenceTracker: Send + Sync {
    /// Add an evidence node. Returns its ID for later reference.
    fn add_node(
        &mut self,
        tool_name: &str,
        args_summary: &str,
        byte_count: usize,
        is_binary: bool,
        error: Option<&str>,
        round: u32,
    ) -> u32;

    /// Record a causal edge: `from` node's output led to `to` node's invocation.
    fn add_edge(&mut self, from: u32, to: u32);

    /// Mark a node as referenced by synthesis.
    fn mark_referenced(&mut self, id: u32);

    /// Mark multiple nodes as referenced.
    fn mark_referenced_batch(&mut self, ids: &[u32]);

    /// Fraction of Good nodes that were referenced. 1.0 = full coverage.
    fn synthesis_coverage(&self) -> f64;

    /// Get summaries of unreferenced Good-quality evidence nodes.
    fn unreferenced_summaries(&self) -> Vec<String>;

    /// Summary string for observability/debugging.
    fn summary(&self) -> String;

    /// Total number of evidence nodes.
    fn node_count(&self) -> usize;

    /// Total readable bytes across Good and Partial nodes.
    fn total_evidence_bytes(&self) -> usize;
}
