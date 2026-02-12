//! Tool implementations for Cuervo CLI.
//!
//! Each tool implements `cuervo_core::traits::Tool`.
//! Tools are registered in a `ToolRegistry` and invoked by the agent loop.

pub mod background;
pub mod bash;
pub mod directory_tree;
pub mod file_delete;
pub mod file_edit;
pub mod file_inspect;
pub mod file_read;
pub mod file_write;
pub mod fuzzy_find;
pub mod git;
pub mod glob_tool;
pub mod grep;
pub mod http_request;
pub mod path_security;
pub mod registry;
pub mod sandbox;
pub mod symbol_search;
pub mod syntax_check;
pub mod task_track;
pub mod web_fetch;
pub mod web_search;

#[cfg(test)]
mod tool_audit_tests;

pub use registry::ToolRegistry;

use std::sync::Arc;

use cuervo_core::types::ToolsConfig;

/// Build a default `ToolRegistry` populated with all standard tools.
pub fn default_registry(config: &ToolsConfig) -> ToolRegistry {
    full_registry(config, None)
}

/// Build a full `ToolRegistry` including background tools when a ProcessRegistry is provided.
pub fn full_registry(
    config: &ToolsConfig,
    process_registry: Option<Arc<background::ProcessRegistry>>,
) -> ToolRegistry {
    let mut reg = ToolRegistry::new();

    reg.register(Arc::new(file_read::FileReadTool::new(
        config.allowed_directories.clone(),
        config.blocked_patterns.clone(),
    )));
    reg.register(Arc::new(file_write::FileWriteTool::new(
        config.allowed_directories.clone(),
        config.blocked_patterns.clone(),
    )));
    reg.register(Arc::new(file_edit::FileEditTool::new(
        config.allowed_directories.clone(),
        config.blocked_patterns.clone(),
    )));
    reg.register(Arc::new(bash::BashTool::new(config.timeout_secs, config.sandbox.clone())));
    reg.register(Arc::new(glob_tool::GlobTool::new()));
    reg.register(Arc::new(grep::GrepTool::new()));
    reg.register(Arc::new(web_fetch::WebFetchTool::new()));
    reg.register(Arc::new(directory_tree::DirectoryTreeTool::new()));
    reg.register(Arc::new(file_inspect::FileInspectTool::new(
        config.allowed_directories.clone(),
        config.blocked_patterns.clone(),
    )));

    // File delete.
    reg.register(Arc::new(file_delete::FileDeleteTool::new(
        config.allowed_directories.clone(),
        config.blocked_patterns.clone(),
    )));

    // Task tracking, fuzzy find, symbol search, web search, and http request.
    reg.register(Arc::new(task_track::TaskTrackTool::new()));
    reg.register(Arc::new(fuzzy_find::FuzzyFindTool::new()));
    reg.register(Arc::new(symbol_search::SymbolSearchTool::new()));
    reg.register(Arc::new(web_search::WebSearchTool::new()));
    reg.register(Arc::new(http_request::HttpRequestTool::new()));

    // Git tools.
    reg.register(Arc::new(git::GitStatusTool::new()));
    reg.register(Arc::new(git::GitDiffTool::new()));
    reg.register(Arc::new(git::GitLogTool::new()));
    reg.register(Arc::new(git::GitAddTool::new()));
    reg.register(Arc::new(git::GitCommitTool::new()));

    // Background tools (require a shared ProcessRegistry).
    if let Some(proc_reg) = process_registry {
        reg.register(Arc::new(background::BackgroundStartTool::new(proc_reg.clone())));
        reg.register(Arc::new(background::BackgroundOutputTool::new(proc_reg.clone())));
        reg.register(Arc::new(background::BackgroundKillTool::new(proc_reg)));
    }

    reg
}

#[cfg(test)]
mod contract_tests {
    use cuervo_core::traits::Tool;
    use cuervo_core::types::ToolsConfig;

    use super::*;

    fn all_tools() -> Vec<Arc<dyn Tool>> {
        let config = ToolsConfig::default();
        let proc_reg = Arc::new(background::ProcessRegistry::new(5));
        vec![
            Arc::new(file_read::FileReadTool::new(
                config.allowed_directories.clone(),
                config.blocked_patterns.clone(),
            )),
            Arc::new(file_write::FileWriteTool::new(
                config.allowed_directories.clone(),
                config.blocked_patterns.clone(),
            )),
            Arc::new(file_edit::FileEditTool::new(
                config.allowed_directories.clone(),
                config.blocked_patterns.clone(),
            )),
            Arc::new(bash::BashTool::new(config.timeout_secs, config.sandbox.clone())),
            Arc::new(glob_tool::GlobTool::new()),
            Arc::new(grep::GrepTool::new()),
            Arc::new(web_fetch::WebFetchTool::new()),
            Arc::new(directory_tree::DirectoryTreeTool::new()),
            Arc::new(file_inspect::FileInspectTool::new(
                config.allowed_directories.clone(),
                config.blocked_patterns.clone(),
            )),
            Arc::new(file_delete::FileDeleteTool::new(
                config.allowed_directories.clone(),
                config.blocked_patterns.clone(),
            )),
            Arc::new(task_track::TaskTrackTool::new()),
            Arc::new(fuzzy_find::FuzzyFindTool::new()),
            Arc::new(symbol_search::SymbolSearchTool::new()),
            Arc::new(web_search::WebSearchTool::new()),
            Arc::new(http_request::HttpRequestTool::new()),
            Arc::new(git::GitStatusTool::new()),
            Arc::new(git::GitDiffTool::new()),
            Arc::new(git::GitLogTool::new()),
            Arc::new(git::GitAddTool::new()),
            Arc::new(git::GitCommitTool::new()),
            Arc::new(background::BackgroundStartTool::new(proc_reg.clone())),
            Arc::new(background::BackgroundOutputTool::new(proc_reg.clone())),
            Arc::new(background::BackgroundKillTool::new(proc_reg)),
        ]
    }

    #[test]
    fn all_tools_have_non_empty_name() {
        for tool in all_tools() {
            assert!(!tool.name().is_empty(), "tool name must not be empty");
        }
    }

    #[test]
    fn all_tools_have_non_empty_description() {
        for tool in all_tools() {
            assert!(
                !tool.description().is_empty(),
                "{}: description must not be empty",
                tool.name()
            );
        }
    }

    #[test]
    fn all_tools_have_valid_input_schema() {
        for tool in all_tools() {
            let schema = tool.input_schema();
            assert_eq!(
                schema["type"],
                "object",
                "{}: input_schema must be type 'object'",
                tool.name()
            );
            assert!(
                schema["properties"].is_object(),
                "{}: input_schema must have 'properties'",
                tool.name()
            );
            assert!(
                schema["required"].is_array(),
                "{}: input_schema must have 'required' array",
                tool.name()
            );
        }
    }

    #[test]
    fn all_tool_names_are_unique() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(names.len(), unique.len(), "tool names must be unique");
    }

    #[test]
    fn default_registry_has_core_tools() {
        let config = ToolsConfig::default();
        let reg = default_registry(&config);
        let defs = reg.tool_definitions();
        assert_eq!(defs.len(), 20, "expected 20 tools in default registry (no background)");

        assert!(reg.get("file_read").is_some());
        assert!(reg.get("file_write").is_some());
        assert!(reg.get("file_edit").is_some());
        assert!(reg.get("bash").is_some());
        assert!(reg.get("glob").is_some());
        assert!(reg.get("grep").is_some());
        assert!(reg.get("web_fetch").is_some());
        assert!(reg.get("directory_tree").is_some());
        assert!(reg.get("file_inspect").is_some());
        assert!(reg.get("git_status").is_some());
        assert!(reg.get("git_diff").is_some());
        assert!(reg.get("git_log").is_some());
        assert!(reg.get("git_add").is_some());
        assert!(reg.get("git_commit").is_some());
        assert!(reg.get("file_delete").is_some());
        assert!(reg.get("task_track").is_some());
        assert!(reg.get("fuzzy_find").is_some());
        assert!(reg.get("symbol_search").is_some());
        assert!(reg.get("web_search").is_some());
        assert!(reg.get("http_request").is_some());
    }

    #[test]
    fn full_registry_has_all_tools() {
        let config = ToolsConfig::default();
        let proc_reg = Arc::new(background::ProcessRegistry::new(5));
        let reg = full_registry(&config, Some(proc_reg));
        let defs = reg.tool_definitions();
        assert_eq!(defs.len(), 23, "expected 23 tools in full registry");

        assert!(reg.get("background_start").is_some());
        assert!(reg.get("background_output").is_some());
        assert!(reg.get("background_kill").is_some());
    }

    #[test]
    fn tool_definitions_match_registered() {
        let config = ToolsConfig::default();
        let reg = default_registry(&config);
        for def in reg.tool_definitions() {
            let tool = reg.get(&def.name).unwrap();
            assert_eq!(tool.name(), def.name);
            assert_eq!(tool.description(), def.description);
        }
    }

    #[test]
    fn destructive_tools_require_confirmation() {
        let config = ToolsConfig::default();
        let reg = default_registry(&config);
        let bash = reg.get("bash").unwrap();
        let dummy_input = cuervo_core::types::ToolInput {
            tool_use_id: "x".into(),
            arguments: serde_json::json!({}),
            working_directory: "/tmp".into(),
        };
        assert!(
            bash.requires_confirmation(&dummy_input),
            "bash tool should require confirmation"
        );

        let file_read = reg.get("file_read").unwrap();
        assert!(
            !file_read.requires_confirmation(&dummy_input),
            "file_read should not require confirmation"
        );
    }
}
