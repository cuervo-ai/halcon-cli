# File Intelligence — Baseline Audit (Phase 1)

> Generated: 2026-02-09 | Cuervo CLI v0.1.0 | 1664 tests

## Current File-Handling Tools (8 registered)

| Tool | File | Permission | Capabilities |
|------|------|-----------|--------------|
| `file_read` | `cuervo-tools/src/file_read.rs` | ReadOnly | UTF-8 text read, line offset/limit, numbered output |
| `file_write` | `cuervo-tools/src/file_write.rs` | Destructive | UTF-8 text write, overwrite only |
| `file_edit` | `cuervo-tools/src/file_edit.rs` | Destructive | String replacement in UTF-8 files, syntax check gate |
| `bash` | `cuervo-tools/src/bash.rs` | Destructive | Shell command execution, sandboxed |
| `glob` | `cuervo-tools/src/glob_tool.rs` | ReadOnly | File pattern matching |
| `grep` | `cuervo-tools/src/grep.rs` | ReadOnly | Content search with regex |
| `web_fetch` | `cuervo-tools/src/web_fetch.rs` | ReadOnly | HTTP GET, HTML→text |
| `directory_tree` | `cuervo-tools/src/directory_tree.rs` | ReadOnly | Recursive directory listing |

## Architecture

### Tool Trait (`cuervo-core/src/traits/tool.rs`)
```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn permission_level(&self) -> PermissionLevel;
    async fn execute(&self, input: ToolInput) -> Result<ToolOutput>;
    fn requires_confirmation(&self, input: &ToolInput) -> bool;
    fn input_schema(&self) -> serde_json::Value;
}
```

### Tool Registry (`cuervo-tools/src/registry.rs`)
- `HashMap<String, Arc<dyn Tool>>` — extensible via `register()`
- `default_registry(config)` builds all 8 tools from `ToolsConfig`
- `tool_definitions()` generates JSON schemas for model API

### ToolInput / ToolOutput (`cuervo-core/src/types/tool.rs`)
- Input: `tool_use_id: String`, `arguments: serde_json::Value`, `working_directory: String`
- Output: `tool_use_id: String`, `content: String`, `is_error: bool`, `metadata: Option<Value>`

### Security Layer (`cuervo-tools/src/path_security.rs`)
- Path traversal prevention via `normalize_path()` (resolve `.` and `..` without fs access)
- Blocked pattern matching (`.env`, `*.pem`, `*.key`, `credentials.json`)
- Allowed directory enforcement (working_dir + explicit allow list)
- `CompiledPatterns` for zero per-call glob compilation
- `resolve_and_validate()` / `resolve_and_validate_compiled()` gate all file ops

### Sandbox (`cuervo-tools/src/sandbox.rs`)
- Unix rlimits: `RLIMIT_CPU` (60s), `RLIMIT_FSIZE` (50MB)
- Output truncation: 100KB max, preserves head (60%) + tail (30%)
- UTF-8 safe truncation at char boundaries
- Memory limit: 512MB (via max_output_bytes, not RLIMIT_AS)

### Syntax Check (`cuervo-tools/src/syntax_check.rs`)
- Post-write/edit validation for: Rust, Python, JavaScript, TypeScript, Go, JSON, TOML, YAML
- Balanced delimiter checking: `{}`, `[]`, `()`
- String/comment/raw-string awareness (skips delimiters inside)
- Appends warnings to tool output for model self-correction

### Context Integration (`cuervo-context/src/assembler.rs`)
- Token estimation: `text.len().div_ceil(4)` (~4 chars/token heuristic)
- Budget-based context assembly from multiple `ContextSource` impls
- Priority-sorted chunk selection within token budget
- No file-type-aware tokenization

## What Works
1. UTF-8 text file read/write/edit with line-level precision
2. Path security: traversal prevention, blocked patterns, sandboxing
3. Syntax checking for 7+ languages after edits
4. Iterator-based line processing (no intermediate Vec allocation)
5. Tool output truncation with head+tail preservation
6. Async execution via tokio::fs
7. ToolRegistry is extensible — new tools register with `Arc::new()`

## Critical Limitations
1. **UTF-8 only**: `read_to_string()` fails on binary files (PDF, images, archives)
2. **No file size pre-check**: Full file loaded to memory before any processing → OOM risk
3. **No binary detection**: No magic byte / MIME type checking
4. **No streaming**: Entire file buffered in memory (single `tokio::fs::read_to_string`)
5. **No format conversion**: Can't extract text from structured formats
6. **No metadata extraction**: No EXIF, PDF metadata, CSV headers, etc.
7. **Token estimation is format-blind**: Same 4-char heuristic for code, prose, and data
8. **Output is always String**: `ToolOutput.content` has no binary variant
