# Landscape de Seguridad, Compliance y Marco Regulatorio 2026

**Proyecto:** Cuervo CLI — Plataforma de IA Generativa para Desarrollo de Software
**Versión:** 1.0
**Fecha:** 6 de febrero de 2026
**Autor:** Equipo de Arquitectura Cuervo
**Clasificación:** Confidencial — Uso Interno

---

## Resumen Ejecutivo

El marco regulatorio para productos de IA ha madurado significativamente en 2025-2026. La EU AI Act entra en vigencia completa para sistemas de alto riesgo en agosto 2026, el Colorado AI Act rige desde febrero 2026, y la convergencia de estándares ISO/IEC con frameworks NIST establece un baseline claro para productos enterprise.

Para Cuervo CLI, **compliance no es un costo sino un diferenciador competitivo**. Mientras los competidores adaptan retroactivamente sus sistemas, Cuervo tiene la oportunidad de diseñar **compliance-by-design** desde la arquitectura.

### Conclusiones Clave

1. **EU AI Act**: Cuervo CLI como herramienta de coding probablemente clasifica como **riesgo limitado** (obligaciones de transparencia), pero componentes enterprise podrían tocar **alto riesgo** si se usan en decisiones de empleo.
2. **GDPR**: Requiere DPIAs, acuerdos de procesamiento de datos, mecanismos de transferencia cross-border, y derechos de erasure implementados.
3. **SOC 2 Type II**: Obligatorio para ventas enterprise — incluir controles AI-específicos emergentes.
4. **ISO 42001**: Primera certificación de sistema de gestión de IA — diferenciador competitivo temprano.
5. **OWASP LLM Top 10**: Baseline de seguridad para todas las aplicaciones basadas en LLM.

---

## 1. EU AI Act — Estado de Enforcement y Requisitos

### 1.1 Timeline de Implementación

```
Ago 2024 ─── Entrada en vigor
  │
Feb 2025 ─── Prohibiciones de IA inaceptable (activo ✓)
  │           • Social scoring
  │           • Vigilancia biométrica en tiempo real
  │           • Reconocimiento de emociones en trabajo/escuelas
  │           • IA manipulativa
  │
Ago 2025 ─── Requisitos GPAI (activo ✓)
  │           • Documentación técnica obligatoria
  │           • Cumplimiento de copyright EU
  │           • AI Office operativa
  │
Ago 2026 ─── Enforcement completo alto riesgo (PRÓXIMO)
  │           • Conformity assessments
  │           • Risk management systems
  │           • Data governance
  │           • Transparencia y oversight humano
  │           • Registro en EU database
  │
2027+ ────── Revisión y actualización
```

### 1.2 Clasificación de Riesgo para Cuervo CLI

| Componente | Clasificación Probable | Obligaciones |
|------------|----------------------|--------------|
| CLI de coding assistido | **Riesgo Limitado** | Transparencia: informar al usuario que interactúa con IA |
| Generación de código | **Riesgo Limitado** | Etiquetado de contenido generado por IA |
| Análisis de código automatizado | **Riesgo Limitado** | Transparencia en el proceso de análisis |
| Uso en decisiones de empleo | **Alto Riesgo (Annex III)** | Conformity assessment completo si se usa para evaluar candidatos |
| Modelo GPAI subyacente | **Obligaciones GPAI** | Si Cuervo entrena/distribuye su propio modelo: documentación técnica, copyright |

### 1.3 Acciones Requeridas

**Inmediatas (antes de lanzamiento):**
- [ ] Implementar aviso transparente de interacción con IA
- [ ] Documentar técnicamente todos los modelos utilizados
- [ ] Implementar etiquetado de contenido generado por IA
- [ ] Establecer mecanismo de opt-out de copyright

**Antes de agosto 2026:**
- [ ] Clasificación formal de riesgo de cada componente
- [ ] Conformity assessment si algún componente califica como alto riesgo
- [ ] Sistema de gestión de riesgos documentado
- [ ] Registro en EU database si aplica

---

## 2. GDPR — Implicaciones para Productos de IA

### 2.1 Principios Aplicables

| Principio GDPR | Aplicación en Cuervo CLI |
|----------------|--------------------------|
| **Base legal** | Consentimiento o interés legítimo para procesamiento de código con datos personales |
| **Limitación de propósito** | Código enviado para asistencia NO puede reutilizarse para training sin base legal adicional |
| **Minimización** | Solo procesar datos estrictamente necesarios para la funcionalidad solicitada |
| **DPIA** | Requerido para procesamiento a escala de datos personales |
| **Derecho de erasure** | Mecanismo para eliminar datos del usuario de logs y almacenamiento |
| **Art. 22** | Si IA toma decisiones con efecto legal → derecho a revisión humana |

### 2.2 Consideraciones Específicas para AI Coding Assistants

```
┌──────────────────────────────────────────────────────┐
│              FLUJO DE DATOS DE CÓDIGO                 │
├──────────────────────────────────────────────────────┤
│                                                      │
│  Código del usuario                                  │
│       │                                              │
│       ▼                                              │
│  ┌──────────────┐     ¿Contiene PII?                │
│  │ Pre-proceso  │──── • Variables con nombres reales │
│  │ (sanitize)   │     • Comentarios con datos pers.  │
│  └──────┬───────┘     • Credenciales hardcoded       │
│         │             • Emails, IPs, tokens          │
│         ▼                                            │
│  ┌──────────────┐                                    │
│  │  PII Filter  │──── Detección + redacción          │
│  └──────┬───────┘                                    │
│         │                                            │
│         ▼                                            │
│  ┌──────────────┐     Transferencia cross-border     │
│  │ API Provider │──── EU-US Data Privacy Framework   │
│  │ (Cloud)      │     Standard Contractual Clauses   │
│  └──────┬───────┘                                    │
│         │                                            │
│         ▼                                            │
│  ┌──────────────┐                                    │
│  │ Respuesta    │──── Zero retention mode (enterprise)│
│  │ (filtrada)   │     Audit log (con controles)      │
│  └──────────────┘                                    │
│                                                      │
└──────────────────────────────────────────────────────┘
```

### 2.3 Mecanismos de Cumplimiento

1. **Data Processing Agreements (DPAs)**: Contratos con cada proveedor de modelo (Anthropic, OpenAI, Google)
2. **Transfer mechanisms**: EU-US Data Privacy Framework + SCCs como fallback
3. **Zero-retention modes**: Enterprise tier sin almacenamiento de prompts/responses
4. **PII detection**: Pipeline automático de detección y redacción de datos personales
5. **Consent management**: Opt-in explícito para telemetría y mejora de servicio
6. **Data portability**: Exportación de todos los datos del usuario en formato estándar
7. **Right to erasure**: API y CLI para eliminación completa de datos

---

## 3. NIST AI Risk Management Framework

### 3.1 Funciones Core Aplicadas a Cuervo CLI

```
┌─────────────────────────────────────────────────────────────┐
│                    NIST AI RMF — CUERVO CLI                  │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌──────────┐                                                │
│  │ GOVERN   │  Políticas, roles, accountability              │
│  │          │  • AI Ethics Board interno                     │
│  │          │  • Responsible AI Policy documentada           │
│  │          │  • Roles: AI Safety Lead, Data Protection Ofc. │
│  └──────────┘                                                │
│                                                              │
│  ┌──────────┐                                                │
│  │   MAP    │  Contextualizar riesgos por caso de uso        │
│  │          │  • Risk taxonomy para coding assistants        │
│  │          │  • Mapping de impacto por feature              │
│  │          │  • Stakeholder analysis                        │
│  └──────────┘                                                │
│                                                              │
│  ┌──────────┐                                                │
│  │ MEASURE  │  Análisis cuantitativo/cualitativo             │
│  │          │  • Hallucination rate tracking                 │
│  │          │  • Bias testing pipeline                       │
│  │          │  • Security vulnerability scanning             │
│  │          │  • SWE-bench regression testing                │
│  └──────────┘                                                │
│                                                              │
│  ┌──────────┐                                                │
│  │ MANAGE   │  Priorizar y actuar sobre riesgos             │
│  │          │  • Incident response plan para AI failures     │
│  │          │  • Rollback mechanisms para modelos            │
│  │          │  • Kill switch para features problemáticas     │
│  │          │  • Post-market monitoring continuo             │
│  └──────────┘                                                │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 NIST AI 600-1 — Riesgos Específicos de GenAI

| Riesgo | Mitigación en Cuervo CLI |
|--------|--------------------------|
| **Confabulación/Alucinación** | Validación automática de código generado, tests obligatorios, confidence scores |
| **Privacidad de datos** | PII filter, zero-retention, data isolation multi-tenant |
| **Contenido dañino** | Output filters, content safety classifiers |
| **Seguridad informática** | Prompt injection defense, sandboxing, least privilege |
| **Impacto ambiental** | Métricas de consumo energético, modelos eficientes, caching agresivo |
| **Propiedad intelectual** | License detection, code provenance tracking, dedup filter |
| **Riesgos de cadena de valor** | Model supply chain verification, signed artifacts |

---

## 4. ISO/IEC Standards Relevantes

### 4.1 Mapa de Estándares

```
┌─────────────────────────────────────────────────────────────┐
│                ESTÁNDARES ISO/IEC PARA CUERVO CLI           │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  GESTIÓN DE IA                                               │
│  ┌────────────────────────────────────────┐                  │
│  │ ISO/IEC 42001:2023 — AI Management Sys │ ← CERTIFICABLE  │
│  │ (Annex SL compatible con 9001, 27001)  │                  │
│  └────────────────────────────────────────┘                  │
│                                                              │
│  SEGURIDAD DE LA INFORMACIÓN                                 │
│  ┌────────────────────────────────────────┐                  │
│  │ ISO/IEC 27001:2022 — ISMS             │ ← CERTIFICABLE  │
│  │ ISO/IEC 27701 — Privacy Extension      │                  │
│  └────────────────────────────────────────┘                  │
│                                                              │
│  GESTIÓN DE RIESGOS AI                                       │
│  ┌────────────────────────────────────────┐                  │
│  │ ISO/IEC 23894:2023 — AI Risk Mgmt     │                  │
│  │ ISO/IEC 38507:2022 — AI Governance    │                  │
│  └────────────────────────────────────────┘                  │
│                                                              │
│  CICLO DE VIDA AI                                            │
│  ┌────────────────────────────────────────┐                  │
│  │ ISO/IEC 5338:2023 — AI Lifecycle      │                  │
│  │ ISO/IEC 23053:2022 — ML Framework     │                  │
│  └────────────────────────────────────────┘                  │
│                                                              │
│  CALIDAD                                                     │
│  ┌────────────────────────────────────────┐                  │
│  │ ISO/IEC 25059:2023 — AI Quality Model │                  │
│  │ ISO/IEC 5259 — Data Quality for AI    │                  │
│  └────────────────────────────────────────┘                  │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 Roadmap de Certificación Recomendado

| Fase | Certificación | Timeline | Inversión Est. |
|------|--------------|----------|----------------|
| **MVP** | Self-assessment ISO 42001 | Q2 2026 | Bajo |
| **Beta** | SOC 2 Type I | Q4 2026 | $50-100K |
| **GA** | SOC 2 Type II + ISO 27001 | Q2 2027 | $150-250K |
| **Enterprise** | ISO 42001 + ISO 27701 | Q4 2027 | $200-350K |

---

## 5. SOC 2 para Productos AI SaaS

### 5.1 Trust Service Criteria Aplicados

| TSC | Aplicación a Cuervo CLI | Controles AI-Específicos |
|-----|-------------------------|--------------------------|
| **Security** | API auth, infra security, encryption at rest/transit | Model access controls, prompt injection defense |
| **Availability** | 99.9% SLA, failover, DR | Model serving reliability, graceful degradation |
| **Processing Integrity** | Input validation, output verification | Hallucination rate monitoring, output testing |
| **Confidentiality** | Data encryption, access controls | Customer code isolation, model weight protection |
| **Privacy** | Consent, data retention, deletion | Training data provenance, opt-out mechanisms |

### 5.2 Controles AI-Específicos Emergentes (SOC 2 para IA)

```
1. MODEL GOVERNANCE
   ├── Model versioning y change management
   ├── Approval workflows para model promotion a producción
   ├── Model performance regression testing
   └── Model deprecation y sunset procedures

2. DATA SEGREGATION
   ├── Multi-tenancy: aislamiento completo de datos entre clientes
   ├── Training data separation de inference data
   ├── Prompt/response logging con retention policies claras
   └── No cross-contamination entre tenants

3. BIAS MONITORING
   ├── Pre-deployment bias testing
   ├── Production bias monitoring continuo
   ├── Documented bias testing methodology
   └── Remediation procedures para bias detectado

4. EXPLAINABILITY
   ├── Reasoning traces disponibles (chain-of-thought)
   ├── Attribution de fuentes en respuestas
   ├── Confidence indicators
   └── Human review mechanisms
```

---

## 6. OWASP Top 10 for LLM Applications (v2025)

### 6.1 Matriz de Riesgos y Mitigaciones para Cuervo CLI

| # | Riesgo | Severidad | Mitigación en Cuervo CLI |
|---|--------|-----------|--------------------------|
| **LLM01** | Prompt Injection | **Crítica** | Input sanitization, instruction-data separation, canary tokens, prompt shields, privilege separation |
| **LLM02** | Sensitive Info Disclosure | **Alta** | Output filtering, PII detection, DLP integration, separate instances per tenant |
| **LLM03** | Supply Chain Vulnerabilities | **Alta** | Model signing, safe serialization (safetensors), provenance tracking, dependency scanning |
| **LLM04** | Data/Model Poisoning | **Alta** | Training data validation, model integrity verification, anomaly detection |
| **LLM05** | Improper Output Handling | **Alta** | Output validation, sandboxed execution, treating LLM output as untrusted |
| **LLM06** | Excessive Agency | **Media** | Human-in-the-loop para operaciones destructivas, confirmación explícita, least privilege |
| **LLM07** | System Prompt Leakage | **Media** | Prompt protection, no sensitive logic in system prompts, defense-in-depth |
| **LLM08** | Vector/Embedding Weaknesses | **Media** | Embedding validation, retrieval filtering, poisoned vector detection |
| **LLM09** | Misinformation | **Media** | Code validation tests, citation/attribution, confidence scores, human review |
| **LLM10** | Unbounded Consumption | **Media** | Rate limiting, token budgets, cost alerts, denial-of-wallet protection |

### 6.2 Arquitectura de Seguridad por Capas

```
┌─────────────────────────────────────────────────────────────┐
│                    CAPA 1: PERÍMETRO                         │
│  Rate limiting │ Auth (JWT+RBAC) │ WAF │ DDoS protection   │
├─────────────────────────────────────────────────────────────┤
│                    CAPA 2: INPUT                             │
│  Input sanitization │ Prompt shields │ PII detection        │
│  Token budget enforcement │ Content safety classifier       │
├─────────────────────────────────────────────────────────────┤
│                    CAPA 3: PROCESAMIENTO                     │
│  Sandboxed execution │ Model isolation │ Privilege sep.     │
│  Instruction-data separation │ Canary tokens                │
├─────────────────────────────────────────────────────────────┤
│                    CAPA 4: OUTPUT                            │
│  Output validation │ PII redaction │ License check          │
│  Code safety scan │ Confidence scoring                      │
├─────────────────────────────────────────────────────────────┤
│                    CAPA 5: MONITOREO                         │
│  Audit logging │ Anomaly detection │ Alerting              │
│  Incident response │ Forensics │ Reporting                  │
└─────────────────────────────────────────────────────────────┘
```

---

## 7. Marco Regulatorio por Región

### 7.1 Mapa Regulatorio Global

| Región | Regulación Principal | Estado | Impacto en Cuervo CLI |
|--------|---------------------|--------|----------------------|
| **EU** | AI Act + GDPR | En vigor / Enforcement ago 2026 | **Alto** — compliance obligatorio |
| **US Federal** | EO 14110 (parcialmente rescindido), sector-specific | Fragmentado | **Medio** — monitorear evolución |
| **Colorado** | Colorado AI Act (SB 24-205) | Vigente desde Feb 2026 | **Alto** — si clientes en Colorado |
| **California** | AB 2013 (transparencia GenAI) + legislación pendiente | Parcialmente vigente | **Medio** — tendencia a más regulación |
| **Brasil** | PL 2338/2023 + LGPD | En progreso legislativo | **Alto** — mercado target LATAM |
| **México** | LFPDPPP + principios éticos IA | Principios voluntarios | **Medio** — regulación emergente |
| **Colombia** | Marco ético IA + SIC guidance | Voluntario + enforcement datos | **Bajo-Medio** |
| **UK** | Sector-specific + AI Safety Institute | Pro-innovación | **Medio** |
| **China** | Regulación GenAI comprehensive | En vigor | **Alto** si se busca ese mercado |
| **Canadá** | AIDA (Bill C-27) | En progreso | **Medio** |
| **Corea del Sur** | AI Framework Act | Vigente 2026 | **Medio** |

### 7.2 Estrategia de Compliance Multi-Regional

```
FASE 1 — CORE (Lanzamiento)
├── GDPR compliance (EU)
├── LGPD compliance (Brasil)
├── Transparencia AI Act (EU)
└── OWASP LLM baseline (global)

FASE 2 — EXPANSION (Beta)
├── Colorado AI Act
├── California AB 2013
├── SOC 2 Type I
└── ISO 42001 self-assessment

FASE 3 — ENTERPRISE (GA)
├── SOC 2 Type II
├── ISO 27001 + 27701
├── ISO 42001 certification
├── EU AI Act full compliance
└── Sector-specific (HIPAA, PCI-DSS si aplica)
```

---

## 8. Propiedad Intelectual y Copyright

### 8.1 Estado Legal del Código Generado por IA

| Jurisdicción | Posición | Implicación |
|-------------|----------|-------------|
| **US** | Código puramente generado por IA no es copyrightable (Thaler v. Perlmutter) | El código generado por Cuervo CLI puede no tener protección de copyright |
| **EU** | Requiere "autoría humana" — IA como herramienta del autor | Si el desarrollador ejerce control creativo, el output puede ser copyrightable |
| **UK** | Computer-generated works tienen copyright limitado (CDPA 1988 s.9(3)) | 50 años de protección para computer-generated works |

### 8.2 Riesgos de Contaminación de Licencias

```
RIESGO: Modelo entrenado con código GPL
         │
         ▼
  Genera código similar a código GPL
         │
         ▼
  ¿Obligaciones GPL se transfieren al output?
         │
    ┌────┴────┐
    │         │
    ▼         ▼
  INCIERTO   MITIGACIONES
              ├── License detection en output
              ├── Dedup filter contra código público conocido
              ├── Provenance tracking
              ├── IP indemnification (enterprise)
              └── Attribution cuando se detecta similitud
```

### 8.3 Controles Implementables

1. **Code provenance tracking**: Rastrear similitud con código público conocido
2. **License detection**: Identificar licencias de código fuente similar
3. **Deduplication filter**: Filtrar outputs que repliquen código de training data
4. **IP indemnification**: Protección contractual para clientes enterprise
5. **Attribution system**: Citar fuentes cuando se detecte similitud significativa

---

## 9. Audit Logging — Requisitos Consolidados

### 9.1 Qué Registrar

```
INTERACCIONES AI
├── Prompt/input del usuario (con PII redactado)
├── Response/output del modelo
├── Modelo y versión utilizada
├── Tokens consumidos
├── Latencia de respuesta
├── Confidence scores
└── Herramientas invocadas (file ops, bash, search)

ACCESO Y OPERACIONES
├── Autenticación y autorización
├── Acceso a archivos del usuario
├── Ejecución de comandos bash
├── Modificaciones a archivos
├── Operaciones git
└── Acceso a APIs externas

MODELO Y SISTEMA
├── Model deployment/rollback events
├── Configuration changes
├── Fine-tuning events
├── Training data access
├── Feature flag changes
└── System errors y anomalías

COMPLIANCE
├── Consent events (opt-in/opt-out)
├── Data deletion requests (GDPR Art. 17)
├── Data export requests (GDPR Art. 20)
├── DPIA reviews
└── Incident reports
```

### 9.2 Requisitos por Framework

| Framework | Requisito de Logging | Retención |
|-----------|---------------------|-----------|
| **EU AI Act** | Logs automáticos de operación del sistema AI | Según propósito + ley nacional |
| **GDPR Art. 30** | Records de actividades de procesamiento | Duración del procesamiento |
| **SOC 2** | Logs de acceso + cambios + integridad de logs | 1 año mínimo (Type II) |
| **ISO 27001** | Event logging + protección + admin logging | Según política organizacional |
| **NIST AI RMF** | Logging como parte de MEASURE y MANAGE | Según risk assessment |

### 9.3 Implementación Técnica

```
┌──────────┐     ┌──────────────┐     ┌─────────────────┐
│ Cuervo   │────▶│ Audit Logger │────▶│ Immutable Store  │
│ CLI      │     │ (append-only)│     │ (encrypted)      │
└──────────┘     └──────┬───────┘     └────────┬────────┘
                        │                      │
                        ▼                      ▼
                 ┌──────────────┐     ┌─────────────────┐
                 │ Real-time    │     │ Long-term        │
                 │ Alerting     │     │ Archive          │
                 │ (anomalies)  │     │ (1-7 years)      │
                 └──────────────┘     └─────────────────┘
                        │
                        ▼
                 ┌──────────────┐
                 │ Audit        │
                 │ Dashboard    │
                 │ (Grafana)    │
                 └──────────────┘
```

**Principios:**
- **Append-only**: Logs inmutables, no modificables
- **Separation of duty**: Quienes operan el sistema no pueden modificar logs
- **Encryption**: Logs encriptados at rest y in transit
- **Tamper-evidence**: Hash chains o similar para detectar manipulación
- **Retention policy**: 1-7 años según regulación aplicable

---

## 10. Recomendaciones de Implementación

### 10.1 Arquitectura Security-by-Design

| Principio | Implementación |
|-----------|---------------|
| **Least Privilege** | Cada agente/servicio con permisos mínimos necesarios |
| **Defense in Depth** | 5 capas de seguridad (perímetro → output → monitoreo) |
| **Zero Trust** | Verificar cada request, no confiar en perímetro |
| **Data Minimization** | Solo procesar datos necesarios para la función |
| **Privacy by Default** | Configuración más restrictiva como default |
| **Secure by Default** | Sandboxing activado, confirmaciones habilitadas |
| **Audit Everything** | Logging comprehensivo con protecciones de privacidad |

### 10.2 Priorización de Controles de Seguridad

**P0 — Críticos (antes de cualquier release):**
- Sandboxed execution de código
- Input sanitization / prompt injection defense
- Authentication (JWT + RBAC)
- TLS 1.3 everywhere
- PII detection basic

**P1 — Altos (antes de Beta):**
- Audit logging completo
- Zero-retention mode
- Multi-tenant data isolation
- Rate limiting + abuse detection
- Output validation

**P2 — Medios (antes de GA):**
- SOC 2 Type I
- Code provenance tracking
- Bias monitoring pipeline
- Incident response plan
- Red teaming program

**P3 — Enterprise (GA+):**
- SOC 2 Type II
- ISO certifications
- IP indemnification
- SIEM integration
- Penetration testing program

---

*Documento generado el 6 de febrero de 2026. Sujeto a actualización conforme evolucione el marco regulatorio.*
