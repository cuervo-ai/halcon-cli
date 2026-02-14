# Cuervo CLI

<div align="center">

**Plataforma de IA Generativa para Desarrollo de Software**

[![Rust](https://img.shields.io/badge/Rust-1.80+-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Build Status](https://img.shields.io/badge/build-passing-brightgreen.svg)]()
[![Documentation](https://img.shields.io/badge/docs-complete-green.svg)](docs/)

**Unifica modelos propietarios, open source y locales en un solo CLI extensible**

---

### 🚀 Instalación Rápida

<table>
<tr>
<td>

**Linux / macOS**
```bash
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
```

</td>
<td>

**Windows**
```powershell
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
```

</td>
</tr>
</table>

[📖 Guía de Inicio Rápido](QUICKSTART.md) | [📚 Documentación Completa](INSTALL.md) | [🎯 Ejemplos de Uso](#-uso-rápido)

---

</div>

## 🚀 Visión

Cuervo CLI es la primera plataforma de IA para desarrollo que unifica modelos propietarios, open source y locales en un solo CLI extensible, con soporte nativo para self-hosting, fine-tuning integrado, y orquestación multi-agente — diseñada desde cero para equipos enterprise y el mercado latinoamericano.

## ✨ Características Principales

| Característica | Descripción | Estado |
|----------------|-------------|--------|
| **Multi-modelo** | Soporte unificado para Anthropic, OpenAI, Ollama, Gemini, DeepSeek y más | ✅ |
| **Self-hosted** | Ejecución local/on-premise con control total de datos | ✅ |
| **Open Source** | Núcleo completamente abierto y extensible | ✅ |
| **Fine-tuning** | Pipeline integrado para personalización de modelos | 🚧 |
| **Multi-agente** | Orquestación de equipos de agentes especializados | ✅ |
| **Modo Offline** | Funcionalidad completa sin conexión a internet | ✅ |
| **Soporte LATAM** | Interfaz en español/portugués y contexto regional | ✅ |
| **Compliance** | Diseñado para cumplimiento normativo (GDPR, LGPD, etc.) | ✅ |
| **MCP Native** | Integración nativa con Model Context Protocol | ✅ |
| **Memoria Persistente** | Sistema de memoria semántica con búsqueda vectorial | ✅ |

## 📦 Instalación

### 🚀 Instalación Rápida (Un Solo Comando)

Instala Cuervo CLI en **menos de 10 segundos** con detección automática de tu plataforma:

<table>
<tr>
<td width="50%">

**Linux / macOS**
```bash
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
```

</td>
<td width="50%">

**Windows (PowerShell)**
```powershell
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
```

</td>
</tr>
</table>

**¿Qué hace el instalador?**
- ✅ **Detecta automáticamente** tu sistema operativo y arquitectura (x86_64, ARM64, etc.)
- ✅ **Descarga el binario** precompilado desde [GitHub Releases](https://github.com/cuervo-ai/cuervo-cli/releases/latest)
- ✅ **Verifica integridad** con checksums SHA256
- ✅ **Instala en** `~/.local/bin/cuervo` (Unix) o `%USERPROFILE%\.local\bin\cuervo.exe` (Windows)
- ✅ **Configura PATH** automáticamente para tu shell (bash, zsh, fish, PowerShell)
- ✅ **Fallback inteligente** a `cargo install` si no hay binario para tu plataforma

### ✅ Verificar Instalación

Después de instalar, verifica que funcione correctamente:

```bash
# Verificar versión
cuervo --version
# Salida esperada: cuervo 0.1.0 (f8f41dd0, aarch64-apple-darwin)

# Ejecutar diagnósticos
cuervo doctor

# Mostrar ayuda
cuervo --help
```

Si el comando `cuervo` no se encuentra, recarga tu shell:
```bash
# Bash
source ~/.bashrc

# Zsh
source ~/.zshrc

# Fish
source ~/.config/fish/config.fish
```

---

### 📦 Métodos Alternativos de Instalación

<details>
<summary><b>Método 2: Instalación desde Cargo</b> (Compilación desde fuentes, ~2-5 minutos)</summary>

**Requisitos previos:**
- Rust 1.80+ ([instalar rustup](https://rustup.rs/))
- SQLite 3.35+ (generalmente incluido en sistemas modernos)

```bash
# Instalar desde repositorio Git
cargo install --git https://github.com/cuervo-ai/cuervo-cli --features tui --locked

# El binario se instalará en ~/.cargo/bin/cuervo
```

**Ventajas:**
- Siempre obtienes la última versión
- Compilado específicamente para tu sistema
- Incluye optimizaciones locales

**Desventajas:**
- Requiere tener Rust instalado
- Toma varios minutos compilar
- Requiere espacio en disco para dependencias

</details>

<details>
<summary><b>Método 3: Descarga Manual de Binarios</b></summary>

1. Ve a la página de [Releases](https://github.com/cuervo-ai/cuervo-cli/releases/latest)
2. Descarga el archivo para tu plataforma:
   - **Linux x64 (glibc)**: `cuervo-x86_64-unknown-linux-gnu.tar.gz`
   - **Linux x64 (musl/Alpine)**: `cuervo-x86_64-unknown-linux-musl.tar.gz`
   - **macOS Intel**: `cuervo-x86_64-apple-darwin.tar.gz`
   - **macOS Apple Silicon (M1/M2/M3/M4)**: `cuervo-aarch64-apple-darwin.tar.gz`
   - **Windows x64**: `cuervo-x86_64-pc-windows-msvc.zip`
3. Descarga también el archivo `.sha256` correspondiente
4. Verifica el checksum:
   ```bash
   # Linux/macOS
   sha256sum -c cuervo-*.tar.gz.sha256

   # Windows (PowerShell)
   (Get-FileHash cuervo-*.zip).Hash -eq (Get-Content cuervo-*.zip.sha256).Split()[0]
   ```
5. Extrae el archivo:
   ```bash
   # Linux/macOS
   tar xzf cuervo-*.tar.gz

   # Windows
   Expand-Archive cuervo-*.zip
   ```
6. Mueve el binario a una ubicación en tu PATH:
   ```bash
   # Linux/macOS
   mv cuervo ~/.local/bin/
   chmod +x ~/.local/bin/cuervo

   # Windows
   move cuervo.exe %USERPROFILE%\.local\bin\
   ```

</details>

<details>
<summary><b>Método 4: Compilación desde Fuentes (Desarrollo)</b></summary>

Para desarrollo activo o contribuciones:

```bash
# Clonar el repositorio
git clone https://github.com/cuervo-ai/cuervo-cli.git
cd cuervo-cli

# Compilar en modo debug (más rápido, sin optimizaciones)
cargo build --features tui

# Compilar en modo release (optimizado, más lento)
cargo build --release --features tui

# El binario estará en:
# - Debug: ./target/debug/cuervo
# - Release: ./target/release/cuervo

# Ejecutar sin instalar
cargo run --features tui -- --help

# Instalar localmente desde el código fuente
cargo install --path crates/cuervo-cli --features tui
```

</details>

---

### ⚙️ Configuración Inicial

Después de instalar, configura tus credenciales de API:

```bash
# Método 1: Asistente interactivo (recomendado)
cuervo init

# Método 2: Configuración manual por proveedor
cuervo auth login anthropic   # Para Claude (Anthropic)
cuervo auth login openai      # Para GPT (OpenAI)
cuervo auth login deepseek    # Para DeepSeek
cuervo auth login ollama      # Para modelos locales (Ollama)

# Verificar configuración
cuervo config show
```

**Variables de entorno (alternativa):**
```bash
# Añadir a ~/.bashrc, ~/.zshrc, o equivalente
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
export DEEPSEEK_API_KEY="sk-..."
```

---

### 📚 Documentación Completa

- **[Guía de Instalación Completa](INSTALL.md)** - Troubleshooting, plataformas soportadas, métodos avanzados
- **[Guía de Usuario](docs/USER_GUIDE.md)** - Uso completo del CLI
- **[Guía de Releases](RELEASE.md)** - Para mantenedores y contribuidores

## 🎯 Uso Rápido

### Chat Interactivo (REPL)
```bash
# Iniciar sesión interactiva (modo por defecto)
cuervo

# Con prompt inicial
cuervo "Ayúdame a escribir una función en Rust"

# Especificar proveedor y modelo
cuervo --provider ollama --model llama3.2 "Explica este código"
```

### Comandos Principales
```bash
# Gestión de configuración
cuervo config show
cuervo config set general.default_model "claude-sonnet-4-5-20250929"

# Estado del sistema
cuervo status
cuervo doctor

# Gestión de sesiones
cuervo chat --resume <session-id>
cuervo trace export <session-id>

# Memoria semántica
cuervo memory search "patrones de diseño"
cuervo memory list --type code_snippet

# Inicializar proyecto
cuervo init --force
```

### Comandos REPL (dentro de sesión interactiva)
```
/help                    # Mostrar ayuda categorizada
/model                   # Mostrar modelo actual
/cost                    # Desglose de costos de sesión
/session list            # Listar sesiones recientes
/memory search <query>   # Buscar en memoria
/doctor                  # Ejecutar diagnósticos
/quit                    # Guardar y salir
```

## 🏗️ Arquitectura

### Estructura del Workspace (14 crates)
```
cuervo-cli/
├── crates/
│   ├── cuervo-cli/          # Binary: REPL, TUI, commands, rendering
│   ├── cuervo-core/         # Domain: types, traits, events (zero I/O)
│   ├── cuervo-providers/    # Model adapters: Anthropic, OpenAI, DeepSeek, Gemini, Ollama
│   ├── cuervo-tools/        # 23 tool implementations: file ops, bash, git, search
│   ├── cuervo-auth/         # Auth: device flow, keychain, JWT, OAuth PKCE
│   ├── cuervo-storage/      # Persistence: SQLite, migrations, audit, cache, metrics
│   ├── cuervo-security/     # Cross-cutting: PII detection, permissions, sanitizer
│   ├── cuervo-context/      # Context engine v2: L0-L4 tiers, pipeline, elider
│   ├── cuervo-mcp/          # MCP runtime: host, server, stdio transport
│   ├── cuervo-files/        # File intelligence: 12 format handlers
│   ├── cuervo-runtime/      # Multi-agent runtime: registry, federation, executor
│   ├── cuervo-api/          # Shared API types + axum server
│   ├── cuervo-client/       # Async typed SDK (HTTP + WebSocket)
│   └── cuervo-desktop/      # egui native desktop app
├── docs/                    # Documentation
├── config/                  # Default configurations
└── scripts/                 # Build and test scripts
```

### Proveedores Soportados
| Proveedor | Modelos | Local | Cloud | API Key |
|-----------|---------|-------|-------|---------|
| **Anthropic** | Claude Sonnet, Haiku, Opus | ❌ | ✅ | ✅ |
| **Ollama** | Llama, Mistral, CodeLlama, etc. | ✅ | ❌ | ❌ |
| **OpenAI** | GPT-4o, GPT-4 Turbo | ❌ | ✅ | ✅ |
| **Gemini** | Gemini Pro, Flash | ❌ | ✅ | ✅ |
| **DeepSeek** | DeepSeek Coder, Chat | ❌ | ✅ | ✅ |
| **OpenAI Compat** | Compatible con APIs OpenAI | ✅/❌ | ✅/❌ | Opcional |
| **Echo** | Debug/testing | ✅ | ❌ | ❌ |
| **Replay** | Reproducción de trazas | ✅ | ❌ | ❌ |

### Herramientas Disponibles (23 tools)
| Herramienta | Descripción | Permisos |
|-------------|-------------|----------|
| `file_read` | Lectura de archivos | ReadOnly |
| `file_write` | Escritura atómica de archivos | Destructive |
| `file_edit` | Edición atómica de archivos | Destructive |
| `file_delete` | Eliminación de archivos | Destructive |
| `file_inspect` | Inspección de formatos de archivo | ReadOnly |
| `directory_tree` | Exploración de directorios | ReadOnly |
| `grep` | Búsqueda en contenido | ReadOnly |
| `glob` | Búsqueda por patrones | ReadOnly |
| `fuzzy_find` | Búsqueda difusa de archivos | ReadOnly |
| `symbol_search` | Búsqueda de símbolos en código | ReadOnly |
| `bash` | Ejecución de comandos shell | Destructive |
| `git_status` | Estado de repositorio Git | ReadOnly |
| `git_diff` | Diferencias Git | ReadOnly |
| `git_log` | Historial de commits | ReadOnly |
| `git_add` | Staging de archivos | ReadWrite |
| `git_commit` | Creación de commits | Destructive |
| `web_fetch` | HTTP GET/fetch | ReadOnly |
| `web_search` | Búsqueda web (Brave API) | ReadOnly |
| `http_request` | HTTP POST/PUT/DELETE/PATCH | Destructive |
| `task_track` | Seguimiento de tareas | ReadWrite |
| `background_start` | Procesos en segundo plano | Destructive |
| `background_output` | Salida de procesos | ReadOnly |
| `background_kill` | Terminar procesos | Destructive |

## 🔧 Configuración

### Archivos de Configuración
Cuervo CLI utiliza configuración jerárquica:
1. **Comandos CLI** (--model, --provider)
2. **Variables de entorno** (CUERVO_MODEL, CUERVO_PROVIDER)
3. **Config local** (`./.cuervo/config.toml`)
4. **Config global** (`~/.cuervo/config.toml`)
5. **Config por defecto** (`config/default.toml`)

### Ejemplo de Configuración
```toml
# ~/.cuervo/config.toml
[general]
default_provider = "anthropic"
default_model = "claude-sonnet-4-5-20250929"
max_tokens = 8192
temperature = 0.0

[models.providers.ollama]
enabled = true
api_base = "http://localhost:11434"
default_model = "llama3.2"

[tools]
confirm_destructive = true
timeout_secs = 120
allowed_directories = ["/home/user/projects"]

[security]
pii_detection = true
pii_action = "warn"
audit_enabled = true
```

### Variables de Entorno
```bash
export CUERVO_MODEL="claude-sonnet-4-5-20250929"
export CUERVO_PROVIDER="anthropic"
export CUERVO_LOG="debug"
export ANTHROPIC_API_KEY="sk-ant-..."
```

## 🛡️ Seguridad

### Características de Seguridad
- **Detección de PII**: Identificación automática de información personal
- **Auditoría**: Registro completo de todas las operaciones
- **Aislamiento**: Sandboxing de herramientas potencialmente peligrosas
- **Cifrado**: Almacenamiento seguro de claves API en keychain del sistema
- **Control de acceso**: Permisos granulares por herramienta y directorio

### Configuración de Seguridad
```toml
[security]
pii_detection = true
pii_action = "block"  # warn, block, or redact
audit_enabled = true
audit_retention_days = 90

[tools]
confirm_destructive = true
allowed_directories = ["/safe/path"]
blocked_patterns = [
    "**/.env",
    "**/.env.*",
    "**/credentials.json",
    "**/*.pem",
    "**/*.key",
]
```

## 📚 Sistema de Memoria

### Tipos de Memoria
```rust
enum MemoryEntryType {
    Fact,           // Hechos aprendidos
    SessionSummary, // Resúmenes de sesiones
    Decision,       // Decisiones tomadas
    CodeSnippet,    // Fragmentos de código
    ProjectMeta,    // Metadatos de proyecto
}
```

### Comandos de Memoria
```bash
# Búsqueda semántica
cuervo memory search "patrón singleton en Rust"

# Listado filtrado
cuervo memory list --type code_snippet --limit 20

# Estadísticas
cuervo memory stats

# Mantenimiento
cuervo memory prune --force
```

## 🔄 Integración MCP (Model Context Protocol)

Cuervo CLI incluye soporte nativo para MCP, permitiendo:

```bash
# Iniciar servidor MCP para integración con IDEs
cuervo mcp-server --working-dir ./project

# Los clientes MCP pueden conectarse via stdio
# para acceder a herramientas y contexto
```

### Características MCP
- **Transporte stdio**: Comunicación bidireccional
- **Pool de conexiones**: Múltiples clientes simultáneos
- **Bridge unificado**: Integración con herramientas existentes
- **Contexto compartido**: Memoria y estado disponibles para clientes

## 🧪 Testing y Calidad

### Suite de Tests
```bash
# Tests unitarios
cargo test

# Tests de integración
cargo test --test cli_e2e

# Tests de proveedores (requiere configuración)
./scripts/test_providers.sh

# Tests interactivos
python tests/interactive/run_pty_tests.py
```

### Métricas de Calidad
- **Cobertura de código**: >85% (objetivo)
- **Tests E2E**: Comandos CLI principales
- **Validación de proveedores**: Tests de integración reales
- **Pruebas de seguridad**: Auditoría de herramientas
- **Benchmarks**: Rendimiento y latencia

## 📊 Roadmap

### Fase Actual (Q1 2026)
- [x] CLI básico con REPL interactivo
- [x] Soporte multi-proveedor (Anthropic, Ollama, OpenAI)
- [x] Sistema de herramientas básicas
- [x] Almacenamiento persistente con SQLite
- [x] Sistema de memoria semántica
- [x] Integración MCP básica

### Próximas Fases
- [ ] Fine-tuning integrado (Q2 2026)
- [ ] Orquestación multi-agente avanzada (Q3 2026)
- [ ] Marketplace de extensiones (Q4 2026)
- [ ] Cuervo Cloud (auto-hosting gestionado) (2027)
- [ ] SDK para desarrolladores (2027)

## 🤝 Contribuir

### Guía de Contribución
1. **Fork** el repositorio
2. **Crea una rama** (`git checkout -b feature/amazing-feature`)
3. **Commit cambios** (`git commit -m 'Add amazing feature'`)
4. **Push a la rama** (`git push origin feature/amazing-feature`)
5. **Abre un Pull Request**

### Estándares de Código
- **Rustfmt**: Formateo automático de código
- **Clippy**: Linting estático
- **Tests**: Nuevas funcionalidades requieren tests
- **Documentación**: Comentarios y docs actualizados

### Estructura de Commits
```
feat: nueva funcionalidad
fix: corrección de bug
docs: documentación
style: formato (sin cambios funcionales)
refactor: refactorización de código
test: tests
chore: mantenimiento
```

## 📄 Licencia

Este proyecto está licenciado bajo la **Apache License 2.0** - ver el archivo [LICENSE](LICENSE) para más detalles.

## 🌐 Recursos

- **Documentación Completa**: [docs/](docs/)
- **Reporte de Investigación**: [docs/01-research/](docs/01-research/)
- **Arquitectura Enterprise**: [docs/08-enterprise-design/](docs/08-enterprise-design/)
- **Sistema de Conocimiento**: [docs/09-knowledge-system/](docs/09-knowledge-system/)
- **Especificaciones UX**: [docs/ux/](docs/ux/)

## 🆘 Soporte

- **Issues**: [GitHub Issues](https://github.com/cuervo-ai/cuervo-cli/issues)
- **Discusiones**: [GitHub Discussions](https://github.com/cuervo-ai/cuervo-cli/discussions)
- **Documentación**: [docs/](docs/)

---

<div align="center">

**Cuervo CLI** - Plataforma de IA Generativa para Desarrollo de Software

*"Unificando el futuro del desarrollo asistido por IA"*

</div>
