//! Repo map: lightweight codebase structure extraction.
//!
//! Scans source files, extracts function/type/trait signatures using regex,
//! and produces a compact "map" of the codebase for context injection.
//! Inspired by Aider's repo map (tree-sitter + PageRank), but uses
//! regex heuristics for zero-dependency extraction.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::assembler::estimate_tokens;

/// A symbol extracted from source code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub file_path: String,
    pub line: usize,
    pub kind: SymbolKind,
    pub signature: String,
    pub indent_level: u32,
}

/// The kind of symbol extracted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Class,
    Interface,
    Constant,
    Type,
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Function => write!(f, "fn"),
            Self::Struct => write!(f, "struct"),
            Self::Enum => write!(f, "enum"),
            Self::Trait => write!(f, "trait"),
            Self::Impl => write!(f, "impl"),
            Self::Module => write!(f, "mod"),
            Self::Class => write!(f, "class"),
            Self::Interface => write!(f, "interface"),
            Self::Constant => write!(f, "const"),
            Self::Type => write!(f, "type"),
        }
    }
}

/// A file with its extracted symbols.
#[derive(Debug, Clone)]
pub struct FileSymbols {
    pub path: String,
    pub symbols: Vec<Symbol>,
    pub token_estimate: usize,
}

/// The repo map: a collection of file symbol summaries.
#[derive(Debug, Clone)]
pub struct RepoMap {
    files: Vec<FileSymbols>,
    root: String,
    total_tokens: usize,
}

impl RepoMap {
    /// Build a repo map from a list of (relative_path, content) pairs.
    pub fn build(root: &str, file_contents: &[(&str, &str)]) -> Self {
        let mut files = Vec::with_capacity(file_contents.len());
        let mut total_tokens = 0;

        for &(path, content) in file_contents {
            let ext = path.rsplit('.').next().unwrap_or("");
            let symbols = extract_symbols(content, path, ext);
            if symbols.is_empty() {
                continue;
            }

            let formatted = format_file_symbols(path, &symbols);
            let tokens = estimate_tokens(&formatted);
            total_tokens += tokens;

            files.push(FileSymbols {
                path: path.to_string(),
                symbols,
                token_estimate: tokens,
            });
        }

        // Sort by path for deterministic output.
        files.sort_by(|a, b| a.path.cmp(&b.path));

        Self {
            files,
            root: root.to_string(),
            total_tokens,
        }
    }

    /// Render the repo map as a compact string within a token budget.
    ///
    /// Returns only the files that fit within the budget, prioritizing
    /// files with more symbols (more structurally important).
    pub fn render(&self, token_budget: usize) -> String {
        if self.files.is_empty() {
            return String::new();
        }

        // Sort files by symbol count descending (most important first).
        let mut ranked: Vec<&FileSymbols> = self.files.iter().collect();
        ranked.sort_by(|a, b| b.symbols.len().cmp(&a.symbols.len()));

        let mut output = String::from("[Repository Map]\n");
        let mut budget_remaining = token_budget.saturating_sub(estimate_tokens("[Repository Map]\n"));

        for file in ranked {
            let entry = format_file_symbols(&file.path, &file.symbols);
            let entry_tokens = estimate_tokens(&entry);
            if entry_tokens > budget_remaining {
                // Try to fit with truncated symbols.
                let truncated = format_file_header(&file.path, file.symbols.len());
                let trunc_tokens = estimate_tokens(&truncated);
                if trunc_tokens <= budget_remaining {
                    output.push_str(&truncated);
                    budget_remaining -= trunc_tokens;
                }
                continue;
            }
            output.push_str(&entry);
            budget_remaining -= entry_tokens;
        }

        output
    }

    /// Total number of files in the map.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Total number of symbols across all files.
    pub fn symbol_count(&self) -> usize {
        self.files.iter().map(|f| f.symbols.len()).sum()
    }

    /// Estimated total tokens for the full map.
    pub fn total_tokens(&self) -> usize {
        self.total_tokens
    }

    /// Root directory of the scanned codebase.
    pub fn root(&self) -> &str {
        &self.root
    }

    /// Get symbols for a specific file.
    pub fn file_symbols(&self, path: &str) -> Option<&FileSymbols> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Search for symbols matching a query string.
    pub fn search(&self, query: &str) -> Vec<&Symbol> {
        let query_lower = query.to_lowercase();
        self.files
            .iter()
            .flat_map(|f| &f.symbols)
            .filter(|s| s.signature.to_lowercase().contains(&query_lower))
            .collect()
    }
}

/// Scan a directory recursively for source files.
///
/// Returns (relative_path, absolute_path) pairs.
pub fn scan_source_files(root: &Path, max_files: usize) -> Vec<(String, PathBuf)> {
    let extensions = [
        "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "c", "cpp", "h", "hpp",
        "swift", "kt", "rb", "lua", "zig",
    ];
    let skip_dirs = [
        "target",
        "node_modules",
        ".git",
        "dist",
        "build",
        "__pycache__",
        ".tox",
        "vendor",
        ".venv",
        "venv",
    ];

    let mut results = Vec::new();
    scan_recursive(root, root, &extensions, &skip_dirs, max_files, &mut results);
    results.sort_by(|a, b| a.0.cmp(&b.0));
    results
}

fn scan_recursive(
    root: &Path,
    current: &Path,
    extensions: &[&str],
    skip_dirs: &[&str],
    max_files: usize,
    results: &mut Vec<(String, PathBuf)>,
) {
    if results.len() >= max_files {
        return;
    }

    let entries = match std::fs::read_dir(current) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !skip_dirs.iter().any(|&d| d == name_str.as_ref()) {
                dirs.push(path);
            }
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if extensions.contains(&ext) {
                    let relative = path
                        .strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    files.push((relative, path));
                }
            }
        }
    }

    // Add files first.
    for f in files {
        if results.len() >= max_files {
            return;
        }
        results.push(f);
    }

    // Then recurse into directories.
    dirs.sort();
    for dir in dirs {
        scan_recursive(root, &dir, extensions, skip_dirs, max_files, results);
    }
}

/// Extract symbols from source code based on file extension.
pub fn extract_symbols(content: &str, path: &str, extension: &str) -> Vec<Symbol> {
    match extension {
        "rs" => extract_rust_symbols(content, path),
        "py" => extract_python_symbols(content, path),
        "js" | "jsx" | "mjs" => extract_js_symbols(content, path),
        "ts" | "tsx" | "mts" => extract_ts_symbols(content, path),
        "go" => extract_go_symbols(content, path),
        _ => Vec::new(),
    }
}

/// Extract Rust symbols: pub fn, pub struct, pub enum, pub trait, impl, mod.
fn extract_rust_symbols(content: &str, path: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        let indent_level = (indent / 4) as u32;

        // Skip comments and empty lines.
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.is_empty() {
            continue;
        }

        // pub fn / fn (top-level or in impl blocks).
        if let Some(sig) = extract_rust_fn_signature(trimmed) {
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Function,
                signature: sig,
                indent_level,
            });
            continue;
        }

        // pub struct
        if trimmed.starts_with("pub struct ") || trimmed.starts_with("pub(crate) struct ") {
            let sig = extract_until_brace_or_semi(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Struct,
                signature: sig,
                indent_level,
            });
            continue;
        }

        // pub enum
        if trimmed.starts_with("pub enum ") || trimmed.starts_with("pub(crate) enum ") {
            let sig = extract_until_brace_or_semi(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Enum,
                signature: sig,
                indent_level,
            });
            continue;
        }

        // pub trait
        if trimmed.starts_with("pub trait ") || trimmed.starts_with("pub(crate) trait ") {
            let sig = extract_until_brace_or_semi(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Trait,
                signature: sig,
                indent_level,
            });
            continue;
        }

        // impl blocks (with or without trait).
        if trimmed.starts_with("impl ") || trimmed.starts_with("impl<") {
            let sig = extract_until_brace_or_semi(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Impl,
                signature: sig,
                indent_level,
            });
            continue;
        }

        // pub mod
        if trimmed.starts_with("pub mod ") || trimmed.starts_with("mod ") {
            let sig = extract_until_brace_or_semi(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Module,
                signature: sig,
                indent_level,
            });
            continue;
        }

        // pub type
        if trimmed.starts_with("pub type ") || trimmed.starts_with("pub(crate) type ") {
            let sig = extract_until_brace_or_semi(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Type,
                signature: sig,
                indent_level,
            });
            continue;
        }

        // pub const / pub static
        if trimmed.starts_with("pub const ") || trimmed.starts_with("pub static ") {
            let sig = extract_const_signature(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Constant,
                signature: sig,
                indent_level,
            });
        }
    }

    symbols
}

fn extract_rust_fn_signature(line: &str) -> Option<String> {
    // Match: pub fn, pub(crate) fn, pub async fn, async fn, fn (only at indent 0-1)
    let is_pub_fn = line.starts_with("pub fn ")
        || line.starts_with("pub(crate) fn ")
        || line.starts_with("pub async fn ")
        || line.starts_with("pub(crate) async fn ");
    let is_fn = line.starts_with("fn ") || line.starts_with("async fn ");

    if !is_pub_fn && !is_fn {
        return None;
    }

    // Extract up to the opening brace or the "where" clause.
    let sig = extract_fn_until_body(line);
    Some(sig)
}

fn extract_fn_until_body(line: &str) -> String {
    // Find the closing paren of the parameter list, then include return type.
    let mut depth = 0i32;
    let mut end = line.len();

    for (i, ch) in line.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    // After closing paren, include return type if present.
                    let rest = &line[i + 1..];
                    let trimmed_rest = rest.trim();
                    if trimmed_rest.starts_with("->") {
                        // Find the return type (up to '{' or 'where').
                        if let Some(brace) = rest.find('{') {
                            end = i + 1 + brace;
                        } else if let Some(w) = rest.find(" where") {
                            end = i + 1 + w;
                        }
                    } else {
                        // No return type — stop at '{' or end.
                        if let Some(brace) = rest.find('{') {
                            end = i + 1 + brace;
                        } else {
                            end = i + 1;
                        }
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

fn extract_until_brace_or_semi(line: &str) -> String {
    let end = line
        .find('{')
        .or_else(|| line.find(';'))
        .unwrap_or(line.len());
    line[..end].trim().to_string()
}

fn extract_const_signature(line: &str) -> String {
    // pub const NAME: Type = ...;  →  pub const NAME: Type
    if let Some(eq_pos) = line.find(" = ") {
        return line[..eq_pos].trim().to_string();
    }
    extract_until_brace_or_semi(line)
}

/// Extract Python symbols: def, class.
fn extract_python_symbols(content: &str, path: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        let indent_level = (indent / 4) as u32;

        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with("def ") {
            let sig = extract_python_def(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Function,
                signature: sig,
                indent_level,
            });
            continue;
        }

        if trimmed.starts_with("class ") {
            let sig = extract_until_colon(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Class,
                signature: sig,
                indent_level,
            });
        }
    }

    symbols
}

fn extract_python_def(line: &str) -> String {
    // def name(...) -> ReturnType:
    // Find closing paren first to avoid matching `:` inside parameter list.
    if let Some(close_paren) = line.rfind(')') {
        if let Some(colon_offset) = line[close_paren..].find(':') {
            return line[..close_paren + colon_offset].trim().to_string();
        }
        return line[..=close_paren].trim().to_string();
    }
    // Fallback: last colon.
    if let Some(colon) = line.rfind(':') {
        return line[..colon].trim().to_string();
    }
    line.trim().to_string()
}

fn extract_until_colon(line: &str) -> String {
    let end = line.find(':').unwrap_or(line.len());
    line[..end].trim().to_string()
}

/// Extract JavaScript symbols: function, class, const/let exports.
fn extract_js_symbols(content: &str, path: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        let indent_level = (indent / 4) as u32;

        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.is_empty() {
            continue;
        }

        // function declarations or exports.
        if trimmed.starts_with("function ")
            || trimmed.starts_with("export function ")
            || trimmed.starts_with("export default function ")
            || trimmed.starts_with("async function ")
            || trimmed.starts_with("export async function ")
        {
            let sig = extract_fn_until_body(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Function,
                signature: sig,
                indent_level,
            });
            continue;
        }

        if trimmed.starts_with("class ") || trimmed.starts_with("export class ") {
            let sig = extract_until_brace_or_semi(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Class,
                signature: sig,
                indent_level,
            });
            continue;
        }

        // export const/let.
        if (trimmed.starts_with("export const ") || trimmed.starts_with("export let "))
            && !trimmed.contains("=>")
        {
            let sig = extract_const_signature(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Constant,
                signature: sig,
                indent_level,
            });
        }
    }

    symbols
}

/// Extract TypeScript symbols: includes interfaces and type aliases.
fn extract_ts_symbols(content: &str, path: &str) -> Vec<Symbol> {
    let mut symbols = extract_js_symbols(content, path);

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        let indent_level = (indent / 4) as u32;

        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with("interface ") || trimmed.starts_with("export interface ") {
            let sig = extract_until_brace_or_semi(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Interface,
                signature: sig,
                indent_level,
            });
            continue;
        }

        if trimmed.starts_with("type ") || trimmed.starts_with("export type ") {
            let sig = extract_const_signature(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Type,
                signature: sig,
                indent_level,
            });
        }
    }

    // Sort by line number since TS extraction does two passes.
    symbols.sort_by_key(|s| s.line);
    symbols
}

/// Extract Go symbols: func, type, const.
fn extract_go_symbols(content: &str, path: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        let indent_level = (indent / 4) as u32;

        if trimmed.starts_with("//") || trimmed.is_empty() {
            continue;
        }

        // func (exported = starts with uppercase after `func `).
        if trimmed.starts_with("func ") {
            let sig = extract_fn_until_body(trimmed);
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind: SymbolKind::Function,
                signature: sig,
                indent_level,
            });
            continue;
        }

        // type declarations.
        if trimmed.starts_with("type ") {
            let sig = extract_until_brace_or_semi(trimmed);
            let kind = if trimmed.contains(" struct") {
                SymbolKind::Struct
            } else if trimmed.contains(" interface") {
                SymbolKind::Interface
            } else {
                SymbolKind::Type
            };
            symbols.push(Symbol {
                file_path: path.to_string(),
                line: line_num + 1,
                kind,
                signature: sig,
                indent_level,
            });
        }
    }

    symbols
}

/// Format a file's symbols into a compact string.
fn format_file_symbols(path: &str, symbols: &[Symbol]) -> String {
    let mut out = format!("## {}\n", path);
    for sym in symbols {
        let indent = "  ".repeat(sym.indent_level as usize);
        out.push_str(&format!("{}  {} {}\n", indent, sym.kind, sym.signature));
    }
    out.push('\n');
    out
}

/// Format just a file header when symbols don't fit.
fn format_file_header(path: &str, symbol_count: usize) -> String {
    format!("## {} ({} symbols)\n\n", path, symbol_count)
}

/// Helper: build a repo map by scanning and reading files.
///
/// This is the convenience entry point for integration with the context pipeline.
pub fn build_repo_map(root: &Path, max_files: usize, token_budget: usize) -> RepoMap {
    let files = scan_source_files(root, max_files);
    let mut contents: Vec<(String, String)> = Vec::new();

    for (rel_path, abs_path) in &files {
        match std::fs::read_to_string(abs_path) {
            Ok(content) => {
                contents.push((rel_path.clone(), content));
            }
            Err(_) => continue,
        }
    }

    let refs: Vec<(&str, &str)> = contents
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();

    let map = RepoMap::build(root.to_str().unwrap_or("."), &refs);

    // Pre-check: if total tokens exceed budget, the render method handles truncation.
    let _ = token_budget;

    map
}

/// Compute a reference count for symbols (how many times each symbol name appears
/// across all files). Higher count = more interconnected = more important.
pub fn rank_symbols(files: &[FileSymbols]) -> HashMap<String, u32> {
    let mut name_counts: HashMap<String, u32> = HashMap::new();

    // First pass: collect all symbol names.
    let mut all_names: Vec<String> = Vec::new();
    for file in files {
        for sym in &file.symbols {
            // Extract the symbol name (first word after kind keyword).
            if let Some(name) = extract_symbol_name(&sym.signature) {
                all_names.push(name);
            }
        }
    }

    // Second pass: count references.
    // A symbol that appears as a name and also appears in other signatures is more connected.
    for file in files {
        for sym in &file.symbols {
            for name in &all_names {
                if sym.signature.contains(name.as_str()) {
                    *name_counts.entry(name.clone()).or_insert(0) += 1;
                }
            }
        }
    }

    name_counts
}

fn extract_symbol_name(signature: &str) -> Option<String> {
    // For "pub fn name(..." → "name"
    // For "pub struct Name" → "Name"
    // For "impl Foo for Bar" → "Bar" (target type, not trait)
    let words: Vec<&str> = signature.split_whitespace().collect();

    // Special case: impl ... for Target — return the target type.
    if words.contains(&"impl") {
        // Look for "for" keyword — return the word after it.
        for (i, word) in words.iter().enumerate() {
            if *word == "for" && i + 1 < words.len() {
                let name = words[i + 1]
                    .split(['<', '{'])
                    .next()
                    .unwrap_or(words[i + 1]);
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
        // No "for" found — it's `impl Type { ... }`, return the type after impl.
        for word in &words {
            if *word != "impl" && !word.starts_with('<') {
                let name = word
                    .split(['<', '{'])
                    .next()
                    .unwrap_or(word);
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
        return None;
    }

    // General case: skip keywords to find the name.
    for word in &words {
        match *word {
            "pub" | "pub(crate)" | "async" | "fn" | "struct" | "enum" | "trait" | "mod"
            | "type" | "const" | "static" | "let" | "class" | "function" | "interface"
            | "export" | "default" | "for" => continue,
            _ => {
                let name = word
                    .split(['<', '(', ':', '{'])
                    .next()
                    .unwrap_or(word);
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Rust extraction ---

    #[test]
    fn extract_rust_pub_fn() {
        let code = "pub fn hello(name: &str) -> String {\n    format!(\"hello {}\", name)\n}\n";
        let symbols = extract_symbols(code, "lib.rs", "rs");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert!(symbols[0].signature.contains("pub fn hello(name: &str) -> String"));
    }

    #[test]
    fn extract_rust_struct() {
        let code = "pub struct Config {\n    pub name: String,\n}\n";
        let symbols = extract_symbols(code, "config.rs", "rs");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Struct);
        assert!(symbols[0].signature.contains("pub struct Config"));
    }

    #[test]
    fn extract_rust_enum() {
        let code = "pub enum Color {\n    Red,\n    Blue,\n}\n";
        let symbols = extract_symbols(code, "color.rs", "rs");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Enum);
    }

    #[test]
    fn extract_rust_trait() {
        let code = "pub trait Drawable {\n    fn draw(&self);\n}\n";
        let symbols = extract_symbols(code, "draw.rs", "rs");
        assert!(symbols.len() >= 1);
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Trait));
    }

    #[test]
    fn extract_rust_impl() {
        let code = "impl Tool for BashTool {\n    fn name(&self) -> &str { \"bash\" }\n}\n";
        let symbols = extract_symbols(code, "bash.rs", "rs");
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Impl));
        assert!(symbols[0].signature.contains("impl Tool for BashTool"));
    }

    #[test]
    fn extract_rust_mod() {
        let code = "pub mod assembler;\npub mod pipeline;\n";
        let symbols = extract_symbols(code, "lib.rs", "rs");
        assert_eq!(symbols.len(), 2);
        assert!(symbols.iter().all(|s| s.kind == SymbolKind::Module));
    }

    #[test]
    fn extract_rust_const() {
        let code = "pub const MAX_RETRIES: u32 = 3;\n";
        let symbols = extract_symbols(code, "config.rs", "rs");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Constant);
        assert!(symbols[0].signature.contains("pub const MAX_RETRIES: u32"));
        assert!(!symbols[0].signature.contains(" = "));
    }

    #[test]
    fn extract_rust_async_fn() {
        let code = "pub async fn fetch_data(url: &str) -> Result<String> {\n    Ok(\"data\".into())\n}\n";
        let symbols = extract_symbols(code, "fetch.rs", "rs");
        assert_eq!(symbols.len(), 1);
        assert!(symbols[0].signature.contains("pub async fn fetch_data"));
    }

    #[test]
    fn extract_rust_skips_comments() {
        let code = "// pub fn commented_out() {}\n/* pub struct Ignored {} */\npub fn real() {}\n";
        let symbols = extract_symbols(code, "lib.rs", "rs");
        assert_eq!(symbols.len(), 1);
        assert!(symbols[0].signature.contains("pub fn real()"));
    }

    // --- Python extraction ---

    #[test]
    fn extract_python_function() {
        let code = "def hello(name: str) -> str:\n    return f\"hello {name}\"\n";
        let symbols = extract_symbols(code, "main.py", "py");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert!(symbols[0].signature.contains("def hello(name: str) -> str"));
    }

    #[test]
    fn extract_python_class() {
        let code = "class Config(BaseModel):\n    name: str = \"\"\n";
        let symbols = extract_symbols(code, "config.py", "py");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Class);
        assert!(symbols[0].signature.contains("class Config(BaseModel)"));
    }

    #[test]
    fn extract_python_class_methods() {
        let code = "class Foo:\n    def bar(self):\n        pass\n    def baz(self, x: int):\n        pass\n";
        let symbols = extract_symbols(code, "foo.py", "py");
        assert_eq!(symbols.len(), 3); // class + 2 methods
        assert_eq!(symbols[0].kind, SymbolKind::Class);
        assert_eq!(symbols[1].kind, SymbolKind::Function);
        assert_eq!(symbols[2].kind, SymbolKind::Function);
    }

    // --- JavaScript extraction ---

    #[test]
    fn extract_js_function() {
        let code = "function hello(name) {\n    return `hello ${name}`;\n}\n";
        let symbols = extract_symbols(code, "app.js", "js");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn extract_js_export_function() {
        let code = "export function fetchData(url) {\n    return fetch(url);\n}\n";
        let symbols = extract_symbols(code, "api.js", "js");
        assert_eq!(symbols.len(), 1);
        assert!(symbols[0].signature.contains("export function fetchData"));
    }

    #[test]
    fn extract_js_class() {
        let code = "export class UserService {\n    constructor() {}\n}\n";
        let symbols = extract_symbols(code, "user.js", "js");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Class);
    }

    // --- TypeScript extraction ---

    #[test]
    fn extract_ts_interface() {
        let code = "export interface UserConfig {\n    name: string;\n    age: number;\n}\n";
        let symbols = extract_symbols(code, "types.ts", "ts");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Interface);
        assert!(symbols[0].signature.contains("export interface UserConfig"));
    }

    #[test]
    fn extract_ts_type_alias() {
        let code = "export type Result<T> = Success<T> | Error;\n";
        let symbols = extract_symbols(code, "types.ts", "ts");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Type);
    }

    // --- Go extraction ---

    #[test]
    fn extract_go_func() {
        let code = "func NewServer(addr string) *Server {\n    return &Server{addr: addr}\n}\n";
        let symbols = extract_symbols(code, "server.go", "go");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Function);
    }

    #[test]
    fn extract_go_type_struct() {
        let code = "type Server struct {\n    addr string\n}\n";
        let symbols = extract_symbols(code, "server.go", "go");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Struct);
    }

    #[test]
    fn extract_go_type_interface() {
        let code = "type Handler interface {\n    Handle(req *Request) error\n}\n";
        let symbols = extract_symbols(code, "handler.go", "go");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, SymbolKind::Interface);
    }

    // --- RepoMap ---

    #[test]
    fn repo_map_build_and_render() {
        let files = vec![
            (
                "src/lib.rs",
                "pub mod config;\npub fn version() -> &'static str { \"1.0\" }\n",
            ),
            (
                "src/config.rs",
                "pub struct Config {\n    pub name: String,\n}\n\npub fn default_config() -> Config {\n    Config { name: \"test\".into() }\n}\n",
            ),
        ];
        let map = RepoMap::build("/project", &files);
        assert_eq!(map.file_count(), 2);
        assert!(map.symbol_count() >= 4); // mod, fn, struct, fn

        let rendered = map.render(10_000);
        assert!(rendered.contains("[Repository Map]"));
        assert!(rendered.contains("src/lib.rs"));
        assert!(rendered.contains("src/config.rs"));
        assert!(rendered.contains("pub fn version()"));
    }

    #[test]
    fn repo_map_budget_truncation() {
        let files = vec![
            ("src/a.rs", "pub fn a() {}\npub fn b() {}\npub fn c() {}\n"),
            ("src/b.rs", "pub fn x() {}\n"),
        ];
        let map = RepoMap::build("/project", &files);

        // Very small budget — should truncate.
        let rendered = map.render(20);
        // Should have at least the header.
        assert!(rendered.contains("[Repository Map]"));
    }

    #[test]
    fn repo_map_search() {
        let files = vec![
            ("src/lib.rs", "pub fn hello() {}\npub fn world() {}\n"),
            ("src/config.rs", "pub struct Config {}\npub fn hello_config() {}\n"),
        ];
        let map = RepoMap::build("/project", &files);

        let results = map.search("hello");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|s| s.signature.contains("hello")));
    }

    #[test]
    fn repo_map_empty_files() {
        let files: Vec<(&str, &str)> = vec![("src/empty.rs", ""), ("src/comments.rs", "// just comments\n")];
        let map = RepoMap::build("/project", &files);
        assert_eq!(map.file_count(), 0);
        assert_eq!(map.symbol_count(), 0);
    }

    // --- Symbol name extraction ---

    #[test]
    fn extract_name_from_fn() {
        assert_eq!(
            extract_symbol_name("pub fn hello(name: &str)"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn extract_name_from_struct() {
        assert_eq!(
            extract_symbol_name("pub struct Config"),
            Some("Config".to_string())
        );
    }

    #[test]
    fn extract_name_from_impl_for() {
        assert_eq!(
            extract_symbol_name("impl Tool for BashTool"),
            Some("BashTool".to_string())
        );
    }

    // --- Ranking ---

    #[test]
    fn rank_symbols_counts_references() {
        let files = vec![
            FileSymbols {
                path: "a.rs".to_string(),
                symbols: vec![
                    Symbol {
                        file_path: "a.rs".to_string(),
                        line: 1,
                        kind: SymbolKind::Struct,
                        signature: "pub struct Config".to_string(),
                        indent_level: 0,
                    },
                ],
                token_estimate: 10,
            },
            FileSymbols {
                path: "b.rs".to_string(),
                symbols: vec![
                    Symbol {
                        file_path: "b.rs".to_string(),
                        line: 1,
                        kind: SymbolKind::Function,
                        signature: "pub fn load_config() -> Config".to_string(),
                        indent_level: 0,
                    },
                ],
                token_estimate: 10,
            },
        ];

        let ranks = rank_symbols(&files);
        // "Config" appears in both signatures, so it should have count > 1.
        assert!(ranks.get("Config").copied().unwrap_or(0) > 1);
    }

    // --- File scanning ---

    #[test]
    fn scan_source_files_respects_max() {
        // Scan the actual cuervo workspace (limited to 5 files).
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let files = scan_source_files(root, 5);
        assert!(files.len() <= 5);
    }

    #[test]
    fn scan_source_files_skips_target() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let files = scan_source_files(root, 1000);
        assert!(files.iter().all(|(p, _)| !p.contains("target/")));
    }

    // --- Format helpers ---

    #[test]
    fn format_file_header_correct() {
        let header = format_file_header("src/main.rs", 5);
        assert_eq!(header, "## src/main.rs (5 symbols)\n\n");
    }

    #[test]
    fn format_file_symbols_correct() {
        let symbols = vec![Symbol {
            file_path: "lib.rs".to_string(),
            line: 1,
            kind: SymbolKind::Function,
            signature: "pub fn hello()".to_string(),
            indent_level: 0,
        }];
        let formatted = format_file_symbols("lib.rs", &symbols);
        assert!(formatted.contains("## lib.rs"));
        assert!(formatted.contains("fn pub fn hello()"));
    }
}
