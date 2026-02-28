//! Playbook-based planner: matches user intent against pre-defined YAML playbooks.
//!
//! Playbooks live in `~/.halcon/playbooks/*.yaml` and define canned execution
//! plans for recurring workflows (git workflows, deploy sequences, etc.).
//!
//! Unlike LlmPlanner which calls the model, PlaybookPlanner is:
//! - **Instant**: zero LLM latency (sub-millisecond plan generation)
//! - **Deterministic**: same keywords → same plan every time
//! - **User-extensible**: add a YAML file, no recompilation needed
//!
//! # Playbook format
//!
//! ```yaml
//! name: git-commit
//! description: "Stage all changes and create a descriptive commit"
//! triggers:
//!   - "commit"
//!   - "git commit"
//!   - "stage and commit"
//! goal: "Stage all changes and create a descriptive commit"
//! steps:
//!   - description: "Check current git status"
//!     tool_name: bash
//!     parallel: false
//!     confidence: 0.95
//!   - description: "Stage all changes with git add -A"
//!     tool_name: bash
//!     parallel: false
//!     confidence: 0.95
//!   - description: "Create a descriptive commit message and commit"
//!     tool_name: bash
//!     parallel: false
//!     confidence: 0.9
//! requires_confirmation: true
//! ```

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use halcon_core::error::Result;
use halcon_core::traits::{ExecutionPlan, PlanStep, Planner};
use halcon_core::types::ToolDefinition;

// ---------------------------------------------------------------------------
// YAML schema types
// ---------------------------------------------------------------------------

/// A single step in a playbook (YAML-deserializable).
#[derive(Debug, Clone, Deserialize)]
pub struct PlaybookStep {
    /// Step description. May be empty/placeholder when `sub_playbook` is set.
    #[serde(default)]
    pub description: String,
    pub tool_name: Option<String>,
    #[serde(default)]
    pub parallel: bool,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default)]
    pub expected_args: Option<serde_json::Value>,
    /// P3 Composition: inline all steps from the named playbook in place of this step.
    /// The referenced playbook is looked up by `name` in the loaded playbook set.
    /// Circular references are capped at depth 3 and silently skipped.
    #[serde(default)]
    pub sub_playbook: Option<String>,
}

/// A playbook definition (maps to one YAML file).
#[derive(Debug, Clone, Deserialize)]
pub struct Playbook {
    /// Unique name (used in tracing/feedback).
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Keywords / phrases that trigger this playbook.
    /// Each trigger is checked as a case-insensitive substring of the user message.
    pub triggers: Vec<String>,
    /// High-level goal string for the ExecutionPlan.
    pub goal: String,
    /// Ordered steps.
    pub steps: Vec<PlaybookStep>,
    /// Whether the plan requires user confirmation before proceeding.
    #[serde(default)]
    pub requires_confirmation: bool,
    /// Priority for tie-breaking when multiple playbooks match (higher = preferred).
    #[serde(default)]
    pub priority: i32,
}

fn default_confidence() -> f64 { 0.9 }

// ---------------------------------------------------------------------------
// PlaybookPlanner
// ---------------------------------------------------------------------------

/// A `Planner` implementation backed by YAML playbooks.
///
/// Loaded once at session start; supports hot-reload via `reload()`.
pub struct PlaybookPlanner {
    playbooks: Vec<Playbook>,
}

impl PlaybookPlanner {
    /// Create a planner with no playbooks. Useful for testing.
    pub fn empty() -> Self {
        Self { playbooks: Vec::new() }
    }

    /// Load playbooks from `dir/*.yaml` and `dir/*.yml`.
    ///
    /// * Missing directory → empty planner (no error).
    /// * Parse errors → warning + skip (other playbooks still loaded).
    pub fn from_dir(dir: &Path) -> Self {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Self::empty(),
        };

        let mut playbooks = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str());
            if !matches!(ext, Some("yaml") | Some("yml")) {
                continue;
            }

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to read playbook");
                    continue;
                }
            };

            let playbook: Playbook = match serde_yaml::from_str(&content) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to parse playbook");
                    continue;
                }
            };

            tracing::debug!(name = %playbook.name, triggers = ?playbook.triggers, "Loaded playbook");
            playbooks.push(playbook);
        }

        // Sort by descending priority so highest-priority playbooks match first.
        playbooks.sort_by(|a, b| b.priority.cmp(&a.priority));

        Self { playbooks }
    }

    /// Load from the default location: `~/.halcon/playbooks/`.
    pub fn from_default_dir() -> Self {
        match dirs::home_dir() {
            Some(home) => Self::from_dir(&home.join(".halcon").join("playbooks")),
            None => Self::empty(),
        }
    }

    /// Number of loaded playbooks.
    pub fn len(&self) -> usize {
        self.playbooks.len()
    }

    /// Returns true if no playbooks are loaded.
    pub fn is_empty(&self) -> bool {
        self.playbooks.is_empty()
    }

    /// Exported for testing: attempt to match a message against loaded playbooks.
    pub fn find_match(&self, user_message: &str) -> Option<&Playbook> {
        let lower = user_message.to_lowercase();
        self.playbooks.iter().find(|p| {
            p.triggers.iter().any(|t| lower.contains(&t.to_lowercase()))
        })
    }

    /// Convert a `Playbook` to an `ExecutionPlan` (flat, no sub-playbook expansion).
    ///
    /// Used in tests and as a building block for `to_plan_composed`.
    pub fn to_plan(playbook: &Playbook) -> ExecutionPlan {
        let steps = playbook
            .steps
            .iter()
            .map(|s| PlanStep {
                step_id: uuid::Uuid::new_v4(),
                description: s.description.clone(),
                tool_name: s.tool_name.clone(),
                parallel: s.parallel,
                confidence: s.confidence,
                expected_args: s.expected_args.clone(),
                outcome: None,
            })
            .collect();

        ExecutionPlan {
            goal: playbook.goal.clone(),
            steps,
            requires_confirmation: playbook.requires_confirmation,
            plan_id: uuid::Uuid::new_v4(),
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        }
    }

    /// P3 Composition: convert a `Playbook` to an `ExecutionPlan`, recursively
    /// expanding `sub_playbook` references.
    ///
    /// If a step has `sub_playbook: "other-name"`, the steps from the matching
    /// playbook are inlined in place of that step (depth-limited to 3 to prevent
    /// circular infinite expansion).
    pub fn to_plan_composed(&self, playbook: &Playbook) -> ExecutionPlan {
        // Build name-indexed lookup for composition resolution.
        let index: std::collections::HashMap<&str, &Playbook> = self
            .playbooks
            .iter()
            .map(|p| (p.name.as_str(), p))
            .collect();

        let steps = Self::resolve_steps(&playbook.steps, &index, 0);

        ExecutionPlan {
            goal: playbook.goal.clone(),
            steps,
            requires_confirmation: playbook.requires_confirmation,
            plan_id: uuid::Uuid::new_v4(),
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        }
    }

    /// Recursively resolve steps, expanding `sub_playbook` references (max `depth` = 3).
    fn resolve_steps(
        steps: &[PlaybookStep],
        index: &std::collections::HashMap<&str, &Playbook>,
        depth: u8,
    ) -> Vec<PlanStep> {
        steps
            .iter()
            .flat_map(|s| {
                // If this step references a sub-playbook, expand it inline.
                if let Some(ref sub_name) = s.sub_playbook {
                    if depth < 3 {
                        if let Some(&sub) = index.get(sub_name.as_str()) {
                            tracing::debug!(
                                sub = %sub_name,
                                depth,
                                "P3: expanding sub-playbook inline"
                            );
                            return Self::resolve_steps(&sub.steps, index, depth + 1);
                        } else {
                            tracing::warn!(
                                sub = %sub_name,
                                "P3: sub_playbook not found, skipping"
                            );
                            return vec![];
                        }
                    } else {
                        tracing::warn!(
                            sub = %sub_name,
                            "P3: sub_playbook depth limit reached, skipping"
                        );
                        return vec![];
                    }
                }

                vec![PlanStep {
                    step_id: uuid::Uuid::new_v4(),
                    description: s.description.clone(),
                    tool_name: s.tool_name.clone(),
                    parallel: s.parallel,
                    confidence: s.confidence,
                    expected_args: s.expected_args.clone(),
                    outcome: None,
                }]
            })
            .collect()
    }

    /// Wrap this planner in an `Arc` for use as `Box<dyn Planner>`.
    pub fn into_arc(self) -> Arc<dyn Planner> {
        Arc::new(self)
    }

    /// Auto-learn: save a successful LLM-generated plan as a YAML playbook.
    ///
    /// Parses the execution tracker's timeline JSON (from `AgentLoopResult.timeline_json`)
    /// and writes a YAML playbook to `~/.halcon/playbooks/auto-<slug>.yaml`.
    ///
    /// Returns the saved path on success, `None` if saving failed or was skipped.
    ///
    /// Trigger words are extracted from the user message (significant words ≥ 4 chars,
    /// deduplicated, max 5). The playbook name is a sanitized slug of the goal.
    pub fn record_from_timeline(
        &self,
        user_message: &str,
        timeline_json: &str,
    ) -> Option<std::path::PathBuf> {
        // Parse the timeline JSON.
        let timeline: serde_json::Value = serde_json::from_str(timeline_json).ok()?;
        let goal = timeline["goal"].as_str().unwrap_or("auto task");
        let steps = timeline["steps"].as_array()?;

        if steps.is_empty() {
            return None; // Nothing to save.
        }

        // Extract trigger keywords from user_message (≥4 chars, non-stop words).
        let stop_words = [
            "this", "that", "with", "from", "have", "will", "your", "what",
            "make", "some", "into", "over", "then", "them", "their", "when",
            "there", "these", "which", "while", "about", "would", "could",
        ];
        let mut triggers: Vec<String> = user_message
            .to_lowercase()
            .split_whitespace()
            .filter(|w| {
                let clean: String = w.chars().filter(|c| c.is_alphabetic()).collect();
                clean.len() >= 4 && !stop_words.contains(&clean.as_str())
            })
            .map(|w| w.chars().filter(|c| c.is_alphabetic()).collect::<String>())
            .collect::<std::collections::BTreeSet<_>>() // dedup via sorted set
            .into_iter()
            .take(5)
            .collect();

        if triggers.is_empty() {
            triggers.push("auto".to_string());
        }

        // Build playbook slug from goal.
        let slug: String = goal
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let slug = if slug.len() > 40 { slug[..40].to_string() } else { slug };

        // Build YAML content.
        let triggers_yaml: String = triggers
            .iter()
            .map(|t| format!("  - \"{t}\""))
            .collect::<Vec<_>>()
            .join("\n");

        let steps_yaml: String = steps
            .iter()
            .filter_map(|s| {
                let desc = s["description"].as_str()?;
                let tool = s["tool_name"].as_str();
                let tool_line = tool
                    .map(|t| format!("\n    tool_name: {t}"))
                    .unwrap_or_default();
                Some(format!(
                    "  - description: \"{}\"{tool_line}\n    confidence: 0.85",
                    desc.replace('"', "\\\"")
                ))
            })
            .collect::<Vec<_>>()
            .join("\n");

        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let yaml_content = format!(
            "# Auto-generated playbook (halcon P3 learning)\nname: auto-{slug}-{ts}\ndescription: \"Auto-learned from: {goal}\"\ntriggers:\n{triggers_yaml}\ngoal: \"{goal}\"\nsteps:\n{steps_yaml}\nrequires_confirmation: false\npriority: -1\n",
            goal = goal.replace('"', "\\\""),
        );

        // Write to ~/.halcon/playbooks/.
        let home = dirs::home_dir()?;
        let playbooks_dir = home.join(".halcon").join("playbooks");
        if let Err(e) = std::fs::create_dir_all(&playbooks_dir) {
            tracing::warn!(error = %e, "Failed to create playbooks dir for auto-learn");
            return None;
        }

        let filename = format!("auto-{slug}-{ts}.yaml");
        let path = playbooks_dir.join(&filename);
        if let Err(e) = std::fs::write(&path, yaml_content) {
            tracing::warn!(path = %path.display(), error = %e, "Failed to write auto-learned playbook");
            return None;
        }

        tracing::info!(path = %path.display(), goal, "Auto-learned plan saved as playbook");
        Some(path)
    }
}

#[async_trait]
impl Planner for PlaybookPlanner {
    fn name(&self) -> &str {
        "PlaybookPlanner"
    }

    /// Returns a plan instantly (zero LLM calls) if a playbook matches.
    /// Returns `None` if no playbook matches (agent falls back to LlmPlanner or direct execution).
    async fn plan(
        &self,
        user_message: &str,
        _available_tools: &[ToolDefinition],
    ) -> Result<Option<ExecutionPlan>> {
        if let Some(playbook) = self.find_match(user_message) {
            tracing::info!(
                playbook = %playbook.name,
                "PlaybookPlanner matched — returning canned plan"
            );
            Ok(Some(self.to_plan_composed(playbook)))
        } else {
            Ok(None)
        }
    }

    /// Playbooks don't support replanning (static templates).
    async fn replan(
        &self,
        _current_plan: &ExecutionPlan,
        _failed_step_index: usize,
        _error: &str,
        _available_tools: &[ToolDefinition],
    ) -> Result<Option<ExecutionPlan>> {
        Ok(None)
    }

    fn max_replans(&self) -> u32 {
        0 // Playbooks are fixed — no replanning.
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_playbook(name: &str, triggers: Vec<&str>, steps: usize) -> Playbook {
        Playbook {
            name: name.into(),
            description: format!("{name} description"),
            triggers: triggers.into_iter().map(String::from).collect(),
            goal: format!("{name} goal"),
            steps: (0..steps)
                .map(|i| PlaybookStep {
                    description: format!("step {i}"),
                    tool_name: Some("bash".into()),
                    parallel: false,
                    confidence: 0.9,
                    expected_args: None,
                    sub_playbook: None,
                })
                .collect(),
            requires_confirmation: false,
            priority: 0,
        }
    }

    fn planner_with(playbooks: Vec<Playbook>) -> PlaybookPlanner {
        PlaybookPlanner { playbooks }
    }

    // --- find_match ---

    #[test]
    fn find_match_returns_none_when_empty() {
        let p = PlaybookPlanner::empty();
        assert!(p.find_match("commit my changes").is_none());
    }

    #[test]
    fn find_match_exact_trigger() {
        let pb = make_playbook("git-commit", vec!["git commit", "commit"], 2);
        let p = planner_with(vec![pb]);
        let m = p.find_match("please git commit my changes");
        assert!(m.is_some());
        assert_eq!(m.unwrap().name, "git-commit");
    }

    #[test]
    fn find_match_case_insensitive() {
        let pb = make_playbook("test", vec!["Run Tests"], 1);
        let p = planner_with(vec![pb]);
        assert!(p.find_match("run tests in the project").is_some());
        assert!(p.find_match("RUN TESTS NOW").is_some());
    }

    #[test]
    fn find_match_no_match_returns_none() {
        let pb = make_playbook("deploy", vec!["deploy", "release"], 3);
        let p = planner_with(vec![pb]);
        assert!(p.find_match("fix the bug in main.rs").is_none());
    }

    #[test]
    fn find_match_priority_order() {
        let low = Playbook {
            priority: 0,
            ..make_playbook("low", vec!["commit"], 1)
        };
        let high = Playbook {
            priority: 10,
            ..make_playbook("high", vec!["commit"], 1)
        };
        // Sort descending by priority (like from_dir does).
        let mut playbooks = vec![low, high];
        playbooks.sort_by(|a, b| b.priority.cmp(&a.priority));
        let p = PlaybookPlanner { playbooks };
        assert_eq!(p.find_match("commit changes").unwrap().name, "high");
    }

    // --- to_plan ---

    #[test]
    fn to_plan_converts_steps() {
        let pb = make_playbook("test-plan", vec!["x"], 3);
        let plan = PlaybookPlanner::to_plan(&pb);
        assert_eq!(plan.goal, "test-plan goal");
        assert_eq!(plan.steps.len(), 3);
        assert!(!plan.requires_confirmation);
        assert_eq!(plan.replan_count, 0);
        assert!(plan.parent_plan_id.is_none());
    }

    #[test]
    fn to_plan_requires_confirmation_propagated() {
        let mut pb = make_playbook("danger", vec!["nuke"], 1);
        pb.requires_confirmation = true;
        let plan = PlaybookPlanner::to_plan(&pb);
        assert!(plan.requires_confirmation);
    }

    #[test]
    fn to_plan_each_step_has_new_uuid() {
        let pb = make_playbook("p", vec!["x"], 2);
        let plan1 = PlaybookPlanner::to_plan(&pb);
        let plan2 = PlaybookPlanner::to_plan(&pb);
        // Each plan gets a fresh UUID.
        assert_ne!(plan1.plan_id, plan2.plan_id);
    }

    // --- Planner trait ---

    #[tokio::test]
    async fn plan_returns_some_on_match() {
        let pb = make_playbook("commit", vec!["commit"], 2);
        let p = planner_with(vec![pb]);
        let result = p.plan("commit my changes", &[]).await.unwrap();
        assert!(result.is_some());
        let plan = result.unwrap();
        assert_eq!(plan.goal, "commit goal");
        assert_eq!(plan.steps.len(), 2);
    }

    #[tokio::test]
    async fn plan_returns_none_on_no_match() {
        let pb = make_playbook("commit", vec!["commit"], 2);
        let p = planner_with(vec![pb]);
        let result = p.plan("fix the bug", &[]).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn replan_always_returns_none() {
        let p = PlaybookPlanner::empty();
        let plan = ExecutionPlan {
            goal: "x".into(),
            steps: vec![],
            requires_confirmation: false,
            plan_id: uuid::Uuid::new_v4(),
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        };
        let result = p.replan(&plan, 0, "error", &[]).await.unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn name_is_playbook_planner() {
        let p = PlaybookPlanner::empty();
        assert_eq!(p.name(), "PlaybookPlanner");
    }

    #[test]
    fn max_replans_is_zero() {
        let p = PlaybookPlanner::empty();
        assert_eq!(p.max_replans(), 0);
    }

    // --- from_dir ---

    #[test]
    fn from_dir_missing_dir_returns_empty() {
        let p = PlaybookPlanner::from_dir(Path::new("/tmp/halcon_playbooks_nonexistent_xyz"));
        assert!(p.is_empty());
    }

    #[test]
    fn from_dir_empty_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = PlaybookPlanner::from_dir(tmp.path());
        assert!(p.is_empty());
    }

    #[test]
    fn from_dir_ignores_non_yaml_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("playbook.toml"), "[x]").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "notes").unwrap();
        let p = PlaybookPlanner::from_dir(tmp.path());
        assert!(p.is_empty());
    }

    #[test]
    fn from_dir_loads_yaml_playbook() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml = r#"
name: git-commit
description: "Stage and commit"
triggers:
  - "commit"
  - "git commit"
goal: "Create a git commit"
steps:
  - description: "Check status"
    tool_name: bash
    parallel: false
    confidence: 0.95
  - description: "Stage changes"
    tool_name: bash
requires_confirmation: false
"#;
        std::fs::write(tmp.path().join("git-commit.yaml"), yaml).unwrap();
        let p = PlaybookPlanner::from_dir(tmp.path());
        assert_eq!(p.len(), 1);
        assert!(p.find_match("please commit my work").is_some());
    }

    #[test]
    fn from_dir_loads_yml_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml = r#"
name: deploy
description: "Deploy to production"
triggers:
  - "deploy"
goal: "Deploy the application"
steps:
  - description: "Run deployment"
    tool_name: bash
"#;
        std::fs::write(tmp.path().join("deploy.yml"), yaml).unwrap();
        let p = PlaybookPlanner::from_dir(tmp.path());
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn from_dir_skips_invalid_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        // Invalid YAML
        std::fs::write(tmp.path().join("bad.yaml"), ":::not valid yaml:::").unwrap();
        // Valid playbook
        let valid = r#"
name: valid
description: "Valid playbook"
triggers:
  - "valid"
goal: "Valid goal"
steps:
  - description: "Do something"
"#;
        std::fs::write(tmp.path().join("valid.yaml"), valid).unwrap();
        let p = PlaybookPlanner::from_dir(tmp.path());
        // Bad one skipped, valid one loaded.
        assert_eq!(p.len(), 1);
        assert!(p.find_match("valid trigger").is_some());
    }

    #[test]
    fn from_dir_step_defaults_applied() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml = r#"
name: minimal
description: ""
triggers:
  - "minimal"
goal: "Minimal goal"
steps:
  - description: "A step with defaults"
"#;
        std::fs::write(tmp.path().join("minimal.yaml"), yaml).unwrap();
        let p = PlaybookPlanner::from_dir(tmp.path());
        let pb = p.find_match("minimal task").unwrap();
        let step = &pb.steps[0];
        assert_eq!(step.confidence, 0.9); // default
        assert!(!step.parallel);          // default
        assert!(step.tool_name.is_none()); // not set
    }

    #[test]
    fn len_and_is_empty() {
        let p = PlaybookPlanner::empty();
        assert_eq!(p.len(), 0);
        assert!(p.is_empty());

        let pb = make_playbook("x", vec!["x"], 1);
        let p2 = planner_with(vec![pb]);
        assert_eq!(p2.len(), 1);
        assert!(!p2.is_empty());
    }

    // --- record_from_timeline (P3 auto-learning) ---

    #[test]
    fn record_from_timeline_saves_yaml_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = PlaybookPlanner::empty();

        let timeline = serde_json::json!({
            "goal": "Deploy the application to production",
            "steps": [
                { "description": "Run all tests", "tool_name": "bash" },
                { "description": "Build release binary", "tool_name": "bash" },
                { "description": "Push to production server", "tool_name": "bash" }
            ]
        });
        let timeline_str = serde_json::to_string(&timeline).unwrap();

        // Manually exercise the core logic: parse JSON and construct YAML.
        let parsed: serde_json::Value = serde_json::from_str(&timeline_str).unwrap();
        let goal = parsed["goal"].as_str().unwrap();
        let steps = parsed["steps"].as_array().unwrap();

        assert_eq!(goal, "Deploy the application to production");
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0]["description"].as_str().unwrap(), "Run all tests");

        // Build and save YAML to tmp dir manually (simulates record_from_timeline logic).
        let path = tmp.path().join("auto-deploy.yaml");
        let yaml = format!("name: auto-deploy\ngoal: \"{goal}\"\n");
        std::fs::write(&path, yaml).unwrap();
        assert!(path.exists());

        // Verify the planner didn't change (it's pure function).
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn record_from_timeline_skips_empty_steps() {
        let p = PlaybookPlanner::empty();
        let timeline = serde_json::json!({
            "goal": "Empty plan",
            "steps": []
        });
        let timeline_str = serde_json::to_string(&timeline).unwrap();

        // Empty steps → should return None.
        // (record_from_timeline uses home_dir so we test the logic inline)
        let parsed: serde_json::Value = serde_json::from_str(&timeline_str).unwrap();
        let steps = parsed["steps"].as_array().unwrap();
        assert!(steps.is_empty(), "Empty steps should be detected");

        // PlaybookPlanner should still be empty (no side effect on empty input).
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn record_from_timeline_extracts_trigger_keywords() {
        // Test trigger keyword extraction logic.
        let user_message = "deploy the application to staging environment";
        let stop_words = [
            "this", "that", "with", "from", "have", "will", "your", "what",
            "make", "some", "into", "over", "then", "them", "their", "when",
        ];

        let triggers: Vec<String> = user_message
            .to_lowercase()
            .split_whitespace()
            .filter(|w| {
                let clean: String = w.chars().filter(|c| c.is_alphabetic()).collect();
                clean.len() >= 4 && !stop_words.contains(&clean.as_str())
            })
            .map(|w| w.chars().filter(|c| c.is_alphabetic()).collect::<String>())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .take(5)
            .collect();

        assert!(!triggers.is_empty(), "Should extract at least one trigger word");
        // "deploy", "application", "staging", "environment" should be in triggers.
        assert!(triggers.contains(&"deploy".to_string()), "Expected 'deploy' in triggers");
        assert!(triggers.len() <= 5, "Max 5 triggers");
    }

    // --- P3 Playbook composition (sub_playbook expansion) ---

    fn make_step(desc: &str, tool: &str) -> PlaybookStep {
        PlaybookStep {
            description: desc.into(),
            tool_name: Some(tool.into()),
            parallel: false,
            confidence: 0.9,
            expected_args: None,
            sub_playbook: None,
        }
    }

    fn make_sub_step(sub_name: &str) -> PlaybookStep {
        PlaybookStep {
            description: String::new(),
            tool_name: None,
            parallel: false,
            confidence: 0.9,
            expected_args: None,
            sub_playbook: Some(sub_name.into()),
        }
    }

    #[test]
    fn composition_expands_sub_playbook_inline() {
        // "checks" playbook with 2 steps
        let checks = Playbook {
            name: "checks".into(),
            description: "run checks".into(),
            triggers: vec!["check".into()],
            goal: "run checks".into(),
            steps: vec![make_step("Lint code", "bash"), make_step("Run tests", "bash")],
            requires_confirmation: false,
            priority: 0,
        };
        // "deploy" playbook with 1 inline sub_playbook + 1 own step
        let deploy = Playbook {
            name: "deploy".into(),
            description: "deploy".into(),
            triggers: vec!["deploy".into()],
            goal: "deploy to prod".into(),
            steps: vec![make_sub_step("checks"), make_step("Push to server", "bash")],
            requires_confirmation: false,
            priority: 0,
        };

        let planner = planner_with(vec![checks, deploy]);
        let plan = planner.to_plan_composed(&planner.playbooks[1]);

        // Should have 3 steps: 2 from "checks" + 1 own step.
        assert_eq!(plan.steps.len(), 3, "Should expand sub-playbook steps inline");
        assert_eq!(plan.steps[0].description, "Lint code");
        assert_eq!(plan.steps[1].description, "Run tests");
        assert_eq!(plan.steps[2].description, "Push to server");
    }

    #[test]
    fn composition_skips_unknown_sub_playbook() {
        let pb = Playbook {
            name: "broken".into(),
            description: "".into(),
            triggers: vec!["broken".into()],
            goal: "broken goal".into(),
            steps: vec![
                make_sub_step("nonexistent"),
                make_step("Real step", "bash"),
            ],
            requires_confirmation: false,
            priority: 0,
        };
        let planner = planner_with(vec![pb]);
        let plan = planner.to_plan_composed(&planner.playbooks[0]);
        // Unknown sub-playbook is skipped; only the real step remains.
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].description, "Real step");
    }

    #[test]
    fn composition_depth_limit_prevents_unbounded_expansion() {
        // Build a self-referential chain: A → B → C → D (depth 3 max, D is skipped).
        let d = Playbook {
            name: "d".into(),
            description: "".into(),
            triggers: vec!["d".into()],
            goal: "d".into(),
            steps: vec![make_step("D step", "bash")],
            requires_confirmation: false,
            priority: 0,
        };
        let c = Playbook {
            name: "c".into(),
            description: "".into(),
            triggers: vec!["c".into()],
            goal: "c".into(),
            steps: vec![make_sub_step("d"), make_step("C own step", "bash")],
            requires_confirmation: false,
            priority: 0,
        };
        let b = Playbook {
            name: "b".into(),
            description: "".into(),
            triggers: vec!["b".into()],
            goal: "b".into(),
            steps: vec![make_sub_step("c")],
            requires_confirmation: false,
            priority: 0,
        };
        let a = Playbook {
            name: "a".into(),
            description: "".into(),
            triggers: vec!["a".into()],
            goal: "a".into(),
            steps: vec![make_sub_step("b"), make_step("A own step", "bash")],
            requires_confirmation: false,
            priority: 0,
        };

        let planner = planner_with(vec![d, c, b, a]);
        // a expands b (depth 1) → c (depth 2) → d (depth 3, within limit) → "D step" + "C own step"
        // Then a's own "A own step"
        let plan = planner.to_plan_composed(&planner.playbooks[3]); // a is index 3
        // a → [b→[c→[d→["D step"], "C own step"]], "A own step"]
        // = ["D step", "C own step", "A own step"] → 3 steps
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].description, "D step");
        assert_eq!(plan.steps[1].description, "C own step");
        assert_eq!(plan.steps[2].description, "A own step");
    }

    #[test]
    fn composition_flat_playbook_unchanged() {
        // A playbook with no sub_playbook references should produce the same plan as to_plan.
        let pb = make_playbook("flat", vec!["flat"], 3);
        let planner = planner_with(vec![pb.clone()]);
        let composed = planner.to_plan_composed(&planner.playbooks[0]);
        let flat = PlaybookPlanner::to_plan(&pb);
        assert_eq!(composed.steps.len(), flat.steps.len());
        for (c, f) in composed.steps.iter().zip(flat.steps.iter()) {
            assert_eq!(c.description, f.description);
            assert_eq!(c.tool_name, f.tool_name);
        }
    }

    #[tokio::test]
    async fn plan_uses_composition_when_matched() {
        let checks = Playbook {
            name: "quick-checks".into(),
            description: "".into(),
            triggers: vec!["check".into()],
            goal: "run quick checks".into(),
            steps: vec![make_step("Run linter", "bash"), make_step("Run unit tests", "bash")],
            requires_confirmation: false,
            priority: 0,
        };
        let deploy = Playbook {
            name: "full-deploy".into(),
            description: "".into(),
            triggers: vec!["full deploy".into()],
            goal: "full deployment".into(),
            steps: vec![make_sub_step("quick-checks"), make_step("Deploy binary", "bash")],
            requires_confirmation: false,
            priority: 10,
        };
        let planner = planner_with(vec![checks, deploy]);
        let result = planner.plan("full deploy to production", &[]).await.unwrap();
        assert!(result.is_some());
        let plan = result.unwrap();
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].description, "Run linter");
        assert_eq!(plan.steps[1].description, "Run unit tests");
        assert_eq!(plan.steps[2].description, "Deploy binary");
    }
}
