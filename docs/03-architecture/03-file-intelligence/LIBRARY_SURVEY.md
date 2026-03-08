# File Intelligence — SOTA Library Survey (Phase 2)

> Generated: 2026-02-09 | Research: 3 parallel agents, 135 crates evaluated

## Recommended Stack

| Category | Crate | Version | Downloads | Binary Impact | Async | Streaming |
|----------|-------|---------|-----------|--------------|-------|-----------|
| **File type detection** | `infer` | 0.19.0 | 70.5M | Tiny (no_std) | N/A | N/A |
| **Binary vs text** | `content_inspector` | 0.2.4 | 9.0M | Tiny (0 deps) | N/A | N/A |
| **PDF extraction** | `pdf-extract` | 0.10.0 | 687K | Medium (9 deps) | spawn_blocking | No |
| **CSV** | `csv` | 1.4.0 | 148M | Tiny (3 deps) | via tokio | Yes (row-by-row) |
| **Excel** | `calamine` | 0.33.0 | 5.2M | Moderate (9 deps) | spawn_blocking | No |
| **XML** | `quick-xml` | 0.39.0 | 207.6M | Small (1 dep) | optional tokio | Yes (pull) |
| **YAML** | `serde-saphyr` | 0.0.17 | 115K | Small (pure Rust) | No | No |
| **Markdown** | `pulldown-cmark` | 0.13.0 | 65.3M | Small (no_std) | No | Yes (pull) |
| **HTML** | `lol_html` | 2.5.0 | 2.5M | Medium | No | Yes (stream) |
| **Image dimensions** | `imagesize` | 0.14.0 | 10.2M | Tiny (0 deps) | No | N/A (16 bytes) |
| **EXIF metadata** | `kamadak-exif` | 0.6.1 | 7.4M | Small (1 dep) | No | No |
| **ZIP** | `zip` | 7.4.0 | 132.4M | Medium | No | Partial |
| **TAR** | `tar` | 0.4.44 | 128.1M | Small | No | Yes |
| **Gzip/Zlib** | `flate2` | 1.1.9 | 379.3M | Small | No | Yes |

### Estimated Total Binary Impact: +800KB to +1.5MB
(With existing LTO + opt-level="z" + panic="abort", from 5.9MB to ~6.7-7.4MB)

---

## Per-Category Analysis

### 1. File Type Detection

**Winner: `infer` 0.19.0**
- Magic byte MIME detection from first few bytes of file
- 50+ file types: images, PDF, archives, audio, video, documents
- `no_std` + `no_alloc` capable, zero mandatory deps
- API: `infer::get(&buf[..64])` → `Option<Type>` (mime + extension)

**Complement: `content_inspector` 0.2.4**
- Binary vs text detection (BOM + NULL byte scan on first 1024 bytes)
- Zero dependencies, by the author of `bat`/`fd`
- API: `content_inspector::inspect(&buf)` → `ContentType::{Text, Binary}`

**Combined pattern:**
```rust
let buf = tokio::fs::read(&path).await?; // or read first 8KB
let is_binary = content_inspector::inspect(&buf[..1024.min(buf.len())]) != ContentType::UTF_8;
let file_type = infer::get(&buf); // MIME type
```

### 2. PDF Text Extraction

**Winner: `pdf-extract` 0.10.0**
- Purpose-built for text extraction (not rendering/manipulation)
- Built on `lopdf` (3.97M downloads, updated Jan 2026)
- Encrypted PDF support via `_encrypted` variants
- 5 releases in 2025, actively maintained
- Sync only → wrap in `spawn_blocking`

**Key API:**
```rust
let bytes = tokio::fs::read("file.pdf").await?;
let text = tokio::task::spawn_blocking(move || {
    pdf_extract::extract_text_from_mem(&bytes)
}).await??;
```

**Alternatives considered:**
- `lopdf` direct: more control but more complex API
- `pdfium-render`: requires 30MB external Pdfium binary — disqualified
- `pdf`: poor documentation (8.85%), risky

### 3. CSV

**Winner: `csv` (BurntSushi) 1.4.0**
- De facto standard, 148M downloads
- True row-by-row streaming with constant memory
- Record reuse: `rdr.read_record(&mut record)` avoids per-row allocation
- First-class serde integration for typed deserialization
- `csv-core`: table-based DFA, few hundred bytes on stack
- Throughput: 108-388 MB/s depending on mode

**Key API:**
```rust
let mut rdr = csv::ReaderBuilder::new()
    .has_headers(true)
    .from_reader(file);
for result in rdr.records() {
    let record = result?;
    // process row
}
```

**Alternatives considered:**
- `polars`: DataFrame library, binary bloat (+5-15MB) — disqualified
- `arrow-csv`: good if unifying with Parquet, but +500KB-1MB for CSV alone

### 4. Excel

**Winner: `calamine` 0.33.0** (read)
- Reads .xlsx, .xls, .xlsb, .ods — broadest format support
- Pure Rust, 5.2M downloads, updated Feb 2026
- 1.75x faster than Go's excelize
- Loads sheet to memory (acceptable for preview/first-N-rows)

**Key API:**
```rust
let mut workbook: Xlsx<_> = calamine::open_workbook("file.xlsx")?;
let sheet = workbook.worksheet_range("Sheet1")?;
for row in sheet.rows().take(100) {
    // process cells
}
```

**Write (if needed): `rust_xlsxwriter` 0.93.0** — best fidelity, `constant_memory` mode

### 5. XML

**Winner: `quick-xml` 0.39.0**
- Pull/SAX parser, 50x faster than xml-rs
- 207.6M downloads, most actively maintained
- Streaming/event-based: near zero-copy with `Cow`
- Optional serde + tokio features
- Dependency: only `memchr`

**Key API:**
```rust
let mut reader = quick_xml::Reader::from_reader(BufReader::new(file));
loop {
    match reader.read_event() {
        Ok(Event::Start(e)) => { /* element */ }
        Ok(Event::Text(e)) => { /* text content */ }
        Ok(Event::Eof) => break,
        Err(e) => return Err(e.into()),
        _ => {}
    }
}
```

**Complement for DOM: `roxmltree` 0.21.1** — read-only tree, `#![forbid(unsafe_code)]`, 33.6M downloads

### 6. YAML

**Winner: `serde-saphyr` 0.0.17**
- Pure Rust, YAML 1.2 spec, no unsafe
- Direct serde deserialization (no intermediate AST)
- DoS protection via budget-based parsing
- Most actively maintained (Feb 2026)

**Alternative: `yaml-rust2` 0.11.0** — for non-serde YAML value manipulation, YAML 1.2, 25.6M downloads

**Note:** `serde_yaml` is deprecated/archived. Do not use.

### 7. Markdown

**Winner: `pulldown-cmark` 0.13.0**
- Pull parser (event iterator), streaming, minimal memory
- CommonMark + extensions: tables, footnotes, task lists, strikethrough, wikilinks
- `no_std` support, optional SIMD acceleration
- 65.3M downloads, used in Rust compiler docs
- Smallest binary impact of all Markdown parsers

**Key API:**
```rust
let parser = pulldown_cmark::Parser::new_ext(markdown_str, Options::all());
for event in parser {
    match event {
        Event::Start(Tag::Heading { level, .. }) => { /* heading */ }
        Event::Text(text) => { /* content */ }
        _ => {}
    }
}
```

**Alternative: `comrak` 0.49.0** — full GFM AST, used by crates.io/docs.rs (heavier)

### 8. HTML

**Winner: `lol_html` 2.5.0** (streaming)
- Cloudflare's streaming HTML rewriter — processes without full DOM
- Low memory, high throughput, CSS selector-based handlers
- Best for: extracting text/elements from large HTML without loading everything

**Complement: `scraper` 0.25.0** (DOM queries)
- Full DOM via html5ever + CSS selectors (BeautifulSoup-like API)
- Best for: querying specific elements from smaller HTML documents
- Heavier: pulls in html5ever parser

### 9. Image Metadata

**Dimensions: `imagesize` 0.14.0**
- Zero dependencies, reads from 16 bytes
- 10.2M downloads, updated Apr 2025

**EXIF: `kamadak-exif` 0.6.1** (conservative)
- Pure Rust, 1 dependency, 7.4M downloads
- JPEG, TIFF, HEIF, PNG, WebP
- Lightweight (~56KB source)

**Upgrade path: `nom-exif` 2.7.0** (modern)
- Async support, video metadata, GPS parsing
- More deps (nom, chrono, regex) but actively maintained (Feb 2026)

### 10. Archives

**ZIP: `zip` 7.4.0** — 132.4M downloads, comprehensive, deflate/bzip2/zstd/AES
**TAR: `tar` 0.4.44** — 128.1M downloads, streaming, pairs with flate2
**Compression: `flate2` 1.1.9** — 379.3M downloads, gzip/zlib/deflate, safe Rust default

**Pattern for .tar.gz:**
```rust
let file = std::fs::File::open("archive.tar.gz")?;
let gz = flate2::read::GzDecoder::new(file);
let mut archive = tar::Archive::new(gz);
for entry in archive.entries()? {
    let entry = entry?;
    // list or extract
}
```

---

## Dependency Overlap Analysis

Crates already in cuervo-cli workspace that overlap with new deps:

| Existing Dep | Used By New Crate |
|-------------|-------------------|
| `serde` | csv, calamine, quick-xml, serde-saphyr |
| `regex` | (nom-exif if upgraded) |
| `chrono` | (nom-exif if upgraded) |
| `thiserror` | (nom-exif if upgraded) |
| `bytes` | (nom-exif if upgraded) |
| `zstd` | zip (optional compression) |
| `sha2` | lopdf (via pdf-extract, for encrypted PDFs) |

Most new deps are lightweight and isolated. The `csv` + `infer` + `content_inspector` + `imagesize` cluster adds near-zero overhead.

---

## Feature-Gating Strategy

```toml
[features]
default = ["file-intelligence"]
file-intelligence = ["file-detect", "file-text", "file-data"]
file-detect = ["dep:infer", "dep:content_inspector", "dep:imagesize"]
file-text = ["dep:pdf-extract", "dep:pulldown-cmark", "dep:quick-xml", "dep:serde-saphyr"]
file-data = ["dep:csv", "dep:calamine"]
file-archives = ["dep:zip", "dep:tar", "dep:flate2"]
file-images = ["dep:kamadak-exif"]
file-html = ["dep:lol_html"]
```

This allows users building cuervo-cli to exclude heavy format support if not needed.

---

## Phase 2 Complete

**Completed:** Phase 2 — SOTA Research
**Artifacts:** `docs/file-intelligence/LIBRARY_SURVEY.md`
**Metrics:** 135 crates evaluated, 14 selected, est. +800KB-1.5MB binary impact
**Next:** Phase 3 — Architecture Design (unified `FileHandler` trait, file registry, streaming reader, token budget integrator)
