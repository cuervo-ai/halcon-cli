//! Capability matching and negotiation.
//!
//! Maintains an index of which agents provide which capabilities,
//! enabling capability-based agent discovery and best-match selection.

use std::collections::HashMap;

use uuid::Uuid;

use crate::agent::AgentCapability;

/// Index mapping capabilities to agent IDs for fast lookup.
#[derive(Debug, Default)]
pub struct CapabilityIndex {
    agents: HashMap<AgentCapability, Vec<Uuid>>,
}

impl CapabilityIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an agent's capabilities in the index.
    pub fn register(&mut self, id: Uuid, caps: &[AgentCapability]) {
        for cap in caps {
            self.agents.entry(cap.clone()).or_default().push(id);
        }
    }

    /// Remove an agent from all capability entries.
    pub fn deregister(&mut self, id: &Uuid) {
        for agents in self.agents.values_mut() {
            agents.retain(|a| a != id);
        }
        // Remove empty capability entries.
        self.agents.retain(|_, agents| !agents.is_empty());
    }

    /// Find all agent IDs that provide a given capability.
    pub fn find_agents(&self, cap: &AgentCapability) -> &[Uuid] {
        self.agents.get(cap).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Find the agent that covers the most of the required capabilities.
    ///
    /// Returns `None` if no agent covers any of the required capabilities.
    pub fn best_match(&self, required: &[AgentCapability]) -> Option<Uuid> {
        if required.is_empty() {
            return None;
        }

        // Count how many required capabilities each agent covers.
        let mut coverage: HashMap<Uuid, usize> = HashMap::new();
        for cap in required {
            for agent_id in self.find_agents(cap) {
                *coverage.entry(*agent_id).or_default() += 1;
            }
        }

        coverage
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(id, _)| id)
    }

    /// All capabilities currently registered.
    pub fn all_capabilities(&self) -> Vec<&AgentCapability> {
        self.agents.keys().collect()
    }

    /// Total number of unique capability-agent mappings.
    pub fn len(&self) -> usize {
        self.agents.values().map(|v| v.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn register_and_find() {
        let mut idx = CapabilityIndex::new();
        let a = id(1);
        idx.register(a, &[AgentCapability::CodeGeneration, AgentCapability::Testing]);

        assert_eq!(idx.find_agents(&AgentCapability::CodeGeneration), &[a]);
        assert_eq!(idx.find_agents(&AgentCapability::Testing), &[a]);
        assert!(idx.find_agents(&AgentCapability::WebSearch).is_empty());
    }

    #[test]
    fn register_multiple_agents_same_cap() {
        let mut idx = CapabilityIndex::new();
        let a = id(1);
        let b = id(2);
        idx.register(a, &[AgentCapability::CodeGeneration]);
        idx.register(b, &[AgentCapability::CodeGeneration]);

        let agents = idx.find_agents(&AgentCapability::CodeGeneration);
        assert_eq!(agents.len(), 2);
        assert!(agents.contains(&a));
        assert!(agents.contains(&b));
    }

    #[test]
    fn deregister() {
        let mut idx = CapabilityIndex::new();
        let a = id(1);
        let b = id(2);
        idx.register(a, &[AgentCapability::CodeGeneration, AgentCapability::Testing]);
        idx.register(b, &[AgentCapability::CodeGeneration]);

        idx.deregister(&a);

        assert_eq!(idx.find_agents(&AgentCapability::CodeGeneration), &[b]);
        assert!(idx.find_agents(&AgentCapability::Testing).is_empty());
    }

    #[test]
    fn deregister_cleans_empty_entries() {
        let mut idx = CapabilityIndex::new();
        let a = id(1);
        idx.register(a, &[AgentCapability::Testing]);
        idx.deregister(&a);
        assert!(idx.is_empty());
    }

    #[test]
    fn best_match_single_agent_full_coverage() {
        let mut idx = CapabilityIndex::new();
        let a = id(1);
        idx.register(
            a,
            &[
                AgentCapability::CodeGeneration,
                AgentCapability::Testing,
                AgentCapability::FileOperations,
            ],
        );
        let result = idx.best_match(&[AgentCapability::CodeGeneration, AgentCapability::Testing]);
        assert_eq!(result, Some(a));
    }

    #[test]
    fn best_match_picks_highest_coverage() {
        let mut idx = CapabilityIndex::new();
        let a = id(1);
        let b = id(2);
        // a covers 1 of 3 required
        idx.register(a, &[AgentCapability::CodeGeneration]);
        // b covers 2 of 3 required
        idx.register(
            b,
            &[AgentCapability::CodeGeneration, AgentCapability::Testing],
        );

        let result = idx.best_match(&[
            AgentCapability::CodeGeneration,
            AgentCapability::Testing,
            AgentCapability::WebSearch,
        ]);
        assert_eq!(result, Some(b));
    }

    #[test]
    fn best_match_empty_required_returns_none() {
        let mut idx = CapabilityIndex::new();
        idx.register(id(1), &[AgentCapability::CodeGeneration]);
        assert_eq!(idx.best_match(&[]), None);
    }

    #[test]
    fn best_match_no_agents_returns_none() {
        let idx = CapabilityIndex::new();
        assert_eq!(idx.best_match(&[AgentCapability::CodeGeneration]), None);
    }

    #[test]
    fn best_match_no_overlap_returns_none() {
        let mut idx = CapabilityIndex::new();
        idx.register(id(1), &[AgentCapability::WebSearch]);
        assert_eq!(idx.best_match(&[AgentCapability::CodeGeneration]), None);
    }

    #[test]
    fn find_agents_unknown_capability() {
        let idx = CapabilityIndex::new();
        assert!(idx
            .find_agents(&AgentCapability::Custom("unknown".to_string()))
            .is_empty());
    }

    #[test]
    fn all_capabilities() {
        let mut idx = CapabilityIndex::new();
        idx.register(
            id(1),
            &[AgentCapability::CodeGeneration, AgentCapability::Testing],
        );
        let caps = idx.all_capabilities();
        assert_eq!(caps.len(), 2);
    }

    #[test]
    fn len_and_is_empty() {
        let mut idx = CapabilityIndex::new();
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);

        idx.register(
            id(1),
            &[AgentCapability::CodeGeneration, AgentCapability::Testing],
        );
        assert!(!idx.is_empty());
        assert_eq!(idx.len(), 2);

        idx.register(id(2), &[AgentCapability::CodeGeneration]);
        assert_eq!(idx.len(), 3);
    }

    #[test]
    fn custom_capability_matching() {
        let mut idx = CapabilityIndex::new();
        let a = id(1);
        let custom = AgentCapability::Custom("image_gen".to_string());
        idx.register(a, std::slice::from_ref(&custom));
        assert_eq!(idx.find_agents(&custom), &[a]);
        // Different custom name should not match
        assert!(idx
            .find_agents(&AgentCapability::Custom("audio_gen".to_string()))
            .is_empty());
    }
}
