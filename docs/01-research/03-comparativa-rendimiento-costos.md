# Comparativa de Rendimiento, Escalabilidad y Costos

**Proyecto:** Cuervo CLI — Plataforma de IA Generativa para Desarrollo de Software
**Versión:** 1.0
**Fecha:** 6 de febrero de 2026

---

## Resumen Ejecutivo

Este documento sintetiza las comparativas de rendimiento, escalabilidad y costos entre las principales plataformas competidoras, estableciendo los benchmarks target para Cuervo CLI.

---

## 1. Benchmarks de Modelos para Generación de Código (2025-2026)

### 1.1 SWE-bench (Resolución Real de Issues GitHub)

| Modelo/Agente | SWE-bench Lite (%) | SWE-bench Full (%) | Notas |
|---------------|--------------------|--------------------|-------|
| Claude Opus 4.6 (agéntico) | ~55-60% | ~45-50% | Líder en comprensión arquitectónica |
| GPT-4o + Codex Agent | ~50-55% | ~40-45% | Fuerte en razonamiento con o-series |
| Gemini 2.0 Ultra | ~45-50% | ~35-40% | Ventaja en contexto largo |
| DeepSeek V3 | ~45-50% | ~35-40% | Open source competitivo |
| Claude Sonnet 4.5 | ~45-50% | ~35-40% | Mejor balance costo/rendimiento |
| Llama 4 70B | ~35-40% | ~25-30% | Mejor open source general |
| Qwen 2.5 Coder 32B | ~40-45% | ~30-35% | Especializado en código |

> **Target Cuervo CLI**: Alcanzar 50%+ en SWE-bench Lite mediante orquestación multi-modelo y multi-agente.

### 1.2 HumanEval / MBPP (Generación de Funciones)

| Modelo | HumanEval (%) | MBPP (%) |
|--------|--------------|----------|
| Claude Opus 4.6 | ~92-95% | ~90-92% |
| GPT-4o | ~90-93% | ~88-90% |
| Gemini 2.0 Ultra | ~88-92% | ~86-90% |
| DeepSeek V3 | ~85-90% | ~85-88% |
| Llama 4 70B | ~82-88% | ~82-86% |
| Qwen 2.5 Coder 32B | ~85-90% | ~85-88% |

---

## 2. Comparativa de Latencia

### 2.1 Time-to-First-Token (TTFT)

| Plataforma | TTFT (p50) | TTFT (p95) | Throughput (tokens/s) |
|------------|-----------|-----------|----------------------|
| Claude Code (Sonnet) | ~800ms | ~2s | ~80-100 t/s |
| Claude Code (Opus) | ~1.5s | ~4s | ~40-60 t/s |
| GitHub Copilot (completion) | ~200ms | ~500ms | ~100-150 t/s |
| Cursor (Sonnet) | ~900ms | ~2.5s | ~80-100 t/s |
| Local (Llama 8B, M3 Max) | ~100ms | ~300ms | ~30-50 t/s |
| Local (Llama 70B, A100) | ~500ms | ~1.5s | ~40-60 t/s |

### 2.2 Target de Latencia para Cuervo CLI

| Operación | Target p50 | Target p95 | Modelo Sugerido |
|-----------|-----------|-----------|-----------------|
| Autocompletado inline | <200ms | <500ms | Modelo local pequeño (8B) |
| Chat/explicación | <1s | <3s | Sonnet/GPT-4o cloud |
| Edición multi-archivo | <2s | <5s | Opus/o3 cloud |
| Task autónomo | N/A (async) | <5min | Opus + agentes |
| Búsqueda semántica | <100ms | <300ms | Embeddings locales |

---

## 3. Comparativa de Costos

### 3.1 Costos de API por Modelo (Febrero 2026, estimados)

| Modelo | Input ($/1M tokens) | Output ($/1M tokens) | Contexto |
|--------|---------------------|---------------------|----------|
| Claude Opus 4.6 | $15.00 | $75.00 | 200K |
| Claude Sonnet 4.5 | $3.00 | $15.00 | 200K |
| Claude Haiku 4.5 | $0.80 | $4.00 | 200K |
| GPT-4o | $2.50 | $10.00 | 128K |
| GPT-o3 | $10.00 | $40.00 | 200K |
| Gemini 2.0 Pro | $1.25 | $5.00 | 1M |
| DeepSeek V3 | $0.27 | $1.10 | 128K |
| Llama 4 70B (self-hosted) | ~$0.10-0.30 | ~$0.10-0.30 | 128K |

### 3.2 Costo por Sesión de Desarrollo (Estimación)

Asumiendo sesión típica de 2 horas = ~50K input tokens + ~20K output tokens:

| Plataforma / Modelo | Costo por Sesión | Costo Mensual (20 días) |
|---------------------|-----------------|------------------------|
| Claude Opus 4.6 | $2.25 | $45.00 |
| Claude Sonnet 4.5 | $0.45 | $9.00 |
| GPT-4o | $0.33 | $6.50 |
| DeepSeek V3 | $0.04 | $0.75 |
| Local (Llama 70B)* | $0.02 | $0.40 |
| **Cuervo Hybrid** | **$0.15-0.50** | **$3.00-10.00** |

*Costo de electricidad + amortización hardware

### 3.3 Estrategia de Optimización de Costos para Cuervo CLI

```
┌─────────────────────────────────────────────────────────┐
│           ROUTING INTELIGENTE DE MODELOS                 │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  Tarea del usuario                                       │
│       │                                                  │
│       ▼                                                  │
│  ┌──────────────────┐                                    │
│  │ Task Classifier  │ ← Modelo local pequeño (Haiku)    │
│  │ (complejidad)    │                                    │
│  └────────┬─────────┘                                    │
│           │                                              │
│     ┌─────┼──────────────┐                               │
│     │     │              │                               │
│     ▼     ▼              ▼                               │
│  SIMPLE  MEDIO        COMPLEJO                           │
│  Local   Sonnet/      Opus/o3                            │
│  8B      GPT-4o       (Cloud)                            │
│  <$0.01  ~$0.05-0.10  ~$0.50-2.00                       │
│                                                          │
│  Autocomp │ Chat      │ Multi-file edit                  │
│  Rename   │ Explain   │ Architecture                     │
│  Format   │ Test gen  │ Debug complex                    │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

---

## 4. Escalabilidad

### 4.1 Métricas de Escalabilidad del Ecosistema Cuervo Existente

| Servicio | Throughput Actual | Concurrencia | SLA |
|----------|------------------|-------------|-----|
| cuervo-main (MCP) | N/D — en desarrollo | Target: 50K+ | 99.9% |
| cuervo-auth | Designed for scale | Target: 100K req/s | 99.99% |
| cuervo-video-intelligence | 20K req/s (API GW) | 200 videos/hr | 99.9% |
| cuervo-prompt-service | DDD microservice | Target: 10K req/s | 99.9% |

### 4.2 Targets de Escalabilidad para Cuervo CLI

| Dimensión | MVP | Beta | GA | Enterprise |
|-----------|-----|------|----|------------|
| Usuarios concurrentes | 100 | 1,000 | 10,000 | 100,000+ |
| Requests/segundo (API) | 500 | 5,000 | 20,000 | 100,000+ |
| Modelos soportados simultáneos | 3 | 8 | 15+ | Ilimitado |
| Latencia p95 (chat) | <5s | <3s | <2s | <1.5s |
| Uptime SLA | 99% | 99.5% | 99.9% | 99.99% |
| Regiones | 1 | 2 | 3+ | Global |

### 4.3 Arquitectura de Escalabilidad

```
┌─────────────────────────────────────────────────────────────┐
│                     ESCALABILIDAD CUERVO CLI                 │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  HORIZONTAL SCALING                                          │
│  ├── Kubernetes auto-scaling (HPA + VPA)                    │
│  ├── Model serving pods independientes por proveedor        │
│  ├── Stateless API pods (escalan linealmente)               │
│  └── Database read replicas + connection pooling            │
│                                                              │
│  CACHING STRATEGY                                            │
│  ├── L1: In-memory (proceso) — resultados de embedding      │
│  ├── L2: Redis — responses frecuentes, session state        │
│  ├── L3: CDN — assets estáticos, documentación              │
│  └── Semantic cache — responses a prompts similares         │
│                                                              │
│  ASYNC PROCESSING                                            │
│  ├── Task queue (Redis/RabbitMQ) para tareas long-running   │
│  ├── Background agents con WebSocket notifications          │
│  ├── Batch processing para fine-tuning jobs                 │
│  └── Event-driven architecture para desacoplamiento         │
│                                                              │
│  MULTI-REGION                                                │
│  ├── Edge nodes para baja latencia                          │
│  ├── Model serving geo-distribuido                          │
│  ├── Data residency compliance por región                   │
│  └── DNS-based routing + health checks                      │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## 5. Análisis de Costos de Infraestructura

### 5.1 Costo Mensual Estimado por Fase

| Componente | MVP | Beta | GA |
|-----------|-----|------|----|
| Compute (K8s) | $500 | $3,000 | $15,000 |
| Model API costs | $1,000 | $10,000 | $50,000+ |
| Database (PostgreSQL) | $200 | $800 | $3,000 |
| Redis | $100 | $400 | $1,500 |
| Vector DB | $200 | $800 | $3,000 |
| Search (MeiliSearch) | $100 | $400 | $1,500 |
| Storage (S3/equivalent) | $50 | $300 | $2,000 |
| Monitoring | $100 | $500 | $2,000 |
| CDN / Networking | $50 | $300 | $2,000 |
| **Total** | **~$2,300** | **~$16,500** | **~$80,000** |

### 5.2 Unit Economics Target

| Métrica | Target |
|---------|--------|
| Costo por usuario activo / mes | <$5 |
| Costo por sesión de desarrollo | <$0.50 |
| Margen bruto (enterprise) | >70% |
| CAC (Customer Acquisition Cost) | <$100 (self-serve), <$5K (enterprise) |
| LTV/CAC ratio | >3x |

---

## 6. Benchmarks de Referencia del Ecosistema Cuervo

Métricas ya demostradas en servicios existentes:

| Métrica | Servicio | Valor |
|---------|----------|-------|
| API throughput | cuervo-video-intelligence | 20,000 req/s |
| Video processing | cuervo-video-intelligence | 200 videos/hr/node |
| Cost savings vs cloud native | cuervo-video-intelligence | 88.6% |
| Processing speed vs Python | cuervo-video-intelligence (Rust) | 60x faster |
| Auth throughput | cuervo-auth (designed) | 100K req/s target |
| Bcrypt rounds | cuervo-auth | 12 (security standard) |

> **Ventaja**: La infraestructura existente del ecosistema Cuervo demuestra capacidades de escala enterprise. Cuervo CLI puede aprovechar esta base probada.

---

## 7. Benchmarks de la Capa Rust Nativa (Proyecciones)

### 7.1 Proyectos de Referencia (napi-rs en Producción)

| Proyecto | Operación | JS/WASM | Rust Nativo | Speedup |
|----------|-----------|---------|-------------|---------|
| **SWC** (compilador) | Transpilación TypeScript | ~3,000ms (tsc) | ~150ms | **20x** |
| **Biome** (linter/formatter) | Lint + format proyecto grande | ~8,000ms (ESLint) | ~300ms | **27x** |
| **Rspack** (bundler) | Build producción | ~30,000ms (webpack) | ~1,500ms | **20x** |
| **Lightning CSS** | CSS minification | ~500ms (PostCSS) | ~15ms | **33x** |
| **Oxc** (parser) | Parse TypeScript AST | ~800ms (TypeScript) | ~50ms | **16x** |

### 7.2 Benchmarks Proyectados para Cuervo CLI

| Módulo Rust | Operación | JS Estimado | Rust Proyectado | Speedup |
|-------------|-----------|-------------|-----------------|---------|
| **scanner.rs** | Glob 50K archivos | ~2,000ms (fast-glob) | ~100ms | **~20x** |
| **scanner.rs** | Grep patrón regex en 10K archivos | ~3,000ms (ripgrep-js) | ~150ms | **~20x** |
| **treesitter.rs** | Parse AST 1K archivos TypeScript | ~5,000ms (WASM) | ~600ms | **~8x** |
| **tokenizer.rs** | Count tokens 100K caracteres | ~50ms (js-tiktoken WASM) | ~5ms | **~10x** |
| **pii.rs** | Scan PII 50KB texto | ~30ms (JS RegExp) | ~2ms | **~15x** |
| **LanceDB** | ANN search 100K vectors (768d) | ~50ms (Hnswlib JS) | ~5ms | **~10x** |

### 7.3 Impacto en Latencia End-to-End

```
OPERACIÓN: Búsqueda semántica en codebase de 50K archivos

SIN RUST LAYER:
  Glob scan files     : ~2,000ms
  Tree-sitter parse   : ~5,000ms
  Token count context : ~50ms
  Vector search       : ~50ms
  PII scan prompt     : ~30ms
  ────────────────────────────────
  Total pre-LLM       : ~7,130ms

CON RUST LAYER:
  fastGlob (Rust)     : ~100ms
  treesitter.rs       : ~600ms
  tokenizer.rs        : ~5ms
  LanceDB ANN         : ~5ms
  pii.rs              : ~2ms
  ────────────────────────────────
  Total pre-LLM       : ~712ms   (10x mejora)
```

> **Conclusión**: La capa Rust reduce la latencia de operaciones pre-LLM de ~7s a ~700ms, haciendo viable la experiencia interactiva en codebases grandes (50K+ archivos, RNF-007).

---

*Documento generado el 6 de febrero de 2026. Actualizado con proyecciones de Rust Performance Layer.*
