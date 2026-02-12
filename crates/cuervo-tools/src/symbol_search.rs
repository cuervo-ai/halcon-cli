//! `symbol_search` tool: regex-based symbol extraction and search.
//!
//! Extracts function/struct/class/interface/enum/trait/const signatures from
//! Rust, Python, JS, TS, Go source files. Searches by query with optional
//! kind filter and directory scope. ReadOnly permission.
//!
//! Extraction logic adapted from `cuervo-context::repo_map` (Option C: duplicate
//! ~60 lines to avoid circular dependency).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::json;

use cuervo_core::error::{CuervoError, Result};
use cuervo_core::traits::Tool;
use cuervo_core::types::{PermissionLevel, ToolInput, ToolOutput};

// ── Symbol types ──────────────────────────────────────────────────────

/// Kind of symbol extracted from source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Class,
    Interface,
    Constant,
    Type,
}

impl SymbolKind {
    fn as_str(&self) -> &str {
        match self {
            Self::Function => "function",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Constant => "constant",
            Self::Type => "type",
        }
    }

    fn from_filter(s: &str) -> Option<Self> {
        match s {
            "function" | "fn" => Some(Self::Function),
            "struct" => Some(Self::Struct),
            "enum" => Some(Self::Enum),
            "trait" => Some(Self::Trait),
            "class" => Some(Self::Class),
            "interface" => Some(Self::Interface),
            "constant" | "const" => Some(Self::Constant),
            "type" => Some(Self::Type),
            _ => None,
        }
    }
}

/// A symbol match from the search.
#[derive(Debug, Clone)]
struct SymbolMatch {
    file: String,
    line: usize,
    kind: SymbolKind,
    signature: String,
}

// ── Extraction helpers ────────────────────────────────────────────────

/// Maximum file size to parse (256KB).
const MAX_FILE_SIZE: usize = 256 * 1024;
/// Maximum files to scan.
const MAX_FILES: usize = 10_000;

const SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "__pycache__",
    "dist",
    "build",
    ".hg",
    ".svn",
    ".next",
    "vendor",
    ".venv",
    "venv",
];

const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "jsx", "tsx", "mjs", "mts", "go",
];

fn extract_until_brace_or_semi(line: &str) -> String {
    let end = line
        .find('{')
        .or_else(|| line.find(';'))
        .unwrap_or(line.len());
    line[..end].trim().to_string()
}

fn extract_fn_until_body(line: &str) -> String {
    let mut depth = 0i32;
    let mut end = line.len();

    for (i, ch) in line.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let rest = &line[i + 1..];
                    let trimmed_rest = rest.trim();
                    if trimmed_rest.starts_with("->") {
                        if let Some(brace) = rest.find('{') {
                            end = i + 1 + brace;
                        } else if let Some(w) = rest.find(" where") {
                            end = i + 1 + w;
                        }
                    } else {
                        end = rest.find('{').map_or(i + 1, |b| i + 1 + b);
                    }
                    break;
                }
            }
            '{' if depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }

    line[..end].trim().to_string()
}

fn extract_const_sig(line: &str) -> String {
    if let Some(eq_pos) = line.find(" = ") {
        return line[..eq_pos].trim().to_string();
    }
    extract_until_brace_or_semi(line)
}

/// Extract symbols from a source file.
fn extract_symbols(content: &str, path: &str, ext: &str) -> Vec<SymbolMatch> {
    match ext {
        "rs" => extract_rust(content, path),
        "py" => extract_python(content, path),
        "js" | "jsx" | "mjs" => extract_js(content, path),
        "ts" | "tsx" | "mts" => extract_ts(content, path),
        "go" => extract_go(content, path),
        _ => Vec::new(),
    }
}

fn extract_rust(content: &str, path: &str) -> Vec<SymbolMatch> {
    let mut syms = Vec::new();
    for (ln, line) in content.lines().enumerate() {
        let t = line.trim();
        if t.starts_with("//") || t.starts_with("/*") || t.is_empty() {
            continue;
        }

        // Functions.
        if t.starts_with("pub fn ")
            || t.starts_with("pub(crate) fn ")
            || t.starts_with("pub async fn ")
            || t.starts_with("pub(crate) async fn ")
            || t.starts_with("fn ")
            || t.starts_with("async fn ")
        {
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Function,
                signature: extract_fn_until_body(t),
            });
            continue;
        }
        if t.starts_with("pub struct ") || t.starts_with("pub(crate) struct ") {
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Struct,
                signature: extract_until_brace_or_semi(t),
            });
            continue;
        }
        if t.starts_with("pub enum ") || t.starts_with("pub(crate) enum ") {
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Enum,
                signature: extract_until_brace_or_semi(t),
            });
            continue;
        }
        if t.starts_with("pub trait ") || t.starts_with("pub(crate) trait ") {
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Trait,
                signature: extract_until_brace_or_semi(t),
            });
            continue;
        }
        if t.starts_with("pub type ") || t.starts_with("pub(crate) type ") {
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Type,
                signature: extract_until_brace_or_semi(t),
            });
            continue;
        }
        if t.starts_with("pub const ") || t.starts_with("pub static ") {
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Constant,
                signature: extract_const_sig(t),
            });
        }
    }
    syms
}

fn extract_python(content: &str, path: &str) -> Vec<SymbolMatch> {
    let mut syms = Vec::new();
    for (ln, line) in content.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('#') || t.is_empty() {
            continue;
        }
        if t.starts_with("def ") {
            let sig = if let Some(close) = t.rfind(')') {
                if let Some(colon_off) = t[close..].find(':') {
                    t[..close + colon_off].trim().to_string()
                } else {
                    t[..=close].trim().to_string()
                }
            } else {
                t.trim_end_matches(':').trim().to_string()
            };
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Function,
                signature: sig,
            });
            continue;
        }
        if t.starts_with("class ") {
            let end = t.find(':').unwrap_or(t.len());
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Class,
                signature: t[..end].trim().to_string(),
            });
        }
    }
    syms
}

fn extract_js(content: &str, path: &str) -> Vec<SymbolMatch> {
    let mut syms = Vec::new();
    for (ln, line) in content.lines().enumerate() {
        let t = line.trim();
        if t.starts_with("//") || t.starts_with("/*") || t.is_empty() {
            continue;
        }
        if t.starts_with("function ")
            || t.starts_with("export function ")
            || t.starts_with("export default function ")
            || t.starts_with("async function ")
            || t.starts_with("export async function ")
        {
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Function,
                signature: extract_fn_until_body(t),
            });
            continue;
        }
        if t.starts_with("class ") || t.starts_with("export class ") {
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Class,
                signature: extract_until_brace_or_semi(t),
            });
            continue;
        }
        if (t.starts_with("export const ") || t.starts_with("export let "))
            && !t.contains("=>")
        {
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Constant,
                signature: extract_const_sig(t),
            });
        }
    }
    syms
}

fn extract_ts(content: &str, path: &str) -> Vec<SymbolMatch> {
    let mut syms = extract_js(content, path);
    for (ln, line) in content.lines().enumerate() {
        let t = line.trim();
        if t.starts_with("//") || t.starts_with("/*") || t.is_empty() {
            continue;
        }
        if t.starts_with("interface ") || t.starts_with("export interface ") {
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Interface,
                signature: extract_until_brace_or_semi(t),
            });
            continue;
        }
        if t.starts_with("type ") || t.starts_with("export type ") {
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Type,
                signature: extract_const_sig(t),
            });
        }
    }
    syms.sort_by_key(|s| s.line);
    syms
}

fn extract_go(content: &str, path: &str) -> Vec<SymbolMatch> {
    let mut syms = Vec::new();
    for (ln, line) in content.lines().enumerate() {
        let t = line.trim();
        if t.starts_with("//") || t.is_empty() {
            continue;
        }
        if t.starts_with("func ") {
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind: SymbolKind::Function,
                signature: extract_fn_until_body(t),
            });
            continue;
        }
        if t.starts_with("type ") {
            let sig = extract_until_brace_or_semi(t);
            let kind = if t.contains(" struct") {
                SymbolKind::Struct
            } else if t.contains(" interface") {
                SymbolKind::Interface
            } else {
                SymbolKind::Type
            };
            syms.push(SymbolMatch {
                file: path.into(),
                line: ln + 1,
                kind,
                signature: sig,
            });
        }
    }
    syms
}

// ── Directory scanning ────────────────────────────────────────────────

fn scan_source_files(root: &Path, max_files: usize) -> Vec<(String, PathBuf)> {
    let mut results = Vec::new();
    scan_recursive(root, root, max_files, &mut results);
    results.sort_by(|a, b| a.0.cmp(&b.0));
    results
}

fn scan_recursive(
    root: &Path,
    current: &Path,
    max_files: usize,
    results: &mut Vec<(String, PathBuf)>,
) {
    if results.len() >= max_files {
        return;
    }

    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut dirs = Vec::new();

    for entry in entries.flatten() {
        if results.len() >= max_files {
            return;
        }

        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with('.')
                && !SKIP_DIRS.contains(&name_str.as_ref())
            {
                dirs.push(path);
            }
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if SOURCE_EXTENSIONS.contains(&ext) {
                    let relative = path
                        .strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    results.push((relative, path));
                }
            }
        }
    }

    dirs.sort();
    for dir in dirs {
        scan_recursive(root, &dir, max_files, results);
    }
}

// ── Tool implementation ───────────────────────────────────────────────

/// Search for symbols across source files.
pub struct SymbolSearchTool;

impl Default for SymbolSearchTool {
    fn default() -> Self {
        Self
    }
}

impl SymbolSearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SymbolSearchTool {
    fn name(&self) -> &str {
        "symbol_search"
    }

    fn description(&self) -> &str {
        "Search for code symbols (functions, structs, classes, etc.) by name across source files."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        let query = input.arguments["query"]
            .as_str()
            .ok_or_else(|| {
                CuervoError::InvalidInput("symbol_search requires 'query' string".into())
            })?;

        let base_path = input.arguments["path"]
            .as_str()
            .unwrap_or(&input.working_directory);

        let kind_filter = input.arguments["kind"]
            .as_str()
            .and_then(SymbolKind::from_filter);

        let base = PathBuf::from(base_path);
        if !base.is_dir() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!(
                    "symbol_search error: '{}' is not a directory",
                    base.display()
                ),
                is_error: true,
                metadata: None,
            });
        }

        let query_owned = query.to_string();
        let matches = tokio::task::spawn_blocking(move || {
            let files = scan_source_files(&base, MAX_FILES);
            let query_lower = query_owned.to_lowercase();
            let mut matches: Vec<SymbolMatch> = Vec::new();

            for (rel_path, abs_path) in &files {
                let content = match std::fs::read_to_string(abs_path) {
                    Ok(c) if c.len() <= MAX_FILE_SIZE => c,
                    _ => continue,
                };

                let ext = rel_path.rsplit('.').next().unwrap_or("");
                let syms = extract_symbols(&content, rel_path, ext);

                for sym in syms {
                    // Case-insensitive substring match on signature.
                    if sym.signature.to_lowercase().contains(&query_lower) {
                        if let Some(filter) = kind_filter {
                            if sym.kind != filter {
                                continue;
                            }
                        }
                        matches.push(sym);
                    }
                }

                // Cap results to avoid huge output.
                if matches.len() >= 100 {
                    break;
                }
            }

            matches
        })
        .await
        .map_err(|e| CuervoError::ToolExecutionFailed {
            tool: "symbol_search".into(),
            message: format!("search task failed: {e}"),
        })?;

        if matches.is_empty() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("No symbols matching '{query}' found."),
                is_error: false,
                metadata: Some(json!({ "match_count": 0 })),
            });
        }

        let match_count = matches.len();
        let mut content = format!("Found {match_count} symbol(s) matching '{query}':\n");
        let symbols_json: Vec<serde_json::Value> = matches
            .iter()
            .map(|m| {
                json!({
                    "name": extract_name_from_sig(&m.signature),
                    "kind": m.kind.as_str(),
                    "file": m.file,
                    "line": m.line,
                    "signature": m.signature,
                })
            })
            .collect();

        for m in &matches {
            content.push_str(&format!(
                "  {}:{} {} {}\n",
                m.file,
                m.line,
                m.kind.as_str(),
                m.signature
            ));
        }

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "match_count": match_count,
                "symbols": symbols_json,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Symbol name or partial match to search for."
                },
                "kind": {
                    "type": "string",
                    "description": "Optional filter: function|struct|enum|trait|class|interface|constant|type."
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: working directory)."
                }
            },
            "required": ["query"]
        })
    }
}

/// Extract the symbol name from a signature (first identifier after keyword).
fn extract_name_from_sig(sig: &str) -> String {
    // Skip keywords to find the name.
    let keywords = [
        "pub(crate) async fn ",
        "pub async fn ",
        "pub(crate) fn ",
        "pub fn ",
        "async fn ",
        "fn ",
        "pub(crate) struct ",
        "pub struct ",
        "pub(crate) enum ",
        "pub enum ",
        "pub(crate) trait ",
        "pub trait ",
        "pub(crate) type ",
        "pub type ",
        "export type ",
        "export interface ",
        "export async function ",
        "export default function ",
        "export function ",
        "async function ",
        "function ",
        "export class ",
        "export const ",
        "export let ",
        "pub const ",
        "pub static ",
        "class ",
        "def ",
        "type ",
        "func ",
        "interface ",
    ];

    let trimmed = sig.trim();
    for kw in &keywords {
        if let Some(rest) = trimmed.strip_prefix(kw) {
            // Name is the first word (up to '(' or '<' or ' ' or end).
            let name_end = rest
                .find(['(', '<', ' ', ':'])
                .unwrap_or(rest.len());
            return rest[..name_end].to_string();
        }
    }

    // Fallback: first word.
    trimmed
        .split_whitespace()
        .last()
        .unwrap_or(trimmed)
        .split('(')
        .next()
        .unwrap_or(trimmed)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(args: serde_json::Value) -> ToolInput {
        ToolInput {
            tool_use_id: "test".to_string(),
            arguments: args,
            working_directory: "/tmp".to_string(),
        }
    }

    /// Create a temp directory with multi-language source files.
    fn setup_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();

        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::create_dir_all(base.join("lib")).unwrap();

        // Rust file.
        std::fs::write(
            base.join("src/main.rs"),
            r#"pub fn compute_fingerprint(messages: &[Message]) -> String {
    "hash".to_string()
}

pub struct AgentConfig {
    pub model: String,
}

pub enum Status {
    Running,
    Stopped,
}

pub trait Processor {
    fn process(&self);
}

pub const MAX_RETRIES: u32 = 5;
"#,
        )
        .unwrap();

        // Python file.
        std::fs::write(
            base.join("lib/utils.py"),
            r#"def compute_hash(data: bytes) -> str:
    return "hash"

class DataProcessor:
    def process(self, data):
        pass
"#,
        )
        .unwrap();

        // JavaScript file.
        std::fs::write(
            base.join("lib/helpers.js"),
            r#"function formatDate(date) {
    return date.toISOString();
}

export function parseJSON(text) {
    return JSON.parse(text);
}

class EventEmitter {
    emit(event) {}
}
"#,
        )
        .unwrap();

        // TypeScript file.
        std::fs::write(
            base.join("src/types.ts"),
            r#"export interface Config {
    model: string;
}

export type StatusCode = number;

export function createConfig(model: string): Config {
    return { model };
}
"#,
        )
        .unwrap();

        // Go file.
        std::fs::write(
            base.join("src/main.go"),
            r#"package main

func ComputeHash(data []byte) string {
    return "hash"
}

type Server struct {
    Port int
}

type Handler interface {
    Handle()
}
"#,
        )
        .unwrap();

        dir
    }

    #[tokio::test]
    async fn rust_function_search() {
        let dir = setup_project();
        let tool = SymbolSearchTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "compute_fingerprint"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("compute_fingerprint"));
        assert!(out.content.contains("main.rs"));
        let meta = out.metadata.unwrap();
        assert!(meta["match_count"].as_u64().unwrap() >= 1);
    }

    #[tokio::test]
    async fn python_class_search() {
        let dir = setup_project();
        let tool = SymbolSearchTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "DataProcessor"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("DataProcessor"));
        assert!(out.content.contains("utils.py"));
    }

    #[tokio::test]
    async fn js_function_search() {
        let dir = setup_project();
        let tool = SymbolSearchTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "formatDate"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("formatDate"));
        assert!(out.content.contains("helpers.js"));
    }

    #[tokio::test]
    async fn ts_interface_search() {
        let dir = setup_project();
        let tool = SymbolSearchTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "Config", "kind": "interface"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("interface"));
        assert!(out.content.contains("types.ts"));
    }

    #[tokio::test]
    async fn go_struct_search() {
        let dir = setup_project();
        let tool = SymbolSearchTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "Server"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("Server"));
        assert!(out.content.contains("main.go"));
    }

    #[tokio::test]
    async fn filter_by_kind() {
        let dir = setup_project();
        let tool = SymbolSearchTool::new();
        // "compute" matches both Rust fn and Python fn and Go fn.
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "compute", "kind": "struct"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        // No structs named "compute".
        assert!(out.content.contains("No symbols matching"));
    }

    #[tokio::test]
    async fn no_match() {
        let dir = setup_project();
        let tool = SymbolSearchTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "nonexistent_symbol_xyz"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("No symbols matching"));
    }

    #[tokio::test]
    async fn case_insensitive() {
        let dir = setup_project();
        let tool = SymbolSearchTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "AGENTCONFIG"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("AgentConfig"));
    }

    #[tokio::test]
    async fn directory_scope() {
        let dir = setup_project();
        let tool = SymbolSearchTool::new();
        // Search only in lib/ — should find Python/JS but not Rust in src/.
        let out = tool
            .execute(make_input(json!({
                "query": "compute",
                "path": dir.path().join("lib").to_str().unwrap()
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        // Should find compute_hash in utils.py.
        assert!(out.content.contains("compute_hash"));
        // Should NOT find compute_fingerprint from src/main.rs.
        assert!(!out.content.contains("compute_fingerprint"));
    }

    #[tokio::test]
    async fn multiple_matches() {
        let dir = setup_project();
        let tool = SymbolSearchTool::new();
        // "process" appears in Rust (Processor trait) and Python (process method).
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "process"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        let meta = out.metadata.unwrap();
        assert!(meta["match_count"].as_u64().unwrap() >= 2);
    }

    #[tokio::test]
    async fn metadata_has_symbols() {
        let dir = setup_project();
        let tool = SymbolSearchTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "AgentConfig"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        let meta = out.metadata.unwrap();
        let symbols = meta["symbols"].as_array().unwrap();
        assert!(!symbols.is_empty());
        let first = &symbols[0];
        assert_eq!(first["kind"], "struct");
        assert!(first["signature"].as_str().unwrap().contains("AgentConfig"));
        assert!(first["line"].as_u64().unwrap() > 0);
    }

    #[test]
    fn schema_is_valid() {
        let tool = SymbolSearchTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "query"));
    }

    #[test]
    fn permission_is_readonly() {
        let tool = SymbolSearchTool::new();
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
    }

    #[tokio::test]
    async fn empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let tool = SymbolSearchTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t".into(),
                arguments: json!({"query": "anything"}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("No symbols matching"));
    }

    #[test]
    fn extract_name_basics() {
        assert_eq!(
            extract_name_from_sig("pub fn compute_fingerprint(messages: &[Message]) -> String"),
            "compute_fingerprint"
        );
        assert_eq!(
            extract_name_from_sig("pub struct AgentConfig"),
            "AgentConfig"
        );
        assert_eq!(
            extract_name_from_sig("def compute_hash(data: bytes) -> str"),
            "compute_hash"
        );
        assert_eq!(
            extract_name_from_sig("class DataProcessor"),
            "DataProcessor"
        );
        assert_eq!(
            extract_name_from_sig("func ComputeHash(data []byte) string"),
            "ComputeHash"
        );
    }
}
