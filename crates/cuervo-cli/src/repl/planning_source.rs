//! ContextSource adapter for the plan-execute prompt.
//!
//! Injects a planning prompt into the model's system context,
//! guiding the model to reason about tool ordering and parallelism
//! before committing to actions. Priority 90 (below instructions=100,
//! above memory=80).

use async_trait::async_trait;

use cuervo_context::estimate_tokens;
use cuervo_core::error::Result;
use cuervo_core::traits::{ContextChunk, ContextQuery, ContextSource};
use cuervo_core::types::PlanningConfig;

/// Default planning prompt template.
const DEFAULT_PLANNING_PROMPT: &str = r#"## Execution Planning

When the user's request requires multiple steps or tool invocations:

1. **Analyze**: Break down the request into discrete subtasks.
2. **Plan**: Determine which tools to use and in what order.
3. **Parallelize**: Identify independent operations that can run concurrently:
   - Read-only operations (file_read, glob, grep) on different targets can run in parallel.
   - Write operations must be sequenced to avoid conflicts.
4. **Execute**: Carry out the plan, adjusting if intermediate results change the approach.

When you need to gather information before acting, prefer issuing multiple read-only
tool calls simultaneously rather than one at a time."#;

/// A ContextSource that injects a planning prompt to guide tool execution strategy.
pub struct PlanningSource {
    config: PlanningConfig,
}

impl PlanningSource {
    pub fn new(config: &PlanningConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }

    fn prompt_text(&self) -> &str {
        if let Some(ref custom) = self.config.custom_prompt {
            if !custom.is_empty() {
                return custom;
            }
        }
        DEFAULT_PLANNING_PROMPT
    }
}

#[async_trait]
impl ContextSource for PlanningSource {
    fn name(&self) -> &str {
        "planning"
    }

    fn priority(&self) -> u32 {
        90 // Below instructions (100), above memory (80).
    }

    async fn gather(&self, _query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        if !self.config.enabled {
            return Ok(vec![]);
        }

        let text = self.prompt_text();
        let tokens = estimate_tokens(text);

        Ok(vec![ContextChunk {
            source: "planning".into(),
            priority: self.priority(),
            content: text.to_string(),
            estimated_tokens: tokens,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> ContextQuery {
        ContextQuery {
            working_directory: "/tmp".into(),
            user_message: Some("test".to_string()),
            token_budget: 10000,
        }
    }

    #[tokio::test]
    async fn disabled_returns_empty() {
        let config = PlanningConfig {
            enabled: false,
            ..PlanningConfig::default()
        };
        let source = PlanningSource::new(&config);
        let chunks = source.gather(&query()).await.unwrap();
        assert!(chunks.is_empty());
    }

    #[tokio::test]
    async fn enabled_returns_planning_prompt() {
        let config = PlanningConfig::default();
        assert!(config.enabled); // default is enabled
        let source = PlanningSource::new(&config);
        let chunks = source.gather(&query()).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].source, "planning");
        assert_eq!(chunks[0].priority, 90);
        assert!(chunks[0].content.contains("Execution Planning"));
        assert!(chunks[0].content.contains("Parallelize"));
    }

    #[tokio::test]
    async fn custom_prompt_overrides_default() {
        let config = PlanningConfig {
            enabled: true,
            custom_prompt: Some("Custom planning rules here.".into()),
            ..PlanningConfig::default()
        };
        let source = PlanningSource::new(&config);
        let chunks = source.gather(&query()).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "Custom planning rules here.");
    }

    #[tokio::test]
    async fn empty_custom_prompt_uses_default() {
        let config = PlanningConfig {
            enabled: true,
            custom_prompt: Some(String::new()),
            ..PlanningConfig::default()
        };
        let source = PlanningSource::new(&config);
        let chunks = source.gather(&query()).await.unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("Execution Planning"));
    }

    #[test]
    fn metadata() {
        let config = PlanningConfig::default();
        let source = PlanningSource::new(&config);
        assert_eq!(source.name(), "planning");
        assert_eq!(source.priority(), 90);
    }

    #[test]
    fn default_prompt_has_parallelism_guidance() {
        assert!(DEFAULT_PLANNING_PROMPT.contains("parallel"));
        assert!(DEFAULT_PLANNING_PROMPT.contains("Read-only"));
        assert!(DEFAULT_PLANNING_PROMPT.contains("Write operations"));
    }
}
