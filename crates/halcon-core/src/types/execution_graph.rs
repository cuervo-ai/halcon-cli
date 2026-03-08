//! Declarative Execution Graph — structural representation of plan execution order.
//!
//! `ExecutionGraph` is derived from an `ExecutionPlan` via `to_execution_graph()`.
//! It makes the implicit sequential execution order explicit and validates structural
//! correctness BEFORE capability checking or provider invocation.
//!
//! # Design invariants
//! - Linear plans (default): node i connects to node i+1. Always acyclic.
//! - `GraphValidator` enforces: acyclicity, no orphan nodes, modality consistency,
//!   declared tools.
//! - Zero-drift guarantee: Rule 4 is skipped when `declared_tools` is empty.
//!
//! # Cost propagation (Step 10)
//! - `assign_base_costs(avg, multiplier)` sets `base_cost` per node by modality.
//! - `total_cost()` sums reachable-node costs via DFS from node 0.
//! - Budget enforcement becomes topology-aware: tool-heavy plans cost more than
//!   text-only plans of equal length.
//!
//! # Crate position
//! Pure data types + structural computation only — no I/O, no business logic.
//! Validation lives in `halcon-cli/src/repl/domain/graph_validator.rs`.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::types::capability_types::Modality;

/// Stable identity for a node in the execution graph.
///
/// Corresponds to the step index in the original `ExecutionPlan`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub usize);

/// A node in the execution graph, representing one plan step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionNode {
    /// Node identity (= step index in source plan).
    pub id: NodeId,
    /// Tool this step invokes. `None` = text-only step.
    pub tool: Option<String>,
    /// Interaction modality for this step.
    pub modality: Modality,
    /// Estimated token cost for this node.
    ///
    /// Default `0` until `ExecutionGraph::assign_base_costs()` is called.
    /// Set by cost propagation rules:
    /// - `Text` → `avg_input_tokens_per_step`
    /// - `ToolUse` | `Vision` → `avg_input_tokens_per_step × tool_cost_multiplier`
    #[serde(default)]
    pub base_cost: usize,
}

/// A directed edge from one node to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionEdge {
    pub from: NodeId,
    pub to: NodeId,
}

/// Structural representation of a plan's execution order with topology-aware cost.
///
/// Derived from `ExecutionPlan::to_execution_graph()`.
/// The default linear derivation produces an acyclic graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionGraph {
    /// Ordered list of execution nodes (1:1 with source plan steps).
    pub nodes: Vec<ExecutionNode>,
    /// Directed edges between nodes (linear by default).
    pub edges: Vec<ExecutionEdge>,
    /// Tool names declared in the plan's `CapabilityDescriptor.required_tools`.
    ///
    /// Used by Rule 4 in `GraphValidator` to verify all step tools are declared.
    /// Empty → Rule 4 is skipped (zero-drift guarantee for undecorated plans).
    pub declared_tools: Vec<String>,
}

impl ExecutionGraph {
    /// Assign node-level token costs based on modality (Step 10.1 cost rules).
    ///
    /// - `Text` node → `avg_input_tokens_per_step`
    /// - `ToolUse` | `Vision` node → `avg_input_tokens_per_step × tool_cost_multiplier`
    ///
    /// Must be called before `total_cost()`. Safe to call multiple times (idempotent given
    /// same parameters). Policy values are passed as primitives to avoid a cross-crate
    /// dependency on `PolicyConfig`.
    pub fn assign_base_costs(&mut self, avg_input_tokens_per_step: usize, tool_cost_multiplier: usize) {
        for node in &mut self.nodes {
            node.base_cost = match node.modality {
                Modality::Text => avg_input_tokens_per_step,
                Modality::ToolUse | Modality::Vision => {
                    avg_input_tokens_per_step.saturating_mul(tool_cost_multiplier)
                }
            };
        }
    }

    /// Compute topology-aware total cost via DFS from node 0 (Step 10.2).
    ///
    /// Accumulates `base_cost` across all nodes reachable from the start node.
    /// For linear graphs (default), this is equivalent to `sum(node.base_cost)`.
    ///
    /// Cycle guard: nodes already visited are skipped — cycles should not occur
    /// after `GraphValidator::validate()` but are handled defensively.
    ///
    /// Returns `0` for empty graphs.
    pub fn total_cost(&self) -> usize {
        if self.nodes.is_empty() {
            return 0;
        }

        // Build adjacency list: node_id → Vec<neighbor_node_id>.
        let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
        for node in &self.nodes {
            adj.entry(node.id.0).or_default();
        }
        for edge in &self.edges {
            adj.entry(edge.from.0).or_default().push(edge.to.0);
        }

        // Cost lookup: node_id → base_cost.
        let cost_map: HashMap<usize, usize> = self.nodes.iter()
            .map(|n| (n.id.0, n.base_cost))
            .collect();

        // DFS from start node (node 0) with visited set.
        let start = self.nodes[0].id.0;
        let mut visited: HashSet<usize> = HashSet::new();
        let mut stack = vec![start];
        let mut total = 0usize;

        while let Some(node_id) = stack.pop() {
            if !visited.insert(node_id) {
                continue; // Cycle guard — already visited (should not occur post-validation).
            }
            total = total.saturating_add(*cost_map.get(&node_id).unwrap_or(&0));
            if let Some(neighbors) = adj.get(&node_id) {
                for &next in neighbors {
                    if !visited.contains(&next) {
                        stack.push(next);
                    }
                }
            }
        }

        total
    }
}
