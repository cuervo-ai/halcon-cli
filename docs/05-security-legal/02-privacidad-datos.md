# Privacidad de Datos

**Proyecto:** Cuervo CLI
**Versión:** 1.0
**Fecha:** 6 de febrero de 2026

---

## Resumen Ejecutivo

La estrategia de privacidad de Cuervo CLI se basa en el principio de **Privacy by Design and Default**, cumpliendo con GDPR, LGPD (Brasil), y preparándose para regulaciones emergentes en LATAM. El diseño offline-first garantiza que la privacidad máxima es la configuración por defecto, no una opción premium.

---

## 1. Clasificación de Datos

### 1.1 Taxonomía de Datos Procesados

| Categoría | Ejemplos | Sensibilidad | Retención Default |
|-----------|----------|-------------|-------------------|
| **Código fuente** | Archivos .ts, .py, .rs del proyecto | Alta | Zero-retention (no se almacena en cloud) |
| **Prompts del usuario** | Instrucciones en lenguaje natural | Media | Session-only (se borra al cerrar) |
| **Responses del modelo** | Código generado, explicaciones | Media | Session-only |
| **Metadatos de sesión** | Timestamp, duración, modelo usado | Baja | 90 días (analytics) |
| **Configuración** | API keys, preferences | Alta | Local encrypted |
| **Embeddings** | Vectores de representación del codebase | Media | Local only |
| **Telemetría** | Métricas de uso anónimas | Baja | 1 año (opt-in only) |

### 1.2 Datos que NUNCA se Recopilan

```
PROHIBIDO — Nunca se envía a cloud ni se almacena:
├── Archivos .env o de configuración de secrets
├── Credenciales, tokens, API keys del usuario
├── Contenido de archivos marcados en .cuervoignore
├── Datos personales identificables (PII) detectados en código
├── Historial de bash/terminal del sistema
├── Contenido del clipboard del usuario
└── Archivos fuera del directorio del proyecto
```

---

## 2. Flujos de Datos y Protecciones

### 2.1 Modo Offline (Máxima Privacidad)

```
┌──────────────────────────────────────┐
│         MODO OFFLINE                  │
│                                       │
│  ┌─────────┐     ┌──────────────┐   │
│  │ Código  │────▶│ Ollama       │   │
│  │ del     │     │ (Local LLM)  │   │  ← Todo queda en la máquina
│  │ usuario │◀────│              │   │  ← Zero data transmission
│  └─────────┘     └──────────────┘   │  ← No telemetry
│                                       │
│  Datos que salen de la máquina: NADA │
│                                       │
└──────────────────────────────────────┘
```

### 2.2 Modo Híbrido (Default Recomendado)

```
┌────────────────────────────────────────────────────────────┐
│                    MODO HÍBRIDO                             │
│                                                             │
│  LOCAL                          CLOUD                       │
│  ┌──────────┐                   ┌──────────────┐           │
│  │ Código   │                   │ Model API    │           │
│  │ original │                   │ (Anthropic/  │           │
│  └────┬─────┘                   │  OpenAI/etc) │           │
│       │                         └──────▲───────┘           │
│       ▼                                │                    │
│  ┌──────────┐                          │                    │
│  │ PII      │   Código sanitizado      │                    │
│  │ Filter   │──────────────────────────┘                    │
│  │ + Redact │                                               │
│  └──────────┘   Solo contexto necesario                     │
│       │         (no el codebase completo)                   │
│       ▼                                                     │
│  ┌──────────┐                                               │
│  │ Response │   Response procesada localmente               │
│  │ Handler  │   Logs sanitizados (sin PII)                  │
│  └──────────┘                                               │
│                                                             │
│  GARANTÍAS:                                                 │
│  • Solo fragmentos necesarios enviados a cloud              │
│  • PII detectado y redactado antes de envío                 │
│  • Zero-retention en provider (configurable)                │
│  • TLS 1.3 en tránsito                                     │
│  • No se usa para training del provider                     │
│                                                             │
└────────────────────────────────────────────────────────────┘
```

### 2.3 Modo Enterprise (Zero-Retention Cloud)

```
ENTERPRISE MODE:
├── Zero-retention: prompts/responses no almacenados por provider
├── Data Processing Agreement con cada provider
├── Audit log completo pero PII-free
├── Opción self-hosted: código nunca sale del perímetro corp.
├── Encryption at rest (AES-256) para datos locales
└── Key management integrado con vault corporativo
```

---

## 3. Cumplimiento GDPR

### 3.1 Base Legal para Procesamiento

| Actividad | Base Legal (Art. 6) | Justificación |
|-----------|---------------------|---------------|
| Envío de código a model API | Ejecución de contrato (6.1.b) | Necesario para proveer el servicio solicitado |
| Almacenamiento de conversaciones | Consentimiento (6.1.a) | Opt-in para persistencia |
| Telemetría de uso | Interés legítimo (6.1.f) | Mejora del servicio, con opt-out |
| Fine-tuning con datos del usuario | Consentimiento explícito (6.1.a) | Siempre opt-in, nunca por defecto |

### 3.2 Derechos del Interesado (GDPR)

| Derecho | Implementación en Cuervo CLI |
|---------|------------------------------|
| **Acceso (Art. 15)** | `cuervo privacy export` — exporta todos los datos del usuario |
| **Rectificación (Art. 16)** | `cuervo privacy update` — modificar datos de perfil |
| **Erasure (Art. 17)** | `cuervo privacy delete` — eliminación completa de datos |
| **Portabilidad (Art. 20)** | `cuervo privacy export --format json` — export en formato estándar |
| **Objeción (Art. 21)** | `cuervo privacy opt-out telemetry` — detener telemetría |
| **No decisión automatizada (Art. 22)** | N/A — Cuervo CLI siempre requiere aprobación humana |

### 3.3 Data Protection Impact Assessment (DPIA)

```
DPIA — RESUMEN:

Actividad: Procesamiento de código fuente para asistencia de desarrollo
Necesidad: Proporcionar sugerencias contextuales de código
Riesgos identificados:
  1. PII en código enviado a cloud → MITIGADO: PII filter pre-envío
  2. Código propietario expuesto → MITIGADO: Zero-retention + encryption
  3. Re-identificación por metadatos → MITIGADO: Anonimización de telemetría
  4. Transfer cross-border → MITIGADO: EU-US DPF + SCCs
  5. Retención excesiva → MITIGADO: Session-only default + auto-delete

Conclusión: Riesgo residual BAJO con mitigaciones implementadas.
DPO review: [Pendiente de asignación de DPO]
```

---

## 4. Cumplimiento LGPD (Brasil)

| Requisito LGPD | Equivalente GDPR | Estado en Cuervo CLI |
|----------------|-------------------|---------------------|
| Base legal (Art. 7) | Art. 6 | Consentimiento + ejecución de contrato |
| Principio de necesidad (Art. 6.III) | Minimización | Data minimization implementado |
| Derechos del titular (Art. 18) | Arts. 15-22 | CLI commands para ejercer derechos |
| Transferencia internacional (Art. 33) | Arts. 44-49 | Cláusulas contractuales estándar |
| Reporte de incidentes (Art. 48) | Art. 33-34 | Proceso de notificación definido |
| DPO/Encarregado (Art. 41) | Art. 37-39 | Designación requerida |

---

## 5. Implementación Técnica de Privacidad

### 5.1 PII Detection Pipeline

```
Input (código del usuario)
       │
       ▼
┌──────────────────┐
│ Regex Patterns   │ ← Emails, IPs, phones, SSNs, credit cards
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ NER (Named       │ ← Nombres de personas, organizaciones
│ Entity Recog.)   │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ Secret Scanner   │ ← API keys, tokens, passwords, certificates
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ Redaction Engine │ ← Reemplaza PII con placeholders: [EMAIL], [NAME]
└────────┬─────────┘
         │
         ▼
Output (código sanitizado) → Enviado a model API
```

### 5.2 Archivo .cuervoignore

```gitignore
# .cuervoignore — Archivos que Cuervo CLI nunca leerá ni enviará
.env
.env.*
*.pem
*.key
*.cert
credentials.json
secrets.yaml
**/secrets/**
**/credentials/**
*.sqlite
*.db
```

### 5.3 Encryption

| Dato | At Rest | In Transit |
|------|---------|-----------|
| API keys del usuario | AES-256 (OS keychain) | N/A (local) |
| Conversaciones guardadas | AES-256 (SQLite encrypted) | N/A (local) |
| Embeddings del codebase | Sin encriptar (local only) | N/A (local) |
| Código enviado a cloud | N/A | TLS 1.3 |
| Telemetría | N/A | TLS 1.3 |

---

*Documento sujeto a revisión legal y aprobación del DPO.*
