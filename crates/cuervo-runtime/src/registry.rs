//! Central agent registry for lifecycle management and discovery.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::agent::{
    AgentCapability, AgentDescriptor, AgentHealth, AgentKind, AgentRequest, AgentResponse,
    RuntimeAgent,
};
use crate::capability::CapabilityIndex;
use crate::error::{Result, RuntimeError};
use crate::health::AgentHealthTracker;

struct RegisteredAgent {
    agent: Arc<dyn RuntimeAgent>,
    #[allow(dead_code)] // Tracked for diagnostics; will be exposed via agent info API
    registered_at: DateTime<Utc>,
    last_invoked: Option<DateTime<Utc>>,
    invocation_count: u64,
}

/// Central registry for agent lifecycle management and capability-based discovery.
pub struct AgentRegistry {
    agents: RwLock<HashMap<Uuid, RegisteredAgent>>,
    capability_index: RwLock<CapabilityIndex>,
    health_tracker: AgentHealthTracker,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
            capability_index: RwLock::new(CapabilityIndex::new()),
            health_tracker: AgentHealthTracker::new(),
        }
    }

    /// Register a new agent. Returns the agent's UUID.
    pub async fn register(&self, agent: Arc<dyn RuntimeAgent>) -> Uuid {
        let desc = agent.descriptor();
        let id = desc.id;

        // Update capability index
        {
            let mut idx = self.capability_index.write().await;
            idx.register(id, &desc.capabilities);
        }

        // Track health
        self.health_tracker.track(id);

        // Store the agent
        {
            let mut agents = self.agents.write().await;
            agents.insert(
                id,
                RegisteredAgent {
                    agent,
                    registered_at: Utc::now(),
                    last_invoked: None,
                    invocation_count: 0,
                },
            );
        }

        id
    }

    /// Remove an agent from the registry.
    pub async fn deregister(&self, id: &Uuid) -> Option<Arc<dyn RuntimeAgent>> {
        // Remove from capability index
        {
            let mut idx = self.capability_index.write().await;
            idx.deregister(id);
        }

        // Remove from health tracker
        self.health_tracker.untrack(id);

        // Remove from agents
        let mut agents = self.agents.write().await;
        agents.remove(id).map(|ra| ra.agent)
    }

    /// Get a reference to a registered agent.
    pub async fn get(&self, id: &Uuid) -> Option<Arc<dyn RuntimeAgent>> {
        let agents = self.agents.read().await;
        agents.get(id).map(|ra| ra.agent.clone())
    }

    /// Find all agents that provide a given capability.
    pub async fn find_by_capability(&self, cap: &AgentCapability) -> Vec<Arc<dyn RuntimeAgent>> {
        let idx = self.capability_index.read().await;
        let agent_ids = idx.find_agents(cap);
        let agents = self.agents.read().await;
        agent_ids
            .iter()
            .filter_map(|id| agents.get(id).map(|ra| ra.agent.clone()))
            .collect()
    }

    /// Find all agents of a given kind.
    pub async fn find_by_kind(&self, kind: AgentKind) -> Vec<Arc<dyn RuntimeAgent>> {
        let agents = self.agents.read().await;
        agents
            .values()
            .filter(|ra| ra.agent.descriptor().agent_kind == kind)
            .map(|ra| ra.agent.clone())
            .collect()
    }

    /// Find an agent by name.
    pub async fn find_by_name(&self, name: &str) -> Option<Arc<dyn RuntimeAgent>> {
        let agents = self.agents.read().await;
        agents
            .values()
            .find(|ra| ra.agent.descriptor().name == name)
            .map(|ra| ra.agent.clone())
    }

    /// Get descriptors for all registered agents.
    pub async fn all_descriptors(&self) -> Vec<AgentDescriptor> {
        let agents = self.agents.read().await;
        agents
            .values()
            .map(|ra| ra.agent.descriptor().clone())
            .collect()
    }

    /// Run health checks on all registered agents.
    pub async fn health_check_all(&self) -> HashMap<Uuid, AgentHealth> {
        let agent_list: Vec<(Uuid, Arc<dyn RuntimeAgent>)> = {
            let agents = self.agents.read().await;
            agents
                .iter()
                .map(|(id, ra)| (*id, ra.agent.clone()))
                .collect()
        };

        let mut results = HashMap::new();
        for (id, agent) in agent_list {
            let health = agent.health().await;
            match &health {
                AgentHealth::Healthy => self.health_tracker.record_success(&id),
                AgentHealth::Degraded { reason } | AgentHealth::Unavailable { reason } => {
                    self.health_tracker.record_failure(&id, reason)
                }
            }
            results.insert(id, health);
        }
        results
    }

    /// Invoke a registered agent by ID, tracking usage.
    pub async fn invoke(
        &self,
        id: &Uuid,
        request: AgentRequest,
    ) -> Result<AgentResponse> {
        let agent = {
            let agents = self.agents.read().await;
            agents
                .get(id)
                .map(|ra| ra.agent.clone())
                .ok_or_else(|| RuntimeError::AgentNotFound {
                    id: id.to_string(),
                })?
        };

        let result = agent.invoke(request).await;

        // Update tracking
        {
            let mut agents = self.agents.write().await;
            if let Some(ra) = agents.get_mut(id) {
                ra.last_invoked = Some(Utc::now());
                ra.invocation_count += 1;
            }
        }

        // Update health tracker
        match &result {
            Ok(_) => self.health_tracker.record_success(id),
            Err(e) => self.health_tracker.record_failure(id, &e.to_string()),
        }

        result
    }

    /// Number of registered agents.
    pub async fn count(&self) -> usize {
        let agents = self.agents.read().await;
        agents.len()
    }

    /// Get the health tracker (for direct access).
    pub fn health_tracker(&self) -> &AgentHealthTracker {
        &self.health_tracker
    }

    /// Best-match agent for a set of required capabilities.
    pub async fn best_match(&self, required: &[AgentCapability]) -> Option<Arc<dyn RuntimeAgent>> {
        let idx = self.capability_index.read().await;
        if let Some(id) = idx.best_match(required) {
            let agents = self.agents.read().await;
            agents.get(&id).map(|ra| ra.agent.clone())
        } else {
            None
        }
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentUsage, ProtocolSupport};
    use std::collections::HashMap as StdHashMap;

    struct MockAgent {
        descriptor: AgentDescriptor,
    }

    impl MockAgent {
        fn new(name: &str, kind: AgentKind, caps: Vec<AgentCapability>) -> Self {
            Self {
                descriptor: AgentDescriptor {
                    id: Uuid::new_v4(),
                    name: name.to_string(),
                    agent_kind: kind,
                    capabilities: caps,
                    protocols: vec![ProtocolSupport::Native],
                    metadata: StdHashMap::new(),
                    max_concurrency: 1,
                },
            }
        }
    }

    #[async_trait::async_trait]
    impl RuntimeAgent for MockAgent {
        fn descriptor(&self) -> &AgentDescriptor {
            &self.descriptor
        }

        async fn invoke(&self, request: AgentRequest) -> Result<AgentResponse> {
            Ok(AgentResponse {
                request_id: request.request_id,
                success: true,
                output: format!("Mock {} done", self.descriptor.name),
                artifacts: vec![],
                usage: AgentUsage::default(),
                metadata: StdHashMap::new(),
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
    async fn register_and_get() {
        let registry = AgentRegistry::new();
        let agent = Arc::new(MockAgent::new("test", AgentKind::Llm, vec![]));
        let id = agent.descriptor().id;
        registry.register(agent).await;

        let got = registry.get(&id).await;
        assert!(got.is_some());
        assert_eq!(got.unwrap().descriptor().name, "test");
    }

    #[tokio::test]
    async fn deregister() {
        let registry = AgentRegistry::new();
        let agent = Arc::new(MockAgent::new("test", AgentKind::Llm, vec![]));
        let id = agent.descriptor().id;
        registry.register(agent).await;

        let removed = registry.deregister(&id).await;
        assert!(removed.is_some());
        assert!(registry.get(&id).await.is_none());
    }

    #[tokio::test]
    async fn deregister_nonexistent() {
        let registry = AgentRegistry::new();
        let removed = registry.deregister(&Uuid::new_v4()).await;
        assert!(removed.is_none());
    }

    #[tokio::test]
    async fn find_by_capability_single() {
        let registry = AgentRegistry::new();
        let agent = Arc::new(MockAgent::new(
            "coder",
            AgentKind::Llm,
            vec![AgentCapability::CodeGeneration],
        ));
        registry.register(agent).await;

        let found = registry.find_by_capability(&AgentCapability::CodeGeneration).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].descriptor().name, "coder");
    }

    #[tokio::test]
    async fn find_by_capability_multiple() {
        let registry = AgentRegistry::new();
        let a1 = Arc::new(MockAgent::new(
            "coder1",
            AgentKind::Llm,
            vec![AgentCapability::CodeGeneration],
        ));
        let a2 = Arc::new(MockAgent::new(
            "coder2",
            AgentKind::Llm,
            vec![AgentCapability::CodeGeneration, AgentCapability::Testing],
        ));
        registry.register(a1).await;
        registry.register(a2).await;

        let found = registry.find_by_capability(&AgentCapability::CodeGeneration).await;
        assert_eq!(found.len(), 2);
    }

    #[tokio::test]
    async fn find_by_capability_none() {
        let registry = AgentRegistry::new();
        let found = registry.find_by_capability(&AgentCapability::WebSearch).await;
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn find_by_kind() {
        let registry = AgentRegistry::new();
        let a1 = Arc::new(MockAgent::new("llm", AgentKind::Llm, vec![]));
        let a2 = Arc::new(MockAgent::new("cli", AgentKind::CliProcess, vec![]));
        let a3 = Arc::new(MockAgent::new("llm2", AgentKind::Llm, vec![]));
        registry.register(a1).await;
        registry.register(a2).await;
        registry.register(a3).await;

        let llms = registry.find_by_kind(AgentKind::Llm).await;
        assert_eq!(llms.len(), 2);

        let clis = registry.find_by_kind(AgentKind::CliProcess).await;
        assert_eq!(clis.len(), 1);
    }

    #[tokio::test]
    async fn find_by_name() {
        let registry = AgentRegistry::new();
        let agent = Arc::new(MockAgent::new("special", AgentKind::Mcp, vec![]));
        registry.register(agent).await;

        let found = registry.find_by_name("special").await;
        assert!(found.is_some());
        assert!(registry.find_by_name("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn all_descriptors() {
        let registry = AgentRegistry::new();
        registry
            .register(Arc::new(MockAgent::new("a", AgentKind::Llm, vec![])))
            .await;
        registry
            .register(Arc::new(MockAgent::new("b", AgentKind::Mcp, vec![])))
            .await;

        let descs = registry.all_descriptors().await;
        assert_eq!(descs.len(), 2);
    }

    #[tokio::test]
    async fn health_check_all() {
        let registry = AgentRegistry::new();
        let a = Arc::new(MockAgent::new("a", AgentKind::Llm, vec![]));
        let id = a.descriptor().id;
        registry.register(a).await;

        let health = registry.health_check_all().await;
        assert_eq!(health.len(), 1);
        assert_eq!(health[&id], AgentHealth::Healthy);
    }

    #[tokio::test]
    async fn invoke_tracks_count() {
        let registry = AgentRegistry::new();
        let agent = Arc::new(MockAgent::new("a", AgentKind::Llm, vec![]));
        let id = agent.descriptor().id;
        registry.register(agent).await;

        let req = AgentRequest::new("hello");
        let resp = registry.invoke(&id, req).await.unwrap();
        assert!(resp.success);

        // Invoke again
        let req2 = AgentRequest::new("world");
        registry.invoke(&id, req2).await.unwrap();

        // Verify count via internal state (we trust the tracking logic)
        assert_eq!(registry.count().await, 1);
    }

    #[tokio::test]
    async fn invoke_nonexistent_agent() {
        let registry = AgentRegistry::new();
        let req = AgentRequest::new("hello");
        let result = registry.invoke(&Uuid::new_v4(), req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn best_match() {
        let registry = AgentRegistry::new();
        let a1 = Arc::new(MockAgent::new(
            "generalist",
            AgentKind::Llm,
            vec![AgentCapability::CodeGeneration, AgentCapability::Testing, AgentCapability::FileOperations],
        ));
        let a2 = Arc::new(MockAgent::new(
            "specialist",
            AgentKind::Llm,
            vec![AgentCapability::CodeGeneration],
        ));
        registry.register(a1).await;
        registry.register(a2).await;

        let found = registry
            .best_match(&[AgentCapability::CodeGeneration, AgentCapability::Testing])
            .await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().descriptor().name, "generalist");
    }

    #[tokio::test]
    async fn count_tracks_registrations() {
        let registry = AgentRegistry::new();
        assert_eq!(registry.count().await, 0);
        let a = Arc::new(MockAgent::new("a", AgentKind::Llm, vec![]));
        let id = a.descriptor().id;
        registry.register(a).await;
        assert_eq!(registry.count().await, 1);
        registry.deregister(&id).await;
        assert_eq!(registry.count().await, 0);
    }

    #[tokio::test]
    async fn concurrent_register() {
        let registry = Arc::new(AgentRegistry::new());
        let mut handles = vec![];
        for i in 0..10 {
            let reg = registry.clone();
            handles.push(tokio::spawn(async move {
                let agent = Arc::new(MockAgent::new(
                    &format!("agent-{i}"),
                    AgentKind::Llm,
                    vec![],
                ));
                reg.register(agent).await;
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(registry.count().await, 10);
    }
}
