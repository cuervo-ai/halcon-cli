# Auditoría, Logging y Explicabilidad

**Proyecto:** Cuervo CLI
**Versión:** 1.0
**Fecha:** 6 de febrero de 2026

---

## Resumen Ejecutivo

El sistema de auditoría de Cuervo CLI provee trazabilidad completa de todas las operaciones de IA, cumpliendo con requisitos de EU AI Act, SOC 2, ISO 27001, y NIST AI RMF. El diseño equilibra la necesidad de accountability con la privacidad del usuario mediante logging selectivo y PII redaction.

---

## 1. Arquitectura de Logging

```
┌──────────────────────────────────────────────────────────────┐
│                   LOGGING ARCHITECTURE                        │
├──────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌──────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │ Cuervo   │────▶│ Structured   │────▶│ Log Router   │    │
│  │ CLI Core │     │ Logger       │     │              │    │
│  └──────────┘     │ (JSON)       │     └──────┬───────┘    │
│                    └──────────────┘            │             │
│                                         ┌─────┼──────┐      │
│                                         │     │      │      │
│                                    ┌────▼──┐ ┌▼────┐ ┌▼───┐│
│                                    │Local  │ │Cloud│ │SIEM││
│                                    │File   │ │(opt)│ │(ent)││
│                                    └───────┘ └─────┘ └─────┘│
│                                                               │
│  LOG LEVELS:                                                  │
│  ┌────────┬──────────────────────────────────────────────┐  │
│  │ DEBUG  │ Detalle completo (solo desarrollo local)      │  │
│  │ INFO   │ Operaciones normales, decisiones de IA        │  │
│  │ WARN   │ Situaciones atípicas, fallbacks activados     │  │
│  │ ERROR  │ Fallos recuperables                           │  │
│  │ FATAL  │ Fallos irrecuperables                         │  │
│  │ AUDIT  │ Eventos de compliance (siempre registrados)   │  │
│  └────────┴──────────────────────────────────────────────┘  │
│                                                               │
└──────────────────────────────────────────────────────────────┘
```

---

## 2. Eventos de Auditoría

### 2.1 Catálogo de Eventos

| Categoría | Evento | Nivel | Datos Registrados |
|-----------|--------|-------|-------------------|
| **AUTH** | user.login | AUDIT | user_id, method, IP, timestamp |
| **AUTH** | user.logout | AUDIT | user_id, session_duration |
| **AUTH** | auth.failed | AUDIT | attempted_user, reason, IP |
| **AI** | model.invoked | AUDIT | model_id, version, tokens_in, tokens_out, latency, cost |
| **AI** | model.fallback | WARN | primary_model, fallback_model, reason |
| **AI** | model.error | ERROR | model_id, error_type, error_message |
| **AI** | agent.started | INFO | agent_type, task_description |
| **AI** | agent.completed | INFO | agent_type, duration, actions_taken |
| **AI** | agent.failed | ERROR | agent_type, error, partial_results |
| **TOOL** | file.read | INFO | file_path (no content), timestamp |
| **TOOL** | file.write | AUDIT | file_path, lines_changed, diff_summary |
| **TOOL** | file.delete | AUDIT | file_path, user_confirmed |
| **TOOL** | bash.execute | AUDIT | command (sanitized), exit_code, user_confirmed |
| **TOOL** | git.commit | AUDIT | commit_hash, message, files_changed |
| **TOOL** | git.push | AUDIT | remote, branch, commit_range |
| **PRIVACY** | pii.detected | WARN | pii_type, action_taken (redacted/blocked) |
| **PRIVACY** | data.exported | AUDIT | user_id, format, data_categories |
| **PRIVACY** | data.deleted | AUDIT | user_id, data_categories, method |
| **CONFIG** | config.changed | AUDIT | setting_name, old_value_hash, new_value_hash |
| **CONFIG** | plugin.installed | AUDIT | plugin_name, version, source |
| **SECURITY** | injection.detected | WARN | injection_type, action_taken |
| **SECURITY** | rate_limit.hit | WARN | user_id, endpoint, current_rate |

### 2.2 Formato de Log Entry

```json
{
  "timestamp": "2026-02-06T16:30:00.000Z",
  "level": "AUDIT",
  "category": "AI",
  "event": "model.invoked",
  "session_id": "sess_abc123",
  "user_id": "usr_xyz789",
  "data": {
    "model_id": "claude-sonnet-4-5",
    "model_version": "20250929",
    "provider": "anthropic",
    "tokens_input": 15234,
    "tokens_output": 3456,
    "latency_ms": 2340,
    "estimated_cost_usd": 0.097,
    "task_type": "code_generation",
    "tools_used": ["file_read", "file_edit", "glob"],
    "agent_type": "executor"
  },
  "context": {
    "project_hash": "a1b2c3d4",
    "cli_version": "0.1.0",
    "os": "darwin",
    "arch": "arm64"
  }
}
```

---

## 3. Explicabilidad

### 3.1 Niveles de Explicabilidad

```
┌─────────────────────────────────────────────────────────────┐
│              NIVELES DE EXPLICABILIDAD                        │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  NIVEL 1: WHAT (Default — siempre visible)                  │
│  "Modifiqué 3 archivos para agregar la validación de input" │
│                                                              │
│  NIVEL 2: WHY (Disponible bajo demanda)                     │
│  "Elegí Zod para validación porque el proyecto ya lo usa    │
│   en src/validators/ y es consistente con el patrón         │
│   existente en UserValidator.ts"                            │
│                                                              │
│  NIVEL 3: HOW (Verbose / Debug mode)                        │
│  "1. Busqué archivos de validación → encontré 3 usando Zod │
│   2. Analicé el patrón: schema → parse → handle errors      │
│   3. Generé schema para InputDto siguiendo mismo patrón     │
│   4. Agregué tests basados en los tests existentes          │
│   5. Modelo usado: Claude Sonnet 4.5, confianza: 0.92"     │
│                                                              │
│  NIVEL 4: TRACE (Audit / Compliance)                        │
│  Chain completa de decisiones del modelo, herramientas       │
│  invocadas, tokens consumidos, latencias, costos             │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 Mecanismos de Explicabilidad

| Mecanismo | Descripción | Disponibilidad |
|-----------|-------------|----------------|
| **Resumen de acciones** | Qué hizo el agente y por qué | Siempre (post-acción) |
| **Plan preview** | Plan detallado antes de ejecutar | Siempre (pre-acción) |
| **Diff view** | Cambios exactos con contexto | Siempre (por archivo) |
| **Reasoning trace** | Chain-of-thought del modelo | Bajo demanda (`--verbose`) |
| **Decision log** | Por qué eligió modelo X, tool Y | Bajo demanda (`--debug`) |
| **Confidence score** | Nivel de confianza en la respuesta | Configurable |
| **Source attribution** | "Basado en patrón encontrado en X.ts" | Cuando aplica |
| **Limitation disclosure** | "No estoy seguro de X, verifica Y" | Automático (baja confianza) |

### 3.3 Implementación de Confidence Scoring

```
Confidence Score = weighted_average(
  model_confidence  × 0.3,   // Logprobs del modelo
  context_quality   × 0.3,   // Calidad del contexto RAG
  pattern_match     × 0.2,   // Consistencia con patrones del proyecto
  test_validation   × 0.2    // Tests pasan después del cambio
)

Score > 0.8  → "Alta confianza" (proceed)
Score 0.5-0.8 → "Confianza media" (suggest review)
Score < 0.5  → "Baja confianza" (warn + recommend manual)
```

---

## 4. Retención y Lifecycle de Logs

### 4.1 Política de Retención

| Tipo de Log | Retención Local | Retención Cloud (Enterprise) |
|-------------|----------------|------------------------------|
| Session logs (conversaciones) | Session only (default) o 30 días (opt-in) | 90 días |
| Audit events | 1 año | 3 años (SOC 2) |
| Error logs | 90 días | 1 año |
| Security events | 1 año | 5 años |
| Telemetría de uso | 90 días (opt-in) | 1 año |
| GDPR/LGPD compliance events | 5 años | 7 años |

### 4.2 Protección de Integridad

```
LOG INTEGRITY CHAIN:

Entry₁ → hash(Entry₁)
Entry₂ → hash(Entry₂ + hash(Entry₁))
Entry₃ → hash(Entry₃ + hash(Entry₂))
...
EntryN → hash(EntryN + hash(EntryN₋₁))

→ Cualquier modificación de un entry invalida la cadena
→ Verificable con: `cuervo audit verify --range 2026-01-01:2026-02-06`
```

---

## 5. Compliance Mapping

### 5.1 Requisitos por Framework

| Framework | Requisito | Implementación en Cuervo CLI |
|-----------|-----------|------------------------------|
| **EU AI Act** | Logging automático de operaciones AI | Evento model.invoked + agent.* |
| **EU AI Act** | Trazabilidad de decisiones | Reasoning trace + decision log |
| **GDPR Art. 30** | Records de procesamiento | Privacy events + data flow docs |
| **SOC 2 CC7** | Monitoreo de sistema | Structured logging + alerting |
| **SOC 2 CC8** | Change management | config.changed + plugin.installed |
| **ISO 27001 A.12.4** | Event logging | Catálogo completo de eventos |
| **ISO 27001 A.12.4** | Protección de logs | Hash chain + access controls |
| **NIST AI RMF (MEASURE)** | Métricas de AI performance | model.invoked con métricas |
| **NIST AI RMF (MANAGE)** | Incident tracking | Security + error events |
| **OWASP LLM** | Logging de interacciones | Todos los eventos AI + TOOL |

---

## 6. Herramientas CLI de Auditoría

```bash
# Ver audit log reciente
cuervo audit log --last 24h

# Filtrar por categoría
cuervo audit log --category AI --level AUDIT

# Exportar para compliance
cuervo audit export --format json --range 2026-01-01:2026-02-06

# Verificar integridad de logs
cuervo audit verify --range 2026-01-01:2026-02-06

# Resumen de costos y uso
cuervo audit summary --period month

# Reporte de compliance
cuervo audit compliance-report --framework gdpr
cuervo audit compliance-report --framework soc2
```

---

*Documento sujeto a revisión por equipo de seguridad y compliance.*
