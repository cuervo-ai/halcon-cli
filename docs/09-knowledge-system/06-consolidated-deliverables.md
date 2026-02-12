# Fase 6 — Entregables Consolidados

> **Documento**: `09-knowledge-system/06-consolidated-deliverables.md`
> **Versión**: 1.0.0
> **Fecha**: 2026-02-06
> **Autores**: Equipo completo (Knowledge Systems Architect, LLMOps Engineer, Software Architect, Technical Writer Automation Specialist, Research Scientist)
> **Estado**: Design Complete

---

## Índice

1. [Arquitectura Completa del Sistema](#1-arquitectura-completa-del-sistema)
2. [Diagrama de Ingestión y Vectorización](#2-diagrama-de-ingestión-y-vectorización)
3. [Diseño del Knowledge Store (Resumen)](#3-diseño-del-knowledge-store)
4. [Diseño del MCP Documentation Agent (Resumen)](#4-diseño-del-mcp-documentation-agent)
5. [APIs/Interfaces TypeScript Consolidadas](#5-apisinterfaces-typescript-consolidadas)
6. [Modelo de Datos Consolidado](#6-modelo-de-datos-consolidado)
7. [Flujo DocOps (End-to-End)](#7-flujo-docops-end-to-end)
8. [Métricas de Calidad](#8-métricas-de-calidad)
9. [Roadmap de Implementación](#9-roadmap-de-implementación)
10. [Lista de Riesgos Técnicos](#10-lista-de-riesgos-técnicos)

---

## 1. Arquitectura Completa del Sistema

### 1.1 Vista C4 — Nivel Contexto

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                                                                             │
│                        CUERVO KNOWLEDGE SYSTEM                              │
│                        System Context Diagram                               │
│                                                                             │
│  ┌─────────────┐         ┌───────────────────────────────┐                 │
│  │  Developer   │────────▶│     CUERVO CLI                │                 │
│  │  (persona)   │◀────────│     + Knowledge System        │                 │
│  └─────────────┘         │                               │                 │
│        │                  │  ┌──────────────────────┐    │                 │
│        │ reviews PRs      │  │  DocAgent (MCP)      │    │                 │
│        │                  │  │  Knowledge Store      │    │                 │
│        │                  │  │  DocOps Pipeline       │    │                 │
│        │                  │  └──────────────────────┘    │                 │
│        │                  └──────────┬────────┬──────────┘                 │
│        │                             │        │                             │
│        │                    ┌────────┘        └────────┐                   │
│        │                    ▼                           ▼                   │
│  ┌─────▼──────┐   ┌────────────────┐          ┌──────────────┐           │
│  │  GitHub     │   │ LLM Providers  │          │ Cuervo       │           │
│  │  (repos,    │   │                │          │ Ecosystem    │           │
│  │   PRs,      │   │ - Claude       │          │ (9+ services)│           │
│  │   issues)   │   │ - GPT          │          │              │           │
│  └────────────┘   │ - Voyage       │          └──────────────┘           │
│                    │ - Cohere       │                                      │
│                    │ - Local/Ollama │                                      │
│                    └────────────────┘                                      │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 1.2 Vista C4 — Nivel Contenedor

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                     CUERVO KNOWLEDGE SYSTEM — Containers                    │
│                                                                             │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                         CUERVO CLI (Process)                         │   │
│  │                                                                      │   │
│  │  ┌───────────┐  ┌──────────────┐  ┌──────────────┐  ┌───────────┐  │   │
│  │  │ CLI REPL  │  │ Knowledge    │  │  DocAgent    │  │  DocOps   │  │   │
│  │  │ Interface │  │ Query API    │  │  (MCP Agent) │  │  Pipeline │  │   │
│  │  │           │  │              │  │              │  │           │  │   │
│  │  │ Commands: │  │ - search()   │  │ - Planner    │  │ - Linters │  │   │
│  │  │ /search   │  │ - ingest()   │  │ - Executor   │  │ - Coverage│  │   │
│  │  │ /doc      │  │ - validate() │  │ - Reviewer   │  │ - Quality │  │   │
│  │  │ /audit    │  │              │  │ - Memory     │  │ - Tests   │  │   │
│  │  └─────┬─────┘  └──────┬───────┘  └──────┬───────┘  └─────┬─────┘  │   │
│  │        │               │                  │                │        │   │
│  │        └───────────────┼──────────────────┼────────────────┘        │   │
│  │                        │                  │                         │   │
│  └────────────────────────┼──────────────────┼─────────────────────────┘   │
│                           │                  │                              │
│  ┌────────────────────────┼──────────────────┼─────────────────────────┐   │
│  │              STORAGE LAYER                │                         │   │
│  │                        │                  │                         │   │
│  │  ┌────────────────┐  ┌▼──────────────┐  ┌▼──────────────┐         │   │
│  │  │ PostgreSQL     │  │ LanceDB       │  │ Redis         │         │   │
│  │  │ + pgvector 0.8 │  │ (Offline)     │  │ (Queues +     │         │   │
│  │  │ + FTS          │  │ + Tantivy     │  │  Cache L3)    │         │   │
│  │  │ + Relations    │  │               │  │               │         │   │
│  │  │ (Online)       │  │ (Local)       │  │ (Cloud only)  │         │   │
│  │  └────────────────┘  └───────────────┘  └───────────────┘         │   │
│  └────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
│  ┌────────────────────────────────────────────────────────────────────┐   │
│  │              EXTERNAL SERVICES                                     │   │
│  │                                                                    │   │
│  │  ┌──────────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐  │   │
│  │  │ Embedding    │  │ Reranker │  │ LLM      │  │ Observability│  │   │
│  │  │ APIs         │  │          │  │ API      │  │              │  │   │
│  │  │              │  │ BGE-M3   │  │          │  │ Langfuse     │  │   │
│  │  │ - Voyage 3.5 │  │ (local   │  │ - Claude │  │ (self-hosted)│  │   │
│  │  │ - Cohere v4  │  │  ONNX)   │  │ - GPT    │  │              │  │   │
│  │  │ - BGE-M3     │  │          │  │ - Local  │  │ OpenTelemetry│  │   │
│  │  │   (local)    │  │          │  │          │  │              │  │   │
│  │  └──────────────┘  └──────────┘  └──────────┘  └──────────────┘  │   │
│  └────────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 1.3 Vista C4 — Nivel Componente (Knowledge Query)

```
┌──────────────────────────────────────────────────────────────────┐
│                 KNOWLEDGE QUERY COMPONENT                        │
│                                                                   │
│  Query Input                                                      │
│       │                                                           │
│       ▼                                                           │
│  ┌──────────────────┐                                            │
│  │  Query Analyzer   │ Detect intent, extract filters, expand     │
│  └────────┬─────────┘                                            │
│           │                                                       │
│  ┌────────▼─────────┐                                            │
│  │  Cache Lookup     │ L1 Memory → L2 Semantic → L3 Redis        │
│  └────────┬─────────┘                                            │
│      hit  │  miss                                                │
│       │   │                                                       │
│       │   ▼                                                       │
│       │  ┌──────────────┐                                        │
│       │  │  Engine       │ Select: PostgreSQL (online)            │
│       │  │  Selector     │    or   LanceDB (offline)             │
│       │  └───────┬──────┘                                        │
│       │          │                                                │
│       │  ┌───────┼──────────────┐                                │
│       │  │       │              │                                 │
│       │  ▼       ▼              ▼                                 │
│       │ Vector  BM25          Graph                              │
│       │ Search  Search        Traversal                          │
│       │ (top50) (top50)       (top20)                            │
│       │  │       │              │                                 │
│       │  └───────┼──────────────┘                                │
│       │          │                                                │
│       │  ┌───────▼──────┐                                        │
│       │  │  RRF Fusion   │ w: 0.50 / 0.35 / 0.15               │
│       │  └───────┬──────┘                                        │
│       │          │                                                │
│       │  ┌───────▼──────┐                                        │
│       │  │  Reranker     │ BGE-reranker (local) / Cohere (cloud) │
│       │  │  (top 20→5)   │                                       │
│       │  └───────┬──────┘                                        │
│       │          │                                                │
│       └──────────┤                                                │
│                  ▼                                                 │
│  ┌──────────────────┐                                            │
│  │  Response Builder │ Format + provenance + cache store          │
│  └──────────────────┘                                            │
└──────────────────────────────────────────────────────────────────┘
```

---

## 2. Diagrama de Ingestión y Vectorización

```
┌──────────────────────────────────────────────────────────────────────────┐
│                     INGESTION & VECTORIZATION PIPELINE                    │
│                                                                          │
│  SOURCES                                                                 │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐     │
│  │  Git     │ │  Docs    │ │  Code    │ │  API     │ │  Config  │     │
│  │  Webhook │ │  (*.md)  │ │  (*.ts)  │ │  Specs   │ │  (YAML)  │     │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘     │
│       │             │            │             │            │            │
│       └─────────────┼────────────┼─────────────┼────────────┘            │
│                     │            │             │                          │
│                     ▼            ▼             ▼                          │
│  STAGE 1    ┌─────────────────────────────────────────┐                  │
│  DETECT     │           CHANGE DETECTOR               │                  │
│             │  • git diff analysis                    │                  │
│             │  • content hash comparison              │                  │
│             │  • affected chunk identification        │                  │
│             │  • cascade invalidation                 │                  │
│             └──────────────────┬──────────────────────┘                  │
│                                │                                         │
│                                ▼                                         │
│  STAGE 2    ┌─────────────────────────────────────────┐                  │
│  EXTRACT    │          CONTENT EXTRACTOR              │                  │
│             │  • Parse Markdown (remark/mdast)        │                  │
│             │  • Parse Code (Tree-sitter)             │                  │
│             │  • Parse OpenAPI (swagger-parser)       │                  │
│             │  • Parse Config (YAML/JSON/TOML)        │                  │
│             │  • Extract frontmatter & metadata       │                  │
│             └──────────────────┬──────────────────────┘                  │
│                                │                                         │
│                                ▼                                         │
│  STAGE 3    ┌─────────────────────────────────────────┐                  │
│  CHUNK      │           CHUNKING ROUTER               │                  │
│             │                                         │                  │
│             │  Markdown ──▶ Heading-Aware Semantic     │                  │
│             │  Code     ──▶ AST-Aware (Tree-sitter)   │                  │
│             │  API Spec ──▶ Endpoint-Level             │                  │
│             │  ADR      ──▶ Section-Level              │                  │
│             │  Config   ──▶ Key-Path                   │                  │
│             │  Long doc ──▶ Late Chunking (128K)       │                  │
│             └──────────────────┬──────────────────────┘                  │
│                                │                                         │
│                                ▼                                         │
│  STAGE 4    ┌─────────────────────────────────────────┐                  │
│  ENRICH     │       CONTEXTUAL ENRICHMENT             │                  │
│             │                                         │                  │
│             │  • Generate contextual prefix           │                  │
│             │    (via LLM or Late Chunking)           │                  │
│             │  • Extract metadata (domain, tags)       │                  │
│             │  • Build breadcrumb hierarchy            │                  │
│             │  • Compute content hash (dedup)         │                  │
│             │  • Extract relations (links, imports)    │                  │
│             └──────────────────┬──────────────────────┘                  │
│                                │                                         │
│                                ▼                                         │
│  STAGE 5    ┌─────────────────────────────────────────┐                  │
│  DEDUP      │         DEDUPLICATION                   │                  │
│             │                                         │                  │
│             │  L1: Exact hash → skip                  │                  │
│             │  L2: MinHash LSH (>0.85) → merge        │                  │
│             │  L3: Semantic (>0.95) → cluster & flag  │                  │
│             └──────────────────┬──────────────────────┘                  │
│                                │                                         │
│                                ▼                                         │
│  STAGE 6    ┌─────────────────────────────────────────┐                  │
│  EMBED      │          EMBEDDING ROUTER               │                  │
│             │                                         │                  │
│             │  Code    ──▶ Voyage 3.5 (1024d)         │                  │
│             │  Docs    ──▶ Cohere embed-v4 (1024d)    │                  │
│             │  Offline ──▶ BGE-M3 ONNX (1024d)        │                  │
│             │                                         │                  │
│             │  Batch: 100 chunks per API call          │                  │
│             │  Rate limit: 3000 req/min                │                  │
│             └──────────────────┬──────────────────────┘                  │
│                                │                                         │
│                                ▼                                         │
│  STAGE 7    ┌─────────────────────────────────────────┐                  │
│  INDEX      │            INDEXING                      │                  │
│             │                                         │                  │
│             │  ┌─────────┐ ┌─────────┐ ┌───────────┐ │                  │
│             │  │ Vector  │ │  FTS    │ │  Graph    │ │                  │
│             │  │ (HNSW)  │ │(tsvector)│ │(relations)│ │                  │
│             │  └─────────┘ └─────────┘ └───────────┘ │                  │
│             │                                         │                  │
│             │  + Sync to LanceDB (offline mirror)     │                  │
│             └─────────────────────────────────────────┘                  │
│                                                                          │
│  OUTPUT: Indexed knowledge ready for hybrid search                       │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Diseño del Knowledge Store

**Referencia completa**: [02-knowledge-store.md](./02-knowledge-store.md)

### Resumen de Decisiones

| Componente | Tecnología | Justificación |
|-----------|-----------|---------------|
| Vector DB (online) | pgvector 0.8 + pgvectorscale | Zero infra adicional, HNSW, iterative scanning |
| Vector DB (offline) | LanceDB + IVF-PQ + DiskANN | In-process, mmap, Rust core, Tantivy integrado |
| Full-text search | PostgreSQL tsvector (online), Tantivy (offline) | Nativo en cada engine |
| Graph | PostgreSQL tablas + CTEs recursivos | <100K nodos, zero servicio adicional |
| Cache | L1 LRU (5min) → L2 Semantic (24h) → L3 Redis (1h) | Multi-nivel, semantic cache reduce queries |
| Object store | PostgreSQL (online), filesystem (offline) | TOAST compression, mmap |

---

## 4. Diseño del MCP Documentation Agent

**Referencia completa**: [03-mcp-doc-agent.md](./03-mcp-doc-agent.md)

### Resumen de Arquitectura

```
TRIGGER → PLANNER → EXECUTOR → REVIEWER → OUTPUT
              ↑                      │
              └──── REVISION LOOP ───┘ (max 2 iterations)

Memory: Episodic + Semantic + Procedural + Repository Context
Tools: 15+ MCP tools (read, write, search, validate, diagram)
Modes: Interactive (conversational) | Batch (CI/CD triggered)
Security: Write operations require approval, escalation policy
```

---

## 5. APIs/Interfaces TypeScript Consolidadas

### 5.1 Core API Surface

```typescript
// ============================================================
// CUERVO KNOWLEDGE SYSTEM — COMPLETE API SURFACE
// ============================================================

// ─── KNOWLEDGE STORE API ─────────────────────────────
export interface IKnowledgeStore {
  // Search
  search(query: SearchQuery): Promise<SearchResponse>;
  semanticSearch(query: SemanticQuery): Promise<SearchResponse>;
  keywordSearch(query: KeywordQuery): Promise<SearchResponse>;
  graphSearch(query: GraphQuery): Promise<GraphSearchResponse>;
  hierarchicalSearch(query: HierarchicalQuery): Promise<HierarchicalResponse>;

  // Ingest
  ingest(request: IngestRequest): Promise<IngestResponse>;
  reindex(documentIds: string[]): Promise<IngestResponse>;
  remove(documentIds: string[]): Promise<void>;

  // Metadata
  stats(): Promise<KnowledgeStats>;
  getDocument(documentId: string): Promise<DocumentWithChunks>;
  getChunk(chunkId: string, includeContext?: boolean): Promise<ChunkWithContext>;
  getRelations(chunkId: string, depth?: number): Promise<ChunkRelation[]>;
}

// ─── EMBEDDING SERVICE ───────────────────────────────
export interface IEmbeddingService {
  embed(text: string, model?: EmbeddingModel): Promise<EmbeddingResult>;
  embedBatch(texts: string[], model?: EmbeddingModel): Promise<EmbeddingResult[]>;
  selectModel(contentType: ContentType, mode: OperationMode): EmbeddingModel;
}

// ─── CHUNKING SERVICE ────────────────────────────────
export interface IChunkingService {
  chunk(document: StructuredDocument): Promise<Chunk[]>;
  selectStrategy(document: StructuredDocument): ChunkingStrategy;
}

// ─── DOC AGENT ───────────────────────────────────────
export interface IDocAgent {
  // Autonomous operations
  plan(trigger: AgentTrigger, context: PlanningContext): Promise<ExecutionPlan>;
  execute(plan: ExecutionPlan): Promise<ExecutionResult>;
  review(artifact: DocArtifact): Promise<ReviewResult>;

  // Interactive operations
  chat(message: string, sessionId: string): Promise<AgentResponse>;
  approveAction(actionId: string): Promise<void>;
  rejectAction(actionId: string, feedback: string): Promise<void>;

  // Memory
  recallEpisodes(query: string, limit: number): Promise<Episode[]>;
  getConventions(repository: string): Promise<Convention[]>;

  // Reporting
  qualityReport(repository: string): Promise<DocQualityReport>;
  coverageReport(repository: string): Promise<DocCoverageReport>;
}

// ─── DOCOPS SERVICE ──────────────────────────────────
export interface IDocOpsService {
  // Validation
  lint(paths: string[]): Promise<LintResult[]>;
  checkLinks(paths: string[]): Promise<LinkCheckResult[]>;
  validateExamples(paths: string[]): Promise<ExampleValidationResult[]>;

  // Coverage
  computeCoverage(repository: string): Promise<DocCoverageMetrics>;
  computeQualityScore(docPath: string): Promise<DQSReport>;

  // Freshness
  detectStale(repository: string): Promise<StalenessReport>;

  // Sync
  checkSync(changedFiles: string[]): Promise<SyncCheckResult>;
  enforcePolicies(pr: PullRequest): Promise<PolicyResult>;
}

// ─── SYNC ENGINE ─────────────────────────────────────
export interface ISyncEngine {
  pushToCloud(localChanges: ChangeSet): Promise<SyncResult>;
  pullFromCloud(since: Date): Promise<ChangeSet>;
  resolveConflicts(conflicts: Conflict[]): Promise<Resolution[]>;
  getStatus(): Promise<SyncStatus>;
}
```

### 5.2 Domain Types

```typescript
// ============================================================
// DOMAIN TYPES
// ============================================================

// ─── CORE ENTITIES ───────────────────────────────────

export interface Document {
  id: string;
  filePath: string;
  repository: string;
  branch: string;
  title: string;
  docType: DocumentType;
  language: string | null;
  contentHash: string;
  tokenCount: number;
  sectionPath: string[];
  frontmatter: Record<string, unknown> | null;
  summary: string | null;
  commitSha: string;
  author: string | null;
  lastModified: Date;
  indexedAt: Date;
}

export interface Chunk {
  id: string;
  documentId: string;
  chunkIndex: number;
  content: string;
  contextualPrefix: string | null;
  contentHash: string;
  contentType: ContentType;
  breadcrumb: string[];
  lineStart: number | null;
  lineEnd: number | null;
  symbolName: string | null;
  symbolType: SymbolType | null;
  dependencies: string[] | null;
  isExported: boolean;
  domainTags: string[];
  tokenCount: number;
  hasCode: boolean;
  hasTable: boolean;
  hasDiagram: boolean;
  completeness: number;
  embeddingModel: string;
  embeddingVersion: string;
  commitSha: string;
  indexedAt: Date;
}

export interface ChunkRelation {
  id: string;
  sourceChunkId: string;
  targetChunkId: string;
  relationType: RelationType;
  confidence: number;
  metadata: Record<string, unknown> | null;
}

// ─── ENUMS ───────────────────────────────────────────

export type DocumentType = 'markdown' | 'code' | 'api_spec' | 'config' | 'adr';

export type ContentType =
  | 'prose'
  | 'code'
  | 'api_endpoint'
  | 'config'
  | 'table'
  | 'diagram'
  | 'adr'
  | 'requirement'
  | 'use_case';

export type SymbolType =
  | 'function'
  | 'class'
  | 'interface'
  | 'type'
  | 'enum'
  | 'const'
  | 'module'
  | 'method'
  | 'property';

export type RelationType =
  | 'references'
  | 'implements'
  | 'extends'
  | 'depends_on'
  | 'supersedes'
  | 'documents'
  | 'tested_by'
  | 'configured_by'
  | 'related_to'
  | 'contradicts'
  | 'elaborates';

export type EmbeddingModel = 'voyage-3.5' | 'cohere-embed-v4' | 'bge-m3-local';
export type OperationMode = 'online' | 'offline' | 'auto';

// ─── SEARCH TYPES ────────────────────────────────────

export interface SearchQuery {
  text: string;
  filters?: SearchFilters;
  options?: SearchOptions;
}

export interface SearchFilters {
  repositories?: string[];
  contentTypes?: ContentType[];
  domains?: string[];
  languages?: string[];
  dateRange?: { from?: Date; to?: Date };
  symbolTypes?: SymbolType[];
  branches?: string[];
}

export interface SearchOptions {
  topK?: number;
  reranking?: boolean;
  includeContext?: boolean;
  includeRelations?: boolean;
  minScore?: number;
  embeddingModel?: EmbeddingModel;
  mode?: OperationMode;
}

export interface SearchResponse {
  results: SearchResult[];
  metadata: SearchMetadata;
}

export interface SearchResult {
  chunk: ChunkSummary;
  document: DocumentSummary;
  score: number;
  scoreBreakdown?: ScoreBreakdown;
  relations?: ChunkRelation[];
  context?: ChunkContext;
}

export interface SearchMetadata {
  totalCandidates: number;
  searchTimeMs: number;
  retrievalMethod: string;
  rerankerUsed: string | null;
  cacheHit: boolean;
  engineUsed: 'postgresql' | 'lancedb';
}

// ─── INGEST TYPES ────────────────────────────────────

export interface IngestRequest {
  sources: IngestSource[];
  options?: IngestOptions;
}

export interface IngestSource {
  type: 'file' | 'directory' | 'git_diff' | 'raw_content';
  path?: string;
  content?: string;
  repository: string;
  branch?: string;
  commitSha?: string;
  metadata?: Record<string, unknown>;
}

export interface IngestResponse {
  jobId: string;
  status: 'queued' | 'processing' | 'completed' | 'failed';
  documentsProcessed: number;
  chunksCreated: number;
  chunksUpdated: number;
  chunksDeleted: number;
  embeddingsGenerated: number;
  relationsExtracted: number;
  costUsd: number;
  durationMs: number;
  errors?: IngestError[];
}

// ─── AGENT TYPES ─────────────────────────────────────

export interface ExecutionPlan {
  id: string;
  trigger: AgentTrigger;
  context: PlanningContext;
  tasks: PlannedTask[];
  estimatedDuration: number;
  confidenceScore: number;
  requiresHumanApproval: boolean;
}

export interface PlannedTask {
  id: string;
  type: TaskType;
  priority: number;
  description: string;
  targetFile: string;
  action: 'create' | 'update' | 'validate' | 'delete' | 'diagram';
  dependencies: string[];
  requiredContext: ContextRequirement[];
  estimatedTokens: number;
  confidence: number;
}

export interface ReviewResult {
  overallScore: number;
  action: 'pass' | 'revise' | 'reject';
  dimensions: QualityDimensions;
  feedback: ReviewFeedback[];
}

export interface QualityDimensions {
  accuracy: number;
  completeness: number;
  consistency: number;
  clarity: number;
  maintainability: number;
}

// ─── DOCOPS TYPES ────────────────────────────────────

export interface DQSReport {
  path: string;
  overall: number;
  dimensions: Record<string, { score: number; checks: Record<string, boolean> }>;
  trend: { previousScore: number | null; direction: string };
}

export interface DocCoverageMetrics {
  api: { totalPublicExports: number; documentedExports: number; coveragePercent: number };
  conceptual: { totalFeatures: number; documentedFeatures: number; coveragePercent: number };
  endpoints: { totalEndpoints: number; documentedEndpoints: number; coveragePercent: number };
  freshness: { totalDocs: number; freshDocs: number; staleDocs: number; averageStalenessHours: number };
}
```

---

## 6. Modelo de Datos Consolidado

### 6.1 Entity-Relationship Diagram

```
┌────────────────────┐     1:N     ┌────────────────────┐
│    documents       │────────────▶│     chunks          │
│                    │             │                    │
│  id (PK)           │             │  id (PK)           │
│  file_path         │             │  document_id (FK)  │
│  repository        │             │  chunk_index       │
│  branch            │             │  content           │
│  title             │             │  contextual_prefix │
│  doc_type          │             │  content_type      │
│  content_hash      │             │  breadcrumb[]      │
│  token_count       │             │  symbol_name       │
│  summary           │             │  domain_tags[]     │
│  summary_embedding │             │  embedding_voyage  │
│  search_vector     │             │  embedding_cohere  │
│  commit_sha        │             │  embedding_local   │
│  tenant_id         │             │  search_vector     │
│  team_ids[]        │             │  content_hash      │
│  visibility        │             │  minhash           │
└────────────────────┘             │  tenant_id         │
                                    │  team_ids[]        │
                                    └─────────┬──────────┘
                                              │
                                    N:M       │       N:M
                                    ┌─────────┴──────────┐
                                    │                    │
                              ┌─────▼────────┐  ┌───────▼──────┐
                              │chunk_relations│  │ query_log    │
                              │              │  │              │
                              │ id (PK)      │  │ id (PK)      │
                              │ source_id(FK)│  │ query_text   │
                              │ target_id(FK)│  │ query_embed  │
                              │ relation_type│  │ results_count│
                              │ confidence   │  │ latency_ms   │
                              │ metadata     │  │ user_feedback│
                              └──────────────┘  └──────────────┘

┌────────────────────┐     ┌─────────────────┐     ┌──────────────────┐
│embedding_versions  │     │ ingestion_jobs  │     │ agent_episodes   │
│                    │     │                 │     │                  │
│ id (PK)            │     │ id (PK)         │     │ id (PK)          │
│ version_hash       │     │ trigger_type    │     │ session_id       │
│ model_name         │     │ status          │     │ task_type        │
│ model_version      │     │ docs_total      │     │ outcome          │
│ dimensions         │     │ docs_processed  │     │ quality_score    │
│ chunking_version   │     │ chunks_created  │     │ user_feedback    │
│ status             │     │ cost_usd        │     │ actions_taken    │
│ chunk_count        │     │ started_at      │     │ lessons_learned  │
│ coverage_pct       │     │ completed_at    │     │ repository       │
└────────────────────┘     └─────────────────┘     └──────────────────┘

┌────────────────────┐     ┌─────────────────┐
│agent_conventions   │     │ repo_context    │
│                    │     │                 │
│ id (PK)            │     │ id (PK)         │
│ repository         │     │ repository      │
│ category           │     │ context_type    │
│ rule               │     │ data (JSONB)    │
│ examples           │     │ computed_at     │
│ confidence         │     │ commit_sha      │
│ usage_count        │     └─────────────────┘
└────────────────────┘
```

### 6.2 Tablas Totales

| Tabla | Propósito | Volumen estimado (Year 1) |
|-------|----------|--------------------------|
| `documents` | Registro de documentos fuente | ~2,100 |
| `chunks` | Unidades atómicas de conocimiento | ~50,000 |
| `chunk_relations` | Grafo de conocimiento | ~100,000 |
| `embedding_versions` | Control de versiones | ~10 |
| `ingestion_jobs` | Tracking de ingestión | ~5,000 |
| `query_log` | Registro de consultas | ~50,000 |
| `agent_episodes` | Memoria episódica del agente | ~2,000 |
| `agent_conventions` | Convenciones aprendidas | ~200 |
| `repo_context` | Contexto de repositorio cacheado | ~50 |

---

## 7. Flujo DocOps (End-to-End)

```
┌──────────────────────────────────────────────────────────────────────────┐
│                    DOCOPS END-TO-END FLOW                                │
│                                                                          │
│  ════════════════════════════════════════════════════════════            │
│  DEVELOPER WORKFLOW                                                      │
│  ════════════════════════════════════════════════════════════            │
│                                                                          │
│  1. Developer writes code                                                │
│     └──▶ src/infrastructure/auth/oauth2.service.ts                      │
│                                                                          │
│  2. Developer commits (pre-commit hook)                                  │
│     ├── markdownlint validates staged .md files                         │
│     ├── Link checker validates internal links                            │
│     └── Pass → commit allowed                                            │
│                                                                          │
│  3. Developer pushes to feature branch                                   │
│     └──▶ Triggers CI pipeline                                            │
│                                                                          │
│  ════════════════════════════════════════════════════════════            │
│  CI PIPELINE (automated)                                                 │
│  ════════════════════════════════════════════════════════════            │
│                                                                          │
│  4. Doc Validation (parallel)                                            │
│     ├── markdownlint (structure)                                         │
│     ├── Vale (prose quality)                                             │
│     ├── Link checker (all links)                                         │
│     ├── Code example validator (TS compiles, JSON/YAML parses)          │
│     └── Mermaid validator (diagrams render)                              │
│                                                                          │
│  5. Doc Coverage Check                                                   │
│     ├── Extract public symbols from code                                 │
│     ├── Check which are documented                                       │
│     ├── Compute coverage % (target: ≥70%)                               │
│     └── Post coverage report as PR comment                               │
│                                                                          │
│  6. Doc Quality Score (DQS)                                              │
│     ├── Structural checks (automated)                                    │
│     ├── Content checks (automated)                                       │
│     ├── LLM quality evaluation (DocAgent Reviewer)                      │
│     └── Compute weighted DQS (target: ≥60/100)                         │
│                                                                          │
│  7. Freshness/Sync Check                                                 │
│     ├── Compare changed code files → sync rules                         │
│     ├── Identify affected documentation                                  │
│     ├── Flag stale docs                                                  │
│     └── For feature PRs: require doc changes                             │
│                                                                          │
│  8. Policy Enforcement                                                   │
│     ├── Feature label → docs required (error)                            │
│     ├── Breaking change → ADR required (error)                           │
│     ├── API change → API docs required (error)                           │
│     └── Coverage < 70% → block merge (error)                            │
│                                                                          │
│  ════════════════════════════════════════════════════════════            │
│  POST-MERGE (main branch)                                                │
│  ════════════════════════════════════════════════════════════            │
│                                                                          │
│  9. Knowledge Re-indexing                                                │
│     ├── Detect changed files                                             │
│     ├── Incremental re-chunk affected documents                          │
│     ├── Re-embed changed chunks                                          │
│     ├── Update graph relations                                           │
│     ├── Invalidate caches                                                │
│     └── Sync to LanceDB (offline mirror)                                 │
│                                                                          │
│  10. Auto-generation                                                     │
│      ├── Regenerate TypeDoc                                              │
│      ├── Regenerate API docs from OpenAPI                                │
│      ├── Update CHANGELOG.md                                             │
│      └── If changes → create auto-update PR                             │
│                                                                          │
│  11. DocAgent Batch (if stale docs detected)                             │
│      ├── Analyze stale docs + related code changes                       │
│      ├── Generate proposed updates                                       │
│      ├── Self-review quality                                             │
│      └── Create PR with proposed documentation updates                   │
│                                                                          │
│  ════════════════════════════════════════════════════════════            │
│  WEEKLY (scheduled)                                                      │
│  ════════════════════════════════════════════════════════════            │
│                                                                          │
│  12. Documentation Quality Audit                                         │
│      ├── Full scan of all docs                                           │
│      ├── Compute DQS for all documents                                   │
│      ├── Detect inconsistencies                                          │
│      ├── Coverage trend analysis                                         │
│      └── Generate weekly health report                                   │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## 8. Métricas de Calidad

### 8.1 KPIs del Knowledge System

| KPI | Target (MVP) | Target (Beta) | Target (GA) |
|-----|-------------|---------------|-------------|
| **Retrieval precision@5** | ≥0.70 | ≥0.80 | ≥0.90 |
| **Search latency p95** | <300ms | <200ms | <150ms |
| **Doc coverage (public APIs)** | ≥50% | ≥70% | ≥90% |
| **Average DQS** | ≥50/100 | ≥65/100 | ≥80/100 |
| **Stale docs** | <30% | <15% | <5% |
| **Agent success rate** | ≥70% | ≥80% | ≥90% |
| **Hallucination rate** | <10% | <5% | <2% |
| **Knowledge freshness** | <7 days | <3 days | <1 day |
| **Ingest latency (single doc)** | <5s | <3s | <2s |
| **Cache hit rate** | ≥30% | ≥50% | ≥60% |

### 8.2 Observability Dashboard

```
┌─────────────────────────────────────────────────────────────┐
│  KNOWLEDGE SYSTEM HEALTH DASHBOARD                           │
│                                                               │
│  ┌─────────────────────┐  ┌─────────────────────┐           │
│  │  DQS: 72/100        │  │  Coverage: 68%      │           │
│  │  ▲ +3 from last week│  │  ▲ +5% from last wk │           │
│  └─────────────────────┘  └─────────────────────┘           │
│                                                               │
│  ┌─────────────────────┐  ┌─────────────────────┐           │
│  │  Stale Docs: 4      │  │  Broken Links: 0    │           │
│  │  ▼ -2 from last week│  │  ✓ All passing      │           │
│  └─────────────────────┘  └─────────────────────┘           │
│                                                               │
│  ┌─────────────────────┐  ┌─────────────────────┐           │
│  │  Search P95: 142ms  │  │  Agent Success: 85% │           │
│  │  ✓ Under target     │  │  ✓ Above target     │           │
│  └─────────────────────┘  └─────────────────────┘           │
│                                                               │
│  Retrieval Quality (7-day trend)                              │
│  1.0 ┤                                                       │
│  0.8 ┤  ──●──●──●──●──●──●──●  precision@5                 │
│  0.6 ┤                                                       │
│  0.4 ┤                                                       │
│  0.0 ┤──────────────────────                                │
│       Mon Tue Wed Thu Fri Sat Sun                             │
│                                                               │
│  Recent Agent Activity                                        │
│  ├── ✓ Updated 02-iam-architecture.md (DQS: 78)             │
│  ├── ✓ Generated API doc for /auth/login (DQS: 82)          │
│  ├── ⚠ Escalated: contradiction in security docs            │
│  └── ✓ Weekly audit report generated                         │
└─────────────────────────────────────────────────────────────┘
```

---

## 9. Roadmap de Implementación

### 9.1 Fases y Sprints

```
═══════════════════════════════════════════════════════════════════
PHASE A: FOUNDATION (Sprints 1-4, ~8 weeks)
═══════════════════════════════════════════════════════════════════

Sprint 1-2 (Weeks 1-4): Knowledge Store Core
├── Set up PostgreSQL + pgvector 0.8
├── Implement chunk schema and indexes
├── Implement basic vector search
├── Implement BM25 (tsvector) search
├── Set up LanceDB for offline mode
├── Implement RRF fusion
└── Deliverable: hybrid search working end-to-end

Sprint 3-4 (Weeks 5-8): Ingestion Pipeline
├── Implement Markdown chunking (heading-aware semantic)
├── Implement Code chunking (Tree-sitter AST-aware)
├── Implement embedding router (BGE-M3 local first)
├── Implement change detection (git diff based)
├── Implement incremental re-indexing
├── Implement content hash deduplication
└── Deliverable: docs/ fully indexed and searchable

═══════════════════════════════════════════════════════════════════
PHASE B: INTELLIGENCE (Sprints 5-8, ~8 weeks)
═══════════════════════════════════════════════════════════════════

Sprint 5-6 (Weeks 9-12): Advanced Retrieval
├── Implement graph relations extraction
├── Implement graph traversal in search
├── Implement cross-encoder reranking (BGE-reranker ONNX)
├── Implement semantic cache
├── Implement contextual prefix generation
├── Add Voyage 3.5 and Cohere embed-v4 support
└── Deliverable: full hybrid search with graph + reranking

Sprint 7-8 (Weeks 13-16): DocAgent MVP
├── Implement MCP tool server (read tools)
├── Implement Planner component
├── Implement Executor component (doc generation)
├── Implement Reviewer component (quality evaluation)
├── Implement episodic memory
├── Basic interactive mode
└── Deliverable: DocAgent can generate docs from code

═══════════════════════════════════════════════════════════════════
PHASE C: AUTOMATION (Sprints 9-12, ~8 weeks)
═══════════════════════════════════════════════════════════════════

Sprint 9-10 (Weeks 17-20): DocOps Pipeline
├── Implement CI pipeline (GitHub Actions)
├── Implement pre-commit hooks
├── Implement doc coverage computation
├── Implement DQS scoring
├── Implement link/example validation
├── Implement freshness detection
└── Deliverable: full CI/CD doc quality gates

Sprint 11-12 (Weeks 21-24): Agent Maturity
├── Implement batch mode workflows
├── Implement staleness auto-remediation
├── Implement changelog generation
├── Implement diagram generation
├── Implement convention learning (memory)
├── Implement provenance tracking
├── Implement Langfuse integration
└── Deliverable: autonomous doc maintenance agent

═══════════════════════════════════════════════════════════════════
PHASE D: ENTERPRISE (Sprints 13-16, ~8 weeks)
═══════════════════════════════════════════════════════════════════

Sprint 13-14 (Weeks 25-28): Scale & Security
├── Implement multi-tenant (RLS)
├── Implement sync engine (PG ↔ LanceDB)
├── Implement evaluation framework
├── Implement audit trail
├── Implement PII detection in docs
└── Deliverable: enterprise-ready knowledge system

Sprint 15-16 (Weeks 29-32): Polish & Compliance
├── ISO 42001 alignment documentation
├── SOC 2 control evidence
├── Performance optimization
├── Evaluation dataset curation (50+ query-answer pairs)
├── Dashboard and reporting
├── Documentation of the knowledge system itself
└── Deliverable: production-ready, compliant system
```

### 9.2 Dependencies

```
Phase A (Foundation) ← No external dependencies
    │
    ▼
Phase B (Intelligence) ← Requires: Tree-sitter Rust module (treesitter.rs)
    │                    ← Requires: Embedding API keys (Voyage, Cohere)
    ▼
Phase C (Automation) ← Requires: CI/CD infrastructure (GitHub Actions)
    │                ← Requires: Phase B complete for DocAgent
    ▼
Phase D (Enterprise) ← Requires: Auth service (cuervo-auth-service)
                      ← Requires: Phase C complete for audit trail
```

### 9.3 Alignment with Cuervo Roadmap

| Knowledge System Phase | Cuervo Phase | Sprint Range |
|----------------------|-------------|-------------|
| Phase A: Foundation | MVP (Feb-May 2026) | Sprints 5-8 |
| Phase B: Intelligence | Beta (Jun-Sep 2026) | Sprints 9-12 |
| Phase C: Automation | Beta (Jun-Sep 2026) | Sprints 13-16 |
| Phase D: Enterprise | GA (Oct 2026-Jan 2027) | Sprints 17-20 |

---

## 10. Lista de Riesgos Técnicos

### 10.1 Riesgos por Probabilidad e Impacto

| # | Riesgo | Prob. | Impacto | Mitigación | Owner |
|---|--------|-------|---------|-----------|-------|
| R1 | **pgvector scaling** — Performance degrades beyond 1M vectors | Baja | Alto | Migration path to Qdrant documented; volumen Year 1 ~50K (20x headroom) | Infra |
| R2 | **Embedding model deprecation** — Voyage/Cohere discontinue model version | Media | Alto | Versioned embeddings + migration pipeline; BGE-M3 local as permanent fallback | ML |
| R3 | **LanceDB immaturity** — Production bugs in young project | Media | Medio | SQLite-vss as fallback; LanceDB backed by Lancedata (funded); active community | Infra |
| R4 | **Chunking quality** — Semantic chunking produces poor boundaries | Media | Alto | Evaluation dataset to measure; multiple strategies to A/B test; human review of sample | ML |
| R5 | **DocAgent hallucinations** — Agent generates incorrect documentation | Media | Muy alto | Hallucination guard + provenance tracking + mandatory human review for writes | AI Safety |
| R6 | **Cost overrun** — Embedding/LLM API costs exceed budget | Baja | Medio | Hard budget limits per run/month; offline BGE-M3 as zero-cost fallback | Ops |
| R7 | **Graph complexity** — PostgreSQL CTEs insufficient for deep traversal | Baja | Bajo | Apache AGE extension (same PG, zero migration) if >5 hop queries needed | Infra |
| R8 | **CI pipeline slowness** — DocOps adds >5min to PR checks | Media | Medio | Parallelize checks; cache validation results; skip unchanged docs | DevOps |
| R9 | **Knowledge store data leak** — Sensitive code indexed without permission | Baja | Muy alto | RLS enforcement; PII detector (pii.rs); .cuervoignore; audit logging | Security |
| R10 | **Sync conflicts** — Online/offline knowledge stores diverge | Media | Medio | Last-write-wins default; content hash comparison; manual merge for conflicts | Infra |
| R11 | **Developer friction** — DocOps policies too strict, slow PRs | Media | Alto | Start with warnings, escalate to blocks gradually; measure developer sentiment | Product |
| R12 | **MCP spec changes** — June 2026 spec breaks existing tools | Baja | Medio | Abstract MCP transport behind interface; spec changes are additive by design | Platform |
| R13 | **Evaluation drift** — Golden dataset becomes stale | Media | Medio | Quarterly review of evaluation dataset; augment with production query logs | ML |
| R14 | **Token budget exceeded** — DocAgent uses too many tokens per task | Media | Bajo | Per-task token limits; streaming generation; early termination | ML/Ops |

### 10.2 Risk Heat Map

```
                    IMPACT
            Low     Medium    High    Very High
        ┌─────────┬─────────┬─────────┬─────────┐
  High  │         │         │         │         │
  Prob  │         │         │         │         │
        ├─────────┼─────────┼─────────┼─────────┤
  Med   │         │ R10,R14 │ R4,R11  │ R5      │
  Prob  │         │ R8,R13  │ R2      │         │
        ├─────────┼─────────┼─────────┼─────────┤
  Low   │ R7      │ R6,R12  │ R1,R3   │ R9      │
  Prob  │         │         │         │         │
        └─────────┴─────────┴─────────┴─────────┘
```

### 10.3 Top 3 Riesgos Críticos

1. **R5 — DocAgent hallucinations**: Mitigado con triple safety net (self-check → reviewer → human approval). Budget: invest in hallucination guard and provenance tracking.

2. **R9 — Knowledge store data leak**: Mitigado con defense-in-depth (RLS + PII detector + .cuervoignore + audit). Budget: prioritize security review.

3. **R4 — Chunking quality**: Mitigado con evaluation-driven development. Budget: invest in golden dataset creation (50+ curated query-answer pairs).

---

## Apéndice A: Resumen de Documentos en Esta Sección

| # | Documento | Contenido |
|---|-----------|-----------|
| 01 | [01-vectorization-strategy.md](./01-vectorization-strategy.md) | Chunking semántico, embeddings, pipeline de ingestión, deduplicación |
| 02 | [02-knowledge-store.md](./02-knowledge-store.md) | Almacenamiento híbrido, vector DB, graph, caching, offline-first |
| 03 | [03-mcp-doc-agent.md](./03-mcp-doc-agent.md) | Arquitectura PER del agente, tools MCP, memoria, workflows |
| 04 | [04-docops-automation.md](./04-docops-automation.md) | CI/CD, hooks, coverage, quality scoring, sync |
| 05 | [05-best-practices-2026.md](./05-best-practices-2026.md) | RAG avanzado, LLMOps, compliance, trade-offs |
| 06 | [06-consolidated-deliverables.md](./06-consolidated-deliverables.md) | Este documento — interfaces, modelo de datos, roadmap, riesgos |

---

*Fin de la sección 09-knowledge-system*
