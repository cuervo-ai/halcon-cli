//! DAG-based task orchestration over RuntimeAgents.

pub mod budget;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use uuid::Uuid;

use crate::agent::{
    AgentBudget, AgentCapability, AgentKind, AgentRequest, AgentResponse, AgentUsage,
};
use crate::error::{Result, RuntimeError};
use crate::federation::router::MessageRouter;
use crate::registry::AgentRegistry;

/// How to select an agent for a task.
#[derive(Debug, Clone)]
pub enum AgentSelector {
    /// Specific agent by ID.
    ById(Uuid),
    /// Best-match agent by required capabilities.
    ByCapability(Vec<AgentCapability>),
    /// First available agent of a given kind.
    ByKind(AgentKind),
    /// Agent lookup by name.
    ByName(String),
}

/// A single task node in the execution DAG.
#[derive(Debug, Clone)]
pub struct TaskNode {
    pub task_id: Uuid,
    pub instruction: String,
    pub agent_selector: AgentSelector,
    pub depends_on: Vec<Uuid>,
    pub budget: Option<AgentBudget>,
    /// Keys to inject from shared context into the request.
    pub context_keys: Vec<String>,
}

/// A directed acyclic graph of tasks to execute.
#[derive(Debug, Default)]
pub struct TaskDAG {
    nodes: Vec<TaskNode>,
}

impl TaskDAG {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, node: TaskNode) {
        self.nodes.push(node);
    }

    pub fn nodes(&self) -> &[TaskNode] {
        &self.nodes
    }

    /// Topologically sort nodes into parallel waves.
    ///
    /// Within each wave, all tasks can run concurrently.
    pub fn waves(&self) -> Result<Vec<Vec<&TaskNode>>> {
        self.validate()?;

        let node_map: HashMap<Uuid, &TaskNode> =
            self.nodes.iter().map(|n| (n.task_id, n)).collect();

        // Compute in-degrees
        let mut in_degree: HashMap<Uuid, usize> = HashMap::new();
        for node in &self.nodes {
            in_degree.entry(node.task_id).or_insert(0);
            for dep in &node.depends_on {
                in_degree.entry(*dep).or_insert(0);
            }
        }
        for node in &self.nodes {
            for _dep in &node.depends_on {
                *in_degree.entry(node.task_id).or_insert(0) += 1;
            }
        }

        let mut waves = Vec::new();
        let mut completed: HashSet<Uuid> = HashSet::new();
        let mut queue: VecDeque<Uuid> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .filter(|(id, _)| node_map.contains_key(id))
            .map(|(id, _)| *id)
            .collect();

        while !queue.is_empty() {
            let wave: Vec<Uuid> = queue.drain(..).collect();
            let wave_nodes: Vec<&TaskNode> = wave
                .iter()
                .filter_map(|id| node_map.get(id).copied())
                .collect();

            completed.extend(&wave);

            // Find next ready nodes
            for node in &self.nodes {
                if !completed.contains(&node.task_id)
                    && node.depends_on.iter().all(|d| completed.contains(d))
                    && !queue.contains(&node.task_id)
                {
                    queue.push_back(node.task_id);
                }
            }

            if !wave_nodes.is_empty() {
                waves.push(wave_nodes);
            }
        }

        Ok(waves)
    }

    /// Validate the DAG: check for cycles and missing dependencies.
    pub fn validate(&self) -> Result<()> {
        let node_ids: HashSet<Uuid> = self.nodes.iter().map(|n| n.task_id).collect();

        // Check for missing dependencies
        for node in &self.nodes {
            for dep in &node.depends_on {
                if !node_ids.contains(dep) {
                    return Err(RuntimeError::MissingDependency {
                        task_id: node.task_id.to_string(),
                        dep_id: dep.to_string(),
                    });
                }
            }
        }

        // Cycle detection via Kahn's algorithm
        let mut in_degree: HashMap<Uuid, usize> = node_ids.iter().map(|id| (*id, 0)).collect();
        for node in &self.nodes {
            for _dep in &node.depends_on {
                *in_degree.entry(node.task_id).or_default() += 1;
            }
        }

        let mut queue: VecDeque<Uuid> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(id, _)| *id)
            .collect();
        let mut visited = 0;

        while let Some(id) = queue.pop_front() {
            visited += 1;
            for node in &self.nodes {
                if node.depends_on.contains(&id) {
                    let deg = in_degree.get_mut(&node.task_id).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(node.task_id);
                    }
                }
            }
        }

        if visited != node_ids.len() {
            return Err(RuntimeError::CycleDetected);
        }

        Ok(())
    }
}

/// Result of executing a task DAG.
#[derive(Debug)]
pub struct ExecutionResult {
    pub results: Vec<(Uuid, Result<AgentResponse>)>,
    pub total_usage: AgentUsage,
    pub wave_count: usize,
}

/// Executes task DAGs over registered agents.
pub struct RuntimeExecutor {
    registry: Arc<AgentRegistry>,
    #[allow(dead_code)]
    router: Arc<MessageRouter>,
}

impl RuntimeExecutor {
    pub fn new(registry: Arc<AgentRegistry>, router: Arc<MessageRouter>) -> Self {
        Self { registry, router }
    }

    /// Execute a task DAG with shared context.
    pub async fn execute(
        &self,
        dag: TaskDAG,
        shared_context: &SharedContext,
    ) -> Result<ExecutionResult> {
        let waves = dag.waves()?;
        let wave_count = waves.len();
        let mut all_results = Vec::new();
        let mut total_usage = AgentUsage::default();
        let start = Instant::now();

        for wave in waves {
            let mut handles = Vec::new();

            for node in wave {
                let registry = self.registry.clone();
                let instruction = node.instruction.clone();
                let selector = node.agent_selector.clone();
                let budget = node.budget.clone();
                let task_id = node.task_id;
                let context_keys = node.context_keys.clone();
                let ctx_snapshot = shared_context.snapshot(&context_keys);

                handles.push(tokio::spawn(async move {
                    // Resolve agent
                    let agent = match &selector {
                        AgentSelector::ById(id) => registry.get(id).await,
                        AgentSelector::ByCapability(caps) => registry.best_match(caps).await,
                        AgentSelector::ByKind(kind) => {
                            let agents = registry.find_by_kind(*kind).await;
                            agents.into_iter().next()
                        }
                        AgentSelector::ByName(name) => registry.find_by_name(name).await,
                    };

                    let agent = match agent {
                        Some(a) => a,
                        None => {
                            return (
                                task_id,
                                Err(RuntimeError::AgentNotFound {
                                    id: format!("{selector:?}"),
                                }),
                            );
                        }
                    };

                    let mut request = AgentRequest::new(instruction);
                    request.context = ctx_snapshot;
                    if let Some(b) = budget {
                        request.budget = Some(b);
                    }

                    let result = agent.invoke(request).await;
                    (task_id, result)
                }));
            }

            for handle in handles {
                let (task_id, result) = handle.await.unwrap_or_else(|e| {
                    (
                        Uuid::nil(),
                        Err(RuntimeError::Execution(format!("task panicked: {e}"))),
                    )
                });

                // Update shared context with results
                if let Ok(ref resp) = result {
                    shared_context.set(
                        format!("result:{task_id}"),
                        serde_json::json!({
                            "success": resp.success,
                            "output": resp.output,
                        }),
                    );
                    total_usage.merge(&resp.usage);
                }

                all_results.push((task_id, result));
            }
        }

        total_usage.latency_ms = start.elapsed().as_millis() as u64;

        Ok(ExecutionResult {
            results: all_results,
            total_usage,
            wave_count,
        })
    }
}

/// Shared context store for inter-wave data passing.
#[derive(Debug, Default)]
pub struct SharedContext {
    store: std::sync::RwLock<HashMap<String, serde_json::Value>>,
}

impl SharedContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, key: String, value: serde_json::Value) {
        let mut store = self.store.write().unwrap_or_else(|e| e.into_inner());
        store.insert(key, value);
    }

    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        let store = self.store.read().unwrap_or_else(|e| e.into_inner());
        store.get(key).cloned()
    }

    /// Get a snapshot of the context for the given keys.
    pub fn snapshot(&self, keys: &[String]) -> HashMap<String, serde_json::Value> {
        let store = self.store.read().unwrap_or_else(|e| e.into_inner());
        keys.iter()
            .filter_map(|k| store.get(k).map(|v| (k.clone(), v.clone())))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentDescriptor, AgentHealth, ProtocolSupport, RuntimeAgent};

    fn tid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    // --- TaskDAG tests ---

    #[test]
    fn dag_empty() {
        let dag = TaskDAG::new();
        assert!(dag.nodes().is_empty());
        let waves = dag.waves().unwrap();
        assert!(waves.is_empty());
    }

    #[test]
    fn dag_single_node() {
        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "hello".to_string(),
            agent_selector: AgentSelector::ByName("test".to_string()),
            depends_on: vec![],
            budget: None,
            context_keys: vec![],
        });
        let waves = dag.waves().unwrap();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].len(), 1);
    }

    #[test]
    fn dag_parallel_nodes() {
        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "a".to_string(),
            agent_selector: AgentSelector::ByName("x".to_string()),
            depends_on: vec![],
            budget: None,
            context_keys: vec![],
        });
        dag.add_node(TaskNode {
            task_id: tid(2),
            instruction: "b".to_string(),
            agent_selector: AgentSelector::ByName("y".to_string()),
            depends_on: vec![],
            budget: None,
            context_keys: vec![],
        });
        let waves = dag.waves().unwrap();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].len(), 2);
    }

    #[test]
    fn dag_sequential_nodes() {
        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "first".to_string(),
            agent_selector: AgentSelector::ByName("x".to_string()),
            depends_on: vec![],
            budget: None,
            context_keys: vec![],
        });
        dag.add_node(TaskNode {
            task_id: tid(2),
            instruction: "second".to_string(),
            agent_selector: AgentSelector::ByName("x".to_string()),
            depends_on: vec![tid(1)],
            budget: None,
            context_keys: vec![],
        });
        let waves = dag.waves().unwrap();
        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0][0].task_id, tid(1));
        assert_eq!(waves[1][0].task_id, tid(2));
    }

    #[test]
    fn dag_diamond() {
        let mut dag = TaskDAG::new();
        // A → B, A → C, B+C → D
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "A".to_string(),
            agent_selector: AgentSelector::ByName("x".to_string()),
            depends_on: vec![],
            budget: None,
            context_keys: vec![],
        });
        dag.add_node(TaskNode {
            task_id: tid(2),
            instruction: "B".to_string(),
            agent_selector: AgentSelector::ByName("x".to_string()),
            depends_on: vec![tid(1)],
            budget: None,
            context_keys: vec![],
        });
        dag.add_node(TaskNode {
            task_id: tid(3),
            instruction: "C".to_string(),
            agent_selector: AgentSelector::ByName("x".to_string()),
            depends_on: vec![tid(1)],
            budget: None,
            context_keys: vec![],
        });
        dag.add_node(TaskNode {
            task_id: tid(4),
            instruction: "D".to_string(),
            agent_selector: AgentSelector::ByName("x".to_string()),
            depends_on: vec![tid(2), tid(3)],
            budget: None,
            context_keys: vec![],
        });

        let waves = dag.waves().unwrap();
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0].len(), 1); // A
        assert_eq!(waves[1].len(), 2); // B, C
        assert_eq!(waves[2].len(), 1); // D
    }

    #[test]
    fn dag_cycle_detected() {
        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "a".to_string(),
            agent_selector: AgentSelector::ByName("x".to_string()),
            depends_on: vec![tid(2)],
            budget: None,
            context_keys: vec![],
        });
        dag.add_node(TaskNode {
            task_id: tid(2),
            instruction: "b".to_string(),
            agent_selector: AgentSelector::ByName("x".to_string()),
            depends_on: vec![tid(1)],
            budget: None,
            context_keys: vec![],
        });
        let result = dag.validate();
        assert!(matches!(result, Err(RuntimeError::CycleDetected)));
    }

    #[test]
    fn dag_self_cycle() {
        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "loop".to_string(),
            agent_selector: AgentSelector::ByName("x".to_string()),
            depends_on: vec![tid(1)],
            budget: None,
            context_keys: vec![],
        });
        let result = dag.validate();
        assert!(matches!(result, Err(RuntimeError::CycleDetected)));
    }

    #[test]
    fn dag_missing_dependency() {
        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "orphan".to_string(),
            agent_selector: AgentSelector::ByName("x".to_string()),
            depends_on: vec![tid(999)],
            budget: None,
            context_keys: vec![],
        });
        let result = dag.validate();
        assert!(matches!(
            result,
            Err(RuntimeError::MissingDependency { .. })
        ));
    }

    // --- SharedContext tests ---

    #[test]
    fn shared_context_set_get() {
        let ctx = SharedContext::new();
        ctx.set("key".to_string(), serde_json::json!("value"));
        assert_eq!(ctx.get("key"), Some(serde_json::json!("value")));
        assert_eq!(ctx.get("missing"), None);
    }

    #[test]
    fn shared_context_snapshot() {
        let ctx = SharedContext::new();
        ctx.set("a".to_string(), serde_json::json!(1));
        ctx.set("b".to_string(), serde_json::json!(2));
        ctx.set("c".to_string(), serde_json::json!(3));

        let snap = ctx.snapshot(&["a".to_string(), "c".to_string(), "missing".to_string()]);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap["a"], serde_json::json!(1));
        assert_eq!(snap["c"], serde_json::json!(3));
    }

    // --- Executor integration tests ---

    struct TestAgent {
        descriptor: AgentDescriptor,
    }

    impl TestAgent {
        fn new(name: &str) -> Self {
            Self {
                descriptor: AgentDescriptor {
                    id: Uuid::new_v4(),
                    name: name.to_string(),
                    agent_kind: AgentKind::Llm,
                    capabilities: vec![AgentCapability::CodeGeneration],
                    protocols: vec![ProtocolSupport::Native],
                    metadata: HashMap::new(),
                    max_concurrency: 5,
                },
            }
        }
    }

    #[async_trait::async_trait]
    impl RuntimeAgent for TestAgent {
        fn descriptor(&self) -> &AgentDescriptor {
            &self.descriptor
        }

        async fn invoke(&self, request: AgentRequest) -> Result<AgentResponse> {
            Ok(AgentResponse {
                request_id: request.request_id,
                success: true,
                output: format!(
                    "{} processed: {}",
                    self.descriptor.name, request.instruction
                ),
                artifacts: vec![],
                usage: AgentUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cost_usd: 0.001,
                    latency_ms: 1,
                    rounds: 1,
                },
                metadata: HashMap::new(),
            })
        }

        async fn health(&self) -> AgentHealth {
            AgentHealth::Healthy
        }

        async fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn execute_single_wave() {
        let registry = Arc::new(AgentRegistry::new());
        let agent = Arc::new(TestAgent::new("worker"));
        let agent_name = agent.descriptor().name.clone();
        registry.register(agent).await;

        let router = Arc::new(MessageRouter::new(16));
        let executor = RuntimeExecutor::new(registry, router);

        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "hello".to_string(),
            agent_selector: AgentSelector::ByName(agent_name),
            depends_on: vec![],
            budget: None,
            context_keys: vec![],
        });

        let ctx = SharedContext::new();
        let result = executor.execute(dag, &ctx).await.unwrap();
        assert_eq!(result.wave_count, 1);
        assert_eq!(result.results.len(), 1);
        let (_, resp) = &result.results[0];
        assert!(resp.as_ref().unwrap().success);
    }

    #[tokio::test]
    async fn execute_multi_wave() {
        let registry = Arc::new(AgentRegistry::new());
        let agent = Arc::new(TestAgent::new("worker"));
        let name = agent.descriptor().name.clone();
        registry.register(agent).await;

        let router = Arc::new(MessageRouter::new(16));
        let executor = RuntimeExecutor::new(registry, router);

        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "first".to_string(),
            agent_selector: AgentSelector::ByName(name.clone()),
            depends_on: vec![],
            budget: None,
            context_keys: vec![],
        });
        dag.add_node(TaskNode {
            task_id: tid(2),
            instruction: "second".to_string(),
            agent_selector: AgentSelector::ByName(name),
            depends_on: vec![tid(1)],
            budget: None,
            context_keys: vec![],
        });

        let ctx = SharedContext::new();
        let result = executor.execute(dag, &ctx).await.unwrap();
        assert_eq!(result.wave_count, 2);
        assert_eq!(result.results.len(), 2);
    }

    #[tokio::test]
    async fn execute_context_injection() {
        let registry = Arc::new(AgentRegistry::new());
        let agent = Arc::new(TestAgent::new("worker"));
        let name = agent.descriptor().name.clone();
        registry.register(agent).await;

        let router = Arc::new(MessageRouter::new(16));
        let executor = RuntimeExecutor::new(registry, router);

        let ctx = SharedContext::new();
        ctx.set("input".to_string(), serde_json::json!("data"));

        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "use context".to_string(),
            agent_selector: AgentSelector::ByName(name),
            depends_on: vec![],
            budget: None,
            context_keys: vec!["input".to_string()],
        });

        let result = executor.execute(dag, &ctx).await.unwrap();
        assert!(result.results[0].1.is_ok());
    }

    #[tokio::test]
    async fn execute_agent_not_found() {
        let registry = Arc::new(AgentRegistry::new());
        let router = Arc::new(MessageRouter::new(16));
        let executor = RuntimeExecutor::new(registry, router);

        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "nobody home".to_string(),
            agent_selector: AgentSelector::ByName("nonexistent".to_string()),
            depends_on: vec![],
            budget: None,
            context_keys: vec![],
        });

        let ctx = SharedContext::new();
        let result = executor.execute(dag, &ctx).await.unwrap();
        assert!(result.results[0].1.is_err());
    }

    #[tokio::test]
    async fn execute_by_capability() {
        let registry = Arc::new(AgentRegistry::new());
        let agent = Arc::new(TestAgent::new("coder"));
        registry.register(agent).await;

        let router = Arc::new(MessageRouter::new(16));
        let executor = RuntimeExecutor::new(registry, router);

        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "generate code".to_string(),
            agent_selector: AgentSelector::ByCapability(vec![AgentCapability::CodeGeneration]),
            depends_on: vec![],
            budget: None,
            context_keys: vec![],
        });

        let ctx = SharedContext::new();
        let result = executor.execute(dag, &ctx).await.unwrap();
        assert!(result.results[0].1.is_ok());
    }

    #[tokio::test]
    async fn execute_concurrent_wave() {
        let registry = Arc::new(AgentRegistry::new());
        for i in 0..3 {
            let agent = Arc::new(TestAgent::new(&format!("agent-{i}")));
            registry.register(agent).await;
        }

        let router = Arc::new(MessageRouter::new(16));
        let executor = RuntimeExecutor::new(registry, router);

        let mut dag = TaskDAG::new();
        for i in 0..3u128 {
            dag.add_node(TaskNode {
                task_id: tid(i + 1),
                instruction: format!("task {i}"),
                agent_selector: AgentSelector::ByName(format!("agent-{i}")),
                depends_on: vec![],
                budget: None,
                context_keys: vec![],
            });
        }

        let ctx = SharedContext::new();
        let result = executor.execute(dag, &ctx).await.unwrap();
        assert_eq!(result.wave_count, 1);
        assert_eq!(result.results.len(), 3);
        assert!(result.results.iter().all(|(_, r)| r.is_ok()));
    }

    #[tokio::test]
    async fn execute_usage_accumulation() {
        let registry = Arc::new(AgentRegistry::new());
        let agent = Arc::new(TestAgent::new("worker"));
        let name = agent.descriptor().name.clone();
        registry.register(agent).await;

        let router = Arc::new(MessageRouter::new(16));
        let executor = RuntimeExecutor::new(registry, router);

        let mut dag = TaskDAG::new();
        for i in 0..3u128 {
            dag.add_node(TaskNode {
                task_id: tid(i + 1),
                instruction: format!("task {i}"),
                agent_selector: AgentSelector::ByName(name.clone()),
                depends_on: vec![],
                budget: None,
                context_keys: vec![],
            });
        }

        let ctx = SharedContext::new();
        let result = executor.execute(dag, &ctx).await.unwrap();
        assert_eq!(result.total_usage.input_tokens, 30); // 10 * 3
        assert_eq!(result.total_usage.output_tokens, 15); // 5 * 3
    }

    #[tokio::test]
    async fn execute_stores_results_in_context() {
        let registry = Arc::new(AgentRegistry::new());
        let agent = Arc::new(TestAgent::new("worker"));
        let name = agent.descriptor().name.clone();
        registry.register(agent).await;

        let router = Arc::new(MessageRouter::new(16));
        let executor = RuntimeExecutor::new(registry, router);

        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: tid(1),
            instruction: "hello".to_string(),
            agent_selector: AgentSelector::ByName(name),
            depends_on: vec![],
            budget: None,
            context_keys: vec![],
        });

        let ctx = SharedContext::new();
        executor.execute(dag, &ctx).await.unwrap();
        let result_key = format!("result:{}", tid(1));
        let stored = ctx.get(&result_key);
        assert!(stored.is_some());
    }
}
