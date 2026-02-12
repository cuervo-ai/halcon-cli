//! CuervoRuntime: unified facade for the multi-agent orchestration runtime.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::agent::{
    AgentBudget, AgentCapability, AgentDescriptor, AgentHealth, AgentRequest, AgentResponse,
    RuntimeAgent,
};
use crate::error::Result;
use crate::executor::{ExecutionResult, RuntimeExecutor, SharedContext, TaskDAG};
use crate::federation::router::MessageRouter;
use crate::plugin::loader::PluginLoader;
use crate::registry::AgentRegistry;

/// Configuration for the runtime.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub plugin_paths: Vec<PathBuf>,
    pub max_concurrent_agents: usize,
    pub health_check_interval: Duration,
    pub default_budget: AgentBudget,
    pub broadcast_capacity: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            plugin_paths: vec![],
            max_concurrent_agents: 10,
            health_check_interval: Duration::from_secs(60),
            default_budget: AgentBudget::default(),
            broadcast_capacity: 256,
        }
    }
}

/// The unified multi-agent orchestration runtime.
pub struct CuervoRuntime {
    registry: Arc<AgentRegistry>,
    router: Arc<MessageRouter>,
    executor: RuntimeExecutor,
    plugin_loader: PluginLoader,
    config: RuntimeConfig,
}

impl CuervoRuntime {
    pub fn new(config: RuntimeConfig) -> Self {
        let registry = Arc::new(AgentRegistry::new());
        let router = Arc::new(MessageRouter::new(config.broadcast_capacity));
        let executor = RuntimeExecutor::new(registry.clone(), router.clone());
        let plugin_loader = PluginLoader::new(config.plugin_paths.clone());

        Self {
            registry,
            router,
            executor,
            plugin_loader,
            config,
        }
    }

    /// Start the runtime: discover and load plugins.
    pub async fn start(&self) -> Result<()> {
        let plugins = self.plugin_loader.load_all();
        for (manifest, result) in plugins {
            match result {
                Ok(agent) => {
                    let id = self.registry.register(agent).await;
                    tracing::info!(
                        plugin = %manifest.name,
                        id = %id,
                        "registered plugin agent"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        plugin = %manifest.name,
                        error = %e,
                        "failed to load plugin"
                    );
                }
            }
        }
        Ok(())
    }

    /// Register an agent with the runtime.
    pub async fn register_agent(&self, agent: Arc<dyn RuntimeAgent>) -> Uuid {
        let id = self.registry.register(agent).await;
        // Create a federation mailbox
        self.router.create_mailbox(id).await;
        id
    }

    /// Deregister an agent from the runtime.
    pub async fn deregister_agent(&self, id: &Uuid) {
        self.registry.deregister(id).await;
        self.router.remove_mailbox(id).await;
    }

    /// Execute a task DAG.
    pub async fn execute_dag(&self, dag: TaskDAG) -> Result<ExecutionResult> {
        let ctx = SharedContext::new();
        self.executor.execute(dag, &ctx).await
    }

    /// Execute a DAG with a pre-populated shared context.
    pub async fn execute_dag_with_context(
        &self,
        dag: TaskDAG,
        context: &SharedContext,
    ) -> Result<ExecutionResult> {
        self.executor.execute(dag, context).await
    }

    /// Invoke a specific agent by ID.
    pub async fn invoke_agent(
        &self,
        id: &Uuid,
        request: AgentRequest,
    ) -> Result<AgentResponse> {
        self.registry.invoke(id, request).await
    }

    /// Find agents by capability.
    pub async fn find_agents(&self, cap: &AgentCapability) -> Vec<AgentDescriptor> {
        let agents = self.registry.find_by_capability(cap).await;
        agents.iter().map(|a| a.descriptor().clone()).collect()
    }

    /// Get all registered agent descriptors.
    pub async fn all_agents(&self) -> Vec<AgentDescriptor> {
        self.registry.all_descriptors().await
    }

    /// Run health checks on all agents.
    pub async fn health_report(&self) -> HashMap<Uuid, AgentHealth> {
        self.registry.health_check_all().await
    }

    /// Number of registered agents.
    pub async fn agent_count(&self) -> usize {
        self.registry.count().await
    }

    /// Gracefully shut down the runtime.
    pub async fn shutdown(&self) -> Result<()> {
        let descriptors = self.registry.all_descriptors().await;
        for desc in &descriptors {
            if let Some(agent) = self.registry.get(&desc.id).await {
                if let Err(e) = agent.shutdown().await {
                    tracing::warn!(
                        agent = %desc.name,
                        error = %e,
                        "agent shutdown failed"
                    );
                }
            }
            self.registry.deregister(&desc.id).await;
        }
        Ok(())
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &AgentRegistry {
        &self.registry
    }

    /// Get a reference to the message router.
    pub fn router(&self) -> &MessageRouter {
        &self.router
    }

    /// Get the runtime config.
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentKind, AgentUsage, ProtocolSupport};

    struct TestAgent {
        descriptor: AgentDescriptor,
    }

    impl TestAgent {
        fn new(name: &str, caps: Vec<AgentCapability>) -> Self {
            Self {
                descriptor: AgentDescriptor {
                    id: Uuid::new_v4(),
                    name: name.to_string(),
                    agent_kind: AgentKind::Llm,
                    capabilities: caps,
                    protocols: vec![ProtocolSupport::Native],
                    metadata: HashMap::new(),
                    max_concurrency: 1,
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
                output: format!("{} done", self.descriptor.name),
                artifacts: vec![],
                usage: AgentUsage::default(),
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
    async fn runtime_construction() {
        let rt = CuervoRuntime::new(RuntimeConfig::default());
        assert_eq!(rt.agent_count().await, 0);
    }

    #[tokio::test]
    async fn runtime_start_no_plugins() {
        let rt = CuervoRuntime::new(RuntimeConfig::default());
        rt.start().await.unwrap();
        assert_eq!(rt.agent_count().await, 0);
    }

    #[tokio::test]
    async fn runtime_register_deregister() {
        let rt = CuervoRuntime::new(RuntimeConfig::default());
        let agent = Arc::new(TestAgent::new("test", vec![]));
        let id = rt.register_agent(agent).await;
        assert_eq!(rt.agent_count().await, 1);

        rt.deregister_agent(&id).await;
        assert_eq!(rt.agent_count().await, 0);
    }

    #[tokio::test]
    async fn runtime_invoke_agent() {
        let rt = CuervoRuntime::new(RuntimeConfig::default());
        let agent = Arc::new(TestAgent::new("worker", vec![]));
        let id = rt.register_agent(agent).await;

        let req = AgentRequest::new("hello");
        let resp = rt.invoke_agent(&id, req).await.unwrap();
        assert!(resp.success);
        assert!(resp.output.contains("worker done"));
    }

    #[tokio::test]
    async fn runtime_invoke_nonexistent() {
        let rt = CuervoRuntime::new(RuntimeConfig::default());
        let req = AgentRequest::new("hello");
        let result = rt.invoke_agent(&Uuid::new_v4(), req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn runtime_find_agents() {
        let rt = CuervoRuntime::new(RuntimeConfig::default());
        let a1 = Arc::new(TestAgent::new("coder", vec![AgentCapability::CodeGeneration]));
        let a2 = Arc::new(TestAgent::new("searcher", vec![AgentCapability::WebSearch]));
        rt.register_agent(a1).await;
        rt.register_agent(a2).await;

        let found = rt.find_agents(&AgentCapability::CodeGeneration).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "coder");
    }

    #[tokio::test]
    async fn runtime_all_agents() {
        let rt = CuervoRuntime::new(RuntimeConfig::default());
        rt.register_agent(Arc::new(TestAgent::new("a", vec![]))).await;
        rt.register_agent(Arc::new(TestAgent::new("b", vec![]))).await;

        let all = rt.all_agents().await;
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn runtime_health_report() {
        let rt = CuervoRuntime::new(RuntimeConfig::default());
        let agent = Arc::new(TestAgent::new("test", vec![]));
        let id = agent.descriptor().id;
        rt.register_agent(agent).await;

        let report = rt.health_report().await;
        assert_eq!(report.len(), 1);
        assert_eq!(report[&id], AgentHealth::Healthy);
    }

    #[tokio::test]
    async fn runtime_shutdown() {
        let rt = CuervoRuntime::new(RuntimeConfig::default());
        rt.register_agent(Arc::new(TestAgent::new("a", vec![]))).await;
        rt.register_agent(Arc::new(TestAgent::new("b", vec![]))).await;

        rt.shutdown().await.unwrap();
        assert_eq!(rt.agent_count().await, 0);
    }

    #[tokio::test]
    async fn runtime_config_defaults() {
        let config = RuntimeConfig::default();
        assert_eq!(config.max_concurrent_agents, 10);
        assert_eq!(config.health_check_interval, Duration::from_secs(60));
        assert!(config.plugin_paths.is_empty());
    }

    #[tokio::test]
    async fn runtime_execute_dag() {
        use crate::executor::{AgentSelector, TaskNode};

        let rt = CuervoRuntime::new(RuntimeConfig::default());
        let agent = Arc::new(TestAgent::new("worker", vec![AgentCapability::CodeGeneration]));
        let name = agent.descriptor().name.clone();
        rt.register_agent(agent).await;

        let mut dag = TaskDAG::new();
        dag.add_node(TaskNode {
            task_id: Uuid::from_u128(1),
            instruction: "do work".to_string(),
            agent_selector: AgentSelector::ByName(name),
            depends_on: vec![],
            budget: None,
            context_keys: vec![],
        });

        let result = rt.execute_dag(dag).await.unwrap();
        assert_eq!(result.wave_count, 1);
        assert!(result.results[0].1.is_ok());
    }
}
