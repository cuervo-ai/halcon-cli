# Fase 3 — MCP Agent de Documentación

> **Documento**: `09-knowledge-system/03-mcp-doc-agent.md`
> **Versión**: 1.0.0
> **Fecha**: 2026-02-06
> **Autores**: Research Scientist (Agentes Autónomos), Software Architect, Technical Writer Automation Specialist
> **Estado**: Design Complete

---

## Índice

1. [Visión del Agente](#1-visión-del-agente)
2. [Arquitectura del Agente (Planner/Executor/Reviewer)](#2-arquitectura-del-agente)
3. [MCP Tools Disponibles](#3-mcp-tools-disponibles)
4. [Sistema de Memoria Persistente](#4-sistema-de-memoria-persistente)
5. [Workflow Planner](#5-workflow-planner)
6. [Prompts Internos del Agente](#6-prompts-internos-del-agente)
7. [Ciclos de Verificación (Self-Check / Self-Eval)](#7-ciclos-de-verificación)
8. [Modo Batch vs Interactivo](#8-modo-batch-vs-interactivo)
9. [Métricas de Calidad](#9-métricas-de-calidad)
10. [Límites de Seguridad](#10-límites-de-seguridad)

---

## 1. Visión del Agente

### 1.1 Definición

El **DocAgent** es un agente MCP autónomo especializado en la gestión integral del ciclo de vida de la documentación técnica. Opera como un "Technical Writer AI" que:

- **Lee** código, ADRs, issues, PRs y documentación existente
- **Analiza** coherencia, completitud y frescura del conocimiento
- **Genera** documentación nueva cuando se implementan features
- **Actualiza** documentación existente cuando el código cambia
- **Detecta** inconsistencias entre docs y código
- **Valida** calidad, links, ejemplos y estructura
- **Crea** diagramas a partir del análisis de código
- **Mantiene** changelogs y release notes

### 1.2 Principios Operativos

| Principio | Descripción |
|-----------|-------------|
| **Autonomía controlada** | El agente propone cambios, el humano aprueba (human-in-the-loop por defecto) |
| **Evidence-based** | Toda generación se basa en evidencia del código/docs, nunca inventa |
| **Incremental** | Prefiere actualizaciones pequeñas y frecuentes sobre reescrituras |
| **Multi-step reasoning** | Descompone tareas complejas en pasos verificables |
| **Self-evaluation** | Cada output pasa por un ciclo de auto-evaluación antes de proponer |
| **Traceable** | Cada cambio documenta su razón, fuentes y confianza |

### 1.3 Capacidades del Agente

```
┌─────────────────────────────────────────────────────────────────┐
│                     DOC AGENT CAPABILITIES                       │
│                                                                   │
│  ┌─────────────┐  ┌──────────────┐  ┌─────────────────────┐    │
│  │ GENERATION  │  │  MAINTENANCE │  │    VALIDATION       │    │
│  │             │  │              │  │                     │    │
│  │ • API docs  │  │ • Sync with  │  │ • Link checking    │    │
│  │ • README    │  │   code       │  │ • Example testing  │    │
│  │ • ADRs      │  │ • Update     │  │ • Consistency      │    │
│  │ • Guides    │  │   outdated   │  │ • Completeness     │    │
│  │ • Diagrams  │  │ • Changelog  │  │ • Style guide      │    │
│  │ • Specs     │  │ • Versioning │  │ • Accuracy         │    │
│  └─────────────┘  └──────────────┘  └─────────────────────┘    │
│                                                                   │
│  ┌─────────────┐  ┌──────────────┐  ┌─────────────────────┐    │
│  │  ANALYSIS   │  │  DISCOVERY   │  │    REPORTING        │    │
│  │             │  │              │  │                     │    │
│  │ • Coverage  │  │ • Undocu-    │  │ • Quality scores   │    │
│  │ • Freshness │  │   mented    │  │ • Coverage reports │    │
│  │ • Quality   │  │   code      │  │ • Staleness alerts │    │
│  │ • Gaps      │  │ • Missing   │  │ • Trend analysis   │    │
│  │ • Conflicts │  │   ADRs      │  │ • Audit logs       │    │
│  └─────────────┘  └──────────────┘  └─────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

---

## 2. Arquitectura del Agente

### 2.1 Patrón Planner/Executor/Reviewer (PER)

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         DOC AGENT ARCHITECTURE                          │
│                                                                         │
│  Input (trigger)                                                        │
│       │                                                                 │
│       ▼                                                                 │
│  ┌──────────────────────────────────────────────────────────────────┐   │
│  │                        PLANNER                                   │   │
│  │                                                                  │   │
│  │  1. Analyze trigger (git push, manual, schedule, CI)            │   │
│  │  2. Gather context (code diff, existing docs, knowledge store)  │   │
│  │  3. Decompose into atomic tasks                                 │   │
│  │  4. Prioritize and order tasks                                  │   │
│  │  5. Estimate effort and confidence                              │   │
│  │  6. Output: ExecutionPlan                                       │   │
│  │                                                                  │   │
│  │  Tools used: knowledge_search, git_diff, file_read, ast_parse   │   │
│  └──────────────────────┬───────────────────────────────────────────┘   │
│                         │                                               │
│                         ▼                                               │
│  ┌──────────────────────────────────────────────────────────────────┐   │
│  │                       EXECUTOR                                   │   │
│  │                                                                  │   │
│  │  For each task in plan:                                         │   │
│  │    1. Load required context (chunks, files, history)            │   │
│  │    2. Execute action (generate, update, validate, diagram)      │   │
│  │    3. Produce artifact (markdown, mermaid, changelog entry)     │   │
│  │    4. Track provenance (what sources informed this output)      │   │
│  │    5. Self-check: does output match intent?                     │   │
│  │                                                                  │   │
│  │  Tools used: file_write, knowledge_search, llm_generate,       │   │
│  │              diagram_generate, git_commit_info                   │   │
│  └──────────────────────┬───────────────────────────────────────────┘   │
│                         │                                               │
│                         ▼                                               │
│  ┌──────────────────────────────────────────────────────────────────┐   │
│  │                       REVIEWER                                   │   │
│  │                                                                  │   │
│  │  For each artifact:                                             │   │
│  │    1. Quality check (structure, grammar, completeness)          │   │
│  │    2. Accuracy check (does doc match code reality?)             │   │
│  │    3. Consistency check (conflicts with other docs?)            │   │
│  │    4. Style check (matches project conventions?)                │   │
│  │    5. Link validation (all references resolve?)                 │   │
│  │    6. Score: pass/revise/reject                                 │   │
│  │                                                                  │   │
│  │  If revise: return to Executor with feedback                    │   │
│  │  If reject: escalate to human                                   │   │
│  │  If pass: proceed to output                                     │   │
│  │                                                                  │   │
│  │  Tools used: knowledge_search, link_checker, style_checker,     │   │
│  │              code_symbol_resolver                                │   │
│  └──────────────────────┬───────────────────────────────────────────┘   │
│                         │                                               │
│              ┌──────────┼──────────┐                                   │
│              │          │          │                                    │
│              ▼          ▼          ▼                                    │
│         ┌────────┐ ┌────────┐ ┌──────────┐                            │
│         │ Output │ │Revision│ │ Escalate │                            │
│         │  (PR)  │ │ Loop   │ │ (Human)  │                            │
│         └────────┘ └────────┘ └──────────┘                            │
│                                                                         │
│  ┌──────────────────────────────────────────────────────────────────┐   │
│  │                    MEMORY SYSTEM                                  │   │
│  │                                                                  │   │
│  │  Persistent across sessions:                                    │   │
│  │  - Past decisions and their outcomes                            │   │
│  │  - User preferences and corrections                             │   │
│  │  - Project-specific conventions                                 │   │
│  │  - Quality score history                                        │   │
│  │  - Known issues and workarounds                                 │   │
│  └──────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Flujo de Decisión del Planner

```typescript
interface ExecutionPlan {
  id: string;
  trigger: PlanTrigger;
  context: PlanContext;
  tasks: PlannedTask[];
  estimatedDuration: Duration;
  confidenceScore: number;            // 0-1, qué tan seguro está el planner
  requiresHumanApproval: boolean;
}

interface PlannedTask {
  id: string;
  type: TaskType;
  priority: number;                   // 1 = highest
  description: string;
  targetFile: string;
  action: 'create' | 'update' | 'validate' | 'delete' | 'diagram';
  dependencies: string[];             // task IDs que deben completarse antes
  requiredContext: ContextRequirement[];
  estimatedTokens: number;
  confidence: number;
}

type TaskType =
  | 'generate_api_doc'
  | 'generate_readme'
  | 'generate_adr'
  | 'generate_changelog'
  | 'generate_diagram'
  | 'update_existing_doc'
  | 'fix_broken_links'
  | 'fix_outdated_content'
  | 'fix_inconsistency'
  | 'validate_quality'
  | 'validate_completeness'
  | 'coverage_report';

interface ContextRequirement {
  type: 'code_file' | 'doc_file' | 'knowledge_search' | 'git_history' | 'ast_analysis';
  query: string;
  required: boolean;
}
```

### 2.3 Revision Loop (Max Iterations)

```
Executor produces artifact
        │
        ▼
    Reviewer evaluates
        │
        ├── Score ≥ 0.8 → PASS → Output
        │
        ├── Score 0.5-0.8 → REVISE (max 2 iterations)
        │   │
        │   ├── Iteration 1: Reviewer provides specific feedback
        │   │   └── Executor revises with feedback context
        │   │
        │   └── Iteration 2: If still < 0.8
        │       └── ESCALATE to human with both versions + feedback
        │
        └── Score < 0.5 → REJECT → Escalate immediately
```

---

## 3. MCP Tools Disponibles

### 3.1 Catálogo de Tools

El DocAgent opera mediante el protocolo MCP (Model Context Protocol), exponiendo y consumiendo tools estandarizados.

#### Tools de Lectura (Read-Only)

```typescript
// ─── FILESYSTEM ───────────────────────────────────────
const fileReadTool: MCPTool = {
  name: 'file_read',
  description: 'Read a file from the repository',
  inputSchema: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Relative path from repo root' },
      repository: { type: 'string' },
      branch: { type: 'string', default: 'main' },
      lineRange: {
        type: 'object',
        properties: {
          start: { type: 'number' },
          end: { type: 'number' },
        },
      },
    },
    required: ['path', 'repository'],
  },
};

const fileListTool: MCPTool = {
  name: 'file_list',
  description: 'List files matching a glob pattern',
  inputSchema: {
    type: 'object',
    properties: {
      pattern: { type: 'string', description: 'Glob pattern, e.g. "src/**/*.ts"' },
      repository: { type: 'string' },
    },
    required: ['pattern', 'repository'],
  },
};

// ─── GIT ──────────────────────────────────────────────
const gitDiffTool: MCPTool = {
  name: 'git_diff',
  description: 'Get diff between commits or branches',
  inputSchema: {
    type: 'object',
    properties: {
      repository: { type: 'string' },
      from: { type: 'string', description: 'Commit SHA or branch' },
      to: { type: 'string', default: 'HEAD' },
      paths: { type: 'array', items: { type: 'string' } },
    },
    required: ['repository'],
  },
};

const gitLogTool: MCPTool = {
  name: 'git_log',
  description: 'Get commit history with messages',
  inputSchema: {
    type: 'object',
    properties: {
      repository: { type: 'string' },
      path: { type: 'string', description: 'Filter by file path' },
      limit: { type: 'number', default: 20 },
      since: { type: 'string', description: 'ISO date' },
    },
    required: ['repository'],
  },
};

const gitBlameTool: MCPTool = {
  name: 'git_blame',
  description: 'Get line-by-line authorship of a file',
  inputSchema: {
    type: 'object',
    properties: {
      path: { type: 'string' },
      repository: { type: 'string' },
      lineRange: {
        type: 'object',
        properties: { start: { type: 'number' }, end: { type: 'number' } },
      },
    },
    required: ['path', 'repository'],
  },
};

// ─── AST PARSER ───────────────────────────────────────
const astParseTool: MCPTool = {
  name: 'ast_parse',
  description: 'Parse source code and extract symbols (functions, classes, interfaces, types)',
  inputSchema: {
    type: 'object',
    properties: {
      path: { type: 'string' },
      repository: { type: 'string' },
      language: { type: 'string', enum: ['typescript', 'rust', 'python', 'go'] },
      extractTypes: {
        type: 'array',
        items: { type: 'string', enum: ['function', 'class', 'interface', 'type', 'enum', 'module', 'export'] },
      },
    },
    required: ['path', 'repository'],
  },
  outputSchema: {
    type: 'object',
    properties: {
      symbols: {
        type: 'array',
        items: {
          type: 'object',
          properties: {
            name: { type: 'string' },
            type: { type: 'string' },
            lineStart: { type: 'number' },
            lineEnd: { type: 'number' },
            signature: { type: 'string' },
            docComment: { type: 'string' },
            dependencies: { type: 'array', items: { type: 'string' } },
            isExported: { type: 'boolean' },
          },
        },
      },
    },
  },
};

// ─── KNOWLEDGE SEARCH ─────────────────────────────────
const knowledgeSearchTool: MCPTool = {
  name: 'knowledge_search',
  description: 'Search the knowledge store using hybrid retrieval (vector + BM25 + graph)',
  inputSchema: {
    type: 'object',
    properties: {
      query: { type: 'string' },
      filters: {
        type: 'object',
        properties: {
          repositories: { type: 'array', items: { type: 'string' } },
          contentTypes: { type: 'array', items: { type: 'string' } },
          domains: { type: 'array', items: { type: 'string' } },
        },
      },
      topK: { type: 'number', default: 5 },
      includeRelations: { type: 'boolean', default: false },
    },
    required: ['query'],
  },
};

// ─── CI/CD ────────────────────────────────────────────
const ciStatusTool: MCPTool = {
  name: 'ci_status',
  description: 'Get CI/CD pipeline status and test results',
  inputSchema: {
    type: 'object',
    properties: {
      repository: { type: 'string' },
      branch: { type: 'string', default: 'main' },
      pipelineId: { type: 'string' },
    },
    required: ['repository'],
  },
};

// ─── ISSUES/PR ────────────────────────────────────────
const issueReadTool: MCPTool = {
  name: 'issue_read',
  description: 'Read GitHub/GitLab issue details',
  inputSchema: {
    type: 'object',
    properties: {
      repository: { type: 'string' },
      issueNumber: { type: 'number' },
      includeComments: { type: 'boolean', default: true },
    },
    required: ['repository', 'issueNumber'],
  },
};

const prReadTool: MCPTool = {
  name: 'pr_read',
  description: 'Read pull request details including diff',
  inputSchema: {
    type: 'object',
    properties: {
      repository: { type: 'string' },
      prNumber: { type: 'number' },
      includeDiff: { type: 'boolean', default: true },
      includeReviews: { type: 'boolean', default: true },
    },
    required: ['repository', 'prNumber'],
  },
};
```

#### Tools de Escritura (Write — Human-Approved)

```typescript
// ─── DOCUMENTATION GENERATION ─────────────────────────
const docWriteTool: MCPTool = {
  name: 'doc_write',
  description: 'Write or update a documentation file. Requires human approval.',
  inputSchema: {
    type: 'object',
    properties: {
      path: { type: 'string' },
      content: { type: 'string' },
      repository: { type: 'string' },
      operation: { type: 'string', enum: ['create', 'update', 'append'] },
      commitMessage: { type: 'string' },
      sourcesUsed: {
        type: 'array',
        items: { type: 'string' },
        description: 'Chunk IDs or file paths used as evidence',
      },
    },
    required: ['path', 'content', 'repository', 'operation', 'commitMessage'],
  },
};

// ─── DIAGRAM GENERATION ───────────────────────────────
const diagramGenerateTool: MCPTool = {
  name: 'diagram_generate',
  description: 'Generate a Mermaid.js diagram from code or documentation analysis',
  inputSchema: {
    type: 'object',
    properties: {
      type: {
        type: 'string',
        enum: ['sequence', 'class', 'flowchart', 'er', 'c4_context', 'c4_container', 'state', 'gantt'],
      },
      title: { type: 'string' },
      sources: {
        type: 'array',
        items: { type: 'string' },
        description: 'File paths or chunk IDs to analyze',
      },
      outputPath: { type: 'string', description: 'Where to save the diagram' },
    },
    required: ['type', 'title', 'sources'],
  },
};

// ─── CHANGELOG ────────────────────────────────────────
const changelogUpdateTool: MCPTool = {
  name: 'changelog_update',
  description: 'Add entries to CHANGELOG.md based on recent commits',
  inputSchema: {
    type: 'object',
    properties: {
      repository: { type: 'string' },
      version: { type: 'string', description: 'Version for the changelog entry' },
      fromCommit: { type: 'string' },
      toCommit: { type: 'string', default: 'HEAD' },
      categories: {
        type: 'array',
        items: { type: 'string', enum: ['added', 'changed', 'deprecated', 'removed', 'fixed', 'security'] },
      },
    },
    required: ['repository', 'version'],
  },
};

// ─── QUALITY VALIDATION ───────────────────────────────
const docValidateTool: MCPTool = {
  name: 'doc_validate',
  description: 'Run quality validation checks on documentation',
  inputSchema: {
    type: 'object',
    properties: {
      paths: { type: 'array', items: { type: 'string' } },
      repository: { type: 'string' },
      checks: {
        type: 'array',
        items: {
          type: 'string',
          enum: [
            'broken_links',
            'outdated_content',
            'code_examples',
            'style_guide',
            'completeness',
            'consistency',
            'spelling',
            'structure',
          ],
        },
      },
    },
    required: ['paths', 'repository'],
  },
};

// ─── KNOWLEDGE STORE MANAGEMENT ───────────────────────
const knowledgeIngestTool: MCPTool = {
  name: 'knowledge_ingest',
  description: 'Trigger re-indexing of documents into the knowledge store',
  inputSchema: {
    type: 'object',
    properties: {
      sources: {
        type: 'array',
        items: {
          type: 'object',
          properties: {
            type: { type: 'string', enum: ['file', 'directory', 'git_diff'] },
            path: { type: 'string' },
            repository: { type: 'string' },
          },
        },
      },
      force: { type: 'boolean', default: false },
    },
    required: ['sources'],
  },
};
```

### 3.2 Tool Registry y Discovery

```typescript
interface MCPToolRegistry {
  // Tools disponibles para el DocAgent
  tools: Map<string, MCPTool>;

  // Categorías de tools
  categories: {
    read: string[];       // ['file_read', 'file_list', 'git_diff', 'git_log', ...]
    write: string[];      // ['doc_write', 'diagram_generate', 'changelog_update']
    search: string[];     // ['knowledge_search', 'ast_parse']
    validate: string[];   // ['doc_validate', 'link_checker']
    manage: string[];     // ['knowledge_ingest']
  };

  // Permisos por modo
  permissions: {
    interactive: string[];  // Todos los tools
    batch: string[];        // Todos excepto los que requieren human-in-the-loop
    readonly: string[];     // Solo tools de read y search
  };
}
```

---

## 4. Sistema de Memoria Persistente

### 4.1 Tipos de Memoria

```
┌─────────────────────────────────────────────────────────────────┐
│                    MEMORY SYSTEM                                 │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │  SHORT-TERM MEMORY (Session)                            │    │
│  │                                                          │    │
│  │  - Current task context                                  │    │
│  │  - Files read in this session                            │    │
│  │  - Search results cache                                  │    │
│  │  - Intermediate reasoning steps                          │    │
│  │  - User feedback received this session                   │    │
│  │                                                          │    │
│  │  Storage: In-memory (conversation context)               │    │
│  │  Lifetime: Single agent session                          │    │
│  └─────────────────────────────────────────────────────────┘    │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │  LONG-TERM MEMORY (Persistent)                          │    │
│  │                                                          │    │
│  │  ┌────────────────────────┐  ┌───────────────────────┐  │    │
│  │  │  EPISODIC MEMORY       │  │  SEMANTIC MEMORY      │  │    │
│  │  │                        │  │                       │  │    │
│  │  │  - Past tasks executed │  │  - Project conventions│  │    │
│  │  │  - Outcomes & scores   │  │  - Style preferences  │  │    │
│  │  │  - User corrections    │  │  - Domain glossary    │  │    │
│  │  │  - Failures & causes   │  │  - Architecture rules │  │    │
│  │  │  - Approval patterns   │  │  - Doc templates      │  │    │
│  │  │                        │  │  - Quality thresholds │  │    │
│  │  │  Storage: SQLite table │  │  Storage: Knowledge   │  │    │
│  │  │  + embeddings          │  │  Store (vector)       │  │    │
│  │  └────────────────────────┘  └───────────────────────┘  │    │
│  │                                                          │    │
│  │  ┌────────────────────────┐  ┌───────────────────────┐  │    │
│  │  │  PROCEDURAL MEMORY     │  │  REPOSITORY CONTEXT   │  │    │
│  │  │                        │  │                       │  │    │
│  │  │  - Learned workflows   │  │  - Repo structure map │  │    │
│  │  │  - Effective prompts   │  │  - Key files index    │  │    │
│  │  │  - Tool chains that    │  │  - Module dependency  │  │    │
│  │  │    work well           │  │    graph              │  │    │
│  │  │  - Error recovery      │  │  - Doc coverage map   │  │    │
│  │  │    patterns            │  │  - Owner/maintainer   │  │    │
│  │  │                        │  │    map                │  │    │
│  │  │  Storage: YAML config  │  │  Storage: Computed &  │  │    │
│  │  │  + versioned           │  │  cached (LanceDB)     │  │    │
│  │  └────────────────────────┘  └───────────────────────┘  │    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

### 4.2 Schema de Memoria

```sql
-- Episodic Memory
CREATE TABLE agent_episodes (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      TEXT NOT NULL,
    task_type       TEXT NOT NULL,
    trigger         TEXT NOT NULL,
    plan_summary    TEXT,
    actions_taken   JSONB NOT NULL,     -- [{tool, input, output, durationMs}]
    outcome         TEXT NOT NULL,       -- 'success' | 'partial' | 'failed' | 'escalated'
    quality_score   REAL,
    user_feedback   TEXT,               -- Direct user feedback if any
    user_corrections JSONB,            -- Specific corrections made by user
    lessons_learned TEXT,              -- Agent's self-reflection
    context_used    JSONB,             -- What knowledge/files were consulted
    tokens_used     INTEGER,
    cost_usd        REAL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    repository      TEXT NOT NULL
);

-- Semantic Memory (project conventions)
CREATE TABLE agent_conventions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    repository      TEXT NOT NULL,
    category        TEXT NOT NULL,     -- 'style' | 'structure' | 'naming' | 'content' | 'process'
    rule            TEXT NOT NULL,     -- The convention in natural language
    examples        JSONB,            -- Positive and negative examples
    source          TEXT,             -- How this was learned (user correction, inference, explicit)
    confidence      REAL DEFAULT 0.5,
    usage_count     INTEGER DEFAULT 0,
    last_used       TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Repository Context Cache
CREATE TABLE repo_context (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    repository      TEXT NOT NULL,
    context_type    TEXT NOT NULL,      -- 'structure' | 'dependencies' | 'coverage' | 'owners'
    data            JSONB NOT NULL,
    computed_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    valid_until     TIMESTAMPTZ,       -- When to recompute
    commit_sha      TEXT               -- Commit at which this was computed
);
```

### 4.3 Memory Retrieval

```typescript
interface AgentMemory {
  // Recuperar episodios relevantes para la tarea actual
  recallSimilarEpisodes(taskDescription: string, limit: number): Promise<Episode[]>;

  // Recuperar convenciones del proyecto
  getConventions(repository: string, category?: string): Promise<Convention[]>;

  // Recuperar contexto del repositorio
  getRepoContext(repository: string): Promise<RepoContext>;

  // Almacenar nuevo episodio
  recordEpisode(episode: Episode): Promise<void>;

  // Aprender nueva convención (de corrección de usuario)
  learnConvention(correction: UserCorrection): Promise<Convention>;

  // Reforzar convención existente (fue útil)
  reinforceConvention(conventionId: string): Promise<void>;

  // Deprecar convención (ya no aplica)
  deprecateConvention(conventionId: string, reason: string): Promise<void>;
}
```

---

## 5. Workflow Planner

### 5.1 Workflows Pre-definidos

```typescript
// ============================================================
// PREDEFINED WORKFLOWS
// ============================================================

const workflows: Record<string, WorkflowDefinition> = {
  // ─── Triggered by: git push con cambios en código ──────
  'code-change-doc-sync': {
    name: 'Sync documentation after code change',
    trigger: 'git_push',
    condition: 'changedFiles.some(f => f.startsWith("src/"))',
    steps: [
      {
        id: 'analyze_diff',
        action: 'Analyze git diff to identify changed symbols and modules',
        tools: ['git_diff', 'ast_parse'],
      },
      {
        id: 'find_affected_docs',
        action: 'Search knowledge store for documentation that references changed symbols',
        tools: ['knowledge_search'],
        dependsOn: ['analyze_diff'],
      },
      {
        id: 'check_staleness',
        action: 'Compare doc content with current code to detect outdated information',
        tools: ['file_read', 'knowledge_search'],
        dependsOn: ['find_affected_docs'],
      },
      {
        id: 'propose_updates',
        action: 'Generate proposed doc updates for stale sections',
        tools: ['doc_write'],
        dependsOn: ['check_staleness'],
        requiresApproval: true,
      },
    ],
  },

  // ─── Triggered by: new feature merged ──────────────────
  'new-feature-doc-generation': {
    name: 'Generate documentation for new feature',
    trigger: 'pr_merged',
    condition: 'pr.labels.includes("feature")',
    steps: [
      {
        id: 'analyze_feature',
        action: 'Read PR description, diff, and linked issues to understand the feature',
        tools: ['pr_read', 'issue_read', 'git_diff'],
      },
      {
        id: 'analyze_code',
        action: 'Parse new/modified files to extract public API surface',
        tools: ['ast_parse', 'file_read'],
        dependsOn: ['analyze_feature'],
      },
      {
        id: 'find_existing_docs',
        action: 'Check if related documentation already exists',
        tools: ['knowledge_search'],
        dependsOn: ['analyze_code'],
      },
      {
        id: 'generate_docs',
        action: 'Generate/update documentation including API docs, usage examples, and architecture notes',
        tools: ['doc_write', 'diagram_generate'],
        dependsOn: ['analyze_code', 'find_existing_docs'],
        requiresApproval: true,
      },
      {
        id: 'update_changelog',
        action: 'Add changelog entry for the new feature',
        tools: ['changelog_update'],
        dependsOn: ['analyze_feature'],
      },
    ],
  },

  // ─── Triggered by: schedule (weekly) ───────────────────
  'doc-quality-audit': {
    name: 'Weekly documentation quality audit',
    trigger: 'schedule',
    cron: '0 9 * * 1',  // Monday 9 AM
    steps: [
      {
        id: 'scan_all_docs',
        action: 'List all documentation files and compute freshness',
        tools: ['file_list', 'git_log'],
      },
      {
        id: 'validate_quality',
        action: 'Run quality checks on all documentation',
        tools: ['doc_validate'],
        dependsOn: ['scan_all_docs'],
      },
      {
        id: 'check_coverage',
        action: 'Compare documented vs undocumented public APIs',
        tools: ['ast_parse', 'knowledge_search'],
        dependsOn: ['scan_all_docs'],
      },
      {
        id: 'check_consistency',
        action: 'Detect contradictions between documents',
        tools: ['knowledge_search'],
        dependsOn: ['scan_all_docs'],
      },
      {
        id: 'generate_report',
        action: 'Produce quality report with scores, issues, and recommendations',
        tools: ['doc_write'],
        dependsOn: ['validate_quality', 'check_coverage', 'check_consistency'],
      },
    ],
  },

  // ─── Triggered by: manual request ──────────────────────
  'generate-adr': {
    name: 'Generate Architecture Decision Record',
    trigger: 'manual',
    steps: [
      {
        id: 'gather_context',
        action: 'Search knowledge store for related decisions, discussions, and code patterns',
        tools: ['knowledge_search', 'file_read'],
      },
      {
        id: 'find_alternatives',
        action: 'Research alternatives that were considered',
        tools: ['knowledge_search', 'issue_read'],
        dependsOn: ['gather_context'],
      },
      {
        id: 'generate_adr',
        action: 'Generate ADR following project template with context, decision, and consequences',
        tools: ['doc_write'],
        dependsOn: ['gather_context', 'find_alternatives'],
        requiresApproval: true,
      },
      {
        id: 'update_index',
        action: 'Update ADR index and link from related documentation',
        tools: ['doc_write'],
        dependsOn: ['generate_adr'],
      },
    ],
  },

  // ─── Triggered by: manual request ──────────────────────
  'generate-architecture-diagram': {
    name: 'Generate or update architecture diagrams',
    trigger: 'manual',
    steps: [
      {
        id: 'analyze_structure',
        action: 'Parse codebase structure, imports, and module boundaries',
        tools: ['ast_parse', 'file_list'],
      },
      {
        id: 'analyze_docs',
        action: 'Read existing architecture documentation for context',
        tools: ['knowledge_search', 'file_read'],
        dependsOn: ['analyze_structure'],
      },
      {
        id: 'generate_diagrams',
        action: 'Generate Mermaid diagrams: C4 context, container, component, and sequence diagrams',
        tools: ['diagram_generate'],
        dependsOn: ['analyze_structure', 'analyze_docs'],
        requiresApproval: true,
      },
    ],
  },
};
```

### 5.2 Dynamic Planning

Además de workflows predefinidos, el Planner puede crear plans dinámicos:

```typescript
interface DynamicPlanner {
  /**
   * Given a natural language request, create an execution plan.
   * Uses knowledge store + episodic memory to inform planning.
   */
  plan(request: string, context: PlanningContext): Promise<ExecutionPlan>;
}

interface PlanningContext {
  repository: string;
  recentChanges: GitChange[];
  userPreferences: Convention[];
  pastEpisodes: Episode[];          // Similar past tasks
  availableTools: string[];
  constraints: {
    maxTokenBudget: number;
    maxDurationMs: number;
    requiresApproval: boolean;
    allowedOperations: ('create' | 'update' | 'delete')[];
  };
}
```

---

## 6. Prompts Internos del Agente

### 6.1 System Prompt del Planner

```markdown
You are the DocAgent Planner for the Cuervo CLI project. Your role is to analyze
triggers (code changes, manual requests, schedules) and create precise execution
plans for documentation tasks.

## Your Capabilities
- You have access to the Knowledge Store via `knowledge_search`
- You can read code via `file_read` and `ast_parse`
- You can analyze git history via `git_diff` and `git_log`

## Planning Rules
1. ALWAYS gather evidence before planning actions. Never plan doc generation
   without first reading the relevant code.
2. PREFER updating existing documentation over creating new files.
3. DECOMPOSE complex tasks into atomic, verifiable steps.
4. ESTIMATE confidence for each step (0-1). If confidence < 0.5, flag for
   human guidance.
5. RESPECT project conventions (loaded from memory).
6. Each planned task MUST have clear acceptance criteria.

## Output Format
Produce a structured ExecutionPlan with ordered tasks, dependencies, and
required context. Include your reasoning for each decision.

## Project Context
{repository_context}

## Conventions
{project_conventions}

## Relevant Past Episodes
{similar_episodes}
```

### 6.2 System Prompt del Executor

```markdown
You are the DocAgent Executor for the Cuervo CLI project. You execute individual
documentation tasks from an approved execution plan.

## Your Responsibilities
1. Load all required context before generating content
2. Generate documentation that is:
   - Accurate: matches the actual code behavior
   - Complete: covers all public APIs and important behaviors
   - Consistent: uses the same terminology as existing docs
   - Well-structured: follows project templates and conventions
3. Track provenance: record which sources informed your output
4. Self-check: verify your output against the original intent

## Writing Guidelines
- Language: Match the language of existing docs (Spanish for Cuervo project docs)
- Format: Markdown with Mermaid for diagrams
- Code examples: Must be syntactically correct and tested
- Links: Use relative paths for internal references
- Headings: Follow existing hierarchy and naming patterns

## Current Task
{task_description}

## Required Context
{loaded_context}

## Project Conventions
{conventions}
```

### 6.3 System Prompt del Reviewer

```markdown
You are the DocAgent Reviewer for the Cuervo CLI project. You evaluate
documentation artifacts produced by the Executor for quality, accuracy,
and consistency.

## Evaluation Criteria (each scored 0-1)

### Accuracy (weight: 0.30)
- Does the documentation correctly describe the code's behavior?
- Are API signatures accurate?
- Are configuration options correct?
- Are there any factual errors?

### Completeness (weight: 0.25)
- Are all public APIs documented?
- Are edge cases and error scenarios covered?
- Are prerequisites and dependencies mentioned?
- Are examples provided for complex operations?

### Consistency (weight: 0.20)
- Does terminology match the rest of the project?
- Are naming conventions followed?
- Does structure match the project template?
- Are there contradictions with other documents?

### Clarity (weight: 0.15)
- Is the writing clear and unambiguous?
- Are complex concepts explained with examples?
- Is the target audience appropriate?
- Are diagrams used where helpful?

### Maintainability (weight: 0.10)
- Will this doc be easy to keep updated?
- Are there hardcoded values that will become stale?
- Are links relative (not absolute)?
- Is the scope narrow enough to update independently?

## Output Format
For each criterion, provide:
- Score (0-1)
- Specific evidence for the score
- Actionable feedback if score < 0.8

Overall weighted score determines action:
- >= 0.8: PASS
- 0.5-0.8: REVISE (with specific feedback)
- < 0.5: REJECT (with explanation)

## Artifact to Review
{artifact_content}

## Source Code Referenced
{source_code}

## Existing Related Documentation
{related_docs}
```

---

## 7. Ciclos de Verificación

### 7.1 Self-Check Pipeline

```
Executor produces artifact
        │
        ▼
┌─────────────────────────────────────────┐
│           SELF-CHECK PIPELINE            │
│                                          │
│  1. STRUCTURAL CHECK                     │
│     □ Valid Markdown (parseable)         │
│     □ Heading hierarchy correct          │
│     □ No broken internal links           │
│     □ Code blocks have language tags     │
│     □ Tables properly formatted          │
│                                          │
│  2. FACTUAL CHECK                        │
│     □ Code references resolve to real    │
│       symbols (via ast_parse)            │
│     □ File paths mentioned exist         │
│     □ Version numbers are current        │
│     □ Config keys mentioned exist        │
│                                          │
│  3. CONSISTENCY CHECK                    │
│     □ No contradictions with knowledge   │
│       store (via knowledge_search)       │
│     □ Terminology matches glossary       │
│     □ Style matches conventions          │
│                                          │
│  4. COMPLETENESS CHECK                   │
│     □ All sections from template present │
│     □ All public symbols documented      │
│     □ Examples provided for complex APIs │
│     □ Error scenarios covered            │
│                                          │
│  Auto-fix: structural issues             │
│  Flag: factual/consistency issues        │
│  Score: aggregate all checks             │
└─────────────────────────────────────────┘
```

### 7.2 Verification Against Code

```typescript
interface CodeVerifier {
  /**
   * Verify that documentation accurately reflects the code.
   * Returns a list of discrepancies.
   */
  verifyAccuracy(
    docContent: string,
    referencedFiles: string[],
    repository: string
  ): Promise<Discrepancy[]>;
}

interface Discrepancy {
  type: 'wrong_signature' | 'missing_parameter' | 'wrong_return_type' |
        'deprecated_reference' | 'nonexistent_symbol' | 'wrong_file_path' |
        'outdated_config' | 'wrong_version';
  severity: 'error' | 'warning' | 'info';
  location: {
    docFile: string;
    docLine: number;
    codeFile?: string;
    codeLine?: number;
  };
  expected: string;
  found: string;
  suggestion: string;
}
```

### 7.3 Hallucination Guard

```typescript
interface HallucinationGuard {
  /**
   * Check if generated content contains claims not supported by evidence.
   * Every factual claim must be traceable to a source (code, doc, or commit).
   */
  checkProvenance(
    generatedContent: string,
    sourcesUsed: Source[]
  ): Promise<ProvenanceReport>;
}

interface ProvenanceReport {
  totalClaims: number;
  supportedClaims: number;
  unsupportedClaims: Claim[];
  provenanceScore: number;            // supportedClaims / totalClaims
}

interface Claim {
  text: string;
  location: { line: number; column: number };
  supportingSource: Source | null;
  confidence: number;
}
```

---

## 8. Modo Batch vs Interactivo

### 8.1 Modo Interactivo

```
┌─────────────────────────────────────────────┐
│           INTERACTIVE MODE                   │
│                                              │
│  User: "Documenta el nuevo endpoint /login"  │
│         │                                    │
│         ▼                                    │
│  Agent: "Analyzing endpoint... Found POST    │
│          /api/auth/login in auth.controller. │
│          I'll generate API documentation.    │
│          Should I also update the README?"   │
│         │                                    │
│         ▼                                    │
│  User: "Yes, and add a sequence diagram"     │
│         │                                    │
│         ▼                                    │
│  Agent: [generates docs, shows preview]      │
│         "Here's the proposed documentation.  │
│          Review the changes?"                │
│         │                                    │
│         ▼                                    │
│  User: "Change the description of param X"   │
│         │                                    │
│         ▼                                    │
│  Agent: [applies correction, learns pref]    │
│         "Updated. Creating PR..."            │
│                                              │
│  Features:                                   │
│  - Conversational flow                       │
│  - Human-in-the-loop at each step           │
│  - Real-time preview of changes             │
│  - Learns from corrections                  │
│  - Uses MCP Elicitation for confirmations   │
└─────────────────────────────────────────────┘
```

### 8.2 Modo Batch

```
┌─────────────────────────────────────────────┐
│            BATCH MODE                        │
│                                              │
│  Trigger: git push / schedule / CI pipeline  │
│         │                                    │
│         ▼                                    │
│  Agent executes predefined workflow          │
│  (no human interaction during execution)     │
│         │                                    │
│         ▼                                    │
│  All write operations collected              │
│  (not committed immediately)                 │
│         │                                    │
│         ▼                                    │
│  Quality validation runs automatically       │
│         │                                    │
│    ┌────┴────┐                              │
│    │         │                              │
│    ▼         ▼                              │
│  PASS      ISSUES                           │
│    │         │                              │
│    ▼         ▼                              │
│  Create    Create PR                        │
│  PR with   with issues                      │
│  changes   flagged as                       │
│            review comments                  │
│                                              │
│  Features:                                   │
│  - Fully autonomous execution               │
│  - Human review via PR (async)              │
│  - Batch optimized (fewer API calls)        │
│  - Scheduled runs (weekly audits)           │
│  - CI/CD integration                        │
│  - No MCP Elicitation (read-only tools)     │
└─────────────────────────────────────────────┘
```

### 8.3 Comparación

| Aspecto | Interactivo | Batch |
|---------|------------|-------|
| Trigger | Usuario pide explícitamente | Git push, schedule, CI |
| Latencia | Segundos (conversacional) | Minutos (async) |
| Human-in-the-loop | En cada paso | Solo vía PR review |
| Tools permitidos | Todos (read + write) | Solo read + doc_write (no destructive) |
| Aprendizaje | En tiempo real de correcciones | Post-PR-review |
| Ideal para | Tareas específicas, exploración | Mantenimiento rutinario, auditorías |
| LLM calls | Muchos (conversación) | Optimizados (batch) |
| Costo típico | $0.05-0.50 por sesión | $0.10-2.00 por run |

---

## 9. Métricas de Calidad

### 9.1 Doc Quality Score (DQS)

```typescript
interface DocQualityScore {
  // Score compuesto (0-100)
  overall: number;

  // Dimensiones
  dimensions: {
    accuracy: number;           // ¿La doc refleja el código actual?
    completeness: number;       // ¿Todos los APIs públicos documentados?
    freshness: number;          // ¿Cuándo se actualizó por última vez vs código?
    consistency: number;        // ¿Coherente con otras docs?
    clarity: number;            // ¿Clara y bien escrita?
    structure: number;          // ¿Bien organizada?
    examples: number;           // ¿Tiene ejemplos funcionales?
    links: number;              // ¿Links válidos?
  };

  // Metadata
  computedAt: Date;
  documentPath: string;
  documentId: string;
  evaluationMethod: 'automated' | 'agent_review' | 'human_review';
}

// Cálculo del DQS
function computeDQS(dimensions: QualityDimensions): number {
  const weights = {
    accuracy: 0.25,
    completeness: 0.20,
    freshness: 0.15,
    consistency: 0.15,
    clarity: 0.10,
    structure: 0.05,
    examples: 0.05,
    links: 0.05,
  };

  return Object.entries(weights).reduce(
    (score, [dim, weight]) => score + dimensions[dim] * weight * 100,
    0
  );
}
```

### 9.2 Coverage Metrics

```typescript
interface DocCoverageReport {
  // Por repositorio
  repository: string;
  computedAt: Date;

  // Cobertura de código
  code: {
    totalPublicSymbols: number;
    documentedSymbols: number;
    coveragePercent: number;       // target: ≥80%
    undocumented: UndocumentedSymbol[];
  };

  // Cobertura de APIs
  api: {
    totalEndpoints: number;
    documentedEndpoints: number;
    coveragePercent: number;       // target: 100%
    undocumented: UndocumentedEndpoint[];
  };

  // Cobertura de ADRs
  adr: {
    totalDecisions: number;        // Inferred from code patterns + issues
    documentedDecisions: number;
    coveragePercent: number;
    suggestedADRs: SuggestedADR[];
  };

  // Frescura
  freshness: {
    totalDocs: number;
    freshDocs: number;             // Updated within 30 days of related code change
    staleDocs: number;             // Not updated after related code change
    averageStalenessHours: number;
    staleDocsList: StaleDoc[];
  };
}
```

### 9.3 Agent Performance Metrics

```typescript
interface AgentPerformanceMetrics {
  // Efectividad
  tasksCompleted: number;
  tasksSuccessful: number;           // Passed review
  tasksRevised: number;              // Required revision
  tasksEscalated: number;            // Required human help
  successRate: number;               // target: ≥85%

  // Calidad
  averageDQS: number;               // Average Doc Quality Score
  averageProvenanceScore: number;    // How well-sourced are outputs
  hallucinationRate: number;         // Claims without evidence / total claims

  // Eficiencia
  averageTaskDurationMs: number;
  averageTokensPerTask: number;
  averageCostPerTask: number;
  cacheHitRate: number;              // Knowledge store cache hits

  // Aprendizaje
  conventionsLearned: number;
  correctionsReceived: number;
  conventionAccuracy: number;        // % of conventions that were useful

  // Period
  period: { from: Date; to: Date };
  repository: string;
}
```

---

## 10. Límites de Seguridad

### 10.1 Guardrails del Agente

```typescript
interface AgentGuardrails {
  // ─── OPERATION LIMITS ────────────────────────────
  maxFilesPerRun: number;              // 20 — prevent runaway batch operations
  maxTokensPerTask: number;            // 100,000 — prevent excessive LLM usage
  maxCostPerRun: number;               // $5.00 — hard budget limit
  maxDurationPerTask: number;          // 300,000ms (5 min) — timeout
  maxRevisionIterations: number;       // 2 — prevent infinite revision loops

  // ─── WRITE LIMITS ────────────────────────────────
  allowedWritePaths: string[];         // Only docs/ directory
  forbiddenPaths: string[];            // Never touch src/, .env, credentials
  maxFileSizeBytes: number;            // 500KB per file
  requireApprovalForDelete: boolean;   // Always true
  requireApprovalForCreate: boolean;   // True in batch mode

  // ─── CONTENT LIMITS ──────────────────────────────
  noPII: boolean;                      // Never include personal information
  noSecrets: boolean;                  // Never include API keys, passwords
  noExecutableCode: boolean;           // Generated code examples are display-only
  languageFilter: boolean;             // No offensive content

  // ─── SCOPE LIMITS ────────────────────────────────
  allowedRepositories: string[];       // Only repos agent is authorized for
  allowedBranches: string[];           // Only feature branches, never main directly
  requirePR: boolean;                  // All changes via PR, never direct push
}
```

### 10.2 Escalation Policy

```
┌─────────────────────────────────────────────────────────┐
│              ESCALATION POLICY                           │
│                                                           │
│  Auto-escalate to human when:                            │
│                                                           │
│  1. Confidence < 0.5 on any planned task                │
│  2. Reviewer rejects artifact (score < 0.5)             │
│  3. Contradictions detected between docs                 │
│  4. Code behavior unclear (ambiguous logic)              │
│  5. Security-sensitive documentation                     │
│  6. Breaking change documentation                        │
│  7. Budget limit approaching (>80% spent)                │
│  8. Novel task type (no similar episodes in memory)      │
│  9. User explicitly requested review                     │
│  10. Multiple revision iterations exhausted              │
│                                                           │
│  Escalation channel:                                     │
│  - Interactive mode: Direct message to user              │
│  - Batch mode: PR comment + label "needs-human-review"   │
│  - CI mode: Pipeline annotation + Slack notification     │
└─────────────────────────────────────────────────────────┘
```

### 10.3 Audit Trail

```typescript
interface AgentAuditEntry {
  id: string;
  timestamp: Date;
  sessionId: string;
  agentVersion: string;

  // What happened
  action: string;                     // tool name called
  input: Record<string, unknown>;     // sanitized tool input (no secrets)
  output: Record<string, unknown>;    // sanitized tool output

  // Why
  reasoning: string;                  // Agent's reasoning for this action
  planTaskId: string;                 // Which plan task this belongs to

  // Who
  triggeredBy: 'user' | 'schedule' | 'git_hook' | 'ci_pipeline';
  userId?: string;

  // Provenance
  sourcesConsulted: string[];         // Chunk IDs, file paths used
  modelUsed: string;                  // Which LLM was used
  tokensUsed: number;
  costUsd: number;

  // Outcome
  result: 'success' | 'failure' | 'escalated';
  humanApproved: boolean | null;      // null = not yet reviewed
}
```

---

*Siguiente documento: [04-docops-automation.md](./04-docops-automation.md) — Automatización DocOps*
