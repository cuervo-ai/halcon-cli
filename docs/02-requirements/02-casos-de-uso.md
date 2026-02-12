# Casos de Uso

**Proyecto:** Cuervo CLI — Plataforma de IA Generativa para Desarrollo de Software
**Versión:** 1.0
**Fecha:** 6 de febrero de 2026

---

## Resumen Ejecutivo

Se definen 8 casos de uso principales agrupados en 3 categorías: **Desarrollo de Software** (core), **Operaciones y DevOps**, y **Enterprise**. Cada caso incluye actores, precondiciones, flujo principal, flujos alternativos y criterios de aceptación.

---

## 1. Mapa de Actores

```
┌─────────────────────────────────────────────────────────────┐
│                      ACTORES DEL SISTEMA                     │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  PRIMARIOS                                                   │
│  ┌──────────────────┐  ┌──────────────────┐                 │
│  │ Desarrollador     │  │ Tech Lead         │                │
│  │ Individual        │  │ (Enterprise)      │                │
│  └──────────────────┘  └──────────────────┘                 │
│  ┌──────────────────┐  ┌──────────────────┐                 │
│  │ DevOps / SRE     │  │ Data Scientist    │                │
│  │ Engineer         │  │ / ML Engineer     │                │
│  └──────────────────┘  └──────────────────┘                 │
│                                                              │
│  SECUNDARIOS                                                 │
│  ┌──────────────────┐  ┌──────────────────┐                 │
│  │ Administrador     │  │ Auditor /         │                │
│  │ de Plataforma    │  │ Compliance Ofc.   │                │
│  └──────────────────┘  └──────────────────┘                 │
│                                                              │
│  SISTEMAS EXTERNOS                                           │
│  ┌──────────────────┐  ┌──────────────────┐                 │
│  │ Proveedores de   │  │ Servicios Cuervo  │                │
│  │ Modelos (APIs)   │  │ (auth, prompt...) │                │
│  └──────────────────┘  └──────────────────┘                 │
│  ┌──────────────────┐  ┌──────────────────┐                 │
│  │ Git Remotes      │  │ CI/CD Pipelines   │                │
│  │ (GitHub, GitLab) │  │                   │                │
│  └──────────────────┘  └──────────────────┘                 │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## 2. Casos de Uso — Desarrollo de Software

### CU-001: Implementar Feature a partir de Descripción

**Actor principal:** Desarrollador Individual
**Nivel:** User Goal
**Prioridad:** Must (MVP)

**Descripción:** El desarrollador describe en lenguaje natural una nueva funcionalidad y Cuervo CLI planifica, implementa y valida el código necesario.

**Precondiciones:**
- Cuervo CLI instalado y configurado
- Proyecto con codebase existente
- Al menos un modelo configurado (local o cloud)

**Flujo Principal:**
```
1. El desarrollador inicia Cuervo CLI en el directorio del proyecto
2. El desarrollador describe la feature: "Agrega autenticación OAuth2 con Google"
3. Cuervo CLI analiza el codebase existente (arquitectura, patrones, dependencias)
4. El agente planificador propone un plan de implementación:
   - Archivos a crear/modificar
   - Dependencias a instalar
   - Tests a generar
5. El desarrollador revisa y aprueba el plan (o solicita cambios)
6. El agente ejecutor implementa los cambios archivo por archivo
7. Para cada cambio, Cuervo muestra el diff y solicita aprobación
8. El agente de testing ejecuta los tests existentes + nuevos
9. Cuervo presenta un resumen de cambios realizados
10. El desarrollador confirma o solicita ajustes
```

**Flujos Alternativos:**
- **5a.** El desarrollador rechaza el plan → Cuervo solicita feedback y re-planifica
- **7a.** El desarrollador rechaza un cambio → Cuervo ajusta la implementación
- **8a.** Tests fallan → Cuervo analiza errores y propone correcciones

**Criterios de Aceptación:**
- [ ] Plan visible antes de cualquier modificación de código
- [ ] Cada cambio de archivo requiere aprobación explícita
- [ ] Tests se ejecutan automáticamente post-implementación
- [ ] Cambios son reversibles (undo/rollback)
- [ ] Tiempo total para feature simple: <10 minutos

---

### CU-002: Debug de Error con Contexto Completo

**Actor principal:** Desarrollador Individual
**Nivel:** User Goal
**Prioridad:** Must (MVP)

**Descripción:** El desarrollador presenta un error (stack trace, mensaje de error, comportamiento inesperado) y Cuervo CLI diagnostica la causa raíz y propone una solución.

**Flujo Principal:**
```
1. El desarrollador pega un error o describe el problema
2. Cuervo analiza el error en contexto del codebase:
   - Identifica archivos relevantes
   - Comprende el flujo de datos
   - Revisa logs si están disponibles
3. Cuervo presenta diagnóstico con causa raíz probable
4. Cuervo propone una o más soluciones rankeadas por confianza
5. El desarrollador selecciona una solución
6. Cuervo implementa el fix y muestra el diff
7. El desarrollador aprueba y se ejecutan tests
```

**Criterios de Aceptación:**
- [ ] Diagnóstico incluye explicación comprensible de la causa raíz
- [ ] Múltiples soluciones propuestas cuando hay ambigüedad
- [ ] Fix no introduce regresiones (tests pasan)
- [ ] Tiempo de diagnóstico: <2 minutos para errores comunes

---

### CU-003: Refactoring Multi-Archivo Guiado

**Actor principal:** Tech Lead
**Nivel:** User Goal
**Prioridad:** Must (MVP)

**Descripción:** El usuario solicita un refactoring que afecta múltiples archivos (renombrar, extraer componente, cambiar patrón arquitectónico) y Cuervo CLI ejecuta los cambios de forma coordinada.

**Flujo Principal:**
```
1. El usuario describe el refactoring: "Migra el UserService de class-based a functional con DI"
2. Cuervo analiza todos los archivos afectados e identifica dependencias
3. Cuervo presenta:
   a. Lista de archivos a modificar
   b. Impacto estimado (líneas cambiadas, tests afectados)
   c. Plan paso a paso
4. El usuario aprueba el plan
5. Cuervo ejecuta cambios en orden de dependencia
6. Para cada archivo, muestra diff y solicita aprobación
7. Ejecuta tests después de cada grupo lógico de cambios
8. Presenta resumen final con métricas del refactoring
```

**Criterios de Aceptación:**
- [ ] Ningún cambio rompe la compilación en ningún paso intermedio
- [ ] Imports y referencias actualizados automáticamente
- [ ] Tests existentes adaptados al nuevo código
- [ ] Rollback completo posible en cualquier punto

---

### CU-004: Code Review Asistido por IA

**Actor principal:** Tech Lead / Desarrollador
**Nivel:** User Goal
**Prioridad:** Should (Beta)

**Descripción:** Cuervo CLI analiza un PR o un conjunto de cambios y proporciona una revisión de código con categorías de severidad.

**Flujo Principal:**
```
1. El usuario ejecuta: `cuervo /review` o `cuervo /review-pr 123`
2. Cuervo analiza los cambios en contexto del codebase completo
3. Cuervo genera un reporte de review con:
   a. Resumen ejecutivo de los cambios
   b. Issues encontrados (critical, warning, info)
   c. Sugerencias de mejora
   d. Compliance con estándares del proyecto
   e. Cobertura de tests analysis
4. El usuario puede solicitar más detalle en cualquier finding
5. Cuervo puede generar comentarios directamente en el PR (GitHub/GitLab)
```

**Criterios de Aceptación:**
- [ ] Review cubre: bugs, seguridad, performance, estilo, tests
- [ ] Findings priorizados por severidad
- [ ] Sugerencias incluyen código de ejemplo
- [ ] Integración directa con GitHub PR comments

---

## 3. Casos de Uso — Operaciones y DevOps

### CU-005: Generación y Gestión de Commits Inteligente

**Actor principal:** Desarrollador Individual
**Nivel:** User Goal
**Prioridad:** Must (MVP)

**Descripción:** Cuervo CLI analiza los cambios staged/unstaged, genera un mensaje de commit apropiado siguiendo las convenciones del proyecto, y ejecuta el commit.

**Flujo Principal:**
```
1. El usuario ejecuta: `cuervo /commit`
2. Cuervo analiza:
   a. `git status` para cambios pendientes
   b. `git diff` para contenido de cambios
   c. `git log` para estilo de commits previos
3. Cuervo genera un mensaje de commit:
   a. Siguiendo conventional commits o el patrón del proyecto
   b. Resumiendo el "por qué" no solo el "qué"
4. El usuario aprueba o edita el mensaje
5. Cuervo ejecuta staging de archivos relevantes y commit
6. Cuervo muestra confirmación con git status resultante
```

**Criterios de Aceptación:**
- [ ] Mensaje sigue convenciones del proyecto automáticamente
- [ ] No commitea archivos sensibles (.env, credentials)
- [ ] Advertencia si hay archivos untracked potencialmente relevantes
- [ ] Co-authored-by header configurable

---

### CU-006: Troubleshooting de Infraestructura

**Actor principal:** DevOps / SRE Engineer
**Nivel:** User Goal
**Prioridad:** Should (Beta)

**Descripción:** El ingeniero DevOps usa Cuervo CLI para diagnosticar problemas de infraestructura, analizar logs, y proponer soluciones.

**Flujo Principal:**
```
1. El usuario describe el problema: "Los pods están en CrashLoopBackOff en staging"
2. Cuervo ejecuta comandos de diagnóstico (kubectl, docker, logs)
3. Cuervo analiza los logs y el estado de la infraestructura
4. Cuervo presenta:
   a. Causa raíz probable
   b. Evidencia de los logs
   c. Pasos de remediación
5. El usuario puede aprobar ejecución de comandos de remediación
```

---

## 4. Casos de Uso — Enterprise

### CU-007: Onboarding de Nuevo Desarrollador al Proyecto

**Actor principal:** Nuevo Desarrollador
**Nivel:** User Goal
**Prioridad:** Should (Beta)

**Descripción:** Un desarrollador nuevo en un proyecto usa Cuervo CLI para comprender la arquitectura, convenciones y estructura del codebase rápidamente.

**Flujo Principal:**
```
1. El nuevo desarrollador ejecuta: `cuervo explain-project`
2. Cuervo analiza el codebase completo y presenta:
   a. Resumen arquitectónico (componentes principales, relaciones)
   b. Stack tecnológico (lenguaje, framework, dependencias)
   c. Patrones de diseño utilizados
   d. Estructura de directorios con explicación
   e. Cómo correr el proyecto (build, test, deploy)
3. El desarrollador puede hacer preguntas follow-up:
   "¿Cómo funciona la autenticación?"
   "¿Dónde se manejan los pagos?"
4. Cuervo navega al código relevante y explica
```

**Criterios de Aceptación:**
- [ ] Resumen preciso de arquitectura en <30 segundos
- [ ] Navegación contextual a código relevante
- [ ] Respuestas basadas en código real, no genéricas
- [ ] Reduce tiempo de onboarding de días a horas

---

### CU-008: Gestión de Modelos y Fine-tuning

**Actor principal:** ML Engineer / Admin
**Nivel:** User Goal
**Prioridad:** Could (GA)

**Descripción:** El ML Engineer usa Cuervo CLI para gestionar el catálogo de modelos, iniciar fine-tuning jobs, y evaluar resultados.

**Flujo Principal:**
```
1. El usuario ejecuta: `cuervo models list`
2. Cuervo muestra el catálogo de modelos disponibles:
   - Nombre, proveedor, versión, capabilities
   - Métricas de rendimiento (latencia, costo, calidad)
   - Estado (active, deprecated, testing)
3. El usuario inicia fine-tuning: `cuervo models finetune --base llama-3-8b --data ./training-data`
4. Cuervo prepara el dataset, valida formato, e inicia el job
5. El usuario monitorea progreso: `cuervo models status <job-id>`
6. Al completar, Cuervo ejecuta evaluación automática vs baseline
7. El usuario despliega: `cuervo models deploy <model-id> --target ollama`
```

---

## 5. Diagrama de Casos de Uso

```
                         ┌──────────────────────────────────────┐
                         │           CUERVO CLI                  │
                         │                                       │
   ┌────────┐           │  ┌─────────────────────────────┐     │
   │ Devel. │───────────┼──│ CU-001: Implementar Feature │     │
   │ Indiv. │───────────┼──│ CU-002: Debug Error         │     │
   │        │───────────┼──│ CU-005: Commit Inteligente  │     │
   └────────┘           │  └─────────────────────────────┘     │
                         │                                       │
   ┌────────┐           │  ┌─────────────────────────────┐     │
   │ Tech   │───────────┼──│ CU-003: Refactoring Multi   │     │
   │ Lead   │───────────┼──│ CU-004: Code Review IA      │     │
   └────────┘           │  └─────────────────────────────┘     │
                         │                                       │
   ┌────────┐           │  ┌─────────────────────────────┐     │
   │ DevOps │───────────┼──│ CU-006: Troubleshoot Infra  │     │
   └────────┘           │  └─────────────────────────────┘     │
                         │                                       │
   ┌────────┐           │  ┌─────────────────────────────┐     │
   │ Nuevo  │───────────┼──│ CU-007: Onboarding Proyecto │     │
   │ Dev    │           │  └─────────────────────────────┘     │
   └────────┘           │                                       │
                         │  ┌─────────────────────────────┐     │
   ┌────────┐           │  │ CU-008: Gestión Modelos &   │     │
   │ ML Eng │───────────┼──│          Fine-tuning         │     │
   └────────┘           │  └─────────────────────────────┘     │
                         │                                       │
                         └──────────────────────────────────────┘
                                         │
                         ┌───────────────┼───────────────┐
                         │               │               │
                    ┌────▼────┐    ┌─────▼─────┐  ┌─────▼─────┐
                    │ Model   │    │ Git       │  │ Cuervo    │
                    │ APIs    │    │ Remotes   │  │ Services  │
                    └─────────┘    └───────────┘  └───────────┘
```

---

## 6. Matriz de Casos de Uso vs Requisitos

| Caso de Uso | Requisitos Funcionales Clave | Fase |
|-------------|------------------------------|------|
| CU-001 | RF-101, 103, 201-208, 401-404, 409 | MVP |
| CU-002 | RF-101, 201, 207, 208, 304, 402 | MVP |
| CU-003 | RF-201-205, 213, 401-404, 407 | MVP |
| CU-004 | RF-201, 207, 210, 301, 302, 406 | Beta |
| CU-005 | RF-005, 301, 302 | MVP |
| CU-006 | RF-304, 306, 402 | Beta |
| CU-007 | RF-201, 204, 207, 213, 402 | Beta |
| CU-008 | RF-106, 110, 701-706 | GA |

---

*Documento sujeto a evolución conforme se detalle la arquitectura.*
