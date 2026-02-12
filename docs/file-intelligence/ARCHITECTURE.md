# File Intelligence — Architecture Design (Phase 3)

> Generated: 2026-02-09

## Design Principles

1. **Extend, don't replace**: Enhance existing `file_read` tool, don't break it
2. **Feature-gated**: Heavy deps behind Cargo features so minimal builds stay small
3. **Async-first**: All handlers async-safe (sync crates wrapped in `spawn_blocking`)
4. **Token-aware**: Every handler estimates output tokens before generating full text
5. **Streaming where possible**: Row-by-row for CSV, pull-based for XML/Markdown
6. **Fail-safe**: Binary detection prevents garbled output; size checks prevent OOM

## New Crate: `crates/cuervo-files/`

```
cuervo-files/
├── Cargo.toml
└── src/
    ├── lib.rs          # Public API: FileInspector, detect(), inspect()
    ├── detect.rs       # FileType detection (infer + content_inspector + extension)
    ├── handler.rs      # FileHandler trait definition
    ├── text.rs         # Plain text / source code handler
    ├── csv.rs          # CSV handler (feature: "csv")
    ├── json.rs         # JSON handler (always on, serde_json already in tree)
    ├── xml.rs          # XML handler (feature: "xml")
    ├── yaml.rs         # YAML handler (feature: "yaml")
    ├── markdown.rs     # Markdown handler (feature: "markdown")
    ├── pdf.rs          # PDF handler (feature: "pdf")
    ├── image.rs        # Image metadata handler (feature: "image")
    ├── excel.rs        # Excel handler (feature: "excel")
    ├── archive.rs      # ZIP/TAR handler (feature: "archive")
    └── budget.rs       # Token budget estimation per format
```

## Core Types

### FileType (detect.rs)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    // Text formats
    PlainText,
    SourceCode { language: Language },
    Json,
    Yaml,
    Toml,
    Xml,
    Html,
    Markdown,
    Log,
    // Binary / structured formats
    Csv,
    Excel,
    Pdf,
    Image { format: ImageFormat },
    Archive { format: ArchiveFormat },
    // Fallback
    Binary,
    Unknown,
}
```

### FileInfo (detect.rs)

```rust
pub struct FileInfo {
    pub path: PathBuf,
    pub file_type: FileType,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub is_binary: bool,
}
```

### FileHandler Trait (handler.rs)

```rust
#[async_trait]
pub trait FileHandler: Send + Sync {
    /// Human-readable name.
    fn name(&self) -> &str;

    /// Which file types this handler supports.
    fn supported_types(&self) -> &[FileType];

    /// Estimate output tokens without reading the full file.
    /// Used for budget checks before committing to extraction.
    async fn estimate_tokens(&self, info: &FileInfo) -> usize;

    /// Extract text content from a file within a token budget.
    /// Returns the extracted text and metadata.
    async fn extract(&self, info: &FileInfo, token_budget: usize) -> Result<FileContent>;
}
```

### FileContent (handler.rs)

```rust
pub struct FileContent {
    /// Extracted text suitable for LLM context.
    pub text: String,
    /// Estimated tokens for the extracted text.
    pub estimated_tokens: usize,
    /// Format-specific metadata (headers, page count, dimensions, etc.)
    pub metadata: serde_json::Value,
    /// Whether the content was truncated to fit budget.
    pub truncated: bool,
}
```

## Detection Pipeline (detect.rs)

```
File Path
    ↓
  stat() → size_bytes
    ↓
  read first 8KB → buf
    ↓
  content_inspector::inspect(&buf) → is_binary
    ↓
  infer::get(&buf) → mime_type (for binary files)
    ↓
  extension matching → FileType refinement
    ↓
  FileInfo { path, file_type, mime_type, size_bytes, is_binary }
```

Order of detection:
1. **Size check**: Reject files > max_file_size (configurable, default 50MB)
2. **Binary check**: `content_inspector` on first 1024 bytes
3. **MIME detection**: `infer::get()` on first 64 bytes (binary files)
4. **Extension matching**: `.csv`, `.xlsx`, `.pdf`, `.json`, `.xml`, `.yaml`, `.md`, etc.
5. **Language detection**: For source code, map extension to `Language` enum

## FileInspector (lib.rs)

Central coordinator — the only public API surface:

```rust
pub struct FileInspector {
    handlers: HashMap<FileType, Box<dyn FileHandler>>,
    max_file_size: u64,
}

impl FileInspector {
    pub fn new() -> Self { /* registers all enabled handlers */ }

    /// Detect file type without reading content.
    pub async fn detect(&self, path: &Path) -> Result<FileInfo>;

    /// Extract text content within token budget.
    pub async fn inspect(&self, path: &Path, token_budget: usize) -> Result<FileContent>;

    /// Inspect with pre-detected FileInfo (avoids re-detection).
    pub async fn inspect_with_info(&self, info: &FileInfo, token_budget: usize) -> Result<FileContent>;
}
```

## Integration with Existing Tools

### Enhanced file_read Tool

The existing `file_read` tool gains smart detection:

```
file_read(path)
    ↓
  FileInspector::detect(path)
    ↓
  if PlainText or SourceCode → existing UTF-8 read (zero overhead)
  if Csv/Json/Xml/Yaml/Md → format-specific extraction
  if Pdf/Image/Excel → specialized handler via spawn_blocking
  if Binary → return metadata-only (size, MIME, first bytes hex dump)
```

This is backward-compatible: text files read identically to before.

### New file_inspect Tool

Dedicated tool for format-aware inspection:

```json
{
  "name": "file_inspect",
  "description": "Inspect any file: detect format, extract text, show metadata",
  "input_schema": {
    "path": "string (required)",
    "token_budget": "integer (optional, default 2000)",
    "metadata_only": "boolean (optional, default false)"
  }
}
```

### Context Integration

`FileContent` integrates with `cuervo-context` via:
- `FileContent.estimated_tokens` feeds into `TokenAccountant` budget
- Format-specific summaries (CSV schema, PDF outline) are prioritized for context
- The `ContextSource` for files uses `FileInspector::inspect()` with the pipeline's remaining budget

## Cargo.toml Feature Gates

```toml
[features]
default = ["detect", "text", "json"]
detect = ["dep:infer", "dep:content_inspector"]
text = []
json = []
csv = ["dep:csv"]
xml = ["dep:quick-xml"]
yaml = ["dep:serde-saphyr"]
markdown = ["dep:pulldown-cmark"]
pdf = ["dep:pdf-extract"]
image = ["dep:imagesize", "dep:kamadak-exif"]
excel = ["dep:calamine"]
archive = ["dep:zip", "dep:tar", "dep:flate2"]
all-formats = ["csv", "xml", "yaml", "markdown", "pdf", "image", "excel", "archive"]
```

## Token Budget Strategy

| Format | Strategy |
|--------|----------|
| Plain text | `text.len() / 4` (existing heuristic) |
| CSV | `header_tokens + (rows * avg_row_tokens)`, capped at budget |
| JSON | Key-count weighted: keys cost more tokens than values |
| XML | Tag-aware: skip attributes, count text nodes |
| Markdown | Heading structure + prose estimate |
| PDF | Page count * ~500 tokens/page (empirical average) |
| Image | Metadata only: ~50 tokens (dimensions, format, EXIF summary) |
| Excel | Schema (headers) + first N rows, like CSV |
| Archive | File listing: ~10 tokens per entry |

## Implementation Order

1. `detect.rs` + `handler.rs` + `lib.rs` (core types and detection)
2. `text.rs` + `json.rs` (always-on, no new deps)
3. `csv.rs` (first new dependency, validates pattern)
4. `xml.rs` + `yaml.rs` + `markdown.rs`
5. `pdf.rs` + `image.rs`
6. `excel.rs` + `archive.rs`
7. `file_inspect` tool in `cuervo-tools`
8. Enhanced `file_read` with detection routing

## Phase 3 Complete

**Completed:** Phase 3 — Architecture Design
**Artifacts:** `docs/file-intelligence/ARCHITECTURE.md`
**Metrics:** 1 crate, 12 modules, 14 feature flags, 8 handlers
**Next:** Phase 4 — Implementation (create `crates/cuervo-files/`, implement detection + handlers)
