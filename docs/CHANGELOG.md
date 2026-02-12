# Changelog

Todos los cambios notables en Cuervo CLI serán documentados en este archivo.

El formato está basado en [Keep a Changelog](https://keepachangelog.com/es-ES/1.0.0/),
y este proyecto adhiere a [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-02-06

### Añadido
- **CLI Principal**: Comandos básicos `chat`, `config`, `auth`, `status`, `init`
- **Sistema REPL**: Terminal interactiva con autocompletado y comandos internos
- **Multi-proveedor**: Soporte para Anthropic, OpenAI, Ollama, Gemini, DeepSeek
- **Sistema de Herramientas**: 8 herramientas básicas (bash, file operations, web fetch, etc.)
- **Almacenamiento Persistente**: SQLite para sesiones, memoria y configuración
- **Sistema de Memoria**: Memoria semántica con búsqueda híbrida (keyword + vectorial)
- **Sistema de Contexto**: Gestión jerárquica de contexto multi-tenant
- **Seguridad Integrada**: Detección de PII, auditoría, sandboxing de herramientas
- **Integración MCP**: Servidor MCP nativo para integración con IDEs
- **Sistema de Eventos**: Bus de eventos para desacoplamiento de componentes
- **Testing E2E**: Suite completa de tests de integración
- **Documentación**: 9 secciones de documentación técnica y de producto

### Características Técnicas
- Arquitectura modular basada en crates de Rust
- Async/await con Tokio runtime
- Serialización con Serde
- CLI con Clap 4
- UI en terminal con Crossterm y Reedline
- Almacenamiento seguro con SQLite y cifrado
- Sistema de cache multi-nivel
- Pool de conexiones y resiliencia
- Streaming de respuestas en tiempo real
- Sistema de métricas y monitoreo

### Configuración
- Configuración jerárquica (CLI > ENV > local > global > defaults)
- Formato TOML con validación de esquema
- Variables de entorno para todos los parámetros
- Configuración por proyecto y por usuario

### Seguridad
- Detección automática de PII (8 tipos)
- Auditoría completa con hash chain
- Sandboxing de herramientas (namespace/chroot)
- Control de permisos granulares
- Cifrado de datos sensibles
- Keychain del sistema para API keys

### Proveedores Soportados
- **Anthropic**: Claude Sonnet, Haiku, Opus
- **OpenAI**: GPT-4o, GPT-4 Turbo, GPT-3.5 Turbo
- **Ollama**: Modelos locales (Llama, Mistral, CodeLlama)
- **Gemini**: Gemini Pro, Flash
- **DeepSeek**: DeepSeek Chat, Coder
- **OpenAI Compat**: APIs compatibles con OpenAI
- **Echo**: Para testing y debug
- **Replay**: Reproducción de trazas

### Herramientas Disponibles
- `bash`: Ejecución de comandos shell
- `file_read`: Lectura de archivos
- `file_write`: Escritura de archivos
- `file_edit`: Edición de archivos
- `directory_tree`: Exploración de directorios
- `grep`: Búsqueda en archivos
- `glob`: Búsqueda por patrones
- `web_fetch`: Fetch HTTP

### Comandos REPL
- `/help`, `/h`, `/?`: Ayuda categorizada
- `/quit`, `/exit`, `/q`: Salir guardando sesión
- `/clear`: Limpiar pantalla
- `/model`: Mostrar modelo actual
- `/cost`: Desglose de costos
- `/session list|show|name`: Gestión de sesiones
- `/memory list|search|stats`: Comandos de memoria
- `/doctor`: Diagnósticos del sistema

### Documentación Incluida
- `docs/01-research/`: Investigación del estado del arte
- `docs/02-requirements/`: Requerimientos funcionales y casos de uso
- `docs/03-architecture/`: Arquitectura y diseño tecnológico
- `docs/04-roadmap/`: Roadmap de desarrollo
- `docs/05-security-legal/`: Seguridad, compliance y ética
- `docs/06-deliverables/`: Entregables consolidados
- `docs/07-review/`: Revisión técnica crítica
- `docs/08-enterprise-design/`: Diseño enterprise
- `docs/09-knowledge-system/`: Sistema de conocimiento
- `docs/ux/`: Investigación y diseño UX
- `docs/diagrams/`: Diagramas de arquitectura

## [0.0.1] - 2026-01-15

### Añadido
- Estructura inicial del proyecto
- Workspace de Cargo con crates básicos
- Configuración inicial de dependencias
- Esqueleto de documentación
- Setup de CI/CD básico

---

## Guía de Actualización

### De versiones anteriores
No hay versiones anteriores públicas. Esta es la primera release pública.

### Migración de Configuración
```bash
# La configuración de versiones alpha/beta no es compatible
# Se recomienda empezar con configuración fresca
rm -rf ~/.cuervo
cuervo setup
```

### Breaking Changes
- Primera release pública - no hay breaking changes de versiones anteriores

### Deprecaciones
- Ninguna en esta versión

---

## Notas de Desarrollo

### Estándares de Versionado
- **MAJOR**: Cambios incompatibles en API
- **MINOR**: Nuevas funcionalidades compatibles
- **PATCH**: Correcciones de bugs compatibles

### Ciclo de Release
- **Nightly**: Builds automáticos de main
- **Beta**: Releases mensuales para testing
- **Stable**: Releases trimestrales

### Canales de Distribución
- **Cargo**: `cargo install --git https://github.com/cuervo-ai/cuervo-cli`
- **GitHub Releases**: Binarios precompilados
- **Source**: Compilación desde fuente

---

## Próximas Versiones

### [0.2.0] - Planeado para Q2 2026
- Fine-tuning integrado
- Marketplace de extensiones
- SDK para desarrolladores
- Mejoras en UI/UX
- Soporte para más proveedores

### [0.3.0] - Planeado para Q3 2026
- Orquestación multi-agente avanzada
- Workflows visuales
- Integración con más IDEs
- Analytics y reporting
- Mejoras de performance

### [1.0.0] - Planeado para Q4 2026
- API estable
- LTS support
- Enterprise features completas
- Certificaciones de seguridad
- Soporte comercial

---

## Historial de Cambios Detallado

### [0.1.0] - Cambios Técnicos Detallados

#### Core Architecture
- Migrado a Rust 2021 edition
- Implementado sistema de eventos con Tokio broadcast
- Refactorizado sistema de errores con thiserror y anyhow
- Implementado sistema de cache con LRU y SQLite
- Añadido sistema de métricas con histogramas y contadores

#### CLI Improvements
- Migrado a Clap 4 con derive macros
- Implementado REPL con Reedline y autocompletado
- Añadido sistema de temas y colores con syntect
- Implementado streaming de output en tiempo real
- Añadido sistema de banners y animaciones

#### Provider System
- Implementado sistema unificado de proveedores
- Añadido soporte para streaming de todos los proveedores
- Implementado sistema de fallback y load balancing
- Añadido health checking y circuit breakers
- Implementado cache de respuestas por contenido

#### Tool System
- Implementado sandboxing con namespaces (Linux)
- Añadido sistema de permisos granulares
- Implementado auditoría con hash chain
- Añadido detección de PII con regex y ML
- Implementado confirmación para operaciones destructivas

#### Storage System
- Migrado a SQLite con WAL journaling
- Implementado migraciones automáticas
- Añadido cifrado AES-256-GCM para datos sensibles
- Implementado backup y restore
- Añadido sistema de quotas y límites

#### Memory System
- Implementado almacenamiento híbrido (SQLite + vector)
- Añadido búsqueda semántica con embeddings
- Implementado sistema de calidad y relevancia
- Añadido TTL y pruning automático
- Implementado indexación incremental

#### Testing
- Añadido tests E2E con assert_cmd
- Implementado mocking de proveedores
- Añadido tests de integración con wiremock
- Implementado benchmarks de performance
- Añadido tests de seguridad y penetration

#### Documentation
- Documentación completa en español e inglés
- Diagramas de arquitectura con Mermaid
- Guías de instalación y configuración
- Tutoriales y ejemplos de uso
- Documentación de API interna

---

## Contribuidores

### Equipo Core
- **Tech Lead / Architect**: Diseño de arquitectura, decisiones técnicas
- **AI/ML Engineer**: Integración de modelos, fine-tuning
- **Backend Engineer**: APIs, almacenamiento, performance
- **Frontend Engineer**: CLI UX, renderizado en terminal
- **Security Engineer**: Compliance, auditoría, pen testing
- **DevOps/SRE**: CI/CD, monitoring, deployment

### Contribuidores Externos
- Lista vacía - primera release pública

---

## Licencia

Copyright 2026 Cuervo AI Team

Licenciado bajo Apache License, Version 2.0 (la "Licencia");
no puedes usar este archivo excepto en cumplimiento con la Licencia.
Puedes obtener una copia de la Licencia en:

    http://www.apache.org/licenses/LICENSE-2.0

A menos que lo requiera la ley aplicable o se acuerde por escrito, el software
distribuido bajo la Licencia se distribuye "TAL CUAL",
SIN GARANTÍAS NI CONDICIONES DE NINGÚN TIPO, ya sean expresas o implícitas.
Consulta la Licencia para el idioma específico que rige los permisos y
limitaciones bajo la Licencia.
