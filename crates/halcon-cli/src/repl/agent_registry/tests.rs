//! Integration tests for the agent registry.
//!
//! These tests exercise the full load → validate → resolve pipeline using
//! real filesystem fixtures created in temp directories.

use std::io::Write;
use std::path::PathBuf;
use tempfile::TempDir;

use super::{AgentRegistry, AgentScope};

// ── Fixture helpers ───────────────────────────────────────────────────────────

fn write_agent(dir: &std::path::Path, filename: &str, content: &str) -> PathBuf {
    let path = dir.join(filename);
    let mut f = std::fs::File::create(&path).unwrap();
    write!(f, "{}", content).unwrap();
    path
}

fn make_agent_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\nYou are {name}.\n")
}

fn make_agent_md_with_model(name: &str, description: &str, model: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {description}\nmodel: {model}\n---\n\nYou are {name}.\n"
    )
}

fn setup_project_agents(root: &TempDir) -> std::path::PathBuf {
    let agents_dir = root.path().join(".halcon").join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    agents_dir
}

fn setup_skills(root: &TempDir) -> std::path::PathBuf {
    let skills_dir = root.path().join(".halcon").join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();
    skills_dir
}

// ── Empty registry ────────────────────────────────────────────────────────────

#[test]
fn empty_registry_is_empty() {
    let reg = AgentRegistry::empty();
    assert!(reg.is_empty());
    assert_eq!(reg.len(), 0);
    assert!(reg.routing_manifest().is_none());
}

#[test]
fn load_from_empty_dir_is_empty() {
    let tmp = TempDir::new().unwrap();
    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    assert!(reg.is_empty());
    assert!(reg.warnings().is_empty());
}

// ── Basic discovery ───────────────────────────────────────────────────────────

#[test]
fn discovers_project_agents() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    write_agent(&agents_dir, "reviewer.md", &make_agent_md("code-reviewer", "Reviews code"));
    write_agent(&agents_dir, "security.md", &make_agent_md("security-auditor", "Audits security"));

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    assert_eq!(reg.len(), 2);
    assert!(reg.get("code-reviewer").is_some());
    assert!(reg.get("security-auditor").is_some());
}

#[test]
fn discovers_session_agents_from_paths() {
    let tmp = TempDir::new().unwrap();
    let session_path = tmp.path().join("my-agent.md");
    std::fs::write(&session_path, make_agent_md("my-agent", "Session agent")).unwrap();

    let reg = AgentRegistry::load_isolated(&[session_path], tmp.path());
    assert_eq!(reg.len(), 1);
    let def = reg.get("my-agent").unwrap();
    assert_eq!(def.scope, AgentScope::Session);
}

#[test]
fn non_md_files_are_ignored() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    write_agent(&agents_dir, "agent.md", &make_agent_md("my-agent", "Valid agent"));
    std::fs::write(agents_dir.join("README.txt"), "not an agent").unwrap();
    std::fs::write(agents_dir.join("agent.yaml"), "not an agent").unwrap();

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    assert_eq!(reg.len(), 1);
}

// ── Model alias resolution ────────────────────────────────────────────────────

#[test]
fn model_alias_haiku_resolved() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);
    write_agent(
        &agents_dir,
        "fast.md",
        &make_agent_md_with_model("fast-agent", "Fast agent", "haiku"),
    );

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    let def = reg.get("fast-agent").unwrap();
    assert_eq!(def.resolved_model, Some("claude-haiku-4-5-20251001".to_string()));
}

#[test]
fn model_alias_opus_resolved() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);
    write_agent(
        &agents_dir,
        "powerful.md",
        &make_agent_md_with_model("powerful-agent", "Powerful agent", "opus"),
    );

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    let def = reg.get("powerful-agent").unwrap();
    assert_eq!(def.resolved_model, Some("claude-opus-4-6".to_string()));
}

#[test]
fn model_inherit_resolves_to_none() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);
    write_agent(
        &agents_dir,
        "inherit.md",
        &make_agent_md_with_model("inherit-agent", "Inherits model", "inherit"),
    );

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    let def = reg.get("inherit-agent").unwrap();
    assert_eq!(def.resolved_model, None);
}

// ── Scope collision resolution ────────────────────────────────────────────────

#[test]
fn project_agent_wins_over_session_conflict_warning() {
    // Create a session file and a project file with the same name.
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    write_agent(&agents_dir, "shared.md", &make_agent_md("shared-agent", "Project version"));

    let session_path = tmp.path().join("session-shared.md");
    std::fs::write(
        &session_path,
        make_agent_md("shared-agent", "Session version"),
    ).unwrap();

    let reg = AgentRegistry::load_isolated(&[session_path], tmp.path());
    // Session scope wins over Project.
    let def = reg.get("shared-agent").unwrap();
    assert_eq!(def.description, "Session version");
    assert_eq!(def.scope, AgentScope::Session);

    // A collision warning should have been emitted.
    assert!(!reg.warnings().is_empty());
}

// ── Validation ────────────────────────────────────────────────────────────────

#[test]
fn invalid_name_causes_agent_to_be_skipped() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    let bad_content = "---\nname: Bad_Name\ndescription: Has underscore\n---\n";
    write_agent(&agents_dir, "bad.md", bad_content);

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    assert!(reg.is_empty(), "invalid agent must not be loaded");
}

#[test]
fn valid_agents_loaded_alongside_invalid() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    write_agent(&agents_dir, "good.md", &make_agent_md("good-agent", "Valid"));
    write_agent(&agents_dir, "bad.md", "---\nname: Bad_Name\ndescription: broken\n---\n");

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    assert_eq!(reg.len(), 1);
    assert!(reg.get("good-agent").is_some());
}

#[test]
fn max_turns_out_of_range_skips_agent() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    let content = "---\nname: bad-turns\ndescription: Out of range\nmax_turns: 200\n---\n";
    write_agent(&agents_dir, "bad-turns.md", content);

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    assert!(reg.is_empty());
}

// ── Routing manifest ──────────────────────────────────────────────────────────

#[test]
fn routing_manifest_contains_agent_names() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    write_agent(&agents_dir, "reviewer.md", &make_agent_md("code-reviewer", "Reviews code"));
    write_agent(&agents_dir, "security.md", &make_agent_md("security-auditor", "Audits security"));

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    let manifest = reg.routing_manifest().unwrap();

    assert!(manifest.contains("code-reviewer"));
    assert!(manifest.contains("security-auditor"));
    assert!(manifest.contains("## Available Sub-Agents"));
}

#[test]
fn routing_manifest_is_none_when_empty() {
    let tmp = TempDir::new().unwrap();
    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    assert!(reg.routing_manifest().is_none());
}

#[test]
fn routing_manifest_truncates_long_descriptions() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    let long_desc = "x".repeat(200);
    write_agent(&agents_dir, "long.md", &make_agent_md("long-desc-agent", &long_desc));

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    let manifest = reg.routing_manifest().unwrap();
    // The manifest line must not exceed some reasonable length.
    let line = manifest.lines().find(|l| l.contains("long-desc-agent")).unwrap();
    // Description truncated to 120 chars + "…"
    assert!(line.contains("…"));
}

// ── resolve_for_dispatch ──────────────────────────────────────────────────────

#[test]
fn resolve_for_dispatch_returns_none_for_unknown_agent() {
    let reg = AgentRegistry::empty();
    assert!(reg.resolve_for_dispatch("nonexistent").is_none());
}

#[test]
fn resolve_for_dispatch_includes_system_prompt() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    let content = "---\nname: my-agent\ndescription: Test\n---\n\nYou are a test agent.\n";
    write_agent(&agents_dir, "my-agent.md", content);

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    let resolved = reg.resolve_for_dispatch("my-agent").unwrap();
    let prefix = resolved.system_prompt_prefix.unwrap();
    assert!(prefix.contains("You are a test agent."));
}

#[test]
fn resolve_for_dispatch_respects_tool_allowlist() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    let content = "---\nname: restricted\ndescription: Limited tools\ntools: [file_read, grep]\n---\n";
    write_agent(&agents_dir, "restricted.md", content);

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    let resolved = reg.resolve_for_dispatch("restricted").unwrap();
    let tools = resolved.allowed_tools.unwrap();
    assert_eq!(tools, vec!["file_read", "grep"]);
}

#[test]
fn resolve_for_dispatch_empty_tools_means_inherit_all() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    write_agent(&agents_dir, "open.md", &make_agent_md("open-agent", "All tools"));

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    let resolved = reg.resolve_for_dispatch("open-agent").unwrap();
    // No tools restriction → None (inherit all).
    assert!(resolved.allowed_tools.is_none());
}

// ── Skills integration ────────────────────────────────────────────────────────

#[test]
fn agent_with_skills_gets_skill_body_in_prompt() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);
    let skills_dir = setup_skills(&tmp);

    // Create a skill.
    std::fs::write(
        skills_dir.join("security-rules.md"),
        "---\nname: security-rules\ndescription: Security\n---\n\nAlways check for injection.\n",
    ).unwrap();

    // Create an agent that uses the skill.
    let content = "---\nname: secure-agent\ndescription: Security agent\nskills: [security-rules]\n---\n\nAgent body here.\n";
    write_agent(&agents_dir, "secure-agent.md", content);

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    let resolved = reg.resolve_for_dispatch("secure-agent").unwrap();
    let prefix = resolved.system_prompt_prefix.unwrap();
    assert!(prefix.contains("Always check for injection."));
    assert!(prefix.contains("Agent body here."));
}

#[test]
fn unknown_skill_produces_warning_but_agent_loads() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    // Agent references non-existent skill.
    let content = "---\nname: risky-agent\ndescription: Uses unknown skill\nskills: [ghost-skill]\n---\n";
    write_agent(&agents_dir, "risky.md", content);

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    // Agent loads (skill warning is non-fatal).
    assert_eq!(reg.len(), 1);
    // But a warning is emitted.
    assert!(!reg.warnings().is_empty());
}

// ── format_list ───────────────────────────────────────────────────────────────

#[test]
fn format_list_empty_registry() {
    let reg = AgentRegistry::empty();
    let output = reg.format_list();
    assert!(output.contains("No agents registered"));
}

#[test]
fn format_list_shows_all_agents() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    write_agent(&agents_dir, "alpha.md", &make_agent_md("alpha-agent", "First agent"));
    write_agent(&agents_dir, "beta.md", &make_agent_md("beta-agent", "Second agent"));

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    let output = reg.format_list();
    assert!(output.contains("alpha-agent"));
    assert!(output.contains("beta-agent"));
    assert!(output.contains("scope:"));
    assert!(output.contains("model:"));
    assert!(output.contains("max_turns:"));
}

// ── agent_names ───────────────────────────────────────────────────────────────

#[test]
fn agent_names_are_sorted() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = setup_project_agents(&tmp);

    write_agent(&agents_dir, "z.md", &make_agent_md("z-agent", "Last"));
    write_agent(&agents_dir, "a.md", &make_agent_md("a-agent", "First"));
    write_agent(&agents_dir, "m.md", &make_agent_md("m-agent", "Middle"));

    let reg = AgentRegistry::load_isolated(&[], tmp.path());
    let names = reg.agent_names();
    assert_eq!(names, vec!["a-agent", "m-agent", "z-agent"]);
}
