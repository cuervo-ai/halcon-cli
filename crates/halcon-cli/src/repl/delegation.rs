//! Delegation router: maps plan steps to sub-agent tasks for orchestrator execution.
//!
//! Analyzes an `ExecutionPlan` and decides which steps can be delegated to
//! specialized sub-agents via the existing `run_orchestrator()` infrastructure.

use std::collections::HashSet;

use uuid::Uuid;

use halcon_core::traits::{ExecutionPlan, PlanStep};
use halcon_core::types::{AgentType, SubAgentTask};

/// Capability profile for routing plan steps to appropriate sub-agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StepCapability {
    /// file_read, file_write, file_edit, file_delete, file_inspect, directory_tree
    FileOperations,
    /// bash
    CodeExecution,
    /// grep, glob, fuzzy_find, symbol_search
    Search,
    /// git_status, git_diff, git_log, git_add, git_commit
    GitOperations,
    /// web_search, web_fetch, http_request
    WebAccess,
    /// No specific capability needed (synthesis, general reasoning).
    General,
}

/// Delegation decision for a single plan step.
pub(crate) struct DelegationDecision {
    /// Whether this step should be delegated.
    #[allow(dead_code)]
    pub delegate: bool,
    /// Detected capability category.
    pub capability: StepCapability,
    /// Tool names suggested for the sub-agent's `allowed_tools`.
    pub suggested_tools: HashSet<String>,
    /// Human-readable reason for the decision.
    #[allow(dead_code)]
    pub reason: String,
}

/// Routes plan steps to sub-agent tasks based on capability matching heuristics.
pub(crate) struct DelegationRouter {
    /// Minimum confidence threshold to consider delegation.
    min_confidence: f64,
    /// Whether delegation is enabled.
    enabled: bool,
}

impl DelegationRouter {
    pub fn new(enabled: bool) -> Self {
        Self {
            min_confidence: 0.7,
            enabled,
        }
    }

    pub fn with_min_confidence(mut self, confidence: f64) -> Self {
        self.min_confidence = confidence;
        self
    }

    /// Analyze a plan and decide which steps should be delegated.
    ///
    /// Returns `(step_index, DelegationDecision)` pairs for delegatable steps.
    pub fn analyze_plan(&self, plan: &ExecutionPlan) -> Vec<(usize, DelegationDecision)> {
        if !self.enabled {
            return Vec::new();
        }

        // Don't delegate plans with fewer than 2 steps — a single-step plan has no
        // parallelism benefit and is cheaper to run inline.
        if plan.steps.len() < 2 {
            return Vec::new();
        }

        let last_index = plan.steps.len().saturating_sub(1);

        plan.steps
            .iter()
            .enumerate()
            .filter_map(|(i, step)| {
                // Skip synthesis steps (last step with no tool_name).
                if i == last_index && step.tool_name.is_none() {
                    return None;
                }

                // Must have a specific tool_name.
                // Defense: DeepSeek sometimes emits "null" string; treat as None.
                let tool_name = step.tool_name.as_deref()
                    .filter(|t| *t != "null" && !t.is_empty())?;

                // Must meet confidence threshold.
                if step.confidence < self.min_confidence {
                    return None;
                }

                // Already has an outcome — skip.
                if step.outcome.is_some() {
                    return None;
                }

                let capability = Self::classify_step(step);
                let suggested_tools = Self::tools_for_capability(&capability, tool_name);

                Some((
                    i,
                    DelegationDecision {
                        delegate: true,
                        capability,
                        suggested_tools,
                        reason: format!("tool '{tool_name}' eligible for delegation"),
                    },
                ))
            })
            .collect()
    }

    /// Convert delegation decisions to `SubAgentTask`s for the orchestrator.
    ///
    /// Returns `(step_index, SubAgentTask)` pairs preserving the step→task mapping.
    pub fn build_tasks(
        &self,
        plan: &ExecutionPlan,
        decisions: &[(usize, DelegationDecision)],
        parent_model: &str,
    ) -> Vec<(usize, SubAgentTask)> {
        // Pre-compute task IDs for dependency resolution.
        let task_ids: Vec<(usize, Uuid)> = decisions
            .iter()
            .map(|(idx, _)| (*idx, Uuid::new_v4()))
            .collect();

        decisions
            .iter()
            .enumerate()
            .filter_map(|(di, (step_idx, decision))| {
                // Audit fix: bounds-check before indexing to avoid panic on stale decisions.
                let Some(step) = plan.steps.get(*step_idx) else {
                    tracing::warn!(
                        step_idx = *step_idx,
                        total_steps = plan.steps.len(),
                        "build_tasks: step index out of bounds — skipping delegation decision"
                    );
                    return None;
                };

                // Determine dependencies: sequential steps depend on the previous delegated step.
                let depends_on = if !step.parallel && di > 0 {
                    vec![task_ids[di - 1].1]
                } else {
                    vec![]
                };

                // Prefix instruction with explicit tool-use directive so the LLM
                // calls the tool immediately instead of describing what it will do.
                // Without this, models like deepseek-chat return planning text first
                // (end_turn), which gets cached and permanently blocks tool execution.
                //
                // For file_write: include the target file path so the sub-agent knows
                // WHERE to write. Without a path, models generate the content as text
                // (end_turn) instead of calling file_write, resulting in 0 tools used.
                let instruction = if let Some(ref tool) = step.tool_name {
                    // MCP search_files → remap instruction to use grep natively.
                    // search_files MCP times out on large directories; grep is reliable.
                    //
                    // dep_check → remap to read_multiple_files.
                    // dep_check fails 100% of the time (17s then fails, 3 consecutive sessions).
                    // Alternative: read Cargo.lock / package.json / requirements.txt directly.
                    let effective_tool = if tool == "search_files" {
                        "grep"
                    } else if tool == "dep_check" {
                        "read_multiple_files"
                    } else {
                        tool.as_str()
                    };

                    let path_hint = if effective_tool == "file_write" {
                        // Try to get path from expected_args first.
                        let path = step.expected_args
                            .as_ref()
                            .and_then(|a| a.get("path").and_then(|p| p.as_str()).map(|s| s.to_string()))
                            .unwrap_or_else(|| Self::infer_file_path(&step.description));
                        format!(
                            "\nTarget file path: {path}\n\
                             Call file_write with path=\"{path}\" and the complete file content."
                        )
                    } else if tool == "search_files" {
                        // Hint: guide the sub-agent to use grep correctly for content search.
                        "\nUse grep with -r -l flags to search file contents recursively. \
                         Example: grep -r -l \"keyword\" /path/to/dir".to_string()
                    } else if tool == "dep_check" {
                        // dep_check is unreliable — remap to reading dependency files directly.
                        // Evidence: 3/3 failures in production (sessions Mon-Key, demo-js).
                        "\nRead dependency files directly to analyze dependencies:\n\
                         1. Use read_multiple_files to read: Cargo.lock, Cargo.toml, package.json, \
                         package-lock.json, requirements.txt, go.mod (whichever exist).\n\
                         2. Analyze the dependency versions and flag any outdated or suspicious packages.\n\
                         Do NOT attempt to run dep_check or any audit command.".to_string()
                    } else {
                        String::new()
                    };
                    format!(
                        "IMPORTANT: Call the `{effective_tool}` tool NOW to complete this task. \
                         Do NOT describe, plan, or explain — execute the tool immediately.{path_hint}\n\n\
                         Task: {}",
                        step.description
                    )
                } else {
                    step.description.clone()
                };

                let task = SubAgentTask {
                    task_id: task_ids[di].1,
                    instruction,
                    agent_type: Self::agent_type_for_capability(&decision.capability),
                    model: Some(parent_model.to_string()),
                    provider: None,
                    allowed_tools: decision.suggested_tools.clone(),
                    limits_override: None,
                    depends_on,
                    priority: 0,
                system_prompt_prefix: None,
            role: halcon_core::types::AgentRole::default(),
            team_id: None,
            mailbox_id: None,
                };

                Some((*step_idx, task))
            })
            .collect()
    }

    /// Classify a plan step's capability from its `tool_name`.
    ///
    /// When a tool_name is not in the match list it falls through to General,
    /// which maps to AgentType::Chat and provides no tool-surface narrowing.
    /// All known tools should be listed explicitly to get the correct agent type.
    fn classify_step(step: &PlanStep) -> StepCapability {
        let tool = match step.tool_name.as_deref() {
            Some(t) => t,
            None => return StepCapability::General,
        };

        match tool {
            // File and directory operations (Coder agent) — halcon native tools
            "file_read" | "file_write" | "file_edit" | "file_delete" | "file_inspect"
            | "directory_tree" | "list_directory_with_sizes" | "list_directory"
            | "read_multiple_files" | "edit_file" | "apply_patch" => {
                StepCapability::FileOperations
            }
            // MCP filesystem server tools — named differently from halcon native tools
            // (@modelcontextprotocol/server-filesystem: read_file, write_file, etc.)
            "read_file" | "write_file" | "create_directory" | "move_file"
            | "get_file_info" | "list_allowed_directories" => StepCapability::FileOperations,
            // Code execution (Coder agent)
            "bash" | "run_command" | "terminal" | "code_execution" | "dep_check"
            | "code_metrics" => StepCapability::CodeExecution,
            // Search and analysis (Coder agent) — halcon native + MCP filesystem search
            "grep" | "glob" | "fuzzy_find" | "symbol_search" | "native_search"
            | "semantic_grep" | "ast_search" | "search_files" => StepCapability::Search,
            // Git operations (Coder agent)
            "git_status" | "git_diff" | "git_log" | "git_add" | "git_commit"
            | "git_push" | "git_pull" | "git_branch" => StepCapability::GitOperations,
            // Web access (Chat agent)
            "web_search" | "web_fetch" | "http_request" => StepCapability::WebAccess,
            // Plugin sentinel tools → treat as code execution (needs real tools, not Chat)
            t if t.starts_with("plugin_halcon_dev_sentinel_") => StepCapability::CodeExecution,
            // Unknown tools: General (Chat agent, full tool surface)
            _ => StepCapability::General,
        }
    }

    /// Suggest the set of tools a sub-agent needs for a given capability.
    ///
    /// Always inserts `primary_tool` — even for the `General` fallback case where
    /// the tool name was not recognised. Narrowing to at least the one required tool
    /// prevents DeepSeek/GPT from receiving the full 63-tool surface and hesitating.
    ///
    /// MCP tool remapping: `search_files` (MCP filesystem) consistently times out
    /// (~31s) when scanning large directories like /Users/.../Documents. It is
    /// silently remapped to `grep` + `native_search` + `glob` which have 100% success
    /// rate in production. The instruction is also rewritten in `build_tasks()`.
    fn tools_for_capability(capability: &StepCapability, primary_tool: &str) -> HashSet<String> {
        // MCP search_files → native remap: replace with reliable native alternatives.
        // Evidence: 3/3 failures (31327ms, 31489ms, 1283ms timeouts) in tool_execution_metrics.
        // grep and native_search have 100% success rate across all recorded invocations.
        //
        // dep_check → read_multiple_files remap.
        // Evidence: 3/3 failures (~17s) in production sessions (Mon-Key, demo-js, demo-js retry).
        // read_multiple_files reads Cargo.lock/package.json directly with 94% success rate.
        let effective_primary = match primary_tool {
            "search_files" => "grep",
            "dep_check" => "read_multiple_files",
            other => other,
        };

        let mut tools = HashSet::new();
        tools.insert(effective_primary.to_string());

        match capability {
            StepCapability::FileOperations => {
                // File tools often need a read companion for verification.
                tools.insert("file_read".into());
                tools.insert("read_file".into()); // MCP filesystem alias
            }
            StepCapability::CodeExecution => {
                // bash is self-contained.
            }
            StepCapability::Search => {
                if primary_tool == "search_files" {
                    // Provide full native search surface: grep (content), glob (pattern),
                    // native_search (semantic). MCP search_files is excluded — it is
                    // unreliable on large trees and causes 31s timeouts.
                    tools.insert("native_search".into());
                    tools.insert("glob".into());
                }
                // Other search tools (grep, glob, native_search) are self-contained.
            }
            StepCapability::GitOperations => {
                // Git ops may need status for context.
                tools.insert("git_status".into());
            }
            StepCapability::WebAccess => {
                // Web tools are self-contained.
            }
            StepCapability::General => {
                // Unknown tool: keep the primary_tool so the sub-agent has a narrowed
                // surface (just the one required tool) rather than the full 63-tool set.
                // This dramatically improves tool-call reliability for models like DeepSeek.
            }
        }

        tools
    }

    /// Infer a reasonable output file path from a step description.
    ///
    /// Used when no explicit `expected_args.path` is set for a `file_write` step.
    /// Checks for common file type keywords and returns a sensible default name.
    fn infer_file_path(description: &str) -> String {
        let lower = description.to_lowercase();
        if lower.contains(".html") || lower.contains("html") || lower.contains("web page") || lower.contains("webpage") {
            "output.html".to_string()
        } else if lower.contains(".py") || lower.contains("python script") || lower.contains("python program") {
            "script.py".to_string()
        } else if lower.contains(".js") || lower.contains("javascript") {
            "script.js".to_string()
        } else if lower.contains(".ts") || lower.contains("typescript") {
            "script.ts".to_string()
        } else if lower.contains(".sh") || lower.contains("shell script") || lower.contains("bash script") {
            "script.sh".to_string()
        } else if lower.contains(".md") || lower.contains("markdown") || lower.contains("readme") {
            "README.md".to_string()
        } else if lower.contains(".json") || lower.contains("json file") {
            "output.json".to_string()
        } else if lower.contains(".rs") || lower.contains("rust") {
            "main.rs".to_string()
        } else if lower.contains(".toml") || lower.contains("config") {
            "config.toml".to_string()
        } else if lower.contains(".txt") || lower.contains("text file") {
            "output.txt".to_string()
        } else {
            "output.txt".to_string()
        }
    }

    /// Map capability to the most appropriate sub-agent type.
    fn agent_type_for_capability(capability: &StepCapability) -> AgentType {
        match capability {
            StepCapability::FileOperations | StepCapability::CodeExecution => AgentType::Coder,
            StepCapability::Search | StepCapability::GitOperations => AgentType::Coder,
            StepCapability::WebAccess => AgentType::Chat,
            StepCapability::General => AgentType::Chat,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::traits::ExecutionPlan;

    fn make_step(desc: &str, tool: Option<&str>, confidence: f64) -> PlanStep {
        PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: desc.into(),
            tool_name: tool.map(|t| t.into()),
            parallel: false,
            confidence,
            expected_args: None,
            outcome: None,
        }
    }

    fn make_parallel_step(desc: &str, tool: &str, confidence: f64) -> PlanStep {
        PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: desc.into(),
            tool_name: Some(tool.into()),
            parallel: true,
            confidence,
            expected_args: None,
            outcome: None,
        }
    }

    fn make_plan(steps: Vec<PlanStep>) -> ExecutionPlan {
        ExecutionPlan {
            goal: "Test goal".into(),
            steps,
            requires_confirmation: false,
            plan_id: Uuid::nil(),
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        }
    }

    #[test]
    fn classify_file_read() {
        let step = make_step("Read file", Some("file_read"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::FileOperations);
    }

    #[test]
    fn classify_file_write() {
        let step = make_step("Write file", Some("file_write"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::FileOperations);
    }

    #[test]
    fn classify_file_edit() {
        let step = make_step("Edit file", Some("file_edit"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::FileOperations);
    }

    #[test]
    fn classify_bash() {
        let step = make_step("Run command", Some("bash"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::CodeExecution);
    }

    #[test]
    fn classify_grep() {
        let step = make_step("Search files", Some("grep"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::Search);
    }

    #[test]
    fn classify_glob() {
        let step = make_step("Find files", Some("glob"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::Search);
    }

    #[test]
    fn classify_git_status() {
        let step = make_step("Check status", Some("git_status"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::GitOperations);
    }

    #[test]
    fn classify_git_diff() {
        let step = make_step("Show diff", Some("git_diff"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::GitOperations);
    }

    #[test]
    fn classify_web_search() {
        let step = make_step("Search web", Some("web_search"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::WebAccess);
    }

    #[test]
    fn classify_none_tool() {
        let step = make_step("Synthesize", None, 1.0);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::General);
    }

    #[test]
    fn classify_unknown_tool() {
        let step = make_step("Custom", Some("custom_tool"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::General);
    }

    #[test]
    fn analyze_plan_empty() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![]);
        let decisions = router.analyze_plan(&plan);
        assert!(decisions.is_empty());
    }

    #[test]
    fn analyze_plan_single_step_skipped() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![make_step("Read file", Some("file_read"), 0.9)]);
        let decisions = router.analyze_plan(&plan);
        assert!(decisions.is_empty(), "Single-step plans should not be delegated");
    }

    #[test]
    fn analyze_plan_two_steps_delegated() {
        // Threshold lowered to ≥2 steps — two-step plans with tool_names are eligible.
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Edit file", Some("file_edit"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        assert_eq!(decisions.len(), 2, "Two-step plans with tool_names should now be delegated");
    }

    #[test]
    fn analyze_plan_one_step_skipped() {
        // Single-step plans are still skipped (< 2 threshold).
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![make_step("Read file", Some("file_read"), 0.9)]);
        let decisions = router.analyze_plan(&plan);
        assert!(decisions.is_empty(), "Single-step plans should not be delegated");
    }

    #[test]
    fn analyze_plan_three_steps() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Edit file", Some("file_edit"), 0.8),
            make_step("Run tests", Some("bash"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        assert_eq!(decisions.len(), 3);
        assert_eq!(decisions[0].0, 0);
        assert_eq!(decisions[1].0, 1);
        assert_eq!(decisions[2].0, 2);
    }

    #[test]
    fn analyze_plan_low_confidence_filtered() {
        let router = DelegationRouter::new(true).with_min_confidence(0.7);
        let plan = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Maybe edit", Some("file_edit"), 0.5), // Below threshold
            make_step("Run tests", Some("bash"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].0, 0); // file_read
        assert_eq!(decisions[1].0, 2); // bash
    }

    #[test]
    fn analyze_plan_no_tool_name_skipped() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Think about it", None, 1.0), // No tool
            make_step("Run tests", Some("bash"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        assert_eq!(decisions.len(), 2);
        assert!(decisions.iter().all(|(_, d)| d.delegate));
    }

    #[test]
    fn analyze_plan_synthesis_step_skipped() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Edit file", Some("file_edit"), 0.9),
            make_step("Summarize changes", None, 1.0), // Last step, no tool = synthesis
        ]);
        let decisions = router.analyze_plan(&plan);
        assert_eq!(decisions.len(), 2);
        // Last synthesis step is excluded.
        assert!(decisions.iter().all(|(idx, _)| *idx < 2));
    }

    #[test]
    fn router_disabled_returns_empty() {
        let router = DelegationRouter::new(false);
        let plan = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Edit file", Some("file_edit"), 0.9),
            make_step("Run tests", Some("bash"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        assert!(decisions.is_empty());
    }

    #[test]
    fn build_tasks_maps_correctly() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Edit file", Some("file_edit"), 0.8),
            make_step("Run tests", Some("bash"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        let tasks = router.build_tasks(&plan, &decisions, "deepseek-chat");

        assert_eq!(tasks.len(), 3);
        // Step indices preserved.
        assert_eq!(tasks[0].0, 0);
        assert_eq!(tasks[1].0, 1);
        assert_eq!(tasks[2].0, 2);
        // Task IDs are unique.
        let ids: HashSet<_> = tasks.iter().map(|(_, t)| t.task_id).collect();
        assert_eq!(ids.len(), 3);
        // Model inherited.
        assert_eq!(tasks[0].1.model.as_deref(), Some("deepseek-chat"));
        // Instructions are prefixed with tool-use directive to force immediate tool execution.
        assert!(tasks[0].1.instruction.starts_with("IMPORTANT: Call the `file_read` tool NOW"));
        assert!(tasks[0].1.instruction.contains("Task: Read file"));
        assert!(tasks[2].1.instruction.starts_with("IMPORTANT: Call the `bash` tool NOW"));
        assert!(tasks[2].1.instruction.contains("Task: Run tests"));
        // Agent types mapped correctly.
        assert_eq!(tasks[0].1.agent_type, AgentType::Coder); // FileOperations
        assert_eq!(tasks[2].1.agent_type, AgentType::Coder); // CodeExecution
    }

    #[test]
    fn build_tasks_sequential_dependencies() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Edit file", Some("file_edit"), 0.9),
            make_step("Run tests", Some("bash"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        let tasks = router.build_tasks(&plan, &decisions, "test-model");

        // First task has no deps.
        assert!(tasks[0].1.depends_on.is_empty());
        // Second depends on first.
        assert_eq!(tasks[1].1.depends_on, vec![tasks[0].1.task_id]);
        // Third depends on second.
        assert_eq!(tasks[2].1.depends_on, vec![tasks[1].1.task_id]);
    }

    #[test]
    fn build_tasks_parallel_no_dependency() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Read file A", Some("file_read"), 0.9),
            make_parallel_step("Read file B", "file_read", 0.9),
            make_parallel_step("Read file C", "file_read", 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        let tasks = router.build_tasks(&plan, &decisions, "test-model");

        assert_eq!(tasks.len(), 3);
        // First has no deps.
        assert!(tasks[0].1.depends_on.is_empty());
        // Parallel steps have no deps (parallel: true).
        assert!(tasks[1].1.depends_on.is_empty());
        assert!(tasks[2].1.depends_on.is_empty());
    }

    #[test]
    fn delegation_decision_includes_suggested_tools() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Run tests", Some("bash"), 0.9),
            make_step("Search code", Some("grep"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);

        // file_read → FileOperations → includes file_read.
        assert!(decisions[0].1.suggested_tools.contains("file_read"));
        // bash → CodeExecution → includes bash.
        assert!(decisions[1].1.suggested_tools.contains("bash"));
        // grep → Search → includes grep.
        assert!(decisions[2].1.suggested_tools.contains("grep"));
    }

    // ── Audit fix: bounds-checked build_tasks ────────────────────────────────

    /// Out-of-bounds step_idx in decisions must not panic — bad entry is skipped.
    /// Simulated by using a plan with fewer steps than the decision's step_idx references.
    #[test]
    fn build_tasks_oob_step_idx_skipped_not_panic() {
        let router = DelegationRouter::new(true);
        // Plan has 3 steps (indices 0-2), used to generate valid decisions.
        let plan_full = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Run tests", Some("bash"), 0.9),
            make_step("Search code", Some("grep"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan_full);

        // Replay against a shorter plan (2 steps → indices 0-1).
        // Decision at step_idx=2 is now out of bounds.
        let plan_short = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Run tests", Some("bash"), 0.9),
        ]);
        // Must not panic — OOB decision is skipped; only 2 tasks produced.
        let tasks = router.build_tasks(&plan_short, &decisions, "test-model");
        assert_eq!(tasks.len(), 2, "OOB decision must be silently skipped");
        assert!(
            tasks.iter().all(|(idx, _)| *idx < 2),
            "all produced tasks must have valid step indices"
        );
    }

    /// All decisions OOB — build_tasks returns empty vec without panicking.
    #[test]
    fn build_tasks_all_oob_returns_empty() {
        let router = DelegationRouter::new(true);
        let plan_full = make_plan(vec![
            make_step("Read file", Some("file_read"), 0.9),
            make_step("Run tests", Some("bash"), 0.9),
            make_step("Search code", Some("grep"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan_full);

        // Empty plan — every decision (indices 0, 1, 2) is now OOB.
        let plan_empty = make_plan(vec![]);
        let tasks = router.build_tasks(&plan_empty, &decisions, "test-model");
        assert!(tasks.is_empty(), "all-OOB decisions produce empty result");
    }

    /// Regression guard: in-bounds build_tasks still works normally after the bounds fix.
    #[test]
    fn build_tasks_inbounds_regression_guard() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Read", Some("file_read"), 0.9),
            make_step("Edit", Some("file_edit"), 0.9),
            make_step("Test", Some("bash"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        let tasks = router.build_tasks(&plan, &decisions, "model");
        assert_eq!(tasks.len(), 3);
        assert!(tasks[0].1.instruction.starts_with("IMPORTANT: Call the `file_read` tool NOW"));
        assert!(tasks[0].1.instruction.contains("Task: Read"));
        assert!(tasks[2].1.instruction.starts_with("IMPORTANT: Call the `bash` tool NOW"));
        assert!(tasks[2].1.instruction.contains("Task: Test"));
    }

    // ── Fix 2: file_write delegation path injection ───────────────────────────

    /// file_write step with explicit path in expected_args → path appears in instruction.
    #[test]
    fn file_write_with_explicit_path_uses_expected_args() {
        let router = DelegationRouter::new(true);
        let mut step = make_step("Write the HTML game to disk", Some("file_write"), 0.9);
        step.expected_args = Some(serde_json::json!({"path": "/tmp/game.html", "content": "..."}));
        let plan = make_plan(vec![
            step,
            make_step("Run tests", Some("bash"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        let tasks = router.build_tasks(&plan, &decisions, "deepseek-chat");

        let file_write_task = &tasks[0].1;
        assert!(file_write_task.instruction.contains("/tmp/game.html"),
            "Instruction must contain explicit path from expected_args");
        assert!(file_write_task.instruction.contains("path=\"/tmp/game.html\""),
            "Instruction must include file_write path= directive");
    }

    /// file_write step with no expected_args but HTML description → inferred .html path.
    #[test]
    fn file_write_infers_html_path_from_description() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Create a web page HTML game", Some("file_write"), 0.9),
            make_step("Run tests", Some("bash"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        let tasks = router.build_tasks(&plan, &decisions, "deepseek-chat");

        let file_write_task = &tasks[0].1;
        assert!(file_write_task.instruction.contains("output.html"),
            "HTML description must infer .html path, got: {}", file_write_task.instruction);
    }

    /// file_write step with Python description → inferred .py path.
    #[test]
    fn file_write_infers_python_path_from_description() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Write a Python script to sort files", Some("file_write"), 0.9),
            make_step("Run tests", Some("bash"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        let tasks = router.build_tasks(&plan, &decisions, "deepseek-chat");

        let file_write_task = &tasks[0].1;
        assert!(file_write_task.instruction.contains("script.py"),
            "Python description must infer .py path, got: {}", file_write_task.instruction);
    }

    /// Non-file_write tools do NOT get a path_hint appended.
    #[test]
    fn non_file_write_tools_have_no_path_hint() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Read data", Some("file_read"), 0.9),
            make_step("Run bash script", Some("bash"), 0.9),
        ]);
        let decisions = router.analyze_plan(&plan);
        let tasks = router.build_tasks(&plan, &decisions, "deepseek-chat");

        for (_, task) in &tasks {
            assert!(!task.instruction.contains("Target file path:"),
                "Only file_write tasks should have path hints, got: {}", task.instruction);
        }
    }

    // ── infer_file_path unit tests ────────────────────────────────────────────

    #[test]
    fn infer_html_variants() {
        assert_eq!(DelegationRouter::infer_file_path("create an HTML game"), "output.html");
        assert_eq!(DelegationRouter::infer_file_path("write a web page"), "output.html");
        assert_eq!(DelegationRouter::infer_file_path("build a .html file"), "output.html");
    }

    #[test]
    fn infer_python_variants() {
        assert_eq!(DelegationRouter::infer_file_path("python script to parse logs"), "script.py");
        assert_eq!(DelegationRouter::infer_file_path("write a .py program"), "script.py");
    }

    #[test]
    fn infer_shell_variants() {
        assert_eq!(DelegationRouter::infer_file_path("write a bash script"), "script.sh");
        assert_eq!(DelegationRouter::infer_file_path("create shell script"), "script.sh");
    }

    #[test]
    fn infer_default_for_unknown() {
        assert_eq!(DelegationRouter::infer_file_path("write some output"), "output.txt");
        assert_eq!(DelegationRouter::infer_file_path(""), "output.txt");
    }

    // ── MCP filesystem tool coverage (RC-1 fix) ───────────────────────────────

    /// search_files (MCP filesystem) must route to Search → Coder, not General → Chat.
    /// This was the root cause of the "Chat [1/3] — 0 tools" failure in the cotización
    /// document analysis session: deepseek generated `search_files` in the plan, which
    /// fell to General/Chat because it wasn't listed in classify_step.
    #[test]
    fn classify_mcp_search_files_is_search_not_general() {
        let step = make_step("Search for cuervo/zuclubit files", Some("search_files"), 0.9);
        assert_eq!(
            DelegationRouter::classify_step(&step),
            StepCapability::Search,
            "search_files (MCP filesystem) must route to Search, not General/Chat"
        );
    }

    /// read_file (MCP filesystem, different from halcon native "file_read") → FileOperations.
    #[test]
    fn classify_mcp_read_file_is_file_operations() {
        let step = make_step("Read document", Some("read_file"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::FileOperations);
    }

    /// write_file (MCP filesystem, different from halcon native "file_write") → FileOperations.
    #[test]
    fn classify_mcp_write_file_is_file_operations() {
        let step = make_step("Write output", Some("write_file"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::FileOperations);
    }

    /// create_directory (MCP filesystem) → FileOperations.
    #[test]
    fn classify_mcp_create_directory_is_file_operations() {
        let step = make_step("Create output dir", Some("create_directory"), 0.9);
        assert_eq!(DelegationRouter::classify_step(&step), StepCapability::FileOperations);
    }

    /// move_file / get_file_info / list_allowed_directories (MCP filesystem) → FileOperations.
    #[test]
    fn classify_mcp_file_management_tools_are_file_operations() {
        for tool in &["move_file", "get_file_info", "list_allowed_directories"] {
            let step = make_step("File op", Some(tool), 0.9);
            assert_eq!(
                DelegationRouter::classify_step(&step),
                StepCapability::FileOperations,
                "{tool} must route to FileOperations"
            );
        }
    }

    /// MCP search_files routes to Coder (via Search), not Chat.
    #[test]
    fn mcp_search_files_routes_to_coder_agent_type() {
        let capability = StepCapability::Search;
        assert_eq!(
            DelegationRouter::agent_type_for_capability(&capability),
            AgentType::Coder,
            "Search capability must map to Coder, not Chat"
        );
    }

    // ── General fallback narrows to primary tool (RC-2 fix) ────────────────────

    /// When classify_step returns General (unknown tool), tools_for_capability must
    /// still include the primary tool name — not return an empty set.
    /// Previously, empty set → orchestrator gave sub-agent all 63 tools → DeepSeek confused.
    #[test]
    fn general_capability_includes_primary_tool_not_empty() {
        let tools = DelegationRouter::tools_for_capability(&StepCapability::General, "some_custom_tool");
        assert!(
            tools.contains("some_custom_tool"),
            "General fallback must always include the primary tool for surface narrowing"
        );
    }

    /// Verify that unknown tool name still triggers a retry-able delegation with narrowed surface.
    #[test]
    fn unknown_tool_plan_step_has_narrowed_tool_surface() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Do something custom", Some("custom_unknown_tool"), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);
        let decisions = router.analyze_plan(&plan);
        assert_eq!(decisions.len(), 1, "custom tool step should be delegated");
        let (_, ref decision) = decisions[0];
        assert!(
            decision.suggested_tools.contains("custom_unknown_tool"),
            "suggested_tools must contain the custom tool even for unknown/General capability"
        );
        assert_eq!(
            decision.suggested_tools.len(),
            1,
            "General capability should produce exactly 1 tool (the primary) not 63"
        );
    }

    // ── MCP search_files → native remap (RC-5 fix) ───────────────────────────
    //
    // Root cause: MCP search_files times out 100% of the time on large directories
    // (~31s for /Users/.../Documents). Evidence: 3/3 failures in tool_execution_metrics.
    // Fix: remap search_files → grep + native_search + glob (all 100% success rate).

    /// search_files must be remapped to grep in the tool surface — never included as-is.
    #[test]
    fn search_files_remapped_to_grep_not_mcp() {
        let tools = DelegationRouter::tools_for_capability(&StepCapability::Search, "search_files");
        assert!(
            tools.contains("grep"),
            "search_files must remap to grep (native, 100% success rate)"
        );
        assert!(
            !tools.contains("search_files"),
            "search_files (MCP, 0% success, 31s timeout) must NOT be in tool surface"
        );
    }

    /// search_files remap provides full native search surface: grep + native_search + glob.
    #[test]
    fn search_files_remap_includes_full_native_search_surface() {
        let tools = DelegationRouter::tools_for_capability(&StepCapability::Search, "search_files");
        assert!(tools.contains("grep"), "must include grep");
        assert!(tools.contains("native_search"), "must include native_search");
        assert!(tools.contains("glob"), "must include glob");
        assert!(!tools.contains("search_files"), "must NOT include broken MCP search_files");
    }

    /// Other Search tools (grep, native_search, glob) are NOT remapped — only search_files.
    #[test]
    fn non_search_files_search_tools_are_not_remapped() {
        for tool in &["grep", "native_search", "glob", "fuzzy_find"] {
            let tools = DelegationRouter::tools_for_capability(&StepCapability::Search, tool);
            assert!(
                tools.contains(*tool),
                "{tool} must remain as primary (no remap)"
            );
            assert!(
                !tools.contains("search_files"),
                "{tool} must not add broken search_files to surface"
            );
        }
    }

    /// build_tasks instruction uses `grep` not `search_files` when step tool is search_files.
    #[test]
    fn search_files_step_instruction_uses_grep_not_mcp() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Buscar archivos de cotizaciones", Some("search_files"), 0.9),
            make_step("Sintetizar", None, 1.0),
        ]);
        let decisions = router.analyze_plan(&plan);
        let tasks = router.build_tasks(&plan, &decisions, "deepseek-chat");

        assert_eq!(tasks.len(), 1);
        let instruction = &tasks[0].1.instruction;
        assert!(
            instruction.contains("`grep`"),
            "instruction must reference `grep`, not `search_files`. Got: {instruction}"
        );
        assert!(
            !instruction.contains("`search_files`"),
            "instruction must NOT reference `search_files`. Got: {instruction}"
        );
        assert!(
            instruction.contains("-r -l"),
            "instruction must include grep usage hint (-r -l flags). Got: {instruction}"
        );
    }

    /// Agent type for search_files is still Coder (via Search capability).
    #[test]
    fn search_files_remap_preserves_coder_agent_type() {
        let router = DelegationRouter::new(true);
        let plan = make_plan(vec![
            make_step("Buscar archivos de cotizaciones", Some("search_files"), 0.9),
            make_step("Sintetizar", None, 1.0),
        ]);
        let decisions = router.analyze_plan(&plan);
        let tasks = router.build_tasks(&plan, &decisions, "deepseek-chat");

        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].1.agent_type,
            AgentType::Coder,
            "search_files remap must still route to Coder agent"
        );
    }

    #[test]
    fn null_string_tool_not_delegated() {
        // DeepSeek emits "null" (string) instead of JSON null for reasoning steps.
        // These must NOT be delegated — otherwise they get an empty tool surface.
        let router = DelegationRouter::new(true).with_min_confidence(0.5);
        let plan = make_plan(vec![
            make_step("Read files", Some("read_multiple_files"), 0.9),
            make_step("Validate results", Some("null"), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);
        let decisions = router.analyze_plan(&plan);
        // Only step 0 should be delegated — "null" string should be filtered out.
        assert_eq!(decisions.len(), 1, "tool_name=\"null\" must not be delegated");
        assert_eq!(decisions[0].0, 0, "only step 0 (read_multiple_files) should delegate");
    }

    #[test]
    fn empty_string_tool_not_delegated() {
        let router = DelegationRouter::new(true).with_min_confidence(0.5);
        let plan = make_plan(vec![
            make_step("Read files", Some("file_read"), 0.9),
            make_step("Think about it", Some(""), 0.9),
            make_step("Synthesize", None, 1.0),
        ]);
        let decisions = router.analyze_plan(&plan);
        assert_eq!(decisions.len(), 1, "empty tool_name must not be delegated");
    }
}
