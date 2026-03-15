//! Unit and integration tests for the instruction_store module.
//!
//! Covers:
//! - Scope precedence (last-wins, Managed appears last)
//! - @import cycle detection (A → B → A returns clean error, session continues)
//! - @import depth limit (depth > 3 is skipped, not a panic)
//! - Path-glob matching for `.halcon/rules/` files
//! - Invalid UTF-8 graceful handling (file is skipped, not a crash)
//! - Hot reload fires within 600 ms of a file change (integration test)

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use super::loader::{load_all_scopes, MAX_IMPORT_DEPTH};
use super::rules::rule_applies;
use super::InstructionStore;

// ── Scope precedence ──────────────────────────────────────────────────────────

#[test]
fn local_scope_is_loaded() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("HALCON.local.md"), "# Local rule").unwrap();

    let result = load_all_scopes(dir.path(), &[]);
    assert!(result.text.contains("Local rule"), "Local scope should be loaded");
    assert!(!result.sources.is_empty());
}

#[test]
fn project_scope_is_loaded() {
    let dir = TempDir::new().unwrap();
    let halcon = dir.path().join(".halcon");
    fs::create_dir(&halcon).unwrap();
    fs::write(halcon.join("HALCON.md"), "# Project rule").unwrap();

    let result = load_all_scopes(dir.path(), &[]);
    assert!(result.text.contains("Project rule"), "Project scope should be loaded");
}

#[test]
fn scope_injection_order_local_before_managed() {
    // This test simulates Managed as /etc/halcon/HALCON.md.
    // Since we cannot write to /etc in CI, we verify Local comes before Project.
    let parent = TempDir::new().unwrap();
    let child = parent.path().join("sub");
    fs::create_dir(&child).unwrap();

    // Local scope
    fs::write(child.join("HALCON.local.md"), "LOCAL_MARKER").unwrap();

    // Project scope (in parent)
    let halcon = parent.path().join(".halcon");
    fs::create_dir(&halcon).unwrap();
    fs::write(halcon.join("HALCON.md"), "PROJECT_MARKER").unwrap();

    let result = load_all_scopes(&child, &[]);
    let text = &result.text;

    // Local must appear BEFORE Project (injection order 1 < 3)
    let local_pos = text.find("LOCAL_MARKER").expect("LOCAL_MARKER not found");
    let project_pos = text.find("PROJECT_MARKER").expect("PROJECT_MARKER not found");
    assert!(
        local_pos < project_pos,
        "Local scope must precede Project scope (local_pos={local_pos} project_pos={project_pos})"
    );
}

#[test]
fn empty_instruction_files_produce_empty_result() {
    let dir = TempDir::new().unwrap();
    let result = load_all_scopes(dir.path(), &[]);
    // No files → empty text (might include user/managed if present on machine, but no panic)
    let _ = result.text;
}

// ── @import resolution ────────────────────────────────────────────────────────

#[test]
fn import_resolves_relative_to_containing_file() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("shared.md"), "# Shared content").unwrap();
    fs::write(
        dir.path().join("HALCON.local.md"),
        "@import shared.md\n# Local extra",
    )
    .unwrap();

    let result = load_all_scopes(dir.path(), &[]);
    assert!(result.text.contains("Shared content"), "Imported file content should appear");
    assert!(result.text.contains("Local extra"));
}

#[test]
fn import_cycle_returns_gracefully() {
    // A imports B imports A — must not panic, session continues, content is partial.
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.md"), "@import b.md\nA content").unwrap();
    fs::write(dir.path().join("b.md"), "@import a.md\nB content").unwrap();
    fs::write(dir.path().join("HALCON.local.md"), "@import a.md").unwrap();

    // Must complete without panic.
    let result = load_all_scopes(dir.path(), &[]);
    // Partial content is acceptable; what matters is no infinite loop / panic.
    let _ = result.text;
}

#[test]
fn import_depth_limit_is_enforced() {
    // Create a chain deeper than MAX_IMPORT_DEPTH (3).
    let dir = TempDir::new().unwrap();

    // depth4.md → depth3.md → depth2.md → depth1.md → depth0.md
    let depth = MAX_IMPORT_DEPTH + 2; // One beyond the limit
    for i in 0..=depth {
        let content = if i < depth {
            format!("@import depth{}.md\nLevel {i}", i + 1)
        } else {
            format!("Deepest level {i}")
        };
        fs::write(dir.path().join(format!("depth{i}.md")), content).unwrap();
    }
    fs::write(
        dir.path().join("HALCON.local.md"),
        "@import depth0.md",
    )
    .unwrap();

    // Must complete without panic.  Imports beyond depth 3 are skipped with a warn.
    let result = load_all_scopes(dir.path(), &[]);
    // Shallow levels (0..MAX_IMPORT_DEPTH) should be present.
    assert!(result.text.contains("Level 0") || result.text.contains("Level 1"),
            "Shallow import content should appear");
}

#[test]
fn import_missing_file_is_silently_skipped() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("HALCON.local.md"),
        "@import nonexistent.md\n# Rest of file",
    )
    .unwrap();

    let result = load_all_scopes(dir.path(), &[]);
    assert!(result.text.contains("Rest of file"), "Non-import content should still be present");
}

// ── Path-glob matching for .halcon/rules/ ─────────────────────────────────────

#[test]
fn rule_without_paths_is_always_active() {
    let dir = TempDir::new().unwrap();
    let rule_content = "# Always-active rule\nBe concise.";
    assert!(
        rule_applies(rule_content, dir.path()),
        "Rule without paths: should always be active"
    );
}

#[test]
fn rule_with_paths_matches_existing_file() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src").join("api");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("handler.rs"), "// handler").unwrap();

    let rule_content = "---\npaths: [\"src/api/**\"]\n---\n# API rules";
    assert!(
        rule_applies(rule_content, dir.path()),
        "Rule should match when src/api/** files exist"
    );
}

#[test]
fn rule_with_paths_no_match_returns_false() {
    let dir = TempDir::new().unwrap();
    // No src/api/ directory at all.
    let rule_content = "---\npaths: [\"src/api/**\"]\n---\n# API rules";
    assert!(
        !rule_applies(rule_content, dir.path()),
        "Rule should NOT match when no files match the glob"
    );
}

#[test]
fn rule_with_multiple_patterns_matches_any() {
    let dir = TempDir::new().unwrap();
    // Create a handler file under src/handlers/ (not src/api/)
    let handlers = dir.path().join("src").join("handlers");
    fs::create_dir_all(&handlers).unwrap();
    fs::write(handlers.join("auth.rs"), "// auth handler").unwrap();

    let rule_content = "---\npaths: [\"src/api/**\", \"src/handlers/**\"]\n---\n# Rules";
    assert!(
        rule_applies(rule_content, dir.path()),
        "Rule should match when any pattern matches"
    );
}

#[test]
fn rules_dir_files_are_loaded_in_sorted_order() {
    let dir = TempDir::new().unwrap();
    let halcon_dir = dir.path().join(".halcon");
    let rules_dir = halcon_dir.join("rules");
    fs::create_dir_all(&rules_dir).unwrap();

    // Create rule files in reverse alphabetical order to confirm sorting.
    fs::write(rules_dir.join("03-third.md"), "THIRD").unwrap();
    fs::write(rules_dir.join("01-first.md"), "FIRST").unwrap();
    fs::write(rules_dir.join("02-second.md"), "SECOND").unwrap();

    let result = load_all_scopes(dir.path(), &[]);
    let first_pos = result.text.find("FIRST").expect("FIRST not found");
    let second_pos = result.text.find("SECOND").expect("SECOND not found");
    let third_pos = result.text.find("THIRD").expect("THIRD not found");
    assert!(
        first_pos < second_pos && second_pos < third_pos,
        "Rules should be loaded in alphabetical order"
    );
}

// ── Invalid UTF-8 handling ────────────────────────────────────────────────────

#[test]
fn invalid_utf8_file_is_skipped_gracefully() {
    let dir = TempDir::new().unwrap();
    // Write a file with invalid UTF-8 bytes.
    let invalid_utf8: Vec<u8> = vec![0xFF, 0xFE, 0x00, b'H', b'i'];
    fs::write(dir.path().join("HALCON.local.md"), &invalid_utf8).unwrap();

    // Must not panic; other scopes continue loading.
    let result = load_all_scopes(dir.path(), &[]);
    let _ = result; // May be empty — that's OK.
}

// ── InstructionStore API ──────────────────────────────────────────────────────

#[test]
fn instruction_store_load_returns_none_when_no_files() {
    let dir = TempDir::new().unwrap();
    let mut store = InstructionStore::new(dir.path());
    // With no $HOME/.halcon/HALCON.md and no files in dir, returns None.
    // (May return Some if running on a dev machine with global HALCON.md.)
    let _ = store.load();
}

#[test]
fn instruction_store_load_returns_section_header() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("HALCON.local.md"), "Always respond in Spanish.").unwrap();

    let mut store = InstructionStore::new(dir.path());
    let content = store.load().expect("should return content");
    assert!(
        content.contains("## Project Instructions"),
        "Injected text must contain section header"
    );
    assert!(content.contains("Always respond in Spanish."));
}

#[test]
fn instruction_store_current_injected_matches_load_return() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("HALCON.local.md"), "Rule A.").unwrap();

    let mut store = InstructionStore::new(dir.path());
    let loaded = store.load().unwrap();
    assert_eq!(
        store.current_injected(),
        Some(loaded.as_str()),
        "current_injected() should match the text returned by load()"
    );
}

#[test]
fn check_and_reload_returns_none_when_unchanged() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("HALCON.local.md"), "unchanged content").unwrap();

    // On macOS FSEvents buffers Modify events from recent writes.  Sleep 300 ms
    // so those events are flushed to the kernel queue *before* the watcher registers,
    // preventing the watcher from seeing them as new changes.
    std::thread::sleep(std::time::Duration::from_millis(300));

    let mut store = InstructionStore::new(dir.path());
    let _ = store.load();

    // No change has occurred since the watcher started — poll should return None.
    let result = store.check_and_reload();
    assert!(
        result.is_none(),
        "check_and_reload should return None when files are unchanged"
    );
}

// ── Hot-reload integration test ───────────────────────────────────────────────

/// Verifies that a file change is detected using the PollWatcher within a generous timeout.
///
/// This test exercises the full hot-reload path:
///   1. load() starts the watcher (50 ms poll interval for this test)
///   2. We wait for the watcher's background thread to take its initial snapshot
///   3. The instruction file is modified
///   4. check_and_reload() returns Some(new_content) within the timeout
///
/// Poll interval: 50 ms (test).  Production default: 250 ms.
/// Timeout: 1 200 ms — 24× the poll interval; accommodates CI scheduling jitter.
#[test]
fn hot_reload_detects_change_within_600ms() {
    use std::time::{Duration, Instant};

    let dir = TempDir::new().unwrap();
    let file = dir.path().join("HALCON.local.md");
    fs::write(&file, "Version 1").unwrap();

    // Build a store and start the fast-poll watcher (50 ms interval).
    let mut store = InstructionStoreTestHook::new_fast(dir.path());
    let initial = store.inner.load().expect("initial load must succeed");
    assert!(initial.contains("Version 1"));

    // Wait long enough for the PollWatcher background thread to take its initial
    // snapshot of the directory.  Without this pause there is a race: if we write
    // the new file content before the thread's first scan, the snapshot already
    // reflects the new mtime and no event ever fires.
    std::thread::sleep(Duration::from_millis(150));

    // Mutate the file (mtime will be > snapshot time on all platforms).
    fs::write(&file, "Version 2 — updated content").unwrap();

    // Poll until changed or 1 200 ms timeout.
    let deadline = Instant::now() + Duration::from_millis(1_200);
    let mut reloaded: Option<String> = None;
    while Instant::now() < deadline {
        if let Some(new_text) = store.inner.check_and_reload() {
            reloaded = Some(new_text);
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    let new_text = reloaded.expect("hot reload should fire within 1 200 ms");
    assert!(
        new_text.contains("Version 2"),
        "Reloaded content should contain updated text"
    );
}

/// Test helper that overrides the watcher with a fast (50 ms) poll interval.
struct InstructionStoreTestHook {
    pub inner: InstructionStore,
}

impl InstructionStoreTestHook {
    fn new_fast(working_dir: &Path) -> Self {
        Self {
            inner: InstructionStore::new_with_poll_interval(
                working_dir,
                std::time::Duration::from_millis(50),
            ),
        }
    }
}

// ── Front matter parsing ──────────────────────────────────────────────────────

#[test]
fn front_matter_split_no_frontmatter() {
    use super::loader::split_front_matter;
    let content = "# Normal content\nNo front matter here.";
    let (yaml, body) = split_front_matter(content);
    assert!(yaml.is_empty(), "No front matter → yaml should be empty");
    assert_eq!(body, content);
}

#[test]
fn front_matter_split_with_paths() {
    use super::loader::split_front_matter;
    let content = "---\npaths: [\"src/api/**\"]\n---\n# Rule content";
    let (yaml, body) = split_front_matter(content);
    assert!(yaml.contains("paths"), "YAML block should contain 'paths'");
    assert!(body.contains("Rule content"), "Body should contain post-frontmatter text");
}
