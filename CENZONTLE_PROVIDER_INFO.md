# 🦜 Cenzontle Provider - Información Detallada

**Cenzontle** es un **agregador/proxy multi-proveedor** que conecta con múltiples APIs de IA a través de una interfaz unificada compatible con OpenAI.

---

## 🌐 Conexión del Provider

### Endpoint Principal
```
https://ca-cenzontle-backend.graypond-e35bfdd8.eastus2.azurecontainerapps.io
```

**Infraestructura:**
- Backend: Azure Container Apps
- Proxy: Cloudflare (DDoS protection, edge caching, WAF)
- CNAME alternativo: `https://api-cenzontle.zuclubit.com`

### Autenticación
- **Método:** JWT access token via OAuth 2.1 PKCE flow
- **SSO:** Zuclubit SSO
- **Login:** `halcon login cenzontle`
- **Almacenamiento:** OS keychain (macOS Keychain)
- **Token en uso:** ✅ Configurado (SSO keychain)

---

## 🤖 APIs Conectadas (14 Modelos)

Cenzontle actúa como **gateway unificado** para múltiples providers de IA:

### 1. **DeepSeek AI** (2 modelos)
| Modelo | Context | Output | Características |
|--------|---------|--------|-----------------|
| `deepseek-v3-2-coding` | 128K | 32K | Coding specialist, tools |
| `deepseek-r1-reasoning` | 163K | 32K | Reasoning mode |

**API Original:** https://api.deepseek.com

---

### 2. **Moonshot AI / Kimi** (2 modelos)
| Modelo | Context | Output | Características |
|--------|---------|--------|-----------------|
| `kimi-k2-5-longctx` | 262K | 16K | Ultra-long context |
| `kimi-k2-thinking` | 262K | 16K | Reasoning mode |

**API Original:** Moonshot AI (China)

---

### 3. **OpenAI** (Modelos próximos - GPT-5)
| Modelo | Context | Output | Características |
|--------|---------|--------|-----------------|
| `gpt-5-nano-fast` | 200K | 16K | Fast inference |
| `gpt-51-codex-mini` | 400K | 32K | Coding, tools (DEFAULT) |
| `gpt-4o` | Variable | Variable | Proxied GPT-4o |
| `gpt-4o-mini` | Variable | Variable | Proxied GPT-4o mini |

**API Original:** https://api.openai.com/v1

**Estado actual:**
- ⚠️ `gpt-4o`: DEGRADED (88% success rate)
- ⚠️ `gpt-4o-mini`: UNHEALTHY (33% success rate)
- ✅ `gpt-51-codex-mini`: OK (100% success, modelo por defecto)

---

### 4. **Google Gemini** (2 modelos)
| Modelo | Context | Output | Características |
|--------|---------|--------|-----------------|
| `gemini-2.0-flash` | 1M | 8K | Vision, tools |
| `gemini-2.0-flash-lite` | 1M | 8K | Vision, lightweight |

**API Original:** https://generativelanguage.googleapis.com

**Estado:** ✅ 100% success rate ambos

---

### 5. **Meta Llama** (3 modelos)
| Modelo | Context | Output | Características |
|--------|---------|--------|-----------------|
| `llama4-maverick` | 128K | 16K | Next-gen Llama 4 |
| `llama-3.3-70b-versatile` | 128K | 32K | General purpose |
| `llama-3.1-8b-instant` | 128K | 8K | Fast, small |

**API Original:** Meta AI / Groq

---

### 6. **Mistral AI** (3 modelos)
| Modelo | Context | Output | Características |
|--------|---------|--------|-----------------|
| `codestral-coding` | 256K | 32K | Coding specialist |
| `mistral-large-latest` | 128K | 8K | Flagship model |
| `mistral-small-latest` | 32K | 8K | Fast, economical |

**API Original:** https://api.mistral.ai

---

## 🔧 Configuración Técnica

### HTTP Settings
```toml
[models.providers.cenzontle.http]
connect_timeout_secs = 10
request_timeout_secs = 300
max_retries = 3
retry_base_delay_ms = 1000
```

### Features
- ✅ Streaming (todos los modelos)
- ✅ Tool calling (mayoría de modelos)
- ✅ Vision (Gemini modelos)
- ✅ Reasoning (DeepSeek R1, Kimi K2 thinking)
- ✅ Long context (hasta 1M tokens con Gemini)

---

## 💰 Costos

Cenzontle usa **pricing unificado** para simplificar la facturación:

| Tier | Input ($/1M tok) | Output ($/1M tok) | Modelos |
|------|------------------|-------------------|---------|
| **Ultra-cheap** | $0.10 | $0.10 | gpt-5-nano, llama-3.1-8b, gemini-lite |
| **Economy** | $0.30 | $1.00 | mistral-small |
| **Standard** | $1.00 | $5.00 | deepseek-v3, kimi, llama-3.3, codestral |
| **Premium** | $3.00 | $15.00 | deepseek-r1, mistral-large |

**Costo actual acumulado:** $0.52 (202 invocaciones)

---

## 📊 Estadísticas de Uso (desde tu sistema)

```
┌────────────────────────────────┬────────┬──────────┬─────────────┐
│ Modelo                         │ Calls  │ Success  │ Avg Latency │
├────────────────────────────────┼────────┼──────────┼─────────────┤
│ gpt-51-codex-mini (DEFAULT)    │   21   │  100%    │  18.6s      │
│ deepseek-r1-reasoning          │    3   │  100%    │  16.2s      │
│ kimi-k2-5-longctx              │    6   │  100%    │   6.8s      │
│ gemini-2.0-flash               │    6   │  100%    │   2.5s      │
│ gemini-2.0-flash-lite          │    3   │  100%    │   2.3s      │
│ deepseek-v3-2-coding           │    1   │  100%    │   4.2s      │
│ gpt-4o                         │   16   │   88%    │   8.4s      │
│ gpt-4o-mini                    │    9   │   33%    │   1.6s      │
└────────────────────────────────┴────────┴──────────┴─────────────┘

Total: 65 llamadas via Cenzontle
```

---

## 🎯 Ventajas de Cenzontle

### 1. **Acceso Unificado**
Un solo token de acceso para 14+ modelos de 6 providers diferentes.

### 2. **Simplicidad de Integración**
API compatible con OpenAI → fácil migración de código existente.

### 3. **Discovery Automático**
Los modelos se descubren dinámicamente desde `/v1/llm/models`.

### 4. **Gestión de Costos**
Pricing unificado y simplificado, sin necesidad de múltiples cuentas.

### 5. **Infraestructura Robusta**
- Cloudflare edge caching
- DDoS protection
- WAF (Web Application Firewall)
- Circuit breakers
- Backpressure handling

### 6. **Multi-región**
Azure Container Apps con despliegue en múltiples regiones.

---

## 🔍 Arquitectura del Provider

```
Halcon CLI
    ↓
CenzontleProvider (OpenAI-compatible)
    ↓
POST https://ca-cenzontle-backend.../v1/llm/chat
    ↓
Cenzontle Gateway (Azure + Cloudflare)
    ↓
┌─────────────┬─────────────┬─────────────┬─────────────┐
│ DeepSeek AI │ Moonshot AI │  Google     │   Meta      │
│   (China)   │   (China)   │  (Gemini)   │  (Llama)    │
└─────────────┴─────────────┴─────────────┴─────────────┘
┌─────────────┬─────────────┐
│ Mistral AI  │   OpenAI    │
│  (France)   │    (USA)    │
└─────────────┴─────────────┘
```

---

## 🚀 Uso Recomendado

### Modelo por Defecto (Actual)
```bash
# Ya configurado
halcon chat
# Usa: cenzontle/gpt-51-codex-mini (400K context, coding)
```

### Tareas de Reasoning
```bash
halcon chat -m deepseek-r1-reasoning
# 163K context, modo de razonamiento profundo
```

### Long Context (hasta 1M tokens)
```bash
halcon chat -m gemini-2.0-flash
# 1M context window, vision support
```

### Económico y Rápido
```bash
halcon chat -m gemini-2.0-flash-lite
# $0.10 por millón de tokens, 2.3s latencia
```

### Coding Specialist
```bash
halcon chat -m codestral-coding
# 256K context, optimizado para código
```

---

## 🔐 Autenticación SSO

### Flujo de Login
```bash
halcon login cenzontle
```

**Proceso:**
1. Abre navegador con Zuclubit SSO
2. Usuario se autentica (OAuth 2.1 PKCE)
3. Token JWT almacenado en macOS Keychain
4. Token auto-renovado por el CLI

**Estado actual:** ✅ Autenticado (token en keychain)

---

## 📈 Health Status

### Provider Health: ✅ OK
- Score: 100/100
- Error rate: 0%
- Timeout rate: 0%
- Average latency: 0ms (sin llamadas recientes)

### Individual Models
```
✅ Healthy (100% success):
   - gpt-51-codex-mini (21 calls)
   - deepseek-r1-reasoning (3 calls)
   - deepseek-v3-2-coding (1 call)
   - gemini-2.0-flash (6 calls)
   - gemini-2.0-flash-lite (3 calls)
   - kimi-k2-5-longctx (6 calls)

⚠️  Degraded (88% success):
   - gpt-4o (16 calls, alta latencia)

❌ Unhealthy (33% success):
   - gpt-4o-mini (9 calls, rate limiting probable)
```

---

## 🛠️ Diagnóstico Rápido

```bash
# Ver todos los modelos disponibles
halcon doctor | grep cenzontle

# Cambiar modelo por defecto
halcon config set default_model "deepseek-v3-2-coding"

# Probar un modelo específico
halcon chat -m gemini-2.0-flash "Hello"

# Ver métricas detalladas
halcon metrics baseline
```

---

## 🌍 Comparación de Providers

| Provider | Modelos | Context Max | Vision | Reasoning | Autenticación |
|----------|---------|-------------|--------|-----------|---------------|
| **Cenzontle** | 14 | 1M | ✅ | ✅ | SSO/JWT |
| Anthropic | 3 | 200K | ✅ | ❌ | API Key |
| OpenAI | 5 | 128K | ✅ | ✅ (o1) | API Key |
| DeepSeek | 2 | 163K | ❌ | ✅ | API Key |
| Gemini | 2 | 2M | ✅ | ❌ | API Key |
| Ollama | Local | Variable | ❌ | ❌ | None |

**Ventaja de Cenzontle:** Acceso a 14 modelos con 1 sola autenticación.

---

## 📝 Resumen Ejecutivo

**Cenzontle** es tu **gateway unificado** que conecta con:

- **DeepSeek AI** → Coding + Reasoning
- **Moonshot AI (Kimi)** → Ultra-long context (262K)
- **Google Gemini** → Vision + 1M context
- **Meta Llama** → Open models (4, 3.3, 3.1)
- **Mistral AI** → European alternative, coding
- **OpenAI** → GPT-4o, GPT-5 (próximamente)

**Total:** 14 modelos, 6 providers, 1 token de acceso.

**Estado actual:** ✅ Completamente funcional y autenticado.

---

## 🔗 Enlaces Útiles

- **Backend:** https://ca-cenzontle-backend.graypond-e35bfdd8.eastus2.azurecontainerapps.io
- **API Docs:** `/v1/llm/models` (discovery endpoint)
- **Config:** `~/.halcon/config.toml`
- **Models Cache:** `~/.halcon/cenzontle-models.json`

---

**Última actualización:** 2026-03-31
**Documentado por:** Claude Sonnet 4.5 (Sistema de Análisis Automático)
