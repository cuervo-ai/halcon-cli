# Requisitos Funcionales y No Funcionales

**Proyecto:** Cuervo CLI — Plataforma de IA Generativa para Desarrollo de Software
**Versión:** 1.0
**Fecha:** 6 de febrero de 2026
**Clasificación:** Confidencial — Uso Interno

---

## Resumen Ejecutivo

Este documento define los requisitos funcionales (RF) y no funcionales (RNF) de Cuervo CLI, priorizados según su impacto en la propuesta de valor diferenciada: **multi-modelo, self-hosted, orquestación multi-agente, y LATAM-first**. La priorización utiliza MoSCoW (Must/Should/Could/Won't) y está alineada con las fases del roadmap (MVP → Beta → GA).

---

## 1. Requisitos Funcionales (RF)

### 1.1 Core — Interfaz CLI y UX

| ID | Requisito | Prioridad | Fase |
|----|-----------|-----------|------|
| RF-001 | REPL interactivo en terminal con soporte para comandos naturales en español e inglés | Must | MVP |
| RF-002 | Sistema de slash commands extensible (`/commit`, `/review`, `/explain`, `/test`, etc.) | Must | MVP |
| RF-003 | Autocompletado inteligente de comandos y argumentos | Should | Beta |
| RF-004 | Historial de conversaciones persistente por proyecto | Must | MVP |
| RF-005 | Soporte para modo interactivo y modo one-shot (comando único) | Must | MVP |
| RF-006 | Output formateado con Markdown renderizado en terminal (syntax highlighting) | Must | MVP |
| RF-007 | Progress indicators y spinners para operaciones largas | Must | MVP |
| RF-008 | Sistema de temas visuales configurable | Could | GA |
| RF-009 | Integración con multiplexores de terminal (tmux, screen) | Could | GA |
| RF-010 | Modo verbose/debug para troubleshooting | Must | MVP |

### 1.2 Gestión de Modelos de IA

| ID | Requisito | Prioridad | Fase |
|----|-----------|-----------|------|
| RF-101 | Soporte multi-proveedor: Anthropic (Claude), OpenAI (GPT/o-series), Google (Gemini), Meta (LLaMA), Alibaba (Qwen), DeepSeek | Must | MVP |
| RF-102 | Integración con Ollama para modelos locales | Must | MVP |
| RF-103 | Routing inteligente automático: clasificar complejidad de tarea y seleccionar modelo óptimo | Must | Beta |
| RF-104 | Fallback automático entre proveedores si uno falla | Must | Beta |
| RF-105 | Configuración de modelo por proyecto (`.cuervo/config.yml`) | Must | MVP |
| RF-106 | Registro de modelos con metadata (versión, capacidades, costos, latencia) | Should | Beta |
| RF-107 | Soporte para custom endpoints de modelos (self-hosted, proxies) | Must | MVP |
| RF-108 | Token budget management: límites configurables por sesión/día/mes | Should | Beta |
| RF-109 | Modelo de pricing transparente: mostrar costo estimado antes de operaciones caras | Should | Beta |
| RF-110 | Soporte para model fine-tuning workflow integrado | Could | GA |
| RF-111 | A/B testing entre modelos para evaluar calidad | Could | GA |
| RF-112 | Caché semántico: reutilizar responses para prompts similares | Should | Beta |

### 1.3 Operaciones sobre Código

| ID | Requisito | Prioridad | Fase |
|----|-----------|-----------|------|
| RF-201 | Lectura y comprensión de archivos individuales y directorios completos | Must | MVP |
| RF-202 | Edición de archivos con diff preview antes de aplicar cambios | Must | MVP |
| RF-203 | Creación de archivos nuevos con contenido generado por IA | Must | MVP |
| RF-204 | Búsqueda semántica en codebase (por significado, no solo texto) | Must | Beta |
| RF-205 | Refactoring multi-archivo con plan de ejecución visible | Must | MVP |
| RF-206 | Generación de tests unitarios, de integración y e2e | Must | MVP |
| RF-207 | Explicación de código con diferentes niveles de detalle | Must | MVP |
| RF-208 | Detección y corrección de bugs con contexto de error | Must | MVP |
| RF-209 | Generación de documentación (JSDoc, docstrings, README) | Should | Beta |
| RF-210 | Code review automatizado con sugerencias priorizadas | Should | Beta |
| RF-211 | Análisis de seguridad de código (SAST básico) | Should | Beta |
| RF-212 | Migración/transformación de código entre lenguajes/frameworks | Could | GA |
| RF-213 | Indexación de codebase con embeddings para RAG | Must | Beta |
| RF-214 | Soporte para Jupyter notebooks (.ipynb) | Should | Beta |

### 1.4 Integración con Herramientas de Desarrollo

| ID | Requisito | Prioridad | Fase |
|----|-----------|-----------|------|
| RF-301 | Integración nativa con Git (status, diff, commit, push, PR) | Must | MVP |
| RF-302 | Integración con GitHub CLI (`gh`) para PRs, issues, reviews | Must | MVP |
| RF-303 | Integración con GitLab y Bitbucket APIs | Should | Beta |
| RF-304 | Ejecución sandboxed de comandos bash | Must | MVP |
| RF-305 | Integración con package managers (npm, pip, cargo, go mod) | Should | Beta |
| RF-306 | Integración con Docker para build y deploy local | Should | Beta |
| RF-307 | Integración con CI/CD pipelines (GitHub Actions, GitLab CI) | Could | GA |
| RF-308 | Integración con sistemas de tickets (Jira, Linear, GitHub Issues) | Could | GA |
| RF-309 | Soporte MCP (Model Context Protocol) para extensibilidad | Must | Beta |
| RF-310 | Integración con cuervo-picura-ide como backend de IA | Should | GA |

### 1.5 Sistema de Agentes

| ID | Requisito | Prioridad | Fase |
|----|-----------|-----------|------|
| RF-401 | Agente principal (orchestrator) que delega a sub-agentes especializados | Must | Beta |
| RF-402 | Agente explorador: navegación rápida de codebase | Must | MVP |
| RF-403 | Agente planificador: diseño de estrategia de implementación antes de codificar | Must | MVP |
| RF-404 | Agente ejecutor: implementación de cambios según plan aprobado | Must | MVP |
| RF-405 | Agente de testing: ejecución y validación de tests | Should | Beta |
| RF-406 | Agente de review: revisión de código con estándares configurables | Should | Beta |
| RF-407 | Ejecución paralela de agentes independientes | Must | Beta |
| RF-408 | Agentes en background con notificación al completar | Should | Beta |
| RF-409 | Sistema de task management visible (todo list de agentes) | Must | MVP |
| RF-410 | Capacidad de interrumpir/cancelar agentes en ejecución | Must | MVP |
| RF-411 | Integración con servicios Cuervo como agentes especializados (auth, prompt, video-intel) | Should | GA |

### 1.6 Sistema de Plugins

| ID | Requisito | Prioridad | Fase |
|----|-----------|-----------|------|
| RF-501 | Arquitectura de plugins extensible con API documentada | Must | Beta |
| RF-502 | Plugin registry (local + remoto) para descubrir y descargar plugins | Should | GA |
| RF-503 | Plugin SDK con templates y tooling de desarrollo | Should | GA |
| RF-504 | Plugins de comunidad: contribuciones open source | Should | GA |
| RF-505 | Hooks system: ejecutar scripts pre/post operaciones del CLI | Must | MVP |
| RF-506 | Custom slash commands definidos por el usuario | Must | Beta |
| RF-507 | Integración de plugins con el sistema de permisos (sandboxing) | Must | Beta |

### 1.7 Gestión de Configuración y Proyectos

| ID | Requisito | Prioridad | Fase |
|----|-----------|-----------|------|
| RF-601 | Archivo de configuración por proyecto (`.cuervo/config.yml`) | Must | MVP |
| RF-602 | Configuración global del usuario (`~/.cuervo/config.yml`) | Must | MVP |
| RF-603 | Archivo de memoria persistente por proyecto (context files tipo `CUERVO.md`) | Must | MVP |
| RF-604 | Variables de entorno para configuración sensible | Must | MVP |
| RF-605 | Perfiles de configuración (dev, staging, prod) | Should | Beta |
| RF-606 | Importación/exportación de configuración entre proyectos | Could | GA |
| RF-607 | Configuración de permisos granulares (qué herramientas puede usar la IA) | Must | MVP |

### 1.8 Pipelines de Training y Fine-tuning

| ID | Requisito | Prioridad | Fase |
|----|-----------|-----------|------|
| RF-701 | CLI para iniciar jobs de fine-tuning con datos del proyecto | Could | GA |
| RF-702 | Preparación automática de datasets de training desde codebase | Could | GA |
| RF-703 | Monitoreo de jobs de training en curso (métricas, loss, etc.) | Could | GA |
| RF-704 | Evaluación automática de modelos fine-tuned vs baseline | Could | GA |
| RF-705 | Deploy de modelos custom a Ollama local o endpoints cloud | Could | GA |
| RF-706 | Versionado de modelos con rollback capability | Could | GA |

### 1.9 Autenticación y Multi-tenancy

| ID | Requisito | Prioridad | Fase |
|----|-----------|-----------|------|
| RF-801 | Autenticación con cuervo-auth-service (JWT) | Must | MVP |
| RF-802 | Login via CLI (username/password, OAuth2 flow) | Must | MVP |
| RF-803 | API key management para proveedores de modelo | Must | MVP |
| RF-804 | Soporte para organizaciones/equipos con permisos compartidos | Should | Beta |
| RF-805 | Single Sign-On (SSO) via SAML/OIDC | Could | GA |
| RF-806 | Gestión de secrets segura (keychain integration) | Must | MVP |

---

## 2. Requisitos No Funcionales (RNF)

### 2.1 Rendimiento

| ID | Requisito | Target | Fase |
|----|-----------|--------|------|
| RNF-001 | Time-to-first-token para autocompletado (modelo local) | <200ms p50 (<100ms con Rust scanner) | MVP |
| RNF-002 | Time-to-first-token para chat (modelo cloud) | <1s p50 | MVP |
| RNF-003 | Latencia de búsqueda en codebase indexado | <100ms p50 (<20ms con LanceDB Rust) | Beta |
| RNF-004 | Startup time del CLI | <500ms | MVP |
| RNF-005 | Memoria máxima en idle | <100MB RSS | MVP |
| RNF-006 | Memoria máxima durante operación activa | <500MB RSS | MVP |
| RNF-007 | Soporte para codebases de hasta 100K archivos | Funcional (<5s full scan con Rust scanner) | Beta |
| RNF-008 | Soporte para archivos individuales de hasta 10MB | Funcional | MVP |
| RNF-009 | Throughput de procesamiento de tokens | >50 tokens/s display | MVP |
| RNF-010 | Token counting pre-request (estimación de contexto) | <1ms/request con tokenizer.rs nativo | Beta |

### 2.2 Escalabilidad

| ID | Requisito | Target | Fase |
|----|-----------|--------|------|
| RNF-101 | Usuarios concurrentes (modo cloud/SaaS) | 100K+ | GA |
| RNF-102 | Requests por segundo (API backend) | 20K+ | GA |
| RNF-103 | Horizontal scaling sin downtime | Kubernetes HPA | Beta |
| RNF-104 | Soporte multi-región | 3+ regiones | GA |
| RNF-105 | Auto-scaling basado en carga | Configurado | Beta |

### 2.3 Disponibilidad y Resiliencia

| ID | Requisito | Target | Fase |
|----|-----------|--------|------|
| RNF-201 | Uptime SLA (modo cloud) | 99.9% | GA |
| RNF-202 | Modo offline funcional (con modelo local) | Sin degradación | MVP |
| RNF-203 | Graceful degradation si proveedor cloud no disponible | Fallback a local | Beta |
| RNF-204 | Recovery time objective (RTO) | <15 min | GA |
| RNF-205 | Recovery point objective (RPO) | <1 hora | GA |
| RNF-206 | Health checks automatizados | Cada 30s | Beta |
| RNF-207 | Circuit breaker para APIs externas | Implementado | Beta |

### 2.4 Seguridad

| ID | Requisito | Target | Fase |
|----|-----------|--------|------|
| RNF-301 | Encriptación en tránsito | TLS 1.3 | MVP |
| RNF-302 | Encriptación at rest para datos sensibles | AES-256 | MVP |
| RNF-303 | Ejecución sandboxed de código generado por IA | Por defecto | MVP |
| RNF-304 | Confirmación explícita antes de operaciones destructivas | Por defecto | MVP |
| RNF-305 | Rate limiting en APIs | Configurado | MVP |
| RNF-306 | Protección contra prompt injection | Multicapa | MVP |
| RNF-307 | PII detection y redaction en prompts | Configurable | Beta |
| RNF-308 | Audit logging de todas las operaciones de IA | Completo | Beta |
| RNF-309 | Vulnerability scanning automatizado (deps) | CI/CD integrado | Beta |
| RNF-310 | Penetration testing periódico | Trimestral | GA |
| RNF-311 | Zero-retention mode para enterprise | Configurable | GA |

### 2.5 Compatibilidad

| ID | Requisito | Target | Fase |
|----|-----------|--------|------|
| RNF-401 | Sistemas operativos soportados | macOS 13+, Ubuntu 22.04+, Windows 11+ (WSL2) | MVP |
| RNF-402 | Shells soportados | bash, zsh, fish, PowerShell | MVP |
| RNF-403 | Node.js runtime | 20 LTS+ | MVP |
| RNF-404 | Lenguajes de programación soportados (code intelligence) | 15+ mainstream | MVP |
| RNF-405 | Arquitecturas de CPU | x86_64, ARM64 (Apple Silicon) | MVP |
| RNF-406 | Soporte para proxy/VPN corporativo | Configurable | Beta |

### 2.6 Usabilidad

| ID | Requisito | Target | Fase |
|----|-----------|-----------|------|
| RNF-501 | Tiempo de instalación desde cero | <2 min | MVP |
| RNF-502 | Tiempo de onboarding (primera tarea exitosa) | <5 min | MVP |
| RNF-503 | Documentación completa en español e inglés | 100% | MVP |
| RNF-504 | Mensajes de error claros y accionables | 100% | MVP |
| RNF-505 | Help system integrado (`--help`, `/help`) | Completo | MVP |
| RNF-506 | Soporte accesibilidad (screen readers, alto contraste) | Básico | Beta |

### 2.7 Mantenibilidad

| ID | Requisito | Target | Fase |
|----|-----------|--------|------|
| RNF-601 | Cobertura de tests unitarios | >80% | MVP |
| RNF-602 | Cobertura de tests de integración | >60% | Beta |
| RNF-603 | CI/CD pipeline completo | GitHub Actions | MVP |
| RNF-604 | Release automatizado (semantic versioning) | Configurado | MVP |
| RNF-605 | Documentación de API auto-generada | OpenAPI 3.0 | Beta |
| RNF-606 | Linting y formatting automatizado | ESLint + Prettier | MVP |
| RNF-607 | Architectural Decision Records (ADRs) | Mantenido | MVP |

### 2.8 Observabilidad

| ID | Requisito | Target | Fase |
|----|-----------|--------|------|
| RNF-701 | Métricas de uso (tokens, latencia, modelo, costo) | Prometheus | Beta |
| RNF-702 | Logging estructurado (JSON) | Winston/Pino | MVP |
| RNF-703 | Tracing distribuido | OpenTelemetry | Beta |
| RNF-704 | Dashboards de monitoreo | Grafana | Beta |
| RNF-705 | Alerting automatizado | PagerDuty/OpsGenie | GA |
| RNF-706 | Error tracking | Sentry | MVP |

### 2.9 Compliance

| ID | Requisito | Target | Fase |
|----|-----------|--------|------|
| RNF-801 | GDPR compliance | Diseño | MVP |
| RNF-802 | EU AI Act (riesgo limitado) | Transparencia | MVP |
| RNF-803 | LGPD (Brasil) compliance | Diseño | Beta |
| RNF-804 | SOC 2 Type I readiness | Controles | Beta |
| RNF-805 | SOC 2 Type II certification | Certificado | GA |
| RNF-806 | ISO 27001 alignment | Self-assessment | GA |
| RNF-807 | ISO 42001 alignment | Self-assessment | GA |
| RNF-808 | OWASP LLM Top 10 mitigations | 10/10 | Beta |

---

## 3. Matriz de Priorización (MoSCoW por Fase)

### MVP (Must Have = 47 requisitos)

```
CORE CLI:          RF-001, 002, 004, 005, 006, 007, 010
MODELOS:           RF-101, 102, 105, 107
CÓDIGO:            RF-201, 202, 203, 205, 206, 207, 208
GIT/TOOLS:         RF-301, 302, 304
AGENTES:           RF-402, 403, 404, 409, 410
PLUGINS:           RF-505
CONFIG:            RF-601, 602, 603, 604, 607
AUTH:              RF-801, 802, 803, 806
RENDIMIENTO:       RNF-001 a 009
SEGURIDAD:         RNF-301 a 306
COMPATIBILIDAD:    RNF-401 a 405
USABILIDAD:        RNF-501 a 505
MANTENIBILIDAD:    RNF-601, 603, 604, 606, 607
OBSERVABILIDAD:    RNF-702, 706
COMPLIANCE:        RNF-801, 802
```

### Beta (Should Have = 36 requisitos)

```
CLI:               RF-003
MODELOS:           RF-103, 104, 106, 108, 109, 112
CÓDIGO:            RF-204, 209, 210, 211, 213, 214
GIT/TOOLS:         RF-303, 305, 306, 309
AGENTES:           RF-401, 405, 406, 407, 408
PLUGINS:           RF-501, 506, 507
CONFIG:            RF-605
AUTH:              RF-804
RENDIMIENTO:       RNF-101 a 105
SEGURIDAD:         RNF-307 a 309, 311
OBSERVABILIDAD:    RNF-701, 703, 704
COMPLIANCE:        RNF-803, 804, 808
```

### GA (Could Have = 24 requisitos)

```
CLI:               RF-008, 009
MODELOS:           RF-110, 111
CÓDIGO:            RF-212
GIT/TOOLS:         RF-307, 308, 310
PLUGINS:           RF-502, 503, 504
CONFIG:            RF-606
TRAINING:          RF-701 a 706
AUTH:              RF-805
SEGURIDAD:         RNF-310
OBSERVABILIDAD:    RNF-705
COMPLIANCE:        RNF-805, 806, 807
```

---

## 4. Trazabilidad con Propuesta de Valor

| Diferenciador | Requisitos Clave |
|---------------|-----------------|
| **Multi-modelo** | RF-101, 102, 103, 104, 106, 107, 112 |
| **Self-hosted** | RF-102, 107, RNF-202, 203, 311 |
| **Multi-agente** | RF-401 a 411 |
| **Open source core** | RF-501 a 507, RNF-601 a 607 |
| **LATAM-first** | RF-001 (español), RNF-503, RNF-803 |
| **Fine-tuning integrado** | RF-110, 701 a 706 |
| **Compliance by design** | RNF-301 a 311, RNF-801 a 808 |
| **Ecosistema Cuervo** | RF-309, 310, 411, 801 |

---

*Documento sujeto a revisión conforme avance el diseño arquitectónico.*
