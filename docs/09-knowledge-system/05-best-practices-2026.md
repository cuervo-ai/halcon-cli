# Fase 5 — Mejores Prácticas y Estándares 2026

> **Documento**: `09-knowledge-system/05-best-practices-2026.md`
> **Versión**: 1.0.0
> **Fecha**: 2026-02-06
> **Autores**: Research Scientist, Knowledge Systems Architect, LLMOps Engineer
> **Estado**: Design Complete

---

## Índice

1. [RAG Avanzado](#1-rag-avanzado)
2. [LLMOps Observability](#2-llmops-observability)
3. [Evaluation-Driven Development](#3-evaluation-driven-development)
4. [Provenance Tracking y Explainability](#4-provenance-tracking-y-explainability)
5. [Compliance y Estándares](#5-compliance-y-estándares)
6. [Docs-as-Code](#6-docs-as-code)
7. [MCP y Agentic Workflows](#7-mcp-y-agentic-workflows)
8. [Reproducibilidad](#8-reproducibilidad)
9. [Trade-offs y Decisiones](#9-trade-offs-y-decisiones)

---

## 1. RAG Avanzado

### 1.1 Prácticas Adoptadas

#### A. Hierarchical Retrieval (A-RAG Pattern)

**Qué**: Búsqueda en tres niveles de granularidad: document → section → chunk, donde un agente selecciona dinámicamente qué nivel consultar.

**Por qué**: Las queries tienen diferentes granularidades. "¿Qué cubre el documento de IAM?" necesita document-level. "¿Cuál es la firma de validateCredentials?" necesita chunk-level. Un solo nivel no satisface ambas.

**Referencia**: A-RAG: Scaling Agentic RAG via Hierarchical Retrieval Interfaces (arXiv 2602.03442, Feb 2026).

**Trade-off**: Mayor complejidad de indexación (embeddings a 3 niveles) a cambio de mayor precisión de retrieval y menor carga en el LLM (contexto más enfocado).

#### B. Graph RAG (Knowledge Graph Augmented Retrieval)

**Qué**: Complementar vector search con traversal de grafo de conocimiento para queries que requieren razonamiento multi-hop.

**Por qué**: "¿Qué módulos se ven afectados si cambio la interfaz AuthProvider?" requiere seguir relaciones `implements` y `depends_on` a través del grafo. Vector search por sí solo no captura relaciones estructurales.

**Referencia**: Microsoft GraphRAG (v2.x), diseño de grafos de conocimiento como "Critical Enabler" según Gartner 2026.

**Trade-off**: Construcción y mantenimiento del grafo requiere pipeline adicional. Justificado porque el grafo se construye incrementalmente desde análisis AST y parsing de links (bajo costo).

#### C. Hybrid Search + Cross-Encoder Reranking

**Qué**: Combinar búsqueda vectorial (semántica) con BM25 (keyword) mediante Reciprocal Rank Fusion, seguido de reranking con cross-encoder.

**Por qué**: Estudios 2025-2026 muestran consistentemente que hybrid search + reranking son las dos técnicas con mayor impacto en calidad de retrieval. Vector solo falla en términos exactos; BM25 solo falla en semántica. Juntos cubren ambos.

**Trade-off**: +50-100ms de latencia por el reranking. Aceptable dado nuestro target de <200ms p95.

#### D. Contextual Enrichment (Contextual Prefix)

**Qué**: Cada chunk almacena un prefijo generado que explica su contexto dentro del documento padre.

**Por qué**: Un chunk aislado como "La rotación ocurre cada 7 días" es ambiguo. Con prefix: "En el documento IAM Architecture, sección OAuth2 Token Rotation: La rotación ocurre cada 7 días" es preciso.

**Trade-off**: Costo de generación (LLM call por chunk, ~$0.01/chunk). Mitigado con caché y generación batch. Alternativa más barata: Late Chunking con Cohere embed-v4 (128K context) que logra efecto similar sin LLM call.

### 1.2 Prácticas Evaluadas pero Diferidas

| Práctica | Razón de diferimiento |
|----------|----------------------|
| **HyDE (Hypothetical Document Embeddings)** | Agrega latencia (LLM call antes de search). Evaluar si hybrid search + reranking ya provee suficiente calidad |
| **RAG Fusion (multi-query generation)** | Similar a HyDE — latencia adicional. Reservar para queries complejas que fallen con approach actual |
| **ColBERT late interaction reranking** | Requiere Qdrant con multivector support. Evaluar en Beta si BGE-reranker es insuficiente |

---

## 2. LLMOps Observability

### 2.1 Stack de Observabilidad

```
┌─────────────────────────────────────────────────────────┐
│              OBSERVABILITY STACK                         │
│                                                           │
│  ┌─────────────────┐                                    │
│  │    Application   │                                    │
│  │  (Cuervo CLI +   │                                    │
│  │   DocAgent)       │                                    │
│  └────────┬─────────┘                                    │
│           │                                               │
│           │  OpenTelemetry SDK                            │
│           │  (traces + spans + metrics)                   │
│           │                                               │
│  ┌────────▼─────────┐     ┌─────────────────────┐       │
│  │  OpenLLMetry     │     │    Custom Spans      │       │
│  │  (auto-instrument│     │                      │       │
│  │   LLM calls)     │     │  - retrieval_span   │       │
│  └────────┬─────────┘     │  - chunking_span    │       │
│           │               │  - reranking_span    │       │
│           │               │  - agent_step_span   │       │
│  ┌────────▼─────────┐    └──────────┬───────────┘       │
│  │  OTel Collector  │◀──────────────┘                    │
│  │  (local)         │                                    │
│  └────────┬─────────┘                                    │
│           │                                               │
│     ┌─────┴──────┐                                       │
│     │            │                                       │
│     ▼            ▼                                       │
│  ┌──────┐  ┌──────────┐                                 │
│  │Langfuse│ │ Grafana  │                                 │
│  │(LLM    │ │(infra    │                                 │
│  │ traces,│ │ metrics, │                                 │
│  │ evals, │ │ dashboards│                                │
│  │ prompts│ │          │                                 │
│  └──────┘  └──────────┘                                 │
│                                                           │
│  Langfuse provides:                                      │
│  - LLM call tracing (input/output/tokens/cost)          │
│  - Prompt management and versioning                      │
│  - Evaluation scores tracking                            │
│  - Session replay                                        │
│  - User feedback collection                              │
│  Self-hosted: MIT license, Docker Compose deploy         │
└─────────────────────────────────────────────────────────┘
```

### 2.2 Métricas Clave a Rastrear

| Categoría | Métrica | Alerta |
|-----------|---------|--------|
| **Quality** | Retrieval precision@5 | < 0.7 |
| **Quality** | Answer faithfulness (LLM-evaluated) | < 0.8 |
| **Quality** | Hallucination rate | > 5% |
| **Quality** | User feedback (thumbs up/down) | < 80% positive |
| **Performance** | Time-to-first-token (TTFT) | > 500ms p95 |
| **Performance** | Total retrieval latency | > 200ms p95 |
| **Performance** | Embedding latency | > 100ms p95 |
| **Cost** | Cost per query (avg) | > $0.05 |
| **Cost** | Cost per doc generation | > $0.50 |
| **Cost** | Monthly total spend | > budget |
| **System** | Cache hit rate | < 40% |
| **System** | Knowledge store size (chunks) | > capacity threshold |
| **System** | Ingestion backlog (queue depth) | > 100 |
| **Agent** | Task success rate | < 85% |
| **Agent** | Escalation rate | > 20% |
| **Agent** | Average revision iterations | > 1.5 |

### 2.3 Trace Propagation

```typescript
// Ejemplo de trace propagación en una búsqueda RAG
async function hybridSearch(query: string, context: SecurityContext): Promise<SearchResponse> {
  return tracer.startActiveSpan('knowledge.hybrid_search', async (span) => {
    span.setAttribute('query.text', query);
    span.setAttribute('query.user_id', context.userId);

    // 1. Embed query
    const embedding = await tracer.startActiveSpan('embedding.query', async (embedSpan) => {
      const result = await embeddingService.embed(query);
      embedSpan.setAttribute('embedding.model', result.model);
      embedSpan.setAttribute('embedding.dimensions', result.dimensions);
      embedSpan.setAttribute('embedding.latency_ms', result.latencyMs);
      return result;
    });

    // 2. Parallel retrieval
    const [vectorResults, bm25Results, graphResults] = await Promise.all([
      tracer.startActiveSpan('retrieval.vector', async (s) => {
        const r = await vectorStore.search(embedding, 50);
        s.setAttribute('retrieval.results_count', r.length);
        return r;
      }),
      tracer.startActiveSpan('retrieval.bm25', async (s) => {
        const r = await ftsStore.search(query, 50);
        s.setAttribute('retrieval.results_count', r.length);
        return r;
      }),
      tracer.startActiveSpan('retrieval.graph', async (s) => {
        const r = await graphStore.expand(query, 20);
        s.setAttribute('retrieval.results_count', r.length);
        return r;
      }),
    ]);

    // 3. Fusion
    const fused = await tracer.startActiveSpan('fusion.rrf', async (s) => {
      return rrfFusion({ vector: vectorResults, bm25: bm25Results, graph: graphResults });
    });

    // 4. Rerank
    const reranked = await tracer.startActiveSpan('reranking.cross_encoder', async (s) => {
      const r = await reranker.rerank(query, fused.slice(0, 20));
      s.setAttribute('reranking.model', 'bge-reranker-v2-m3');
      s.setAttribute('reranking.input_count', 20);
      s.setAttribute('reranking.output_count', r.length);
      return r;
    });

    span.setAttribute('search.total_results', reranked.length);
    span.setAttribute('search.cache_hit', false);
    return formatResponse(reranked);
  });
}
```

---

## 3. Evaluation-Driven Development

### 3.1 Principio

No desplegar cambios al pipeline de retrieval o generación sin evaluar su impacto en métricas de calidad. Cada cambio (modelo de embedding, estrategia de chunking, prompt de agente) se testa contra un evaluation set antes de ir a producción.

### 3.2 Evaluation Framework

```typescript
interface EvaluationFramework {
  // Dataset de evaluación curado
  dataset: EvalDataset;

  // Métricas a computar
  metrics: EvalMetric[];

  // Comparar dos configuraciones
  compare(configA: PipelineConfig, configB: PipelineConfig): Promise<ComparisonReport>;

  // Ejecutar evaluación completa
  evaluate(config: PipelineConfig): Promise<EvalReport>;
}

interface EvalDataset {
  // Pares query → respuestas esperadas
  retrievalQueries: {
    query: string;
    expectedChunkIds: string[];       // Ground truth chunks
    domain: string;
    difficulty: 'easy' | 'medium' | 'hard';
  }[];

  // Pares query → respuesta ideal (para generation eval)
  generationQueries: {
    query: string;
    context: string[];                // Chunks to use as context
    idealResponse: string;            // Human-written ideal answer
    rubric: EvalRubric;
  }[];
}

interface EvalMetric {
  name: string;
  compute(predicted: unknown, expected: unknown): number;
}

// Métricas de retrieval:
const retrievalMetrics: EvalMetric[] = [
  { name: 'precision@5', compute: precisionAtK(5) },
  { name: 'recall@10', compute: recallAtK(10) },
  { name: 'nDCG@10', compute: normalizedDCG(10) },
  { name: 'MRR', compute: meanReciprocalRank },
];

// Métricas de generation (LLM-as-judge):
const generationMetrics: EvalMetric[] = [
  { name: 'faithfulness', compute: llmJudgeFaithfulness },    // ¿respeta el contexto?
  { name: 'relevance', compute: llmJudgeRelevance },          // ¿responde la pregunta?
  { name: 'completeness', compute: llmJudgeCompleteness },    // ¿cubre todos los aspectos?
  { name: 'correctness', compute: llmJudgeCorrectness },      // ¿factualmente correcto?
];
```

### 3.3 Proceso de Evaluation

```
1. Maintain golden dataset (50-100 query-answer pairs)
   - Curated by engineers
   - Updated quarterly
   - Covers all content types and difficulty levels

2. Before any pipeline change:
   a. Run evaluation on current config → baseline scores
   b. Apply change
   c. Run evaluation on new config → candidate scores
   d. Compare: if candidate >= baseline on all metrics → approve
   e. If regression on any metric → investigate and justify

3. Track evaluation history in Langfuse:
   - Each evaluation run stored with config hash
   - Trend analysis over time
   - Alerts on score degradation
```

---

## 4. Provenance Tracking y Explainability

### 4.1 Provenance Model

Cada output del sistema (resultado de búsqueda, documento generado, respuesta a query) debe ser **traceable** hasta sus fuentes originales.

```typescript
interface ProvenanceRecord {
  outputId: string;                   // ID del output generado
  outputType: 'search_result' | 'generated_doc' | 'agent_response';
  timestamp: Date;

  // Fuentes primarias
  sources: {
    chunkIds: string[];               // Chunks usados del knowledge store
    filePaths: string[];              // Archivos leídos directamente
    commitShas: string[];             // Commits relevantes
    externalRefs: string[];           // URLs externas citadas
  };

  // Cadena de transformación
  transformations: {
    step: string;                     // 'retrieval' | 'reranking' | 'generation' | 'review'
    model: string;                    // Modelo usado
    promptVersion: string;            // Versión del prompt
    inputTokens: number;
    outputTokens: number;
    temperature: number;
  }[];

  // Explicabilidad
  explanation: {
    // Por qué estos chunks fueron seleccionados
    retrievalReasoning: string;

    // Scores de cada etapa
    scores: {
      vectorSimilarity: number[];
      bm25Score: number[];
      rrfScore: number[];
      rerankScore: number[];
    };

    // Confidence del output final
    confidence: number;
    confidenceFactors: string[];      // Qué factores afectaron la confianza
  };
}
```

### 4.2 Explicabilidad para el Usuario

Cuando el usuario recibe un resultado de búsqueda o documentación generada, puede pedir "¿por qué?":

```
User: "¿Cómo funciona la autenticación OAuth2?"

System: [respuesta generada]

Sources used:
  1. docs/08-enterprise-design/02-iam-architecture.md
     Section: "OAuth2 Flow > Authorization Code Flow"
     Relevance: 0.96 (vector: 0.93, bm25: 0.89, rerank: 0.96)

  2. src/infrastructure/auth/oauth2.service.ts
     Function: handleAuthorizationCode()
     Relevance: 0.91

  3. docs/05-security-legal/02-privacidad-datos.md
     Section: "Data Classification > Session Tokens"
     Relevance: 0.78

Confidence: 0.92
Reasoning: High confidence based on strong matches in both documentation
and implementation code. Multiple corroborating sources found.
```

---

## 5. Compliance y Estándares

### 5.1 ISO/IEC 42001 (AI Management System)

**Qué**: Estándar internacional para gestión de sistemas de IA. Publicado 2023, creciente adopción 2025-2026.

**Aplicación al Knowledge System**:

| Requisito ISO 42001 | Implementación en Cuervo |
|---------------------|-------------------------|
| AI risk assessment | Risk analysis del pipeline RAG (hallucination, bias, data leaks) |
| Data governance | Provenance tracking, data classification, access control |
| Performance monitoring | LLMOps observability con Langfuse + OpenTelemetry |
| Transparency | Explainability layer, source attribution |
| Human oversight | Human-in-the-loop para DocAgent write operations |
| Continuous improvement | Evaluation-driven development, feedback loops |
| Documentation | Este conjunto de documentos (09-knowledge-system) |
| Incident management | Escalation policy, audit trail |

**Trade-off**: Overhead de compliance es significativo. Justificado por:
1. Mercado enterprise requiere ISO certification
2. EU AI Act alignment (obligatorio agosto 2026)
3. Diferenciador competitivo (ningún CLI rival tiene ISO 42001)

### 5.2 SOC 2 Controls para Knowledge System

| Control Area | Implementation |
|-------------|---------------|
| **CC6.1**: Logical access | RLS en PostgreSQL, tenant isolation |
| **CC6.3**: External access | API authentication, rate limiting |
| **CC7.2**: System monitoring | Langfuse traces, Grafana dashboards |
| **CC8.1**: Change management | Embedding version migration, blue-green deploy |
| **PI1.1**: Data integrity | Content hashing, checksum verification |
| **C1.1**: Confidentiality | PII detection (pii.rs), no-secret policy |

### 5.3 GDPR / Data Privacy

| Aspecto | Implementación |
|---------|---------------|
| Right to erasure | Cascade delete: document → chunks → embeddings → relations |
| Data minimization | Embeddings son irreversibles (no se puede reconstruir el texto) |
| Purpose limitation | Knowledge store solo contiene datos técnicos (código, docs) |
| Consent | Para Enterprise: DPA con términos de indexación |
| Cross-border transfer | Offline mode evita transferencia; cloud usa SCCs |

---

## 6. Docs-as-Code

### 6.1 Prácticas Adoptadas

| Práctica | Implementación | Herramienta |
|----------|---------------|-------------|
| **Plain text formats** | Todo en Markdown con Mermaid para diagramas | markdownlint |
| **Version control** | Docs en mismo repo que código | Git |
| **CI/CD pipeline** | DocOps pipeline (Fase 4) | GitHub Actions |
| **PR review workflow** | Docs changes requieren review | GitHub PR |
| **Prose linting** | Vale con estilos custom para Cuervo | Vale |
| **Link validation** | Checker en CI, pre-commit hook | Custom |
| **Code example testing** | TSC validation en CI | TypeScript compiler |
| **Diagram-as-code** | Mermaid.js para todos los diagramas | mermaid-cli |
| **Search** | MeiliSearch para UI, Knowledge Store para RAG | MeiliSearch + pgvector |
| **AI-assisted maintenance** | DocAgent (MCP) | Custom |

### 6.2 Por Qué Estas Herramientas

| Elección | Alternativas consideradas | Razón |
|----------|--------------------------|-------|
| **Markdown** (no AsciiDoc) | AsciiDoc, reStructuredText | Ecosistema más amplio, GitHub rendering nativo, menor curva de aprendizaje |
| **Mermaid** (no PlantUML) | PlantUML, D2, Structurizr | Rendering nativo en GitHub/GitLab/VS Code, JS ecosystem |
| **Vale** (no textlint) | textlint, LanguageTool | Más rápido, mejor ecosistema de estilos, CI-friendly |
| **MkDocs Material** (futuro site) | Docusaurus, Hugo | Python ecosystem compatible, best-in-class search, Mermaid nativo |

---

## 7. MCP y Agentic Workflows

### 7.1 MCP Spec Adoptada

**Versión**: MCP 2025-06-18 (current stable)

**Features implementadas**:

| Feature | Uso en Cuervo |
|---------|---------------|
| **Streamable HTTP transport** | Para MCP servers remotos (enterprise) |
| **stdio transport** | Para MCP servers locales (CLI tools) |
| **Tool definitions** | 15+ tools para DocAgent (definidos en Fase 3) |
| **Structured outputs** | outputSchema en tool responses para type safety |
| **Elicitation** | Human-in-the-loop confirmations en modo interactivo |

**Features planificadas** (spec June 2026):
- MCP Apps (interactive UI components)
- Enhanced async patterns
- Expanded media support

### 7.2 Agentic Workflow Patterns

| Pattern | Aplicación |
|---------|-----------|
| **Plan → Execute → Review** | DocAgent principal loop (Fase 3) |
| **Manager → Worker** | Planner descompone, sub-agentes ejecutan en paralelo |
| **Tool-augmented reasoning** | Cada paso de reasoning puede invocar tools MCP |
| **Memory-augmented** | Episodic + semantic memory inform future decisions |
| **Self-reflection** | Reviewer evalúa output del Executor, loop de mejora |
| **Escalation** | Agente reconoce sus límites y escala a humano |

### 7.3 Seguridad MCP

Siguiendo las mejores prácticas de seguridad MCP (Adversa AI, Feb 2026):

| Riesgo | Mitigación |
|--------|-----------|
| **Tool poisoning** (server malicioso) | Allowlist de MCP servers, signature verification |
| **Prompt injection via tool output** | Sanitize tool outputs, structured schemas |
| **Excessive permissions** | Principle of least privilege per tool |
| **Data exfiltration** | No-outbound policy for sensitive repos |
| **Resource exhaustion** | Rate limits, token budgets, timeouts |

---

## 8. Reproducibilidad

### 8.1 Principio

Dado el mismo input y configuración, el sistema debe producir resultados idénticos o con variación controlada.

### 8.2 Factores de Reproducibilidad

| Componente | Determinismo | Control |
|-----------|-------------|---------|
| **Chunking** | Determinístico | Mismo contenido → mismos chunks (content hash) |
| **Embeddings** | API-dependent | Fijar model version, almacenar embeddings generados |
| **BM25 Search** | Determinístico | Mismo query → mismos resultados (deterministic scoring) |
| **Vector Search** | Approximate | HNSW es probabilístico; fijar ef_search para consistencia |
| **RRF Fusion** | Determinístico | Pesos fijos → mismo ranking |
| **Reranking** | Model-dependent | Fijar model version, temperature=0 |
| **LLM Generation** | Non-deterministic | temperature=0 reduce varianza, pero no elimina |

### 8.3 Estrategia

```typescript
interface ReproducibilityConfig {
  // Fijar versiones de modelos
  embeddingModelVersion: string;      // "voyage-3.5-2026-01-15"
  rerankModelVersion: string;         // "bge-reranker-v2-m3-onnx-20260101"
  llmModelVersion: string;            // "claude-haiku-4-5-20251001"

  // Fijar parámetros de búsqueda
  hnswEfSearch: number;               // 100
  rrfWeights: Record<string, number>; // Fijos
  temperature: number;                // 0 para generación determinística

  // Logging completo
  logAllInputsOutputs: boolean;       // Para audit y replay
  snapshotEmbeddings: boolean;        // Store embeddings con su model version
}
```

---

## 9. Trade-offs y Decisiones

### 9.1 Matriz de Trade-offs Consolidada

| Decisión | Opción elegida | Alternativa | Trade-off |
|----------|---------------|-------------|-----------|
| **Vector DB** | pgvector 0.8 | Qdrant, Milvus | Menos features nativas (no multivector) a cambio de zero ops adicionales |
| **Offline DB** | LanceDB | SQLite-vss | Mejor rendimiento pero más joven como proyecto |
| **Embedding models** | Multi-modelo (Voyage+Cohere+BGE) | Modelo único | Mayor complejidad a cambio de óptimo por dominio |
| **Chunking** | Agentic (multi-estrategia) | Fixed-size | Complejidad de pipeline a cambio de -60% error rate |
| **BM25** | PostgreSQL FTS | OpenSearch | Menor calidad de búsqueda a cambio de zero infra adicional |
| **Grafo** | PG tablas + CTEs | Neo4j | Menos expresivo para queries profundas a cambio de zero infra |
| **Reranking** | BGE-reranker local | Cohere Rerank cloud | Menor calidad a cambio de zero-cost y offline capability |
| **Observability** | Langfuse (MIT) | Datadog, LangSmith | Self-hostable y free a cambio de menos features enterprise |
| **Agent pattern** | PER (Plan/Execute/Review) | ReAct, simple chain | Mayor token usage a cambio de mejor calidad y self-correction |
| **Human-in-the-loop** | Default ON | Default OFF | Mayor fricción a cambio de seguridad y control |
| **DocOps enforcement** | Block merge | Warning only | Mayor fricción en PRs a cambio de doc quality garantizada |
| **Multi-tenant** | PG RLS | Schemas separados | Menos isolation a cambio de queries cross-tenant posibles |

### 9.2 Riesgos Aceptados

| Riesgo | Probabilidad | Impacto | Mitigación |
|--------|-------------|---------|-----------|
| pgvector performance ceiling | Baja (volumen < 500K) | Alto | Migration path a Qdrant definido |
| LanceDB maturity issues | Media | Medio | Fallback a SQLite-vss |
| Embedding model API changes | Media | Alto | Versioned embeddings, migration pipeline |
| Cost overrun (API calls) | Baja | Medio | Hard budget limits, offline fallback |
| DocAgent hallucinations | Media | Alto | Hallucination guard, provenance tracking, human review |
| Knowledge store data leak | Baja | Muy alto | RLS, PII detection, audit logging |

---

*Siguiente documento: [06-consolidated-deliverables.md](./06-consolidated-deliverables.md) — Entregables Consolidados*
