//! Pre-execution capability validation — validates plan feasibility.
//!
//! Checks each plan step against available tools, blocked tools, and environment
//! capabilities BEFORE execution. Invalid steps are marked as Skipped in the
//! ExecutionTracker, preventing wasted rounds.
//!
//! Pure business logic — no I/O.

use crate::repl::tool_aliases;

/// Result of validating a single plan step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    /// Step can be executed with available tools.
    Valid,
    /// Required tool is missing; alternatives may exist.
    MissingTool {
        required: String,
        alternatives: Vec<String>,
    },
    /// Required environment feature is unavailable.
    MissingEnvironment {
        feature: String,
        detail: String,
    },
    /// Step is fundamentally impossible to execute.
    Impossible {
        reason: String,
    },
}

/// Aggregated validation for an entire plan.
#[derive(Debug, Clone)]
pub struct PlanValidation {
    /// Per-step validation results.
    pub step_results: Vec<(usize, ValidationResult)>,
    /// Whether any step failed validation.
    pub has_invalid_steps: bool,
    /// Indices of steps that should be skipped.
    pub skip_indices: Vec<usize>,
}

/// Snapshot of current environment capabilities.
#[derive(Debug, Clone)]
pub struct EnvironmentSnapshot {
    /// Whether git is available in the working directory.
    pub has_git: bool,
    /// Whether CI/CD environment is detected.
    pub has_ci: bool,
    /// Whether network access is available.
    pub has_network: bool,
}

impl Default for EnvironmentSnapshot {
    fn default() -> Self {
        Self {
            has_git: true,
            has_ci: false,
            has_network: true,
        }
    }
}

/// Tools that require git to be available.
const GIT_TOOLS: &[&str] = &[
    "git_status", "git_diff", "git_log", "git_commit", "git_add", "git_branch",
];

/// Tools that require network access.
const NETWORK_TOOLS: &[&str] = &["web_search", "web_fetch"];

/// Tools that require CI/CD environment.
const CI_TOOLS: &[&str] = &["ci_trigger", "ci_status"];

/// Validate a single plan step against available capabilities.
///
/// # Arguments
/// - `step_description`: Human-readable description of the step
/// - `primary_tool`: The main tool the step intends to use (if extractable)
/// - `available_tools`: Tools currently available to the agent
/// - `blocked_tools`: Tools blocked by guardrails `(tool_name, reason)`
/// - `env`: Current environment snapshot
pub fn validate_step(
    _step_description: &str,
    primary_tool: Option<&str>,
    available_tools: &[String],
    blocked_tools: &[(String, String)],
    env: &EnvironmentSnapshot,
) -> ValidationResult {
    let Some(tool) = primary_tool else {
        return ValidationResult::Valid;
    };

    let canonical = tool_aliases::canonicalize(tool);

    // Check blocked tools
    for (blocked, reason) in blocked_tools {
        if tool_aliases::are_equivalent(canonical, blocked) {
            return ValidationResult::Impossible {
                reason: format!("tool '{}' is blocked: {}", canonical, reason),
            };
        }
    }

    // Check environment requirements
    if GIT_TOOLS.contains(&canonical) && !env.has_git {
        return ValidationResult::MissingEnvironment {
            feature: "git".to_string(),
            detail: format!("tool '{}' requires git, but no repository detected", canonical),
        };
    }
    if NETWORK_TOOLS.contains(&canonical) && !env.has_network {
        return ValidationResult::MissingEnvironment {
            feature: "network".to_string(),
            detail: format!("tool '{}' requires network access", canonical),
        };
    }
    if CI_TOOLS.contains(&canonical) && !env.has_ci {
        return ValidationResult::MissingEnvironment {
            feature: "ci".to_string(),
            detail: format!("tool '{}' requires CI/CD environment", canonical),
        };
    }

    // Check available tools
    let available = available_tools
        .iter()
        .any(|t| tool_aliases::are_equivalent(t, canonical));

    if !available {
        let alternatives = find_alternatives(canonical, available_tools);
        return ValidationResult::MissingTool {
            required: canonical.to_string(),
            alternatives,
        };
    }

    ValidationResult::Valid
}

/// Validate an entire plan's steps.
///
/// Returns a `PlanValidation` with per-step results and skip indices.
pub fn validate_plan(
    steps: &[(String, Option<String>)], // (description, primary_tool)
    available_tools: &[String],
    blocked_tools: &[(String, String)],
    env: &EnvironmentSnapshot,
    auto_skip: bool,
) -> PlanValidation {
    let mut step_results = Vec::with_capacity(steps.len());
    let mut skip_indices = Vec::new();
    let mut has_invalid = false;

    for (idx, (desc, tool)) in steps.iter().enumerate() {
        let result = validate_step(
            desc,
            tool.as_deref(),
            available_tools,
            blocked_tools,
            env,
        );
        if result != ValidationResult::Valid {
            has_invalid = true;
            if auto_skip {
                skip_indices.push(idx);
            }
        }
        step_results.push((idx, result));
    }

    PlanValidation {
        step_results,
        has_invalid_steps: has_invalid,
        skip_indices,
    }
}

/// Find alternative tools for a missing one based on category similarity.
fn find_alternatives(missing: &str, available_tools: &[String]) -> Vec<String> {
    // Simple heuristic: look for tools in the same category
    let category = tool_category(missing);
    available_tools
        .iter()
        .filter(|t| {
            let canonical = tool_aliases::canonicalize(t);
            tool_category(canonical) == category && canonical != missing
        })
        .map(|t| tool_aliases::canonicalize(t).to_string())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect()
}

/// Simple tool categorization for alternative suggestions.
fn tool_category(tool: &str) -> &'static str {
    match tool {
        "file_read" | "file_write" | "file_edit" | "file_delete" | "read_multiple_files" => "file",
        "grep" | "glob" | "directory_tree" => "search",
        "git_status" | "git_diff" | "git_log" | "git_commit" | "git_add" | "git_branch" => "git",
        "bash" => "shell",
        "web_search" | "web_fetch" => "web",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic_tools() -> Vec<String> {
        vec![
            "file_read".to_string(),
            "file_write".to_string(),
            "grep".to_string(),
            "bash".to_string(),
            "glob".to_string(),
        ]
    }

    fn default_env() -> EnvironmentSnapshot {
        EnvironmentSnapshot::default()
    }

    #[test]
    fn phase3_capability_valid_step() {
        let result = validate_step(
            "Read main.rs",
            Some("file_read"),
            &basic_tools(),
            &[],
            &default_env(),
        );
        assert_eq!(result, ValidationResult::Valid);
    }

    #[test]
    fn phase3_capability_missing_tool() {
        let result = validate_step(
            "Search the web",
            Some("web_search"),
            &basic_tools(),
            &[],
            &default_env(),
        );
        assert!(matches!(result, ValidationResult::MissingTool { .. }));
    }

    #[test]
    fn phase3_capability_blocked_tool() {
        let blocked = vec![("file_write".to_string(), "guardrail: read-only".to_string())];
        let result = validate_step(
            "Write output",
            Some("file_write"),
            &basic_tools(),
            &blocked,
            &default_env(),
        );
        assert!(matches!(result, ValidationResult::Impossible { .. }));
    }

    #[test]
    fn phase3_capability_missing_git() {
        let env = EnvironmentSnapshot {
            has_git: false,
            ..default_env()
        };
        let result = validate_step(
            "Check git status",
            Some("git_status"),
            &["git_status".to_string()],
            &[],
            &env,
        );
        assert!(matches!(result, ValidationResult::MissingEnvironment { feature, .. } if feature == "git"));
    }

    #[test]
    fn phase3_capability_missing_network() {
        let env = EnvironmentSnapshot {
            has_network: false,
            ..default_env()
        };
        let result = validate_step(
            "Fetch URL",
            Some("web_fetch"),
            &["web_fetch".to_string()],
            &[],
            &env,
        );
        assert!(matches!(result, ValidationResult::MissingEnvironment { feature, .. } if feature == "network"));
    }

    #[test]
    fn phase3_capability_no_tool_always_valid() {
        let result = validate_step(
            "Think about the problem",
            None,
            &basic_tools(),
            &[],
            &default_env(),
        );
        assert_eq!(result, ValidationResult::Valid);
    }

    #[test]
    fn phase3_capability_alias_resolution() {
        let result = validate_step(
            "Read file",
            Some("read_file"), // alias for file_read
            &basic_tools(),
            &[],
            &default_env(),
        );
        assert_eq!(result, ValidationResult::Valid);
    }

    #[test]
    fn phase3_capability_validate_plan_all_valid() {
        let steps = vec![
            ("Read main.rs".to_string(), Some("file_read".to_string())),
            ("Search for errors".to_string(), Some("grep".to_string())),
        ];
        let plan = validate_plan(&steps, &basic_tools(), &[], &default_env(), true);
        assert!(!plan.has_invalid_steps);
        assert!(plan.skip_indices.is_empty());
    }

    #[test]
    fn phase3_capability_validate_plan_with_skip() {
        let steps = vec![
            ("Read main.rs".to_string(), Some("file_read".to_string())),
            ("Deploy to prod".to_string(), Some("ci_trigger".to_string())),
            ("Search for errors".to_string(), Some("grep".to_string())),
        ];
        let plan = validate_plan(&steps, &basic_tools(), &[], &default_env(), true);
        assert!(plan.has_invalid_steps);
        assert_eq!(plan.skip_indices, vec![1]);
    }

    #[test]
    fn phase3_capability_validate_plan_no_auto_skip() {
        let steps = vec![
            ("Deploy to prod".to_string(), Some("ci_trigger".to_string())),
        ];
        let plan = validate_plan(&steps, &basic_tools(), &[], &default_env(), false);
        assert!(plan.has_invalid_steps);
        assert!(plan.skip_indices.is_empty(), "auto_skip=false means no skip_indices");
    }

    #[test]
    fn phase3_capability_alternatives_suggested() {
        let result = validate_step(
            "Use directory tree",
            Some("directory_tree"),
            &basic_tools(), // has grep, glob but not directory_tree
            &[],
            &default_env(),
        );
        if let ValidationResult::MissingTool { alternatives, .. } = result {
            // grep and glob are in "search" category, same as directory_tree
            assert!(!alternatives.is_empty());
        } else {
            panic!("Expected MissingTool, got {:?}", result);
        }
    }

    #[test]
    fn phase3_capability_blocked_alias_detected() {
        let blocked = vec![("read_file".to_string(), "blocked".to_string())];
        let result = validate_step(
            "Read file",
            Some("file_read"), // canonical form
            &basic_tools(),
            &blocked,
            &default_env(),
        );
        assert!(matches!(result, ValidationResult::Impossible { .. }));
    }
}
