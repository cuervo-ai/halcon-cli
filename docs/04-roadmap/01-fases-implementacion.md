# Roadmap de Desarrollo — Fases de Implementación

**Proyecto:** Cuervo CLI
**Versión:** 1.0
**Fecha:** 6 de febrero de 2026

---

## Resumen Ejecutivo

El desarrollo de Cuervo CLI se estructura en **3 fases principales** (MVP, Beta, GA) con un timeline estimado de **12 meses** hasta General Availability. Cada fase tiene entregables claros, criterios de exit, y métricas de éxito.

---

## 1. Timeline General

```
2026
Feb    Mar    Apr    May    Jun    Jul    Aug    Sep    Oct    Nov    Dec    Jan 2027
 ▼      ▼      ▼      ▼      ▼      ▼      ▼      ▼      ▼      ▼      ▼      ▼
 ├──────────────────────┤
 │    FASE 1: MVP       │
 │   (Feb — May 2026)   │
 │   ~16 semanas        │
 │                      │
 │  Sprint 1-2: Core    │
 │  Sprint 3-4: Models  │
 │  Sprint 5-6: Tools   │
 │  Sprint 7-8: Polish  │
 ├──────────────────────┼────────────────────────┤
                        │    FASE 2: BETA        │
                        │   (Jun — Sep 2026)     │
                        │   ~16 semanas          │
                        │                        │
                        │  Sprint 9-10: Agents   │
                        │  Sprint 11-12: RAG     │
                        │  Sprint 13-14: Plugins │
                        │  Sprint 15-16: Scale   │
                        ├────────────────────────┼──────────────────────┤
                                                 │   FASE 3: GA        │
                                                 │  (Oct — Jan 2027)   │
                                                 │  ~16 semanas        │
                                                 │                     │
                                                 │  Sprint 17-18: Ent. │
                                                 │  Sprint 19-20: FT   │
                                                 │  Sprint 21-22: Cert │
                                                 │  Sprint 23-24: GA   │
                                                 └─────────────────────┘
```

---

## 2. FASE 1: MVP (Febrero — Mayo 2026)

### 2.1 Objetivo

> Entregar un CLI funcional que un desarrollador pueda instalar, configurar con al menos un modelo (local o cloud), y usar para tareas básicas de desarrollo: editar código, debug, commit, y explicar código.

### 2.2 Sprints y Entregables

| Sprint | Semanas | Entregable | Criterio de Done |
|--------|---------|-----------|-----------------|
| **S1-S2** | 1-4 | **Core CLI + REPL** | REPL funcional, command parser, output renderer, historial |
| **S3-S4** | 5-8 | **Model Gateway (2 providers)** | Anthropic + Ollama integrados, streaming, error handling |
| **S5-S6** | 9-12 | **Tool System + Rust Scanner** | File ops (read/write/edit), bash sandboxed, git basics, glob/grep. **scanner.rs** (Rust/napi-rs): fastGlob + fastGrep con fallback TS |
| **S7-S8** | 13-16 | **Polish + Auth + Native Build** | Config system, auth client, installer, docs, testing. CI pipeline para compilar binarios Rust multiplataforma |

### 2.3 Features MVP

```
✅ INCLUIDO EN MVP
├── REPL interactivo con Markdown rendering
├── Slash commands: /commit, /explain, /help
├── Modelo Anthropic (Claude Sonnet/Opus)
├── Modelo local (Ollama)
├── Configuración por proyecto (.cuervo/config.yml)
├── Configuración global (~/.cuervo/)
├── File operations: read, write, edit, create
├── Bash execution (sandboxed)
├── Git integration: status, diff, commit
├── Rust native scanner (scanner.rs via napi-rs) con fallback TS
├── CI cross-compilation: darwin-arm64, darwin-x64, linux-x64-gnu, win32-x64
├── Proyecto memory file (CUERVO.md)
├── Permission system (confirm destructive ops)
├── Error handling y graceful degradation
├── Instalación via npm / brew
├── Documentación básica en español e inglés
├── Unit tests (>80% coverage)
├── CI/CD pipeline (GitHub Actions)
└── Telemetría básica (opt-in)

❌ EXCLUIDO DEL MVP (defer a Beta/GA)
├── Multi-agent orchestration
├── RAG / semantic search
├── Plugin system
├── Model routing inteligente
├── Code review automatizado
├── Fine-tuning pipeline
├── SOC 2 / ISO certifications
├── Enterprise features
└── Self-hosted platform
```

### 2.4 Criterios de Exit de MVP

| Criterio | Target |
|----------|--------|
| CLI se instala en <2 min en macOS/Linux | ✓ |
| Primer uso exitoso en <5 min | ✓ |
| Funciona con Ollama (offline) | ✓ |
| Funciona con Claude API (online) | ✓ |
| Tests unitarios >80% coverage | ✓ |
| 0 vulnerabilidades críticas/altas | ✓ |
| Documentación completa (español + inglés) | ✓ |
| 10+ beta testers internos satisfechos | ✓ |

---

## 3. FASE 2: BETA (Junio — Septiembre 2026)

### 3.1 Objetivo

> Añadir capacidades agénticas (multi-agent, RAG, plugins), soporte multi-modelo completo, y preparar la plataforma para adopción externa con programa de beta testers.

### 3.2 Sprints y Entregables

| Sprint | Semanas | Entregable | Criterio de Done |
|--------|---------|-----------|-----------------|
| **S9-S10** | 1-4 | **Multi-Agent System** | Orchestrator + Explorer + Planner + Executor agents |
| **S11-S12** | 5-8 | **RAG + Semantic Search (LanceDB + tree-sitter)** | Codebase indexing via **treesitter.rs** (Rust), vector store **LanceDB** (Rust core), hybrid search (ANN + FTS Tantivy) |
| **S13-S14** | 9-12 | **Plugin System + Multi-Model + Tokenizer** | Plugin API, 5+ model providers, routing inteligente. **tokenizer.rs** (Rust): token counting nativo multi-provider |
| **S15-S16** | 13-16 | **Scale + Security + PII Nativo** | Rate limiting, audit logging, **pii.rs** (Rust SIMD-accelerated), circuit breaker |

### 3.3 Features Beta

```
✅ INCLUIDO EN BETA (adicional al MVP)
├── Multi-agent: orchestrator, explorer, planner, executor, reviewer
├── Agent parallelism y background execution
├── Task tracking visible
├── RAG: codebase indexing con embeddings (LanceDB + treesitter.rs)
├── Semantic search en codebase (LanceDB ANN + Tantivy FTS)
├── Hybrid search (keyword + semantic + AST via Rust nativo)
├── Plugin system con API documentada
├── Custom slash commands
├── Hooks system (pre/post)
├── Model providers: +OpenAI, +Google, +DeepSeek, +Mistral
├── Intelligent model routing (auto-selection)
├── Fallback chain entre providers
├── Token budget management (tokenizer.rs nativo)
├── Code review automatizado (/review, /review-pr)
├── Audit logging completo
├── PII detection y redaction (pii.rs Rust SIMD-accelerated)
├── Zero-retention mode
├── Rate limiting
├── Circuit breaker per provider
├── Semantic cache
├── Integration tests (>60%)
├── Beta program público (100+ testers)
├── SOC 2 Type I readiness
└── OWASP LLM Top 10 mitigations
```

### 3.4 Criterios de Exit de Beta

| Criterio | Target |
|----------|--------|
| 100+ beta testers activos | ✓ |
| NPS > 40 | ✓ |
| SWE-bench Lite > 40% (orquestación multi-agent) | ✓ |
| Latencia p95 < 3s (chat con Sonnet) | ✓ |
| 5+ model providers integrados y testeados | ✓ |
| Plugin API estable (sin breaking changes) | ✓ |
| Audit logging 100% de operaciones AI | ✓ |
| 0 vulnerabilidades críticas | ✓ |
| SOC 2 Type I assessment completado | ✓ |
| Documentación 100% actualizada | ✓ |

---

## 4. FASE 3: GA (Octubre 2026 — Enero 2027)

### 4.1 Objetivo

> Lanzar la versión General Availability con features enterprise, certificaciones de seguridad, fine-tuning pipeline, y preparación para monetización.

### 4.2 Sprints y Entregables

| Sprint | Semanas | Entregable | Criterio de Done |
|--------|---------|-----------|-----------------|
| **S17-S18** | 1-4 | **Enterprise Features** | SSO, teams, org management, self-hosted mode |
| **S19-S20** | 5-8 | **Fine-tuning + Advanced** | Training pipeline, model evaluation, code transformation |
| **S21-S22** | 9-12 | **Certifications + Security** | SOC 2 Type II, ISO prep, pen testing |
| **S23-S24** | 13-16 | **GA Launch** | Production hardening, marketing, launch |

### 4.3 Features GA

```
✅ INCLUIDO EN GA (adicional a Beta)
├── Enterprise: SSO (SAML/OIDC), teams, org management
├── Self-hosted deployment mode
├── Fine-tuning pipeline integrado
├── Model evaluation framework
├── Code transformation (language/framework migration)
├── CI/CD pipeline integration (GitHub Actions, GitLab CI)
├── Issue tracker integration (Jira, Linear)
├── cuervo-picura-ide integration
├── Plugin marketplace / registry
├── Plugin SDK con templates
├── IP indemnification (enterprise tier)
├── SOC 2 Type II certification
├── ISO 27001 alignment
├── ISO 42001 self-assessment
├── Penetration testing program
├── Multi-region deployment
├── 99.9% SLA
├── Enterprise support (SLA)
├── Billing y subscription management
└── Public documentation site
```

---

## 5. Plan de Pruebas

### 5.1 Estrategia de Testing por Fase

| Tipo de Test | MVP | Beta | GA |
|-------------|-----|------|----|
| **Unit tests** | >80% coverage | >85% | >90% |
| **Integration tests** | Core flows | >60% coverage | >75% |
| **E2E tests** | Happy paths | Happy + error paths | Comprehensive |
| **Performance tests** | Baseline metrics | Regression suite | Load testing |
| **Security tests** | SAST basic | OWASP LLM Top 10 | Pen testing |
| **Bias tests** | — | Basic framework | Quarterly audits |
| **Accessibility tests** | — | Basic | WCAG 2.1 AA |

### 5.2 Ambientes de Testing

```
LOCAL DEV     → Unit + Integration (Vitest)
CI/CD         → Full suite + SAST + dependency scan
STAGING       → E2E + Performance + Multi-model
PRE-PROD      → Canary deployment + Smoke tests
PRODUCTION    → Health checks + Synthetic monitoring
```

---

## 6. Métricas de Éxito y KPIs

### 6.1 KPIs de Producto

| KPI | MVP Target | Beta Target | GA Target |
|-----|-----------|-------------|-----------|
| **Usuarios activos diarios (DAU)** | 10 (internos) | 500 | 5,000 |
| **Usuarios activos mensuales (MAU)** | 30 | 2,000 | 20,000 |
| **Retención D7** | >50% | >60% | >70% |
| **Retención D30** | >30% | >40% | >50% |
| **NPS** | >30 | >40 | >50 |
| **Sesiones por usuario/día** | >2 | >3 | >4 |
| **Tiempo promedio de sesión** | >15 min | >20 min | >30 min |
| **GitHub stars** | 100 | 1,000 | 5,000 |

### 6.2 KPIs Técnicos

| KPI | MVP Target | Beta Target | GA Target |
|-----|-----------|-------------|-----------|
| **Latencia p50 (chat)** | <2s | <1.5s | <1s |
| **Latencia p95 (chat)** | <5s | <3s | <2s |
| **Uptime** | 95% | 99.5% | 99.9% |
| **Error rate** | <5% | <2% | <1% |
| **SWE-bench Lite** | 30% | 40% | 50% |
| **Test coverage** | 80% | 85% | 90% |
| **Startup time** | <1s | <500ms | <300ms |
| **Memory footprint (idle)** | <150MB | <100MB | <80MB |

### 6.3 KPIs de Negocio

| KPI | Beta Target | GA Target | Year 1 |
|-----|-------------|-----------|--------|
| **Conversiones free → paid** | — | 5% | 8% |
| **Revenue (MRR)** | $0 | $10K | $100K |
| **Enterprise deals** | 0 | 2 | 10 |
| **Costo por usuario activo** | <$10 | <$5 | <$3 |
| **Margen bruto** | — | >50% | >70% |
| **CAC (self-serve)** | — | <$50 | <$30 |
| **LTV/CAC** | — | >2x | >3x |

---

## 7. Riesgos y Mitigaciones

| Riesgo | Probabilidad | Impacto | Mitigación |
|--------|-------------|---------|-----------|
| Costos de API de modelos excesivos | Alta | Alto | Caching agresivo, routing inteligente, modelos locales |
| Cambios breaking en APIs de providers | Media | Alto | Abstracción via Model Gateway, tests de integración, versionado |
| Competidor lanza feature similar | Alta | Medio | Diferenciación en multi-modelo + self-hosted + LATAM |
| Regulaciones más estrictas (EU AI Act) | Confirmada | Alto | Compliance-by-design desde MVP |
| Dificultad de contratación ML/AI talent | Media | Alto | Remote-first, LATAM talent pool |
| Baja adopción inicial | Media | Alto | Open source core, community building, content marketing |
| Vulnerabilidad de seguridad crítica | Baja | Crítico | Security-by-design, pen testing, bug bounty |

---

## 8. Dependencias Externas

| Dependencia | Tipo | Riesgo | Mitigación |
|-------------|------|--------|-----------|
| Anthropic API | Provider | API changes, pricing | Multi-provider, abstraction layer |
| OpenAI API | Provider | API changes, rate limits | Multi-provider, caching |
| Ollama | Runtime local | Version compatibility | Pin versions, integration tests |
| Rust toolchain | Build nativo | Versión de compilador | Pin via rust-toolchain.toml, CI matrix |
| napi-rs | Bridge Rust→Node.js | Breaking changes | Pin major version, integration tests |
| LanceDB | Vector store | API stability (pre-1.0) | Abstracción VectorStore con fallback USearch |
| Node.js 22 LTS | Runtime | Security patches | Automated updates |
| GitHub Actions | CI/CD | Service availability | Self-hosted runner fallback |
| npm registry | Distribution | Availability | Mirror + alternative (brew) |

---

*Roadmap sujeto a ajuste basado en feedback de usuarios y evolución del mercado.*
