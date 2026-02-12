//! Phase 2 — Comprehensive Tool Audit Tests
//!
//! Systematic validation of ALL 23 tools:
//! - Valid input execution
//! - Invalid input rejection
//! - Edge cases (empty, max, boundary)
//! - Determinism (repeated calls yield same result)
//! - Schema contract (JSON schema structure)
//! - Error handling (graceful failures, no panics)
//! - tool_use_id propagation
//! - Metadata correctness

#[cfg(test)]
mod audit {
    use std::sync::Arc;

    use cuervo_core::traits::Tool;
    use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput, ToolsConfig};
    use serde_json::json;

    use crate::background::{BackgroundKillTool, BackgroundOutputTool, BackgroundStartTool, ProcessRegistry};
    use crate::bash::BashTool;
    use crate::directory_tree::DirectoryTreeTool;
    use crate::file_delete::FileDeleteTool;
    use crate::file_edit::FileEditTool;
    use crate::file_inspect::FileInspectTool;
    use crate::file_read::FileReadTool;
    use crate::file_write::FileWriteTool;
    use crate::fuzzy_find::FuzzyFindTool;
    use crate::glob_tool::GlobTool;
    use crate::grep::GrepTool;
    use crate::http_request::HttpRequestTool;
    use crate::symbol_search::SymbolSearchTool;
    use crate::task_track::TaskTrackTool;
    use crate::web_fetch::WebFetchTool;
    use crate::web_search::WebSearchTool;
    use crate::git::{GitStatusTool, GitDiffTool, GitLogTool, GitAddTool, GitCommitTool};

    // ===== Helpers =====

    fn input(id: &str, args: serde_json::Value, wd: &str) -> ToolInput {
        ToolInput {
            tool_use_id: id.into(),
            arguments: args,
            working_directory: wd.into(),
        }
    }

    fn tmp_input(id: &str, args: serde_json::Value, dir: &tempfile::TempDir) -> ToolInput {
        input(id, args, dir.path().to_str().unwrap())
    }

    /// Validate that a schema follows the tool contract.
    fn assert_valid_schema(tool: &dyn Tool) {
        let s = tool.input_schema();
        assert_eq!(s["type"], "object", "{}: schema type must be 'object'", tool.name());
        assert!(s["properties"].is_object(), "{}: schema must have properties", tool.name());
        assert!(s["required"].is_array(), "{}: schema must have required array", tool.name());
    }

    /// Validate tool_use_id propagation.
    fn assert_id_propagated(output: &ToolOutput, expected: &str) {
        assert_eq!(output.tool_use_id, expected, "tool_use_id not propagated correctly");
    }

    // ============================================================
    //  SECTION 1: FILE_READ AUDIT
    // ============================================================
    mod file_read_audit {
        use super::*;

        fn tool() -> FileReadTool {
            FileReadTool::new(vec![], vec![])
        }

        #[tokio::test]
        async fn valid_read_returns_content_and_metadata() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("hello.txt");
            std::fs::write(&f, "line1\nline2\nline3").unwrap();

            let out = tool().execute(tmp_input("fr-1", json!({"path": f.to_str().unwrap()}), &dir)).await.unwrap();
            assert_id_propagated(&out, "fr-1");
            assert!(!out.is_error);
            assert!(out.content.contains("line1"));
            assert!(out.content.contains("line3"));
            let meta = out.metadata.as_ref().unwrap();
            assert_eq!(meta["total_lines"], 3);
        }

        #[tokio::test]
        async fn empty_file_returns_empty_content() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("empty.txt");
            std::fs::write(&f, "").unwrap();

            let out = tool().execute(tmp_input("fr-2", json!({"path": f.to_str().unwrap()}), &dir)).await.unwrap();
            assert!(!out.is_error);
            let meta = out.metadata.as_ref().unwrap();
            assert_eq!(meta["total_lines"], 0);
        }

        #[tokio::test]
        async fn unicode_content_preserved() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("unicode.txt");
            std::fs::write(&f, "日本語\nEmoji: 🦀\nAccents: café").unwrap();

            let out = tool().execute(tmp_input("fr-3", json!({"path": f.to_str().unwrap()}), &dir)).await.unwrap();
            assert!(!out.is_error);
            assert!(out.content.contains("日本語"));
            assert!(out.content.contains("🦀"));
            assert!(out.content.contains("café"));
        }

        #[tokio::test]
        async fn offset_beyond_file_returns_empty() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("short.txt");
            std::fs::write(&f, "one\ntwo").unwrap();

            let out = tool().execute(tmp_input("fr-4", json!({"path": f.to_str().unwrap(), "offset": 100}), &dir)).await.unwrap();
            assert!(!out.is_error);
            assert!(out.content.is_empty() || out.content.trim().is_empty());
        }

        #[tokio::test]
        async fn limit_zero_reads_all() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("all.txt");
            std::fs::write(&f, "a\nb\nc").unwrap();

            let out = tool().execute(tmp_input("fr-5", json!({"path": f.to_str().unwrap(), "limit": 0}), &dir)).await.unwrap();
            assert!(!out.is_error);
            assert!(out.content.contains("a"));
            assert!(out.content.contains("c"));
        }

        #[tokio::test]
        async fn nonexistent_file_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("fr-6", json!({"path": "/nonexistent/file.txt"}), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn missing_path_arg_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("fr-7", json!({}), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn null_path_arg_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("fr-8", json!({"path": null}), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn numeric_path_arg_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("fr-9", json!({"path": 42}), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn deterministic_repeated_reads() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("det.txt");
            std::fs::write(&f, "stable content").unwrap();

            let t = tool();
            let out1 = t.execute(tmp_input("d1", json!({"path": f.to_str().unwrap()}), &dir)).await.unwrap();
            let out2 = t.execute(tmp_input("d2", json!({"path": f.to_str().unwrap()}), &dir)).await.unwrap();
            assert_eq!(out1.content, out2.content);
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }

        #[test]
        fn permission_level_is_readonly() {
            assert_eq!(tool().permission_level(), PermissionLevel::ReadOnly);
        }
    }

    // ============================================================
    //  SECTION 2: FILE_WRITE AUDIT
    // ============================================================
    mod file_write_audit {
        use super::*;

        fn tool() -> FileWriteTool {
            FileWriteTool::new(vec![], vec![])
        }

        #[tokio::test]
        async fn valid_write_creates_file() {
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("new.txt");

            let out = tool().execute(tmp_input("fw-1", json!({"path": p.to_str().unwrap(), "content": "hello"}), &dir)).await.unwrap();
            assert_id_propagated(&out, "fw-1");
            assert!(!out.is_error);
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello");
            let meta = out.metadata.as_ref().unwrap();
            assert_eq!(meta["bytes_written"], 5);
        }

        #[tokio::test]
        async fn creates_parent_directories() {
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("a/b/c/deep.txt");

            let out = tool().execute(tmp_input("fw-2", json!({"path": p.to_str().unwrap(), "content": "deep"}), &dir)).await.unwrap();
            assert!(!out.is_error);
            assert!(p.exists());
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "deep");
        }

        #[tokio::test]
        async fn overwrites_existing_file() {
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("existing.txt");
            std::fs::write(&p, "old content").unwrap();

            let out = tool().execute(tmp_input("fw-3", json!({"path": p.to_str().unwrap(), "content": "new content"}), &dir)).await.unwrap();
            assert!(!out.is_error);
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "new content");
        }

        #[tokio::test]
        async fn empty_content_creates_empty_file() {
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("empty.txt");

            let out = tool().execute(tmp_input("fw-4", json!({"path": p.to_str().unwrap(), "content": ""}), &dir)).await.unwrap();
            assert!(!out.is_error);
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "");
        }

        #[tokio::test]
        async fn unicode_content_written_correctly() {
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("uni.txt");

            let out = tool().execute(tmp_input("fw-5", json!({"path": p.to_str().unwrap(), "content": "日本語 🦀"}), &dir)).await.unwrap();
            assert!(!out.is_error);
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "日本語 🦀");
        }

        #[tokio::test]
        async fn missing_path_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("fw-6", json!({"content": "data"}), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn missing_content_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("nodata.txt");
            let result = tool().execute(tmp_input("fw-7", json!({"path": p.to_str().unwrap()}), &dir)).await;
            assert!(result.is_err());
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }

        #[test]
        fn permission_is_destructive() {
            // file_write can overwrite existing files — requires user confirmation.
            assert_eq!(tool().permission_level(), PermissionLevel::Destructive);
        }
    }

    // ============================================================
    //  SECTION 3: FILE_EDIT AUDIT
    // ============================================================
    mod file_edit_audit {
        use super::*;

        fn tool() -> FileEditTool {
            FileEditTool::new(vec![], vec![])
        }

        #[tokio::test]
        async fn valid_replace() {
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("edit.txt");
            std::fs::write(&p, "Hello World").unwrap();

            let out = tool().execute(tmp_input("fe-1", json!({
                "path": p.to_str().unwrap(),
                "old_string": "World",
                "new_string": "Rust"
            }), &dir)).await.unwrap();
            assert_id_propagated(&out, "fe-1");
            assert!(!out.is_error);
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "Hello Rust");
            let meta = out.metadata.as_ref().unwrap();
            assert_eq!(meta["replacements"], 1);
        }

        #[tokio::test]
        async fn replace_all_flag() {
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("multi.txt");
            std::fs::write(&p, "aaa bbb aaa ccc aaa").unwrap();

            let out = tool().execute(tmp_input("fe-2", json!({
                "path": p.to_str().unwrap(),
                "old_string": "aaa",
                "new_string": "zzz",
                "replace_all": true
            }), &dir)).await.unwrap();
            assert!(!out.is_error);
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "zzz bbb zzz ccc zzz");
            let meta = out.metadata.as_ref().unwrap();
            assert_eq!(meta["replacements"], 3);
        }

        #[tokio::test]
        async fn non_unique_match_without_replace_all_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("dup.txt");
            std::fs::write(&p, "foo bar foo baz").unwrap();

            let out = tool().execute(tmp_input("fe-3", json!({
                "path": p.to_str().unwrap(),
                "old_string": "foo",
                "new_string": "qux"
            }), &dir)).await.unwrap();
            // Should error because "foo" appears twice and replace_all is false.
            assert!(out.is_error);
        }

        #[tokio::test]
        async fn old_string_not_found() {
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("nf.txt");
            std::fs::write(&p, "hello world").unwrap();

            let out = tool().execute(tmp_input("fe-4", json!({
                "path": p.to_str().unwrap(),
                "old_string": "NOTHERE",
                "new_string": "x"
            }), &dir)).await.unwrap();
            assert!(out.is_error);
        }

        #[tokio::test]
        async fn edit_nonexistent_file_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("fe-5", json!({
                "path": "/nonexistent/file.txt",
                "old_string": "a",
                "new_string": "b"
            }), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn missing_old_string_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("mo.txt");
            std::fs::write(&p, "content").unwrap();

            let result = tool().execute(tmp_input("fe-6", json!({
                "path": p.to_str().unwrap(),
                "new_string": "x"
            }), &dir)).await;
            assert!(result.is_err());
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }
    }

    // ============================================================
    //  SECTION 4: FILE_DELETE AUDIT
    // ============================================================
    mod file_delete_audit {
        use super::*;

        fn tool() -> FileDeleteTool {
            FileDeleteTool::new(vec![], vec![])
        }

        #[tokio::test]
        async fn valid_delete() {
            let dir = tempfile::TempDir::new().unwrap();
            let p = dir.path().join("del.txt");
            std::fs::write(&p, "doomed").unwrap();

            let out = tool().execute(tmp_input("fd-1", json!({"path": p.to_str().unwrap()}), &dir)).await.unwrap();
            assert_id_propagated(&out, "fd-1");
            assert!(!out.is_error);
            assert!(!p.exists());
        }

        #[tokio::test]
        async fn delete_nonexistent_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("fd-2", json!({"path": "/nonexistent/file.txt"}), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn delete_directory_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let subdir = dir.path().join("subdir");
            std::fs::create_dir(&subdir).unwrap();

            let result = tool().execute(tmp_input("fd-3", json!({"path": subdir.to_str().unwrap()}), &dir)).await;
            // file_delete should not delete directories.
            assert!(result.is_err() || {
                let out = result.unwrap();
                out.is_error
            });
        }

        #[tokio::test]
        async fn missing_path_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("fd-4", json!({}), &dir)).await;
            assert!(result.is_err());
        }

        #[test]
        fn permission_is_destructive() {
            assert_eq!(tool().permission_level(), PermissionLevel::Destructive);
        }

        #[test]
        fn requires_confirmation() {
            let dummy = ToolInput {
                tool_use_id: "x".into(),
                arguments: json!({}),
                working_directory: "/tmp".into(),
            };
            assert!(tool().requires_confirmation(&dummy));
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }
    }

    // ============================================================
    //  SECTION 5: BASH AUDIT
    // ============================================================
    mod bash_audit {
        use super::*;
        use cuervo_core::types::SandboxConfig;
        use cuervo_core::error::CuervoError;

        fn tool() -> BashTool {
            BashTool::new(120, SandboxConfig::default())
        }

        #[tokio::test]
        async fn valid_echo() {
            let out = tool().execute(input("b-1", json!({"command": "echo hello"}), "/tmp")).await.unwrap();
            assert_id_propagated(&out, "b-1");
            assert!(!out.is_error);
            assert!(out.content.contains("hello"));
            let meta = out.metadata.as_ref().unwrap();
            assert_eq!(meta["exit_code"], 0);
        }

        #[tokio::test]
        async fn exit_code_propagated() {
            let out = tool().execute(input("b-2", json!({"command": "exit 7"}), "/tmp")).await.unwrap();
            assert!(out.is_error);
            let meta = out.metadata.as_ref().unwrap();
            assert_eq!(meta["exit_code"], 7);
        }

        #[tokio::test]
        async fn stderr_captured() {
            let out = tool().execute(input("b-3", json!({"command": "echo err >&2"}), "/tmp")).await.unwrap();
            assert!(out.content.contains("STDERR:"));
            assert!(out.content.contains("err"));
        }

        #[tokio::test]
        async fn combined_stdout_stderr() {
            let out = tool().execute(input("b-4", json!({"command": "echo out && echo err >&2"}), "/tmp")).await.unwrap();
            assert!(out.content.contains("out"));
            assert!(out.content.contains("STDERR:"));
            assert!(out.content.contains("err"));
        }

        #[tokio::test]
        async fn timeout_enforcement() {
            let result = tool().execute(input("b-5", json!({"command": "sleep 60", "timeout_ms": 200}), "/tmp")).await;
            assert!(result.is_err());
            match result.unwrap_err() {
                CuervoError::ToolTimeout { tool, .. } => assert_eq!(tool, "bash"),
                other => panic!("expected ToolTimeout, got: {other:?}"),
            }
        }

        #[tokio::test]
        async fn max_timeout_capped_at_600s() {
            // If timeout_ms > 600000 it should be capped.
            let out = tool().execute(input("b-6", json!({"command": "echo cap", "timeout_ms": 999999}), "/tmp")).await.unwrap();
            assert!(!out.is_error);
            assert!(out.content.contains("cap"));
        }

        #[tokio::test]
        async fn empty_output_shows_placeholder() {
            let out = tool().execute(input("b-7", json!({"command": "true"}), "/tmp")).await.unwrap();
            assert!(!out.is_error);
            assert!(out.content.contains("(no output)"));
        }

        #[tokio::test]
        async fn missing_command_is_error() {
            let result = tool().execute(input("b-8", json!({}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn null_command_is_error() {
            let result = tool().execute(input("b-9", json!({"command": null}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn working_directory_respected() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("marker.txt"), "found").unwrap();
            let out = tool().execute(input("b-10", json!({"command": "cat marker.txt"}), dir.path().to_str().unwrap())).await.unwrap();
            assert!(!out.is_error);
            assert!(out.content.contains("found"));
        }

        #[tokio::test]
        async fn deterministic_repeated_execution() {
            let t = tool();
            let o1 = t.execute(input("r1", json!({"command": "echo deterministic"}), "/tmp")).await.unwrap();
            let o2 = t.execute(input("r2", json!({"command": "echo deterministic"}), "/tmp")).await.unwrap();
            assert_eq!(o1.content, o2.content);
        }

        #[test]
        fn permission_is_destructive() {
            assert_eq!(tool().permission_level(), PermissionLevel::Destructive);
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }
    }

    // ============================================================
    //  SECTION 6: GLOB AUDIT
    // ============================================================
    mod glob_audit {
        use super::*;

        fn tool() -> GlobTool {
            GlobTool::new()
        }

        #[tokio::test]
        async fn valid_glob_finds_files() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("a.txt"), "").unwrap();
            std::fs::write(dir.path().join("b.txt"), "").unwrap();
            std::fs::write(dir.path().join("c.rs"), "").unwrap();

            let out = tool().execute(tmp_input("g-1", json!({"pattern": "*.txt", "path": dir.path().to_str().unwrap()}), &dir)).await.unwrap();
            assert_id_propagated(&out, "g-1");
            assert!(!out.is_error);
            assert!(out.content.contains("a.txt"));
            assert!(out.content.contains("b.txt"));
            assert!(!out.content.contains("c.rs"));
            let meta = out.metadata.as_ref().unwrap();
            assert_eq!(meta["count"], 2);
        }

        #[tokio::test]
        async fn no_matches_returns_no_matches() {
            let dir = tempfile::TempDir::new().unwrap();
            let out = tool().execute(tmp_input("g-2", json!({"pattern": "*.zzz", "path": dir.path().to_str().unwrap()}), &dir)).await.unwrap();
            assert!(!out.is_error);
            assert!(out.content.contains("No matches"));
        }

        #[tokio::test]
        async fn missing_pattern_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("g-3", json!({}), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn recursive_glob() {
            let dir = tempfile::TempDir::new().unwrap();
            let sub = dir.path().join("sub");
            std::fs::create_dir(&sub).unwrap();
            std::fs::write(dir.path().join("top.rs"), "").unwrap();
            std::fs::write(sub.join("deep.rs"), "").unwrap();

            let out = tool().execute(tmp_input("g-4", json!({"pattern": "**/*.rs", "path": dir.path().to_str().unwrap()}), &dir)).await.unwrap();
            assert!(!out.is_error);
            let meta = out.metadata.as_ref().unwrap();
            assert!(meta["count"].as_u64().unwrap() >= 2);
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }
    }

    // ============================================================
    //  SECTION 7: GREP AUDIT
    // ============================================================
    mod grep_audit {
        use super::*;

        fn tool() -> GrepTool {
            GrepTool::new()
        }

        #[tokio::test]
        async fn valid_grep_finds_matches() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("src.rs"), "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

            let out = tool().execute(tmp_input("gr-1", json!({
                "pattern": "println",
                "path": dir.path().to_str().unwrap()
            }), &dir)).await.unwrap();
            assert_id_propagated(&out, "gr-1");
            assert!(!out.is_error);
            assert!(out.content.contains("println"));
            let meta = out.metadata.as_ref().unwrap();
            assert!(meta["total_matches"].as_u64().unwrap() >= 1);
        }

        #[tokio::test]
        async fn no_matches_returns_empty() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("empty.txt"), "nothing here").unwrap();

            let out = tool().execute(tmp_input("gr-2", json!({
                "pattern": "ZZZZNOTFOUND",
                "path": dir.path().to_str().unwrap()
            }), &dir)).await.unwrap();
            assert!(!out.is_error);
            let meta = out.metadata.as_ref().unwrap();
            assert_eq!(meta["total_matches"], 0);
        }

        #[tokio::test]
        async fn invalid_regex_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("a.txt"), "data").unwrap();

            let result = tool().execute(tmp_input("gr-3", json!({
                "pattern": "[invalid",
                "path": dir.path().to_str().unwrap()
            }), &dir)).await;
            // Invalid regex should produce an error.
            assert!(result.is_err() || result.unwrap().is_error);
        }

        #[tokio::test]
        async fn missing_pattern_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("gr-4", json!({}), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn regex_pattern_works() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("data.txt"), "foo123bar\nhello456world\n").unwrap();

            let out = tool().execute(tmp_input("gr-5", json!({
                "pattern": "\\d+",
                "path": dir.path().to_str().unwrap()
            }), &dir)).await.unwrap();
            assert!(!out.is_error);
            let meta = out.metadata.as_ref().unwrap();
            assert!(meta["total_matches"].as_u64().unwrap() >= 2);
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }
    }

    // ============================================================
    //  SECTION 8: DIRECTORY_TREE AUDIT
    // ============================================================
    mod directory_tree_audit {
        use super::*;

        fn tool() -> DirectoryTreeTool {
            DirectoryTreeTool::new()
        }

        #[tokio::test]
        async fn valid_tree() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("a.txt"), "").unwrap();
            std::fs::create_dir(dir.path().join("sub")).unwrap();
            std::fs::write(dir.path().join("sub/b.txt"), "").unwrap();

            let out = tool().execute(tmp_input("dt-1", json!({"path": dir.path().to_str().unwrap()}), &dir)).await.unwrap();
            assert_id_propagated(&out, "dt-1");
            assert!(!out.is_error);
            assert!(out.content.contains("a.txt"));
            assert!(out.content.contains("sub"));
        }

        #[tokio::test]
        async fn depth_limit() {
            let dir = tempfile::TempDir::new().unwrap();
            let deep = dir.path().join("a/b/c/d/e");
            std::fs::create_dir_all(&deep).unwrap();
            std::fs::write(deep.join("deep.txt"), "").unwrap();

            let out = tool().execute(tmp_input("dt-2", json!({
                "path": dir.path().to_str().unwrap(), "depth": 2
            }), &dir)).await.unwrap();
            assert!(!out.is_error);
            // Depth 2 should not reach deep.txt at depth 5+.
            assert!(!out.content.contains("deep.txt"));
        }

        #[tokio::test]
        async fn empty_directory() {
            let dir = tempfile::TempDir::new().unwrap();
            let out = tool().execute(tmp_input("dt-3", json!({"path": dir.path().to_str().unwrap()}), &dir)).await.unwrap();
            assert!(!out.is_error);
        }

        #[tokio::test]
        async fn missing_path_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("dt-4", json!({}), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn nonexistent_path_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("dt-5", json!({"path": "/nonexistent/dir"}), &dir)).await;
            assert!(result.is_err() || result.unwrap().is_error);
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }
    }

    // ============================================================
    //  SECTION 9: SYMBOL_SEARCH AUDIT
    // ============================================================
    mod symbol_search_audit {
        use super::*;

        fn tool() -> SymbolSearchTool {
            SymbolSearchTool::new()
        }

        #[tokio::test]
        async fn finds_rust_function() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("lib.rs"), "pub fn hello_world() {\n}\n").unwrap();

            let out = tool().execute(tmp_input("ss-1", json!({
                "query": "hello_world",
                "path": dir.path().to_str().unwrap()
            }), &dir)).await.unwrap();
            assert_id_propagated(&out, "ss-1");
            assert!(!out.is_error);
            assert!(out.content.contains("hello_world"));
        }

        #[tokio::test]
        async fn finds_python_class() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("mod.py"), "class MyClass:\n    pass\n").unwrap();

            let out = tool().execute(tmp_input("ss-2", json!({
                "query": "MyClass",
                "path": dir.path().to_str().unwrap()
            }), &dir)).await.unwrap();
            assert!(!out.is_error);
            assert!(out.content.contains("MyClass"));
        }

        #[tokio::test]
        async fn no_matches() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("empty.rs"), "// nothing\n").unwrap();

            let out = tool().execute(tmp_input("ss-3", json!({
                "query": "ZZZNOTFOUND",
                "path": dir.path().to_str().unwrap()
            }), &dir)).await.unwrap();
            assert!(!out.is_error);
            let meta = out.metadata.as_ref().unwrap();
            assert_eq!(meta["match_count"], 0);
        }

        #[tokio::test]
        async fn missing_query_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("ss-4", json!({}), &dir)).await;
            assert!(result.is_err());
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }
    }

    // ============================================================
    //  SECTION 10: GIT TOOLS AUDIT
    // ============================================================
    mod git_audit {
        use super::*;

        fn init_git_repo() -> tempfile::TempDir {
            let dir = tempfile::TempDir::new().unwrap();
            std::process::Command::new("git")
                .args(["init"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            std::process::Command::new("git")
                .args(["config", "user.email", "test@test.com"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            std::process::Command::new("git")
                .args(["config", "user.name", "Test"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            dir
        }

        #[tokio::test]
        async fn git_status_in_repo() {
            let dir = init_git_repo();
            let tool = GitStatusTool::new();
            let out = tool.execute(tmp_input("gs-1", json!({}), &dir)).await.unwrap();
            assert_id_propagated(&out, "gs-1");
            assert!(!out.is_error);
            // Fresh repo should mention branch.
        }

        #[tokio::test]
        async fn git_status_not_a_repo() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = GitStatusTool::new();
            let result = tool.execute(tmp_input("gs-2", json!({}), &dir)).await;
            // Should either error or return is_error=true.
            assert!(result.is_err() || result.unwrap().is_error);
        }

        #[tokio::test]
        async fn git_log_empty_repo() {
            let dir = init_git_repo();
            let tool = GitLogTool::new();
            let result = tool.execute(tmp_input("gl-1", json!({}), &dir)).await;
            // Empty repo (no commits) should handle gracefully.
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn git_log_with_commits() {
            let dir = init_git_repo();
            std::fs::write(dir.path().join("file.txt"), "data").unwrap();
            std::process::Command::new("git")
                .args(["add", "file.txt"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            std::process::Command::new("git")
                .args(["commit", "-m", "initial commit"])
                .current_dir(dir.path())
                .output()
                .unwrap();

            let tool = GitLogTool::new();
            let out = tool.execute(tmp_input("gl-2", json!({"count": 5}), &dir)).await.unwrap();
            assert!(!out.is_error);
            assert!(out.content.contains("initial commit"));
        }

        #[tokio::test]
        async fn git_diff_no_changes() {
            let dir = init_git_repo();
            std::fs::write(dir.path().join("f.txt"), "data").unwrap();
            std::process::Command::new("git")
                .args(["add", "f.txt"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            std::process::Command::new("git")
                .args(["commit", "-m", "init"])
                .current_dir(dir.path())
                .output()
                .unwrap();

            let tool = GitDiffTool::new();
            let out = tool.execute(tmp_input("gd-1", json!({}), &dir)).await.unwrap();
            assert!(!out.is_error);
            // No changes, diff should be empty or minimal.
        }

        #[tokio::test]
        async fn git_diff_with_changes() {
            let dir = init_git_repo();
            let f = dir.path().join("f.txt");
            std::fs::write(&f, "original").unwrap();
            std::process::Command::new("git")
                .args(["add", "f.txt"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            std::process::Command::new("git")
                .args(["commit", "-m", "init"])
                .current_dir(dir.path())
                .output()
                .unwrap();

            std::fs::write(&f, "modified").unwrap();
            let tool = GitDiffTool::new();
            let out = tool.execute(tmp_input("gd-2", json!({}), &dir)).await.unwrap();
            assert!(!out.is_error);
            assert!(out.content.contains("modified") || out.content.contains("original"));
        }

        #[tokio::test]
        async fn git_add_valid() {
            let dir = init_git_repo();
            std::fs::write(dir.path().join("new.txt"), "content").unwrap();

            let tool = GitAddTool::new();
            let out = tool.execute(tmp_input("ga-1", json!({"paths": ["new.txt"]}), &dir)).await.unwrap();
            assert!(!out.is_error);
        }

        #[tokio::test]
        async fn git_add_rejects_dot() {
            let dir = init_git_repo();
            let tool = GitAddTool::new();
            let result = tool.execute(tmp_input("ga-2", json!({"paths": ["."]}), &dir)).await;
            // Should reject "." pattern.
            assert!(result.is_err() || result.unwrap().is_error);
        }

        #[tokio::test]
        async fn git_add_rejects_all_flag() {
            let dir = init_git_repo();
            let tool = GitAddTool::new();
            let result = tool.execute(tmp_input("ga-3", json!({"paths": ["-A"]}), &dir)).await;
            assert!(result.is_err() || result.unwrap().is_error);
        }

        #[tokio::test]
        async fn git_add_empty_paths_is_error() {
            let dir = init_git_repo();
            let tool = GitAddTool::new();
            let result = tool.execute(tmp_input("ga-4", json!({"paths": []}), &dir)).await;
            assert!(result.is_err() || result.unwrap().is_error);
        }

        #[tokio::test]
        async fn git_commit_requires_staged() {
            let dir = init_git_repo();
            let tool = GitCommitTool::new();
            let result = tool.execute(tmp_input("gc-1", json!({"message": "empty commit"}), &dir)).await;
            // Nothing staged, should fail.
            assert!(result.is_err() || result.unwrap().is_error);
        }

        #[tokio::test]
        async fn git_commit_missing_message_is_error() {
            let dir = init_git_repo();
            let tool = GitCommitTool::new();
            let result = tool.execute(tmp_input("gc-2", json!({}), &dir)).await;
            assert!(result.is_err());
        }

        #[test]
        fn all_git_schemas_valid() {
            assert_valid_schema(&GitStatusTool::new());
            assert_valid_schema(&GitDiffTool::new());
            assert_valid_schema(&GitLogTool::new());
            assert_valid_schema(&GitAddTool::new());
            assert_valid_schema(&GitCommitTool::new());
        }

        #[test]
        fn git_permission_levels() {
            assert_eq!(GitStatusTool::new().permission_level(), PermissionLevel::ReadOnly);
            assert_eq!(GitDiffTool::new().permission_level(), PermissionLevel::ReadOnly);
            assert_eq!(GitLogTool::new().permission_level(), PermissionLevel::ReadOnly);
            assert_eq!(GitAddTool::new().permission_level(), PermissionLevel::ReadWrite);
            assert_eq!(GitCommitTool::new().permission_level(), PermissionLevel::Destructive);
        }
    }

    // ============================================================
    //  SECTION 11: TASK_TRACK AUDIT
    // ============================================================
    mod task_track_audit {
        use super::*;

        fn tool() -> TaskTrackTool {
            TaskTrackTool::new()
        }

        #[tokio::test]
        async fn add_then_list() {
            let t = tool();
            t.execute(input("tt-1", json!({"action": "add", "content": "Task A"}), "/tmp")).await.unwrap();
            let out = t.execute(input("tt-2", json!({"action": "list"}), "/tmp")).await.unwrap();
            assert!(!out.is_error);
            assert!(out.content.contains("Task A"));
        }

        #[tokio::test]
        async fn invalid_status_is_error() {
            let t = tool();
            t.execute(input("tt-3", json!({"action": "add", "content": "T"}), "/tmp")).await.unwrap();
            let out = t.execute(input("tt-4", json!({
                "action": "update", "task_index": 0, "status": "invalid_status"
            }), "/tmp")).await;
            assert!(out.is_err());
        }

        #[tokio::test]
        async fn missing_action_is_error() {
            let t = tool();
            let result = t.execute(input("tt-5", json!({}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn add_missing_content_is_error() {
            let t = tool();
            let result = t.execute(input("tt-6", json!({"action": "add"}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn update_missing_index_is_error() {
            let t = tool();
            t.execute(input("tt-7", json!({"action": "add", "content": "T"}), "/tmp")).await.unwrap();
            let result = t.execute(input("tt-8", json!({"action": "update", "status": "completed"}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn update_missing_status_is_error() {
            let t = tool();
            t.execute(input("tt-9", json!({"action": "add", "content": "T"}), "/tmp")).await.unwrap();
            let result = t.execute(input("tt-10", json!({"action": "update", "task_index": 0}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn task_track_id_propagation() {
            let t = tool();
            let out = t.execute(input("unique-id-abc", json!({"action": "list"}), "/tmp")).await.unwrap();
            assert_id_propagated(&out, "unique-id-abc");
        }

        #[tokio::test]
        async fn metadata_counts_correct() {
            let t = tool();
            t.execute(input("m1", json!({"action": "add", "content": "A"}), "/tmp")).await.unwrap();
            t.execute(input("m2", json!({"action": "add", "content": "B"}), "/tmp")).await.unwrap();
            t.execute(input("m3", json!({"action": "add", "content": "C"}), "/tmp")).await.unwrap();
            t.execute(input("m4", json!({"action": "update", "task_index": 0, "status": "in_progress"}), "/tmp")).await.unwrap();
            t.execute(input("m5", json!({"action": "update", "task_index": 0, "status": "completed"}), "/tmp")).await.unwrap();

            let out = t.execute(input("m6", json!({"action": "list"}), "/tmp")).await.unwrap();
            let meta = out.metadata.unwrap();
            assert_eq!(meta["task_count"], 3);
            assert_eq!(meta["completed"], 1);
            assert_eq!(meta["pending"], 2);
            assert_eq!(meta["in_progress"], 0);
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }
    }

    // ============================================================
    //  SECTION 12: FUZZY_FIND AUDIT
    // ============================================================
    mod fuzzy_find_audit {
        use super::*;

        fn tool() -> FuzzyFindTool {
            FuzzyFindTool::new()
        }

        #[tokio::test]
        async fn finds_matching_files() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("hello_world.rs"), "").unwrap();
            std::fs::write(dir.path().join("other.txt"), "").unwrap();

            let out = tool().execute(tmp_input("ff-1", json!({
                "query": "hello",
                "path": dir.path().to_str().unwrap()
            }), &dir)).await.unwrap();
            assert_id_propagated(&out, "ff-1");
            assert!(!out.is_error);
            assert!(out.content.contains("hello_world.rs"));
        }

        #[tokio::test]
        async fn no_matches_returns_empty() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("file.txt"), "").unwrap();

            let out = tool().execute(tmp_input("ff-2", json!({
                "query": "ZZZNOTFOUND",
                "path": dir.path().to_str().unwrap()
            }), &dir)).await.unwrap();
            assert!(!out.is_error);
            let meta = out.metadata.as_ref().unwrap();
            assert_eq!(meta["match_count"], 0);
        }

        #[tokio::test]
        async fn missing_query_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("ff-3", json!({}), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn max_results_capped() {
            let dir = tempfile::TempDir::new().unwrap();
            for i in 0..30 {
                std::fs::write(dir.path().join(format!("file_{i}.txt")), "").unwrap();
            }

            let out = tool().execute(tmp_input("ff-4", json!({
                "query": "file",
                "path": dir.path().to_str().unwrap(),
                "max_results": 5
            }), &dir)).await.unwrap();
            assert!(!out.is_error);
            let meta = out.metadata.as_ref().unwrap();
            // match_count should be at most max_results.
            assert!(meta["match_count"].as_u64().unwrap() <= 5);
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }
    }

    // ============================================================
    //  SECTION 13: WEB_FETCH AUDIT
    // ============================================================
    mod web_fetch_audit {
        use super::*;

        fn tool() -> WebFetchTool {
            WebFetchTool::new()
        }

        #[tokio::test]
        async fn missing_url_is_error() {
            let result = tool().execute(input("wf-1", json!({}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn null_url_is_error() {
            let result = tool().execute(input("wf-2", json!({"url": null}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn non_string_url_is_error() {
            let result = tool().execute(input("wf-3", json!({"url": 42}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn invalid_scheme_is_error() {
            let result = tool().execute(input("wf-4", json!({"url": "ftp://example.com"}), "/tmp")).await;
            assert!(result.is_err() || result.unwrap().is_error);
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }

        #[test]
        fn permission_is_readonly() {
            assert_eq!(tool().permission_level(), PermissionLevel::ReadOnly);
        }
    }

    // ============================================================
    //  SECTION 14: WEB_SEARCH AUDIT
    // ============================================================
    mod web_search_audit {
        use super::*;

        fn tool() -> WebSearchTool {
            WebSearchTool::new()
        }

        #[tokio::test]
        async fn missing_query_is_error() {
            let result = tool().execute(input("ws-1", json!({}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn null_query_is_error() {
            let result = tool().execute(input("ws-2", json!({"query": null}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn no_api_key_handles_gracefully() {
            // Without BRAVE_API_KEY, should fail gracefully.
            let result = tool().execute(input("ws-3", json!({"query": "test search"}), "/tmp")).await;
            // Expect either error or is_error output.
            match result {
                Ok(out) => assert!(out.is_error || !out.content.is_empty()),
                Err(_) => {} // also acceptable
            }
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }
    }

    // ============================================================
    //  SECTION 15: HTTP_REQUEST AUDIT
    // ============================================================
    mod http_request_audit {
        use super::*;

        fn tool() -> HttpRequestTool {
            HttpRequestTool::new()
        }

        #[tokio::test]
        async fn missing_url_is_error() {
            let result = tool().execute(input("hr-1", json!({}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn null_url_is_error() {
            let result = tool().execute(input("hr-2", json!({"url": null}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[test]
        fn permission_is_destructive() {
            assert_eq!(tool().permission_level(), PermissionLevel::Destructive);
        }

        #[test]
        fn requires_confirmation() {
            let dummy = ToolInput {
                tool_use_id: "x".into(),
                arguments: json!({}),
                working_directory: "/tmp".into(),
            };
            assert!(tool().requires_confirmation(&dummy));
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }
    }

    // ============================================================
    //  SECTION 16: FILE_INSPECT AUDIT
    // ============================================================
    mod file_inspect_audit {
        use super::*;

        fn tool() -> FileInspectTool {
            FileInspectTool::new(vec![], vec![])
        }

        #[tokio::test]
        async fn inspect_text_file() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("readme.txt");
            std::fs::write(&f, "Hello world\nLine 2\n").unwrap();

            let out = tool().execute(tmp_input("fi-1", json!({"path": f.to_str().unwrap()}), &dir)).await.unwrap();
            assert_id_propagated(&out, "fi-1");
            assert!(!out.is_error);
            let meta = out.metadata.as_ref().unwrap();
            assert!(meta["size_bytes"].as_u64().unwrap() > 0);
        }

        #[tokio::test]
        async fn inspect_nonexistent_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("fi-2", json!({"path": "/nonexistent/file.xyz"}), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn missing_path_is_error() {
            let dir = tempfile::TempDir::new().unwrap();
            let result = tool().execute(tmp_input("fi-3", json!({}), &dir)).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn metadata_only_mode() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("meta.json");
            std::fs::write(&f, r#"{"key": "value"}"#).unwrap();

            let out = tool().execute(tmp_input("fi-4", json!({
                "path": f.to_str().unwrap(),
                "metadata_only": true
            }), &dir)).await.unwrap();
            assert!(!out.is_error);
        }

        #[test]
        fn schema_contract() {
            assert_valid_schema(&tool());
        }
    }

    // ============================================================
    //  SECTION 17: BACKGROUND TOOLS AUDIT
    // ============================================================
    mod background_audit {
        use super::*;

        fn registry() -> Arc<ProcessRegistry> {
            Arc::new(ProcessRegistry::new(5))
        }

        #[tokio::test]
        async fn start_and_output() {
            let reg = registry();
            let start_tool = BackgroundStartTool::new(reg.clone());
            let output_tool = BackgroundOutputTool::new(reg.clone());

            let out = start_tool.execute(input("bs-1", json!({"command": "echo hello_bg"}), "/tmp")).await.unwrap();
            assert_id_propagated(&out, "bs-1");
            assert!(!out.is_error);
            let meta = out.metadata.as_ref().unwrap();
            let job_id = meta["job_id"].as_str().unwrap().to_string();

            // Wait for process to complete.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            // Collect output from child.
            if let Some(child) = reg.take_child(&job_id) {
                #[allow(unused_mut)]
                let mut child = child;
                let output = child.wait_with_output().await.unwrap();
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                reg.update_output(&job_id, &stdout, "", true, output.status.code());
            }

            let out2 = output_tool.execute(input("bo-1", json!({"job_id": &job_id}), "/tmp")).await.unwrap();
            assert!(!out2.is_error);
        }

        #[tokio::test]
        async fn output_unknown_job_is_error() {
            let reg = registry();
            let tool = BackgroundOutputTool::new(reg);
            let out = tool.execute(input("bo-2", json!({"job_id": "bg-nonexistent"}), "/tmp")).await.unwrap();
            assert!(out.is_error);
        }

        #[tokio::test]
        async fn kill_unknown_job_is_error() {
            let reg = registry();
            let tool = BackgroundKillTool::new(reg);
            let out = tool.execute(input("bk-1", json!({"job_id": "bg-nonexistent"}), "/tmp")).await.unwrap();
            assert!(out.is_error);
        }

        #[tokio::test]
        async fn start_missing_command_is_error() {
            let reg = registry();
            let tool = BackgroundStartTool::new(reg);
            let result = tool.execute(input("bs-2", json!({}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn output_missing_job_id_is_error() {
            let reg = registry();
            let tool = BackgroundOutputTool::new(reg);
            let result = tool.execute(input("bo-3", json!({}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn kill_missing_job_id_is_error() {
            let reg = registry();
            let tool = BackgroundKillTool::new(reg);
            let result = tool.execute(input("bk-2", json!({}), "/tmp")).await;
            assert!(result.is_err());
        }

        #[test]
        fn permission_levels() {
            let reg = registry();
            assert_eq!(BackgroundStartTool::new(reg.clone()).permission_level(), PermissionLevel::Destructive);
            assert_eq!(BackgroundOutputTool::new(reg.clone()).permission_level(), PermissionLevel::ReadOnly);
            assert_eq!(BackgroundKillTool::new(reg).permission_level(), PermissionLevel::Destructive);
        }

        #[test]
        fn schemas_valid() {
            let reg = registry();
            assert_valid_schema(&BackgroundStartTool::new(reg.clone()));
            assert_valid_schema(&BackgroundOutputTool::new(reg.clone()));
            assert_valid_schema(&BackgroundKillTool::new(reg));
        }
    }

    // ============================================================
    //  SECTION 18: CROSS-CUTTING AUDIT
    // ============================================================
    mod cross_cutting_audit {
        use super::*;

        /// Verify every tool name is non-empty and unique.
        #[test]
        fn all_tool_names_unique_and_nonempty() {
            let config = ToolsConfig::default();
            let proc_reg = Arc::new(ProcessRegistry::new(5));
            let reg = crate::full_registry(&config, Some(proc_reg));
            let defs = reg.tool_definitions();

            assert_eq!(defs.len(), 23, "expected 23 tools in full registry");

            let mut names: Vec<String> = defs.iter().map(|d| d.name.clone()).collect();
            for name in &names {
                assert!(!name.is_empty(), "tool name must not be empty");
            }

            let original_len = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), original_len, "tool names must be unique");
        }

        /// Verify every tool has valid JSON schema.
        #[test]
        fn all_schemas_valid() {
            let config = ToolsConfig::default();
            let proc_reg = Arc::new(ProcessRegistry::new(5));
            let reg = crate::full_registry(&config, Some(proc_reg));

            let expected_tools = [
                "file_read", "file_write", "file_edit", "file_delete", "file_inspect",
                "directory_tree", "bash", "glob", "grep", "symbol_search",
                "web_fetch", "web_search", "http_request", "task_track", "fuzzy_find",
                "git_status", "git_diff", "git_log", "git_add", "git_commit",
                "background_start", "background_output", "background_kill",
            ];

            for name in &expected_tools {
                let tool = reg.get(name).unwrap_or_else(|| panic!("tool '{name}' not in registry"));
                let schema = tool.input_schema();
                assert_eq!(schema["type"], "object", "{name}: type must be 'object'");
                assert!(schema["properties"].is_object(), "{name}: must have properties");
                assert!(schema["required"].is_array(), "{name}: must have required");
            }
        }

        /// Verify permission levels are correctly assigned.
        #[test]
        fn permission_levels_correct() {
            let config = ToolsConfig::default();
            let proc_reg = Arc::new(ProcessRegistry::new(5));
            let reg = crate::full_registry(&config, Some(proc_reg));

            let readonly_tools = [
                "file_read", "glob", "grep", "web_fetch", "directory_tree",
                "file_inspect", "git_status", "git_diff", "git_log",
                "symbol_search", "task_track", "fuzzy_find", "background_output",
                "web_search",
            ];

            let readwrite_tools = ["git_add"];

            let destructive_tools = [
                "bash", "file_write", "file_edit", "file_delete", "http_request", "git_commit",
                "background_start", "background_kill",
            ];

            for name in &readonly_tools {
                let tool = reg.get(name).unwrap();
                assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly, "{name} should be ReadOnly");
            }

            for name in &readwrite_tools {
                let tool = reg.get(name).unwrap();
                assert_eq!(tool.permission_level(), PermissionLevel::ReadWrite, "{name} should be ReadWrite");
            }

            for name in &destructive_tools {
                let tool = reg.get(name).unwrap();
                assert_eq!(tool.permission_level(), PermissionLevel::Destructive, "{name} should be Destructive");
            }
        }

        /// Verify confirmation requirements.
        #[test]
        fn confirmation_requirements() {
            let config = ToolsConfig::default();
            let proc_reg = Arc::new(ProcessRegistry::new(5));
            let reg = crate::full_registry(&config, Some(proc_reg));

            let dummy = ToolInput {
                tool_use_id: "x".into(),
                arguments: json!({}),
                working_directory: "/tmp".into(),
            };

            // Destructive tools should require confirmation by default.
            let confirm_tools = ["bash", "file_delete", "http_request", "git_commit", "background_start"];
            for name in &confirm_tools {
                let tool = reg.get(name).unwrap();
                assert!(tool.requires_confirmation(&dummy), "{name} should require confirmation");
            }

            // ReadOnly tools should NOT require confirmation.
            let no_confirm = ["file_read", "glob", "grep", "task_track", "fuzzy_find"];
            for name in &no_confirm {
                let tool = reg.get(name).unwrap();
                assert!(!tool.requires_confirmation(&dummy), "{name} should NOT require confirmation");
            }
        }

        /// Verify default registry has 20 tools and full has 23.
        #[test]
        fn registry_counts() {
            let config = ToolsConfig::default();
            let default_reg = crate::default_registry(&config);
            assert_eq!(default_reg.tool_definitions().len(), 20);

            let proc_reg = Arc::new(ProcessRegistry::new(5));
            let full_reg = crate::full_registry(&config, Some(proc_reg));
            assert_eq!(full_reg.tool_definitions().len(), 23);
        }

        /// Verify tool_use_id propagation for ALL tools that can be called without network.
        #[tokio::test]
        async fn tool_use_id_propagation_all_local_tools() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("test.txt");
            std::fs::write(&f, "content").unwrap();

            let unique_id = "propagation-test-xyz-123";

            // file_read
            let tool = FileReadTool::new(vec![], vec![]);
            let out = tool.execute(tmp_input(unique_id, json!({"path": f.to_str().unwrap()}), &dir)).await.unwrap();
            assert_eq!(out.tool_use_id, unique_id);

            // glob
            let tool = GlobTool::new();
            let out = tool.execute(tmp_input(unique_id, json!({"pattern": "*.txt", "path": dir.path().to_str().unwrap()}), &dir)).await.unwrap();
            assert_eq!(out.tool_use_id, unique_id);

            // grep
            let tool = GrepTool::new();
            let out = tool.execute(tmp_input(unique_id, json!({"pattern": "content", "path": dir.path().to_str().unwrap()}), &dir)).await.unwrap();
            assert_eq!(out.tool_use_id, unique_id);

            // directory_tree
            let tool = DirectoryTreeTool::new();
            let out = tool.execute(tmp_input(unique_id, json!({"path": dir.path().to_str().unwrap()}), &dir)).await.unwrap();
            assert_eq!(out.tool_use_id, unique_id);

            // fuzzy_find
            let tool = FuzzyFindTool::new();
            let out = tool.execute(tmp_input(unique_id, json!({"query": "test", "path": dir.path().to_str().unwrap()}), &dir)).await.unwrap();
            assert_eq!(out.tool_use_id, unique_id);

            // task_track
            let tool = TaskTrackTool::new();
            let out = tool.execute(input(unique_id, json!({"action": "list"}), "/tmp")).await.unwrap();
            assert_eq!(out.tool_use_id, unique_id);

            // bash
            let tool = BashTool::new(120, cuervo_core::types::SandboxConfig::default());
            let out = tool.execute(input(unique_id, json!({"command": "echo test"}), "/tmp")).await.unwrap();
            assert_eq!(out.tool_use_id, unique_id);

            // symbol_search
            let tool = SymbolSearchTool::new();
            let out = tool.execute(tmp_input(unique_id, json!({"query": "test", "path": dir.path().to_str().unwrap()}), &dir)).await.unwrap();
            assert_eq!(out.tool_use_id, unique_id);
        }
    }

    // ============================================================
    //  SECTION 19: CONCURRENCY AUDIT (Phase 4)
    //  Validates thread-safety, no race conditions, no cross-tool leakage,
    //  no deadlocks, correct results returned to correct caller.
    // ============================================================
    mod concurrency_audit {
        use super::*;

        /// Simple join_all implementation using tokio (no futures crate needed).
        async fn join_all<T: Send + 'static>(handles: Vec<tokio::task::JoinHandle<T>>) -> Vec<Result<T, tokio::task::JoinError>> {
            let mut results = Vec::with_capacity(handles.len());
            for h in handles {
                results.push(h.await);
            }
            results
        }

        /// Run 10 file_read calls in parallel — each reads a different file.
        /// Validates no cross-tool leakage (each result matches its input).
        #[tokio::test]
        async fn ten_parallel_file_reads_no_leakage() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = Arc::new(FileReadTool::new(vec![], vec![]));

            // Create 10 unique files.
            for i in 0..10 {
                std::fs::write(dir.path().join(format!("file_{i}.txt")), format!("content_{i}")).unwrap();
            }

            let mut handles = vec![];
            for i in 0..10 {
                let t = tool.clone();
                let path = dir.path().join(format!("file_{i}.txt"));
                let wd = dir.path().to_str().unwrap().to_string();
                handles.push(tokio::spawn(async move {
                    let inp = ToolInput {
                        tool_use_id: format!("par-{i}"),
                        arguments: json!({"path": path.to_str().unwrap()}),
                        working_directory: wd,
                    };
                    let out = t.execute(inp).await.unwrap();
                    (i, out)
                }));
            }

            let results = join_all(handles).await;

            assert_eq!(results.len(), 10);
            for r in &results {
                let (i, out) = r.as_ref().unwrap();
                assert_eq!(out.tool_use_id, format!("par-{i}"));
                assert!(!out.is_error, "file_{i} should succeed");
                assert!(out.content.contains(&format!("content_{i}")), "file_{i} content mismatch");
            }
        }

        /// Run 50 bash echo calls in parallel — validates no shared state corruption.
        #[tokio::test]
        async fn fifty_parallel_bash_echo_no_corruption() {
            let tool = Arc::new(BashTool::new(120, cuervo_core::types::SandboxConfig::default()));

            let mut handles = vec![];
            for i in 0..50 {
                let t = tool.clone();
                handles.push(tokio::spawn(async move {
                    let inp = ToolInput {
                        tool_use_id: format!("bash-{i}"),
                        arguments: json!({"command": format!("echo unique_marker_{i}")}),
                        working_directory: "/tmp".to_string(),
                    };
                    let out = t.execute(inp).await.unwrap();
                    (i, out)
                }));
            }

            let results = join_all(handles).await;

            assert_eq!(results.len(), 50);
            for r in &results {
                let (i, out) = r.as_ref().unwrap();
                assert_eq!(out.tool_use_id, format!("bash-{i}"));
                assert!(!out.is_error, "bash-{i} should succeed");
                assert!(
                    out.content.contains(&format!("unique_marker_{i}")),
                    "bash-{i}: expected unique_marker_{i} in output, got: {}",
                    &out.content[..out.content.len().min(100)]
                );
            }
        }

        /// Mixed tool types in parallel: file_read + grep + glob + directory_tree.
        #[tokio::test]
        async fn mixed_tool_types_parallel() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("mixed.txt"), "searchable content").unwrap();
            std::fs::create_dir(dir.path().join("sub")).unwrap();

            let wd = dir.path().to_str().unwrap().to_string();
            let path = dir.path().join("mixed.txt");

            let file_read = Arc::new(FileReadTool::new(vec![], vec![]));
            let grep = Arc::new(GrepTool::new());
            let glob = Arc::new(GlobTool::new());
            let tree = Arc::new(DirectoryTreeTool::new());

            let fr = {
                let t = file_read.clone();
                let p = path.to_str().unwrap().to_string();
                let w = wd.clone();
                tokio::spawn(async move {
                    t.execute(ToolInput {
                        tool_use_id: "mix-fr".into(),
                        arguments: json!({"path": p}),
                        working_directory: w,
                    }).await
                })
            };

            let gr = {
                let t = grep.clone();
                let w = wd.clone();
                let d = dir.path().to_str().unwrap().to_string();
                tokio::spawn(async move {
                    t.execute(ToolInput {
                        tool_use_id: "mix-gr".into(),
                        arguments: json!({"pattern": "searchable", "path": d}),
                        working_directory: w,
                    }).await
                })
            };

            let gl = {
                let t = glob.clone();
                let w = wd.clone();
                let d = dir.path().to_str().unwrap().to_string();
                tokio::spawn(async move {
                    t.execute(ToolInput {
                        tool_use_id: "mix-gl".into(),
                        arguments: json!({"pattern": "*.txt", "path": d}),
                        working_directory: w,
                    }).await
                })
            };

            let dt = {
                let t = tree.clone();
                let w = wd.clone();
                let d = dir.path().to_str().unwrap().to_string();
                tokio::spawn(async move {
                    t.execute(ToolInput {
                        tool_use_id: "mix-dt".into(),
                        arguments: json!({"path": d}),
                        working_directory: w,
                    }).await
                })
            };

            let (fr_r, gr_r, gl_r, dt_r) = tokio::join!(fr, gr, gl, dt);
            let fr_out = fr_r.unwrap().unwrap();
            let gr_out = gr_r.unwrap().unwrap();
            let gl_out = gl_r.unwrap().unwrap();
            let dt_out = dt_r.unwrap().unwrap();

            assert_eq!(fr_out.tool_use_id, "mix-fr");
            assert_eq!(gr_out.tool_use_id, "mix-gr");
            assert_eq!(gl_out.tool_use_id, "mix-gl");
            assert_eq!(dt_out.tool_use_id, "mix-dt");

            assert!(!fr_out.is_error);
            assert!(!gr_out.is_error);
            assert!(!gl_out.is_error);
            assert!(!dt_out.is_error);
        }

        /// Concurrent task_track operations — validates Mutex correctness.
        #[tokio::test]
        async fn concurrent_task_track_operations() {
            let tool = Arc::new(TaskTrackTool::new());

            // Add 20 tasks concurrently.
            let mut handles = vec![];
            for i in 0..20 {
                let t = tool.clone();
                handles.push(tokio::spawn(async move {
                    t.execute(ToolInput {
                        tool_use_id: format!("tt-add-{i}"),
                        arguments: json!({"action": "add", "content": format!("Task {i}")}),
                        working_directory: "/tmp".to_string(),
                    }).await.unwrap()
                }));
            }

            let results = join_all(handles).await;

            // All should succeed.
            for r in &results {
                let out = r.as_ref().unwrap();
                assert!(!out.is_error, "task_track add failed: {}", out.content);
            }

            // List should show all 20 tasks.
            let list_out = tool.execute(ToolInput {
                tool_use_id: "tt-list".into(),
                arguments: json!({"action": "list"}),
                working_directory: "/tmp".into(),
            }).await.unwrap();

            let meta = list_out.metadata.unwrap();
            assert_eq!(meta["task_count"], 20);
        }

        /// Concurrent file writes to DIFFERENT files — no corruption.
        #[tokio::test]
        async fn concurrent_writes_to_different_files() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = Arc::new(FileWriteTool::new(vec![], vec![]));

            let mut handles = vec![];
            for i in 0..10 {
                let t = tool.clone();
                let p = dir.path().join(format!("conc_{i}.txt"));
                let wd = dir.path().to_str().unwrap().to_string();
                handles.push(tokio::spawn(async move {
                    t.execute(ToolInput {
                        tool_use_id: format!("cw-{i}"),
                        arguments: json!({"path": p.to_str().unwrap(), "content": format!("data_{i}")}),
                        working_directory: wd,
                    }).await.unwrap()
                }));
            }

            let results = join_all(handles).await;

            for r in &results {
                let out = r.as_ref().unwrap();
                assert!(!out.is_error, "write failed: {}", out.content);
            }

            // Verify each file has correct content.
            for i in 0..10 {
                let content = std::fs::read_to_string(dir.path().join(format!("conc_{i}.txt"))).unwrap();
                assert_eq!(content, format!("data_{i}"), "file conc_{i}.txt corrupted");
            }
        }

        /// ProcessRegistry concurrent access — no deadlocks.
        #[test]
        fn process_registry_concurrent_access() {
            use std::sync::Arc;

            let reg = Arc::new(crate::background::ProcessRegistry::new(100));

            // Pre-register all 20 processes (serial) to avoid race on register+update.
            let mut ids = Vec::new();
            for i in 0..20 {
                let id = reg.next_id();
                let process = crate::background::BackgroundProcess {
                    job_id: id.clone(),
                    command: format!("echo {i}"),
                    child: None,
                    started_at: std::time::Instant::now(),
                    stdout_buf: String::new(),
                    stderr_buf: String::new(),
                    exit_code: None,
                    finished: false,
                };
                reg.register(process).unwrap();
                ids.push((id, i));
            }

            // Now concurrently update and read.
            let mut handles = vec![];
            for (id, i) in ids {
                let r = reg.clone();
                handles.push(std::thread::spawn(move || {
                    r.update_output(&id, &format!("output_{i}\n"), "", true, Some(0));
                    let (stdout, _, finished, exit_code, _) = r.get_output(&id).unwrap();
                    assert!(stdout.contains(&format!("output_{i}")));
                    assert!(finished);
                    assert_eq!(exit_code, Some(0));
                }));
            }

            for h in handles {
                h.join().unwrap();
            }

            // All 20 should be registered and finished.
            let list = reg.list();
            assert_eq!(list.len(), 20);
            assert!(list.iter().all(|(_, finished, _)| *finished));
        }

        /// No duplicated tool_use_ids in parallel results.
        #[tokio::test]
        async fn no_duplicated_tool_use_ids() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("f.txt"), "x").unwrap();

            let tool = Arc::new(FileReadTool::new(vec![], vec![]));
            let path = dir.path().join("f.txt");

            let mut handles = vec![];
            for i in 0..20 {
                let t = tool.clone();
                let p = path.to_str().unwrap().to_string();
                let wd = dir.path().to_str().unwrap().to_string();
                handles.push(tokio::spawn(async move {
                    t.execute(ToolInput {
                        tool_use_id: format!("dup-check-{i}"),
                        arguments: json!({"path": p}),
                        working_directory: wd,
                    }).await.unwrap()
                }));
            }

            let results = join_all(handles).await;

            let ids: Vec<String> = results.iter().map(|r| r.as_ref().unwrap().tool_use_id.clone()).collect();
            let mut unique_ids = ids.clone();
            unique_ids.sort();
            unique_ids.dedup();
            assert_eq!(ids.len(), unique_ids.len(), "duplicated tool_use_ids found");
        }
    }

    // ============================================================
    //  SECTION 20: STRESS AUDIT (Phase 5)
    //  High-volume tool calls: 200+ rapid-fire, random mix,
    //  forced errors, memory stability.
    // ============================================================
    mod stress_audit {
        use super::*;

        async fn join_all<T: Send + 'static>(handles: Vec<tokio::task::JoinHandle<T>>) -> Vec<Result<T, tokio::task::JoinError>> {
            let mut results = Vec::with_capacity(handles.len());
            for h in handles {
                results.push(h.await);
            }
            results
        }

        /// 200 rapid-fire file_read calls — validates stability.
        #[tokio::test]
        async fn two_hundred_rapid_file_reads() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("stress.txt");
            std::fs::write(&f, "stress test data\n".repeat(10)).unwrap();

            let tool = Arc::new(FileReadTool::new(vec![], vec![]));
            let mut handles = vec![];

            for i in 0..200 {
                let t = tool.clone();
                let p = f.to_str().unwrap().to_string();
                let wd = dir.path().to_str().unwrap().to_string();
                handles.push(tokio::spawn(async move {
                    t.execute(ToolInput {
                        tool_use_id: format!("stress-{i}"),
                        arguments: json!({"path": p}),
                        working_directory: wd,
                    }).await
                }));
            }

            let results = join_all(handles).await;

            let mut success_count = 0;
            let mut error_count = 0;
            for r in &results {
                match r.as_ref().unwrap() {
                    Ok(out) => {
                        assert!(out.content.contains("stress test data"));
                        success_count += 1;
                    }
                    Err(_) => error_count += 1,
                }
            }

            assert_eq!(success_count, 200, "all 200 reads should succeed");
            assert_eq!(error_count, 0);
        }

        /// 100 mixed tool calls: grep + glob + directory_tree.
        #[tokio::test]
        async fn hundred_mixed_readonly_tools() {
            let dir = tempfile::TempDir::new().unwrap();
            for i in 0..5 {
                std::fs::write(dir.path().join(format!("f{i}.txt")), format!("line {i}")).unwrap();
            }

            let grep = Arc::new(GrepTool::new());
            let glob = Arc::new(GlobTool::new());
            let tree = Arc::new(DirectoryTreeTool::new());

            let mut handles = vec![];
            for i in 0..100 {
                let wd = dir.path().to_str().unwrap().to_string();
                let d = dir.path().to_str().unwrap().to_string();
                match i % 3 {
                    0 => {
                        let t = grep.clone();
                        handles.push(tokio::spawn(async move {
                            t.execute(ToolInput {
                                tool_use_id: format!("mix-{i}"),
                                arguments: json!({"pattern": "line", "path": d}),
                                working_directory: wd,
                            }).await
                        }));
                    }
                    1 => {
                        let t = glob.clone();
                        handles.push(tokio::spawn(async move {
                            t.execute(ToolInput {
                                tool_use_id: format!("mix-{i}"),
                                arguments: json!({"pattern": "*.txt", "path": d}),
                                working_directory: wd,
                            }).await
                        }));
                    }
                    _ => {
                        let t = tree.clone();
                        handles.push(tokio::spawn(async move {
                            t.execute(ToolInput {
                                tool_use_id: format!("mix-{i}"),
                                arguments: json!({"path": d}),
                                working_directory: wd,
                            }).await
                        }));
                    }
                }
            }

            let results = join_all(handles).await;
            assert_eq!(results.len(), 100);

            let all_ok = results.iter().all(|r: &Result<cuervo_core::error::Result<ToolOutput>, tokio::task::JoinError>| {
                r.as_ref().unwrap().as_ref().map_or(false, |o| !o.is_error)
            });
            assert!(all_ok, "all 100 mixed tool calls should succeed");
        }

        /// Forced errors interspersed with valid calls — validates error isolation.
        #[tokio::test]
        async fn forced_errors_dont_corrupt_valid_calls() {
            let dir = tempfile::TempDir::new().unwrap();
            let valid_file = dir.path().join("valid.txt");
            std::fs::write(&valid_file, "valid_content").unwrap();

            let tool = Arc::new(FileReadTool::new(vec![], vec![]));
            let mut handles = vec![];

            for i in 0..50 {
                let t = tool.clone();
                let wd = dir.path().to_str().unwrap().to_string();
                let p = if i % 2 == 0 {
                    valid_file.to_str().unwrap().to_string()
                } else {
                    "/nonexistent/error_path.txt".to_string()
                };

                handles.push(tokio::spawn(async move {
                    let result = t.execute(ToolInput {
                        tool_use_id: format!("err-{i}"),
                        arguments: json!({"path": p}),
                        working_directory: wd,
                    }).await;
                    (i, result)
                }));
            }

            let results = join_all(handles).await;

            for r in &results {
                let (i, result) = r.as_ref().unwrap();
                if i % 2 == 0 {
                    // Valid calls should succeed with correct content.
                    let out = result.as_ref().unwrap();
                    assert!(!out.is_error, "valid call {i} should succeed");
                    assert!(out.content.contains("valid_content"), "valid call {i} has wrong content");
                } else {
                    // Error calls should fail without corrupting anything.
                    assert!(result.is_err(), "error call {i} should fail");
                }
            }
        }

        /// task_track under stress — 100 adds, then verify count.
        #[tokio::test]
        async fn task_track_stress_hundred_adds() {
            let tool = Arc::new(TaskTrackTool::new());
            let mut handles = vec![];

            for i in 0..100 {
                let t = tool.clone();
                handles.push(tokio::spawn(async move {
                    t.execute(ToolInput {
                        tool_use_id: format!("stress-tt-{i}"),
                        arguments: json!({"action": "add", "content": format!("Stress task {i}")}),
                        working_directory: "/tmp".to_string(),
                    }).await.unwrap()
                }));
            }

            join_all(handles).await;

            let out = tool.execute(ToolInput {
                tool_use_id: "final-list".into(),
                arguments: json!({"action": "list"}),
                working_directory: "/tmp".into(),
            }).await.unwrap();

            let meta = out.metadata.unwrap();
            assert_eq!(meta["task_count"], 100, "all 100 tasks should be tracked");
        }

        /// Repeated calls to same tool produce deterministic results.
        #[tokio::test]
        async fn deterministic_under_repetition() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("det.txt");
            std::fs::write(&f, "deterministic_content").unwrap();

            let tool = FileReadTool::new(vec![], vec![]);
            let mut outputs = vec![];

            for i in 0..50 {
                let out = tool.execute(ToolInput {
                    tool_use_id: format!("det-{i}"),
                    arguments: json!({"path": f.to_str().unwrap()}),
                    working_directory: dir.path().to_str().unwrap().to_string(),
                }).await.unwrap();
                outputs.push(out.content.clone());
            }

            // All outputs should be identical.
            let first = &outputs[0];
            for (i, content) in outputs.iter().enumerate() {
                assert_eq!(content, first, "output {i} differs from output 0");
            }
        }
    }

    // ============================================================
    //  SECTION 19: STRESS TESTS — FILE_READ
    // ============================================================
    mod file_read_stress {
        use super::*;

        fn tool() -> FileReadTool {
            FileReadTool::new(vec![], vec![])
        }

        #[tokio::test]
        async fn stress_file_read_10mb() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("big.txt");
            let content = "x".repeat(10 * 1024 * 1024); // 10MB
            std::fs::write(&f, &content).unwrap();

            let out = tool().execute(tmp_input("fr-s1", json!({"path": f.to_str().unwrap()}), &dir)).await.unwrap();
            assert!(!out.is_error, "10MB file_read should not error: {}", out.content);
            let meta = out.metadata.as_ref().unwrap();
            assert!(meta["total_lines"].as_u64().unwrap() >= 1);
        }

        #[tokio::test]
        async fn stress_file_read_binary_content() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("binary.bin");
            let content: Vec<u8> = (0..256).map(|i| i as u8).collect();
            std::fs::write(&f, &content).unwrap();

            let result = tool().execute(tmp_input("fr-s2", json!({"path": f.to_str().unwrap()}), &dir)).await;
            // Binary file may return Ok(error) or Err — either is acceptable.
            match result {
                Ok(out) => {
                    assert_id_propagated(&out, "fr-s2");
                }
                Err(e) => {
                    // Graceful error on binary content (e.g., "not valid UTF-8").
                    let msg = format!("{e}");
                    assert!(msg.contains("UTF-8") || msg.contains("binary"), "Unexpected error: {msg}");
                }
            }
        }

        #[tokio::test]
        async fn stress_file_read_symlink() {
            let dir = tempfile::TempDir::new().unwrap();
            let real = dir.path().join("real.txt");
            let link = dir.path().join("link.txt");
            std::fs::write(&real, "symlink content").unwrap();

            #[cfg(unix)]
            std::os::unix::fs::symlink(&real, &link).unwrap();
            #[cfg(not(unix))]
            std::fs::copy(&real, &link).unwrap();

            let out = tool().execute(tmp_input("fr-s3", json!({"path": link.to_str().unwrap()}), &dir)).await.unwrap();
            assert!(!out.is_error, "Symlink read should succeed");
            assert!(out.content.contains("symlink content"));
        }

        #[tokio::test]
        async fn stress_file_read_deleted_before_read() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("ephemeral.txt");
            std::fs::write(&f, "temp").unwrap();
            let path_str = f.to_str().unwrap().to_string();
            std::fs::remove_file(&f).unwrap();

            let result = tool().execute(tmp_input("fr-s4", json!({"path": path_str}), &dir)).await;
            // Deleted file should error — either Ok(is_error=true) or Err.
            match result {
                Ok(out) => assert!(out.is_error, "Deleted file should produce error"),
                Err(_) => {} // Err is also acceptable.
            }
        }
    }

    // ============================================================
    //  SECTION 20: STRESS TESTS — FILE_EDIT
    // ============================================================
    mod file_edit_stress {
        use super::*;

        fn tool() -> FileEditTool {
            FileEditTool::new(vec![], vec![])
        }

        #[tokio::test]
        async fn stress_file_edit_100kb_old_string() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("big_edit.txt");
            let content = "A".repeat(100_000);
            std::fs::write(&f, &content).unwrap();

            let out = tool().execute(tmp_input("fe-s1", json!({
                "path": f.to_str().unwrap(),
                "old_string": &content,
                "new_string": "replaced"
            }), &dir)).await.unwrap();
            assert!(!out.is_error, "100KB edit should succeed: {}", out.content);
            assert_eq!(std::fs::read_to_string(&f).unwrap(), "replaced");
        }

        #[tokio::test]
        async fn stress_file_edit_unicode() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("unicode.txt");
            std::fs::write(&f, "hello 🌍 world 你好 café").unwrap();

            let out = tool().execute(tmp_input("fe-s2", json!({
                "path": f.to_str().unwrap(),
                "old_string": "🌍 world 你好",
                "new_string": "🎉 universe 世界"
            }), &dir)).await.unwrap();
            assert!(!out.is_error, "Unicode edit should succeed: {}", out.content);
            let result = std::fs::read_to_string(&f).unwrap();
            assert!(result.contains("🎉 universe 世界"));
        }

        #[tokio::test]
        async fn stress_file_edit_concurrent_same_file() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("concurrent.txt");
            std::fs::write(&f, "first second third").unwrap();

            // Two sequential edits (not truly concurrent — verifies no corruption).
            let t = tool();
            let out1 = t.execute(tmp_input("fe-s3a", json!({
                "path": f.to_str().unwrap(),
                "old_string": "first",
                "new_string": "1st"
            }), &dir)).await.unwrap();
            assert!(!out1.is_error);

            let out2 = t.execute(tmp_input("fe-s3b", json!({
                "path": f.to_str().unwrap(),
                "old_string": "second",
                "new_string": "2nd"
            }), &dir)).await.unwrap();
            assert!(!out2.is_error);

            let result = std::fs::read_to_string(&f).unwrap();
            assert!(result.contains("1st"), "First edit should persist");
            assert!(result.contains("2nd"), "Second edit should persist");
            assert!(result.contains("third"), "Unedited text should persist");
        }
    }

    // ============================================================
    //  SECTION 21: STRESS TESTS — BASH
    // ============================================================
    mod bash_stress {
        use super::*;

        fn tool() -> BashTool {
            BashTool::new(120, cuervo_core::types::SandboxConfig::default())
        }

        #[tokio::test]
        async fn stress_bash_large_output() {
            let dir = tempfile::TempDir::new().unwrap();
            let out = tool().execute(tmp_input("ba-s1", json!({
                "command": "seq 1 10000"
            }), &dir)).await.unwrap();
            assert!(!out.is_error, "seq should succeed");
            // Output should contain numbers.
            assert!(out.content.contains("1") && out.content.contains("10000"));
        }

        #[tokio::test]
        async fn stress_bash_special_chars() {
            let dir = tempfile::TempDir::new().unwrap();
            let out = tool().execute(tmp_input("ba-s2", json!({
                "command": "echo \"hello $USER\" | tr 'a-z' 'A-Z'"
            }), &dir)).await.unwrap();
            assert!(!out.is_error, "Special chars should be handled");
            assert!(out.content.contains("HELLO"));
        }

        #[tokio::test]
        async fn stress_bash_timeout_boundary() {
            let dir = tempfile::TempDir::new().unwrap();
            // Short timeout tool.
            let fast_tool = BashTool::new(1, cuervo_core::types::SandboxConfig::default());

            // Command that completes quickly — should succeed.
            let out = fast_tool.execute(tmp_input("ba-s3a", json!({
                "command": "echo fast"
            }), &dir)).await.unwrap();
            assert!(!out.is_error, "Fast command should not timeout");

            // Command that takes too long — should timeout (Err or Ok with is_error).
            let result = fast_tool.execute(tmp_input("ba-s3b", json!({
                "command": "sleep 10"
            }), &dir)).await;
            match result {
                Ok(out) => assert!(out.is_error, "Slow command should report error"),
                Err(e) => {
                    let msg = format!("{e}").to_lowercase();
                    assert!(
                        msg.contains("timeout") || msg.contains("timed out"),
                        "Expected timeout error: {msg}"
                    );
                }
            }
        }
    }

    // ============================================================
    //  SECTION 22: STRESS TESTS — GREP
    // ============================================================
    mod grep_stress {
        use super::*;

        fn tool() -> GrepTool {
            GrepTool::new()
        }

        #[tokio::test]
        async fn stress_grep_1000_matches() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("many_matches.txt");
            let content: String = (0..1000).map(|i| format!("match_line_{i}\n")).collect();
            std::fs::write(&f, &content).unwrap();

            // Grep searches directories, use dir path.
            let out = tool().execute(tmp_input("gr-s1", json!({
                "pattern": "match_line_",
                "path": dir.path().to_str().unwrap()
            }), &dir)).await.unwrap();
            assert!(!out.is_error, "1000-match grep should succeed: {}", out.content);
            // Should contain results (possibly capped by MAX_RESULTS).
            assert!(out.content.contains("match_line_"));
        }

        #[tokio::test]
        async fn stress_grep_complex_regex() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("regex.txt");
            std::fs::write(&f, "foo123bar\nbaz456qux\nhello\nfoo789bar").unwrap();

            let out = tool().execute(tmp_input("gr-s2", json!({
                "pattern": "foo\\d+bar",
                "path": dir.path().to_str().unwrap()
            }), &dir)).await.unwrap();
            assert!(!out.is_error, "Complex regex should work: {}", out.content);
            assert!(out.content.contains("foo123bar"));
            assert!(out.content.contains("foo789bar"));
        }

        #[tokio::test]
        async fn stress_grep_binary_skip() {
            let dir = tempfile::TempDir::new().unwrap();
            let f = dir.path().join("binary.bin");
            let mut content = vec![0u8; 512];
            content.extend_from_slice(b"findme");
            content.extend_from_slice(&vec![0u8; 512]);
            std::fs::write(&f, &content).unwrap();

            // Should not panic on binary file.
            let out = tool().execute(tmp_input("gr-s3", json!({
                "pattern": "findme",
                "path": dir.path().to_str().unwrap()
            }), &dir)).await.unwrap();
            assert_id_propagated(&out, "gr-s3");
            // May or may not find the match in binary — important thing is no panic.
        }
    }

    // ============================================================
    //  SECTION 23: STRESS TESTS — GIT TOOLS
    // ============================================================
    mod git_stress {
        use super::*;

        fn init_git_repo(dir: &tempfile::TempDir) {
            std::process::Command::new("git")
                .args(["init"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            std::process::Command::new("git")
                .args(["config", "user.email", "test@test.com"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            std::process::Command::new("git")
                .args(["config", "user.name", "Test"])
                .current_dir(dir.path())
                .output()
                .unwrap();
        }

        #[tokio::test]
        async fn stress_git_status_dirty_repo() {
            let dir = tempfile::TempDir::new().unwrap();
            init_git_repo(&dir);

            // Create untracked + modified files.
            std::fs::write(dir.path().join("untracked.txt"), "untracked").unwrap();
            std::fs::write(dir.path().join("tracked.txt"), "initial").unwrap();
            std::process::Command::new("git")
                .args(["add", "tracked.txt"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            std::process::Command::new("git")
                .args(["commit", "-m", "init"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            std::fs::write(dir.path().join("tracked.txt"), "modified").unwrap();

            let tool = GitStatusTool::new();
            let out = tool.execute(tmp_input("gs-s1", json!({}), &dir)).await.unwrap();
            assert!(!out.is_error, "git status on dirty repo should succeed: {}", out.content);
            assert!(out.content.contains("untracked") || out.content.contains("tracked"));
        }

        #[tokio::test]
        async fn stress_git_diff_with_staged() {
            let dir = tempfile::TempDir::new().unwrap();
            init_git_repo(&dir);
            std::fs::write(dir.path().join("file.txt"), "original").unwrap();
            std::process::Command::new("git")
                .args(["add", "file.txt"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            std::process::Command::new("git")
                .args(["commit", "-m", "init"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            std::fs::write(dir.path().join("file.txt"), "changed").unwrap();
            std::process::Command::new("git")
                .args(["add", "file.txt"])
                .current_dir(dir.path())
                .output()
                .unwrap();

            let tool = GitDiffTool::new();
            let out = tool.execute(tmp_input("gd-s1", json!({"staged": true}), &dir)).await.unwrap();
            assert!(!out.is_error, "git diff --staged should succeed: {}", out.content);
            assert!(out.content.contains("changed") || out.content.contains("original"));
        }

        #[tokio::test]
        async fn stress_git_log_empty_repo() {
            let dir = tempfile::TempDir::new().unwrap();
            init_git_repo(&dir);

            let tool = GitLogTool::new();
            let out = tool.execute(tmp_input("gl-s1", json!({}), &dir)).await.unwrap();
            // Empty repo has no commits — should handle gracefully.
            assert_id_propagated(&out, "gl-s1");
        }
    }

    // ============================================================
    //  SECTION 24: STRESS TESTS — BACKGROUND TOOLS
    // ============================================================
    mod background_stress {
        use super::*;

        fn registry() -> Arc<ProcessRegistry> {
            Arc::new(ProcessRegistry::new(10))
        }

        #[tokio::test]
        async fn stress_background_rapid_start_kill() {
            let reg = registry();
            let start = BackgroundStartTool::new(reg.clone());
            let kill = BackgroundKillTool::new(reg.clone());
            let dir = tempfile::TempDir::new().unwrap();

            let mut job_ids = Vec::new();
            // Start 5 processes.
            for i in 0..5 {
                let out = start.execute(tmp_input(
                    &format!("bg-s1-{i}"),
                    json!({"command": "sleep 60"}),
                    &dir,
                )).await.unwrap();
                assert!(!out.is_error, "start {i} should succeed: {}", out.content);
                // Extract job_id from metadata.
                if let Some(meta) = &out.metadata {
                    if let Some(jid) = meta.get("job_id") {
                        job_ids.push(jid.as_str().unwrap().to_string());
                    }
                }
            }

            assert_eq!(job_ids.len(), 5, "Should have started 5 jobs");

            // Kill all.
            for (i, jid) in job_ids.iter().enumerate() {
                let out = kill.execute(tmp_input(
                    &format!("bg-k1-{i}"),
                    json!({"job_id": jid}),
                    &dir,
                )).await.unwrap();
                assert!(!out.is_error, "kill {i} should succeed: {}", out.content);
            }
        }

        #[tokio::test]
        async fn stress_background_output_completed() {
            let reg = registry();
            let start = BackgroundStartTool::new(reg.clone());
            let output = BackgroundOutputTool::new(reg.clone());
            let dir = tempfile::TempDir::new().unwrap();

            let out = start.execute(tmp_input("bg-s2", json!({"command": "echo done_marker"}), &dir))
                .await.unwrap();
            assert!(!out.is_error);

            let job_id = out.metadata.as_ref().unwrap()["job_id"]
                .as_str().unwrap().to_string();

            // Wait a bit for the process to complete.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            let out = output.execute(tmp_input("bg-o2", json!({"job_id": &job_id}), &dir))
                .await.unwrap();
            // Output should contain our marker.
            assert!(
                out.content.contains("done_marker"),
                "Expected 'done_marker' in output: {}",
                out.content
            );
        }

        #[tokio::test]
        async fn stress_background_kill_dead_process() {
            let reg = registry();
            let start = BackgroundStartTool::new(reg.clone());
            let kill = BackgroundKillTool::new(reg.clone());
            let dir = tempfile::TempDir::new().unwrap();

            // Start a process that exits immediately.
            let out = start.execute(tmp_input("bg-s3", json!({"command": "true"}), &dir))
                .await.unwrap();
            let job_id = out.metadata.as_ref().unwrap()["job_id"]
                .as_str().unwrap().to_string();

            // Wait for it to exit.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            // Kill the already-dead process — should not panic.
            let out = kill.execute(tmp_input("bg-k3", json!({"job_id": &job_id}), &dir))
                .await.unwrap();
            // May succeed or return a graceful error.
            assert_id_propagated(&out, "bg-k3");
        }
    }

    // ============================================================
    //  SECTION 25: PHASE 32 — DESTRUCTIVE TOOL HARDENING TESTS
    // ============================================================
    //
    // Adversarial, stress, and security tests for all file-writing tools.
    // Tests: atomic writes, symlink protection, size limits, concurrent
    // writes, TOCTOU, permission levels, crash recovery.
    // ============================================================

    mod destructive_hardening {
        use super::*;

        // --- file_write atomic write tests ---

        #[tokio::test]
        async fn file_write_atomic_no_partial_on_disk() {
            // Verify the write is atomic: file should contain complete content or not exist.
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);
            let content = "A".repeat(100_000); // 100KB

            let out = tool.execute(tmp_input(
                "aw1",
                json!({"path": "atomic_test.txt", "content": content}),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error, "write should succeed: {}", out.content);

            // Verify content is complete (not truncated).
            let on_disk = std::fs::read_to_string(dir.path().join("atomic_test.txt")).unwrap();
            assert_eq!(on_disk.len(), 100_000, "file must contain exactly 100KB");
            assert!(on_disk.chars().all(|c| c == 'A'), "content must be all A's");
        }

        #[tokio::test]
        async fn file_write_no_temp_file_left_behind() {
            // After a successful write, no .cuervo_tmp_ files should remain.
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);

            tool.execute(tmp_input(
                "aw2",
                json!({"path": "clean.txt", "content": "hello"}),
                &dir,
            )).await.unwrap();

            let entries: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with(".cuervo_tmp_"))
                .collect();
            assert!(entries.is_empty(), "temp files left behind: {:?}",
                entries.iter().map(|e| e.file_name()).collect::<Vec<_>>());
        }

        #[tokio::test]
        async fn file_write_overwrite_preserves_atomicity() {
            // Overwriting an existing file should be atomic.
            let dir = tempfile::TempDir::new().unwrap();
            let file = dir.path().join("overwrite.txt");
            std::fs::write(&file, "original content that must not be corrupted").unwrap();

            let tool = FileWriteTool::new(vec![], vec![]);
            tool.execute(tmp_input(
                "aw3",
                json!({"path": "overwrite.txt", "content": "new content"}),
                &dir,
            )).await.unwrap();

            let on_disk = std::fs::read_to_string(&file).unwrap();
            assert_eq!(on_disk, "new content");
        }

        // --- file_write size limit tests ---

        #[tokio::test]
        async fn file_write_rejects_oversized_content() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);
            let huge = "X".repeat(11 * 1024 * 1024); // 11 MB > 10 MB limit

            let result = tool.execute(tmp_input(
                "sz1",
                json!({"path": "huge.bin", "content": huge}),
                &dir,
            )).await;
            assert!(result.is_err(), "should reject >10MB writes");
            let err = result.unwrap_err().to_string();
            assert!(err.contains("exceeds limit"), "error: {err}");

            // File must NOT exist.
            assert!(!dir.path().join("huge.bin").exists(), "no partial file on disk");
        }

        #[tokio::test]
        async fn file_write_accepts_just_under_limit() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);
            let content = "X".repeat(10 * 1024 * 1024); // exactly 10 MB

            let out = tool.execute(tmp_input(
                "sz2",
                json!({"path": "big.bin", "content": content}),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error, "10MB should be accepted: {}", out.content);
        }

        // --- file_write symlink protection tests ---

        #[cfg(unix)]
        #[tokio::test]
        async fn file_write_rejects_symlink_target() {
            let dir = tempfile::TempDir::new().unwrap();
            let real_file = dir.path().join("real.txt");
            std::fs::write(&real_file, "original").unwrap();

            let link_path = dir.path().join("link.txt");
            std::os::unix::fs::symlink(&real_file, &link_path).unwrap();

            let tool = FileWriteTool::new(vec![], vec![]);
            let result = tool.execute(tmp_input(
                "sym1",
                json!({"path": "link.txt", "content": "malicious"}),
                &dir,
            )).await;
            assert!(result.is_err(), "should reject writes through symlinks");
            let err = result.unwrap_err().to_string();
            assert!(err.contains("symlink"), "error: {err}");

            // Original file must NOT be modified.
            let content = std::fs::read_to_string(&real_file).unwrap();
            assert_eq!(content, "original", "symlink target must not be modified");
        }

        #[cfg(unix)]
        #[tokio::test]
        async fn file_write_allows_non_symlink() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);

            // Write to a regular file (no symlink).
            let out = tool.execute(tmp_input(
                "sym2",
                json!({"path": "regular.txt", "content": "safe"}),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error);
        }

        // --- file_edit atomic write tests ---

        #[tokio::test]
        async fn file_edit_atomic_preserves_content_on_success() {
            let dir = tempfile::TempDir::new().unwrap();
            let file = dir.path().join("edit_atomic.txt");
            std::fs::write(&file, "hello world\ngoodbye world\n").unwrap();

            let tool = FileEditTool::new(vec![], vec![]);
            let out = tool.execute(tmp_input(
                "ea1",
                json!({
                    "path": "edit_atomic.txt",
                    "old_string": "hello",
                    "new_string": "HELLO"
                }),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error, "edit should succeed: {}", out.content);

            let on_disk = std::fs::read_to_string(&file).unwrap();
            assert_eq!(on_disk, "HELLO world\ngoodbye world\n");
        }

        #[tokio::test]
        async fn file_edit_no_temp_files_after_success() {
            let dir = tempfile::TempDir::new().unwrap();
            let file = dir.path().join("edit_clean.txt");
            std::fs::write(&file, "aaa bbb ccc").unwrap();

            let tool = FileEditTool::new(vec![], vec![]);
            tool.execute(tmp_input(
                "ea2",
                json!({
                    "path": "edit_clean.txt",
                    "old_string": "bbb",
                    "new_string": "BBB"
                }),
                &dir,
            )).await.unwrap();

            let temps: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with(".cuervo_tmp_"))
                .collect();
            assert!(temps.is_empty(), "temp files left: {:?}",
                temps.iter().map(|e| e.file_name()).collect::<Vec<_>>());
        }

        // --- file_edit size limit tests ---

        #[tokio::test]
        async fn file_edit_rejects_oversized_source_file() {
            let dir = tempfile::TempDir::new().unwrap();
            let file = dir.path().join("huge_source.txt");
            let huge = "X".repeat(11 * 1024 * 1024);
            std::fs::write(&file, &huge).unwrap();

            let tool = FileEditTool::new(vec![], vec![]);
            let result = tool.execute(tmp_input(
                "esz1",
                json!({
                    "path": "huge_source.txt",
                    "old_string": "X",
                    "new_string": "Y"
                }),
                &dir,
            )).await;
            assert!(result.is_err(), "should reject editing files >10MB");
            let err = result.unwrap_err().to_string();
            assert!(err.contains("exceeds limit"), "error: {err}");
        }

        // --- file_edit symlink protection ---

        #[cfg(unix)]
        #[tokio::test]
        async fn file_edit_rejects_symlink() {
            let dir = tempfile::TempDir::new().unwrap();
            let real = dir.path().join("real_edit.txt");
            std::fs::write(&real, "important data").unwrap();

            let link = dir.path().join("link_edit.txt");
            std::os::unix::fs::symlink(&real, &link).unwrap();

            let tool = FileEditTool::new(vec![], vec![]);
            let result = tool.execute(tmp_input(
                "esym1",
                json!({
                    "path": "link_edit.txt",
                    "old_string": "important",
                    "new_string": "HACKED"
                }),
                &dir,
            )).await;
            assert!(result.is_err(), "should reject edit through symlink");

            // Original must be untouched.
            let content = std::fs::read_to_string(&real).unwrap();
            assert_eq!(content, "important data");
        }

        // --- file_edit permission level ---

        #[test]
        fn file_edit_is_destructive() {
            let tool = FileEditTool::new(vec![], vec![]);
            assert_eq!(
                tool.permission_level(),
                PermissionLevel::Destructive,
                "file_edit should be Destructive (read-modify-write can corrupt)"
            );
        }

        #[test]
        fn file_write_is_destructive() {
            let tool = FileWriteTool::new(vec![], vec![]);
            assert_eq!(tool.permission_level(), PermissionLevel::Destructive);
        }

        #[test]
        fn file_delete_is_destructive() {
            let tool = FileDeleteTool::new(vec![], vec![]);
            assert_eq!(tool.permission_level(), PermissionLevel::Destructive);
        }

        // --- file_delete TOCTOU / symlink tests ---

        #[cfg(unix)]
        #[tokio::test]
        async fn file_delete_rejects_symlink() {
            let dir = tempfile::TempDir::new().unwrap();
            let real = dir.path().join("precious.txt");
            std::fs::write(&real, "do not delete").unwrap();

            let link = dir.path().join("link_to_precious.txt");
            std::os::unix::fs::symlink(&real, &link).unwrap();

            let tool = FileDeleteTool::new(vec![dir.path().to_path_buf()], vec![]);
            let out = tool.execute(tmp_input(
                "dsym1",
                json!({"path": "link_to_precious.txt"}),
                &dir,
            )).await.unwrap();
            assert!(out.is_error, "should refuse to delete symlinks");
            assert!(out.content.contains("symlink"), "error: {}", out.content);

            // Both the symlink and the target must still exist.
            assert!(link.exists(), "symlink must not be deleted");
            assert!(real.exists(), "real file must not be deleted");
        }

        // --- Concurrent write stress test ---

        #[tokio::test]
        async fn concurrent_writes_to_different_files_no_corruption() {
            // 50 concurrent writes to different files — no data loss.
            let dir = tempfile::TempDir::new().unwrap();
            let tool = std::sync::Arc::new(FileWriteTool::new(vec![], vec![]));
            let wd = dir.path().to_str().unwrap().to_string();

            let mut handles = Vec::new();
            for i in 0..50 {
                let t = tool.clone();
                let w = wd.clone();
                handles.push(tokio::spawn(async move {
                    let content = format!("file_{i}_content_{}", "X".repeat(1000));
                    t.execute(ToolInput {
                        tool_use_id: format!("cw-{i}"),
                        arguments: json!({"path": format!("file_{i}.txt"), "content": content}),
                        working_directory: w,
                    }).await
                }));
            }

            for (i, h) in handles.into_iter().enumerate() {
                let result = h.await.unwrap();
                assert!(result.is_ok(), "write {i} failed: {:?}", result.err());
            }

            // Verify all files exist with correct content.
            for i in 0..50 {
                let path = dir.path().join(format!("file_{i}.txt"));
                let content = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("file_{i}.txt missing: {e}"));
                assert!(content.starts_with(&format!("file_{i}_content_")),
                    "file_{i} has wrong content prefix");
                assert_eq!(content.len(), format!("file_{i}_content_").len() + 1000);
            }
        }

        // --- Unicode edge cases ---

        #[tokio::test]
        async fn file_write_unicode_content_preserved() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);
            let unicode = "Hello 你好 مرحبا こんにちは 🦀🔥💯 Ñ ü ö ä";

            let out = tool.execute(tmp_input(
                "uni1",
                json!({"path": "unicode.txt", "content": unicode}),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error);

            let on_disk = std::fs::read_to_string(dir.path().join("unicode.txt")).unwrap();
            assert_eq!(on_disk, unicode, "unicode content must be preserved exactly");
        }

        #[tokio::test]
        async fn file_edit_unicode_replacement() {
            let dir = tempfile::TempDir::new().unwrap();
            let file = dir.path().join("uni_edit.txt");
            std::fs::write(&file, "Hello 你好 world").unwrap();

            let tool = FileEditTool::new(vec![], vec![]);
            let out = tool.execute(tmp_input(
                "uni2",
                json!({
                    "path": "uni_edit.txt",
                    "old_string": "你好",
                    "new_string": "こんにちは"
                }),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error, "unicode edit should work: {}", out.content);

            let on_disk = std::fs::read_to_string(&file).unwrap();
            assert_eq!(on_disk, "Hello こんにちは world");
        }

        // --- Empty file edge cases ---

        #[tokio::test]
        async fn file_write_empty_content_creates_empty_file() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);

            let out = tool.execute(tmp_input(
                "empty1",
                json!({"path": "empty.txt", "content": ""}),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error);
            assert!(out.content.contains("0 bytes"));

            let on_disk = std::fs::read_to_string(dir.path().join("empty.txt")).unwrap();
            assert_eq!(on_disk, "");
        }

        #[tokio::test]
        async fn file_edit_on_file_that_becomes_empty() {
            let dir = tempfile::TempDir::new().unwrap();
            let file = dir.path().join("vanish.txt");
            std::fs::write(&file, "goodbye").unwrap();

            let tool = FileEditTool::new(vec![], vec![]);
            let out = tool.execute(tmp_input(
                "empty2",
                json!({
                    "path": "vanish.txt",
                    "old_string": "goodbye",
                    "new_string": ""
                }),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error);

            let on_disk = std::fs::read_to_string(&file).unwrap();
            assert_eq!(on_disk, "");
        }

        // --- Path traversal hardening ---

        #[tokio::test]
        async fn file_write_rejects_path_traversal() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);

            let result = tool.execute(tmp_input(
                "pt1",
                json!({"path": "../../../etc/passwd", "content": "hacked"}),
                &dir,
            )).await;
            assert!(result.is_err(), "path traversal must be rejected");
        }

        #[tokio::test]
        async fn file_write_rejects_absolute_path_outside_workspace() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);

            let result = tool.execute(tmp_input(
                "pt2",
                json!({"path": "/etc/passwd", "content": "hacked"}),
                &dir,
            )).await;
            assert!(result.is_err(), "absolute path outside workspace must be rejected");
        }

        #[tokio::test]
        async fn file_edit_rejects_path_traversal() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileEditTool::new(vec![], vec![]);

            let result = tool.execute(tmp_input(
                "pt3",
                json!({
                    "path": "../../etc/passwd",
                    "old_string": "root",
                    "new_string": "hacked"
                }),
                &dir,
            )).await;
            assert!(result.is_err(), "path traversal must be rejected");
        }

        // --- Blocked pattern tests ---

        #[tokio::test]
        async fn file_write_blocks_sensitive_patterns() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![
                ".env".into(),
                "*.pem".into(),
                "*.key".into(),
                "credentials.json".into(),
            ]);

            for name in &[".env", "server.pem", "api.key", "credentials.json"] {
                let result = tool.execute(tmp_input(
                    "bp",
                    json!({"path": name, "content": "secret"}),
                    &dir,
                )).await;
                assert!(result.is_err(), "should block write to {name}");
            }
        }

        // --- Permission denied handling ---

        #[cfg(unix)]
        #[tokio::test]
        async fn file_write_handles_permission_denied() {
            // Skip if running as root (root bypasses filesystem permissions).
            if unsafe { libc::geteuid() } == 0 {
                eprintln!("SKIP: running as root, permission tests not applicable");
                return;
            }
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::TempDir::new().unwrap();
            let subdir = dir.path().join("readonly_dir");
            std::fs::create_dir(&subdir).unwrap();
            std::fs::write(subdir.join("existing.txt"), "old").unwrap();
            std::fs::set_permissions(&subdir, std::fs::Permissions::from_mode(0o555)).unwrap();

            let tool = FileWriteTool::new(vec![], vec![]);
            let result = tool.execute(tmp_input(
                "perm1",
                json!({"path": "readonly_dir/existing.txt", "content": "blocked"}),
                &dir,
            )).await;
            assert!(result.is_err(), "should fail on read-only directory");

            let content = std::fs::read_to_string(subdir.join("existing.txt")).unwrap();
            assert_eq!(content, "old", "file in read-only dir must not be modified");

            std::fs::set_permissions(&subdir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        #[cfg(unix)]
        #[tokio::test]
        async fn file_edit_handles_readonly_directory() {
            // Skip if running as root (root bypasses filesystem permissions).
            if unsafe { libc::geteuid() } == 0 {
                eprintln!("SKIP: running as root, permission tests not applicable");
                return;
            }
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::TempDir::new().unwrap();
            let subdir = dir.path().join("ro_edit_dir");
            std::fs::create_dir(&subdir).unwrap();
            std::fs::write(subdir.join("target.txt"), "original").unwrap();
            std::fs::set_permissions(&subdir, std::fs::Permissions::from_mode(0o555)).unwrap();

            let tool = FileEditTool::new(vec![], vec![]);
            let result = tool.execute(tmp_input(
                "perm2",
                json!({
                    "path": "ro_edit_dir/target.txt",
                    "old_string": "original",
                    "new_string": "modified"
                }),
                &dir,
            )).await;
            assert!(result.is_err(), "should fail when parent dir is read-only");

            let content = std::fs::read_to_string(subdir.join("target.txt")).unwrap();
            assert_eq!(content, "original", "file must not be modified");

            std::fs::set_permissions(&subdir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // --- Determinism: repeated identical calls yield same result ---

        #[tokio::test]
        async fn file_write_deterministic_repeated_calls() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);

            for i in 0..10 {
                let out = tool.execute(tmp_input(
                    &format!("det-{i}"),
                    json!({"path": "deterministic.txt", "content": "exact same content"}),
                    &dir,
                )).await.unwrap();
                assert!(!out.is_error);
            }

            // File should contain exactly the last write (all identical).
            let on_disk = std::fs::read_to_string(dir.path().join("deterministic.txt")).unwrap();
            assert_eq!(on_disk, "exact same content");
        }

        #[tokio::test]
        async fn file_edit_deterministic_after_first_apply() {
            let dir = tempfile::TempDir::new().unwrap();
            let file = dir.path().join("det_edit.txt");
            std::fs::write(&file, "aaa bbb ccc").unwrap();

            let tool = FileEditTool::new(vec![], vec![]);

            // First edit: succeeds.
            let out = tool.execute(tmp_input(
                "det1",
                json!({
                    "path": "det_edit.txt",
                    "old_string": "bbb",
                    "new_string": "BBB"
                }),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error);

            // Second identical edit: old_string "bbb" no longer found → is_error.
            let out = tool.execute(tmp_input(
                "det2",
                json!({
                    "path": "det_edit.txt",
                    "old_string": "bbb",
                    "new_string": "BBB"
                }),
                &dir,
            )).await.unwrap();
            assert!(out.is_error, "second identical edit should report not-found");
            assert!(out.content.contains("not found"));
        }

        // --- Stress: 100 rapid sequential writes ---

        #[tokio::test]
        async fn stress_100_sequential_writes() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);

            for i in 0..100 {
                let content = format!("write_{i}_{}", "X".repeat(500));
                let out = tool.execute(tmp_input(
                    &format!("s100-{i}"),
                    json!({"path": "stress_file.txt", "content": content}),
                    &dir,
                )).await.unwrap();
                assert!(!out.is_error, "write {i} failed: {}", out.content);
            }

            // Final content should be the last write.
            let on_disk = std::fs::read_to_string(dir.path().join("stress_file.txt")).unwrap();
            assert!(on_disk.starts_with("write_99_"));
        }

        // --- Stress: 50 rapid sequential edits ---

        #[tokio::test]
        async fn stress_50_sequential_edits() {
            let dir = tempfile::TempDir::new().unwrap();
            let file = dir.path().join("stress_edit.txt");
            std::fs::write(&file, "counter=0").unwrap();

            let tool = FileEditTool::new(vec![], vec![]);
            for i in 0..50 {
                let out = tool.execute(tmp_input(
                    &format!("se-{i}"),
                    json!({
                        "path": "stress_edit.txt",
                        "old_string": format!("counter={i}"),
                        "new_string": format!("counter={}", i + 1)
                    }),
                    &dir,
                )).await.unwrap();
                assert!(!out.is_error, "edit {i} failed: {}", out.content);
            }

            let on_disk = std::fs::read_to_string(&file).unwrap();
            assert_eq!(on_disk, "counter=50");
        }

        // --- Newline preservation ---

        #[tokio::test]
        async fn file_write_preserves_line_endings() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);

            // Unix line endings.
            let unix_content = "line1\nline2\nline3\n";
            tool.execute(tmp_input(
                "nl1",
                json!({"path": "unix.txt", "content": unix_content}),
                &dir,
            )).await.unwrap();
            let on_disk = std::fs::read_to_string(dir.path().join("unix.txt")).unwrap();
            assert_eq!(on_disk, unix_content);

            // Windows line endings.
            let win_content = "line1\r\nline2\r\nline3\r\n";
            tool.execute(tmp_input(
                "nl2",
                json!({"path": "win.txt", "content": win_content}),
                &dir,
            )).await.unwrap();
            let on_disk = std::fs::read_to_string(dir.path().join("win.txt")).unwrap();
            assert_eq!(on_disk, win_content);
        }

        // --- Deep nested directory creation ---

        #[tokio::test]
        async fn file_write_creates_deep_nested_dirs() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);

            let out = tool.execute(tmp_input(
                "deep1",
                json!({"path": "a/b/c/d/e/f/g/h/deep.txt", "content": "deep"}),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error);

            let on_disk = std::fs::read_to_string(
                dir.path().join("a/b/c/d/e/f/g/h/deep.txt")
            ).unwrap();
            assert_eq!(on_disk, "deep");
        }

        // --- Binary-like content ---

        #[tokio::test]
        async fn file_write_handles_null_bytes_in_content() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);

            // Content with null bytes (valid UTF-8 but unusual).
            let content = "before\0middle\0after";
            let out = tool.execute(tmp_input(
                "null1",
                json!({"path": "nullbytes.txt", "content": content}),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error);

            let on_disk = std::fs::read_to_string(dir.path().join("nullbytes.txt")).unwrap();
            assert_eq!(on_disk, content);
        }

        // --- Missing arguments ---

        #[tokio::test]
        async fn file_write_missing_path_arg() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);
            let result = tool.execute(tmp_input(
                "miss1",
                json!({"content": "no path"}),
                &dir,
            )).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn file_write_missing_content_arg() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);
            let result = tool.execute(tmp_input(
                "miss2",
                json!({"path": "test.txt"}),
                &dir,
            )).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn file_edit_nonexistent_file() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileEditTool::new(vec![], vec![]);
            let result = tool.execute(tmp_input(
                "nexist1",
                json!({
                    "path": "nonexistent.txt",
                    "old_string": "x",
                    "new_string": "y"
                }),
                &dir,
            )).await;
            assert!(result.is_err(), "editing nonexistent file should fail");
        }

        // --- Metadata correctness ---

        #[tokio::test]
        async fn file_write_metadata_contains_bytes_and_path() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);

            let out = tool.execute(tmp_input(
                "meta1",
                json!({"path": "meta.txt", "content": "12345"}),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error);

            let meta = out.metadata.unwrap();
            assert_eq!(meta["bytes_written"], 5);
            assert!(meta["path"].as_str().unwrap().ends_with("meta.txt"));
        }

        #[tokio::test]
        async fn file_edit_metadata_contains_replacements() {
            let dir = tempfile::TempDir::new().unwrap();
            let file = dir.path().join("meta_edit.txt");
            std::fs::write(&file, "aaa bbb ccc").unwrap();

            let tool = FileEditTool::new(vec![], vec![]);
            let out = tool.execute(tmp_input(
                "meta2",
                json!({
                    "path": "meta_edit.txt",
                    "old_string": "bbb",
                    "new_string": "BBB"
                }),
                &dir,
            )).await.unwrap();
            assert!(!out.is_error);

            let meta = out.metadata.unwrap();
            assert_eq!(meta["replacements"], 1);
        }

        // --- tool_use_id propagation ---

        #[tokio::test]
        async fn file_write_propagates_tool_use_id() {
            let dir = tempfile::TempDir::new().unwrap();
            let tool = FileWriteTool::new(vec![], vec![]);

            let out = tool.execute(tmp_input(
                "unique-id-42",
                json!({"path": "id_test.txt", "content": "x"}),
                &dir,
            )).await.unwrap();
            assert_id_propagated(&out, "unique-id-42");
        }

        #[tokio::test]
        async fn file_edit_propagates_tool_use_id() {
            let dir = tempfile::TempDir::new().unwrap();
            let file = dir.path().join("id_edit.txt");
            std::fs::write(&file, "old").unwrap();

            let tool = FileEditTool::new(vec![], vec![]);
            let out = tool.execute(tmp_input(
                "edit-id-99",
                json!({
                    "path": "id_edit.txt",
                    "old_string": "old",
                    "new_string": "new"
                }),
                &dir,
            )).await.unwrap();
            assert_id_propagated(&out, "edit-id-99");
        }

        #[tokio::test]
        async fn file_delete_propagates_tool_use_id() {
            let dir = tempfile::TempDir::new().unwrap();
            let file = dir.path().join("del_id.txt");
            std::fs::write(&file, "bye").unwrap();

            let tool = FileDeleteTool::new(vec![dir.path().to_path_buf()], vec![]);
            let out = tool.execute(tmp_input(
                "del-id-77",
                json!({"path": "del_id.txt"}),
                &dir,
            )).await.unwrap();
            assert_id_propagated(&out, "del-id-77");
        }
    }
}
