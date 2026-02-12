# Integración de Modelos Internos y Externos

**Proyecto:** Cuervo CLI
**Versión:** 1.0
**Fecha:** 6 de febrero de 2026

---

## 1. Catálogo de Modelos Soportados

### 1.1 Modelos Cloud (Externos)

| Provider | Modelos | Protocolo | Casos de Uso |
|----------|---------|-----------|-------------|
| **Anthropic** | Claude Opus 4.6, Sonnet 4.5, Haiku 4.5 | Messages API | Razonamiento complejo, multi-file editing, code review |
| **OpenAI** | GPT-4o, o1, o3, GPT-4o-mini | Chat Completions API | Coding general, razonamiento, multimodal |
| **Google** | Gemini 2.0 Pro, Ultra, Flash | Generative Language API | Long-context (1M tokens), multimodal |
| **DeepSeek** | DeepSeek V3, Coder V3 | OpenAI-compatible API | Coding especializado, costo bajo |
| **Mistral** | Mistral Large, Codestral | Chat API | Coding, multilingual |

### 1.2 Modelos Locales (Ollama)

| Modelo | Parámetros | VRAM Requerida | Caso de Uso |
|--------|-----------|----------------|-------------|
| Llama 3.1/4 8B | 8B | 6GB | Autocompletado rápido, tareas simples |
| Llama 3.1/4 70B | 70B | 40GB | Tareas complejas sin cloud |
| DeepSeek Coder V3 | 16B/33B | 12-24GB | Coding especializado |
| Qwen 2.5 Coder | 7B/14B/32B | 6-24GB | Coding, multilingual |
| CodeLlama 3 | 7B/13B/34B | 6-24GB | Código, instrucciones |
| Phi-4 | 3.8B | 4GB | Ultra-rápido, tareas triviales |
| Mistral/Mixtral | 7B/8x7B | 6-32GB | General purpose |

### 1.3 Modelos Custom (Fine-tuned)

```
PIPELINE:
1. Base model (Llama, Qwen, etc.) + training data del proyecto
2. LoRA/QLoRA fine-tuning
3. Evaluation vs baseline
4. Deploy a Ollama o endpoint custom
5. Register en model_registry de Cuervo CLI
```

---

## 2. Model Gateway Architecture

```
┌────────────────────────────────────────────────────────────┐
│                    MODEL GATEWAY                            │
├────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌─────────────────────────────────┐                       │
│  │        Router                    │                       │
│  │  (complexity classifier)         │                       │
│  └──────────────┬──────────────────┘                       │
│                  │                                           │
│  ┌───────────────┼───────────────────────┐                 │
│  │               │                       │                  │
│  ▼               ▼                       ▼                  │
│  ┌──────┐    ┌──────┐    ┌──────┐    ┌──────┐             │
│  │Adapter│    │Adapter│    │Adapter│    │Adapter│            │
│  │Claude │    │OpenAI │    │Gemini │    │Ollama │            │
│  └───┬───┘    └───┬───┘    └───┬───┘    └───┬───┘           │
│      │            │            │            │               │
│  ┌───▼───┐    ┌───▼───┐    ┌───▼───┐    ┌───▼───┐         │
│  │Anthropic    │OpenAI │    │Google │    │Local  │          │
│  │API     │    │API    │    │API    │    │Ollama │          │
│  └────────┘    └───────┘    └───────┘    └───────┘         │
│                                                             │
│  CROSS-CUTTING:                                             │
│  ├── Circuit breaker (per provider)                         │
│  ├── Rate limiting (per provider + global)                  │
│  ├── Token counting (Rust nativo vía tiktoken-rs)           │
│  ├── Cost tracking (per request)                            │
│  ├── Latency monitoring                                     │
│  ├── Retry logic (exponential backoff)                      │
│  ├── Request/response logging                               │
│  ├── PII redaction (Rust nativo vía regex SIMD)             │
│  └── Semantic caching                                       │
│                                                             │
└────────────────────────────────────────────────────────────┘
```

---

## 3. Protocolo Unificado de Comunicación

### 3.1 Request Normalization

Todos los providers son normalizados a un formato interno:

```typescript
interface UnifiedRequest {
  model: string;               // "claude-opus-4-6", "gpt-4o", "llama-3.1-8b"
  messages: UnifiedMessage[];
  tools?: UnifiedTool[];
  config: {
    maxTokens: number;
    temperature: number;
    stream: boolean;
    topP?: number;
    stopSequences?: string[];
  };
  metadata: {
    sessionId: string;
    agentType: AgentType;
    taskComplexity: ComplexityLevel;
    budgetRemaining: TokenBudget;
  };
}
```

### 3.2 Response Normalization

```typescript
interface UnifiedResponse {
  content: string;
  toolCalls?: UnifiedToolCall[];
  thinking?: string;           // Extended thinking (Claude, o-series)
  usage: {
    inputTokens: number;
    outputTokens: number;
    totalTokens: number;
    estimatedCostUSD: number;
  };
  metadata: {
    model: string;
    provider: string;
    latencyMs: number;
    finishReason: 'end_turn' | 'tool_use' | 'max_tokens' | 'stop';
  };
}
```

---

## 4. Gestión de Modelos

### 4.1 Model Registry

```yaml
# .cuervo/models.yml — Configuración de modelos del proyecto
models:
  default: "claude-sonnet-4-5"

  profiles:
    fast:
      model: "llama-3.1-8b"
      provider: "ollama"
      use_for: ["completion", "simple_edit", "search"]

    balanced:
      model: "claude-sonnet-4-5"
      provider: "anthropic"
      use_for: ["chat", "explain", "test_generation"]

    powerful:
      model: "claude-opus-4-6"
      provider: "anthropic"
      use_for: ["architecture", "complex_debug", "multi_file_refactor"]

    reasoning:
      model: "o3"
      provider: "openai"
      use_for: ["algorithmic", "optimization", "complex_logic"]

  routing:
    auto: true                # Selección automática basada en complejidad
    fallback_chain:
      - "anthropic"
      - "openai"
      - "ollama"

  budget:
    daily_limit_usd: 10.00
    per_request_max_usd: 2.00
    warn_threshold_usd: 0.50
```

### 4.2 Versionado y Lifecycle

```
┌────────┐    ┌─────────┐    ┌──────────┐    ┌────────────┐
│REGISTER│───▶│EVALUATE │───▶│ ACTIVE   │───▶│DEPRECATED  │
│        │    │(benchmark│    │(production│    │(sunset     │
│        │    │ testing) │    │ use)     │    │ scheduled) │
└────────┘    └─────────┘    └──────────┘    └────────────┘
                                                    │
                                                    ▼
                                              ┌────────────┐
                                              │  RETIRED   │
                                              │ (removed)  │
                                              └────────────┘
```

---

## 5. Plan de Integración con Plataformas Externas

### 5.1 Claude Code Patterns

Cuervo CLI adopta e integra patrones probados de Claude Code:
- **Tool-use paradigm**: LLM orquesta herramientas (read, write, edit, bash, glob, grep)
- **Sandboxed execution**: Bash sandboxed por defecto
- **Human-in-the-loop**: Confirmación para operaciones destructivas
- **Slash commands/Skills**: Sistema extensible de comandos
- **Subagent delegation**: Agentes especializados para tareas paralelas
- **Rust-accelerated tools**: Glob y grep nativos vía Rust (mismo patrón que ripgrep en VS Code)

### 5.2 OpenAI Codex Integration

- API compatible con formato OpenAI Chat Completions
- Soporte para function calling (tool use)
- Integration con o-series reasoning models para tareas algorítmicas

### 5.3 Integración con Herramientas Emergentes

| Herramienta | Tipo de Integración | Valor Agregado |
|-------------|--------------------|-|
| **Aider** | Inspiración de CLI UX patterns | Terminal-native UX patterns |
| **Continue.dev** | Plugin compatibility | Extensibilidad cross-IDE |
| **LangChain/LangGraph** | Opcional para pipelines complejos | Workflow orchestration |
| **MCP Protocol** | Server/Client implementation | Interoperabilidad de tools |

---

## 6. Token Counting Nativo (Rust Layer)

### 6.1 Problema

Cada provider usa tokenizers diferentes. El presupuesto de tokens, estimación de costos, y context management dependen de conteo preciso. Las implementaciones JavaScript de tokenizers son 5-10x más lentas que las nativas, lo cual es crítico para operaciones que se ejecutan en cada request.

### 6.2 Solución: tokenizer.rs

El módulo `tokenizer.rs` (Rust, compilado vía napi-rs) provee conteo de tokens nativo:

```typescript
// API expuesta al TypeScript
export function countTokens(text: string, provider: 'openai' | 'anthropic' | 'local'): number;
export function truncateToTokens(text: string, maxTokens: number, provider: string): string;
export function estimateTokens(text: string): number; // Heurística rápida ~5ns
```

**Implementación por provider:**

| Provider | Tokenizer Rust | Precisión | Latencia |
|----------|---------------|-----------|----------|
| OpenAI (GPT/o-series) | `tiktoken-rs` (cl100k_base, o200k_base) | Exacta | ~0.1ms/1K tokens |
| Anthropic (Claude) | Estimador calibrado (chars × factor) | ±5% | ~0.01ms |
| Google (Gemini) | Estimador calibrado | ±8% | ~0.01ms |
| Ollama (local) | `tiktoken-rs` (llama tokenizer) | ±3% | ~0.1ms |

**Fallback TypeScript:** Si el módulo nativo no está disponible, se usa `js-tiktoken` (WASM) con ~5-10x más latencia.

### 6.3 PII Detection en Pipeline de Modelos

Antes de enviar prompts a providers cloud, el módulo `pii.rs` escanea y redacta información sensible:

```typescript
// Integrado en Model Gateway como middleware pre-request
export function scanPII(text: string): PIIMatch[];
export function redactPII(text: string): string;

interface PIIMatch {
  type: 'email' | 'ip' | 'api_key' | 'credit_card' | 'ssn' | 'phone' | 'custom';
  start: number;
  end: number;
  confidence: number; // 0.0 - 1.0
}
```

**Performance:** El crate `regex` de Rust usa SIMD (AVX2/NEON) para matching paralelo, logrando ~5-20x sobre RegExp de JavaScript en textos largos. Esto permite escanear el prompt completo sin impacto perceptible en latencia.

---

*Documento de referencia técnica para el equipo de desarrollo.*
