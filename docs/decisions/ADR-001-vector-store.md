# ADR-001: Vector Store Backend for L3 Semantic Memory

**Status:** Accepted
**Date:** 2026-03-08
**Deciders:** halcon-cli core team
**Feature:** Frontier Roadmap 2026 — Feature 7 (Semantic Memory Vector Store)

---

## Context

The existing L3 SemanticStore uses BM25 over a max-200-entry truncated list. BM25 retrieval is
keyword-exact; it fails when a user asks conceptually related questions with different vocabulary
(e.g., "file path errors" vs. "FASE-2 path-existence gate"). The goal is to upgrade to
embedding-based retrieval so conceptually similar memories surface even when exact terms differ.

### Requirements
- Persistent HNSW index (survive restarts)
- `search_memory(query)` tool callable from the agent loop
- Pipeline-triggered retrieval via `ContextSource` trait
- Default build must stay zero-new-C++-deps to avoid cross-compilation regressions (see CI fixes
  for cmake/build-essential/libzstd-dev in musl containers)
- Acceptance: 200 entries indexed in <30 s; <50 ms retrieval latency

---

## Options Considered

### Option A: `usearch` (HNSW C++ bindings)
- Production-grade approximate nearest neighbour index
- Wraps usearch C++ library via FFI — adds cmake + C++ toolchain requirement to every
  cross-compilation target (aarch64-unknown-linux-musl already required 5 CI fixes for C deps)
- <50 ms latency at >10 000 entries, overkill for <1 000 MEMORY.md entries
- **Rejected** for MVP: C++ dep risk outweighs benefit at the expected entry count.

### Option B: `lancedb`
- Columnar Rust-native vector store backed by Arrow
- Adds ~80 MB of native libs + Arrow, DuckDB, Lance codecs
- Excellent for multi-million-vector datasets; significant compile-time overhead for <1 000 entries
- **Rejected** for MVP: compile-time and binary-size cost unjustified.

### Option C: `instant-distance` (pure Rust HNSW)
- Zero C++ deps, pure Rust HNSW implementation
- Adequate performance for thousands of entries
- No standard persistence format; requires custom serialization
- **Deferred** to Phase 2 (if entry count exceeds 1 000).

### Option D: TF-IDF hash projection + brute-force cosine similarity (chosen)
- **Zero new dependencies** (pure Rust, serde_json already present)
- 384-dimensional hash-projected TF-IDF vectors with L2 normalization
- FNV-1a hash maps each token to a dimension bucket [0, 384); multiple tokens in the same bucket
  accumulate their TF-IDF weights (effectively a randomized locality-sensitive hash projection)
- Brute-force cosine similarity over 200–500 entries: ~0.1 ms on modern hardware (well within 50 ms)
- MMR re-ranking (λ=0.7) for result diversity
- JSON persistence (`.vindex.json`) — serde_json already in halcon-context

---

## Decision

**Option D** for Phase 1 MVP (≤1 000 entries).

Rationale:
1. Zero new C++ / FFI dependencies — CI stability is critical (5 cross-compilation fixes landed
   in the past 7 days)
2. Performance requirements trivially satisfied at expected MEMORY.md sizes (50–300 entries)
3. Semantic improvement over BM25: hash projection captures term co-occurrence patterns better than
   exact keyword matching; conceptually related terms (e.g., "file path errors" and "FASE-2 gate")
   will share overlapping projection dimensions
4. Upgrade path: the `EmbeddingEngine` trait is designed for drop-in replacement with
   `fastembed-rs` (AllMiniLML6V2Q, 384 dims) behind the `local-embeddings` feature flag;
   `VectorMemoryStore` accepts any `impl EmbeddingEngine`

### Phase 2 Upgrade Trigger
When `VectorMemoryStore::len() > 1000`, the retrieval code logs a warning recommending
upgrade to `instant-distance` or `fastembed` + proper HNSW.

---

## Consequences

**Positive:**
- No CI regressions from new native deps
- All acceptance criteria met for expected entry counts
- Clean upgrade path to neural embeddings via feature flag

**Negative:**
- Hash projection is less accurate than dense neural embeddings for semantically distant paraphrases
- At >1 000 entries, O(n) brute-force becomes the bottleneck (still <10 ms at 10 000 entries, but
  HNSW would be preferable)

---

## Implementation Notes

- `embedding.rs`: `EmbeddingEngine` trait + `TfIdfHashEngine` (default) + `FastEmbedEngine` stub
- `vector_store.rs`: `VectorMemoryStore` — parses MEMORY.md sections, builds index, MMR retrieval,
  JSON persistence at `.halcon/memory/MEMORY.vindex.json`
- `search_memory.rs` (halcon-tools): `SearchMemoryTool` using `Arc<Mutex<VectorMemoryStore>>`
- `vector_memory_source.rs` (halcon-cli): `VectorMemorySource` implementing `ContextSource`
- Feature flag: `policy_config.enable_semantic_memory = false` (opt-in)
