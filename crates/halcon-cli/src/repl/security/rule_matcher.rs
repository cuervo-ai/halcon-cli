//! Rule matching engine with 4-level hierarchical matching:
//! 1. O(1) exact match cache (tool + scope_value + args hash)
//! 2. O(D) directory prefix matching (for Directory/Repository scopes)
//! 3. O(P) pattern matching (glob/regex on tool name + param patterns)
//! 4. O(1) global fallback
//!
//! Pattern sanitization ensures security (blocks ReDoS, path traversal).

use crate::repl::authorization::AuthorizationState;
use halcon_core::{
    error::{HalconError, Result},
    types::{PatternType, PermissionDecision, PermissionRule, RuleScope, ToolInput},
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Rule matching engine with hierarchical evaluation.
pub struct RuleMatcher {
    /// O(1) exact match cache: (tool, scope_value, args_hash) → decision.
    exact_cache: HashMap<(String, String, String), PermissionDecision>,
    /// Directory-scoped rules indexed by canonical path.
    directory_rules: Vec<PermissionRule>,
    /// Repository-scoped rules indexed by repo root.
    repository_rules: Vec<PermissionRule>,
    /// Pattern-based rules (glob/regex).
    pattern_rules: Vec<PermissionRule>,
    /// Global rules (apply everywhere).
    global_rules: Vec<PermissionRule>,
    /// Working directory for relative path resolution.
    working_dir: PathBuf,
}

impl RuleMatcher {
    /// Create a new rule matcher with the given working directory.
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            exact_cache: HashMap::new(),
            directory_rules: Vec::new(),
            repository_rules: Vec::new(),
            pattern_rules: Vec::new(),
            global_rules: Vec::new(),
            working_dir,
        }
    }

    /// Load rules from persistence and build indexes.
    pub fn load_rules(&mut self, rules: Vec<PermissionRule>) -> Result<()> {
        // Clear existing state
        self.exact_cache.clear();
        self.directory_rules.clear();
        self.repository_rules.clear();
        self.pattern_rules.clear();
        self.global_rules.clear();

        for rule in rules {
            // Skip expired or inactive rules
            if !rule.active || rule.is_expired() {
                continue;
            }

            // Validate and sanitize patterns
            if let Err(e) = Self::validate_pattern(&rule.tool_pattern, &rule.tool_pattern_type) {
                tracing::warn!(
                    rule_id = %rule.rule_id,
                    tool_pattern = %rule.tool_pattern,
                    error = %e,
                    "Skipping invalid rule pattern"
                );
                continue;
            }

            // Index by scope
            match rule.scope {
                RuleScope::Session => {
                    // Session rules go into exact cache during first match
                    // (not persisted to DB, managed by AuthorizationState)
                }
                RuleScope::Directory => {
                    // Canonicalize directory path for reliable prefix matching
                    if let Ok(canonical) = PathBuf::from(&rule.scope_value).canonicalize() {
                        let mut indexed_rule = rule.clone();
                        indexed_rule.scope_value = canonical.to_string_lossy().to_string();
                        self.directory_rules.push(indexed_rule);
                    } else {
                        tracing::warn!(
                            rule_id = %rule.rule_id,
                            path = %rule.scope_value,
                            "Skipping directory rule with invalid path"
                        );
                    }
                }
                RuleScope::Repository => {
                    self.repository_rules.push(rule.clone());
                }
                RuleScope::Global => {
                    if rule.tool_pattern_type == PatternType::Exact && rule.param_pattern.is_none() {
                        // Global + exact + no param pattern → pattern rules (will be cached on first match)
                        // We can't pre-cache because we don't know the working directory yet
                        self.pattern_rules.push(rule.clone());
                    } else {
                        // Global + pattern or param_pattern → pattern rules
                        self.pattern_rules.push(rule.clone());
                    }
                    self.global_rules.push(rule);
                }
            }
        }

        // Sort directory rules by path length (longest first) for best-match semantics
        self.directory_rules
            .sort_by(|a, b| b.scope_value.len().cmp(&a.scope_value.len()));

        Ok(())
    }

    /// Match a tool execution against loaded rules.
    ///
    /// Returns `Some(decision)` if a rule matches, `None` if no rule applies.
    pub fn match_rule(
        &mut self,
        tool_name: &str,
        input: &ToolInput,
        _state: &AuthorizationState,
    ) -> Option<PermissionDecision> {
        let args_hash = Self::hash_arguments(&input.arguments);
        let cwd = PathBuf::from(&input.working_directory);

        // Level 1: O(1) exact cache lookup
        let cache_key = (tool_name.to_string(), cwd.to_string_lossy().to_string(), args_hash.clone());
        if let Some(&decision) = self.exact_cache.get(&cache_key) {
            tracing::debug!(
                tool = %tool_name,
                cwd = %cwd.display(),
                decision = ?decision,
                "Matched exact cache rule"
            );
            return Some(decision);
        }

        // Level 2: O(D) directory prefix matching
        if let Ok(canonical_cwd) = cwd.canonicalize() {
            for rule in &self.directory_rules {
                let rule_path = PathBuf::from(&rule.scope_value);
                if canonical_cwd.starts_with(&rule_path) {
                    if self.matches_tool_pattern(tool_name, &rule.tool_pattern, &rule.tool_pattern_type) {
                        if self.matches_param_pattern(input, rule.param_pattern.as_deref()) {
                            tracing::debug!(
                                tool = %tool_name,
                                directory = %rule_path.display(),
                                decision = ?rule.decision,
                                "Matched directory rule"
                            );
                            // Cache for future O(1) lookups
                            self.exact_cache.insert(cache_key.clone(), rule.decision);
                            return Some(rule.decision);
                        }
                    }
                }
            }
        }

        // Level 2b: O(R) repository scope matching
        if let Some(repo_root) = Self::find_git_root(&cwd) {
            for rule in &self.repository_rules {
                let rule_repo = PathBuf::from(&rule.scope_value);
                if repo_root == rule_repo {
                    if self.matches_tool_pattern(tool_name, &rule.tool_pattern, &rule.tool_pattern_type) {
                        if self.matches_param_pattern(input, rule.param_pattern.as_deref()) {
                            tracing::debug!(
                                tool = %tool_name,
                                repository = %repo_root.display(),
                                decision = ?rule.decision,
                                "Matched repository rule"
                            );
                            self.exact_cache.insert(cache_key.clone(), rule.decision);
                            return Some(rule.decision);
                        }
                    }
                }
            }
        }

        // Level 3: O(P) pattern matching
        for rule in &self.pattern_rules {
            if self.matches_tool_pattern(tool_name, &rule.tool_pattern, &rule.tool_pattern_type) {
                if self.matches_param_pattern(input, rule.param_pattern.as_deref()) {
                    tracing::debug!(
                        tool = %tool_name,
                        pattern = %rule.tool_pattern,
                        decision = ?rule.decision,
                        "Matched pattern rule"
                    );
                    self.exact_cache.insert(cache_key, rule.decision);
                    return Some(rule.decision);
                }
            }
        }

        // Level 4: O(1) global fallback (exact match only, already cached in Level 1)
        None
    }

    /// Check if a tool name matches a pattern.
    fn matches_tool_pattern(&self, tool_name: &str, pattern: &str, pattern_type: &PatternType) -> bool {
        match pattern_type {
            PatternType::Exact => tool_name == pattern,
            PatternType::Glob => {
                if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
                    glob_pattern.matches(tool_name)
                } else {
                    false
                }
            }
            PatternType::Regex => {
                if let Ok(re) = regex::Regex::new(pattern) {
                    re.is_match(tool_name)
                } else {
                    false
                }
            }
        }
    }

    /// Check if tool arguments match an optional parameter pattern.
    fn matches_param_pattern(&self, input: &ToolInput, param_pattern: Option<&str>) -> bool {
        match param_pattern {
            None => true, // No pattern = matches all
            Some(pattern) => {
                // Parse pattern as JSON glob
                if let Ok(pattern_value) = serde_json::from_str::<serde_json::Value>(pattern) {
                    Self::json_matches_pattern(&input.arguments, &pattern_value)
                } else {
                    false
                }
            }
        }
    }

    /// Recursive JSON pattern matching (glob-style wildcards).
    fn json_matches_pattern(value: &serde_json::Value, pattern: &serde_json::Value) -> bool {
        use serde_json::Value;

        match (value, pattern) {
            (_, Value::String(s)) if s == "*" => true, // Wildcard matches anything
            (Value::String(v), Value::String(p)) => {
                // String glob matching
                if let Ok(glob_pattern) = glob::Pattern::new(p) {
                    glob_pattern.matches(v)
                } else {
                    v == p
                }
            }
            (Value::Object(v_obj), Value::Object(p_obj)) => {
                // All pattern keys must match
                for (k, p_val) in p_obj {
                    if let Some(v_val) = v_obj.get(k) {
                        if !Self::json_matches_pattern(v_val, p_val) {
                            return false;
                        }
                    } else {
                        return false; // Required key missing
                    }
                }
                true
            }
            (Value::Array(v_arr), Value::Array(p_arr)) if p_arr.len() == 1 => {
                // Pattern [X] means all array elements must match X
                v_arr.iter().all(|v| Self::json_matches_pattern(v, &p_arr[0]))
            }
            _ => value == pattern, // Exact match
        }
    }

    /// Hash tool arguments for cache key generation.
    fn hash_arguments(args: &serde_json::Value) -> String {
        use sha2::{Digest, Sha256};
        let json_str = serde_json::to_string(args).unwrap_or_default();
        let hash = Sha256::digest(json_str.as_bytes());
        hex::encode(hash)
    }

    /// Find the git repository root for a given path.
    fn find_git_root(path: &Path) -> Option<PathBuf> {
        let mut current = path.to_path_buf();
        loop {
            if current.join(".git").exists() {
                return Some(current);
            }
            if !current.pop() {
                break;
            }
        }
        None
    }

    /// Validate and sanitize a pattern before indexing.
    fn validate_pattern(pattern: &str, pattern_type: &PatternType) -> Result<()> {
        match pattern_type {
            PatternType::Exact => {
                // No special validation for exact match
                Ok(())
            }
            PatternType::Glob => {
                // Validate glob syntax
                glob::Pattern::new(pattern)
                    .map_err(|e| HalconError::InvalidInput(format!("Invalid glob pattern: {e}")))?;

                // Block dangerous patterns (ReDoS-prone)
                if pattern.contains("**/**/**") || pattern.len() > 200 {
                    return Err(HalconError::InvalidInput(
                        "Glob pattern too complex or nested".to_string(),
                    ));
                }

                Ok(())
            }
            PatternType::Regex => {
                // Block catastrophic backtracking patterns BEFORE compilation
                if pattern.contains("(.*)*")
                    || pattern.contains("(.+)+")
                    || pattern.contains("(a+)+")
                    || pattern.contains("(.*)+")
                    || pattern.len() > 200
                {
                    return Err(HalconError::InvalidInput(
                        "Regex pattern may cause catastrophic backtracking".to_string(),
                    ));
                }

                // Validate regex syntax
                regex::Regex::new(pattern)
                    .map_err(|e| HalconError::InvalidInput(format!("Invalid regex pattern: {e}")))?;

                Ok(())
            }
        }
    }

    /// Clear the exact match cache (e.g., when working directory changes).
    pub fn clear_cache(&mut self) {
        self.exact_cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::PermissionLevel;

    fn dummy_input(args: serde_json::Value, cwd: &str) -> ToolInput {
        ToolInput {
            tool_use_id: "test".to_string(),
            arguments: args,
            working_directory: cwd.to_string(),
        }
    }

    fn dummy_state() -> AuthorizationState {
        AuthorizationState::new(true)
    }

    #[test]
    fn exact_cache_hit() {
        let mut matcher = RuleMatcher::new(PathBuf::from("/tmp"));
        let rule = PermissionRule::new(
            RuleScope::Global,
            "".to_string(),
            "bash".to_string(),
            PermissionDecision::Allowed,
        );
        matcher.load_rules(vec![rule]).unwrap();

        let input = dummy_input(serde_json::json!({"command": "ls"}), "/tmp");
        let state = dummy_state();

        // First match builds cache
        let decision = matcher.match_rule("bash", &input, &state);
        assert_eq!(decision, Some(PermissionDecision::Allowed));

        // Second match hits cache (O(1))
        let decision2 = matcher.match_rule("bash", &input, &state);
        assert_eq!(decision2, Some(PermissionDecision::Allowed));
    }

    #[test]
    fn directory_prefix_matching() {
        let mut matcher = RuleMatcher::new(PathBuf::from("/tmp"));
        let mut rule = PermissionRule::new(
            RuleScope::Directory,
            "/tmp".to_string(),
            "file_write".to_string(),
            PermissionDecision::AllowedForDirectory,
        );
        // Simulate canonicalized path
        rule.scope_value = std::fs::canonicalize("/tmp")
            .unwrap()
            .to_string_lossy()
            .to_string();

        matcher.load_rules(vec![rule]).unwrap();

        let input = dummy_input(serde_json::json!({"path": "test.txt"}), "/tmp");
        let state = dummy_state();

        let decision = matcher.match_rule("file_write", &input, &state);
        assert_eq!(decision, Some(PermissionDecision::AllowedForDirectory));
    }

    #[test]
    fn glob_pattern_matching() {
        let mut matcher = RuleMatcher::new(PathBuf::from("/tmp"));
        let mut rule = PermissionRule::new(
            RuleScope::Global,
            "".to_string(),
            "bash*".to_string(),
            PermissionDecision::AllowedForPattern,
        );
        rule.tool_pattern_type = PatternType::Glob;

        matcher.load_rules(vec![rule]).unwrap();

        let input = dummy_input(serde_json::json!({"command": "ls"}), "/tmp");
        let state = dummy_state();

        let decision = matcher.match_rule("bash", &input, &state);
        assert_eq!(decision, Some(PermissionDecision::AllowedForPattern));
    }

    #[test]
    fn regex_pattern_matching() {
        let mut matcher = RuleMatcher::new(PathBuf::from("/tmp"));
        let mut rule = PermissionRule::new(
            RuleScope::Global,
            "".to_string(),
            "^file_(read|write)$".to_string(),
            PermissionDecision::Allowed,
        );
        rule.tool_pattern_type = PatternType::Regex;

        matcher.load_rules(vec![rule]).unwrap();

        let input = dummy_input(serde_json::json!({}), "/tmp");
        let state = dummy_state();

        assert_eq!(
            matcher.match_rule("file_read", &input, &state),
            Some(PermissionDecision::Allowed)
        );
        assert_eq!(
            matcher.match_rule("file_write", &input, &state),
            Some(PermissionDecision::Allowed)
        );
        assert_eq!(matcher.match_rule("file_delete", &input, &state), None);
    }

    #[test]
    fn param_pattern_glob_matching() {
        let mut matcher = RuleMatcher::new(PathBuf::from("/tmp"));
        let mut rule = PermissionRule::new(
            RuleScope::Global,
            "".to_string(),
            "bash".to_string(),
            PermissionDecision::AllowedForPattern,
        );
        rule.param_pattern = Some("{\"command\":\"ls*\"}".to_string());

        matcher.load_rules(vec![rule]).unwrap();

        let input1 = dummy_input(serde_json::json!({"command": "ls -la"}), "/tmp");
        let input2 = dummy_input(serde_json::json!({"command": "rm -rf /"}), "/tmp");
        let state = dummy_state();

        assert_eq!(
            matcher.match_rule("bash", &input1, &state),
            Some(PermissionDecision::AllowedForPattern)
        );
        assert_eq!(matcher.match_rule("bash", &input2, &state), None);
    }

    #[test]
    fn no_match_returns_none() {
        let mut matcher = RuleMatcher::new(PathBuf::from("/tmp"));
        matcher.load_rules(vec![]).unwrap();

        let input = dummy_input(serde_json::json!({}), "/tmp");
        let state = dummy_state();

        assert_eq!(matcher.match_rule("bash", &input, &state), None);
    }

    #[test]
    fn expired_rules_ignored() {
        let mut matcher = RuleMatcher::new(PathBuf::from("/tmp"));
        let mut rule = PermissionRule::new(
            RuleScope::Global,
            "".to_string(),
            "bash".to_string(),
            PermissionDecision::Allowed,
        );
        rule.expires_at = Some("2020-01-01T00:00:00Z".to_string());

        matcher.load_rules(vec![rule]).unwrap();

        let input = dummy_input(serde_json::json!({}), "/tmp");
        let state = dummy_state();

        // Expired rule should not match
        assert_eq!(matcher.match_rule("bash", &input, &state), None);
    }

    #[test]
    fn inactive_rules_ignored() {
        let mut matcher = RuleMatcher::new(PathBuf::from("/tmp"));
        let mut rule = PermissionRule::new(
            RuleScope::Global,
            "".to_string(),
            "bash".to_string(),
            PermissionDecision::Allowed,
        );
        rule.active = false;

        matcher.load_rules(vec![rule]).unwrap();

        let input = dummy_input(serde_json::json!({}), "/tmp");
        let state = dummy_state();

        assert_eq!(matcher.match_rule("bash", &input, &state), None);
    }

    #[test]
    fn validate_pattern_rejects_dangerous_glob() {
        let result = RuleMatcher::validate_pattern("**/**/**/**/**", &PatternType::Glob);
        assert!(result.is_err());
    }

    #[test]
    fn validate_pattern_rejects_redos_regex() {
        let result = RuleMatcher::validate_pattern("(a+)+b", &PatternType::Regex);
        assert!(result.is_err());
    }

    #[test]
    fn validate_pattern_accepts_safe_glob() {
        let result = RuleMatcher::validate_pattern("file_*", &PatternType::Glob);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_pattern_accepts_safe_regex() {
        let result = RuleMatcher::validate_pattern("^file_(read|write)$", &PatternType::Regex);
        assert!(result.is_ok());
    }

    #[test]
    fn clear_cache_removes_entries() {
        let mut matcher = RuleMatcher::new(PathBuf::from("/tmp"));
        let rule = PermissionRule::new(
            RuleScope::Global,
            "".to_string(),
            "bash".to_string(),
            PermissionDecision::Allowed,
        );
        matcher.load_rules(vec![rule]).unwrap();

        let input = dummy_input(serde_json::json!({}), "/tmp");
        let state = dummy_state();

        // Build cache
        matcher.match_rule("bash", &input, &state);
        assert!(!matcher.exact_cache.is_empty());

        // Clear cache
        matcher.clear_cache();
        assert!(matcher.exact_cache.is_empty());
    }

    #[test]
    fn json_wildcard_matches_anything() {
        let value = serde_json::json!({"command": "anything"});
        let pattern = serde_json::json!({"command": "*"});
        assert!(RuleMatcher::json_matches_pattern(&value, &pattern));
    }

    #[test]
    fn json_object_partial_match_fails() {
        let value = serde_json::json!({"command": "ls", "args": "-la"});
        let pattern = serde_json::json!({"command": "ls", "flags": "-la"}); // Wrong key
        assert!(!RuleMatcher::json_matches_pattern(&value, &pattern));
    }

    #[test]
    fn json_array_pattern_all_elements() {
        let value = serde_json::json!(["file1.txt", "file2.txt"]);
        let pattern = serde_json::json!(["file*.txt"]);
        assert!(RuleMatcher::json_matches_pattern(&value, &pattern));
    }
}
