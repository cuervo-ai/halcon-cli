# File Intelligence — Gap Analysis (Phase 1)

> Generated: 2026-02-09 | Cuervo CLI v0.1.0

## Format Support Matrix

| Format | Read | Write | Parse | Extract | Search | Current Support |
|--------|------|-------|-------|---------|--------|----------------|
| **Plain text** | Y | Y | Y | N/A | Y (grep) | Full |
| **Source code** | Y | Y | Y | Symbols (6 langs) | Y (grep) | Full |
| **JSON** | Y | Y | Y (serde_json) | N | Y (grep) | Full |
| **TOML** | Y | Y | Y (toml crate) | N | Y (grep) | Full |
| **YAML** | Y | Y | N (syntect only) | N | Y (grep) | Partial (highlight only) |
| **XML/HTML** | Y | Y | N | N | Y (grep) | Text-only |
| **Markdown** | Y | Y | N (no AST) | N | Y (grep) | Text-only |
| **CSV** | N | N | N | N | N | None |
| **Excel (.xlsx)** | N | N | N | N | N | None |
| **PDF** | N | N | N | N | N | None |
| **Images (PNG/JPG/etc)** | N | N | N | N | N | None |
| **Archives (zip/tar/gz)** | N | N | N | N | N | None |
| **Parquet/Arrow** | N | N | N | N | N | None |
| **SQLite (as file)** | N | N | N | N | N | None (DB access only) |
| **Log files** | Y | N | N (no structure) | N | Y (grep) | Partial |

## Dependency Gaps

### Current File-Related Dependencies
```
tokio (fs module) — async file I/O
serde_json — JSON parse/serialize
toml — TOML parse/serialize
syntect — syntax highlighting (includes YAML grammar)
regex — pattern matching
glob — file path matching
zstd — compression (context pipeline, not file processing)
```

### Missing Dependencies (by format)
| Format | Crate Needed | Purpose |
|--------|-------------|---------|
| PDF text extraction | `pdf-extract` or `lopdf` | Parse PDF, extract text layers |
| PDF metadata | `lopdf` | Title, author, page count, bookmarks |
| Images (metadata) | `image` | Dimensions, format detection, thumbnails |
| Images (OCR) | External: `tesseract` via bash | Text extraction from images |
| CSV | `csv` | Typed parsing, header detection, streaming |
| Excel | `calamine` | Read .xlsx/.xls/.ods, sheet enumeration |
| XML | `quick-xml` | SAX/DOM parsing, XPath-like queries |
| YAML (full) | `serde_yaml` | Parse/serialize, multi-document support |
| Markdown AST | `pulldown-cmark` | Parse to AST, heading extraction, link collection |
| HTML parsing | `scraper` or `select` | DOM queries, text extraction |
| Archives | `zip`, `tar`, `flate2` | List contents, extract files, stream entries |
| Parquet | `parquet` (arrow2) | Columnar data read, schema extraction |
| Binary detection | `infer` or `tree_magic_mini` | Magic byte MIME type detection |
| File hashing | `sha2` (already present) | Content fingerprinting |

## Architecture Gaps

### 1. No Binary-Safe I/O Path
- `file_read` calls `tokio::fs::read_to_string()` → fails on non-UTF-8
- Need: `tokio::fs::read()` → `Vec<u8>` path with format dispatch

### 2. No File Type Detection
- No MIME type or magic byte checking
- Need: Detect format before choosing handler (text vs binary vs structured)

### 3. No Streaming Read
- Full file loaded to memory in single call
- Need: Chunked/streaming read for large files (>10MB)

### 4. No Size Pre-Check
- No `fs::metadata().len()` before read
- Need: Check file size, reject or stream based on threshold

### 5. ToolOutput is String-Only
- `ToolOutput.content: String` cannot carry binary data or structured metadata
- Need: Either base64-encoded binary or structured output variant

### 6. Token Estimation is Format-Blind
- `text.len().div_ceil(4)` is wrong for:
  - CSV (dense numeric data → fewer tokens per char)
  - JSON (braces/quotes → more chars per semantic token)
  - Code (imports/boilerplate → low information density)
  - PDF text (extracted text may differ from rendered layout)

### 7. No Format-Specific Context Integration
- Context pipeline treats all content as flat text
- Need: Format-aware summarization (table → schema, code → signatures, PDF → outline)

## Risk Assessment

| Risk | Severity | Mitigation |
|------|----------|-----------|
| OOM on large binary files | HIGH | Size pre-check + streaming + budget limits |
| Binary file as UTF-8 garbles output | HIGH | Magic byte detection before read |
| PDF/image extraction quality | MEDIUM | Use established crates, fallback to metadata-only |
| Dependency bloat | MEDIUM | Feature-gate heavy crates (pdf, image), keep optional |
| Sandbox bypass via format parsing | LOW | Parse in sandboxed context, limit recursion depth |
| Token budget overshoot | MEDIUM | Format-specific token estimation |

## Priority Ranking (by agent utility)

1. **Binary detection + size pre-check** — Prevents crashes, enables routing
2. **CSV/JSON/YAML structured read** — Most common data formats in dev workflows
3. **PDF text extraction** — Documentation, papers, specs
4. **Markdown AST** — README parsing, documentation navigation
5. **Image metadata** — Dimensions, format info (OCR is lower priority)
6. **Archive listing** — Inspect zip/tar without extraction
7. **Excel read** — Spreadsheet data access
8. **XML parsing** — Config files, APIs
9. **Parquet/Arrow** — Data engineering workflows
10. **Log structure detection** — Pattern-based log parsing
