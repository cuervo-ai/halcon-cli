# Fase 1 — Estrategia de Vectorización (Knowledge Engineering)

> **Documento**: `09-knowledge-system/01-vectorization-strategy.md`
> **Versión**: 1.0.0
> **Fecha**: 2026-02-06
> **Autores**: Knowledge Systems Architect, Research Scientist, LLMOps Engineer
> **Estado**: Design Complete

---

## Índice

1. [Visión General](#1-visión-general)
2. [Estrategia de Segmentación Semántica (Chunking)](#2-estrategia-de-segmentación-semántica)
3. [Jerarquía Documental](#3-jerarquía-documental)
4. [Modelos de Embedding Recomendados](#4-modelos-de-embedding-recomendados)
5. [Pipeline de Ingestión](#5-pipeline-de-ingestión)
6. [Índice Híbrido (Vector + BM25 + Graph)](#6-índice-híbrido)
7. [Deduplicación, Normalización y Canonicalización](#7-deduplicación-normalización-y-canonicalización)
8. [Versionado de Embeddings e Invalidación](#8-versionado-de-embeddings-e-invalidación)
9. [Modelo de Datos del Knowledge Store](#9-modelo-de-datos-del-knowledge-store)
10. [Análisis Costo vs Rendimiento](#10-análisis-costo-vs-rendimiento)

---

## 1. Visión General

### 1.1 Problema

El proyecto Cuervo CLI posee 20+ documentos técnicos (~15,000+ líneas), código fuente en múltiples repositorios (9+ microservicios), ADRs, specs, requisitos y diagramas. Este conocimiento está disperso, no es buscable semánticamente, y carece de trazabilidad entre documentación, código y decisiones arquitectónicas.

### 1.2 Objetivo

Construir un pipeline de vectorización que transforme todo el conocimiento del proyecto en una base semántica consultable con alta precisión, manteniendo:

- **Fidelidad semántica**: Los chunks preservan significado completo
- **Trazabilidad**: Cada chunk mantiene linaje hasta su origen (archivo, sección, commit)
- **Frescura**: Actualización incremental al detectar cambios Git
- **Multi-modalidad**: Soportar texto, código, diagramas Mermaid, tablas, YAML/JSON schemas

### 1.3 Principios de Diseño

| Principio | Justificación |
|-----------|---------------|
| **Chunking semántico sobre tokenizado fijo** | Tokens fijos cortan oraciones y pierden contexto. El chunking semántico preserva unidades de significado |
| **Enriquecimiento contextual por chunk** | Cada chunk debe llevar metadata suficiente para ser entendido sin su documento padre |
| **Embeddings específicos por dominio** | Código y documentación técnica requieren modelos optimizados para estos dominios |
| **Offline-first** | El pipeline debe funcionar completamente local, con cloud como acelerador opcional |
| **Incremental por defecto** | Re-indexar solo lo que cambió. Nunca full-reindex salvo migración de modelo |

---

## 2. Estrategia de Segmentación Semántica

### 2.1 Por Qué NO Chunking por Tokens Fijos

```
❌ Chunking fijo (512 tokens):
"...el módulo de autenticación implementa JWT con rotación de refresh tokens cada 7 días. La"
"configuración se almacena en el archivo auth.config.ts ubicado en src/infrastructure/auth/..."

✅ Chunking semántico:
"El módulo de autenticación implementa JWT con rotación de refresh tokens cada 7 días.
La configuración se almacena en el archivo auth.config.ts ubicado en src/infrastructure/auth/.
Esto aplica tanto al modo standalone como al modo enterprise."
```

El chunking fijo produce fragmentos que cortan ideas a mitad. Estudios recientes (2026) demuestran que el chunking semántico **reduce errores de RAG en ~60%** comparado con chunking fijo.

### 2.2 Estrategia Multi-Modal de Chunking

Cuervo adopta un **Agentic Chunking** donde un clasificador determina la estrategia óptima por tipo de documento:

```
┌──────────────────────────────────────────────────────┐
│               CHUNKING ROUTER                        │
│                                                       │
│  Input Document                                       │
│       │                                               │
│       ▼                                               │
│  ┌─────────────┐                                     │
│  │ Classifier  │──▶ detect(fileType, structure)      │
│  └──────┬──────┘                                     │
│         │                                             │
│    ┌────┼────┬────────┬──────────┬──────────┐        │
│    ▼    ▼    ▼        ▼          ▼          ▼        │
│  Code  Markdown  API/Swagger  ADR/RFC   Config/YAML  │
│  (AST)  (Heading) (Endpoint)  (Section)  (Key-Path)  │
│    │    │    │        │          │          │         │
│    ▼    ▼    ▼        ▼          ▼          ▼        │
│  ┌─────────────────────────────────────────────┐     │
│  │        Contextual Enrichment Layer          │     │
│  │  + parent_context + breadcrumb + metadata   │     │
│  └─────────────────────────────────────────────┘     │
│                       │                               │
│                       ▼                               │
│              Enriched Chunks[]                        │
└──────────────────────────────────────────────────────┘
```

### 2.3 Estrategias por Tipo de Contenido

#### A. Código TypeScript/Rust — AST-Aware Chunking

**Herramienta**: Tree-sitter (ya planificado en Rust layer vía `treesitter.rs`)

```
Estrategia:
1. Parsear AST completo del archivo
2. Extraer nodos semánticos:
   - function_declaration / method_definition
   - class_declaration / interface_declaration
   - type_alias_declaration
   - export_statement (solo top-level con lógica)
   - module (Rust) / namespace (TS)
3. Para cada nodo:
   - Incluir JSDoc/doc-comments como parte del chunk
   - Incluir la firma completa + cuerpo
   - Si el cuerpo > 200 líneas → sub-dividir por métodos internos
4. Generar chunk con metadata:
   - symbol_name, symbol_type, file_path, line_range
   - imports relevantes (dependencias del chunk)
   - exported: boolean
```

**Tamaños dinámicos**:
| Tipo de símbolo | Tamaño típico | Máximo | Overlap |
|----------------|---------------|--------|---------|
| Función simple | 20-80 líneas | 150 líneas | Firma incluida en chunks contiguos |
| Clase completa | 50-300 líneas | Si > 300, dividir por método | Declaración de clase como contexto |
| Interface/Type | 5-50 líneas | 100 líneas | Ninguno (atómico) |
| Módulo/Archivo | Variable | 500 líneas max por chunk | Header del módulo |

**Ejemplo de chunk enriquecido para código**:

```json
{
  "chunk_id": "ck_a1b2c3d4",
  "content": "/**\n * Validates user credentials against the auth provider.\n * Supports JWT and OAuth2 flows.\n * @throws AuthenticationError if credentials are invalid\n */\nasync function validateCredentials(\n  credentials: UserCredentials,\n  provider: AuthProvider\n): Promise<AuthToken> {\n  const strategy = this.strategyFactory.create(provider);\n  const result = await strategy.authenticate(credentials);\n  if (!result.success) {\n    throw new AuthenticationError(result.error);\n  }\n  return result.token;\n}",
  "metadata": {
    "source_type": "code",
    "language": "typescript",
    "file_path": "src/infrastructure/auth/auth.service.ts",
    "symbol_name": "validateCredentials",
    "symbol_type": "function",
    "line_start": 45,
    "line_end": 62,
    "exports": true,
    "dependencies": ["UserCredentials", "AuthProvider", "AuthToken", "AuthenticationError"],
    "doc_summary": "Validates user credentials against the auth provider. Supports JWT and OAuth2.",
    "repository": "cuervo-auth-service",
    "layer": "infrastructure",
    "domain": "authentication"
  },
  "contextual_prefix": "This function is part of the authentication infrastructure layer in cuervo-auth-service. It handles credential validation for both JWT and OAuth2 authentication flows.",
  "version": {
    "commit_sha": "abc123",
    "indexed_at": "2026-02-06T10:30:00Z",
    "embedding_model": "voyage-3.5"
  }
}
```

#### B. Documentación Markdown — Heading-Aware Semantic Chunking

**Estrategia**: Chunking jerárquico por headings con análisis de coherencia semántica.

```
Algoritmo:
1. Parsear Markdown a AST (remark/mdast)
2. Identificar jerarquía de headings (H1 → H2 → H3 → H4)
3. Para cada sección (heading + contenido hasta siguiente heading del mismo nivel o superior):
   a. Si contenido < 100 tokens → fusionar con sección hermana siguiente
   b. Si contenido > 1500 tokens → sub-dividir por:
      - Párrafos con análisis de similaridad coseno entre párrafos adyacentes
      - Cuando similaridad cae < 0.7 → crear boundary
   c. Si contiene tabla → la tabla es un chunk atómico
   d. Si contiene bloque de código → el código es chunk separado con context_ref al párrafo anterior
   e. Si contiene diagrama Mermaid → extraer como chunk tipo "diagram" con descripción generada
4. Aplicar Contextual Retrieval:
   - Generar breadcrumb: "Documento > Sección > Subsección"
   - Prepend contextual summary al chunk
```

**Tamaños dinámicos**:
| Tipo de sección | Tamaño objetivo | Mínimo | Máximo | Overlap |
|----------------|----------------|--------|--------|---------|
| H2 section | 300-800 tokens | 100 tokens | 1500 tokens | Heading padre incluido |
| H3 subsection | 200-500 tokens | 50 tokens | 800 tokens | Heading padre + H2 incluido |
| Tabla | Completa | - | 2000 tokens | Caption + heading padre |
| Código inline | Completo | - | 1000 tokens | Párrafo previo como contexto |
| Lista numerada | Completa | - | 1000 tokens | Heading padre |

**Overlap contextual (no textual)**:

En vez de overlap textual (duplicar texto entre chunks), Cuervo usa **overlap contextual**:

```
Chunk N:
  contextual_prefix: "En el documento '02-IAM Architecture', sección 'OAuth2 Flow'..."
  content: [contenido de la subsección]
  breadcrumb: ["08-enterprise-design", "02-iam-architecture", "OAuth2 Flow", "Token Rotation"]

Chunk N+1:
  contextual_prefix: "Continuando con OAuth2 Flow en IAM Architecture, tras describir Token Rotation..."
  content: [siguiente subsección]
  breadcrumb: ["08-enterprise-design", "02-iam-architecture", "OAuth2 Flow", "Refresh Strategy"]
```

Esto evita duplicación de tokens en el índice pero preserva contexto.

#### C. API Specs (OpenAPI/Swagger) — Endpoint-Level Chunking

```
Estrategia:
1. Parsear spec OpenAPI 3.x
2. Para cada endpoint:
   - Un chunk = path + method + summary + parameters + request body + response schemas
   - Incluir tag/grupo como metadata
3. Para schemas compartidos (#/components/schemas):
   - Un chunk por schema con refs resueltos
4. Metadata especial:
   - http_method, path, tags, auth_required, deprecated
```

#### D. ADRs (Architecture Decision Records) — Section-Level Chunking

```
Estrategia:
1. Parsear estructura ADR estándar:
   - Title, Status, Context, Decision, Consequences
2. Cada sección es un chunk atómico
3. El ADR completo también se indexa como un chunk de nivel superior
4. Relaciones: links a otros ADRs se convierten en graph edges
5. Metadata especial:
   - adr_id, status (proposed/accepted/deprecated/superseded), date
   - superseded_by, related_adrs[]
```

#### E. Configuración (YAML/JSON/TOML) — Key-Path Chunking

```
Estrategia:
1. Parsear estructura jerárquica
2. Para cada key-path significativo (depth ≤ 3):
   - Chunk = key path + value + type + description (si existe)
3. El archivo completo también se indexa como chunk de contexto
4. Metadata: file_type, key_path, value_type, is_secret (redacted)
```

### 2.4 Late Chunking para Documentos Largos

Para documentos que excedan 4,000 tokens (como los docs de sección 08-enterprise-design que alcanzan ~3,000 líneas), se aplica **Late Chunking**:

```
Pipeline Late Chunking:
1. Enviar documento completo a modelo de embedding con contexto largo
   (Cohere embed-v4: 128K tokens, o Jina v3: 8K tokens)
2. El modelo genera token-level embeddings con atención global
3. Aplicar segmentación semántica post-embedding
4. Mean-pooling por segmento → embedding contextualizado por chunk
5. Cada chunk embedding "sabe" del contexto global del documento
```

**Ventaja**: Los chunks de secciones internas entienden el contexto del documento completo sin necesidad de LLM call por chunk (más barato que Contextual Retrieval de Anthropic).

**Cuándo usar cada estrategia**:

| Documento | Estrategia primaria | Fallback |
|-----------|-------------------|----------|
| < 4,000 tokens | Chunking semántico directo | - |
| 4,000 - 128,000 tokens | Late Chunking (Cohere embed-v4) | Semantic chunking + contextual prefix |
| > 128,000 tokens | Segmentación jerárquica + Late Chunking por sección | Recursive character split |
| Código | AST-aware (siempre) | Line-based con heurísticas |

---

## 3. Jerarquía Documental

### 3.1 Modelo de Cuatro Niveles

```
Level 0: CORPUS
├── Level 1: DOCUMENT
│   ├── Level 2: SECTION
│   │   ├── Level 3: CHUNK (unidad mínima de retrieval)
│   │   │   └── [artefactos: tabla, código, diagrama, lista]
│   │   ├── Level 3: CHUNK
│   │   └── Level 3: CHUNK
│   ├── Level 2: SECTION
│   └── Level 2: SECTION
├── Level 1: DOCUMENT
└── Level 1: DOCUMENT
```

### 3.2 Embeddings por Nivel (Hierarchical Retrieval)

Siguiendo el patrón A-RAG (Agentic RAG, arXiv 2602.03442), se generan embeddings a **múltiples niveles**:

| Nivel | Qué se embebe | Uso en retrieval |
|-------|--------------|------------------|
| **Document** | Título + resumen ejecutivo (generado por LLM si no existe) | Filtrado rápido: "¿Qué documento habla de X?" |
| **Section** | Heading + primer párrafo + resumen de subsecciones | Navegación: "¿Qué sección cubre OAuth2?" |
| **Chunk** | Contenido completo con contextual prefix | Recuperación precisa para RAG |
| **Artifact** | Tabla/diagrama/código con descripción | Búsqueda especializada: "Muéstrame el diagrama de auth flow" |

**Patrón de retrieval jerárquico**:

```
Query: "¿Cómo funciona la rotación de tokens JWT?"

Paso 1 (Document-level): Filtrar corpus → Documentos relevantes:
  - 08-enterprise-design/02-iam-architecture.md (score: 0.89)
  - 05-security-legal/02-privacidad-datos.md (score: 0.72)

Paso 2 (Section-level): Dentro de docs relevantes → Secciones:
  - "OAuth2 Flow > Token Rotation" (score: 0.94)
  - "JWT Implementation" (score: 0.87)

Paso 3 (Chunk-level): Retrieval fino → Top-K chunks:
  - Chunk sobre refresh token rotation policy (score: 0.96)
  - Chunk sobre token lifetime configuration (score: 0.91)
  - Chunk código de implementación (score: 0.88)
```

Este approach reduce el espacio de búsqueda progresivamente, mejorando tanto precisión como latencia.

---

## 4. Modelos de Embedding Recomendados

### 4.1 Matriz de Decisión

| Criterio | Voyage 3.5 | Cohere embed-v4 | OpenAI text-embedding-3-large | Local (BGE-M3) |
|----------|-----------|-----------------|-------------------------------|-----------------|
| **MTEB Score** | ~63.8 | 65.2 | 64.6 | ~62.1 |
| **Code retrieval** | ★★★★★ | ★★★★ | ★★★★ | ★★★ |
| **Multilingual (ES)** | ★★★★ | ★★★★★ | ★★★★★ | ★★★★★ |
| **Dimensiones** | 1024 | 256-1536 (Matryoshka) | 256-3072 | 1024 |
| **Contexto máximo** | 32K tokens | 128K tokens | 8K tokens | 8K tokens |
| **Precio/1M tokens** | $0.06 | $0.10 | $0.13 | $0 (local) |
| **Self-hosted** | No | No | No | Sí |
| **Late Chunking** | No nativo | Sí (128K) | No | No |
| **Quantización binaria** | No | Sí (ubinary) | No | No |

### 4.2 Estrategia Multi-Modelo Recomendada

```
┌─────────────────────────────────────────────────┐
│           EMBEDDING ROUTER                       │
│                                                   │
│  ┌──────────┐   ┌──────────┐   ┌──────────┐     │
│  │  Voyage   │   │  Cohere   │   │  BGE-M3  │     │
│  │  3.5      │   │ embed-v4  │   │  (local) │     │
│  └────┬─────┘   └────┬─────┘   └────┬─────┘     │
│       │               │               │           │
│  Code search    Long docs +      Offline mode     │
│  Technical      Late Chunking    Privacy mode     │
│  docs           Multimodal       Dev/test         │
│  API specs      128K context     Zero-cost        │
│                 Matryoshka                         │
└─────────────────────────────────────────────────┘
```

**Decisión arquitectónica**: Cuervo usa un **Embedding Router** que selecciona el modelo óptimo por caso:

| Caso de uso | Modelo primario | Dimensiones | Justificación |
|------------|----------------|-------------|---------------|
| **Código fuente** | Voyage 3.5 | 1024 | +9.7% sobre OpenAI en code retrieval (benchmarks Voyage) |
| **Documentación larga** | Cohere embed-v4 | 1024 (Matryoshka) | 128K context permite Late Chunking de docs completos |
| **Búsqueda general** | Cohere embed-v4 | 768 (Matryoshka reduced) | Balance costo/calidad con dimensiones reducidas |
| **Modo offline** | BGE-M3 (ONNX) | 1024 | Ejecutable 100% local, zero-cost, multilingual |
| **Dev/testing** | BGE-M3 (ONNX) | 1024 | Sin dependencia de API keys en CI/CD |

**Nota sobre Matryoshka embeddings**: Cohere embed-v4 permite truncar dimensiones (1536 → 1024 → 768 → 512 → 256) con degradación mínima. Esto permite:
- Almacenar a 1024 dims por defecto (buen balance)
- Reducir a 256 dims para índice de búsqueda rápida (pre-filtrado)
- Usar 1536 dims completos solo para re-ranking fino

### 4.3 Consistencia de Espacio Vectorial

**Regla crítica**: Todos los chunks indexados con un modelo dado deben buscarse con el mismo modelo. No se pueden mezclar embeddings de Voyage con queries de Cohere.

Solución: **Namespaced collections** por modelo de embedding:

```
knowledge_store/
├── collection:voyage-3.5/          ← código y API specs
│   ├── index_hnsw (1024 dims)
│   └── metadata
├── collection:cohere-embed-v4/     ← documentación y docs largos
│   ├── index_hnsw (1024 dims)
│   └── metadata
└── collection:bge-m3-local/        ← índice offline completo
    ├── index_hnsw (1024 dims)
    └── metadata
```

---

## 5. Pipeline de Ingestión

### 5.1 Arquitectura del Pipeline

```
┌─────────────────────────────────────────────────────────────────────┐
│                     INGESTION PIPELINE                              │
│                                                                     │
│  ┌──────────┐    ┌───────────┐    ┌──────────┐    ┌────────────┐  │
│  │  Source   │    │  Change   │    │  Content │    │  Chunking  │  │
│  │ Watcher  │───▶│ Detector  │───▶│ Extractor│───▶│  Router    │  │
│  │ (Git)    │    │ (diff)    │    │ (parser) │    │            │  │
│  └──────────┘    └───────────┘    └──────────┘    └─────┬──────┘  │
│                                                          │         │
│                                                    ┌─────┴──────┐  │
│                                                    │            │  │
│                                          ┌─────────┤  Chunkers  │  │
│                                          │         │            │  │
│                                          │         └────────────┘  │
│                                          ▼                         │
│  ┌──────────┐    ┌───────────┐    ┌──────────┐                    │
│  │  Vector  │    │ Contextual│    │ Metadata │                    │
│  │  Store   │◀───│ Enrichment│◀───│ Extractor│                    │
│  │ (write)  │    │ + Embed   │    │          │                    │
│  └──────────┘    └───────────┘    └──────────┘                    │
│       │                                                            │
│       ▼                                                            │
│  ┌──────────┐    ┌───────────┐                                    │
│  │  Graph   │    │  Search   │                                    │
│  │  Index   │    │  Index    │                                    │
│  │ (rels)   │    │  (BM25)   │                                    │
│  └──────────┘    └───────────┘                                    │
└─────────────────────────────────────────────────────────────────────┘
```

### 5.2 Etapas Detalladas

#### Etapa 1: Source Watcher

```typescript
// Detecta cambios en fuentes de conocimiento
interface SourceWatcher {
  // Monitoreo de cambios Git
  watchGitChanges(config: WatchConfig): AsyncIterable<ChangeEvent>;

  // Polling para fuentes sin watch nativo (APIs, wikis)
  pollExternalSources(interval: Duration): AsyncIterable<ChangeEvent>;

  // Ingestión manual / batch
  ingestBatch(sources: SourceDescriptor[]): Promise<IngestResult>;
}

interface ChangeEvent {
  source: SourceDescriptor;
  changeType: 'created' | 'modified' | 'deleted' | 'renamed';
  filePath: string;
  diff?: string;              // Git diff para cambios
  commitSha?: string;
  timestamp: Date;
}

interface SourceDescriptor {
  type: 'git_repo' | 'api_spec' | 'markdown' | 'code' | 'config';
  repository: string;
  branch: string;
  pathGlob: string;           // e.g., "docs/**/*.md", "src/**/*.ts"
  priority: 'critical' | 'high' | 'medium' | 'low';
}
```

#### Etapa 2: Change Detector

```typescript
interface ChangeDetector {
  // Determina qué chunks están afectados por un cambio
  detectAffectedChunks(event: ChangeEvent): Promise<AffectedChunks>;

  // Determina si un cambio requiere re-embedding
  requiresReembedding(event: ChangeEvent): boolean;
}

interface AffectedChunks {
  invalidated: ChunkId[];       // Chunks que deben eliminarse
  modified: ChunkId[];          // Chunks que deben re-procesarse
  newContent: ContentRange[];   // Rangos de contenido nuevo a procesar
  cascadeInvalidation: ChunkId[]; // Chunks en otros docs que referencian al cambiado
}
```

**Lógica de detección inteligente**:

```
Para un cambio en archivo X:
1. Si X es código:
   a. Parsear diff → identificar funciones/clases modificadas
   b. Buscar chunks existentes con symbol_name ∈ modified_symbols
   c. Marcar esos chunks como invalidated
   d. Buscar chunks de documentación que referencian a esos símbolos → cascade

2. Si X es documentación:
   a. Parsear diff → identificar secciones modificadas (por heading)
   b. Buscar chunks existentes con breadcrumb matching
   c. Marcar como modified solo las secciones cambiadas
   d. Si se modificó estructura (headings) → re-chunk documento completo

3. Si X es config:
   a. Parsear diff → identificar key-paths modificados
   b. Invalidar chunks de ese key-path
   c. Buscar documentación que referencia esos configs → cascade warning
```

#### Etapa 3: Content Extractor

```typescript
interface ContentExtractor {
  extract(filePath: string, content: string): Promise<ExtractedContent>;
}

interface ExtractedContent {
  raw: string;
  structured: StructuredDocument;
  frontmatter?: Record<string, unknown>;
  language?: string;
  encoding: string;
}

interface StructuredDocument {
  type: 'markdown' | 'code' | 'api_spec' | 'config' | 'adr';
  title: string;
  sections: DocumentSection[];
  artifacts: Artifact[];       // tablas, diagramas, bloques de código
  references: Reference[];     // links a otros docs/archivos
  metadata: DocumentMetadata;
}
```

#### Etapa 4: Chunking Router

```typescript
interface ChunkingRouter {
  selectStrategy(doc: StructuredDocument): ChunkingStrategy;
  chunk(doc: StructuredDocument, strategy: ChunkingStrategy): Chunk[];
}

type ChunkingStrategy =
  | { type: 'ast_aware'; language: string; parser: 'tree-sitter' }
  | { type: 'heading_semantic'; minTokens: number; maxTokens: number; similarityThreshold: number }
  | { type: 'endpoint_level'; specVersion: string }
  | { type: 'adr_sectional' }
  | { type: 'key_path'; maxDepth: number }
  | { type: 'late_chunking'; model: string; maxContext: number }
  | { type: 'recursive_character'; separators: string[]; chunkSize: number; overlap: number };
```

#### Etapa 5: Metadata Extractor & Contextual Enrichment

```typescript
interface MetadataExtractor {
  // Extrae metadata estructurada del chunk y su contexto
  extractMetadata(chunk: RawChunk, doc: StructuredDocument): ChunkMetadata;

  // Genera contextual prefix usando LLM (cacheable)
  generateContextualPrefix(chunk: RawChunk, doc: StructuredDocument): Promise<string>;
}

interface ChunkMetadata {
  // Identificación
  chunkId: string;                    // hash determinístico del contenido + posición
  documentId: string;
  sectionId: string;

  // Ubicación
  filePath: string;
  repository: string;
  branch: string;
  lineStart: number;
  lineEnd: number;
  breadcrumb: string[];               // ["section", "subsection", "topic"]

  // Clasificación
  contentType: ContentType;
  language?: string;
  domain: string[];                   // ["authentication", "security", "oauth2"]
  tags: string[];

  // Código específico
  symbolName?: string;
  symbolType?: SymbolType;
  dependencies?: string[];
  exports?: boolean;

  // Trazabilidad
  commitSha: string;
  lastModified: Date;
  author: string;
  version: string;

  // Calidad
  tokenCount: number;
  hasCodeExamples: boolean;
  hasTable: boolean;
  hasDiagram: boolean;
  completenessScore: number;          // 0-1, qué tan completo es el chunk por sí solo
}

type ContentType =
  | 'prose'
  | 'code'
  | 'api_endpoint'
  | 'config'
  | 'table'
  | 'diagram'
  | 'adr'
  | 'requirement'
  | 'use_case';
```

#### Etapa 6: Embedding & Storage

```typescript
interface EmbeddingService {
  // Embebe un batch de chunks con el modelo apropiado
  embedBatch(chunks: EnrichedChunk[], model: EmbeddingModel): Promise<EmbeddingResult[]>;

  // Selecciona modelo óptimo para el tipo de contenido
  selectModel(contentType: ContentType, mode: 'online' | 'offline'): EmbeddingModel;
}

interface EmbeddingResult {
  chunkId: string;
  vector: Float32Array;
  model: string;
  dimensions: number;
  tokensCounted: number;
  cost: number;                       // USD
  latencyMs: number;
}
```

### 5.3 Workers y Colas

```
┌──────────────────────────────────────────┐
│          WORKER ARCHITECTURE             │
│                                          │
│  Git Webhook / Watcher                   │
│       │                                  │
│       ▼                                  │
│  ┌──────────────┐                       │
│  │ Change Queue │  (BullMQ / Redis)     │
│  │  priority    │                       │
│  └──────┬───────┘                       │
│         │                                │
│    ┌────┴────┐                          │
│    ▼         ▼                          │
│  ┌──────┐ ┌──────┐                     │
│  │Worker│ │Worker│  (2-4 concurrent)    │
│  │  #1  │ │  #2  │                     │
│  └──┬───┘ └──┬───┘                     │
│     │        │                          │
│     ▼        ▼                          │
│  ┌──────────────┐                      │
│  │Embedding Queue│ (rate-limited)      │
│  │  batch: 100  │                      │
│  └──────┬───────┘                      │
│         │                               │
│    ┌────┴────┐                         │
│    ▼         ▼                         │
│  ┌──────┐ ┌──────┐                    │
│  │Embed │ │Embed │  (API rate limit)   │
│  │ W #1 │ │ W #2 │                    │
│  └──────┘ └──────┘                    │
└──────────────────────────────────────────┘
```

**Configuración de colas**:

| Cola | Concurrencia | Batch size | Rate limit | Prioridad |
|------|-------------|-----------|------------|-----------|
| `change-detection` | 4 workers | 1 evento | Sin límite | Por prioridad de source |
| `chunking` | 4 workers | 1 documento | Sin límite | FIFO |
| `embedding` | 2 workers | 100 chunks | 3000 req/min (Voyage) | Batch optimized |
| `indexing` | 2 workers | 50 chunks | Sin límite | After embedding |
| `graph-update` | 1 worker | 10 relaciones | Sin límite | Baja prioridad |

### 5.4 Re-indexación Incremental

```
┌─────────────────────────────────────────────────────────┐
│              INCREMENTAL RE-INDEXING                      │
│                                                           │
│  Trigger: git push / manual / schedule                    │
│       │                                                   │
│       ▼                                                   │
│  git diff HEAD~1..HEAD --name-status                     │
│       │                                                   │
│       ├──▶ Added files    → Full ingest                  │
│       ├──▶ Modified files → Diff-based partial re-chunk  │
│       ├──▶ Deleted files  → Remove chunks + cascade      │
│       └──▶ Renamed files  → Update metadata (no re-embed)│
│                                                           │
│  Optimization: Content hash comparison                    │
│  - Hash(new_chunk_content) == Hash(existing) → skip embed │
│  - Solo re-embebe si el contenido realmente cambió        │
│                                                           │
│  Cascade invalidation:                                    │
│  - Si chunk A referencia chunk B, y B cambió → mark A     │
│    as "stale_reference" (no re-embed, pero flag para      │
│    review por agente MCP)                                 │
└─────────────────────────────────────────────────────────┘
```

---

## 6. Índice Híbrido (Vector + BM25 + Graph)

### 6.1 Arquitectura de Tres Pilares

```
         User Query
             │
             ▼
    ┌─────────────────┐
    │  Query Analyzer  │
    │  - intent detect │
    │  - query expand  │
    │  - filter extract│
    └────────┬────────┘
             │
    ┌────────┼────────────────┐
    │        │                │
    ▼        ▼                ▼
┌────────┐ ┌────────┐  ┌──────────┐
│ Vector │ │  BM25  │  │  Graph   │
│ Search │ │ Search │  │ Traverse │
│(dense) │ │(sparse)│  │(rels)    │
│ Top-50 │ │ Top-50 │  │ Top-20   │
└───┬────┘ └───┬────┘  └────┬─────┘
    │          │             │
    └──────────┼─────────────┘
               │
               ▼
    ┌─────────────────┐
    │   RRF Fusion    │
    │ w=0.50 / 0.35 / │
    │     0.15        │
    └────────┬────────┘
             │
             ▼
    ┌─────────────────┐
    │  Cross-Encoder  │
    │   Re-ranking    │
    │   Top-10        │
    └────────┬────────┘
             │
             ▼
    ┌─────────────────┐
    │  Final Results   │
    │  Top-5 chunks    │
    └─────────────────┘
```

### 6.2 Detalle de Cada Pilar

**Vector Search (Dense)**:
- Motor: pgvector 0.8 con HNSW index (o LanceDB para modo offline)
- Distancia: cosine similarity
- Index params: `m=32, ef_construction=200, ef_search=100`
- Pre-filtrado por metadata (repository, content_type, language) usando iterative scanning de pgvector 0.8

**BM25 Search (Sparse)**:
- Motor: PostgreSQL full-text search con `tsvector` + `tsquery`
- Configuración: diccionario español + inglés, stemming, stop words
- Boost fields: title (x3), symbol_name (x2.5), tags (x2), content (x1)
- Ventaja: Captura términos exactos (nombres de función, error codes, config keys)

**Graph Traversal**:
- Motor: PostgreSQL con tablas de relaciones (no necesita DB separada para escala actual)
- Relaciones indexadas: `references`, `implements`, `extends`, `depends_on`, `supersedes`, `documents`
- Query: Expandir resultados iniciales siguiendo relaciones a 1-2 hops de distancia
- Ventaja: "Búscame la documentación del módulo que implementa esta interfaz"

### 6.3 Reciprocal Rank Fusion (RRF)

```typescript
interface FusionConfig {
  k: number;                          // RRF constant, default 60
  weights: {
    vector: number;                   // 0.50 — semantic relevance
    bm25: number;                     // 0.35 — keyword precision
    graph: number;                    // 0.15 — structural context
  };
  topK: number;                       // Candidates per retriever
  finalK: number;                     // Final results after fusion
}

function reciprocalRankFusion(
  results: Map<string, RankedResult[]>,
  config: FusionConfig
): ScoredResult[] {
  const scores = new Map<string, number>();

  for (const [retriever, ranked] of results) {
    const weight = config.weights[retriever];
    for (let i = 0; i < ranked.length; i++) {
      const doc = ranked[i].chunkId;
      const rrfScore = weight * (1 / (i + 1 + config.k));
      scores.set(doc, (scores.get(doc) || 0) + rrfScore);
    }
  }

  return [...scores.entries()]
    .sort(([, a], [, b]) => b - a)
    .slice(0, config.finalK)
    .map(([chunkId, score]) => ({ chunkId, score }));
}
```

### 6.4 Cross-Encoder Re-ranking

Después del RRF, los top-20 candidatos se pasan por un cross-encoder para scoring fino:

| Opción | Latencia | Calidad | Costo | Self-hosted |
|--------|----------|---------|-------|-------------|
| Cohere Rerank v3 | ~50ms/batch | ★★★★★ | $2/1K queries | No |
| BGE-reranker-v2-m3 | ~100ms/batch | ★★★★ | $0 | Sí |
| ColBERT via Qdrant | ~30ms/batch | ★★★★★ | $0 (infra) | Sí |

**Recomendación**: BGE-reranker-v2-m3 ejecutado localmente (ONNX runtime) para modo offline, Cohere Rerank para cloud.

---

## 7. Deduplicación, Normalización y Canonicalización

### 7.1 Deduplicación

**Problema**: El mismo concepto puede aparecer en múltiples documentos (e.g., "OAuth2 flow" en IAM doc, en security doc, y en requirements).

**Estrategia de 3 niveles**:

```
Nivel 1: Dedup exacta
  - Hash SHA-256 del contenido normalizado
  - Si hash idéntico → mantener solo el de mayor prioridad (por source_priority)

Nivel 2: Dedup near-duplicate
  - MinHash + LSH (Locality-Sensitive Hashing) con threshold 0.85
  - Si dos chunks tienen Jaccard similarity > 0.85:
    → Mantener el más completo (mayor token count)
    → Crear referencia cruzada del descartado al retenido

Nivel 3: Dedup semántica
  - Cosine similarity entre embeddings > 0.95
  - Marcar como "semantic_duplicate" pero NO eliminar
  - El retriever aplica dedup en query-time (max 1 resultado por cluster)
```

### 7.2 Normalización

```typescript
interface ContentNormalizer {
  normalize(content: string, type: ContentType): string;
}

// Reglas de normalización:
// 1. Unicode NFC normalization
// 2. Whitespace: colapsar múltiples espacios/newlines
// 3. Markdown: resolver relative links a absolute paths
// 4. Code: preservar indentación original (no normalizar)
// 5. URLs: normalizar trailing slashes, resolve redirects
// 6. Acrónimos: expandir en primera ocurrencia
//    "MCP" → "Model Context Protocol (MCP)"
// 7. Fechas: normalizar a ISO 8601
// 8. Versiones semánticas: normalizar a semver estricto
```

### 7.3 Canonicalización

**Problema**: El mismo concepto tiene múltiples nombres en el proyecto.

```typescript
interface CanonicalTerms {
  // Mapeo de términos canónicos
  aliases: Map<string, string>;  // alias → canonical
}

// Ejemplo para Cuervo:
const canonicalTerms: Record<string, string[]> = {
  "cuervo-cli": ["cuervo cli", "cuervo", "the cli", "la cli"],
  "model-gateway": ["gateway de modelos", "model router", "router de modelos"],
  "clean-architecture": ["arquitectura limpia", "clean arch", "hexagonal"],
  "mcp": ["model context protocol", "protocolo mcp"],
  "ollama": ["modelos locales", "local inference", "inferencia local"],
  "tree-sitter": ["treesitter", "tree sitter", "ast parser"],
  "pgvector": ["pg vector", "postgresql vector", "vector store"],
};
```

La canonicalización se aplica:
1. **En indexación**: Los términos canónicos se agregan como metadata `canonical_terms[]`
2. **En query**: La query se expande con los aliases del término buscado
3. **En BM25**: Los sinónimos se configuran en el diccionario de búsqueda

---

## 8. Versionado de Embeddings e Invalidación

### 8.1 Modelo de Versionado

```
embedding_version = hash(
  model_name +
  model_version +
  chunking_strategy_version +
  enrichment_prompt_version
)
```

Cada cambio en cualquiera de estos componentes genera una nueva versión de embeddings.

### 8.2 Estrategia de Migración

```
┌─────────────────────────────────────────────────────────┐
│              EMBEDDING VERSION MIGRATION                  │
│                                                           │
│  Trigger: Cambio de modelo o estrategia de chunking      │
│                                                           │
│  Fase 1: Crear nueva colección con nuevo modelo          │
│          (nombre: collection_{embedding_version})         │
│                                                           │
│  Fase 2: Re-indexar en background (batch, low-priority)  │
│          - Procesar chunks más consultados primero        │
│          - Progress: tracked en tabla migrations          │
│                                                           │
│  Fase 3: Blue-green switch                               │
│          - Cuando nueva colección alcance >95% coverage  │
│          - Cambiar alias de búsqueda a nueva colección   │
│          - Mantener colección anterior 7 días             │
│                                                           │
│  Fase 4: Cleanup                                         │
│          - Eliminar colección anterior                    │
│          - Actualizar metadata de versión                 │
└─────────────────────────────────────────────────────────┘
```

### 8.3 Invalidación por Cambios Git

```typescript
interface InvalidationPolicy {
  // Qué trigger causa invalidación
  triggers: {
    contentChange: true;          // Siempre: contenido de archivo cambió
    dependencyChange: boolean;    // Si un símbolo importado cambió
    configChange: boolean;        // Si config referenciada cambió
    schemaChange: boolean;        // Si schema de API cambió
  };

  // Tiempo de gracia antes de re-embeber (debounce)
  debounceMs: number;             // 5000ms — evitar re-embed por cada commit de un push

  // Cascading: ¿hasta dónde propagar?
  cascadeDepth: number;           // 1 = solo dependencias directas
                                  // 2 = dependencias de dependencias
}
```

---

## 9. Modelo de Datos del Knowledge Store

### 9.1 Schema Principal

```sql
-- ============================================================
-- KNOWLEDGE STORE SCHEMA
-- PostgreSQL 16+ con pgvector 0.8
-- ============================================================

-- Extensiones requeridas
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;     -- trigram para fuzzy search

-- -----------------------------------------------------------
-- DOCUMENTS: Registro de documentos fuente
-- -----------------------------------------------------------
CREATE TABLE documents (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    file_path       TEXT NOT NULL,
    repository      TEXT NOT NULL,
    branch          TEXT NOT NULL DEFAULT 'main',
    title           TEXT NOT NULL,
    doc_type        TEXT NOT NULL,             -- 'markdown' | 'code' | 'api_spec' | 'config' | 'adr'
    language        TEXT,                       -- 'typescript' | 'rust' | 'markdown' | etc.
    content_hash    TEXT NOT NULL,              -- SHA-256 del contenido raw
    token_count     INTEGER NOT NULL,
    section_path    TEXT[],                     -- ["08-enterprise-design", "02-iam-architecture"]
    frontmatter     JSONB,
    summary         TEXT,                       -- Resumen generado por LLM
    summary_embedding VECTOR(1024),            -- Embedding del resumen (document-level)

    -- Trazabilidad
    commit_sha      TEXT NOT NULL,
    author          TEXT,
    last_modified   TIMESTAMPTZ NOT NULL,
    indexed_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Búsqueda full-text
    search_vector   TSVECTOR GENERATED ALWAYS AS (
        setweight(to_tsvector('spanish', COALESCE(title, '')), 'A') ||
        setweight(to_tsvector('english', COALESCE(title, '')), 'A') ||
        setweight(to_tsvector('spanish', COALESCE(summary, '')), 'B') ||
        setweight(to_tsvector('english', COALESCE(summary, '')), 'B')
    ) STORED,

    UNIQUE(file_path, repository, branch)
);

-- -----------------------------------------------------------
-- CHUNKS: Unidades atómicas de conocimiento
-- -----------------------------------------------------------
CREATE TABLE chunks (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    document_id     UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    chunk_index     INTEGER NOT NULL,          -- Orden dentro del documento

    -- Contenido
    content         TEXT NOT NULL,
    contextual_prefix TEXT,                    -- Prefijo contextual generado
    content_hash    TEXT NOT NULL,             -- Para dedup exacta

    -- Clasificación
    content_type    TEXT NOT NULL,              -- 'prose' | 'code' | 'table' | etc.
    breadcrumb      TEXT[] NOT NULL,            -- Jerarquía de secciones

    -- Ubicación
    line_start      INTEGER,
    line_end        INTEGER,

    -- Código-específico
    symbol_name     TEXT,
    symbol_type     TEXT,                       -- 'function' | 'class' | 'interface' | etc.
    dependencies    TEXT[],
    is_exported     BOOLEAN DEFAULT FALSE,

    -- Metadata
    domain_tags     TEXT[] NOT NULL DEFAULT '{}',
    token_count     INTEGER NOT NULL,
    has_code        BOOLEAN DEFAULT FALSE,
    has_table       BOOLEAN DEFAULT FALSE,
    has_diagram     BOOLEAN DEFAULT FALSE,
    completeness    REAL DEFAULT 1.0,          -- 0-1

    -- Embeddings (multi-modelo)
    embedding_voyage    VECTOR(1024),          -- Voyage 3.5 (código)
    embedding_cohere    VECTOR(1024),          -- Cohere embed-v4 (docs)
    embedding_local     VECTOR(1024),          -- BGE-M3 (offline)
    embedding_model     TEXT NOT NULL,          -- Modelo primario usado
    embedding_version   TEXT NOT NULL,          -- Versión del pipeline

    -- Búsqueda full-text
    search_vector   TSVECTOR GENERATED ALWAYS AS (
        setweight(to_tsvector('spanish', COALESCE(symbol_name, '')), 'A') ||
        setweight(to_tsvector('english', COALESCE(symbol_name, '')), 'A') ||
        setweight(to_tsvector('spanish', COALESCE(content, '')), 'B') ||
        setweight(to_tsvector('english', COALESCE(content, '')), 'B')
    ) STORED,

    -- Trazabilidad
    commit_sha      TEXT NOT NULL,
    indexed_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Dedup
    minhash         BIT(256),                  -- Para near-duplicate detection
    dedup_cluster   UUID,                      -- Cluster de duplicados semánticos

    UNIQUE(document_id, chunk_index)
);

-- -----------------------------------------------------------
-- RELATIONS: Grafo de conocimiento
-- -----------------------------------------------------------
CREATE TABLE chunk_relations (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_chunk_id UUID NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    target_chunk_id UUID NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    relation_type   TEXT NOT NULL,              -- 'references' | 'implements' | 'extends' |
                                                -- 'depends_on' | 'supersedes' | 'documents' |
                                                -- 'related_to' | 'contradicts' | 'elaborates'
    confidence      REAL DEFAULT 1.0,          -- 0-1, para relaciones inferidas
    metadata        JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(source_chunk_id, target_chunk_id, relation_type)
);

-- -----------------------------------------------------------
-- EMBEDDING_VERSIONS: Control de versiones de embeddings
-- -----------------------------------------------------------
CREATE TABLE embedding_versions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    version_hash    TEXT NOT NULL UNIQUE,
    model_name      TEXT NOT NULL,
    model_version   TEXT NOT NULL,
    dimensions      INTEGER NOT NULL,
    chunking_version TEXT NOT NULL,
    enrichment_version TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    status          TEXT NOT NULL DEFAULT 'active', -- 'active' | 'migrating' | 'deprecated'
    chunk_count     INTEGER DEFAULT 0,
    coverage_pct    REAL DEFAULT 0.0
);

-- -----------------------------------------------------------
-- INGESTION_JOBS: Tracking de trabajos de ingestión
-- -----------------------------------------------------------
CREATE TABLE ingestion_jobs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    trigger_type    TEXT NOT NULL,              -- 'git_push' | 'manual' | 'schedule' | 'migration'
    trigger_ref     TEXT,                       -- commit SHA o job ID
    status          TEXT NOT NULL DEFAULT 'pending', -- 'pending' | 'processing' | 'completed' | 'failed'
    documents_total INTEGER DEFAULT 0,
    documents_processed INTEGER DEFAULT 0,
    chunks_created  INTEGER DEFAULT 0,
    chunks_updated  INTEGER DEFAULT 0,
    chunks_deleted  INTEGER DEFAULT 0,
    embeddings_generated INTEGER DEFAULT 0,
    cost_usd        REAL DEFAULT 0.0,
    started_at      TIMESTAMPTZ,
    completed_at    TIMESTAMPTZ,
    error           TEXT,
    metadata        JSONB
);

-- -----------------------------------------------------------
-- QUERY_LOG: Registro de consultas para analytics y mejora
-- -----------------------------------------------------------
CREATE TABLE query_log (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    query_text      TEXT NOT NULL,
    query_embedding VECTOR(1024),
    retrieval_method TEXT NOT NULL,             -- 'hybrid' | 'vector_only' | 'bm25_only'
    results_returned INTEGER,
    latency_ms      INTEGER,
    user_feedback    TEXT,                      -- 'relevant' | 'partial' | 'irrelevant' | null
    reranker_used   TEXT,
    filters_applied JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- -----------------------------------------------------------
-- INDEXES
-- -----------------------------------------------------------

-- Vector indexes (HNSW para búsqueda aproximada)
CREATE INDEX idx_chunks_embedding_voyage ON chunks
    USING hnsw (embedding_voyage vector_cosine_ops)
    WITH (m = 32, ef_construction = 200);

CREATE INDEX idx_chunks_embedding_cohere ON chunks
    USING hnsw (embedding_cohere vector_cosine_ops)
    WITH (m = 32, ef_construction = 200);

CREATE INDEX idx_chunks_embedding_local ON chunks
    USING hnsw (embedding_local vector_cosine_ops)
    WITH (m = 32, ef_construction = 200);

CREATE INDEX idx_documents_summary_embedding ON documents
    USING hnsw (summary_embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 100);

-- Full-text search indexes
CREATE INDEX idx_chunks_search ON chunks USING gin(search_vector);
CREATE INDEX idx_documents_search ON documents USING gin(search_vector);

-- Trigram index para fuzzy search
CREATE INDEX idx_chunks_symbol_trgm ON chunks USING gin(symbol_name gin_trgm_ops);

-- Relational indexes
CREATE INDEX idx_relations_source ON chunk_relations(source_chunk_id);
CREATE INDEX idx_relations_target ON chunk_relations(target_chunk_id);
CREATE INDEX idx_relations_type ON chunk_relations(relation_type);

-- Metadata indexes
CREATE INDEX idx_chunks_content_type ON chunks(content_type);
CREATE INDEX idx_chunks_domain_tags ON chunks USING gin(domain_tags);
CREATE INDEX idx_chunks_document_id ON chunks(document_id);
CREATE INDEX idx_chunks_symbol_name ON chunks(symbol_name) WHERE symbol_name IS NOT NULL;
CREATE INDEX idx_documents_repository ON documents(repository);
CREATE INDEX idx_documents_doc_type ON documents(doc_type);
CREATE INDEX idx_chunks_content_hash ON chunks(content_hash);
CREATE INDEX idx_chunks_dedup_cluster ON chunks(dedup_cluster) WHERE dedup_cluster IS NOT NULL;
```

### 9.2 Schema para LanceDB (Modo Offline)

Para el modo offline, LanceDB (ya decidido como vector store local en ADR-009) replica un subconjunto:

```typescript
// LanceDB table schema (Arrow-compatible)
interface LanceChunkSchema {
  id: string;
  document_id: string;
  content: string;
  contextual_prefix: string;
  content_type: string;
  breadcrumb: string;                 // JSON serialized
  symbol_name: string | null;
  domain_tags: string;                // JSON serialized
  token_count: number;
  embedding: Float32Array;            // 1024 dims (BGE-M3)
  content_hash: string;
  commit_sha: string;
  indexed_at: string;                 // ISO 8601
}

// LanceDB con Tantivy FTS integrado (hybrid search nativo)
// IVF-PQ para vectors + Tantivy para BM25 en un solo motor
```

---

## 10. Análisis Costo vs Rendimiento

### 10.1 Estimación de Volumen

| Fuente | Archivos estimados | Tokens estimados | Chunks estimados |
|--------|-------------------|------------------|------------------|
| Docs (20+ archivos) | 23 | ~120,000 | ~800 |
| Código cuervo-cli | ~200 (proyección) | ~400,000 | ~2,000 |
| Código ecosystem (9 repos) | ~1,800 | ~3,600,000 | ~18,000 |
| Config/schemas | ~100 | ~50,000 | ~500 |
| **Total inicial** | **~2,100** | **~4,170,000** | **~21,300** |
| **Proyección 1 año** | **~5,000** | **~10,000,000** | **~50,000** |

### 10.2 Costo de Embedding

| Modelo | Precio/1M tokens | Costo indexación inicial | Costo mensual incremental |
|--------|-----------------|------------------------|--------------------------|
| Voyage 3.5 | $0.06 | $0.25 | $0.03 |
| Cohere embed-v4 | $0.10 | $0.42 | $0.05 |
| BGE-M3 (local) | $0 (compute) | $0 | $0 |
| **Dual-index (Voyage + Cohere)** | - | **$0.67** | **$0.08** |

**Conclusión**: El costo de embedding es negligible (~$1/mes) incluso con dual-indexing. La inversión real está en infraestructura de compute y almacenamiento.

### 10.3 Costo de Infraestructura

| Componente | Self-hosted | Cloud (mínimo) | Cloud (producción) |
|-----------|------------|----------------|-------------------|
| PostgreSQL + pgvector | $0 (dev local) | $50/mes (RDS) | $200/mes (Aurora) |
| LanceDB (offline) | $0 (local disk) | N/A | N/A |
| Redis (colas) | $0 (dev local) | $15/mes | $50/mes |
| Compute (workers) | Laptop CPU | $30/mes (ECS) | $100/mes |
| **Total** | **$0 (dev)** | **$95/mes** | **$350/mes** |

### 10.4 Latencia Objetivo

| Operación | Target p50 | Target p95 | Actual con pgvector 0.8 |
|-----------|-----------|-----------|------------------------|
| Vector search (50K chunks) | <20ms | <50ms | ~15ms (HNSW) |
| BM25 search | <10ms | <30ms | ~8ms (GIN index) |
| Hybrid search + RRF | <40ms | <80ms | ~35ms |
| + Cross-encoder rerank | <80ms | <150ms | ~90ms (local ONNX) |
| **End-to-end retrieval** | **<100ms** | **<200ms** | **~120ms** |

### 10.5 Trade-offs Documentados

| Decisión | Beneficio | Costo/Trade-off |
|----------|----------|----------------|
| Multi-modelo embedding | Mejor precisión por dominio | Complejidad de routing, colecciones separadas |
| Late Chunking | Contexto global por chunk | Requiere modelo con 128K context (solo Cohere) |
| Contextual prefix (LLM) | +15-20% retrieval quality | ~$0.01/chunk en generación |
| AST-aware chunking | Chunks semánticos para código | Dependencia de Tree-sitter, más lento que regex |
| Graph index | Multi-hop reasoning | Construcción/mantenimiento del grafo |
| Dual BM25+vector | Keyword + semantic coverage | Doble storage, fusión complexity |
| Cross-encoder rerank | +10-15% precision en top-5 | +50-100ms latencia por query |
| Offline BGE-M3 index | 100% offline capability | Menor quality que modelos cloud |

---

## Apéndice A: Configuración del Pipeline

```yaml
# knowledge-pipeline.config.yaml
pipeline:
  name: "cuervo-knowledge-pipeline"
  version: "1.0.0"

sources:
  - name: "cuervo-cli-docs"
    type: git_repo
    repository: "cuervo-cli"
    branch: "main"
    paths:
      - "docs/**/*.md"
    priority: critical
    chunking: heading_semantic

  - name: "cuervo-cli-code"
    type: git_repo
    repository: "cuervo-cli"
    branch: "main"
    paths:
      - "src/**/*.ts"
      - "native/**/*.rs"
    priority: high
    chunking: ast_aware

  - name: "cuervo-ecosystem"
    type: git_repo
    repositories:
      - "cuervo-main"
      - "cuervo-auth-service"
      - "cuervo-prompt-service"
    branch: "main"
    paths:
      - "src/**/*.ts"
      - "docs/**/*.md"
    priority: medium
    chunking: auto

chunking:
  heading_semantic:
    min_tokens: 100
    max_tokens: 1500
    similarity_threshold: 0.70
    overlap_type: contextual

  ast_aware:
    parser: tree-sitter
    languages: [typescript, rust]
    max_lines: 200
    include_docs: true
    include_imports: true

  auto:
    classifier: content_type
    fallback: recursive_character

embedding:
  models:
    code:
      name: "voyage-3.5"
      dimensions: 1024
      batch_size: 100
    docs:
      name: "cohere-embed-v4"
      dimensions: 1024
      batch_size: 96
    offline:
      name: "bge-m3"
      dimensions: 1024
      runtime: onnx

  enrichment:
    contextual_prefix: true
    prefix_model: "claude-haiku-4-5"
    cache_prefix: true

search:
  hybrid:
    vector_weight: 0.50
    bm25_weight: 0.35
    graph_weight: 0.15
    rrf_k: 60
    top_k_per_retriever: 50
    final_k: 10

  reranking:
    enabled: true
    model: "bge-reranker-v2-m3"
    runtime: onnx
    top_k_input: 20
    top_k_output: 5

deduplication:
  exact_hash: true
  near_duplicate:
    enabled: true
    threshold: 0.85
    algorithm: minhash_lsh
  semantic:
    enabled: true
    threshold: 0.95
    action: cluster_and_flag

invalidation:
  trigger: git_push
  debounce_ms: 5000
  cascade_depth: 1
  content_hash_check: true

workers:
  change_detection: 4
  chunking: 4
  embedding: 2
  indexing: 2
  graph_update: 1

storage:
  primary: postgresql   # pgvector 0.8
  offline: lancedb
  cache: redis
  graph: postgresql     # tablas de relaciones
```

---

*Siguiente documento: [02-knowledge-store.md](./02-knowledge-store.md) — Arquitectura del Knowledge Store*
