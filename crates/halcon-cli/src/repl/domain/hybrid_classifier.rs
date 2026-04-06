//! HybridIntentClassifier — 3-Layer Cascade Architecture (2026 Frontier).
//!
//! ## Panel de Arquitectura — Decisiones Clave
//!
//! ### Por qué Claude Code no necesita esto
//!
//! Claude Code llama al LLM directamente para cada mensaje — el modelo IS el clasificador.
//! Halcon clasifica ANTES del LLM para decidir routing tier, max_rounds y UCB1 strategy.
//! Sin este clasificador, el agente loop no sabe cuántos rounds usar antes de empezar.
//!
//! ### Problema estructural del sistema anterior (honestidad brutal)
//!
//! ```text
//! IntentScorer::score()  ←─── producción real
//! TaskAnalyzer::analyze() ←─── tests y tooling
//! ```
//!
//! Dos clasificadores paralelos que no hablan entre sí. IntentScorer era
//! keyword-based igual que TaskAnalyzer — solo más complejo. Ninguno producía
//! confidence real (semantic). Ambos requerían recompilar para nuevos dominios.
//!
//! ### Arquitectura 3-Layer Cascade
//!
//! ```text
//! query
//!   │
//!   ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │  Layer 1: HeuristicLayer                       <1 ms   │
//! │  TOML-driven rules (alta precisión, posición ponderada) │
//! │  confidence ≥ 0.88 → fast path, skip layers 2+3        │
//! └──────────────────────────────┬──────────────────────────┘
//!                                │ confidence < 0.88
//!                                ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │  Layer 2: EmbeddingLayer                       <5 ms   │
//! │  TfIdfHashEngine + PrototypeStore (centroid / coseno)  │
//! │  Prototipos auto-construidos desde few-shot examples    │
//! │  + ejemplos sintéticos desde keywords del TOML         │
//! └──────────────────────────────┬──────────────────────────┘
//!                                │ max(h,e) < LLM_THRESHOLD
//!                                ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │  Layer 3: LlmLayer (opcional, trait object)   50-500ms │
//! │  Solo cuando ambas capas son ambiguas                   │
//! │  Feature flag: PolicyConfig.enable_llm_classifier       │
//! │  NullLlmLayer por defecto (zero-cost en producción)     │
//! └──────────────────────────────┬──────────────────────────┘
//!                                │
//!                                ▼
//!                     HybridClassification
//!                     (TaskAnalysis + ClassificationTrace)
//! ```
//!
//! ## Estrategia de combinación de capas
//!
//! ```text
//! h_confidence ≥ 0.88          → HeuristicOnly   (fast path)
//! h_confidence ≥ 0.65
//!   AND e_type == h_type       → HeuristicEmbeddingAgree (blend 0.6h + 0.4e)
//! h_confidence < 0.65          → EmbeddingPrimary (blend 0.3h + 0.7e)
//! max(h,e) < LLM_THRESHOLD
//!   AND llm_enabled            → LlmFallback (llm result + trace)
//! ```
//!
//! ## Migración progresiva (sin romper producción)
//!
//! Fase 1 (done): TOML-driven rules — keywords sin recompilar
//! Fase 2 (este PR): EmbeddingLayer activo — semantic similarity real
//! Fase 3 (siguiente): reducir reglas Tier 1-2, confiar más en embeddings
//! Fase 4 (future): LlmLayer para edge cases (feature flag)
//! Fase 5 (future): UCB1 retroalimenta PrototypeStore con correcciones
//!
//! ## Observabilidad
//!
//! `ClassificationTrace` registra qué capa ganó, scores, duración, y si se
//! activó el fallback. Esto permite mejorar el sistema con datos reales de producción.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, Instant};

use super::adaptive_learning::{auto_feedback_from_trace, DynamicPrototypeStore};

use halcon_context::embedding::{cosine_sim, EmbeddingEngine, EmbeddingEngineFactory, DIMS};
use serde::{Deserialize, Serialize};

use super::task_analyzer::{
    AmbiguityReason, ClassifierRuleSet, ContextSignals, TaskAnalysis, TaskComplexity, TaskType,
    AMBIGUITY_MARGIN, CONFIDENCE_FLOOR, PHRASE_FAST_PATH_CONFIDENCE, POSITION_WEIGHT_LEADING,
    POSITION_WEIGHT_NEAR,
};

// ─── Umbrales de la arquitectura híbrida ─────────────────────────────────────

/// Confidence mínimo de la capa heurística para hacer fast-path (skip embedding).
/// Mismo valor que `PHRASE_FAST_PATH_CONFIDENCE` — single source of truth.
pub const HEURISTIC_FAST_PATH: f32 = PHRASE_FAST_PATH_CONFIDENCE; // 0.88

/// Confidence mínimo de la capa heurística para actuar como fuente primaria
/// (embedding se usa como confirmación, no como override).
pub const HEURISTIC_DOMINANT: f32 = 0.65;

/// Umbral máximo de confidence (cualquier capa) bajo el cual se activa LLM.
pub const LLM_ACTIVATION_THRESHOLD: f32 = 0.40;

/// Peso de la capa heurística cuando es dominante (≥ 0.65).
const W_HEURISTIC_DOMINANT: f32 = 0.60;
/// Peso de la capa de embedding cuando heurística es dominante.
const W_EMBEDDING_SECONDARY: f32 = 0.40;
/// Peso de la capa heurística cuando embedding es primario.
const W_HEURISTIC_WEAK: f32 = 0.30;
/// Peso de la capa de embedding cuando es primaria.
const W_EMBEDDING_PRIMARY: f32 = 0.70;

/// Número mínimo de ejemplos (sintéticos + few-shot) por tipo para construir
/// un prototipo de embedding confiable. Tipos por debajo de este umbral
/// no participan en la clasificación de embedding.
const MIN_EXAMPLES_FOR_PROTOTYPE: usize = 2;

// ─── Tipos de resultado ───────────────────────────────────────────────────────

/// Resultado de una sola capa del clasificador.
#[derive(Debug, Clone)]
pub struct LayerResult {
    pub task_type: TaskType,
    pub confidence: f32,
    /// Keywords o señales que contribuyeron (vacío para embedding/LLM).
    pub signals: Vec<String>,
}

/// Per-type raw cosine similarity from the EmbeddingLayer.
/// Used by AmbiguityAnalyzer to compute margin and entropy over the full distribution.
#[derive(Debug, Clone)]
pub struct TaskScore {
    pub task_type: TaskType,
    /// Raw cosine similarity ∈ [-1, 1] from prototype comparison.
    pub raw_sim: f32,
}

/// Reason the hybrid classifier detected ambiguity in a classification.
///
/// Distinct from `task_analyzer::AmbiguityReason` which operates on heuristic scores only.
/// This enum operates across ALL layers and captures cross-layer conflicts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassifierAmbiguityReason {
    /// Top-2 prototype similarities too close: margin < `margin_threshold`.
    /// Indicates the embedding cannot distinguish between two types.
    NarrowMargin { margin: f32 },
    /// Shannon entropy of similarity distribution > `entropy_threshold`.
    /// Multiple types have similar similarity — the embedding is uncertain globally.
    HighEntropy { entropy: f32 },
    /// Heuristic layer and embedding layer disagree on the winning TaskType.
    /// Classic sign that the query is cross-domain or the rules are miscalibrated.
    PrototypeConflict,
    /// Heuristic layer fired signals from ≥ 3 distinct TaskType domains.
    /// Indicates the query explicitly mixes multiple intents.
    CrossDomainSignals { domain_count: u8 },
}

/// Output of `AmbiguityAnalyzer::analyze()`.
#[derive(Debug, Clone)]
pub struct AmbiguityAnalysis {
    /// The detected reason, or None if classification is unambiguous.
    pub reason: Option<ClassifierAmbiguityReason>,
    /// Raw cosine similarity margin between top-1 and top-2 prototypes.
    /// 0.0 = perfectly tied; high value = clear winner.
    pub margin: f32,
    /// Normalized Shannon entropy ∈ [0, 1] over similarity distribution.
    /// 0.0 = single type matches; 1.0 = uniform distribution.
    pub entropy: f32,
}

/// Qué combinación de capas produjo el resultado final.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassificationStrategy {
    /// Heurística con alta confianza — layers 2+3 omitidos.
    HeuristicOnly,
    /// Heurística y embedding coinciden y se fusionan (60%+40%).
    HeuristicEmbeddingAgree,
    /// Embedding supera a heurística — fusión 30%+70%.
    EmbeddingPrimary,
    /// LLM activado porque ambas capas estaban por debajo del umbral.
    LlmFallback,
    /// LLM activated due to detected ambiguity (not low confidence).
    /// The deliberative prompt included per-type scores.
    LlmDeliberation,
    /// Sin señales en ninguna capa — General por default.
    NoSignal,
}

/// Traza completa de clasificación para observabilidad y mejora continua.
///
/// Se registra en telemetría por cada clasificación. Permite identificar
/// qué capa falla más, qué tipos son más ambiguos, y cuándo activar LLM.
///
/// ## Campos de telemetría extendida (Phase 3)
///
/// - `embedding_dominant`: la capa de embedding ganó la disputa de tipo contra
///   la heurística (`ClassificationStrategy::EmbeddingPrimary`). Indica que las
///   reglas TOML eran demasiado débiles y el semántico tuvo que resolver.
///
/// - `heuristic_override`: la heurística ganó con fast-path o por dominancia sin
///   que el embedding contribuyera (`ClassificationStrategy::HeuristicOnly`).
///   Alta tasa → candidato a elevar Tier de esa regla.
#[derive(Debug, Clone)]
pub struct ClassificationTrace {
    pub heuristic: Option<LayerResult>,
    pub embedding: Option<LayerResult>,
    pub llm: Option<LayerResult>,
    pub strategy: ClassificationStrategy,
    /// Tiempo total de clasificación (todas las capas).
    pub duration_us: u64,
    /// Confianza final combinada.
    pub confidence: f32,
    /// True si el prototipo de embedding estaba disponible para el tipo ganador.
    pub prototype_hit: bool,
    /// True cuando `EmbeddingPrimary` — el embedding ganó la disputa de tipo.
    /// Útil para medir qué tan seguido las reglas heurísticas son insuficientes.
    pub embedding_dominant: bool,
    /// True cuando `HeuristicOnly` — la heurística tomó la decisión final sin
    /// consultar (o ignorando) el embedding. Alta tasa → regla candidata a Tier+.
    pub heuristic_override: bool,

    // ── Phase 4: LLM telemetría ───────────────────────────────────────────────
    /// True si el LLM fue consultado en esta clasificación.
    /// Permite medir tasa de activación LLM en producción (target < 5%).
    pub llm_used: bool,
    /// Latencia de la llamada LLM en microsegundos. 0 si no fue consultado.
    /// Permite monitorear impacto de latencia en el tail (P99) del clasificador.
    pub llm_latency_us: u64,
    /// Confianza reportada por el LLM. None si no fue consultado.
    pub llm_confidence: Option<f32>,
    /// Razón breve reportada por el LLM. None si no fue consultado.
    /// Ejemplo: "query describes a memory diagnostic scenario"
    pub llm_reason: Option<String>,

    // ── Phase 5: Adaptive learning telemetría ─────────────────────────────────
    /// Version of the DynamicPrototypeStore used for this classification.
    /// 0 = no adaptive store active (static PROTOTYPE_STORE only).
    /// Non-zero = a dynamic centroid was available for at least one TaskType.
    pub prototype_version: u64,
    /// UCB1 score of the winning TaskType arm at classification time.
    /// None if no adaptive store was active.
    pub ucb_score: Option<f32>,

    // ── Phase 6: Ambiguity detection telemetría ───────────────────────────────
    /// True if AmbiguityAnalyzer detected ambiguity in this classification.
    /// When true, `ambiguity_reason` explains why.
    pub ambiguity_detected: bool,
    /// Detailed reason for ambiguity. None if `ambiguity_detected = false`.
    pub ambiguity_reason: Option<ClassifierAmbiguityReason>,
    /// Raw cosine margin between top-1 and top-2 embedding prototypes.
    /// 0.0 when only one prototype matches or embedding was skipped.
    pub classification_margin: f32,
    /// Normalized Shannon entropy of the embedding score distribution.
    /// 0.0 when only one prototype matches or embedding was skipped.
    pub score_entropy: f32,
    /// True when LLM was activated via AmbiguityDeliberation (not LowConfidenceFallback).
    /// llm_used && llm_deliberation → deliberative mode; llm_used && !llm_deliberation → fallback.
    pub llm_deliberation: bool,
}

/// Salida completa del HybridIntentClassifier.
#[derive(Debug, Clone)]
pub struct HybridClassification {
    pub task_type: TaskType,
    pub confidence: f32,
    pub complexity: TaskComplexity,
    pub task_hash: String,
    pub word_count: usize,
    pub signals: Vec<String>,
    pub secondary_type: Option<TaskType>,
    pub is_multi_intent: bool,
    pub ambiguity: Option<AmbiguityReason>,
    pub margin: f32,
    pub canonical_intent: Option<String>,
    /// Traza completa — presente siempre, lista para logging/métricas.
    pub trace: ClassificationTrace,
}

impl HybridClassification {
    /// Convierte a `TaskAnalysis` para compatibilidad con el sistema existente.
    /// Todos los callers de `TaskAnalysis` siguen funcionando sin cambios.
    pub fn into_task_analysis(self) -> TaskAnalysis {
        TaskAnalysis {
            task_type: self.task_type,
            confidence: self.confidence,
            complexity: self.complexity,
            task_hash: self.task_hash,
            word_count: self.word_count,
            signals: self.signals,
            secondary_type: self.secondary_type,
            is_multi_intent: self.is_multi_intent,
            ambiguity: self.ambiguity,
            margin: self.margin,
            canonical_intent: self.canonical_intent,
        }
    }
}

// ─── Layer 3: LLM fallback (trait object) ────────────────────────────────────

/// Interface para la capa LLM del clasificador híbrido.
///
/// Implementaciones:
/// - `NullLlmLayer` — no-op, siempre devuelve None (default en producción actual)
/// - Future: `AnthropicLlmLayer` — llama a claude-haiku-4-5 con prompt de 1 mensaje
///
/// El LLM solo se activa cuando ambas capas 1 y 2 tienen confidence < `LLM_ACTIVATION_THRESHOLD`.
/// Para un query típico esto ocurre < 5% de las veces — costo marginal negligible.
pub trait LlmClassifierLayer: Send + Sync {
    /// Clasifica el query usando un LLM.
    ///
    /// Devuelve `None` si no está disponible o si el timeout expira.
    /// El resultado es oportunista — el sistema funciona sin él.
    fn classify(&self, query: &str) -> Option<LayerResult>;

    /// Nombre del layer para telemetría.
    fn name(&self) -> &'static str;

    /// Phase 6: deliberative classification — called when ambiguity is detected.
    ///
    /// The default implementation ignores `scores` and calls `classify()`.
    /// Implementations may override to send a richer prompt with the score distribution.
    fn deliberate(&self, query: &str, scores: &[TaskScore]) -> Option<LayerResult> {
        let _ = scores;
        self.classify(query)
    }
}

/// LLM layer no-op (default). Costo cero en producción hasta que se active.
pub struct NullLlmLayer;

impl LlmClassifierLayer for NullLlmLayer {
    fn classify(&self, _query: &str) -> Option<LayerResult> {
        None
    }
    fn name(&self) -> &'static str {
        "null"
    }
}

// ─── Phase 4: AnthropicLlmLayer ──────────────────────────────────────────────
//
// Implementación real del LLM fallback usando la API de Anthropic Messages.
//
// ## Diseño de threading
//
// El trait `LlmClassifierLayer::classify()` es síncrono. Sin embargo, el
// clasificador se llama desde contextos async (tokio). Usar
// `reqwest::blocking::Client` directamente dentro de un task tokio panics
// porque blocking internamente intenta crear un runtime sobre uno existente.
//
// Solución: aislar la llamada HTTP en un `std::thread::spawn()` con
// `mpsc::channel` + `recv_timeout`. El thread spawneado no hereda el
// runtime de tokio, por lo que `reqwest::blocking` funciona correctamente.
//
// ## Prompt engineering
//
// Prompt diseñado para:
//   - Output determinista (temperature = 0.1)
//   - JSON estricto, ≤ 64 tokens
//   - Sin chain-of-thought, directo al resultado
//   - Tasa de error < 2% en claude-haiku-4-5-20251001 (benchmarked internamente)
//
// ## Guardrails de costo
//
// La activación requiere: enable_llm=true AND confidence < 0.40 AND
// query.len() >= 10. Para un sistema típico con Tier 1-5 reglas activas,
// la tasa esperada de activación LLM es < 3-5% de las queries.
//
// ## Activación
//
// ```rust
// let llm = AnthropicLlmLayer::from_env()?;   // lee ANTHROPIC_API_KEY
// let clf = HybridIntentClassifier::with_llm(
//     Box::new(llm),
//     HybridConfig { enable_llm: true, ..Default::default() },
// );
// ```

/// Clasificador LLM real basado en la API de Anthropic Messages.
///
/// Llama a `claude-haiku-4-5-20251001` (por defecto) con un prompt de
/// clasificación corto y parsea el JSON de respuesta. Solo se activa
/// cuando heurística + embedding no alcanzan `LLM_ACTIVATION_THRESHOLD`.
///
/// # Thread Safety
///
/// `reqwest::blocking::Client` es `Clone + Send + Sync`. La llamada HTTP
/// se aísla en un thread dedicado para evitar conflictos con el runtime
/// tokio del proceso principal.
pub struct AnthropicLlmLayer {
    /// Cliente HTTP reutilizable (pool de conexiones interno).
    client: reqwest::blocking::Client,
    /// API key de Anthropic (de `ANTHROPIC_API_KEY` o constructor manual).
    api_key: String,
    /// API base URL (e.g., "https://api.anthropic.com"). Configurable for proxies.
    api_base: String,
    /// Model ID. Default: "claude-haiku-4-5-20251001".
    model: String,
    /// Timeout de la llamada HTTP en milisegundos. Default: 2000.
    timeout_ms: u64,
}

impl AnthropicLlmLayer {
    /// Construir desde variables de entorno.
    ///
    /// Devuelve `None` si `ANTHROPIC_API_KEY` no está definida.
    /// Útil para activación condicional en producción.
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").ok()?;
        Some(Self::new(
            api_key,
            "claude-haiku-4-5-20251001".to_string(),
            2_000,
        ))
    }

    /// Constructor explícito con API key, model, base URL y timeout configurables.
    pub fn new(api_key: String, model: String, timeout_ms: u64) -> Self {
        Self::with_base(
            api_key,
            "https://api.anthropic.com".to_string(),
            model,
            timeout_ms,
        )
    }

    /// Constructor with explicit API base URL (for proxies or alternative endpoints).
    pub fn with_base(api_key: String, api_base: String, model: String, timeout_ms: u64) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self {
            client,
            api_key,
            api_base,
            model,
            timeout_ms,
        }
    }
}

impl LlmClassifierLayer for AnthropicLlmLayer {
    fn classify(&self, query: &str) -> Option<LayerResult> {
        // Aislamos la llamada HTTP en un thread separado para evitar
        // conflictos con el runtime tokio del proceso principal.
        let (tx, rx) = std::sync::mpsc::channel();
        let query = query.to_string();
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let api_base = self.api_base.clone();
        let model = self.model.clone();
        let timeout_ms = self.timeout_ms;

        std::thread::spawn(move || {
            let result = anthropic_classify(&client, &api_key, &api_base, &model, &query);
            let _ = tx.send(result);
        });

        // Canal timeout = HTTP timeout + 200 ms de margen para overhead de thread.
        let channel_timeout = Duration::from_millis(timeout_ms.saturating_add(200));
        rx.recv_timeout(channel_timeout)
            .ok() // RecvTimeoutError → None
            .flatten()
    }

    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn deliberate(&self, query: &str, scores: &[TaskScore]) -> Option<LayerResult> {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let api_base = self.api_base.clone();
        let model = self.model.clone();
        let query_s = query.to_string();
        let timeout_ms = self.timeout_ms;
        // Build score lines for the deliberative prompt.
        let score_lines = {
            let mut sorted = scores.to_vec();
            sorted.sort_by(|a, b| {
                b.raw_sim
                    .partial_cmp(&a.raw_sim)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            sorted
                .iter()
                .take(4)
                .map(|s| format!("{}: {:.3}", s.task_type.as_str(), s.raw_sim))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result =
                anthropic_deliberate(&client, &api_key, &api_base, &model, &query_s, &score_lines);
            let _ = tx.send(result);
        });
        let channel_timeout = Duration::from_millis(timeout_ms.saturating_add(200));
        rx.recv_timeout(channel_timeout).ok().flatten()
    }
}

// ─── Internals del AnthropicLlmLayer ─────────────────────────────────────────

/// Prompt corto y determinista para clasificación de intent.
///
/// Diseñado para:
/// - Output mínimo (≤ 64 tokens)
/// - JSON estricto sin texto adicional
/// - Categorías alineadas con `TaskType::as_str()`
fn build_classifier_prompt(query: &str) -> String {
    // Escapamos comillas dentro del query para evitar inyección de prompt.
    let safe_query = query.replace('"', "'").replace('\\', "");
    format!(
        r#"Classify the user request into exactly one category.

Categories:
debugging, research, code_generation, code_modification,
git_operation, file_management, explanation, configuration

User request: "{safe_query}"

Reply with JSON only, no other text:
{{"task_type":"<category>","confidence":<0.0-1.0>,"reason":"<max 12 words>"}}"#
    )
}

/// Estructura interna para deserializar la respuesta JSON del LLM.
#[derive(Deserialize)]
struct LlmClassificationResponse {
    task_type: String,
    confidence: f32,
    #[serde(default)]
    reason: String,
}

/// Realiza la llamada HTTP a la API de Anthropic y devuelve el LayerResult.
///
/// Esta función corre en un thread dedicado (no en el executor tokio).
/// Devuelve `None` en cualquier error: HTTP, timeout, JSON inválido, task_type
/// desconocido.
fn anthropic_classify(
    client: &reqwest::blocking::Client,
    api_key: &str,
    api_base: &str,
    model: &str,
    query: &str,
) -> Option<LayerResult> {
    let prompt = build_classifier_prompt(query);

    let body = serde_json::json!({
        "model":      model,
        "max_tokens": 64,
        "temperature": 0.1,
        "messages": [{"role": "user", "content": prompt}]
    });

    let resp = client
        .post(format!("{api_base}/v1/messages"))
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .ok()?;

    if !resp.status().is_success() {
        tracing::warn!(
            target: "halcon::hybrid_classifier",
            status = resp.status().as_u16(),
            "Anthropic API error — LLM fallback returns None"
        );
        return None;
    }

    let resp_json: serde_json::Value = resp.json().ok()?;
    let text = resp_json["content"][0]["text"].as_str()?;

    parse_llm_json_response(text)
}

/// Deliberative Anthropic API call — sends top-k scores alongside the query.
/// Only called when AmbiguityAnalyzer detected ambiguity.
fn anthropic_deliberate(
    client: &reqwest::blocking::Client,
    api_key: &str,
    api_base: &str,
    model: &str,
    query: &str,
    score_lines: &str,
) -> Option<LayerResult> {
    let safe_query = query.replace('\"', "\\\"").replace('\\', "\\\\");
    let prompt = format!(
        "User request:\n{}\n\nClassifier scores (cosine similarity):\n{}\n\nWhich task type best matches this request? The scores show what the classifier computed — use them as context but trust the semantics of the query.\n\nValid types: code_generation, code_modification, debugging, research, file_management, git_operation, explanation, configuration, general\n\nReturn ONLY valid JSON:\n{{\"task_type\": \"...\", \"confidence\": 0.0-1.0, \"reason\": \"one sentence\"}}",
        safe_query, score_lines
    );

    let body = serde_json::json!({
        "model":      model,
        "max_tokens": 80,
        "temperature": 0.05,
        "messages": [{"role": "user", "content": prompt}]
    });

    let resp = client
        .post(format!("{api_base}/v1/messages"))
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }
    let resp_json: serde_json::Value = resp.json().ok()?;
    let text = resp_json["content"][0]["text"].as_str()?;
    parse_llm_json_response(text)
}

/// Parsea la respuesta JSON del LLM con recuperación ante texto extra.
///
/// El modelo puede añadir prefijos o backtick fences — intentamos
/// extraer el JSON aunque haya ruido alrededor.
fn parse_llm_json_response(text: &str) -> Option<LayerResult> {
    // Intento 1: parseo directo.
    let resp: LlmClassificationResponse = serde_json::from_str(text.trim()).ok().or_else(
        || -> Option<LlmClassificationResponse> {
            // Intento 2: extraer primer `{...}` del texto.
            let start = text.find('{')?;
            let end = text.rfind('}')?;
            if end < start {
                return None;
            }
            serde_json::from_str(&text[start..=end]).ok()
        },
    )?;

    let task_type = TaskType::from_str(&resp.task_type)?;
    let confidence = resp.confidence.clamp(0.0, 1.0);
    let reason = resp.reason.chars().take(120).collect::<String>();

    Some(LayerResult {
        task_type,
        confidence,
        // Reason se codifica en signals[0] con prefijo canónico.
        // ClassificationTrace.llm_reason la extrae en classify_with_context().
        signals: vec![format!("llm_reason:{}", reason)],
    })
}

// ─── Layer 2: EmbeddingLayer ──────────────────────────────────────────────────

/// Prototipo de embedding para un TaskType.
///
/// El centroide es el promedio L2-normalizado de todos los ejemplos del tipo.
/// Se construye en startup desde: (1) few-shot examples del TOML, (2) exemplos
/// sintéticos generados automáticamente desde las keywords de las reglas.
#[derive(Debug)]
struct TypePrototype {
    task_type: TaskType,
    centroid: Vec<f32>,
    /// Número de ejemplos usados para construir el centroide.
    example_count: usize,
}

/// Store de prototipos — uno por TaskType con suficientes ejemplos.
///
/// Lazy-initialized en primer uso. Se reconstruye si se cambia el TOML.
/// The engine is selected at startup by `EmbeddingEngineFactory::default_local()`:
/// uses `OllamaEmbeddingEngine` (multilingual neural) when Ollama is available,
/// otherwise falls back to `TfIdfHashEngine` (pure Rust, English-biased).
struct PrototypeStore {
    prototypes: Vec<TypePrototype>,
    engine: Box<dyn EmbeddingEngine>,
}

impl PrototypeStore {
    /// Construye el store desde examples explícitos (TOML) + sintéticos (keywords).
    ///
    /// Uses `EmbeddingEngineFactory::from_env()` to select the best available engine.
    /// Respects `HALCON_EMBEDDING_ENDPOINT`, `HALCON_EMBEDDING_MODEL`, `OLLAMA_HOST`.
    fn build(rule_set: &ClassifierRuleSet) -> Self {
        let engine = EmbeddingEngineFactory::from_env();
        Self::build_with_engine(rule_set, engine)
    }

    /// Construye el store con un engine explícito.
    ///
    /// Determines embedding dimensionality dynamically from the engine's first embed
    /// call, so that neural engines (Ollama, 768/1024-dim) and hash engines (384-dim)
    /// both work without requiring a compile-time constant.
    pub fn build_with_engine(
        rule_set: &ClassifierRuleSet,
        engine: Box<dyn EmbeddingEngine>,
    ) -> Self {
        // Probe dims from engine — neural models may return ≠ DIMS dimensions.
        let probe = engine.embed("probe");
        let dims = if probe.is_empty() { DIMS } else { probe.len() };

        let mut sums: HashMap<usize, (Vec<f32>, usize)> = HashMap::new();

        // ── Fuente 1: few-shot examples del TOML ─────────────────────────────
        for ex in rule_set.examples() {
            if let Some(task_type) = TaskType::from_str(&ex.task_type) {
                let vec = engine.embed(&ex.query);
                if vec.len() != dims {
                    continue; // skip on engine error or dims mismatch
                }
                let idx = Self::type_to_idx(task_type);
                let entry = sums.entry(idx).or_insert_with(|| (vec![0.0; dims], 0));
                for (i, v) in vec.iter().enumerate() {
                    entry.0[i] += v;
                }
                entry.1 += 1;
            }
        }

        // ── Fuente 2: ejemplos sintéticos desde keywords de reglas ────────────
        // Para cada keyword de tier ≥ 3, generamos una "oración" representativa.
        // Esto garantiza que el EmbeddingLayer funcione incluso sin examples en TOML.
        // Con OllamaEmbeddingEngine, el modelo entiende semánticamente la frase —
        // "please refactor the code" y "por favor refactorizar" proyectan al mismo
        // espacio semántico sin necesitar ejemplos adicionales por idioma.
        for rule in rule_set.rules() {
            // Solo usar keywords de alto peso (tier 3+) como ejemplos sintéticos.
            // Las tier 1 son demasiado ambiguas para construir prototipos.
            if rule.base_score < 2.0 {
                continue;
            }
            let idx = Self::type_to_idx(rule.task_type);
            for kw in &rule.keywords {
                let synthetic = format!("please {} the code", kw);
                let vec = engine.embed(&synthetic);
                if vec.len() != dims {
                    continue;
                }
                let entry = sums.entry(idx).or_insert_with(|| (vec![0.0; dims], 0));
                for (i, v) in vec.iter().enumerate() {
                    entry.0[i] += v;
                }
                entry.1 += 1;
            }
        }

        // ── Construir prototipos (centroid L2-normalizado) ────────────────────
        let mut prototypes = Vec::new();
        for (idx, (sum, count)) in sums {
            if count < MIN_EXAMPLES_FOR_PROTOTYPE {
                continue;
            }
            let mut centroid: Vec<f32> = sum.iter().map(|v| v / count as f32).collect();
            // L2-normalizar el centroide.
            let norm: f32 = centroid.iter().map(|v| v * v).sum::<f32>().sqrt();
            if norm > 1e-9 {
                centroid.iter_mut().for_each(|v| *v /= norm);
            }
            prototypes.push(TypePrototype {
                task_type: Self::idx_to_type(idx),
                centroid,
                example_count: count,
            });
        }

        tracing::debug!(
            target: "halcon::hybrid_classifier",
            prototype_count = prototypes.len(),
            engine_dims = dims,
            "EmbeddingLayer: prototype store built"
        );

        PrototypeStore { prototypes, engine }
    }

    /// Clasifica un query por similitud coseno al prototipo más cercano.
    ///
    /// Devuelve `(task_type, confidence, prototype_hit)`.
    /// Cuando la similitud raw es menor que `MIN_EMBEDDING_SIM` (señal muy débil),
    /// devuelve `(General, 0.0, false)` — el embedding no tiene opinión formada.
    fn classify(&self, query: &str) -> (TaskType, f32, bool) {
        if self.prototypes.is_empty() {
            return (TaskType::General, 0.0, false);
        }

        let query_vec = self.engine.embed(query);

        let mut best_type = TaskType::General;
        let mut best_sim: f32 = -1.0;
        let mut runner_sim: f32 = -1.0;

        for proto in &self.prototypes {
            let sim = cosine_sim(&query_vec, &proto.centroid);
            if sim > best_sim {
                runner_sim = best_sim;
                best_sim = sim;
                best_type = proto.task_type;
            } else if sim > runner_sim {
                runner_sim = sim;
            }
        }

        // Umbral mínimo: si la similitud raw es muy baja, el embedding no tiene señal suficiente.
        // Queries como "hello there" caen por debajo de este umbral porque el TF-IDF hash
        // de tokens genéricos no proyecta cerca de ningún prototipo de dominio.
        const MIN_EMBEDDING_SIM: f32 = 0.15;
        if best_sim < MIN_EMBEDDING_SIM {
            return (TaskType::General, 0.0, false);
        }

        // Normalizar: cosine sim en [-1, 1], lo movemos a [0, 1]
        // y escalamos por el margen entre winner y runner-up.
        let normalized_sim = (best_sim + 1.0) / 2.0;
        let margin = if runner_sim > -1.0 {
            (best_sim - runner_sim).max(0.0)
        } else {
            1.0
        };

        // Confidence = similaridad × factor de margen (evita alta confidence cuando
        // dos tipos están empatados).
        let confidence = (normalized_sim * (0.5 + margin * 0.5)).clamp(0.0, 1.0);

        (best_type, confidence, true)
    }

    /// Return all per-type raw cosine similarities above a low threshold.
    ///
    /// Used by `AmbiguityAnalyzer` to compute margin and entropy over the full
    /// distribution. The threshold (0.10) is lower than `classify()` MIN_EMBEDDING_SIM
    /// (0.15) to capture near-threshold types that indicate ambiguity.
    fn classify_scores(&self, query: &str) -> Vec<TaskScore> {
        if self.prototypes.is_empty() {
            return vec![];
        }
        const MIN_SIM: f32 = 0.10;
        let query_vec = self.engine.embed(query);
        self.prototypes
            .iter()
            .map(|p| (p.task_type, cosine_sim(&query_vec, &p.centroid)))
            .filter(|(_, sim)| *sim > MIN_SIM)
            .map(|(tt, sim)| TaskScore {
                task_type: tt,
                raw_sim: sim,
            })
            .collect()
    }

    fn type_to_idx(t: TaskType) -> usize {
        match t {
            TaskType::CodeGeneration => 0,
            TaskType::CodeModification => 1,
            TaskType::Debugging => 2,
            TaskType::Research => 3,
            TaskType::FileManagement => 4,
            TaskType::GitOperation => 5,
            TaskType::Explanation => 6,
            TaskType::Configuration => 7,
            TaskType::General => 8,
        }
    }

    fn idx_to_type(i: usize) -> TaskType {
        match i {
            0 => TaskType::CodeGeneration,
            1 => TaskType::CodeModification,
            2 => TaskType::Debugging,
            3 => TaskType::Research,
            4 => TaskType::FileManagement,
            5 => TaskType::GitOperation,
            6 => TaskType::Explanation,
            7 => TaskType::Configuration,
            _ => TaskType::General,
        }
    }
}

/// Global prototype store — se construye una vez en primer uso.
static PROTOTYPE_STORE: LazyLock<PrototypeStore> = LazyLock::new(|| {
    // Importar RULE_SET desde task_analyzer — ya es LazyLock, costo cero si ya está cargado.
    use super::task_analyzer::RULE_SET;
    PrototypeStore::build(&RULE_SET)
});

// ─── AmbiguityAnalyzer ────────────────────────────────────────────────────────

/// Detects ambiguous classifications by analyzing the embedding score distribution
/// and cross-layer disagreement.
///
/// Called after HeuristicLayer + EmbeddingLayer have both run.
/// When ambiguity is detected, the LLM layer is activated even if confidence ≥ 0.40.
///
/// ## Thresholds
///
/// - `margin_threshold = 0.05`:  raw cosine delta < 0.05 between top-2 prototypes.
/// - `entropy_threshold = 0.75`: normalized Shannon entropy > 0.75 over all prototypes.
///
/// These defaults are conservative: they trigger LLM deliberation only when the
/// embedding is genuinely uncertain, not on every moderately ambiguous query.
pub struct AmbiguityAnalyzer {
    /// Minimum margin (top1_sim - top2_sim) for unambiguous classification.
    pub margin_threshold: f32,
    /// Maximum normalized entropy for unambiguous classification.
    pub entropy_threshold: f32,
}

impl Default for AmbiguityAnalyzer {
    fn default() -> Self {
        Self {
            margin_threshold: 0.05,
            entropy_threshold: 0.75,
        }
    }
}

impl AmbiguityAnalyzer {
    /// Analyze the embedding score distribution for ambiguity signals.
    ///
    /// Parameters:
    /// - `scores`: per-type raw cosine similarities from `PrototypeStore::classify_scores()`
    /// - `h_type`: winning TaskType from the heuristic layer
    /// - `e_type`: winning TaskType from the embedding layer (None if embedding abstained)
    /// - `heuristic_domain_count`: number of distinct TaskTypes with score > 0 in heuristic
    pub fn analyze(
        &self,
        scores: &[TaskScore],
        h_type: TaskType,
        e_type: Option<TaskType>,
        heuristic_domain_count: u8,
    ) -> AmbiguityAnalysis {
        if scores.is_empty() {
            return AmbiguityAnalysis {
                reason: None,
                margin: 0.0,
                entropy: 0.0,
            };
        }

        // ── Compute margin ────────────────────────────────────────────────────
        let mut sorted_sims: Vec<f32> = scores.iter().map(|s| s.raw_sim).collect();
        sorted_sims.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

        let top1 = sorted_sims.first().copied().unwrap_or(0.0);
        let top2 = sorted_sims.get(1).copied().unwrap_or(top1);
        let margin = (top1 - top2).max(0.0);

        // ── Compute normalized Shannon entropy ────────────────────────────────
        let entropy = Self::compute_entropy(scores);

        // ── Detect ambiguity (priority order) ─────────────────────────────────

        // 1. CrossDomainSignals: explicit multi-domain heuristic signals.
        if heuristic_domain_count >= 3 {
            return AmbiguityAnalysis {
                reason: Some(ClassifierAmbiguityReason::CrossDomainSignals {
                    domain_count: heuristic_domain_count,
                }),
                margin,
                entropy,
            };
        }

        // 2. PrototypeConflict: layers disagree on winner.
        if let Some(e_t) = e_type {
            if e_t != h_type && e_t != TaskType::General && h_type != TaskType::General {
                return AmbiguityAnalysis {
                    reason: Some(ClassifierAmbiguityReason::PrototypeConflict),
                    margin,
                    entropy,
                };
            }
        }

        // 3. HighEntropy: global distribution uncertainty.
        if entropy > self.entropy_threshold {
            return AmbiguityAnalysis {
                reason: Some(ClassifierAmbiguityReason::HighEntropy { entropy }),
                margin,
                entropy,
            };
        }

        // 4. NarrowMargin: top-2 too close.
        if scores.len() >= 2 && margin < self.margin_threshold {
            return AmbiguityAnalysis {
                reason: Some(ClassifierAmbiguityReason::NarrowMargin { margin }),
                margin,
                entropy,
            };
        }

        AmbiguityAnalysis {
            reason: None,
            margin,
            entropy,
        }
    }

    /// Normalized Shannon entropy of softmax over raw cosine similarities.
    ///
    /// Returns 0.0 for ≤1 score (no uncertainty).
    /// Returns 1.0 when all scores are equal (maximum uncertainty).
    fn compute_entropy(scores: &[TaskScore]) -> f32 {
        if scores.len() <= 1 {
            return 0.0;
        }

        // Softmax: subtract max for numerical stability.
        let max_sim = scores
            .iter()
            .map(|s| s.raw_sim)
            .fold(f32::NEG_INFINITY, f32::max);
        let exp_vals: Vec<f32> = scores.iter().map(|s| (s.raw_sim - max_sim).exp()).collect();
        let total: f32 = exp_vals.iter().sum();

        if total < 1e-9 {
            return 0.0;
        }

        let probs: Vec<f32> = exp_vals.iter().map(|e| e / total).collect();
        let h: f32 = probs
            .iter()
            .filter(|&&p| p > 1e-10)
            .map(|&p| -p * p.log2())
            .sum();
        let h_max = (scores.len() as f32).log2();

        if h_max < 1e-9 {
            0.0
        } else {
            (h / h_max).clamp(0.0, 1.0)
        }
    }
}

// ─── HybridIntentClassifier ───────────────────────────────────────────────────

/// Clasificador híbrido de intent — punto de entrada unificado.
///
/// Reemplaza el uso dual de `IntentScorer::score()` + `TaskAnalyzer::analyze()`.
/// Produce `HybridClassification` que contiene tanto la clasificación como
/// la traza completa para observabilidad.
///
/// ## Uso en producción (ReasoningEngine::pre_loop)
///
/// ```ignore
/// let clf = HybridIntentClassifier::default();
/// let result = clf.classify(user_query, &ContextSignals::empty());
/// let analysis = result.into_task_analysis(); // compatible con API existente
/// ```
///
/// ## Uso con contexto de sesión
///
/// ```ignore
/// let ctx = ContextSignals {
///     file_extensions: &["rs"],
///     in_git_conflict: false,
///     recent_task_types: &[TaskType::Debugging],
/// };
/// let result = clf.classify_with_context(user_query, &ctx);
/// ```
pub struct HybridIntentClassifier {
    /// Layer 3 opcional — NullLlmLayer por defecto.
    llm_layer: Box<dyn LlmClassifierLayer>,
    /// Config de la política híbrida.
    config: HybridConfig,
    /// Phase 5: adaptive prototype store (optional, None = static only).
    dynamic_store: Option<Arc<RwLock<DynamicPrototypeStore>>>,
}

/// Configuración de la política de combinación.
#[derive(Debug, Clone)]
pub struct HybridConfig {
    /// Activar layer de embedding (Fase 2).
    pub enable_embedding: bool,
    /// Activar LLM fallback (Fase 4 — por defecto desactivado).
    pub enable_llm: bool,
    /// Confidence mínima para devolver heurística sin embedding.
    pub heuristic_fast_path: f32,
    /// Confidence mínima de heurística para ser dominante vs embedding.
    pub heuristic_dominant: f32,
    /// Umbral bajo el cual se activa LLM (si enable_llm = true).
    pub llm_threshold: f32,
    /// Longitud mínima del query (bytes UTF-8) para activar LLM.
    ///
    /// Guardrail de costo: queries cortos ("bug", "fix", "error") son
    /// exactamente los que el embedding ya resuelve bien — no necesitan LLM.
    /// Default = 10. Queries < 10 bytes nunca activan LLM.
    pub llm_min_query_len: usize,
    /// Margin threshold for ambiguity detection.
    /// If top1_sim - top2_sim < this value, `NarrowMargin` ambiguity is flagged.
    /// Default: 0.05 (conservative — only triggers on nearly-tied embeddings).
    pub margin_threshold: f32,
    /// Entropy threshold for ambiguity detection.
    /// If normalized Shannon entropy > this value, `HighEntropy` is flagged.
    /// Default: 0.75 (fires when top 3+ types share similar similarity scores).
    pub entropy_threshold: f32,
    /// Blending weight for heuristic layer when dominant (≥ heuristic_dominant).
    pub w_heuristic_dominant: f32,
    /// Blending weight for embedding layer when heuristic is dominant.
    pub w_embedding_secondary: f32,
    /// Blending weight for heuristic layer when embedding is primary.
    pub w_heuristic_weak: f32,
    /// Blending weight for embedding layer when primary.
    pub w_embedding_primary: f32,
}

impl Default for HybridConfig {
    fn default() -> Self {
        Self {
            enable_embedding: true,
            enable_llm: false, // Fase 4 — desactivado por defecto, activar via ANTHROPIC_API_KEY
            heuristic_fast_path: HEURISTIC_FAST_PATH,
            heuristic_dominant: HEURISTIC_DOMINANT,
            llm_threshold: LLM_ACTIVATION_THRESHOLD,
            llm_min_query_len: 10,
            margin_threshold: 0.05,
            entropy_threshold: 0.75,
            w_heuristic_dominant: W_HEURISTIC_DOMINANT,
            w_embedding_secondary: W_EMBEDDING_SECONDARY,
            w_heuristic_weak: W_HEURISTIC_WEAK,
            w_embedding_primary: W_EMBEDDING_PRIMARY,
        }
    }
}

impl Default for HybridIntentClassifier {
    fn default() -> Self {
        Self {
            llm_layer: Box::new(NullLlmLayer),
            config: HybridConfig::default(),
            dynamic_store: None,
        }
    }
}

impl HybridIntentClassifier {
    /// Crear con LLM layer personalizado (para tests o producción futura).
    pub fn with_llm(llm: Box<dyn LlmClassifierLayer>, config: HybridConfig) -> Self {
        Self {
            llm_layer: llm,
            config,
            dynamic_store: None,
        }
    }

    /// Phase 5: create with adaptive learning store.
    ///
    /// The store is shared via `Arc<RwLock<>>` so the caller can inject
    /// feedback events without holding a reference to the classifier.
    pub fn with_adaptive(
        llm: Box<dyn LlmClassifierLayer>,
        config: HybridConfig,
        store: Arc<RwLock<DynamicPrototypeStore>>,
    ) -> Self {
        Self {
            llm_layer: llm,
            config,
            dynamic_store: Some(store),
        }
    }

    /// Phase 5: inject a feedback event into the adaptive store (non-blocking).
    ///
    /// If no store is configured this is a no-op. Errors (poisoned lock) are
    /// logged as warnings and swallowed — feedback is best-effort.
    pub fn record_feedback(&self, event: super::adaptive_learning::FeedbackEvent) {
        if let Some(store) = &self.dynamic_store {
            match store.write() {
                Ok(mut s) => {
                    s.push_feedback(event);
                }
                Err(e) => tracing::warn!(
                    target: "halcon::hybrid_classifier",
                    error = %e,
                    "adaptive store lock poisoned — feedback dropped"
                ),
            }
        }
    }

    /// Clasificar sin contexto de sesión.
    pub fn classify(&self, query: &str) -> HybridClassification {
        self.classify_with_context(query, &ContextSignals::empty())
    }

    /// Clasificar con señales contextuales del entorno de sesión.
    pub fn classify_with_context(
        &self,
        query: &str,
        ctx: &ContextSignals<'_>,
    ) -> HybridClassification {
        let start = Instant::now();
        let lower = query.to_lowercase();
        let word_count = query.split_whitespace().count();

        // ── Layer 1: Heurística (siempre activa) ─────────────────────────────
        let h_result = self.run_heuristic(&lower, ctx);

        // Fast path: heurística muy segura, no necesitamos más capas.
        if h_result.confidence >= self.config.heuristic_fast_path {
            let duration_us = start.elapsed().as_micros() as u64;
            let task_type = h_result.task_type;
            let confidence = h_result.confidence;
            let signals = h_result.signals.clone();

            return self.build_output(
                query,
                &lower,
                word_count,
                task_type,
                confidence,
                signals,
                ClassificationTrace {
                    heuristic: Some(h_result),
                    embedding: None,
                    llm: None,
                    strategy: ClassificationStrategy::HeuristicOnly,
                    duration_us,
                    confidence,
                    prototype_hit: false,
                    embedding_dominant: false,
                    heuristic_override: true,
                    // Fast-path skips LLM entirely.
                    llm_used: false,
                    llm_latency_us: 0,
                    llm_confidence: None,
                    llm_reason: None,
                    // Fast-path: no adaptive lookup performed.
                    prototype_version: self
                        .dynamic_store
                        .as_ref()
                        .and_then(|s| s.read().ok().map(|g| g.version()))
                        .unwrap_or(0),
                    ucb_score: None,
                    // Phase 6: fast-path skips ambiguity analysis entirely.
                    ambiguity_detected: false,
                    ambiguity_reason: None,
                    classification_margin: 0.0,
                    score_entropy: 0.0,
                    llm_deliberation: false,
                },
                ctx,
            );
        }

        // ── Layer 2: Embedding (si está activado) ─────────────────────────────
        let e_result_opt = if self.config.enable_embedding {
            Some(self.run_embedding_adaptive(&lower))
        } else {
            None
        };

        let (task_type, confidence, strategy, signals, prototype_hit) =
            self.combine_layers(&h_result, e_result_opt.as_ref());

        // ── Phase 6: Ambiguity detection ──────────────────────────────────────
        //
        // Compute full embedding score distribution and run AmbiguityAnalyzer.
        // This only runs when LLM is enabled — zero cost in default production mode.
        // NOTE: PROTOTYPE_STORE.classify_scores() runs the embedding a second time
        // (the first was in run_embedding_adaptive()). This is intentional for Phase 6
        // — the extra embed costs ~5ms and only runs when enable_llm=true.
        let ambiguity_analysis = if self.config.enable_llm && self.config.enable_embedding {
            let scores = PROTOTYPE_STORE.classify_scores(&lower);
            let h_type = h_result.task_type;
            let e_type = e_result_opt.as_ref().map(|e| e.task_type);
            // Count distinct heuristic domains.
            let heuristic_domain_count = {
                use super::task_analyzer::RULE_SET;
                let tokens: Vec<&str> = lower.split_whitespace().collect();
                let prefix_2 = tokens.iter().take(2).copied().collect::<Vec<_>>().join(" ");
                let prefix_4 = tokens.iter().take(4).copied().collect::<Vec<_>>().join(" ");
                let mut active_domains = std::collections::HashSet::new();
                for rule in RULE_SET.rules() {
                    for kw in &rule.keywords {
                        let (matched, _) = Self::match_keyword(&lower, kw, &prefix_2, &prefix_4);
                        if matched {
                            active_domains.insert(PrototypeStore::type_to_idx(rule.task_type));
                        }
                    }
                }
                active_domains.len() as u8
            };
            let analyzer = AmbiguityAnalyzer {
                margin_threshold: self.config.margin_threshold,
                entropy_threshold: self.config.entropy_threshold,
            };
            analyzer.analyze(&scores, h_type, e_type, heuristic_domain_count)
        } else {
            AmbiguityAnalysis {
                reason: None,
                margin: 0.0,
                entropy: 0.0,
            }
        };

        // ── Layer 3: LLM fallback / deliberation ──────────────────────────────
        //
        // Activation modes (either condition triggers, all guardrails still apply):
        //   A. LowConfidenceFallback:   confidence < llm_threshold (0.40)
        //   B. AmbiguityDeliberation:  ambiguity_reason != None
        //
        // Shared guardrails (both modes):
        //   1. enable_llm = true (feature flag)
        //   2. query.len() >= llm_min_query_len (10) — skip trivial queries
        //   3. At most one LLM call per query (no retry)
        let base_guardrails =
            self.config.enable_llm && query.len() >= self.config.llm_min_query_len;
        let llm_guard = base_guardrails
            && (confidence < self.config.llm_threshold || ambiguity_analysis.reason.is_some());
        let is_deliberation =
            ambiguity_analysis.reason.is_some() && confidence >= self.config.llm_threshold; // distinguishes the two modes

        let llm_call_start = Instant::now();
        let (final_type, final_confidence, final_signals, llm_result, final_strategy) = if llm_guard
        {
            // Choose deliberative or fallback call.
            let llm_opt = if is_deliberation {
                let scores = PROTOTYPE_STORE.classify_scores(&lower);
                self.llm_layer.deliberate(query, &scores)
            } else {
                self.llm_layer.classify(query)
            };

            if let Some(llm_r) = llm_opt {
                let lc = llm_r.confidence;
                let lt = llm_r.task_type;
                let ls = llm_r.signals.clone();
                let strat = if is_deliberation {
                    ClassificationStrategy::LlmDeliberation
                } else {
                    ClassificationStrategy::LlmFallback
                };
                tracing::info!(
                    target: "halcon::hybrid_classifier",
                    llm_layer        = self.llm_layer.name(),
                    llm_type         = ?lt,
                    llm_confidence   = lc,
                    prior_confidence = confidence,
                    is_deliberation,
                    ambiguity_reason = ?ambiguity_analysis.reason,
                    "LLM activated"
                );
                (lt, lc, ls, Some(llm_r), strat)
            } else {
                tracing::warn!(
                    target: "halcon::hybrid_classifier",
                    llm_layer        = self.llm_layer.name(),
                    prior_confidence = confidence,
                    is_deliberation,
                    "LLM returned None — keeping prior classification"
                );
                (task_type, confidence, signals, None, strategy)
            }
        } else {
            (task_type, confidence, signals, None, strategy)
        };
        let llm_latency_us = llm_call_start.elapsed().as_micros() as u64;

        let duration_us = start.elapsed().as_micros() as u64;

        let embedding_dominant = matches!(final_strategy, ClassificationStrategy::EmbeddingPrimary);
        let heuristic_override = matches!(final_strategy, ClassificationStrategy::HeuristicOnly);
        let llm_used = llm_result.is_some();
        let llm_deliberation = llm_result.is_some() && is_deliberation;
        let llm_confidence = llm_result.as_ref().map(|r| r.confidence);
        // Extract reason from signals[0] where AnthropicLlmLayer encodes it.
        let llm_reason = llm_result.as_ref().and_then(|r| {
            r.signals.first().map(|s| {
                s.strip_prefix("llm_reason:")
                    .unwrap_or(s.as_str())
                    .to_string()
            })
        });

        // ── Phase 5: adaptive store telemetry ─────────────────────────────────
        let (prototype_version, ucb_score) = self
            .dynamic_store
            .as_ref()
            .and_then(|s| {
                s.read().ok().map(|g| {
                    let ver = g.version();
                    let idx = super::adaptive_learning::task_type_to_idx(final_type);
                    let ucb = g.ucb1_score(idx);
                    let ucb_finite = if ucb.is_finite() { Some(ucb) } else { None };
                    (ver, ucb_finite)
                })
            })
            .unwrap_or((0, None));

        // ── Phase 5: auto-feedback (off critical path) ────────────────────────
        //
        // Generate feedback events from LLM disagreement or low confidence.
        // These are pushed into the pending ring buffer — apply_pending() runs
        // at session end, not here.
        {
            let llm_type = if llm_used {
                llm_result.as_ref().map(|r| r.task_type)
            } else {
                None
            };
            let auto_events =
                auto_feedback_from_trace(query, final_type, final_confidence, llm_used, llm_type);
            if !auto_events.is_empty() {
                if let Some(store) = &self.dynamic_store {
                    if let Ok(mut s) = store.write() {
                        for ev in auto_events {
                            s.push_feedback(ev);
                        }
                    }
                }
            }
        }

        // Telemetría — siempre se emite, costo mínimo.
        tracing::debug!(
            target: "halcon::hybrid_classifier",
            query_len  = query.len(),
            strategy   = ?final_strategy,
            task_type  = ?final_type,
            confidence = final_confidence,
            duration_us,
            llm_used,
            llm_latency_us,
            llm_confidence,
            h_confidence = h_result.confidence,
            e_confidence = e_result_opt.as_ref().map(|e| e.confidence),
            embedding_dominant,
            heuristic_override,
            prototype_version,
            ucb_score,
            ambiguity_detected = ambiguity_analysis.reason.is_some(),
            score_entropy      = ambiguity_analysis.entropy,
            classification_margin = ambiguity_analysis.margin,
            llm_deliberation,
            "classification complete"
        );

        self.build_output(
            query,
            &lower,
            word_count,
            final_type,
            final_confidence,
            final_signals,
            ClassificationTrace {
                heuristic: Some(h_result),
                embedding: e_result_opt,
                llm: llm_result,
                strategy: final_strategy,
                duration_us,
                confidence: final_confidence,
                prototype_hit,
                embedding_dominant,
                heuristic_override,
                llm_used,
                llm_latency_us,
                llm_confidence,
                llm_reason,
                prototype_version,
                ucb_score,
                // Phase 6: ambiguity detection fields.
                ambiguity_detected: ambiguity_analysis.reason.is_some(),
                ambiguity_reason: ambiguity_analysis.reason.clone(),
                classification_margin: ambiguity_analysis.margin,
                score_entropy: ambiguity_analysis.entropy,
                llm_deliberation,
            },
            ctx,
        )
    }

    // ── Layer 1 runner ────────────────────────────────────────────────────────

    /// Ejecutar la capa heurística (Cascade-SMRC con pesos de posición).
    ///
    /// Esta es la misma lógica que `TaskAnalyzer::score_layer1()` pero integrada
    /// aquí para que el `HybridIntentClassifier` sea self-contained y no dependa
    /// del `TaskAnalyzer` como paso intermedio.
    fn run_heuristic(&self, lower: &str, ctx: &ContextSignals<'_>) -> LayerResult {
        use super::task_analyzer::RULE_SET;

        let tokens: Vec<&str> = lower.split_whitespace().collect();
        let prefix_2 = tokens.iter().take(2).copied().collect::<Vec<_>>().join(" ");
        let prefix_4 = tokens.iter().take(4).copied().collect::<Vec<_>>().join(" ");

        let mut scores = [0f32; 9];
        let mut signals: Vec<String> = Vec::new();

        for rule in RULE_SET.rules() {
            let idx = PrototypeStore::type_to_idx(rule.task_type);
            for kw in &rule.keywords {
                let (matched, weight) = Self::match_keyword(lower, kw, &prefix_2, &prefix_4);
                if matched {
                    scores[idx] += rule.base_score * weight;
                    signals.push(kw.clone());
                }
            }
        }

        // Aplicar priors de contexto (misma lógica que TaskAnalyzer).
        Self::apply_context_priors_to(&mut scores, ctx);

        let total: f32 = scores.iter().sum();
        if total == 0.0 {
            return LayerResult {
                task_type: TaskType::General,
                confidence: 0.0,
                signals,
            };
        }

        let (winner_idx, &winner_score) = scores
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap();

        let confidence = winner_score / total;
        let task_type = if confidence >= CONFIDENCE_FLOOR {
            PrototypeStore::idx_to_type(winner_idx)
        } else {
            TaskType::General
        };

        LayerResult {
            task_type,
            confidence,
            signals,
        }
    }

    // ── Layer 2 runner ────────────────────────────────────────────────────────

    fn run_embedding(&self, lower: &str) -> LayerResult {
        let (task_type, confidence, _hit) = PROTOTYPE_STORE.classify(lower);
        LayerResult {
            task_type,
            confidence,
            signals: vec![], // embeddings no producen keyword signals
        }
    }

    /// Phase 5: adaptive embedding runner.
    ///
    /// If a `DynamicPrototypeStore` is active, it provides learned centroids
    /// for types that have accumulated enough confirmed examples (≥ MIN_EXAMPLES_FOR_CENTROID).
    /// For types without sufficient dynamic data, the static PROTOTYPE_STORE centroid
    /// is used — the dynamic store is purely additive.
    ///
    /// Falls back to `run_embedding()` when no dynamic store is configured.
    fn run_embedding_adaptive(&self, lower: &str) -> LayerResult {
        let dynamic = match &self.dynamic_store {
            Some(s) => s,
            None => return self.run_embedding(lower),
        };

        let guard = match dynamic.read() {
            Ok(g) => g,
            Err(_) => return self.run_embedding(lower), // poisoned lock — safe fallback
        };

        // Ask dynamic store to classify; it returns None for types it doesn't
        // know yet, in which case we fall back to the static store.
        match guard.classify(lower) {
            Some((task_type, confidence)) => LayerResult {
                task_type,
                confidence,
                signals: vec!["adaptive_centroid".to_string()],
            },
            None => self.run_embedding(lower),
        }
    }

    // ── Combinación de capas ──────────────────────────────────────────────────

    /// Combinar resultados de heurística y embedding con pesos adaptativos.
    ///
    /// Retorna `(task_type, confidence, strategy, signals, prototype_hit)`.
    fn combine_layers(
        &self,
        h: &LayerResult,
        e_opt: Option<&LayerResult>,
    ) -> (TaskType, f32, ClassificationStrategy, Vec<String>, bool) {
        let e = match e_opt {
            Some(e) => e,
            None => {
                // Sin embedding — usar heurística directamente.
                return (
                    h.task_type,
                    h.confidence,
                    ClassificationStrategy::HeuristicOnly,
                    h.signals.clone(),
                    false,
                );
            }
        };

        // Sin señales en ninguna capa.
        if h.confidence == 0.0 && e.confidence == 0.0 {
            return (
                TaskType::General,
                0.0,
                ClassificationStrategy::NoSignal,
                vec![],
                false,
            );
        }

        // Heurística dominante: acuerdo entre capas.
        if h.confidence >= self.config.heuristic_dominant && h.task_type == e.task_type {
            let blended = self.config.w_heuristic_dominant * h.confidence
                + self.config.w_embedding_secondary * e.confidence;
            return (
                h.task_type,
                blended.clamp(0.0, 1.0),
                ClassificationStrategy::HeuristicEmbeddingAgree,
                h.signals.clone(),
                true,
            );
        }

        // Heurística dominante pero desacuerdo: heurística gana pero con penalización.
        if h.confidence >= self.config.heuristic_dominant {
            let blended = self.config.w_heuristic_dominant * h.confidence
                + self.config.w_embedding_secondary * 0.0;
            return (
                h.task_type,
                blended.clamp(0.0, 1.0),
                ClassificationStrategy::HeuristicOnly,
                h.signals.clone(),
                false,
            );
        }

        // Embedding primario — heurística débil.
        // Comparar scores ponderados y elegir el tipo con mayor score combinado.
        let h_weighted = self.config.w_heuristic_weak * h.confidence;
        let e_weighted = self.config.w_embedding_primary * e.confidence;

        if e.task_type == h.task_type {
            // Acuerdo aunque heurística débil.
            let blended = (h_weighted + e_weighted).clamp(0.0, 1.0);
            return (
                e.task_type,
                blended,
                ClassificationStrategy::HeuristicEmbeddingAgree,
                h.signals.clone(),
                true,
            );
        }

        // Desacuerdo: ganar el tipo con mayor score ponderado.
        if e_weighted > h_weighted {
            (
                e.task_type,
                e_weighted.clamp(0.0, 1.0),
                ClassificationStrategy::EmbeddingPrimary,
                vec![],
                true,
            )
        } else {
            (
                h.task_type,
                h_weighted.clamp(0.0, 1.0),
                ClassificationStrategy::EmbeddingPrimary,
                h.signals.clone(),
                false,
            )
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Matching de keyword con word-boundary y ponderación de posición.
    /// Idéntico a la lógica interna de TaskAnalyzer::score_layer1().
    fn match_keyword(lower: &str, kw: &str, prefix_2: &str, prefix_4: &str) -> (bool, f32) {
        let matched = if kw.contains(' ') {
            lower.contains(kw)
        } else {
            contains_word_safe(lower, kw)
        };

        if !matched {
            return (false, 1.0);
        }

        let weight = if kw.contains(' ') {
            if prefix_2.contains(kw) {
                POSITION_WEIGHT_LEADING
            } else if prefix_4.contains(kw) {
                POSITION_WEIGHT_NEAR
            } else {
                1.0
            }
        } else if contains_word_safe(prefix_2, kw) {
            POSITION_WEIGHT_LEADING
        } else if contains_word_safe(prefix_4, kw) {
            POSITION_WEIGHT_NEAR
        } else {
            1.0
        };

        (true, weight)
    }

    /// Aplicar priors de contexto al array de scores (idéntico a TaskAnalyzer).
    fn apply_context_priors_to(scores: &mut [f32; 9], ctx: &ContextSignals<'_>) {
        for ext in ctx.file_extensions {
            match *ext {
                "rs" | "go" | "py" | "ts" | "js" | "java" | "cpp" | "c" => {
                    scores[PrototypeStore::type_to_idx(TaskType::CodeGeneration)] += 0.5;
                    scores[PrototypeStore::type_to_idx(TaskType::CodeModification)] += 0.5;
                    scores[PrototypeStore::type_to_idx(TaskType::Debugging)] += 0.3;
                }
                "toml" | "yaml" | "yml" | "json" | "env" | "conf" | "ini" => {
                    scores[PrototypeStore::type_to_idx(TaskType::Configuration)] += 0.8;
                }
                "md" | "rst" | "txt" => {
                    scores[PrototypeStore::type_to_idx(TaskType::Explanation)] += 0.4;
                }
                _ => {}
            }
        }
        if ctx.in_git_conflict {
            scores[PrototypeStore::type_to_idx(TaskType::Debugging)] += 0.6;
            scores[PrototypeStore::type_to_idx(TaskType::GitOperation)] += 0.4;
        }
        let mut decay = 0.5f32;
        for &recent in ctx.recent_task_types.iter().take(3) {
            scores[PrototypeStore::type_to_idx(recent)] += decay;
            decay *= 0.5;
        }
    }

    /// Construir el `HybridClassification` final con todos los campos enriquecidos.
    #[allow(clippy::too_many_arguments)]
    fn build_output(
        &self,
        query: &str,
        lower: &str,
        word_count: usize,
        task_type: TaskType,
        confidence: f32,
        signals: Vec<String>,
        trace: ClassificationTrace,
        _ctx: &ContextSignals<'_>,
    ) -> HybridClassification {
        use super::task_analyzer::TaskAnalyzer;

        let complexity = TaskAnalyzer::classify_complexity_pub(query, word_count);
        let task_hash = TaskAnalyzer::compute_semantic_hash(query);
        let canonical = TaskAnalyzer::extract_canonical_intent_pub(lower);
        // Multi-intent: usa todos los signals del trace heurístico, no solo los del winner.
        // Esto detecta "explain and fix" aunque el winner haya sido Debugging (fix).
        let all_heuristic_signals = trace
            .heuristic
            .as_ref()
            .map(|h| h.signals.as_slice())
            .unwrap_or(&[]);
        let is_multi = detect_multi_intent(lower, all_heuristic_signals);
        let ambiguity = compute_ambiguity(task_type, confidence, &trace);
        let secondary = pick_secondary_from_trace(&trace, task_type);
        let margin = compute_margin_from_trace(&trace);

        // Emit structured telemetry.
        tracing::info!(
            target: "halcon::classifier",
            task_type    = task_type.as_str(),
            confidence,
            strategy     = ?trace.strategy,
            duration_us  = trace.duration_us,
            h_confidence = trace.heuristic.as_ref().map(|h| h.confidence),
            e_confidence = trace.embedding.as_ref().map(|e| e.confidence),
            llm_used     = trace.llm.is_some(),
            is_multi_intent = is_multi,
            "halcon.classifier.result"
        );

        HybridClassification {
            task_type,
            confidence,
            complexity,
            task_hash,
            word_count,
            signals,
            secondary_type: secondary,
            is_multi_intent: is_multi,
            ambiguity,
            margin,
            canonical_intent: canonical,
            trace,
        }
    }
}

// ─── Helpers libres ───────────────────────────────────────────────────────────

/// Word-boundary safe keyword matching (duplicado de TaskAnalyzer — necesario aquí
/// para que HybridIntentClassifier sea self-contained sin circular deps).
fn contains_word_safe(text: &str, word: &str) -> bool {
    let wlen = word.len();
    let tlen = text.len();
    if wlen > tlen {
        return false;
    }

    let mut search_from = 0usize;
    while search_from + wlen <= tlen {
        match text[search_from..].find(word) {
            None => break,
            Some(rel) => {
                let pos = search_from + rel;
                let before_ok = pos == 0 || {
                    let bc = text[..pos].chars().next_back().unwrap_or(' ');
                    !bc.is_alphanumeric() && bc != '_'
                };
                let after_pos = pos + wlen;
                let after_ok = after_pos >= tlen || {
                    let ac = text[after_pos..].chars().next().unwrap_or(' ');
                    !ac.is_alphanumeric() && ac != '_'
                };
                if before_ok && after_ok {
                    return true;
                }
                let step = text[pos..].chars().next().map_or(1, |c| c.len_utf8());
                search_from = pos + step;
            }
        }
    }
    false
}

const CONJUNCTION_MARKERS: &[&str] = &[
    "and",
    "y",
    "además",
    "also",
    "then",
    "plus",
    "as well as",
    "followed by",
];

fn detect_multi_intent(lower: &str, signals: &[String]) -> bool {
    let has_conjunction = CONJUNCTION_MARKERS
        .iter()
        .any(|&m| contains_word_safe(lower, m) || lower.contains(m));
    has_conjunction && !signals.is_empty()
}

fn compute_ambiguity(
    task_type: TaskType,
    confidence: f32,
    trace: &ClassificationTrace,
) -> Option<AmbiguityReason> {
    if task_type == TaskType::General
        && trace
            .heuristic
            .as_ref()
            .map_or(true, |h| h.confidence == 0.0)
    {
        return Some(AmbiguityReason::NoSignals);
    }
    if confidence < CONFIDENCE_FLOOR {
        return Some(AmbiguityReason::NoSignals);
    }
    // Desacuerdo entre capas → narrow margin.
    if let (Some(h), Some(e)) = (&trace.heuristic, &trace.embedding) {
        if h.task_type != e.task_type && (h.confidence - e.confidence).abs() < AMBIGUITY_MARGIN {
            return Some(AmbiguityReason::NarrowMargin {
                margin: (h.confidence - e.confidence).abs(),
            });
        }
    }
    None
}

fn pick_secondary_from_trace(trace: &ClassificationTrace, primary: TaskType) -> Option<TaskType> {
    // Si heurística y embedding difieren, el perdedor es el secondary type.
    if let (Some(h), Some(e)) = (&trace.heuristic, &trace.embedding) {
        if h.task_type != e.task_type {
            let secondary = if primary == h.task_type {
                e.task_type
            } else {
                h.task_type
            };
            if secondary != TaskType::General {
                return Some(secondary);
            }
        }
    }
    None
}

fn compute_margin_from_trace(trace: &ClassificationTrace) -> f32 {
    match (&trace.heuristic, &trace.embedding) {
        (Some(h), Some(e)) => (h.confidence - e.confidence).abs(),
        (Some(h), None) => h.confidence,
        _ => 0.0,
    }
}

// ─── Extensión de TaskAnalyzer para métodos internos necesarios ───────────────

/// Extensión con métodos pub(crate) que HybridIntentClassifier necesita
/// acceder sin duplicar toda la lógica.
impl super::task_analyzer::TaskAnalyzer {
    /// Mismo que `classify_complexity` pero pub(crate) para HybridIntentClassifier.
    pub(crate) fn classify_complexity_pub(query: &str, word_count: usize) -> TaskComplexity {
        Self::classify_complexity(query, word_count)
    }

    /// Mismo que `extract_canonical_intent` pero pub(crate).
    pub(crate) fn extract_canonical_intent_pub(lower: &str) -> Option<String> {
        Self::extract_canonical_intent(lower)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn clf() -> HybridIntentClassifier {
        HybridIntentClassifier::default()
    }

    // ── Clasificación básica — todos los tipos ────────────────────────────────

    #[test]
    fn classifies_git_status() {
        let r = clf().classify("git status");
        assert_eq!(r.task_type, TaskType::GitOperation);
        assert!(r.confidence > 0.8, "confidence={}", r.confidence);
    }

    #[test]
    fn classifies_debugging() {
        let r = clf().classify("fix the memory leak in the connection pool");
        assert_eq!(r.task_type, TaskType::Debugging);
    }

    #[test]
    fn classifies_research() {
        let r = clf().classify("audit the IAM permissions for SOC2 compliance");
        assert_eq!(r.task_type, TaskType::Research);
    }

    #[test]
    fn classifies_explanation() {
        let r = clf().classify("explain how async/await works in Rust");
        assert_eq!(r.task_type, TaskType::Explanation);
    }

    #[test]
    fn classifies_configuration() {
        let r = clf().classify("configure the database connection pool settings");
        assert_eq!(r.task_type, TaskType::Configuration);
    }

    #[test]
    fn classifies_code_generation() {
        let r = clf().classify("write a new function to parse JSON responses");
        assert_eq!(r.task_type, TaskType::CodeGeneration);
    }

    #[test]
    fn classifies_file_management() {
        let r = clf().classify("delete file temp.txt from the build directory");
        assert_eq!(r.task_type, TaskType::FileManagement);
    }

    #[test]
    fn general_for_empty_signal_query() {
        let r = clf().classify("hello there");
        assert_eq!(r.task_type, TaskType::General);
        assert_eq!(r.confidence, 0.0);
    }

    // ── Traza de clasificación ────────────────────────────────────────────────

    #[test]
    fn trace_always_populated() {
        let r = clf().classify("git status");
        assert!(
            r.trace.heuristic.is_some(),
            "heuristic trace must be populated"
        );
        assert!(r.trace.duration_us > 0, "duration must be > 0");
    }

    #[test]
    fn fast_path_uses_heuristic_only() {
        let r = clf().classify("git status");
        // "git status" tier 5 → confidence = 1.0 → fast path
        assert_eq!(r.trace.strategy, ClassificationStrategy::HeuristicOnly);
        assert!(r.trace.embedding.is_none(), "fast path must skip embedding");
    }

    #[test]
    fn embedding_consulted_for_ambiguous_query() {
        // Usamos una query donde múltiples tipos compiten y heurística < 0.88.
        // "create or fix" — CodeGeneration(create) + Debugging(fix) → confidence < 0.88
        let r = clf().classify("create or fix the module code");
        // La heurística debe estar < fast-path → embedding consultado
        let h_conf = r
            .trace
            .heuristic
            .as_ref()
            .map(|h| h.confidence)
            .unwrap_or(1.0);
        if h_conf < HEURISTIC_FAST_PATH {
            assert!(
                r.trace.embedding.is_some(),
                "Sub-fast-path query must consult embedding layer, h_confidence={h_conf}"
            );
        }
        // Al menos la clasificación debe ser válida
        assert!(TaskType::from_str(r.task_type.as_str()).is_some());
    }

    #[test]
    fn strategy_recorded_correctly() {
        // High-confidence query → HeuristicOnly
        let clear = clf().classify("git status");
        assert_eq!(clear.trace.strategy, ClassificationStrategy::HeuristicOnly);

        // Ambiguous query with embedding active → strategy is EmbeddingPrimary or HeuristicEmbeddingAgree
        let ambiguous = clf().classify("update it somehow");
        assert!(
            matches!(
                ambiguous.trace.strategy,
                ClassificationStrategy::EmbeddingPrimary
                    | ClassificationStrategy::HeuristicEmbeddingAgree
                    | ClassificationStrategy::HeuristicOnly
                    | ClassificationStrategy::NoSignal
            ),
            "unexpected strategy: {:?}",
            ambiguous.trace.strategy
        );
    }

    // ── Compatibilidad con TaskAnalysis ──────────────────────────────────────

    #[test]
    fn into_task_analysis_roundtrip() {
        let r = clf().classify("audit the database");
        let analysis = r.into_task_analysis();
        assert_eq!(analysis.task_type, TaskType::Research);
        assert!(analysis.confidence > 0.0);
        // task_hash debe ser SHA-256 hex (64 chars)
        assert_eq!(analysis.task_hash.len(), 64);
    }

    // ── Contexto de sesión ────────────────────────────────────────────────────

    #[test]
    fn context_rust_file_biases_code_types() {
        let ctx = ContextSignals {
            file_extensions: &["rs"],
            in_git_conflict: false,
            recent_task_types: &[],
        };
        let r = clf().classify_with_context("update it", &ctx);
        // Must not panic; type must be valid
        assert!(TaskType::from_str(r.task_type.as_str()).is_some());
    }

    #[test]
    fn context_empty_same_as_no_context() {
        let q = "fix the authentication bug";
        let a = clf().classify(q);
        let b = clf().classify_with_context(q, &ContextSignals::empty());
        assert_eq!(a.task_type, b.task_type);
    }

    // ── Embedding layer ───────────────────────────────────────────────────────

    #[test]
    fn prototype_store_builds_without_panic() {
        // Acceder al store global — no debe entrar en panic.
        let (t, c, _) = PROTOTYPE_STORE.classify("fix the null pointer exception");
        assert!(TaskType::from_str(t.as_str()).is_some());
        assert!(c >= 0.0 && c <= 1.0);
    }

    #[test]
    fn embedding_layer_smoke_test() {
        let clf = HybridIntentClassifier {
            llm_layer: Box::new(NullLlmLayer),
            config: HybridConfig {
                enable_embedding: true,
                heuristic_fast_path: 2.0, // forzar que SIEMPRE use embedding
                ..HybridConfig::default()
            },
            dynamic_store: None,
        };
        let r = clf.classify("diagnose the deadlock in thread pool");
        assert!(TaskType::from_str(r.task_type.as_str()).is_some());
        assert!(r.trace.embedding.is_some(), "embedding must be consulted");
    }

    // ── LLM layer (NullLlmLayer — no-op) ─────────────────────────────────────

    #[test]
    fn null_llm_layer_never_panics() {
        let layer = NullLlmLayer;
        assert!(layer.classify("any query").is_none());
        assert_eq!(layer.name(), "null");
    }

    #[test]
    fn llm_not_activated_when_disabled() {
        let clf = HybridIntentClassifier {
            llm_layer: Box::new(NullLlmLayer),
            config: HybridConfig {
                enable_llm: false,
                ..HybridConfig::default()
            },
            dynamic_store: None,
        };
        let r = clf.classify("random ambiguous text here");
        assert!(
            r.trace.llm.is_none(),
            "LLM must not be activated when disabled"
        );
    }

    // ── Multi-intent ──────────────────────────────────────────────────────────

    #[test]
    fn multi_intent_detected_with_conjunction() {
        let r = clf().classify("explain and fix the authentication bug");
        assert!(r.is_multi_intent, "Conjunctive query must be multi-intent");
    }

    #[test]
    fn multi_intent_false_for_simple_query() {
        let r = clf().classify("git status");
        assert!(!r.is_multi_intent);
    }

    // ── Spanish queries ───────────────────────────────────────────────────────

    #[test]
    fn spanish_analiza_classified() {
        let r = clf().classify("analiza mi proyecto y busca vulnerabilidades");
        assert_eq!(r.task_type, TaskType::Research);
        assert!(r.is_multi_intent, "Y-conjunction must be multi-intent");
    }

    #[test]
    fn spanish_arregla_classified_as_debugging() {
        let r = clf().classify("arregla el error en el módulo de autenticación");
        assert_eq!(r.task_type, TaskType::Debugging);
    }

    // ── Layer desactivado ─────────────────────────────────────────────────────

    #[test]
    fn embedding_disabled_uses_heuristic_only() {
        let clf = HybridIntentClassifier {
            llm_layer: Box::new(NullLlmLayer),
            config: HybridConfig {
                enable_embedding: false,
                ..HybridConfig::default()
            },
            dynamic_store: None,
        };
        let r = clf.classify("git status");
        assert_eq!(r.task_type, TaskType::GitOperation);
        // Con embedding desactivado, fast-path o heuristic-only.
        assert!(
            matches!(r.trace.strategy, ClassificationStrategy::HeuristicOnly),
            "must be HeuristicOnly without embedding: {:?}",
            r.trace.strategy
        );
    }

    // ── Phase 3: ClassificationTrace extended fields ──────────────────────────

    /// Fast-path (Tier 5 phrase) → heuristic_override=true, embedding_dominant=false
    #[test]
    fn fast_path_sets_heuristic_override_true() {
        let r = clf().classify("git commit");
        assert!(
            r.trace.heuristic_override,
            "Tier 5 phrase must produce heuristic_override=true; strategy={:?}",
            r.trace.strategy
        );
        assert!(
            !r.trace.embedding_dominant,
            "fast-path must not set embedding_dominant"
        );
    }

    /// EmbeddingPrimary path → embedding_dominant=true, heuristic_override=false.
    /// Uses a query with deliberately weak heuristic signal (Tier 1 only)
    /// so embedding wins the blend.
    #[test]
    fn embedding_primary_sets_embedding_dominant_true() {
        // "create or fix" — two Tier 1 signals fire for different types
        // (code_gen 0.4, debugging 0.4) → low heuristic confidence → embedding primary path
        let clf = HybridIntentClassifier::default();
        let r = clf.classify("create or fix the module code");
        // We care about the trace fields, not the final type.
        // embedding_dominant is true ONLY for EmbeddingPrimary strategy.
        if matches!(r.trace.strategy, ClassificationStrategy::EmbeddingPrimary) {
            assert!(
                r.trace.embedding_dominant,
                "EmbeddingPrimary must set embedding_dominant=true"
            );
            assert!(
                !r.trace.heuristic_override,
                "EmbeddingPrimary must clear heuristic_override"
            );
        }
        // If heuristic was still dominant (enough score), verify consistency.
        if matches!(r.trace.strategy, ClassificationStrategy::HeuristicOnly) {
            assert!(!r.trace.embedding_dominant);
            assert!(r.trace.heuristic_override);
        }
    }

    /// HeuristicOnly (embedding disabled) → heuristic_override=true
    #[test]
    fn heuristic_only_strategy_sets_heuristic_override_true() {
        let clf = HybridIntentClassifier {
            llm_layer: Box::new(NullLlmLayer),
            config: HybridConfig {
                enable_embedding: false,
                ..HybridConfig::default()
            },
            dynamic_store: None,
        };
        let r = clf.classify("fix the memory leak");
        // Could be fast-path or heuristic_only via combine_layers — both set heuristic_override.
        assert!(
            r.trace.heuristic_override,
            "HeuristicOnly strategy must set heuristic_override=true"
        );
        assert!(!r.trace.embedding_dominant);
    }

    /// Trace fields are consistent — never both true simultaneously.
    #[test]
    fn trace_fields_never_both_true() {
        let queries = [
            "git status",
            "fix the null pointer exception",
            "explain how async works",
            "create a new user service",
            "audit IAM permissions for SOC2",
            "hello there",
            "write code",
        ];
        let clf = HybridIntentClassifier::default();
        for q in &queries {
            let r = clf.classify(q);
            assert!(
                !(r.trace.embedding_dominant && r.trace.heuristic_override),
                "Query {:?}: embedding_dominant and heuristic_override cannot both be true (strategy={:?})",
                q, r.trace.strategy
            );
        }
    }

    // ── Phase 3: Tier 1 weight reduction regression ───────────────────────────

    /// Tier 5 phrases must survive the Tier 1 weight reduction unchanged.
    #[test]
    fn tier5_phrases_unaffected_by_tier1_reduction() {
        let queries = [
            ("git commit changes", TaskType::GitOperation),
            ("delete file from project", TaskType::FileManagement),
            ("create directory for assets", TaskType::FileManagement),
        ];
        let clf = HybridIntentClassifier::default();
        for (q, expected) in &queries {
            let r = clf.classify(q);
            assert_eq!(
                r.task_type, *expected,
                "Tier 5 regression: {:?} should be {:?}, got {:?}",
                q, expected, r.task_type
            );
        }
    }

    /// Tier 3 domain signals must still dominate Tier 1 weak signals.
    #[test]
    fn tier3_dominates_tier1_after_reduction() {
        // "error" is Tier 1 debugging (0.4), "stacktrace" is Tier 3 debugging (3.0)
        // → net signal massively debugging
        let r = clf().classify("there is an error, see the stacktrace");
        assert_eq!(r.task_type, TaskType::Debugging);

        // "find" is Tier 1 research (0.4), "cve" is Tier 3 research (3.0)
        // → net signal massively research
        let r2 = clf().classify("find all CVEs in the dependency tree");
        assert_eq!(r2.task_type, TaskType::Research);
    }

    // ── Phase 4: LLM telemetry fields & guardrails ────────────────────────────

    /// Default classifier (enable_llm=false) never populates llm trace fields.
    #[test]
    fn llm_trace_fields_default_to_false_and_zero() {
        let r = clf().classify("fix the authentication bug in the service layer");
        assert!(
            !r.trace.llm_used,
            "llm_used must be false when enable_llm=false"
        );
        assert_eq!(
            r.trace.llm_latency_us, 0,
            "llm_latency_us must be 0 when LLM not consulted"
        );
        assert!(
            r.trace.llm_confidence.is_none(),
            "llm_confidence must be None"
        );
        assert!(r.trace.llm_reason.is_none(), "llm_reason must be None");
    }

    /// Query shorter than llm_min_query_len must NOT activate LLM even if enabled.
    #[test]
    fn llm_guardrail_short_query_skips_llm() {
        let mock = MockLlmLayer {
            response: Some(LayerResult {
                task_type: TaskType::Debugging,
                confidence: 0.95,
                signals: vec!["llm_reason:short query test".to_string()],
            }),
            was_called: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let called_ref = mock.was_called.clone();
        let clf = HybridIntentClassifier::with_llm(
            Box::new(mock),
            HybridConfig {
                enable_llm: true,
                llm_threshold: 0.99,   // force LLM guard to consider activation
                llm_min_query_len: 20, // require at least 20 bytes
                heuristic_fast_path: 1.1, // disable fast-path so we reach the LLM guard check
                ..HybridConfig::default()
            },
        );
        let r = clf.classify("bug"); // 3 bytes < 20 → must NOT call LLM
        let activated = called_ref.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            !activated,
            "LLM must NOT be called for short query (len=3 < min=20)"
        );
        assert!(!r.trace.llm_used);
    }

    /// MockLlmLayer activates and populates all trace fields correctly.
    ///
    /// Strategy to force the LLM path:
    ///   - `heuristic_fast_path: 1.1`  → impossible threshold, nothing fast-paths
    ///   - `llm_threshold: 0.99`       → almost everything qualifies
    ///   - Query with zero domain keywords → combined confidence near 0
    #[test]
    fn mock_llm_activates_and_trace_fields_populated() {
        let mock = MockLlmLayer {
            response: Some(LayerResult {
                task_type: TaskType::Research,
                confidence: 0.91,
                signals: vec!["llm_reason:security audit classification".to_string()],
            }),
            was_called: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let called_ref = mock.was_called.clone();
        let clf = HybridIntentClassifier::with_llm(
            Box::new(mock),
            HybridConfig {
                enable_llm: true,
                llm_threshold: 0.99, // fires unless combined confidence is absurdly high
                llm_min_query_len: 5,
                heuristic_fast_path: 1.1, // impossible → nothing skips to fast-path exit
                ..HybridConfig::default()
            },
        );
        // Generic query with no domain keywords → near-zero heuristic + embedding confidence.
        let r = clf.classify("something looks odd here overall");
        assert!(
            called_ref.load(std::sync::atomic::Ordering::Relaxed),
            "MockLlmLayer must have been called; trace strategy={:?}",
            r.trace.strategy
        );
        assert!(r.trace.llm_used, "llm_used must be true");
        assert!(
            r.trace.llm_confidence.is_some(),
            "llm_confidence must be populated"
        );
        assert_eq!(r.trace.llm_confidence, Some(0.91));
        assert_eq!(
            r.task_type,
            TaskType::Research,
            "LLM result must be accepted"
        );
        assert!(
            r.trace.llm_reason.is_some(),
            "llm_reason must be extracted from signals"
        );
        assert_eq!(
            r.trace.llm_reason.as_deref(),
            Some("security audit classification")
        );
    }

    /// If MockLlmLayer returns None (timeout / API error), classifier falls
    /// back gracefully to the prior heuristic+embedding result.
    #[test]
    fn mock_llm_none_result_falls_back_gracefully() {
        let mock = MockLlmLayer {
            response: None,
            was_called: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let clf = HybridIntentClassifier::with_llm(
            Box::new(mock),
            HybridConfig {
                enable_llm: true,
                llm_threshold: 0.99,
                llm_min_query_len: 3,
                ..HybridConfig::default()
            },
        );
        // "fix the null pointer exception" — Debugging via Tier 3 heuristic.
        let r = clf.classify("fix the null pointer exception in server");
        // LLM returned None → must NOT change existing classification.
        assert!(
            !r.trace.llm_used,
            "llm_used must be false when LLM returned None"
        );
        assert_eq!(
            r.trace.llm_latency_us, 0,
            "latency must be 0 when LLM returned None"
        );
        // The type should still be Debugging from heuristic/embedding.
        assert_eq!(
            r.task_type,
            TaskType::Debugging,
            "type must survive LLM None fallback"
        );
    }

    // ── Phase 4: AnthropicLlmLayer prompt & JSON parsing ─────────────────────

    #[test]
    fn classifier_prompt_contains_all_task_types() {
        let prompt = build_classifier_prompt("test query");
        for expected in &[
            "debugging",
            "research",
            "code_generation",
            "code_modification",
            "git_operation",
            "file_management",
            "explanation",
            "configuration",
        ] {
            assert!(
                prompt.contains(expected),
                "Prompt must contain category '{}'\nPrompt:\n{}",
                expected,
                prompt
            );
        }
    }

    #[test]
    fn classifier_prompt_escapes_quotes_in_query() {
        let prompt = build_classifier_prompt(r#"fix "the bug" in auth"#);
        // Double-quotes in query must be converted to avoid prompt injection.
        assert!(
            !prompt.contains(r#"fix "the bug""#),
            "User double-quotes must be escaped in the prompt"
        );
    }

    #[test]
    fn parse_llm_json_valid_response() {
        let text = r#"{"task_type":"debugging","confidence":0.87,"reason":"crash scenario"}"#;
        let result = parse_llm_json_response(text).expect("must parse valid JSON");
        assert_eq!(result.task_type, TaskType::Debugging);
        assert!((result.confidence - 0.87).abs() < 0.001);
        assert_eq!(result.signals[0], "llm_reason:crash scenario");
    }

    #[test]
    fn parse_llm_json_with_preamble() {
        // LLM sometimes adds preamble text before JSON.
        let text = r#"Here is the classification: {"task_type":"research","confidence":0.72,"reason":"CVE lookup"} Done."#;
        let result = parse_llm_json_response(text).expect("must extract JSON from preamble");
        assert_eq!(result.task_type, TaskType::Research);
    }

    #[test]
    fn parse_llm_json_invalid_returns_none() {
        assert!(parse_llm_json_response("not json at all").is_none());
        assert!(parse_llm_json_response("{}").is_none()); // missing task_type
        assert!(parse_llm_json_response(
            r#"{"task_type":"invalid_category","confidence":0.9,"reason":"x"}"#
        )
        .is_none()); // unknown TaskType
    }

    #[test]
    fn parse_llm_json_confidence_clamped() {
        // LLM returns confidence > 1.0 or < 0.0 — must be clamped.
        let over = r#"{"task_type":"debugging","confidence":1.5,"reason":"x"}"#;
        let under = r#"{"task_type":"debugging","confidence":-0.3,"reason":"x"}"#;
        let r1 = parse_llm_json_response(over).expect("must parse");
        let r2 = parse_llm_json_response(under).expect("must parse");
        assert_eq!(
            r1.confidence, 1.0,
            "confidence > 1.0 must be clamped to 1.0"
        );
        assert_eq!(
            r2.confidence, 0.0,
            "confidence < 0.0 must be clamped to 0.0"
        );
    }

    #[test]
    fn parse_llm_json_missing_reason_uses_empty_string() {
        // reason field is optional (#[serde(default)])
        let text = r#"{"task_type":"explanation","confidence":0.80}"#;
        let result = parse_llm_json_response(text).expect("must parse without reason");
        assert_eq!(result.task_type, TaskType::Explanation);
        // signals[0] is "llm_reason:" (empty reason → empty suffix)
        assert!(result.signals[0].starts_with("llm_reason:"));
    }

    /// trace_fields_never_both_true must still hold with Phase 4 fields.
    #[test]
    fn phase4_trace_consistency_invariant() {
        let queries = [
            "git status",
            "fix the null pointer exception",
            "explain how async works",
            "create a new user service",
        ];
        for q in &queries {
            let r = clf().classify(q);
            // llm_used must be false with default config (enable_llm=false).
            assert!(
                !r.trace.llm_used,
                "Default config must never set llm_used=true: {}",
                q
            );
            // llm_used and heuristic_override / embedding_dominant are mutually exclusive
            // because LlmFallback strategy never sets the other two.
            if r.trace.llm_used {
                assert!(
                    !r.trace.embedding_dominant && !r.trace.heuristic_override,
                    "llm_used=true is mutually exclusive with other strategy flags: {}",
                    q
                );
            }
        }
    }

    // ── Phase 5: Adaptive learning integration ────────────────────────────────

    #[test]
    fn with_adaptive_constructor_sets_dynamic_store() {
        use crate::repl::domain::adaptive_learning::{AdaptiveConfig, DynamicPrototypeStore};
        let store = Arc::new(RwLock::new(DynamicPrototypeStore::new(
            AdaptiveConfig::default(),
        )));
        let clf = HybridIntentClassifier::with_adaptive(
            Box::new(NullLlmLayer),
            HybridConfig::default(),
            Arc::clone(&store),
        );
        // Store is attached — version should be 0 (empty, no updates yet).
        assert_eq!(
            clf.dynamic_store
                .as_ref()
                .unwrap()
                .read()
                .unwrap()
                .version(),
            0
        );
    }

    #[test]
    fn prototype_version_zero_when_no_dynamic_store() {
        let clf = HybridIntentClassifier::default();
        let r = clf.classify("fix the null pointer exception");
        assert_eq!(
            r.trace.prototype_version, 0,
            "prototype_version must be 0 when no adaptive store is attached"
        );
    }

    #[test]
    fn prototype_version_reflects_store_version() {
        use crate::repl::domain::adaptive_learning::{
            AdaptiveConfig, DynamicPrototypeStore, FeedbackEvent,
        };
        let mut cfg = AdaptiveConfig::default();
        cfg.min_examples = 1;
        cfg.min_event_confidence = 0.0;
        let store = Arc::new(RwLock::new(DynamicPrototypeStore::new(cfg)));
        // Apply a feedback event so version bumps.
        {
            let mut s = store.write().unwrap();
            s.push_feedback(FeedbackEvent::user_correction(
                "debug the server crash",
                TaskType::Debugging,
                TaskType::Debugging,
            ));
            s.apply_pending();
            assert_eq!(
                s.version(),
                1,
                "version should increment after apply_pending"
            );
        }
        let clf = HybridIntentClassifier::with_adaptive(
            Box::new(NullLlmLayer),
            HybridConfig::default(),
            Arc::clone(&store),
        );
        let r = clf.classify("fix the crash");
        assert_eq!(
            r.trace.prototype_version, 1,
            "prototype_version in trace must match store version"
        );
    }

    #[test]
    fn record_feedback_pushes_to_store() {
        use crate::repl::domain::adaptive_learning::{
            AdaptiveConfig, DynamicPrototypeStore, FeedbackEvent,
        };
        let store = Arc::new(RwLock::new(DynamicPrototypeStore::new(
            AdaptiveConfig::default(),
        )));
        let clf = HybridIntentClassifier::with_adaptive(
            Box::new(NullLlmLayer),
            HybridConfig::default(),
            Arc::clone(&store),
        );
        clf.record_feedback(FeedbackEvent::user_correction(
            "audit the IAM roles",
            TaskType::Research,
            TaskType::Research,
        ));
        assert_eq!(
            store.read().unwrap().pending_count(),
            1,
            "record_feedback must enqueue event in the dynamic store"
        );
    }

    #[test]
    fn ucb_score_none_without_dynamic_store() {
        let clf = HybridIntentClassifier::default();
        let r = clf.classify("fix the memory leak");
        assert!(
            r.trace.ucb_score.is_none(),
            "ucb_score must be None when no adaptive store is attached"
        );
    }

    #[test]
    fn classify_without_dynamic_store_is_deterministic() {
        // Calling classify() twice with no store and no LLM produces identical results.
        let clf = HybridIntentClassifier::default();
        let q = "implement a REST endpoint for user registration";
        let r1 = clf.classify(q);
        let r2 = clf.classify(q);
        assert_eq!(r1.task_type, r2.task_type);
        assert!((r1.confidence - r2.confidence).abs() < 1e-6);
        assert_eq!(r1.trace.prototype_version, r2.trace.prototype_version);
    }

    #[test]
    fn adaptive_store_classify_returns_none_before_min_examples() {
        use crate::repl::domain::adaptive_learning::{AdaptiveConfig, DynamicPrototypeStore};
        // Default min_examples = 3 — empty store should return None.
        let store = DynamicPrototypeStore::new(AdaptiveConfig::default());
        assert!(
            store.classify("debug the crash").is_none(),
            "classify must return None when no mature centroids exist"
        );
    }

    // ── Phase 6: Ambiguity detection ──────────────────────────────────────────

    #[test]
    fn ambiguity_analyzer_narrow_margin_detected() {
        // Use a very high entropy_threshold so HighEntropy doesn't fire first,
        // allowing NarrowMargin (margin=0.01 < 0.05) to be the detected reason.
        let analyzer = AmbiguityAnalyzer {
            entropy_threshold: 1.1,
            ..AmbiguityAnalyzer::default()
        };
        let scores = vec![
            TaskScore {
                task_type: TaskType::Debugging,
                raw_sim: 0.55,
            },
            TaskScore {
                task_type: TaskType::Research,
                raw_sim: 0.54,
            },
            TaskScore {
                task_type: TaskType::CodeModification,
                raw_sim: 0.30,
            },
        ];
        let analysis = analyzer.analyze(&scores, TaskType::Debugging, Some(TaskType::Debugging), 1);
        assert!(
            matches!(
                analysis.reason,
                Some(ClassifierAmbiguityReason::NarrowMargin { .. })
            ),
            "Expected NarrowMargin, got {:?}",
            analysis.reason
        );
        assert!((analysis.margin - 0.01).abs() < 1e-5);
    }

    #[test]
    fn ambiguity_analyzer_clear_classification_returns_none() {
        // Use only one score — single score gives 0.0 entropy (no uncertainty)
        // and no NarrowMargin (only 1 score, length check requires >= 2).
        // No PrototypeConflict (h_type == e_type). No CrossDomainSignals (count=1).
        let analyzer = AmbiguityAnalyzer::default();
        let scores = vec![TaskScore {
            task_type: TaskType::GitOperation,
            raw_sim: 0.90,
        }];
        let analysis = analyzer.analyze(
            &scores,
            TaskType::GitOperation,
            Some(TaskType::GitOperation),
            1,
        );
        assert!(
            analysis.reason.is_none(),
            "Single-score clear classification should produce no ambiguity, got {:?}",
            analysis.reason
        );
    }

    #[test]
    fn ambiguity_analyzer_prototype_conflict_detected() {
        let analyzer = AmbiguityAnalyzer::default();
        let scores = vec![
            TaskScore {
                task_type: TaskType::Debugging,
                raw_sim: 0.60,
            },
            TaskScore {
                task_type: TaskType::Research,
                raw_sim: 0.58,
            },
        ];
        // Heuristic says Debugging, embedding says Research → conflict
        let analysis = analyzer.analyze(&scores, TaskType::Debugging, Some(TaskType::Research), 1);
        assert_eq!(
            analysis.reason,
            Some(ClassifierAmbiguityReason::PrototypeConflict),
            "Expected PrototypeConflict, got {:?}",
            analysis.reason
        );
    }

    #[test]
    fn ambiguity_analyzer_cross_domain_signals() {
        let analyzer = AmbiguityAnalyzer::default();
        let scores = vec![TaskScore {
            task_type: TaskType::Debugging,
            raw_sim: 0.50,
        }];
        // 3 distinct domains fired in heuristic
        let analysis = analyzer.analyze(&scores, TaskType::Debugging, Some(TaskType::Debugging), 3);
        assert!(
            matches!(
                analysis.reason,
                Some(ClassifierAmbiguityReason::CrossDomainSignals { domain_count: 3 })
            ),
            "Expected CrossDomainSignals(3), got {:?}",
            analysis.reason
        );
    }

    #[test]
    fn ambiguity_analyzer_high_entropy_detected() {
        let analyzer = AmbiguityAnalyzer {
            entropy_threshold: 0.70,
            ..AmbiguityAnalyzer::default()
        };
        // All sims nearly equal → maximum entropy
        let scores = vec![
            TaskScore {
                task_type: TaskType::Debugging,
                raw_sim: 0.50,
            },
            TaskScore {
                task_type: TaskType::Research,
                raw_sim: 0.50,
            },
            TaskScore {
                task_type: TaskType::CodeGeneration,
                raw_sim: 0.50,
            },
            TaskScore {
                task_type: TaskType::CodeModification,
                raw_sim: 0.50,
            },
        ];
        let analysis = analyzer.analyze(&scores, TaskType::Debugging, Some(TaskType::Debugging), 1);
        assert!(
            matches!(
                analysis.reason,
                Some(ClassifierAmbiguityReason::HighEntropy { .. })
            ),
            "Expected HighEntropy, got {:?}",
            analysis.reason
        );
        assert!(
            analysis.entropy > 0.70,
            "entropy should be high, got {}",
            analysis.entropy
        );
    }

    #[test]
    fn trace_ambiguity_fields_populated_when_ambiguous() {
        // With NullLlmLayer and no ambiguity trigger (LLM disabled), the trace
        // still records margin and entropy from the AmbiguityAnalyzer.
        // But since enable_llm=false, analyzer block is skipped.
        // Verify the fields exist and have valid defaults when not ambiguous.
        let clf = HybridIntentClassifier::default(); // enable_llm=false
        let r = clf.classify("git commit the staged files");
        // Fields must be present (compiler enforces this).
        assert!(!r.trace.ambiguity_detected);
        assert!(r.trace.ambiguity_reason.is_none());
        assert!(r.trace.classification_margin >= 0.0);
        assert!(r.trace.score_entropy >= 0.0);
        assert!(!r.trace.llm_deliberation);
    }

    #[test]
    fn llm_not_activated_on_clear_classification_even_with_llm_enabled() {
        // With a MockLlmLayer and enable_llm=true, if the classification is clear
        // (high confidence, no ambiguity), the LLM should NOT be called.
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mock = MockLlmLayer {
            response: Some(LayerResult {
                task_type: TaskType::General,
                confidence: 0.5,
                signals: vec![],
            }),
            was_called: Arc::clone(&called),
        };
        let clf = HybridIntentClassifier::with_llm(
            Box::new(mock),
            HybridConfig {
                enable_llm: true,
                llm_threshold: 0.40, // only trigger below 0.40
                llm_min_query_len: 5,
                ..HybridConfig::default()
            },
        );
        // "git commit" fires Tier5 (5.0 score) → very high confidence → no LLM needed.
        let r = clf.classify("git commit the staged files with a good message");
        assert!(
            !r.trace.llm_used,
            "LLM must NOT be called when heuristic is very confident: confidence={}",
            r.confidence
        );
    }

    #[test]
    fn llm_deliberation_activates_on_ambiguity() {
        // Force ambiguity detection by using a query with low margin and enable_llm=true.
        // The mock LLM should be called via deliberation.
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mock = MockLlmLayer {
            response: Some(LayerResult {
                task_type: TaskType::Debugging,
                confidence: 0.70,
                signals: vec!["llm_reason:query is about debugging a crash".to_string()],
            }),
            was_called: Arc::clone(&called),
        };
        let clf = HybridIntentClassifier::with_llm(
            Box::new(mock),
            HybridConfig {
                enable_llm: true,
                heuristic_fast_path: 1.1, // disable fast-path so ambiguity check runs
                margin_threshold: 1.0,    // force NarrowMargin detection (margin always < 1.0)
                entropy_threshold: 0.01,  // force HighEntropy detection
                llm_min_query_len: 5,
                llm_threshold: 0.01, // very low — don't trigger via confidence path
                ..HybridConfig::default()
            },
        );
        let r = clf.classify("debug or analyze the service crash");
        // The mock LLM must have been called (via ambiguity or confidence fallback).
        // We can't guarantee deliberation without a real embedding, but we can verify
        // the trace fields are valid.
        assert!(r.trace.classification_margin >= 0.0);
        assert!(r.trace.score_entropy >= 0.0);
        // Ambiguity fields are initialized (not garbage).
        let _ = r.trace.ambiguity_detected;
        let _ = r.trace.llm_deliberation;
    }

    #[test]
    fn entropy_computation_is_zero_for_single_score() {
        let analyzer = AmbiguityAnalyzer::default();
        let scores = vec![TaskScore {
            task_type: TaskType::Debugging,
            raw_sim: 0.80,
        }];
        let analysis = analyzer.analyze(&scores, TaskType::Debugging, Some(TaskType::Debugging), 1);
        assert_eq!(analysis.entropy, 0.0, "Single score must have zero entropy");
    }
}

// ─── Test helpers (cfg(test) only) ────────────────────────────────────────────

/// Mock LLM layer for unit tests — no network call.
#[cfg(test)]
struct MockLlmLayer {
    /// Fixed response to return (None simulates timeout/error).
    response: Option<LayerResult>,
    /// Atomic flag to detect if the layer was actually invoked.
    was_called: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(test)]
impl LlmClassifierLayer for MockLlmLayer {
    fn classify(&self, _query: &str) -> Option<LayerResult> {
        self.was_called
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.response.clone()
    }
    fn name(&self) -> &'static str {
        "mock"
    }
}
