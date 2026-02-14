//! Content-addressed artifact store for structured task outputs.

use std::collections::HashMap;

use chrono::Utc;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use cuervo_core::types::{ArtifactType, TaskArtifact};

/// Content-addressed store for task artifacts.
pub(crate) struct ArtifactStore {
    /// content_hash → artifact.
    artifacts: HashMap<String, TaskArtifact>,
    /// task_id → list of content hashes.
    by_task: HashMap<Uuid, Vec<String>>,
}

impl ArtifactStore {
    pub fn new() -> Self {
        Self {
            artifacts: HashMap::new(),
            by_task: HashMap::new(),
        }
    }

    /// Store an artifact, computing its SHA-256 content hash.
    /// Returns the stored artifact (or existing if deduplicated).
    pub fn store(
        &mut self,
        task_id: Uuid,
        name: String,
        artifact_type: ArtifactType,
        content: &[u8],
        path: Option<String>,
    ) -> TaskArtifact {
        let content_hash = hex::encode(Sha256::digest(content));
        let size_bytes = content.len() as u64;

        // Dedup: if same hash exists, just link to the task.
        if let Some(existing) = self.artifacts.get(&content_hash) {
            self.by_task
                .entry(task_id)
                .or_default()
                .push(content_hash.clone());
            return existing.clone();
        }

        let artifact = TaskArtifact {
            artifact_id: Uuid::new_v4(),
            name,
            artifact_type,
            content_hash: content_hash.clone(),
            size_bytes,
            path,
            metadata: serde_json::Value::Object(serde_json::Map::new()),
            created_at: Utc::now(),
        };

        self.artifacts
            .insert(content_hash.clone(), artifact.clone());
        self.by_task
            .entry(task_id)
            .or_default()
            .push(content_hash);

        artifact
    }

    /// Get an artifact by its content hash.
    pub fn get_by_hash(&self, hash: &str) -> Option<&TaskArtifact> {
        self.artifacts.get(hash)
    }

    /// Get all artifacts for a task.
    pub fn artifacts_for_task(&self, task_id: Uuid) -> Vec<&TaskArtifact> {
        self.by_task
            .get(&task_id)
            .map(|hashes| {
                hashes
                    .iter()
                    .filter_map(|h| self.artifacts.get(h))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether an artifact with the given hash exists.
    pub fn contains(&self, hash: &str) -> bool {
        self.artifacts.contains_key(hash)
    }

    /// Get the most recently created artifact for a task with a given name.
    pub fn latest_version(&self, task_id: Uuid, name: &str) -> Option<&TaskArtifact> {
        self.by_task.get(&task_id).and_then(|hashes| {
            hashes
                .iter()
                .rev() // latest stored last
                .filter_map(|h| self.artifacts.get(h))
                .find(|a| a.name == name)
        })
    }

    /// Total size of all artifacts in bytes.
    pub fn total_size(&self) -> u64 {
        self.artifacts.values().map(|a| a.size_bytes).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_correctness() {
        let mut store = ArtifactStore::new();
        let task_id = Uuid::new_v4();
        let content = b"Hello, world!";

        let artifact = store.store(
            task_id,
            "test.txt".into(),
            ArtifactType::File,
            content,
            None,
        );

        // Verify SHA-256 hash.
        let expected_hash = hex::encode(Sha256::digest(content));
        assert_eq!(artifact.content_hash, expected_hash);
        assert_eq!(artifact.size_bytes, 13);
    }

    #[test]
    fn get_by_hash() {
        let mut store = ArtifactStore::new();
        let task_id = Uuid::new_v4();
        let artifact = store.store(
            task_id,
            "out.json".into(),
            ArtifactType::ToolOutput,
            b"{}",
            None,
        );

        let got = store.get_by_hash(&artifact.content_hash).unwrap();
        assert_eq!(got.name, "out.json");
    }

    #[test]
    fn dedup_same_content() {
        let mut store = ArtifactStore::new();
        let t1 = Uuid::new_v4();
        let t2 = Uuid::new_v4();
        let content = b"same content";

        let a1 = store.store(t1, "a.txt".into(), ArtifactType::File, content, None);
        let a2 = store.store(t2, "b.txt".into(), ArtifactType::File, content, None);

        // Same hash, same artifact_id (deduped).
        assert_eq!(a1.content_hash, a2.content_hash);
        assert_eq!(a1.artifact_id, a2.artifact_id);

        // Both tasks reference it.
        assert_eq!(store.artifacts_for_task(t1).len(), 1);
        assert_eq!(store.artifacts_for_task(t2).len(), 1);

        // Total size counts only once (unique artifacts).
        assert_eq!(store.total_size(), content.len() as u64);
    }

    #[test]
    fn artifacts_for_task() {
        let mut store = ArtifactStore::new();
        let task_id = Uuid::new_v4();

        store.store(task_id, "a.txt".into(), ArtifactType::File, b"aaa", None);
        store.store(task_id, "b.txt".into(), ArtifactType::File, b"bbb", None);

        let artifacts = store.artifacts_for_task(task_id);
        assert_eq!(artifacts.len(), 2);
    }

    #[test]
    fn latest_version() {
        let mut store = ArtifactStore::new();
        let task_id = Uuid::new_v4();

        store.store(task_id, "out.txt".into(), ArtifactType::File, b"v1", None);
        store.store(task_id, "out.txt".into(), ArtifactType::File, b"v2", None);
        store.store(task_id, "other.txt".into(), ArtifactType::File, b"x", None);

        let latest = store.latest_version(task_id, "out.txt").unwrap();
        // Latest should be the second version (different content).
        let v2_hash = hex::encode(Sha256::digest(b"v2"));
        assert_eq!(latest.content_hash, v2_hash);
    }

    #[test]
    fn total_size() {
        let mut store = ArtifactStore::new();
        let t = Uuid::new_v4();

        store.store(t, "a".into(), ArtifactType::File, b"12345", None);
        store.store(t, "b".into(), ArtifactType::File, b"67890", None);

        assert_eq!(store.total_size(), 10);
    }

    #[test]
    fn contains_check() {
        let mut store = ArtifactStore::new();
        let t = Uuid::new_v4();

        let a = store.store(t, "x".into(), ArtifactType::Summary, b"data", None);
        assert!(store.contains(&a.content_hash));
        assert!(!store.contains("nonexistent_hash"));
    }

    #[test]
    fn empty_task_returns_empty() {
        let store = ArtifactStore::new();
        assert!(store.artifacts_for_task(Uuid::new_v4()).is_empty());
        assert!(store.latest_version(Uuid::new_v4(), "any").is_none());
    }
}
