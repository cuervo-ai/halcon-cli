# Claude Code vs Halcon — Análisis Comparativo de Ingeniería 2026

> **Fuentes**: Documentación oficial Claude Code (`code.claude.com/docs`, marzo 2026) + inspección directa del código fuente de Halcon v0.3.0 (`feature/sota-intent-architecture`)
>
> **Metodología**: Toda comparación está basada en código real (Halcon) o documentación oficial verificada (Claude Code). Las inferencias arquitectónicas están marcadas con `[inferido]`.

---

## Tabla de Contenidos

1. [Resumen Ejecutivo](#1-resumen-ejecutivo)
2. [Superficies y Distribución](#2-superficies-y-distribución)
3. [Arquitectura del Loop de Agente](#3-arquitectura-del-loop-de-agente)
4. [Sistema de Sub-agentes y Orquestación](#4-sistema-de-sub-agentes-y-orquestación)
5. [Agent Teams (Claude Code) vs Orquestador Multi-wave (Halcon)](#5-agent-teams-claude-code-vs-orquestador-multi-wave-halcon)
6. [Sistema de Memoria y Contexto](#6-sistema-de-memoria-y-contexto)
7. [Hooks y Automatización de Ciclo de Vida](#7-hooks-y-automatización-de-ciclo-de-vida)
8. [Integración MCP — Model Context Protocol](#8-integración-mcp--model-context-protocol)
9. [Selección y Ejecución de Herramientas](#9-selección-y-ejecución-de-herramientas)
10. [Planificación y Control de Convergencia](#10-planificación-y-control-de-convergencia)
11. [Seguridad y Permisos](#11-seguridad-y-permisos)
12. [Extensibilidad y Ecosistema](#12-extensibilidad-y-ecosistema)
13. [CLI — Referencia de Comandos Comparada](#13-cli--referencia-de-comandos-comparada)
14. [Métricas de Código y Complejidad](#14-métricas-de-código-y-complejidad)
15. [Diferencias Algorítmicas Clave](#15-diferencias-algorítmicas-clave)
16. [Rendimiento y Eficiencia de Tokens](#16-rendimiento-y-eficiencia-de-tokens)
17. [Fortalezas y Debilidades](#17-fortalezas-y-debilidades)
18. [Brechas Funcionales — Qué le falta a cada uno](#18-brechas-funcionales--qué-le-falta-a-cada-uno)
19. [Insights Arquitectónicos Estratégicos](#19-insights-arquitectónicos-estratégicos)
20. [Recomendaciones](#20-recomendaciones)

---

## 1. Resumen Ejecutivo

| Dimensión | Claude Code (2026) | Halcon v0.3.0 |
|---|---|---|
| **Modelo de agente** | Reactivo single-turn + sub-agentes especializados | FSM multi-round explícita con 9 fases |
| **Orquestación** | Agent Teams (experimental) + subagentes en-proceso | Waves topológicas con `SharedBudget` atómico |
| **Memoria** | CLAUDE.md (4 scopes) + auto-memory + agent memory | 5 tiers (L0–L4) + SQLite 16 tablas + SemanticStore |
| **Hooks** | 8 eventos de ciclo de vida (PreToolUse, PostToolUse, SubagentStart/Stop, TeammateIdle, TaskCompleted...) | Sin hooks de ciclo de vida para el usuario |
| **MCP** | 1er ciudadano: HTTP/SSE/stdio, OAuth, 100+ servidores, Tool Search | Soporte básico vía `halcon-mcp` crate (no documentado) |
| **Superficies** | Terminal, VS Code, JetBrains, Desktop, Web, Slack, Chrome, iOS | Solo terminal (TUI ratatui + REPL clásico) |
| **Extensibilidad** | Plugins + Skills + Subagents + MCP | Plugin registry (inactivo por defecto) |
| **Control de convergencia** | Implícito (LLM decide) + `--max-turns` | TerminationOracle explícito con 4 autoridades |
| **Guardrails** | Permisos por modo (default/acceptEdits/bypass) | FASE-2 security gate + CATASTROPHIC_PATTERNS + circuit breakers |
| **Observabilidad** | Sesiones persistentes + transcripts JSONL | DecisionTrace + 17-field RoundMetrics + SQLite |

**Veredicto de ingeniería**: Claude Code ganó amplitud y ecosistema en 2026. Halcon ganó profundidad de control y confiabilidad en ejecución autónoma. Son arquitecturas complementarias que apuntan a casos de uso diferentes.

---

## 2. Superficies y Distribución

### Claude Code 2026 — Superficies disponibles (documentado)

```
Terminal CLI         → full-featured, composable, Unix-friendly
VS Code extension   → inline diffs, @-mentions, plan review, conversation history
JetBrains plugin    → IntelliJ, PyCharm, WebStorm — diff viewing + selection context
Desktop app         → macOS/Windows, múltiples sesiones paralelas, scheduled tasks, cloud sessions
Web (claude.ai/code) → browser-based, long-running tasks, repos remotos, paralelo
iOS app             → continuar sesiones web, remote control
Slack               → @Claude → PR automático desde bug report
Chrome extension    → debugging web apps en vivo
```

**Instalación** (documentado en `code.claude.com/docs/en/overview`):
```bash
# macOS/Linux — native (auto-update en background)
curl -fsSL https://claude.ai/install.sh | bash

# Windows PowerShell
irm https://claude.ai/install.ps1 | iex

# Homebrew (no auto-update)
brew install --cask claude-code

# WinGet
winget install Anthropic.ClaudeCode
```

**Características cross-surface** (confirmado):
- `CLAUDE.md`, settings y MCP servers comparten configuración en todas las superficies
- `/teleport` → mueve sesión web al terminal local
- `/desktop` → mueve sesión terminal al Desktop app
- `--remote` → crea sesión web desde CLI
- Remote Control → controlar Claude Code local desde móvil/browser
- `--from-pr` → reanuda sesiones vinculadas a un PR específico de GitHub

### Halcon v0.3.0 — Superficies disponibles (código)

```
Terminal REPL       → reedline-based, streaming, slash commands
TUI (ratatui)       → 3-zone layout, activity model, overlay, widgets
Headless mode       → feature = "headless" sin terminal UI
HTTP Control Plane  → halcon-api (axum + WebSocket) — control programático
Desktop app         → halcon-desktop (egui) — experimental
```

**Distribución**:
```bash
# Binario compilado (aarch64-apple-darwin confirmado)
curl -fsSL https://releases.cli.cuervo.cloud/latest/install.sh | bash

# Homebrew tap (CI activo)
brew install cuervo-ai/tap/halcon

# Compilación desde fuente
cargo build --release --features tui
```

### Brecha: +6 superficies en Claude Code vs Halcon

Claude Code tiene presencia nativa en el ecosistema completo del desarrollador (IDE, browser, mobile, CI, chat). Halcon está confinado al terminal. Esta diferencia es **la brecha de adopción más importante** entre los dos sistemas.

---

## 3. Arquitectura del Loop de Agente

### Claude Code — Loop reactivo por turno (inferido + confirmado)

```
Usuario envía mensaje
    ↓
[Un solo turno del asistente]
    → Razonamiento en texto natural
    → Emite 0..N tool_calls en bloque único
    → Infraestructura ejecuta tools en paralelo
    → Resultados inyectados como ToolResult blocks
    → Nueva ronda de razonamiento si se necesitan más tools
    → Emite respuesta final
[Fin del turno]
```

**Características confirmadas**:
- `--max-turns N` → limita el número de rondas de tools en modo no-interactivo
- `--max-budget-usd N` → presupuesto en USD (modo print/headless)
- Compactación automática al ~95% de capacidad (configurable con `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE`)
- Sin FSM explícita — el modelo decide cuándo dejar de usar tools
- Sin loop guard — repetición prevenida por entrenamiento del modelo, no por código

### Halcon — FSM multi-round explícita (confirmado en código)

```rust
// loop_state.rs:144 — 9 fases explícitas
enum AgentPhase {
    Idle → Planning → Executing ↔ ToolWait → Reflecting
    → Synthesizing → Evaluating → Completed | Halted
}

// Transiciones validadas en transition() — inválidas logean warning, no panican
fn transition(current: AgentPhase, event: AgentEvent) -> AgentPhase
```

**Flujo completo del loop** (confirmado en `agent/mod.rs`, `convergence_phase.rs`, `post_batch.rs`):

```
F1: Inicialización — SLA budget, PolicyConfig, BoundaryDecisionEngine routing
F2: IntentPipeline → ConvergenceController calibrado con effective_max_rounds
F3: [LOOP] Round setup → selección de tools → system prompt inyección
F4: Invocación del modelo con fallback chain (invoke_with_fallback)
F5: Acumulación de streaming: text + tool_use blocks
F6: Post-batch: dedup → ejecución → guardrails scan → supervisor → reflexión → failure tracking
F7: Fase de convergencia: ConvergenceController.observe() → TerminationOracle.adjudicate()
    → LoopGuard match → RoundScorer → SynthesisGate → auto-save
F8: Result assembly + LoopCritic evaluation
```

**Autoridades de terminación** (confirmado en `termination_oracle.rs`):
```
Precedencia:
1. Halt          — ConvergenceAction::Halt OR LoopSignal::Break
2. InjectSynthesis — ConvergenceAction::Synthesize OR synthesis_advised
3. Replan        — ConvergenceAction::Replan OR stagnation detected
4. ForceNoTools  — LoopSignal::ForceNoTools
5. Continue      — default
```

**9 triggers de síntesis** en `SynthesisTrigger`: EvidenceThreshold, TokenHeadroom, SlaExhaustion, DiminishingReturns, LoopGuardStagnation, SupervisorFailure, GovernanceRescue, OracleConvergence, PlanComplete.

### Diferencia fundamental

| | Claude Code | Halcon |
|---|---|---|
| **Control de loop** | LLM self-termination | TerminationOracle (4 autoridades externas) |
| **Estado** | Context window (implícito) | LoopState (62+ campos, explícito) |
| **Rondas** | `--max-turns` (límite externo) | SLA calibrado por IntentPipeline |
| **Detección de stagnation** | Ninguna (entrenamiento del modelo) | ToolLoopGuard: oscillation + saturation + Bayesian |
| **Replanning** | Implícito (next-token) | `Planner::replan()` explícito con hasta 3 intentos |
| **FSM validación** | Ninguna | `transition()` — no-ops en lugar de panics |

---

## 4. Sistema de Sub-agentes y Orquestación

### Claude Code — Subagentes como archivos Markdown (documentado)

**Definición** (YAML frontmatter en `.claude/agents/` o `~/.claude/agents/`):

```yaml
---
name: code-reviewer
description: Expert code reviewer. Use proactively after code changes.
tools: Read, Grep, Glob, Bash
model: sonnet          # sonnet | opus | haiku | inherit
permissionMode: default # default | acceptEdits | dontAsk | bypassPermissions | plan
maxTurns: 20
skills:
  - api-conventions
memory: user           # user | project | local — memoria persistente cross-session
background: false      # true = siempre background task
isolation: worktree    # crea git worktree aislado
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: command
          command: "./scripts/validate-command.sh"
mcpServers:
  - slack
  - name: custom-db
    url: https://db.example.com/mcp
---

You are a senior code reviewer. When invoked, analyze the code and provide
specific, actionable feedback on quality, security, and best practices.
```

**Scope y prioridad** (documentado):

| Ubicación | Scope | Prioridad |
|---|---|---|
| `--agents` CLI flag | Sesión actual | 1 (máxima) |
| `.claude/agents/` | Proyecto actual | 2 |
| `~/.claude/agents/` | Todos tus proyectos | 3 |
| Plugin `agents/` | Donde el plugin está activo | 4 (mínima) |

**Subagentes built-in** (documentado):

| Agente | Modelo | Herramientas | Cuándo se usa |
|---|---|---|---|
| **Explore** | Haiku (rápido) | Solo lectura (sin Write/Edit) | Búsqueda y análisis de codebase |
| **Plan** | Inherit | Solo lectura | Investigación en plan mode |
| **general-purpose** | Inherit | Todas | Tareas complejas multi-step |
| **Bash** | Inherit | Bash | Comandos en contexto separado |
| **statusline-setup** | Sonnet | Read, Edit | Config de status line |
| **Claude Code Guide** | Haiku | Search | Preguntas sobre Claude Code |

**Memoria persistente de agentes** (documentado `sub-agents#enable-persistent-memory`):

```yaml
memory: user
# → ~/.claude/agent-memory/<name-of-agent>/MEMORY.md
# → Primeras 200 líneas inyectadas al inicio de cada sesión
# → El agente lee/escribe sus propios archivos de memoria
```

**Restricción clave**: Los subagentes NO pueden spawnar otros subagentes. Solo el thread principal puede hacerlo.

**Foreground vs Background** (documentado):
- **Foreground**: bloquea hasta completar, permission prompts pasan al usuario
- **Background**: concurrente, pre-aprueba permisos antes de lanzar, `Ctrl+B` para background
- `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` → deshabilita background tasks

### Halcon — Orquestador con dependency waves (confirmado en código)

**Definición de tarea** (en `halcon-core/src/types/orchestrator.rs`):

```rust
pub struct SubAgentTask {
    pub task_id: Uuid,
    pub description: String,
    pub depends_on: Vec<Uuid>,   // dependencias explícitas
    pub priority: u8,            // orden dentro de wave
    pub allowed_tools: Vec<String>,
    // ...
}
```

**Ejecución por waves topológicas** (confirmado en `orchestrator.rs:81-139`):

```rust
pub fn topological_waves(tasks: &[SubAgentTask]) -> Vec<Vec<&SubAgentTask>> {
    // Iterative BFS: cada wave = tasks con deps satisfechas
    // Dependencias circulares → detectadas → tasks marcadas como failed
    // Orden dentro de wave: by priority descending
}
```

**SharedBudget atómico** (confirmado con memory ordering correcto):

```rust
pub struct SharedBudget {
    tokens_used: AtomicU64,  // Ordering::Release en add, Acquire en read
    token_limit: u64,
    start: Instant,
    duration_limit: Duration,
}
```

**Límites derivados** (confirmado en `derive_sub_limits()`):
```rust
max_rounds = parent.max_rounds.min(10)
max_total_tokens = if shared_budget { parent / max_concurrent_agents } else { parent }
max_duration_secs = config.sub_agent_timeout_secs || parent / 2
```

**Comunicación inter-agente** (confirmado en `agent_comm.rs`):
```rust
SharedContextStore::new() // Arc<Mutex<HashMap>> — wave N lee outputs de wave N-1
// Solo activo cuando config.enable_communication = true
```

### Comparación directa

| Dimensión | Claude Code | Halcon |
|---|---|---|
| **Config de agente** | Markdown + YAML frontmatter | Código Rust (SubAgentTask struct) |
| **Dependencias** | Manual (parent coordina) | Explícitas: `depends_on: Vec<Uuid>` |
| **Paralelismo** | Manual spawning | Wave automático con toposort |
| **Budget compartido** | No (por agente) | SharedBudget atómico cross-agents |
| **Ciclos** | No aplica | Detectados, tasks marcadas failed |
| **Comunicación** | Unidireccional (task → result) | SharedContextStore entre waves |
| **Memoria persistente** | Sí: `memory: user/project/local` | No (solo SQLite de sesión) |
| **Aislamiento** | `isolation: worktree` opcional | No (compartido por defecto) |
| **Modelo por agente** | Sí: `model: haiku/sonnet/opus/inherit` | No (mismo modelo que parent) |
| **Restricción de tools** | `tools` allowlist + `disallowedTools` | `allowed_tools: Vec<String>` |
| **Permission mode** | Por agente: `permissionMode` | `is_sub_agent: bool` → tighter limits |

---

## 5. Agent Teams (Claude Code) vs Orquestador Multi-wave (Halcon)

### Claude Code Agent Teams (documentado, experimental en 2026)

```
Estado: EXPERIMENTAL — requiere CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1

Arquitectura:
├── Team Lead        → sesión principal, coordina, asigna tasks
├── Teammates        → sesiones Claude Code independientes, cada uno con su context window
├── Task List        → lista compartida con file locking (previene race conditions)
└── Mailbox          → mensajería directa entre agentes (no solo lead→worker)
```

**Diferencia arquitectónica crítica vs subagentes**:
- **Subagentes**: solo reportan resultados al agente principal
- **Agent Teams**: teammates se comunican directamente entre ellos (many-to-many)

**Display modes** (documentado):
```bash
# In-process: todos en terminal principal, Shift+Down para navegar
claude --teammate-mode in-process

# Split panes: cada teammate en su propio pane (requiere tmux o iTerm2)
claude --teammate-mode tmux

# Auto (default): usa split panes si ya en tmux session, otherwise in-process
```

**Coordinación** (documentado):
- Task claiming con file locking para prevenir race conditions
- Dependencies automáticas: completed task unbloquea dependientes sin intervención manual
- `broadcast` → mensaje a todos los teammates simultáneamente
- Plan approval workflow: teammate en read-only plan mode hasta aprobación del lead
- `TeammateIdle` hook → gate de calidad antes de que teammate se duerma
- `TaskCompleted` hook → gate de calidad antes de marcar task completado

**Almacenamiento** (documentado):
```
~/.claude/teams/{team-name}/config.json     → team config (members array)
~/.claude/tasks/{team-name}/               → task list con status
~/.claude/projects/{project}/{sessionId}/subagents/agent-{agentId}.jsonl → transcripts
```

**Limitaciones conocidas** (documentado):
- `/resume` y `/rewind` no restauran in-process teammates
- Task status puede lagear (teammate falla en marcar completed)
- Shutdown puede ser lento (espera request/tool call actual)
- Una team por sesión
- No nested teams
- Lead es fijo (no se puede promover teammate a lead)
- Split panes NO soportado en VS Code terminal, Windows Terminal, Ghostty

### Halcon — Orquestador wave-based (confirmado en código)

El orchestrator de Halcon (`orchestrator.rs:172-400`) es más determinista pero menos flexible:

```rust
pub async fn run_orchestrator(
    tasks: Vec<SubAgentTask>,  // definidos por el agente principal
    // No hay comunicación entre sub-agentes
    // SharedContextStore solo lee output previo, no envía mensajes
) -> Result<OrchestratorResult>
```

**Coordinación**: exclusivamente hub-and-spoke. El agente principal define las tareas, el orquestador las ejecuta en waves, los resultados regresan al principal. No hay comunicación peer-to-peer.

### Resumen Agent Teams vs Halcon Orchestrator

| | Claude Code Agent Teams | Halcon Orchestrator |
|---|---|---|
| **Comunicación** | Many-to-many (mailbox) | Hub-and-spoke (solo resultados al lead) |
| **Coordinación** | Shared task list + self-claim | Wave topológica centralizada |
| **Race conditions** | File locking en task claiming | No aplica (waves seriales) |
| **Dependency tracking** | Automático por task list | Explícito `depends_on: Vec<Uuid>` |
| **Budget** | Por sesión separada | SharedBudget atómico |
| **Status** | Experimental | Estable |
| **Nested teams** | No | No |
| **Model por worker** | Configurable (haiku/sonnet/opus) | No (mismo que parent) |
| **Context isolation** | Total (cada teammate = sesión propia) | Total (cada sub-agent = run_agent_loop separado) |

---

## 6. Sistema de Memoria y Contexto

### Claude Code — 3 capas de memoria (documentado)

#### Capa 1: CLAUDE.md — Instrucciones persistentes escritas por el usuario

**4 scopes con prioridad jerárquica** (documentado en `memory`):

```
Managed policy (IT/DevOps):
  macOS:   /Library/Application Support/ClaudeCode/CLAUDE.md
  Linux:   /etc/claude-code/CLAUDE.md
  Windows: C:\Program Files\ClaudeCode\CLAUDE.md
  → No puede ser excluido por settings individuales

Project instructions (equipo):
  ./CLAUDE.md  o  ./.claude/CLAUDE.md
  → Shared via source control

User instructions (personal):
  ~/.claude/CLAUDE.md
  → Aplica a todos tus proyectos

Local instructions (personal + proyecto, no en git):
  ./CLAUDE.local.md
  → Automáticamente en .gitignore
```

**`.claude/rules/` — Reglas por path** (documentado):

```markdown
---
paths:
  - "src/api/**/*.ts"
  - "lib/**/*.ts"
---

# API Development Rules
- All API endpoints must include input validation
```

Rules sin `paths` se cargan al inicio. Rules con `paths` se cargan cuando Claude trabaja con archivos matching.

**Límites recomendados**: ≤200 líneas por CLAUDE.md. Más líneas = menos adherencia + más tokens consumidos.

**Imports** (documentado): `@path/to/file` expande e inyecta el archivo referenciado. Máx 5 niveles de recursión.

#### Capa 2: Auto Memory — Notas que Claude escribe solo

```
Ubicación: ~/.claude/projects/<project>/memory/
├── MEMORY.md        → index (primeras 200 líneas cargadas cada sesión)
├── debugging.md     → patterns de debugging descubiertos
├── api-conventions.md
└── ...

Scope: por git repo (todos los worktrees comparten el mismo directorio)
Machine-local: NO compartido entre máquinas o cloud environments
```

**Activación**: on por defecto. Toggle con `/memory` o `autoMemoryEnabled: false` en settings.

**Qué guarda Claude** (documentado): build commands, debugging insights, architecture notes, code style preferences, workflow habits. Claude decide qué vale la pena recordar.

#### Capa 3: Agent Memory — Memoria de subagentes

```yaml
# En frontmatter del agente
memory: user     # ~/.claude/agent-memory/<name>/
memory: project  # .claude/agent-memory/<name>/  (compartible vía git)
memory: local    # .claude/agent-memory-local/<name>/  (no en git)
```

Las primeras 200 líneas de `MEMORY.md` del agente se inyectan al inicio de cada sesión del subagente.

### Halcon — 5 tiers de contexto + SQLite (confirmado en código)

**ContextPipeline** (`halcon-context/src/pipeline.rs:60-73`):

```rust
pub struct ContextPipeline {
    accountant: TokenAccountant,    // budget tracking contra max_context_tokens
    l0: HotBuffer,                  // N mensajes más recientes (default: 8)
    l1: SlidingWindow,              // ventana deslizante de segmentos recientes
    l2: ColdStore,                  // compresión agresiva (max 100 entradas)
    l3: SemanticStore,              // índice semántico (max 200 entradas)
    l4: ColdArchive,               // archivo frío de largo plazo (max 500 entradas)
    elider: ToolOutputElider,       // budget por tool output → truncación
    instruction_cache: InstructionCache, // content-hash invalidation
}

// Defaults:
max_context_tokens: 200_000
hot_buffer_capacity: 8
default_tool_output_budget: 2_000 tokens
max_cold_entries: 100
max_semantic_entries: 200
max_archive_entries: 500
```

**Persistencia SQLite** (`halcon-storage`): 16 tablas incluyendo sesiones, tool metrics, trace steps, invocation metrics, reflection data, episode links.

**Compactación explícita**: `ContextCompactor` se activa por K5-2 token growth violation counter (no automático por porcentaje). Usa LLM call separado.

### Comparación de Sistemas de Memoria

| Dimensión | Claude Code 2026 | Halcon v0.3.0 |
|---|---|---|
| **Instrucciones usuario** | CLAUDE.md (4 scopes, jerarquía, org-level) | No equivalente |
| **Auto-learning** | Auto memory (Claude escribe sus notas) | No equivalente |
| **Memoria de agente** | `memory: user/project/local` en frontmatter | No (solo sesión) |
| **Tiers de contexto** | 1 tier (flat + compaction automática ~95%) | 5 tiers (L0-L4) + TokenAccountant |
| **Retrieval semántico** | No (flat window o compaction) | L3 SemanticStore (max 200 entradas) |
| **Persistencia cross-session** | MEMORY.md (file-based) | SQLite (16 tablas, estructurado) |
| **Tool output management** | Full output inyectado (sin elision) | ToolOutputElider con budget por tool |
| **Sharing** | CLAUDE.md vía git, agent memory vía scope | SQLite local (no compartible) |
| **Organization-wide** | Managed policy CLAUDE.md (IT deploy) | No equivalente |
| **Reglas por path** | `.claude/rules/*.md` con `paths:` frontmatter | No equivalente |

---

## 7. Hooks y Automatización de Ciclo de Vida

### Claude Code — Sistema de Hooks (documentado, `hooks` reference)

**8 eventos de hooks** (documentado):

| Evento | Cuándo dispara | Matcher input |
|---|---|---|
| `PreToolUse` | Antes de que Claude use una tool | Nombre de tool |
| `PostToolUse` | Después de que Claude usa una tool | Nombre de tool |
| `Stop` | Cuando Claude va a parar | — |
| `SubagentStart` | Cuando un subagente comienza | Nombre del agent type |
| `SubagentStop` | Cuando un subagente termina | Nombre del agent type |
| `TeammateIdle` | Cuando un teammate está a punto de dormirse | — |
| `TaskCompleted` | Cuando una task está siendo marcada como completa | — |
| `InstructionsLoaded` | Cuando instruction files son cargados | — |

**Exit codes significativos** (documentado):
```
exit 0 → éxito, continúa normalmente
exit 1 → error no-fatal, Claude ve el error pero continúa
exit 2 → BLOCK: bloquea la operación y devuelve error a Claude (PreToolUse)
         o bloquea la transición (TeammateIdle, TaskCompleted) y envía feedback
```

**Tipos de hooks** (documentado):
```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "./scripts/validate.sh"
          }
        ]
      }
    ]
  }
}
```

**Input a los hooks** (JSON via stdin):
```json
{
  "tool_name": "Bash",
  "tool_input": { "command": "rm -rf /tmp/test" },
  "session_id": "abc123"
}
```

**Hook en subagente** (combinación documentada):
```yaml
---
name: db-reader
tools: Bash
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: command
          command: "./scripts/validate-readonly-query.sh"
---
```

**Caso de uso concreto** (documentado): Permitir Bash pero bloquear solo SQL writes:
```bash
#!/bin/bash
INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')
if echo "$COMMAND" | grep -iE '\b(INSERT|UPDATE|DELETE|DROP|CREATE|ALTER)\b' > /dev/null; then
  echo "Blocked: Only SELECT queries allowed" >&2
  exit 2
fi
exit 0
```

### Halcon — Sin hooks de ciclo de vida para el usuario (confirmado)

Halcon tiene hooks internos (guardrails, supervisor checks) pero **no expone un sistema de hooks al usuario**. Los equivalentes son:

- Guardrails internos: `Box<dyn halcon_security::Guardrail>` — implementados en Rust, no configurables sin compilar
- FASE-2 gate: validación de paths + CATASTROPHIC_PATTERNS — hardcoded
- Plugin registry: `plugin_registry.rs` — API para plugins en Rust, `None` por defecto en todas las configs conocidas

**Brecha crítica**: Claude Code permite a un developer de Python/Bash interceptar cualquier tool call con un script shell. En Halcon, hacer lo equivalente requiere escribir un Guardrail en Rust y recompilar.

---

## 8. Integración MCP — Model Context Protocol

### Claude Code — MCP como ciudadano de primer nivel (documentado)

**3 tipos de transporte** (documentado, `mcp`):
```bash
# HTTP (recomendado para servidores cloud)
claude mcp add --transport http notion https://mcp.notion.com/mcp

# SSE (deprecated, usar HTTP)
claude mcp add --transport sse asana https://mcp.asana.com/sse

# stdio (servidores locales)
claude mcp add --transport stdio airtable \
  --env AIRTABLE_API_KEY=YOUR_KEY \
  -- npx -y airtable-mcp-server
```

**3 scopes de MCP** (documentado):

| Scope | Ubicación | Uso |
|---|---|---|
| `local` (default) | `~/.claude.json` bajo el proyecto | Personal, proyecto específico |
| `project` | `.mcp.json` en raíz del proyecto | Equipo, via git |
| `user` | `~/.claude.json` global | Personal, todos los proyectos |

**Gestión** (documentado):
```bash
claude mcp list              # lista todos
claude mcp get github        # detalle de uno
claude mcp remove github     # eliminar
claude mcp add-json ...      # add desde JSON
claude mcp add-from-claude-desktop  # importar desde Claude Desktop
/mcp                         # status dentro de Claude Code + OAuth auth
```

**OAuth 2.0 integrado** (documentado):
- Tokens guardados en keychain, refresh automático
- `--callback-port` para fixed OAuth callback port
- `--client-id` / `--client-secret` para pre-configured OAuth credentials
- `authServerMetadataUrl` override para OIDC endpoint no-standard

**MCP Tool Search** (documentado, nuevo en 2026):
```
Problema: muchos MCP servers → tool definitions consumen >10% del context window
Solución: deferred tool loading + search tool para descubrir tools on-demand

# Auto (default): activa cuando tools > 10% del contexto
ENABLE_TOOL_SEARCH=auto

# Custom threshold
ENABLE_TOOL_SEARCH=auto:5    # activa al 5%

# Always on
ENABLE_TOOL_SEARCH=true

# Off
ENABLE_TOOL_SEARCH=false
```

**MCP dinámico** (documentado): `list_changed` notifications → Claude Code refresca tools sin desconectar.

**Managed MCP** (enterprise, documentado):
```
Option 1: managed-mcp.json en directorio system-wide → control exclusivo
  macOS: /Library/Application Support/ClaudeCode/managed-mcp.json
  Linux: /etc/claude-code/managed-mcp.json

Option 2: allowedMcpServers / deniedMcpServers en settings → allowlist/denylist
  Por serverName, serverCommand (exact match), o serverUrl (wildcards)
```

**Claude Code como servidor MCP** (documentado):
```bash
claude mcp serve  # expone tools de Claude Code a otras apps via stdio
```

**Servidores MCP populares** (documentado, lista verificada en `api.anthropic.com/mcp-registry`):
GitHub, Sentry, Notion, Slack, PostgreSQL, Jira, Figma, HubSpot, Stripe, PayPal, Playwright, etc.

**Límites de output MCP** (documentado):
```
Advertencia: >10,000 tokens en output de tool MCP
Default max: 25,000 tokens
Custom: MAX_MCP_OUTPUT_TOKENS=50000
```

### Halcon — MCP básico (código: `halcon-mcp` crate, 8 archivos, ~2,250 LOC)

El crate `halcon-mcp` existe pero no está documentado públicamente. Basado en el código:
- Soporte básico de protocolo MCP
- Gestión de conexiones a servidores MCP
- Integrado como `mcp_manager.rs` en el módulo `repl`
- Sin OAuth built-in, sin MCP Tool Search, sin gestión de scopes

**Brecha masiva**: Claude Code tiene un ecosistema MCP activo con 100+ servidores verificados, OAuth, Tool Search, managed enterprise config, y Claude Code actuando como servidor MCP. Halcon tiene un cliente MCP funcional pero sin ecosistema.

---

## 9. Selección y Ejecución de Herramientas

### Claude Code — Todas las tools, siempre (confirmado + documentado)

**Todas las tools presentes en cada invocación** (confirmado desde este mismo session):
- No hay filtrado por intent
- El modelo ve todas las tools disponibles en cada turno
- El modelo selecciona basándose en razonamiento en lenguaje natural

**Tools built-in** (documentado en `settings#tools-available-to-claude`):
```
Read, Write, Edit, MultiEdit, Bash, Glob, Grep, LS, WebFetch, WebSearch,
TodoRead, TodoWrite, Agent, AskUserQuestion, ExitPlanMode, EnterPlanMode,
NotebookRead, NotebookEdit, mcp__<server>__<tool>
```

**Restricción de tools** (documentado):
```bash
# Allowlist — solo estas tools
claude --tools "Bash,Edit,Read"

# Disable all
claude --tools ""

# Denylist — remover estas tools del contexto
claude --disallowedTools "Bash(git log *)" "Edit"
```

**Parallel tool calls**: Claude emite múltiples tool calls en un bloque → infraestructura ejecuta en paralelo.

**Permission prompt por tool** (documentado):
```bash
# Sin prompts para estas operaciones específicas
claude --allowedTools "Bash(git log *)" "Bash(git diff *)" "Read"
```

### Halcon — Selección por intent + CORE_TOOLS siempre (confirmado en código)

**Intent classification** (confirmado en `tool_selector.rs`):

```rust
// CORE_TOOLS siempre incluidas (confirmado como CORE_RUNTIME_TOOLS en agent/mod.rs)
const CORE_TOOLS: &[&str] = &["file_read", "bash", "grep"];

// 5 intents con sus tools
enum TaskIntent {
    FileOperation,    // ["file_read", "file_write", "file_edit", "file_delete", "file_inspect", "directory_tree"]
    CodeExecution,    // ["bash", "background_start", "background_output", "background_kill"]
    Search,           // ["grep", "glob", "fuzzy_find", "symbol_search"]
    GitOperation,     // ["git_status", "git_diff", "git_log", "git_add", "git_commit"]
    WebAccess,        // ["web_search", "web_fetch", "http_request"]
    Conversational,   // sin tools (solo saludo/chat)
    Mixed,            // todas las tools
}
```

**Clasificación por keywords** (confirmado): matching de inglés + español en listas hardcoded. `Mixed` cuando múltiples intents detectados.

**Ejecutor paralelo** (confirmado en `executor.rs:70-100`):
```rust
// ReadOnly → parallel_batch (join_all)
// ReadWrite/Destructive → sequential_batch (uno a la vez)
let can_parallel = tool.permission_level() == PermissionLevel::ReadOnly;
```

**FASE-2 Security Gate** (confirmado en executor.rs):
- Path validation antes de file tools
- CATASTROPHIC_PATTERNS (18 patrones) de `halcon-core::security`
- DANGEROUS_COMMAND_PATTERNS (12 patrones G7)

**Idempotency Registry** (confirmado): deduplication de tool calls idénticos dentro de sesión.

**Retry con backoff** (confirmado): `ToolRetryConfig` configurable por tool.

**Guardrail scan** (confirmado): `Box<dyn Guardrail>` scan de outputs ANTES de inyección al modelo.

### Comparación de Ejecución de Tools

| Dimensión | Claude Code | Halcon |
|---|---|---|
| **Filtrado de tools** | Ninguno (todas visible siempre) | Intent-based (ahorra ~60-70% tokens en tareas focalizadas) |
| **Paralelismo** | Batch en un turno → infra paralela | join_all para ReadOnly, secuencial para Destructive |
| **Security gate** | Permission prompts por modo | FASE-2: regex + path validation + catastrophic patterns |
| **Idempotency** | Ninguna | IdempotencyRegistry cross-session |
| **Retry** | Ninguno built-in | ToolRetryConfig con exponential backoff |
| **Output scan** | Ninguno | Guardrail scan antes de inyección al modelo |
| **Tool aliasing** | Ninguno (nombres canónicos) | `tool_aliases::canonicalize()` en 3+ call sites |
| **61 tools** | No (herramientas focalizadas) | Sí: file, git, bash, web, code analysis, docker, etc. |
| **Custom tools** | Via MCP servers | Implementando Guardrail trait en Rust |

---

## 10. Planificación y Control de Convergencia

### Claude Code — Plan Mode (documentado)

**Plan Mode** (documentado en `settings`, `common-workflows`):
```bash
claude --permission-mode plan
```

En plan mode:
- Claude solo puede leer (no escribir/ejecutar)
- Cuando se activa el subagente `Plan`, investiga el codebase (solo lectura)
- Claude presenta el plan al usuario para aprobación
- Usuario aprueba → Claude ejecuta con permisos normales

**Subagentes no pueden spawnar otros subagentes** — previene nesting infinito.

**Sin plan persistente**: el plan es conversacional, no un `ExecutionPlan` serializado.

### Halcon — Plan-Execute-Reflect cycle (confirmado en código)

**Planner trait** (confirmado en `halcon-core/traits/planner.rs`):
```rust
pub trait Planner: Send + Sync {
    async fn plan(user_message: &str, available_tools: &[ToolDefinition]) -> Result<Option<ExecutionPlan>>;
    async fn replan(current_plan: &ExecutionPlan, failed_step_index: usize, error: &str, ...) -> Result<Option<ExecutionPlan>>;
    fn max_replans(&self) -> u32 { 3 }
}
```

**ExecutionPlan** (confirmado en código):
- `goal: String`, `steps: Vec<PlanStep>`, `requires_confirmation: bool`
- `replan_count: u32`, `parent_plan_id: Option<Uuid>`, `mode: ExecutionMode`
- `capability_descriptor: CapabilityDescriptor` → graph-based cost estimation
- `blocked_tools: Vec<String>`, `requires_evidence: bool`

**ExecutionGraph + GraphValidator**: DAG de nodos, 4 reglas estructurales validadas antes de ejecución.

**ExecutionTracker**: `TaskStatus` FSM (Pending→Running→Completed/Failed/Skipped/Cancelled). Un step NO puede ir de Pending a Completed sin pasar por Running.

**Replanning triggers** (confirmado en `termination_oracle.rs`):
- `ConvergenceAction::Replan` (stagnation detectada)
- `LoopAction::ReplanRequired` (read saturation + 0% plan completion)
- `RoundScorer.should_trigger_replan()` (persistent low trajectory)

**StrategySelector UCB1** (confirmado en `domain/strategy_selector.rs`):
```
UCB1 score = avg_reward + sqrt(2 * log(total_pulls) / arm_pulls)
Arms: DirectExecution vs PlanExecuteReflect
```

---

## 11. Seguridad y Permisos

### Claude Code — Permission Modes (documentado)

**5 modos de permiso** (documentado):

| Modo | Comportamiento |
|---|---|
| `default` | Prompts interactivos para tools peligrosas |
| `acceptEdits` | Auto-acepta edits de archivos |
| `dontAsk` | Auto-deniega permission prompts (excepto tools explícitamente allowlisted) |
| `bypassPermissions` | Skip todos los permission checks |
| `plan` | Read-only exploration mode |

```bash
claude --permission-mode plan
claude --permission-mode acceptEdits
claude --dangerously-skip-permissions  # alias de bypass
```

**Permission rules** (documentado en settings):
```json
{
  "permissions": {
    "allow": ["Bash(git log *)", "Bash(git diff *)", "Read"],
    "deny": ["Agent(Explore)", "Bash(rm *)"]
  }
}
```

**Managed policy** (enterprise, documentado):
```json
// /Library/Application Support/ClaudeCode/managed-settings.json
{
  "allowedMcpServers": [...],
  "deniedMcpServers": [...],
  "disallowedTools": [...]
}
```

**PreToolUse hooks como security gate** (documentado): scripts shell pueden bloquear cualquier tool call antes de ejecución, con mensajes de error enviados al modelo.

### Halcon — Defense in depth (confirmado en código)

**FASE-2 Security Gate** (confirmado):
- Path existence check antes de file tools
- `CATASTROPHIC_PATTERNS` (18 regexes) de `halcon-core::security`
- `DANGEROUS_COMMAND_PATTERNS` (12 patrones G7)
- Error incluye working directory + sugerencia de exploración alternativa

**TBAC** (Tool-Based Access Control): confirmado en `LoopState.tbac_pushed` — tracking de push de permisos basado en tools.

**Circuit breakers** (confirmado en `failure_tracker.rs`):
- "do not exist"/"does not exist" → patrón "not_found"
- Tool trip después de N failures
- Recovery directive inyectada cuando circuit breaker trips

**Command blacklist** (confirmado): dos fuentes unificadas en `halcon-core::security`:
- `command_blacklist.rs` — runtime check para bash
- `bash.rs` — validación de comandos bash individuales

**supervisor.rs** (confirmado):
- Gate 1: hint para explorar primero, luego usar `file_inspect` con paths verificados
- Gate 2: `is_file_read_capable()` acepta `file_inspect` como sustituto

---

## 12. Extensibilidad y Ecosistema

### Claude Code — 4 mecanismos de extensibilidad (documentado)

**1. Skills (Slash Commands personalizados)**:
```markdown
# ~/.claude/skills/review-pr.md
---
name: review-pr
description: Reviews a pull request for code quality
---

Review the current PR changes for security vulnerabilities,
performance issues, and code style...
```

**2. Plugins** (documentado en `plugins`):
```
Plugin = bundle de:
├── agents/     → subagentes
├── skills/     → slash commands
├── mcp-servers → servidores MCP
└── plugin.json → manifest
```

**3. MCP Servers** (documentado): cualquier tool externa via protocolo estándar.

**4. Agent SDK** (documentado en `platform.claude.com/docs/en/agent-sdk`):
- API programática para construir agentes custom con tools y capabilities de Claude Code
- Control completo sobre orchestración, tool access, permisos
- Structured outputs con JSON Schema: `--json-schema '{"type":"object",...}'`
- Input format: `--input-format stream-json`
- Output format: `--output-format json|stream-json|text`

### Halcon — Extensibilidad por compilación

- **Tools**: implementar `ToolDefinition` + registrar en `ToolRegistry` → compilar
- **Guardrails**: implementar `Box<dyn Guardrail>` → compilar
- **Providers**: implementar `ModelProvider` trait → compilar
- **Planners**: implementar `Planner` trait → compilar
- **Plugins**: `plugin_registry.rs` existe pero está `None` por defecto — sin ecosistema activo

**Plugin system** (código): API bien diseñada en Rust para pre/post invoke gates, cost tracking, circuit breakers. Pero sin marketplace, sin distribución, sin documentación pública.

---

## 13. CLI — Referencia de Comandos Comparada

### Claude Code — Comandos y Flags (documentado)

**Comandos principales**:
```bash
claude                          # sesión interactiva
claude "query"                  # con prompt inicial
claude -p "query"               # print mode (no interactivo), exit al terminar
claude -c                       # continuar conversación más reciente
claude -r "session-name"        # reanudar por ID o nombre
claude --remote "task"          # crear sesión web en claude.ai
claude --teleport               # reanudar sesión web en terminal local
claude agents                   # listar todos los subagentes configurados
claude mcp                      # gestionar MCP servers
claude auth login/logout/status # autenticación
claude update                   # actualizar a última versión
claude remote-control           # controlar desde Claude.ai mientras corre localmente
```

**Flags más relevantes** (documentado, todos verificados):
```bash
--add-dir ../apps ../lib        # directorios adicionales accesibles
--agent my-custom-agent         # especificar agente para la sesión
--agents '{...json...}'         # definir subagentes via JSON
--allowedTools "Bash(git *)"    # tools sin prompt de permiso
--disallowedTools "Edit"        # remover tools del contexto
--append-system-prompt "..."    # agregar al system prompt
--system-prompt "..."           # reemplazar system prompt completo
--betas interleaved-thinking    # beta headers API
--chrome                        # habilitar Chrome browser integration
--continue / -c                 # continuar conversación reciente
--dangerously-skip-permissions  # skip todos los permission checks
--debug "api,mcp"              # debug con filtro de categorías
--disable-slash-commands        # deshabilitar skills y commands
--fallback-model sonnet         # modelo fallback cuando principal está overloaded
--fork-session                  # nueva session ID al reanudar
--from-pr 123                   # reanudar sesiones vinculadas a PR
--ide                           # conectar automáticamente al IDE
--include-partial-messages      # streaming events parciales
--input-format stream-json      # formato de input (print mode)
--json-schema '{...}'          # output JSON estructurado validado
--max-budget-usd 5.00          # presupuesto máximo USD (print mode)
--max-turns 3                   # límite de turns agénticos (print mode)
--mcp-config ./mcp.json        # cargar MCP servers desde JSON
--model claude-sonnet-4-6       # modelo específico o alias (sonnet/opus)
--no-session-persistence        # no guardar sesión en disco
--output-format json            # formato de output (print mode)
--permission-mode plan          # modo de permisos
--permission-prompt-tool mcp_tool # tool para permission prompts no-interactivo
--plugin-dir ./my-plugins       # cargar plugins desde directorio
--print / -p                    # print mode sin sesión interactiva
--remote "Fix the bug"          # nueva sesión web desde CLI
--resume / -r "session"         # reanudar sesión por ID o nombre
--session-id "uuid"             # usar session ID específico
--setting-sources user,project  # fuentes de settings a cargar
--settings ./settings.json      # settings adicionales
--strict-mcp-config             # solo MCP de --mcp-config
--system-prompt-file ./prompt   # system prompt desde archivo
--teleport                      # traer sesión web al terminal
--teammate-mode in-process|tmux # modo de display para agent teams
--tools "Bash,Edit,Read"        # restricción de tools disponibles
--verbose                       # logging completo turn-by-turn
--worktree / -w feature-auth    # iniciar en git worktree aislado
```

### Halcon — Comandos y Flags (confirmado en código)

```bash
halcon chat                     # sesión REPL interactiva (comando principal)
halcon agent "task"             # invocación no-interactiva single-shot
halcon auth login/logout/status # gestión de API keys via OS keyring
halcon config get/set/list      # gestión de configuración
halcon init                     # inicialización del proyecto
halcon doctor                   # health check del sistema (15+ checks)
halcon tools list/info/test     # inspección del registry de tools
halcon theme set/list/preview   # gestión de themes del terminal
halcon plugin install/list/remove # gestión de plugins
halcon --model claude-3-5-sonnet # modelo específico
halcon --provider anthropic/ollama # proveedor específico
halcon --tui                    # activar TUI ratatui
halcon --headless               # sin terminal UI
halcon --no-plan                # skip planning phase
halcon --max-rounds N           # límite de rondas del loop
```

**Slash commands en sesión** (confirmado en `slash_commands.rs`):
```
/code       /edit       /agents     /memory
/checkpoint /help       /plan       /review
/debug      /explain    /refactor   /test
```

---

## 14. Métricas de Código y Complejidad

### Halcon — Métricas Exactas (confirmado con `wc -l` y análisis de código)

```
Total LOC:          306,090 líneas (.rs files)
Total crates:       19 workspace crates
Tests que pasan:    4,140 (halcon-cli) + 4,344 (total workspace)
```

**Archivos más grandes** (confirmado):
```
repl/agent/tests.rs              5,484 líneas
repl/mod.rs                      4,221 líneas
repl/executor.rs                 3,250 líneas
repl/agent/mod.rs                2,233 líneas
render/sink.rs                   2,207 líneas
render/theme.rs                  2,264 líneas
types/config.rs                  2,155 líneas
storage/migrations.rs            2,045 líneas
repl/agent/convergence_phase.rs  1,889 líneas
repl/orchestrator.rs             1,815 líneas
repl/model_selector.rs           1,776 líneas
repl/agent/provider_round.rs     1,470 líneas
repl/slash_commands.rs           1,440 líneas
repl/agent/post_batch.rs         1,334 líneas
repl/agent/loop_state.rs         1,331 líneas
repl/execution_tracker.rs        1,280 líneas
repl/reward_pipeline.rs          ~1,200 líneas (50KB)
```

**Deuda técnica cuantificada**:
```
#[allow(dead_code)]    116 ocurrencias
.unwrap() en agent/    93 (86 en tests, resto producción)
TODO/FIXME markers     22
Phase annotations      1–113 en agent/mod.rs (113 fases en 1 función)
Deps privadas path     3 (momoto-core/metrics/intelligence → stub en CI)
Sin sistema migración  16 tablas hardcodeadas en create_tables()
```

**God objects confirmados**:
- `AgentContext`: 44+ campos (struct de configuración pasado a todo)
- `LoopState`: 62+ campos públicos, 6 sub-structs embebidos, 140+ entradas pub totales
- `run_agent_loop()`: ~2,000 líneas, cyclomatic complexity >80, 8 concerns

### Claude Code — Métricas (inferido de documentación + comportamiento observable)

```
Instalación: single binary (~50-100MB estimado)
Sin configuración de DB requerida
Sin sistema de migración expuesto
Tests: no públicamente divulgados
Código fuente: no público
```

**Overhead de configuración**: Cero para uso básico. `CLAUDE.md` opcional. MCP servers opcionales.

---

## 15. Diferencias Algorítmicas Clave

### Algoritmo 1: Terminación del Loop

**Claude Code**:
```
next_token_prediction(context) → P(emit_response) vs P(emit_tool_call)
Si P(emit_response) > threshold → terminar
Sin oracle externo, sin precedencia explícita
```

**Halcon** (confirmado en `termination_oracle.rs`):
```
adjudicate(convergence_action, loop_signal, feedback) → TerminationDecision:
  Priority 1: Halt  (hard stop — convergence halt OR loop break)
  Priority 2: InjectSynthesis (convergence synthesize OR loop inject OR synthesis_advised)
  Priority 3: Replan (convergence replan OR loop replan OR replan_advised)
  Priority 4: ForceNoTools (loop ForceNoTools signal)
  Priority 5: Continue (default)
```

### Algoritmo 2: Selección de Herramientas

**Claude Code**: todas disponibles → modelo selecciona. O(model_inference) complejidad.

**Halcon**:
```
classify_intent(message) → TaskIntent en O(|message| × |keywords|)
tools_for_session = CORE_TOOLS ∪ intent_tools[intent]
Reducción: 61 tools → ~6-15 tools según intent
```

### Algoritmo 3: Gestión de Contexto

**Claude Code**: sliding window + auto-compaction a ~95% capacity. Gestionado por infraestructura, no configurable desde app.

**Halcon**:
```
add_message(msg):
  tokens = estimate_message_tokens(msg)
  tier = accountant.charge(tokens)
  match tier:
    L0: hot_buffer.push(msg)     → siempre en contexto
    L1: sliding_window.add(msg)  → compresión ligera
    L2: cold_store.compress(msg) → compresión agresiva
    L3: semantic_store.index(msg) → retrieval semántico
    L4: cold_archive.archive(msg) → archivo permanente
```

### Algoritmo 4: Orquestación de Sub-agentes

**Claude Code (Subagentes)**:
```
# Manual hub-and-spoke
parent_decides_parallelism() {
    if tasks_are_independent:
        spawn_multiple_agents_same_turn()
    else:
        chain_agents_sequentially()
}
# No dependency graph → O(developer_reasoning)
```

**Claude Code (Agent Teams)**:
```
# Shared task list con self-claim
teammates_claim_tasks() {
    while task_list.has_unclaimed():
        task = task_list.claim_with_file_lock()  # atomic
        execute(task)
        mark_completed(task)
        unblock_dependents(task)
}
```

**Halcon**:
```
# Topological sort automático
waves = topological_waves(tasks)  # O(V+E)
for wave in waves:
    if budget.is_over_budget(): break
    spawn_concurrent(wave)  # tokio::spawn por task
    await_wave_completion()
```

### Algoritmo 5: Selección de Estrategia

**Claude Code**: implícita en el modelo (entrenamiento).

**Halcon** (confirmado en `domain/strategy_selector.rs`):
```
UCB1: score = avg_reward + sqrt(2 * ln(total_pulls) / arm_pulls)
Arms: DirectExecution vs PlanExecuteReflect
→ Balanceo exploration-exploitation intra-sesión
```

---

## 16. Rendimiento y Eficiencia de Tokens

### Eficiencia de Tokens por Request

| Factor | Claude Code | Halcon |
|---|---|---|
| **Tool definitions** | Todas (estimado ~5K tokens para full set) | Intent-filtered (estimado ~1K-2K tokens para una intent) |
| **Tool output** | Full output inyectado | ToolOutputElider con budget de 2K tokens por tool |
| **Context overhead** | Auto-compaction → infraestructura gestiona | 5-tier + SemanticStore → control fino |
| **System prompt** | Fijo (Claude Code default) o `--system-prompt` | Dinámico: plan + directives + domain context |
| **MCP tool search** | Tool Search → deferred loading cuando >10% contexto | No aplica |

**Estimación de ahorro de tokens Halcon** para una sesión larga:
- Filtrado de tools: ~60-70% reducción de tool definitions
- Tool output elision: ~40% reducción de output tokens en tools verbosos
- SemanticStore: context histórico sin consumir window principal

### Latencia

| Escenario | Claude Code | Halcon |
|---|---|---|
| **Primera respuesta** | Baja (sin planning overhead) | Alta (planning = extra LLM call) |
| **Task multi-step** | Similar (reactive rounds) | Similar por round |
| **Multi-agent paralelo** | Wave latency = max(sub-agent) si mismo turno | Wave latency = max(wave tasks) |
| **Compactación** | Automática por infra, transparente | Explícita (LLM call separado = +latencia) |
| **Recuperación de error** | Siguiente round con nueva info | Circuit breaker + recovery directive + replan |

### Escalabilidad

**Claude Code escala con context window**: simple tasks=1-2 rounds, complex tasks=muchos rounds secuenciales, degradación cuando window se llena.

**Halcon escala con complejidad de task**: BoundaryEngine calibra el número de rounds por complejidad, Wave orchestrator paraleliza tareas independientes, SharedBudget escala a N agentes concurrentes.

---

## 17. Fortalezas y Debilidades

### Claude Code — Fortalezas

1. **Ecosistema MCP maduro**: 100+ servidores verificados, OAuth, Tool Search, managed enterprise config. Conecta Claude a cualquier herramienta sin escribir código.

2. **Subagentes declarativos**: definir un agente es escribir un archivo Markdown con YAML frontmatter. No requiere compilación, no requiere conocer Rust.

3. **Hooks como security/quality gates**: PreToolUse con exit code 2 es el mecanismo más simple posible para interceptar tool calls. Un script bash de 10 líneas puede implementar validación compleja.

4. **Agent Teams con comunicación peer-to-peer**: los teammates se mensajean directamente, permitiendo debate científico (hipótesis competitivas) que ningún sistema hub-and-spoke puede emular.

5. **Superficies**: Terminal, VS Code, JetBrains, Desktop, Web, Slack, Chrome, iOS. Un agente iniciado en el terminal puede continuarse desde el móvil.

6. **Auto memory**: Claude aprende preferencias y patrones sin que el usuario escriba nada. Memory files son Markdown editables.

7. **Skills y Plugins**: distribución de workflows reutilizables. Un equipo puede compartir `/review-pr` como un skill en `.claude/skills/`.

8. **Developer-first**: `--output-format json`, `--json-schema`, `--input-format stream-json` hacen a Claude Code composable en scripts y CI/CD.

9. **Path a la mejora**: el modelo mejora con training → todos los comportamientos mejoran sin tocar infraestructura.

### Claude Code — Debilidades

1. **Sin safety floor en ejecución autónoma**: no hay TerminationOracle, no hay circuit breakers, no hay LoopGuard. Un modelo confundido puede loopearse sin fin.

2. **Opacidad de decisiones**: no es posible saber por qué Claude decidió sintetizar en round 5 vs round 8. Sin DecisionTrace, sin RoundMetrics observables.

3. **Agent Teams es experimental**: limitaciones serias documentadas (no session resumption, task status lag, shutdown lento, sin nested teams).

4. **Sin planificación estructurada**: el plan está en conversación natural, no en un struct validable. Sin ExecutionTracker, sin TaskStatus FSM.

5. **Sin provider fallback**: una sesión usa un único provider. Si el provider falla, la sesión muere.

6. **Sin semantic retrieval**: una vez que el contenido sale del context window, desaparece. L3 SemanticStore no existe.

7. **Configuración de memoria limitada para teams**: auto memory es machine-local, no se comparte entre máquinas. Para teams distribuidos, CLAUDE.md es el único mecanismo compartido.

### Halcon — Fortalezas

1. **Reliability stack completo**: TerminationOracle (4 autoridades), ToolLoopGuard (oscillation + saturation + Bayesian), circuit breakers, GovernanceRescue, HICON MetacognitiveLoop. Un safety floor que no depende del modelo.

2. **Observabilidad total**: cada decisión es estructurada, logeada, y persistida. DecisionTrace, RoundMetrics (17 campos), MetricsCollector, SQLite trace. Un operador puede auditar por qué ocurrió cada decisión.

3. **Wave-based orchestration**: el grafo de dependencias + topological sort es el algoritmo correcto para paralelizar tareas dependientes. Claude Code Agent Teams lo hace manualmente.

4. **SharedBudget atómico**: `AtomicU64` con `Ordering::Release`/`Acquire` es concurrencia correcta. El presupuesto de tokens no puede ser excedido por race condition.

5. **5-tier context pipeline**: SemanticStore permite retrieval de contexto que ya no cabe en el context window. ToolOutputElider previene que outputs verbosos consuman el window.

6. **Plan estructurado con FSM**: `ExecutionTracker` con `TaskStatus` FSM garantiza que cada step pasa por Running antes de Completed. `GraphValidator` valida el DAG antes de ejecución.

7. **Multi-provider resilience**: `invoke_with_fallback()` con cadena de fallback providers. Si Anthropic está down, Halcon puede continuar con otro provider.

8. **BoundaryDecisionEngine**: 6-stage pipeline que pre-calibra el session antes de la primera invocación del modelo. El SLA y la política de convergencia son función de la complejidad del task.

9. **UCB1 strategy selection**: multi-armed bandit intra-sesión que aprende qué estrategia funciona mejor para este task específico.

### Halcon — Debilidades

1. **Solo terminal**: sin IDE extension, sin web, sin mobile. La adopción está artificialmente limitada.

2. **Sin hooks de ciclo de vida**: no hay forma de interceptar tool calls sin compilar Rust. La extensibilidad para usuarios es inexistente.

3. **Sin ecosistema MCP**: `halcon-mcp` existe pero sin 100+ servidores verificados, sin OAuth, sin Tool Search, sin managed enterprise config.

4. **Phase 113 problem**: `run_agent_loop()` tiene 113 fases embebidas en una función. Cada nueva feature añade más complejidad a este monolito.

5. **God objects**: `LoopState` (62+ fields), `AgentContext` (44+ fields). Alta cognitive load para contributors.

6. **Deps privadas como path**: los 3 crates de `momoto-*` usan `path = "../Zuclubit/..."`. CI crea stubs cuando el repo privado no está disponible, testeando un binario diferente al de producción.

7. **Sin sistema de memoria declarativa para usuarios**: CLAUDE.md, auto memory, `.claude/rules/` no tienen equivalente en Halcon. Los usuarios no pueden dar instrucciones persistentes sin editar archivos de configuración internos.

8. **Plugin system inactivo**: `plugin_registry.rs` es un buen diseño pero `None` por defecto. Sin marketplace, sin distribución, sin documentación para plugin authors.

---

## 18. Brechas Funcionales — Qué le falta a cada uno

### Features en Claude Code que NO tiene Halcon

| Feature | Impacto | Complejidad de implementar |
|---|---|---|
| `CLAUDE.md` (4 scopes, org-managed) | Alto | Medio (file loading) |
| Auto memory | Alto | Medio (LLM escribe sus notas) |
| Agent memory persistente (`memory: user/project`) | Alto | Medio (directory + MEMORY.md pattern) |
| Hooks de ciclo de vida (PreToolUse, PostToolUse, etc.) | Alto | Bajo (event system ya existe) |
| MCP con 100+ servidores + OAuth + Tool Search | Alto | Alto (ecosistema) |
| VS Code / JetBrains extension | Alto | Muy alto |
| Web sessions | Alto | Muy alto |
| Subagente declarativo (Markdown frontmatter) | Alto | Medio |
| Agent Teams peer-to-peer messaging | Medio | Alto |
| Skills / slash commands compartibles | Medio | Bajo |
| Plugins con distribución | Medio | Medio |
| Chrome extension | Bajo | Alto |
| Remote Control (mobile → terminal) | Bajo | Alto |
| `--json-schema` structured output | Bajo | Bajo |
| MCP resources con @-mentions | Bajo | Medio |
| `.claude/rules/` por path | Bajo | Bajo |
| `--worktree` git worktree isolation | Bajo | Bajo |
| `--from-pr` session vinculada a PR | Bajo | Bajo |

### Features en Halcon que NO tiene Claude Code

| Feature | Impacto | ¿Claude Code lo necesita? |
|---|---|---|
| TerminationOracle (4 autoridades) | Alto | No — LLM es más confiable |
| ToolLoopGuard (oscillation + Bayesian) | Alto | Parcialmente — `--max-turns` es proxy |
| Circuit breakers + recovery directives | Alto | Posiblemente útil en autónomo |
| 5-tier context pipeline + SemanticStore | Alto | Para sesiones muy largas |
| ToolOutputElider (budget por tool) | Medio | No — context window es grande |
| SharedBudget atómico multi-agent | Medio | No — budget tracking es menos crítico |
| BoundaryDecisionEngine (6-stage) | Medio | No — routing implícito en modelo |
| UCB1 strategy selection | Medio | No — entrenamiento cubre esto |
| ExecutionTracker con TaskStatus FSM | Medio | No — Agent Teams tiene task list |
| GraphValidator (4 reglas estructurales) | Bajo | No |
| ARIMA resource predictor | Bajo | No |
| Multi-provider fallback chain | Bajo | Útil en enterprise |
| 61 tools built-in | Bajo | Parcialmente — MCP servers cubren muchos |
| Reward pipeline | Bajo | No |
| ContextCompactor explícito | Bajo | No — auto-compaction es mejor UX |

---

## 19. Insights Arquitectónicos Estratégicos

### Insight 1: Claude Code Apostó a la Plataforma

El movimiento estratégico más importante de Claude Code en 2026 no fue un feature técnico — fue la expansión a 8+ superficies y el ecosistema MCP. Al estar en VS Code, JetBrains, Web, iOS, y Slack simultáneamente, Claude Code se convirtió en la capa de inteligencia del flujo de trabajo completo del desarrollador, no solo del terminal.

Halcon está construyendo mejor infraestructura para un caso de uso que ya quedó suboptimamente posicionado (solo terminal).

### Insight 2: Los Hooks Democratizan la Seguridad

El sistema de hooks de Claude Code (`PreToolUse` con `exit 2`) es un insight de ingeniería profundo: en lugar de construir guardrails hardcoded en Rust que requieren compilación, el sistema delega la validación a scripts shell que cualquier desarrollador puede escribir. Un team de Python puede implementar validación compleja de comandos sin tocar el código de Claude Code.

Halcon tiene guardrails más sofisticados pero no accesibles. Claude Code tiene guardrails más simples pero democráticos.

### Insight 3: La Memoria como Producto

Claude Code 2026 convirtió la memoria en un producto completo:
- Managed policy CLAUDE.md (IT/DevOps)
- Project CLAUDE.md (equipo, via git)
- User CLAUDE.md (personal, todos los proyectos)
- Local CLAUDE.md (personal, un proyecto)
- `.claude/rules/` por path (contextual)
- Auto memory (Claude aprende solo)
- Agent memory (subagentes aprenden solos)

Halcon tiene SQLite para persistencia técnica y un context pipeline sofisticado, pero no hay concepto de "instrucciones persistentes del usuario" o "el agente aprende de sus errores". Esta es una brecha de producto, no solo técnica.

### Insight 4: Agent Teams Resuelve el Problema de Coordinación Correctamente

El insight de Agent Teams (vs subagentes) es el correcto para problemas de investigación y revisión: cuando múltiples agentes trabajan en el mismo problema desde ángulos diferentes, necesitan **debatir**, no solo reportar. Un subagente que encuentra "error de autenticación" reporta al lead, que puede o no saber si otro subagente encontró "error de base de datos" relacionado. Con Agent Teams, los teammates comparten activamente hallazgos y se contradicen entre sí.

Halcon's SharedContextStore permite que wave N lea outputs de wave N-1, pero no permite que sub-agentes concurrentes se comuniquen en tiempo real.

### Insight 5: La Brecha de Complejidad se Invirtió

En 2023-2024, la complejidad de Halcon (TerminationOracle, BoundaryDecisionEngine, UCB1) era necesaria porque los LLMs no eran lo suficientemente confiables para auto-regular su comportamiento. En 2026, con Claude Sonnet 4.6 y Opus 4.6, la confiabilidad del modelo ha aumentado significativamente. La complejidad de infraestructura de Halcon resuelve cada vez menos problemas reales mientras mantiene el mismo costo de mantenimiento.

El "Phase 113 problem" (113 fases en una función) es la manifestación de este patrón: cada nueva failure mode requirió una nueva pieza de infraestructura, y el modelo habría resuelto el mismo problema de forma más elegante con mejor entrenamiento.

### Insight 6: El Futuro es Híbrido

La arquitectura óptima para 2027 combinará:
- **LLM nativo** para razonamiento, estrategia, y adaptación (fortaleza de Claude Code)
- **Safety floors mínimos** para ejecución autónoma (fortaleza de Halcon)
- **Memoria semántica** como puente entre sesiones (ventaja actual de Halcon sobre Claude Code)
- **Ecosistema de herramientas** como commodidad (ventaja masiva de Claude Code via MCP)
- **Hooks declarativos** para extensibilidad (solo Claude Code)

---

## 20. Recomendaciones

### Para el equipo de Halcon

**Prioridad Alta** (3 meses):

1. **Implementar CLAUDE.md-equivalent**: sistema de instrucciones persistentes con scopes (proyecto, usuario, org). Es el feature de mayor impacto para retención de usuarios y es relativamente simple de implementar (file loading + hot reload).

2. **Exponer sistema de hooks al usuario**: `PreToolUse` y `PostToolUse` con exit codes. Permite que la comunidad construya validación personalizada sin compilar Rust. Usa el event system existente.

3. **Aumentar transparencia de Auto Memory**: implementar un mecanismo where Halcon guarda learnings automáticamente (análogo a Claude Code auto memory). Los usuarios no deberían tener que recordar qué comandos de build usar.

**Prioridad Media** (6 meses):

4. **VS Code extension**: la brecha de adopción más importante. El developer moderno trabaja en su IDE, no solo en el terminal.

5. **Publicar `momoto-*` crates en registry privado**: eliminar el problema de stubs en CI. Testear el mismo binario que se distribuye.

6. **Skills/Slash commands distribuibles**: permitir que teams compartan slash commands via git (`.halcon/skills/`).

7. **Agentes declarativos en Markdown**: reducir la barrera de crear agentes de "escribir Rust y compilar" a "escribir un archivo Markdown".

**Prioridad Baja** (12 meses):

8. **Ecosistema MCP**: sin 100+ servidores MCP verificados, la estrategia de extensibilidad via MCP no es competitiva.

9. **Refactorización de `run_agent_loop()`**: extraer las 8 concerns en funciones separadas (R1 del audit anterior). Es la deuda técnica más costosa en términos de velocidad de desarrollo futura.

10. **Deprecar `SignalArbitrator`**: eliminarlo en v0.4.0 según el deprecation notice existente.

---

## Tabla de Comparación Final

| Categoría | Claude Code 2026 | Halcon v0.3.0 | Ganador |
|---|---|---|---|
| **Ecosistema MCP** | 100+ servidores, OAuth, Tool Search, managed | Soporte básico | **Claude Code** |
| **Superficies** | Terminal, VS Code, JetBrains, Desktop, Web, iOS, Slack, Chrome | Terminal únicamente | **Claude Code** |
| **Sub-agentes** | Declarativo (Markdown), memoria persistente, hooks | Programático (Rust), sin memoria | **Claude Code** |
| **Agent Teams** | Peer-to-peer messaging, shared task list (experimental) | Wave topológica determinista | Empate (diferente uso) |
| **Memoria de usuario** | CLAUDE.md (4 scopes) + auto memory + agent memory | SQLite + context pipeline | **Claude Code** |
| **Hooks de ciclo de vida** | 8 eventos, scripts shell, exit codes semánticos | No disponible para usuarios | **Claude Code** |
| **Extensibilidad** | Skills + Plugins + MCP + Agent SDK | Plugin registry (inactivo) | **Claude Code** |
| **Safety en autónomo** | Sin safety floor (solo --max-turns) | TerminationOracle + circuit breakers + LoopGuard | **Halcon** |
| **Observabilidad** | Transcripts JSONL + sesiones persistentes | DecisionTrace + RoundMetrics + SQLite | **Halcon** |
| **Context management** | Flat window + auto-compaction | 5 tiers + SemanticStore + ToolOutputElider | **Halcon** |
| **Multi-provider** | Un provider por sesión | `invoke_with_fallback()` | **Halcon** |
| **Planificación** | Plan mode (read-only + aprobación) | ExecutionPlan + FSM + GraphValidator | **Halcon** |
| **Eficiencia de tokens** | Alta por MCP Tool Search, baja por no-filtering | Alta por intent-filtering + elision | **Halcon** |
| **Paralelismo** | Agent Teams (experimental) | Waves topológicas (estable, atómico) | **Halcon** |
| **Simplicidad de uso** | Alta (zero config para empezar) | Media (PolicyConfig, SLA tuning) | **Claude Code** |
| **Mantenibilidad** | Alta (model mejora = sistema mejora) | Baja (Phase 113 problem) | **Claude Code** |
| **Adaptabilidad a tareas nuevas** | Alta (reasoning general del modelo) | Media (clasificación por keywords) | **Claude Code** |
| **Ejecución autónoma sin supervisión** | Media (sin safety floor técnico) | Alta (multiple guardrails) | **Halcon** |
| **Deployment enterprise** | Managed policy, SSO, org-wide CLAUDE.md, managed MCP | Sin features enterprise específicos | **Claude Code** |

---

*Documento generado: Marzo 2026*
*Fuentes: `code.claude.com/docs` (verificado) + `crates/halcon-cli/src/` (código fuente v0.3.0)*
*Autor: Análisis de ingeniería basado en documentación oficial y código fuente*
