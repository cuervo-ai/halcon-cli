//! Multi-scope agent definition file discovery and frontmatter parsing.
//!
//! Discovery order (highest priority first):
//! 1. Session scope — files passed via `--agents` CLI flag
//! 2. Project scope — `.halcon/agents/*.md` (walk ancestors from CWD)
//! 3. User scope   — `~/.halcon/agents/*.md`
//!
//! On name collision across scopes, the higher-priority scope wins and a
//! warning is emitted via `tracing::warn!`.

use std::path::{Path, PathBuf};

use super::schema::{AgentDefinition, AgentFrontmatter, AgentScope, resolve_model_alias};

/// Load a single agent definition file.  Returns `None` on parse error (details
/// are logged via `tracing::warn!`).
pub fn load_agent_file(path: &Path, scope: AgentScope) -> Option<AgentDefinition> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("agent_registry: cannot read {:?}: {e}", path);
            return None;
        }
    };

    let (frontmatter, body) = split_frontmatter(&raw);

    let fm: AgentFrontmatter = match serde_yaml::from_str(&frontmatter) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("agent_registry: YAML parse error in {:?}: {e}", path);
            return None;
        }
    };

    let resolved_model = resolve_model_alias(fm.model.as_deref());

    Some(AgentDefinition {
        name: fm.name,
        description: fm.description,
        tools: fm.tools,
        disallowed_tools: fm.disallowed_tools,
        resolved_model,
        max_turns: fm.max_turns,
        memory: fm.memory,
        skills: fm.skills,
        background: fm.background,
        system_prompt: body.trim().to_string(),
        source_path: path.to_path_buf(),
        scope,
    })
}

/// Discover and load all agent definitions for a given scope.
///
/// - `Session` scope: `paths` contains explicitly supplied file paths.
/// - `Project` scope: searches `.halcon/agents/` anchored by `working_dir`.
/// - `User` scope: searches `~/.halcon/agents/`.
pub fn load_scope(scope: AgentScope, working_dir: &Path) -> Vec<AgentDefinition> {
    let dir = match scope {
        AgentScope::Session => return vec![], // session scope uses load_session_files()
        AgentScope::Project => {
            match find_halcon_agents_dir(working_dir) {
                Some(d) => d,
                None => return vec![],
            }
        }
        AgentScope::User => {
            match dirs::home_dir() {
                Some(home) => home.join(".halcon").join("agents"),
                None => return vec![],
            }
        }
    };

    load_agents_from_dir(&dir, scope)
}

/// Load agents from the User scope given an explicit home directory.
///
/// This variant is used internally by `AgentRegistry::load_impl` so that
/// tests can supply an isolated directory instead of the real `~/.halcon`.
pub fn load_scope_user(user_home: Option<&std::path::Path>) -> Vec<super::schema::AgentDefinition> {
    match user_home {
        Some(home) => {
            let dir = home.join(".halcon").join("agents");
            load_agents_from_dir(&dir, super::schema::AgentScope::User)
        }
        None => vec![],
    }
}

/// Load agents from explicitly-supplied file paths (Session scope).
pub fn load_session_files(paths: &[PathBuf]) -> Vec<AgentDefinition> {
    paths
        .iter()
        .filter_map(|p| load_agent_file(p, AgentScope::Session))
        .collect()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn load_agents_from_dir(dir: &Path, scope: AgentScope) -> Vec<AgentDefinition> {
    if !dir.is_dir() {
        return vec![];
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!("agent_registry: cannot read dir {:?}: {e}", dir);
            return vec![];
        }
    };

    let mut defs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Some(def) = load_agent_file(&path, scope) {
                defs.push(def);
            }
        }
    }

    // Sort by name for deterministic ordering.
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    defs
}

/// Walk ancestors of `working_dir` to find the first `.halcon/agents/` directory.
fn find_halcon_agents_dir(working_dir: &Path) -> Option<PathBuf> {
    let mut current = working_dir;
    loop {
        let candidate = current.join(".halcon").join("agents");
        if candidate.is_dir() {
            return Some(candidate);
        }
        current = current.parent()?;
    }
}

/// Split a Markdown file into (frontmatter_yaml, body).
///
/// The file must start with `---` on line 1.  The frontmatter ends at the
/// next `---` line.  If the file does not begin with `---`, returns
/// `("", full_content)`.
pub fn split_frontmatter(content: &str) -> (String, String) {
    let lines: Vec<&str> = content.lines().collect();

    if lines.first().map(|l| l.trim()) != Some("---") {
        return (String::new(), content.to_string());
    }

    // Find the closing `---` (line index ≥ 1).
    let close_idx = lines[1..]
        .iter()
        .position(|l| l.trim() == "---")
        .map(|i| i + 1); // adjust offset

    match close_idx {
        None => (String::new(), content.to_string()),
        Some(end) => {
            let fm = lines[1..end].join("\n");
            let body = if end + 1 < lines.len() {
                lines[end + 1..].join("\n")
            } else {
                String::new()
            };
            (fm, body)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── split_frontmatter ─────────────────────────────────────────────────────

    #[test]
    fn splits_valid_frontmatter() {
        let content = "---\nname: foo\ndescription: bar\n---\n\nBody text here.";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.contains("name: foo"));
        assert!(body.trim() == "Body text here.");
    }

    #[test]
    fn no_frontmatter_returns_full_body() {
        let content = "# Just a markdown file\n\nNo frontmatter.";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.is_empty());
        assert!(body.contains("# Just a markdown file"));
    }

    #[test]
    fn unclosed_frontmatter_returns_full_content() {
        let content = "---\nname: foo\n\nBody without closing dashes.";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.is_empty());
        assert!(body.contains("---"));
    }

    #[test]
    fn empty_body_ok() {
        let content = "---\nname: foo\ndescription: bar\n---";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.contains("name: foo"));
        assert!(body.is_empty());
    }

    #[test]
    fn body_with_multiple_sections() {
        let content = "---\nname: foo\ndescription: x\n---\n\n## Section 1\n\nText.\n\n## Section 2";
        let (_, body) = split_frontmatter(content);
        assert!(body.contains("Section 1"));
        assert!(body.contains("Section 2"));
    }

    // ── load_agent_file ───────────────────────────────────────────────────────

    #[test]
    fn loads_minimal_agent_file() {
        use tempfile::NamedTempFile;
        use std::io::Write;

        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "name: my-agent").unwrap();
        writeln!(f, "description: Does things").unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "").unwrap();
        writeln!(f, "You are a helpful agent.").unwrap();

        let def = load_agent_file(f.path(), AgentScope::Project).unwrap();
        assert_eq!(def.name, "my-agent");
        assert_eq!(def.description, "Does things");
        assert_eq!(def.system_prompt, "You are a helpful agent.");
        assert_eq!(def.scope, AgentScope::Project);
    }

    #[test]
    fn loads_agent_with_model_alias() {
        use tempfile::NamedTempFile;
        use std::io::Write;

        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "name: fast-agent").unwrap();
        writeln!(f, "description: Speed matters").unwrap();
        writeln!(f, "model: haiku").unwrap();
        writeln!(f, "---").unwrap();

        let def = load_agent_file(f.path(), AgentScope::User).unwrap();
        assert_eq!(def.resolved_model, Some("claude-haiku-4-5-20251001".to_string()));
    }

    #[test]
    fn loads_agent_with_tools_and_max_turns() {
        use tempfile::NamedTempFile;
        use std::io::Write;

        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "name: code-reviewer").unwrap();
        writeln!(f, "description: Reviews code").unwrap();
        writeln!(f, "tools: [file_read, grep, glob]").unwrap();
        writeln!(f, "max_turns: 15").unwrap();
        writeln!(f, "---").unwrap();

        let def = load_agent_file(f.path(), AgentScope::Project).unwrap();
        assert_eq!(def.tools, vec!["file_read", "grep", "glob"]);
        assert_eq!(def.max_turns, 15);
    }

    #[test]
    fn returns_none_for_bad_yaml() {
        use tempfile::NamedTempFile;
        use std::io::Write;

        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "name: [invalid yaml").unwrap(); // malformed YAML
        writeln!(f, "---").unwrap();

        let result = load_agent_file(f.path(), AgentScope::Project);
        assert!(result.is_none());
    }

    #[test]
    fn returns_none_for_missing_file() {
        let result = load_agent_file(Path::new("/nonexistent/path/agent.md"), AgentScope::User);
        assert!(result.is_none());
    }

    // ── load_agents_from_dir ──────────────────────────────────────────────────

    #[test]
    fn loads_all_md_files_from_dir() {
        use tempfile::TempDir;
        use std::io::Write;

        let dir = TempDir::new().unwrap();

        for name in &["alpha", "beta", "gamma"] {
            let path = dir.path().join(format!("{name}.md"));
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(f, "---").unwrap();
            writeln!(f, "name: {name}").unwrap();
            writeln!(f, "description: Agent {name}").unwrap();
            writeln!(f, "---").unwrap();
        }

        // Also add a non-md file that should be ignored.
        std::fs::write(dir.path().join("README.txt"), "ignored").unwrap();

        let defs = load_agents_from_dir(dir.path(), AgentScope::Project);
        assert_eq!(defs.len(), 3);
        // Sorted by name.
        assert_eq!(defs[0].name, "alpha");
        assert_eq!(defs[1].name, "beta");
        assert_eq!(defs[2].name, "gamma");
    }

    #[test]
    fn empty_dir_returns_empty_vec() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let defs = load_agents_from_dir(dir.path(), AgentScope::Project);
        assert!(defs.is_empty());
    }

    #[test]
    fn non_existent_dir_returns_empty_vec() {
        let defs = load_agents_from_dir(Path::new("/no/such/dir"), AgentScope::User);
        assert!(defs.is_empty());
    }
}
