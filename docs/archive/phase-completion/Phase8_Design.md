# Phase 8: State-of-the-Art Agent Architecture — Technical Design

> **Stage**: 2 (Design) | **Status**: COMPLETE
> **Prerequisite**: Phase8_Research.md (Stage 1)
> **Target**: 5 sub-phases, ~6,000-9,000 new LOC, 60-100 new tests
> **Baseline**: 593 tests, 5.0MB binary, 7 migrations, 21k+ LOC

---

## Table of Contents

1. [Design Principles](#1-design-principles)
2. [Sub-Phase 8.1: Adaptive Planner](#2-sub-phase-81-adaptive-planner)
3. [Sub-Phase 8.2: Reflexion Self-Improvement](#3-sub-phase-82-reflexion-self-improvement)
4. [Sub-Phase 8.3: TBAC Authorization](#4-sub-phase-83-tbac-authorization)
5. [Sub-Phase 8.4: Safety Guardrails](#5-sub-phase-84-safety-guardrails)
6. [Sub-Phase 8.5: Episodic Memory](#6-sub-phase-85-episodic-memory)
7. [Agent Loop Refactoring](#7-agent-loop-refactoring)
8. [Migration Summary](#8-migration-summary)
9. [Config Extensions](#9-config-extensions)
10. [Execution Order](#10-execution-order)
11. [Test Strategy](#11-test-strategy)
12. [Risk Mitigations](#12-risk-mitigations)

---

## 1. Design Principles

1. **Extension-first**: Wire existing unused traits (Planner, EmbeddingProvider) before creating new abstractions.
2. **Incremental compilation**: Each sub-phase compiles and passes all tests independently. No cross-sub-phase dependencies except where noted.
3. **Backward compatible**: All new config fields use `#[serde(default)]`. All new DB columns use `DEFAULT` values. All new agent loop params use `Option<&T>`.
4. **Minimal coupling increase**: New modules live as siblings of `agent.rs` in `crates/cuervo-cli/src/repl/`, not inline in agent.rs.
5. **No binary bloat**: Target ≤ 6.0MB (from 5.0MB baseline). No new heavy deps; reuse existing crates (regex, sha2, rusqlite, serde_json).

---

## 2. Sub-Phase 8.1: Adaptive Planner

### 2.1 Goal

Wire the existing `Planner` trait (cuervo-core/src/traits/planner.rs) into the agent loop. Implement `LlmPlanner` that generates execution plans via the model, persist plans to DB for observability, and add failure-triggered replanning.

### 2.2 New Types

#### `crates/cuervo-core/src/traits/planner.rs` — Extend existing types

```rust
// ADD to existing PlanStep struct:
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub description: String,
    pub tool_name: Option<String>,
    pub parallel: bool,
    pub confidence: f64,
    // NEW FIELDS:
    /// Expected arguments for the tool (optional hint, not enforced).
    pub expected_args: Option<serde_json::Value>,
    /// Outcome after execution: None until executed.
    pub outcome: Option<StepOutcome>,
}

/// Outcome of executing a plan step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepOutcome {
    Success { summary: String },
    Failed { error: String },
    Skipped { reason: String },
}

// ADD to existing ExecutionPlan struct:
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub goal: String,
    pub steps: Vec<PlanStep>,
    pub requires_confirmation: bool,
    // NEW FIELDS:
    /// Unique plan ID for persistence.
    pub plan_id: uuid::Uuid,
    /// Number of replans that produced this plan (0 = initial).
    pub replan_count: u32,
    /// Original plan ID if this is a replan.
    pub parent_plan_id: Option<uuid::Uuid>,
}

// ADD to Planner trait:
#[async_trait]
pub trait Planner: Send + Sync {
    /// Generate a plan for the given user message and available tools.
    async fn plan(
        &self,
        user_message: &str,
        available_tools: &[ToolDefinition],
    ) -> Result<Option<ExecutionPlan>>;

    /// Replan after a step failure, given the current plan and failure context.
    async fn replan(
        &self,
        current_plan: &ExecutionPlan,
        failed_step_index: usize,
        error: &str,
        available_tools: &[ToolDefinition],
    ) -> Result<Option<ExecutionPlan>> {
        // Default: no replanning. Implementations can override.
        let _ = (current_plan, failed_step_index, error, available_tools);
        Ok(None)
    }

    fn name(&self) -> &str;

    /// Maximum replans allowed before giving up.
    fn max_replans(&self) -> u32 {
        3
    }
}
```

#### `crates/cuervo-cli/src/repl/planner.rs` — NEW FILE

```rust
use std::sync::Arc;
use async_trait::async_trait;
use uuid::Uuid;

use cuervo_core::error::Result;
use cuervo_core::traits::{ExecutionPlan, ModelProvider, PlanStep, Planner, StepOutcome};
use cuervo_core::types::{
    ChatMessage, MessageContent, ModelChunk, ModelRequest, Role, ToolDefinition,
};

/// LLM-based planner that generates execution plans by prompting the model.
pub struct LlmPlanner {
    provider: Arc<dyn ModelProvider>,
    model: String,
    max_replans: u32,
}

impl LlmPlanner {
    pub fn new(provider: Arc<dyn ModelProvider>, model: String) -> Self {
        Self {
            provider,
            model,
            max_replans: 3,
        }
    }

    pub fn with_max_replans(mut self, max: u32) -> Self {
        self.max_replans = max;
        self
    }

    /// Build the planning prompt from user message and available tools.
    fn build_plan_prompt(user_message: &str, tools: &[ToolDefinition]) -> String {
        let tool_list: Vec<String> = tools.iter().map(|t| {
            format!("- {} ({}): {}", t.name, t.permission_level_hint(), t.description)
        }).collect();

        format!(
            "You are a planning agent. Given the user's request and available tools, \
             produce a JSON execution plan.\n\n\
             User request: {user_message}\n\n\
             Available tools:\n{}\n\n\
             Respond with ONLY a JSON object:\n\
             {{\n  \"goal\": \"<one-line summary>\",\n  \
             \"steps\": [\n    {{\n      \"description\": \"<what this step does>\",\n      \
             \"tool_name\": \"<tool or null>\",\n      \"parallel\": false,\n      \
             \"confidence\": 0.9,\n      \"expected_args\": {{}} \n    }}\n  ],\n  \
             \"requires_confirmation\": false\n}}\n\n\
             Rules:\n\
             - Only use tools from the list above.\n\
             - Set parallel=true ONLY for ReadOnly steps with no data dependencies.\n\
             - Set requires_confirmation=true for plans with Destructive tools.\n\
             - If no plan is needed (simple question), return null.",
            tool_list.join("\n")
        )
    }

    /// Build the replanning prompt after a step failure.
    fn build_replan_prompt(
        plan: &ExecutionPlan,
        failed_step_index: usize,
        error: &str,
        tools: &[ToolDefinition],
    ) -> String {
        let completed: Vec<String> = plan.steps[..failed_step_index]
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let outcome = s.outcome.as_ref()
                    .map(|o| format!("{o:?}"))
                    .unwrap_or_else(|| "pending".into());
                format!("  Step {i}: {} → {outcome}", s.description)
            })
            .collect();

        let failed = &plan.steps[failed_step_index];
        let tool_list: Vec<String> = tools.iter()
            .map(|t| format!("- {}: {}", t.name, t.description))
            .collect();

        format!(
            "The execution plan failed. Replan the remaining work.\n\n\
             Original goal: {}\n\
             Completed steps:\n{}\n\
             Failed step {failed_step_index}: {} (tool: {:?})\n\
             Error: {error}\n\n\
             Available tools:\n{}\n\n\
             Respond with ONLY a JSON execution plan for the REMAINING work (not already-completed steps). \
             Return null if the goal cannot be achieved.",
            plan.goal,
            completed.join("\n"),
            failed.description,
            failed.tool_name,
            tool_list.join("\n"),
        )
    }

    /// Invoke the model and parse the JSON response into an ExecutionPlan.
    async fn invoke_for_plan(&self, prompt: String) -> Result<Option<ExecutionPlan>> {
        let request = ModelRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(prompt),
            }],
            tools: vec![],
            max_tokens: Some(2048),
            temperature: Some(0.0),
            system: None,
            stream: true,
        };

        // Collect streamed response.
        use futures::StreamExt;
        let mut text = String::new();
        let mut stream = self.provider.invoke(&request).await?;
        while let Some(chunk) = stream.next().await {
            if let Ok(ModelChunk::TextDelta(delta)) = chunk {
                text.push_str(&delta);
            }
        }

        let trimmed = text.trim();
        if trimmed == "null" || trimmed.is_empty() {
            return Ok(None);
        }

        // Parse JSON into ExecutionPlan.
        let mut plan: ExecutionPlan = serde_json::from_str(trimmed)
            .map_err(|e| cuervo_core::error::CuervoError::PlanningFailed(
                format!("Failed to parse plan JSON: {e}")
            ))?;

        // Assign plan metadata.
        plan.plan_id = Uuid::new_v4();
        plan.replan_count = 0;
        plan.parent_plan_id = None;

        Ok(Some(plan))
    }
}

#[async_trait]
impl Planner for LlmPlanner {
    async fn plan(
        &self,
        user_message: &str,
        available_tools: &[ToolDefinition],
    ) -> Result<Option<ExecutionPlan>> {
        let prompt = Self::build_plan_prompt(user_message, available_tools);
        self.invoke_for_plan(prompt).await
    }

    async fn replan(
        &self,
        current_plan: &ExecutionPlan,
        failed_step_index: usize,
        error: &str,
        available_tools: &[ToolDefinition],
    ) -> Result<Option<ExecutionPlan>> {
        if current_plan.replan_count >= self.max_replans {
            tracing::warn!(
                replans = current_plan.replan_count,
                max = self.max_replans,
                "Max replans exceeded"
            );
            return Ok(None);
        }

        let prompt = Self::build_replan_prompt(current_plan, failed_step_index, error, available_tools);
        let mut plan = self.invoke_for_plan(prompt).await?;
        if let Some(ref mut p) = plan {
            p.replan_count = current_plan.replan_count + 1;
            p.parent_plan_id = Some(current_plan.plan_id);
        }
        Ok(plan)
    }

    fn name(&self) -> &str {
        "llm_planner"
    }

    fn max_replans(&self) -> u32 {
        self.max_replans
    }
}
```

### 2.3 Migration 008: planning_steps

```sql
-- Migration 008: Planning step tracking with outcomes.
CREATE TABLE IF NOT EXISTS planning_steps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id TEXT NOT NULL,
    parent_plan_id TEXT,
    session_id TEXT NOT NULL,
    goal TEXT NOT NULL,
    step_index INTEGER NOT NULL,
    description TEXT NOT NULL,
    tool_name TEXT,
    confidence REAL NOT NULL DEFAULT 0.0,
    outcome TEXT,            -- 'success', 'failed', 'skipped', NULL (pending)
    outcome_detail TEXT,     -- summary or error message
    replan_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_planning_session ON planning_steps(session_id);
CREATE INDEX IF NOT EXISTS idx_planning_plan ON planning_steps(plan_id);
CREATE INDEX IF NOT EXISTS idx_planning_created ON planning_steps(created_at DESC);
```

### 2.4 Storage Layer

#### `crates/cuervo-storage/src/sqlite.rs` — Add methods

```rust
/// Persist an execution plan's steps to the planning_steps table.
pub fn save_plan_steps(&self, session_id: &uuid::Uuid, plan: &ExecutionPlan) -> Result<()>;

/// Update a plan step's outcome after execution.
pub fn update_plan_step_outcome(
    &self,
    plan_id: &uuid::Uuid,
    step_index: u32,
    outcome: &str,        // "success" | "failed" | "skipped"
    outcome_detail: &str,
) -> Result<()>;

/// Load plan steps for a session (for doctor / diagnostics).
pub fn load_plan_steps(&self, session_id: &uuid::Uuid) -> Result<Vec<PlanStepRow>>;
```

#### `crates/cuervo-storage/src/async_db.rs` — Async wrappers

```rust
pub async fn save_plan_steps(&self, session_id: &Uuid, plan: &ExecutionPlan) -> Result<()>;
pub async fn update_plan_step_outcome(&self, plan_id: &Uuid, step_index: u32, outcome: &str, detail: &str) -> Result<()>;
```

### 2.5 New Error Variant

#### `crates/cuervo-core/src/error.rs`

```rust
// ADD to CuervoError enum:
#[error("Planning failed: {0}")]
PlanningFailed(String),
```

### 2.6 Config Extension

#### `crates/cuervo-core/src/types/config.rs` — Extend PlanningConfig

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningConfig {
    pub enabled: bool,
    #[serde(default)]
    pub custom_prompt: Option<String>,
    // NEW FIELDS:
    /// Enable LLM-based adaptive planning (generates plan before tool loop).
    #[serde(default)]
    pub adaptive: bool,
    /// Maximum replanning attempts on step failure.
    #[serde(default = "default_max_replans")]
    pub max_replans: u32,
    /// Minimum confidence threshold for auto-executing plan steps (0.0-1.0).
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f64,
}

fn default_max_replans() -> u32 { 3 }
fn default_min_confidence() -> f64 { 0.7 }
```

### 2.7 Wiring Points

#### `crates/cuervo-cli/src/repl/agent.rs`

**New parameter** (position 15):
```rust
pub async fn run_agent_loop(
    // ... existing 14 params ...
    planner: Option<&dyn Planner>,   // NEW — position 15
) -> Result<AgentLoopResult>
```

**Wiring in agent loop** — Insert BEFORE the main `for round in 0..limits.max_rounds` loop (before line 230):

```rust
// Adaptive planning: generate plan before entering tool loop.
let mut active_plan: Option<ExecutionPlan> = None;
if let Some(planner) = planner {
    let tool_defs = request.tools.clone();
    let user_msg = messages.last()
        .and_then(|m| match &m.content {
            MessageContent::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or("");

    match planner.plan(user_msg, &tool_defs).await {
        Ok(Some(plan)) => {
            tracing::info!(goal = %plan.goal, steps = plan.steps.len(), "Plan generated");
            // Persist plan steps.
            if let Some(db) = trace_db {
                let _ = db.save_plan_steps(&session_id, &plan).await;
            }
            active_plan = Some(plan);
        }
        Ok(None) => {
            tracing::debug!("Planner returned no plan (simple query)");
        }
        Err(e) => {
            tracing::warn!("Planning failed, proceeding without plan: {e}");
        }
    }
}
```

**Wiring for replanning** — After tool execution results (inside the tool-use arm), when a tool fails:

```rust
// After tool execution, check for failures and trigger replan.
if let (Some(ref mut plan), Some(planner)) = (&mut active_plan, planner) {
    // Find the step that corresponds to this tool execution.
    if let Some((step_idx, step)) = plan.steps.iter_mut().enumerate()
        .find(|(_, s)| s.tool_name.as_deref() == Some(&failed_tool_name) && s.outcome.is_none())
    {
        step.outcome = Some(StepOutcome::Failed { error: error_msg.clone() });
        // Persist outcome.
        if let Some(db) = trace_db {
            let _ = db.update_plan_step_outcome(&plan.plan_id, step_idx as u32, "failed", &error_msg).await;
        }
        // Attempt replan.
        match planner.replan(plan, step_idx, &error_msg, &request.tools).await {
            Ok(Some(new_plan)) => {
                tracing::info!(goal = %new_plan.goal, replan = new_plan.replan_count, "Replanned");
                if let Some(db) = trace_db {
                    let _ = db.save_plan_steps(&session_id, &new_plan).await;
                }
                *plan = new_plan;
            }
            Ok(None) => tracing::debug!("No replan available"),
            Err(e) => tracing::warn!("Replan failed: {e}"),
        }
    }
}
```

#### `crates/cuervo-cli/src/repl/mod.rs`

In `Repl` struct — add field:
```rust
planner: Option<Box<dyn Planner>>,
```

In `Repl::new()` — after context_sources setup:
```rust
let planner: Option<Box<dyn Planner>> = if config.planning.adaptive {
    if let Some(p) = registry.get(&provider).cloned() {
        Some(Box::new(LlmPlanner::new(p, model.clone())
            .with_max_replans(config.planning.max_replans)))
    } else {
        None
    }
} else {
    None
};
```

In `handle_message()` — pass planner to run_agent_loop:
```rust
self.planner.as_deref(), // maps Option<Box<dyn Planner>> to Option<&dyn Planner>
```

### 2.8 New Event Variants

```rust
// ADD to EventPayload:
PlanGenerated {
    plan_id: uuid::Uuid,
    goal: String,
    step_count: usize,
    replan_count: u32,
},
PlanStepCompleted {
    plan_id: uuid::Uuid,
    step_index: usize,
    outcome: String,  // "success", "failed", "skipped"
},
```

### 2.9 Files Modified/Created

| File | Action |
|------|--------|
| `crates/cuervo-core/src/traits/planner.rs` | MODIFY — extend PlanStep, ExecutionPlan, Planner trait |
| `crates/cuervo-core/src/error.rs` | MODIFY — add PlanningFailed variant |
| `crates/cuervo-core/src/types/config.rs` | MODIFY — extend PlanningConfig |
| `crates/cuervo-core/src/types/event.rs` | MODIFY — add PlanGenerated, PlanStepCompleted |
| `crates/cuervo-cli/src/repl/planner.rs` | CREATE — LlmPlanner |
| `crates/cuervo-cli/src/repl/mod.rs` | MODIFY — add planner field, wire in new() and handle_message() |
| `crates/cuervo-cli/src/repl/agent.rs` | MODIFY — add planner param, pre-loop planning, replan on failure |
| `crates/cuervo-storage/src/migrations.rs` | MODIFY — add migration 008 |
| `crates/cuervo-storage/src/sqlite.rs` | MODIFY — save_plan_steps, update_plan_step_outcome |
| `crates/cuervo-storage/src/async_db.rs` | MODIFY — async wrappers |

### 2.10 Tests (12 new)

| Test | Location | Validates |
|------|----------|-----------|
| `plan_prompt_includes_tools` | planner.rs | build_plan_prompt formats tool list correctly |
| `parse_valid_plan_json` | planner.rs | JSON → ExecutionPlan deserialization |
| `parse_null_plan` | planner.rs | "null" response → Ok(None) |
| `parse_invalid_json_returns_error` | planner.rs | Malformed JSON → PlanningFailed error |
| `replan_prompt_includes_failure` | planner.rs | build_replan_prompt includes error context |
| `max_replans_exceeded` | planner.rs | replan_count ≥ max → Ok(None) |
| `plan_step_outcome_serialization` | planner.rs | StepOutcome round-trips through serde |
| `migration_008_creates_planning_table` | migrations.rs | Table + 3 indexes exist after migration |
| `save_and_load_plan_steps` | sqlite.rs | Round-trip persistence |
| `update_plan_step_outcome` | sqlite.rs | Outcome update persists correctly |
| `planning_config_defaults` | config.rs | adaptive=false, max_replans=3, min_confidence=0.7 |
| `agent_loop_without_planner_unchanged` | agent.rs | planner=None → existing behavior preserved |

### 2.11 Estimated Size

- New LOC: 800-1,200
- New tests: 12
- New file: 1 (planner.rs)
- New migration: 1 (008)

---

## 3. Sub-Phase 8.2: Reflexion Self-Improvement

### 3.1 Goal

Implement a Reflexion loop (NeurIPS 2023 pattern) that evaluates completed agent rounds, generates verbal self-reflections on failures/suboptimal outcomes, and injects those reflections into context for future rounds. Reflections are stored as memory entries for cross-session learning.

### 3.2 Architecture

```
Agent Round N → Tool Results → Evaluator (success/partial/failure)
                                    ↓
                              Self-Reflection (LLM prompt)
                                    ↓
                              ReflectionEntry → memory_entries (entry_type = 'reflection')
                                    ↓
                              ReflectionSource (ContextSource, priority=85)
                                    ↓
                              Injected into Agent Round N+1 system context
```

### 3.3 New Types

#### `crates/cuervo-cli/src/repl/reflexion.rs` — NEW FILE

```rust
use async_trait::async_trait;
use std::sync::Arc;
use futures::StreamExt;

use cuervo_core::error::Result;
use cuervo_core::traits::ModelProvider;
use cuervo_core::types::{
    ChatMessage, ContentBlock, MessageContent, ModelChunk, ModelRequest, Role,
};

/// Evaluation of a completed agent round.
#[derive(Debug, Clone)]
pub enum RoundOutcome {
    /// All tool executions succeeded, model response was useful.
    Success,
    /// Some tools failed or model needed multiple retries.
    Partial { failures: Vec<String> },
    /// Critical failure — tools crashed, permission denied, etc.
    Failure { error: String },
}

/// A self-reflection generated after evaluating a round.
#[derive(Debug, Clone)]
pub struct Reflection {
    /// What went wrong or could be improved.
    pub analysis: String,
    /// Concrete advice for future rounds.
    pub advice: String,
    /// The round number this reflection is about.
    pub round: usize,
    /// The outcome that triggered this reflection.
    pub trigger: RoundOutcome,
}

/// Evaluates agent rounds and generates reflections.
pub struct Reflector {
    provider: Arc<dyn ModelProvider>,
    model: String,
    /// Only reflect on non-success outcomes.
    reflect_on_success: bool,
}

impl Reflector {
    pub fn new(provider: Arc<dyn ModelProvider>, model: String) -> Self {
        Self {
            provider,
            model,
            reflect_on_success: false,
        }
    }

    /// Evaluate tool execution results to determine the round outcome.
    pub fn evaluate_round(
        tool_results: &[ContentBlock],
    ) -> RoundOutcome {
        let mut failures = Vec::new();
        for block in tool_results {
            if let ContentBlock::ToolResult { content, is_error, tool_use_id } = block {
                if *is_error {
                    failures.push(format!("{tool_use_id}: {content}"));
                }
            }
        }
        if failures.is_empty() {
            RoundOutcome::Success
        } else if failures.len() == tool_results.len() {
            RoundOutcome::Failure {
                error: failures.join("; "),
            }
        } else {
            RoundOutcome::Partial { failures }
        }
    }

    /// Generate a self-reflection for a non-success round.
    pub async fn reflect(
        &self,
        round: usize,
        outcome: &RoundOutcome,
        recent_messages: &[ChatMessage],
    ) -> Result<Option<Reflection>> {
        // Don't reflect on success (unless configured).
        if matches!(outcome, RoundOutcome::Success) && !self.reflect_on_success {
            return Ok(None);
        }

        let outcome_desc = match outcome {
            RoundOutcome::Success => "All tools succeeded.".to_string(),
            RoundOutcome::Partial { failures } => {
                format!("Partial failure. Failed tools:\n{}", failures.join("\n"))
            }
            RoundOutcome::Failure { error } => {
                format!("Complete failure: {error}")
            }
        };

        // Build recent context summary (last 4 messages max).
        let context: Vec<String> = recent_messages.iter().rev().take(4).rev()
            .map(|m| {
                let role = match m.role {
                    Role::User => "User",
                    Role::Assistant => "Assistant",
                };
                let text = match &m.content {
                    MessageContent::Text(t) => t.chars().take(500).collect::<String>(),
                    MessageContent::Blocks(blocks) => {
                        blocks.iter().filter_map(|b| match b {
                            ContentBlock::Text(t) => Some(t.chars().take(200).collect::<String>()),
                            _ => None,
                        }).collect::<Vec<_>>().join(" ")
                    }
                };
                format!("[{role}]: {text}")
            })
            .collect();

        let prompt = format!(
            "You are a self-reflection agent. Analyze the following failed execution round \
             and provide concrete advice for improvement.\n\n\
             Round {round} outcome: {outcome_desc}\n\n\
             Recent conversation context:\n{}\n\n\
             Respond with ONLY a JSON object:\n\
             {{\n  \"analysis\": \"<what went wrong and why>\",\n  \
             \"advice\": \"<specific, actionable advice for the next attempt>\"\n}}",
            context.join("\n"),
        );

        let request = ModelRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(prompt),
            }],
            tools: vec![],
            max_tokens: Some(1024),
            temperature: Some(0.0),
            system: None,
            stream: true,
        };

        let mut text = String::new();
        let mut stream = self.provider.invoke(&request).await?;
        while let Some(chunk) = stream.next().await {
            if let Ok(ModelChunk::TextDelta(delta)) = chunk {
                text.push_str(&delta);
            }
        }

        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }

        // Parse JSON response.
        #[derive(serde::Deserialize)]
        struct ReflectionJson {
            analysis: String,
            advice: String,
        }

        match serde_json::from_str::<ReflectionJson>(trimmed) {
            Ok(parsed) => Ok(Some(Reflection {
                analysis: parsed.analysis,
                advice: parsed.advice,
                round,
                trigger: outcome.clone(),
            })),
            Err(e) => {
                tracing::warn!("Failed to parse reflection JSON: {e}");
                // Fallback: use raw text as both analysis and advice.
                Ok(Some(Reflection {
                    analysis: trimmed.to_string(),
                    advice: String::new(),
                    round,
                    trigger: outcome.clone(),
                }))
            }
        }
    }
}
```

#### `crates/cuervo-cli/src/repl/reflection_source.rs` — NEW FILE

```rust
use async_trait::async_trait;

use cuervo_core::error::Result;
use cuervo_core::traits::{ContextChunk, ContextQuery, ContextSource};
use cuervo_storage::AsyncDatabase;

/// ContextSource that injects recent self-reflections into the system prompt.
///
/// Priority 85: above memory (80), below planning (90) and instructions (100).
pub struct ReflectionSource {
    db: AsyncDatabase,
    max_reflections: usize,
}

impl ReflectionSource {
    pub fn new(db: AsyncDatabase, max_reflections: usize) -> Self {
        Self { db, max_reflections }
    }
}

#[async_trait]
impl ContextSource for ReflectionSource {
    fn name(&self) -> &str {
        "reflections"
    }

    fn priority(&self) -> u32 {
        85
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        // Query memory_entries where entry_type = 'reflection', ordered by created_at DESC.
        let entries = self.db.search_memory_by_type("reflection", self.max_reflections).await?;

        if entries.is_empty() {
            return Ok(vec![]);
        }

        let mut content = String::from("## Previous Self-Reflections\n\n");
        content.push_str("Use these reflections to avoid repeating past mistakes:\n\n");
        for entry in &entries {
            content.push_str(&format!("- {}\n", entry.content));
        }

        let tokens = cuervo_context::assembler::estimate_tokens(&content);
        Ok(vec![ContextChunk {
            source: "reflection".into(),
            priority: self.priority(),
            content,
            estimated_tokens: tokens,
        }])
    }
}
```

### 3.4 Storage Layer Extensions

#### `crates/cuervo-storage/src/sqlite.rs`

```rust
/// Search memory entries by type, ordered by created_at DESC.
pub fn search_memory_by_type(&self, entry_type: &str, limit: usize) -> Result<Vec<MemoryEntry>>;
```

#### `crates/cuervo-storage/src/async_db.rs`

```rust
pub async fn search_memory_by_type(&self, entry_type: &str, limit: usize) -> Result<Vec<MemoryEntry>>;
```

### 3.5 Config Extension

```rust
// ADD to AppConfig:
#[serde(default)]
pub reflexion: ReflexionConfig,

// NEW struct:
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflexionConfig {
    /// Enable the reflexion self-improvement loop.
    pub enabled: bool,
    /// Number of recent reflections to inject into context.
    #[serde(default = "default_max_reflections")]
    pub max_reflections: usize,
    /// Also reflect on successful rounds (usually false).
    #[serde(default)]
    pub reflect_on_success: bool,
}

impl Default for ReflexionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_reflections: 3,
            reflect_on_success: false,
        }
    }
}

fn default_max_reflections() -> usize { 3 }
```

### 3.6 Wiring Points

#### `crates/cuervo-cli/src/repl/agent.rs`

**New parameter** (position 16):
```rust
reflector: Option<&Reflector>,  // NEW — position 16
```

**Wiring** — After tool results are appended to messages (inside the ToolUse arm, after executor results are collected):

```rust
// Reflexion: evaluate round and generate reflection on non-success.
if let Some(reflector) = reflector {
    let outcome = Reflector::evaluate_round(&tool_result_blocks);
    if !matches!(outcome, RoundOutcome::Success) {
        match reflector.reflect(round, &outcome, &messages).await {
            Ok(Some(reflection)) => {
                tracing::info!(
                    round,
                    analysis = %reflection.analysis,
                    "Self-reflection generated"
                );
                // Store as memory entry.
                if let Some(db) = trace_db {
                    let content = if reflection.advice.is_empty() {
                        reflection.analysis.clone()
                    } else {
                        format!("{}\nAdvice: {}", reflection.analysis, reflection.advice)
                    };
                    let _ = db.insert_memory_entry(
                        &uuid::Uuid::new_v4().to_string(),
                        Some(&session_id.to_string()),
                        "reflection",
                        &content,
                        None, // no expiry for reflections
                    ).await;
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("Reflection failed: {e}"),
        }
    }
}
```

#### `crates/cuervo-cli/src/repl/mod.rs`

In `Repl` struct — add field:
```rust
reflector: Option<Reflector>,
```

In `Repl::new()` — after planner setup:
```rust
let reflector = if config.reflexion.enabled {
    if let Some(p) = registry.get(&provider).cloned() {
        Some(Reflector::new(p, model.clone()))
    } else {
        None
    }
} else {
    None
};
```

In context_sources — add ReflectionSource:
```rust
if config.reflexion.enabled {
    if let Some(ref adb) = async_db {
        context_sources.push(Box::new(ReflectionSource::new(
            adb.clone(),
            config.reflexion.max_reflections,
        )));
    }
}
```

### 3.7 New Event Variant

```rust
// ADD to EventPayload:
ReflectionGenerated {
    round: usize,
    trigger: String,  // "partial", "failure"
},
```

### 3.8 Files Modified/Created

| File | Action |
|------|--------|
| `crates/cuervo-cli/src/repl/reflexion.rs` | CREATE — Reflector, RoundOutcome, Reflection |
| `crates/cuervo-cli/src/repl/reflection_source.rs` | CREATE — ReflectionSource (ContextSource) |
| `crates/cuervo-cli/src/repl/mod.rs` | MODIFY — add reflector field, wire ReflectionSource |
| `crates/cuervo-cli/src/repl/agent.rs` | MODIFY — add reflector param, wire reflection after tool results |
| `crates/cuervo-core/src/types/config.rs` | MODIFY — add ReflexionConfig |
| `crates/cuervo-core/src/types/event.rs` | MODIFY — add ReflectionGenerated |
| `crates/cuervo-storage/src/sqlite.rs` | MODIFY — search_memory_by_type |
| `crates/cuervo-storage/src/async_db.rs` | MODIFY — async wrapper |

### 3.9 Tests (10 new)

| Test | Location | Validates |
|------|----------|-----------|
| `evaluate_all_success` | reflexion.rs | All ToolResult is_error=false → Success |
| `evaluate_partial_failure` | reflexion.rs | Mixed results → Partial with failure list |
| `evaluate_all_failure` | reflexion.rs | All is_error=true → Failure |
| `reflection_json_parsing` | reflexion.rs | Valid JSON → Reflection struct |
| `reflection_fallback_on_bad_json` | reflexion.rs | Invalid JSON → raw text fallback |
| `reflection_source_priority` | reflection_source.rs | priority() == 85 |
| `reflection_source_name` | reflection_source.rs | name() == "reflections" |
| `search_memory_by_type_returns_typed` | sqlite.rs | Filters by entry_type correctly |
| `reflexion_config_defaults` | config.rs | enabled=false, max_reflections=3 |
| `agent_loop_without_reflector` | agent.rs | reflector=None → no behavior change |

### 3.10 Estimated Size

- New LOC: 600-1,000
- New tests: 10
- New files: 2 (reflexion.rs, reflection_source.rs)

---

## 4. Sub-Phase 8.3: TBAC Authorization

### 4.1 Goal

Implement Task-Based Authorization Control (TBAC) that scopes tool permissions to the current task context. Instead of blanket per-tool permissions, TBAC defines scoped authorization tokens with tool allowlists, parameter constraints, and temporal expiry. This replaces the flat `HashSet<String>` in `PermissionChecker` with a structured policy system.

### 4.2 New Types

#### `crates/cuervo-core/src/types/auth.rs` — NEW FILE

```rust
use std::collections::{HashMap, HashSet};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A task context that scopes tool authorization.
///
/// Each task context defines what tools are allowed, with what parameters,
/// and for how long. Contexts can be nested (a sub-task inherits from parent
/// but may have narrower scope).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    /// Unique ID for this task context.
    pub context_id: Uuid,
    /// Human-readable description of the task.
    pub task_description: String,
    /// Parent context ID (for sub-task scoping). None = root.
    pub parent_id: Option<Uuid>,
    /// Tools allowed in this context.
    pub allowed_tools: HashSet<String>,
    /// Parameter constraints per tool (tool_name → constraint).
    pub parameter_constraints: HashMap<String, ParameterConstraint>,
    /// When this context was created.
    pub created_at: DateTime<Utc>,
    /// When this context expires. None = session-scoped.
    pub expires_at: Option<DateTime<Utc>>,
    /// Maximum number of tool invocations under this context.
    pub max_invocations: Option<u32>,
    /// Number of invocations consumed.
    pub invocations_used: u32,
}

impl TaskContext {
    pub fn new(task_description: String, allowed_tools: HashSet<String>) -> Self {
        Self {
            context_id: Uuid::new_v4(),
            task_description,
            parent_id: None,
            allowed_tools,
            parameter_constraints: HashMap::new(),
            created_at: Utc::now(),
            expires_at: None,
            max_invocations: None,
            invocations_used: 0,
        }
    }

    /// Create a child context with a narrower scope.
    pub fn child(&self, task_description: String, allowed_tools: HashSet<String>) -> Self {
        // Child can only restrict, not expand — intersect with parent.
        let effective_tools: HashSet<String> = allowed_tools
            .intersection(&self.allowed_tools)
            .cloned()
            .collect();

        Self {
            context_id: Uuid::new_v4(),
            task_description,
            parent_id: Some(self.context_id),
            allowed_tools: effective_tools,
            parameter_constraints: self.parameter_constraints.clone(),
            created_at: Utc::now(),
            expires_at: self.expires_at,
            max_invocations: self.max_invocations,
            invocations_used: 0,
        }
    }

    /// Check if this context is still valid (not expired, not exhausted).
    pub fn is_valid(&self) -> bool {
        if let Some(expires) = self.expires_at {
            if Utc::now() > expires {
                return false;
            }
        }
        if let Some(max) = self.max_invocations {
            if self.invocations_used >= max {
                return false;
            }
        }
        true
    }

    /// Check if a tool is allowed under this context.
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        self.is_valid() && self.allowed_tools.contains(tool_name)
    }

    /// Check parameter constraints for a specific tool.
    pub fn check_params(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        if let Some(constraint) = self.parameter_constraints.get(tool_name) {
            constraint.check(args)
        } else {
            true // No constraints = all params allowed.
        }
    }

    /// Consume one invocation slot. Returns false if exhausted.
    pub fn consume_invocation(&mut self) -> bool {
        if let Some(max) = self.max_invocations {
            if self.invocations_used >= max {
                return false;
            }
        }
        self.invocations_used += 1;
        true
    }
}

/// Constraints on tool parameters (path restrictions, command allowlists).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParameterConstraint {
    /// Path must be within these directories.
    PathRestriction { allowed_dirs: Vec<String> },
    /// Command must match one of these glob patterns.
    CommandAllowlist { patterns: Vec<String> },
    /// Argument value must be one of these.
    ValueAllowlist { field: String, allowed: Vec<serde_json::Value> },
}

impl ParameterConstraint {
    pub fn check(&self, args: &serde_json::Value) -> bool {
        match self {
            ParameterConstraint::PathRestriction { allowed_dirs } => {
                if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                    allowed_dirs.iter().any(|dir| path.starts_with(dir))
                } else {
                    true // No path param = no restriction.
                }
            }
            ParameterConstraint::CommandAllowlist { patterns } => {
                if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                    patterns.iter().any(|p| {
                        glob::Pattern::new(p).map_or(false, |g| g.matches(cmd))
                    })
                } else {
                    true
                }
            }
            ParameterConstraint::ValueAllowlist { field, allowed } => {
                if let Some(val) = args.get(field) {
                    allowed.contains(val)
                } else {
                    true
                }
            }
        }
    }
}

/// Result of a TBAC authorization check.
#[derive(Debug, Clone)]
pub enum AuthzDecision {
    /// Allowed by active task context.
    Allowed { context_id: Uuid },
    /// Tool not in context allowlist.
    ToolNotAllowed { tool: String, context_id: Uuid },
    /// Parameter constraint violated.
    ParamViolation { tool: String, constraint: String },
    /// Context expired or exhausted.
    ContextInvalid { context_id: Uuid, reason: String },
    /// No active task context — fall back to legacy permission check.
    NoContext,
}
```

### 4.3 Migration 009: policy_decisions

```sql
-- Migration 009: TBAC policy decision audit trail.
CREATE TABLE IF NOT EXISTS policy_decisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    context_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    decision TEXT NOT NULL,          -- 'allowed', 'denied_tool', 'denied_param', 'denied_expired'
    reason TEXT,
    arguments_hash TEXT,             -- SHA-256 of args JSON (not raw args for privacy)
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_policy_session ON policy_decisions(session_id);
CREATE INDEX IF NOT EXISTS idx_policy_context ON policy_decisions(context_id);
CREATE INDEX IF NOT EXISTS idx_policy_created ON policy_decisions(created_at DESC);
```

### 4.4 Extend PermissionChecker

#### `crates/cuervo-cli/src/repl/permissions.rs`

```rust
// ADD fields to PermissionChecker:
pub struct PermissionChecker {
    always_allowed: HashSet<String>,
    confirm_destructive: bool,
    // NEW:
    /// Active task context stack (innermost = most restrictive).
    task_contexts: Vec<TaskContext>,
    /// Whether TBAC is enabled.
    tbac_enabled: bool,
}

// ADD methods:
impl PermissionChecker {
    /// Push a new task context (enters a scoped authorization).
    pub fn push_context(&mut self, ctx: TaskContext) {
        self.task_contexts.push(ctx);
    }

    /// Pop the current task context (exits scoped authorization).
    pub fn pop_context(&mut self) -> Option<TaskContext> {
        self.task_contexts.pop()
    }

    /// Get the active (innermost) task context.
    pub fn active_context(&self) -> Option<&TaskContext> {
        self.task_contexts.last()
    }

    /// Check TBAC authorization. Returns NoContext if TBAC disabled or no context active.
    pub fn check_tbac(&mut self, tool_name: &str, args: &serde_json::Value) -> AuthzDecision {
        if !self.tbac_enabled {
            return AuthzDecision::NoContext;
        }

        let Some(ctx) = self.task_contexts.last_mut() else {
            return AuthzDecision::NoContext;
        };

        if !ctx.is_valid() {
            return AuthzDecision::ContextInvalid {
                context_id: ctx.context_id,
                reason: "expired or exhausted".into(),
            };
        }

        if !ctx.is_tool_allowed(tool_name) {
            return AuthzDecision::ToolNotAllowed {
                tool: tool_name.into(),
                context_id: ctx.context_id,
            };
        }

        if !ctx.check_params(tool_name, args) {
            return AuthzDecision::ParamViolation {
                tool: tool_name.into(),
                constraint: format!("{:?}", ctx.parameter_constraints.get(tool_name)),
            };
        }

        ctx.consume_invocation();

        AuthzDecision::Allowed {
            context_id: ctx.context_id,
        }
    }
}
```

### 4.5 Wiring Points

#### `crates/cuervo-cli/src/repl/executor.rs`

In `execute_one_tool()` — BEFORE calling `tool.execute()`, add TBAC check:

```rust
// TBAC check (before legacy permission check).
if let AuthzDecision::ToolNotAllowed { tool, context_id }
    | AuthzDecision::ParamViolation { tool, .. }
    | AuthzDecision::ContextInvalid { context_id, .. } = permissions.check_tbac(&tool_call.name, &tool_call.input)
{
    // Log policy decision and return denied.
    tracing::info!(tool = %tool_call.name, "TBAC denied");
    return ToolExecResult {
        tool_use_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        content_block: ContentBlock::ToolResult {
            tool_use_id: tool_call.id.clone(),
            content: format!("Error: tool '{}' denied by task context policy", tool_call.name),
            is_error: true,
        },
        duration_ms: 0,
        was_parallel: false,
    };
}
```

#### `crates/cuervo-cli/src/repl/agent.rs`

At the start of the agent loop (after planning), derive a TaskContext from the plan if available:

```rust
// If adaptive planning produced a plan, create a TBAC context scoping to planned tools.
if let Some(ref plan) = active_plan {
    if permissions.tbac_enabled {
        let planned_tools: HashSet<String> = plan.steps.iter()
            .filter_map(|s| s.tool_name.clone())
            .collect();
        let ctx = TaskContext::new(plan.goal.clone(), planned_tools);
        permissions.push_context(ctx);
    }
}
// ... after loop ends ...
if active_plan.is_some() && permissions.tbac_enabled {
    permissions.pop_context();
}
```

### 4.6 Config Extension

```rust
// ADD to SecurityConfig:
#[serde(default)]
pub tbac_enabled: bool,
```

### 4.7 New Event Variant

```rust
// ADD to EventPayload:
PolicyDecision {
    tool: String,
    decision: String,  // "allowed", "denied_tool", "denied_param", "denied_expired"
    context_id: uuid::Uuid,
},
```

### 4.8 Files Modified/Created

| File | Action |
|------|--------|
| `crates/cuervo-core/src/types/auth.rs` | CREATE — TaskContext, ParameterConstraint, AuthzDecision |
| `crates/cuervo-core/src/types/mod.rs` | MODIFY — pub mod auth, pub use auth::* |
| `crates/cuervo-core/src/types/event.rs` | MODIFY — add PolicyDecision variant |
| `crates/cuervo-core/src/types/config.rs` | MODIFY — add tbac_enabled to SecurityConfig |
| `crates/cuervo-cli/src/repl/permissions.rs` | MODIFY — add TaskContext stack, check_tbac() |
| `crates/cuervo-cli/src/repl/executor.rs` | MODIFY — TBAC check before tool execution |
| `crates/cuervo-cli/src/repl/agent.rs` | MODIFY — push/pop context from plan |
| `crates/cuervo-storage/src/migrations.rs` | MODIFY — add migration 009 |
| `crates/cuervo-storage/src/sqlite.rs` | MODIFY — save_policy_decision() |
| `crates/cuervo-storage/src/async_db.rs` | MODIFY — async wrapper |

### 4.9 Tests (14 new)

| Test | Location | Validates |
|------|----------|-----------|
| `task_context_allows_listed_tools` | auth.rs | is_tool_allowed returns true for listed tools |
| `task_context_denies_unlisted_tools` | auth.rs | is_tool_allowed returns false for unlisted |
| `task_context_expiry` | auth.rs | Expired context → is_valid() = false |
| `task_context_invocation_limit` | auth.rs | Exhausted invocations → is_valid() = false |
| `child_context_intersects` | auth.rs | Child tools = intersection of parent + child |
| `path_constraint_restricts` | auth.rs | PathRestriction blocks paths outside allowed_dirs |
| `command_allowlist_filters` | auth.rs | CommandAllowlist only allows matching commands |
| `value_allowlist_checks` | auth.rs | ValueAllowlist blocks non-listed values |
| `check_tbac_no_context` | permissions.rs | No context → NoContext decision |
| `check_tbac_tool_denied` | permissions.rs | Tool not in context → ToolNotAllowed |
| `check_tbac_param_violation` | permissions.rs | Param constraint fails → ParamViolation |
| `check_tbac_allowed_consumes` | permissions.rs | Allowed → invocations_used incremented |
| `migration_009_creates_policy_table` | migrations.rs | Table + 3 indexes exist |
| `tbac_config_default_disabled` | config.rs | tbac_enabled defaults to false |

### 4.10 Estimated Size

- New LOC: 800-1,500
- New tests: 14
- New files: 1 (auth.rs)
- New migration: 1 (009)

---

## 5. Sub-Phase 8.4: Safety Guardrails

### 5.1 Goal

Implement a Guardrail trait for validating inputs to and outputs from the model, with a regex-based initial implementation. Guardrails run at two checkpoints: pre-invocation (input validation) and post-invocation (output validation). Violations can block, warn, or redact.

### 5.2 New Types

#### `crates/cuervo-security/src/guardrails.rs` — NEW FILE

```rust
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Result of a guardrail check.
#[derive(Debug, Clone)]
pub struct GuardrailResult {
    /// Name of the guardrail that triggered.
    pub guardrail: String,
    /// What was matched.
    pub matched: String,
    /// Action to take.
    pub action: GuardrailAction,
    /// Human-readable reason.
    pub reason: String,
}

/// Action to take when a guardrail triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardrailAction {
    /// Block the request/response entirely.
    Block,
    /// Warn but allow through.
    Warn,
    /// Redact the matched content and allow.
    Redact,
}

/// Checkpoint where the guardrail runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardrailCheckpoint {
    /// Before sending to the model (validates user input + context).
    PreInvocation,
    /// After receiving from the model (validates model output).
    PostInvocation,
    /// Both checkpoints.
    Both,
}

/// Trait for implementing guardrails.
pub trait Guardrail: Send + Sync {
    fn name(&self) -> &str;
    fn checkpoint(&self) -> GuardrailCheckpoint;
    fn check(&self, text: &str) -> Vec<GuardrailResult>;
}

/// Regex-based guardrail loaded from configuration.
pub struct RegexGuardrail {
    name: String,
    checkpoint: GuardrailCheckpoint,
    patterns: Vec<(Regex, GuardrailAction, String)>, // (pattern, action, reason)
}

impl RegexGuardrail {
    pub fn new(
        name: String,
        checkpoint: GuardrailCheckpoint,
        patterns: Vec<(Regex, GuardrailAction, String)>,
    ) -> Self {
        Self {
            name,
            checkpoint,
            patterns,
        }
    }

    /// Create a guardrail from config patterns.
    pub fn from_config(config: &GuardrailRuleConfig) -> Option<Self> {
        let checkpoint = match config.checkpoint.as_str() {
            "pre" => GuardrailCheckpoint::PreInvocation,
            "post" => GuardrailCheckpoint::PostInvocation,
            _ => GuardrailCheckpoint::Both,
        };

        let patterns: Vec<_> = config.patterns.iter().filter_map(|p| {
            let regex = Regex::new(&p.regex).ok()?;
            let action = match p.action.as_str() {
                "block" => GuardrailAction::Block,
                "redact" => GuardrailAction::Redact,
                _ => GuardrailAction::Warn,
            };
            Some((regex, action, p.reason.clone()))
        }).collect();

        if patterns.is_empty() {
            return None;
        }

        Some(Self::new(config.name.clone(), checkpoint, patterns))
    }
}

impl Guardrail for RegexGuardrail {
    fn name(&self) -> &str {
        &self.name
    }

    fn checkpoint(&self) -> GuardrailCheckpoint {
        self.checkpoint
    }

    fn check(&self, text: &str) -> Vec<GuardrailResult> {
        let mut results = Vec::new();
        for (regex, action, reason) in &self.patterns {
            for mat in regex.find_iter(text) {
                results.push(GuardrailResult {
                    guardrail: self.name.clone(),
                    matched: mat.as_str().to_string(),
                    action: *action,
                    reason: reason.clone(),
                });
            }
        }
        results
    }
}

/// Built-in guardrails that don't require configuration.
pub fn builtin_guardrails() -> Vec<Box<dyn Guardrail>> {
    vec![
        Box::new(PromptInjectionGuardrail::new()),
        Box::new(CodeInjectionGuardrail::new()),
    ]
}

/// Detects common prompt injection patterns.
struct PromptInjectionGuardrail {
    patterns: Vec<Regex>,
}

impl PromptInjectionGuardrail {
    fn new() -> Self {
        let patterns = vec![
            Regex::new(r"(?i)ignore\s+(all\s+)?previous\s+instructions").unwrap(),
            Regex::new(r"(?i)you\s+are\s+now\s+(a|an)\s+").unwrap(),
            Regex::new(r"(?i)system\s*:\s*you\s+are").unwrap(),
            Regex::new(r"(?i)disregard\s+(all\s+)?prior").unwrap(),
        ];
        Self { patterns }
    }
}

impl Guardrail for PromptInjectionGuardrail {
    fn name(&self) -> &str {
        "prompt_injection"
    }

    fn checkpoint(&self) -> GuardrailCheckpoint {
        GuardrailCheckpoint::PreInvocation
    }

    fn check(&self, text: &str) -> Vec<GuardrailResult> {
        self.patterns.iter().filter_map(|p| {
            p.find(text).map(|m| GuardrailResult {
                guardrail: self.name().into(),
                matched: m.as_str().to_string(),
                action: GuardrailAction::Warn,
                reason: "Potential prompt injection detected".into(),
            })
        }).collect()
    }
}

/// Detects dangerous code patterns in model output.
struct CodeInjectionGuardrail {
    patterns: Vec<(Regex, String)>,
}

impl CodeInjectionGuardrail {
    fn new() -> Self {
        let patterns = vec![
            (Regex::new(r"(?i)rm\s+-rf\s+/\s").unwrap(), "Destructive rm -rf / command".into()),
            (Regex::new(r"(?i):(){ :\|:& };:").unwrap(), "Fork bomb detected".into()),
            (Regex::new(r"(?i)mkfs\.\w+\s+/dev/").unwrap(), "Filesystem format command".into()),
            (Regex::new(r"(?i)dd\s+if=.*of=/dev/[sh]d").unwrap(), "Raw disk write detected".into()),
            (Regex::new(r"(?i)curl\s+.*\|\s*(ba)?sh").unwrap(), "Pipe to shell pattern".into()),
        ];
        Self { patterns }
    }
}

impl Guardrail for CodeInjectionGuardrail {
    fn name(&self) -> &str {
        "code_injection"
    }

    fn checkpoint(&self) -> GuardrailCheckpoint {
        GuardrailCheckpoint::PostInvocation
    }

    fn check(&self, text: &str) -> Vec<GuardrailResult> {
        self.patterns.iter().filter_map(|(p, reason)| {
            p.find(text).map(|m| GuardrailResult {
                guardrail: self.name().into(),
                matched: m.as_str().to_string(),
                action: GuardrailAction::Warn,
                reason: reason.clone(),
            })
        }).collect()
    }
}

/// Run all guardrails at a given checkpoint.
pub fn run_guardrails(
    guardrails: &[Box<dyn Guardrail>],
    text: &str,
    checkpoint: GuardrailCheckpoint,
) -> Vec<GuardrailResult> {
    guardrails.iter()
        .filter(|g| {
            g.checkpoint() == checkpoint || g.checkpoint() == GuardrailCheckpoint::Both
        })
        .flat_map(|g| g.check(text))
        .collect()
}

/// Check results for blocking violations.
pub fn has_blocking_violation(results: &[GuardrailResult]) -> bool {
    results.iter().any(|r| r.action == GuardrailAction::Block)
}
```

### 5.3 Config Extension

```rust
// ADD to SecurityConfig:
#[serde(default)]
pub guardrails: GuardrailsConfig,

// NEW structs:
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GuardrailsConfig {
    /// Enable guardrails.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Built-in guardrails (prompt injection, code injection).
    #[serde(default = "default_true")]
    pub builtins: bool,
    /// Custom regex-based guardrail rules.
    #[serde(default)]
    pub rules: Vec<GuardrailRuleConfig>,
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardrailRuleConfig {
    pub name: String,
    /// "pre", "post", or "both".
    pub checkpoint: String,
    pub patterns: Vec<GuardrailPatternConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardrailPatternConfig {
    pub regex: String,
    /// "block", "warn", or "redact".
    pub action: String,
    pub reason: String,
}
```

### 5.4 Wiring Points

#### `crates/cuervo-cli/src/repl/agent.rs`

**New parameter** (position 17):
```rust
guardrails: &[Box<dyn Guardrail>],  // NEW — position 17
```

**Pre-invocation check** — BEFORE `invoke_with_fallback()` (after building round_request, ~line 302):

```rust
// Guardrail pre-invocation check.
if !guardrails.is_empty() {
    let input_text = messages.iter()
        .filter(|m| m.role == Role::User)
        .last()
        .map(|m| match &m.content {
            MessageContent::Text(t) => t.as_str(),
            _ => "",
        })
        .unwrap_or("");

    let violations = run_guardrails(guardrails, input_text, GuardrailCheckpoint::PreInvocation);
    for v in &violations {
        tracing::warn!(guardrail = %v.guardrail, matched = %v.matched, "Guardrail triggered: {}", v.reason);
        let _ = event_tx.send(DomainEvent::new(EventPayload::GuardrailTriggered {
            guardrail: v.guardrail.clone(),
            checkpoint: "pre".into(),
            action: format!("{:?}", v.action),
        }));
    }
    if has_blocking_violation(&violations) {
        eprintln!("\n[blocked by guardrail]");
        break;
    }
}
```

**Post-invocation check** — AFTER model response text is collected:

```rust
// Guardrail post-invocation check on accumulated text.
if !guardrails.is_empty() && !round_text.is_empty() {
    let violations = run_guardrails(guardrails, &round_text, GuardrailCheckpoint::PostInvocation);
    for v in &violations {
        tracing::warn!(guardrail = %v.guardrail, matched = %v.matched, "Output guardrail: {}", v.reason);
        let _ = event_tx.send(DomainEvent::new(EventPayload::GuardrailTriggered {
            guardrail: v.guardrail.clone(),
            checkpoint: "post".into(),
            action: format!("{:?}", v.action),
        }));
    }
    // Blocking on output: discard the response and inform user.
    if has_blocking_violation(&violations) {
        eprintln!("\n[response blocked by guardrail]");
        break;
    }
}
```

#### `crates/cuervo-cli/src/repl/mod.rs`

In `Repl` struct — add field:
```rust
guardrails: Vec<Box<dyn Guardrail>>,
```

In `Repl::new()`:
```rust
let mut guardrails: Vec<Box<dyn Guardrail>> = Vec::new();
if config.security.guardrails.enabled {
    if config.security.guardrails.builtins {
        guardrails.extend(cuervo_security::guardrails::builtin_guardrails());
    }
    for rule_cfg in &config.security.guardrails.rules {
        if let Some(g) = RegexGuardrail::from_config(rule_cfg) {
            guardrails.push(Box::new(g));
        }
    }
}
```

### 5.5 New Event Variant

```rust
// ADD to EventPayload:
GuardrailTriggered {
    guardrail: String,
    checkpoint: String,  // "pre" or "post"
    action: String,      // "Block", "Warn", "Redact"
},
```

### 5.6 Files Modified/Created

| File | Action |
|------|--------|
| `crates/cuervo-security/src/guardrails.rs` | CREATE — Guardrail trait, RegexGuardrail, builtins |
| `crates/cuervo-security/src/lib.rs` | MODIFY — pub mod guardrails |
| `crates/cuervo-core/src/types/config.rs` | MODIFY — add GuardrailsConfig |
| `crates/cuervo-core/src/types/event.rs` | MODIFY — add GuardrailTriggered |
| `crates/cuervo-cli/src/repl/agent.rs` | MODIFY — add guardrails param, pre/post checks |
| `crates/cuervo-cli/src/repl/mod.rs` | MODIFY — add guardrails field, wire in new() |

### 5.7 Tests (12 new)

| Test | Location | Validates |
|------|----------|-----------|
| `prompt_injection_detects_ignore_instructions` | guardrails.rs | "ignore all previous instructions" → Warn |
| `prompt_injection_detects_system_override` | guardrails.rs | "system: you are" → Warn |
| `code_injection_detects_rm_rf` | guardrails.rs | "rm -rf /" → Warn |
| `code_injection_detects_fork_bomb` | guardrails.rs | Fork bomb → Warn |
| `code_injection_detects_pipe_to_shell` | guardrails.rs | "curl ... \| sh" → Warn |
| `regex_guardrail_from_config` | guardrails.rs | Config → RegexGuardrail construction |
| `regex_guardrail_matches` | guardrails.rs | Custom regex pattern matches |
| `run_guardrails_filters_checkpoint` | guardrails.rs | Only pre-invocation guardrails at PreInvocation |
| `has_blocking_violation_true` | guardrails.rs | Block action → true |
| `has_blocking_violation_false_on_warn` | guardrails.rs | Warn action → false |
| `builtin_guardrails_count` | guardrails.rs | builtin_guardrails() returns 2 |
| `guardrails_config_defaults` | config.rs | enabled=true, builtins=true, rules=empty |

### 5.8 Estimated Size

- New LOC: 500-800
- New tests: 12
- New files: 1 (guardrails.rs)

---

## 6. Sub-Phase 8.5: Episodic Memory

### 6.1 Goal

Add episodic memory that groups related memory entries into episodes (task-scoped sessions of work), implements embedding-based retrieval (via the existing EmbeddingProvider trait), and fuses BM25 + embedding scores using Reciprocal Rank Fusion (RRF) with temporal decay.

### 6.2 Architecture

```
Memory System Stack:
┌─────────────────────────────────────────────────────┐
│ ContextQuery (user_message + working_directory)      │
├─────────────────────────────────────────────────────┤
│ HybridRetriever                                      │
│   ├── BM25 search (FTS5) → ranked list A            │
│   ├── Embedding search (cosine sim) → ranked list B  │
│   └── RRF fusion (k=60) + temporal decay → final    │
├─────────────────────────────────────────────────────┤
│ memory_entries + memory_episodes + memory_fts        │
│ memory_entry_episodes (join table)                   │
└─────────────────────────────────────────────────────┘
```

### 6.3 Migration 010: episodic memory

```sql
-- Migration 010: Episodic memory — group entries into task episodes.
CREATE TABLE IF NOT EXISTS memory_episodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    episode_id TEXT NOT NULL UNIQUE,
    session_id TEXT,
    title TEXT NOT NULL,
    summary TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_episode_session ON memory_episodes(session_id);
CREATE INDEX IF NOT EXISTS idx_episode_started ON memory_episodes(started_at DESC);

-- Join table: many-to-many between entries and episodes.
CREATE TABLE IF NOT EXISTS memory_entry_episodes (
    entry_id INTEGER NOT NULL REFERENCES memory_entries(id) ON DELETE CASCADE,
    episode_id INTEGER NOT NULL REFERENCES memory_episodes(id) ON DELETE CASCADE,
    position INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (entry_id, episode_id)
);

CREATE INDEX IF NOT EXISTS idx_entry_episode ON memory_entry_episodes(episode_id);
```

### 6.4 New Types

#### `crates/cuervo-storage/src/types.rs` — Episode types

```rust
// ADD:
#[derive(Debug, Clone)]
pub struct MemoryEpisode {
    pub episode_id: String,
    pub session_id: Option<String>,
    pub title: String,
    pub summary: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub metadata: serde_json::Value,
}
```

#### `crates/cuervo-cli/src/repl/hybrid_retriever.rs` — NEW FILE

```rust
use chrono::Utc;

use cuervo_core::error::Result;
use cuervo_core::traits::{Embedding, EmbeddingProvider};
use cuervo_storage::AsyncDatabase;

/// Reciprocal Rank Fusion retriever combining BM25 (FTS5) and embedding similarity.
pub struct HybridRetriever {
    db: AsyncDatabase,
    embedding_provider: Option<Box<dyn EmbeddingProvider>>,
    /// RRF constant k (default: 60, per Cormack et al.).
    rrf_k: f64,
    /// Temporal decay half-life in days.
    decay_half_life_days: f64,
}

impl HybridRetriever {
    pub fn new(db: AsyncDatabase) -> Self {
        Self {
            db,
            embedding_provider: None,
            rrf_k: 60.0,
            decay_half_life_days: 30.0,
        }
    }

    pub fn with_embedding_provider(mut self, provider: Box<dyn EmbeddingProvider>) -> Self {
        self.embedding_provider = Some(provider);
        self
    }

    pub fn with_rrf_k(mut self, k: f64) -> Self {
        self.rrf_k = k;
        self
    }

    pub fn with_decay_half_life(mut self, days: f64) -> Self {
        self.decay_half_life_days = days;
        self
    }

    /// Retrieve and rank memory entries using hybrid BM25 + embedding search.
    pub async fn retrieve(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<ScoredEntry>> {
        // Phase 1: BM25 search via FTS5.
        let bm25_results = self.db.search_memory_bm25(query, top_k * 3).await?;

        // Phase 2: Embedding search (if provider available).
        let embedding_results = if let Some(ref embedder) = self.embedding_provider {
            let query_embedding = embedder.embed(query).await?;
            self.db.search_memory_by_embedding(&query_embedding.values, top_k * 3).await?
        } else {
            vec![]
        };

        // Phase 3: RRF fusion.
        let mut scored: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();
        let mut entry_map: std::collections::HashMap<i64, cuervo_storage::MemoryEntry> =
            std::collections::HashMap::new();

        // BM25 scores.
        for (rank, entry) in bm25_results.iter().enumerate() {
            let rrf_score = 1.0 / (self.rrf_k + rank as f64 + 1.0);
            *scored.entry(entry.id).or_insert(0.0) += rrf_score;
            entry_map.entry(entry.id).or_insert_with(|| entry.clone());
        }

        // Embedding scores.
        for (rank, entry) in embedding_results.iter().enumerate() {
            let rrf_score = 1.0 / (self.rrf_k + rank as f64 + 1.0);
            *scored.entry(entry.id).or_insert(0.0) += rrf_score;
            entry_map.entry(entry.id).or_insert_with(|| entry.clone());
        }

        // Phase 4: Apply temporal decay.
        let now = Utc::now();
        let mut results: Vec<ScoredEntry> = scored.into_iter()
            .filter_map(|(id, rrf_score)| {
                let entry = entry_map.remove(&id)?;
                let age_days = (now - entry.created_at).num_days().max(0) as f64;
                let decay = (-age_days.ln() * 2.0 / self.decay_half_life_days).exp();
                let final_score = rrf_score * decay;
                Some(ScoredEntry {
                    entry,
                    score: final_score,
                })
            })
            .collect();

        // Sort by score descending, take top_k.
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);

        Ok(results)
    }
}

/// A memory entry with a computed relevance score.
#[derive(Debug, Clone)]
pub struct ScoredEntry {
    pub entry: cuervo_storage::MemoryEntry,
    pub score: f64,
}
```

#### `crates/cuervo-cli/src/repl/episodic_source.rs` — NEW FILE (replaces MemorySource when episodic enabled)

```rust
use async_trait::async_trait;

use cuervo_core::error::Result;
use cuervo_core::traits::{ContextChunk, ContextQuery, ContextSource};

use super::hybrid_retriever::HybridRetriever;
use cuervo_context::assembler::estimate_tokens;

/// ContextSource that uses hybrid retrieval (BM25 + embedding + RRF).
///
/// Priority 80: same as existing MemorySource (replaces it when episodic enabled).
pub struct EpisodicSource {
    retriever: HybridRetriever,
    top_k: usize,
    token_budget: usize,
}

impl EpisodicSource {
    pub fn new(retriever: HybridRetriever, top_k: usize, token_budget: usize) -> Self {
        Self {
            retriever,
            top_k,
            token_budget,
        }
    }
}

#[async_trait]
impl ContextSource for EpisodicSource {
    fn name(&self) -> &str {
        "episodic_memory"
    }

    fn priority(&self) -> u32 {
        80
    }

    async fn gather(&self, query: &ContextQuery) -> Result<Vec<ContextChunk>> {
        let search_query = query.user_message.as_deref().unwrap_or("");
        if search_query.is_empty() {
            return Ok(vec![]);
        }

        let results = self.retriever.retrieve(search_query, self.top_k).await?;
        if results.is_empty() {
            return Ok(vec![]);
        }

        // Build context chunk respecting token budget.
        let mut content = String::from("## Relevant Memories\n\n");
        let mut total_tokens = estimate_tokens(&content);

        for scored in &results {
            let entry_text = format!(
                "- [{}] (score: {:.3}) {}\n",
                scored.entry.entry_type,
                scored.score,
                scored.entry.content,
            );
            let entry_tokens = estimate_tokens(&entry_text);
            if total_tokens + entry_tokens > self.token_budget {
                break;
            }
            content.push_str(&entry_text);
            total_tokens += entry_tokens;
        }

        Ok(vec![ContextChunk {
            source: "episodic_memory".into(),
            priority: self.priority(),
            content,
            estimated_tokens: total_tokens,
        }])
    }
}
```

### 6.5 Storage Layer Extensions

#### `crates/cuervo-storage/src/sqlite.rs`

```rust
/// BM25 search returning entries (existing, but formalized).
pub fn search_memory_bm25(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

/// Cosine similarity search on embedding vectors.
/// Requires entries to have non-null embedding BLOB.
pub fn search_memory_by_embedding(&self, query_vec: &[f32], limit: usize) -> Result<Vec<MemoryEntry>>;

/// Save a memory episode.
pub fn save_episode(&self, episode: &MemoryEpisode) -> Result<()>;

/// Link a memory entry to an episode.
pub fn link_entry_to_episode(&self, entry_id: i64, episode_id: &str, position: u32) -> Result<()>;

/// Load entries for an episode.
pub fn load_episode_entries(&self, episode_id: &str) -> Result<Vec<MemoryEntry>>;

/// Update embedding for a memory entry.
pub fn update_entry_embedding(&self, entry_id: i64, embedding: &[f32], model: &str) -> Result<()>;
```

#### Embedding storage format

Embeddings stored as BLOB in `memory_entries.embedding`:
```rust
// Store: f32 vec → little-endian bytes
let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();

// Load: bytes → f32 vec
let floats: Vec<f32> = bytes.chunks_exact(4)
    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
    .collect();
```

#### Cosine similarity in SQLite

Since SQLite has no built-in vector operations, cosine similarity is computed in Rust after loading candidates:

```rust
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { 0.0 } else { dot / (norm_a * norm_b) }
}

// In search_memory_by_embedding:
// 1. Load all entries with non-null embedding
// 2. Compute cosine_similarity(query_vec, entry_embedding) for each
// 3. Sort by similarity descending
// 4. Take top limit
```

**Note**: For the initial implementation, this full-scan approach is acceptable given typical memory sizes (< 10k entries). Future optimization: add an approximate nearest neighbor index or use SQLite's vector extension when stable.

### 6.6 Config Extension

```rust
// ADD to MemoryConfig:
/// Enable episodic memory (groups entries into episodes with hybrid retrieval).
#[serde(default)]
pub episodic: bool,
/// Temporal decay half-life in days for relevance scoring.
#[serde(default = "default_decay_half_life")]
pub decay_half_life_days: f64,
/// RRF fusion constant k.
#[serde(default = "default_rrf_k")]
pub rrf_k: f64,

fn default_decay_half_life() -> f64 { 30.0 }
fn default_rrf_k() -> f64 { 60.0 }
```

### 6.7 Wiring Points

#### `crates/cuervo-cli/src/repl/mod.rs`

In context_sources construction, replace MemorySource with EpisodicSource when episodic is enabled:

```rust
if config.memory.enabled {
    if let Some(ref adb) = async_db {
        if config.memory.episodic {
            // Episodic memory with hybrid retrieval.
            let retriever = HybridRetriever::new(adb.clone())
                .with_rrf_k(config.memory.rrf_k)
                .with_decay_half_life(config.memory.decay_half_life_days);
            // TODO (Phase 8.5+): wire EmbeddingProvider when one is implemented.
            context_sources.push(Box::new(EpisodicSource::new(
                retriever,
                config.memory.retrieval_top_k,
                config.memory.retrieval_token_budget,
            )));
        } else {
            // Legacy BM25-only memory source.
            context_sources.push(Box::new(MemorySource::new(
                adb.clone(),
                config.memory.retrieval_top_k,
                config.memory.retrieval_token_budget,
            )));
        }
    }
}
```

#### Episode lifecycle in agent loop

At agent loop start (after planning), create an episode:
```rust
// Create episode for this task.
if config.memory.episodic {
    if let Some(db) = trace_db {
        let episode = MemoryEpisode {
            episode_id: uuid::Uuid::new_v4().to_string(),
            session_id: Some(session_id.to_string()),
            title: user_msg.chars().take(100).collect(),
            summary: None,
            started_at: Utc::now(),
            ended_at: None,
            metadata: serde_json::json!({}),
        };
        let _ = db.save_episode(&episode).await;
    }
}
```

### 6.8 New Event Variant

```rust
// ADD to EventPayload:
EpisodeCreated {
    episode_id: String,
    title: String,
},
MemoryRetrieved {
    query: String,
    result_count: usize,
    top_score: f64,
},
```

### 6.9 Files Modified/Created

| File | Action |
|------|--------|
| `crates/cuervo-cli/src/repl/hybrid_retriever.rs` | CREATE — HybridRetriever, RRF, temporal decay |
| `crates/cuervo-cli/src/repl/episodic_source.rs` | CREATE — EpisodicSource (ContextSource) |
| `crates/cuervo-cli/src/repl/mod.rs` | MODIFY — conditional EpisodicSource vs MemorySource |
| `crates/cuervo-core/src/types/config.rs` | MODIFY — episodic, decay, rrf_k fields in MemoryConfig |
| `crates/cuervo-core/src/types/event.rs` | MODIFY — EpisodeCreated, MemoryRetrieved |
| `crates/cuervo-storage/src/migrations.rs` | MODIFY — add migration 010 |
| `crates/cuervo-storage/src/types.rs` | MODIFY — MemoryEpisode struct |
| `crates/cuervo-storage/src/sqlite.rs` | MODIFY — episode CRUD, embedding search, cosine similarity |
| `crates/cuervo-storage/src/async_db.rs` | MODIFY — async wrappers |

### 6.10 Tests (16 new)

| Test | Location | Validates |
|------|----------|-----------|
| `rrf_combines_bm25_and_embedding` | hybrid_retriever.rs | Two ranked lists → correct fused order |
| `rrf_bm25_only_when_no_embeddings` | hybrid_retriever.rs | No embedding provider → BM25-only |
| `temporal_decay_recent_preferred` | hybrid_retriever.rs | Recent entries score higher than old |
| `temporal_decay_half_life` | hybrid_retriever.rs | 30-day-old entry scores ~50% of fresh |
| `top_k_limits_results` | hybrid_retriever.rs | retrieve(query, 5) returns ≤ 5 |
| `episodic_source_priority` | episodic_source.rs | priority() == 80 |
| `episodic_source_empty_query` | episodic_source.rs | Empty query → empty chunks |
| `episodic_source_respects_budget` | episodic_source.rs | Token budget limits included entries |
| `cosine_similarity_identical` | sqlite.rs | Same vector → 1.0 |
| `cosine_similarity_orthogonal` | sqlite.rs | Orthogonal → 0.0 |
| `cosine_similarity_opposite` | sqlite.rs | Negated → -1.0 |
| `embedding_roundtrip` | sqlite.rs | Store f32 → load f32 matches |
| `save_and_load_episode` | sqlite.rs | Episode CRUD round-trip |
| `link_entry_to_episode` | sqlite.rs | Link + load_episode_entries works |
| `migration_010_creates_episode_tables` | migrations.rs | Tables + indexes exist |
| `memory_config_episodic_defaults` | config.rs | episodic=false, decay=30.0, rrf_k=60.0 |

### 6.11 Estimated Size

- New LOC: 1,500-2,500
- New tests: 16
- New files: 2 (hybrid_retriever.rs, episodic_source.rs)
- New migration: 1 (010)

---

## 7. Agent Loop Refactoring

### 7.1 Problem

`run_agent_loop` currently takes 14 positional parameters. Sub-phases 8.1-8.5 add 3 more (planner, reflector, guardrails), reaching 17. This is unsustainable.

### 7.2 Solution: AgentContext Struct

Extract parameters into a context struct. This is a **prerequisite** refactoring done at the START of Phase 8.1.

```rust
/// Bundled configuration and dependencies for the agent loop.
pub struct AgentContext<'a> {
    // Core (always required):
    pub provider: &'a Arc<dyn ModelProvider>,
    pub session: &'a mut Session,
    pub request: &'a ModelRequest,
    pub tool_registry: &'a ToolRegistry,
    pub permissions: &'a mut PermissionChecker,
    pub working_dir: &'a str,
    pub event_tx: &'a EventSender,
    pub limits: &'a AgentLimits,

    // Infrastructure (optional):
    pub trace_db: Option<&'a AsyncDatabase>,
    pub response_cache: Option<&'a ResponseCache>,
    pub resilience: &'a mut ResilienceManager,
    pub fallback_providers: &'a [(String, Arc<dyn ModelProvider>)],
    pub routing_config: &'a RoutingConfig,
    pub compactor: Option<&'a ContextCompactor>,

    // Phase 8 additions (optional):
    pub planner: Option<&'a dyn Planner>,
    pub reflector: Option<&'a Reflector>,
    pub guardrails: &'a [Box<dyn Guardrail>],
}
```

**New signature**:
```rust
pub async fn run_agent_loop(ctx: AgentContext<'_>) -> Result<AgentLoopResult>
```

**Migration strategy**:
1. Create `AgentContext` struct in `agent.rs`.
2. Change `run_agent_loop` to accept `AgentContext` instead of 14+ positional params.
3. Update `handle_message()` in `mod.rs` to construct `AgentContext`.
4. Update ALL test call sites (currently ~16 tests) to construct `AgentContext`.
5. Phase 8 additions just add `Option` fields with `Default`-like values.

### 7.3 Test Update Strategy

Create a helper function for tests:
```rust
#[cfg(test)]
pub fn test_agent_context<'a>(
    provider: &'a Arc<dyn ModelProvider>,
    session: &'a mut Session,
    request: &'a ModelRequest,
    tool_registry: &'a ToolRegistry,
    permissions: &'a mut PermissionChecker,
    working_dir: &'a str,
    event_tx: &'a EventSender,
    limits: &'a AgentLimits,
) -> AgentContext<'a> {
    AgentContext {
        provider, session, request, tool_registry, permissions,
        working_dir, event_tx, limits,
        trace_db: None,
        response_cache: None,
        resilience: /* ... */,
        fallback_providers: &[],
        routing_config: &RoutingConfig::default(),
        compactor: None,
        planner: None,
        reflector: None,
        guardrails: &[],
    }
}
```

---

## 8. Migration Summary

| Migration | Version | Name | Tables | Indexes |
|-----------|---------|------|--------|---------|
| 008 | 8 | adaptive_planning | planning_steps | 3 |
| 009 | 9 | tbac_policy | policy_decisions | 3 |
| 010 | 10 | episodic_memory | memory_episodes, memory_entry_episodes | 3 |

Total new tables: 4
Total new indexes: 9
All backward compatible (new tables only, no ALTER).

---

## 9. Config Extensions

### Full AppConfig delta

```rust
pub struct AppConfig {
    // EXISTING (unchanged):
    pub general: GeneralConfig,
    pub models: ModelsConfig,
    pub tools: ToolsConfig,
    pub security: SecurityConfig,    // MODIFIED: +tbac_enabled, +guardrails
    pub storage: StorageConfig,
    pub logging: LoggingConfig,
    pub mcp: McpConfig,
    pub agent: AgentConfig,
    pub memory: MemoryConfig,        // MODIFIED: +episodic, +decay_half_life_days, +rrf_k
    pub planning: PlanningConfig,    // MODIFIED: +adaptive, +max_replans, +min_confidence
    pub cache: CacheConfig,
    pub resilience: ResilienceConfig,
    // NEW:
    #[serde(default)]
    pub reflexion: ReflexionConfig,
}
```

### Config validation additions

```rust
// ADD to validate_config():
// Reflexion enabled but planning disabled (reflexion works better with planning).
if config.reflexion.enabled && !config.planning.adaptive {
    issues.push(ConfigIssue {
        level: IssueLevel::Warning,
        field: "reflexion + planning".into(),
        message: "reflexion is enabled but adaptive planning is disabled".into(),
        suggestion: Some("Enable planning.adaptive for best reflexion results".into()),
    });
}

// TBAC enabled but confirm_destructive is also enabled (redundant).
if config.security.tbac_enabled && config.tools.confirm_destructive {
    issues.push(ConfigIssue {
        level: IssueLevel::Warning,
        field: "security.tbac_enabled + tools.confirm_destructive".into(),
        message: "both TBAC and legacy destructive confirmation are enabled".into(),
        suggestion: Some("Consider disabling confirm_destructive when TBAC is active".into()),
    });
}
```

---

## 10. Execution Order

```
                     ┌──── 8.4 Guardrails (independent)
                     │
8.0 AgentContext ────┤
  (refactoring)      ├──── 8.1 Adaptive Planner ──→ 8.2 Reflexion (uses plan outcomes)
                     │                                    ↓
                     └──── 8.3 TBAC (uses plan)     8.5 Episodic Memory
                                                    (independent, but benefits
                                                     from reflections stored)
```

**Execution sequence**: **8.0** → **8.1** → **8.4** → **8.3** → **8.2** → **8.5**

**Rationale**:
- **8.0** first: refactor agent.rs params → AgentContext (prevents param explosion)
- **8.1** next: core planner wiring, foundation for 8.2 and 8.3
- **8.4** before 8.3: guardrails are simpler, no DB, pure validation — quick win
- **8.3** after 8.1: TBAC derives scopes from plans
- **8.2** after 8.1: reflexion evaluates plan step outcomes
- **8.5** last: episodic memory is the most complex, benefits from all prior sub-phases

---

## 11. Test Strategy

### Per sub-phase quality gates

1. `cargo test --workspace` — all tests pass
2. `cargo clippy --workspace -- -D warnings` — zero warnings
3. Zero `unwrap()` in new production code
4. Zero `block_on()` anywhere
5. All new code async-safe
6. Backward compatible: `serde(default)` on all new config fields, migration-safe

### Test summary

| Sub-Phase | New Tests | Cumulative |
|-----------|-----------|------------|
| 8.0 (refactoring) | 0 (update existing) | 593 |
| 8.1 Adaptive Planner | 12 | 605 |
| 8.2 Reflexion | 10 | 615 |
| 8.3 TBAC | 14 | 629 |
| 8.4 Guardrails | 12 | 641 |
| 8.5 Episodic Memory | 16 | 657 |
| **Total** | **64** | **657** |

### Integration testing (cross-sub-phase)

After all sub-phases complete, add 4 integration tests:

1. **plan_to_tbac_scope** — Planning generates a plan → TBAC auto-scopes to planned tools → tool outside plan is denied
2. **reflexion_improves_retry** — Tool fails → reflection stored → next similar query retrieves reflection
3. **guardrail_blocks_injection** — Prompt injection attempt → guardrail blocks → no model invocation
4. **episodic_retrieval_ranking** — Insert entries at different times → hybrid retrieval prefers recent + relevant

---

## 12. Risk Mitigations

| Risk | Mitigation |
|------|------------|
| LLM plan JSON parsing failures | Strict validation + fallback to no-plan (agent.rs continues normally) |
| Reflection prompt cost (extra invocations) | Default disabled; reflect only on non-success; 1024 token cap |
| TBAC context mismatch with plan | Intersection semantics (child never expands parent); NoContext = legacy behavior |
| Guardrail false positives | Default action = Warn (not Block); user can customize patterns |
| Embedding search full-scan cost | Cap candidates at 1000; add TODO for ANN index; SQLite vector ext in future |
| Agent loop param count | AgentContext refactoring in 8.0 (prerequisite) |
| Test call site churn | test_agent_context helper + Default-populated optional fields |
| Binary size growth | No new heavy deps; reuse regex, sha2, serde_json; target ≤ 6.0MB |

---

## Appendix: New Module Map

```
crates/cuervo-cli/src/repl/
├── agent.rs              ← MODIFIED (AgentContext, planner, reflector, guardrails wiring)
├── planner.rs            ← NEW (LlmPlanner)
├── reflexion.rs          ← NEW (Reflector, RoundOutcome)
├── reflection_source.rs  ← NEW (ReflectionSource — ContextSource)
├── hybrid_retriever.rs   ← NEW (HybridRetriever, RRF)
├── episodic_source.rs    ← NEW (EpisodicSource — ContextSource)
├── executor.rs           ← MODIFIED (TBAC check)
├── permissions.rs        ← MODIFIED (TaskContext stack, check_tbac)
├── mod.rs                ← MODIFIED (new fields, conditional source wiring)
└── ... (existing, unchanged)

crates/cuervo-core/src/types/
├── auth.rs               ← NEW (TaskContext, ParameterConstraint, AuthzDecision)
├── config.rs             ← MODIFIED (PlanningConfig, ReflexionConfig, GuardrailsConfig, MemoryConfig)
├── event.rs              ← MODIFIED (6 new EventPayload variants)
└── mod.rs                ← MODIFIED (pub mod auth)

crates/cuervo-security/src/
├── guardrails.rs         ← NEW (Guardrail trait, RegexGuardrail, builtins)
└── lib.rs                ← MODIFIED (pub mod guardrails)

crates/cuervo-storage/src/
├── migrations.rs         ← MODIFIED (migrations 008-010)
├── sqlite.rs             ← MODIFIED (plan steps, policy, episodes, embeddings)
├── async_db.rs           ← MODIFIED (async wrappers)
└── types.rs              ← MODIFIED (MemoryEpisode, PlanStepRow)

crates/cuervo-core/src/
├── error.rs              ← MODIFIED (PlanningFailed variant)
└── traits/planner.rs     ← MODIFIED (StepOutcome, replan(), extended fields)
```
