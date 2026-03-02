//! Intent-based tool selection for reducing context bloat.
//!
//! Instead of sending all 23+ tool definitions to every model invocation,
//! the ToolSelector classifies the user's message into a `TaskIntent` and
//! returns only the tools relevant to that intent. This saves context tokens
//! and helps the model focus on the right tools.

use halcon_core::types::ToolDefinition;

/// Intent classification for tool selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskIntent {
    /// File read, write, edit, delete, inspect, directory tree.
    FileOperation,
    /// Bash execution, background processes.
    CodeExecution,
    /// Grep, glob, fuzzy find, symbol search.
    Search,
    /// git_status, git_diff, git_log, git_add, git_commit.
    GitOperation,
    /// web_search, web_fetch, http_request.
    WebAccess,
    /// Simple Q&A — no tools needed.
    Conversational,
    /// Multiple intents detected or ambiguous → send all tools.
    Mixed,
}

/// Core tools that are always included regardless of intent.
const CORE_TOOLS: &[&str] = &["file_read", "bash", "grep"];

/// Message prefixes that indicate a pure conversational exchange (no task tools needed).
/// Only matched when the message is ≤ 8 words AND starts with one of these.
const GREETING_PREFIXES: &[&str] = &[
    "hello", "hi ", "hey ", "hola", "buenos", "buenas", "good morning", "good afternoon",
    "thanks", "thank you", "gracias", "ok ", "okay", "sure", "yes", "no ",
    "what is your", "what are you", "quién eres", "qué eres",
    "how are you", "cómo estás",
];

/// Keywords that signal each intent category.
const FILE_KEYWORDS: &[&str] = &[
    "read", "write", "edit", "create file", "delete file", "list files",
    "directory", "tree", "inspect", "file",
    // Analysis / exploration (en+es) — project/codebase investigation tasks
    "analyze", "analiza", "analysis", "análisis",
    "explore", "explora", "explorar",
    "examine", "examina", "examinar",
    "review", "revisa", "revisar",
    "show", "muestra", "muéstrame",
    "project", "proyecto",
    "codebase", "código", "repository", "repositorio",
    "structure", "estructura",
    "check", "verifica",
    "open", "abre",
    "what is", "qué es", "qué hay",
    "list", "lista",
    "contents", "contenido",
];
const EXEC_KEYWORDS: &[&str] = &[
    "run", "execute", "compile", "test", "build", "script", "command",
    "install", "npm", "cargo", "make",
    // Spanish execution keywords
    "ejecuta", "ejecutar", "compila", "prueba",
    "instala",
];
const SEARCH_KEYWORDS: &[&str] = &[
    "find", "search", "grep", "look for", "where is", "locate", "symbol",
    // Broader discovery — the most common cause of agent path failures
    "where", "donde", "donde está", "dónde",
    "busca", "buscar", "encuentra", "encontrar",
    "which", "cuál",
    "discover", "descubre",
    "path", "ruta", "ubicación", "location",
];
const GIT_KEYWORDS: &[&str] = &[
    "commit", "diff", "status", "log", "branch", "push", "pull", "merge",
    "git", "staged",
    // Spanish git terms
    "rama", "historial", "cambios",
];
const WEB_KEYWORDS: &[&str] = &[
    "fetch", "download", "url", "web", "http", "api call", "request",
    "search", "find online", "look up", "index", "crawl",
    // Spanish web terms
    "descarga", "busca en internet", "busca online",
];

/// Tool names associated with each intent.
const FILE_TOOLS: &[&str] = &[
    "file_read", "file_write", "file_edit", "file_delete", "file_inspect",
    "directory_tree",
];
const EXEC_TOOLS: &[&str] = &["bash", "background_start", "background_output", "background_kill"];
const SEARCH_TOOLS: &[&str] = &["grep", "glob", "fuzzy_find", "symbol_search"];
const GIT_TOOLS: &[&str] = &["git_status", "git_diff", "git_log", "git_add", "git_commit"];
const WEB_TOOLS: &[&str] = &[
    "native_search",       // Primary search (halcon-search semantic + BM25)
    "native_crawl",        // Indexing capability
    "native_index_query",  // Index introspection
    "web_fetch",           // Direct URL fetch
    "http_request",        // HTTP write operations
    "web_search",          // Fast search (halcon-search FTS5 + BM25)
];

/// Check if `text` contains `keyword` as a word (not a substring of another word).
///
/// Multi-word keywords (e.g., "create file") use simple substring matching.
/// Single-word keywords require word boundaries (whitespace or start/end of string).
fn contains_word(text: &str, keyword: &str) -> bool {
    if keyword.contains(' ') {
        // Multi-word keywords: plain substring match is fine.
        return text.contains(keyword);
    }
    // Single-word keyword: check word boundaries.
    for (i, _) in text.match_indices(keyword) {
        let before_ok = i == 0 || !text.as_bytes()[i - 1].is_ascii_alphanumeric();
        let end = i + keyword.len();
        let after_ok = end >= text.len() || !text.as_bytes()[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

pub(crate) struct ToolSelector {
    /// Whether dynamic tool selection is enabled.
    enabled: bool,
}

impl ToolSelector {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Classify task intent from user message text.
    pub fn classify_intent(&self, user_message: &str) -> TaskIntent {
        if !self.enabled {
            return TaskIntent::Mixed;
        }

        let lower = user_message.to_lowercase();

        let mut intent_count = 0u32;
        let mut last_intent = TaskIntent::Conversational;

        if FILE_KEYWORDS.iter().any(|kw| contains_word(&lower, kw)) {
            intent_count += 1;
            last_intent = TaskIntent::FileOperation;
        }
        if EXEC_KEYWORDS.iter().any(|kw| contains_word(&lower, kw)) {
            intent_count += 1;
            last_intent = TaskIntent::CodeExecution;
        }
        if SEARCH_KEYWORDS.iter().any(|kw| contains_word(&lower, kw)) {
            intent_count += 1;
            last_intent = TaskIntent::Search;
        }
        if GIT_KEYWORDS.iter().any(|kw| contains_word(&lower, kw)) {
            intent_count += 1;
            last_intent = TaskIntent::GitOperation;
        }
        if WEB_KEYWORDS.iter().any(|kw| contains_word(&lower, kw)) {
            intent_count += 1;
            last_intent = TaskIntent::WebAccess;
        }

        match intent_count {
            0 => {
                // Classify as Conversational only for true greetings/short Q&A.
                // Heuristic: ≤ 8 words AND starts with a greeting or question word
                // (no actionable task implied). Anything longer → Mixed (send all tools)
                // so the model can decide whether tools are needed.
                let word_count = user_message.split_whitespace().count();
                let is_greeting = GREETING_PREFIXES
                    .iter()
                    .any(|g| lower.starts_with(g));
                if word_count <= 8 && is_greeting {
                    TaskIntent::Conversational
                } else if word_count < 30 {
                    // Short but not a greeting → could be an implicit task (e.g.,
                    // "analyze mon-key", "show me the structure"). Use Mixed so the
                    // model receives all tools and can decide what to call.
                    TaskIntent::Mixed
                } else {
                    TaskIntent::Mixed
                }
            }
            1 => last_intent,
            _ => TaskIntent::Mixed,
        }
    }

    /// Select tools relevant to the given intent.
    ///
    /// Returns a filtered subset of `all_tools`. Core tools (file_read, bash, grep)
    /// are always included. If disabled or Mixed, returns all tools.
    /// Conversational intent returns NO tools — the model should respond directly without
    /// calling any tools, preventing greetings/simple Q&A from triggering tool execution.
    pub fn select_tools(
        &self,
        intent: &TaskIntent,
        all_tools: &[ToolDefinition],
    ) -> Vec<ToolDefinition> {
        if !self.enabled || *intent == TaskIntent::Mixed {
            return all_tools.to_vec();
        }
        // Conversational inputs (greetings, simple Q&A) → no tools.
        // Sending tool schemas to the model for "hola" causes it to proactively call
        // directory_tree, native_search, etc. due to the engineering system prompt.
        if *intent == TaskIntent::Conversational {
            return vec![];
        }

        let intent_tools: &[&str] = match intent {
            TaskIntent::FileOperation => FILE_TOOLS,
            TaskIntent::CodeExecution => EXEC_TOOLS,
            TaskIntent::Search => SEARCH_TOOLS,
            TaskIntent::GitOperation => GIT_TOOLS,
            TaskIntent::WebAccess => WEB_TOOLS,
            TaskIntent::Conversational | TaskIntent::Mixed => unreachable!(),
        };

        all_tools
            .iter()
            .filter(|tool| {
                CORE_TOOLS.contains(&tool.name.as_str())
                    || intent_tools.contains(&tool.name.as_str())
            })
            .cloned()
            .collect()
    }
}

// ── Environment-aware tool filter (FASE 5) ──────────────────────────────────

/// Tools that require a git repository to function.
const GIT_DEPENDENT_TOOLS: &[&str] = &[
    "git_status", "git_diff", "git_log", "git_add", "git_commit",
    "git_branch", "git_stash", "git_push", "git_pull",
];

/// Tools that require CI configuration to function.
const CI_DEPENDENT_TOOLS: &[&str] = &[
    "ci_logs", "ci_status", "ci_trigger", "pipeline_status",
];

/// Environment context for tool filtering.
///
/// Detected once at session start, passed to `filter_by_environment()`.
#[derive(Debug, Clone)]
pub(crate) struct EnvironmentContext {
    pub is_git_repo: bool,
    pub has_ci_config: bool,
}

impl EnvironmentContext {
    /// Detect environment context from the working directory.
    pub fn detect(working_dir: &str) -> Self {
        let path = std::path::Path::new(working_dir);
        let is_git_repo = path.join(".git").exists()
            || std::process::Command::new("git")
                .args(["rev-parse", "--is-inside-work-tree"])
                .current_dir(path)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

        let has_ci_config = path.join(".github/workflows").exists()
            || path.join(".gitlab-ci.yml").exists()
            || path.join(".circleci").exists()
            || path.join("Jenkinsfile").exists()
            || path.join(".travis.yml").exists()
            || path.join("azure-pipelines.yml").exists();

        Self { is_git_repo, has_ci_config }
    }

    /// Filter tools based on environment availability.
    ///
    /// Removes tools that depend on unavailable environment features (e.g., git
    /// tools when not in a git repo). Returns the filtered tool list.
    pub fn filter_tools(&self, tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
        tools.into_iter()
            .filter(|tool| {
                let name = tool.name.as_str();
                // Remove git tools when not in a git repo
                if !self.is_git_repo && GIT_DEPENDENT_TOOLS.contains(&name) {
                    tracing::debug!(tool = name, "EnvironmentFilter: removed (no git repo)");
                    return false;
                }
                // Remove CI tools when no CI config detected
                if !self.has_ci_config && CI_DEPENDENT_TOOLS.contains(&name) {
                    tracing::debug!(tool = name, "EnvironmentFilter: removed (no CI config)");
                    return false;
                }
                true
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("{name} tool"),
            input_schema: serde_json::json!({}),
        }
    }

    fn all_tools() -> Vec<ToolDefinition> {
        vec![
            make_tool("file_read"),
            make_tool("file_write"),
            make_tool("file_edit"),
            make_tool("bash"),
            make_tool("grep"),
            make_tool("glob"),
            make_tool("git_status"),
            make_tool("git_diff"),
            make_tool("web_search"),
            make_tool("web_fetch"),
            make_tool("symbol_search"),
        ]
    }

    #[test]
    fn classify_file_operation() {
        let s = ToolSelector::new(true);
        assert_eq!(s.classify_intent("read the config file"), TaskIntent::FileOperation);
        assert_eq!(s.classify_intent("create file foo.txt"), TaskIntent::FileOperation);
    }

    #[test]
    fn classify_code_execution() {
        let s = ToolSelector::new(true);
        assert_eq!(s.classify_intent("run the tests"), TaskIntent::CodeExecution);
        assert_eq!(s.classify_intent("compile the project"), TaskIntent::CodeExecution);
    }

    #[test]
    fn classify_search() {
        let s = ToolSelector::new(true);
        assert_eq!(s.classify_intent("find the login function"), TaskIntent::Search);
        assert_eq!(s.classify_intent("where is the config struct"), TaskIntent::Search);
    }

    #[test]
    fn classify_git_operation() {
        let s = ToolSelector::new(true);
        assert_eq!(s.classify_intent("show the git diff"), TaskIntent::GitOperation);
        assert_eq!(s.classify_intent("commit the changes"), TaskIntent::GitOperation);
    }

    #[test]
    fn classify_web_access() {
        let s = ToolSelector::new(true);
        assert_eq!(s.classify_intent("fetch the url"), TaskIntent::WebAccess);
        assert_eq!(s.classify_intent("call the http endpoint"), TaskIntent::WebAccess);
    }

    #[test]
    fn classify_conversational_short() {
        let s = ToolSelector::new(true);
        assert_eq!(s.classify_intent("hello"), TaskIntent::Conversational);
        assert_eq!(s.classify_intent("what is Rust?"), TaskIntent::Conversational);
    }

    #[test]
    fn classify_mixed_multiple_intents() {
        let s = ToolSelector::new(true);
        // "read" → FileOperation, "commit" → GitOperation → Mixed
        assert_eq!(
            s.classify_intent("read the file and commit the changes"),
            TaskIntent::Mixed
        );
    }

    #[test]
    fn classify_empty_message_conversational() {
        let s = ToolSelector::new(true);
        assert_eq!(s.classify_intent(""), TaskIntent::Conversational);
    }

    #[test]
    fn disabled_returns_mixed() {
        let s = ToolSelector::new(false);
        assert_eq!(s.classify_intent("read the file"), TaskIntent::Mixed);
    }

    #[test]
    fn select_tools_file_includes_core() {
        let s = ToolSelector::new(true);
        let tools = all_tools();
        let selected = s.select_tools(&TaskIntent::FileOperation, &tools);
        let names: Vec<&str> = selected.iter().map(|t| t.name.as_str()).collect();
        // Core tools always included
        assert!(names.contains(&"file_read"));
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"grep"));
        // File tools included
        assert!(names.contains(&"file_write"));
        assert!(names.contains(&"file_edit"));
        // Non-file tools excluded
        assert!(!names.contains(&"git_status"));
        assert!(!names.contains(&"web_search"));
    }

    #[test]
    fn select_tools_conversational_returns_empty() {
        // CRITICAL: Conversational intent must return NO tools.
        // This prevents "hola" from triggering directory_tree/native_search calls.
        let s = ToolSelector::new(true);
        let tools = all_tools();
        let selected = s.select_tools(&TaskIntent::Conversational, &tools);
        assert!(
            selected.is_empty(),
            "Conversational intent must return zero tools, got: {:?}",
            selected.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn select_tools_conversational_empty_input_returns_empty() {
        let s = ToolSelector::new(true);
        let selected = s.select_tools(&TaskIntent::Conversational, &all_tools());
        assert!(selected.is_empty());
    }

    #[test]
    fn select_tools_mixed_returns_all() {
        let s = ToolSelector::new(true);
        let tools = all_tools();
        let selected = s.select_tools(&TaskIntent::Mixed, &tools);
        assert_eq!(selected.len(), tools.len());
    }

    #[test]
    fn select_tools_disabled_returns_all() {
        let s = ToolSelector::new(false);
        let tools = all_tools();
        let selected = s.select_tools(&TaskIntent::FileOperation, &tools);
        assert_eq!(selected.len(), tools.len());
    }

    #[test]
    fn select_tools_preserves_order() {
        let s = ToolSelector::new(true);
        let tools = all_tools();
        let selected = s.select_tools(&TaskIntent::Search, &tools);
        // Should be in original order: file_read, bash (skipped since not search),
        // then grep, glob, symbol_search from the search set
        let names: Vec<&str> = selected.iter().map(|t| t.name.as_str()).collect();
        // file_read is core → included first (index 0 in original)
        // bash is core → included
        // grep is both core and search → included
        // glob is search → included
        // symbol_search is search → included
        assert_eq!(names, vec!["file_read", "bash", "grep", "glob", "symbol_search"]);
    }

    // ── EnvironmentContext tests (FASE 5) ──

    #[test]
    fn env_filter_removes_git_tools_when_no_git() {
        let ctx = EnvironmentContext { is_git_repo: false, has_ci_config: true };
        let tools = vec![
            make_tool("file_read"),
            make_tool("git_status"),
            make_tool("git_diff"),
            make_tool("bash"),
            make_tool("git_commit"),
        ];
        let filtered = ctx.filter_tools(tools);
        let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["file_read", "bash"]);
    }

    #[test]
    fn env_filter_keeps_git_tools_when_git_exists() {
        let ctx = EnvironmentContext { is_git_repo: true, has_ci_config: false };
        let tools = vec![
            make_tool("file_read"),
            make_tool("git_status"),
            make_tool("git_diff"),
            make_tool("bash"),
        ];
        let filtered = ctx.filter_tools(tools);
        assert_eq!(filtered.len(), 4);
    }

    #[test]
    fn env_filter_removes_ci_tools_when_no_ci() {
        let ctx = EnvironmentContext { is_git_repo: true, has_ci_config: false };
        let tools = vec![
            make_tool("file_read"),
            make_tool("ci_logs"),
            make_tool("ci_status"),
            make_tool("bash"),
        ];
        let filtered = ctx.filter_tools(tools);
        let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["file_read", "bash"]);
    }

    #[test]
    fn env_filter_keeps_all_when_environment_complete() {
        let ctx = EnvironmentContext { is_git_repo: true, has_ci_config: true };
        let tools = vec![
            make_tool("file_read"),
            make_tool("git_status"),
            make_tool("ci_logs"),
            make_tool("bash"),
        ];
        let filtered = ctx.filter_tools(tools);
        assert_eq!(filtered.len(), 4);
    }

    #[test]
    fn env_filter_empty_input_returns_empty() {
        let ctx = EnvironmentContext { is_git_repo: false, has_ci_config: false };
        let filtered = ctx.filter_tools(vec![]);
        assert!(filtered.is_empty());
    }
}
