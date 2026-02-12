# Fase 2 — Arquitectura del Knowledge Store

> **Documento**: `09-knowledge-system/02-knowledge-store.md`
> **Versión**: 1.0.0
> **Fecha**: 2026-02-06
> **Autores**: Software Architect, Knowledge Systems Architect, LLMOps Engineer
> **Estado**: Design Complete

---

## Índice

1. [Evaluación de Tecnologías](#1-evaluación-de-tecnologías)
2. [Arquitectura Híbrida Recomendada](#2-arquitectura-híbrida-recomendada)
3. [Diagrama de Componentes](#3-diagrama-de-componentes)
4. [Capa de Almacenamiento Vectorial](#4-capa-de-almacenamiento-vectorial)
5. [Capa de Búsqueda Keyword (BM25)](#5-capa-de-búsqueda-keyword)
6. [Capa de Grafo de Conocimiento](#6-capa-de-grafo-de-conocimiento)
7. [Object Store para Documentos](#7-object-store-para-documentos)
8. [Estrategia de Caching](#8-estrategia-de-caching)
9. [Estrategia Offline-First](#9-estrategia-offline-first)
10. [Multi-Tenant y Seguridad por Contexto](#10-multi-tenant-y-seguridad-por-contexto)
11. [Query Patterns y Contratos de Acceso](#11-query-patterns-y-contratos-de-acceso)
12. [Replicación y Alta Disponibilidad](#12-replicación-y-alta-disponibilidad)

---

## 1. Evaluación de Tecnologías

### 1.1 Vector Database — Análisis Comparativo

| Criterio | pgvector 0.8 | Qdrant | Milvus/Zilliz | Weaviate | LanceDB |
|----------|-------------|--------|---------------|----------|---------|
| **Rendimiento (50K vecs)** | 471 QPS (pgvectorscale) | 41 QPS @ 50M | Highest raw throughput | Good @ moderate | Excellent local |
| **Integración PostgreSQL** | ★★★★★ Nativo | ★★ Requiere servicio separado | ★★ Servicio separado | ★★ Servicio separado | ★★★ Embeddable |
| **Hybrid search nativo** | ★★★★ FTS + vector | ★★★★★ Sparse+dense+multivector | ★★★★ Sparse+dense | ★★★★★ Invertido+vector | ★★★★ Tantivy+vector |
| **Offline/embeddable** | ★★★ SQLite-like posible | ★★ Requiere proceso separado | ★ Cloud-first | ★★ Proceso separado | ★★★★★ In-process, mmap |
| **Filtrado + vector** | ★★★★★ Iterative scanning (0.8) | ★★★★★ Payload filtering | ★★★★ Attribute filtering | ★★★★ Where filtering | ★★★★ SQL-like predicates |
| **Mantenimiento** | ★★★★★ Ya en stack (PostgreSQL) | ★★★ Servicio adicional | ★★ Complejidad ops | ★★★ Complejidad moderada | ★★★★★ Zero ops |
| **Escalabilidad** | ★★★★ Hasta ~10M vectors | ★★★★★ Billones | ★★★★★ Billones | ★★★★ Millones | ★★★ Hasta ~10M |
| **Costo** | $0 adicional | $0 (self-hosted) | $$-$$$ | $-$$ | $0 |
| **Maturity** | ★★★★★ PostgreSQL ecosystem | ★★★★ Producción probada | ★★★★★ Battle-tested | ★★★★ Estable | ★★★ Joven pero sólido |
| **ColBERT/Multivector** | ★★ No nativo | ★★★★★ Nativo | ★★★ Via plugin | ★★★ Via módulo | ★★ No nativo |

### 1.2 Decisión: Arquitectura Dual

**Selección primaria (Cloud/Enterprise)**: **pgvector 0.8 + pgvectorscale**

Justificación:
- PostgreSQL ya es parte del stack de Cuervo (decisión de ecosistema)
- pgvector 0.8 introduce **iterative index scanning** que resuelve el problema histórico de filtrado + vector search
- pgvectorscale alcanza **28x mejor p95 latency** y **16x mayor throughput** que Pinecone s1
- Elimina un servicio adicional en la infraestructura
- SQL nativo para queries complejas combinando vector + relacional + full-text
- HNSW index para sub-10M vectors es óptimo para nuestro volumen (~50K chunks year 1)

**Selección secundaria (Offline/Local)**: **LanceDB**

Justificación:
- Decisión ya tomada en ADR-009 del proyecto
- Embeddable (in-process), zero-config, mmap I/O
- Rust core con bindings TypeScript vía napi-rs (alineado con Rust performance layer)
- Tantivy integrado para hybrid search (BM25 + vector en un solo motor)
- IVF-PQ + DiskANN para indexación eficiente en disco
- Perfect para modo offline-first del CLI

**Futura evaluación**: **Qdrant** si se requiere ColBERT/multivector nativo para reranking de alta calidad (evaluar en Beta, sprint 12+).

### 1.3 Keyword Search — Evaluación

| Criterio | PostgreSQL FTS | MeiliSearch | OpenSearch | Tantivy (LanceDB) |
|----------|---------------|-------------|-----------|-------------------|
| **Ya en stack** | ★★★★★ | ★★★★★ (ecosistema Cuervo) | ★★ | ★★★★ (via LanceDB) |
| **Typo tolerance** | ★★ | ★★★★★ | ★★★★ | ★★★ |
| **Multilingual** | ★★★★ (diccionarios) | ★★★★★ (auto-detect) | ★★★★★ | ★★★ |
| **Faceted search** | ★★★ | ★★★★★ | ★★★★★ | ★★ |
| **Latencia** | ★★★★ (~8ms) | ★★★★★ (<2ms) | ★★★ (~15ms) | ★★★★ (~5ms) |
| **Offline** | ★★★ (requiere PG) | ★★★ (proceso) | ★ | ★★★★★ (in-process) |

**Decisión**:
- **Cloud**: PostgreSQL FTS (tsvector) — ya integrado, cero overhead
- **Offline**: Tantivy via LanceDB — búsqueda full-text in-process
- **UI de búsqueda de docs (futuro)**: MeiliSearch — ya en stack para búsqueda user-facing

### 1.4 Graph Database — Evaluación

| Criterio | PostgreSQL (tablas) | Neo4j | Apache AGE | TypeDB |
|----------|-------------------|-------|-----------|--------|
| **Escala requerida** | ★★★★★ <100K nodos | ★★★★★ Millones | ★★★★ | ★★★★ |
| **Complejidad ops** | ★★★★★ Zero (ya existe) | ★★ Servicio adicional | ★★★★ Extension PG | ★★ Servicio adicional |
| **Query language** | SQL + CTEs recursivos | Cypher | Cypher-like | TypeQL |
| **Traversal profundo** | ★★★ OK para 2-3 hops | ★★★★★ Optimizado | ★★★★ | ★★★★★ |
| **Integración vector** | ★★★★★ Mismo PG | ★★ Separado | ★★★★ Mismo PG | ★★ Separado |

**Decisión**: **PostgreSQL con tablas de relaciones + CTEs recursivos**

Justificación:
- Nuestro grafo tiene <100K nodos y traversals de max 3 hops
- PostgreSQL CTEs recursivos son suficientes para este volumen
- Evita un servicio adicional
- Las relaciones se almacenan en la tabla `chunk_relations` (ya diseñada en Fase 1)
- Si en futuro se requiere traversal profundo (>5 hops), migrar a Apache AGE (extensión PostgreSQL, zero cambio de infra)

---

## 2. Arquitectura Híbrida Recomendada

### 2.1 Principio: "Unified Store, Dual Engine"

```
┌──────────────────────────────────────────────────────────────┐
│                    KNOWLEDGE STORE                            │
│                                                               │
│   ┌─────────────────────────────────────────────────────┐    │
│   │              QUERY INTERFACE (API)                   │    │
│   │        KnowledgeQueryService (TypeScript)            │    │
│   └──────────────────────┬──────────────────────────────┘    │
│                          │                                    │
│   ┌──────────────────────┼──────────────────────────────┐    │
│   │              QUERY ORCHESTRATOR                      │    │
│   │   ┌──────────┐ ┌──────────┐ ┌──────────────┐       │    │
│   │   │  Vector  │ │  BM25    │ │    Graph      │       │    │
│   │   │ Retriever│ │ Retriever│ │  Retriever    │       │    │
│   │   └────┬─────┘ └────┬─────┘ └──────┬───────┘       │    │
│   │        │             │              │               │    │
│   │   ┌────┴─────────────┴──────────────┴───────┐       │    │
│   │   │          RRF Fusion + Re-ranking        │       │    │
│   │   └─────────────────────────────────────────┘       │    │
│   └─────────────────────────────────────────────────────┘    │
│                          │                                    │
│          ┌───────────────┼───────────────┐                   │
│          │               │               │                   │
│   ┌──────▼──────┐ ┌─────▼──────┐ ┌──────▼──────┐           │
│   │  ONLINE     │ │  OFFLINE   │ │   CACHE     │           │
│   │  ENGINE     │ │  ENGINE    │ │   LAYER     │           │
│   │             │ │            │ │             │           │
│   │ PostgreSQL  │ │  LanceDB   │ │   Redis +   │           │
│   │ + pgvector  │ │  + Tantivy │ │   In-Memory │           │
│   │ + FTS       │ │            │ │   LRU       │           │
│   │ + Relations │ │            │ │             │           │
│   └─────────────┘ └────────────┘ └─────────────┘           │
│                                                               │
│   ┌─────────────────────────────────────────────────────┐    │
│   │              SYNC ENGINE                             │    │
│   │   Online ←→ Offline bidirectional sync               │    │
│   │   Conflict resolution: last-write-wins + merge       │    │
│   └─────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────┘
```

### 2.2 Decisiones Arquitectónicas

| Decisión | Selección | Alternativa descartada | Justificación |
|----------|-----------|----------------------|---------------|
| Almacenamiento dual | PG (online) + LanceDB (offline) | Solo PG | Offline-first es requisito core (ADR-007) |
| Grafo en PG | Tablas + CTEs | Neo4j | Volumen <100K nodos, zero servicio adicional |
| BM25 en PG | tsvector nativo | MeiliSearch | Para RAG, tsvector es suficiente. MeiliSearch para UI |
| Cache multinivel | Memory + Redis | Solo Redis | L1 in-memory reduce latencia a <1ms para hot queries |
| Sync bidireccional | Event-based | Polling | Menor latencia, menor carga |
| Query orchestration | In-process (TS) | Servicio separado | Latencia, simplicidad para CLI |

---

## 3. Diagrama de Componentes

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         CUERVO KNOWLEDGE SYSTEM                         │
│                                                                         │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │                      APPLICATION LAYER                          │   │
│  │                                                                  │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐  │   │
│  │  │ SearchUseCase │  │ IngestUseCase│  │ DocAgentUseCase      │  │   │
│  │  │              │  │              │  │                      │  │   │
│  │  │ - semantic() │  │ - ingest()   │  │ - generateDoc()     │  │   │
│  │  │ - keyword()  │  │ - reindex()  │  │ - validateDoc()     │  │   │
│  │  │ - hybrid()   │  │ - delete()   │  │ - updateDoc()       │  │   │
│  │  │ - graph()    │  │ - sync()     │  │ - auditDoc()        │  │   │
│  │  └──────┬───────┘  └──────┬───────┘  └──────────┬──────────┘  │   │
│  └─────────┼─────────────────┼──────────────────────┼──────────────┘   │
│            │                 │                      │                   │
│  ┌─────────┼─────────────────┼──────────────────────┼──────────────┐   │
│  │         │         DOMAIN LAYER                   │              │   │
│  │         │                 │                      │              │   │
│  │  ┌──────▼───────┐ ┌──────▼───────┐ ┌────────────▼─────────┐   │   │
│  │  │ KnowledgeBase│ │   Document   │ │     Chunk            │   │   │
│  │  │  (Aggregate) │ │   (Entity)   │ │     (Entity)         │   │   │
│  │  │              │ │              │ │                      │   │   │
│  │  │ - search()   │ │ - sections[] │ │ - content            │   │   │
│  │  │ - index()    │ │ - metadata   │ │ - embedding          │   │   │
│  │  │ - validate() │ │ - version    │ │ - metadata           │   │   │
│  │  └──────────────┘ └──────────────┘ │ - relations[]        │   │   │
│  │                                     └──────────────────────┘   │   │
│  │  ┌────────────────┐ ┌──────────────────┐ ┌────────────────┐   │   │
│  │  │ ChunkRelation  │ │ EmbeddingVersion │ │ IngestionJob   │   │   │
│  │  │ (Value Object) │ │ (Value Object)   │ │ (Entity)       │   │   │
│  │  └────────────────┘ └──────────────────┘ └────────────────┘   │   │
│  │                                                                │   │
│  │  ┌──────────────────────────────────────────────────────────┐  │   │
│  │  │               REPOSITORY INTERFACES (Ports)              │  │   │
│  │  │                                                          │  │   │
│  │  │  IVectorRepository    IDocumentRepository                │  │   │
│  │  │  ISearchRepository    IGraphRepository                   │  │   │
│  │  │  IEmbeddingService    ICacheRepository                   │  │   │
│  │  └──────────────────────────────────────────────────────────┘  │   │
│  └────────────────────────────────────────────────────────────────┘   │
│                                                                       │
│  ┌────────────────────────────────────────────────────────────────┐   │
│  │                   INFRASTRUCTURE LAYER                         │   │
│  │                                                                │   │
│  │  ┌─────────────────┐  ┌──────────────────┐                   │   │
│  │  │  PgVectorRepo   │  │  LanceDBRepo     │                   │   │
│  │  │  (online)       │  │  (offline)        │                   │   │
│  │  │                 │  │                   │                   │   │
│  │  │  - pgvector 0.8 │  │  - LanceDB       │                   │   │
│  │  │  - tsvector     │  │  - Tantivy        │                   │   │
│  │  │  - relations    │  │  - IVF-PQ         │                   │   │
│  │  └─────────────────┘  └──────────────────┘                   │   │
│  │                                                                │   │
│  │  ┌─────────────────┐  ┌──────────────────┐                   │   │
│  │  │  EmbeddingRouter│  │  CacheService    │                   │   │
│  │  │                 │  │                   │                   │   │
│  │  │  - Voyage 3.5   │  │  - L1: LRU       │                   │   │
│  │  │  - Cohere v4    │  │  - L2: Redis     │                   │   │
│  │  │  - BGE-M3       │  │  - Semantic cache │                   │   │
│  │  └─────────────────┘  └──────────────────┘                   │   │
│  │                                                                │   │
│  │  ┌─────────────────┐  ┌──────────────────┐                   │   │
│  │  │  GitWatcher     │  │  SyncEngine      │                   │   │
│  │  │                 │  │                   │                   │   │
│  │  │  - webhooks     │  │  - PG → LanceDB  │                   │   │
│  │  │  - polling      │  │  - LanceDB → PG  │                   │   │
│  │  │  - file watch   │  │  - conflict res.  │                   │   │
│  │  └─────────────────┘  └──────────────────┘                   │   │
│  └────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 4. Capa de Almacenamiento Vectorial

### 4.1 PostgreSQL + pgvector 0.8 (Online Engine)

**Configuración de producción**:

```sql
-- Configuración óptima para pgvector 0.8
ALTER SYSTEM SET shared_buffers = '4GB';
ALTER SYSTEM SET effective_cache_size = '12GB';
ALTER SYSTEM SET maintenance_work_mem = '2GB';
ALTER SYSTEM SET max_parallel_workers_per_gather = 4;

-- Configuración específica de pgvector
SET hnsw.ef_search = 100;                    -- Balance precision/speed
SET ivfflat.probes = 20;                      -- Para IVF fallback

-- Iterative scanning (nueva feature 0.8)
-- Previene overfiltering cuando se combinan vector + WHERE
SET hnsw.iterative_scan = 'relaxed_order';   -- Permite resultados cercanos a threshold
```

**Queries de ejemplo**:

```sql
-- Búsqueda semántica con filtrado por metadata
SELECT
    c.id,
    c.content,
    c.breadcrumb,
    c.symbol_name,
    c.contextual_prefix,
    1 - (c.embedding_cohere <=> $1::vector) AS similarity
FROM chunks c
JOIN documents d ON c.document_id = d.id
WHERE d.repository = $2
  AND c.content_type = ANY($3::text[])
  AND c.domain_tags && $4::text[]           -- Array overlap (GIN index)
ORDER BY c.embedding_cohere <=> $1::vector
LIMIT 50;

-- Búsqueda híbrida (vector + BM25) con RRF en SQL
WITH vector_results AS (
    SELECT c.id, ROW_NUMBER() OVER (
        ORDER BY c.embedding_cohere <=> $1::vector
    ) AS rank
    FROM chunks c
    WHERE c.content_type = ANY($2::text[])
    ORDER BY c.embedding_cohere <=> $1::vector
    LIMIT 50
),
bm25_results AS (
    SELECT c.id, ROW_NUMBER() OVER (
        ORDER BY ts_rank_cd(c.search_vector, websearch_to_tsquery('spanish', $3)) DESC
    ) AS rank
    FROM chunks c
    WHERE c.search_vector @@ websearch_to_tsquery('spanish', $3)
    LIMIT 50
),
fused AS (
    SELECT
        COALESCE(v.id, b.id) AS chunk_id,
        COALESCE(0.50 * (1.0 / (v.rank + 60)), 0) +
        COALESCE(0.35 * (1.0 / (b.rank + 60)), 0) AS rrf_score
    FROM vector_results v
    FULL OUTER JOIN bm25_results b ON v.id = b.id
)
SELECT
    f.chunk_id,
    f.rrf_score,
    c.content,
    c.contextual_prefix,
    c.breadcrumb,
    c.symbol_name
FROM fused f
JOIN chunks c ON f.chunk_id = c.id
ORDER BY f.rrf_score DESC
LIMIT 20;
```

### 4.2 LanceDB (Offline Engine)

**Configuración e integración**:

```typescript
import { connect, Table } from '@lancedb/lancedb';

interface LanceDBConfig {
  dbPath: string;                     // ~/.cuervo/knowledge/lance.db
  tableName: string;                  // 'chunks'
  indexType: 'IVF_PQ' | 'IVF_HNSW_SQ';
  nPartitions: number;                // 256 para ~50K vectors
  nSubVectors: number;                // 64
  ftsEnabled: boolean;                // true — Tantivy
}

class LanceDBKnowledgeStore {
  private db: Connection;
  private table: Table;

  async initialize(config: LanceDBConfig): Promise<void> {
    this.db = await connect(config.dbPath);

    // Crear o abrir tabla
    this.table = await this.db.openTable(config.tableName);

    // Crear índice vectorial
    await this.table.createIndex('embedding', {
      type: config.indexType,
      num_partitions: config.nPartitions,
      num_sub_vectors: config.nSubVectors,
    });

    // Crear índice FTS (Tantivy)
    if (config.ftsEnabled) {
      await this.table.createIndex('content', {
        type: 'FTS',
        with_position: true,          // Para phrase queries
      });
    }
  }

  async hybridSearch(
    queryEmbedding: Float32Array,
    queryText: string,
    filters: Record<string, unknown>,
    topK: number = 10
  ): Promise<SearchResult[]> {
    // LanceDB hybrid search combina vector + FTS nativamente
    const vectorResults = await this.table
      .search(queryEmbedding)
      .where(this.buildFilter(filters))
      .limit(50)
      .toArray();

    const ftsResults = await this.table
      .search(queryText, { queryType: 'fts' })
      .where(this.buildFilter(filters))
      .limit(50)
      .toArray();

    // RRF Fusion
    return this.rrfFusion(vectorResults, ftsResults, topK);
  }
}
```

### 4.3 Capacidad por Motor

| Métrica | pgvector 0.8 | LanceDB |
|---------|-------------|---------|
| Vectores máximo recomendado | ~10M (HNSW) | ~10M (IVF-PQ) |
| Volumen Cuervo Year 1 | ~50K chunks | ~50K chunks |
| Volumen Cuervo Year 3 | ~500K chunks | ~500K chunks |
| Headroom | 20x | 20x |
| Latencia p50 (50K) | ~15ms | ~8ms |
| Latencia p95 (50K) | ~40ms | ~20ms |
| Disk usage (50K, 1024d) | ~200MB vectors + ~500MB data | ~150MB total |

---

## 5. Capa de Búsqueda Keyword

### 5.1 PostgreSQL Full-Text Search (Online)

**Configuración de diccionarios**:

```sql
-- Diccionario español para documentación
CREATE TEXT SEARCH CONFIGURATION cuervo_es (COPY = spanish);
ALTER TEXT SEARCH CONFIGURATION cuervo_es
    ALTER MAPPING FOR asciiword, asciihword, hword_asciipart, word, hword, hword_part
    WITH spanish_stem, english_stem;

-- Diccionario de sinónimos técnicos
CREATE TEXT SEARCH DICTIONARY cuervo_synonyms (
    TEMPLATE = synonym,
    SYNONYMS = cuervo_tech_synonyms    -- archivo de sinónimos
);

-- Contenido de cuervo_tech_synonyms.syn:
-- mcp modelcontextprotocol
-- jwt jsonwebtoken
-- cli commandlineinterface
-- rag retrievalaugmentedgeneration
-- llm largelanguagemodel
-- oauth oauth2
-- k8s kubernetes
```

**Pesos de búsqueda por campo**:

```sql
-- El search_vector ya tiene pesos en la definición de la tabla:
-- 'A' (1.0): title, symbol_name        — Máxima prioridad
-- 'B' (0.4): summary, content          — Prioridad media
-- 'C' (0.2): tags, domain_tags         — Prioridad baja
-- 'D' (0.1): breadcrumb, dependencies  — Contexto
```

### 5.2 Tantivy via LanceDB (Offline)

Tantivy es el motor full-text integrado en LanceDB. Soporta:

- Tokenización multilingual
- Boolean queries (`auth AND (jwt OR oauth2)`)
- Phrase queries (`"token rotation"`)
- Prefix queries (`validate*`)
- Boosting por campo

No requiere configuración adicional — se activa al crear un FTS index en LanceDB.

---

## 6. Capa de Grafo de Conocimiento

### 6.1 Modelo de Relaciones

```
┌──────────────────────────────────────────────────────────────┐
│                    KNOWLEDGE GRAPH                            │
│                                                               │
│  ┌──────┐  references  ┌──────┐  implements  ┌──────┐       │
│  │ Doc  │─────────────▶│ ADR  │◀─────────────│ Code │       │
│  │Chunk │              │Chunk │              │Chunk │       │
│  └──┬───┘              └──┬───┘              └──┬───┘       │
│     │                     │                     │            │
│     │ elaborates          │ supersedes          │ depends_on │
│     ▼                     ▼                     ▼            │
│  ┌──────┐  documents   ┌──────┐  tested_by  ┌──────┐       │
│  │Detail│─────────────▶│ New  │◀─────────────│ Test │       │
│  │Chunk │              │ ADR  │              │Chunk │       │
│  └──────┘              └──────┘              └──────┘       │
│     │                                           │            │
│     │ related_to                    extends     │            │
│     ▼                                           ▼            │
│  ┌──────┐  configured_by  ┌──────┐           ┌──────┐      │
│  │ Req  │◀────────────────│Config│           │ Base │      │
│  │Chunk │                 │Chunk │           │Chunk │      │
│  └──────┘                 └──────┘           └──────┘      │
└──────────────────────────────────────────────────────────────┘
```

### 6.2 Tipos de Relación

| Relación | Descripción | Detección | Ejemplo |
|----------|-------------|-----------|---------|
| `references` | A cita o menciona a B | Link parsing, import analysis | Doc chunk → Code symbol |
| `implements` | A es implementación de B | Naming conventions, annotations | Code → Interface/ADR |
| `extends` | A extiende/hereda de B | AST analysis | Class → Base class |
| `depends_on` | A requiere B para funcionar | Import analysis | Module → Dependency |
| `supersedes` | A reemplaza a B | ADR status, Git history | New ADR → Old ADR |
| `documents` | A es documentación de B | Path matching, annotations | Doc → Code module |
| `tested_by` | A tiene tests en B | Naming conventions | Code → Test file |
| `configured_by` | A usa configuración de B | Config references | Code → Config file |
| `related_to` | A está temáticamente relacionado con B | Embedding similarity > 0.85 | Any → Any |
| `contradicts` | A contradice información de B | Agente MCP detection | Doc → Doc |
| `elaborates` | A expande el tema de B | Document hierarchy | Detail chunk → Summary chunk |

### 6.3 Construcción del Grafo

```typescript
interface GraphBuilder {
  // Relaciones explícitas (determinísticas)
  extractExplicitRelations(chunk: EnrichedChunk): ChunkRelation[];

  // Relaciones inferidas (heurísticas)
  inferRelations(chunk: EnrichedChunk, existingChunks: ChunkIndex): ChunkRelation[];

  // Relaciones semánticas (embedding-based)
  discoverSemanticRelations(chunk: EnrichedChunk, threshold: number): ChunkRelation[];
}

// Reglas de extracción explícita:
const explicitRules: ExtractionRule[] = [
  {
    // Markdown links → references
    pattern: /\[([^\]]+)\]\(([^)]+)\)/g,
    relation: 'references',
    confidence: 1.0,
  },
  {
    // TypeScript imports → depends_on
    pattern: /import\s+.*\s+from\s+['"]([^'"]+)['"]/g,
    relation: 'depends_on',
    confidence: 1.0,
  },
  {
    // Class extends → extends
    pattern: /class\s+\w+\s+extends\s+(\w+)/g,
    relation: 'extends',
    confidence: 1.0,
  },
  {
    // ADR supersedes → supersedes
    pattern: /Supersedes:\s*(ADR-\d+)/g,
    relation: 'supersedes',
    confidence: 1.0,
  },
  {
    // Test file for source → tested_by
    sourcePattern: /^src\/(.+)\.ts$/,
    targetPattern: /^tests\/(.+)\.(test|spec)\.ts$/,
    relation: 'tested_by',
    confidence: 0.9,
  },
];
```

### 6.4 Graph Traversal Queries

```sql
-- Encontrar todos los chunks relacionados a 2 hops de distancia
WITH RECURSIVE related AS (
    -- Base: chunks del resultado inicial
    SELECT
        cr.target_chunk_id AS chunk_id,
        cr.relation_type,
        cr.confidence,
        1 AS depth
    FROM chunk_relations cr
    WHERE cr.source_chunk_id = ANY($1::uuid[])   -- IDs de chunks iniciales
      AND cr.confidence >= 0.7

    UNION ALL

    -- Recursivo: expandir a siguiente hop
    SELECT
        cr.target_chunk_id,
        cr.relation_type,
        cr.confidence * r.confidence AS confidence,  -- Decay por profundidad
        r.depth + 1
    FROM chunk_relations cr
    JOIN related r ON cr.source_chunk_id = r.chunk_id
    WHERE r.depth < $2                              -- Max hops (default: 2)
      AND cr.confidence >= 0.5
)
SELECT DISTINCT ON (chunk_id)
    r.chunk_id,
    r.relation_type,
    r.confidence,
    r.depth,
    c.content,
    c.breadcrumb,
    c.symbol_name
FROM related r
JOIN chunks c ON r.chunk_id = c.id
ORDER BY chunk_id, confidence DESC;
```

---

## 7. Object Store para Documentos

### 7.1 Almacenamiento de Documentos Fuente

Los documentos raw se almacenan con su contenido completo para:
- Regeneración de chunks sin acceso a Git
- Historial de versiones para diff
- Serving directo para visualización

```typescript
interface DocumentStore {
  // Almacenar versión del documento
  store(doc: DocumentVersion): Promise<void>;

  // Recuperar última versión
  getLatest(filePath: string, repo: string): Promise<DocumentVersion | null>;

  // Recuperar versión específica
  getByCommit(filePath: string, repo: string, commitSha: string): Promise<DocumentVersion | null>;

  // Listar versiones
  listVersions(filePath: string, repo: string): Promise<DocumentVersionMeta[]>;

  // Diff entre versiones
  diff(filePath: string, repo: string, fromCommit: string, toCommit: string): Promise<DocumentDiff>;
}

interface DocumentVersion {
  filePath: string;
  repository: string;
  branch: string;
  commitSha: string;
  content: string;
  contentHash: string;
  size: number;
  author: string;
  timestamp: Date;
}
```

**Implementación**:
- **Online**: PostgreSQL tabla `document_versions` con contenido en columna TEXT (comprimido con TOAST automáticamente)
- **Offline**: Filesystem local en `~/.cuervo/knowledge/documents/` con estructura de directorio por hash
- **Enterprise**: S3-compatible object store para archivos grandes (>1MB)

---

## 8. Estrategia de Caching

### 8.1 Cache Multi-Nivel

```
┌─────────────────────────────────────────────────────────┐
│                   CACHE ARCHITECTURE                     │
│                                                           │
│  Request                                                  │
│    │                                                      │
│    ▼                                                      │
│  ┌───────────────────────────┐                           │
│  │  L1: In-Memory LRU        │  TTL: 5 min              │
│  │  - Query → Results cache   │  Size: 1000 entries      │
│  │  - Embedding cache         │  Hit rate target: 60%    │
│  │  - Metadata cache          │                          │
│  └────────────┬──────────────┘                           │
│       miss    │                                          │
│               ▼                                          │
│  ┌───────────────────────────┐                           │
│  │  L2: Semantic Cache        │  TTL: 24 hours           │
│  │  - Similar query detection │  Similarity: >0.95       │
│  │  - Stored in SQLite/PG     │  Hit rate target: 30%    │
│  │  - Query embedding → match │                          │
│  └────────────┬──────────────┘                           │
│       miss    │                                          │
│               ▼                                          │
│  ┌───────────────────────────┐                           │
│  │  L3: Redis (Cloud only)   │  TTL: 1 hour             │
│  │  - Cross-session cache     │  Shared between users    │
│  │  - Popular queries         │  Enterprise mode only    │
│  └────────────┬──────────────┘                           │
│       miss    │                                          │
│               ▼                                          │
│  ┌───────────────────────────┐                           │
│  │  Full Search Execution     │                          │
│  │  Vector + BM25 + Graph     │                          │
│  │  + Re-ranking              │                          │
│  └───────────────────────────┘                           │
└─────────────────────────────────────────────────────────┘
```

### 8.2 Semantic Cache

El semantic cache evita re-ejecutar búsquedas para queries semánticamente equivalentes:

```typescript
interface SemanticCache {
  // Buscar query similar en cache
  findSimilar(queryEmbedding: Float32Array, threshold: number): Promise<CachedResult | null>;

  // Almacenar resultado
  store(queryEmbedding: Float32Array, queryText: string, results: SearchResult[]): Promise<void>;

  // Invalidar por documento modificado
  invalidateByDocument(documentId: string): Promise<number>;

  // Invalidar por antigüedad
  evictStale(maxAge: Duration): Promise<number>;
}

// Implementación: tabla en PostgreSQL con pgvector
// SELECT * FROM query_cache
// WHERE embedding <=> $1 < 0.05  -- threshold invertido (distancia < 0.05 ≈ similarity > 0.95)
// AND created_at > NOW() - INTERVAL '24 hours'
// LIMIT 1;
```

### 8.3 Cache Invalidation

```
Trigger: Nuevo ingestion job completado
    │
    ├──▶ L1 (Memory): Flush completo (rápido, conservative)
    │
    ├──▶ L2 (Semantic): Invalidar solo queries que involucraban
    │    documentos modificados (selectivo)
    │
    └──▶ L3 (Redis): Invalidar por tags de documento
         (pub/sub para notificar a otras instancias)
```

---

## 9. Estrategia Offline-First

### 9.1 Principio

El Knowledge Store funciona **completamente offline** como modo primario. La conectividad cloud mejora la calidad pero no es requisito.

### 9.2 Flujo Offline

```
┌─────────────────────────────────────────────────────────┐
│              OFFLINE-FIRST FLOW                          │
│                                                           │
│  1. Usuario instala Cuervo CLI                           │
│     → Se crea ~/.cuervo/knowledge/                       │
│     → LanceDB inicializado vacío                         │
│     → BGE-M3 ONNX model descargado (~100MB one-time)    │
│                                                           │
│  2. Usuario abre proyecto                                │
│     → Git watcher detecta archivos                       │
│     → Chunking pipeline ejecuta (local, sync)            │
│     → BGE-M3 genera embeddings (local CPU/GPU)           │
│     → LanceDB indexa (local disk)                        │
│     → Tantivy indexa FTS (local disk)                    │
│                                                           │
│  3. Usuario busca                                        │
│     → Query embebida con BGE-M3 (local)                  │
│     → Hybrid search en LanceDB (vector + Tantivy)        │
│     → Re-ranking con BGE-reranker (local ONNX)           │
│     → Resultados en <100ms                               │
│                                                           │
│  4. Si hay conectividad (upgrade automático):            │
│     → Re-embed con Voyage/Cohere (mejor calidad)         │
│     → Sync a PostgreSQL cloud (si enterprise)            │
│     → Cross-encoder rerank con Cohere (si disponible)    │
│     → Resultados mejoran sin intervención del usuario    │
└─────────────────────────────────────────────────────────┘
```

### 9.3 Sync Engine

```typescript
interface SyncEngine {
  // Sincronizar offline → online
  pushToCloud(localChanges: ChangeSet): Promise<SyncResult>;

  // Sincronizar online → offline
  pullFromCloud(since: Date): Promise<ChangeSet>;

  // Resolver conflictos
  resolveConflicts(conflicts: Conflict[]): Promise<Resolution[]>;
}

interface SyncPolicy {
  direction: 'push' | 'pull' | 'bidirectional';
  frequency: 'realtime' | 'periodic' | 'manual';
  conflictResolution: 'last_write_wins' | 'merge' | 'manual';
  bandwidth: 'full' | 'metadata_only' | 'delta';
}

// Para el CLI: push on commit, pull on startup
// Para enterprise: bidirectional realtime via websocket
```

### 9.4 Footprint Local

| Componente | Tamaño en disco | RAM en uso |
|-----------|----------------|------------|
| BGE-M3 ONNX model | ~100MB (one-time download) | ~200MB (loaded) |
| BGE-reranker ONNX | ~50MB (one-time) | ~100MB (loaded) |
| LanceDB (50K chunks, 1024d) | ~150MB | ~50MB (mmap) |
| Tantivy FTS index | ~30MB | ~10MB (mmap) |
| Document cache | ~50MB | Lazy loaded |
| **Total** | **~380MB** | **~360MB peak** |

Aceptable para un CLI de desarrollo. Comparable a un node_modules mediano.

---

## 10. Multi-Tenant y Seguridad por Contexto

### 10.1 Isolation Model

```
┌─────────────────────────────────────────────────────────┐
│              MULTI-TENANT ISOLATION                      │
│                                                           │
│  Modo Standalone (CLI personal):                         │
│  └── Tenant = user local                                 │
│      └── Scope = directorio de proyecto                  │
│          └── Todos los chunks visibles                   │
│                                                           │
│  Modo Enterprise (self-hosted):                          │
│  └── Tenant = organización                               │
│      ├── Team A                                          │
│      │   ├── Repo privado A1 → Chunks solo Team A       │
│      │   └── Repo compartido → Chunks visibles           │
│      └── Team B                                          │
│          ├── Repo privado B1 → Chunks solo Team B       │
│          └── Repo compartido → Chunks visibles           │
│                                                           │
│  Implementación:                                         │
│  - Row-Level Security (RLS) en PostgreSQL                │
│  - tenant_id + team_id en cada chunk                     │
│  - Filtrado automático en todas las queries              │
└─────────────────────────────────────────────────────────┘
```

### 10.2 Row-Level Security

```sql
-- Habilitar RLS
ALTER TABLE chunks ENABLE ROW LEVEL SECURITY;
ALTER TABLE documents ENABLE ROW LEVEL SECURITY;

-- Columnas de tenant (agregar al schema)
ALTER TABLE chunks ADD COLUMN tenant_id UUID NOT NULL;
ALTER TABLE chunks ADD COLUMN team_ids UUID[] DEFAULT '{}';
ALTER TABLE chunks ADD COLUMN visibility TEXT NOT NULL DEFAULT 'team';
-- visibility: 'private' | 'team' | 'org' | 'public'

ALTER TABLE documents ADD COLUMN tenant_id UUID NOT NULL;
ALTER TABLE documents ADD COLUMN team_ids UUID[] DEFAULT '{}';
ALTER TABLE documents ADD COLUMN visibility TEXT NOT NULL DEFAULT 'team';

-- Políticas RLS
CREATE POLICY chunk_isolation ON chunks
    USING (
        tenant_id = current_setting('app.tenant_id')::uuid
        AND (
            visibility = 'org'
            OR visibility = 'public'
            OR team_ids && ARRAY[current_setting('app.team_id')::uuid]
        )
    );

CREATE POLICY document_isolation ON documents
    USING (
        tenant_id = current_setting('app.tenant_id')::uuid
        AND (
            visibility = 'org'
            OR visibility = 'public'
            OR team_ids && ARRAY[current_setting('app.team_id')::uuid]
        )
    );
```

### 10.3 Seguridad por Contexto

```typescript
interface SecurityContext {
  tenantId: string;
  userId: string;
  teamIds: string[];
  roles: Role[];
  permissions: Permission[];
  repositoryAccess: Map<string, AccessLevel>;  // repo → read/write/admin
}

interface QuerySecurityFilter {
  // Aplicar filtros de seguridad a cualquier query
  applyFilters(query: SearchQuery, context: SecurityContext): SecuredQuery;

  // Verificar acceso a chunk específico
  canAccess(chunkId: string, context: SecurityContext): Promise<boolean>;

  // Redactar contenido sensible
  redactSensitive(chunk: Chunk, context: SecurityContext): Chunk;
}
```

---

## 11. Query Patterns y Contratos de Acceso

### 11.1 API Principal del Knowledge Store

```typescript
// ============================================================
// KNOWLEDGE STORE API — Contratos de acceso
// ============================================================

interface KnowledgeStoreAPI {
  // ─── BÚSQUEDA ───────────────────────────────────────

  /**
   * Búsqueda híbrida (vector + BM25 + graph).
   * Método principal de recuperación.
   */
  search(query: SearchQuery): Promise<SearchResponse>;

  /**
   * Búsqueda semántica pura (solo vector).
   * Para queries conceptuales donde keywords no ayudan.
   */
  semanticSearch(query: SemanticQuery): Promise<SearchResponse>;

  /**
   * Búsqueda keyword (solo BM25).
   * Para términos exactos: nombres de función, error codes.
   */
  keywordSearch(query: KeywordQuery): Promise<SearchResponse>;

  /**
   * Búsqueda por grafo (expansión de relaciones).
   * Para "muéstrame todo lo relacionado con X".
   */
  graphSearch(query: GraphQuery): Promise<GraphSearchResponse>;

  /**
   * Búsqueda multi-nivel jerárquica.
   * Document → Section → Chunk, con progressive refinement.
   */
  hierarchicalSearch(query: HierarchicalQuery): Promise<HierarchicalResponse>;

  // ─── INGESTIÓN ──────────────────────────────────────

  /**
   * Ingestar documento(s) nuevo(s) o actualizado(s).
   */
  ingest(request: IngestRequest): Promise<IngestResponse>;

  /**
   * Re-indexar documentos específicos.
   */
  reindex(documentIds: string[]): Promise<IngestResponse>;

  /**
   * Eliminar documentos y sus chunks.
   */
  remove(documentIds: string[]): Promise<void>;

  // ─── METADATA ───────────────────────────────────────

  /**
   * Obtener estadísticas del knowledge store.
   */
  stats(): Promise<KnowledgeStats>;

  /**
   * Obtener documento con sus chunks.
   */
  getDocument(documentId: string): Promise<DocumentWithChunks>;

  /**
   * Obtener chunk con su contexto.
   */
  getChunk(chunkId: string, includeContext?: boolean): Promise<ChunkWithContext>;

  /**
   * Obtener relaciones de un chunk.
   */
  getRelations(chunkId: string, depth?: number): Promise<ChunkRelation[]>;
}

// ─── QUERY TYPES ────────────────────────────────────────

interface SearchQuery {
  text: string;                       // Query en lenguaje natural
  filters?: SearchFilters;
  options?: SearchOptions;
}

interface SearchFilters {
  repositories?: string[];
  contentTypes?: ContentType[];
  domains?: string[];                 // domain_tags filter
  languages?: string[];
  dateRange?: { from?: Date; to?: Date };
  symbolTypes?: SymbolType[];
  branches?: string[];
}

interface SearchOptions {
  topK?: number;                      // Default: 5
  reranking?: boolean;                // Default: true
  includeContext?: boolean;           // Include surrounding chunks
  includeRelations?: boolean;         // Include graph relations
  minScore?: number;                  // Minimum relevance score
  embeddingModel?: string;            // Override model selection
  mode?: 'online' | 'offline' | 'auto'; // Engine selection
}

interface SearchResponse {
  results: SearchResult[];
  metadata: {
    totalCandidates: number;
    searchTimeMs: number;
    retrievalMethod: string;
    rerankerUsed: string | null;
    cacheHit: boolean;
    engineUsed: 'postgresql' | 'lancedb';
  };
}

interface SearchResult {
  chunk: {
    id: string;
    content: string;
    contextualPrefix: string;
    breadcrumb: string[];
    contentType: ContentType;
    symbolName?: string;
    symbolType?: SymbolType;
    lineStart?: number;
    lineEnd?: number;
  };
  document: {
    id: string;
    filePath: string;
    repository: string;
    title: string;
  };
  score: number;                      // 0-1, composite relevance
  scoreBreakdown?: {
    vectorScore: number;
    bm25Score: number;
    graphScore: number;
    rerankScore: number;
  };
  relations?: ChunkRelation[];
  context?: {                         // Chunks adyacentes si includeContext=true
    before: ChunkSummary[];
    after: ChunkSummary[];
  };
}

// ─── INGEST TYPES ───────────────────────────────────────

interface IngestRequest {
  sources: IngestSource[];
  options?: IngestOptions;
}

interface IngestSource {
  type: 'file' | 'directory' | 'git_diff' | 'raw_content';
  path?: string;
  content?: string;
  repository: string;
  branch?: string;
  commitSha?: string;
  metadata?: Record<string, unknown>;
}

interface IngestOptions {
  force?: boolean;                    // Re-ingest even if hash matches
  chunkingStrategy?: string;          // Override auto-detection
  embeddingModels?: string[];         // Which models to embed with
  generateContextualPrefix?: boolean; // Use LLM for prefix generation
  buildGraphRelations?: boolean;      // Extract and store relations
  priority?: 'critical' | 'high' | 'medium' | 'low';
}

interface IngestResponse {
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
```

### 11.2 Query Patterns Comunes

| Pattern | Método | Ejemplo | Filtros típicos |
|---------|--------|---------|----------------|
| "¿Cómo funciona X?" | `search()` hybrid | "¿Cómo funciona el model gateway?" | contentType: prose,code |
| "Búscame la función X" | `keywordSearch()` | "validateCredentials" | symbolType: function |
| "¿Qué se decidió sobre X?" | `search()` hybrid | "¿Qué se decidió sobre JWT vs sessions?" | contentType: adr |
| "Todo sobre módulo X" | `graphSearch()` | Expandir desde módulo auth | depth: 2 |
| "¿Dónde se usa X?" | `graphSearch()` | Buscar relaciones depends_on hacia X | relationType: depends_on |
| "Docs del endpoint Y" | `search()` hybrid | "POST /api/auth/login" | contentType: api_endpoint |
| "¿Qué cambió desde commit Z?" | `ingest()` con git_diff | diff entre commits | - |

---

## 12. Replicación y Alta Disponibilidad

### 12.1 Estrategia por Deployment Model

| Modelo | Replicación | HA | Backup |
|--------|------------|-----|--------|
| **Standalone (CLI)** | Ninguna | N/A (local) | Git es el backup |
| **Hybrid** | LanceDB local + PG cloud | PG managed (RDS/Aurora) | PG automated backups |
| **Enterprise** | PG read replicas + Redis cluster | Multi-AZ | PG PITR + S3 snapshots |
| **SaaS** | Aurora Global + Redis cluster | Multi-region | Continuous + cross-region |

### 12.2 Latencia Objetivo por Deployment

| Operación | Standalone | Hybrid | Enterprise |
|-----------|-----------|--------|-----------|
| Hybrid search | <80ms | <120ms | <100ms |
| Ingest (single doc) | <2s | <3s | <2s |
| Sync (100 chunks) | N/A | <5s | <2s |
| Graph traversal (2 hops) | <30ms | <50ms | <40ms |

---

*Siguiente documento: [03-mcp-doc-agent.md](./03-mcp-doc-agent.md) — MCP Agent de Documentación*
