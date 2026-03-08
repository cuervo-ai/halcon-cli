# Integrating Rust with Node.js for CLI Tools and AI Applications

**Project:** Cuervo CLI -- Generative AI Platform for Software Development
**Version:** 1.0
**Date:** February 6, 2026
**Author:** Architecture Team
**Classification:** Internal Use

---

## Executive Summary

This report evaluates the current state of integrating Rust with Node.js/TypeScript for performance-critical paths in CLI tools and AI applications. The analysis covers napi-rs bindings, Tree-sitter AST parsing, embedded vector search, tokenization, regex scanning, and real-world production examples. The goal is to inform Cuervo CLI's architecture decisions for components where JavaScript performance is insufficient: AST parsing for RAG chunking, vector similarity search, token counting, and PII/pattern scanning.

**Key Findings:**

1. **napi-rs is production-ready** (v2.x/v3.x) and is the dominant approach for Rust-to-Node.js bindings. Used by SWC, Turbopack, Biome, Rspack, Lightning CSS, and Oxc. Cross-platform prebuild distribution via npm is a solved problem.
2. **Tree-sitter via Rust** provides the fastest AST parsing available, with incremental parsing support. Node.js bindings exist via `tree-sitter` (C/N-API) and `web-tree-sitter` (WASM). For a CLI tool, the native N-API approach is preferred.
3. **Embedded vector search in Rust** is viable via `usearch` (best napi-rs integration), `hnswlib` (C++ with Node bindings), or a custom HNSW implementation. For 10K-100K vectors, all options deliver sub-millisecond to low-millisecond query times.
4. **Rust tokenizers** (tiktoken-rs, HuggingFace tokenizers) are 5-20x faster than WASM and 50-100x faster than pure JS. They can be compiled to napi-rs addons straightforwardly.
5. **Rust regex via napi-rs** delivers 3-10x speedup over JavaScript RegExp for complex pattern matching (PII detection), with the advantage of guaranteed linear-time execution (no ReDoS).
6. **WASM is a viable fallback** for simpler deployment but sacrifices 30-60% performance vs native napi-rs. Use WASM for browser/cross-platform portability; use napi-rs for CLI tools where performance matters.

**Recommendation for Cuervo CLI:** Adopt a hybrid TypeScript + Rust (napi-rs) architecture for four specific performance-critical modules: (1) AST parsing via tree-sitter, (2) embedded vector search, (3) token counting, and (4) PII/regex scanning. Distribute Rust binaries as platform-specific npm packages following the SWC/Turbopack pattern.

---

## Table of Contents

1. [napi-rs: The Bridge Between Rust and Node.js](#1-napi-rs-the-bridge-between-rust-and-nodejs)
2. [Tree-sitter + Rust for AST Parsing](#2-tree-sitter--rust-for-ast-parsing)
3. [Vector Search in Rust](#3-vector-search-in-rust)
4. [Tokenizer Implementations in Rust](#4-tokenizer-implementations-in-rust)
5. [Regex/Scanning in Rust](#5-regexscanning-in-rust)
6. [Real-World Examples of TypeScript + Rust Hybrid Pattern](#6-real-world-examples-of-typescript--rust-hybrid-pattern)
7. [Cross-Compilation Challenges and Distribution](#7-cross-compilation-challenges-and-distribution)
8. [napi-rs vs WASM: When to Use Each](#8-napi-rs-vs-wasm-when-to-use-each)
9. [Concrete Recommendations for Cuervo CLI](#9-concrete-recommendations-for-cuervo-cli)
10. [Appendix: Benchmark Data and Version Reference](#10-appendix-benchmark-data-and-version-reference)

---

## 1. napi-rs: The Bridge Between Rust and Node.js

### 1.1 Overview and Current Version

**napi-rs** is a framework for building pre-compiled Node.js addons in Rust. It generates Node-API (N-API) compatible native modules that work across Node.js versions without recompilation.

| Property | Detail |
|----------|--------|
| **Current stable version** | v2.16.x (napi crate) / v3.0.x (napi-rs CLI toolchain) |
| **Rust edition** | 2021 (compatible with Rust 1.70+) |
| **Node-API version** | Targets N-API v8+ (Node.js 18+) |
| **GitHub stars** | ~6,500+ |
| **License** | MIT |
| **Maintainer** | LongYinan (Brooooooklyn) and community |

The napi-rs ecosystem consists of:
- `napi` crate: Core Rust macros and types for defining N-API functions
- `napi-derive`: Procedural macros (`#[napi]`) for automatic JS binding generation
- `@napi-rs/cli`: Node.js CLI tool for scaffolding, building, and publishing
- `napi-build`: Build script helper for `build.rs`

### 1.2 Stability Assessment

napi-rs is **production-grade** software. It has been in production at massive scale for 3+ years in projects like SWC (used by Next.js, which serves millions of production sites). The API surface is stable with backward-compatible evolution. Breaking changes are rare and well-communicated.

Key stability indicators:
- Used in production by Vercel (SWC/Turbopack), the Biome team, the Rspack team (ByteDance), and the Oxc project
- Comprehensive test suite across all target platforms
- Regular releases (approximately biweekly)
- Active maintainer community with responsive issue resolution
- Well-documented migration guides between major versions

### 1.3 Cross-Platform Build Support

napi-rs provides first-class cross-compilation support via the `@napi-rs/cli` tool and pre-configured GitHub Actions workflows.

| Target | Triple | Status | Notes |
|--------|--------|--------|-------|
| macOS ARM64 (Apple Silicon) | `aarch64-apple-darwin` | Stable | Primary dev target for many users |
| macOS x64 (Intel) | `x86_64-apple-darwin` | Stable | Universal binary possible |
| Linux x64 (glibc) | `x86_64-unknown-linux-gnu` | Stable | Most common server/CI target |
| Linux x64 (musl) | `x86_64-unknown-linux-musl` | Stable | Alpine Docker, static linking |
| Linux ARM64 (glibc) | `aarch64-unknown-linux-gnu` | Stable | AWS Graviton, Docker on M-series |
| Linux ARM64 (musl) | `aarch64-unknown-linux-musl` | Stable | Alpine ARM |
| Windows x64 (MSVC) | `x86_64-pc-windows-msvc` | Stable | Primary Windows target |
| Windows ARM64 | `aarch64-pc-windows-msvc` | Beta | Snapdragon laptops |
| FreeBSD x64 | `x86_64-unknown-freebsd` | Experimental | Niche |
| Android ARM64 | `aarch64-linux-android` | Experimental | For Termux/embedded |

**Cross-compilation toolchain:** napi-rs uses Zig as a cross-compilation linker (via `cargo-zigbuild`) or platform-specific cross toolchains. The `@napi-rs/cli` scaffolding command generates a complete GitHub Actions matrix that builds for all targets in parallel.

### 1.4 Prebuild Binary Distribution via npm

napi-rs implements a **platform-specific npm package** distribution pattern:

```
@cuervo/native                     # Main package (JS loader)
@cuervo/native-darwin-arm64        # macOS ARM64 binary
@cuervo/native-darwin-x64          # macOS Intel binary
@cuervo/native-linux-x64-gnu      # Linux x64 (glibc) binary
@cuervo/native-linux-x64-musl     # Linux x64 (musl) binary
@cuervo/native-linux-arm64-gnu    # Linux ARM64 binary
@cuervo/native-win32-x64-msvc     # Windows x64 binary
```

The main package contains:
1. A JavaScript loader that detects the current platform/arch
2. `optionalDependencies` pointing to all platform-specific packages
3. npm only downloads the matching platform package at install time

```json
{
  "name": "@cuervo/native",
  "optionalDependencies": {
    "@cuervo/native-darwin-arm64": "1.0.0",
    "@cuervo/native-darwin-x64": "1.0.0",
    "@cuervo/native-linux-x64-gnu": "1.0.0",
    "@cuervo/native-win32-x64-msvc": "1.0.0"
  }
}
```

The loader (`index.js`) typically looks like:

```javascript
const { existsSync, readFileSync } = require('fs');
const { join } = require('path');
const { platform, arch } = process;

let nativeBinding = null;
let loadError = null;

// Platform detection and loading logic
switch (platform) {
  case 'darwin':
    switch (arch) {
      case 'arm64':
        nativeBinding = require('@cuervo/native-darwin-arm64');
        break;
      case 'x64':
        nativeBinding = require('@cuervo/native-darwin-x64');
        break;
    }
    break;
  case 'linux':
    // ... glibc vs musl detection
    break;
  case 'win32':
    nativeBinding = require('@cuervo/native-win32-x64-msvc');
    break;
}
```

**This pattern is proven at scale:** SWC distributes 15+ platform-specific packages per release, each containing a `.node` native binary of 20-50 MB (compressed to 5-15 MB for download).

### 1.5 Performance Characteristics

napi-rs overhead for crossing the JS/Rust boundary is minimal:

| Operation | Overhead | Notes |
|-----------|----------|-------|
| Function call (no args) | ~50-100 ns | N-API call overhead |
| Pass string (1 KB) | ~200-500 ns | UTF-8 copy from V8 to Rust |
| Pass Buffer (1 KB) | ~50-100 ns | Zero-copy via shared memory |
| Return string (1 KB) | ~200-500 ns | UTF-8 copy from Rust to V8 |
| Return Buffer (1 KB) | ~50-100 ns | Zero-copy |
| Pass/return JSON object | ~1-10 us | Serialization overhead |
| Async task (thread pool) | ~5-20 us | Thread scheduling |

**Key insight:** The boundary crossing overhead is negligible for any computation-heavy task (>100 us). For tasks like AST parsing (ms), vector search (ms), tokenization (us-ms), and regex scanning (us-ms), the Rust computation time dominates and the napi-rs overhead is <1%.

### 1.6 Major Projects Using napi-rs

| Project | Organization | Usage | npm Downloads/week |
|---------|-------------|-------|-------------------|
| **SWC** | Vercel | JavaScript/TypeScript compiler | ~30M+ |
| **Turbopack** | Vercel | Next.js bundler (via SWC) | Bundled with Next.js |
| **Lightning CSS** | Devon Govett / Parcel | CSS parser and transformer | ~5M+ |
| **Rspack** | ByteDance | Webpack-compatible Rust bundler | ~500K+ |
| **Biome** | Biome Project | Linter + formatter (Rome successor) | ~1M+ |
| **Oxc (oxlint)** | Oxc Project | JavaScript toolchain (parser, linter) | ~500K+ |
| **Rolldown** | Vite team / Evan You | Rust bundler for Vite | Early adoption |
| **Prisma (query engine)** | Prisma | Database ORM query engine | ~3M+ |
| **tree-sitter** | GitHub/Community | Parser generator (uses N-API directly, pattern compatible) | ~500K+ |
| **napi-nanoid** | Community | Fast ID generation | ~100K+ |
| **magic-string-rs** | Community | String manipulation | Growing |

---

## 2. Tree-sitter + Rust for AST Parsing

### 2.1 Overview

Tree-sitter is an incremental parsing library originally written in C, now with significant Rust integration. It generates concrete syntax trees (CSTs) that are suitable for code intelligence, syntax highlighting, and structural code analysis.

| Property | Detail |
|----------|--------|
| **Language** | Core in C; Rust bindings (`tree-sitter` crate); CLI in Rust |
| **Current version** | v0.24.x (core) / v0.24.x (Rust crate) |
| **Supported languages** | 200+ grammar definitions (community-maintained) |
| **Parsing model** | GLR (Generalized LR) with incremental reparsing |
| **License** | MIT |

### 2.2 Node.js Bindings

There are two main approaches for using Tree-sitter from Node.js:

**Option A: `tree-sitter` npm package (Native N-API)**
- Uses C bindings compiled via node-gyp (not napi-rs, but same N-API foundation)
- Full performance (no WASM overhead)
- Requires native compilation or prebuilt binaries
- Version: 0.21.x+ (stable for years)
- Grammar packages: `tree-sitter-javascript`, `tree-sitter-typescript`, `tree-sitter-python`, etc.

**Option B: `web-tree-sitter` npm package (WASM)**
- Tree-sitter core compiled to WebAssembly via Emscripten
- Cross-platform without native compilation
- ~30-50% slower than native
- Works in browser and Node.js identically
- Grammar files are `.wasm` blobs loaded at runtime

**Option C: Custom Rust napi-rs wrapper**
- Write a Rust crate that uses the `tree-sitter` Rust crate directly
- Expose parsing functions to Node.js via napi-rs
- Full control over API surface and performance
- Can bundle multiple grammars into a single native addon
- This is the recommended approach for Cuervo CLI

### 2.3 Performance for Code Intelligence

Tree-sitter performance benchmarks (representative, on modern hardware -- Apple M2/M3 or equivalent x64):

| Operation | File Size | Time | Throughput |
|-----------|-----------|------|------------|
| Parse TypeScript file | 1 KB (~30 lines) | ~0.1-0.3 ms | ~3-10 MB/s |
| Parse TypeScript file | 10 KB (~300 lines) | ~0.5-1.5 ms | ~7-20 MB/s |
| Parse TypeScript file | 100 KB (~3000 lines) | ~3-8 ms | ~12-30 MB/s |
| Parse large JS file | 1 MB (~25K lines) | ~20-50 ms | ~20-50 MB/s |
| Incremental reparse (small edit) | Any size | ~0.05-0.5 ms | Near-instant |
| Parse entire codebase (10K files, ~50MB total) | 50 MB | ~2-5 seconds | ~10-25 MB/s |

**Key advantages for Cuervo CLI's RAG pipeline:**
1. **Incremental parsing:** After initial parse, editing a file only requires reparsing the changed region. This maps directly to the incremental indexing requirement identified in the technical review.
2. **AST-aware chunking:** Tree-sitter's CST allows splitting code by semantic boundaries (functions, classes, methods) rather than arbitrary line counts. This produces higher-quality embeddings.
3. **Language-agnostic:** One parsing framework handles TypeScript, Python, Rust, Go, Java, C/C++, and 200+ other languages.
4. **Query language:** Tree-sitter has a built-in S-expression query language for structural pattern matching (e.g., "find all function definitions" or "find all import statements").

### 2.4 Recommended Architecture for Cuervo CLI

```
TypeScript (Node.js)                    Rust (napi-rs)
-----------------------------           ---------------------------
ContextBuilder.ts                       tree_sitter_wrapper.rs
  |                                       |
  |-- requestParse(filePath, lang)  -->   |-- parse(source, lang)
  |                                       |     uses tree-sitter crate
  |<-- returns AST node handles     ---   |     returns serialized AST
  |                                       |
  |-- requestChunks(ast, strategy)  -->   |-- chunk(ast, strategy)
  |                                       |     walks AST, extracts
  |<-- returns CodeChunk[]          ---   |     functions/classes/blocks
  |                                       |
  |-- requestQuery(ast, pattern)    -->   |-- query(ast, pattern)
  |                                       |     runs tree-sitter query
  |<-- returns QueryMatch[]         ---   |     returns matches

CodeChunk {
  content: string;
  filePath: string;
  startLine: number;
  endLine: number;
  kind: 'function' | 'class' | 'method' | 'module' | 'block';
  name?: string;
  language: string;
}
```

### 2.5 Bundling Grammars

For Cuervo CLI, we should pre-bundle grammars for the most common languages:

| Priority | Language | Grammar Package | Size (compiled) |
|----------|----------|----------------|-----------------|
| P0 | TypeScript/TSX | `tree-sitter-typescript` | ~300 KB |
| P0 | JavaScript/JSX | `tree-sitter-javascript` | ~200 KB |
| P0 | Python | `tree-sitter-python` | ~150 KB |
| P0 | Rust | `tree-sitter-rust` | ~250 KB |
| P0 | Go | `tree-sitter-go` | ~150 KB |
| P1 | Java | `tree-sitter-java` | ~200 KB |
| P1 | C/C++ | `tree-sitter-c` / `tree-sitter-cpp` | ~400 KB |
| P1 | Ruby | `tree-sitter-ruby` | ~150 KB |
| P1 | PHP | `tree-sitter-php` | ~200 KB |
| P2 | CSS/SCSS | `tree-sitter-css` | ~100 KB |
| P2 | HTML | `tree-sitter-html` | ~100 KB |
| P2 | SQL | `tree-sitter-sql` | ~150 KB |
| P2 | Bash | `tree-sitter-bash` | ~100 KB |
| P2 | YAML/TOML/JSON | Various | ~300 KB |

Total bundled size for P0+P1: ~2 MB (compiled into the native addon).

---

## 3. Vector Search in Rust

### 3.1 Requirements for Cuervo CLI

Based on the architecture documents, Cuervo CLI needs an embedded vector database for:
- Local codebase indexing (embeddings of code chunks)
- Semantic search for RAG context assembly
- Semantic cache (prompt similarity matching)

Scale requirements:
- 10K-100K vectors (typical codebase: ~10K-50K code chunks)
- Embedding dimensions: 384-1536 (depending on model)
- Query latency: <100ms (target), ideally <10ms
- Must be embeddable (no external server process)
- Persistent storage (survive CLI restarts)

### 3.2 Rust Vector Search Libraries

| Library | Type | HNSW Support | Persistence | Node.js Bindings | Maturity |
|---------|------|-------------|-------------|-----------------|----------|
| **usearch** (USearch) | Rust/C++ | Yes (core algorithm) | Yes (memory-mapped) | Yes (official npm) | Production |
| **hnswlib** | C++ | Yes | Yes (save/load) | Yes (`hnswlib-node`) | Production |
| **hora** | Pure Rust | Yes + others | Serialization | None (must wrap) | Beta |
| **instant-distance** | Pure Rust | Yes | Serialization | None (must wrap) | Stable but minimal |
| **qdrant (core)** | Rust | Yes + others | Yes | No (server mode) | Production |
| **lance/lancedb** | Rust | IVF_PQ, others | Yes (columnar) | Yes (official npm) | Production |
| **faiss** | C++ | Yes + others | Yes | Python only mainstream | Production |
| **annoy** | C++ | Random projection trees | Yes (mmap) | Yes (`annoy-node`) | Production |

### 3.3 Detailed Analysis

#### USearch (Recommended for napi-rs integration)

USearch is a single-header C++ / Rust library for Approximate Nearest Neighbor (ANN) search. It has official Node.js bindings published as `usearch`.

| Property | Detail |
|----------|--------|
| **npm package** | `usearch` |
| **Version** | v2.x |
| **Algorithm** | HNSW (Hierarchical Navigable Small World) |
| **Persistence** | Memory-mapped files (zero-copy load) |
| **Index types** | f32, f16, i8 (quantized) |
| **Distance metrics** | Cosine, L2 (Euclidean), IP (Inner Product), Hamming |
| **Max dimensions** | Unlimited (practical up to 4096) |
| **License** | Apache-2.0 |

**Performance benchmarks (estimated, Apple M2, 768-dim vectors, cosine):**

| Vectors | Index Build | Query (top-10) | Memory | Recall@10 |
|---------|-------------|----------------|--------|-----------|
| 10,000 | ~0.5s | ~0.1 ms | ~30 MB | 0.97 |
| 50,000 | ~3s | ~0.3 ms | ~150 MB | 0.96 |
| 100,000 | ~7s | ~0.5 ms | ~300 MB | 0.95 |

USearch advantages:
- Official Node.js bindings (no custom wrapping needed)
- Memory-mapped persistence (instant load of pre-built indexes)
- Supports quantized vectors (f16, i8) for 2-4x memory reduction
- Cross-platform (macOS, Linux, Windows)

#### hnswlib-node

hnswlib-node wraps the original C++ hnswlib library. It is the most battle-tested option.

| Property | Detail |
|----------|--------|
| **npm package** | `hnswlib-node` |
| **Version** | v3.x |
| **Persistence** | Save/load to file |
| **Build** | node-gyp (C++ compilation) |

**Performance benchmarks (estimated, similar hardware):**

| Vectors | Index Build | Query (top-10) | Memory |
|---------|-------------|----------------|--------|
| 10,000 | ~0.8s | ~0.1 ms | ~35 MB |
| 50,000 | ~5s | ~0.4 ms | ~170 MB |
| 100,000 | ~12s | ~0.8 ms | ~340 MB |

#### Custom Rust HNSW via napi-rs

For maximum control, Cuervo CLI could implement a thin Rust wrapper around either `usearch` (Rust crate) or `instant-distance` and expose it via napi-rs.

Advantages of custom wrapper:
- Bundle with other Rust components (tree-sitter, tokenizer, regex) into a single native addon
- Fine-grained control over serialization format
- Can add Cuervo-specific features (hybrid BM25+vector, metadata filtering)
- Single binary instead of multiple native dependencies

#### LanceDB (Alternative: Columnar embedded DB)

LanceDB is an embedded vector database built on the Lance columnar format. It has official TypeScript/Node.js bindings.

| Property | Detail |
|----------|--------|
| **npm package** | `@lancedb/lancedb` (vectordb) |
| **Storage** | Lance columnar format (disk-based, zero-copy) |
| **Indexing** | IVF_PQ, IVF_HNSW_SQ |
| **Extras** | Full-text search, metadata filtering, SQL-like queries |

LanceDB is a stronger choice if Cuervo CLI needs more than pure vector search (e.g., metadata filtering, full-text hybrid search), but it has a larger footprint than pure HNSW.

### 3.4 Recommendation

**Primary: USearch** via its official npm bindings for MVP/Beta. Reasons:
- Official Node.js bindings eliminate custom wrapping work
- Memory-mapped persistence is ideal for CLI (instant startup)
- Sub-millisecond queries at the required scale
- Quantization support reduces memory footprint

**Long-term: Custom napi-rs addon** bundling USearch (Rust crate) + tree-sitter + tokenizer + regex into a single `@cuervo/native` package. This reduces the number of native dependencies and simplifies distribution.

---

## 4. Tokenizer Implementations in Rust

### 4.1 Why Rust Tokenizers Matter for Cuervo CLI

The technical review identified token counting as a non-trivial problem (Hallazgo ALTO #7). Precise token counting is needed for:
- Context window management (fitting content within model limits)
- Cost estimation (billing is per-token)
- Budget allocation across system prompt, history, and codebase context
- Semantic cache key computation

Each model provider uses a different tokenizer. Running tokenizers in JavaScript is possible but slow. Rust tokenizers via napi-rs provide 50-100x speedup.

### 4.2 Available Rust Tokenizer Libraries

#### tiktoken-rs

Rust port of OpenAI's tiktoken tokenizer (BPE-based).

| Property | Detail |
|----------|--------|
| **Crate** | `tiktoken-rs` |
| **Version** | v0.5.x+ |
| **Encodings** | cl100k_base (GPT-4/4o), o200k_base (GPT-4o), p50k_base (GPT-3.5) |
| **License** | MIT |
| **Performance** | ~10-50 MB/s throughput |

Suitable for: OpenAI models (GPT-4, GPT-4o, o-series).

Can it be compiled to napi-rs? **Yes.** Straightforward wrapping:

```rust
use napi_derive::napi;
use tiktoken_rs::cl100k_base;

#[napi]
pub fn count_tokens_openai(text: String, encoding: String) -> u32 {
    let bpe = match encoding.as_str() {
        "cl100k_base" => cl100k_base().unwrap(),
        "o200k_base" => tiktoken_rs::o200k_base().unwrap(),
        _ => cl100k_base().unwrap(),
    };
    bpe.encode_with_special_tokens(&text).len() as u32
}
```

#### HuggingFace Tokenizers

The `tokenizers` crate is the Rust core of HuggingFace's tokenizers library. It supports BPE, WordPiece, Unigram, and other algorithms. It can load any tokenizer from the HuggingFace Hub.

| Property | Detail |
|----------|--------|
| **Crate** | `tokenizers` |
| **Version** | v0.20.x+ |
| **Algorithms** | BPE, WordPiece, Unigram, SentencePiece |
| **Model support** | Any HuggingFace-compatible tokenizer.json |
| **License** | Apache-2.0 |
| **Performance** | ~20-100 MB/s throughput (varies by tokenizer) |

Suitable for: Claude (uses a BPE-variant tokenizer), Llama, Mistral, DeepSeek, Qwen, and any model with a `tokenizer.json` file.

**Note on Claude tokenization:** Anthropic does not officially publish their tokenizer, but the token counts returned by the API can be used for post-hoc tracking. For pre-request estimation, a conservative heuristic (words x 1.3) or a proxy tokenizer (cl100k_base gives roughly similar counts for English text) is the pragmatic approach. If Anthropic publishes a tokenizer model, it would likely be loadable via the `tokenizers` crate.

#### Custom BPE Implementation

For very fast, minimal token counting where exact accuracy per-provider is not critical, a custom Rust BPE implementation (~200 lines) with a pre-trained vocabulary can achieve:
- ~100+ MB/s throughput
- ~5% accuracy margin vs exact provider tokenizer
- Minimal binary size (~100 KB)

### 4.3 Performance: Rust vs WASM vs JavaScript

Benchmark: Tokenizing 100 KB of mixed English/code text, counting tokens.

| Implementation | Time | Throughput | Relative |
|---------------|------|------------|----------|
| **tiktoken-rs via napi-rs** | ~2-5 ms | ~20-50 MB/s | 1x (baseline) |
| **tokenizers (HF) via napi-rs** | ~1-3 ms | ~30-100 MB/s | 0.7-1.5x |
| **tiktoken WASM** | ~5-15 ms | ~7-20 MB/s | 0.3-0.5x |
| **js-tiktoken (pure JS)** | ~50-200 ms | ~0.5-2 MB/s | 0.02-0.1x |
| **gpt-tokenizer (pure JS)** | ~30-100 ms | ~1-3 MB/s | 0.03-0.15x |

**Key takeaway:** Rust via napi-rs is 5-20x faster than WASM and 50-100x faster than pure JavaScript for tokenization. For a CLI tool that needs to count tokens on every request (context management, cost display), Rust tokenizers are the clear choice.

### 4.4 Recommendation for Cuervo CLI

Build a unified `TokenCounter` napi-rs addon:

```rust
use napi_derive::napi;

#[napi(object)]
pub struct TokenCount {
    pub count: u32,
    pub encoding: String,
    pub is_exact: bool,
}

#[napi]
pub fn count_tokens(text: String, provider: String, model: String) -> TokenCount {
    match provider.as_str() {
        "openai" => count_tokens_openai(&text, &model),
        "anthropic" => count_tokens_anthropic_estimate(&text, &model),
        "ollama" | "local" => count_tokens_hf(&text, &model),
        _ => count_tokens_fallback(&text),
    }
}
```

This handles the multi-provider token counting requirement with a single function call from TypeScript.

---

## 5. Regex/Scanning in Rust

### 5.1 The Case for Rust Regex in Cuervo CLI

Cuervo CLI's PII detection pipeline (documented in `05-security-legal/02-privacidad-datos.md`) and code scanning features need fast regex execution for:
- Detecting API keys, tokens, credentials in code/prompts
- Detecting email addresses, IP addresses, credit card numbers
- Code pattern scanning (security vulnerabilities, anti-patterns)
- Custom rule-based scanning (user-defined regex patterns)

### 5.2 Rust `regex` Crate

The `regex` crate is the de facto standard regex library in Rust.

| Property | Detail |
|----------|--------|
| **Crate** | `regex` |
| **Version** | v1.10.x+ |
| **Engine** | Finite automaton (NFA/DFA hybrid) |
| **Features** | Unicode, named captures, lazy quantifiers, character classes |
| **Guarantee** | **Linear-time execution** (no catastrophic backtracking / ReDoS) |
| **License** | MIT/Apache-2.0 |

The **linear-time guarantee** is critically important for security scanning. JavaScript's RegExp engine uses backtracking, which means a maliciously crafted input (or a poorly written regex) can cause exponential execution time (ReDoS attacks). The Rust `regex` crate uses a Thompson NFA construction that guarantees O(n) execution regardless of the pattern.

### 5.3 Performance: Rust regex vs JavaScript RegExp

Benchmark: Scanning a 100 KB text file with 20 PII detection patterns applied sequentially.

| Implementation | Time | Notes |
|---------------|------|-------|
| **Rust `regex` via napi-rs (compiled RegexSet)** | ~0.5-2 ms | All 20 patterns in one pass |
| **Rust `regex` via napi-rs (individual)** | ~2-5 ms | 20 sequential regex matches |
| **JavaScript RegExp (individual)** | ~5-20 ms | 20 sequential `string.match()` |
| **JavaScript RegExp (complex patterns)** | ~10-100 ms | Patterns with lookahead/lookbehind |

**Key advantage: `RegexSet`**

The Rust `regex` crate supports `RegexSet`, which compiles multiple patterns into a single automaton and matches them all in a single pass over the input. This is ideal for PII detection where you have N patterns to check:

```rust
use regex::RegexSet;
use napi_derive::napi;

#[napi(object)]
pub struct ScanResult {
    pub pattern_name: String,
    pub match_text: String,
    pub start: u32,
    pub end: u32,
    pub line: u32,
}

#[napi]
pub fn scan_pii(text: String) -> Vec<ScanResult> {
    let patterns = RegexSet::new(&[
        r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}",  // Email
        r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b",            // IPv4
        r"\b\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}\b",        // Credit card
        r"(?i)(sk-[a-zA-Z0-9]{32,})",                           // OpenAI API key
        r"(?i)(ghp_[a-zA-Z0-9]{36,})",                          // GitHub PAT
        r"(?i)(AKIA[0-9A-Z]{16})",                               // AWS Access Key
        // ... more patterns
    ]).unwrap();
    // ... matching logic
}
```

JavaScript has no equivalent to `RegexSet`. You must run each pattern separately, resulting in N passes over the input.

### 5.4 Recommendation

Include a `PiiScanner` module in the Rust napi-rs addon:
- Pre-compiled `RegexSet` with ~30-50 common PII/credential patterns
- Configurable (load additional patterns from config)
- Returns structured results with line numbers and pattern names
- Linear-time guarantee prevents DoS via crafted inputs
- 3-10x faster than equivalent JavaScript for typical workloads

---

## 6. Real-World Examples of TypeScript + Rust Hybrid Pattern

### 6.1 SWC / Turbopack (Vercel)

**How they use Rust:**
- SWC is a Rust-based JavaScript/TypeScript compiler (parser, transformer, minifier)
- Turbopack is a Rust-based bundler for Next.js
- Both expose their functionality to Node.js via napi-rs

**Distribution pattern:**
```
@swc/core                          # Main package (loader + TS types)
@swc/core-darwin-arm64            # macOS Apple Silicon
@swc/core-darwin-x64              # macOS Intel
@swc/core-linux-x64-gnu           # Linux glibc
@swc/core-linux-x64-musl          # Linux musl
@swc/core-linux-arm64-gnu         # Linux ARM64
@swc/core-win32-x64-msvc          # Windows x64
@swc/core-linux-arm64-musl        # Linux ARM64 musl
# ... 12+ platform packages total
```

**Key learnings:**
- They publish all platform binaries on every release using GitHub Actions
- CI matrix builds take ~20-30 minutes across all targets
- Binary sizes: 20-50 MB per platform (compressed: 5-15 MB)
- The `@swc/core` package contains only JS loader code (~10 KB)
- TypeScript types are auto-generated from Rust structs via `napi-derive`

**Build infrastructure:**
- GitHub Actions with matrix strategy
- Uses `cargo-zigbuild` or cross-compilation toolchains
- Publishes to npm via `@napi-rs/cli publish` command
- Separate npm scope for platform packages

### 6.2 Biome (Rome Successor)

**Architecture:**
- Entire tool (parser, linter, formatter) written in Rust
- Node.js bindings via napi-rs for programmatic API
- Also distributes a standalone Rust binary (no Node.js required)
- Dual distribution: npm package (napi-rs) + standalone binary (GitHub releases)

**Key learnings:**
- Biome demonstrates that a Rust CLI tool can have both a standalone binary AND a Node.js API
- The standalone binary is preferred for CI/CD (faster, no Node.js dependency)
- The Node.js API is used for IDE integrations and programmatic access
- Performance: Biome formats code 20-35x faster than Prettier

### 6.3 Rspack (ByteDance)

**Architecture:**
- Rust-based bundler with Webpack-compatible plugin API
- Node.js plugin system communicates with Rust core via napi-rs
- Hybrid architecture: Rust does the heavy lifting (module graph, code generation), JavaScript handles plugin execution

**Key learnings:**
- Demonstrates successful interop between Rust core and JavaScript plugin ecosystem
- The N-API boundary is where most complexity lives (callback management, thread safety)
- Performance: 5-10x faster than Webpack for large projects
- Uses `napi-rs` ThreadsafeFunction for calling back into JavaScript from Rust threads

### 6.4 Lightning CSS

**Architecture:**
- CSS parser and transformer written in Rust
- Node.js bindings via napi-rs (`lightningcss` npm package)
- Also available as WASM for browser use

**Key learnings:**
- Clean example of the "napi-rs for Node.js, WASM for browser" dual approach
- The napi-rs version is ~3x faster than the WASM version
- Small, focused API surface (parse, transform, minify) makes the binding layer thin
- Excellent TypeScript type generation from Rust structs

### 6.5 Oxc (oxlint)

**Architecture:**
- JavaScript toolchain written in Rust (parser, linter, formatter, transformer)
- Node.js bindings via napi-rs
- Also WASM for playground/browser

**Key learnings:**
- Oxc's parser is the fastest JavaScript parser available (~3x faster than SWC's parser)
- Demonstrates that Rust can match or exceed hand-tuned C/C++ performance
- Multi-tool bundling: parser + linter + formatter in one native addon
- Good example of TypeScript type generation from Rust AST types

### 6.6 Pattern Summary

All major Rust+Node.js projects follow the same pattern:

```
1. Core logic in Rust (pure Rust crate, no Node.js dependencies)
2. Thin napi-rs binding layer (derives JS types from Rust types)
3. TypeScript package with loader + type definitions
4. Platform-specific npm packages with prebuilt binaries
5. GitHub Actions CI matrix for cross-platform builds
6. Optional: WASM build for browser/portable use
```

---

## 7. Cross-Compilation Challenges and Distribution

### 7.1 Common Gotchas

#### 7.1.1 C/C++ Dependencies

If the Rust crate depends on C/C++ libraries (e.g., tree-sitter grammars, usearch), cross-compilation requires:
- Cross-compilation toolchain for C/C++ (not just Rust)
- `cc` crate must find the right cross-compiler
- Header files for the target platform
- Solution: Use `cargo-zigbuild` which bundles Zig as a cross-compiler (handles C/C++ cross-compilation seamlessly)

#### 7.1.2 OpenSSL / System Libraries

If any dependency links to system libraries (OpenSSL, zlib):
- These must be available for the target platform
- Solution: Prefer Rust-native alternatives (`rustls` instead of OpenSSL, `miniz_oxide` instead of zlib)
- For Cuervo CLI's native addon: avoid system library dependencies entirely

#### 7.1.3 macOS Universal Binaries

- Apple Silicon (ARM64) and Intel (x64) require separate builds
- Universal binaries (fat binaries) are possible via `lipo` but double the file size
- Solution: Distribute separate packages (the napi-rs standard approach)

#### 7.1.4 Linux glibc vs musl

- glibc binaries don't work on musl systems (Alpine Linux)
- musl binaries are more portable but may be slightly slower
- Solution: Build both variants (napi-rs supports this out of the box)

#### 7.1.5 Windows ARM64

- Growing market (Snapdragon laptops) but toolchain support is less mature
- Solution: Support x64 first (runs via emulation on ARM64 Windows), add native ARM64 later

#### 7.1.6 Node.js Version Compatibility

- N-API provides ABI stability across Node.js versions
- Target N-API v8 (Node.js 18+) for broad compatibility
- No need to build separate binaries per Node.js version (unlike old nan/v8 approach)

### 7.2 CI/CD Setup (GitHub Actions)

The standard napi-rs GitHub Actions workflow:

```yaml
name: Build and Publish Native Addon
on:
  push:
    tags: ['v*']

jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        settings:
          - host: macos-latest
            target: aarch64-apple-darwin
            build: |
              yarn build --target aarch64-apple-darwin
          - host: macos-latest
            target: x86_64-apple-darwin
            build: |
              yarn build --target x86_64-apple-darwin
          - host: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            build: |
              yarn build --target x86_64-unknown-linux-gnu --use-napi-cross
          - host: ubuntu-latest
            target: x86_64-unknown-linux-musl
            build: |
              yarn build --target x86_64-unknown-linux-musl -x
          - host: ubuntu-latest
            target: aarch64-unknown-linux-gnu
            build: |
              yarn build --target aarch64-unknown-linux-gnu --use-napi-cross
          - host: ubuntu-latest
            target: aarch64-unknown-linux-musl
            build: |
              yarn build --target aarch64-unknown-linux-musl -x
          - host: windows-latest
            target: x86_64-pc-windows-msvc
            build: |
              yarn build --target x86_64-pc-windows-msvc

    runs-on: ${{ matrix.settings.host }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.settings.target }}
      - name: Install ziglang (for cross-compilation)
        if: matrix.settings.host == 'ubuntu-latest'
        uses: goto-bus-stop/setup-zig@v2
      - name: Build
        run: ${{ matrix.settings.build }}
      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: bindings-${{ matrix.settings.target }}
          path: '*.node'

  publish:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with:
          path: artifacts
      - name: Move artifacts
        run: npx @napi-rs/cli artifacts
      - name: Publish to npm
        run: |
          npm config set //registry.npmjs.org/:_authToken=$NPM_TOKEN
          npx @napi-rs/cli publish --access public
        env:
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
```

**Build times (approximate):**

| Target | Build Time | Binary Size |
|--------|-----------|-------------|
| macOS ARM64 | ~3-5 min | ~15-30 MB |
| macOS x64 | ~3-5 min | ~15-30 MB |
| Linux x64 glibc | ~4-7 min | ~15-30 MB |
| Linux x64 musl | ~5-8 min | ~15-30 MB |
| Linux ARM64 glibc | ~5-8 min | ~15-30 MB |
| Windows x64 | ~5-8 min | ~15-30 MB |
| **Total (parallel)** | **~8-10 min** | ~100-180 MB total |

The `@napi-rs/cli` handles all the npm package scaffolding, artifact collection, and multi-package publishing in a single command.

### 7.3 Local Development Experience

For developers contributing to Cuervo CLI:

```bash
# Prerequisites
# - Rust toolchain (rustup)
# - Node.js 20+
# - Yarn/npm

# Build native addon for current platform only
yarn build:native

# This compiles Rust code and produces a .node file
# for the developer's current OS/arch only
# Build time: ~30-60 seconds (incremental: ~5-10 seconds)

# Run tests with native addon
yarn test

# For CI: build for all platforms
yarn build:native --target aarch64-apple-darwin
```

The local development loop is fast because Rust incremental compilation + napi-rs only recompiles changed code.

---

## 8. napi-rs vs WASM: When to Use Each

### 8.1 Comparison Table

| Dimension | napi-rs (Native) | WASM (wasm-pack) |
|-----------|------------------|-------------------|
| **Performance** | Full native speed | 30-60% slower than native |
| **Startup overhead** | ~1-5 ms (shared library load) | ~10-50 ms (WASM instantiation) |
| **Memory access** | Full system memory | Sandboxed linear memory |
| **File system access** | Direct (via Rust std) | None (must call back to JS) |
| **Thread support** | Full (std::thread, rayon) | Limited (SharedArrayBuffer) |
| **Distribution** | Platform-specific binaries | Single .wasm file, universal |
| **Binary size** | 15-30 MB per platform | 5-15 MB (one file for all) |
| **Build complexity** | Higher (cross-compilation) | Lower (single target) |
| **Browser compatible** | No | Yes |
| **npm install friction** | Low (optionalDeps pattern) | None |
| **Debugging** | Platform-native tools | WASM debugger (less mature) |
| **C/C++ interop** | Full | Limited (must compile to WASM) |
| **Node.js version dep** | N-API v8+ (Node 18+) | None (runs in any JS runtime) |

### 8.2 When to Use napi-rs

Use napi-rs when:
- **Performance is critical** and the 30-60% WASM overhead matters
- **Threading is needed** (e.g., parallel AST parsing, parallel vector indexing)
- **Memory efficiency matters** (WASM has memory copying overhead at boundaries)
- **C/C++ libraries are involved** (tree-sitter, usearch)
- **The target is CLI/server only** (no browser requirement)
- **File system access is needed** from Rust (reading files directly)
- **The project already has a cross-platform CI/CD pipeline**

**This describes Cuervo CLI exactly.** A CLI tool running on Node.js, needing maximum performance for AST parsing, vector search, tokenization, and regex scanning.

### 8.3 When to Use WASM

Use WASM when:
- **Browser compatibility is required** (same code in Node.js and browser)
- **Distribution simplicity is paramount** (single file, no native compilation)
- **The computation is self-contained** (no file system, no threads needed)
- **Performance is "good enough"** (simple transformations, small inputs)
- **The target audience cannot install native dependencies** (e.g., serverless environments with limited build support)

Example: If Cuervo CLI had a web playground or browser-based IDE component, WASM would be the right choice for that distribution target.

### 8.4 Hybrid Approach (Recommended for Cuervo CLI)

The optimal strategy is to structure the Rust code so it can be compiled to both targets:

```
cuervo-native/
├── crates/
│   ├── core/              # Pure Rust logic (no I/O dependencies)
│   │   ├── ast_parser/    # Tree-sitter wrapping
│   │   ├── vector_search/ # HNSW implementation
│   │   ├── tokenizer/     # Token counting
│   │   └── scanner/       # Regex/PII scanning
│   ├── napi/              # napi-rs bindings (Node.js native)
│   │   └── src/lib.rs     # #[napi] exports
│   └── wasm/              # wasm-bindgen exports (for browser/fallback)
│       └── src/lib.rs     # #[wasm_bindgen] exports
```

The `core` crate contains all business logic with no I/O dependencies. The `napi` and `wasm` crates are thin binding layers. This allows:
1. Primary distribution: napi-rs for CLI users (maximum performance)
2. Fallback: WASM for environments where native binaries can't be loaded
3. Future: Browser-based tools reuse the same Rust core via WASM

---

## 9. Concrete Recommendations for Cuervo CLI

### 9.1 Architecture Decision

**Adopt the TypeScript + Rust (napi-rs) hybrid pattern** for performance-critical components. The Rust code should be contained in a single native addon package (`@cuervo/native`) that bundles:

1. **AST Parser** (tree-sitter + grammars for top 15 languages)
2. **Vector Search** (HNSW index, memory-mapped persistence)
3. **Token Counter** (tiktoken-rs + HuggingFace tokenizers)
4. **PII Scanner** (Rust regex RegexSet with 30-50 patterns)

All other components (REPL, agent loop, model providers, git operations, config) remain in TypeScript.

### 9.2 Package Structure

```
@cuervo/cli                           # Main CLI package (TypeScript)
@cuervo/native                        # Rust native addon (JS loader + types)
@cuervo/native-darwin-arm64           # macOS ARM64 binary
@cuervo/native-darwin-x64            # macOS Intel binary
@cuervo/native-linux-x64-gnu         # Linux x64 (glibc)
@cuervo/native-linux-x64-musl        # Linux x64 (musl/Alpine)
@cuervo/native-linux-arm64-gnu       # Linux ARM64
@cuervo/native-win32-x64-msvc        # Windows x64
```

### 9.3 API Surface (TypeScript Types)

```typescript
// @cuervo/native

// --- AST Parsing ---
export function parseCode(source: string, language: string): AstHandle;
export function getChunks(handle: AstHandle, strategy: ChunkStrategy): CodeChunk[];
export function queryAst(handle: AstHandle, pattern: string): QueryMatch[];
export function freeAst(handle: AstHandle): void;

export interface CodeChunk {
  content: string;
  startLine: number;
  endLine: number;
  startByte: number;
  endByte: number;
  kind: 'function' | 'class' | 'method' | 'interface' | 'module' | 'block' | 'comment';
  name?: string;
  language: string;
}

export type ChunkStrategy = {
  maxChunkSize: number;       // Max tokens per chunk
  overlapLines: number;        // Overlap between adjacent chunks
  respectBoundaries: boolean;  // Don't split mid-function
};

// --- Vector Search ---
export class VectorIndex {
  constructor(dimensions: number, metric: 'cosine' | 'l2' | 'ip');
  add(id: number, vector: Float32Array, label?: string): void;
  addBatch(ids: Uint32Array, vectors: Float32Array): void;
  search(query: Float32Array, k: number): SearchResult[];
  remove(id: number): boolean;
  save(path: string): void;
  static load(path: string): VectorIndex;
  get size(): number;
}

export interface SearchResult {
  id: number;
  distance: number;
  label?: string;
}

// --- Tokenization ---
export function countTokens(text: string, provider: string, model: string): TokenCount;
export function countTokensBatch(texts: string[], provider: string, model: string): number[];

export interface TokenCount {
  count: number;
  encoding: string;
  isExact: boolean;  // false = estimate
}

// --- PII Scanning ---
export function scanPii(text: string, patterns?: string[]): PiiMatch[];
export function scanPiiBatch(texts: string[]): PiiMatch[][];
export function addPiiPattern(name: string, pattern: string): void;

export interface PiiMatch {
  patternName: string;
  matchText: string;
  startOffset: number;
  endOffset: number;
  line: number;
  column: number;
  severity: 'critical' | 'high' | 'medium' | 'low';
}
```

### 9.4 Development Phases

| Phase | Component | Effort | Priority |
|-------|-----------|--------|----------|
| **MVP-0 (Sem 1-8)** | Token counter (Rust napi-rs) | 1-2 weeks | Needed for context management |
| **MVP-0 (Sem 1-8)** | PII scanner (Rust napi-rs) | 1 week | Needed for privacy pipeline |
| **MVP-1 (Sem 9-16)** | AST parser (tree-sitter napi-rs) | 2-3 weeks | Needed for RAG chunking |
| **MVP-1 (Sem 9-16)** | Vector search (usearch or custom HNSW) | 1-2 weeks | Needed for semantic search |
| **Beta (Sem 17-32)** | Single `@cuervo/native` bundle | 2-3 weeks | Unify all Rust components |
| **Beta (Sem 17-32)** | WASM fallback build | 1-2 weeks | For environments without native support |

### 9.5 Performance Budget

Expected performance improvements over pure JavaScript/WASM alternatives:

| Component | JS/WASM Baseline | Rust napi-rs Target | Improvement |
|-----------|-----------------|--------------------|----|
| Parse 10K TypeScript files | ~30-60s (WASM tree-sitter) | ~3-8s (native tree-sitter) | 5-10x |
| Count tokens (100 KB text) | ~50-200ms (pure JS) | ~2-5ms (tiktoken-rs) | 20-100x |
| Vector search (50K vectors, top-10) | ~5-15ms (JS HNSW) | ~0.3ms (Rust HNSW) | 15-50x |
| PII scan (100 KB, 20 patterns) | ~10-50ms (JS RegExp) | ~0.5-2ms (Rust RegexSet) | 10-25x |
| Startup overhead (load native addon) | 0ms (JS) / 10-50ms (WASM) | ~1-5ms (dlopen) | Negligible |

### 9.6 Binary Size Budget

| Component | Estimated Size (per platform) |
|-----------|------------------------------|
| Tree-sitter core + 15 grammars | ~3-5 MB |
| HNSW vector search | ~0.5-1 MB |
| Tokenizer (tiktoken + HF) | ~2-3 MB |
| Regex scanner | ~0.5-1 MB |
| Rust runtime + overhead | ~1-2 MB |
| **Total (uncompressed)** | **~7-12 MB** |
| **Total (gzip compressed for npm)** | **~3-5 MB** |

This is well within acceptable limits for a CLI tool (SWC ships 20-50 MB per platform).

### 9.7 Risk Mitigation

| Risk | Mitigation |
|------|------------|
| Rust build adds complexity to dev setup | Provide pre-built binaries; only Rust contributors need Rust toolchain |
| Cross-compilation failures on CI | Use napi-rs's battle-tested GitHub Actions templates |
| Native addon fails to load on exotic platform | Implement JS fallback (slower) for all Rust functions |
| Rust dependency CVE | Use `cargo-audit` in CI; minimal dependency tree |
| Binary size too large | Use `cargo build --release` with LTO and `strip`; consider `wasm-opt` for WASM |
| Developer experience friction | `yarn build:native` for single-platform build; no Rust needed for TS-only changes |

---

## 10. Appendix: Benchmark Data and Version Reference

### 10.1 Version Reference (as of February 2026)

| Technology | Version | Status |
|------------|---------|--------|
| napi-rs (napi crate) | ~2.16.x | Stable |
| napi-rs CLI (@napi-rs/cli) | ~3.0.x | Stable |
| Tree-sitter (core) | ~0.24.x | Stable (pre-1.0 but widely deployed) |
| tree-sitter (npm, native) | ~0.21.x | Stable |
| web-tree-sitter (npm, WASM) | ~0.24.x | Stable |
| usearch (npm) | ~2.x | Stable |
| hnswlib-node (npm) | ~3.x | Stable |
| tiktoken-rs (crate) | ~0.5.x+ | Stable |
| tokenizers (HF crate) | ~0.20.x+ | Stable |
| regex (Rust crate) | ~1.10.x+ | Stable |
| Rust edition | 2021 | Stable (2024 edition emerging) |
| Node.js (minimum) | 18 LTS | Active LTS |
| Node.js (recommended) | 20 LTS / 22 | Current LTS |

### 10.2 Benchmark Methodology Notes

The benchmarks cited in this report are based on:
- Published benchmarks from project maintainers and third-party evaluations
- Reasonable extrapolations from known performance characteristics
- Hardware baseline: Apple M2/M3 or equivalent modern x64 (AMD Zen 4 / Intel 13th gen)
- All benchmarks assume release builds with optimizations (`--release`, LTO enabled)

For Cuervo CLI, we recommend running validation benchmarks during the Experiment phase (E2 in the repriorized roadmap) to confirm these numbers on the target hardware.

### 10.3 HNSW Parameter Tuning Guide

For the vector search index, HNSW parameters significantly affect the quality/speed tradeoff:

| Parameter | Description | Recommended Value | Impact |
|-----------|-------------|-------------------|--------|
| `M` | Max connections per layer | 16-32 | Higher = better recall, more memory |
| `ef_construction` | Build-time search width | 100-200 | Higher = better index quality, slower build |
| `ef_search` | Query-time search width | 50-100 | Higher = better recall, slower query |

For Cuervo CLI's scale (10K-100K vectors, 768-1536 dims):

```
M = 16           # Good balance for this scale
ef_construction = 128  # High quality index
ef_search = 64        # Fast queries with >95% recall@10

Expected results:
- Build: ~3-7s for 50K vectors
- Query: ~0.3-0.5ms for top-10
- Memory: ~150-300MB for 50K x 768-dim vectors
- Recall@10: >0.95
```

### 10.4 Tokenizer Encoding Reference

| Provider | Model | Encoding | Rust Library |
|----------|-------|----------|-------------|
| OpenAI | GPT-4, GPT-4o | cl100k_base | tiktoken-rs |
| OpenAI | GPT-4o (newer) | o200k_base | tiktoken-rs |
| OpenAI | o1, o3 | o200k_base | tiktoken-rs |
| Anthropic | Claude 3.x, 4.x | Custom (unpublished) | Estimate via cl100k_base proxy |
| Google | Gemini 2.0 | SentencePiece variant | HF tokenizers (if model published) |
| Meta | Llama 3.x, 4.x | SentencePiece BPE | HF tokenizers |
| Mistral | Mistral, Mixtral | SentencePiece BPE | HF tokenizers |
| DeepSeek | DeepSeek V3 | Custom BPE | HF tokenizers |
| Alibaba | Qwen 2.5 | Custom BPE | HF tokenizers |

### 10.5 PII Pattern Categories (Default Set)

| Category | Pattern Count | Examples | Severity |
|----------|--------------|---------|----------|
| API Keys & Tokens | ~15 | OpenAI, AWS, GitHub, Stripe, Slack | Critical |
| Credentials | ~5 | Passwords in config, basic auth | Critical |
| Personal Identifiers | ~5 | Email, phone, SSN | High |
| Network | ~3 | IPv4, IPv6, MAC address | Medium |
| Financial | ~3 | Credit card, IBAN, routing number | High |
| Custom (user-defined) | Variable | Project-specific patterns | Configurable |

---

## References

### napi-rs
- Repository: https://github.com/napi-rs/napi-rs
- Documentation: https://napi.rs
- Examples: https://github.com/napi-rs/napi-rs/tree/main/examples

### SWC (Reference Implementation)
- Repository: https://github.com/swc-project/swc
- npm: https://www.npmjs.com/package/@swc/core
- Build infrastructure: https://github.com/swc-project/swc/tree/main/.github/workflows

### Tree-sitter
- Repository: https://github.com/tree-sitter/tree-sitter
- Node.js bindings: https://github.com/tree-sitter/node-tree-sitter
- Grammar directory: https://tree-sitter.github.io/tree-sitter/#parsers

### USearch
- Repository: https://github.com/unum-cloud/usearch
- npm: https://www.npmjs.com/package/usearch

### tiktoken-rs
- Repository: https://github.com/zurawiki/tiktoken-rs
- crates.io: https://crates.io/crates/tiktoken-rs

### HuggingFace Tokenizers
- Repository: https://github.com/huggingface/tokenizers
- Rust crate: https://crates.io/crates/tokenizers

### Rust Regex
- Repository: https://github.com/rust-lang/regex
- Documentation: https://docs.rs/regex

### Real-World Projects
- Biome: https://github.com/biomejs/biome
- Rspack: https://github.com/web-infra-dev/rspack
- Lightning CSS: https://github.com/parcel-bundler/lightningcss
- Oxc: https://github.com/oxc-project/oxc
- Rolldown: https://github.com/rolldown/rolldown

---

*Document generated February 6, 2026. Subject to update as technologies evolve.*
