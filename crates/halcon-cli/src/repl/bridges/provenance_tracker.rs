//! Provenance tracker — accumulates execution lineage per task.

use std::collections::HashMap;

use uuid::Uuid;

use halcon_core::types::TaskProvenance;

/// Builder for accumulating provenance data during task execution.
struct ProvenanceBuilder {
    session_id: Option<Uuid>,
    model: Option<String>,
    provider: Option<String>,
    tools_used: Vec<String>,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    delegated_to: Option<String>,
    parent_task_id: Option<Uuid>,
}

/// Tracks provenance for multiple concurrent tasks.
pub(crate) struct ProvenanceTracker {
    entries: HashMap<Uuid, ProvenanceBuilder>,
}

impl ProvenanceTracker {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Begin tracking provenance for a task.
    pub fn begin(&mut self, task_id: Uuid, session_id: Option<Uuid>) {
        self.entries.insert(
            task_id,
            ProvenanceBuilder {
                session_id,
                model: None,
                provider: None,
                tools_used: Vec::new(),
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
                delegated_to: None,
                parent_task_id: None,
            },
        );
    }

    /// Record the model and provider used for a task.
    pub fn record_model(&mut self, task_id: Uuid, model: &str, provider: &str) {
        if let Some(entry) = self.entries.get_mut(&task_id) {
            entry.model = Some(model.to_string());
            entry.provider = Some(provider.to_string());
        }
    }

    /// Accumulate token usage and cost.
    pub fn add_tokens(&mut self, task_id: Uuid, input: u64, output: u64, cost: f64) {
        if let Some(entry) = self.entries.get_mut(&task_id) {
            entry.input_tokens += input;
            entry.output_tokens += output;
            entry.cost_usd += cost;
        }
    }

    /// Record a tool used during execution.
    pub fn record_tool(&mut self, task_id: Uuid, tool_name: &str) {
        if let Some(entry) = self.entries.get_mut(&task_id) {
            if !entry.tools_used.contains(&tool_name.to_string()) {
                entry.tools_used.push(tool_name.to_string());
            }
        }
    }

    /// Record delegation to a sub-agent.
    pub fn record_delegation(&mut self, task_id: Uuid, agent_type: &str) {
        if let Some(entry) = self.entries.get_mut(&task_id) {
            entry.delegated_to = Some(agent_type.to_string());
        }
    }

    /// Set the parent task ID (for sub-tasks).
    #[allow(dead_code)]
    pub fn set_parent(&mut self, task_id: Uuid, parent_id: Uuid) {
        if let Some(entry) = self.entries.get_mut(&task_id) {
            entry.parent_task_id = Some(parent_id);
        }
    }

    /// Finalize and return the provenance for a task, removing it from tracking.
    pub fn finalize(&mut self, task_id: Uuid, round: Option<usize>) -> Option<TaskProvenance> {
        let entry = self.entries.remove(&task_id)?;
        Some(TaskProvenance {
            model: entry.model,
            provider: entry.provider,
            tools_used: entry.tools_used,
            input_tokens: entry.input_tokens,
            output_tokens: entry.output_tokens,
            cost_usd: entry.cost_usd,
            context_hash: None,
            parent_task_id: entry.parent_task_id,
            delegated_to: entry.delegated_to,
            session_id: entry.session_id,
            round,
        })
    }

    /// Discard tracking for a task without producing provenance.
    pub fn discard(&mut self, task_id: Uuid) {
        self.entries.remove(&task_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_and_finalize_lifecycle() {
        let mut tracker = ProvenanceTracker::new();
        let task_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        tracker.begin(task_id, Some(session_id));
        let prov = tracker.finalize(task_id, Some(3)).unwrap();

        assert_eq!(prov.session_id, Some(session_id));
        assert_eq!(prov.round, Some(3));
        assert!(prov.model.is_none());
        assert!(prov.tools_used.is_empty());
        assert_eq!(prov.input_tokens, 0);
    }

    #[test]
    fn record_model() {
        let mut tracker = ProvenanceTracker::new();
        let task_id = Uuid::new_v4();

        tracker.begin(task_id, None);
        tracker.record_model(task_id, "gpt-4o", "openai");

        let prov = tracker.finalize(task_id, None).unwrap();
        assert_eq!(prov.model.as_deref(), Some("gpt-4o"));
        assert_eq!(prov.provider.as_deref(), Some("openai"));
    }

    #[test]
    fn add_tokens_accumulates() {
        let mut tracker = ProvenanceTracker::new();
        let task_id = Uuid::new_v4();

        tracker.begin(task_id, None);
        tracker.add_tokens(task_id, 100, 50, 0.01);
        tracker.add_tokens(task_id, 200, 100, 0.02);

        let prov = tracker.finalize(task_id, None).unwrap();
        assert_eq!(prov.input_tokens, 300);
        assert_eq!(prov.output_tokens, 150);
        assert!((prov.cost_usd - 0.03).abs() < f64::EPSILON);
    }

    #[test]
    fn record_tool_deduplicates() {
        let mut tracker = ProvenanceTracker::new();
        let task_id = Uuid::new_v4();

        tracker.begin(task_id, None);
        tracker.record_tool(task_id, "file_read");
        tracker.record_tool(task_id, "bash");
        tracker.record_tool(task_id, "file_read"); // duplicate

        let prov = tracker.finalize(task_id, None).unwrap();
        assert_eq!(prov.tools_used, vec!["file_read", "bash"]);
    }

    #[test]
    fn record_delegation() {
        let mut tracker = ProvenanceTracker::new();
        let task_id = Uuid::new_v4();

        tracker.begin(task_id, None);
        tracker.record_delegation(task_id, "Coder");

        let prov = tracker.finalize(task_id, None).unwrap();
        assert_eq!(prov.delegated_to.as_deref(), Some("Coder"));
    }

    #[test]
    fn discard_removes_entry() {
        let mut tracker = ProvenanceTracker::new();
        let task_id = Uuid::new_v4();

        tracker.begin(task_id, None);
        tracker.discard(task_id);

        assert!(tracker.finalize(task_id, None).is_none());
    }

    #[test]
    fn finalize_unknown_returns_none() {
        let mut tracker = ProvenanceTracker::new();
        assert!(tracker.finalize(Uuid::new_v4(), None).is_none());
    }

    #[test]
    fn operations_on_unknown_task_are_noop() {
        let mut tracker = ProvenanceTracker::new();
        let fake = Uuid::new_v4();
        // These should not panic.
        tracker.record_model(fake, "m", "p");
        tracker.add_tokens(fake, 100, 50, 0.01);
        tracker.record_tool(fake, "bash");
        tracker.record_delegation(fake, "Coder");
    }
}
