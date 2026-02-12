# Rust como Capa de Performance — Investigación Técnica

**Proyecto:** Cuervo CLI
**Versión:** 1.0
**Fecha:** 6 de febrero de 2026

---

## Resumen Ejecutivo

Esta investigación valida la integración de Rust como capa de performance para operaciones computacionalmente intensivas en Cuervo CLI. Se analizan las tecnologías de integración (napi-rs), se comparan 8 bases de datos vectoriales embebidas, y se propone una arquitectura híbrida TypeScript + Rust con plan de implementación concreto.

**Conclusiones clave:**
1. **napi-rs** es el estándar de facto para integrar Rust con Node.js — usado por SWC, Biome, Rspack, Lightning CSS y Oxc
2. **LanceDB** (Rust core + napi-rs SDK) es la mejor opción para vector store embebido
3. El patrón "TypeScript para orquestación, Rust para hot paths" reduce latencia 5-50x en operaciones críticas
4. El ecosistema Cuervo ya valida este approach: cuervo-video-intelligence logra 60x vs Python con Rust

---

## 1. napi-rs — Integración Rust ↔ Node.js

### 1.1 Estado de la Tecnología

**napi-rs** (v2.x / v3.x) es un framework para construir native addons de Node.js precompilados en Rust. Usa la Node-API (N-API) estable, que garantiza compatibilidad ABI entre versiones de Node.js.

| Aspecto | Detalle |
|---------|---------|
| **Versión** | napi-rs v2.16+ / @napi-rs/cli v3.x |
| **N-API level** | N-API v9 (Node.js 18+), full async/threadsafe support |
| **Plataformas** | macOS ARM64, macOS x64, Linux x64 (glibc/musl), Windows x64, FreeBSD |
| **Distribución** | Prebuilt binaries via platform-specific npm packages |
| **Overhead FFI** | ~10-50ns per call (negligible) |
| **Async support** | Tokio runtime integration, non-blocking I/O |
| **Thread safety** | ThreadsafeFunction para callbacks JS desde Rust threads |

### 1.2 Proyectos de Referencia

| Proyecto | Uso de Rust (napi-rs) | Scale | Impacto |
|----------|----------------------|-------|---------|
| **SWC** (Vercel) | Transpiler + bundler completo | 40K+ GitHub stars | 20-70x más rápido que Babel |
| **Biome** (ex-Rome) | Linter + formatter | 16K+ stars | 100x más rápido que ESLint + Prettier |
| **Rspack** | Webpack-compatible bundler | 10K+ stars | 5-10x más rápido que Webpack |
| **Lightning CSS** | CSS parser + transformer | 7K+ stars | 100x más rápido que PostCSS |
| **Oxc** (oxlint) | JS/TS toolchain completo | 12K+ stars | 50-100x más rápido que ESLint |
| **Turbopack** | Next.js bundler | Vercel-backed | Rust core + napi-rs bridge |
| **tree-sitter** | Parser generator | GitHub-adopted | AST parsing para code intelligence |

### 1.3 Patrón de Distribución npm

```
@cuervo/cli                    # Main package (TypeScript)
├── @cuervo/native-darwin-arm64  # macOS Apple Silicon binary
├── @cuervo/native-darwin-x64    # macOS Intel binary
├── @cuervo/native-linux-x64-gnu # Linux glibc binary
├── @cuervo/native-linux-x64-musl # Linux musl (Alpine) binary
├── @cuervo/native-win32-x64-msvc # Windows binary
└── @cuervo/native              # Meta-package con optionalDependencies
```

El package principal detecta la plataforma en `postinstall` y carga el binary correcto. Si no hay binary prebuilt, cae a compilación local (requiere Rust toolchain).

### 1.4 CI/CD para Cross-Compilation

```yaml
# GitHub Actions matrix (simplificado)
strategy:
  matrix:
    include:
      - target: x86_64-apple-darwin
        os: macos-latest
      - target: aarch64-apple-darwin
        os: macos-latest
      - target: x86_64-unknown-linux-gnu
        os: ubuntu-latest
      - target: x86_64-pc-windows-msvc
        os: windows-latest

steps:
  - uses: actions/checkout@v4
  - uses: actions-rs/toolchain@v1
    with:
      target: ${{ matrix.target }}
  - run: cd native && cargo build --release --target ${{ matrix.target }}
  - uses: napi-rs/napi-action@v1    # Publish platform package to npm
```

**Tiempo de build CI:** ~5-10 minutos por plataforma (parallelizable).

### 1.5 napi-rs vs WASM

| Criterio | napi-rs (Native) | wasm-pack (WASM) |
|----------|-----------------|-----------------|
| **Performance** | Nativo — full SIMD, threads | ~2-5x más lento, SIMD limitado |
| **Threading** | Multi-thread nativo | Single-thread (sin SharedArrayBuffer) |
| **Memory** | Acceso directo, zero-copy | Copia entre heaps JS/WASM |
| **Distribución** | Binary por plataforma | Universal .wasm file |
| **Tamaño** | ~5-20MB por plataforma | ~1-5MB universal |
| **Startup** | ~1-5ms (dlopen) | ~10-50ms (compile + instantiate) |
| **Debugging** | Herramientas nativas (lldb, perf) | Limitado |

**Decisión:** napi-rs para Cuervo CLI. Las operaciones target (vector search, AST parsing, regex) son CPU-bound y se benefician enormemente de SIMD y multi-threading. WASM es insuficiente.

---

## 2. Componentes Rust Propuestos

### 2.1 Mapa de Hot Paths

```
┌──────────────────────────────────────────────────────────┐
│                CUERVO CLI — PERFORMANCE MAP               │
├──────────────────────────────────────────────────────────┤
│                                                           │
│  HOT PATH (Rust via napi-rs)         COLD PATH (TypeScript)
│  ─────────────────────────           ────────────────────
│  ● Tree-sitter AST parsing          ○ REPL / UI rendering
│  ● Vector index (HNSW search)       ○ Model API calls
│  ● Fast glob (file discovery)       ○ Agent orchestration
│  ● Fast grep (content search)       ○ Config management
│  ● Tokenizer (multi-provider)       ○ Git operations
│  ● PII regex scanner                ○ Session management
│  ● Diff computation                 ○ Permission prompts
│                                      ○ Plugin loading
│  Criterio: >1000 invocaciones/sesión ○ Auth client
│  o >100ms en JS puro                ○ Event bus
│                                                           │
└──────────────────────────────────────────────────────────┘
```

### 2.2 Módulos Rust Detallados

#### `scanner.rs` — Fast File Discovery & Search

**Problema:** `node-glob` y `grep` en JS son 10-20x más lentos que alternativas nativas para codebases de 100K archivos.

**Implementación:**
- `globwalk` crate para file pattern matching (mismas semantics que ripgrep)
- `grep-regex` / `regex` crate para content search
- `ignore` crate para .gitignore-aware traversal (misma base que ripgrep)
- Streaming results via napi-rs `AsyncTask` para no bloquear el event loop

**Benchmarks estimados (100K archivos):**

| Operación | Node.js (glob/grep) | Rust (napi-rs) | Speedup |
|-----------|---------------------|----------------|---------|
| Glob `**/*.ts` | ~2.0s | ~100ms | 20x |
| Grep en 10K archivos | ~3.5s | ~200ms | 17x |
| .gitignore-aware walk | ~1.5s | ~80ms | 19x |

**API expuesta:**

```typescript
// @cuervo/native
export function fastGlob(pattern: string, options: GlobOptions): Promise<string[]>;
export function fastGrep(pattern: string, paths: string[], options: GrepOptions): Promise<GrepMatch[]>;
export function walkDirectory(root: string, options: WalkOptions): Promise<FileEntry[]>;
```

#### `treesitter.rs` — AST Parsing & Code Chunking

**Problema:** El chunking inteligente de código para RAG requiere AST parsing. Tree-sitter está escrito en C/Rust y es el estándar para esto.

**Implementación:**
- `tree-sitter` Rust crate + grammars para 15+ lenguajes
- Chunking strategy: funciones, clases, interfaces, módulos
- Output: chunks con metadata (tipo, nombre, rango de líneas, lenguaje)

**Benchmarks estimados:**

| Operación | JS (tree-sitter WASM) | Rust (napi-rs) | Speedup |
|-----------|----------------------|----------------|---------|
| Parse 1 archivo (1K LOC) | ~15ms | ~2ms | 7x |
| Parse + chunk 1K archivos | ~20s | ~3s | 7x |
| Incremental re-parse | ~5ms | ~0.5ms | 10x |

**API expuesta:**

```typescript
export interface CodeChunk {
  filePath: string;
  language: string;
  type: 'function' | 'class' | 'interface' | 'module' | 'block';
  name: string;
  startLine: number;
  endLine: number;
  content: string;
  hash: string;
}

export function parseFile(path: string, language: string): Promise<CodeChunk[]>;
export function parseFiles(paths: string[]): Promise<CodeChunk[]>;  // Parallel
export function supportedLanguages(): string[];
```

#### `tokenizer.rs` — Multi-Provider Token Counting

**Problema:** Cada provider usa tokenizers diferentes. Token counting preciso es crítico para context management y cost estimation.

**Implementación:**
- `tiktoken-rs` para OpenAI models (cl100k_base, o200k_base)
- Estimación por caracteres para Claude (Anthropic no publica tokenizer exacto)
- `tokenizers` crate (HuggingFace) para modelos locales
- Cache de tokenizer instances para evitar re-inicialización

**Benchmarks estimados:**

| Operación | JS (tiktoken WASM) | Rust (napi-rs) | Speedup |
|-----------|---------------------|----------------|---------|
| Count 10K tokens | ~5ms | ~0.1ms | 50x |
| Count 100K tokens | ~50ms | ~1ms | 50x |
| Encode + decode | ~8ms | ~0.2ms | 40x |

**API expuesta:**

```typescript
export function countTokens(text: string, provider: 'openai' | 'anthropic' | 'local'): number;
export function truncateToTokens(text: string, maxTokens: number, provider: string): string;
export function estimateCost(text: string, model: string): { inputCost: number; tokens: number };
```

#### `pii_scanner.rs` — PII Detection via Regex

**Problema:** El pipeline PII necesita regex matching rápido sobre todo el código antes de enviar a cloud APIs.

**Implementación:**
- `regex` crate (el motor regex más rápido disponible, usado por ripgrep)
- Patrones precompilados para: emails, IPs, phones, SSNs, credit cards, API keys, tokens
- `aho-corasick` para multi-pattern matching simultáneo

**API expuesta:**

```typescript
export interface PIIMatch {
  type: 'email' | 'ip' | 'phone' | 'ssn' | 'credit_card' | 'api_key' | 'token';
  start: number;
  end: number;
  redacted: string;  // e.g., "[EMAIL]"
}

export function scanPII(text: string): PIIMatch[];
export function redactPII(text: string): string;
```

---

## 3. Vector Store — Comparativa de Opciones

### 3.1 Opciones Evaluadas

| Opción | Lenguaje | Madurez | Embebible | ANN | Persistencia | Node.js bindings |
|--------|---------|---------|-----------|-----|-------------|-----------------|
| **LanceDB** | Rust | Beta (v0.5-0.9) | Sí | IVF-PQ + DiskANN | Nativa (Lance format) | Oficial (napi-rs) |
| **USearch** | C++ | Producción (v2.x) | Sí | HNSW | Nativa + mmap | Oficial npm |
| **Hnswlib-node** | C++ | Estable (v3.x) | Sí | HNSW | Archivo binario | Oficial npm |
| **Qdrant** | Rust | Producción (v1.12+) | No (server) | HNSW modificado | WAL + segments | Solo HTTP client |
| **sqlite-vec** | C | Pre-1.0 | Sí (SQLite ext) | Brute force | SQLite tables | Extension load |
| **Hora** | Rust | Abandonado | Sí | HNSW/IVF | Ninguna | Ninguno |
| **instant-distance** | Rust | Minimal | Sí | HNSW | Serde manual | Ninguno |
| **Voy** | Rust→WASM | Experimental | Sí | k-d tree | Ninguna | npm (WASM) |

### 3.2 Performance a 100K vectores x 768 dimensiones

| Opción | Build index | Search p50 | Search p95 | Memoria (RSS) |
|--------|-----------|-----------|-----------|--------------|
| **LanceDB** | ~5-15s | ~1-3ms | ~5ms | ~50-100MB (mmap) |
| **USearch** | ~2-5s | ~0.1-0.3ms | ~0.5ms | ~50-100MB (mmap) |
| **Hnswlib-node** | ~3-8s | ~0.5-1ms | ~2ms | ~330-350MB (full load) |
| **sqlite-vec** | ~10-30s | ~50-100ms | ~200ms | ~300MB+ |
| **Voy (WASM)** | ~30-60s | ~5-10ms | ~20ms | ~320MB |

### 3.3 Recomendación: LanceDB (Primario) + USearch (Fallback)

**LanceDB es la mejor opción** por estas razones:

1. **SDK TypeScript oficial con napi-rs** — alineación perfecta con stack Cuervo
2. **True embedded DB** — persistencia automática, sin paso de "save"
3. **Metadata + vectores juntos** — elimina necesidad de SQLite paralelo para metadata de chunks
4. **Hybrid search integrado** — Tantivy-based FTS para búsqueda keyword + semántica
5. **Incremental updates** — append/update/delete nativos (resuelve hallazgo #9 de la revisión)
6. **Memory-mapped I/O** — ~50-100MB RSS vs ~330MB de Hnswlib
7. **Versioned data** — snapshots nativos alineados con Propuesta P4 (Snapshot & Rollback)

**USearch como fallback** si LanceDB presenta issues de estabilidad (pre-1.0):
- HNSW más rápido disponible (~0.1ms search)
- Bindings Node.js probados en producción
- Requiere SQLite paralelo para metadata

### 3.4 Arquitectura de Vector Store Propuesta

```
┌─────────────────────────────────────────────────────────┐
│               VECTOR STORE (infrastructure/storage/)     │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  VectorStore.ts                                          │
│  ├── uses @lancedb/lancedb (napi-rs → Rust)             │
│  ├── Database: ~/.cuervo/projects/<hash>/embeddings/     │
│  │                                                       │
│  │   embeddings table:                                   │
│  │   ┌──────────────────────────────────────────┐       │
│  │   │ id: string (UUID)                         │       │
│  │   │ file_path: string                         │       │
│  │   │ chunk_type: string (function|class|module)│       │
│  │   │ chunk_name: string                        │       │
│  │   │ start_line: int                           │       │
│  │   │ end_line: int                             │       │
│  │   │ content_hash: string (SHA-256)            │       │
│  │   │ content_text: string (FTS indexed)        │       │
│  │   │ vector: float32[768]  (ANN indexed)       │       │
│  │   │ updated_at: timestamp                     │       │
│  │   └──────────────────────────────────────────┘       │
│  │                                                       │
│  ├── ANN index: IVF-PQ (default) or DiskANN             │
│  ├── FTS index: Tantivy on content_text                  │
│  └── Hybrid search: vector + FTS + re-ranking            │
│                                                          │
│  API:                                                    │
│  ├── addChunks(chunks: CodeChunk[]): Promise<void>      │
│  ├── search(vector, k): Promise<SearchResult[]>          │
│  ├── hybridSearch(query, vector, k): Promise<Result[]>   │
│  ├── removeByPath(filePath): Promise<void>               │
│  ├── invalidateByHash(hash): Promise<string[]>           │
│  └── getStats(): Promise<IndexStats>                     │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

---

## 4. Estructura de Proyecto con Rust

### 4.1 Directorio `native/`

```
cuervo-cli/
├── src/                         # TypeScript (orquestación, UI, lógica)
│   └── ...
│
├── native/                      # Rust (performance layer)
│   ├── Cargo.toml               # Workspace raíz
│   ├── Cargo.lock
│   ├── .cargo/
│   │   └── config.toml          # Cross-compilation settings
│   ├── src/
│   │   ├── lib.rs               # napi-rs entry point + re-exports
│   │   ├── scanner.rs           # Fast glob + grep + directory walk
│   │   ├── treesitter.rs        # AST parsing + code chunking
│   │   ├── tokenizer.rs         # Multi-provider token counting
│   │   └── pii.rs               # PII regex detection + redaction
│   ├── build.rs                 # Build script (tree-sitter grammars)
│   └── __tests__/
│       ├── scanner.spec.ts      # Integration tests (TS → Rust)
│       ├── treesitter.spec.ts
│       └── tokenizer.spec.ts
│
├── npm/                         # Platform-specific binary packages
│   ├── darwin-arm64/
│   │   └── package.json         # @cuervo/native-darwin-arm64
│   ├── darwin-x64/
│   │   └── package.json         # @cuervo/native-darwin-x64
│   ├── linux-x64-gnu/
│   │   └── package.json         # @cuervo/native-linux-x64-gnu
│   └── win32-x64-msvc/
│       └── package.json         # @cuervo/native-win32-x64-msvc
│
├── package.json                 # Main package
├── tsconfig.json
└── vitest.config.ts
```

### 4.2 Cargo.toml

```toml
[package]
name = "cuervo-native"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
napi = { version = "2", features = ["async", "napi9"] }
napi-derive = "2"

# Scanner
globwalk = "0.9"
ignore = "0.4"          # .gitignore-aware walk (from ripgrep)
grep-regex = "0.1"

# Tree-sitter
tree-sitter = "0.24"
tree-sitter-typescript = "0.23"
tree-sitter-javascript = "0.23"
tree-sitter-python = "0.23"
tree-sitter-rust = "0.23"
tree-sitter-go = "0.23"

# Tokenizer
tiktoken-rs = "0.6"
tokenizers = "0.20"

# PII Scanner
regex = "1"
aho-corasick = "1"

# Utilities
rayon = "1"             # Parallel iteration
sha2 = "0.10"           # Content hashing

[build-dependencies]
napi-build = "2"

[profile.release]
lto = true
codegen-units = 1
strip = true
```

---

## 5. Plan de Integración por Fase

### MVP-1 (Semanas 9-16): `scanner.rs`

**Justificación:** El scanner (glob + grep) es el módulo con mayor impacto en UX inmediato. Cada búsqueda en codebase se beneficia.

| Semana | Entregable |
|--------|-----------|
| 12 | Scaffolding napi-rs + CI cross-compilation |
| 13 | `scanner.rs` implementado y testeado |
| 14 | Integración en SearchEngine.ts + fallback JS |

**Binary size estimado:** ~5MB por plataforma.

### Beta (Semanas 17-32): `treesitter.rs` + `tokenizer.rs` + LanceDB

| Semana | Entregable |
|--------|-----------|
| 18-19 | `treesitter.rs` — AST parsing + chunking para 10+ lenguajes |
| 20 | Integración Tree-sitter → LanceDB pipeline (indexación) |
| 21-22 | `tokenizer.rs` — token counting multi-provider |
| 24 | `pii.rs` — regex scanner para PII detection |

**Binary size estimado acumulado:** ~15-20MB por plataforma.

### GA (Semanas 33-48): Optimización

| Semana | Entregable |
|--------|-----------|
| 34 | Performance benchmarks formales vs competencia |
| 36 | Quantización de vectores (f16) para reducir memoria |
| 38 | Incremental re-indexing optimizado |

---

## 6. Riesgos y Mitigaciones

| Riesgo | Probabilidad | Impacto | Mitigación |
|--------|-------------|---------|-----------|
| Cross-compilation falla en alguna plataforma | Media | Alto | CI matrix testing, fallback a JS puro |
| LanceDB breaking changes (pre-1.0) | Media | Medio | Pin versión, USearch como fallback |
| Rust compilation lenta en CI | Baja | Bajo | Cache de Cargo dependencies, sccache |
| Usuarios sin binary prebuilt | Baja | Medio | Fallback a JS puro + mensaje claro |
| napi-rs incompatibilidad con Node.js futuro | Muy baja | Alto | N-API es ABI stable, garantizado por Node.js |

### Fallback Strategy

**Cada módulo Rust tiene un fallback en TypeScript puro:**

```typescript
// infrastructure/tools/SearchEngine.ts
let nativeSearch: typeof import('@cuervo/native') | null = null;

try {
  nativeSearch = require('@cuervo/native');
} catch {
  // Native module not available — fallback to JS
  console.warn('Native module not found. Using JS fallback (slower).');
}

export async function glob(pattern: string, options: GlobOptions) {
  if (nativeSearch) {
    return nativeSearch.fastGlob(pattern, options);  // ~100ms
  }
  return jsGlob(pattern, options);  // ~2000ms
}
```

Esto garantiza que Cuervo CLI **siempre funciona**, incluso sin los binarios Rust. La experiencia es degradada pero funcional.

---

## 7. Impacto en Performance Targets

| Métrica (RNF) | Target Original | Con Rust | Notas |
|---------------|----------------|---------|-------|
| RNF-001: TTFT autocompletado | <200ms | <200ms | Tokenizer Rust elimina overhead |
| RNF-003: Búsqueda en codebase | <100ms p50 | <5ms p50 | LanceDB HNSW + Rust scanner |
| RNF-004: Startup time | <500ms | ~600ms | +100ms para dlopen nativo, compensado con init lazy |
| RNF-005: Memoria idle | <100MB | <80MB | LanceDB mmap vs Hnswlib full-load |
| RNF-007: 100K archivos | Funcional | <5s index | Rust Tree-sitter paralelo |

---

*Documento de investigación técnica para decisiones arquitectónicas. Validado contra ecosistema Cuervo existente.*
