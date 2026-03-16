//! Adaptive learning subsystem for HybridIntentClassifier — Phase 5 (2026).
//!
//! ## Architecture
//!
//! ```text
//! classify() result
//!       │
//!       ▼  (non-blocking, O(1))
//! FeedbackEvent → DynamicPrototypeStore.pending (VecDeque, ring buffer)
//!       │
//!       ▼  (off critical path — called by agent loop at session end)
//! apply_pending() → EMA centroid update per TaskType
//!       │
//!       ▼
//! Updated centroid → run_embedding_adaptive() prefers it next call
//! ```
//!
//! ## Why EMA centroid update works
//!
//! The TfIdfHashEngine is token-level: tokens hash to fixed positions in R^384.
//! Two semantically similar sentences share tokens → similar positions → similar
//! cosine similarity. EMA accumulates the token distribution of real queries,
//! so the centroid converges toward the "average token fingerprint" of each type.
//!
//! **Honest limitation**: "NPE" and "NullPointerException" hash to different dims —
//! learning from Java examples won't generalize to abbreviations. The heuristic
//! layer (TOML rules) handles abbreviation → type directly. Phase 6 would require
//! dense semantic embeddings (e.g., sentence-transformers) for true generalization.
//!
//! ## Drift safety
//!
//! Each TaskType has an `ArmState` that tracks correction rate.
//! If `n_corrections / n_pulls > drift_threshold (20%)`, centroid updates for that
//! type are paused (drift guardrail). ManualReview events always bypass this gate.
//!
//! ## Persistence
//!
//! Versioned JSON snapshots: `{dir}/prototypes_v{N}.json`.
//! Keep last `max_snapshot_versions` (default 5) for rollback.
//! `prototypes_latest.json` always points to the current version.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use halcon_context::embedding::{cosine_sim, EmbeddingEngine, EmbeddingEngineFactory};

use super::task_analyzer::TaskType;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Default EMA learning rate α ∈ (0, 1).
/// α = 0.10 means a new example contributes 10% to the centroid.
/// After 23 examples the cumulative weight of history is < 10% (conservative).
pub const DEFAULT_LEARNING_RATE: f32 = 0.10;

/// Minimum examples before a dynamic centroid is exposed to run_embedding().
/// Prevents single-example corruption.
pub const MIN_EXAMPLES_FOR_CENTROID: usize = 3;

/// Minimum confidence of a feedback event to incorporate it into the centroid.
/// LowConfidence events below this are queued but skipped during apply_pending().
/// UserCorrection / ManualReview bypass this threshold (authoritative signal).
pub const MIN_EVENT_CONFIDENCE: f32 = 0.40;

/// Correction rate above which centroid updates for a type are paused.
pub const DRIFT_THRESHOLD: f32 = 0.20;

/// Maximum pending events in the ring buffer.
/// Oldest is dropped (not lost — it already influenced UCB1 counters) when full.
pub const MAX_PENDING_EVENTS: usize = 256;

/// UCB1 exploration constant c = √2 ≈ 1.414.
const UCB1_C: f32 = 1.414_213_5_f32;

/// Number of distinct TaskType variants (must match PrototypeStore::type_to_idx).
pub const TASK_TYPE_COUNT: usize = 9;

// ─── Type index mapping ───────────────────────────────────────────────────────
// Duplicated from PrototypeStore to avoid circular dependency with
// hybrid_classifier.rs.  Must be kept in sync with PrototypeStore::type_to_idx.

/// Map TaskType → stable array index [0, TASK_TYPE_COUNT).
#[inline]
pub fn task_type_to_idx(t: TaskType) -> usize {
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

/// Map array index → TaskType (inverse of `task_type_to_idx`).
#[inline]
pub fn idx_to_task_type(i: usize) -> TaskType {
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

// ─── FeedbackSource ───────────────────────────────────────────────────────────

/// Origin of a feedback event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackSource {
    /// User explicitly corrected the classification ("that should be X, not Y").
    UserCorrection,
    /// LLM fallback layer produced a different type than heuristic+embedding.
    LlmDisagreement,
    /// Combined confidence fell below the low-confidence threshold (0.50).
    /// Represents system uncertainty, not explicit error signal.
    LowConfidence,
    /// Developer tooling tagged this query manually.
    ManualReview,
}

// ─── FeedbackEvent ────────────────────────────────────────────────────────────

/// A single learning signal from one classification event.
///
/// Semantics:
/// - `corrected = None`     → prediction was CONFIRMED correct (positive signal)
/// - `corrected = Some(t)`  → prediction was WRONG; `t` is the correct type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackEvent {
    /// The original query text.
    pub query: String,
    /// The type the classifier predicted.
    pub predicted: TaskType,
    /// The correct type (None = confirmed correct).
    pub corrected: Option<TaskType>,
    /// Combined classifier confidence at prediction time.
    pub confidence: f32,
    /// What triggered this event.
    pub source: FeedbackSource,
    /// Pre-computed embedding for this query (optional — computed on-demand if None).
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
    /// Unix epoch timestamp in milliseconds.
    pub timestamp_ms: u64,
}

impl FeedbackEvent {
    /// Create a low-confidence auto-event (generated by the system, not the user).
    pub fn low_confidence(query: &str, predicted: TaskType, confidence: f32) -> Self {
        Self {
            query: query.to_string(),
            predicted,
            corrected: None,
            confidence,
            source: FeedbackSource::LowConfidence,
            embedding: None,
            timestamp_ms: now_ms(),
        }
    }

    /// Create a user-correction event.
    pub fn user_correction(query: &str, predicted: TaskType, correct: TaskType) -> Self {
        Self {
            query: query.to_string(),
            predicted,
            corrected: Some(correct),
            confidence: 0.0,
            source: FeedbackSource::UserCorrection,
            embedding: None,
            timestamp_ms: now_ms(),
        }
    }

    /// Create an LLM disagreement event.
    pub fn llm_disagreement(
        query: &str,
        predicted: TaskType,
        llm_type: TaskType,
        confidence: f32,
    ) -> Self {
        Self {
            query: query.to_string(),
            predicted,
            corrected: Some(llm_type),
            confidence,
            source: FeedbackSource::LlmDisagreement,
            embedding: None,
            timestamp_ms: now_ms(),
        }
    }

    /// The TaskType that should learn from this event.
    ///
    /// - Correction → the CORRECTED type (we want that centroid to improve).
    /// - Confirmation → the PREDICTED type (we reinforce what was already correct).
    #[inline]
    pub fn learning_target(&self) -> TaskType {
        self.corrected.unwrap_or(self.predicted)
    }

    /// True if this is an authoritative correction (user or developer-sourced).
    /// Authoritative events bypass the `min_event_confidence` guardrail.
    #[inline]
    pub fn is_authoritative(&self) -> bool {
        matches!(
            self.source,
            FeedbackSource::UserCorrection | FeedbackSource::ManualReview
        )
    }
}

// ─── UCB1 arm state ───────────────────────────────────────────────────────────

/// Per-TaskType UCB1 statistics.
///
/// UCB1 formula: μ + c × √(ln N / n)
///   μ = n_rewards / n_pulls  (exploitation)
///   N = total pulls across all arms
///   n = pulls for this arm
///   c = UCB1_C (exploration constant)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArmState {
    /// Total classifications that produced this type.
    pub n_pulls: u32,
    /// Classifications confirmed correct (rewards).
    pub n_rewards: u32,
    /// Classifications that were corrected (penalty counter).
    pub n_corrections: u32,
}

impl ArmState {
    /// UCB1 score given total pulls across all arms.
    /// Returns `f32::INFINITY` for unexplored arms (force exploration first).
    pub fn ucb1(&self, total_pulls: u32) -> f32 {
        if self.n_pulls == 0 || total_pulls == 0 {
            return f32::INFINITY;
        }
        let mu = self.n_rewards as f32 / self.n_pulls as f32;
        let exploration = UCB1_C * ((total_pulls as f32).ln() / self.n_pulls as f32).sqrt();
        mu + exploration
    }

    /// Correction rate = corrections / pulls.
    /// High rate (> DRIFT_THRESHOLD) triggers the drift guardrail.
    pub fn correction_rate(&self) -> f32 {
        if self.n_pulls == 0 {
            0.0
        } else {
            self.n_corrections as f32 / self.n_pulls as f32
        }
    }
}

// ─── Live prototype ───────────────────────────────────────────────────────────

/// A mutable centroid for one TaskType in the DynamicPrototypeStore.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LivePrototype {
    /// L2-normalized centroid vector (same dimensionality as TfIdfHashEngine::DIMS).
    centroid: Vec<f32>,
    /// Total number of examples incorporated.
    example_count: usize,
    /// Per-prototype version (monotonic, bumped on each EMA update).
    version: u64,
    /// Last-update timestamp (Unix epoch ms).
    last_update_ms: u64,
}

// ─── Persistence types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PrototypeRecord {
    task_type: String,
    centroid: Vec<f32>,
    example_count: usize,
    version: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct PrototypeSnapshot {
    version: u64,
    timestamp_ms: u64,
    prototypes: Vec<PrototypeRecord>,
    arm_states: Vec<(String, ArmState)>,
}

// ─── AdaptiveConfig ───────────────────────────────────────────────────────────

/// Configuration for the adaptive learning subsystem.
#[derive(Debug, Clone)]
pub struct AdaptiveConfig {
    /// EMA learning rate α ∈ (0, 1). Default: 0.10.
    pub learning_rate: f32,
    /// Minimum examples before a dynamic centroid is used in classification.
    pub min_examples: usize,
    /// Minimum confidence to incorporate an event into the centroid.
    /// LowConfidence events below this are skipped. Authoritative events bypass.
    pub min_event_confidence: f32,
    /// Max pending events in the ring buffer. Oldest is dropped when full.
    pub max_pending: usize,
    /// Correction rate threshold above which a type's updates are paused.
    pub drift_threshold: f32,
    /// If true, save versioned snapshots after each successful apply_pending().
    pub enable_persistence: bool,
    /// Directory for prototype snapshots. None = persistence disabled.
    pub persistence_dir: Option<PathBuf>,
    /// Number of old snapshot files to keep (rolling window).
    pub max_snapshot_versions: usize,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        Self {
            learning_rate: DEFAULT_LEARNING_RATE,
            min_examples: MIN_EXAMPLES_FOR_CENTROID,
            min_event_confidence: MIN_EVENT_CONFIDENCE,
            max_pending: MAX_PENDING_EVENTS,
            drift_threshold: DRIFT_THRESHOLD,
            enable_persistence: false,
            persistence_dir: None,
            max_snapshot_versions: 5,
        }
    }
}

// ─── ApplyResult ─────────────────────────────────────────────────────────────

/// Statistics from one `apply_pending()` invocation.
#[derive(Debug, Default)]
pub struct ApplyResult {
    /// Events that went through the processing pipeline (guardrails may have skipped them).
    pub events_processed: usize,
    /// TaskTypes whose centroids were actually updated.
    pub types_updated: Vec<TaskType>,
    /// Events blocked by guardrails (low confidence, drift paused, etc.).
    pub events_skipped: usize,
    /// Store version after this call.
    pub new_version: u64,
}

// ─── DynamicPrototypeStore ────────────────────────────────────────────────────

/// Mutable prototype store that evolves through feedback.
///
/// ## Lifecycle
///
/// 1. `DynamicPrototypeStore::new(config)` — empty, no centroids yet.
/// 2. `push_feedback(event)` — non-blocking O(1) enqueue (ring buffer).
/// 3. `apply_pending()` — called off-critical-path; processes queue, updates centroids.
/// 4. `HybridIntentClassifier::run_embedding_adaptive()` prefers learned centroids.
///
/// ## Guardrails
///
/// - `min_examples` before centroid is used in classification.
/// - `min_event_confidence` filters low-quality LowConfidence events.
/// - Per-type `ArmState.correction_rate() > drift_threshold` → pause updates.
/// - ManualReview events always bypass all guardrails.
pub struct DynamicPrototypeStore {
    /// Learned centroids — only populated types are present.
    prototypes: HashMap<usize, LivePrototype>,
    /// UCB1 arm statistics — one per TaskType index.
    arm_states: [ArmState; TASK_TYPE_COUNT],
    /// Ring buffer of pending feedback events.
    pending: VecDeque<FeedbackEvent>,
    /// Monotonically increasing version, bumped after each successful apply_pending().
    version: u64,
    /// Configuration.
    config: AdaptiveConfig,
    /// Embedding engine — matches PROTOTYPE_STORE engine for dim alignment.
    /// Selected at construction time via `EmbeddingEngineFactory::default_local()`.
    engine: Box<dyn EmbeddingEngine>,
}

impl DynamicPrototypeStore {
    // ── Construction ──────────────────────────────────────────────────────────

    /// Create an empty dynamic store.
    ///
    /// No initial centroids — falls back entirely to the static PROTOTYPE_STORE
    /// until sufficient feedback accumulates per type.
    pub fn new(config: AdaptiveConfig) -> Self {
        Self {
            prototypes: HashMap::new(),
            arm_states: std::array::from_fn(|_| ArmState::default()),
            pending: VecDeque::new(),
            version: 0,
            config,
            engine: EmbeddingEngineFactory::from_env(),
        }
    }

    // ── Feedback ingestion ────────────────────────────────────────────────────

    /// Push a feedback event into the pending queue.
    ///
    /// Returns `true` if accepted without dropping.
    /// Returns `false` if the queue was full — the oldest event was dropped
    /// (ring-buffer semantics). UCB1 stats are updated immediately regardless.
    ///
    /// **Non-blocking, O(1). Safe to call from the classify hot path.**
    pub fn push_feedback(&mut self, event: FeedbackEvent) -> bool {
        let accepted = if self.pending.len() >= self.config.max_pending {
            self.pending.pop_front();
            false
        } else {
            true
        };
        self.pending.push_back(event);
        accepted
    }

    // ── Batch processing ──────────────────────────────────────────────────────

    /// Process pending feedback events and update centroids.
    ///
    /// Processes up to 64 events per call to bound latency.
    /// Should be called **off the critical path** — e.g., at session end or in
    /// a background task spawned after each agent round.
    ///
    /// Returns `ApplyResult` describing what changed.
    pub fn apply_pending(&mut self) -> ApplyResult {
        const MAX_PER_CALL: usize = 64;

        let mut result = ApplyResult {
            new_version: self.version,
            ..Default::default()
        };
        let n = self.pending.len().min(MAX_PER_CALL);
        let mut updated: HashSet<usize> = HashSet::new();

        for _ in 0..n {
            let event = match self.pending.pop_front() {
                Some(e) => e,
                None => break,
            };

            let target_idx = task_type_to_idx(event.learning_target());
            let predict_idx = task_type_to_idx(event.predicted);

            // ── UCB1 arm accounting ─────────────────────────────────────────
            self.arm_states[predict_idx].n_pulls += 1;
            if let Some(corrected) = event.corrected {
                self.arm_states[predict_idx].n_corrections += 1;
                self.arm_states[task_type_to_idx(corrected)].n_rewards += 1;
            } else {
                self.arm_states[predict_idx].n_rewards += 1;
            }

            // ── Guardrail 1: Drift protection ───────────────────────────────
            // High correction rate on this type → pause centroid updates.
            // ManualReview always bypasses (trusted signal).
            if !event.is_authoritative()
                && self.arm_states[target_idx].correction_rate() > self.config.drift_threshold
            {
                tracing::warn!(
                    target:          "halcon::adaptive_learning",
                    task_type        = idx_to_task_type(target_idx).as_str(),
                    correction_rate  = self.arm_states[target_idx].correction_rate(),
                    drift_threshold  = self.config.drift_threshold,
                    "Adaptive update paused — correction rate exceeds drift threshold"
                );
                result.events_skipped += 1;
                result.events_processed += 1;
                continue;
            }

            // ── Guardrail 2: Confidence minimum ─────────────────────────────
            // Auto-generated LowConfidence events below threshold are skipped.
            // Authoritative corrections always proceed.
            let should_update =
                event.is_authoritative() || event.confidence >= self.config.min_event_confidence;

            if should_update {
                let embedding = match event.embedding {
                    Some(ref v) => v.clone(),
                    None => self.engine.embed(&event.query),
                };

                let proto = self
                    .prototypes
                    .entry(target_idx)
                    .or_insert_with(|| LivePrototype {
                        centroid: vec![0.0_f32; embedding.len()],
                        example_count: 0,
                        version: 0,
                        last_update_ms: 0,
                    });

                if proto.example_count == 0 {
                    // Bootstrap: initialize centroid directly from first example.
                    proto.centroid = embedding.clone();
                    Self::l2_normalize(&mut proto.centroid);
                } else {
                    // EMA update: blend old centroid with new example.
                    Self::ema_update(&mut proto.centroid, &embedding, self.config.learning_rate);
                }

                proto.example_count += 1;
                proto.version += 1;
                proto.last_update_ms = now_ms();
                updated.insert(target_idx);
            } else {
                result.events_skipped += 1;
            }

            result.events_processed += 1;
        }

        if !updated.is_empty() {
            self.version += 1;
        }

        result.types_updated = updated.into_iter().map(idx_to_task_type).collect();
        result.new_version = self.version;

        // Persist if configured.
        if self.config.enable_persistence && !result.types_updated.is_empty() {
            if let Some(ref dir) = self.config.persistence_dir.clone() {
                if let Err(e) = self.save_snapshot(dir) {
                    tracing::warn!(
                        target: "halcon::adaptive_learning",
                        error   = %e,
                        "Failed to persist prototype snapshot — continuing without persistence"
                    );
                }
            }
        }

        if result.events_processed > 0 {
            tracing::info!(
                target:           "halcon::adaptive_learning",
                events_processed  = result.events_processed,
                events_skipped    = result.events_skipped,
                types_updated     = ?result.types_updated,
                new_version       = result.new_version,
                "Prototype store updated"
            );
        }

        result
    }

    // ── Query interface ───────────────────────────────────────────────────────

    /// Classify a query using learned dynamic centroids.
    ///
    /// Returns `Some((TaskType, confidence))` if at least one learned centroid
    /// exceeds the minimum similarity threshold (0.15).
    /// Returns `None` when no dynamic centroids are available or none match —
    /// callers fall back to the static PROTOTYPE_STORE in that case.
    pub fn classify(&self, query: &str) -> Option<(TaskType, f32)> {
        if self.prototypes.is_empty() {
            return None;
        }

        let query_vec = self.engine.embed(query);
        const MIN_SIM: f32 = 0.15;

        let mut best_type: Option<TaskType> = None;
        let mut best_sim: f32 = -1.0;
        let mut runner_sim: f32 = -1.0;

        for (&idx, proto) in &self.prototypes {
            if proto.example_count < self.config.min_examples {
                continue; // not mature enough
            }
            let sim = cosine_sim(&query_vec, &proto.centroid);
            if sim > best_sim {
                runner_sim = best_sim;
                best_sim = sim;
                best_type = Some(idx_to_task_type(idx));
            } else if sim > runner_sim {
                runner_sim = sim;
            }
        }

        let task_type = best_type?;
        if best_sim < MIN_SIM {
            return None;
        }

        let normalized = (best_sim + 1.0) / 2.0;
        let margin = if runner_sim > -1.0 {
            (best_sim - runner_sim).max(0.0)
        } else {
            1.0
        };
        let confidence = (normalized * (0.5 + margin * 0.5)).clamp(0.0, 1.0);

        Some((task_type, confidence))
    }

    /// Return the learned centroid for a type index if it has enough examples.
    ///
    /// Returns `None` when:
    /// - No examples have been accumulated for this type yet.
    /// - `example_count < config.min_examples` (not reliable enough).
    ///
    /// Callers fall back to the static PROTOTYPE_STORE when this returns `None`.
    pub fn prototype_centroid(&self, idx: usize) -> Option<&[f32]> {
        self.prototypes.get(&idx).and_then(|p| {
            if p.example_count >= self.config.min_examples {
                Some(p.centroid.as_slice())
            } else {
                None
            }
        })
    }

    /// UCB1 score for a type index (telemetry and routing hint).
    ///
    /// Returns `f32::INFINITY` for unexplored types (never classified as this type).
    /// Returns a finite value once the type has been observed.
    pub fn ucb1_score(&self, idx: usize) -> f32 {
        if idx >= TASK_TYPE_COUNT {
            return 0.0;
        }
        let total: u32 = self.arm_states.iter().map(|a| a.n_pulls).sum();
        self.arm_states[idx].ucb1(total)
    }

    /// Record a direct arm reward/pull (for callers outside the feedback pipeline).
    ///
    /// `correct = true`  → the classification was confirmed correct (reward).
    /// `correct = false` → the classification was wrong (no reward, just pull).
    pub fn reward_arm(&mut self, idx: usize, correct: bool) {
        if idx >= TASK_TYPE_COUNT {
            return;
        }
        self.arm_states[idx].n_pulls += 1;
        if correct {
            self.arm_states[idx].n_rewards += 1;
        }
    }

    /// Current store version (bumped on each successful apply_pending()).
    pub fn version(&self) -> u64 {
        self.version
    }

    /// UCB1 arm statistics for a type: `(n_pulls, n_rewards, n_corrections)`.
    pub fn arm_stats(&self, idx: usize) -> (u32, u32, u32) {
        if idx >= TASK_TYPE_COUNT {
            return (0, 0, 0);
        }
        let s = &self.arm_states[idx];
        (s.n_pulls, s.n_rewards, s.n_corrections)
    }

    /// Number of events waiting in the pending queue.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    // ── Persistence ───────────────────────────────────────────────────────────

    /// Save a versioned snapshot to `{dir}/prototypes_v{N}.json`.
    ///
    /// Also writes `{dir}/prototypes_latest.json` for fast retrieval.
    /// Prunes old versions to `config.max_snapshot_versions`.
    /// Returns the path of the new snapshot file.
    pub fn save_snapshot(&self, dir: &Path) -> io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;

        let prototypes: Vec<PrototypeRecord> = self
            .prototypes
            .iter()
            .filter(|(_, p)| p.example_count >= self.config.min_examples)
            .map(|(&idx, p)| PrototypeRecord {
                task_type: idx_to_task_type(idx).as_str().to_string(),
                centroid: p.centroid.clone(),
                example_count: p.example_count,
                version: p.version,
            })
            .collect();

        let arm_states: Vec<(String, ArmState)> = self
            .arm_states
            .iter()
            .enumerate()
            .filter(|(_, s)| s.n_pulls > 0)
            .map(|(idx, s)| (idx_to_task_type(idx).as_str().to_string(), s.clone()))
            .collect();

        let snapshot = PrototypeSnapshot {
            version: self.version,
            timestamp_ms: now_ms(),
            prototypes,
            arm_states,
        };

        let json = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let snapshot_path = dir.join(format!("prototypes_v{}.json", self.version));
        std::fs::write(&snapshot_path, &json)?;

        // Keep a stable "latest" pointer.
        let latest_path = dir.join("prototypes_latest.json");
        std::fs::write(&latest_path, &json)?;

        // Prune old versions.
        Self::prune_snapshots(dir, self.config.max_snapshot_versions);

        tracing::debug!(
            target:  "halcon::adaptive_learning",
            path     = %snapshot_path.display(),
            version  = self.version,
            "Prototype snapshot saved"
        );

        Ok(snapshot_path)
    }

    /// Load the latest snapshot from `{dir}/prototypes_latest.json`.
    ///
    /// Returns an error if no snapshot exists in the directory.
    pub fn load_latest(dir: &Path, config: AdaptiveConfig) -> io::Result<Self> {
        let json = std::fs::read_to_string(dir.join("prototypes_latest.json"))?;

        let snapshot: PrototypeSnapshot = serde_json::from_str(&json)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let mut store = Self::new(config);
        store.version = snapshot.version;

        // Probe current engine dims to detect engine changes (e.g., TfIdf→Ollama).
        // Centroids from a different engine have incompatible dimensions and are discarded.
        let engine_dims = {
            let v = store.engine.embed("probe");
            if v.is_empty() { 0 } else { v.len() }
        };

        for record in snapshot.prototypes {
            if let Some(t) = TaskType::from_str(&record.task_type) {
                // Discard centroids whose dims don't match the current engine.
                if engine_dims > 0 && record.centroid.len() != engine_dims {
                    tracing::warn!(
                        target: "halcon::adaptive_learning",
                        task_type   = record.task_type,
                        stored_dims = record.centroid.len(),
                        engine_dims = engine_dims,
                        "Discarding stored centroid — engine changed, dims mismatch"
                    );
                    continue;
                }
                let idx = task_type_to_idx(t);
                store.prototypes.insert(
                    idx,
                    LivePrototype {
                        centroid: record.centroid,
                        example_count: record.example_count,
                        version: record.version,
                        last_update_ms: snapshot.timestamp_ms,
                    },
                );
            }
        }

        for (type_str, arm) in snapshot.arm_states {
            if let Some(t) = TaskType::from_str(&type_str) {
                store.arm_states[task_type_to_idx(t)] = arm;
            }
        }

        Ok(store)
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Exponential moving average update in-place: v = (1-α)v + α·new.
    /// Followed by L2 normalization to keep the centroid on the unit sphere.
    fn ema_update(centroid: &mut Vec<f32>, new_vec: &[f32], alpha: f32) {
        let beta = 1.0 - alpha;
        let n = centroid.len().min(new_vec.len());
        for i in 0..n {
            centroid[i] = beta * centroid[i] + alpha * new_vec[i];
        }
        Self::l2_normalize(centroid);
    }

    /// L2-normalize a vector in-place. No-op if the norm is zero.
    fn l2_normalize(v: &mut Vec<f32>) {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-9 {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
    }

    /// Remove old snapshot files, keeping only the `keep` most recent versions.
    fn prune_snapshots(dir: &Path, keep: usize) {
        let mut files: Vec<(u64, PathBuf)> = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return,
        }
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().into_string().ok()?;
            if name.starts_with("prototypes_v") && name.ends_with(".json") {
                let ver: u64 = name
                    .trim_start_matches("prototypes_v")
                    .trim_end_matches(".json")
                    .parse()
                    .ok()?;
                Some((ver, e.path()))
            } else {
                None
            }
        })
        .collect();

        if files.len() <= keep {
            return;
        }
        files.sort_by_key(|(v, _)| *v);
        for (_, path) in files.iter().take(files.len() - keep) {
            let _ = std::fs::remove_file(path);
        }
    }
}

// ─── Auto-feedback generation ─────────────────────────────────────────────────

/// Inspect a completed classification result and emit zero, one, or two
/// feedback events for the dynamic store.
///
/// This function is called internally by `HybridIntentClassifier` after
/// each `classify()` call when a `DynamicPrototypeStore` is attached.
/// It is purely additive — no external state is read or written here.
pub fn auto_feedback_from_trace(
    query: &str,
    predicted: TaskType,
    confidence: f32,
    llm_used: bool,
    llm_type: Option<TaskType>,
) -> Vec<FeedbackEvent> {
    let mut events = Vec::new();

    // ── Signal 1: Low confidence → ambiguous, worth learning from ────────────
    const LOW_CONFIDENCE_THRESHOLD: f32 = 0.50;
    if confidence < LOW_CONFIDENCE_THRESHOLD {
        events.push(FeedbackEvent::low_confidence(query, predicted, confidence));
    }

    // ── Signal 2: LLM disagreement → heuristic+embedding may be miscalibrated
    if llm_used {
        if let Some(llm_t) = llm_type {
            if llm_t != predicted {
                events.push(FeedbackEvent::llm_disagreement(
                    query, predicted, llm_t, confidence,
                ));
            }
        }
    }

    events
}

// ─── Utility ─────────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AdaptiveConfig {
        AdaptiveConfig::default()
    }

    fn cfg_permissive() -> AdaptiveConfig {
        AdaptiveConfig {
            min_examples: 1,
            min_event_confidence: 0.0,
            ..AdaptiveConfig::default()
        }
    }

    // ── FeedbackEvent ─────────────────────────────────────────────────────────

    #[test]
    fn feedback_event_learning_target_confirmed() {
        let ev = FeedbackEvent::low_confidence("fix the bug", TaskType::Debugging, 0.35);
        assert_eq!(ev.learning_target(), TaskType::Debugging);
        assert!(!ev.is_authoritative());
    }

    #[test]
    fn feedback_event_learning_target_corrected() {
        let ev = FeedbackEvent::user_correction(
            "analyze the CVE report",
            TaskType::Debugging,
            TaskType::Research,
        );
        assert_eq!(ev.learning_target(), TaskType::Research);
        assert!(ev.is_authoritative());
    }

    #[test]
    fn feedback_event_llm_disagreement_constructor() {
        let ev = FeedbackEvent::llm_disagreement(
            "explain and debug this",
            TaskType::Explanation,
            TaskType::Debugging,
            0.45,
        );
        assert_eq!(ev.source, FeedbackSource::LlmDisagreement);
        assert_eq!(ev.corrected, Some(TaskType::Debugging));
    }

    // ── Ring buffer ───────────────────────────────────────────────────────────

    #[test]
    fn push_feedback_accepted_when_not_full() {
        let mut store = DynamicPrototypeStore::new(cfg());
        let ev = FeedbackEvent::low_confidence("test query", TaskType::General, 0.1);
        assert!(store.push_feedback(ev));
        assert_eq!(store.pending_count(), 1);
    }

    #[test]
    fn push_feedback_drops_oldest_when_full() {
        let mut c = cfg();
        c.max_pending = 2;
        let mut store = DynamicPrototypeStore::new(c);
        for _ in 0..3 {
            store.push_feedback(FeedbackEvent::low_confidence("x", TaskType::General, 0.1));
        }
        // Ring buffer — always capped at max_pending.
        assert_eq!(store.pending_count(), 2);
    }

    // ── apply_pending ─────────────────────────────────────────────────────────

    #[test]
    fn apply_pending_empty_is_noop() {
        let mut store = DynamicPrototypeStore::new(cfg());
        let result = store.apply_pending();
        assert_eq!(result.events_processed, 0);
        assert_eq!(result.new_version, 0);
    }

    #[test]
    fn apply_pending_low_confidence_below_threshold_skipped() {
        let mut c = cfg();
        c.min_event_confidence = 0.60;
        let mut store = DynamicPrototypeStore::new(c);

        let ev = FeedbackEvent::low_confidence("some query", TaskType::Debugging, 0.35);
        store.push_feedback(ev);

        let result = store.apply_pending();
        assert_eq!(
            result.events_processed, 1,
            "event was processed (evaluated)"
        );
        assert_eq!(
            result.events_skipped, 1,
            "but skipped due to low confidence"
        );
        assert!(result.types_updated.is_empty());
    }

    #[test]
    fn apply_pending_authoritative_always_updates_centroid() {
        let mut store = DynamicPrototypeStore::new(cfg_permissive());

        let ev = FeedbackEvent::user_correction(
            "fix the null pointer exception",
            TaskType::Debugging,
            TaskType::Debugging,
        );
        store.push_feedback(ev);

        let result = store.apply_pending();
        assert!(
            result.types_updated.contains(&TaskType::Debugging),
            "UserCorrection must update centroid regardless of confidence threshold"
        );
    }

    #[test]
    fn prototype_centroid_absent_before_min_examples() {
        let mut c = cfg_permissive();
        c.min_examples = 3;
        let mut store = DynamicPrototypeStore::new(c);

        for _ in 0..2 {
            store.push_feedback(FeedbackEvent::user_correction(
                "audit SOC2 compliance",
                TaskType::Research,
                TaskType::Research,
            ));
        }
        store.apply_pending();

        let idx = task_type_to_idx(TaskType::Research);
        assert!(
            store.prototype_centroid(idx).is_none(),
            "Centroid must not be available before min_examples"
        );
    }

    #[test]
    fn prototype_centroid_available_after_min_examples() {
        let mut c = cfg_permissive();
        c.min_examples = 2;
        let mut store = DynamicPrototypeStore::new(c);

        for _ in 0..3 {
            store.push_feedback(FeedbackEvent::user_correction(
                "audit IAM permissions for SOC2",
                TaskType::Research,
                TaskType::Research,
            ));
        }
        store.apply_pending();

        let idx = task_type_to_idx(TaskType::Research);
        let centroid = store.prototype_centroid(idx);
        assert!(
            centroid.is_some(),
            "Centroid must be available after min_examples"
        );

        // Must be L2-normalized (norm ≈ 1.0).
        let norm: f32 = centroid.unwrap().iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "Centroid must be L2-normalized (norm = {})",
            norm
        );
    }

    // ── UCB1 ──────────────────────────────────────────────────────────────────

    #[test]
    fn ucb1_infinite_for_unvisited_arm() {
        let store = DynamicPrototypeStore::new(cfg());
        let score = store.ucb1_score(task_type_to_idx(TaskType::Debugging));
        assert!(
            score.is_infinite(),
            "Unvisited arm must have infinite UCB1: {}",
            score
        );
    }

    #[test]
    fn ucb1_finite_after_first_pull() {
        let mut store = DynamicPrototypeStore::new(cfg());
        let idx = task_type_to_idx(TaskType::Research);
        store.reward_arm(idx, true);
        // Pull a second arm so total_pulls > 1 (needed for ln(N) to be positive).
        store.reward_arm(task_type_to_idx(TaskType::Debugging), false);
        let score = store.ucb1_score(idx);
        assert!(
            score.is_finite(),
            "UCB1 must be finite after arm is pulled: {}",
            score
        );
    }

    #[test]
    fn arm_stats_tracks_pulls_rewards_corrections() {
        let mut store = DynamicPrototypeStore::new(cfg());
        let idx = task_type_to_idx(TaskType::CodeGeneration);

        store.reward_arm(idx, true);
        store.reward_arm(idx, true);
        store.reward_arm(idx, false);

        let (pulls, rewards, _corrections) = store.arm_stats(idx);
        assert_eq!(pulls, 3, "n_pulls");
        assert_eq!(rewards, 2, "n_rewards");
    }

    #[test]
    fn apply_pending_updates_ucb1_arm_stats() {
        let mut store = DynamicPrototypeStore::new(cfg_permissive());

        let ev = FeedbackEvent::user_correction(
            "fix the memory leak",
            TaskType::Research,
            TaskType::Debugging,
        );
        store.push_feedback(ev);
        store.apply_pending();

        // predicted=Research got 1 pull + 1 correction
        let research_idx = task_type_to_idx(TaskType::Research);
        let (pulls, _rewards, corrections) = store.arm_stats(research_idx);
        assert_eq!(pulls, 1, "Research must have 1 pull");
        assert_eq!(corrections, 1, "Research must have 1 correction");

        // corrected=Debugging got 1 reward
        let debug_idx = task_type_to_idx(TaskType::Debugging);
        let (_pulls2, rewards2, _) = store.arm_stats(debug_idx);
        assert_eq!(rewards2, 1, "Debugging must have 1 reward from correction");
    }

    // ── Drift guardrail ───────────────────────────────────────────────────────

    #[test]
    fn drift_guardrail_pauses_updates_at_high_correction_rate() {
        let mut c = cfg_permissive();
        c.drift_threshold = 0.20;
        let mut store = DynamicPrototypeStore::new(c);

        let idx = task_type_to_idx(TaskType::Debugging);
        // Simulate 80% correction rate (8 corrections / 10 pulls).
        store.arm_states[idx].n_pulls = 10;
        store.arm_states[idx].n_corrections = 8;

        let ev = FeedbackEvent {
            query: "debug something".to_string(),
            predicted: TaskType::Debugging,
            corrected: Some(TaskType::Debugging),
            confidence: 0.9,
            source: FeedbackSource::LlmDisagreement,
            embedding: None,
            timestamp_ms: now_ms(),
        };
        store.push_feedback(ev);

        let result = store.apply_pending();
        assert_eq!(
            result.events_skipped, 1,
            "Drift guardrail must skip the update"
        );
    }

    #[test]
    fn manual_review_bypasses_drift_guardrail() {
        let mut c = cfg_permissive();
        c.drift_threshold = 0.01; // Extremely sensitive.
        let mut store = DynamicPrototypeStore::new(c);

        let idx = task_type_to_idx(TaskType::Research);
        store.arm_states[idx].n_pulls = 1;
        store.arm_states[idx].n_corrections = 1; // 100% correction rate.

        let ev = FeedbackEvent {
            query: "audit IAM permissions for SOC2".to_string(),
            predicted: TaskType::Research,
            corrected: Some(TaskType::Research),
            confidence: 0.9,
            source: FeedbackSource::ManualReview, // ← bypasses drift
            embedding: None,
            timestamp_ms: now_ms(),
        };
        store.push_feedback(ev);

        let result = store.apply_pending();
        assert_eq!(
            result.events_skipped, 0,
            "ManualReview must bypass drift guardrail"
        );
    }

    // ── Version tracking ──────────────────────────────────────────────────────

    #[test]
    fn version_increments_after_successful_update() {
        let mut store = DynamicPrototypeStore::new(cfg_permissive());
        assert_eq!(store.version(), 0);

        store.push_feedback(FeedbackEvent::user_correction(
            "fix the segfault in the allocator",
            TaskType::Debugging,
            TaskType::Debugging,
        ));
        store.apply_pending();

        assert_eq!(
            store.version(),
            1,
            "Version must increment after successful update"
        );
    }

    #[test]
    fn version_unchanged_when_only_skipped() {
        let mut c = cfg();
        c.min_event_confidence = 0.99;
        let mut store = DynamicPrototypeStore::new(c);

        store.push_feedback(FeedbackEvent::low_confidence("x", TaskType::General, 0.1));
        store.apply_pending();

        assert_eq!(
            store.version(),
            0,
            "Version must NOT increment when all events are skipped"
        );
    }

    // ── EMA update properties ─────────────────────────────────────────────────

    #[test]
    fn ema_update_keeps_centroid_normalized() {
        let mut centroid = vec![0.6_f32, 0.8_f32]; // norm = 1.0
        let new_vec = vec![1.0_f32, 0.0_f32]; // different direction
        DynamicPrototypeStore::ema_update(&mut centroid, &new_vec, 0.1);
        let norm: f32 = centroid.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "EMA must preserve L2 normalization: norm={}",
            norm
        );
    }

    #[test]
    fn l2_normalize_produces_unit_vector() {
        let mut v = vec![3.0_f32, 4.0_f32]; // norm = 5
        DynamicPrototypeStore::l2_normalize(&mut v);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6, "norm={}", norm);
    }

    // ── auto_feedback_from_trace ──────────────────────────────────────────────

    #[test]
    fn auto_feedback_low_confidence_emits_event() {
        let events =
            auto_feedback_from_trace("some ambiguous query", TaskType::General, 0.25, false, None);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source, FeedbackSource::LowConfidence);
    }

    #[test]
    fn auto_feedback_llm_disagreement_emits_event() {
        let events = auto_feedback_from_trace(
            "explain and debug",
            TaskType::Debugging,
            0.60,
            true,
            Some(TaskType::Explanation),
        );
        assert!(
            events
                .iter()
                .any(|e| e.source == FeedbackSource::LlmDisagreement),
            "LLM disagreement must emit a FeedbackSource::LlmDisagreement event"
        );
    }

    #[test]
    fn auto_feedback_confident_and_llm_agree_no_events() {
        let events = auto_feedback_from_trace(
            "git commit the changes",
            TaskType::GitOperation,
            0.95,
            false,
            None,
        );
        assert!(
            events.is_empty(),
            "Confident unambiguous classification should emit no feedback"
        );
    }

    // ── Persistence roundtrip ─────────────────────────────────────────────────

    #[test]
    fn save_and_load_snapshot_roundtrip() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let mut c = cfg_permissive();
        c.min_examples = 1;

        let mut store = DynamicPrototypeStore::new(c.clone());

        for i in 0..3 {
            store.push_feedback(FeedbackEvent::user_correction(
                &format!("audit IAM permissions round {}", i),
                TaskType::Research,
                TaskType::Research,
            ));
        }
        store.apply_pending();

        let path = store.save_snapshot(dir.path()).expect("save");
        assert!(path.exists(), "Snapshot file must exist");

        let loaded = DynamicPrototypeStore::load_latest(dir.path(), c).expect("load");
        assert_eq!(
            loaded.version(),
            store.version(),
            "Version must survive roundtrip"
        );

        let idx = task_type_to_idx(TaskType::Research);
        assert!(
            loaded.prototype_centroid(idx).is_some(),
            "Loaded prototype must be available for Research"
        );
    }

    #[test]
    fn snapshot_pruning_keeps_only_max_versions() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let mut c = cfg_permissive();
        c.max_snapshot_versions = 2;
        c.enable_persistence = true;
        c.persistence_dir = Some(dir.path().to_path_buf());

        let mut store = DynamicPrototypeStore::new(c);

        // Generate 4 versions (each apply_pending creates a new snapshot).
        for _ in 0..4 {
            store.push_feedback(FeedbackEvent::user_correction(
                "fix the null pointer exception",
                TaskType::Debugging,
                TaskType::Debugging,
            ));
            store.apply_pending();
        }

        // Count versioned snapshot files.
        let count = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let n = e.file_name().into_string().unwrap_or_default();
                n.starts_with("prototypes_v") && n.ends_with(".json")
            })
            .count();

        assert!(
            count <= 2,
            "Must keep at most max_snapshot_versions=2, found {}",
            count
        );
    }

    // ── idx mapping consistency ───────────────────────────────────────────────

    #[test]
    fn idx_mapping_is_invertible_for_all_types() {
        let types = [
            TaskType::CodeGeneration,
            TaskType::CodeModification,
            TaskType::Debugging,
            TaskType::Research,
            TaskType::FileManagement,
            TaskType::GitOperation,
            TaskType::Explanation,
            TaskType::Configuration,
            TaskType::General,
        ];
        for t in types {
            let idx = task_type_to_idx(t);
            let roundtrip = idx_to_task_type(idx);
            assert_eq!(roundtrip, t, "Roundtrip failed for {:?}", t);
        }
    }

    #[test]
    fn task_type_count_matches_all_variants() {
        let types = [
            TaskType::CodeGeneration,
            TaskType::CodeModification,
            TaskType::Debugging,
            TaskType::Research,
            TaskType::FileManagement,
            TaskType::GitOperation,
            TaskType::Explanation,
            TaskType::Configuration,
            TaskType::General,
        ];
        assert_eq!(types.len(), TASK_TYPE_COUNT);
    }
}
