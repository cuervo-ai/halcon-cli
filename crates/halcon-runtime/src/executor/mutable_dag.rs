//! Mutable DAG — runtime-modifiable task graph for V2 remote-control.
//!
//! Unlike `TaskDAG` (immutable after construction), `MutableDag` supports:
//! - Live node insertion/removal while execution is in progress
//! - Dependency mutation (add/remove edges)
//! - Version tracking (monotonic increment on every mutation)
//! - Mutation log for event sourcing / replay
//! - Status tracking per node (Pending → Ready → Running → Completed/Failed/Skipped)
//!
//! Thread safety: `RwLock` guards all mutations. Readers (executor wave loop) can
//! proceed concurrently; writers (API mutations) acquire exclusive access.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AgentSelector, TaskNode};
use crate::error::{Result, RuntimeError};

/// Status of a single node in the mutable DAG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    /// Not yet ready (dependencies incomplete).
    Pending,
    /// All dependencies satisfied; awaiting executor slot.
    Ready,
    /// Currently being executed by an agent.
    Running,
    /// Execution completed successfully.
    Completed,
    /// Execution failed.
    Failed { error: String, retryable: bool },
    /// Removed from DAG or dependency chain broken.
    Skipped,
    /// Parallel speculative branch — may be discarded.
    Speculative,
}

impl NodeStatus {
    /// Whether this node has reached a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            NodeStatus::Completed | NodeStatus::Failed { .. } | NodeStatus::Skipped
        )
    }

    /// Whether this node can be mutated (only non-running, non-terminal nodes).
    pub fn is_mutable(&self) -> bool {
        matches!(
            self,
            NodeStatus::Pending | NodeStatus::Ready | NodeStatus::Speculative
        )
    }
}

/// A node in the mutable DAG with status tracking.
#[derive(Debug, Clone)]
pub struct DagNode {
    pub task: TaskNode,
    pub status: NodeStatus,
    pub result: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub retry_count: u32,
}

/// Who authored a mutation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationAuthor {
    User { user_id: String },
    Cenzontle,
    AutoReplan,
    System,
}

/// A single mutation to the DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagMutation {
    pub mutation_id: Uuid,
    pub kind: DagMutationKind,
    pub author: MutationAuthor,
    pub timestamp: DateTime<Utc>,
    pub dag_version_after: u64,
}

/// The type of DAG mutation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum DagMutationKind {
    /// Insert a new node. If `after` is specified, add dependency edge.
    InsertNode {
        node_id: Uuid,
        instruction: String,
        after: Option<Uuid>,
    },
    /// Remove a pending/ready node.
    RemoveNode { node_id: Uuid, cascade: bool },
    /// Update a node's instruction or properties.
    UpdateNode {
        node_id: Uuid,
        instruction: Option<String>,
    },
    /// Add a dependency edge.
    AddDependency { from: Uuid, to: Uuid },
    /// Remove a dependency edge.
    RemoveDependency { from: Uuid, to: Uuid },
}

/// Snapshot of the DAG for serialization / API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagSnapshot {
    pub version: u64,
    pub nodes: Vec<DagNodeInfo>,
}

/// Serializable node info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNodeInfo {
    pub id: Uuid,
    pub instruction: String,
    pub depends_on: Vec<Uuid>,
    pub status: NodeStatus,
    pub retry_count: u32,
}

/// A mutable, version-tracked DAG.
pub struct MutableDag {
    inner: RwLock<MutableDagInner>,
}

struct MutableDagInner {
    nodes: HashMap<Uuid, DagNode>,
    version: u64,
    mutation_log: Vec<DagMutation>,
}

impl MutableDag {
    /// Create an empty mutable DAG.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(MutableDagInner {
                nodes: HashMap::new(),
                version: 0,
                mutation_log: Vec::new(),
            }),
        }
    }

    /// Create from an existing immutable TaskDAG.
    pub fn from_task_dag(nodes: Vec<TaskNode>) -> Self {
        let dag_nodes: HashMap<Uuid, DagNode> = nodes
            .into_iter()
            .map(|task| {
                (
                    task.task_id,
                    DagNode {
                        task,
                        status: NodeStatus::Pending,
                        result: None,
                        started_at: None,
                        completed_at: None,
                        retry_count: 0,
                    },
                )
            })
            .collect();

        Self {
            inner: RwLock::new(MutableDagInner {
                nodes: dag_nodes,
                version: 1,
                mutation_log: Vec::new(),
            }),
        }
    }

    /// Current version number.
    pub fn version(&self) -> u64 {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).version
    }

    /// Get a snapshot of the DAG for API responses.
    pub fn snapshot(&self) -> DagSnapshot {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        DagSnapshot {
            version: inner.version,
            nodes: inner
                .nodes
                .values()
                .map(|n| DagNodeInfo {
                    id: n.task.task_id,
                    instruction: n.task.instruction.clone(),
                    depends_on: n.task.depends_on.clone(),
                    status: n.status.clone(),
                    retry_count: n.retry_count,
                })
                .collect(),
        }
    }

    /// Get the mutation log.
    pub fn mutation_log(&self) -> Vec<DagMutation> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .mutation_log
            .clone()
    }

    /// Get ready nodes (all dependencies completed, status is Pending).
    pub fn ready_nodes(&self) -> Vec<TaskNode> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let completed: HashSet<Uuid> = inner
            .nodes
            .iter()
            .filter(|(_, n)| n.status == NodeStatus::Completed)
            .map(|(id, _)| *id)
            .collect();

        inner
            .nodes
            .values()
            .filter(|n| {
                n.status == NodeStatus::Pending
                    && n.task.depends_on.iter().all(|dep| completed.contains(dep))
            })
            .map(|n| n.task.clone())
            .collect()
    }

    /// Mark a node as running.
    pub fn mark_running(&self, node_id: Uuid) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if let Some(node) = inner.nodes.get_mut(&node_id) {
            node.status = NodeStatus::Running;
            node.started_at = Some(Utc::now());
        }
    }

    /// Mark a node as completed.
    pub fn mark_completed(&self, node_id: Uuid, result: String) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if let Some(node) = inner.nodes.get_mut(&node_id) {
            node.status = NodeStatus::Completed;
            node.result = Some(result);
            node.completed_at = Some(Utc::now());
        }
    }

    /// Mark a node as failed.
    pub fn mark_failed(&self, node_id: Uuid, error: String, retryable: bool) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if let Some(node) = inner.nodes.get_mut(&node_id) {
            node.status = NodeStatus::Failed {
                error,
                retryable,
            };
            node.completed_at = Some(Utc::now());
        }
    }

    /// Check if all nodes are terminal (DAG execution complete).
    pub fn is_complete(&self) -> bool {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.nodes.values().all(|n| n.status.is_terminal())
    }

    /// Check if any node is currently running.
    pub fn has_running(&self) -> bool {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner
            .nodes
            .values()
            .any(|n| n.status == NodeStatus::Running)
    }

    // ── Mutation Operations ─────────────────────────────────────────────

    /// Insert a new node into the DAG.
    pub fn insert_node(
        &self,
        instruction: String,
        agent_selector: AgentSelector,
        after: Option<Uuid>,
        author: MutationAuthor,
    ) -> Result<Uuid> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let node_id = Uuid::new_v4();

        let depends_on = if let Some(after_id) = after {
            if !inner.nodes.contains_key(&after_id) {
                return Err(RuntimeError::MissingDependency {
                    task_id: node_id.to_string(),
                    dep_id: after_id.to_string(),
                });
            }
            vec![after_id]
        } else {
            vec![]
        };

        let task = TaskNode {
            task_id: node_id,
            instruction: instruction.clone(),
            agent_selector,
            depends_on,
            budget: None,
            context_keys: vec![],
        };

        inner.nodes.insert(
            node_id,
            DagNode {
                task,
                status: NodeStatus::Pending,
                result: None,
                started_at: None,
                completed_at: None,
                retry_count: 0,
            },
        );

        // Validate no cycles after insertion.
        if Self::has_cycle_inner(&inner.nodes) {
            inner.nodes.remove(&node_id);
            return Err(RuntimeError::CycleDetected);
        }

        inner.version += 1;
        let ver = inner.version;
        inner.mutation_log.push(DagMutation {
            mutation_id: Uuid::new_v4(),
            kind: DagMutationKind::InsertNode {
                node_id,
                instruction,
                after,
            },
            author,
            timestamp: Utc::now(),
            dag_version_after: ver,
        });

        Ok(node_id)
    }

    /// Remove a node from the DAG. Only mutable (Pending/Ready/Speculative) nodes.
    pub fn remove_node(
        &self,
        node_id: Uuid,
        cascade: bool,
        author: MutationAuthor,
    ) -> Result<()> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());

        let node = inner.nodes.get(&node_id).ok_or(RuntimeError::Execution(
            format!("Node {node_id} not found"),
        ))?;

        if !node.status.is_mutable() {
            return Err(RuntimeError::Execution(format!(
                "Node {node_id} is {:?} and cannot be removed",
                node.status
            )));
        }

        if cascade {
            // Find all transitive dependents and mark them Skipped.
            let dependents = Self::find_dependents(&inner.nodes, node_id);
            for dep_id in &dependents {
                if let Some(dep_node) = inner.nodes.get_mut(dep_id) {
                    if dep_node.status.is_mutable() {
                        dep_node.status = NodeStatus::Skipped;
                    }
                }
            }
        } else {
            // Check if any non-terminal node depends on this one.
            let has_active_dependents = inner.nodes.values().any(|n| {
                n.task.depends_on.contains(&node_id) && !n.status.is_terminal()
            });
            if has_active_dependents {
                return Err(RuntimeError::Execution(format!(
                    "Node {node_id} has active dependents; use cascade=true to remove"
                )));
            }
        }

        inner.nodes.get_mut(&node_id).unwrap().status = NodeStatus::Skipped;

        inner.version += 1;
        let ver = inner.version;
        inner.mutation_log.push(DagMutation {
            mutation_id: Uuid::new_v4(),
            kind: DagMutationKind::RemoveNode { node_id, cascade },
            author,
            timestamp: Utc::now(),
            dag_version_after: ver,
        });

        Ok(())
    }

    /// Update a node's instruction.
    pub fn update_node(
        &self,
        node_id: Uuid,
        instruction: Option<String>,
        author: MutationAuthor,
    ) -> Result<()> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());

        let node = inner
            .nodes
            .get_mut(&node_id)
            .ok_or(RuntimeError::Execution(format!(
                "Node {node_id} not found"
            )))?;

        if !node.status.is_mutable() {
            return Err(RuntimeError::Execution(format!(
                "Node {node_id} is {:?} and cannot be updated",
                node.status
            )));
        }

        if let Some(ref instr) = instruction {
            node.task.instruction = instr.clone();
        }

        inner.version += 1;
        let ver = inner.version;
        inner.mutation_log.push(DagMutation {
            mutation_id: Uuid::new_v4(),
            kind: DagMutationKind::UpdateNode {
                node_id,
                instruction,
            },
            author,
            timestamp: Utc::now(),
            dag_version_after: ver,
        });

        Ok(())
    }

    /// Add a dependency edge (from depends on to).
    pub fn add_dependency(
        &self,
        from: Uuid,
        to: Uuid,
        author: MutationAuthor,
    ) -> Result<()> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());

        if !inner.nodes.contains_key(&from) || !inner.nodes.contains_key(&to) {
            return Err(RuntimeError::MissingDependency {
                task_id: from.to_string(),
                dep_id: to.to_string(),
            });
        }

        let node = inner.nodes.get_mut(&from).unwrap();
        if !node.status.is_mutable() {
            return Err(RuntimeError::Execution(format!(
                "Node {from} is {:?} and cannot be modified",
                node.status
            )));
        }

        if !node.task.depends_on.contains(&to) {
            node.task.depends_on.push(to);
        }

        // Validate no cycles.
        if Self::has_cycle_inner(&inner.nodes) {
            // Rollback.
            let node = inner.nodes.get_mut(&from).unwrap();
            node.task.depends_on.retain(|d| *d != to);
            return Err(RuntimeError::CycleDetected);
        }

        inner.version += 1;
        let ver = inner.version;
        inner.mutation_log.push(DagMutation {
            mutation_id: Uuid::new_v4(),
            kind: DagMutationKind::AddDependency { from, to },
            author,
            timestamp: Utc::now(),
            dag_version_after: ver,
        });

        Ok(())
    }

    /// Remove a dependency edge.
    pub fn remove_dependency(
        &self,
        from: Uuid,
        to: Uuid,
        author: MutationAuthor,
    ) -> Result<()> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());

        let node = inner
            .nodes
            .get_mut(&from)
            .ok_or(RuntimeError::Execution(format!(
                "Node {from} not found"
            )))?;

        node.task.depends_on.retain(|d| *d != to);

        inner.version += 1;
        let ver = inner.version;
        inner.mutation_log.push(DagMutation {
            mutation_id: Uuid::new_v4(),
            kind: DagMutationKind::RemoveDependency { from, to },
            author,
            timestamp: Utc::now(),
            dag_version_after: ver,
        });

        Ok(())
    }

    // ── Internal helpers ────────────────────────────────────────────────

    /// Cycle detection on the inner node map (Kahn's algorithm).
    fn has_cycle_inner(nodes: &HashMap<Uuid, DagNode>) -> bool {
        let mut in_degree: HashMap<Uuid, usize> = nodes.keys().map(|id| (*id, 0)).collect();

        for node in nodes.values() {
            for _dep in &node.task.depends_on {
                *in_degree.entry(node.task.task_id).or_default() += 1;
            }
        }

        let mut queue: VecDeque<Uuid> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(id, _)| *id)
            .collect();

        let mut visited = 0usize;

        while let Some(id) = queue.pop_front() {
            visited += 1;
            for node in nodes.values() {
                if node.task.depends_on.contains(&id) {
                    let deg = in_degree.get_mut(&node.task.task_id).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(node.task.task_id);
                    }
                }
            }
        }

        visited != nodes.len()
    }

    /// Find all transitive dependents of a node.
    fn find_dependents(nodes: &HashMap<Uuid, DagNode>, root: Uuid) -> Vec<Uuid> {
        let mut result = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(root);

        while let Some(current) = queue.pop_front() {
            for node in nodes.values() {
                if node.task.depends_on.contains(&current)
                    && !result.contains(&node.task.task_id)
                {
                    result.push(node.task.task_id);
                    queue.push_back(node.task.task_id);
                }
            }
        }

        result
    }
}

impl Default for MutableDag {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_selector() -> AgentSelector {
        AgentSelector::ByName("test".to_string())
    }

    #[test]
    fn empty_dag() {
        let dag = MutableDag::new();
        assert_eq!(dag.version(), 0);
        assert!(dag.ready_nodes().is_empty());
        assert!(dag.is_complete());
    }

    #[test]
    fn insert_and_ready() {
        let dag = MutableDag::new();
        let id = dag
            .insert_node(
                "task 1".to_string(),
                make_selector(),
                None,
                MutationAuthor::System,
            )
            .unwrap();

        assert_eq!(dag.version(), 1);
        let ready = dag.ready_nodes();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].task_id, id);
    }

    #[test]
    fn insert_with_dependency() {
        let dag = MutableDag::new();
        let a = dag
            .insert_node(
                "A".to_string(),
                make_selector(),
                None,
                MutationAuthor::System,
            )
            .unwrap();
        let _b = dag
            .insert_node(
                "B".to_string(),
                make_selector(),
                Some(a),
                MutationAuthor::System,
            )
            .unwrap();

        // Only A is ready (B depends on A).
        let ready = dag.ready_nodes();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].task_id, a);

        // Complete A, then B becomes ready.
        dag.mark_completed(a, "done".to_string());
        let ready = dag.ready_nodes();
        assert_eq!(ready.len(), 1);
    }

    #[test]
    fn cycle_detection_on_insert() {
        let dag = MutableDag::new();
        let a = dag
            .insert_node("A".to_string(), make_selector(), None, MutationAuthor::System)
            .unwrap();
        let b = dag
            .insert_node("B".to_string(), make_selector(), Some(a), MutationAuthor::System)
            .unwrap();

        // Try to add dependency A → B (would create cycle A→B→A).
        let result = dag.add_dependency(a, b, MutationAuthor::System);
        assert!(matches!(result, Err(RuntimeError::CycleDetected)));
    }

    #[test]
    fn remove_node_no_cascade() {
        let dag = MutableDag::new();
        let a = dag
            .insert_node("A".to_string(), make_selector(), None, MutationAuthor::System)
            .unwrap();

        dag.remove_node(a, false, MutationAuthor::System).unwrap();
        assert!(dag.ready_nodes().is_empty());
    }

    #[test]
    fn remove_running_node_fails() {
        let dag = MutableDag::new();
        let a = dag
            .insert_node("A".to_string(), make_selector(), None, MutationAuthor::System)
            .unwrap();

        dag.mark_running(a);
        let result = dag.remove_node(a, false, MutationAuthor::System);
        assert!(result.is_err());
    }

    #[test]
    fn cascade_remove() {
        let dag = MutableDag::new();
        let a = dag
            .insert_node("A".to_string(), make_selector(), None, MutationAuthor::System)
            .unwrap();
        let _b = dag
            .insert_node("B".to_string(), make_selector(), Some(a), MutationAuthor::System)
            .unwrap();
        let _c = dag
            .insert_node("C".to_string(), make_selector(), Some(a), MutationAuthor::System)
            .unwrap();

        dag.remove_node(a, true, MutationAuthor::System).unwrap();

        // All nodes should be terminal (Skipped).
        assert!(dag.is_complete());
    }

    #[test]
    fn update_node() {
        let dag = MutableDag::new();
        let a = dag
            .insert_node("old".to_string(), make_selector(), None, MutationAuthor::System)
            .unwrap();

        dag.update_node(a, Some("new".to_string()), MutationAuthor::System)
            .unwrap();

        let ready = dag.ready_nodes();
        assert_eq!(ready[0].instruction, "new");
    }

    #[test]
    fn mutation_log_tracks_all() {
        let dag = MutableDag::new();
        dag.insert_node("A".to_string(), make_selector(), None, MutationAuthor::System)
            .unwrap();
        dag.insert_node("B".to_string(), make_selector(), None, MutationAuthor::System)
            .unwrap();

        let log = dag.mutation_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].dag_version_after, 1);
        assert_eq!(log[1].dag_version_after, 2);
    }

    #[test]
    fn version_increments() {
        let dag = MutableDag::new();
        assert_eq!(dag.version(), 0);

        dag.insert_node("A".to_string(), make_selector(), None, MutationAuthor::System)
            .unwrap();
        assert_eq!(dag.version(), 1);

        let a_id = dag.ready_nodes()[0].task_id;
        dag.update_node(a_id, Some("B".to_string()), MutationAuthor::System)
            .unwrap();
        assert_eq!(dag.version(), 2);
    }

    #[test]
    fn from_task_dag() {
        let nodes = vec![
            TaskNode {
                task_id: Uuid::from_u128(1),
                instruction: "first".to_string(),
                agent_selector: make_selector(),
                depends_on: vec![],
                budget: None,
                context_keys: vec![],
            },
            TaskNode {
                task_id: Uuid::from_u128(2),
                instruction: "second".to_string(),
                agent_selector: make_selector(),
                depends_on: vec![Uuid::from_u128(1)],
                budget: None,
                context_keys: vec![],
            },
        ];

        let dag = MutableDag::from_task_dag(nodes);
        assert_eq!(dag.version(), 1);

        let ready = dag.ready_nodes();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].task_id, Uuid::from_u128(1));
    }

    #[test]
    fn snapshot_serializable() {
        let dag = MutableDag::new();
        dag.insert_node("A".to_string(), make_selector(), None, MutationAuthor::System)
            .unwrap();

        let snap = dag.snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"version\":1"));
        assert!(json.contains("\"instruction\":\"A\""));
    }
}
