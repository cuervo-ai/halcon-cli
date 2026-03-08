#![allow(dead_code)] // Infrastructure module: wired via /inspect context, not all methods called yet
//! Context Manager: unified facade coordinating context pipeline, sources,
//! governance, and metrics.
//!
//! The ContextManager does NOT replace existing systems — it wraps them into
//! a single entry point for the agent loop, adding provenance tracking,
//! governance enforcement, and observability.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use halcon_context::assembler::{assemble_context, chunks_to_system_prompt};
use halcon_context::pipeline::{ContextPipeline, ContextPipelineConfig};
use halcon_core::traits::{ContextQuery, ContextSource};
use halcon_core::types::ChatMessage;

use super::context_governance::{ContextGovernance, ContextProvenance};
use super::context_metrics::ContextMetrics;

/// Aggregated context assembly result.
pub(crate) struct AssembledContext {
    /// System prompt assembled from context sources (None if no sources contributed).
    pub system_prompt: Option<String>,
    /// Token-budgeted messages from the pipeline (L0-L4).
    pub messages: Vec<ChatMessage>,
    /// Provenance records for each context contribution.
    pub provenance: Vec<ContextProvenance>,
    /// Total tokens contributed by sources (system prompt only, not messages).
    pub total_source_tokens: u32,
    /// Wall-clock assembly duration in microseconds.
    pub assembly_duration_us: u64,
}

/// Scoped context for a sub-agent.
pub(crate) struct SubAgentContext {
    /// System prompt inherited from the parent.
    pub system_prompt: Option<String>,
    /// Seed messages (recent history from parent, trimmed).
    pub seed_messages: Vec<ChatMessage>,
    /// Task-specific instruction for the sub-agent.
    pub task_instruction: String,
}

/// Estadísticas de uso en runtime por source (para Context Servers modal).
#[derive(Debug, Clone)]
pub(crate) struct SourceStats {
    /// Tokens totales procesados por este source.
    pub total_tokens: u32,
    /// Última vez que se consultó este source (None si nunca).
    pub last_query: Option<Instant>,
    /// Número total de consultas exitosas.
    pub query_count: u64,
}

impl Default for SourceStats {
    fn default() -> Self {
        Self {
            total_tokens: 0,
            last_query: None,
            query_count: 0,
        }
    }
}

/// Central facade coordinating context pipeline + sources + governance + metrics.
pub(crate) struct ContextManager {
    pipeline: ContextPipeline,
    sources: Vec<Box<dyn ContextSource>>,
    governance: ContextGovernance,
    metrics: ContextMetrics,
    /// Cached system prompt from the last assembly (for sub-agent scoping).
    last_system_prompt: Option<String>,
    /// Runtime statistics per source (name → stats).
    /// Updated on each assemble() call for Context Servers modal.
    source_stats: HashMap<String, SourceStats>,
}

impl ContextManager {
    /// Create a new context manager wrapping existing infrastructure.
    pub fn new(
        pipeline_config: &ContextPipelineConfig,
        sources: Vec<Box<dyn ContextSource>>,
        governance: ContextGovernance,
    ) -> Self {
        // Inicializar stats para cada source con valores por defecto
        let mut source_stats = HashMap::with_capacity(sources.len());
        for source in &sources {
            source_stats.insert(source.name().to_string(), SourceStats::default());
        }

        Self {
            pipeline: ContextPipeline::new(pipeline_config),
            sources,
            governance,
            metrics: ContextMetrics::default(),
            last_system_prompt: None,
            source_stats,
        }
    }

    /// Assemble full context for a model request.
    ///
    /// 1. Gathers chunks from all context sources (parallel, via assembler)
    /// 2. Applies governance rules (token limits, disabled sources)
    /// 3. Builds system prompt from filtered chunks
    /// 4. Builds token-budgeted messages from the pipeline
    /// 5. Records provenance and metrics
    pub async fn assemble(&mut self, query: &ContextQuery) -> AssembledContext {
        let start = Instant::now();

        // 1. Gather chunks from sources.
        let raw_chunks = assemble_context(&self.sources, query).await;
        self.metrics
            .record_source_invocations(self.sources.len() as u64);

        // 2. Apply governance.
        let (governed_chunks, provenance) = self.governance.apply(raw_chunks);
        let truncation_count = provenance
            .iter()
            .zip(governed_chunks.iter())
            .filter(|(p, c)| (p.token_count as usize) > c.estimated_tokens)
            .count();
        for _ in 0..truncation_count {
            self.metrics.record_governance_truncation();
        }

        // 3. Build system prompt.
        let system_prompt = if governed_chunks.is_empty() {
            None
        } else {
            Some(chunks_to_system_prompt(&governed_chunks))
        };

        let total_source_tokens: u32 = governed_chunks
            .iter()
            .map(|c| c.estimated_tokens as u32)
            .sum();

        // Track stats per source for Context Servers modal
        let now = Instant::now();
        for chunk in &governed_chunks {
            if let Some(stats) = self.source_stats.get_mut(&chunk.source) {
                stats.total_tokens += chunk.estimated_tokens as u32;
                stats.last_query = Some(now);
                stats.query_count += 1;
            }
        }

        // 4. Build messages.
        let messages = self.pipeline.build_messages();

        // Cache for sub-agent scoping.
        self.last_system_prompt = system_prompt.clone();

        let duration_us = start.elapsed().as_micros() as u64;
        self.metrics.record_assembly(total_source_tokens, duration_us);

        AssembledContext {
            system_prompt,
            messages,
            provenance,
            total_source_tokens,
            assembly_duration_us: duration_us,
        }
    }

    /// Add a message to the pipeline (delegates to ContextPipeline::add_message).
    pub fn add_message(&mut self, msg: ChatMessage) {
        self.pipeline.add_message(msg);
    }

    /// Build token-budgeted messages (delegates to ContextPipeline::build_messages).
    pub fn build_messages(&mut self) -> Vec<ChatMessage> {
        self.pipeline.build_messages()
    }

    /// Initialize the pipeline with system prompt and working directory.
    pub fn initialize_pipeline(&mut self, system_prompt: &str, working_dir: &Path) {
        self.pipeline.initialize(system_prompt, working_dir);
    }

    /// Refresh instruction files (delegates to ContextPipeline::refresh_instructions).
    pub fn refresh_instructions(&mut self, working_dir: &Path) -> Option<String> {
        self.pipeline.refresh_instructions(working_dir)
    }

    /// Set the round counter on the pipeline.
    pub fn set_round(&mut self, round: u32) {
        self.pipeline.set_round(round);
    }

    /// Get a metrics snapshot.
    pub fn metrics(&self) -> &ContextMetrics {
        &self.metrics
    }

    /// Get the number of registered context sources.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// Iterate over context sources for inspection (name + priority).
    pub fn sources(&self) -> impl Iterator<Item = (&str, u32)> + '_ {
        self.sources.iter().map(|s| (s.name(), s.priority()))
    }

    /// Get sources with their runtime statistics (for Context Servers modal).
    /// Returns: (name, priority, SourceStats cloned)
    pub fn sources_with_stats(&self) -> impl Iterator<Item = (&str, u32, SourceStats)> + '_ {
        self.sources.iter().map(move |s| {
            let name = s.name();
            let priority = s.priority();
            // Unwrap safe: all sources initialized in constructor
            let stats = self.source_stats.get(name)
                .cloned()
                .unwrap_or_default();
            (name, priority, stats)
        })
    }

    /// Get stats for a specific source by name.
    pub fn get_source_stats(&self, name: &str) -> Option<&SourceStats> {
        self.source_stats.get(name)
    }

    /// Access the inner pipeline for direct operations (flush L4, load L4, elider, etc.).
    pub fn pipeline_mut(&mut self) -> &mut ContextPipeline {
        &mut self.pipeline
    }

    /// Access the inner pipeline (immutable).
    pub fn pipeline(&self) -> &ContextPipeline {
        &self.pipeline
    }

    /// Create a lightweight scoped context for a sub-agent.
    ///
    /// Includes the parent's system prompt, a limited number of recent history
    /// messages, and the task-specific instruction.
    pub fn scoped_for_sub_agent(
        &self,
        task_instruction: &str,
        max_history_messages: usize,
    ) -> SubAgentContext {
        let seed_messages = {
            let all = self.pipeline.build_messages();
            let start = all.len().saturating_sub(max_history_messages);
            all[start..].to_vec()
        };

        SubAgentContext {
            system_prompt: self.last_system_prompt.clone(),
            seed_messages,
            task_instruction: task_instruction.to_string(),
        }
    }

    /// Reset the pipeline (used after compaction).
    pub fn reset_pipeline(&mut self) {
        self.pipeline.reset();
    }

    /// Add a new context source at runtime (e.g., multimodal context after init).
    ///
    /// The source is appended after all existing sources; priority ordering
    /// is applied during assemble() when chunks are sorted by priority.
    pub(crate) fn add_source(&mut self, source: Box<dyn ContextSource>) {
        let name = source.name().to_string();
        self.source_stats.insert(name, SourceStats::default());
        self.sources.push(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::context_governance::SourceGovernance;
    use async_trait::async_trait;
    use halcon_context::assembler::estimate_tokens;
    use halcon_core::traits::ContextChunk;
    use std::collections::HashMap;

    struct MockSource {
        name: &'static str,
        priority: u32,
        content: &'static str,
    }

    #[async_trait]
    impl ContextSource for MockSource {
        fn name(&self) -> &str {
            self.name
        }
        fn priority(&self) -> u32 {
            self.priority
        }
        async fn gather(
            &self,
            _query: &ContextQuery,
        ) -> halcon_core::error::Result<Vec<ContextChunk>> {
            Ok(vec![ContextChunk {
                source: self.name.into(),
                priority: self.priority,
                content: self.content.into(),
                estimated_tokens: estimate_tokens(self.content),
            }])
        }
    }

    fn test_query() -> ContextQuery {
        ContextQuery {
            working_directory: "/tmp".into(),
            user_message: Some("test".into()),
            token_budget: 10_000,
        }
    }

    fn test_manager(sources: Vec<Box<dyn ContextSource>>) -> ContextManager {
        ContextManager::new(
            &ContextPipelineConfig::default(),
            sources,
            ContextGovernance::new(HashMap::new()),
        )
    }

    #[tokio::test]
    async fn new_with_no_sources() {
        let mut mgr = test_manager(vec![]);
        let result = mgr.assemble(&test_query()).await;
        assert!(result.system_prompt.is_none());
        assert!(result.provenance.is_empty());
        assert_eq!(result.total_source_tokens, 0);
    }

    #[tokio::test]
    async fn assemble_with_sources() {
        let mut mgr = test_manager(vec![
            Box::new(MockSource {
                name: "test",
                priority: 10,
                content: "hello world",
            }),
        ]);
        let result = mgr.assemble(&test_query()).await;
        assert!(result.system_prompt.is_some());
        assert_eq!(result.system_prompt.unwrap(), "hello world");
        assert_eq!(result.provenance.len(), 1);
        assert_eq!(result.provenance[0].source_name, "test");
    }

    #[tokio::test]
    async fn assemble_with_provenance() {
        let mut mgr = test_manager(vec![
            Box::new(MockSource {
                name: "src_a",
                priority: 10,
                content: "alpha content",
            }),
            Box::new(MockSource {
                name: "src_b",
                priority: 20,
                content: "beta content",
            }),
        ]);
        let result = mgr.assemble(&test_query()).await;
        assert_eq!(result.provenance.len(), 2);
        // Sorted by priority desc: src_b first, then src_a
        assert_eq!(result.provenance[0].source_name, "src_b");
        assert_eq!(result.provenance[1].source_name, "src_a");
    }

    #[tokio::test]
    async fn metrics_updated_after_assembly() {
        let mut mgr = test_manager(vec![
            Box::new(MockSource {
                name: "s",
                priority: 1,
                content: "data",
            }),
        ]);
        mgr.assemble(&test_query()).await;
        let snap = mgr.metrics().snapshot();
        assert_eq!(snap.assemblies, 1);
        assert!(snap.total_tokens_assembled > 0);
        assert!(snap.total_assembly_duration_us > 0);
        assert_eq!(snap.source_invocations, 1);
    }

    #[test]
    fn add_message_delegates() {
        let mut mgr = test_manager(vec![]);
        let msg = ChatMessage {
            role: halcon_core::types::Role::User,
            content: halcon_core::types::MessageContent::Text("hello".into()),
        };
        mgr.add_message(msg);
        let msgs = mgr.build_messages();
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn build_messages_delegates() {
        let mut mgr = test_manager(vec![]);
        let msgs = mgr.build_messages();
        assert!(msgs.is_empty());
    }

    #[test]
    fn pipeline_mut_access() {
        let mut mgr = test_manager(vec![]);
        let _pipeline = mgr.pipeline_mut();
    }

    #[tokio::test]
    async fn scoped_for_sub_agent_includes_system_prompt() {
        let mut mgr = test_manager(vec![
            Box::new(MockSource {
                name: "src",
                priority: 10,
                content: "system prompt content",
            }),
        ]);
        // Assemble to populate last_system_prompt
        mgr.assemble(&test_query()).await;
        let scoped = mgr.scoped_for_sub_agent("do task X", 5);
        assert!(scoped.system_prompt.is_some());
        assert_eq!(scoped.task_instruction, "do task X");
    }

    #[test]
    fn scoped_for_sub_agent_limits_history() {
        let mut mgr = test_manager(vec![]);
        // Add 5 messages
        for i in 0..5 {
            mgr.add_message(ChatMessage {
                role: halcon_core::types::Role::User,
                content: halcon_core::types::MessageContent::Text(format!("msg {i}")),
                });
        }
        let scoped = mgr.scoped_for_sub_agent("task", 3);
        assert!(scoped.seed_messages.len() <= 3);
        assert_eq!(scoped.task_instruction, "task");
    }

    #[test]
    fn scoped_for_sub_agent_empty_pipeline() {
        let mgr = test_manager(vec![]);
        let scoped = mgr.scoped_for_sub_agent("task", 5);
        assert!(scoped.system_prompt.is_none());
        assert!(scoped.seed_messages.is_empty());
    }

    #[tokio::test]
    async fn governance_applied_in_assembly() {
        let mut limits = HashMap::new();
        limits.insert(
            "disabled_src".to_string(),
            SourceGovernance {
                max_tokens: 0,
                enabled: false,
            },
        );
        let mut mgr = ContextManager::new(
            &ContextPipelineConfig::default(),
            vec![
                Box::new(MockSource {
                    name: "disabled_src",
                    priority: 10,
                    content: "should not appear",
                }),
                Box::new(MockSource {
                    name: "enabled_src",
                    priority: 20,
                    content: "visible content",
                }),
            ],
            ContextGovernance::new(limits),
        );
        let result = mgr.assemble(&test_query()).await;
        assert_eq!(result.provenance.len(), 1);
        assert_eq!(result.provenance[0].source_name, "enabled_src");
    }
}
