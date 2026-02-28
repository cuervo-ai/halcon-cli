//! Goal Hierarchy — wraps the flat `ExecutionPlan` in a tree of `GoalNode`s.
//!
//! The flat `ExecutionPlan` (a `Vec<PlanStep>`) is sufficient for linear tasks.
//! Complex tasks benefit from a hierarchical goal model that captures:
//!
//! - **Root goal**: the user's top-level intent
//! - **Sub-goals**: logical groupings of plan steps (e.g., "Set up infrastructure" > "Run tests")
//! - **Leaf nodes**: individual `PlanStep` items
//!
//! # Design
//!
//! `GoalHierarchyBuilder` converts an `ExecutionPlan` into a `GoalTree`.
//! The conversion is non-destructive — the original plan is preserved.
//! Sub-agent delegation, dependency reasoning, and plan visualization can
//! operate on the `GoalTree` without touching the original flat plan.
//!
//! # Backward Compatibility
//!
//! All callers of `ExecutionPlan` remain unchanged.  `GoalTree` is opt-in —
//! only the planning layer and the delegation router use it.

use halcon_core::traits::{ExecutionPlan, PlanStep, StepOutcome};

// ──────────────────────────────────────────────────────────────────────────────
// Node types
// ──────────────────────────────────────────────────────────────────────────────

/// Unique identifier for a goal node (index into `GoalTree.nodes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GoalId(pub usize);

/// A node in the goal hierarchy.
#[derive(Debug, Clone)]
pub struct GoalNode {
    /// Unique identifier within the tree.
    pub id: GoalId,
    /// Human-readable label.
    pub label: String,
    /// Depth in the tree (root = 0).
    pub depth: usize,
    /// Children goal IDs.
    pub children: Vec<GoalId>,
    /// Parent goal ID (`None` for root).
    pub parent: Option<GoalId>,
    /// Leaf nodes carry a reference to the original plan step index.
    pub plan_step_index: Option<usize>,
    /// Aggregated status computed from children / plan step.
    pub status: NodeStatus,
}

/// Aggregated status for a goal node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    /// All children / step pending (no outcome yet).
    Pending,
    /// At least one child running, none failed.
    InProgress,
    /// All children / step completed successfully.
    Complete,
    /// At least one child / step failed.
    Failed,
    /// Not applicable (root before execution starts, or skipped).
    Unknown,
}

impl NodeStatus {
    /// Derive node status from a `PlanStep`'s `outcome` field.
    ///
    /// Steps with no outcome yet are `Pending`.
    pub fn from_step(step: &PlanStep) -> Self {
        match &step.outcome {
            None => NodeStatus::Pending,
            Some(StepOutcome::Success { .. }) => NodeStatus::Complete,
            Some(StepOutcome::Failed { .. }) => NodeStatus::Failed,
            Some(StepOutcome::Skipped { .. }) => NodeStatus::Unknown,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Goal tree
// ──────────────────────────────────────────────────────────────────────────────

/// A hierarchical tree of goal nodes wrapping a flat `ExecutionPlan`.
#[derive(Debug)]
pub struct GoalTree {
    /// All nodes; index == `GoalId.0`.
    pub nodes: Vec<GoalNode>,
    /// Root node ID (always `GoalId(0)` by convention).
    pub root: GoalId,
    /// The original flat plan this tree was built from.
    pub source_plan: ExecutionPlan,
}

impl GoalTree {
    /// Return the root node.
    pub fn root_node(&self) -> &GoalNode {
        &self.nodes[self.root.0]
    }

    /// Return a node by ID, or `None` if out of range.
    pub fn get(&self, id: GoalId) -> Option<&GoalNode> {
        self.nodes.get(id.0)
    }

    /// Leaf nodes (nodes that carry a `plan_step_index`).
    pub fn leaves(&self) -> impl Iterator<Item = &GoalNode> {
        self.nodes.iter().filter(|n| n.plan_step_index.is_some())
    }

    /// All nodes at a given depth (0 = root).
    pub fn at_depth(&self, depth: usize) -> impl Iterator<Item = &GoalNode> {
        self.nodes.iter().filter(move |n| n.depth == depth)
    }

    /// Aggregate status of the root node (reflects overall plan health).
    pub fn aggregate_status(&self) -> NodeStatus {
        self.aggregate(self.root)
    }

    fn aggregate(&self, id: GoalId) -> NodeStatus {
        let node = &self.nodes[id.0];
        if node.children.is_empty() {
            return node.status;
        }
        let child_statuses: Vec<NodeStatus> =
            node.children.iter().map(|&c| self.aggregate(c)).collect();

        if child_statuses.contains(&NodeStatus::Failed) {
            NodeStatus::Failed
        } else if child_statuses.iter().all(|s| *s == NodeStatus::Complete) {
            NodeStatus::Complete
        } else if child_statuses
            .iter()
            .any(|s| matches!(s, NodeStatus::InProgress | NodeStatus::Complete))
        {
            NodeStatus::InProgress
        } else {
            NodeStatus::Pending
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Builder
// ──────────────────────────────────────────────────────────────────────────────

/// Converts a flat `ExecutionPlan` into a `GoalTree`.
///
/// # Grouping heuristic
///
/// Steps are grouped by a simple prefix rule:
/// - If two consecutive steps share the same first word (verb) they are placed
///   under the same sub-goal group node.
/// - Otherwise each step gets a direct child of root.
///
/// This is intentionally lightweight — real sub-goal decomposition requires
/// an LLM and is deferred to the planning module.
pub struct GoalHierarchyBuilder;

impl GoalHierarchyBuilder {
    /// Build a `GoalTree` from the given plan.
    ///
    /// Returns `None` if the plan has no steps.
    pub fn build(plan: &ExecutionPlan, root_label: impl Into<String>) -> Option<GoalTree> {
        if plan.steps.is_empty() {
            return None;
        }

        let mut nodes: Vec<GoalNode> = Vec::new();
        let root_label = root_label.into();

        // Root node (placeholder children filled below).
        nodes.push(GoalNode {
            id: GoalId(0),
            label: root_label,
            depth: 0,
            children: vec![],
            parent: None,
            plan_step_index: None,
            status: NodeStatus::Unknown,
        });

        // Build group nodes + leaf nodes.
        let mut root_children: Vec<GoalId> = vec![];
        let mut group_map: std::collections::HashMap<String, GoalId> =
            std::collections::HashMap::new();

        for (i, step) in plan.steps.iter().enumerate() {
            let verb = first_word(&step.description);
            let group_id = group_map.entry(verb.clone()).or_insert_with(|| {
                let gid = GoalId(nodes.len());
                nodes.push(GoalNode {
                    id: gid,
                    label: format!("{} tasks", capitalise(&verb)),
                    depth: 1,
                    children: vec![],
                    parent: Some(GoalId(0)),
                    plan_step_index: None,
                    status: NodeStatus::Unknown,
                });
                root_children.push(gid);
                gid
            });
            let group_id = *group_id;

            // Leaf node — status derived from outcome.
            let leaf_id = GoalId(nodes.len());
            let status = NodeStatus::from_step(step);
            nodes.push(GoalNode {
                id: leaf_id,
                label: step.description.clone(),
                depth: 2,
                children: vec![],
                parent: Some(group_id),
                plan_step_index: Some(i),
                status,
            });
            nodes[group_id.0].children.push(leaf_id);
        }

        // Populate root children.
        nodes[0].children = root_children;

        Some(GoalTree {
            nodes,
            root: GoalId(0),
            source_plan: plan.clone(),
        })
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

fn first_word(s: &str) -> String {
    s.split_whitespace()
        .next()
        .unwrap_or("do")
        .to_lowercase()
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::traits::{ExecutionPlan, PlanStep};

    fn step(desc: &str) -> PlanStep {
        PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: desc.to_string(),
            tool_name: None,
            parallel: false,
            confidence: 0.9,
            expected_args: None,
            outcome: None,
        }
    }

    fn step_failed(desc: &str) -> PlanStep {
        PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: desc.to_string(),
            tool_name: None,
            parallel: false,
            confidence: 0.9,
            expected_args: None,
            outcome: Some(halcon_core::traits::StepOutcome::Failed {
                error: "test error".to_string(),
            }),
        }
    }

    fn step_done(desc: &str) -> PlanStep {
        PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: desc.to_string(),
            tool_name: None,
            parallel: false,
            confidence: 0.9,
            expected_args: None,
            outcome: Some(halcon_core::traits::StepOutcome::Success {
                summary: "ok".to_string(),
            }),
        }
    }

    fn plan(steps: Vec<PlanStep>) -> ExecutionPlan {
        ExecutionPlan {
            goal: "test goal".to_string(),
            steps,
            requires_confirmation: false,
            plan_id: uuid::Uuid::nil(),
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        }
    }

    #[test]
    fn empty_plan_returns_none() {
        let p = plan(vec![]);
        assert!(GoalHierarchyBuilder::build(&p, "Root").is_none());
    }

    #[test]
    fn single_step_creates_tree_with_one_leaf() {
        let p = plan(vec![step("read config.toml")]);
        let tree = GoalHierarchyBuilder::build(&p, "Root").unwrap();
        assert_eq!(tree.leaves().count(), 1);
        let leaf = tree.leaves().next().unwrap();
        assert_eq!(leaf.plan_step_index, Some(0));
        assert_eq!(leaf.depth, 2);
    }

    #[test]
    fn root_node_label_is_set() {
        let p = plan(vec![step("fix bug")]);
        let tree = GoalHierarchyBuilder::build(&p, "Fix the issue").unwrap();
        assert_eq!(tree.root_node().label, "Fix the issue");
    }

    #[test]
    fn steps_with_same_verb_grouped_under_same_parent() {
        let p = plan(vec![
            step("read auth.rs"),
            step("read config.rs"),
        ]);
        let tree = GoalHierarchyBuilder::build(&p, "Root").unwrap();
        // Both leaves share parent = same group node.
        let leaves: Vec<_> = tree.leaves().collect();
        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[0].parent, leaves[1].parent);
    }

    #[test]
    fn steps_with_different_verbs_get_different_group_nodes() {
        let p = plan(vec![
            step("read auth.rs"),
            step("write output.rs"),
        ]);
        let tree = GoalHierarchyBuilder::build(&p, "Root").unwrap();
        let leaves: Vec<_> = tree.leaves().collect();
        assert_eq!(leaves.len(), 2);
        // Different verbs → different parent groups.
        assert_ne!(leaves[0].parent, leaves[1].parent);
        // Root has 2 group children.
        assert_eq!(tree.root_node().children.len(), 2);
    }

    #[test]
    fn aggregate_status_all_pending() {
        let p = plan(vec![step("read a"), step("write b")]);
        let tree = GoalHierarchyBuilder::build(&p, "Root").unwrap();
        assert_eq!(tree.aggregate_status(), NodeStatus::Pending);
    }

    #[test]
    fn aggregate_status_with_one_failed_is_failed() {
        let p = plan(vec![step_failed("read a"), step("write b")]);
        let tree = GoalHierarchyBuilder::build(&p, "Root").unwrap();
        assert_eq!(tree.aggregate_status(), NodeStatus::Failed);
    }

    #[test]
    fn aggregate_status_all_complete() {
        let p = plan(vec![step_done("read x")]);
        let tree = GoalHierarchyBuilder::build(&p, "Root").unwrap();
        assert_eq!(tree.aggregate_status(), NodeStatus::Complete);
    }

    #[test]
    fn at_depth_returns_correct_nodes() {
        let p = plan(vec![step("run tests"), step("run linter")]);
        let tree = GoalHierarchyBuilder::build(&p, "Root").unwrap();
        let depth0: Vec<_> = tree.at_depth(0).collect();
        let depth1: Vec<_> = tree.at_depth(1).collect();
        let depth2: Vec<_> = tree.at_depth(2).collect();
        assert_eq!(depth0.len(), 1); // root
        assert_eq!(depth1.len(), 1); // one group ("run" verb)
        assert_eq!(depth2.len(), 2); // two leaves
    }

    #[test]
    fn source_plan_preserved_in_tree() {
        let p = plan(vec![step("fix bug"), step("write test")]);
        let tree = GoalHierarchyBuilder::build(&p, "Root").unwrap();
        assert_eq!(tree.source_plan.steps.len(), 2);
        assert_eq!(tree.source_plan.steps[0].description, "fix bug");
    }

    #[test]
    fn get_returns_none_for_out_of_range_id() {
        let p = plan(vec![step("read a")]);
        let tree = GoalHierarchyBuilder::build(&p, "Root").unwrap();
        assert!(tree.get(GoalId(9999)).is_none());
    }
}
