//! Intent-based tool selection for reducing context bloat.
//!
//! Instead of sending all 23+ tool definitions to every model invocation,
//! the ToolSelector classifies the user's message into a `TaskIntent` and
//! returns only the tools relevant to that intent. This saves context tokens
//! and helps the model focus on the right tools.

use cuervo_core::types::ToolDefinition;

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

/// Keywords that signal each intent category.
const FILE_KEYWORDS: &[&str] = &[
    "read", "write", "edit", "create file", "delete file", "list files",
    "directory", "tree", "inspect", "file",
];
const EXEC_KEYWORDS: &[&str] = &[
    "run", "execute", "compile", "test", "build", "script", "command",
    "install", "npm", "cargo", "make",
];
const SEARCH_KEYWORDS: &[&str] = &[
    "find", "search", "grep", "look for", "where is", "locate", "symbol",
];
const GIT_KEYWORDS: &[&str] = &[
    "commit", "diff", "status", "log", "branch", "push", "pull", "merge",
    "git", "staged",
];
const WEB_KEYWORDS: &[&str] = &[
    "fetch", "download", "url", "web", "http", "api call", "request",
];

/// Tool names associated with each intent.
const FILE_TOOLS: &[&str] = &[
    "file_read", "file_write", "file_edit", "file_delete", "file_inspect",
    "directory_tree",
];
const EXEC_TOOLS: &[&str] = &["bash", "background_start", "background_output", "background_kill"];
const SEARCH_TOOLS: &[&str] = &["grep", "glob", "fuzzy_find", "symbol_search"];
const GIT_TOOLS: &[&str] = &["git_status", "git_diff", "git_log", "git_add", "git_commit"];
const WEB_TOOLS: &[&str] = &["web_search", "web_fetch", "http_request"];

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
                // Short messages with no tool keywords → conversational.
                if user_message.split_whitespace().count() < 30 {
                    TaskIntent::Conversational
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
    /// are always included. If disabled or Mixed/Conversational, returns all tools.
    pub fn select_tools(
        &self,
        intent: &TaskIntent,
        all_tools: &[ToolDefinition],
    ) -> Vec<ToolDefinition> {
        if !self.enabled || *intent == TaskIntent::Mixed || *intent == TaskIntent::Conversational {
            return all_tools.to_vec();
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
}
