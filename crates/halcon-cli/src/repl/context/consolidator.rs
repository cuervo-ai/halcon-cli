//! Memory consolidator: deduplicates and merges similar reflections across sessions.
//!
//! SOTA pattern: cross-session learning. Instead of accumulating unbounded reflections,
//! periodically consolidate similar ones into higher-confidence entries and prune
//! low-relevance stale entries.
//!
//! Operations:
//! - **Deduplicate**: Merge reflections with overlapping content (Jaccard > threshold).
//! - **Prune**: Remove reflections below a relevance threshold after decay.
//! - **Consolidate**: Combine N similar reflections into one with boosted relevance.

use halcon_core::error::Result;
use halcon_storage::{AsyncDatabase, MemoryEntry, MemoryEntryType};

/// Configuration for memory consolidation.
pub struct ConsolidationConfig {
    /// Jaccard similarity threshold for considering two reflections similar.
    pub similarity_threshold: f64,
    /// Minimum relevance score — entries below this after decay are pruned.
    pub min_relevance: f64,
    /// Maximum number of reflection entries to keep.
    pub max_reflections: u32,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.4,
            min_relevance: 0.2,
            max_reflections: 50,
        }
    }
}

/// Result of a consolidation run.
#[derive(Debug, Default)]
pub struct ConsolidationResult {
    /// Number of entries merged into existing ones.
    pub merged: usize,
    /// Number of entries pruned (low relevance).
    pub pruned: usize,
    /// Number of entries remaining after consolidation.
    pub remaining: usize,
}

/// Compute Jaccard similarity between two texts using word-level tokens.
///
/// Returns a value in [0.0, 1.0] where 1.0 means identical word sets.
pub fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let set_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let set_b: std::collections::HashSet<&str> = b.split_whitespace().collect();

    if set_a.is_empty() && set_b.is_empty() {
        return 1.0;
    }

    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Find clusters of similar reflections using single-linkage clustering.
///
/// Returns groups of entry indices where each group has pairwise Jaccard > threshold.
fn cluster_similar(entries: &[MemoryEntry], threshold: f64) -> Vec<Vec<usize>> {
    let n = entries.len();
    let mut parent: Vec<usize> = (0..n).collect();

    // Union-Find helpers.
    fn find(parent: &mut [usize], i: usize) -> usize {
        if parent[i] != i {
            parent[i] = find(parent, parent[i]);
        }
        parent[i]
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }

    // Compare all pairs — O(n²) but n ≤ 50 so ≤ 1225 comparisons.
    for i in 0..n {
        for j in (i + 1)..n {
            if jaccard_similarity(&entries[i].content, &entries[j].content) >= threshold {
                union(&mut parent, i, j);
            }
        }
    }

    // Group by root.
    let mut groups: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for i in 0..n {
        groups.entry(find(&mut parent, i)).or_default().push(i);
    }

    groups.into_values().collect()
}

/// Run memory consolidation: deduplicate, merge, and prune reflections.
///
/// 1. Load all reflection entries.
/// 2. Cluster similar reflections (Jaccard > threshold).
/// 3. For each cluster with >1 entry: keep the highest-relevance one, boost it,
///    delete the others.
/// 4. Prune entries below min_relevance.
/// 5. Enforce max_reflections limit.
pub async fn consolidate(
    db: &AsyncDatabase,
    config: &ConsolidationConfig,
) -> Result<ConsolidationResult> {
    let mut result = ConsolidationResult::default();

    // Load all reflections.
    let entries = db
        .list_memories(Some(MemoryEntryType::Reflection), 500)
        .await?;

    if entries.is_empty() {
        return Ok(result);
    }

    // Step 1: Cluster similar entries.
    let clusters = cluster_similar(&entries, config.similarity_threshold);

    // Step 2: Merge clusters with >1 entry.
    for cluster in &clusters {
        if cluster.len() <= 1 {
            continue;
        }

        // Find the entry with the highest relevance — it's the "keeper".
        let keeper_idx = *cluster
            .iter()
            .max_by(|&&a, &&b| {
                entries[a]
                    .relevance_score
                    .partial_cmp(&entries[b].relevance_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();

        // Boost the keeper's relevance by the cluster size (diminishing returns).
        let boost = 0.1 * (cluster.len() as f64 - 1.0).min(3.0);
        let keeper = &entries[keeper_idx];
        let new_score = (keeper.relevance_score + boost).min(2.0);
        let _ = db
            .update_memory_relevance(keeper.entry_id, new_score)
            .await;

        // Delete the non-keeper entries.
        for &idx in cluster {
            if idx != keeper_idx {
                let _ = db.inner().delete_memory(entries[idx].entry_id);
                result.merged += 1;
            }
        }
    }

    // Step 3: Prune low-relevance entries.
    let remaining = db
        .list_memories(Some(MemoryEntryType::Reflection), 500)
        .await?;

    for entry in &remaining {
        if entry.relevance_score < config.min_relevance {
            let _ = db.inner().delete_memory(entry.entry_id);
            result.pruned += 1;
        }
    }

    // Step 4: Enforce max limit.
    let after_prune = db
        .list_memories(Some(MemoryEntryType::Reflection), 500)
        .await?;

    if after_prune.len() > config.max_reflections as usize {
        // Sort by relevance ascending (lowest first) and prune excess.
        let mut sorted = after_prune;
        sorted.sort_by(|a, b| {
            a.relevance_score
                .partial_cmp(&b.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let excess = sorted.len() - config.max_reflections as usize;
        for entry in sorted.iter().take(excess) {
            let _ = db.inner().delete_memory(entry.entry_id);
            result.pruned += 1;
        }
    }

    let final_count = db
        .list_memories(Some(MemoryEntryType::Reflection), 500)
        .await?;
    result.remaining = final_count.len();

    Ok(result)
}

/// Threshold for triggering automatic consolidation.
const AUTO_CONSOLIDATION_THRESHOLD: u32 = 20;

/// Check if consolidation is needed and run it in the background.
///
/// Triggers when the number of reflections exceeds the threshold.
/// Returns the consolidation result if consolidation was performed, None if skipped.
pub async fn maybe_consolidate(db: &AsyncDatabase) -> Option<ConsolidationResult> {
    // Quick count check — avoid loading all reflections if not needed.
    let count = match db
        .list_memories(Some(MemoryEntryType::Reflection), AUTO_CONSOLIDATION_THRESHOLD + 1)
        .await
    {
        Ok(entries) => entries.len() as u32,
        Err(_) => return None,
    };

    if count <= AUTO_CONSOLIDATION_THRESHOLD {
        return None;
    }

    let config = ConsolidationConfig::default();
    match consolidate(db, &config).await {
        Ok(result) => {
            if result.merged > 0 || result.pruned > 0 {
                tracing::info!(
                    merged = result.merged,
                    pruned = result.pruned,
                    remaining = result.remaining,
                    "Auto-consolidated reflections"
                );
            }
            Some(result)
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to auto-consolidate reflections");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    use uuid::Uuid;

    fn test_db() -> AsyncDatabase {
        AsyncDatabase::new(Arc::new(halcon_storage::Database::open_in_memory().unwrap()))
    }

    fn make_reflection(content: &str, relevance: f64) -> MemoryEntry {
        MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: MemoryEntryType::Reflection,
            content: content.to_string(),
            content_hash: hex::encode(Sha256::digest(content.as_bytes())),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: relevance,
        }
    }

    #[test]
    fn jaccard_identical() {
        let sim = jaccard_similarity("hello world foo", "hello world foo");
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn jaccard_no_overlap() {
        let sim = jaccard_similarity("hello world", "foo bar baz");
        assert!(sim.abs() < 0.001);
    }

    #[test]
    fn jaccard_partial_overlap() {
        // {"hello", "world", "foo"} ∩ {"hello", "world", "bar"} = {"hello", "world"}
        // Union = {"hello", "world", "foo", "bar"} = 4
        // Jaccard = 2/4 = 0.5
        let sim = jaccard_similarity("hello world foo", "hello world bar");
        assert!((sim - 0.5).abs() < 0.001);
    }

    #[test]
    fn jaccard_empty_strings() {
        assert!((jaccard_similarity("", "") - 1.0).abs() < 0.001);
        assert!(jaccard_similarity("hello", "").abs() < 0.001);
    }

    #[test]
    fn cluster_similar_groups() {
        let entries = vec![
            make_reflection("Always check file permissions before writing", 1.0),
            make_reflection("Always check file permissions before reading", 1.0),
            make_reflection("Use thiserror for library error handling", 1.0),
        ];
        let clusters = cluster_similar(&entries, 0.4);
        // Entries 0 and 1 are similar (high Jaccard), entry 2 is different.
        assert_eq!(clusters.len(), 2);
        let has_pair = clusters.iter().any(|c| c.len() == 2);
        let has_single = clusters.iter().any(|c| c.len() == 1);
        assert!(has_pair, "should have a cluster of 2 similar entries");
        assert!(has_single, "should have a singleton cluster");
    }

    #[test]
    fn cluster_all_different() {
        let entries = vec![
            make_reflection("Rust async patterns", 1.0),
            make_reflection("Python web frameworks", 1.0),
            make_reflection("Docker container setup", 1.0),
        ];
        let clusters = cluster_similar(&entries, 0.4);
        assert_eq!(clusters.len(), 3, "all entries should be in separate clusters");
    }

    #[tokio::test]
    async fn consolidate_merges_similar() {
        let db = test_db();
        let r1 = make_reflection("Always check file permissions before writing files", 1.0);
        let r2 = make_reflection("Always check file permissions before reading files", 0.8);
        let r3 = make_reflection("Use thiserror for library error handling in Rust", 1.0);
        db.insert_memory(&r1).await.unwrap();
        db.insert_memory(&r2).await.unwrap();
        db.insert_memory(&r3).await.unwrap();

        let config = ConsolidationConfig {
            similarity_threshold: 0.4,
            min_relevance: 0.1,
            max_reflections: 50,
        };
        let result = consolidate(&db, &config).await.unwrap();
        assert_eq!(result.merged, 1, "one entry should be merged");
        assert_eq!(result.remaining, 2, "2 entries should remain");

        // The keeper should have boosted relevance.
        let remaining = db
            .list_memories(Some(MemoryEntryType::Reflection), 10)
            .await
            .unwrap();
        let keeper = remaining
            .iter()
            .find(|e| e.content.contains("file permissions"))
            .unwrap();
        assert!(
            keeper.relevance_score > 1.0,
            "keeper should have boosted relevance, got {}",
            keeper.relevance_score
        );
    }

    #[tokio::test]
    async fn consolidate_prunes_low_relevance() {
        let db = test_db();
        // Content is deliberately dissimilar to avoid clustering.
        db.insert_memory(&make_reflection(
            "Stale advice about deprecated Python two syntax migration",
            0.1,
        ))
        .await
        .unwrap();
        db.insert_memory(&make_reflection(
            "Always validate Rust lifetimes in generic trait bounds",
            1.5,
        ))
        .await
        .unwrap();

        let config = ConsolidationConfig {
            similarity_threshold: 0.4,
            min_relevance: 0.2,
            max_reflections: 50,
        };
        let result = consolidate(&db, &config).await.unwrap();
        assert_eq!(result.pruned, 1);
        assert_eq!(result.remaining, 1);
    }

    #[tokio::test]
    async fn consolidate_enforces_max_limit() {
        let db = test_db();
        for i in 0..10 {
            let mut entry = make_reflection(&format!("Unique reflection number {i} with distinct content"), 1.0);
            entry.content_hash = format!("hash_max_{i}");
            db.insert_memory(&entry).await.unwrap();
        }

        let config = ConsolidationConfig {
            similarity_threshold: 0.8, // High threshold → no merging.
            min_relevance: 0.0,
            max_reflections: 5,
        };
        let result = consolidate(&db, &config).await.unwrap();
        assert_eq!(result.remaining, 5, "should enforce max of 5");
        assert_eq!(result.pruned, 5, "should prune 5 excess entries");
    }

    #[tokio::test]
    async fn consolidate_empty_db() {
        let db = test_db();
        let config = ConsolidationConfig::default();
        let result = consolidate(&db, &config).await.unwrap();
        assert_eq!(result.merged, 0);
        assert_eq!(result.pruned, 0);
        assert_eq!(result.remaining, 0);
    }

    #[tokio::test]
    async fn consolidate_no_similar_entries() {
        let db = test_db();
        db.insert_memory(&make_reflection("Rust async patterns are complex", 1.0))
            .await
            .unwrap();
        db.insert_memory(&make_reflection("Python web frameworks comparison", 1.0))
            .await
            .unwrap();

        let config = ConsolidationConfig::default();
        let result = consolidate(&db, &config).await.unwrap();
        assert_eq!(result.merged, 0);
        assert_eq!(result.remaining, 2);
    }

    #[tokio::test]
    async fn maybe_consolidate_skips_when_below_threshold() {
        let db = test_db();
        // Insert fewer than threshold (20).
        for i in 0..5 {
            let mut entry = make_reflection(
                &format!("Unique insight number {i} about different topics"),
                1.0,
            );
            entry.content_hash = format!("hash_maybe_{i}");
            db.insert_memory(&entry).await.unwrap();
        }

        // Should not consolidate — below threshold.
        maybe_consolidate(&db).await;

        let remaining = db
            .list_memories(Some(MemoryEntryType::Reflection), 100)
            .await
            .unwrap();
        assert_eq!(remaining.len(), 5, "no consolidation should have occurred");
    }

    #[tokio::test]
    async fn maybe_consolidate_triggers_above_threshold() {
        let db = test_db();
        // Insert more than threshold (20) — use distinct content to avoid merging.
        for i in 0..25 {
            let mut entry = make_reflection(
                &format!("Completely unique reflection {i} with distinct vocabulary word{i}"),
                0.05, // Low relevance — should be pruned.
            );
            entry.content_hash = format!("hash_trigger_{i}");
            db.insert_memory(&entry).await.unwrap();
        }

        // Should trigger consolidation — above threshold.
        maybe_consolidate(&db).await;

        let remaining = db
            .list_memories(Some(MemoryEntryType::Reflection), 100)
            .await
            .unwrap();
        // Default min_relevance is 0.2, all entries have 0.05 → all pruned.
        assert!(
            remaining.len() < 25,
            "consolidation should have pruned some entries, got {}",
            remaining.len()
        );
    }

    #[tokio::test]
    async fn maybe_consolidate_empty_db_is_noop() {
        let db = test_db();
        maybe_consolidate(&db).await;
        // No panic, no errors.
    }
}
