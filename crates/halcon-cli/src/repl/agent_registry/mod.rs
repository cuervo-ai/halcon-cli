//! Declarative sub-agent configuration registry (Feature 4 — Frontier Roadmap 2026).
//!
//! The registry discovers agent definitions from three scopes (in priority order):
//!
//! 1. **Session** — files passed via `--agents` CLI flag
//! 2. **Project** — `.halcon/agents/*.md` (walked from CWD upward)
//! 3. **User**    — `~/.halcon/agents/*.md`
//!
//! Higher-priority scopes override lower-priority agents with the same name
//! (warning emitted).  All validation errors are collected before loading
//! (batch validation — no fail-fast).
//!
//! # Routing manifest
//!
//! The registry exposes a concise text manifest that is injected into the parent
//! agent's system prompt so it can route tasks to sub-agents by name:
//!
//! ```text
//! ## Available Sub-Agents
//!
//! - **code-reviewer** (project): Expert code reviewer. Use after any code changes.
//! - **security-auditor** (user): Deep security analysis specialist.
//! ```

pub mod loader;
pub mod schema;
pub mod skills;
pub mod validator;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use dirs;

pub use schema::{AgentDefinition, AgentScope, SkillDefinition};
pub use validator::Diagnostic;

/// The agent registry.  Loaded once at session start.
#[derive(Debug, Clone)]
pub struct AgentRegistry {
    /// All validated agent definitions, keyed by name.
    agents: HashMap<String, AgentDefinition>,
    /// All available skills, keyed by name.
    skills: HashMap<String, SkillDefinition>,
    /// Non-fatal diagnostics accumulated during loading.
    warnings: Vec<Diagnostic>,
}

impl AgentRegistry {
    /// Build an empty registry (when the feature flag is disabled).
    pub fn empty() -> Self {
        AgentRegistry {
            agents: HashMap::new(),
            skills: HashMap::new(),
            warnings: vec![],
        }
    }

    /// Discover and load all agent definitions from all scopes.
    ///
    /// `session_paths` — agent files explicitly provided at session start.
    /// `working_dir`   — project root for `.halcon/agents/` discovery.
    pub fn load(session_paths: &[PathBuf], working_dir: &Path) -> Self {
        Self::load_impl(session_paths, working_dir, dirs::home_dir().as_deref())
    }

    /// Like `load` but with an explicit user home directory.
    ///
    /// Used in tests to prevent reading from the real `~/.halcon/agents/`
    /// directory, which varies per developer machine.
    #[cfg(test)]
    pub(crate) fn load_isolated(session_paths: &[PathBuf], working_dir: &Path) -> Self {
        // Pass `working_dir` as user home — it won't have `.halcon/agents/`
        // unless the test explicitly creates it, giving full isolation.
        Self::load_impl(session_paths, working_dir, Some(working_dir))
    }

    fn load_impl(session_paths: &[PathBuf], working_dir: &Path, user_home: Option<&Path>) -> Self {
        // 1. Load skills first (needed for validation).
        let skills = skills::load_all_skills_with_home(working_dir, user_home);

        // 2. Collect raw definitions from all three scopes.
        let mut raw: Vec<AgentDefinition> = Vec::new();
        raw.extend(loader::load_session_files(session_paths));
        raw.extend(loader::load_scope(AgentScope::Project, working_dir));
        raw.extend(loader::load_scope_user(user_home));

        // 3. Resolve name collisions (higher scope wins).
        let (candidates, collision_diags) = validator::resolve_collisions(raw);

        // 4. Batch-validate each candidate.
        let known_skills: HashSet<String> = skills.keys().cloned().collect();
        let mut all_warnings: Vec<Diagnostic> = collision_diags;
        let mut agents: HashMap<String, AgentDefinition> = HashMap::new();

        for def in candidates {
            let diags = validator::validate_agent(&def, &known_skills);

            // Errors prevent loading; warnings are collected.
            let has_error = diags.iter().any(|d| d.is_error());
            for d in &diags {
                tracing::debug!("agent_registry: {d}");
            }

            if has_error {
                for d in diags {
                    if d.is_error() {
                        tracing::warn!("agent_registry: skipping '{}': {}", def.name, d.message());
                    } else {
                        all_warnings.push(d);
                    }
                }
            } else {
                all_warnings.extend(diags.into_iter().filter(|d| !d.is_error()));
                agents.insert(def.name.clone(), def);
            }
        }

        AgentRegistry { agents, skills, warnings: all_warnings }
    }

    /// Look up an agent by exact name.
    pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
        self.agents.get(name)
    }

    /// All registered agent names (sorted).
    pub fn agent_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.agents.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// True if there are no registered agents.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Number of registered agents.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Non-fatal warnings accumulated during loading.
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    /// Look up a skill by exact name.
    pub fn get_skill(&self, name: &str) -> Option<&SkillDefinition> {
        self.skills.get(name)
    }

    /// Build the routing manifest injected into the parent agent's system prompt.
    ///
    /// Returns `None` if the registry is empty (no injection needed).
    pub fn routing_manifest(&self) -> Option<String> {
        if self.agents.is_empty() {
            return None;
        }

        let mut lines = vec![
            "## Available Sub-Agents".to_string(),
            String::new(),
            "You can delegate tasks to the following specialized sub-agents by name.".to_string(),
            "To use a sub-agent, specify its name in your delegation request.".to_string(),
            String::new(),
        ];

        for name in self.agent_names() {
            let def = &self.agents[name];
            let scope_label = def.scope.to_string();
            // Truncate description to 120 chars for manifest readability.
            let desc = if def.description.len() > 120 {
                format!("{}…", &def.description[..120])
            } else {
                def.description.clone()
            };
            lines.push(format!("- **{name}** ({scope_label}): {desc}"));
        }

        Some(lines.join("\n"))
    }

    /// Resolve an agent definition to the fields needed for sub-agent dispatch.
    ///
    /// Returns `None` if the agent name is not found.
    pub fn resolve_for_dispatch(&self, name: &str) -> Option<ResolvedAgent> {
        let def = self.agents.get(name)?;

        // Resolve skill bodies.
        let skill_content = skills::resolve_skills(&def.skills, &self.skills, name);

        // Build effective system prompt (skills + agent body).
        let system_prompt_prefix = if skill_content.is_empty() {
            if def.system_prompt.is_empty() {
                None
            } else {
                Some(def.system_prompt.clone())
            }
        } else if def.system_prompt.is_empty() {
            Some(skill_content)
        } else {
            Some(format!("{}\n\n{}", skill_content, def.system_prompt))
        };

        Some(ResolvedAgent {
            name: def.name.clone(),
            model: def.resolved_model.clone(),
            allowed_tools: if def.tools.is_empty() { None } else { Some(def.tools.clone()) },
            disallowed_tools: def.disallowed_tools.clone(),
            max_turns: def.max_turns,
            system_prompt_prefix,
            background: def.background,
        })
    }

    /// Format a user-facing listing of all registered agents (for `halcon agents list`).
    pub fn format_list(&self) -> String {
        if self.agents.is_empty() {
            return "No agents registered. Add agent definitions to .halcon/agents/*.md\n".to_string();
        }

        let mut out = String::new();
        for name in self.agent_names() {
            let def = &self.agents[name];
            let model = def.resolved_model.as_deref().unwrap_or("inherit");
            let scope = def.scope.to_string();
            out.push_str(&format!("{name}\n"));
            out.push_str(&format!("  scope:     {scope}\n"));
            out.push_str(&format!("  model:     {model}\n"));
            out.push_str(&format!("  max_turns: {}\n", def.max_turns));
            out.push_str(&format!("  desc:      {}\n", def.description));
            if !def.tools.is_empty() {
                out.push_str(&format!("  tools:     {}\n", def.tools.join(", ")));
            }
            if !def.skills.is_empty() {
                out.push_str(&format!("  skills:    {}\n", def.skills.join(", ")));
            }
            out.push('\n');
        }
        out
    }
}

/// Resolved agent fields ready for sub-agent dispatch.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    /// Agent name.
    pub name: String,
    /// Resolved model ID.  `None` = inherit parent model.
    pub model: Option<String>,
    /// Tool allowlist for this agent.  `None` = inherit all.
    pub allowed_tools: Option<Vec<String>>,
    /// Tools explicitly denied.
    pub disallowed_tools: Vec<String>,
    /// Maximum agent loop turns.
    pub max_turns: u32,
    /// Combined skill content + agent body to prepend to system prompt.
    pub system_prompt_prefix: Option<String>,
    /// Whether the agent runs in the background.
    pub background: bool,
}
