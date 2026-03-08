//! Tool implementations for Halcon CLI.
//!
//! Each tool implements `halcon_core::traits::Tool`.
//! Tools are registered in a `ToolRegistry` and invoked by the agent loop.

pub mod archive;
pub mod background;
pub mod bash;
pub mod changelog_gen;
pub mod checksum;
pub mod ci_logs;
pub mod code_coverage;
pub mod code_metrics;
pub mod config_validator;
pub mod dep_check;
pub mod dependency_graph;
pub mod diff_apply;
pub mod directory_tree;
pub mod docker_tool;
pub mod env_inspect;
pub mod execute_test;
pub mod file_delete;
pub mod file_diff;
pub mod file_edit;
pub mod file_inspect;
pub mod file_read;
pub mod file_write;
pub mod fs_service;
pub mod fuzzy_find;
pub mod git;
pub mod git_blame;
pub mod glob_tool;
pub mod grep;
pub mod http_probe;
pub mod http_request;
pub mod json_schema_validate;
pub mod json_transform;
pub mod lint_check;
pub mod make_tool;
pub mod native_crawl;
pub mod native_index_query;
pub mod native_search;
pub mod openapi_validate;
pub mod parse_logs;
pub mod patch_apply;
pub mod path_security;
pub mod perf_analyze;
pub mod port_check;
pub mod process_list;
pub mod process_monitor;
pub mod regex_test;
pub mod registry;
pub mod sandbox;
pub mod search_memory;
pub mod secret_scan;
pub mod semantic_grep;
pub mod sql_query;
pub mod symbol_search;
pub mod syntax_check;
pub mod task_track;
pub mod template_engine;
pub mod test_data_gen;
pub mod test_run;
pub mod token_count;
pub mod url_parse;
pub mod web_fetch;
pub mod web_search;

#[cfg(test)]
mod tool_audit_tests;

pub use registry::ToolRegistry;

use std::sync::Arc;

use halcon_core::types::ToolsConfig;
use halcon_storage::Database;

/// Build a default `ToolRegistry` populated with all standard tools.
pub fn default_registry(config: &ToolsConfig) -> ToolRegistry {
    full_registry(config, None, None, None)
}

/// Build a full `ToolRegistry` including background tools and native search engine when available.
///
/// - `process_registry`: Enables background_start/output/kill tools when provided.
/// - `db`: Used by web_search (FTS5 fallback) when `search_engine` is None.
/// - `search_engine`: When provided, registers native_search + native_crawl + native_index_query
///   as the sole search interface (replacing web_search). When None, falls back to web_search (FTS5).
pub fn full_registry(
    config: &ToolsConfig,
    process_registry: Option<Arc<background::ProcessRegistry>>,
    db: Option<Arc<Database>>,
    search_engine: Option<native_search::SharedSearchEngine>,
) -> ToolRegistry {
    let fs = Arc::new(fs_service::FsService::new(
        config.allowed_directories.clone(),
        config.blocked_patterns.clone(),
    ));

    let mut reg = ToolRegistry::new();

    reg.register(Arc::new(file_read::FileReadTool::new(fs.clone())));
    reg.register(Arc::new(file_write::FileWriteTool::new(fs.clone())));
    reg.register(Arc::new(file_edit::FileEditTool::new(fs.clone())));
    reg.register(Arc::new(
        bash::BashTool::new(
            config.timeout_secs,
            config.sandbox.clone(),
            config.command_blacklist.clone(),
            config.disable_builtin_blacklist,
        )
        .expect("Failed to compile bash blacklist patterns"),
    ));
    reg.register(Arc::new(glob_tool::GlobTool::new()));
    reg.register(Arc::new(grep::GrepTool::new()));
    reg.register(Arc::new(web_fetch::WebFetchTool::new()));
    reg.register(Arc::new(directory_tree::DirectoryTreeTool::new(fs.clone())));
    reg.register(Arc::new(file_inspect::FileInspectTool::new(fs.clone())));
    reg.register(Arc::new(file_delete::FileDeleteTool::new(fs.clone())));

    // Task tracking, fuzzy find, symbol search, and http request.
    reg.register(Arc::new(task_track::TaskTrackTool::new()));
    reg.register(Arc::new(fuzzy_find::FuzzyFindTool::new()));
    reg.register(Arc::new(symbol_search::SymbolSearchTool::new()));
    reg.register(Arc::new(http_request::HttpRequestTool::new()));

    // Phase 5 new tools: diff, environment inspection, process/port utilities, JSON validation.
    reg.register(Arc::new(diff_apply::DiffApplyTool::new(fs.clone())));
    reg.register(Arc::new(env_inspect::EnvInspectTool::new()));
    reg.register(Arc::new(process_list::ProcessListTool::new()));
    reg.register(Arc::new(port_check::PortCheckTool::new()));
    reg.register(Arc::new(json_schema_validate::JsonSchemaValidateTool::new()));

    // Native semantic search suite (halcon-search: BM25 + PageRank + freshness + semantic).
    // When available, replaces web_search as the sole search interface.
    // When unavailable, falls back to web_search (FTS5-only, no API dependencies).
    if let Some(engine) = search_engine {
        reg.register(Arc::new(native_search::NativeSearchTool::new(engine.clone())));
        reg.register(Arc::new(native_crawl::NativeCrawlTool::new(engine.clone())));
        reg.register(Arc::new(native_index_query::NativeIndexQueryTool::new(engine)));
    } else {
        // Fallback: local FTS5 web_search when native search engine is not configured.
        reg.register(Arc::new(web_search::WebSearchTool::new(db.clone())));
    }

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

    // PHASE 1: Structured test execution with traceback parsing
    reg.register(Arc::new(execute_test::ExecuteTestTool::new(
        config.timeout_secs,
    )));

    // FASE6: SOTA expanded tool suite — security, analysis, infrastructure, data
    // Git extended
    reg.register(Arc::new(git::GitBranchTool::new()));
    reg.register(Arc::new(git::GitStashTool::new()));
    reg.register(Arc::new(git_blame::GitBlameTool::new(config.timeout_secs)));
    // Archive & packaging
    reg.register(Arc::new(archive::ArchiveTool::new()));
    reg.register(Arc::new(changelog_gen::ChangelogGenTool::new()));
    // Security scanning
    reg.register(Arc::new(secret_scan::SecretScanTool::new()));
    // Testing & code quality
    reg.register(Arc::new(test_run::TestRunTool::new(config.timeout_secs)));
    reg.register(Arc::new(code_coverage::CodeCoverageTool::new(config.timeout_secs)));
    reg.register(Arc::new(code_metrics::CodeMetricsTool::new()));
    reg.register(Arc::new(lint_check::LintCheckTool::new(config.timeout_secs)));
    // Code analysis
    reg.register(Arc::new(semantic_grep::SemanticGrepTool::new()));
    reg.register(Arc::new(dependency_graph::DependencyGraphTool::new(config.timeout_secs)));
    reg.register(Arc::new(dep_check::DepCheckTool::new(config.timeout_secs)));
    // Infrastructure
    reg.register(Arc::new(docker_tool::DockerTool::new(config.timeout_secs)));
    reg.register(Arc::new(process_monitor::ProcessMonitorTool::new(config.timeout_secs)));
    reg.register(Arc::new(make_tool::MakeTool::new(config.timeout_secs)));
    reg.register(Arc::new(http_probe::HttpProbeTool::new()));
    reg.register(Arc::new(ci_logs::CiLogsTool::new(config.timeout_secs)));
    reg.register(Arc::new(parse_logs::ParseLogsTool::new()));
    // Data & formatting
    reg.register(Arc::new(json_transform::JsonTransformTool::new()));
    reg.register(Arc::new(template_engine::TemplateEngineTool::new()));
    reg.register(Arc::new(sql_query::SqlQueryTool::new()));
    reg.register(Arc::new(test_data_gen::TestDataGenTool::new()));
    reg.register(Arc::new(openapi_validate::OpenApiValidateTool::new()));
    reg.register(Arc::new(config_validator::ConfigValidatorTool::new()));
    // Utilities
    reg.register(Arc::new(checksum::ChecksumTool::new()));
    reg.register(Arc::new(url_parse::UrlParseTool::new()));
    reg.register(Arc::new(regex_test::RegexTestTool::new()));
    reg.register(Arc::new(token_count::TokenCountTool::new()));
    reg.register(Arc::new(file_diff::FileDiffTool::new()));
    reg.register(Arc::new(patch_apply::PatchApplyTool::new()));
    reg.register(Arc::new(perf_analyze::PerfAnalyzeTool::new(config.timeout_secs)));

    reg
}

#[cfg(test)]
mod contract_tests {
    use halcon_core::traits::Tool;
    use halcon_core::types::ToolsConfig;

    use super::*;

    fn all_tools() -> Vec<Arc<dyn Tool>> {
        let config = ToolsConfig::default();
        let fs = Arc::new(fs_service::FsService::new(
            config.allowed_directories.clone(),
            config.blocked_patterns.clone(),
        ));
        let proc_reg = Arc::new(background::ProcessRegistry::new(5));
        vec![
            // Core file operations
            Arc::new(file_read::FileReadTool::new(fs.clone())),
            Arc::new(file_write::FileWriteTool::new(fs.clone())),
            Arc::new(file_edit::FileEditTool::new(fs.clone())),
            Arc::new(
                bash::BashTool::new(
                    config.timeout_secs,
                    config.sandbox.clone(),
                    config.command_blacklist.clone(),
                    config.disable_builtin_blacklist,
                )
                .expect("Failed to compile bash blacklist patterns"),
            ),
            Arc::new(glob_tool::GlobTool::new()),
            Arc::new(grep::GrepTool::new()),
            Arc::new(web_fetch::WebFetchTool::new()),
            Arc::new(directory_tree::DirectoryTreeTool::new(fs.clone())),
            Arc::new(file_inspect::FileInspectTool::new(fs.clone())),
            Arc::new(file_delete::FileDeleteTool::new(fs.clone())),
            Arc::new(task_track::TaskTrackTool::new()),
            Arc::new(fuzzy_find::FuzzyFindTool::new()),
            Arc::new(symbol_search::SymbolSearchTool::new()),
            Arc::new(http_request::HttpRequestTool::new()),
            Arc::new(web_search::WebSearchTool::new(None)),
            // Git core
            Arc::new(git::GitStatusTool::new()),
            Arc::new(git::GitDiffTool::new()),
            Arc::new(git::GitLogTool::new()),
            Arc::new(git::GitAddTool::new()),
            Arc::new(git::GitCommitTool::new()),
            // Phase 5 tools
            Arc::new(diff_apply::DiffApplyTool::new(fs.clone())),
            Arc::new(env_inspect::EnvInspectTool::new()),
            Arc::new(process_list::ProcessListTool::new()),
            Arc::new(port_check::PortCheckTool::new()),
            Arc::new(json_schema_validate::JsonSchemaValidateTool::new()),
            // Background tools
            Arc::new(background::BackgroundStartTool::new(proc_reg.clone())),
            Arc::new(background::BackgroundOutputTool::new(proc_reg.clone())),
            Arc::new(background::BackgroundKillTool::new(proc_reg)),
            // Phase 1 test execution
            Arc::new(execute_test::ExecuteTestTool::new(config.timeout_secs)),
            // FASE6: SOTA expanded tool suite
            Arc::new(git::GitBranchTool::new()),
            Arc::new(git::GitStashTool::new()),
            Arc::new(git_blame::GitBlameTool::new(config.timeout_secs)),
            Arc::new(archive::ArchiveTool::new()),
            Arc::new(changelog_gen::ChangelogGenTool::new()),
            Arc::new(secret_scan::SecretScanTool::new()),
            Arc::new(test_run::TestRunTool::new(config.timeout_secs)),
            Arc::new(code_coverage::CodeCoverageTool::new(config.timeout_secs)),
            Arc::new(code_metrics::CodeMetricsTool::new()),
            Arc::new(lint_check::LintCheckTool::new(config.timeout_secs)),
            Arc::new(semantic_grep::SemanticGrepTool::new()),
            Arc::new(dependency_graph::DependencyGraphTool::new(config.timeout_secs)),
            Arc::new(dep_check::DepCheckTool::new(config.timeout_secs)),
            Arc::new(docker_tool::DockerTool::new(config.timeout_secs)),
            Arc::new(process_monitor::ProcessMonitorTool::new(config.timeout_secs)),
            Arc::new(make_tool::MakeTool::new(config.timeout_secs)),
            Arc::new(http_probe::HttpProbeTool::new()),
            Arc::new(ci_logs::CiLogsTool::new(config.timeout_secs)),
            Arc::new(parse_logs::ParseLogsTool::new()),
            Arc::new(json_transform::JsonTransformTool::new()),
            Arc::new(template_engine::TemplateEngineTool::new()),
            Arc::new(sql_query::SqlQueryTool::new()),
            Arc::new(test_data_gen::TestDataGenTool::new()),
            Arc::new(openapi_validate::OpenApiValidateTool::new()),
            Arc::new(config_validator::ConfigValidatorTool::new()),
            Arc::new(checksum::ChecksumTool::new()),
            Arc::new(url_parse::UrlParseTool::new()),
            Arc::new(regex_test::RegexTestTool::new()),
            Arc::new(token_count::TokenCountTool::new()),
            Arc::new(file_diff::FileDiffTool::new()),
            Arc::new(patch_apply::PatchApplyTool::new()),
            Arc::new(perf_analyze::PerfAnalyzeTool::new(config.timeout_secs)),
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
        assert_eq!(defs.len(), 58, "expected 58 tools in default registry (no background, no search engine)");

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
        assert!(reg.get("diff_apply").is_some());
        assert!(reg.get("env_inspect").is_some());
        assert!(reg.get("process_list").is_some());
        assert!(reg.get("port_check").is_some());
        assert!(reg.get("json_schema_validate").is_some());
    }

    #[test]
    fn full_registry_has_all_tools() {
        let config = ToolsConfig::default();
        let proc_reg = Arc::new(background::ProcessRegistry::new(5));
        let reg = full_registry(&config, Some(proc_reg), None, None);
        let defs = reg.tool_definitions();
        assert_eq!(defs.len(), 61, "expected 61 tools in full registry (with background, no search engine)");

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
        let dummy_input = halcon_core::types::ToolInput {
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
