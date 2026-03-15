//! Skills loading and cycle detection.
//!
//! Skills are reusable system-prompt snippets stored in:
//! - `.halcon/skills/<name>.md`  (project scope)
//! - `~/.halcon/skills/<name>.md` (user scope)
//!
//! Project skills shadow user skills of the same name.
//!
//! # Cycle detection
//!
//! If a skill's body references another skill via `{{skill: name}}` we detect
//! cycles before expanding to prevent infinite loops.  The current
//! implementation does NOT expand skill references — it only validates that
//! no direct or transitive cycles exist among the declared `skills:` lists in
//! agent definitions.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::schema::{SkillDefinition, SkillFrontmatter};
use super::loader::split_frontmatter;

// ── Loading ───────────────────────────────────────────────────────────────────

/// Load all skills visible to an agent session.
///
/// Project skills (`.halcon/skills/`) take priority over user skills
/// (`~/.halcon/skills/`).  Returns a map from skill name → definition.
pub fn load_all_skills(working_dir: &Path) -> HashMap<String, SkillDefinition> {
    load_all_skills_with_home(working_dir, dirs::home_dir().as_deref())
}

/// Like `load_all_skills` but with an explicit user home directory.
///
/// Used by `AgentRegistry::load_impl` to support isolated test environments
/// that must not read from the real `~/.halcon/skills/` directory.
pub fn load_all_skills_with_home(
    working_dir: &Path,
    user_home: Option<&Path>,
) -> HashMap<String, SkillDefinition> {
    let mut map: HashMap<String, SkillDefinition> = HashMap::new();

    // Load user skills first (lowest priority).
    if let Some(home) = user_home {
        let user_dir = home.join(".halcon").join("skills");
        load_skills_from_dir(&user_dir, &mut map);
    }

    // Load project skills second (overrides user skills on collision).
    if let Some(halcon_dir) = find_halcon_dir(working_dir) {
        let project_dir = halcon_dir.join("skills");
        load_skills_from_dir(&project_dir, &mut map);
    }

    map
}

/// Load a single skill file, returning `None` on error.
pub fn load_skill_file(path: &Path) -> Option<SkillDefinition> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("agent_registry/skills: cannot read {:?}: {e}", path);
            return None;
        }
    };

    let (fm_str, body) = split_frontmatter(&raw);

    let fm: SkillFrontmatter = if fm_str.is_empty() {
        SkillFrontmatter::default()
    } else {
        serde_yaml::from_str(&fm_str).unwrap_or_default()
    };

    // Derive name from frontmatter or filename.
    let name = fm.name.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    });

    Some(SkillDefinition {
        name,
        description: fm.description.unwrap_or_default(),
        body: body.trim().to_string(),
        source_path: path.to_path_buf(),
    })
}

// ── Cycle detection ───────────────────────────────────────────────────────────

/// Detect circular skill references in the agent `skills:` lists.
///
/// `agent_skill_lists` maps agent name → skill names it declares.
/// `skill_skill_lists` maps skill name → skills that skill depends on
/// (currently empty; skill bodies do not support `{{skill:}}` references yet).
///
/// Returns `Ok(())` if no cycles exist, or `Err(Vec<String>)` with one
/// error message per cycle detected.
pub fn detect_skill_cycles(
    agent_skill_lists: &HashMap<String, Vec<String>>,
) -> Result<(), Vec<String>> {
    // Build adjacency: skill → skills (currently skills don't depend on each other).
    // We only check that an agent's declared skills don't form a cycle among themselves.
    // Since skills don't reference each other yet, cycles can only occur if the same
    // skill appears twice in one agent's list.
    let mut errors = Vec::new();

    for (agent_name, skills) in agent_skill_lists {
        let mut seen = HashSet::new();
        for skill in skills {
            if !seen.insert(skill.clone()) {
                errors.push(format!(
                    "agent '{}': skill '{}' appears more than once",
                    agent_name, skill
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Resolve skill names for an agent, returning the concatenated body text.
///
/// Unknown skill names are logged as warnings and skipped.
/// Returns the combined skill content to prepend to the system prompt.
pub fn resolve_skills(
    skill_names: &[String],
    skill_map: &HashMap<String, SkillDefinition>,
    agent_name: &str,
) -> String {
    let mut parts = Vec::new();

    for name in skill_names {
        match skill_map.get(name.as_str()) {
            Some(def) => {
                if !def.body.is_empty() {
                    parts.push(def.body.clone());
                }
            }
            None => {
                tracing::warn!(
                    "agent_registry: agent '{}' references unknown skill '{}'",
                    agent_name,
                    name
                );
            }
        }
    }

    parts.join("\n\n")
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn load_skills_from_dir(dir: &Path, map: &mut HashMap<String, SkillDefinition>) {
    if !dir.is_dir() {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!("agent_registry/skills: cannot read dir {:?}: {e}", dir);
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Some(skill) = load_skill_file(&path) {
                // Later inserts (project scope) overwrite earlier ones (user scope).
                map.insert(skill.name.clone(), skill);
            }
        }
    }
}

fn find_halcon_dir(working_dir: &Path) -> Option<PathBuf> {
    let mut current = working_dir;
    loop {
        let candidate = current.join(".halcon");
        if candidate.is_dir() {
            return Some(candidate);
        }
        current = current.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── load_skill_file ───────────────────────────────────────────────────────

    #[test]
    fn loads_skill_with_frontmatter() {
        use tempfile::NamedTempFile;
        use std::io::Write;

        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "name: security-guidelines").unwrap();
        writeln!(f, "description: Security best practices").unwrap();
        writeln!(f, "---").unwrap();
        writeln!(f, "").unwrap();
        writeln!(f, "Always validate user input.").unwrap();
        writeln!(f, "Never expose secrets.").unwrap();

        let skill = load_skill_file(f.path()).unwrap();
        assert_eq!(skill.name, "security-guidelines");
        assert_eq!(skill.description, "Security best practices");
        assert!(skill.body.contains("Always validate user input."));
    }

    #[test]
    fn loads_skill_without_frontmatter_uses_filename() {
        use tempfile::NamedTempFile;
        use std::io::Write;

        let mut f = NamedTempFile::with_suffix(".md").unwrap();
        writeln!(f, "This is the skill body.").unwrap();

        let skill = load_skill_file(f.path()).unwrap();
        // Name derived from filename (temp file name, not "unknown").
        assert!(!skill.name.is_empty());
        assert!(skill.body.contains("This is the skill body."));
    }

    #[test]
    fn returns_none_for_missing_file() {
        let result = load_skill_file(Path::new("/nonexistent/skill.md"));
        assert!(result.is_none());
    }

    // ── detect_skill_cycles ───────────────────────────────────────────────────

    #[test]
    fn no_cycle_when_all_unique() {
        let mut map = HashMap::new();
        map.insert("agent-a".to_string(), vec!["skill-1".to_string(), "skill-2".to_string()]);
        map.insert("agent-b".to_string(), vec!["skill-2".to_string(), "skill-3".to_string()]);

        assert!(detect_skill_cycles(&map).is_ok());
    }

    #[test]
    fn detects_duplicate_skill_in_agent() {
        let mut map = HashMap::new();
        map.insert(
            "agent-dup".to_string(),
            vec!["skill-x".to_string(), "skill-y".to_string(), "skill-x".to_string()],
        );

        let result = detect_skill_cycles(&map);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("skill-x"));
    }

    #[test]
    fn detects_multiple_duplicates_across_agents() {
        let mut map = HashMap::new();
        map.insert(
            "agent-1".to_string(),
            vec!["s1".to_string(), "s1".to_string()],
        );
        map.insert(
            "agent-2".to_string(),
            vec!["s2".to_string(), "s2".to_string()],
        );

        let result = detect_skill_cycles(&map);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().len(), 2);
    }

    #[test]
    fn empty_skill_lists_ok() {
        let map: HashMap<String, Vec<String>> = HashMap::new();
        assert!(detect_skill_cycles(&map).is_ok());
    }

    // ── resolve_skills ────────────────────────────────────────────────────────

    #[test]
    fn resolves_known_skills() {
        let mut skill_map = HashMap::new();
        skill_map.insert("skill-a".to_string(), SkillDefinition {
            name: "skill-a".to_string(),
            description: String::new(),
            body: "Content A".to_string(),
            source_path: PathBuf::from("a.md"),
        });
        skill_map.insert("skill-b".to_string(), SkillDefinition {
            name: "skill-b".to_string(),
            description: String::new(),
            body: "Content B".to_string(),
            source_path: PathBuf::from("b.md"),
        });

        let names = vec!["skill-a".to_string(), "skill-b".to_string()];
        let result = resolve_skills(&names, &skill_map, "my-agent");
        assert!(result.contains("Content A"));
        assert!(result.contains("Content B"));
    }

    #[test]
    fn skips_unknown_skills_gracefully() {
        let skill_map: HashMap<String, SkillDefinition> = HashMap::new();
        let names = vec!["nonexistent".to_string()];
        let result = resolve_skills(&names, &skill_map, "my-agent");
        assert!(result.is_empty());
    }

    #[test]
    fn empty_skill_body_not_included() {
        let mut skill_map = HashMap::new();
        skill_map.insert("empty-skill".to_string(), SkillDefinition {
            name: "empty-skill".to_string(),
            description: String::new(),
            body: String::new(),
            source_path: PathBuf::from("empty.md"),
        });

        let names = vec!["empty-skill".to_string()];
        let result = resolve_skills(&names, &skill_map, "my-agent");
        assert!(result.is_empty());
    }

    // ── load_all_skills (integration) ─────────────────────────────────────────

    #[test]
    fn project_skills_override_user_skills() {
        use tempfile::TempDir;
        use std::io::Write;

        let dir = TempDir::new().unwrap();

        // Simulate project skills dir.
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        let mut f = std::fs::File::create(skills_dir.join("common.md")).unwrap();
        writeln!(f, "---\nname: common\ndescription: project\n---\nProject version.").unwrap();

        // Manually test load_skills_from_dir with two dirs (unit level).
        let mut map: HashMap<String, SkillDefinition> = HashMap::new();

        // "user" version first
        let user_dir = dir.path().join("user_skills");
        std::fs::create_dir_all(&user_dir).unwrap();
        let mut uf = std::fs::File::create(user_dir.join("common.md")).unwrap();
        writeln!(uf, "---\nname: common\ndescription: user\n---\nUser version.").unwrap();

        load_skills_from_dir(&user_dir, &mut map);
        assert_eq!(map["common"].body, "User version.");

        // Project overwrites user.
        load_skills_from_dir(&skills_dir, &mut map);
        assert_eq!(map["common"].body, "Project version.");
    }
}
