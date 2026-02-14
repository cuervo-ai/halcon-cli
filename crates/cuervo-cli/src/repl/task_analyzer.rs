//! Task analyzer — classifies user queries by complexity and type.
//!
//! Uses keyword heuristics (no LLM call) to determine task complexity,
//! type, estimated steps, and a deterministic hash for experience lookup.

use sha2::{Digest, Sha256};

use cuervo_core::types::{TaskAnalysis, TaskComplexity, TaskType};

/// Heuristic-based query analyzer.
pub(crate) struct TaskAnalyzer;

impl TaskAnalyzer {
    /// Analyze a user query and produce a `TaskAnalysis`.
    pub fn analyze(query: &str) -> TaskAnalysis {
        let normalized = normalize_query(query);
        let words: Vec<&str> = normalized.split_whitespace().collect();
        let word_count = words.len();

        let complexity = classify_complexity(&words, word_count);
        let task_type = classify_task_type(&words);
        let estimated_steps = match complexity {
            TaskComplexity::Simple => 1,
            TaskComplexity::Moderate => 3,
            TaskComplexity::Complex => 5,
        };
        let keywords = extract_keywords(&words);
        let task_hash = compute_task_hash(&normalized);

        TaskAnalysis {
            complexity,
            task_type,
            estimated_steps,
            keywords,
            task_hash,
        }
    }
}

/// Normalize query: lowercase, collapse whitespace.
fn normalize_query(query: &str) -> String {
    query
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Check if a word appears as a whole word in the word list.
fn contains_word(words: &[&str], target: &str) -> bool {
    words.iter().any(|w| *w == target)
}

/// Classify complexity by word count and keyword density.
fn classify_complexity(words: &[&str], word_count: usize) -> TaskComplexity {
    let complex_keywords = ["refactor", "implement", "migrate", "redesign", "rewrite", "architect"];
    let has_complex_keyword = words.iter().any(|w| complex_keywords.contains(w));

    if word_count > 50 || has_complex_keyword {
        TaskComplexity::Complex
    } else if word_count < 15 && !has_multi_step_indicators(words) {
        TaskComplexity::Simple
    } else {
        TaskComplexity::Moderate
    }
}

/// Check for multi-step indicators like "and then", "first...then", etc.
fn has_multi_step_indicators(words: &[&str]) -> bool {
    let joined = words.join(" ");
    joined.contains("and then")
        || joined.contains("first")
        || joined.contains("after that")
        || joined.contains("step")
}

/// Classify task type from keywords.
fn classify_task_type(words: &[&str]) -> TaskType {
    let code_terms = ["code", "function", "class", "module", "struct", "impl", "method", "api", "endpoint"];
    let has_code_context = words.iter().any(|w| code_terms.contains(w));

    // Debugging (check before CodeModification since "fix" overlaps)
    let debug_terms = ["debug", "error", "bug", "crash", "panic", "traceback", "stacktrace"];
    if words.iter().any(|w| debug_terms.contains(w)) && (contains_word(words, "fix") || contains_word(words, "debug")) {
        return TaskType::Debugging;
    }

    // Git operations
    let git_terms = ["git", "commit", "push", "pull", "branch", "merge", "rebase", "stash", "cherry-pick"];
    if words.iter().any(|w| git_terms.contains(w)) {
        return TaskType::GitOperation;
    }

    // Code generation
    let gen_terms = ["write", "create", "generate", "build", "add", "new"];
    if words.iter().any(|w| gen_terms.contains(w)) && has_code_context {
        return TaskType::CodeGeneration;
    }

    // Code modification
    let mod_terms = ["fix", "edit", "modify", "update", "change", "refactor", "rename", "replace"];
    if words.iter().any(|w| mod_terms.contains(w)) {
        return TaskType::CodeModification;
    }

    // File management
    let file_terms = ["file", "move", "copy", "delete", "rename", "directory", "folder", "mkdir"];
    if words.iter().any(|w| file_terms.contains(w)) {
        return TaskType::FileManagement;
    }

    // Configuration
    let config_terms = ["config", "configure", "setup", "install", "settings", "environment"];
    if words.iter().any(|w| config_terms.contains(w)) {
        return TaskType::Configuration;
    }

    // Explanation (must check before Research)
    let explain_terms = ["explain", "what", "how", "why", "describe", "tell"];
    if words.iter().any(|w| explain_terms.contains(w)) {
        return TaskType::Explanation;
    }

    // Research
    let research_terms = ["search", "find", "look", "grep", "locate", "where"];
    if words.iter().any(|w| research_terms.contains(w)) {
        return TaskType::Research;
    }

    // Code generation fallback (write/create without explicit code terms)
    if words.iter().any(|w| gen_terms.contains(w)) {
        return TaskType::CodeGeneration;
    }

    TaskType::General
}

/// Extract action keywords from the query.
fn extract_keywords(words: &[&str]) -> Vec<String> {
    let action_words = [
        "write", "create", "generate", "build", "add", "new",
        "fix", "edit", "modify", "update", "change", "refactor",
        "delete", "remove", "rename", "move", "copy",
        "search", "find", "grep", "explain",
        "debug", "test", "run", "deploy",
        "commit", "push", "pull", "merge",
        "install", "configure", "setup",
        "implement", "migrate", "redesign", "rewrite",
    ];
    words
        .iter()
        .filter(|w| action_words.contains(w))
        .map(|w| w.to_string())
        .collect()
}

/// SHA-256 of normalized query for experience matching.
fn compute_task_hash(normalized: &str) -> String {
    hex::encode(Sha256::digest(normalized.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_query_classification() {
        let analysis = TaskAnalyzer::analyze("hello");
        assert_eq!(analysis.complexity, TaskComplexity::Simple);
    }

    #[test]
    fn moderate_query_classification() {
        let analysis = TaskAnalyzer::analyze("fix the authentication bug in the login handler and then update the tests to cover the new behavior");
        assert_eq!(analysis.complexity, TaskComplexity::Moderate);
    }

    #[test]
    fn complex_query_with_keyword() {
        let analysis = TaskAnalyzer::analyze("refactor the entire database layer");
        assert_eq!(analysis.complexity, TaskComplexity::Complex);
    }

    #[test]
    fn complex_query_by_length() {
        let long_query = "I need you to take all of the existing authentication code and completely rewrite it from scratch using a new pattern that includes JWT tokens and refresh tokens and session management and rate limiting and IP whitelisting and two factor authentication and audit logging and all of the related tests";
        let analysis = TaskAnalyzer::analyze(long_query);
        assert_eq!(analysis.complexity, TaskComplexity::Complex);
    }

    #[test]
    fn task_type_code_generation() {
        let analysis = TaskAnalyzer::analyze("write a new function to parse config files");
        assert_eq!(analysis.task_type, TaskType::CodeGeneration);
    }

    #[test]
    fn task_type_code_modification() {
        let analysis = TaskAnalyzer::analyze("update the header component to use flexbox");
        assert_eq!(analysis.task_type, TaskType::CodeModification);
    }

    #[test]
    fn task_type_debugging() {
        let analysis = TaskAnalyzer::analyze("debug the error in the auth module and fix it");
        assert_eq!(analysis.task_type, TaskType::Debugging);
    }

    #[test]
    fn task_type_git_operation() {
        let analysis = TaskAnalyzer::analyze("commit all changes and push to main");
        assert_eq!(analysis.task_type, TaskType::GitOperation);
    }

    #[test]
    fn task_type_file_management() {
        let analysis = TaskAnalyzer::analyze("move the old logs to an archive directory");
        assert_eq!(analysis.task_type, TaskType::FileManagement);
    }

    #[test]
    fn task_type_configuration() {
        let analysis = TaskAnalyzer::analyze("configure the CI pipeline settings");
        assert_eq!(analysis.task_type, TaskType::Configuration);
    }

    #[test]
    fn task_type_explanation() {
        let analysis = TaskAnalyzer::analyze("explain how the context pipeline works");
        assert_eq!(analysis.task_type, TaskType::Explanation);
    }

    #[test]
    fn task_type_research() {
        let analysis = TaskAnalyzer::analyze("search for all uses of deprecated API");
        assert_eq!(analysis.task_type, TaskType::Research);
    }

    #[test]
    fn task_type_general() {
        let analysis = TaskAnalyzer::analyze("hello there");
        assert_eq!(analysis.task_type, TaskType::General);
    }

    #[test]
    fn estimated_steps_by_complexity() {
        let simple = TaskAnalyzer::analyze("hello");
        assert_eq!(simple.estimated_steps, 1);

        let moderate = TaskAnalyzer::analyze("fix the authentication bug in the login handler and then update all the tests to cover the new behavior");
        assert_eq!(moderate.estimated_steps, 3);

        let complex = TaskAnalyzer::analyze("refactor the database module");
        assert_eq!(complex.estimated_steps, 5);
    }

    #[test]
    fn task_hash_deterministic() {
        let a = TaskAnalyzer::analyze("fix the bug");
        let b = TaskAnalyzer::analyze("fix the bug");
        assert_eq!(a.task_hash, b.task_hash);
    }

    #[test]
    fn task_hash_normalized() {
        let a = TaskAnalyzer::analyze("  Fix   The  Bug  ");
        let b = TaskAnalyzer::analyze("fix the bug");
        assert_eq!(a.task_hash, b.task_hash);
    }

    #[test]
    fn empty_query_general_simple() {
        let analysis = TaskAnalyzer::analyze("");
        assert_eq!(analysis.task_type, TaskType::General);
        assert_eq!(analysis.complexity, TaskComplexity::Simple);
    }

    #[test]
    fn whitespace_query_general_simple() {
        let analysis = TaskAnalyzer::analyze("   ");
        assert_eq!(analysis.task_type, TaskType::General);
        assert_eq!(analysis.complexity, TaskComplexity::Simple);
    }

    #[test]
    fn keywords_extracted() {
        let analysis = TaskAnalyzer::analyze("write a function and test it then deploy");
        assert!(analysis.keywords.contains(&"write".to_string()));
        assert!(analysis.keywords.contains(&"test".to_string()));
        assert!(analysis.keywords.contains(&"deploy".to_string()));
    }
}
